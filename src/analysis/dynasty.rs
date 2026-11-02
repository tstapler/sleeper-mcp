//! Dynasty-format analysis: tiered roster reports, age-filtered waiver/trade
//! targets, and the age-curve tier classifier + player-enrichment helpers
//! that back both this module and the rest of `analysis::*`.
use std::collections::HashMap;

use serde_json::Value;

use crate::error::Result;

use super::{rostered_player_ids, AnalysisService, StatsContext};

/// Full dynasty roster report: tiers, trade candidates, cut list, and
/// age-filtered waiver targets. Designed to be run repeatedly each offseason.
impl AnalysisService {
    pub async fn dynasty_report(
        &self,
        league_id: &str,
        roster_id: i32,
    ) -> Result<serde_json::Value> {
        let (rosters, users, all_players, nfl_state) =
            self.fetch_rosters_users_players_state(league_id).await?;
        let (_eval_season, stats_ctx) = self.eval_season_stats(&nfl_state.season).await;

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

        let rostered = rostered_player_ids(&rosters);

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

    /// Dynasty-specific available player scan: returns young unrostered players
    /// sorted by age (youngest first), optionally filtered by position.
    pub async fn dynasty_targets(
        &self,
        league_id: &str,
        position: Option<&str>,
        max_age: u32,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let (rosters, all_players) = self.fetch_rosters_and_players(league_id).await?;
        let rostered = rostered_player_ids(&rosters);

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
}

pub(super) fn enrich_player(pid: &str, all_players: &HashMap<String, Value>) -> serde_json::Value {
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
pub(super) fn enrich_player_full(
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
