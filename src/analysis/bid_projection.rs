// kibitzer:ignore file-size -- Epic 3.1's full scope (5 signal types + 2
// AnalysisService entry points + extensive unit/integration tests, since
// this repo has no HTTP-mocking harness to split tests into a separate
// file against) is scoped to this one new file by the implementation plan.
//! FAAB bid-projection: Phase 3's five-signal model for "what should I bid"
//! (Epic 3.1). Pure analysis over already-synced data (`StatsDb`) plus the
//! existing `enrich_player` dynasty signal — never launches a browser, and
//! never hand-rolls a single opaque confidence score (Pattern Decisions:
//! "Bid-projection scoring").
//!
//! Testability note: this repo has no HTTP-mocking harness, and no other
//! `analysis::*` method with a `self.client` (live REST) dependency is unit
//! tested directly — `roster.rs`'s sole test targets the pure `resolve_side`
//! helper, not `roster_with_names`/`analyze_roster_needs`. Following that
//! precedent, [`AnalysisService::project_waiver_bid`] is a thin REST-fetching
//! wrapper around [`AnalysisService::project_waiver_bid_with`], which takes
//! already-fetched `rosters`/`all_players`/`league` as parameters and does
//! all five-signal composition — that's the function the tests below drive
//! directly against a temp `StatsDb`, with hand-built fixtures standing in
//! for the REST data.
use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::models::{League, Roster};

use super::dynasty::enrich_player;
use super::AnalysisService;

// ---------------------------------------------------------------------------
// Signal types
// ---------------------------------------------------------------------------

/// Task 3.1.1a: the target player's dynasty tier / search rank, sourced from
/// `enrich_player` (`dynasty.rs:165-192`) verbatim — never recomputed here.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PerceivedValueSignal {
    pub player_id: String,
    pub position: String,
    pub dynasty_tier: i64,
    pub search_rank: Option<i64>,
}

/// Task 3.1.1b: per-roster positional-demand pull for `position`, across
/// every roster in the league — not just the caller's own roster.
///
/// `pull_by_roster` is a deficit count per roster (`max(ideal_low - count,
/// 0)`, the same `(low, high)` ideal targets `analyze_roster_needs` uses —
/// `roster.rs:268-275`), so "demand" means "how far below this roster's own
/// ideal floor for this position," not an absolute headcount.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LeagueDemandSignal {
    pub position: String,
    pub pull_by_roster: BTreeMap<i64, usize>,
}

impl LeagueDemandSignal {
    pub fn total_pull(&self) -> usize {
        self.pull_by_roster.values().sum()
    }

    /// Coarse 3-level label for the short render. The real per-roster counts
    /// live in `pull_by_roster`; this is only a human-scale summary of them,
    /// never presented as a precision score on its own (ux.md §2).
    pub fn level(&self) -> &'static str {
        let rosters = self.pull_by_roster.len().max(1);
        let avg = self.total_pull() as f64 / rosters as f64;
        if avg <= 0.0 {
            "low"
        } else if avg < 1.0 {
            "moderate"
        } else {
            "high"
        }
    }

    /// Task 3.1.3b: the 1-2 rosters (excluding `exclude_roster_id`) with the
    /// highest positional pull — the rosters "most likely to compete for
    /// this claim." Only rosters with positive pull qualify (a roster with
    /// zero need for this position isn't a real competing threat); ties
    /// break by roster_id ascending for determinism.
    pub fn top_competing_rosters(&self, exclude_roster_id: i64, n: usize) -> Vec<i64> {
        let mut candidates: Vec<(i64, usize)> = self
            .pull_by_roster
            .iter()
            .filter(|(&rid, &pull)| rid != exclude_roster_id && pull > 0)
            .map(|(&rid, &pull)| (rid, pull))
            .collect();
        candidates.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        candidates.into_iter().take(n).map(|(rid, _)| rid).collect()
    }
}

/// Task 3.1.2a: this league's own historical bid-clearing price for a
/// position, tagged with `sample_size` so a thin sample (n=3) never looks as
/// confident as a thick one (n=22) — `mean_winning_bid` is never reported
/// alone.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BidHistorySignal {
    pub mean_winning_bid: f64,
    pub sample_size: usize,
    pub position: String,
}

/// Task 3.1.2b: how often *this specific roster* has submitted a waiver
/// claim this season — a raw count, not a rate or percentile. Counts every
/// status (`pending`/`complete`/`rejected`/`cancelled`): "how often do they
/// even submit a claim" (requirements.md §Scope item 2) asks about
/// submission behavior, not win rate.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClaimFrequencySignal {
    pub roster_id: i64,
    pub claims_this_season: usize,
    pub position: Option<String>,
}

/// Replaces a bare `bool is_complete` so a caller/reader can see *why* a
/// FAAB-remaining number might be a lower bound (Pattern Decisions:
/// "FAAB-remaining confidence representation").
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CoverageConfidence {
    Verified { as_of: DateTime<Utc> },
    Floor { reason: String },
}

