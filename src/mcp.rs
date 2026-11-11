// kibitzer:ignore file-size -- every MCP tool/params-struct pair lives here
// by established convention (one surface, one file); Epic 3.2's addition
// (one params struct, one tool method, two tests) is the same grain as
// every existing entry, not a new module's worth of code.
/// MCP server tools exposing Sleeper analytics via the rmcp stdio transport.
use std::sync::Arc;

use rmcp::{
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo, Implementation},
    schemars::{self, JsonSchema},
    tool, Error as McpError, ServerHandler,
};
use serde::Deserialize;

use crate::analysis::AnalysisService;
use crate::config::Config;

#[derive(Clone)]
pub struct SleeperMcpServer {
    svc: Arc<AnalysisService>,
    config: Arc<Config>,
}

impl SleeperMcpServer {
    pub fn new(svc: AnalysisService, config: Config) -> Self {
        Self { svc: Arc::new(svc), config: Arc::new(config) }
    }
}

// Helper to serialise any Result into a CallToolResult.
fn ok<T: serde::Serialize>(v: T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(&v)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

fn err(e: crate::error::Error) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

async fn resolve(db: &crate::db::StatsDb, league_id: &str, identifier: &str) -> std::result::Result<i32, McpError> {
    db.resolve_roster_id(league_id, identifier)
        .await
        .map_err(err)?
        .ok_or_else(|| McpError::invalid_params(
            format!("No roster for '{}' in league {}. Run sync_league first.", identifier, league_id),
            None,
        ))
}

// ---------------------------------------------------------------------------
// Parameter structs (all must derive JsonSchema for schema generation)
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
struct GetUserParams {
    /// Username or user_id to look up
    identifier: String,
}

#[derive(Deserialize, JsonSchema)]
struct GetUserLeaguesParams {
    user_id: String,
    season: String,
    #[serde(default = "default_nfl")]
    sport: String,
}

#[derive(Deserialize, JsonSchema)]
struct LeagueIdParams {
    league_id: String,
}

#[derive(Deserialize, JsonSchema)]
struct MatchupsParams {
    league_id: String,
    week: i32,
}

#[derive(Deserialize, JsonSchema)]
struct WaiversParams {
    league_id: String,
    #[serde(default = "default_48")]
    lookback_hours: i32,
    #[serde(default = "default_15")]
    limit: usize,
}

#[derive(Deserialize, JsonSchema)]
struct LineupParams {
    league_id: String,
    /// Roster ID (number), username, or display name
    roster_id: String,
    week: i32,
}

#[derive(Deserialize, JsonSchema)]
struct TradeParams {
    giving_player_ids: Vec<String>,
    receiving_player_ids: Vec<String>,
    season: Option<String>,
    week: Option<i32>,
}

#[derive(Deserialize, JsonSchema)]
struct TransactionsParams {
    league_id: String,
    week: i32,
    type_filter: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct RosterParams {
    league_id: String,
    /// Roster ID (number), username, or display name
    roster_id: String,
    /// Bypass this process's own cache and ask Sleeper to skip its own caching too. Use
    /// this right after a lineup/waiver write to confirm it actually landed — Sleeper's own
    /// API can otherwise lag several minutes behind a real change. Leave false for normal
    /// reads; forcing on every call defeats the point of caching.
    #[serde(default)]
    force_refresh: bool,
}

#[derive(Deserialize, JsonSchema)]
struct AvailableParams {
    league_id: String,
    position: Option<String>,
    season: Option<String>,
    #[serde(default = "default_25")]
    limit: usize,
}

#[derive(Deserialize, JsonSchema)]
struct SearchParams {
    query: String,
    position: Option<String>,
    #[serde(default = "default_10")]
    limit: usize,
}

#[derive(Deserialize, JsonSchema)]
struct TrendingParams {
    #[serde(default = "default_add")]
    kind: String,
    #[serde(default = "default_nfl")]
    sport: String,
    #[serde(default = "default_24")]
    lookback_hours: i32,
    #[serde(default = "default_trending_limit")]
    limit: i32,
}

#[derive(Deserialize, JsonSchema)]
struct PlayerStatsParams {
    season_type: String,
    season: String,
    week: i32,
    #[serde(default = "default_nfl")]
    sport: String,
}

#[derive(Deserialize, JsonSchema)]
struct DraftPicksParams {
    draft_id: String,
}

#[derive(Deserialize, JsonSchema)]
struct DynastyReportParams {
    league_id: String,
    /// Roster ID (number), username, or display name
    roster_id: String,
}

#[derive(Deserialize, JsonSchema)]
struct FindTradesParams {
    league_id: String,
    /// Roster ID (number), username, or display name
    roster_id: String,
    #[serde(default = "default_10")]
    limit: usize,
}

#[derive(Deserialize, JsonSchema)]
struct DynastyScoutParams {
    league_id: String,
    position: Option<String>,
    #[serde(default = "default_26")]
    max_age: u32,
    #[serde(default = "default_30_usize")]
    limit: usize,
}

fn default_26() -> u32 { 26 }
fn default_30_usize() -> usize { 30 }

#[derive(Deserialize, JsonSchema)]
struct PickParam {
    /// e.g. "2027"
    season: String,
    /// 1-indexed (round 1 = first round)
    round: i32,
    /// The roster_id this pick originally belonged to — NOT the current owner;
    /// ownership (after any trades) is resolved internally via `traded_picks`.
    original_roster_id: i32,
}

#[derive(Deserialize, JsonSchema)]
struct TradeValueParams {
    league_id: String,
    /// Sleeper player IDs to look up (mix both sides of a proposed trade in one call)
    #[serde(default)]
    player_ids: Vec<String>,
    /// Draft picks to look up alongside (or instead of) players
    #[serde(default)]
    picks: Vec<PickParam>,
}

/// LIVE WRITE ACTION: submits a real waiver claim/free-agent add against
/// sleeper.com through the stealth-browser backend (plan.md
/// undetected-chromedriver-rust, Story 3.2) on the operator's own persisted,
/// manually-authenticated Chrome profile. Not a simulation — this spends
/// FAAB and changes the live roster. Requires `sleeper browser login` to
/// have been completed by hand at least once beforehand.
#[derive(Deserialize, JsonSchema)]
struct WaiverClaimParams {
    league_id: String,
    /// Sleeper player_id to claim/add.
    player_id: String,
    /// Sleeper player_id to drop, if any.
    drop_player_id: Option<String>,
    /// FAAB bid amount; ignored for a free agent not on waivers.
    #[serde(default)]
    bid: u32,
}

/// Read-only: your own pending waiver claims. Root-cause fix (2026-11-10) —
/// the public REST transactions endpoint only ever returns `status:
/// "complete"`, so pending claims are invisible there. Reads through the
/// operator's own persisted, manually-authenticated Chrome profile instead,
/// via the same authenticated GraphQL query the Sleeper web app itself
/// fires. Requires `sleeper browser login` to have been completed by hand
/// at least once beforehand. Not a write action.
#[derive(Deserialize, JsonSchema)]
struct PendingWaiversParams {
    league_id: String,
    /// Your numeric Sleeper roster_id in this league.
    roster_id: i64,
}

/// Read-only: the league's real Trade Block. Discovery (2026-11-11):
/// Sleeper has no dedicated trade-block query — it's `settings.otb` on the
/// existing `league_players` GraphQL query, same auth path as pending
/// waivers above. Not a write action.
#[derive(Deserialize, JsonSchema)]
struct TradeBlockParams {
    league_id: String,
}

/// Read-only: transaction history served from the unified cache
/// (Epic 2.1/2.2's `read_through`), refreshing live only when a row's
/// status/age genuinely warrants it — never on a fixed interval. Not a
/// write action.
#[derive(Deserialize, JsonSchema)]
struct TransactionHistoryParams {
    league_id: String,
    /// Restrict to one kind (`waiver`, `trade`, `free_agent`); omitted returns all three.
    kind_filter: Option<String>,
    /// Explicit cache tolerance in seconds; omitted uses the Tuesday-window-aware
    /// default per row's status (ux.md §7). `0` forces a live refresh.
    max_age_secs: Option<i64>,
}

/// LIVE WRITE ACTION: swaps two roster slots against sleeper.com through
/// the stealth-browser backend (plan.md undetected-chromedriver-rust,
/// Story 4.1) on the operator's own persisted, manually-authenticated
/// Chrome profile. Not a simulation — this changes the live roster
/// immediately with no confirm step. Also covers IR/Taxi moves
/// (live-verified 2026-11-11) — the DOM swap mechanism has no slot-type
/// restriction: pass the IR/Taxi occupant as `out` and the bench mover as
/// `in` (or the reverse, to activate someone back to the bench). Requires
/// `sleeper browser login` to have been completed by hand at least once
/// beforehand.
#[derive(Deserialize, JsonSchema)]
struct LineupSetParams {
    league_id: String,
    /// Sleeper player_id currently occupying the target slot (starter, IR, or Taxi) to bench.
    out_player_id: String,
    /// Sleeper player_id of the bench player moving into that slot in their place.
    in_player_id: String,
}

/// Read-only: Epic 3.1's five-signal FAAB bid-projection model ("what
/// should I bid"). Never launches a browser — pure analysis over
/// already-cached data.
#[derive(Debug, Deserialize, JsonSchema)]
struct BidProjectionParams {
    league_id: String,
    /// Sleeper player_id of the waiver target.
    player_id: String,
    /// Your numeric Sleeper roster_id in this league.
    roster_id: i64,
}

/// LIVE WRITE ACTION: edits the FAAB bid on a real pending waiver claim
/// against sleeper.com (Epic 4.3, Story 4.3.2). Same real-money framing as
/// `WaiverClaimParams` — `new_bid` is required so a missing/invalid bid is
/// a validation error before any browser launch, never a launch followed
/// by a DOM-level failure.
#[derive(Debug, Deserialize, JsonSchema)]
struct EditWaiverClaimParams {
    league_id: String,
    /// The Sleeper transaction_id of the pending claim to edit.
    transaction_id: String,
    /// New FAAB bid amount to replace the claim's current bid.
    new_bid: u32,
}

/// LIVE WRITE ACTION: cancels a real pending waiver claim against
/// sleeper.com (Epic 4.3, Story 4.3.2).
#[derive(Deserialize, JsonSchema)]
struct CancelWaiverClaimParams {
    league_id: String,
    /// The Sleeper transaction_id of the pending claim to cancel.
    transaction_id: String,
}

#[derive(Deserialize, JsonSchema)]
struct PlayerReportParams {
    /// Sleeper player IDs — use search_players first if you only have a name
    player_ids: Vec<String>,
    /// NFL season, e.g. "2026". Defaults to the live season if omitted.
    season: Option<String>,
}

/// Persist a dated analysis finding (e.g. a standings/trade-scan review) to
/// the local stats DB so it survives past this chat session. Not a Sleeper
/// write action — this never touches the live league.
#[derive(Deserialize, JsonSchema)]
struct LogAnalysisNoteParams {
    league_id: String,
    /// Short category label, e.g. "standings_review", "trade_scan".
    topic: String,
    summary: String,
    /// Optional longer supporting detail.
    detail: Option<String>,
}

/// Read back previously logged analysis notes, most recent first.
#[derive(Deserialize, JsonSchema)]
struct GetAnalysisNotesParams {
    /// Omit to return notes across every league.
    league_id: Option<String>,
    #[serde(default = "default_20")]
    limit: i64,
}

fn default_20() -> i64 { 20 }
fn default_nfl() -> String { "nfl".to_string() }
fn default_add() -> String { "add".to_string() }
fn default_48() -> i32 { 48 }
fn default_24() -> i32 { 24 }
fn default_15() -> usize { 15 }
fn default_25() -> usize { 25 }
fn default_10() -> usize { 10 }
fn default_trending_limit() -> i32 { 25 }

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

#[tool(tool_box)]
impl SleeperMcpServer {
    #[tool(description = "Get a Sleeper user by username or user_id")]
    async fn get_user(
        &self,
        #[tool(aggr)] p: GetUserParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.client.get_user(&p.identifier).await.map_err(err)?)
    }

    #[tool(description = "Get all leagues for a user in a given season")]
    async fn get_user_leagues(
        &self,
        #[tool(aggr)] p: GetUserLeaguesParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.client.get_user_leagues(&p.user_id, &p.season, &p.sport).await.map_err(err)?)
    }

    #[tool(description = "Get details of a specific league")]
    async fn get_league(
        &self,
        #[tool(aggr)] p: LeagueIdParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.client.get_league(&p.league_id).await.map_err(err)?)
    }

    #[tool(description = "Get current NFL season state (week, season, season_type)")]
    async fn get_nfl_state(&self) -> Result<CallToolResult, McpError> {
        ok(self.svc.client.get_nfl_state().await.map_err(err)?)
    }

    #[tool(description = "Get league standings sorted by wins then points")]
    async fn get_league_standings(
        &self,
        #[tool(aggr)] p: LeagueIdParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.standings(&p.league_id).await.map_err(err)?)
    }

    #[tool(description = "Get all rosters in a league")]
    async fn get_league_rosters(
        &self,
        #[tool(aggr)] p: LeagueIdParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.client.get_league_rosters(&p.league_id).await.map_err(err)?)
    }

    #[tool(description = "Get matchup overview for a specific week")]
    async fn get_matchups(
        &self,
        #[tool(aggr)] p: MatchupsParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.matchup_overview(&p.league_id, p.week).await.map_err(err)?)
    }

    #[tool(description = "Get start/sit lineup recommendations based on projected points")]
    async fn get_start_sit_recommendations(
        &self,
        #[tool(aggr)] p: LineupParams,
    ) -> Result<CallToolResult, McpError> {
        let rid = resolve(&self.svc.db, &p.league_id, &p.roster_id).await?;
        ok(self.svc.start_sit(&p.league_id, rid, p.week).await.map_err(err)?)
    }

    #[tool(description = "Get waiver wire recommendations: trending adds not yet rostered in this league")]
    async fn get_waiver_recommendations(
        &self,
        #[tool(aggr)] p: WaiversParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.waiver_recommendations(&p.league_id, p.lookback_hours, p.limit).await.map_err(err)?)
    }

    #[tool(description = "Get trending players across all Sleeper leagues (adds or drops)")]
    async fn get_trending_players(
        &self,
        #[tool(aggr)] p: TrendingParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.client.get_trending_players(&p.kind, &p.sport, p.lookback_hours, p.limit).await.map_err(err)?)
    }

    #[tool(description = "Analyze a potential trade based on last week's actual stats")]
    async fn analyze_trade(
        &self,
        #[tool(aggr)] p: TradeParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.analyze_trade(
            &p.giving_player_ids,
            &p.receiving_player_ids,
            p.season.as_deref(),
            p.week,
        ).await.map_err(err)?)
    }

    #[tool(description = "Get transactions for a league week (waiver claims, trades, free agent adds)")]
    async fn get_recent_transactions(
        &self,
        #[tool(aggr)] p: TransactionsParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.recent_transactions(&p.league_id, p.week, p.type_filter.as_deref()).await.map_err(err)?)
    }

    #[tool(description = "Search NFL players by name (and optionally position)")]
    async fn search_players(
        &self,
        #[tool(aggr)] p: SearchParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.search_players(&p.query, p.position.as_deref(), p.limit).await.map_err(err)?)
    }

    #[tool(description = "Get player stats for a specific week")]
    async fn get_player_stats(
        &self,
        #[tool(aggr)] p: PlayerStatsParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.client.get_player_stats(&p.season_type, &p.season, p.week, &p.sport).await.map_err(err)?)
    }

    #[tool(description = "Get all drafts associated with a league")]
    async fn get_league_drafts(
        &self,
        #[tool(aggr)] p: LeagueIdParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.client.get_league_drafts(&p.league_id).await.map_err(err)?)
    }

    #[tool(description = "Get all picks for a specific draft")]
    async fn get_draft_picks(
        &self,
        #[tool(aggr)] p: DraftPicksParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.client.get_draft_picks(&p.draft_id).await.map_err(err)?)
    }

    #[tool(description = "Get roster details with player names, positions, and injury status. Set force_refresh:true right after a lineup/waiver write to confirm it actually landed — the normal cached read can otherwise show Sleeper's own stale server-side state for several minutes.")]
    async fn get_roster_with_names(
        &self,
        #[tool(aggr)] p: RosterParams,
    ) -> Result<CallToolResult, McpError> {
        let rid = resolve(&self.svc.db, &p.league_id, &p.roster_id).await?;
        ok(self.svc.roster_with_names(&p.league_id, rid, p.force_refresh).await.map_err(err)?)
    }

    #[tool(description = "Analyze roster needs: positional strengths, weaknesses, injury risks, and priorities")]
    async fn analyze_roster_needs(
        &self,
        #[tool(aggr)] p: RosterParams,
    ) -> Result<CallToolResult, McpError> {
        let rid = resolve(&self.svc.db, &p.league_id, &p.roster_id).await?;
        ok(self.svc.analyze_roster_needs(&p.league_id, rid, p.force_refresh).await.map_err(err)?)
    }

    #[tool(description = "Get unrostered players ranked by season fantasy points — useful for waivers and draft prep")]
    async fn get_available_players(
        &self,
        #[tool(aggr)] p: AvailableParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.available_players(
            &p.league_id,
            p.position.as_deref(),
            p.season.as_deref(),
            p.limit,
        ).await.map_err(err)?)
    }

    #[tool(description = "Dynasty roster report: classifies each player into tiers (cornerstone/keeper/trade/cut) based on age and team situation, and surfaces young unrostered targets. Run this each offseason to decide who to keep, trade, and target.")]
    async fn dynasty_report(
        &self,
        #[tool(aggr)] p: DynastyReportParams,
    ) -> Result<CallToolResult, McpError> {
        let rid = resolve(&self.svc.db, &p.league_id, &p.roster_id).await?;
        ok(self.svc.dynasty_report(&p.league_id, rid).await.map_err(err)?)
    }

    #[tool(description = "Dynasty waiver scouting: returns young unrostered players (age ≤ max_age), ranked by Sleeper's relevance signal (search_rank) with age as a tiebreak, optionally filtered by position. dynasty_tier alone does NOT differentiate same-age rookies — they all collapse to Tier 1 — so use list order / search_rank, not tier, to judge relative prospect quality. Use this to find add candidates for dynasty leagues.")]
    async fn dynasty_scout(
        &self,
        #[tool(aggr)] p: DynastyScoutParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.dynasty_targets(
            &p.league_id,
            p.position.as_deref(),
            p.max_age,
            p.limit,
        ).await.map_err(err)?)
    }

    #[tool(description = "Look up real dynasty trade values (FantasyCalc, market-consensus) for Sleeper player IDs and/or draft picks — pass both sides of a proposed trade in one call to compare. Values auto-adjust for the league's actual superflex/PPR settings. Players: no dynasty value source (FantasyCalc, DynastyProcess, KeepTradeCut) prices IDP players — for a player not in FantasyCalc's own dataset (found:false), value is instead this project's own heuristic (value_source:heuristic_tier_median): the median FantasyCalc value of offensive players sharing that player's dynasty_tier. Picks: identify by season/round/original_roster_id (the pick's original owner, NOT necessarily who holds it now — ownership after trades is resolved internally). FantasyCalc only prices a generic year+round slot, not a specific team's pick, so for the one upcoming draft year it splits into Early/Mid/Late terciles, value_source:pick_slot_projection means this project projected which tercile the current owner's pick lands in from this league's own current standings (a projection that will move as standings change, and carries extra caveats if that roster is still playoff-eligible — read the `note` field); value_source:pick_slot_generic means it's too far out to project a slot, just the plain year+round value; value_source:unpriced means outside FantasyCalc's coverage entirely. Check value_source on every row — never assume a market price.")]
    async fn get_trade_value(
        &self,
        #[tool(aggr)] p: TradeValueParams,
    ) -> Result<CallToolResult, McpError> {
        let picks: Vec<crate::analysis::PickDescriptor> = p
            .picks
            .into_iter()
            .map(|pp| crate::analysis::PickDescriptor {
                season: pp.season,
                round: pp.round,
                original_roster_id: pp.original_roster_id,
            })
            .collect();
        ok(self.svc.trade_value_check(&p.league_id, &p.player_ids, &picks).await.map_err(err)?)
    }

    #[tool(description = "Full report for specific players: current Sleeper status/injury/practice-participation fields, aggregated recent news (RotoWire + ESPN), dynasty tier, and season production trend (PPG, games played, trend arrow). League-agnostic. Use search_players first if you only have a name, not a player_id.")]
    async fn get_player_report(
        &self,
        #[tool(aggr)] p: PlayerReportParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.player_report(&p.player_ids, p.season.as_deref()).await.map_err(err)?)
    }

    #[tool(description = "Scan all rosters and return win-win trade proposals: situations where you have what another team needs AND they have what you need. Returns proposals ranked by mutual benefit score.")]
    async fn find_win_win_trades(
        &self,
        #[tool(aggr)] p: FindTradesParams,
    ) -> Result<CallToolResult, McpError> {
        let rid = resolve(&self.svc.db, &p.league_id, &p.roster_id).await?;
        ok(self.svc.find_win_win_trades(&p.league_id, rid, p.limit).await.map_err(err)?)
    }

    #[tool(description = "Fetch league settings from the Sleeper API and store them locally. Run once per league to enable offline league-info queries.")]
    async fn sync_league(
        &self,
        #[tool(aggr)] p: LeagueIdParams,
    ) -> Result<CallToolResult, McpError> {
        let league = self.svc.client.get_league(&p.league_id).await.map_err(err)?;
        self.svc.db.upsert_league(&league).await.map_err(err)?;
        ok(serde_json::json!({
            "stored": true,
            "league_id": league.league_id,
            "name": league.name,
            "scoring": league.scoring_type(),
            "is_superflex": league.is_superflex(),
            "roster_positions": league.roster_positions,
        }))
    }

    #[tool(description = "Show locally stored league settings: roster positions, scoring format, superflex status. Requires sync_league to have been run first.")]
    async fn get_league_info(
        &self,
        #[tool(aggr)] p: LeagueIdParams,
    ) -> Result<CallToolResult, McpError> {
        match self.svc.db.get_league(&p.league_id).await.map_err(err)? {
            None => ok(serde_json::json!({
                "error": format!("League {} not found locally. Call sync_league first.", p.league_id)
            })),
            Some(league) => ok(serde_json::json!({
                "league_id": league.league_id,
                "name": league.name,
                "season": league.season,
                "status": league.status,
                "scoring": league.scoring_type(),
                "total_rosters": league.total_rosters,
                "is_superflex": league.is_superflex(),
                "roster_positions": league.roster_positions,
                "roster_slot_counts": league.roster_slot_counts(),
                "draft_rounds": league.settings.draft_rounds,
                "draft_id": league.draft_id,
            })),
        }
    }

    #[tool(description = "LIVE WRITE ACTION — submits a real waiver claim or free-agent add against sleeper.com (not a simulation: spends FAAB and changes the live roster immediately). Launches a headless Chrome using the operator's already-authenticated profile; requires `sleeper browser login` to have been run by hand at least once first. Returns an error (never a false success) if the session is not logged in or the live UI has changed since the last verification.")]
    async fn waiver_claim(
        &self,
        #[tool(aggr)] p: WaiverClaimParams,
    ) -> Result<CallToolResult, McpError> {
        let req = crate::cli::browser::ClaimRequest {
            league: &p.league_id,
            player_id: &p.player_id,
            drop_id: p.drop_player_id.as_deref(),
            bid: p.bid,
        };
        crate::cli::browser::run_claim(&self.config, &self.svc.client, req)
            .await
            .map_err(err)?;
        ok(serde_json::json!({ "claim_submitted": true, "league_id": p.league_id, "player_id": p.player_id }))
    }

    #[tool(description = "LIVE WRITE ACTION — swaps two roster slots against sleeper.com (not a simulation: changes the live roster immediately with no confirm step). Covers starter<->bench swaps AND IR/Taxi moves (live-verified 2026-11-11) — the DOM mechanism has no slot-type restriction. Pass `out` as whoever currently occupies the target slot (a started player, or an IR/Taxi occupant) and `in` as the player moving into it (a bench player, including one moving onto IR/Taxi), or the reverse to activate someone back to the bench. Launches a headless Chrome using the operator's already-authenticated profile; requires `sleeper browser login` to have been run by hand at least once first. Returns an error (never a false success) if the session is not logged in, the in-player is not eligible for that slot, or the live UI has changed since the last verification.")]
    async fn lineup_set(
        &self,
        #[tool(aggr)] p: LineupSetParams,
    ) -> Result<CallToolResult, McpError> {
        crate::cli::browser::run_lineup_set(
            &self.config,
            &self.svc.client,
            &p.league_id,
            &p.out_player_id,
            &p.in_player_id,
        )
        .await
        .map_err(err)?;
        ok(serde_json::json!({
            "lineup_saved": true,
            "league_id": p.league_id,
            "out_player_id": p.out_player_id,
            "in_player_id": p.in_player_id,
        }))
    }

    #[tool(description = "Read-only: your own pending waiver claims (player, FAAB bid, drop if any, status). The public REST transactions endpoint never exposes pending claims — this reads through the operator's authenticated browser session instead. Requires `sleeper browser login` to have been run by hand at least once first.")]
    async fn get_pending_waivers(
        &self,
        #[tool(aggr)] p: PendingWaiversParams,
    ) -> Result<CallToolResult, McpError> {
        let txns =
            crate::cli::browser::run_pending_transactions(&self.config, &p.league_id, p.roster_id, &["waiver"])
                .await
                .map_err(err)?;
        ok(serde_json::json!({ "league_id": p.league_id, "roster_id": p.roster_id, "pending_waivers": txns }))
    }

    #[tool(description = "Read-only: every player currently on another manager's Trade Block in this league (player, position, team, which roster flagged them, when). The public REST API exposes no trade-block data at all — this reads through the operator's authenticated browser session, same mechanism as get_pending_waivers. Requires `sleeper browser login` to have been run by hand at least once first. Not a write action — does not propose or accept any trade.")]
    async fn get_trade_block(
        &self,
        #[tool(aggr)] p: TradeBlockParams,
    ) -> Result<CallToolResult, McpError> {
        let players = crate::cli::browser::run_trade_block(&self.config, &self.svc, &p.league_id)
            .await
            .map_err(err)?;
        ok(serde_json::json!({ "league_id": p.league_id, "trade_block": players }))
    }

    #[tool(description = "Persist a dated analysis finding (e.g. a standings check or trade-scan review) to the local stats DB so it survives past this chat session, for a future session to read back with get_analysis_notes. Not a Sleeper write action.")]
    async fn log_analysis_note(
        &self,
        #[tool(aggr)] p: LogAnalysisNoteParams,
    ) -> Result<CallToolResult, McpError> {
        let id = self
            .svc
            .db
            .log_analysis_note(&p.league_id, &p.topic, &p.summary, p.detail.as_deref())
            .await
            .map_err(err)?;
        ok(serde_json::json!({ "id": id }))
    }

    #[tool(description = "Read back previously logged analysis notes (see log_analysis_note), most recent first. Omit league_id to return notes across every league.")]
    async fn get_analysis_notes(
        &self,
        #[tool(aggr)] p: GetAnalysisNotesParams,
    ) -> Result<CallToolResult, McpError> {
        let notes = self.svc.db.list_analysis_notes(p.league_id.as_deref(), p.limit).await.map_err(err)?;
        ok(serde_json::json!({ "notes": notes.iter().map(|n| serde_json::json!({
            "id": n.id,
            "league_id": n.league_id,
            "created_at": n.created_at,
            "topic": n.topic,
            "summary": n.summary,
            "detail": n.detail,
        })).collect::<Vec<_>>() }))
    }

    #[tool(description = "Read-only: transaction history for a league (waiver claims, trades, free-agent adds) served from the local unified cache, refreshing live only when a row's status/age genuinely warrants it (a pending claim inside the league's Tuesday waiver-processing window, or a never-synced league) rather than on a fixed interval. Returns structured JSON per row, including `source` (\"rest\" or \"graphql\") and `fetched_at` (when this tool's own cache last wrote the row) — never a formatted sentence. Omit `max_age_secs` to use this per-row default; pass `max_age_secs: 0` to force a live refresh.")]
    async fn get_transaction_history(
        &self,
        #[tool(aggr)] p: TransactionHistoryParams,
    ) -> Result<CallToolResult, McpError> {
        let league = self.svc.client.get_league(&p.league_id).await.map_err(err)?;
        let (waiver_day_of_week, daily_waivers) =
            crate::cli::browser::transactions::waiver_window_fields(&league);
        let rows = crate::cli::browser::transactions::transactions_for_display(
            &self.config,
            &self.svc,
            &p.league_id,
            p.kind_filter.as_deref(),
            p.max_age_secs,
            false, // no separate "force" param at this surface — max_age_secs: 0 does the same job
            waiver_day_of_week,
            daily_waivers,
        )
        .await
        .map_err(err)?;
        let transactions: Vec<serde_json::Value> =
            rows.iter().map(crate::cli::browser::transactions::transaction_to_json).collect();
        ok(serde_json::json!({ "league_id": p.league_id, "transactions": transactions }))
    }

    #[tool(description = "FAAB bid-projection: the five-signal model (perceived value, league demand, this league's own bid history, target-roster claim frequency, FAAB remaining) for \"what should I bid\" on a waiver target. Returns the full BidProjection object verbatim — every signal and its sample size/confidence, never a single opaque score or a pre-rendered sentence — so the caller can defend the number rather than re-derive it. Reads cached signals only; never launches a browser.")]
    async fn project_waiver_bid(
        &self,
        #[tool(aggr)] p: BidProjectionParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.project_waiver_bid(&p.league_id, &p.player_id, p.roster_id).await.map_err(err)?)
    }

    #[tool(description = "LIVE WRITE ACTION — edits the FAAB bid on a real pending waiver claim against sleeper.com (not a simulation: changes the live bid immediately, with no confirm step). Launches a headless Chrome using the operator's already-authenticated profile; requires `sleeper browser login` to have been run by hand at least once first. Returns an error (never a false success) if the claim is no longer pending, the session is not logged in, the target claim could not be matched to exactly one row in the live claims panel, the claims panel's expected structure could not be found at all (the live UI may have changed since the last verification), the target row moved before the write could fire (safe — nothing changed), or the outcome is unknown (e.g. a timeout between the DOM click and confirmation) — and on any error, re-check pending-claim state via get_transaction_history/get_pending_waivers before retrying, since the write may already have landed even when this tool can't confirm it.")]
    async fn edit_waiver_claim(
        &self,
        #[tool(aggr)] p: EditWaiverClaimParams,
    ) -> Result<CallToolResult, McpError> {
        let action = crate::browser::session::EditCancelAction::EditBid(p.new_bid);
        let outcome =
            crate::cli::browser::run_edit_cancel_claim(&self.config, &self.svc, &p.league_id, &p.transaction_id, action)
                .await;
        match crate::cli::browser::render_edit_cancel_outcome(outcome, action) {
            Ok(_detail) => ok(serde_json::json!({
                "claim_bid_updated": true,
                "league_id": p.league_id,
                "transaction_id": p.transaction_id,
                "new_bid": p.new_bid,
            })),
            Err(triage) => Err(McpError::internal_error(triage, None)),
        }
    }

    #[tool(description = "LIVE WRITE ACTION — cancels a real pending waiver claim against sleeper.com (not a simulation: frees the FAAB that claim had committed, immediately, with no confirm step). Launches a headless Chrome using the operator's already-authenticated profile; requires `sleeper browser login` to have been run by hand at least once first. Returns an error (never a false success) if the claim is no longer pending, the session is not logged in, the target claim could not be matched to exactly one row in the live claims panel, the claims panel's expected structure could not be found at all (the live UI may have changed since the last verification), the target row moved before the write could fire (safe — nothing changed), or the outcome is unknown (e.g. a timeout between the DOM click and confirmation) — and on any error, re-check pending-claim state via get_transaction_history/get_pending_waivers before retrying, since the write may already have landed even when this tool can't confirm it.")]
    async fn cancel_waiver_claim(
        &self,
        #[tool(aggr)] p: CancelWaiverClaimParams,
    ) -> Result<CallToolResult, McpError> {
        let action = crate::browser::session::EditCancelAction::Cancel;
        let outcome =
            crate::cli::browser::run_edit_cancel_claim(&self.config, &self.svc, &p.league_id, &p.transaction_id, action)
                .await;
        match crate::cli::browser::render_edit_cancel_outcome(outcome, action) {
            Ok(_detail) => ok(serde_json::json!({
                "claim_cancelled": true,
                "league_id": p.league_id,
                "transaction_id": p.transaction_id,
            })),
            Err(triage) => Err(McpError::internal_error(triage, None)),
        }
    }
}

