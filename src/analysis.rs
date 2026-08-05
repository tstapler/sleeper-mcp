/// Higher-level fantasy analytics built on SleeperClient.
///
/// All parallel API calls use `tokio::spawn` explicitly (goroutine semantics).
/// Each spawned task gets its own `Arc`-clone of the client.
use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::client::SleeperClient;
use crate::db::StatsDb;
use crate::error::Result;
use crate::models::*;

/// Per-player season stats pre-computed for fast enrichment.
#[derive(Clone, Debug, Default)]
pub struct PlayerStats {
    pub season_pts: f64,
    pub games_played: usize,
    pub ppg: f64,
    pub recent_avg: f64,
    pub trend: &'static str,
}

pub type StatsContext = HashMap<String, PlayerStats>;

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
        let rosters_h = {
            let c = client.clone();
            let l = lid.clone();
            tokio::spawn(async move { c.get_league_rosters(&l).await })
        };
        let users_h = {
            let c = client.clone();
            let l = lid.clone();
            tokio::spawn(async move { c.get_league_users(&l).await })
        };

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
    // Roster with player names
    // -----------------------------------------------------------------------

    pub async fn roster_with_names(
        &self,
        league_id: &str,
        roster_id: i32,
    ) -> Result<serde_json::Value> {
        let client = self.client.clone();
        let lid = league_id.to_string();

        let rosters_h = {
            let c = client.clone();
            let l = lid.clone();
            tokio::spawn(async move { c.get_league_rosters(&l).await })
        };
        let users_h = {
            let c = client.clone();
            let l = lid.clone();
            tokio::spawn(async move { c.get_league_users(&l).await })
        };
        let players_h = {
            let c = client.clone();
            tokio::spawn(async move { c.get_all_players("nfl").await })
        };

        let rosters = rosters_h.await??;
        let users = users_h.await??;
        let all_players = players_h.await??;

        let roster = rosters
            .iter()
            .find(|r| r.roster_id == roster_id)
            .ok_or_else(|| crate::error::Error::NotFound(format!("roster {roster_id}")))?;

        let user_map: HashMap<_, _> = users
            .iter()
            .map(|u| (u.user_id.clone(), u.display_name.clone()))
            .collect();
        let owner_name = roster
            .owner_id
            .as_deref()
            .and_then(|id| user_map.get(id))
            .cloned()
            .unwrap_or_else(|| "Unknown".to_string());

        let enrich = |pid: &str| enrich_player(pid, &all_players);

        let mut players: Vec<_> = roster.players.iter().map(|p| enrich(p)).collect();
        players.sort_by_key(|p| {
            (
                p["position"].as_str().unwrap_or("").to_string(),
                p["name"].as_str().unwrap_or("").to_string(),
            )
        });

        Ok(serde_json::json!({
            "roster_id": roster_id,
            "owner": owner_name,
            "players": players,
            "starters": roster.starters.iter().map(|p| enrich(p)).collect::<Vec<_>>(),
            "reserve": roster.reserve.iter().map(|p| enrich(p)).collect::<Vec<_>>(),
            "record": {
                "wins": roster.settings.wins,
                "losses": roster.settings.losses,
                "ties": roster.settings.ties,
                "fpts": roster.settings.total_fpts(),
            },
        }))
    }

    // -----------------------------------------------------------------------
    // Roster needs analysis
    // -----------------------------------------------------------------------

    pub async fn analyze_roster_needs(
        &self,
        league_id: &str,
        roster_id: i32,
    ) -> Result<serde_json::Value> {
        let data = self.roster_with_names(league_id, roster_id).await?;

        let mut pos_counts: HashMap<String, usize> = HashMap::new();
        let mut injured: Vec<&Value> = Vec::new();
        let mut no_team: Vec<&Value> = Vec::new();

        let players = data["players"].as_array().cloned().unwrap_or_default();
        for p in &players {
            let pos = p["position"].as_str().unwrap_or("?").to_string();
            *pos_counts.entry(pos).or_insert(0) += 1;

            let inj = p["injury_status"].as_str().unwrap_or("");
            if !inj.is_empty() && inj != "Questionable" {
                injured.push(p);
            }
            if p["team"].as_str() == Some("FA") {
                no_team.push(p);
            }
        }

        let ideal: &[(&str, usize, usize)] = &[
            ("QB", 1, 2),
            ("RB", 4, 6),
            ("WR", 4, 6),
            ("TE", 1, 2),
            ("K", 0, 1),
            ("DEF", 0, 1),
        ];
        let mut strengths: Vec<String> = Vec::new();
        let mut weaknesses: Vec<String> = Vec::new();
        for (pos, low, high) in ideal {
            let count = *pos_counts.get(*pos).unwrap_or(&0);
            if count > *high {
                strengths.push(format!("{pos}: {count} players (surplus — trade bait)"));
            } else if count < *low {
                weaknesses.push(format!("{pos}: {count} players (below target {low}-{high})"));
            }
        }

        let drop_recs: Vec<String> = no_team
            .iter()
            .map(|p| {
                format!(
                    "Drop/trade: {} (no team)",
                    p["name"].as_str().unwrap_or("?")
                )
            })
            .collect();

        let mut priorities = weaknesses.clone();
        priorities.extend(drop_recs);

        Ok(serde_json::json!({
            "owner": data["owner"],
            "position_counts": pos_counts,
            "strengths": strengths,
            "weaknesses": weaknesses,
            "injured_or_out": injured,
            "no_team_free_agents": no_team,
            "suggested_priorities": priorities,
        }))
    }

    // -----------------------------------------------------------------------
    // Available players (waivers / draft prep)
    // -----------------------------------------------------------------------

    pub async fn available_players(
        &self,
        league_id: &str,
        position: Option<&str>,
        season: Option<&str>,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let client = self.client.clone();
        let lid = league_id.to_string();

        let rosters_h = {
            let c = client.clone();
            let l = lid.clone();
            tokio::spawn(async move { c.get_league_rosters(&l).await })
        };
        let players_h = {
            let c = client.clone();
            tokio::spawn(async move { c.get_all_players("nfl").await })
        };
        let state_h = {
            let c = client.clone();
            tokio::spawn(async move { c.get_nfl_state().await })
        };

        let rosters = rosters_h.await??;
        let all_players = players_h.await??;
        let nfl_state = state_h.await??;

        let rostered: std::collections::HashSet<String> = rosters
            .iter()
            .flat_map(|r| r.players.clone())
            .collect();

        let target_season = season.map(String::from).unwrap_or_else(|| {
            (nfl_state.season.parse::<i32>().unwrap_or(2024) - 1).to_string()
        });

        // Prefer local DB season totals; fall back to API for a recent week.
        let cached_weeks = self.db.cached_weeks(&target_season).await?;
        let season_stats: HashMap<String, HashMap<String, f64>> = if !cached_weeks.is_empty() {
            self.db.season_totals(&target_season).await?
        } else {
            // Scan recent weeks for first non-empty week
            let mut stats = HashMap::new();
            for wk in (1..=17).rev() {
                match self
                    .client
                    .get_player_stats("regular", &target_season, wk, "nfl")
                    .await
                {
                    Ok(data) if data.len() > 100 => {
                        for (pid, v) in &data {
                            if let Some(obj) = v.as_object() {
                                let inner: HashMap<String, f64> = obj
                                    .iter()
                                    .filter_map(|(k, v)| v.as_f64().map(|f| (k.clone(), f)))
                                    .collect();
                                stats.insert(pid.clone(), inner);
                            }
                        }
                        break;
                    }
                    _ => continue,
                }
            }
            stats
        };

        const SKILL: &[&str] = &["QB", "RB", "WR", "TE", "K", "DEF", "DL", "LB", "DB"];
        let pos_filter = position.map(|p| p.to_uppercase());

        let mut available: Vec<_> = all_players
            .iter()
            .filter_map(|(pid, info)| {
                let obj = info.as_object()?;
                if rostered.contains(pid) {
                    return None;
                }
                let pos = obj.get("position")?.as_str()?;
                if let Some(ref pf) = pos_filter {
                    if pos != pf {
                        return None;
                    }
                }
                if !SKILL.contains(&pos) {
                    return None;
                }
                let team = obj.get("team").and_then(Value::as_str)?;
                if team.is_empty() {
                    return None;
                }
                let s = season_stats.get(pid.as_str());
                let pts = s
                    .and_then(|m| {
                        m.get("pts_ppr")
                            .or_else(|| m.get("pts_half_ppr"))
                            .or_else(|| m.get("pts_std"))
                            .copied()
                    })
                    .unwrap_or(0.0);

                Some(serde_json::json!({
                    "player_id": pid,
                    "name": obj.get("full_name").and_then(Value::as_str).unwrap_or(pid),
                    "position": pos,
                    "team": team,
                    "status": obj.get("status").and_then(Value::as_str).unwrap_or("Active"),
                    "injury_status": obj.get("injury_status").and_then(Value::as_str),
                    "pts": pts,
                }))
            })
            .collect();

        available.sort_by(|a, b| {
            b["pts"]
                .as_f64()
                .partial_cmp(&a["pts"].as_f64())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        available.truncate(limit);
        Ok(available)
    }

    // -----------------------------------------------------------------------
    // Waiver recommendations
    // -----------------------------------------------------------------------

    pub async fn waiver_recommendations(
        &self,
        league_id: &str,
        lookback_hours: i32,
        limit: usize,
    ) -> Result<serde_json::Value> {
        let client = self.client.clone();
        let lid = league_id.to_string();

        let rosters_h = {
            let c = client.clone();
            let l = lid.clone();
            tokio::spawn(async move { c.get_league_rosters(&l).await })
        };
        let adds_h = {
            let c = client.clone();
            let lh = limit * 3;
            let lbh = lookback_hours;
            tokio::spawn(async move {
                c.get_trending_players("add", "nfl", lbh, lh as i32).await
            })
        };
        let drops_h = {
            let c = client.clone();
            let lh = limit;
            let lbh = lookback_hours;
            tokio::spawn(async move {
                c.get_trending_players("drop", "nfl", lbh, lh as i32).await
            })
        };
        let players_h = {
            let c = client.clone();
            tokio::spawn(async move { c.get_all_players("nfl").await })
        };

        let rosters = rosters_h.await??;
        let trending_adds = adds_h.await??;
        let trending_drops = drops_h.await??;
        let all_players = players_h.await??;

        let rostered: std::collections::HashSet<String> = rosters
            .iter()
            .flat_map(|r| r.players.clone())
            .collect();

        let enrich = |pid: &str, count: i64, count_key: &str| {
            let mut v = enrich_player(pid, &all_players);
            v[count_key] = serde_json::Value::from(count);
            v
        };

        let top_add_ids: Vec<String> = trending_adds
            .iter()
            .filter(|p| !rostered.contains(&p.player_id))
            .take(limit)
            .map(|p| p.player_id.clone())
            .collect();

        // Fetch news for waiver candidates before building the response.
        let news_map = self.fetch_news_for_players(&top_add_ids, &all_players).await;

        let top_adds: Vec<_> = top_add_ids
            .iter()
            .zip(
                trending_adds
                    .iter()
                    .filter(|p| !rostered.contains(&p.player_id))
                    .take(limit),
            )
            .map(|(pid, tp)| {
                let mut v = enrich(&tp.player_id, tp.count, "add_count");
                if let Some(news) = news_map.get(pid) {
                    v["news"] = serde_json::json!(news);
                }
                v
            })
            .collect();

        let top_drops: Vec<_> = trending_drops
            .iter()
            .map(|p| enrich(&p.player_id, p.count, "drop_count"))
            .collect();

        Ok(serde_json::json!({
            "top_available_trending_adds": top_adds,
            "trending_drops": top_drops,
            "lookback_hours": lookback_hours,
        }))
    }

    // -----------------------------------------------------------------------
    // Trade analysis
    // -----------------------------------------------------------------------

    pub async fn analyze_trade(
        &self,
        giving: &[String],
        receiving: &[String],
        season: Option<&str>,
        week: Option<i32>,
    ) -> Result<serde_json::Value> {
        let state = self.client.get_nfl_state().await?;
        let eval_season = match season {
            Some(s) => s.to_string(),
            None => best_cached_season(&self.db, &state.season).await,
        };
        let ref_week = week.unwrap_or_else(|| (state.week - 1).max(1));

        let players_h = {
            let c = self.client.clone();
            tokio::spawn(async move { c.get_all_players("nfl").await })
        };
        let all_players = players_h.await??;
        let stats_ctx = self.build_stats_context(&eval_season).await.unwrap_or_default();

        let summarize = |pid: &str| {
            let st = stats_ctx.get(pid);
            let obj = all_players.get(pid).and_then(Value::as_object);
            serde_json::json!({
                "player_id": pid,
                "name": obj.and_then(|m| m.get("full_name")).and_then(Value::as_str).unwrap_or(pid),
                "position": obj.and_then(|m| m.get("position")).and_then(Value::as_str).unwrap_or("?"),
                "team": obj.and_then(|m| m.get("team")).and_then(Value::as_str).unwrap_or("FA"),
                "ppg": st.map(|s| s.ppg).unwrap_or(0.0),
                "season_pts": st.map(|s| s.season_pts).unwrap_or(0.0),
                "games_played": st.map(|s| s.games_played).unwrap_or(0),
                "trend": st.map(|s| s.trend).unwrap_or("→"),
            })
        };

        let giving_data: Vec<_> = giving.iter().map(|p| summarize(p)).collect();
        let receiving_data: Vec<_> = receiving.iter().map(|p| summarize(p)).collect();

        let giving_ppg: f64 = giving_data.iter().filter_map(|v| v["ppg"].as_f64()).sum();
        let receiving_ppg: f64 = receiving_data.iter().filter_map(|v| v["ppg"].as_f64()).sum();
        let diff = receiving_ppg - giving_ppg;

        let verdict = if giving_ppg == 0.0 {
            "Insufficient stats data — sync weeks first"
        } else if diff > giving_ppg * 0.1 {
            "Favorable for you"
        } else if diff < -giving_ppg * 0.1 {
            "Unfavorable for you"
        } else {
            "Roughly even"
        };

        Ok(serde_json::json!({
            "you_give": giving_data,
            "you_receive": receiving_data,
            "giving_ppg_total": giving_ppg,
            "receiving_ppg_total": receiving_ppg,
            "ppg_difference": diff,
            "verdict": verdict,
            "based_on": format!("{eval_season} season PPG (ref week {ref_week})"),
        }))
    }

    // -----------------------------------------------------------------------
    // Start/sit recommendations
    // -----------------------------------------------------------------------

    pub async fn start_sit(
        &self,
        league_id: &str,
        roster_id: i32,
        week: i32,
    ) -> Result<serde_json::Value> {
        let client = self.client.clone();
        let lid = league_id.to_string();

        let state_h = {
            let c = client.clone();
            tokio::spawn(async move { c.get_nfl_state().await })
        };
        let rosters_h = {
            let c = client.clone();
            let l = lid.clone();
            tokio::spawn(async move { c.get_league_rosters(&l).await })
        };

        let state = state_h.await??;
        let rosters = rosters_h.await??;

        let roster = rosters
            .iter()
            .find(|r| r.roster_id == roster_id)
            .ok_or_else(|| crate::error::Error::NotFound(format!("roster {roster_id}")))?;

        let projections = self
            .client
            .get_player_projections("regular", &state.season, week, "nfl")
            .await
            .unwrap_or_default();

        let proj_pts = |pid: &str| -> f64 {
            projections
                .get(pid)
                .and_then(Value::as_object)
                .and_then(|m| {
                    m.get("pts_ppr")
                        .or_else(|| m.get("pts_half_ppr"))
                        .or_else(|| m.get("pts_std"))
                })
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
        };

        let mut ranked: Vec<_> = roster
            .players
            .iter()
            .map(|pid| {
                serde_json::json!({
                    "player_id": pid,
                    "projected_points": proj_pts(pid),
                })
            })
            .collect();
        ranked.sort_by(|a, b| {
            b["projected_points"]
                .as_f64()
                .partial_cmp(&a["projected_points"].as_f64())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let starter_count = if roster.starters.is_empty() {
            9.min(ranked.len())
        } else {
            roster.starters.len()
        };

        Ok(serde_json::json!({
            "roster_id": roster_id,
            "week": week,
            "current_starters": roster.starters,
            "recommended_starters": ranked[..starter_count.min(ranked.len())]
                .iter().map(|p| &p["player_id"]).collect::<Vec<_>>(),
            "recommended_bench": ranked[starter_count.min(ranked.len())..]
                .iter().map(|p| &p["player_id"]).collect::<Vec<_>>(),
            "all_players_ranked": ranked,
        }))
    }

    // -----------------------------------------------------------------------
    // Transactions
    // -----------------------------------------------------------------------

    pub async fn recent_transactions(
        &self,
        league_id: &str,
        week: i32,
        type_filter: Option<&str>,
    ) -> Result<Vec<serde_json::Value>> {
        let txns = self
            .client
            .get_league_transactions(league_id, week)
            .await?;
        Ok(txns
            .into_iter()
            .filter(|t| type_filter.map_or(true, |f| t.kind == f))
            .map(|t| {
                serde_json::json!({
                    "transaction_id": t.transaction_id,
                    "type": t.kind,
                    "status": t.status,
                    "adds": t.adds.unwrap_or_default(),
                    "drops": t.drops.unwrap_or_default(),
                    "roster_ids": t.roster_ids,
                    "created": t.created,
                })
            })
            .collect())
    }

    // -----------------------------------------------------------------------
    // Player search
    // -----------------------------------------------------------------------

    pub async fn search_players(
        &self,
        query: &str,
        position: Option<&str>,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let all_players = self.client.get_all_players("nfl").await?;
        let query_lower = query.to_lowercase();
        let pos_filter = position.map(|p| p.to_uppercase());

        let mut matches: Vec<_> = all_players
            .iter()
            .filter_map(|(pid, info)| {
                let obj = info.as_object()?;
                let full_name = obj.get("full_name")?.as_str()?;
                if !full_name.to_lowercase().contains(&query_lower) {
                    return None;
                }
                let pos = obj.get("position").and_then(Value::as_str).unwrap_or("");
                if let Some(ref pf) = pos_filter {
                    if pos != pf {
                        return None;
                    }
                }
                Some(serde_json::json!({
                    "player_id": pid,
                    "full_name": full_name,
                    "position": pos,
                    "team": obj.get("team").and_then(Value::as_str).unwrap_or("FA"),
                    "status": obj.get("status").and_then(Value::as_str).unwrap_or("Active"),
                    "injury_status": obj.get("injury_status").and_then(Value::as_str),
                }))
            })
            .collect();

        matches.sort_by_key(|m| m["full_name"].as_str().unwrap_or("").to_string());
        matches.truncate(limit);
        Ok(matches)
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Dynasty report
    // -----------------------------------------------------------------------

    /// Full dynasty roster report: tiers, trade candidates, cut list, and
    /// age-filtered waiver targets. Designed to be run repeatedly each offseason.
    pub async fn dynasty_report(
        &self,
        league_id: &str,
        roster_id: i32,
    ) -> Result<serde_json::Value> {
        let client = self.client.clone();
        let lid = league_id.to_string();

        let rosters_h = {
            let c = client.clone();
            let l = lid.clone();
            tokio::spawn(async move { c.get_league_rosters(&l).await })
        };
        let users_h = {
            let c = client.clone();
            let l = lid.clone();
            tokio::spawn(async move { c.get_league_users(&l).await })
        };
        let players_h = {
            let c = client.clone();
            tokio::spawn(async move { c.get_all_players("nfl").await })
        };
        let state_h = {
            let c = client.clone();
            tokio::spawn(async move { c.get_nfl_state().await })
        };

        let rosters = rosters_h.await??;
        let users = users_h.await??;
        let all_players = players_h.await??;
        let nfl_state = state_h.await??;

        let eval_season = best_cached_season(&self.db, &nfl_state.season).await;
        let stats_ctx = self.build_stats_context(&eval_season).await.unwrap_or_default();

        let roster = rosters
            .iter()
            .find(|r| r.roster_id == roster_id)
            .ok_or_else(|| crate::error::Error::NotFound(format!("roster {roster_id}")))?;

        let user_map: HashMap<_, _> = users
            .iter()
            .map(|u| (u.user_id.clone(), u.display_name.clone()))
            .collect();
        let owner = roster
            .owner_id
            .as_deref()
            .and_then(|id| user_map.get(id))
            .cloned()
            .unwrap_or_else(|| "Unknown".to_string());

        let rostered: std::collections::HashSet<String> = rosters
            .iter()
            .flat_map(|r| r.players.clone())
            .collect();

        // Fetch news for rostered players (HTTP-cached, runs concurrently with nothing left).
        let news_map = self.fetch_news_for_players(&roster.players, &all_players).await;

        let mut players: Vec<serde_json::Value> = roster
            .players
            .iter()
            .map(|pid| {
                let mut p = enrich_player_full(pid, &all_players, &stats_ctx);
                if let Some(news) = news_map.get(pid) {
                    p["news"] = serde_json::json!(news);
                }
                p
            })
            .collect();

        players.sort_by_key(|p| {
            (
                p["dynasty_tier"].as_i64().unwrap_or(9),
                p["position"].as_str().unwrap_or("").to_string(),
                p["name"].as_str().unwrap_or("").to_string(),
            )
        });

        let cornerstones: Vec<_> = players.iter().filter(|p| p["dynasty_tier"].as_i64() == Some(1)).cloned().collect();
        let keepers: Vec<_> = players.iter().filter(|p| p["dynasty_tier"].as_i64() == Some(2)).cloned().collect();
        let trade_bait: Vec<_> = players.iter().filter(|p| p["dynasty_tier"].as_i64() == Some(3)).cloned().collect();
        let cuts: Vec<_> = players.iter().filter(|p| p["dynasty_tier"].as_i64() == Some(4)).cloned().collect();

        const SKILL: &[&str] = &["QB", "RB", "WR", "TE"];
        let mut waiver_targets: Vec<_> = all_players
            .iter()
            .filter_map(|(pid, info)| {
                if rostered.contains(pid) {
                    return None;
                }
                let obj = info.as_object()?;
                let pos = obj.get("position")?.as_str()?;
                if !SKILL.contains(&pos) {
                    return None;
                }
                let team = obj.get("team").and_then(Value::as_str)?;
                if team.is_empty() {
                    return None;
                }
                let age = obj.get("age")?.as_i64()?;
                let years_exp = obj.get("years_exp").and_then(Value::as_i64).unwrap_or(99);
                if age > 26 && years_exp > 2 {
                    return None;
                }
                Some(enrich_player_full(pid, &all_players, &stats_ctx))
            })
            .collect();

        waiver_targets.sort_by_key(prospect_sort_key);
        waiver_targets.truncate(30);

        Ok(serde_json::json!({
            "owner": owner,
            "roster_id": roster_id,
            "summary": {
                "cornerstones": cornerstones.len(),
                "keepers": keepers.len(),
                "trade_bait": trade_bait.len(),
                "cut_candidates": cuts.len(),
                "total_roster": players.len(),
            },
            "tier1_cornerstones": cornerstones,
            "tier2_keepers": keepers,
            "tier3_trade_bait": trade_bait,
            "tier4_cut_list": cuts,
            "dynasty_waiver_targets": waiver_targets,
        }))
    }

    /// Look up real dynasty trade values (FantasyCalc, sleeperId-keyed) for a specific list of
    /// players. Auto-derives numQbs/ppr from the league's actual settings so values match its
    /// format (e.g. superflex leagues value QBs much higher than 1-QB leagues).
    ///
    /// IDP is not covered by FantasyCalc — requested IDP players come back with `found: false`
    /// and a `null` value; fall back to `dynasty_tier`/`search_rank` reasoning for those.
    pub async fn trade_value_check(
        &self,
        league_id: &str,
        player_ids: &[String],
    ) -> Result<serde_json::Value> {
        let league = self.client.get_league(league_id).await?;
        let num_qbs: u32 = if league.is_superflex() { 2 } else { 1 };
        let ppr = league.ppr();

        let all_values = self
            .client
            .get_trade_values(true, num_qbs, 12, ppr)
            .await?;
        let all_players = self.client.get_all_players("nfl").await?;

        let by_sleeper_id: HashMap<&str, &Value> = all_values
            .iter()
            .filter_map(|v| v["player"]["sleeperId"].as_str().map(|id| (id, v)))
            .collect();

        let players: Vec<_> = player_ids
            .iter()
            .map(|pid| {
                let name = all_players
                    .get(pid)
                    .and_then(|p| p.get("full_name"))
                    .and_then(Value::as_str)
                    .unwrap_or(pid);
                match by_sleeper_id.get(pid.as_str()) {
                    Some(v) => serde_json::json!({
                        "player_id": pid,
                        "name": name,
                        "position": v["player"]["position"],
                        "value": v["value"],
                        "overall_rank": v["overallRank"],
                        "position_rank": v["positionRank"],
                        "found": true,
                    }),
                    None => serde_json::json!({
                        "player_id": pid,
                        "name": name,
                        "value": null,
                        "found": false,
                    }),
                }
            })
            .collect();

        Ok(serde_json::json!({
            "source": "fantasycalc.com",
            "is_dynasty": true,
            "num_qbs": num_qbs,
            "ppr": ppr,
            "note": "IDP players are not covered by FantasyCalc and will show found:false",
            "players": players,
        }))
    }

    /// Dynasty-specific available player scan: returns young unrostered players
    /// sorted by age (youngest first), optionally filtered by position.
    pub async fn dynasty_targets(
        &self,
        league_id: &str,
        position: Option<&str>,
        max_age: u32,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let client = self.client.clone();
        let lid = league_id.to_string();

        let rosters_h = {
            let c = client.clone();
            let l = lid.clone();
            tokio::spawn(async move { c.get_league_rosters(&l).await })
        };
        let players_h = {
            let c = client.clone();
            tokio::spawn(async move { c.get_all_players("nfl").await })
        };

        let rosters = rosters_h.await??;
        let all_players = players_h.await??;

        let rostered: std::collections::HashSet<String> = rosters
            .iter()
            .flat_map(|r| r.players.clone())
            .collect();

        let pos_filter = position.map(|p| p.to_uppercase());
        const SKILL: &[&str] = &["QB", "RB", "WR", "TE", "DL", "LB", "DB"];

        let mut targets: Vec<_> = all_players
            .iter()
            .filter_map(|(pid, info)| {
                if rostered.contains(pid) {
                    return None;
                }
                let obj = info.as_object()?;
                let pos = obj.get("position")?.as_str()?;
                if !SKILL.contains(&pos) {
                    return None;
                }
                if let Some(ref pf) = pos_filter {
                    if pos != pf {
                        return None;
                    }
                }
                let team = obj.get("team").and_then(Value::as_str)?;
                if team.is_empty() {
                    return None;
                }
                let age = obj.get("age")?.as_i64()? as u32;
                if age > max_age {
                    return None;
                }
                Some(enrich_player(pid, &all_players))
            })
            .collect();

        targets.sort_by_key(prospect_sort_key);
        targets.truncate(limit);
        Ok(targets)
    }

    // -----------------------------------------------------------------------
    // Win-win trade finder
    // -----------------------------------------------------------------------

    /// Scans all rosters and returns trade ideas that improve both sides.
    ///
    /// Logic:
    ///   - Build a positional need/surplus profile for every team.
    ///   - "Need" = fewer than ideal keepers (tier ≤2) at a position.
    ///   - "Can offer" = tier 3/4 players at positions they don't need depth.
    ///   - Win-win pair: I have what they need AND they have what I need.
    ///   - Score = sum of dynasty tier improvement on each side.
    pub async fn find_win_win_trades(
        &self,
        league_id: &str,
        my_roster_id: i32,
        limit: usize,
    ) -> Result<serde_json::Value> {
        let client = self.client.clone();
        let lid = league_id.to_string();

        let rosters_h = {
            let c = client.clone();
            let l = lid.clone();
            tokio::spawn(async move { c.get_league_rosters(&l).await })
        };
        let users_h = {
            let c = client.clone();
            let l = lid.clone();
            tokio::spawn(async move { c.get_league_users(&l).await })
        };
        let players_h = {
            let c = client.clone();
            tokio::spawn(async move { c.get_all_players("nfl").await })
        };
        let state_h = {
            let c = client.clone();
            tokio::spawn(async move { c.get_nfl_state().await })
        };

        let rosters = rosters_h.await??;
        let users = users_h.await??;
        let all_players = players_h.await??;
        let nfl_state = state_h.await??;

        let eval_season = best_cached_season(&self.db, &nfl_state.season).await;
        let stats_ctx = self.build_stats_context(&eval_season).await.unwrap_or_default();

        let user_map: HashMap<_, _> = users
            .iter()
            .map(|u| (u.user_id.clone(), u.display_name.clone()))
            .collect();

        // Minimum keeper depth per position to be considered "satisfied".
        // QB is 2 because of the SUPER_FLEX slot.
        let min_keepers: HashMap<&str, usize> =
            [("QB", 2), ("RB", 3), ("WR", 4), ("TE", 1)]
                .into_iter()
                .collect();

        // For each roster, build:
        //   needs:    positions where keeper count < min_keepers
        //   offerables: tier 3/4 players at positions they have depth (or just any tier 3/4)
        let profile_roster = |roster: &Roster| -> (Vec<String>, Vec<serde_json::Value>) {
            let mut keepers_by_pos: HashMap<String, usize> = HashMap::new();
            let mut offerables: Vec<serde_json::Value> = Vec::new();

            for pid in &roster.players {
                let p = enrich_player_full(pid, &all_players, &stats_ctx);
                let pos = p["position"].as_str().unwrap_or("?").to_string();
                let tier = p["dynasty_tier"].as_i64().unwrap_or(4);

                if tier <= 2 {
                    *keepers_by_pos.entry(pos.clone()).or_default() += 1;
                }
                if tier >= 3 {
                    offerables.push(p);
                }
            }

            let needs: Vec<String> = min_keepers
                .iter()
                .filter_map(|(pos, min)| {
                    let have = keepers_by_pos.get(*pos).copied().unwrap_or(0);
                    if have < *min {
                        Some(pos.to_string())
                    } else {
                        None
                    }
                })
                .collect();

            (needs, offerables)
        };

        let my_roster = rosters
            .iter()
            .find(|r| r.roster_id == my_roster_id)
            .ok_or_else(|| crate::error::Error::NotFound(format!("roster {my_roster_id}")))?;

        let (my_needs, my_offerables) = profile_roster(my_roster);

        let mut proposals: Vec<serde_json::Value> = Vec::new();

        for other in &rosters {
            if other.roster_id == my_roster_id {
                continue;
            }

            let (their_needs, their_offerables) = profile_roster(other);

            // What can they give me? Their tier 3/4 at positions I need.
            // But more useful: their tier 1-2 players at positions I need that
            // they have MORE than enough of.
            let they_give_me: Vec<&serde_json::Value> = their_offerables
                .iter()
                .filter(|p| {
                    let pos = p["position"].as_str().unwrap_or("");
                    my_needs.iter().any(|n| n == pos)
                })
                .collect();

            // What can I give them? My tier 3/4 at positions they need.
            let i_give_them: Vec<&serde_json::Value> = my_offerables
                .iter()
                .filter(|p| {
                    let pos = p["position"].as_str().unwrap_or("");
                    their_needs.iter().any(|n| n == pos)
                })
                .collect();

            if they_give_me.is_empty() || i_give_them.is_empty() {
                continue;
            }

            // Score: tier improvement on my side + position alignment on their side.
            // Higher is better.
            let my_gain: i64 = they_give_me
                .iter()
                .map(|p| 4 - p["dynasty_tier"].as_i64().unwrap_or(4))
                .sum();
            let their_gain: i64 = i_give_them
                .iter()
                .map(|p| p["dynasty_tier"].as_i64().unwrap_or(4) - 1)
                .sum();
            let score = my_gain + their_gain;

            let partner_name = other
                .owner_id
                .as_deref()
                .and_then(|id| user_map.get(id))
                .cloned()
                .unwrap_or_else(|| format!("Roster {}", other.roster_id));

            // Surface best 3 players from each side.
            let mut they_give_sorted = they_give_me.clone();
            they_give_sorted.sort_by(|a, b| {
                let ta = a["dynasty_tier"].as_i64().unwrap_or(4);
                let tb = b["dynasty_tier"].as_i64().unwrap_or(4);
                ta.cmp(&tb).then_with(|| {
                    b["ppg"].as_f64().partial_cmp(&a["ppg"].as_f64())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            });
            let mut i_give_sorted = i_give_them.clone();
            i_give_sorted.sort_by(|a, b| {
                b["ppg"].as_f64().partial_cmp(&a["ppg"].as_f64())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            let they_give_top: Vec<_> = they_give_sorted.into_iter().take(3).collect();
            let i_give_top: Vec<_> = i_give_sorted.into_iter().take(3).collect();

            let my_needs_filled: Vec<_> = they_give_top
                .iter()
                .map(|p| p["position"].as_str().unwrap_or("?"))
                .collect();
            let their_needs_filled: Vec<_> = i_give_top
                .iter()
                .map(|p| p["position"].as_str().unwrap_or("?"))
                .collect();

            proposals.push(serde_json::json!({
                "partner": partner_name,
                "partner_roster_id": other.roster_id,
                "win_win_score": score,
                "my_needs": my_needs,
                "their_needs": their_needs,
                "you_give": i_give_top,
                "you_receive": they_give_top,
                "summary": format!(
                    "Give {} {} → fills their {:?} need. Receive {} {} → fills your {:?} need.",
                    i_give_top.first().and_then(|p| p["name"].as_str()).unwrap_or("?"),
                    i_give_top.first().and_then(|p| p["position"].as_str()).unwrap_or(""),
                    their_needs_filled,
                    they_give_top.first().and_then(|p| p["name"].as_str()).unwrap_or("?"),
                    they_give_top.first().and_then(|p| p["position"].as_str()).unwrap_or(""),
                    my_needs_filled,
                ),
            }));
        }

        proposals.sort_by(|a, b| {
            b["win_win_score"]
                .as_i64()
                .cmp(&a["win_win_score"].as_i64())
        });
        proposals.truncate(limit);

        Ok(serde_json::json!({
            "my_roster_id": my_roster_id,
            "my_needs": my_needs,
            "my_offerables": my_offerables,
            "trade_proposals": proposals,
        }))
    }

    // -----------------------------------------------------------------------
    // News
    // -----------------------------------------------------------------------

    /// Fetch RotowWire + ESPN news and return a map of player_id → news items
    /// for the requested player IDs. Uses name matching against `all_players`.
    pub async fn fetch_news_for_players(
        &self,
        player_ids: &[String],
        all_players: &HashMap<String, Value>,
    ) -> HashMap<String, Vec<serde_json::Value>> {
        // Build normalized_name → player_id lookup for only the players we care about.
        let name_to_id: HashMap<String, String> = player_ids
            .iter()
            .filter_map(|pid| {
                let name = all_players.get(pid)
                    .and_then(|v| v.get("full_name"))
                    .and_then(Value::as_str)?;
                Some((normalize_name(name), pid.clone()))
            })
            .collect();

        // Fetch both sources concurrently, ignoring errors gracefully.
        let c1 = self.client.clone();
        let c2 = self.client.clone();
        let rw_h = tokio::spawn(async move { c1.get_rotowire_news().await });
        let espn_h = tokio::spawn(async move { c2.get_espn_rss_news().await });

        let rw_news = rw_h.await.ok().and_then(|r| r.ok()).unwrap_or_default();
        let espn_news = espn_h.await.ok().and_then(|r| r.ok()).unwrap_or_default();

        let mut result: HashMap<String, Vec<serde_json::Value>> = HashMap::new();

        // RotowWire items have an explicit player_name.
        for item in &rw_news {
            let key = normalize_name(&item.player_name);
            if let Some(pid) = name_to_id.get(&key) {
                result.entry(pid.clone()).or_default().push(serde_json::json!({
                    "source": item.source,
                    "headline": item.headline,
                    "body": item.body,
                    "date": item.date,
                }));
            }
        }

        // ESPN items: scan the title for any matching player name.
        for item in &espn_news {
            let title_lower = item.headline.to_lowercase();
            for (norm_name, pid) in &name_to_id {
                if title_lower.contains(norm_name.as_str()) {
                    result.entry(pid.clone()).or_default().push(serde_json::json!({
                        "source": item.source,
                        "headline": item.headline,
                        "body": item.body,
                        "date": item.date,
                    }));
                    break;
                }
            }
        }

        result
    }

    // -----------------------------------------------------------------------
    // Stats context
    // -----------------------------------------------------------------------

    /// Build a per-player stats summary for a full season from the local DB.
    /// Returns empty map if no weeks are cached for that season.
    pub async fn build_stats_context(&self, season: &str) -> Result<StatsContext> {
        let weekly = self.db.weekly_pts_all_players(season).await?;
        let mut ctx = StatsContext::new();
        for (pid, weeks) in weekly {
            // weeks is sorted week DESC (most-recent first) by the DB query
            let games_played = weeks.len();
            let season_pts: f64 = weeks.iter().map(|(_, p)| p).sum();
            let ppg = if games_played > 0 {
                season_pts / games_played as f64
            } else {
                0.0
            };

            let recent: Vec<f64> = weeks.iter().take(3).map(|(_, p)| *p).collect();
            let recent_avg = if recent.is_empty() {
                0.0
            } else {
                recent.iter().sum::<f64>() / recent.len() as f64
            };

            let trend: &'static str = if recent.len() >= 2 {
                let oldest = *recent.last().unwrap();
                let newest = recent[0];
                if newest > oldest * 1.1 {
                    "↑"
                } else if newest < oldest * 0.9 {
                    "↓"
                } else {
                    "→"
                }
            } else {
                "→"
            };

            ctx.insert(pid, PlayerStats { season_pts, games_played, ppg, recent_avg, trend });
        }
        Ok(ctx)
    }

    async fn fetch_rosters_and_users(
        &self,
        league_id: &str,
    ) -> Result<(Vec<Roster>, Vec<User>)> {
        let client = self.client.clone();
        let lid = league_id.to_string();

        let rosters_h = {
            let c = client.clone();
            let l = lid.clone();
            tokio::spawn(async move { c.get_league_rosters(&l).await })
        };
        let users_h = {
            let c = client.clone();
            let l = lid.clone();
            tokio::spawn(async move { c.get_league_users(&l).await })
        };

        let rosters = rosters_h.await??;
        let users = users_h.await??;
        Ok((rosters, users))
    }
}

/// Normalize a player name for fuzzy matching (lowercase, alpha + spaces only).
fn normalize_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| c.is_alphabetic() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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

fn enrich_player(pid: &str, all_players: &HashMap<String, Value>) -> serde_json::Value {
    let obj = all_players.get(pid).and_then(Value::as_object);
    let age = obj.and_then(|m| m.get("age")).and_then(Value::as_i64);
    let years_exp = obj.and_then(|m| m.get("years_exp")).and_then(Value::as_i64);
    let team = obj.and_then(|m| m.get("team")).and_then(Value::as_str).unwrap_or("FA");
    let pos = obj.and_then(|m| m.get("position")).and_then(Value::as_str).unwrap_or("?");
    // Sleeper's own relevance/ADP-like signal. Lower = more fantasy-relevant.
    // Without this, same-age rookies (e.g. all Year-0 RBs) are indistinguishable —
    // dynasty_tier collapses them all to Tier 1 with no way to tell a true prospect
    // from a camp body. See prospect_sort_key, which is what actually orders them.
    let search_rank = obj.and_then(|m| m.get("search_rank")).and_then(Value::as_i64);

    let dynasty_tier = dynasty_tier_for_pos(age, years_exp, team, pos);

    serde_json::json!({
        "player_id": pid,
        "name": obj.and_then(|m| m.get("full_name")).and_then(Value::as_str)
            .unwrap_or(pid),
        "position": pos,
        "team": team,
        "age": age,
        "years_exp": years_exp,
        "status": obj.and_then(|m| m.get("status")).and_then(Value::as_str)
            .unwrap_or("Active"),
        "injury_status": obj.and_then(|m| m.get("injury_status")).and_then(Value::as_str),
        "dynasty_tier": dynasty_tier,
        "search_rank": search_rank,
    })
}

/// Like `enrich_player` but includes all injury/news fields and PPG from a stats context.
/// Also applies production-aware dynasty tier overrides.
fn enrich_player_full(
    pid: &str,
    all_players: &HashMap<String, Value>,
    stats_ctx: &StatsContext,
) -> serde_json::Value {
    let obj = all_players.get(pid).and_then(Value::as_object);
    let age = obj.and_then(|m| m.get("age")).and_then(Value::as_i64);
    let years_exp = obj.and_then(|m| m.get("years_exp")).and_then(Value::as_i64);
    let team = obj.and_then(|m| m.get("team")).and_then(Value::as_str).unwrap_or("FA");
    let pos = obj.and_then(|m| m.get("position")).and_then(Value::as_str).unwrap_or("?");

    let mut dynasty_tier = dynasty_tier_for_pos(age, years_exp, team, pos);

    let st = stats_ctx.get(pid);
    let ppg = st.map(|s| s.ppg).unwrap_or(0.0);
    let games_played = st.map(|s| s.games_played).unwrap_or(0);
    let season_pts = st.map(|s| s.season_pts).unwrap_or(0.0);
    let recent_avg = st.map(|s| s.recent_avg).unwrap_or(0.0);
    let trend = st.map(|s| s.trend).unwrap_or("→");

    // Production-aware override: active players who outperform their age tier.
    if team != "FA" && !team.is_empty() {
        if dynasty_tier == 4 && ppg >= 8.0 {
            dynasty_tier = 3;
        } else if dynasty_tier == 3 && ppg >= 12.0 && games_played >= 10 {
            dynasty_tier = 2;
        }
    }

    let injury_status = obj.and_then(|m| m.get("injury_status")).and_then(Value::as_str);
    let practice = obj.and_then(|m| m.get("practice_participation")).and_then(Value::as_str);
    let risk_flag = matches!(injury_status, Some("Out") | Some("IR") | Some("PUP"))
        || matches!(practice, Some("Did Not Participate") | Some("Limited"));
    let search_rank = obj.and_then(|m| m.get("search_rank")).and_then(Value::as_i64);

    serde_json::json!({
        "player_id": pid,
        "name": obj.and_then(|m| m.get("full_name")).and_then(Value::as_str).unwrap_or(pid),
        "position": pos,
        "team": team,
        "age": age,
        "years_exp": years_exp,
        "status": obj.and_then(|m| m.get("status")).and_then(Value::as_str).unwrap_or("Active"),
        "injury_status": injury_status,
        "search_rank": search_rank,
        "injury_body_part": obj.and_then(|m| m.get("injury_body_part")).and_then(Value::as_str),
        "injury_notes": obj.and_then(|m| m.get("injury_notes")).and_then(Value::as_str),
        "injury_start_date": obj.and_then(|m| m.get("injury_start_date")).and_then(Value::as_str),
        "practice_description": obj.and_then(|m| m.get("practice_description")).and_then(Value::as_str),
        "practice_participation": practice,
        "depth_chart_order": obj.and_then(|m| m.get("depth_chart_order")).and_then(Value::as_i64),
        "news_updated": obj.and_then(|m| m.get("news_updated")).and_then(Value::as_i64),
        "dynasty_tier": dynasty_tier,
        "ppg": ppg,
        "games_played": games_played,
        "season_pts": season_pts,
        "recent_avg": recent_avg,
        "trend": trend,
        "risk_flag": risk_flag,
    })
}

/// Sort key for ranking prospects/targets by real-world relevance, not just age.
///
/// `dynasty_tier` alone cannot do this: every player with `years_exp <= 1` collapses
/// to the same Tier 1 bucket (see `dynasty_tier_for_pos`), so a true blue-chip rookie
/// and a rookie with no realistic path to the field are indistinguishable by tier.
/// This was the root cause of a bad rookie-draft recommendation — an untested prospect
/// got recommended ahead of several better-regarded ones because nothing in the tool
/// output differentiated same-tier rookies from each other.
///
/// Sleeper's own `search_rank` (lower = more fantasy-relevant) is the cheapest available
/// signal to break that tie. Missing search_rank sorts last, not first, so unranked
/// players don't accidentally outrank ranked ones.
fn prospect_sort_key(p: &Value) -> (i64, i64, String, String) {
    (
        p["search_rank"].as_i64().unwrap_or(i64::MAX),
        p["age"].as_i64().unwrap_or(99),
        p["position"].as_str().unwrap_or("").to_string(),
        p["name"].as_str().unwrap_or("").to_string(),
    )
}

#[cfg(test)]
mod prospect_sort_tests {
    use super::*;

    #[test]
    fn search_rank_breaks_ties_within_the_same_dynasty_tier() {
        // Both are Year-0 rookies, so dynasty_tier_for_pos gives them the same tier (1).
        // blue_chip is deliberately made OLDER than deep_bench: under the old age-only
        // sort this would put deep_bench first (younger wins), which is exactly the
        // wrong-candidate bug this test guards against. Only search_rank gets it right.
        let blue_chip = serde_json::json!({
            "name": "Jeremiyah Love", "age": 23, "position": "RB", "search_rank": 15
        });
        let deep_bench = serde_json::json!({
            "name": "Adam Randall", "age": 21, "position": "RB", "search_rank": 999
        });

        let mut targets = vec![deep_bench.clone(), blue_chip.clone()];
        targets.sort_by_key(prospect_sort_key);

        assert_eq!(
            targets[0]["name"], blue_chip["name"],
            "the better-regarded prospect (lower search_rank) must sort first, \
             not whichever one happened to be younger or alphabetically first"
        );
    }

    #[test]
    fn missing_search_rank_sorts_last_not_first() {
        let ranked = serde_json::json!({
            "name": "Ranked Guy", "age": 22, "position": "RB", "search_rank": 500
        });
        let unranked = serde_json::json!({
            "name": "Unranked Guy", "age": 22, "position": "RB", "search_rank": null
        });

        let mut targets = vec![unranked.clone(), ranked.clone()];
        targets.sort_by_key(prospect_sort_key);

        assert_eq!(targets[0]["name"], ranked["name"]);
    }
}

/// Classify a player into a dynasty value tier using position-aware aging curves.
///
/// Different positions have different longevity profiles:
///   RB:       peak 22-27, decline 28+, done 31+
///   WR/TE/IDP: peak 23-29, decline 30+, done 33+
///   QB:       peak 24-33, decline 34+, done 37+
///
/// Tier 1 — Cornerstone (never trade)
/// Tier 2 — Keeper (hold, still in prime)
/// Tier 3 — Trade bait (declining or aging, sell for picks)
/// Tier 4 — Cut (no team, career over, or deep backup / no path)
fn dynasty_tier(age: Option<i64>, years_exp: Option<i64>, team: &str) -> i64 {
    dynasty_tier_for_pos(age, years_exp, team, "")
}

fn dynasty_tier_for_pos(age: Option<i64>, years_exp: Option<i64>, team: &str, pos: &str) -> i64 {
    let is_fa = team == "FA" || team.is_empty();
    let age = age.unwrap_or(99);
    let yrs = years_exp.unwrap_or(99);

    if is_fa {
        return 4;
    }
    // Rookie / very young = always Tier 1 regardless of position
    if yrs <= 1 || age <= 22 {
        return 1;
    }

    match pos {
        "QB" => {
            if age >= 37 { 4 }
            else if age >= 34 { 3 }
            else if age <= 26 { 1 }
            else { 2 } // 27-33: prime QB years
        }
        "RB" => {
            if age >= 31 { 4 }
            else if age >= 28 { 3 }
            else if age <= 24 { 1 }
            else { 2 } // 25-27: RB prime
        }
        "WR" | "TE" | "DL" | "LB" | "DB" => {
            if age >= 34 { 4 }
            else if age >= 30 { 3 }
            else if age <= 25 { 1 }
            else { 2 } // 26-29: WR/TE prime
        }
        _ => {
            // Generic fallback
            if age >= 35 { 4 }
            else if age >= 31 { 3 }
            else if age <= 25 { 1 }
            else { 2 }
        }
    }
}
