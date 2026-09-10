//! Session integration tests — OPERATOR-GATED, `#[ignore]` by default.
//! Require live sleeper.com + a headed Chrome for the manual-login states.
//! Do NOT run in CI: `cargo test -- --ignored <name>` only by the operator.
//!
//! DOM notes + re-baselining procedure for the login signals land here
//! after live verification (Story 2.3 Task 3, Q5-adjacent). Selectors are
//! NEVER guessed: this stub records states, not selectors.

use sleeper::browser::{BrowserSession, LaunchConfig, SleeperBrowser, StealthLauncher};
use std::path::PathBuf;

/// Live tri-state distinction on sleeper.com. Operator flow:
/// 1. fresh profile → expect LoggedOut (all signals miss);
/// 2. log in by hand in the window → expect LoggedIn (all signals fire);
/// 3. verdicts recorded below; any split → Unknown (exit 4), never a guess.
#[tokio::test]
#[ignore]
async fn is_logged_in_should_distinguish_states_when_on_live_sleeper() {
    let profile = std::env::temp_dir().join(format!("sleeper-session-{}", std::process::id()));
    let chrome_binary = std::env::var("SLEEPER_TEST_CHROME_BINARY")
        .expect("operator gates this test: set SLEEPER_TEST_CHROME_BINARY to the CfT chrome binary");
    let config = LaunchConfig::headed_manual_login(profile.clone())
        .with_chrome_binary(PathBuf::from(chrome_binary));
    let launched = StealthLauncher::spawn(&config).await.expect("spawn");
    let mut session: Box<dyn BrowserSession> =
        Box::new(SleeperBrowser::connect(&launched.endpoint).await.expect("connect"));
    // LIVE-VERIFY-PENDING: operator records DOM notes per state here
    // (button text, session marker, avatar/nav marker) + verdict observed.
    // DO NOT fabricate: leave PENDING until driven against live sleeper.com.
    let verdict = session.login_verdict().await.expect("verdict");
    eprintln!("LIVE-VERIFY-PENDING: observed verdict on live sleeper.com: {verdict:?}");
    session.close().await.expect("close");
    launched.close().await.expect("teardown");
    let _ = std::fs::remove_dir_all(&profile);
}
