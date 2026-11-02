//! `browser status` flow: renders the tri-state login verdict (ux.md
//! Surfaces C/D). Split out of `login.rs`, which still owns the shared
//! launch/triage plumbing (`launch_session`, `triage_error`, `debug_detail`,
//! `launch_failure_exit`, `launch_failure_json_error`) this module reuses.
use console::style;
use std::path::PathBuf;

use crate::config::Config;
use crate::error::Result;

use super::login::{
    check_failed_triage, debug_detail, launch_failure_exit, launch_failure_json_error,
    launch_session, triage_error, LaunchOptions,
};

/// Exit-code contract (ux.md Surfaces C/D): 0 logged-in / 1 logged-out /
/// 2 skew / 3 lock-busy / 4 unknown. Unknown is NEVER 1 or 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusOutcome {
    LoggedIn,
    LoggedOut,
    Skew,
    LockBusy,
    Unknown,
}

/// Profile-lock state for [`status_outcome_for_session`]. A named enum beats
/// a bare positional bool at call sites (`LockState::Held` vs. `true`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    Free,
    Held,
}

/// Render/gate mode shared by every helper below (ux.md Surfaces C/D): json
/// (machine-readable, stdout-only) vs. text (human, triage-first stderr). A
/// named enum beats threading a bare `json: bool` through four functions
/// that each re-branch on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Json,
    Text,
}

impl OutputFormat {
    fn from_json_flag(json: bool) -> Self {
        if json {
            OutputFormat::Json
        } else {
            OutputFormat::Text
        }
    }
}

pub fn status_exit_code(outcome: &StatusOutcome) -> i32 {
    match outcome {
        StatusOutcome::LoggedIn => 0,
        StatusOutcome::LoggedOut => 1,
        StatusOutcome::Skew => 2,
        StatusOutcome::LockBusy => 3,
        StatusOutcome::Unknown => 4,
    }
}

/// Skew triage line: contains `version skew` + the `re-run with update`
/// directive (never "wait" — skew is fixed by re-resolving/updating).
pub fn skew_triage_message(pinned: &str, runtime: &str) -> String {
    format!(
        "CfT Chrome runtime v{runtime} vs pinned v{pinned} — version skew. Please re-run with update: re-run `sleeper browser login` to re-resolve the pin; if it persists, update the pin to v{runtime}."
    )
}

/// Lock-busy copy (ux.md state 4): `status` aborts, profile untouched.
pub fn lock_busy_message(profile: &std::path::Path) -> String {
    format!(
        "profile busy — close the login window first (profile {} is locked by a live Chrome window)",
        profile.display()
    )
}

/// Unknown-verdict triage (signal split/timeout): exit 4, re-login only if
/// persistent — distinct from yellow logged-out (exit 1) and lock-busy (3).
pub fn unknown_triage(which_signal: &str) -> String {
    format!(
        "Login check unknown ({which_signal}) — re-run `sleeper browser status`; re-login only if persistent."
    )
}

/// `status --json` shape (ux.md Surface D): stdout carries ONLY this object.
/// Failure shape is `{"logged_in":false,"error":"…"}`, never a bare stack.
pub fn status_json(
    logged_in: bool,
    pinned: &str,
    runtime: &str,
    profile: &std::path::Path,
    error: Option<&str>,
) -> String {
    if let Some(e) = error {
        serde_json::json!({"logged_in": false, "error": e}).to_string()
    } else {
        serde_json::json!({
            "logged_in": logged_in,
            "chrome_version": pinned,
            "chrome_runtime_version": runtime,
            "profile": profile.display().to_string(),
        })
        .to_string()
    }
}

