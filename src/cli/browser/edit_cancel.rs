//! Epic 4.1 / Story 4.1.1: `run_edit_cancel_claim` — the pre-write (and
//! post-write) status re-check wrapped around Epic 4.2's live DOM
//! edit/cancel flow (`edit_cancel_claim_via_seam`, `flows.rs`). Split out of
//! `actions.rs` (plan.md Task 4.1.1a explicitly allows this once that file
//! would exceed its original scope) so `actions.rs` stays under the
//! repo's file-size check.
//!
//! No printing/process exit here — this is the reusable core shared by a
//! future CLI command and MCP tool (Epic 4.3), the same split
//! `run_claim`/`run_lineup_set` (`actions.rs`) already use for their own
//! write actions.
use crate::analysis::AnalysisService;
use crate::browser::session::{EditCancelAction, EditCancelOutcome};
use crate::config::Config;
use crate::db::CachedTransaction;
use crate::error::Result;

use super::login::{launch_failure_error, launch_session, LaunchOptions};
use super::transactions::{read_through, StalenessTolerance};

/// Epic 4.1 / Story 4.1.1: pre-write (and post-write) status re-check
/// triage, matching `pending_failed_triage()`'s shape (`pending.rs:220`) and
/// ux.md §4c/§11's exact pattern verbatim. Used both when the pre-write
/// re-check finds the claim already resolved (no DOM write attempted) and,
/// with `"not found"` as `current_status`, when `transaction_id` isn't in
/// the cache at all even after a fresh `AlwaysLive` fetch — neither case is
/// safe to write against, since this tool can no longer independently
/// confirm the claim is still `pending`.
fn edit_cancel_already_resolved_triage(transaction_id: &str, league_id: &str, current_status: &str) -> String {
    format!(
        "claim {transaction_id} is no longer pending (now: {current_status}) — nothing to edit/cancel. Run `sleeper transactions --league {league_id}` to see its resolved outcome."
    )
}

/// Pure, no I/O (Task 4.1.1a): decides whether `transaction_id`'s cached
/// status permits an edit/cancel write to proceed — the guard this story is
/// actually about, extracted out of the `read_through` call site so it's
/// unit-testable without a database or browser, the same testability-first
/// pattern Epic 2.1's `needs_refresh`/`populator_for` used
/// (`transactions.rs:370`/`345`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum PreWriteCheck {
    StillPending,
    AlreadyResolved(String),
    /// Not in the cache at all even after a fresh `AlwaysLive` fetch —
    /// treated the same as already-resolved: never safe to write against a
    /// claim this tool can't independently verify is still pending.
    NotFound,
}

fn pre_write_status_check(cached: &[CachedTransaction], transaction_id: &str) -> PreWriteCheck {
    match cached.iter().find(|t| t.transaction_id == transaction_id) {
        Some(t) if t.status.as_deref() == Some("pending") => PreWriteCheck::StillPending,
        Some(t) => PreWriteCheck::AlreadyResolved(t.status.clone().unwrap_or_else(|| "unknown".to_string())),
        None => PreWriteCheck::NotFound,
    }
}

/// Pure, no I/O: whether the post-write re-check's cached row
/// independently confirms the write landed — action-aware, since `Cancel`
/// and `EditBid` change different fields (`status` vs. `waiver_bid`). The
/// seam flow's own `EditCancelOutcome::Confirmed` only means the in-page
/// confirmation read looked right; this is what lets Epic 4.3 trust it
/// against a source of truth the DOM itself didn't produce. Returns the
/// detail string Epic 4.3 needs for its success copy (new status / new bid)
/// when verified, `None` otherwise.
fn post_write_verified(
    cached: &[CachedTransaction],
    transaction_id: &str,
    action: EditCancelAction,
) -> Option<String> {
    let t = cached.iter().find(|t| t.transaction_id == transaction_id)?;
    match action {
        EditCancelAction::Cancel => {
            (t.status.as_deref() != Some("pending")).then(|| t.status.clone().unwrap_or_default())
        }
        EditCancelAction::EditBid(new_bid) => {
            (t.waiver_bid == Some(i64::from(new_bid))).then(|| new_bid.to_string())
        }
    }
}