#[tool(tool_box)]
impl ServerHandler for SleeperMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation {
                name: "sleeper-mcp".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::bid_projection::{
        BidHistorySignal, BidProjection, ClaimFrequencySignal, CoverageConfidence, FaabRemaining,
        LeagueDemandSignal, PerceivedValueSignal,
    };
    use std::collections::BTreeMap;

    /// Hand-built `BidProjection`, matching ux.md §4's sample shape.
    /// `project_waiver_bid` itself wraps `AnalysisService::project_waiver_bid`
    /// — a REST-fetching wrapper (bid_projection.rs's own testability note)
    /// this sandbox has no network for — so this proves the one
    /// network-independent part of that tool: `ok::<BidProjection>`'s
    /// serialization, the exact call this tool's body makes.
    // kibitzer:ignore long-function -- one flat struct literal, one field
    // per line (test fixture, same shape as bid_projection.rs's own
    // `sample_projection`); splitting it would only move the field count
    // around, not reduce it.
    fn sample_projection() -> BidProjection {
        // kibitzer:ignore duplicate-code-cross-file -- a private per-module
        // `#[cfg(test)]` fixture, same as bid_projection.rs's own
        // `sample_projection`/dynasty_cmds.rs's copy; each module's test
        // code is its own compilation unit, and a shared `pub(crate)`
        // test-fixtures module is out of this task's file scope
        // (src/cli/mod.rs, src/cli/dynasty_cmds.rs, this file).
        BidProjection {
            player_id: "3257".to_string(),
            roster_id: 5,
            perceived_value: PerceivedValueSignal {
                player_id: "3257".to_string(),
                position: "QB".to_string(),
                dynasty_tier: 2,
                search_rank: Some(48),
            },
            league_demand: LeagueDemandSignal {
                position: "QB".to_string(),
                pull_by_roster: BTreeMap::from([(5, 0), (9, 2)]),
            },
            bid_history: BidHistorySignal {
                mean_winning_bid: 22.0,
                sample_size: 11,
                position: "QB".to_string(),
            },
            claim_frequency: ClaimFrequencySignal {
                roster_id: 5,
                claims_this_season: 6,
                position: Some("QB".to_string()),
            },
            faab_remaining: FaabRemaining {
                amount: 142,
                confidence: CoverageConfidence::Verified { as_of: chrono::Utc::now() },
            },
            competing_roster_faab: vec![(
                9,
                FaabRemaining {
                    amount: 34,
                    confidence: CoverageConfidence::Verified { as_of: chrono::Utc::now() },
                },
            )],
            suggested_bid: 23,
            suggested_bid_pct_of_remaining: 23.0 / 142.0,
            low_confidence: false,
        }
    }

