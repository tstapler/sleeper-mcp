//! Epic 2.2 / Story 2.2.2: whole-league GraphQL transaction backfill. One
//! `league_transactions_via_seam` round-trip, persisted through the same
//! `upsert_transaction`/`resolve_side` enrichment the REST sync
//! (`AnalysisService::sync_transaction_history`) already uses, plus a
//! `transaction_sync_runs(roster_scope='all')` coverage-ledger row so
//! `has_full_league_graphql_coverage` can tell a real backfill apart from
//! "never ran."
//!
//! This file does NOT yet implement `StalenessTolerance`/`read_through`
//! (Epic 2.1) — those land separately and will extend this same file.
use std::collections::HashMap;

use crate::analysis::{resolve_side, AnalysisService};
use crate::browser::BrowserSession;
use crate::config::Config;
use crate::db::{CachedTransaction, StatsDb, TransactionRow, TransactionSyncRun};
use crate::error::Result;

use super::login::{
    debug_detail, launch_failure_error, launch_session, triage_error, LaunchOptions,
};
use super::pending::{parse_league_transactions, LeagueTransaction};

/// Failure triage (stderr-only, mirrors `pending.rs`'s `pending_failed_triage`):
/// raw detail rides behind `SLEEPER_BROWSER_DEBUG`.
fn league_transactions_failed_triage() -> String {
    "Whole-league transaction backfill failed — the Sleeper GraphQL schema may have changed. Re-run with SLEEPER_BROWSER_DEBUG=1 for detail.".to_string()
}

/// Whether persisting `entry` would actually change the cached row —
/// computed by comparing against the row already in `StatsDb` (read via the
/// existing `list_transactions` API) rather than trusting SQLite's own
/// `ON CONFLICT DO UPDATE` affected-row count, which `upsert_transaction`
/// doesn't expose and which SQLite increments even for a no-op write to an
/// unchanged row. A transaction absent from the cache always counts as
/// changed. Pure/no I/O — the caller does the one `list_transactions` read.
fn transaction_would_change(
    current: Option<&CachedTransaction>,
    entry: &LeagueTransaction,
    adds: &serde_json::Value,
    drops: &serde_json::Value,
    roster_ids: &serde_json::Value,
) -> bool {
    match current {
        None => true,
        Some(c) => {
            c.week != entry.leg as i32
                || c.kind != entry.kind
                || c.status.as_deref() != Some(entry.status.as_str())
                || c.created != entry.created
                || c.adds != *adds
                || c.drops != *drops
                || c.roster_ids != *roster_ids
                || c.waiver_bid != entry.waiver_bid
        }
    }
}

/// `roster_id -> display name`, the same way `sync_transaction_history`
/// resolves it (`roster.rs:167-178`) — display_name, falling back to
/// username, falling back to a synthetic label.
fn owner_names_by_roster(members: Vec<(i32, String, Option<String>, Option<String>)>) -> HashMap<i32, String> {
    members
        .into_iter()
        .map(|(roster_id, _user_id, username, display_name)| {
            (roster_id, display_name.or(username).unwrap_or_else(|| format!("roster {roster_id}")))
        })
        .collect()
}

/// Everything [`persist_one_entry`] needs besides the entry itself —
/// grouped into a struct rather than positional args since a call site
/// can't keep six independent references straight by position (same
/// rationale as `db.rs`'s `RosterSnapshotRow`/`TransactionRow`).
struct SyncContext<'a> {
    db: &'a StatsDb,
    league_id: &'a str,
    season: &'a str,
    all_players: &'a HashMap<String, serde_json::Value>,
    owner_by_roster: &'a HashMap<i32, String>,
    existing_by_id: &'a HashMap<String, CachedTransaction>,
}

