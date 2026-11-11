// kibitzer:ignore file-size -- Epic 4.3 added the shared CLI/MCP renderer
// (`render_edit_cancel_outcome`), three new triage fns, and the CLI wrapper
// to the ~300-line non-test core; the rest of the file's growth past 500
// lines is its own `#[cfg(test)]` module (same "one surface, one file"
// convention `mcp.rs`'s own file-size suppression already documents) —
// splitting tests into a second file would scatter, not shrink, this epic's
// footprint.
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
use console::style;

use crate::analysis::AnalysisService;
use crate::browser::session::{EditCancelAction, EditCancelOutcome};
use crate::config::Config;
use crate::db::CachedTransaction;
use crate::error::Result;

use super::login::{debug_detail, launch_failure_error, launch_session, triage_error, LaunchOptions};
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

/// Epic 4.3 / Tasks 4.3.1a/4.3.2a: catch-all triage for a generic flow-level
/// failure (selector/DOM-shape drift — `edit_cancel_claim_via_seam` itself
/// returned `Err` without ever reaching one of `EditCancelOutcome`'s three
/// typed checkpoints), sibling to `claim_failed_triage()`/
/// `lineup_failed_triage()` (`actions.rs:25-33`) and identical in shape.
/// Per the Observability Plan this is never a `sleeper browser doctor`
/// suggestion — selector drift isn't a session/login-health check in the
/// sense `doctor.rs` covers.
fn edit_cancel_failed_triage() -> String {
    "Edit/cancel failed — the live claims panel may have changed since the last verification, or the selector bundle needs live re-verification. Re-run with SLEEPER_BROWSER_DEBUG=1 for detail.".to_string()
}

/// Epic 4.3 / Task 4.2.1b's described (but previously unbuilt) triage fn:
/// the click already fired and the post-click confirmation read couldn't
/// confirm it landed — the outcome is genuinely unknown, so this must never
/// read like a safe-to-retry case. Shared verbatim by both the CLI print
/// and the MCP tool's `Err` text (one copy source, two renderers) — see
/// `render_edit_cancel_outcome` — so it names no CLI- or MCP-specific
/// command, only the concrete recovery action common to both surfaces.
fn edit_cancel_ambiguous_outcome_triage(reason: &str) -> String {
    format!(
        "Edit/cancel outcome unknown ({reason}) — the write may already have landed; do not assume the claim is still active. Re-check its current status (`sleeper transactions`, or the `get_transaction_history`/`get_pending_waivers` MCP tools) before retrying."
    )
}

/// Epic 4.3 / Task 4.2.1c's described (but previously unbuilt) triage fn:
/// the click-stage row-identity re-assertion found the target row had
/// already moved before any click fired — nothing was clicked, so this is a
/// provable fact, never the `Ambiguous` case's genuine unknown. Same
/// shared-copy rationale as `edit_cancel_ambiguous_outcome_triage` above.
fn edit_cancel_aborted_before_write_triage(reason: &str) -> String {
    format!(
        "Edit/cancel aborted before the write could fire ({reason}) — nothing was clicked, the claim is unchanged. Safe to retry immediately."
    )
}

/// Epic 4.1 core result (Task 4.1.1a): everything Epic 4.3's CLI/MCP
/// surface needs to pick its own copy, without re-deriving any decision
/// `run_edit_cancel_claim` already made.
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
    // Fix 1 (Phase 6 review): capture both close results separately and only
    // surface them once `outcome` is known to be `Ok` — a `close()` error
    // must never shadow the seam flow's own, more actionable error.
    let session_close_result = session.close().await;
    let launched_close_result = launched.close().await;
    let outcome = match outcome {
        Ok(o) => {
            session_close_result?;
            launched_close_result?;
            o
        }
        Err(e) if e.to_string() == DEAD_SESSION_HINT => return Err(e),
        Err(e) => {
            return Err(crate::browser::other(triage_error(
                &edit_cancel_failed_triage(),
                &debug_detail(&e.to_string()),
            )))
        }
    };

    match outcome {
        EditCancelOutcome::Confirmed => {
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
        other_outcome => Ok(EditCancelRunOutcome::SeamOutcome(other_outcome)),
    }
}

