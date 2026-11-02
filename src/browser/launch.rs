//! Stealth launch: flag assembly + self-launched Chrome + WS endpoint parse.
//!
//! Owns the real Chrome child-process lifetime (spawn with stealth flags,
//! expose WS endpoint, guarantee teardown). Page scripting never happens
//! here — the endpoint is a plain string parsed from Chrome stderr
//! (layer gates: JS-read helpers live in `session.rs` only).
//!
//! Lifecycle (Story 2.2): every launch runs sweep → lock → spawn. The child
//! is spawned in its own `setsid` process group; sync `Drop` does
//! block-and-kill (`SIGTERM` → `SIGKILL`, never await) while async `close()`
//! owns kill → wait → unlock. A SIGKILL-parent can only be recovered by the
//! next launch's sweep, which reaps recorded orphan PIDs and reports them.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs2::FileExt;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::browser::{other, stealth::stealth_flags};
use crate::error::Result;

/// Launch order gate: sweep orphans → acquire lock → spawn. Enforced in
/// `StealthLauncher::spawn` and pinned by
/// `lock_order_should_sweep_before_lock_before_spawn_when_launching`.
pub const LAUNCH_ORDER: &[&str] = &["sweep", "lock", "spawn"];

/// Behavior selector (GoF Strategy): visible manual-login vs headless check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    HeadedManualLogin,
    HeadlessStatus,
}

/// Centralized launch parameters (GoF Builder): flags, profile dir, CfT
/// binary path, and timeouts. The CfT resolver (Story 2.1) feeds the binary
/// via [`LaunchConfig::with_cft`]; system-Chrome fallback requires the
/// explicit opt-in flag — never silent.
#[derive(Debug, Clone)]
pub struct LaunchConfig {
    pub mode: LaunchMode,
    pub profile_dir: PathBuf,
    pub chrome_binary: Option<PathBuf>,
    pub allow_system_chrome_fallback: bool,
    pub endpoint_timeout: Duration,
}

impl LaunchConfig {
    pub fn headed_manual_login(profile_dir: PathBuf) -> Self {
        Self {
            mode: LaunchMode::HeadedManualLogin,
            profile_dir,
            chrome_binary: None,
            allow_system_chrome_fallback: false,
            endpoint_timeout: Duration::from_secs(10),
        }
    }

    pub fn headless_status(profile_dir: PathBuf) -> Self {
        Self {
            mode: LaunchMode::HeadlessStatus,
            profile_dir,
            chrome_binary: None,
            allow_system_chrome_fallback: false,
            endpoint_timeout: Duration::from_secs(10),
        }
    }

    pub fn with_chrome_binary(mut self, path: PathBuf) -> Self {
        self.chrome_binary = Some(path);
        self
    }

    /// Wire the resolved CfT binary (Story 2.1 Task 4): fail-fast on skew
    /// happens at the runtime check; a missing cached binary is a hard
    /// error, never a silent system-Chrome fallback.
    pub fn with_cft(mut self, data_dir: &Path, pinned: &crate::browser::PinnedVersion) -> Self {
        self.chrome_binary =
            Some(crate::browser::cft::cft_chrome_binary(data_dir, pinned));
        self
    }

    pub fn with_system_chrome_fallback(mut self, allow: bool) -> Self {
        self.allow_system_chrome_fallback = allow;
        self
    }

    /// Pure flag assembly (no I/O): stealth set + mode/profile wiring.
    pub fn flags(&self) -> Vec<String> {
        stealth_flags(self.mode, &self.profile_dir).iter().map(|f| f.as_str().to_string()).collect()
    }}
/// Parse the WS endpoint from one Chrome stderr line, e.g.
/// `DevTools listening on ws://127.0.0.1:41233/devtools/browser/<id>`.
/// Pure and unit-testable (the 10s deadline lives in `spawn`).
pub fn parse_ws_endpoint(line: &str) -> Option<String> {
    const MARKER: &str = "DevTools listening on ";
    let idx = line.find(MARKER)?;
    let endpoint = line[idx + MARKER.len()..].trim().to_string();
    if endpoint.starts_with("ws://") {
        Some(endpoint)
    } else {
        None
    }
}