    /// Story 3.2.2 AC: the JSON result must carry every `BidProjection`
    /// field, never the pre-rendered `render_short()` string (ux.md §2 —
    /// "don't hide the breakdown from the assistant that might need to
    /// defend the number").
    #[test]
    fn project_waiver_bid_tool_should_return_full_structured_object_when_called_via_mcp() {
        let projection = sample_projection();
        let short_line = projection.render_short();

        let result = ok(projection).expect("serializing a BidProjection must succeed");
        let text = result.content[0]
            .as_text()
            .expect("tool result must carry a text content block")
            .text
            .clone();
        let json: serde_json::Value = serde_json::from_str(&text).expect("content must be valid JSON");

        for field in [
            "player_id",
            "roster_id",
            "perceived_value",
            "league_demand",
            "bid_history",
            "claim_frequency",
            "faab_remaining",
            "competing_roster_faab",
            "suggested_bid",
            "suggested_bid_pct_of_remaining",
            "low_confidence",
        ] {
            assert!(json.get(field).is_some(), "missing field `{field}` in {json}");
        }
        assert_ne!(
            text, short_line,
            "the MCP tool must never return the pre-rendered short-line string"
        );
    }

    /// Story 3.2.2 AC, error path: a missing required `player_id` must fail
    /// parameter deserialization — the same mechanism `#[tool(aggr)]` uses
    /// to build `p` before the tool body (and any DB/analysis call) runs —
    /// never reach `AnalysisService::project_waiver_bid`.
    #[test]
    fn bid_projection_params_should_fail_deserialization_when_player_id_param_missing() {
        let value = serde_json::json!({ "league_id": "1352883872131481600", "roster_id": 5 });
        let result: std::result::Result<BidProjectionParams, _> = serde_json::from_value(value);
        assert!(
            result.is_err(),
            "missing player_id must be a validation Err before any DB/analysis call, got {result:?}"
        );
    }

