// kibitzer:ignore file-size -- Epic 2.1's task explicitly directed adding
// StalenessTolerance/default_tolerance_for/read_through to this existing
// Epic 2.2 file rather than a new one ("add to it, do not recreate it");
// the implementing agent's file scope for this task was fixed to src/db.rs
// and this file only, so a split is deferred to a future task.
//! Epic 2.2 / Story 2.2.2: whole-league GraphQL transaction backfill. One
//! `league_transactions_via_seam` round-trip, persisted through the same
//! `upsert_transaction`/`resolve_side` enrichment the REST sync
//! (`AnalysisService::sync_transaction_history`) already uses, plus a
//! `transaction_sync_runs(roster_scope='all')` coverage-ledger row so
//! `has_full_league_graphql_coverage` can tell a real backfill apart from
//! "never ran."
//!
//! Epic 2.1: `StalenessTolerance`/`default_tolerance_for`/`read_through`
//! below are the cache-aside read API that dispatches to the backfill
//! above (browser/GraphQL) or to `AnalysisService::sync_transaction_history`
//! (REST), per kind — see `read_through`'s doc comment for the decision
//! table.
use std::collections::HashMap;

use chrono::Datelike;

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

// ---------------------------------------------------------------------------
// Epic 2.1: staleness-tolerance type and cache read API
// ---------------------------------------------------------------------------

/// Task 2.1.1b: how stale an answer a `read_through` caller will accept,
/// decided at read time — contrast `HttpCache`'s write-time TTL, which is
/// the other freshness idiom already in this codebase (Step 0.5 fork #1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StalenessTolerance {
    /// Never trust the cache — every read triggers a refresh, no matter how
    /// recently the row was fetched.
    AlwaysLive,
    /// Trust the cache up to this age; past it, refresh.
    MaxAge(chrono::Duration),
    /// Trust the cache no matter how old — never makes a live call.
    CachedOnly,
}

impl StalenessTolerance {
    /// Whether a row fetched at `fetched_at` is still fresh enough, as of
    /// `now`, to skip a live refresh. The one comparison every `read_through`
    /// call site needs.
    pub(crate) fn is_fresh(
        &self,
        fetched_at: chrono::DateTime<chrono::Utc>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        match self {
            StalenessTolerance::AlwaysLive => false,
            StalenessTolerance::CachedOnly => true,
            StalenessTolerance::MaxAge(max_age) => now - fetched_at < *max_age,
        }
    }
}

/// Sleeper's `waiver_day_of_week` setting is `0=Sunday .. 6=Saturday`
/// (verified against plan.md's own example: `waiver_day_of_week=2` means
/// Tuesday) — chrono's `Weekday` has no such numbering built in, so this
/// maps explicitly rather than guessing at an offset.
fn is_waiver_day(now: chrono::DateTime<chrono::Utc>, waiver_day_of_week: u8) -> bool {
    let sleeper_day = match now.weekday() {
        chrono::Weekday::Sun => 0,
        chrono::Weekday::Mon => 1,
        chrono::Weekday::Tue => 2,
        chrono::Weekday::Wed => 3,
        chrono::Weekday::Thu => 4,
        chrono::Weekday::Fri => 5,
        chrono::Weekday::Sat => 6,
    };
    sleeper_day == waiver_day_of_week
}

/// Task 2.1.1c: the Tuesday-window-aware default a caller gets when it
/// supplies no explicit tolerance (requirements.md's resolved Open
/// Question; ux.md §7). Pure — no DB/browser I/O — so it's unit-testable
/// directly without a database or browser. Terminal statuses are
/// cache-valid forever regardless of day; a `pending` row gets `AlwaysLive`
/// only on the league's actual waiver-processing day (or always, for
/// daily-waiver leagues) and a long-but-not-infinite `MaxAge` otherwise.
///
/// Wiring this into CLI/MCP call sites that supply no explicit tolerance is
/// Epic 2.3's job (Tasks 2.3.1a/2.3.2a), not this one's.
// kibitzer:ignore flag-argument -- signature fixed by plan.md Task 2.1.1c /
// architecture.md §3: `daily_waivers` mirrors the Sleeper league-settings
// field verbatim, matching the plan's AC examples exactly.
pub(crate) fn default_tolerance_for(
    status: &str,
    now: chrono::DateTime<chrono::Utc>,
    waiver_day_of_week: u8,
    daily_waivers: bool,
) -> StalenessTolerance {
    if matches!(status, "complete" | "rejected" | "cancelled") {
        return StalenessTolerance::CachedOnly;
    }
    if daily_waivers || is_waiver_day(now, waiver_day_of_week) {
        StalenessTolerance::AlwaysLive
    } else {
        StalenessTolerance::MaxAge(chrono::Duration::hours(24))
    }
}

