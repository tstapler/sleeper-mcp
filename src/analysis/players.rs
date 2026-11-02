//! Player-centric lookups: full player reports (status/news/trend), free-text
//! search, cross-source news aggregation, and the season stats context that
//! dynasty/trade analysis is built on.
use std::collections::HashMap;

use serde_json::Value;

use crate::error::Result;

use super::dynasty::enrich_player_full;
use super::AnalysisService;

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

impl AnalysisService {
    // -----------------------------------------------------------------------
    // Player report — current status, aggregated news, and production trend
    // -----------------------------------------------------------------------

    /// Full report for a specific set of players: Sleeper's own current status/injury/practice
    /// fields, aggregated news (RotoWire + ESPN, see `fetch_news_for_players`), dynasty tier, and
    /// season production trend. League-agnostic — resolve player_ids via `search_players` first
    /// if you only have a name. Defaults to the live NFL season if none is given.
    pub async fn player_report(
        &self,
        player_ids: &[String],
        season: Option<&str>,
    ) -> Result<Vec<serde_json::Value>> {
        let season = match season {
            Some(s) => s.to_string(),
            None => self.client.get_nfl_state().await?.season,
        };

        let all_players_h = self.spawn_all_players();
        let stats_ctx_h = {
            let svc = self.clone();
            let s = season.clone();
            tokio::spawn(async move { svc.build_stats_context(&s).await })
        };

        let all_players = all_players_h.await??;
        let stats_ctx = stats_ctx_h.await??;
        let news_map = self.fetch_news_for_players(player_ids, &all_players).await;

        Ok(player_ids
            .iter()
            .map(|pid| {
                let mut p = enrich_player_full(pid, &all_players, &stats_ctx);
                p["news"] = serde_json::json!(news_map.get(pid).cloned().unwrap_or_default());
                p["season"] = serde_json::json!(season);
                p
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

        // RotoWire has an explicit player_name per item; ESPN/CBS/PFT/Yahoo are RSS feeds with
        // no per-item player field, matched by scanning the headline instead. Fetch all five
        // concurrently, ignoring individual source failures gracefully.
        let c = self.client.clone();
        let rw_h = tokio::spawn({ let c = c.clone(); async move { c.get_rotowire_news().await } });
        let espn_h = tokio::spawn({ let c = c.clone(); async move { c.get_espn_rss_news().await } });
        let cbs_h = tokio::spawn({ let c = c.clone(); async move { c.get_cbssports_news().await } });
        let pft_h = tokio::spawn({ let c = c.clone(); async move { c.get_pft_news().await } });
        let yahoo_h = tokio::spawn(async move { c.get_yahoo_news().await });

        let rw_news = rw_h.await.ok().and_then(|r| r.ok()).unwrap_or_default();
        let title_scan_sources = [
            espn_h.await.ok().and_then(|r| r.ok()).unwrap_or_default(),
            cbs_h.await.ok().and_then(|r| r.ok()).unwrap_or_default(),
            pft_h.await.ok().and_then(|r| r.ok()).unwrap_or_default(),
            yahoo_h.await.ok().and_then(|r| r.ok()).unwrap_or_default(),
        ];

        let mut result: HashMap<String, Vec<serde_json::Value>> = HashMap::new();

        // RotoWire items have an explicit player_name.
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

        // RSS-shaped sources: scan the title for any matching player name.
        for source_items in &title_scan_sources {
            for item in source_items {
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