    /// Serializes `projection` the same way the tool body does (`ok(...)`)
    /// and parses the result back into a `Value` for shape assertions.
    fn projection_json(projection: BidProjection) -> serde_json::Value {
        let result = ok(projection).expect("serializing a BidProjection must succeed");
        let text = result.content[0]
            .as_text()
            .expect("tool result must carry a text content block")
            .text
            .clone();
        serde_json::from_str(&text).expect("content must be valid JSON")
    }

    /// validation.md Surface 4, AC2: `coverage`/`confidence` fields must be a
    /// tagged variant (`{"kind": "verified"|"floor", ...}`), never a bare
    /// bool — the whole point of `CoverageConfidence` replacing a bare
    /// `is_complete: bool` (plan.md Pattern Decisions: "FAAB-remaining
    /// confidence representation").
    #[test]
    fn project_waiver_bid_tool_coverage_field_should_be_tagged_variant_not_bare_bool() {
        let verified = sample_projection();
        let json = projection_json(verified);
        let confidence = &json["faab_remaining"]["confidence"];
        assert!(
            confidence.is_object(),
            "confidence must be a tagged object, got {confidence}"
        );
        assert_eq!(confidence["kind"], "verified");
        assert!(confidence.get("as_of").is_some());

        let mut floor = sample_projection();
        floor.faab_remaining = FaabRemaining {
            amount: 58,
            confidence: CoverageConfidence::Floor {
                reason: "no full-league GraphQL backfill recorded for season 2026".to_string(),
            },
        };
        let json = projection_json(floor);
        let confidence = &json["faab_remaining"]["confidence"];
        assert!(
            confidence.is_object(),
            "confidence must be a tagged object, got {confidence}"
        );
        assert_eq!(confidence["kind"], "floor");
        assert!(confidence.get("reason").is_some());
        assert_ne!(
            confidence, &serde_json::json!(true),
            "coverage must never collapse to a bare bool"
        );
        assert_ne!(confidence, &serde_json::json!(false));
    }

