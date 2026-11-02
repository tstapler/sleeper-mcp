//! Waiver-claim flow tests, run against `FakeSession` (no live Chrome).
//! Story 3.2 implemented the real claim flow post-Q5 (plan.md Epic 3); these
//! tests exercise it through the seam's generic primitives, which `FakeSession`
//! satisfies unconditionally — proving the flow runs end to end without
//! depending on a live sleeper.com DOM. They do NOT replace the operator's
//! live ≥3-run table (plan.md Story 3.2), which still needs a real browser.

use sleeper::browser::session::{claim_player_via_seam, FakeSession};

const LEAGUE_ID: &str = "123456";
const PLAYER_NAME: &str = "Some Player";

/// Happy path: a logged-in session drives the full claim flow (search →
/// detect waiver-vs-add → open modal → optional drop → bid → submit) to
/// completion with no error.
#[tokio::test]
async fn claim_should_succeed_when_session_logged_in() {
    let session = FakeSession::logged_in();
    claim_player_via_seam(&session, LEAGUE_ID, PLAYER_NAME, 5, None)
        .await
        .expect("claim flow must complete through the seam primitives");
}

/// Happy path with a drop: same as above but exercising the optional
/// drop-row click branch.
#[tokio::test]
async fn claim_should_succeed_when_dropping_a_player() {
    let session = FakeSession::logged_in();
    claim_player_via_seam(&session, LEAGUE_ID, PLAYER_NAME, 5, Some("Bench Player"))
        .await
        .expect("claim flow with a drop must complete");
}

/// R22 error path: a dead/logged-out session surfaces the dead-session
/// hint instead of attempting any DOM automation.
#[tokio::test]
async fn claim_should_surface_dead_session_hint_when_logged_out() {
    use sleeper::browser::session::DEAD_SESSION_HINT;
    let session = FakeSession::logged_out();
    let err = claim_player_via_seam(&session, LEAGUE_ID, PLAYER_NAME, 0, None)
        .await
        .expect_err("claim flow must refuse to run without a logged-in session");
    assert_eq!(err.to_string(), DEAD_SESSION_HINT);
}

/// A transport error during the initial login check is swallowed by
/// `is_logged_in().await.unwrap_or(false)` in the flow (same remedial action
/// either way — re-run `sleeper browser login`), so it surfaces as the
/// dead-session hint rather than a distinct transport error. Documents the
/// real behavior rather than asserting an untrue "propagates distinctly"
/// claim.
#[tokio::test]
async fn claim_should_surface_dead_session_hint_when_login_check_transport_errors() {
    use sleeper::browser::session::DEAD_SESSION_HINT;
    let session = FakeSession::with_transport_error();
    let err = claim_player_via_seam(&session, LEAGUE_ID, PLAYER_NAME, 0, None)
        .await
        .expect_err("a failed login check must never proceed to automation");
    assert_eq!(err.to_string(), DEAD_SESSION_HINT);
}