/// Resolves one entry's `adds`/`drops` via the shared `resolve_side` (Task
/// 2.2.1c), upserts it, and reports whether the write actually changed
/// anything (for the caller's `rows_changed` tally).
async fn persist_one_entry(ctx: &SyncContext<'_>, entry: &LeagueTransaction) -> Result<bool> {
    let adds = resolve_side(&entry.raw_adds, ctx.all_players, ctx.owner_by_roster);
    let drops = resolve_side(&entry.raw_drops, ctx.all_players, ctx.owner_by_roster);
    let roster_ids_json = serde_json::json!(entry.roster_ids);
    let existing = ctx.existing_by_id.get(&entry.transaction_id);
    let changed = transaction_would_change(existing, entry, &adds, &drops, &roster_ids_json);

    ctx.db
        .upsert_transaction(TransactionRow {
            transaction_id: &entry.transaction_id,
            league_id: ctx.league_id,
            season: ctx.season,
            week: entry.leg as i32,
            kind: &entry.kind,
            status: Some(entry.status.as_str()),
            created: entry.created,
            adds: &adds,
            drops: &drops,
            roster_ids: &roster_ids_json,
            waiver_bid: entry.waiver_bid,
            status_updated: entry.status_updated,
            source: "graphql",
        })
        .await?;
    Ok(changed)
}

/// Task 2.2.2a: resolves and persists every parsed entry (via
/// [`persist_one_entry`]) through `db` alone, then records exactly one
/// `transaction_sync_runs(roster_scope='all')` row. Deliberately takes the
/// already-fetched `season`/`all_players`/`owner_by_roster` as plain data
/// rather than an `AnalysisService` + league_id it would fetch them from
/// itself — the only network/browser dependency in this whole module lives
/// in [`fetch_league_transactions`]/[`run_league_transactions_sync`], so
/// this function (the one Story 2.2.2's persistence/idempotency ACs are
/// actually about) is testable against a temp `StatsDb` with zero network
/// calls. Returns `(rows_fetched, rows_changed)`.
// kibitzer:ignore long-function -- snapshot the pre-write cache, loop
// (delegated to persist_one_entry), record the ledger row: one atomic
// persistence operation; splitting further would fragment it across more
// indirection than it saves.
// kibitzer:ignore long-parameter-list -- each param is already-fetched,
// independent context (db handle, scope, and three lookup maps); wrapping
// them in a struct wouldn't reduce what a caller/test needs to supply, only
// add a layer of indirection around data this function's own tests build
// fresh each time.
pub(crate) async fn persist_league_transactions(
    db: &StatsDb,
    league_id: &str,
    season: &str,
    all_players: &HashMap<String, serde_json::Value>,
    owner_by_roster: &HashMap<i32, String>,
    entries: &[LeagueTransaction],
) -> Result<(usize, usize)> {
    // Snapshot the pre-write cache state once (not per-entry) so idempotency
    // is measured against what was actually stored before this sync run,
    // not re-queried mid-loop against rows this same run just wrote.
    let existing_by_id: HashMap<String, CachedTransaction> = db
        .list_transactions(league_id, None)
        .await?
        .into_iter()
        .map(|t| (t.transaction_id.clone(), t))
        .collect();

    let ctx = SyncContext {
        db,
        league_id,
        season,
        all_players,
        owner_by_roster,
        existing_by_id: &existing_by_id,
    };

    let rows_fetched = entries.len();
    let mut rows_changed = 0usize;
    for entry in entries {
        if persist_one_entry(&ctx, entry).await? {
            rows_changed += 1;
        }
    }

    db.record_sync_run(TransactionSyncRun {
        league_id,
        season,
        source: "graphql",
        roster_scope: "all",
        synced_at: chrono::Utc::now(),
        rows_fetched: rows_fetched as i64,
        rows_changed: rows_changed as i64,
    })
    .await?;

    Ok((rows_fetched, rows_changed))
}