    /// validation.md Surface 4, AC3: walk the full JSON response and assert
    /// no field named `confidence` anywhere (top-level or nested, including
    /// `competing_roster_faab`) is a bare numeric value unaccompanied by a
    /// sample/range — every `confidence` must be the tagged
    /// `CoverageConfidence` object.
    #[test]
    fn project_waiver_bid_tool_should_never_emit_bare_confidence_float_in_response() {
        fn assert_no_bare_confidence_number(value: &serde_json::Value) {
            match value {
                serde_json::Value::Object(map) => {
                    for (key, v) in map {
                        if key == "confidence" {
                            assert!(
                                !v.is_number(),
                                "found a bare numeric `confidence` field: {v} (full value: {value})"
                            );
                        }
                        assert_no_bare_confidence_number(v);
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        assert_no_bare_confidence_number(item);
                    }
                }
                _ => {}
            }
        }

        let mut projection = sample_projection();
        projection.faab_remaining = FaabRemaining {
            amount: 0,
            confidence: CoverageConfidence::Floor { reason: "zero coverage".to_string() },
        };
        let json = projection_json(projection);
        assert_no_bare_confidence_number(&json);
    }

    /// validation.md Surface 4, AC4: `bid_history.sample_size` names a
    /// concrete count (an integer field), never folded into a vague
    /// percentage/confidence description.
    #[test]
    fn project_waiver_bid_tool_sample_field_should_name_concrete_count_not_percentage() {
        let mut projection = sample_projection();
        projection.bid_history = BidHistorySignal {
            mean_winning_bid: 22.0,
            sample_size: 11,
            position: "QB".to_string(),
        };
        let json = projection_json(projection);
        let sample_size = &json["bid_history"]["sample_size"];
        assert!(
            sample_size.is_u64(),
            "sample_size must be a concrete integer count, got {sample_size}"
        );
        assert_eq!(sample_size.as_u64(), Some(11));
        // A percentage/score would be a fraction in [0.0, 1.0] or a float —
        // the real field is a whole bid count, not a confidence score.
        assert!(!sample_size.is_f64());
    }

