//! Live-launch integration tests — OPERATOR-GATED, `#[ignore]` by default.
//!
//! Require a display, Chrome (CfT pin pending Story 2.1 — pass
//! `--chrome-binary` equivalent via `SLEEPER_TEST_CHROME_BINARY` or
//! `--system-chrome` flow), and manual login for the headed case.
//! Do NOT run in CI: `cargo test -- --ignored <name>` only by the operator.
//!
//! Run 1/3 record lives in `project_plans/.../implementation/plan.md`
//! (Story 1.4, PENDING-OPERATOR tables).

use sleeper::browser::{BrowserSession, LaunchConfig, SleeperBrowser, StealthLauncher};
use std::path::PathBuf;

fn test_profile(suffix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sleeper-epic1-{suffix}"));
    std::fs::create_dir_all(&dir).expect("temp profile");
    dir
}

fn test_config(headed: bool, suffix: &str) -> LaunchConfig {
    let mut cfg = if headed {
        LaunchConfig::headed_manual_login(test_profile(suffix))
    } else {
        LaunchConfig::headless_status(test_profile(suffix))
    };
    if let Some(bin) = std::env::var_os("SLEEPER_TEST_CHROME_BINARY") {
        cfg = cfg.with_chrome_binary(PathBuf::from(bin));
    } else {
        cfg = cfg.with_system_chrome_fallback(true);
    }
    cfg
}

/// Story 1.3 AC1: visible Chrome opens and `connect` attaches within 10s via
/// the stderr-parsed WS endpoint.
#[ignore = "operator-gated: needs live Chrome + display"]
#[tokio::test]
async fn spawn_should_attach_within_10s_when_headed_login_runs() {
    let cfg = test_config(true, "headed-attach");
    let launched = StealthLauncher::spawn(&cfg).await.expect("spawn headed Chrome");
    assert!(launched.endpoint.starts_with("ws://"), "endpoint parsed from stderr");
    let mut session: Box<dyn BrowserSession> =
        Box::new(SleeperBrowser::connect(&launched.endpoint).await.expect("connect"));
    session.goto("https://sleeper.com").await.expect("goto");
    session.close().await.expect("close");
    launched.close().await.expect("teardown");
}

/// Story 1.3 AC2: stealth signals coherent on pinned Chrome —
/// `navigator.webdriver` false and UA major matches `$PINNED`.
#[ignore = "operator-gated: needs live pinned Chrome"]
#[tokio::test]
async fn eval_bool_should_report_webdriver_false_when_connected_to_pinned_chrome() {
    let cfg = test_config(false, "stealth-probe");
    let launched = StealthLauncher::spawn(&cfg).await.expect("spawn headless Chrome");
    let mut session: Box<dyn BrowserSession> =
        Box::new(SleeperBrowser::connect(&launched.endpoint).await.expect("connect"));
    session.goto("https://sleeper.com").await.expect("goto");
    let stealthy = session
        .eval_bool("navigator.webdriver === undefined || navigator.webdriver === false")
        .await
        .expect("webdriver probe");
    assert!(stealthy, "navigator.webdriver must read false/undefined");
    let dump = session.eval_dump().await.expect("dump");
    assert!(dump.contains("sleeper.com"), "dump targets sleeper.com, got {dump}");
    session.close().await.expect("close");
    launched.close().await.expect("teardown");
}
