/// Higher-level fantasy analytics built on SleeperClient.
///
/// All parallel API calls use `tokio::spawn` explicitly (goroutine semantics).
/// Each spawned task gets its own `Arc`-clone of the client.
///
/// Split by functional area — see each submodule's doc comment for what it covers.
/// Cross-submodule helpers (endpoint-fetch primitives/combos, `rostered_player_ids`,
/// `best_cached_season`) live here since every submodule needs at least one of them.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;

use crate::client::SleeperClient;
use crate::db::StatsDb;
use crate::error::Result;
use crate::models::*;

// Epic 3.2: `pub(crate)` (not `pub use`-only) so `mcp.rs`'s test module can
// name the signal substructs directly to build a `BidProjection` fixture —
// no `Deserialize` derive exists on these types to build one any other way.
// Crate-internal only; the public surface is still just `BidProjection`
// itself, re-exported below.
pub(crate) mod bid_projection;
mod dynasty;
mod lineup;
mod picks;
mod players;
mod roster;
mod trades;
mod waivers;

pub use picks::PickDescriptor;

pub use bid_projection::BidProjection;
pub use players::{PlayerStats, StatsContext};
// Epic 2.2 / Task 2.2.1c: `resolve_side` lives in the private `roster`
// submodule but `run_league_transactions_sync` (src/cli/browser/transactions.rs)
// is outside the `analysis` module tree, so it needs this re-export to reach it.
pub(crate) use roster::resolve_side;

#[derive(Clone)]
pub struct AnalysisService {
    pub client: Arc<SleeperClient>,
    pub db: Arc<StatsDb>,
}

impl AnalysisService {
    pub fn new(client: SleeperClient, db: StatsDb) -> Self {
        Self {
            client: Arc::new(client),
            db: Arc::new(db),
        }
    }

    // -----------------------------------------------------------------------
    // Standings
    // -----------------------------------------------------------------------