/// Epic 4.3 / Tasks 4.3.1a & 4.3.2a: the single rendering function shared by
/// the CLI (`cmd_waiver_edit_cancel` below) and the MCP `edit_waiver_claim`/
/// `cancel_waiver_claim` tools (`src/mcp.rs`) — one copy source, two
/// renderers. `Ok` is the success line to print/return verbatim;
/// `Err` is the triage line to print/return verbatim. Both are already
/// fully formed — neither caller does any further string-building.
pub(crate) fn render_edit_cancel_outcome(
    outcome: Result<EditCancelRunOutcome>,
    action: EditCancelAction,
) -> std::result::Result<String, String> {
    match outcome {
        Ok(EditCancelRunOutcome::Confirmed { detail }) => Ok(match action {
            EditCancelAction::Cancel => "claim cancelled".to_string(),
            EditCancelAction::EditBid(_) => format!("claim bid updated to ${detail}"),
        }),
        // The seam flow claimed success but the independent post-write
        // re-check (`AlwaysLive`, the real source of truth) still disagrees
        // — never a bare success print. Practically identical risk to
        // `Ambiguous` (the write may or may not have landed), so it shares
        // that triage.
        Ok(EditCancelRunOutcome::ConfirmedButUnverified) => Err(edit_cancel_ambiguous_outcome_triage(
            "the live DOM reported success but the independent post-write re-check could not confirm it",
        )),
        // Defensive only — per `EditCancelRunOutcome`'s own doc comment this
        // arm shouldn't occur in practice (a real `Confirmed` is intercepted
        // into `Confirmed{detail}`/`ConfirmedButUnverified` above first).
        // Still never a bare success print if it somehow does.
        Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::Confirmed)) => {
            Err(edit_cancel_ambiguous_outcome_triage(
                "the seam flow reported success without going through the post-write re-check (unexpected path)",
            ))
        }
        Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::Ambiguous { reason })) => {
            Err(edit_cancel_ambiguous_outcome_triage(&reason))
        }
        Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::AbortedBeforeWrite { reason })) => {
            Err(edit_cancel_aborted_before_write_triage(&reason))
        }
        // Already ready-to-print `Display` text: the dead-session hint, the
        // already-resolved pre-write triage, or (per `run_edit_cancel_claim`'s
        // own generic `Err` arm above) `edit_cancel_failed_triage()`.
        Err(e) => Err(e.to_string()),
    }
}

