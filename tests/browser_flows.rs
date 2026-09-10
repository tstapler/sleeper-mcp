//! Lineup/trade flow tests — OPERATOR-GATED, `#[ignore]` by default.
//! Epic 4 is CUT per the appetite rule (see the plan.md cut note): this
//! stub asserts the DISABLED-triage path only and is marked PENDING-OPERATOR.
//! Post-cut-lift the operator live-verifies the selector bundles first, then
//! extends to the `lineup saved` / `trade proposed` assertions. Do NOT run
//! in CI: `cargo test -- --ignored lineup_should_save` only by the operator.

use sleeper::browser::session::{lineup_set_via_seam, trade_propose_via_seam, FakeSession};
use sleeper::browser::BrowserSession;
use sleeper::cli::{lineup_disabled_triage, trade_disabled_triage};

/// R23: while Epic 4 is cut, lineup/trade surface the disabled triage.
/// PENDING-OPERATOR: the `lineup saved` / `trade proposed` assertions below
/// activate only after the live-verify bundles land (plan.md Epic 4).
#[tokio::test]
#[ignore]
async fn lineup_should_save_when_selectors_verified() {
    let mut session = FakeSession::logged_in();
    let err = lineup_set_via_seam(&session, "123456", "WR", "9999")
        .await
        .expect_err("Epic 4 CUT: lineup flow must stay disabled");
    assert!(err.to_string().contains("live-unverified"), "got {err}");
    let triage = lineup_disabled_triage();
    assert!(
        !triage.contains("lineup saved"),
        "disabled path must never print a success verdict"
    );
    let terr = trade_propose_via_seam(&session, "123456", "9999", "8888")
        .await
        .expect_err("Epic 4 CUT: trade flow must stay disabled");
    assert!(terr.to_string().contains("live-unverified"), "got {terr}");
    let ttriage = trade_disabled_triage();
    assert!(
        !ttriage.contains("trade proposed"),
        "disabled path must never print a success verdict"
    );
    // No orphan: the session closes clean exactly once.
    session.close().await.expect("close leaves profile clean");
}