/// Task 2.2.2a: calls `league_transactions_via_seam`, closes `session` on
/// every branch (never `?` before close — mirrors
/// `pending.rs::run_pending_transactions`'s discipline exactly), and
/// returns the parsed entries on success. Isolated from
/// `run_league_transactions_sync`'s real `launch_session` so this leg is
/// unit-testable against `FakeSession` without a live Chrome
/// (`launch_session` itself has no seam to fake — the same existing gap
/// `run_pending_transactions`/`run_claim` have today, not new here).
async fn fetch_league_transactions(
    session: &mut dyn BrowserSession,
    league_id: &str,
) -> Result<Vec<LeagueTransaction>> {
    use crate::browser::session::{league_transactions_via_seam, DEAD_SESSION_HINT};

    let outcome = league_transactions_via_seam(session, league_id, &[]).await;
    session.close().await?;
    let raw = match outcome {
        Ok(raw) => raw,
        Err(e) if e.to_string() == DEAD_SESSION_HINT => return Err(e),
        Err(e) => {
            return Err(crate::browser::other(triage_error(
                &league_transactions_failed_triage(),
                &debug_detail(&e.to_string()),
            )))
        }
    };
    parse_league_transactions(&raw)
}

/// Task 2.2.2a core: launch → [`fetch_league_transactions`] (seam call +
/// close-on-every-branch + parse) → close the launched-browser process
/// guard on every branch too → on success, fetch the enrichment context
/// (`all_players`/`owner_by_roster`, the same way
/// `AnalysisService::sync_transaction_history` already does) and persist
/// via [`persist_league_transactions`]. Returns `(rows_fetched, rows_changed)`.
pub(crate) async fn run_league_transactions_sync(
    config: &Config,
    analysis: &AnalysisService,
    league_id: &str,
) -> Result<(usize, usize)> {
    let (mut session, launched) = match launch_session(config, LaunchOptions::headless()).await {
        Ok(pair) => pair,
        Err(e) => return Err(launch_failure_error(&e)),
    };
    let result = fetch_league_transactions(&mut *session, league_id).await;
    launched.close().await?;
    let entries = result?;

    let season = analysis.client.get_league(league_id).await?.season;
    let all_players = analysis.client.get_all_players("nfl").await?;
    let members = analysis.db.list_league_members(league_id).await?;
    let owner_by_roster = owner_names_by_roster(members);

    persist_league_transactions(&analysis.db, league_id, &season, &all_players, &owner_by_roster, &entries)
        .await
}

#[cfg(test)]
mod tests {
    use super::transaction_would_change;
    use crate::db::CachedTransaction;

    use super::super::pending::LeagueTransaction;

    fn entry() -> LeagueTransaction {
        LeagueTransaction {
            transaction_id: "1".to_string(),
            kind: "waiver".to_string(),
            status: "pending".to_string(),
            leg: 9,
            created: 1_781_000_000,
            waiver_bid: Some(78),
            status_updated: Some(1_781_850_000),
            roster_ids: vec![5],
            adds: vec!["Jacoby Brissett (QB - IND)".to_string()],
            drops: vec![],
            raw_adds: [("3257".to_string(), 5)].into_iter().collect(),
            raw_drops: Default::default(),
        }
    }

    fn resolved_adds() -> serde_json::Value {
        serde_json::json!([{"player_id":"3257","name":"Jacoby Brissett","position":"QB","roster_id":5,"owner":"Tyler"}])
    }

    /// A transaction absent from the cache always counts as changed.
    #[test]
    fn transaction_would_change_should_return_true_when_no_current_row_exists() {
        let e = entry();
        let empty = serde_json::json!([]);
        let roster_ids = serde_json::json!([5]);
        assert!(transaction_would_change(None, &e, &resolved_adds(), &empty, &roster_ids));
    }

    /// Idempotency proof (Story 2.2.2 AC): a current row with identical
    /// fields must report no change, so a re-run's ledger `rows_changed`
    /// can be `0`.
    #[test]
    fn transaction_would_change_should_return_false_when_current_row_matches_exactly() {
        let e = entry();
        let adds = resolved_adds();
        let drops = serde_json::json!([]);
        let roster_ids = serde_json::json!([5]);
        let current = CachedTransaction {
            transaction_id: e.transaction_id.clone(),
            season: "2026".to_string(),
            week: e.leg as i32,
            kind: e.kind.clone(),
            status: Some(e.status.clone()),
            created: e.created,
            adds: adds.clone(),
            drops: drops.clone(),
            roster_ids: roster_ids.clone(),
            waiver_bid: e.waiver_bid,
        };
        assert!(!transaction_would_change(Some(&current), &e, &adds, &drops, &roster_ids));
    }