/// Epic 4.1 core result (Task 4.1.1a): everything Epic 4.3's CLI/MCP
/// surface needs to pick its own copy, without re-deriving any decision
/// `run_edit_cancel_claim` already made.
// Epic 4.1 lands ahead of Epic 4.3's CLI/MCP wiring (plan.md's own task
// split) — nothing in the crate calls `run_edit_cancel_claim` yet outside
// this file's own tests, so both are allowed dead code until that lands,
// the same forward-declared-but-unwired situation `RowTarget::Fallback`
// (`flows.rs`) documents for Epic 1.4/ADR-004.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditCancelRunOutcome {
    /// The seam flow reported `Confirmed` AND the post-write re-check
    /// (`AlwaysLive`, independent of the DOM) agrees the write landed.
    /// `detail` is the new status (`Cancel`) or new bid amount (`EditBid`)
    /// — Epic 4.3 may render "claim cancelled"/"claim bid updated to $N"
    /// only on this variant (ux.md §5 AC1).
    Confirmed { detail: String },
    /// The seam flow reported `Confirmed`, but the post-write re-check
    /// still disagrees (status still `pending`, bid unchanged, or the row
    /// is missing) — the DOM claimed success but the independent source of
    /// truth hasn't caught up or disagrees. Must never be rendered as plain
    /// success.
    ConfirmedButUnverified,
    /// The seam flow's own typed non-success outcome
    /// (`EditCancelOutcome::AbortedBeforeWrite`/`Ambiguous`), passed through
    /// verbatim for Epic 4.3's own triage fns
    /// (`edit_cancel_aborted_before_write_triage`/
    /// `edit_cancel_ambiguous_outcome_triage`, Tasks 4.2.1c/4.2.1b) to
    /// render — not this function's job.
    SeamOutcome(EditCancelOutcome),
}

