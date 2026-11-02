//! Waiver-wire surface: unrostered players ranked by points, trending
//! add/drop recommendations, and raw transaction listing.
use std::collections::HashMap;

use serde_json::Value;

use crate::error::Result;

use super::dynasty::enrich_player;
use super::{rostered_player_ids, AnalysisService};

impl AnalysisService {
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
        let state_h = self.spawn_nfl_state();
        let (rosters, all_players) = self.fetch_rosters_and_players(league_id).await?;
        let nfl_state = state_h.await??;

        let rostered = rostered_player_ids(&rosters);

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

        let rosters_h = self.spawn_rosters(league_id);
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
        let players_h = self.spawn_all_players();

        let rosters = rosters_h.await??;
        let trending_adds = adds_h.await??;
        let trending_drops = drops_h.await??;
        let all_players = players_h.await??;

        let rostered = rostered_player_ids(&rosters);

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
}
