//! Login flow (manual login + poll) plus the shared launch/triage plumbing
//! (`launch_session`, `triage_error`, `debug_detail`, `launch_failure_exit`,
//! `launch_failure_error`, `launch_failure_json_error`) that `status.rs`,
//! `doctor.rs`, `actions.rs`, and `pending.rs` all reuse for their own flows.
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;

use crate::config::Config;
use crate::error::Result;

/// Verbatim fast-path verdict (ux.md Surface A). Exact copy guarded by
/// `login_fast_path_should_print_exact_verdict_when_already_authenticated`.
pub fn already_logged_in_message() -> String {
    "Already logged in — session persisted from a previous run.".to_string()
}

/// Transport-error triage: distinct copy from signal-split Unknown.
pub fn check_failed_triage(detail: &str) -> String {
    format!("Login check failed ({detail}) — re-run `sleeper browser status`.")
}

/// Triage-line contract (ux.md Surface F): the triage line always comes
/// FIRST; raw CDP/anyhow detail follows `—` (or hides behind
/// `SLEEPER_BROWSER_DEBUG`). Never a bare stack on stdout.
pub fn triage_error(triage: &str, raw_detail: &str) -> String {
    if raw_detail.is_empty() {
        triage.to_string()
    } else {
        format!("{triage} — {raw_detail}")
    }
}

/// `9m45s` countdown formatting for the login poll spinner.
pub fn format_countdown(secs: u64) -> String {
    format!("{}m{:02}s", secs / 60, secs % 60)
}

/// One spinner tick (stderr only): elapsed + remaining, updated per 5s poll.
pub fn login_poll_tick(elapsed_secs: u64, total_secs: u64) -> String {
    let left = total_secs.saturating_sub(elapsed_secs);
    format!(
        "waiting for manual login… ({} elapsed, {} left)",
        format_countdown(elapsed_secs),
        format_countdown(left)
    )
}

/// Timeout verdict (ux.md §2): directs to `login`, never `status`.
pub fn login_timeout_copy() -> String {
    "Still not logged in after 10m — finish logging in in the browser window, then re-run `sleeper browser login`.".to_string()
}

/// Challenge-loop tool news (plan Story 2.4 Task 6, verbatim framing):
/// recurrence is tool news, never operator blame.
pub fn challenge_loop_copy(polls: u64) -> String {
    format!(
        "Login page is still showing a challenge after {polls} polls. This is unexpected with the stealth backend — solve it once by hand if you can; if it loops, file the CfT Chrome pinned/runtime versions from `sleeper browser status --debug`."
    )
}

/// Ctrl-C copy: exit 130 after closing the browser.
pub fn login_cancelled_copy() -> String {
    "login wait cancelled — closing browser…".to_string()
}

/// Parse `Google Chrome 126.0.6478.126` → `126.0.6478.126`.
/// Thin seam-compat wrapper over [`crate::browser::cft::parse_chrome_version`];
/// the implementation lives in the browser layer, the CLI only calls it.
pub fn parse_chrome_version(output: &str) -> Option<String> {
    crate::browser::cft::parse_chrome_version(output)
}

/// Best-effort runtime version of a Chrome binary (`unknown` when
/// undetectable — never omitted from `--json`).
/// Thin seam-compat wrapper over [`crate::browser::cft::runtime_version`].
pub fn chrome_runtime_version(binary: &std::path::Path) -> String {
    crate::browser::cft::runtime_version(binary)
}

/// Launch parameters for [`launch_session`]. `headed`/`system_chrome` are
/// independent toggles, not a single enum-shaped choice, and most call sites
/// (every DOM-action command) pass the same headless/no-override literals —
/// bundling them means those call sites say [`LaunchOptions::headless`]
/// instead of repeating `false, false, None`.
pub(super) struct LaunchOptions {
    pub(super) headed: bool,
    pub(super) system_chrome: bool,
    pub(super) chrome_binary: Option<PathBuf>,
}

impl LaunchOptions {
    /// The common case: every status/doctor probe and DOM-action command
    /// launches headless with no explicit Chrome override.
    pub(super) fn headless() -> Self {
        Self { headed: false, system_chrome: false, chrome_binary: None }
    }
}

/// Downloads the pinned CfT Chrome on first use. Root-cause fix (2026-11-01):
/// `resolve()` (Story 2.1 Task 2 — download → sha256 → unzip → cache) had
/// no caller anywhere outside an operator-gated test that required a human
/// to paste a sha256 via `SLEEPER_CFT_SHA256` by hand — so `sleeper browser
/// login` on a fresh machine (no pre-existing `<data_dir>/cft/`) could only
/// ever fail with "CfT Chrome binary missing ... re-run `sleeper browser
/// login`", which re-running did nothing to fix (same missing path, same
/// error, forever). Resolves against the trust-on-first-use
/// `PINNED_CHROME_SHA256` constant (see its doc comment — the CfT dashboard
/// was found to publish no per-download checksum at all, for any version),
/// closing that loop.
async fn ensure_cft_downloaded(data_dir: &std::path::Path, pinned: &crate::browser::PinnedVersion) -> Result<()> {
    use crate::browser::stealth::PINNED_CHROME_SHA256;
    if crate::browser::cft::cft_chrome_binary(data_dir, pinned).is_file() {
        return Ok(());
    }
    crate::browser::cft::resolve(data_dir, pinned, PINNED_CHROME_SHA256).await?;
    Ok(())
}

