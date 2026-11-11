//! Edit/cancel row-identity-disambiguation flow tests, run against
//! `FakeSession` (no live Chrome). Epic 1.4 / Story 1.4.1 (plan.md, ADR-004):
//! proves `edit_cancel_claim_via_seam`'s row-resolution and outcome-typing
//! logic against fixtures ahead of Phase 4's CLI/MCP wiring. The claims-panel
//! selectors themselves (`edit_claim_selectors` in flows.rs) stay
//! live-unverified until a real-browser run — these tests exercise the seam's
//! generic primitives only, never the real sleeper.com DOM.

use sleeper::browser::session::{edit_cancel_claim_via_seam, EditCancelAction, EditCancelOutcome, FakeSession};

const LEAGUE_ID: &str = "123456";

/// The two-claim fixture from plan.md Story 1.4.1's acceptance criteria:
/// both rows render text containing "Brissett", disambiguated only by
/// `transaction_id`.
const TWO_CLAIM_ROWS_JSON: &str = r#"[{"index":0,"text":"Jacoby Brissett — bid $78","transaction_id":"1"},{"index":1,"text":"Brissett Jones — bid $12","transaction_id":"2"}]"#;

/// Happy path (Story 1.4.1 AC2): a `FakeSession` canned with the two-claim
/// JSON for the row read, then a confirmed-cancel JSON for the post-click
/// read, resolves row index 0 by `transaction_id` and returns `Confirmed`.
#[tokio::test]
async fn edit_cancel_claim_via_seam_should_resolve_and_cancel_when_fake_session_returns_two_claim_rows() {
    let confirm_json = r#"{"reasserted":true,"clicked":true,"confirmed":true}"#;
    let session = FakeSession::with_eval_string_responses(&[TWO_CLAIM_ROWS_JSON, confirm_json]);

    let outcome = edit_cancel_claim_via_seam(&session, LEAGUE_ID, "1", EditCancelAction::Cancel)
        .await
        .expect("flow must resolve the ambiguous rows via transaction_id and complete");

    assert_eq!(outcome, EditCancelOutcome::Confirmed);
}

/// Same fixture, editing a bid instead of cancelling — proves both
/// `EditCancelAction` variants drive the same disambiguation + re-assertion
/// path.
#[tokio::test]
async fn edit_cancel_claim_via_seam_should_resolve_and_edit_bid_when_fake_session_returns_two_claim_rows() {
    let confirm_json = r#"{"reasserted":true,"clicked":true,"confirmed":true}"#;
    let session = FakeSession::with_eval_string_responses(&[TWO_CLAIM_ROWS_JSON, confirm_json]);

    let outcome = edit_cancel_claim_via_seam(&session, LEAGUE_ID, "2", EditCancelAction::EditBid(50))
        .await
        .expect("flow must resolve row 1 (transaction_id \"2\") and complete");

    assert_eq!(outcome, EditCancelOutcome::Confirmed);
}

/// Pre-click re-assertion failure (Domain Glossary: `AbortedBeforeWrite`):
/// the second `eval_string` call reports the resolved row's identity no
/// longer matches, so nothing was clicked — distinct from a bare `Err`
/// that would imply the same, but typed per plan.md's discipline.
#[tokio::test]
async fn edit_cancel_claim_via_seam_should_abort_before_write_when_reassertion_fails() {
    let reassertion_failed_json = r#"{"reasserted":false}"#;
    let session =
        FakeSession::with_eval_string_responses(&[TWO_CLAIM_ROWS_JSON, reassertion_failed_json]);

    let outcome = edit_cancel_claim_via_seam(&session, LEAGUE_ID, "1", EditCancelAction::Cancel)
        .await
        .expect("a failed re-assertion is a typed outcome, not a transport Err");

    match outcome {
        EditCancelOutcome::AbortedBeforeWrite { reason } => {
            assert!(!reason.is_empty(), "reason must explain why nothing was clicked");
        }
        other => panic!("expected AbortedBeforeWrite, got {other:?}"),
    }
}

/// Post-click confirmation-read failure (Domain Glossary: `Ambiguous`): the
/// click fired (`reasserted: true`) but the confirmation field never came
/// back, so the outcome is genuinely unknown — never collapsed into
/// `Confirmed`.
#[tokio::test]
async fn edit_cancel_claim_via_seam_should_be_ambiguous_when_post_click_confirmation_is_missing() {
    let clicked_unconfirmed_json = r#"{"reasserted":true,"clicked":true}"#;
    let session =
        FakeSession::with_eval_string_responses(&[TWO_CLAIM_ROWS_JSON, clicked_unconfirmed_json]);

    let outcome = edit_cancel_claim_via_seam(&session, LEAGUE_ID, "1", EditCancelAction::Cancel)
        .await
        .expect("an unconfirmed click is a typed outcome, not a transport Err");

    match outcome {
        EditCancelOutcome::Ambiguous { reason } => {
            assert!(!reason.is_empty(), "reason must explain the click fired but confirmation failed");
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }
}

/// Post-click confirmation-read failure via malformed JSON (e.g. the
/// `eval_string` call itself returned garbage) must also surface as
/// `Ambiguous`, never panic and never silently read as `Confirmed`.
#[tokio::test]
async fn edit_cancel_claim_via_seam_should_be_ambiguous_when_confirmation_read_is_unparseable() {
    let session = FakeSession::with_eval_string_responses(&[TWO_CLAIM_ROWS_JSON, "not json"]);

    let outcome = edit_cancel_claim_via_seam(&session, LEAGUE_ID, "1", EditCancelAction::Cancel)
        .await
        .expect("unparseable confirmation JSON is a typed outcome, not a panic");

    assert!(matches!(outcome, EditCancelOutcome::Ambiguous { .. }));
}

/// Error path: two rows sharing the same `transaction_id` make the initial
/// resolution itself ambiguous — the flow must fail closed before issuing
/// the second (write) `eval_string` call at all, never guessing.
#[tokio::test]
async fn edit_cancel_claim_via_seam_should_fail_closed_when_initial_resolution_is_ambiguous() {
    let duplicate_transaction_id_json =
        r#"[{"index":0,"text":"Row A","transaction_id":"1"},{"index":1,"text":"Row B","transaction_id":"1"}]"#;
    let session = FakeSession::with_eval_string_responses(&[duplicate_transaction_id_json]);

    let err = edit_cancel_claim_via_seam(&session, LEAGUE_ID, "1", EditCancelAction::Cancel)
        .await
        .expect_err("ambiguous initial resolution must be Err, never a guessed row");
    assert!(err.to_string().contains("ambiguous"), "got {err}");
}

/// A dead/logged-out session surfaces the dead-session hint instead of
/// attempting any DOM read or write.
#[tokio::test]
async fn edit_cancel_claim_via_seam_should_surface_dead_session_hint_when_logged_out() {
    use sleeper::browser::session::DEAD_SESSION_HINT;
    let session = FakeSession::logged_out();

    let err = edit_cancel_claim_via_seam(&session, LEAGUE_ID, "1", EditCancelAction::Cancel)
        .await
        .expect_err("flow must refuse to run without a logged-in session");
    assert_eq!(err.to_string(), DEAD_SESSION_HINT);
}