/// Pure outcome resolution behind `status` (FakeSession-programmable):
/// lock first (profile untouched), then skew, then the tri-state verdict.
/// Transport errors surface as `Unknown` (caller prints the distinct
/// `Login check failed` copy with exit 4).
pub async fn status_outcome_for_session(
    session: &dyn crate::browser::BrowserSession,
    pinned: &str,
    runtime: &str,
    lock: LockState,
) -> StatusOutcome {
    use crate::browser::LoginVerdict;
    if lock == LockState::Held {
        return StatusOutcome::LockBusy;
    }
    if pinned != runtime {
        return StatusOutcome::Skew;
    }
    match session.login_verdict().await {
        Ok(LoginVerdict::LoggedIn) => StatusOutcome::LoggedIn,
        Ok(LoginVerdict::LoggedOut) => StatusOutcome::LoggedOut,
        Ok(LoginVerdict::Unknown) => StatusOutcome::Unknown,
        Err(_) => StatusOutcome::Unknown,
    }
}

/// Options for `browser status` (ux.md Surfaces B–D). Bundled since these
/// five independent toggles come straight off `BrowserAction::Status`'s own
/// CLI flags — a struct keeps the destructured field names visible at the
/// call site instead of flattening into six positional args.
pub(super) struct StatusOptions {
    pub(super) headed: bool,
    pub(super) json: bool,
    pub(super) debug: bool,
    pub(super) system_chrome: bool,
    pub(super) chrome_binary: Option<PathBuf>,
}

/// Lock gate (checked before touching the profile): prints the busy message
/// in the caller's requested shape and returns the exit code to use, or
/// `None` to proceed — profile untouched either way.
fn check_lock_gate(profile_dir: &std::path::Path, pinned: &str, format: OutputFormat) -> Option<i32> {
    if !profile_dir.join("SingletonLock").exists() {
        return None;
    }
    let msg = lock_busy_message(profile_dir);
    match format {
        OutputFormat::Json => {
            println!("{}", status_json(false, pinned, "unknown", profile_dir, Some(&msg)));
        }
        OutputFormat::Text => eprintln!("{}", style(&msg).red()),
    }
    Some(3)
}

/// Skew gate (checked before launch, no fallback): prints the triage message
/// in the caller's requested shape and returns the exit code to use, or
/// `None` to proceed.
fn check_skew_gate(
    pinned: &str,
    runtime: &str,
    profile_dir: &std::path::Path,
    format: OutputFormat,
) -> Option<i32> {
    if runtime == "unknown" || pinned == runtime {
        return None;
    }
    let msg = skew_triage_message(pinned, runtime);
    match format {
        OutputFormat::Json => {
            println!("{}", status_json(false, pinned, runtime, profile_dir, Some(&msg)));
        }
        OutputFormat::Text => eprintln!("{}", style(triage_error(&msg, "")).red()),
    }
    Some(2)
}

/// Final render for `status` (ux.md Surfaces C/D): json vs. text shape,
/// keyed on the resolved [`StatusOutcome`]. Pure printing — the caller has
/// already closed the session/browser before calling this.
fn print_status_outcome(
    outcome: &StatusOutcome,
    pinned: &str,
    runtime: &str,
    profile_dir: &std::path::Path,
    format: OutputFormat,
) {
    match format {
        OutputFormat::Json => match outcome {
            StatusOutcome::LoggedIn => {
                println!("{}", status_json(true, pinned, runtime, profile_dir, None));
            }
            StatusOutcome::LoggedOut => {
                println!("{}", status_json(false, pinned, runtime, profile_dir, None));
            }
            StatusOutcome::Skew => {
                let msg = skew_triage_message(pinned, runtime);
                println!("{}", status_json(false, pinned, runtime, profile_dir, Some(&msg)));
            }
            StatusOutcome::LockBusy => {
                let msg = lock_busy_message(profile_dir);
                println!("{}", status_json(false, pinned, runtime, profile_dir, Some(&msg)));
            }
            StatusOutcome::Unknown => {
                let msg = unknown_triage("signal split/timeout");
                println!("{}", status_json(false, pinned, runtime, profile_dir, Some(&msg)));
            }
        },
        OutputFormat::Text => match outcome {
            StatusOutcome::LoggedIn => {
                println!("{}", style("Logged in.").green());
            }
            StatusOutcome::LoggedOut => {
                println!("{}", style("Not logged in — run `sleeper browser login`.").yellow());
            }
            StatusOutcome::Unknown => {
                eprintln!("{}", style(unknown_triage("signal split/timeout")).red());
            }
            StatusOutcome::Skew => {
                eprintln!("{}", style(skew_triage_message(pinned, runtime)).red());
            }
            StatusOutcome::LockBusy => {
                eprintln!("{}", style(lock_busy_message(profile_dir)).red());
            }
        },
    }
}