impl CoverageConfidence {
    pub fn is_floor(&self) -> bool {
        matches!(self, CoverageConfidence::Floor { .. })
    }
}

/// Task 3.1.3a: a roster's remaining FAAB — never a bare number.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FaabRemaining {
    pub amount: i64,
    pub confidence: CoverageConfidence,
}

/// Task 3.1.4a: the bid-projection tool's full output. All five signals
/// carry real values and sample sizes — never a single opaque score
/// (Pattern Decisions: "Bid-projection scoring").
#[derive(Debug, Clone, Serialize)]
pub struct BidProjection {
    pub player_id: String,
    pub roster_id: i64,
    pub perceived_value: PerceivedValueSignal,
    pub league_demand: LeagueDemandSignal,
    pub bid_history: BidHistorySignal,
    pub claim_frequency: ClaimFrequencySignal,
    pub faab_remaining: FaabRemaining,
    /// The 1-2 likeliest competing rosters' own `FaabRemaining` (pre-mortem
    /// P1 #1) — never just the target roster's own figure.
    pub competing_roster_faab: Vec<(i64, FaabRemaining)>,
    pub suggested_bid: i64,
    pub suggested_bid_pct_of_remaining: f64,
    /// Set when `faab_remaining.confidence` is `Floor` or
    /// `bid_history.sample_size < 5` — lets an MCP caller detect a
    /// low-confidence projection without re-deriving it from the individual
    /// signals.
    pub low_confidence: bool,
}