const SYSTEM_CHROME_CANDIDATES: &[&str] =
    &["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome"];

fn probe_path(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(bin))
            .find(|p| p.is_file())
    })
}

/// Orphan-PID record file inside the profile dir (written at spawn,
/// removed by `close()`; a SIGKILL-parent leaves it for the next sweep).
fn orphan_pid_file(profile_dir: &Path) -> PathBuf {
    profile_dir.join("orphan_pids")
}

fn pid_alive(pid: u32) -> bool {
    // SAFETY: kill with sig 0 performs error-checking only (0 = alive,
    // ESRCH = gone); no signal is delivered.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Trust gate for orphan-PID records: the file is untrusted input (a planted
/// `1` or a recycled PID must never drive a whole-group kill). Accept only a
/// live group leader of its own group — never pid ≤ 1, never our own pid,
/// never pgid 0/1 — so the negative-kill below can only hit the setsid group
/// the prior run created.
fn is_safe_orphan_target(pid: u32) -> bool {
    if pid <= 1 || pid == std::process::id() {
        return false;
    }
    // SAFETY: getpgid with a positive pid only reads kernel state; it sends
    // no signal and touches no memory.
    let pgid = unsafe { libc::getpgid(pid as libc::pid_t) };
    if pgid <= 1 {
        return false;
    }
    pgid == pid as libc::pid_t
}

/// Block-and-kill a `setsid` process group: `SIGTERM`, brief grace window,
/// then `SIGKILL`. Synchronous — safe to call from `Drop` (never awaits).
/// Callers pass only PIDs this launcher spawned (or sweep-validated as own
/// group leaders); never call with an untrusted PID.
pub fn kill_process_group(pid: u32) -> Vec<u32> {
    let pgid = -(pid as libc::pid_t);
    // SAFETY: pgid is our own setsid group (spawn always setsid's the child,
    // sweep only forwards validated group leaders); the negative pid targets
    // that group alone.
    unsafe {
        libc::kill(pgid, libc::SIGTERM);
    }
    // Grace window without async: poll liveness up to ~500ms.
    for _ in 0..50 {
        if !pid_alive(pid) {
            return vec![pid];
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // SAFETY: same own-group pgid as the SIGTERM above; SIGKILL follows only
    // after the grace window confirmed the group still lives.
    unsafe {
        libc::kill(pgid, libc::SIGKILL);
    }
    vec![pid]
}

/// Async wrapper around [`kill_process_group`] for use on the executor:
/// the sync SIGTERM→SIGKILL grace loop parks up to ~500ms in
/// `std::thread::sleep`, so it must run in `spawn_blocking`, never inline
/// in async code.
async fn kill_group_async(pid: u32) -> Vec<u32> {
    tokio::task::spawn_blocking(move || kill_process_group(pid))
        .await
        .unwrap_or_default()
}

/// Orphan sweep helper: runs BEFORE the lock on every launch. Reaps PIDs
/// recorded by a prior run that never reached `close()` (panic,
/// SIGKILL-parent) and reports each reaped PID. Never deletes the profile.
pub fn sweep_orphans(profile_dir: &Path) -> Vec<u32> {
    let file = orphan_pid_file(profile_dir);
    let content = match std::fs::read_to_string(&file) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut reaped = Vec::new();
    for line in content.lines() {
        if let Ok(pid) = line.trim().parse::<u32>() {
            if !is_safe_orphan_target(pid) {
                // Untrusted record (pid ≤ 1, self, non-leader, or gone):
                // log + unlink, never kill.
                eprintln!(
                    "sweep: ignoring suspect orphan PID {pid} (not a live group leader) — record removed without killing"
                );
                continue;
            }
            if pid_alive(pid) {
                reaped.extend(kill_process_group(pid));
            } else {
                // Already gone — still report it as reaped-by-sweep so the
                // next-launch copy can name the recovered PID.
                reaped.push(pid);
            }
        }
    }
    let _ = std::fs::remove_file(&file);
    reaped
}

/// Profile lock: an `fs2` lockfile plus respect for Chrome's `SingletonLock`.
/// Status aborts with `profile busy — close the login window first` (exit 3
/// upstream) without touching the profile. The lock releases on drop.
#[derive(Debug)]
pub struct ProfileLock {
    _file: std::fs::File,
    _profile_dir: PathBuf,
}

impl ProfileLock {
    pub fn lockfile_path(profile_dir: &Path) -> PathBuf {
        profile_dir.join("sleeper-browser.lock")
    }

    /// Acquire without sweeping (the caller runs `sweep_orphans` first to
    /// honor sweep → lock → spawn).
    pub fn acquire(profile_dir: &Path) -> Result<Self> {
        if profile_dir.join("SingletonLock").exists() {
            return Err(other(format!(
                "profile busy — close the login window first (profile {} is locked by a live Chrome window)",
                profile_dir.display()
            )));
        }
        std::fs::create_dir_all(profile_dir).map_err(other)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(Self::lockfile_path(profile_dir))
            .map_err(other)?;
        file.try_lock_exclusive().map_err(|_| {
            other(format!(
                "profile busy — close the login window first (profile {} is locked by another run)",
                profile_dir.display()
            ))
        })?;
        Ok(Self { _file: file, _profile_dir: profile_dir.to_path_buf() })
    }

    /// Explicit unlock (drop also unlocks): removes nothing but the lock.
    pub fn unlock(self) {
        drop(self);
    }
}

/// The 12-state error × cleanup table (ux.md §2 + Story 2.2 Task 3).
/// Each row names its detection point, triage copy, and cleanup action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupState {
    ChromeMissing,
    CftSkew,
    FlagSetMismatch,
    ProfileLocked,
    TimeoutNoLogin,
    OrphanCleanupFailure,
    CheckAmbiguous,
    NoDisplay,
    PanicMidSession,
    SigkillParent,
    PartialSpawnAfterLock,
    LaunchFailAfterLock,
}

impl CleanupState {
    pub const ALL_12: [CleanupState; 12] = [
        CleanupState::ChromeMissing,
        CleanupState::CftSkew,
        CleanupState::FlagSetMismatch,
        CleanupState::ProfileLocked,
        CleanupState::TimeoutNoLogin,
        CleanupState::OrphanCleanupFailure,
        CleanupState::CheckAmbiguous,
        CleanupState::NoDisplay,
        CleanupState::PanicMidSession,
        CleanupState::SigkillParent,
        CleanupState::PartialSpawnAfterLock,
        CleanupState::LaunchFailAfterLock,
    ];

    /// Expected cleanup action for the row (the matrix test pins all 12).
    pub fn cleanup_action(&self) -> &'static str {
        match self {
            CleanupState::ChromeMissing => "nothing-to-clean-no-spawn",
            CleanupState::CftSkew => "kill-group-before-exit",
            CleanupState::FlagSetMismatch => "kill-group-profile-untouched",
            CleanupState::ProfileLocked => "reap-orphans-report-pids-profile-untouched",
            CleanupState::TimeoutNoLogin => "close-headed-browser",
            CleanupState::OrphanCleanupFailure => "log-reaped-vs-surviving",
            CleanupState::CheckAmbiguous => "close-headless-browser",
            CleanupState::NoDisplay => "exit-before-poll",
            CleanupState::PanicMidSession => "drop-guard-block-and-kill",
            CleanupState::SigkillParent => "next-launch-sweep-reaps",
            CleanupState::PartialSpawnAfterLock => "release-lock-kill-half-spawn",
            CleanupState::LaunchFailAfterLock => "release-lock-kill-group-log-pids",
        }
    }

    /// Triage line for the row: names what happened + the fix + the exact
    /// re-run command. Every line carries a wait / re-run / update verb.
    pub fn triage_line(&self) -> String {
        match self {
            CleanupState::ChromeMissing => "Chrome not found at <path>. Install Chrome stable or set CHROME_PATH — then re-run `sleeper browser login`.".into(),
            CleanupState::CftSkew => "CfT Chrome runtime vs pinned — version skew. Please re-run with update: re-run `sleeper browser login` to re-resolve the pin; if it persists, update the pin.".into(),
            CleanupState::FlagSetMismatch => "Stealth flag set mismatch — update sleeper and re-run `sleeper browser login`; if it persists, update the pin to the last-known-good CfT Chrome.".into(),
            CleanupState::ProfileLocked => "profile busy — close the login window first, then re-run `sleeper browser login`.".into(),
            CleanupState::TimeoutNoLogin => "Still not logged in after 10m — finish logging in in the browser window, then re-run `sleeper browser login`.".into(),
            CleanupState::OrphanCleanupFailure => "Could not stop browser PID <pid> — run `kill -9 <pid>` then re-run `sleeper browser login`.".into(),
            CleanupState::CheckAmbiguous => "Login check unknown — re-run `sleeper browser status`; re-login only if persistent.".into(),
            CleanupState::NoDisplay => "No display detected — `sleeper browser login` needs a visible window. Run on a machine with a display (or SSH -X) and retry.".into(),
            CleanupState::PanicMidSession => "Previous run ended unexpectedly — cleaned up browser PID <pid>. Continuing with launch… (wait for the fresh verdict; re-run `sleeper browser status` if it fails).".into(),
            CleanupState::SigkillParent => "Cleaned up orphan browser PID <pid> from killed run. (wait for the fresh verdict; re-run `sleeper browser login` if it fails).".into(),
            CleanupState::PartialSpawnAfterLock => "Chrome launch failed (<detail>) — profile lock released. Re-run `sleeper browser login`.".into(),
            CleanupState::LaunchFailAfterLock => "Launch failed (<step>: <detail>) — profile lock released, browser stopped. Re-run `sleeper browser login`.".into(),
        }
    }
}

pub struct StealthLauncher;

impl StealthLauncher {
    /// Resolve the Chrome binary: explicit CfT path first, explicit
    /// system-Chrome opt-in second, hard error otherwise (never silent).
    pub fn resolve_binary(config: &LaunchConfig) -> Result<PathBuf> {
        if let Some(path) = &config.chrome_binary {
            if path.is_file() {
                return Ok(path.clone());
            }
            return Err(other(format!(
                "CfT Chrome binary missing at {} — re-run `sleeper browser login` to re-resolve the pin with update, or pass --chrome-binary/--system-chrome",
                path.display()
            )));
        }
        if config.allow_system_chrome_fallback {
            if let Some(found) = SYSTEM_CHROME_CANDIDATES.iter().filter_map(|b| probe_path(b)).next() {
                return Ok(found);
            }
            return Err(other(
                "system Chrome fallback enabled but no chrome/chromium binary found on PATH — install Chrome stable, then re-run `sleeper browser login`",
            ));
        }
        Err(other(
            "no CfT Chrome binary configured and system-Chrome fallback not enabled — re-run `sleeper browser login` to resolve the CfT pin with update, or pass --chrome-binary/--system-chrome",
        ))
    }

    /// Spawn Chrome with stealth flags in a `setsid` process group, parse
    /// the WS endpoint from stderr with a 10s deadline, and return the
    /// endpoint plus a teardown guard. Order: sweep → lock → spawn.
    /// Fails fast on a missing binary — before any port bind or spawn.
    ///
    /// Observability boundary (plan.md Layer 3): span covers resolve + spawn
    /// + the async stderr-parse loop below; no behavior change. Uses the
    /// `#[instrument]` attribute rather than a manually `.entered()` span —
    /// an `EnteredSpan` guard held across an `.await` is not `Send`, which
    /// broke every caller that needs a `Send` future (the MCP tool
    /// dispatcher boxes tool-call futures as `dyn Future + Send`).
    #[tracing::instrument(
        name = "browser.launch",
        skip(config),
        fields(mode = ?config.mode, profile = %config.profile_dir.display(), cft = ?config.chrome_binary)
    )]
    pub async fn spawn(config: &LaunchConfig) -> Result<LaunchedBrowser> {
        let binary = Self::resolve_binary(config)?;
        let profile_dir = config.profile_dir.clone();
        let flags = config.flags();
        let endpoint_timeout = config.endpoint_timeout;
        // Blocking prologue off the executor: sweep (before lock) → lock →
        // spawn → PID record. All std::fs / fs2 / kill / thread::sleep work
        // stays here; the async stderr-read loop below never blocks.
        // Order: sweep → lock → spawn (partial-spawn failures release the
        // lock — `lock` drops on every early return below).
        let (lock, mut child, pid, profile_dir) = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&profile_dir).map_err(other)?;
            // 1. sweep (before lock): reap orphans from a killed prior run.
            let swept = sweep_orphans(&profile_dir);
            // 2. lock: abort without touching the profile when busy.
            let lock = ProfileLock::acquire(&profile_dir).map_err(|e| {
                if !swept.is_empty() {
                    other(format!("{e} (cleaned up orphan PIDs {swept:?} automatically)"))
                } else {
                    e
                }
            })?;
            // 3. spawn in a setsid process group.
            let mut std_cmd = std::process::Command::new(&binary);
            std_cmd.args(&flags);
            std_cmd.stderr(std::process::Stdio::piped());
            std_cmd.stdout(std::process::Stdio::null());
            std_cmd.stdin(std::process::Stdio::null());
            // SAFETY: pre_exec runs post-fork pre-exec; setsid is
            // async-signal-safe and touches no locks or heap state.
            unsafe {
                std_cmd.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
            let child = tokio::process::Command::from(std_cmd).spawn().map_err(|e| {
                other(format!(
                    "Chrome launch failed ({e}) — profile lock released. Re-run `sleeper browser login`."
                ))
            })?;
            let pid = child.id().ok_or_else(|| {
                other("Chrome launch failed (no PID) — profile lock released. Re-run `sleeper browser login`.")
            })?;
            // Record the PID so a SIGKILL-parent is recoverable by next sweep.
            let _ = std::fs::write(orphan_pid_file(&profile_dir), format!("{pid}\n"));
            Ok::<_, crate::error::Error>((lock, child, pid, profile_dir))
        })
        .await??;
        let stderr = child.stderr.take().ok_or_else(|| other("failed to pipe Chrome stderr"))?;
        let mut lines = BufReader::new(stderr).lines();
        let deadline = tokio::time::Instant::now() + endpoint_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                kill_group_async(pid).await;
                let _ = child.kill().await;
                drop(lock);
                return Err(other(format!(
                    "timed out after {}s waiting for Chrome DevTools endpoint — profile lock released. Re-run `sleeper browser login`.",
                    endpoint_timeout.as_secs()
                )));
            }
            match tokio::time::timeout(remaining, lines.next_line()).await.map_err(other)? {
                Ok(Some(line)) => {
                    if let Some(endpoint) = parse_ws_endpoint(&line) {
                        tracing::info!("browser.launch endpoint parsed");
                        return Ok(LaunchedBrowser {
                            endpoint,
                            child: Some(child),
                            pid,
                            profile_dir,
                            lock: Some(lock),
                        });
                    }
                }
                Ok(None) => {
                    kill_group_async(pid).await;
                    let _ = child.kill().await;
                    drop(lock);
                    return Err(other("Chrome exited before printing a DevTools endpoint — profile lock released. Re-run `sleeper browser login`."));
                }
                Err(_) => {
                    kill_group_async(pid).await;
                    let _ = child.kill().await;
                    drop(lock);
                    return Err(other("failed reading Chrome stderr while waiting for endpoint — profile lock released. Re-run `sleeper browser login`."));
                }
            }
        }
    }
}

