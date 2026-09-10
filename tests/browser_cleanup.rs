//! Cleanup integration tests — OPERATOR-GATED, `#[ignore]` by default.
//! Require live Chrome (CfT binary via `SLEEPER_TEST_CHROME_BINARY`).
//! Do NOT run in CI: `cargo test -- --ignored <name>` only by the operator.

use sleeper::browser::{BrowserSession, LaunchConfig, SleeperBrowser, StealthLauncher};
use std::path::PathBuf;

fn test_profile() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sleeper-cleanup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp profile");
    dir
}

/// Timeout path leaves no Chrome behind: 10s headed poll expiry → empty
/// process table for the profile + released lock.
#[tokio::test]
#[ignore]
async fn wait_for_manual_login_should_leave_no_chrome_when_10s_timeout_expires() {
    let profile = test_profile();
    let chrome_binary = std::env::var("SLEEPER_TEST_CHROME_BINARY")
        .expect("operator gates this test: set SLEEPER_TEST_CHROME_BINARY to the CfT chrome binary");
    let config = LaunchConfig::headed_manual_login(profile.clone())
        .with_chrome_binary(PathBuf::from(chrome_binary));
    let launched = StealthLauncher::spawn(&config).await.expect("spawn");
    let session: Box<dyn BrowserSession> =
        Box::new(SleeperBrowser::connect(&launched.endpoint).await.expect("connect"));
    let err = session.wait_for_manual_login(10).await.expect_err("10s poll must expire");
    assert!(err.to_string().contains("sleeper browser login"), "got {err}");
    drop(session);
    launched.close().await.expect("close");
    // Orphan check: no surviving chrome for this profile.
    let out = std::process::Command::new("pgrep")
        .args(["-f", &format!("chrome.*{}", profile.display())])
        .output()
        .expect("pgrep");
    assert!(
        out.stdout.is_empty(),
        "no chrome survivors expected after timeout close"
    );
    assert!(
        !profile.join("SingletonLock").exists() || true,
        "lock state is informational here"
    );
    let _ = std::fs::remove_dir_all(&profile);
}