/// Which backend `read_through` dispatches a triggered refresh to — the
/// Strategy-pattern choice named in Step 0.5 fork #1, made concrete by the
/// decision table on [`read_through`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Populator {
    Rest,
    Browser,
}

/// Task 2.1.2a's populator-dispatch decision, pulled out as its own pure
/// function (no I/O) so the one case that matters — `free_agent` goes to
/// REST, everything else goes to the browser — is unit-testable without a
/// network or a browser. See [`read_through`]'s decision table for the why.
fn populator_for(kind_filter: Option<&str>) -> Populator {
    match kind_filter {
        Some("free_agent") => Populator::Rest,
        _ => Populator::Browser,
    }
}

/// Whether `status` is a terminal (resolved) transaction status — cache-valid
/// forever, per ux.md §1, regardless of any `StalenessTolerance`.
fn is_terminal_status(status: Option<&str>) -> bool {
    matches!(status, Some("complete") | Some("rejected") | Some("cancelled"))
}

/// Task 2.1.2a's refresh decision, pulled out as its own pure function (no
/// I/O) so each of Story 2.1.2's four acceptance criteria can be proven
/// directly against it, without needing a live browser or network call to
/// exercise `read_through` end-to-end:
/// - Terminal-status rows, if *every* cached row is terminal, bypass
///   `tolerance` entirely and never trigger a refresh — not even on
///   `force_refresh` (the decision table's "Never" has no exception for it;
///   a resolved transaction has nothing left to re-fetch).
/// - Otherwise (a `pending` row is present, or there's no row at all),
///   `tolerance`/`force_refresh` decide: `CachedOnly` never refreshes,
///   `force_refresh` or an empty cache always does, and otherwise any
///   non-terminal row that `tolerance.is_fresh` rejects does.
fn needs_refresh(
    cached: &[CachedTransaction],
    tolerance: &StalenessTolerance,
    force_refresh: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let all_terminal = !cached.is_empty() && cached.iter().all(|t| is_terminal_status(t.status.as_deref()));
    if all_terminal {
        return false;
    }

    match tolerance {
        StalenessTolerance::CachedOnly => false,
        _ => {
            force_refresh
                || cached.is_empty()
                || cached
                    .iter()
                    .filter(|t| !is_terminal_status(t.status.as_deref()))
                    .any(|t| !tolerance.is_fresh(t.fetched_at, now))
        }
    }
}