    /// A changed field (here: status) must still be detected as a change.
    #[test]
    fn transaction_would_change_should_return_true_when_status_differs() {
        let e = entry();
        let adds = resolved_adds();
        let drops = serde_json::json!([]);
        let roster_ids = serde_json::json!([5]);
        let current = CachedTransaction {
            transaction_id: e.transaction_id.clone(),
            season: "2026".to_string(),
            week: e.leg as i32,
            kind: e.kind.clone(),
            status: Some("complete".to_string()),
            created: e.created,
            adds: adds.clone(),
            drops: drops.clone(),
            roster_ids: roster_ids.clone(),
            waiver_bid: e.waiver_bid,
        };
        assert!(transaction_would_change(Some(&current), &e, &adds, &drops, &roster_ids));
    }

    // -------------------------------------------------------------------
    // Story 2.2.2 AC: persist_league_transactions against a real temp
    // StatsDb (no network — `season`/`all_players`/`owner_by_roster` are
    // supplied as fixtures, matching how `run_league_transactions_sync`
    // splits the network leg from the persistence leg above).
    // -------------------------------------------------------------------

    use super::{fetch_league_transactions, persist_league_transactions};
    use crate::browser::session::FakeSession;
    use crate::browser::BrowserSession;
    use crate::db::StatsDb;
    use std::collections::HashMap;

    const LEAGUE_ID: &str = "1352883872131481600";