/// Spawned Chrome guard: explicit async `close` owns kill → wait → unlock;
/// sync `Drop` does block-and-kill on the process group (never await) so
/// timeout/early-return/panic paths cannot orphan.
pub struct LaunchedBrowser {
    pub endpoint: String,
    child: Option<tokio::process::Child>,
    pid: u32,
    profile_dir: PathBuf,
    lock: Option<ProfileLock>,
}

impl LaunchedBrowser {
    pub async fn close(mut self) -> Result<()> {
        // Kill off the executor (sync SIGTERM→SIGKILL grace loop), then
        // remove the PID record with async fs. Drop keeps its own sync kill
        // (sync context — never await there).
        let pid = self.pid;
        kill_group_async(pid).await;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        self.lock.take();
        let _ = tokio::fs::remove_file(orphan_pid_file(&self.profile_dir)).await;
        Ok(())
    }
}

impl Drop for LaunchedBrowser {
    fn drop(&mut self) {
        // Block-and-kill the group synchronously; never await here.
        kill_process_group(self.pid);
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        kill_process_group, parse_ws_endpoint, sweep_orphans, CleanupState, LaunchConfig,
        ProfileLock, StealthLauncher, LAUNCH_ORDER,
    };
    use std::os::unix::process::CommandExt;
    use std::path::PathBuf;

