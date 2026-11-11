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
struct TradeValueParams {
    league_id: String,
    /// Sleeper player IDs to look up (mix both sides of a proposed trade in one call)
    player_ids: Vec<String>,
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

/// LIVE WRITE ACTION: swaps a bench player into a started slot against
/// sleeper.com through the stealth-browser backend (plan.md
/// undetected-chromedriver-rust, Story 4.1) on the operator's own persisted,
/// manually-authenticated Chrome profile. Not a simulation — this changes
/// the live starting lineup immediately with no confirm step. Requires
/// `sleeper browser login` to have been completed by hand at least once
/// beforehand.
#[derive(Deserialize, JsonSchema)]
struct LineupSetParams {
    league_id: String,
    /// Sleeper player_id of the currently-started player to bench.
    out_player_id: String,
    /// Sleeper player_id of the bench player to start in their place.
    in_player_id: String,
}

#[derive(Deserialize, JsonSchema)]
struct PlayerReportParams {
    /// Sleeper player IDs — use search_players first if you only have a name
    player_ids: Vec<String>,
    /// NFL season, e.g. "2026". Defaults to the live season if omitted.
    season: Option<String>,
}

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

    #[tool(description = "Get roster details with player names, positions, and injury status")]
    async fn get_roster_with_names(
        &self,
        #[tool(aggr)] p: RosterParams,
    ) -> Result<CallToolResult, McpError> {
        let rid = resolve(&self.svc.db, &p.league_id, &p.roster_id).await?;
        ok(self.svc.roster_with_names(&p.league_id, rid).await.map_err(err)?)
    }

    #[tool(description = "Analyze roster needs: positional strengths, weaknesses, injury risks, and priorities")]
    async fn analyze_roster_needs(
        &self,
        #[tool(aggr)] p: RosterParams,
    ) -> Result<CallToolResult, McpError> {
        let rid = resolve(&self.svc.db, &p.league_id, &p.roster_id).await?;
        ok(self.svc.analyze_roster_needs(&p.league_id, rid).await.map_err(err)?)
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

    #[tool(description = "Look up real dynasty trade values (FantasyCalc, market-consensus) for a list of Sleeper player IDs — pass both sides of a proposed trade in one call to compare. Values auto-adjust for the league's actual superflex/PPR settings. Does NOT cover IDP players (they return found:false) — use dynasty_tier/search_rank reasoning for those instead.")]
    async fn get_trade_value(
        &self,
        #[tool(aggr)] p: TradeValueParams,
    ) -> Result<CallToolResult, McpError> {
        ok(self.svc.trade_value_check(&p.league_id, &p.player_ids).await.map_err(err)?)
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

    #[tool(description = "LIVE WRITE ACTION — swaps a bench player into a started slot against sleeper.com (not a simulation: changes the live starting lineup immediately with no confirm step). Launches a headless Chrome using the operator's already-authenticated profile; requires `sleeper browser login` to have been run by hand at least once first. Returns an error (never a false success) if the session is not logged in, the in-player is not eligible for that slot, or the live UI has changed since the last verification.")]
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