    /// validation.md Surface 8, AC1, reworded: validation.md's literal name
    /// (`..._should_report_null_remaining_with_floor_reason_when_zero_coverage`)
    /// implied `faab_remaining.amount` should be `null`/absent under `Floor`
    /// confidence. That contradicts plan.md's authoritative Story 3.1.3 AC
    /// (`FaabRemaining { amount, confidence }`, `amount: i64` always a real
    /// computed number) and the Pattern Decisions table's explicit "a bool
    /// loses *why*" rationale for choosing an enum `confidence` field over
    /// hiding the number — confirmed by reading `faab_remaining()`
    /// (`src/analysis/bid_projection.rs:473-500`), which always returns
    /// `Ok(FaabRemaining { amount: waiver_budget - spent, confidence })`
    /// regardless of coverage. Renamed to assert the actual contract: under
    /// `Floor`, `amount` is still a real `i64`, and `confidence` is a
    /// `Floor { reason }` variant structurally distinguishable from
    /// `Verified { as_of }` — a caller can't mistake a floor estimate for a
    /// verified one even though the number itself is never hidden.
    #[test]
    fn project_waiver_bid_tool_floor_confidence_should_report_real_amount_not_null() {
        let mut floor = sample_projection();
        floor.faab_remaining = FaabRemaining {
            amount: 58,
            confidence: CoverageConfidence::Floor {
                reason: "no full-league GraphQL backfill recorded for season 2026".to_string(),
            },
        };
        let json = projection_json(floor);
        let amount = &json["faab_remaining"]["amount"];
        assert!(!amount.is_null(), "Floor confidence must never null out `amount`");
        assert_eq!(amount.as_i64(), Some(58));
        assert_eq!(json["faab_remaining"]["confidence"]["kind"], "floor");
        assert!(json["faab_remaining"]["confidence"].get("reason").is_some());

        let mut verified = sample_projection();
        verified.faab_remaining = FaabRemaining {
            amount: 58,
            confidence: CoverageConfidence::Verified { as_of: chrono::Utc::now() },
        };
        let verified_json = projection_json(verified);
        assert_ne!(
            verified_json["faab_remaining"]["confidence"]["kind"],
            json["faab_remaining"]["confidence"]["kind"],
            "Floor and Verified must be structurally distinguishable by `kind`, not just by amount"
        );
    }