    pub async fn standings(&self, league_id: &str) -> Result<Vec<serde_json::Value>> {
        let (rosters, users) = self.fetch_rosters_and_users(league_id).await?;
        let user_map: HashMap<_, _> = users
            .iter()
            .map(|u| (u.user_id.clone(), u.display_name.clone()))
            .collect();

        let mut standings: Vec<_> = rosters
            .iter()
            .map(|r| {
                let name = r
                    .owner_id
                    .as_deref()
                    .and_then(|id| user_map.get(id))
                    .cloned()
                    .unwrap_or_else(|| "Unknown".to_string());
                serde_json::json!({
                    "rank": 0,
                    "roster_id": r.roster_id,
                    "owner_id": r.owner_id,
                    "display_name": name,
                    "wins": r.settings.wins,
                    "losses": r.settings.losses,
                    "ties": r.settings.ties,
                    "fpts": r.settings.total_fpts(),
                    "fpts_against": r.settings.total_fpts_against(),
                })
            })
            .collect();

        standings.sort_by(|a, b| {
            let w = b["wins"].as_i64().cmp(&a["wins"].as_i64());
            if w != std::cmp::Ordering::Equal {
                return w;
            }
            b["fpts"]
                .as_f64()
                .partial_cmp(&a["fpts"].as_f64())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for (i, t) in standings.iter_mut().enumerate() {
            t["rank"] = serde_json::Value::from(i as i64 + 1);
        }
        Ok(standings)
    }

    // -----------------------------------------------------------------------
    // Matchups
    // -----------------------------------------------------------------------

    pub async fn matchup_overview(
        &self,
        league_id: &str,
        week: i32,
    ) -> Result<Vec<serde_json::Value>> {
        let client = self.client.clone();
        let lid = league_id.to_string();

        let matchups_h = {
            let c = client.clone();
            let l = lid.clone();
            tokio::spawn(async move { c.get_league_matchups(&l, week).await })
        };
        let rosters_h = self.spawn_rosters(league_id);
        let users_h = self.spawn_users(league_id);

        let matchups = matchups_h.await??;
        let rosters = rosters_h.await??;
        let users = users_h.await??;

        let roster_map: HashMap<_, _> = rosters.iter().map(|r| (r.roster_id, r)).collect();
        let user_map: HashMap<_, _> = users
            .iter()
            .map(|u| (u.user_id.clone(), u.display_name.clone()))
            .collect();

        let mut groups: HashMap<i32, Vec<&Matchup>> = HashMap::new();
        for m in &matchups {
            if let Some(mid) = m.matchup_id {
                groups.entry(mid).or_default().push(m);
            }
        }

        let mut result = Vec::new();
        let mut keys: Vec<_> = groups.keys().cloned().collect();
        keys.sort();

        for matchup_id in keys {
            let teams = &groups[&matchup_id];
            if teams.len() != 2 {
                continue;
            }
            let paired: Vec<_> = teams
                .iter()
                .map(|m| {
                    let owner_id = roster_map
                        .get(&m.roster_id)
                        .and_then(|r| r.owner_id.as_deref())
                        .unwrap_or("");
                    let name = user_map
                        .get(owner_id)
                        .cloned()
                        .unwrap_or_else(|| "Unknown".to_string());
                    serde_json::json!({
                        "roster_id": m.roster_id,
                        "display_name": name,
                        "points": m.points,
                        "starters": m.starters,
                    })
                })
                .collect();
            result.push(serde_json::json!({
                "matchup_id": matchup_id,
                "week": week,
                "team1": paired[0],
                "team2": paired[1],
            }));
        }
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Endpoint-fetch primitives — each spawns exactly one `tokio::spawn` task
    // (goroutine-style, per the module-level convention) and hands back its
    // `JoinHandle` uninverted so callers can spawn several before awaiting any,
    // preserving full concurrency across whichever combination they need.
    // -----------------------------------------------------------------------

    fn spawn_rosters(&self, league_id: &str) -> tokio::task::JoinHandle<Result<Vec<Roster>>> {
        let c = self.client.clone();
        let l = league_id.to_string();
        tokio::spawn(async move { c.get_league_rosters(&l).await })
    }

    fn spawn_users(&self, league_id: &str) -> tokio::task::JoinHandle<Result<Vec<User>>> {
        let c = self.client.clone();
        let l = league_id.to_string();
        tokio::spawn(async move { c.get_league_users(&l).await })
    }

    fn spawn_all_players(&self) -> tokio::task::JoinHandle<Result<HashMap<String, Value>>> {
        let c = self.client.clone();
        tokio::spawn(async move { c.get_all_players("nfl").await })
    }

    fn spawn_league(&self, league_id: &str) -> tokio::task::JoinHandle<Result<League>> {
        let c = self.client.clone();
        let l = league_id.to_string();
        tokio::spawn(async move { c.get_league(&l).await })
    }

    fn spawn_nfl_state(&self) -> tokio::task::JoinHandle<Result<NflState>> {
        let c = self.client.clone();
        tokio::spawn(async move { c.get_nfl_state().await })
    }

    async fn fetch_rosters_and_users(
        &self,
        league_id: &str,
    ) -> Result<(Vec<Roster>, Vec<User>)> {
        let rosters_h = self.spawn_rosters(league_id);
        let users_h = self.spawn_users(league_id);
        Ok((rosters_h.await??, users_h.await??))
    }

    /// Fetch rosters + users + the full NFL player index concurrently.
    async fn fetch_rosters_users_players(
        &self,
        league_id: &str,
    ) -> Result<(Vec<Roster>, Vec<User>, HashMap<String, Value>)> {
        let rosters_h = self.spawn_rosters(league_id);
        let users_h = self.spawn_users(league_id);
        let players_h = self.spawn_all_players();
        Ok((rosters_h.await??, users_h.await??, players_h.await??))
    }

    /// Fetch rosters + users + the full NFL player index + the league itself, concurrently.
    async fn fetch_rosters_users_players_league(
        &self,
        league_id: &str,
    ) -> Result<(Vec<Roster>, Vec<User>, HashMap<String, Value>, League)> {
        let rosters_h = self.spawn_rosters(league_id);
        let users_h = self.spawn_users(league_id);
        let players_h = self.spawn_all_players();
        let league_h = self.spawn_league(league_id);
        Ok((
            rosters_h.await??,
            users_h.await??,
            players_h.await??,
            league_h.await??,
        ))
    }

    /// Fetch rosters + users + the full NFL player index + current NFL state, concurrently.
    async fn fetch_rosters_users_players_state(
        &self,
        league_id: &str,
    ) -> Result<(Vec<Roster>, Vec<User>, HashMap<String, Value>, NflState)> {
        let rosters_h = self.spawn_rosters(league_id);
        let users_h = self.spawn_users(league_id);
        let players_h = self.spawn_all_players();
        let state_h = self.spawn_nfl_state();
        Ok((
            rosters_h.await??,
            users_h.await??,
            players_h.await??,
            state_h.await??,
        ))
    }

    /// Fetch rosters + the full NFL player index concurrently.
    async fn fetch_rosters_and_players(
        &self,
        league_id: &str,
    ) -> Result<(Vec<Roster>, HashMap<String, Value>)> {
        let rosters_h = self.spawn_rosters(league_id);
        let players_h = self.spawn_all_players();
        Ok((rosters_h.await??, players_h.await??))
    }

    /// Resolve the evaluation season (prefer prior season if cached) and build its
    /// stats context in one step — the repeated pairing across trade/dynasty analysis.
    async fn eval_season_stats(&self, current_season: &str) -> (String, StatsContext) {
        let eval_season = best_cached_season(&self.db, current_season).await;
        let stats_ctx = self.build_stats_context(&eval_season).await.unwrap_or_default();
        (eval_season, stats_ctx)
    }
}

/// Set of every player_id rostered by anyone in the league.
fn rostered_player_ids(rosters: &[Roster]) -> HashSet<String> {
    rosters.iter().flat_map(|r| r.players.clone()).collect()
}

/// Returns prev season if it has cached weeks, otherwise current season.
async fn best_cached_season(db: &StatsDb, current_season: &str) -> String {
    let prev = (current_season.parse::<i32>().unwrap_or(2025) - 1).to_string();
    if db.cached_weeks(&prev).await.map(|w| !w.is_empty()).unwrap_or(false) {
        prev
    } else {
        current_season.to_string()
    }
}