    #[test]
    fn launch_config_should_assemble_headed_flags_when_headed_manual_login() {
        let config =
            LaunchConfig::headed_manual_login(PathBuf::from("/tmp/fake-profile"));
        let flags = config.flags();
        assert!(
            flags.iter().any(|f| f == "--remote-debugging-port=0"),
            "headed flags must carry --remote-debugging-port=0, got {flags:?}"
        );
        assert!(
            !flags.iter().any(|f| f.contains("--headless")),
            "headed login must stay visible, got {flags:?}"
        );
        assert!(
            !flags.iter().any(|f| f.to_lowercase().contains("chromedriver")),
            "self-launch: no chromedriver arg allowed, got {flags:?}"
        );
        assert!(
            flags.iter().any(|f| f.contains("fake-profile")),
            "profile dir must be wired into flags, got {flags:?}"
        );
    }

    #[tokio::test]
    async fn spawn_should_fail_fast_when_cft_binary_missing() {
        let config = LaunchConfig::headed_manual_login(PathBuf::from("/tmp/fake-profile"))
            .with_chrome_binary(PathBuf::from("/nonexistent/cft-126/chrome"));
        let err = match StealthLauncher::spawn(&config).await {
            Ok(_) => panic!("missing binary must Err"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("missing"),
            "fail-fast message must name the missing binary, got {err}"
        );
        // Returns before any spawn/port bind by construction (resolve_binary
        // runs first); endpoint parser sanity:
        assert_eq!(
            parse_ws_endpoint(
                "DevTools listening on ws://127.0.0.1:41233/devtools/browser/abc"
            )
            .as_deref(),
            Some("ws://127.0.0.1:41233/devtools/browser/abc")
        );
        assert_eq!(parse_ws_endpoint("[noise] unrelated line"), None);
    }

    #[test]
    fn lock_order_should_sweep_before_lock_before_spawn_when_launching() {
        assert_eq!(
            LAUNCH_ORDER,
            &["sweep", "lock", "spawn"],
            "launch must run sweep → lock → spawn so a SIGKILL-parent is recoverable"
        );
    }

    #[test]
    fn drop_guard_should_own_kill_when_close_never_runs() {
        // Hermetic: spawn a real setsid sleep, drop the guard WITHOUT
        // close(), and prove the group is reaped (timeout-Err path shape).
        let mut std_cmd = std::process::Command::new("sleep");
        std_cmd.arg("60");
        // SAFETY: test-only setsid in pre_exec (post-fork, async-signal-safe).
        unsafe {
            std_cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = std_cmd.spawn().expect("sleep must spawn");
        let pid = child.id();
        // Simulate the Drop path: block-and-kill owns teardown, never await.
        let reaped = kill_process_group(pid);
        assert_eq!(reaped, vec![pid], "Drop path must own the kill");
        // Reap the zombie (we are the parent) so liveness below is honest.
        let _ = child.wait();
        // SAFETY: kill with sig 0 is an error-checking liveness read only.
        let alive = unsafe { libc::kill(pid as libc::pid_t, 0) == 0 };
        assert!(!alive, "guard must leave no surviving process behind");
    }

    #[test]
    fn profile_lock_should_acquire_when_free() {
        let dir = std::env::temp_dir().join(format!("sleeper-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let _lock = ProfileLock::acquire(&dir).expect("free profile must lock");
            // Second acquire while held must report busy, never corrupt.
            let err = ProfileLock::acquire(&dir).expect_err("held lock must Err");
            assert!(
                err.to_string().contains("profile busy — close the login window first"),
                "lock-busy copy must match the status contract, got {err}"
            );
        }
        // Released on drop: re-acquire works (no poisoned lock).
        let _lock = ProfileLock::acquire(&dir).expect("released lock must re-acquire");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_matrix_should_cover_all_12_states_when_table_runs() {
        assert_eq!(CleanupState::ALL_12.len(), 12, "the table must hold all 12 states");
        let mut seen = std::collections::HashSet::new();
        for state in CleanupState::ALL_12 {
            let action = state.cleanup_action();
            assert!(!action.is_empty(), "{state:?} must name a cleanup action");
            assert!(seen.insert(action), "each row needs a distinct cleanup action");
            let triage = state.triage_line();
            let lower = triage.to_lowercase();
            assert!(
                lower.contains("wait")
                    || lower.contains("re-run")
                    || lower.contains("retry")
                    || lower.contains("update")
                    || lower.contains("kill -9"),
                "{state:?} triage must carry a wait/re-run/update verb, got {triage}"
            );
        }
    }

    #[test]
    fn next_launch_sweep_should_reap_when_sigkill_parent_killed_prior_run() {
        // Simulate a SIGKILL-parent: a live setsid sleep whose PID record
        // survives (no close() ever ran), then the next launch sweeps.
        let dir =
            std::env::temp_dir().join(format!("sleeper-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("sweep dir");
        let mut std_cmd = std::process::Command::new("sleep");
        std_cmd.arg("60");
        // SAFETY: test-only setsid in pre_exec (post-fork, async-signal-safe).
        unsafe {
            std_cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = std_cmd.spawn().expect("sleep must spawn");
        let pid = child.id();
        std::fs::write(dir.join("orphan_pids"), format!("{pid}\n")).expect("pid record");
        let reaped = sweep_orphans(&dir);
        assert!(
            reaped.contains(&pid),
            "next-launch sweep must reap the orphaned PID, got {reaped:?}"
        );
        assert!(
            !dir.join("orphan_pids").exists(),
            "sweep must consume the PID record (no poisoned state)"
        );
        // Reap the zombie (we are the parent) so liveness below is honest.
        let _ = child.wait();
        assert!(
            // SAFETY: kill with sig 0 is an error-checking liveness read only.
            unsafe { libc::kill(pid as libc::pid_t, 0) != 0 },
            "swept orphan must not survive"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_should_ignore_when_pid_record_untrusted() {
        // Planted pid 1 and our own pid must never drive a group kill: the
        // sweep logs + unlinks the record and reaps nothing.
        let dir = std::env::temp_dir()
            .join(format!("sleeper-sweep-untrusted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("sweep dir");
        let mine = std::process::id();
        std::fs::write(dir.join("orphan_pids"), format!("1\n{mine}\n")).expect("pid record");
        let reaped = sweep_orphans(&dir);
        assert!(
            reaped.is_empty(),
            "untrusted PIDs (1, self) must never be killed, got {reaped:?}"
        );
        assert!(
            !dir.join("orphan_pids").exists(),
            "sweep must consume the suspect record (no poisoned state)"
        );
        // We are obviously still alive — the sweep must not have signalled us.
        assert!(
            super::pid_alive(mine),
            "sweep must leave the current process untouched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