    /// validation.md Surface 8, AC2: when `faab_remaining`'s coverage is
    /// `Floor`, the other four signals (`perceived_value`, `league_demand`,
    /// `bid_history`, `claim_frequency`) still render with their real
    /// values — degradation is scoped to the one dependent signal, never the
    /// whole output.
    #[test]
    fn project_waiver_bid_tool_should_render_other_signals_normally_when_only_faab_coverage_missing()
    {
        let mut projection = sample_projection();
        projection.faab_remaining = FaabRemaining {
            amount: 0,
            confidence: CoverageConfidence::Floor {
                reason: "no full-league GraphQL backfill recorded for season 2026".to_string(),
            },
        };
        let json = projection_json(projection);

        assert_eq!(json["perceived_value"]["dynasty_tier"], 2);
        assert_eq!(json["perceived_value"]["search_rank"], 48);
        assert_eq!(json["league_demand"]["pull_by_roster"]["9"], 2);
        assert_eq!(json["bid_history"]["mean_winning_bid"], 22.0);
        assert_eq!(json["bid_history"]["sample_size"], 11);
        assert_eq!(json["claim_frequency"]["claims_this_season"], 6);
        assert_eq!(json["faab_remaining"]["confidence"]["kind"], "floor");
    }

    /// validation.md Surface 8, AC3, reworded: validation.md's literal name
    /// (`..._should_distinguish_partial_coverage_reason_from_zero_history_reason`)
    /// implies `faab_remaining()` produces two different `Floor` reason
    /// strings depending on whether a roster has partial vs. zero synced
    /// history. Reading `faab_remaining()` in full
    /// (`src/analysis/bid_projection.rs:473-500`) and plan.md's Story 3.1.3
    /// AC (line 803-809) shows this distinction was never implemented or
    /// specified there: the `Floor` reason is gated solely on
    /// `has_full_league_graphql_coverage` returning `None`, and is always
    /// exactly `"no full-league GraphQL backfill recorded for season
    /// {season}"` regardless of partial vs. zero history — only ux.md §8
    /// (design aspiration, not plan.md's authoritative AC) describes the
    /// finer-grained distinction. This is the same category of validation.md
    /// imprecision as the already-flagged null-remaining test, not a
    /// bid_projection.rs logic bug, so per that same guidance I reworded this
    /// test to the property this task's prompt actually asked for instead:
    /// `low_confidence` and `faab_remaining.confidence` are two independent,
    /// separately-inspectable fields, not the same value duplicated under
    /// two names.
    #[test]
    fn project_waiver_bid_tool_low_confidence_flag_and_faab_confidence_should_be_independent_fields()
    {
        // low_confidence: false, faab_remaining.confidence: Floor — proves
        // `low_confidence` isn't just a mirror of `confidence.is_floor()`.
        let mut floor_but_not_low_confidence = sample_projection();
        floor_but_not_low_confidence.faab_remaining = FaabRemaining {
            amount: 58,
            confidence: CoverageConfidence::Floor { reason: "no backfill".to_string() },
        };
        floor_but_not_low_confidence.low_confidence = false;
        let json = projection_json(floor_but_not_low_confidence);
        assert_eq!(json["low_confidence"], false);
        assert_eq!(json["faab_remaining"]["confidence"]["kind"], "floor");

        // low_confidence: true, faab_remaining.confidence: Verified — proves
        // the inverse: `low_confidence` can be driven by a different signal
        // entirely (e.g. a thin `bid_history.sample_size`), independent of
        // FAAB coverage.
        let mut low_confidence_but_verified_faab = sample_projection();
        low_confidence_but_verified_faab.faab_remaining = FaabRemaining {
            amount: 58,
            confidence: CoverageConfidence::Verified { as_of: chrono::Utc::now() },
        };
        low_confidence_but_verified_faab.low_confidence = true;
        let json = projection_json(low_confidence_but_verified_faab);
        assert_eq!(json["low_confidence"], true);
        assert_eq!(json["faab_remaining"]["confidence"]["kind"], "verified");
    }