impl BidProjection {
    /// One-line terse render matching the existing CLI voice. Never lets a
    /// low-confidence `suggested_bid` render identically to a well-supported
    /// one (pre-mortem P1 #1 — anchoring risk): a visible caveat marker is
    /// prepended instead of the bare `"suggested bid $N"` shape.
    pub fn render_short(&self) -> String {
        let faab_desc = match &self.faab_remaining.confidence {
            CoverageConfidence::Verified { .. } => "verified".to_string(),
            CoverageConfidence::Floor { reason } => reason.clone(),
        };
        let claim_position = self
            .claim_frequency
            .position
            .as_deref()
            .unwrap_or(self.bid_history.position.as_str());

        let line = format!(
            "suggested bid ${} — {} market (n={}, ${:.0} avg), roster {} has ${} FAAB left ({}), \
             league-wide {} demand: {}, roster {} has filed {} {} claims this season",
            self.suggested_bid,
            self.bid_history.position,
            self.bid_history.sample_size,
            self.bid_history.mean_winning_bid,
            self.roster_id,
            self.faab_remaining.amount,
            faab_desc,
            self.league_demand.position,
            self.league_demand.level(),
            self.claim_frequency.roster_id,
            self.claim_frequency.claims_this_season,
            claim_position,
        );

        if self.low_confidence {
            format!("low-confidence — {line}")
        } else {
            line
        }
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (no I/O)
// ---------------------------------------------------------------------------

/// Task 3.1.1a: perceived-value signal. Errors on an unknown `player_id`
/// rather than silently reporting a placeholder tier.
pub(super) fn perceived_value_signal(
    player_id: &str,
    all_players: &HashMap<String, Value>,
) -> Result<PerceivedValueSignal> {
    if !all_players.contains_key(player_id) {
        return Err(Error::NotFound(format!("player {player_id}")));
    }
    let enriched = enrich_player(player_id, all_players);
    Ok(PerceivedValueSignal {
        player_id: player_id.to_string(),
        position: enriched["position"].as_str().unwrap_or("?").to_string(),
        dynasty_tier: enriched["dynasty_tier"].as_i64().unwrap_or(4),
        search_rank: enriched["search_rank"].as_i64(),
    })
}

/// Task 3.1.1b: league-wide demand signal.
///
/// Replicates `analyze_roster_needs`' per-roster positional-count logic
/// (roster.rs:250-285) inline rather than calling it directly: that method
/// re-fetches the whole league (rosters/users/players) via REST on every
/// call, so looping it across all 12 rosters here would multiply an
/// already-fetched REST round-trip by the roster count for no new data.
/// `rosters`/`all_players` are fetched once by the caller
/// (`project_waiver_bid`) and shared across every roster's count instead.
pub(super) fn league_demand_signal(
    rosters: &[Roster],
    all_players: &HashMap<String, Value>,
    position: &str,
) -> LeagueDemandSignal {
    const IDEAL_LOW: &[(&str, usize)] =
        &[("QB", 1), ("RB", 4), ("WR", 4), ("TE", 1), ("K", 0), ("DEF", 0)];
    let low = IDEAL_LOW
        .iter()
        .find(|(p, _)| *p == position)
        .map(|(_, l)| *l)
        .unwrap_or(0);

    let mut pull_by_roster = BTreeMap::new();
    for r in rosters {
        let count = r
            .players
            .iter()
            .filter(|pid| {
                all_players
                    .get(pid.as_str())
                    .and_then(|p| p.get("position"))
                    .and_then(Value::as_str)
                    == Some(position)
            })
            .count();
        pull_by_roster.insert(r.roster_id as i64, low.saturating_sub(count));
    }
    LeagueDemandSignal { position: position.to_string(), pull_by_roster }
}

/// A waiver transaction's `adds` array (the `resolve_side`-enriched
/// `{player_id, name, position, roster_id, owner}` shape, roster.rs:24-41)
/// in this league's data always carries exactly one entry per transaction —
/// the position attributed to the transaction's single `waiver_bid` amount
/// is taken from the first add. A transaction with zero/malformed `adds`
/// contributes no signal (skipped, not errored).
fn first_add_position(adds: &Value) -> Option<String> {
    adds.as_array()?.first()?.get("position")?.as_str().map(str::to_string)
}

/// Whether any entry in a waiver transaction's `adds` array names `position`
/// — used by claim-frequency's optional position scope (Task 3.1.2b).
fn any_add_position(adds: &Value, position: &str) -> bool {
    adds.as_array()
        .map(|a| a.iter().any(|e| e.get("position").and_then(Value::as_str) == Some(position)))
        .unwrap_or(false)
}

/// Whether a transaction's `roster_ids` JSON array contains `roster_id`.
fn roster_ids_contains(roster_ids: &Value, roster_id: i64) -> bool {
    roster_ids
        .as_array()
        .map(|a| a.iter().any(|v| v.as_i64() == Some(roster_id)))
        .unwrap_or(false)
}

/// Pattern Decisions: "Bid-projection scoring" — hand-rolled weighted
/// combination of the five signals, each carrying its own sample size; no
/// single opaque score, and no statistical/ML fit at this ~150-200-row
/// sample size (overfitting risk, pitfalls.md §4). Clamped to
/// `[0, faab_remaining.amount]` so a suggestion is never literally
/// unaffordable.
fn suggest_bid(
    perceived_value: &PerceivedValueSignal,
    league_demand: &LeagueDemandSignal,
    bid_history: &BidHistorySignal,
    faab_remaining: &FaabRemaining,
    waiver_budget: i64,
) -> (i64, f64) {
    let tier_factor = match perceived_value.dynasty_tier {
        1 => 1.3,
        2 => 1.0,
        3 => 0.7,
        _ => 0.4,
    };
    let demand_factor = match league_demand.level() {
        "high" => 1.25,
        "moderate" => 1.0,
        _ => 0.8,
    };
    // Cold-start: no league-specific price history for this position yet —
    // anchor on a flat slice of the season budget rather than inventing a
    // market price out of zero samples.
    let base = if bid_history.sample_size > 0 {
        bid_history.mean_winning_bid
    } else {
        waiver_budget as f64 * 0.1
    };
    let raw = (base * tier_factor * demand_factor).round() as i64;
    let cap = faab_remaining.amount.max(0);
    let suggested_bid = raw.clamp(0, cap);
    let pct = if faab_remaining.amount > 0 {
        suggested_bid as f64 / faab_remaining.amount as f64
    } else {
        0.0
    };
    (suggested_bid, pct)
}

// ---------------------------------------------------------------------------
// AnalysisService
// ---------------------------------------------------------------------------

impl AnalysisService {
    /// Epic 3.1: the five-signal FAAB bid-projection model. Reads cached
    /// transactions + enrichment only — never launches a browser (ux.md §3
    /// "no browser launch, ever").
    pub async fn project_waiver_bid(
        &self,
        league_id: &str,
        player_id: &str,
        roster_id: i64,
    ) -> Result<BidProjection> {
        let (rosters, _users, all_players, league) =
            self.fetch_rosters_users_players_league(league_id).await?;
        self.project_waiver_bid_with(&league, &rosters, &all_players, player_id, roster_id).await
    }

    /// Core composition — see module doc comment for why this takes
    /// already-fetched `league`/`rosters`/`all_players` rather than fetching
    /// them itself.
    // kibitzer:ignore long-function -- composes the five required signals
    // (Task 3.1.4a) into one struct; each line is a distinct signal, and
    // splitting further would fragment one atomic composition step, the
    // same rationale db.rs's upsert_transaction uses for its own ignore.
    async fn project_waiver_bid_with(
        &self,
        league: &League,
        rosters: &[Roster],
        all_players: &HashMap<String, Value>,
        player_id: &str,
        roster_id: i64,
    ) -> Result<BidProjection> {
        if !rosters.iter().any(|r| r.roster_id as i64 == roster_id) {
            return Err(Error::NotFound(format!("roster {roster_id} in league {}", league.league_id)));
        }

        let perceived_value = perceived_value_signal(player_id, all_players)?;
        let position = perceived_value.position.clone();

        let league_demand = league_demand_signal(rosters, all_players, &position);

        let bid_history =
            self.bid_history_signal(&league.league_id, &league.season, &position).await?;
        let claim_frequency = self
            .claim_frequency_signal(&league.league_id, roster_id, Some(position.as_str()))
            .await?;
        let faab_remaining = self.faab_remaining(league, roster_id).await?;

        let competing_ids = league_demand.top_competing_rosters(roster_id, 2);
        let competing_roster_faab = self.competing_roster_faab(league, &competing_ids).await?;

        let waiver_budget =
            league.settings.extra.get("waiver_budget").and_then(Value::as_i64).unwrap_or(0);
        let (suggested_bid, suggested_bid_pct_of_remaining) =
            suggest_bid(&perceived_value, &league_demand, &bid_history, &faab_remaining, waiver_budget);

        let low_confidence = faab_remaining.confidence.is_floor() || bid_history.sample_size < 5;

        Ok(BidProjection {
            player_id: player_id.to_string(),
            roster_id,
            perceived_value,
            league_demand,
            bid_history,
            claim_frequency,
            faab_remaining,
            competing_roster_faab,
            suggested_bid,
            suggested_bid_pct_of_remaining,
            low_confidence,
        })
    }

    /// Task 3.1.2a: this league's own historical bid-clearing price for
    /// `position`. Aggregates in Rust over `list_transactions` rather than a
    /// raw SQL `GROUP BY` — `position` isn't a column on `transactions`
    /// (it lives inside the pre-resolved `adds` JSON blob, roster.rs:24-41)
    /// — but the grouping still includes `season` (a real SQL column,
    /// filtered here rather than queried): a mismatched `season` never
    /// contributes to the mean, which is the behavior a `GROUP BY (position,
    /// season)` would also produce. Only `complete`-status bids count (a
    /// cleared price, not a pending ask).
    async fn bid_history_signal(
        &self,
        league_id: &str,
        season: &str,
        position: &str,
    ) -> Result<BidHistorySignal> {
        let rows = self.db.list_transactions(league_id, Some("waiver")).await?;
        let (sum, count) = rows
            .iter()
            .filter(|t| t.status.as_deref() == Some("complete") && t.season == season)
            .filter_map(|t| {
                let bid = t.waiver_bid?;
                let pos = first_add_position(&t.adds)?;
                (pos == position).then_some(bid)
            })
            .fold((0i64, 0usize), |(sum, count), bid| (sum + bid, count + 1));

        let mean_winning_bid = if count > 0 { sum as f64 / count as f64 } else { 0.0 };
        Ok(BidHistorySignal { mean_winning_bid, sample_size: count, position: position.to_string() })
    }

    /// Task 3.1.2b: how often `roster_id` has submitted a waiver claim this
    /// season, optionally scoped to `position`. Counts every status —
    /// "how often do they even submit a claim" is about submission
    /// behavior, not win rate.
    async fn claim_frequency_signal(
        &self,
        league_id: &str,
        roster_id: i64,
        position: Option<&str>,
    ) -> Result<ClaimFrequencySignal> {
        let rows = self.db.list_transactions(league_id, Some("waiver")).await?;
        let claims_this_season = rows
            .iter()
            .filter(|t| roster_ids_contains(&t.roster_ids, roster_id))
            .filter(|t| position.map(|pos| any_add_position(&t.adds, pos)).unwrap_or(true))
            .count();
        Ok(ClaimFrequencySignal {
            roster_id,
            claims_this_season,
            position: position.map(str::to_string),
        })
    }

    /// Task 3.1.3a: `waiver_budget` (from `league.settings.extra` — no typed
    /// field exists on `LeagueSettings`, models.rs:25-36) minus summed
    /// `complete`-status waiver bids for `roster_id`, gated by
    /// `has_full_league_graphql_coverage`. Sums over `list_transactions`'
    /// existing output in Rust rather than a dedicated SQL query — the
    /// plan's own preference for this ~150-200-row, single-season dataset.
    async fn faab_remaining(&self, league: &League, roster_id: i64) -> Result<FaabRemaining> {
        let waiver_budget =
            league.settings.extra.get("waiver_budget").and_then(Value::as_i64).unwrap_or(0);

        let rows = self.db.list_transactions(&league.league_id, Some("waiver")).await?;
        let spent: i64 = rows
            .iter()
            .filter(|t| t.status.as_deref() == Some("complete"))
            .filter(|t| roster_ids_contains(&t.roster_ids, roster_id))
            .filter_map(|t| t.waiver_bid)
            .sum();

        let confidence = match self
            .db
            .has_full_league_graphql_coverage(&league.league_id, &league.season)
            .await?
        {
            Some(as_of) => CoverageConfidence::Verified { as_of },
            None => CoverageConfidence::Floor {
                reason: format!(
                    "no full-league GraphQL backfill recorded for season {}",
                    league.season
                ),
            },
        };

        Ok(FaabRemaining { amount: waiver_budget - spent, confidence })
    }

    /// Task 3.1.3b: `FaabRemaining` for each of the given competing roster
    /// ids, via the exact same `faab_remaining` logic as the target roster
    /// (pre-mortem P1 #1) — never a second computation path.
    async fn competing_roster_faab(
        &self,
        league: &League,
        competing_roster_ids: &[i64],
    ) -> Result<Vec<(i64, FaabRemaining)>> {
        let mut out = Vec::with_capacity(competing_roster_ids.len());
        for &rid in competing_roster_ids {
            out.push((rid, self.faab_remaining(league, rid).await?));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::SleeperClient;
    use crate::config::Config;
    use crate::db::{StatsDb, TransactionRow, TransactionSyncRun};
    use crate::models::{LeagueSettings, RosterSettings};

    // -----------------------------------------------------------------------
    // Test fixtures
    // -----------------------------------------------------------------------

    /// Same construction as `cli/browser/transactions.rs`'s
    /// `test_analysis_service` helper: a scratch-dir `Config` per test (no
    /// in-memory DB mode), a `SleeperClient` built but never asked to make a
    /// network call, and a fresh `StatsDb` file. None of this module's tests
    /// exercise `AnalysisService::client` — `project_waiver_bid_with` (the
    /// tested core) takes `rosters`/`all_players`/`league` as parameters —
    /// so the client is only here to satisfy `AnalysisService`'s shape.
    async fn test_analysis_service(name: &str) -> AnalysisService {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data_dir = std::env::temp_dir()
            .join(format!("sleeper_bid_projection_test_{name}_{}_{nanos}", std::process::id()));
        let config = Config { data_dir, ..Config::default() };
        let client = SleeperClient::new(&config).await.expect("build client (no network at construction)");
        let db = StatsDb::open(&config.stats_db_path()).await.expect("open test db");
        AnalysisService::new(client, db)
    }

    fn brissett_players() -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert(
            "3257".to_string(),
            serde_json::json!({
                "full_name": "Jacoby Brissett",
                "position": "QB",
                "team": "ARI",
                "age": 33,
                "years_exp": 10,
                "search_rank": 48,
            }),
        );
        m
    }

    fn roster_fixture(roster_id: i32, qb_count: usize) -> Roster {
        Roster {
            roster_id,
            owner_id: None,
            league_id: "league1".to_string(),
            players: (0..qb_count).map(|i| format!("qb{roster_id}_{i}")).collect(),
            starters: vec![],
            reserve: vec![],
            taxi: None,
            settings: RosterSettings::default(),
        }
    }

    fn qb_players(roster_id: i32, qb_count: usize) -> HashMap<String, Value> {
        (0..qb_count)
            .map(|i| {
                (
                    format!("qb{roster_id}_{i}"),
                    serde_json::json!({"full_name": "QB", "position": "QB", "team": "ARI"}),
                )
            })
            .collect()
    }

    fn league_fixture(waiver_budget: i64) -> League {
        League {
            league_id: "1352883872131481600".to_string(),
            name: "Test League".to_string(),
            season: "2026".to_string(),
            status: "in_season".to_string(),
            sport: "nfl".to_string(),
            settings: LeagueSettings {
                extra: HashMap::from([("waiver_budget".to_string(), Value::from(waiver_budget))]),
                ..Default::default()
            },
            total_rosters: 12,
            draft_id: None,
            avatar: None,
            scoring_settings: None,
            roster_positions: vec![],
        }
    }

    fn add_entry(position: &str) -> Value {
        serde_json::json!([{"player_id": "x", "name": "X", "position": position, "roster_id": 5, "owner": "Tyler"}])
    }

    // kibitzer:ignore long-parameter-list -- test-only helper mirroring
    // `TransactionRow`'s own field set (db.rs:45-59); a config struct here
    // would just re-wrap that struct for one call site per test.
    async fn seed_waiver(
        db: &StatsDb,
        transaction_id: &str,
        league_id: &str,
        season: &str,
        status: &str,
        roster_ids: Vec<i32>,
        waiver_bid: Option<i64>,
        position: &str,
    ) {
        db.upsert_transaction(TransactionRow {
            transaction_id,
            league_id,
            season,
            week: 9,
            kind: "waiver",
            status: Some(status),
            created: 1_781_000_000,
            adds: &add_entry(position),
            drops: &serde_json::json!([]),
            roster_ids: &serde_json::json!(roster_ids),
            waiver_bid,
            status_updated: None,
            source: "rest",
        })
        .await
        .expect("seed waiver transaction");
    }

    // -----------------------------------------------------------------------
    // Story 3.1.1: perceived-value + league-demand
    // -----------------------------------------------------------------------

    #[test]
    fn perceived_value_signal_should_include_dynasty_tier_and_search_rank_when_computed_for_known_player(
    ) {
        let all_players = brissett_players();
        let signal = perceived_value_signal("3257", &all_players).expect("known player");
        assert_eq!(signal.player_id, "3257");
        assert_eq!(signal.position, "QB");
        assert_eq!(signal.search_rank, Some(48));
        // dynasty_tier sourced from enrich_player, not recomputed here.
        let expected = enrich_player("3257", &all_players);
        assert_eq!(signal.dynasty_tier, expected["dynasty_tier"].as_i64().unwrap());
    }

    #[test]
    fn perceived_value_signal_should_return_err_when_player_id_is_unknown() {
        let all_players = HashMap::new();
        let result = perceived_value_signal("unknown", &all_players);
        assert!(result.is_err(), "unknown player_id must error, never a silent placeholder tier");
    }

    #[test]
    fn league_demand_for_position_should_aggregate_across_all_twelve_rosters_when_computed_for_qb()
    {
        // 12 rosters, each with a different QB count (0..11) — a pull signal
        // computed only from roster 1 (qb_count=0, pull=1) would look
        // identical to one that actually swept all 12.
        let rosters: Vec<Roster> = (1..=12).map(|rid| roster_fixture(rid, (rid as usize) % 3)).collect();
        let mut all_players = HashMap::new();
        for rid in 1..=12 {
            all_players.extend(qb_players(rid, (rid as usize) % 3));
        }

        let signal = league_demand_signal(&rosters, &all_players, "QB");
        assert_eq!(signal.pull_by_roster.len(), 12, "must reflect a pull across all 12 rosters");
        // Rosters with 0 QBs (rid % 3 == 0: 3,6,9,12) are below the ideal
        // low of 1 — pull 1 — not just roster 1 (the hypothetical caller).
        assert_eq!(signal.pull_by_roster[&3], 1);
        assert_eq!(signal.pull_by_roster[&6], 1);
        assert!(signal.total_pull() >= 4, "demand must aggregate deficits across multiple rosters");
    }

    // -----------------------------------------------------------------------
    // Story 3.1.2: bid-history + claim-frequency
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn bid_history_signal_should_report_mean_and_sample_size_together_when_four_qb_bids_cached(
    ) {
        let service = test_analysis_service("bid_history_mean").await;
        let league_id = "1352883872131481600";
        for (i, bid) in [12i64, 8, 15, 5].into_iter().enumerate() {
            seed_waiver(&service.db, &i.to_string(), league_id, "2026", "complete", vec![5], Some(bid), "QB")
                .await;
        }

        let signal = service.bid_history_signal(league_id, "2026", "QB").await.expect("bid history");
        assert_eq!(
            signal,
            BidHistorySignal { mean_winning_bid: 10.0, sample_size: 4, position: "QB".to_string() }
        );
    }

    #[tokio::test]
    async fn bid_history_signal_should_aggregate_separately_per_season_when_multiple_seasons_cached()
    {
        let service = test_analysis_service("bid_history_per_season").await;
        let league_id = "1352883872131481600";
        seed_waiver(&service.db, "s2025-a", league_id, "2025", "complete", vec![5], Some(100), "QB").await;
        seed_waiver(&service.db, "s2026-a", league_id, "2026", "complete", vec![5], Some(12), "QB").await;
        seed_waiver(&service.db, "s2026-b", league_id, "2026", "complete", vec![5], Some(8), "QB").await;

        let season_2026 =
            service.bid_history_signal(league_id, "2026", "QB").await.expect("2026 history");
        let season_2025 =
            service.bid_history_signal(league_id, "2025", "QB").await.expect("2025 history");

        assert_eq!(season_2026.sample_size, 2);
        assert_eq!(season_2026.mean_winning_bid, 10.0);
        assert_eq!(season_2025.sample_size, 1);
        assert_eq!(season_2025.mean_winning_bid, 100.0);
    }

    #[tokio::test]
    async fn claim_frequency_signal_should_count_seven_claims_when_roster_has_seven_waiver_transactions(
    ) {
        let service = test_analysis_service("claim_frequency_count").await;
        let league_id = "1352883872131481600";
        for i in 0..7 {
            // Mix of statuses — claim frequency counts submission behavior,
            // not win rate.
            let status = if i % 2 == 0 { "complete" } else { "pending" };
            seed_waiver(&service.db, &format!("c{i}"), league_id, "2026", status, vec![5], None, "RB").await;
        }

        let signal =
            service.claim_frequency_signal(league_id, 5, None).await.expect("claim frequency");
        assert_eq!(
            signal,
            ClaimFrequencySignal { roster_id: 5, claims_this_season: 7, position: None }
        );
    }

    #[tokio::test]
    async fn claim_frequency_signal_should_return_zero_when_roster_has_no_claims_for_requested_position(
    ) {
        let service = test_analysis_service("claim_frequency_scoped").await;
        let league_id = "1352883872131481600";
        for i in 0..3 {
            seed_waiver(&service.db, &format!("rb{i}"), league_id, "2026", "complete", vec![5], None, "RB")
                .await;
        }

        let signal = service
            .claim_frequency_signal(league_id, 5, Some("QB"))
            .await
            .expect("claim frequency scoped to QB");
        assert_eq!(
            signal,
            ClaimFrequencySignal { roster_id: 5, claims_this_season: 0, position: Some("QB".to_string()) }
        );
    }

    // -----------------------------------------------------------------------
    // Story 3.1.3: FAAB-remaining + coverage confidence
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn faab_remaining_should_report_verified_confidence_when_full_league_graphql_coverage_recorded(
    ) {
        let service = test_analysis_service("faab_verified").await;
        let league = league_fixture(100);
        let as_of = Utc::now();
        service
            .db
            .record_sync_run(TransactionSyncRun {
                league_id: &league.league_id,
                season: &league.season,
                source: "graphql",
                roster_scope: "all",
                synced_at: as_of,
                rows_fetched: 10,
                rows_changed: 10,
            })
            .await
            .expect("record sync run");
        seed_waiver(&service.db, "w1", &league.league_id, "2026", "complete", vec![5], Some(13), "QB")
            .await;

        let faab = service.faab_remaining(&league, 5).await.expect("faab remaining");
        assert_eq!(faab.amount, 87);
        match faab.confidence {
            CoverageConfidence::Verified { as_of: got } => {
                assert_eq!(got.timestamp(), as_of.timestamp());
            }
            CoverageConfidence::Floor { reason } => {
                panic!("expected Verified confidence, got Floor({reason})")
            }
        }
    }

    #[tokio::test]
    async fn faab_remaining_should_report_floor_confidence_when_no_full_league_backfill_recorded() {
        let service = test_analysis_service("faab_floor").await;
        let league = league_fixture(100);
        // No `record_sync_run` call at all — no full-league GraphQL backfill.

        let faab = service.faab_remaining(&league, 5).await.expect("faab remaining");
        assert_eq!(
            faab.confidence,
            CoverageConfidence::Floor {
                reason: "no full-league GraphQL backfill recorded for season 2026".to_string()
            }
        );
    }

    #[tokio::test]
    async fn bid_projection_should_include_competing_roster_faab_when_other_rosters_have_positional_demand(
    ) {
        let service = test_analysis_service("competing_roster_faab").await;
        let league = league_fixture(100);

        // Roster 5 is the target; rosters 9 and 11 have the highest QB pull.
        let mut pull_by_roster = BTreeMap::new();
        pull_by_roster.insert(5i64, 0);
        pull_by_roster.insert(9i64, 2);
        pull_by_roster.insert(11i64, 1);
        let demand = LeagueDemandSignal { position: "QB".to_string(), pull_by_roster };

        let competing_ids = demand.top_competing_rosters(5, 2);
        assert_eq!(competing_ids, vec![9, 11]);

        let competing = service
            .competing_roster_faab(&league, &competing_ids)
            .await
            .expect("competing roster faab");
        assert!(!competing.is_empty(), "must be populated for the top competing rosters");
        assert_eq!(competing.len(), 2);
        assert_eq!(competing[0].0, 9);
        assert_eq!(competing[1].0, 11);
    }

    // -----------------------------------------------------------------------
    // Story 3.1.4: compose BidProjection + render
    // -----------------------------------------------------------------------

    #[tokio::test]
    // kibitzer:ignore long-function -- seeds + asserts all five required
    // signals in one call (Story 3.1.4's own AC), the single scenario this
    // test exists to cover; splitting it would just scatter one assertion
    // set across several tests that all need the same seeded fixture.
    async fn project_waiver_bid_should_return_non_placeholder_values_for_all_five_signals_when_called_for_known_player_and_roster(
    ) {
        let service = test_analysis_service("project_waiver_bid_full").await;
        let league = league_fixture(100);
        for (i, bid) in [12i64, 8, 15, 5, 20].into_iter().enumerate() {
            seed_waiver(&service.db, &format!("h{i}"), &league.league_id, "2026", "complete", vec![9], Some(bid), "QB")
                .await;
        }
        for i in 0..3 {
            seed_waiver(&service.db, &format!("own{i}"), &league.league_id, "2026", "pending", vec![5], None, "QB")
                .await;
        }
        service
            .db
            .record_sync_run(TransactionSyncRun {
                league_id: &league.league_id,
                season: &league.season,
                source: "graphql",
                roster_scope: "all",
                synced_at: Utc::now(),
                rows_fetched: 8,
                rows_changed: 8,
            })
            .await
            .expect("record sync run");

        let rosters = vec![roster_fixture(5, 1), roster_fixture(9, 0), roster_fixture(11, 3)];
        let all_players = brissett_players();

        let projection = service
            .project_waiver_bid_with(&league, &rosters, &all_players, "3257", 5)
            .await
            .expect("project waiver bid");

        assert_eq!(projection.player_id, "3257");
        assert_eq!(projection.roster_id, 5);
        assert_eq!(projection.perceived_value.position, "QB");
        assert!(projection.perceived_value.search_rank.is_some());
        assert_eq!(projection.league_demand.position, "QB");
        assert!(!projection.league_demand.pull_by_roster.is_empty());
        assert_eq!(projection.bid_history.sample_size, 5);
        assert_eq!(projection.claim_frequency.roster_id, 5);
        assert_eq!(projection.claim_frequency.claims_this_season, 3);
        assert!(projection.faab_remaining.amount >= 0);
        assert!(!projection.competing_roster_faab.is_empty());
        assert!(projection.suggested_bid >= 0);
    }

    #[tokio::test]
    async fn project_waiver_bid_should_return_err_when_roster_id_does_not_exist_in_league() {
        let service = test_analysis_service("project_waiver_bid_unknown_roster").await;
        let league = league_fixture(100);
        let rosters = vec![roster_fixture(5, 1)];
        let all_players = brissett_players();

        let result = service
            .project_waiver_bid_with(&league, &rosters, &all_players, "3257", 999)
            .await;
        assert!(result.is_err(), "unknown roster_id must error, never silently default to another roster");
    }

    fn sample_projection(low_confidence_via: &str) -> BidProjection {
        let mut pull_by_roster = BTreeMap::new();
        pull_by_roster.insert(5i64, 0);
        pull_by_roster.insert(9i64, 1);

        let (faab_confidence, sample_size) = match low_confidence_via {
            "floor" => (CoverageConfidence::Floor { reason: "no full-league GraphQL backfill recorded for season 2026".to_string() }, 6),
            "thin_sample" => (CoverageConfidence::Verified { as_of: Utc::now() }, 3),
            _ => (CoverageConfidence::Verified { as_of: Utc::now() }, 6),
        };

        BidProjection {
            player_id: "3257".to_string(),
            roster_id: 5,
            perceived_value: PerceivedValueSignal {
                player_id: "3257".to_string(),
                position: "QB".to_string(),
                dynasty_tier: 2,
                search_rank: Some(48),
            },
            league_demand: LeagueDemandSignal { position: "QB".to_string(), pull_by_roster },
            bid_history: BidHistorySignal {
                mean_winning_bid: 10.0,
                sample_size,
                position: "QB".to_string(),
            },
            claim_frequency: ClaimFrequencySignal {
                roster_id: 5,
                claims_this_season: 7,
                position: Some("QB".to_string()),
            },
            faab_remaining: FaabRemaining { amount: 87, confidence: faab_confidence },
            competing_roster_faab: vec![],
            suggested_bid: 23,
            suggested_bid_pct_of_remaining: 23.0 / 87.0,
            low_confidence: low_confidence_via != "none",
        }
    }

    #[test]
    fn bid_projection_render_short_should_produce_one_line_with_real_numbers_when_called() {
        let projection = sample_projection("none");
        let projection = BidProjection { low_confidence: false, ..projection };
        let line = projection.render_short();

        assert!(!line.contains('\n'), "render_short must be exactly one line");
        assert!(line.starts_with("suggested bid $23"), "got: {line}");
        assert!(line.contains("QB market (n=6, $10 avg)"), "got: {line}");
        assert!(line.contains("roster 5 has $87 FAAB left (verified)"), "got: {line}");
        assert!(line.contains("league-wide QB demand:"), "got: {line}");
        assert!(line.contains("roster 5 has filed 7 QB claims this season"), "got: {line}");
        assert!(!line.contains("confidence:"), "no bare opaque confidence score allowed (ux.md §2)");
    }

    #[test]
    fn bid_projection_render_short_should_prepend_caveat_marker_when_faab_confidence_is_floor_or_sample_size_below_five(
    ) {
        let floor_case = sample_projection("floor");
        assert!(floor_case.low_confidence);
        let floor_line = floor_case.render_short();
        assert!(
            floor_line.starts_with("low-confidence — suggested bid"),
            "Floor confidence must prefix the caveat marker, got: {floor_line}"
        );

        let thin_sample_case = sample_projection("thin_sample");
        assert!(thin_sample_case.low_confidence);
        let thin_line = thin_sample_case.render_short();
        assert!(
            thin_line.starts_with("low-confidence — suggested bid"),
            "sample_size < 5 must prefix the caveat marker, got: {thin_line}"
        );

        let well_supported = sample_projection("none");
        let well_supported = BidProjection { low_confidence: false, ..well_supported };
        let well_supported_line = well_supported.render_short();
        assert!(
            well_supported_line.starts_with("suggested bid $"),
            "a well-supported projection must never carry the low-confidence prefix, got: {well_supported_line}"
        );
    }
}