    /// `StatsDb::open` takes a file path (no in-memory mode, same as
    /// `db.rs`'s own test helper) — each test gets its own scratch file.
    async fn test_db(name: &str) -> StatsDb {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "sleeper_transactions_test_{name}_{}_{nanos}.db",
            std::process::id()
        ));
        StatsDb::open(&path).await.expect("open test db")
    }

    fn fixture_all_players() -> HashMap<String, serde_json::Value> {
        let mut m = HashMap::new();
        m.insert(
            "3257".to_string(),
            serde_json::json!({"full_name": "Jacoby Brissett", "position": "QB", "team": "IND"}),
        );
        m
    }

    fn fixture_owner_by_roster() -> HashMap<i32, String> {
        let mut m = HashMap::new();
        m.insert(5, "Tyler".to_string());
        m
    }

    fn fixture_entries() -> Vec<LeagueTransaction> {
        vec![entry(), {
            let mut e = entry();
            e.transaction_id = "2".to_string();
            e.raw_adds = Default::default();
            e.raw_drops = [("3257".to_string(), 5)].into_iter().collect();
            e.adds = vec![];
            e.drops = vec!["Jacoby Brissett (QB - IND)".to_string()];
            e
        }]
    }

    /// Story 2.2.2 AC, happy path: a successful backfill persists every
    /// transaction (both fixture entries, each as a `≤N`-rows-changed
    /// write) and records exactly one `transaction_sync_runs` coverage row
    /// (`source='graphql', roster_scope='all'`) — proven via
    /// `has_full_league_graphql_coverage` rather than a schema-private row
    /// count, since `StatsDb`'s public API has no "count ledger rows"
    /// method and this test deliberately stays off `src/db.rs`.
    #[tokio::test]
    async fn persist_league_transactions_should_persist_rows_and_record_coverage_when_backfill_succeeds(
    ) {
        let db = test_db("persist_happy_path").await;
        let entries = fixture_entries();

        assert!(
            db.has_full_league_graphql_coverage(LEAGUE_ID, "2026").await.expect("query coverage").is_none(),
            "coverage must be absent before any sync run"
        );

        let (rows_fetched, rows_changed) = persist_league_transactions(
            &db,
            LEAGUE_ID,
            "2026",
            &fixture_all_players(),
            &fixture_owner_by_roster(),
            &entries,
        )
        .await
        .expect("first backfill must succeed");

        assert_eq!(rows_fetched, 2);
        assert_eq!(rows_changed, 2, "both brand-new rows must count as changed");

        let cached = db.list_transactions(LEAGUE_ID, None).await.expect("list cached rows");
        assert_eq!(cached.len(), 2, "every parsed transaction must be persisted");
        let brissett_add = cached.iter().find(|t| t.transaction_id == "1").expect("txn 1 persisted");
        assert_eq!(
            brissett_add.adds[0]["position"],
            serde_json::json!("QB"),
            "persisted adds must carry the enriched {{player_id, name, position, roster_id, owner}} shape, got {:?}",
            brissett_add.adds
        );

        assert!(
            db.has_full_league_graphql_coverage(LEAGUE_ID, "2026").await.expect("query coverage").is_some(),
            "a graphql/all coverage row must exist after the backfill"
        );
    }

    /// Story 2.2.2 AC, idempotency: re-running the identical backfill
    /// immediately after must report `rows_changed == 0` — run twice and
    /// check, not asserted from a single run (the first run alone can't
    /// distinguish "idempotent" from "ran once").
    #[tokio::test]
    async fn persist_league_transactions_should_report_zero_rows_changed_when_rerun_immediately_with_same_data(
    ) {
        let db = test_db("persist_idempotent").await;
        let entries = fixture_entries();
        let all_players = fixture_all_players();
        let owner_by_roster = fixture_owner_by_roster();

        let (_, first_changed) =
            persist_league_transactions(&db, LEAGUE_ID, "2026", &all_players, &owner_by_roster, &entries)
                .await
                .expect("first run must succeed");
        assert_eq!(first_changed, 2);

        let (second_fetched, second_changed) =
            persist_league_transactions(&db, LEAGUE_ID, "2026", &all_players, &owner_by_roster, &entries)
                .await
                .expect("second run must succeed");
        assert_eq!(second_fetched, 2, "the same entries are still fetched/considered");
        assert_eq!(second_changed, 0, "an immediate identical re-run must change nothing");
    }

    /// Story 2.2.2 AC, error path: a malformed-JSON parse failure must
    /// still close the session before the `Err` propagates — asserted via
    /// a second `close()` call returning "browser already closed" (proof
    /// the first close already ran), mirroring how `fake.rs`'s own
    /// `close_should_return_error_when_called_twice_on_fake_session` test
    /// proves the same thing for `BrowserSession::close` directly.
    #[tokio::test]
    async fn fetch_league_transactions_should_close_session_when_parse_fails_on_malformed_json() {
        let mut session = FakeSession::with_eval_string_response("not valid json at all");
        let err = fetch_league_transactions(&mut session, LEAGUE_ID)
            .await
            .expect_err("malformed JSON must surface as a parse Err");
        assert!(!err.to_string().is_empty());

        let close_err = session
            .close()
            .await
            .expect_err("session must already be closed by fetch_league_transactions");
        assert!(
            close_err.to_string().contains("already closed"),
            "got {close_err}"
        );
    }

    /// Happy path (sanity check alongside the error path above): a
    /// logged-in session with a valid fixture payload parses successfully
    /// and still closes the session.
    #[tokio::test]
    async fn fetch_league_transactions_should_return_entries_and_close_session_when_payload_valid() {
        let fixture = r#"{"data":{"league_transactions_filtered":[
            {"transaction_id":"1","status":"pending","type":"waiver","leg":9,"created":1,
             "status_updated":1781850000,"roster_ids":[5],
             "settings":{"waiver_bid":78},"adds":{"3257":5},"drops":{},
             "player_map":{"3257":{"first_name":"Jacoby","last_name":"Brissett","position":"QB","team":"IND"}}}
        ]}}"#;
        let mut session = FakeSession::with_eval_string_response(fixture);
        let entries = fetch_league_transactions(&mut session, LEAGUE_ID)
            .await
            .expect("valid payload must parse");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].transaction_id, "1");

        let close_err = session.close().await.expect_err("session must already be closed");
        assert!(close_err.to_string().contains("already closed"));
    }
}