/// Launch through the seam only: `StealthLauncher::spawn` + `connect`.
/// Returns the boxed session plus the process guard; callers close both.
/// CfT wiring (Story 2.1 Task 4): without an explicit binary/flag the
/// resolved CfT path is used; a missing cache entry triggers
/// [`ensure_cft_downloaded`] (never a silent system-Chrome fallback).
pub(super) async fn launch_session(
    config: &Config,
    opts: LaunchOptions,
) -> Result<(Box<dyn crate::browser::BrowserSession>, crate::browser::LaunchedBrowser)> {
    use crate::browser::{BrowserSession, LaunchConfig, PinnedVersion, SleeperBrowser, StealthLauncher};
    let LaunchOptions { headed, system_chrome, chrome_binary } = opts;
    let profile_dir = config.chrome_profile_dir();
    let mut cfg = if headed {
        LaunchConfig::headed_manual_login(profile_dir)
    } else {
        LaunchConfig::headless_status(profile_dir)
    };
    cfg = cfg.with_system_chrome_fallback(system_chrome);
    if let Some(path) = chrome_binary {
        cfg = cfg.with_chrome_binary(path);
    } else if !system_chrome {
        let pinned = PinnedVersion::pinned();
        ensure_cft_downloaded(&config.data_dir, &pinned).await?;
        cfg = cfg.with_cft(&config.data_dir, &pinned);
    }
    let launched = StealthLauncher::spawn(&cfg).await?;
    let session: Box<dyn BrowserSession> =
        Box::new(SleeperBrowser::connect(&launched.endpoint).await?);
    Ok((session, launched))
}

pub(super) async fn cmd_browser_login(
    config: &Config,
    system_chrome: bool,
    chrome_binary: Option<PathBuf>,
) -> Result<()> {
    let (mut session, launched) =
        match launch_session(config, LaunchOptions { headed: true, system_chrome, chrome_binary }).await {
            Ok(pair) => pair,
            Err(e) => launch_failure_exit(&e),
        };
    // AC4 fast path: check is_logged_in() before starting the poll.
    if session.is_logged_in().await? {
        println!("{}", style(already_logged_in_message()).green());
        session.close().await?;
        return launched.close().await;
    }

    eprintln!("Log into Sleeper by hand in the window that just opened — including any CAPTCHA. This step isn't automated, on purpose; waiting up to 10 minutes…");
    // LoginPoll: indicatif spinner + countdown on stderr only (Surface B);
    // 5s ticks, 600s bound, Ctrl-C → exit 130 with copy.
    const TOTAL_SECS: u64 = 600;
    // Observability boundary (plan.md Layer 3): one span + start / timeout
    // events; per-tick progress stays on the spinner/stderr.
    let poll_span = tracing::info_span!("login.poll", timeout_s = TOTAL_SECS);
    let _poll_enter = poll_span.enter();
    tracing::info!("login.poll start");
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.dim} {msg}")
            .unwrap(),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(120));
    let start = tokio::time::Instant::now();
    let mut polls: u64 = 0;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                pb.abandon_with_message("login wait stopped");
                eprintln!("{}", style(login_cancelled_copy()).red());
                session.close().await?;
                launched.close().await?;
                std::process::exit(130);
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
        }
        polls += 1;
        let elapsed = start.elapsed().as_secs().min(TOTAL_SECS);
        pb.set_message(login_poll_tick(elapsed, TOTAL_SECS));
        if polls % 60 == 0 {
            // Tool-news framing (never operator blame) on long waits.
            eprintln!("{}", style(challenge_loop_copy(polls)).dim());
        }
        // No `goto` here (see `is_logged_in_here` doc comment) — this tab
        // is shared with the human actively logging in by hand; reloading
        // it every tick would knock them out of the flow mid-CAPTCHA or
        // mid-passkey.
        if session.is_logged_in_here().await? {
            pb.finish_with_message("login detected");
            println!("{}", style("Logged in. Session persisted for future runs.").green());
            session.close().await?;
            return launched.close().await;
        }
        if elapsed >= TOTAL_SECS {
            pb.abandon_with_message("still waiting stopped");
            tracing::info!("login.poll timeout");
            println!("{}", style(login_timeout_copy()).yellow());
            session.close().await?;
            launched.close().await?;
            std::process::exit(1);
        }
    }
}

/// Raw detail is stderr/debug-gated: shown only when `SLEEPER_BROWSER_DEBUG`
/// is set, so the triage line always leads.
pub(super) fn debug_detail(detail: &str) -> String {
    if std::env::var("SLEEPER_BROWSER_DEBUG").is_ok() {
        detail.to_string()
    } else {
        String::new()
    }
}