/// Story 4.1.1 core: pre-write re-check (`AlwaysLive`, scoped to one
/// transaction_id by filtering `read_through`'s result in Rust — neither
/// `read_through` nor the underlying `list_transactions` takes a
/// single-transaction filter today) → if still pending, launch → seam flow
/// → (on `Confirmed`) post-write re-check → close every branch, no
/// printing/process exit (shared by a future CLI command and MCP tool, per
/// Epic 4.3). An already-resolved (or not-found) claim short-circuits
/// before `launch_session` is even called — no browser launch, no DOM write
/// attempted (ux.md §5/§11's explicit ordering requirement).
#[allow(dead_code)] // Epic 4.3 wires this in; see `EditCancelRunOutcome`'s note above.
// kibitzer:ignore long-function -- pre-check, launch, seam call, close
// (every branch), post-check: one atomic launch→check→flow→triage
// operation, the same shape/rationale as `read_through`'s own suppression
// (transactions.rs:410) and `run_claim`/`run_lineup_set` (actions.rs).
pub(crate) async fn run_edit_cancel_claim(
    config: &Config,
    analysis: &AnalysisService,
    league_id: &str,
    transaction_id: &str,
    action: EditCancelAction,
) -> Result<EditCancelRunOutcome> {
    use crate::browser::session::{edit_cancel_claim_via_seam, DEAD_SESSION_HINT};

    let pre_check =
        read_through(config, analysis, league_id, None, StalenessTolerance::AlwaysLive, false).await?;
    match pre_write_status_check(&pre_check, transaction_id) {
        PreWriteCheck::StillPending => {}
        PreWriteCheck::AlreadyResolved(status) => {
            return Err(crate::browser::other(edit_cancel_already_resolved_triage(
                transaction_id,
                league_id,
                &status,
            )));
        }
        PreWriteCheck::NotFound => {
            return Err(crate::browser::other(edit_cancel_already_resolved_triage(
                transaction_id,
                league_id,
                "not found",
            )));
        }
    }

    let (mut session, launched) = match launch_session(config, LaunchOptions::headless()).await {
        Ok(pair) => pair,
        Err(e) => return Err(launch_failure_error(&e)),
    };
    let outcome = edit_cancel_claim_via_seam(&*session, league_id, transaction_id, action).await;
    session.close().await?;
    launched.close().await?;

    match outcome {
        Ok(EditCancelOutcome::Confirmed) => {
            // Post-write re-check: `Confirmed` only means the in-page
            // confirmation read looked right — ask the independent source
            // of truth too, before Epic 4.3 is allowed to print success.
            let post_check =
                read_through(config, analysis, league_id, None, StalenessTolerance::AlwaysLive, false)
                    .await?;
            match post_write_verified(&post_check, transaction_id, action) {
                Some(detail) => Ok(EditCancelRunOutcome::Confirmed { detail }),
                None => Ok(EditCancelRunOutcome::ConfirmedButUnverified),
            }
        }
        Ok(other_outcome) => Ok(EditCancelRunOutcome::SeamOutcome(other_outcome)),
        Err(e) if e.to_string() == DEAD_SESSION_HINT => Err(e),
        // Generic flow failure (no-unique-row-match, selector/DOM-drift,
        // etc.) — passed through verbatim; Epic 4.3 (Tasks 4.3.1a/4.3.2a)
        // builds `edit_cancel_failed_triage()` to render it, not this fn.
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::SleeperClient;
    use crate::db::{StatsDb, TransactionRow};

    fn cached_row(transaction_id: &str, status: Option<&str>, waiver_bid: Option<i64>) -> CachedTransaction {
        CachedTransaction {
            transaction_id: transaction_id.to_string(),
            season: "2026".to_string(),
            week: 9,
            kind: "waiver".to_string(),
            status: status.map(str::to_string),
            created: 1_781_000_000,
            adds: serde_json::json!([]),
            drops: serde_json::json!([]),
            roster_ids: serde_json::json!([5]),
            waiver_bid,
            fetched_at: chrono::Utc::now(),
            status_updated: Some(1_781_850_000),
            source: Some("graphql".to_string()),
        }
    }

    // -----------------------------------------------------------------
    // `edit_cancel_already_resolved_triage` — ux.md §4c/§11's exact pattern.
    // -----------------------------------------------------------------

    #[test]
    fn edit_cancel_already_resolved_triage_should_match_ux_exact_pattern() {
        let msg = edit_cancel_already_resolved_triage("1", "1352883872131481600", "complete");
        assert_eq!(
            msg,
            "claim 1 is no longer pending (now: complete) — nothing to edit/cancel. Run `sleeper transactions --league 1352883872131481600` to see its resolved outcome."
        );
    }

    // -----------------------------------------------------------------
    // `pre_write_status_check` — Story 4.1.1's pure guard (Task 4.1.1a).
    // -----------------------------------------------------------------

    #[test]
    fn pre_write_status_check_should_return_still_pending_when_cached_status_is_pending() {
        let rows = vec![cached_row("1", Some("pending"), Some(78))];
        assert_eq!(pre_write_status_check(&rows, "1"), PreWriteCheck::StillPending);
    }

    #[test]
    fn pre_write_status_check_should_return_already_resolved_when_cached_status_is_terminal() {
        let rows = vec![cached_row("1", Some("complete"), Some(78))];
        assert_eq!(
            pre_write_status_check(&rows, "1"),
            PreWriteCheck::AlreadyResolved("complete".to_string())
        );
    }

    #[test]
    fn pre_write_status_check_should_return_not_found_when_transaction_id_absent_from_cache() {
        let rows = vec![cached_row("2", Some("pending"), Some(78))];
        assert_eq!(pre_write_status_check(&rows, "1"), PreWriteCheck::NotFound);
    }

    // -----------------------------------------------------------------
    // `post_write_verified` — action-aware post-write confirmation guard.
    // -----------------------------------------------------------------

    #[test]
    fn post_write_verified_should_confirm_cancel_when_cached_status_is_no_longer_pending() {
        let rows = vec![cached_row("1", Some("cancelled"), Some(78))];
        assert_eq!(
            post_write_verified(&rows, "1", EditCancelAction::Cancel),
            Some("cancelled".to_string())
        );
    }

    #[test]
    fn post_write_verified_should_not_confirm_cancel_when_cached_status_is_still_pending() {
        let rows = vec![cached_row("1", Some("pending"), Some(78))];
        assert_eq!(post_write_verified(&rows, "1", EditCancelAction::Cancel), None);
    }

    #[test]
    fn post_write_verified_should_confirm_edit_bid_when_cached_bid_matches_new_amount() {
        let rows = vec![cached_row("1", Some("pending"), Some(85))];
        assert_eq!(
            post_write_verified(&rows, "1", EditCancelAction::EditBid(85)),
            Some("85".to_string())
        );
    }

    #[test]
    fn post_write_verified_should_not_confirm_edit_bid_when_cached_bid_does_not_match() {
        let rows = vec![cached_row("1", Some("pending"), Some(78))];
        assert_eq!(post_write_verified(&rows, "1", EditCancelAction::EditBid(85)), None);
    }

    #[test]
    fn post_write_verified_should_return_none_when_transaction_id_absent_from_cache() {
        let rows = vec![cached_row("2", Some("cancelled"), None)];
        assert_eq!(post_write_verified(&rows, "1", EditCancelAction::Cancel), None);
    }

    // -----------------------------------------------------------------
    // `run_edit_cancel_claim` — the already-resolved short-circuit, proven
    // end-to-end against a real (temp-file) `StatsDb`/`AnalysisService`,
    // with NO browser/Chrome involved: seeding the cache with a terminal
    // (`complete`) status makes `read_through`'s own `needs_refresh` gate
    // report no refresh needed (every cached row is terminal — see
    // `transactions.rs`'s `needs_refresh`), so the pre-write re-check reads
    // straight from the cache and the function returns `Err` from the
    // `PreWriteCheck::AlreadyResolved` branch BEFORE `launch_session` is
    // ever called — there is no code path from there to a browser launch,
    // so this is a structural proof, not an assumption about timing.
    //
    // The complementary "still pending -> proceeds to the live seam flow"
    // half of Story 4.1.1 is NOT covered by an equivalent end-to-end test:
    // a `pending` cached row is not terminal, so `read_through`'s
    // `AlwaysLive` tolerance *would* trigger a real browser launch via
    // `run_league_transactions_sync` — the same documented gap
    // `run_claim`/`run_lineup_set` (`actions.rs`) and
    // `run_pending_transactions`/`run_league_transactions_sync`
    // (`transactions.rs`) already carry: no live Chrome in this sandbox.
    // `pre_write_status_check`'s own `StillPending` branch is proven
    // directly above instead, and the seam flow it hands off to
    // (`edit_cancel_claim_via_seam`) is independently proven end-to-end
    // against `FakeSession` in `tests/browser_edit_cancel.rs`.
    // -----------------------------------------------------------------

    async fn test_analysis_service(name: &str) -> (Config, AnalysisService) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data_dir = std::env::temp_dir().join(format!(
            "sleeper_edit_cancel_test_{name}_{}_{nanos}",
            std::process::id()
        ));
        let config = Config { data_dir, ..Config::default() };
        let client = SleeperClient::new(&config).await.expect("build client (no network at construction)");
        let db = StatsDb::open(&config.stats_db_path()).await.expect("open test db");
        (config.clone(), AnalysisService::new(client, db))
    }

    const LEAGUE_ID: &str = "1352883872131481600";

    #[tokio::test]
    async fn run_edit_cancel_claim_should_short_circuit_with_resolved_triage_and_no_browser_launch_when_already_resolved(
    ) {
        let (config, analysis) = test_analysis_service("already_resolved").await;
        let adds = serde_json::json!([]);
        let drops = serde_json::json!([]);
        let roster_ids = serde_json::json!([5]);
        analysis
            .db
            .upsert_transaction(TransactionRow {
                transaction_id: "1",
                league_id: LEAGUE_ID,
                season: "2026",
                week: 9,
                kind: "waiver",
                status: Some("complete"),
                created: 1_781_000_000,
                adds: &adds,
                drops: &drops,
                roster_ids: &roster_ids,
                waiver_bid: Some(78),
                status_updated: Some(1_781_850_000),
                source: "graphql",
            })
            .await
            .expect("seed resolved transaction");

        let err = run_edit_cancel_claim(&config, &analysis, LEAGUE_ID, "1", EditCancelAction::Cancel)
            .await
            .expect_err("an already-resolved claim must be Err, never a DOM write attempt");

        assert_eq!(
            err.to_string(),
            edit_cancel_already_resolved_triage("1", LEAGUE_ID, "complete"),
            "triage text must match ux.md §4c/§11's exact pattern"
        );
        // No code path above reaches `launch_session` before this return —
        // structurally, not just observably, zero browser launches.
    }
}
