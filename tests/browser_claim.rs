//! Waiver-claim gating tests — OPERATOR-GATED, `#[ignore]` by default.
//! Gated on Q5 live verification (plan.md Epic 3): while Q5 is OPEN this
//! test asserts the DISABLED-triage path only (invalid player 0000 → triage
//! + no orphan). Post-Q5 the operator extends it to the live claim flow on
//! the test league. Do NOT run in CI:
//! `cargo test -- --ignored claim_should_surface_triage` only by the operator.

use sleeper::browser::session::{claim_player_via_seam, FakeSession};
use sleeper::browser::BrowserSession;
use sleeper::cli::claim_disabled_triage;

/// R22 error path: invalid player `0000` surfaces triage + leaves no orphan.
/// While Q5 is OPEN the seam-side flow returns the Q5-unverified error with
/// no selector use; the CLI stub prints the disabled triage to stderr and
/// exits non-zero (never attempts automation).
#[tokio::test]
#[ignore]
async fn claim_should_surface_triage_when_player_invalid() {
    let mut session = FakeSession::logged_in();
    let err = claim_player_via_seam(&session, "123456", "0000", None)
        .await
        .expect_err("Q5 OPEN: claim flow must stay disabled, never attempt automation");
    assert!(
        err.to_string().contains("Q5 not yet verified"),
        "triage must name the Q5 gate, got {err}"
    );
    // Triage contract: stderr-only helper, never a stdout success verdict.
    let triage = claim_disabled_triage();
    assert!(
        triage.contains("Q5 not yet verified — claim automation disabled"),
        "got {triage}"
    );
    assert!(
        !triage.contains("claim submitted"),
        "disabled path must never print a success verdict"
    );
    // No orphan: the session closes clean exactly once.
    session.close().await.expect("close leaves profile clean");
}