/// Launch-failure variant of [`launch_failure_exit`] that returns an `Err`
/// instead of exiting the process — the shared action cores (`actions.rs`,
/// `pending.rs`) run inside a long-lived MCP server, which a launch failure
/// must not take down.
pub(super) fn launch_failure_error(e: &crate::error::Error) -> crate::error::Error {
    let msg = e.to_string();
    if msg.contains("version skew") {
        return crate::browser::other(triage_error(&msg, &debug_detail(&msg)));
    }
    crate::browser::other(triage_error(&check_failed_triage("browser launch"), &debug_detail(&msg)))
}

/// Shared CLI-wrapper tail for the write/read action commands (claim,
/// lineup-set, pending-waivers): a dead session prints the yellow Surface C
/// hint, anything else prints the error's `Display` (already the full
/// triage line) in red. Both branches exit 1.
pub(super) fn exit_on_action_error(e: crate::error::Error) -> ! {
    use crate::browser::session::DEAD_SESSION_HINT;
    if e.to_string() == DEAD_SESSION_HINT {
        eprintln!("{}", style(DEAD_SESSION_HINT).yellow());
    } else {
        eprintln!("{}", style(e.to_string()).red());
    }
    std::process::exit(1);
}

/// Launch/transport failure exit (contract: 1 = logged-out, so a failed
/// launch must never exit 1 via `?`). Prints the triage line FIRST to stderr
/// (raw detail behind `SLEEPER_BROWSER_DEBUG`) and exits 4 — or 2 when the
/// error proves version skew.
pub(super) fn launch_failure_exit(e: &crate::error::Error) -> ! {
    let msg = e.to_string();
    if msg.contains("version skew") {
        eprintln!("{}", style(triage_error(&msg, &debug_detail(&msg))).red());
        std::process::exit(2);
    }
    let triage = check_failed_triage("browser launch");
    eprintln!("{}", style(triage_error(&triage, &debug_detail(&msg))).red());
    std::process::exit(4);
}

/// Short JSON-safe error for the launch-failure path under `--json`
/// (ux.md Surface D + Story 2.4 AC2): skew errors carry their own triage,
/// everything else carries the `browser launch` triage. Properly escaped
/// downstream via `status_json` (`serde_json::json!`), never a bare stack.
pub fn launch_failure_json_error(e: &crate::error::Error) -> String {
    let msg = e.to_string();
    if msg.contains("version skew") {
        msg
    } else {
        check_failed_triage("browser launch")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        already_logged_in_message, format_countdown, login_cancelled_copy, login_poll_tick,
        login_timeout_copy, parse_chrome_version, triage_error,
    };
    use crate::browser::{BrowserSession, FakeSession};

    /// CLI-level seam test: the login fast path resolves through
    /// `Box<dyn BrowserSession>` with no Chrome installed.
    #[tokio::test]
    async fn login_fast_path_should_print_exact_verdict_when_already_authenticated() {
        let session: Box<dyn BrowserSession> = Box::new(FakeSession::logged_in());
        assert!(
            session.is_logged_in().await.expect("seam is_logged_in"),
            "fast path triggers only when the seam reports logged-in"
        );
        assert_eq!(
            already_logged_in_message(),
            "Already logged in — session persisted from a previous run."
        );
    }

    #[test]
    fn login_poll_should_render_countdown_when_waiting() {
        // 15s into a 600s poll: 585s = 9m45s left (stderr shape; stdout untouched).
        let tick = login_poll_tick(15, 600);
        assert!(tick.contains("9m45s"), "countdown must show remaining, got {tick}");
        assert!(tick.contains("elapsed"), "got {tick}");
        assert_eq!(format_countdown(151), "2m31s");
        assert_eq!(format_countdown(0), "0m00s");
    }

    #[test]
    fn timeout_copy_should_direct_to_login_when_poll_expires() {
        let copy = login_timeout_copy();
        assert!(
            copy.contains("sleeper browser login"),
            "timeout must direct to login, got {copy}"
        );
        assert!(
            !copy.contains("browser status"),
            "timeout must never direct to status, got {copy}"
        );
        // Verbatim durability promise preserved.
        assert!(copy.contains("10m") || copy.contains("10 minutes"));
        // Ctrl-C copy + exit contract live beside the timeout copy.
        assert_eq!(
            login_cancelled_copy(),
            "login wait cancelled — closing browser…"
        );
    }

    #[test]
    fn error_should_lead_with_triage_when_raw_detail_present() {
        let triage = "Login check failed (timeout) — re-run `sleeper browser status`.";
        let out = triage_error(triage, "CDP Runtime.enable: websocket closed");
        let triage_pos = out.find(triage).expect("triage leads");
        let detail_pos = out.find("websocket closed").expect("detail follows");
        assert!(
            triage_pos < detail_pos,
            "triage must precede raw detail, got {out}"
        );
        assert_eq!(triage_error(triage, ""), triage);
        // chrome --version parsing feeds the skew gate, never panics.
        assert_eq!(
            parse_chrome_version("Google Chrome 126.0.6478.126 "),
            Some("126.0.6478.126".to_string())
        );
        assert_eq!(parse_chrome_version("garbage"), None);
    }
}