/// Epic 4.3 / Task 4.3.1a: CLI wrapper for `WaiverAction::Edit`/`Cancel` —
/// prints the shared render's success line to stdout, or its triage line to
/// stderr (yellow for a dead session, matching the existing Surface C hint
/// `login.rs`/`doctor.rs`/`exit_on_action_error` already use; red for
/// everything else), and exits 1 on any failure branch.
pub(crate) async fn cmd_waiver_edit_cancel(
    config: &Config,
    analysis: &AnalysisService,
    league: &str,
    transaction: &str,
    action: EditCancelAction,
) -> Result<()> {
    use crate::browser::session::DEAD_SESSION_HINT;
    let outcome = run_edit_cancel_claim(config, analysis, league, transaction, action).await;
    match render_edit_cancel_outcome(outcome, action) {
        Ok(success) => {
            println!("{success}");
            Ok(())
        }
        Err(triage) => {
            if triage == DEAD_SESSION_HINT {
                eprintln!("{}", style(&triage).yellow());
            } else {
                eprintln!("{}", style(&triage).red());
            }
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::TransactionRow;
    use crate::test_support::test_analysis_service;

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
    // Epic 4.3's three new triage fns — distinguishable from each other and
    // from `edit_cancel_already_resolved_triage` above (ux.md §11 AC: five
    // distinguishable failure messages, never collapsed into one wording).
    // -----------------------------------------------------------------

    #[test]
    fn edit_cancel_failed_triage_should_match_the_catch_all_shape_verbatim() {
        assert_eq!(
            edit_cancel_failed_triage(),
            "Edit/cancel failed — the live claims panel may have changed since the last verification, or the selector bundle needs live re-verification. Re-run with SLEEPER_BROWSER_DEBUG=1 for detail."
        );
    }

    #[test]
    fn edit_cancel_ambiguous_outcome_triage_should_state_outcome_unknown_and_never_doctor() {
        let msg = edit_cancel_ambiguous_outcome_triage("click fired but confirmation read failed");
        assert!(msg.contains("outcome unknown"), "must state the outcome is unknown: {msg}");
        assert!(msg.contains("do not assume"), "must warn against assuming success: {msg}");
        assert!(!msg.contains("sleeper browser doctor"), "must never suggest doctor: {msg}");
    }

    #[test]
    fn edit_cancel_aborted_before_write_triage_should_state_nothing_changed_and_safe_to_retry() {
        let msg = edit_cancel_aborted_before_write_triage("row identity changed between resolution and click");
        assert!(msg.contains("nothing was clicked"), "must state nothing was clicked: {msg}");
        assert!(msg.contains("Safe to retry"), "must state it's safe to retry: {msg}");
        assert!(!msg.contains("outcome unknown"), "must never use Ambiguous's wording: {msg}");
    }

    #[test]
    fn ambiguous_and_aborted_before_write_triage_should_never_share_wording() {
        let ambiguous = edit_cancel_ambiguous_outcome_triage("same reason text");
        let aborted = edit_cancel_aborted_before_write_triage("same reason text");
        assert_ne!(
            ambiguous, aborted,
            "Ambiguous (risky, re-check before retrying) and AbortedBeforeWrite (safe, nothing changed) \
             must never be rendered with the same copy — collapsing them misrepresents one as the other"
        );
    }

    #[test]
    fn all_three_epic_4_3_triage_fns_should_be_mutually_distinct() {
        let failed = edit_cancel_failed_triage();
        let ambiguous = edit_cancel_ambiguous_outcome_triage("reason");
        let aborted = edit_cancel_aborted_before_write_triage("reason");
        assert_ne!(failed, ambiguous);
        assert_ne!(failed, aborted);
        assert_ne!(ambiguous, aborted);
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

    // -----------------------------------------------------------------
    // `render_edit_cancel_outcome` — Epic 4.3's shared CLI/MCP renderer.
    // Constructed `EditCancelRunOutcome`/`Result` values only, no live
    // launch, per plan.md Tasks 4.3.1a/4.3.2a's own test guidance (same
    // pattern every prior CLI epic used).
    // -----------------------------------------------------------------

    #[test]
    fn render_should_print_claim_cancelled_verbatim_on_confirmed_cancel() {
        let outcome = Ok(EditCancelRunOutcome::Confirmed { detail: "cancelled".to_string() });
        assert_eq!(render_edit_cancel_outcome(outcome, EditCancelAction::Cancel), Ok("claim cancelled".to_string()));
    }

    #[test]
    fn render_should_print_bid_updated_with_new_amount_on_confirmed_edit_bid() {
        let outcome = Ok(EditCancelRunOutcome::Confirmed { detail: "85".to_string() });
        assert_eq!(
            render_edit_cancel_outcome(outcome, EditCancelAction::EditBid(85)),
            Ok("claim bid updated to $85".to_string())
        );
    }

    #[test]
    fn render_should_never_print_success_on_confirmed_but_unverified() {
        let outcome = Ok(EditCancelRunOutcome::ConfirmedButUnverified);
        let rendered = render_edit_cancel_outcome(outcome, EditCancelAction::Cancel);
        let triage = rendered.expect_err("ConfirmedButUnverified must never render as success");
        assert!(triage.contains("outcome unknown"), "must share Ambiguous's triage shape: {triage}");
    }

    #[test]
    fn render_should_never_print_success_on_defensive_seam_confirmed() {
        // Per `EditCancelRunOutcome`'s doc comment this path shouldn't occur
        // in practice (a real `Confirmed` is intercepted above first) — but
        // must still never render as a bare success if it somehow does.
        let outcome = Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::Confirmed));
        let rendered = render_edit_cancel_outcome(outcome, EditCancelAction::Cancel);
        assert!(rendered.is_err(), "defensive SeamOutcome(Confirmed) must never render as success");
    }

    #[test]
    fn render_should_surface_ambiguous_reason_in_the_triage_text() {
        let outcome = Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::Ambiguous {
            reason: "transport error between click and confirmation".to_string(),
        }));
        let triage = render_edit_cancel_outcome(outcome, EditCancelAction::Cancel)
            .expect_err("Ambiguous must never render as success");
        assert!(triage.contains("outcome unknown"));
        assert!(triage.contains("transport error between click and confirmation"));
    }

    #[test]
    fn render_should_surface_aborted_before_write_reason_in_the_triage_text() {
        let outcome = Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::AbortedBeforeWrite {
            reason: "row identity changed between resolution and click".to_string(),
        }));
        let triage = render_edit_cancel_outcome(outcome, EditCancelAction::Cancel)
            .expect_err("AbortedBeforeWrite must never render as success");
        assert!(triage.contains("nothing was clicked"));
        assert!(triage.contains("row identity changed between resolution and click"));
    }

    #[test]
    fn render_should_pass_through_an_already_ready_to_print_err_display_verbatim() {
        let ready_text = edit_cancel_already_resolved_triage("1", LEAGUE_ID, "complete");
        let outcome: Result<EditCancelRunOutcome> = Err(crate::browser::other(ready_text.clone()));
        assert_eq!(render_edit_cancel_outcome(outcome, EditCancelAction::Cancel), Err(ready_text));
    }

    #[test]
    fn ambiguous_and_aborted_before_write_renders_should_never_collapse_to_the_same_text() {
        let same_reason = "row no longer matched by the time the write-stage eval_string ran";
        let ambiguous = render_edit_cancel_outcome(
            Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::Ambiguous { reason: same_reason.to_string() })),
            EditCancelAction::Cancel,
        )
        .unwrap_err();
        let aborted = render_edit_cancel_outcome(
            Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::AbortedBeforeWrite {
                reason: same_reason.to_string(),
            })),
            EditCancelAction::Cancel,
        )
        .unwrap_err();
        assert_ne!(
            ambiguous, aborted,
            "same underlying reason text must still render as genuinely different triage copy \
             for the risky (Ambiguous) vs. safe (AbortedBeforeWrite) case"
        );
    }

    #[test]
    fn only_confirmed_detail_ever_renders_ok() {
        // Every other `EditCancelRunOutcome`/`Err` variant must render `Err`
        // — proof for Task 4.3.2a's "only Confirmed{detail} ever produces
        // Ok" MCP-mapping requirement, at the shared-core level both the
        // CLI and MCP tool call.
        let non_success_outcomes: Vec<Result<EditCancelRunOutcome>> = vec![
            Ok(EditCancelRunOutcome::ConfirmedButUnverified),
            Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::Confirmed)),
            Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::Ambiguous { reason: "r".to_string() })),
            Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::AbortedBeforeWrite { reason: "r".to_string() })),
            Err(crate::browser::other("generic flow failure")),
        ];
        for outcome in non_success_outcomes {
            assert!(render_edit_cancel_outcome(outcome, EditCancelAction::Cancel).is_err());
        }
        assert!(render_edit_cancel_outcome(
            Ok(EditCancelRunOutcome::Confirmed { detail: "cancelled".to_string() }),
            EditCancelAction::Cancel
        )
        .is_ok());
    }
}
