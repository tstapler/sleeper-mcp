//! Lineup-swap and trade-propose flow tests, run against `FakeSession` (no
//! live Chrome). Story 4.1 implemented the real lineup flow (plan.md Epic 4);
//! trade-propose is still gated per the Epic 4 cut decision — Q5-equivalent
//! live-UI verification for trades never happened. These tests do NOT
//! replace the operator's live ≥3-run table for lineups (plan.md Story 4.1),
//! which still needs a real browser.

use sleeper::browser::session::{lineup_set_via_seam, trade_propose_via_seam, FakeSession};

const LEAGUE_ID: &str = "123456";

/// Happy path: a logged-in session drives the lineup-swap flow (enter
/// select mode → wait for a valid bench target → click it) to completion.
#[tokio::test]
async fn lineup_should_save_when_session_logged_in() {
    let session = FakeSession::logged_in();
    lineup_set_via_seam(&session, LEAGUE_ID, "Started Player", "Bench Player")
        .await
        .expect("lineup flow must complete through the seam primitives");
}

/// A dead/logged-out session surfaces the dead-session hint instead of
/// attempting any DOM automation.
#[tokio::test]
async fn lineup_should_surface_dead_session_hint_when_logged_out() {
    use sleeper::browser::session::DEAD_SESSION_HINT;
    let session = FakeSession::logged_out();
    let err = lineup_set_via_seam(&session, LEAGUE_ID, "Started Player", "Bench Player")
        .await
        .expect_err("lineup flow must refuse to run without a logged-in session");
    assert_eq!(err.to_string(), DEAD_SESSION_HINT);
}

/// R23: trade-propose stays disabled regardless of session state — Epic 4's
/// cut decision for trades (unlike lineups) was never lifted, so the flow
/// must keep failing closed rather than attempting unverified automation.
#[tokio::test]
async fn trade_should_stay_disabled_when_session_logged_in() {
    let session = FakeSession::logged_in();
    let err = trade_propose_via_seam(&session, LEAGUE_ID, "9999", "8888")
        .await
        .expect_err("Epic 4 CUT: trade flow must stay disabled even when logged in");
    assert!(err.to_string().contains("live-unverified"), "got {err}");
    assert!(
        !err.to_string().contains("trade proposed"),
        "disabled path must never imply a success verdict"
    );
}

/// Dead-session check still takes priority over the disabled-trade error.
#[tokio::test]
async fn trade_should_surface_dead_session_hint_when_logged_out() {
    use sleeper::browser::session::DEAD_SESSION_HINT;
    let session = FakeSession::logged_out();
    let err = trade_propose_via_seam(&session, LEAGUE_ID, "9999", "8888")
        .await
        .expect_err("trade flow must refuse to run without a logged-in session");
    assert_eq!(err.to_string(), DEAD_SESSION_HINT);
}
