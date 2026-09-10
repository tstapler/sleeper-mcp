//! Login/status parity runs — OPERATOR-GATED, `#[ignore]` by default.
//!
//! Story 1.4 AC1–AC3 + AC5: ≥3 consecutive challenge-free runs per profile
//! state, full manual logins, 24h session survival, headless-vs-headed
//! agreement. Results go in `project_plans/.../implementation/plan.md`
//! (PENDING-OPERATOR tables) — never fabricated here.

use sleeper::browser::{BrowserSession, LaunchConfig, SleeperBrowser, StealthLauncher};
use std::path::PathBuf;

/// Fresh-profile login reaches the manual prompt challenge-free
/// (operator completes ≥1 FULL manual login per run; runs 1–2 wipe between).
#[ignore = "operator-gated: needs live Chrome + display + human login"]
#[tokio::test]
async fn login_should_reach_prompt_challenge_free_when_fresh_profile() {
    let profile = std::env::temp_dir().join("sleeper-epic1-fresh");
    let _ = std::fs::remove_dir_all(&profile);
    let mut cfg = LaunchConfig::headed_manual_login(profile);
    if let Some(bin) = std::env::var_os("SLEEPER_TEST_CHROME_BINARY") {
        cfg = cfg.with_chrome_binary(PathBuf::from(bin));
    } else {
        cfg = cfg.with_system_chrome_fallback(true);
    }
    let launched = StealthLauncher::spawn(&cfg).await.expect("spawn headed Chrome");
    let mut session: Box<dyn BrowserSession> =
        Box::new(SleeperBrowser::connect(&launched.endpoint).await.expect("connect"));
    // Operator: reach the prompt with no CAPTCHA loop BEFORE human action,
    // then log the run in plan.md Table 1.4.1.
    session.goto("https://sleeper.com").await.expect("goto");
    session.close().await.expect("close");
    launched.close().await.expect("teardown");
}

/// 3× authenticated `status` report logged-in with zero survivors
/// (`pgrep -f "chrome.*<profile>"` empty after each run).
#[ignore = "operator-gated: needs authenticated profile + pgrep check"]
#[tokio::test]
async fn status_should_report_no_survivors_when_run_three_times_authenticated() {
    let profile = std::env::temp_dir().join("sleeper-epic1-auth");
    let mut cfg = LaunchConfig::headless_status(profile.clone());
    if let Some(bin) = std::env::var_os("SLEEPER_TEST_CHROME_BINARY") {
        cfg = cfg.with_chrome_binary(PathBuf::from(bin));
    } else {
        cfg = cfg.with_system_chrome_fallback(true);
    }
    for run in 1..=3 {
        let launched = StealthLauncher::spawn(&cfg).await.expect("spawn");
        let mut session: Box<dyn BrowserSession> =
            Box::new(SleeperBrowser::connect(&launched.endpoint).await.expect("connect"));
        let logged_in = session.is_logged_in().await.expect("verdict");
        assert!(logged_in, "run {run}: authenticated profile must report logged-in");
        session.close().await.expect("close");
        launched.close().await.expect("teardown");
        // Operator: `pgrep -f "chrome.*sleeper-epic1-auth"` must be empty here.
    }
    let _ = profile;
}
