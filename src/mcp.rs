/// MCP server tools exposing Sleeper analytics via the rmcp stdio transport.
use std::sync::Arc;

use rmcp::{
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo, Implementation},
    schemars::{self, JsonSchema},
    tool, Error as McpError, ServerHandler,
};
use serde::Deserialize;

use crate::analysis::AnalysisService;

#[derive(Clone)]
pub struct SleeperMcpServer {
    svc: Arc<AnalysisService>,
}

impl SleeperMcpServer {
    pub fn new(svc: AnalysisService) -> Self {
        Self { svc: Arc::new(svc) }
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