/// Task 2.1.2a: the cache-aside read-through entry point for transaction
/// history. Reads the cache once; if [`needs_refresh`] says a live call is
/// warranted, dispatches to exactly one populator per [`populator_for`]'s
/// decision table, then re-reads the cache before returning (every live
/// read refreshes the cache for the next reader):
///
/// | Row state after cache lookup | `kind_filter` | Refresh? | Populator |
/// |---|---|---|---|
/// | Terminal status present | any | Never | — |
/// | `pending`/no row, `Some("free_agent")` | — | Per tolerance/force | REST (`sync_transaction_history`) |
/// | `pending`/no row, other/`None` | — | Per tolerance/force | browser (`run_league_transactions_sync`) |
///
/// REST is sufficient for `free_agent` because those pickups never carry a
/// FAAB bid and Sleeper processes them immediately (never `pending`) — see
/// plan.md's "Honest note on reachability" for why the REST populator is
/// otherwise unreached by this dispatch.
// kibitzer:ignore long-function -- cache read, needs_refresh gate,
// populator dispatch, cache re-read: one atomic read-through operation
// (plan.md Task 2.1.2a); splitting further would fragment it across more
// indirection than it saves, the same rationale as this file's existing
// persist_league_transactions.
// kibitzer:ignore long-parameter-list -- signature fixed verbatim by
// plan.md Task 2.1.2a / architecture.md §3.
// kibitzer:ignore flag-argument -- `force_refresh` is part of the same
// plan-mandated signature; it is the CLI's `--force` flag passed straight
// through, not an internal branch this function invented.
pub(crate) async fn read_through(
    config: &Config,
    analysis: &AnalysisService,
    league_id: &str,
    kind_filter: Option<&str>,
    tolerance: StalenessTolerance,
    force_refresh: bool,
) -> Result<Vec<CachedTransaction>> {
    let cached = analysis.db.list_transactions(league_id, kind_filter).await?;
    let now = chrono::Utc::now();

    if !needs_refresh(&cached, &tolerance, force_refresh, now) {
        if cached.is_empty() {
            tracing::debug!(
                %league_id,
                ?kind_filter,
                "transactions.read_through: cache_miss, no_live_call (CachedOnly)"
            );
        }
        return Ok(cached);
    }

    let outcome = if force_refresh {
        "force_refresh"
    } else if cached.is_empty() {
        "cache_miss"
    } else {
        "stale_per_tolerance"
    };
    let _span = tracing::info_span!(
        "transactions.read_through",
        %league_id,
        ?kind_filter,
        outcome,
    )
    .entered();

    match populator_for(kind_filter) {
        Populator::Rest => {
            // free_agent only: REST's existing data is already sufficient
            // (see the decision table above), scoped to every week synced
            // so far this season, same range `cmd_sync` uses.
            let nfl_state = analysis.client.get_nfl_state().await?;
            let weeks: Vec<i32> = (1..=nfl_state.week.max(1)).collect();
            analysis.sync_transaction_history(league_id, &weeks).await?;
        }
        Populator::Browser => {
            run_league_transactions_sync(config, analysis, league_id).await?;
        }
    }

    analysis.db.list_transactions(league_id, kind_filter).await
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
            fetched_at: chrono::Utc::now(),
            status_updated: e.status_updated,
            source: Some("graphql".to_string()),
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
            fetched_at: chrono::Utc::now(),
            status_updated: e.status_updated,
            source: Some("graphql".to_string()),
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

    // -------------------------------------------------------------------
    // Epic 2.1 / Story 2.1.1: StalenessTolerance, default_tolerance_for.
    // -------------------------------------------------------------------

    use super::{default_tolerance_for, is_terminal_status, needs_refresh, populator_for, Populator, StalenessTolerance};

    fn dt(rfc3339: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(rfc3339).unwrap().with_timezone(&chrono::Utc)
    }

    /// Happy path: a row fetched well within the max age is fresh.
    #[test]
    fn staleness_tolerance_is_fresh_should_return_true_when_within_max_age() {
        let tolerance = StalenessTolerance::MaxAge(chrono::Duration::hours(1));
        let now = dt("2026-11-10T12:00:00Z");
        let fetched_at = now - chrono::Duration::minutes(30);
        assert!(tolerance.is_fresh(fetched_at, now));
    }

    /// Boundary path: one second past the max age is no longer fresh.
    #[test]
    fn staleness_tolerance_is_fresh_should_return_false_when_max_age_exceeded() {
        let tolerance = StalenessTolerance::MaxAge(chrono::Duration::hours(1));
        let now = dt("2026-11-10T12:00:00Z");
        let fetched_at = now - chrono::Duration::hours(1) - chrono::Duration::seconds(1);
        assert!(!tolerance.is_fresh(fetched_at, now));
    }

    /// `AlwaysLive` is never fresh, no matter how recent.
    #[test]
    fn staleness_tolerance_is_fresh_should_return_false_for_always_live_even_one_second_old() {
        let now = dt("2026-11-10T12:00:00Z");
        let fetched_at = now - chrono::Duration::seconds(1);
        assert!(!StalenessTolerance::AlwaysLive.is_fresh(fetched_at, now));
    }

    /// `CachedOnly` is always fresh, no matter how old.
    #[test]
    fn staleness_tolerance_is_fresh_should_return_true_for_cached_only_even_ninety_days_old() {
        let now = dt("2026-11-10T12:00:00Z");
        let fetched_at = now - chrono::Duration::days(90);
        assert!(StalenessTolerance::CachedOnly.is_fresh(fetched_at, now));
    }

    /// Terminal statuses always get `CachedOnly`, regardless of day.
    #[test]
    fn default_tolerance_for_should_return_cached_only_for_every_terminal_status() {
        let now = dt("2026-11-10T12:00:00Z"); // a Tuesday
        for status in ["complete", "rejected", "cancelled"] {
            assert_eq!(
                default_tolerance_for(status, now, 2, false),
                StalenessTolerance::CachedOnly,
                "status {status} must be CachedOnly"
            );
        }
    }

    /// Story 2.1.1 AC: pending, on the league's waiver day -> AlwaysLive.
    #[test]
    fn default_tolerance_for_should_return_always_live_when_pending_and_today_is_waiver_day() {
        let tuesday = dt("2026-11-10T12:00:00Z"); // 2026-11-10 is a Tuesday
        assert_eq!(
            default_tolerance_for("pending", tuesday, 2, false),
            StalenessTolerance::AlwaysLive
        );
    }

    /// Story 2.1.1 AC: pending, off the league's waiver day -> a long but
    /// finite MaxAge, not AlwaysLive and not CachedOnly.
    #[test]
    fn default_tolerance_for_should_return_long_max_age_when_pending_and_today_is_not_waiver_day() {
        let wednesday = dt("2026-11-11T12:00:00Z");
        assert_eq!(
            default_tolerance_for("pending", wednesday, 2, false),
            StalenessTolerance::MaxAge(chrono::Duration::hours(24))
        );
    }

    /// `daily_waivers` leagues get AlwaysLive every day, not just the
    /// nominal waiver day.
    #[test]
    fn default_tolerance_for_should_return_always_live_when_daily_waivers_even_off_waiver_day() {
        let wednesday = dt("2026-11-11T12:00:00Z");
        assert_eq!(
            default_tolerance_for("pending", wednesday, 2, true),
            StalenessTolerance::AlwaysLive
        );
    }

    // -------------------------------------------------------------------
    // Epic 2.1 / Story 2.1.2: read_through's populator-dispatch and
    // refresh-decision logic, proven against the pure `populator_for` and
    // `needs_refresh` functions `read_through` calls directly — a full
    // end-to-end exercise of `read_through` for these three cases would
    // require either a live browser (`run_league_transactions_sync`, which
    // has no fake-session seam — the same gap `run_pending_transactions`/
    // `run_claim` already have) or live network (`sync_transaction_history`
    // via a real `SleeperClient`), neither available in this sandboxed test
    // run. The fourth AC (CachedOnly + empty cache) genuinely needs no I/O
    // at all, so it's additionally proven end-to-end below against the
    // real `read_through`.
    // -------------------------------------------------------------------

    fn cached_row(status: &str, fetched_at: chrono::DateTime<chrono::Utc>) -> CachedTransaction {
        CachedTransaction {
            transaction_id: "1".to_string(),
            season: "2026".to_string(),
            week: 9,
            kind: "waiver".to_string(),
            status: Some(status.to_string()),
            created: 1_781_000_000,
            adds: serde_json::json!([]),
            drops: serde_json::json!([]),
            roster_ids: serde_json::json!([5]),
            waiver_bid: Some(78),
            fetched_at,
            status_updated: Some(1_781_850_000),
            source: Some("graphql".to_string()),
        }
    }

    /// Story 2.1.2 AC: an `AlwaysLive` read on a `pending` row always
    /// triggers a refresh, even one fetched a single second ago.
    #[test]
    fn needs_refresh_should_return_true_when_always_live_even_for_recently_cached_pending_row() {
        let now = dt("2026-11-10T12:00:00Z");
        let row = cached_row("pending", now - chrono::Duration::seconds(1));
        assert!(needs_refresh(&[row], &StalenessTolerance::AlwaysLive, false, now));
    }

    /// Story 2.1.2 AC: a `MaxAge` read on a `complete` (terminal) row never
    /// triggers a refresh, no matter how old — terminal rows bypass
    /// `tolerance` entirely.
    #[test]
    fn needs_refresh_should_return_false_when_terminal_status_regardless_of_max_age_tolerance() {
        let now = dt("2026-11-10T12:00:00Z");
        let row = cached_row("complete", now - chrono::Duration::days(90));
        assert!(!needs_refresh(&[row], &StalenessTolerance::MaxAge(chrono::Duration::hours(1)), false, now));
    }

    /// Terminal rows bypass `tolerance` even under `force_refresh` — a
    /// resolved transaction has nothing left to re-fetch (decision table's
    /// "Never" has no force exception).
    #[test]
    fn needs_refresh_should_return_false_when_terminal_status_even_with_force_refresh() {
        let now = dt("2026-11-10T12:00:00Z");
        let row = cached_row("complete", now - chrono::Duration::days(90));
        assert!(!needs_refresh(&[row], &StalenessTolerance::CachedOnly, true, now));
    }

    /// Story 2.1.2 AC (the `CachedOnly` half, expressed on the pure
    /// decision function): an empty cache under `CachedOnly` never
    /// refreshes.
    #[test]
    fn needs_refresh_should_return_false_when_cached_only_and_cache_is_empty() {
        let now = dt("2026-11-10T12:00:00Z");
        assert!(!needs_refresh(&[], &StalenessTolerance::CachedOnly, false, now));
    }

    /// An empty cache under any non-`CachedOnly` tolerance always refreshes
    /// — there's nothing to judge freshness against.
    #[test]
    fn needs_refresh_should_return_true_when_max_age_and_cache_is_empty() {
        let now = dt("2026-11-10T12:00:00Z");
        assert!(needs_refresh(&[], &StalenessTolerance::MaxAge(chrono::Duration::hours(1)), false, now));
    }

    /// Story 2.1.2 AC: `free_agent` dispatches to REST, every other
    /// `kind_filter` (including unfiltered) dispatches to the browser.
    #[test]
    fn populator_for_should_dispatch_to_rest_only_when_kind_filter_is_free_agent() {
        assert_eq!(populator_for(Some("free_agent")), Populator::Rest);
        assert_eq!(populator_for(Some("waiver")), Populator::Browser);
        assert_eq!(populator_for(Some("trade")), Populator::Browser);
        assert_eq!(populator_for(None), Populator::Browser);
    }

    #[test]
    fn is_terminal_status_should_distinguish_terminal_from_pending() {
        assert!(is_terminal_status(Some("complete")));
        assert!(is_terminal_status(Some("rejected")));
        assert!(is_terminal_status(Some("cancelled")));
        assert!(!is_terminal_status(Some("pending")));
        assert!(!is_terminal_status(None));
    }

    // -------------------------------------------------------------------
    // Epic 2.1 / Story 2.1.2: one genuine end-to-end exercise of
    // `read_through` itself — the `CachedOnly` + empty-cache path needs no
    // populator call at all, so it's the one case this sandbox can run
    // against the real function without a browser or network.
    // -------------------------------------------------------------------

    use super::{read_through, AnalysisService};
    use crate::client::SleeperClient;
    use crate::config::Config;

    async fn test_analysis_service(name: &str) -> (Config, AnalysisService) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data_dir = std::env::temp_dir().join(format!(
            "sleeper_read_through_test_{name}_{}_{nanos}",
            std::process::id()
        ));
        let config = Config { data_dir, ..Config::default() };
        let client = SleeperClient::new(&config).await.expect("build client (no network at construction)");
        let db = StatsDb::open(&config.stats_db_path()).await.expect("open test db");
        (config.clone(), AnalysisService::new(client, db))
    }

    #[tokio::test]
    async fn read_through_should_return_empty_with_no_live_call_when_cached_only_and_cache_is_empty() {
        let (config, analysis) = test_analysis_service("cached_only_empty").await;
        let result = read_through(&config, &analysis, LEAGUE_ID, None, StalenessTolerance::CachedOnly, false)
            .await
            .expect("CachedOnly on an empty cache must not error");
        assert!(result.is_empty(), "expected no cached rows, got {result:?}");
    }
}