    // -----------------------------------------------------------------
    // Epic 4.3 / Task 4.3.2a: `edit_waiver_claim`/`cancel_waiver_claim`'s
    // outcome mapping — proven against the shared renderer
    // (`render_edit_cancel_outcome`) directly, the exact call both tool
    // bodies make, since the full tool call always launches a real browser
    // (no live Chrome in this sandbox — the same documented gap
    // `waiver_claim`/`lineup_set` already carry).
    // -----------------------------------------------------------------

    use crate::browser::session::{EditCancelAction, EditCancelOutcome};
    use crate::cli::browser::{render_edit_cancel_outcome, EditCancelRunOutcome};

    /// Mirrors exactly what `edit_waiver_claim`/`cancel_waiver_claim`'s tool
    /// bodies do with `render_edit_cancel_outcome`'s result — one copy
    /// source (the renderer), proven here to map correctly into `McpError`.
    fn to_mcp_result(
        outcome: crate::error::Result<EditCancelRunOutcome>,
        action: EditCancelAction,
    ) -> std::result::Result<(), McpError> {
        match render_edit_cancel_outcome(outcome, action) {
            Ok(_detail) => Ok(()),
            Err(triage) => Err(McpError::internal_error(triage, None)),
        }
    }

    #[test]
    fn only_confirmed_detail_maps_to_an_mcp_ok_result() {
        assert!(
            to_mcp_result(
                Ok(EditCancelRunOutcome::Confirmed { detail: "cancelled".to_string() }),
                EditCancelAction::Cancel,
            )
            .is_ok(),
            "Confirmed{{detail}} must map to Ok"
        );

        let non_success = [
            to_mcp_result(Ok(EditCancelRunOutcome::ConfirmedButUnverified), EditCancelAction::Cancel),
            to_mcp_result(
                Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::Confirmed)),
                EditCancelAction::Cancel,
            ),
            to_mcp_result(
                Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::Ambiguous { reason: "r".to_string() })),
                EditCancelAction::Cancel,
            ),
            to_mcp_result(
                Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::AbortedBeforeWrite {
                    reason: "r".to_string(),
                })),
                EditCancelAction::Cancel,
            ),
            to_mcp_result(Err(crate::browser::other("already-formed triage text")), EditCancelAction::Cancel),
        ];
        for result in non_success {
            assert!(result.is_err(), "every non-Confirmed{{detail}} outcome must map to Err, never Ok");
        }
    }

    #[test]
    fn ambiguous_and_aborted_before_write_map_to_genuinely_different_mcp_error_text() {
        let same_reason = "row no longer matched by the time the write-stage eval_string ran";
        let ambiguous = to_mcp_result(
            Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::Ambiguous { reason: same_reason.to_string() })),
            EditCancelAction::Cancel,
        )
        .unwrap_err();
        let aborted = to_mcp_result(
            Ok(EditCancelRunOutcome::SeamOutcome(EditCancelOutcome::AbortedBeforeWrite {
                reason: same_reason.to_string(),
            })),
            EditCancelAction::Cancel,
        )
        .unwrap_err();
        assert_ne!(
            ambiguous.message, aborted.message,
            "Ambiguous (re-check before retrying) and AbortedBeforeWrite (safe, nothing changed) \
             must never produce the same McpError text, even given the identical underlying reason"
        );
        assert!(ambiguous.message.contains("outcome unknown"));
        assert!(aborted.message.contains("nothing was clicked"));
    }

    #[test]
    fn already_formed_err_text_passes_through_into_mcp_error_unmodified() {
        // `run_edit_cancel_claim`'s own `Err` is already ready-to-print
        // (the dead-session hint, the already-resolved triage, or its own
        // catch-all-wrapped generic-flow-failure triage) — the MCP mapping
        // layer must never re-wrap or alter it.
        let ready_text = "Edit/cancel failed — the live claims panel may have changed since the last verification, or the selector bundle needs live re-verification. Re-run with SLEEPER_BROWSER_DEBUG=1 for detail.";
        let mapped = to_mcp_result(Err(crate::browser::other(ready_text)), EditCancelAction::Cancel).unwrap_err();
        assert_eq!(mapped.message, ready_text);
    }

    #[test]
    fn edit_waiver_claim_params_should_fail_deserialization_when_new_bid_param_missing() {
        let value = serde_json::json!({ "league_id": "1352883872131481600", "transaction_id": "1" });
        let result: std::result::Result<EditWaiverClaimParams, _> = serde_json::from_value(value);
        assert!(
            result.is_err(),
            "missing new_bid must be a validation Err before any browser launch, got {result:?}"
        );
    }
}