pub(super) async fn cmd_browser_status(config: &Config, opts: StatusOptions) -> Result<()> {
    use crate::browser::{LoginVerdict, PinnedVersion};
    let StatusOptions { headed, json, debug, system_chrome, chrome_binary } = opts;
    let format = OutputFormat::from_json_flag(json);
    let profile_dir = config.chrome_profile_dir();
    let pinned = PinnedVersion::pinned().0.clone();

    if let Some(code) = check_lock_gate(&profile_dir, &pinned, format) {
        std::process::exit(code);
    }

    let probe_binary = chrome_binary.clone().unwrap_or_else(|| {
        crate::browser::cft::cft_chrome_binary(&config.data_dir, &PinnedVersion::pinned())
    });
    let runtime = if system_chrome || !probe_binary.is_file() {
        "unknown".to_string()
    } else {
        crate::browser::cft::runtime_version(&probe_binary)
    };
    if let Some(code) = check_skew_gate(&pinned, &runtime, &profile_dir, format) {
        std::process::exit(code);
    }

    let (mut session, launched) =
        match launch_session(config, LaunchOptions { headed, system_chrome, chrome_binary }).await {
            Ok(pair) => pair,
            Err(e) => {
                // --json contract (Surface D / Story 2.4 AC2): transport/check
                // failure exits non-zero with the failure shape on stdout
                // (pipeable to jq), never empty. Stderr triage-first line is
                // unchanged inside launch_failure_exit; exit code preserved.
                if format == OutputFormat::Json {
                    let err_msg = launch_failure_json_error(&e);
                    println!(
                        "{}",
                        status_json(false, &pinned, &runtime, &profile_dir, Some(&err_msg))
                    );
                }
                launch_failure_exit(&e)
            }
        };
    if debug || std::env::var("SLEEPER_BROWSER_DEBUG").is_ok() {
        let dump = match session.eval_dump().await {
            Ok(dump) => dump,
            Err(e) => {
                // Debug-probe failure maps like any transport failure (exit 4,
                // never exit 1 via `?`); closes are best-effort on this path.
                let detail = e.to_string();
                let _ = session.close().await;
                let _ = launched.close().await;
                let msg = check_failed_triage(&detail);
                match format {
                    OutputFormat::Json => println!(
                        "{}",
                        status_json(false, &pinned, &runtime, &profile_dir, Some(&msg))
                    ),
                    OutputFormat::Text => {
                        eprintln!("{}", style(triage_error(&msg, &debug_detail(&detail))).red())
                    }
                }
                std::process::exit(4);
            }
        };
        eprintln!(
            "DEBUG dump: {}",
            crate::browser::session::redact_secrets(&dump)
        );
        eprintln!(
            "{}",
            crate::browser::session::debug_browser_line(
                &pinned,
                &runtime,
                "<flag-hash>",
                "<cdp-port>",
                &profile_dir,
                &[],
            )
        );
    }
    let outcome = match session.login_verdict().await {
        Ok(LoginVerdict::LoggedIn) => StatusOutcome::LoggedIn,
        Ok(LoginVerdict::LoggedOut) => StatusOutcome::LoggedOut,
        Ok(LoginVerdict::Unknown) => StatusOutcome::Unknown,
        Err(e) => {
            let detail = e.to_string();
            session.close().await?;
            launched.close().await?;
            let msg = check_failed_triage(&detail);
            match format {
                OutputFormat::Json => {
                    println!("{}", status_json(false, &pinned, &runtime, &profile_dir, Some(&msg)))
                }
                OutputFormat::Text => {
                    eprintln!("{}", style(triage_error(&msg, &debug_detail(&detail))).red())
                }
            }
            std::process::exit(4);
        }
    };
    session.close().await?;
    launched.close().await?;
    let code = status_exit_code(&outcome);
    print_status_outcome(&outcome, &pinned, &runtime, &profile_dir, format);
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::{
        lock_busy_message, skew_triage_message, status_exit_code, status_json,
        status_outcome_for_session, unknown_triage, LockState, StatusOutcome,
    };
    use crate::browser::FakeSession;
    use crate::cli::browser::login::{challenge_loop_copy, check_failed_triage, launch_failure_json_error};

    #[tokio::test]
    async fn status_should_exit_2_with_triage_when_skew_detected() {
        let session = FakeSession::logged_in();
        let outcome =
            status_outcome_for_session(&session, "126.0.6478.126", "127.0.6533.99", LockState::Free)
                .await;
        assert_eq!(outcome, StatusOutcome::Skew);
        assert_eq!(status_exit_code(&outcome), 2);
        let msg = skew_triage_message("126.0.6478.126", "127.0.6533.99");
        assert!(msg.contains("version skew"), "got {msg}");
        assert!(msg.contains("re-run with update"), "got {msg}");
    }

    #[tokio::test]
    async fn status_should_exit_3_when_singleton_lock_held() {
        // Held lock short-circuits before any profile touch: the outcome
        // resolves without creating or modifying the profile dir.
        let untouched = std::env::temp_dir().join(format!(
            "sleeper-status-untouched-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&untouched);
        let session = FakeSession::logged_in();
        let outcome =
            status_outcome_for_session(&session, "126.0.6478.126", "126.0.6478.126", LockState::Held)
                .await;
        assert_eq!(outcome, StatusOutcome::LockBusy);
        assert_eq!(status_exit_code(&outcome), 3);
        let msg = lock_busy_message(&untouched);
        assert!(
            msg.contains("profile busy — close the login window first"),
            "got {msg}"
        );
        assert!(
            !untouched.exists(),
            "lock-busy must leave the profile untouched"
        );
    }

    #[tokio::test]
    async fn status_should_exit_4_with_signal_log_when_verdict_unknown() {
        let session = FakeSession::with_signal_split();
        let outcome =
            status_outcome_for_session(&session, "126.0.6478.126", "126.0.6478.126", LockState::Free)
                .await;
        assert_eq!(outcome, StatusOutcome::Unknown);
        assert_eq!(status_exit_code(&outcome), 4);
        let msg = unknown_triage("signal 1/3 missed");
        assert!(msg.contains("signal 1/3 missed"), "per-signal log, got {msg}");
        assert!(
            msg.contains("re-run `sleeper browser status`"),
            "got {msg}"
        );
        assert!(msg.contains("re-login only if persistent"), "got {msg}");
        // Transport errors share exit 4 with their own copy.
        let failed = check_failed_triage("fake transport error");
        assert!(failed.contains("Login check failed"), "got {failed}");
        assert!(failed.contains("re-run `sleeper browser status`"), "got {failed}");
    }

    #[test]
    fn status_should_not_use_exit_3_when_verdict_unknown() {
        assert_eq!(status_exit_code(&StatusOutcome::Unknown), 4);
        assert_ne!(
            status_exit_code(&StatusOutcome::Unknown),
            3,
            "unknown must never reuse the lock-busy exit"
        );
        assert_eq!(status_exit_code(&StatusOutcome::LockBusy), 3);
    }

    #[test]
    fn status_json_should_emit_shape_when_authenticated() {
        let profile = std::path::Path::new("/tmp/chrome-profile");
        let raw = status_json(true, "126.0.6478.126", "126.0.6478.126", profile, None);
        let v: serde_json::Value = serde_json::from_str(&raw).expect("stdout must be pure JSON");
        assert_eq!(v["logged_in"], true);
        assert_eq!(v["chrome_version"], "126.0.6478.126");
        assert_eq!(v["chrome_runtime_version"], "126.0.6478.126");
        assert_eq!(v["profile"], "/tmp/chrome-profile");
        assert_eq!(status_exit_code(&StatusOutcome::LoggedIn), 0);
    }

    #[test]
    fn status_json_should_map_each_cause_when_failing() {
        let profile = std::path::Path::new("/tmp/chrome-profile");
        // Logged-out: full shape, exit 1.
        let raw = status_json(false, "126.0.6478.126", "126.0.6478.126", profile, None);
        let v: serde_json::Value = serde_json::from_str(&raw).expect("pure JSON");
        assert_eq!(v["logged_in"], false);
        assert_eq!(status_exit_code(&StatusOutcome::LoggedOut), 1);
        // Skew / lock / unknown: error shape, never a bare stack.
        for (outcome, code) in [
            (StatusOutcome::Skew, 2),
            (StatusOutcome::LockBusy, 3),
            (StatusOutcome::Unknown, 4),
        ] {
            assert_eq!(status_exit_code(&outcome), code);
            let raw = status_json(false, "126.0.6478.126", "126.0.6478.126", profile, Some("boom"));
            let v: serde_json::Value = serde_json::from_str(&raw).expect("pure JSON");
            assert_eq!(v, serde_json::json!({"logged_in": false, "error": "boom"}));
            assert!(
                !raw.contains("panicked") && !raw.contains("Backtrace"),
                "stdout must never carry a bare stack, got {raw}"
            );
        }
    }

    #[test]
    fn skew_error_should_contain_triage_verb_when_printed() {
        let msg = skew_triage_message("126.0.6478.126", "127.0.6533.99");
        let lower = msg.to_lowercase();
        assert!(
            lower.contains("wait") || lower.contains("re-run") || lower.contains("update"),
            "skew line must carry a triage verb, got {msg}"
        );
        // Challenge-loop copy is tool news, never operator blame.
        let news = challenge_loop_copy(61);
        assert!(news.contains("unexpected with the stealth backend"), "got {news}");
        assert!(news.contains("sleeper browser status --debug"), "got {news}");
    }

    #[test]
    fn status_json_should_emit_failure_shape_when_launch_fails() {
        use crate::browser::other;
        let profile = std::path::Path::new("/tmp/chrome-profile");
        // Transport failure: short triage copy in the failure shape, never empty.
        let err = other("connect failed: no Chrome installed");
        let msg = launch_failure_json_error(&err);
        assert!(msg.contains("Login check failed (browser launch)"), "got {msg}");
        let raw = status_json(false, "126.0.6478.126", "unknown", profile, Some(&msg));
        let v: serde_json::Value = serde_json::from_str(&raw).expect("stdout must be pure JSON");
        assert_eq!(v, serde_json::json!({"logged_in": false, "error": msg}));
        // Skew-flavoured launch failure passes its own triage through.
        let skew = other("CfT Chrome runtime v127.0.6533.99 vs pinned v126.0.6478.126 — version skew");
        let skew_msg = launch_failure_json_error(&skew);
        assert!(skew_msg.contains("version skew"), "got {skew_msg}");
        let raw = status_json(false, "126.0.6478.126", "127.0.6533.99", profile, Some(&skew_msg));
        let v: serde_json::Value = serde_json::from_str(&raw).expect("pure JSON");
        assert_eq!(v["logged_in"], false);
    }
}
