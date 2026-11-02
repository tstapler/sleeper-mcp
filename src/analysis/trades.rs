//! Trade analysis: a single give/receive PPG comparison, real dynasty trade
//! values from FantasyCalc, and the league-wide win-win trade scanner.
use std::collections::HashMap;

use serde_json::Value;

use crate::error::Result;
use crate::models::Roster;

use super::dynasty::enrich_player_full;
use super::{best_cached_season, AnalysisService};

impl AnalysisService {
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

        let players_h = self.spawn_all_players();
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
        let (rosters, users, all_players, nfl_state) =
            self.fetch_rosters_users_players_state(league_id).await?;
        let (_eval_season, stats_ctx) = self.eval_season_stats(&nfl_state.season).await;

        let user_map: HashMap<_, _> = users
            .iter()
            .map(|u| (u.user_id.clone(), u.display_name.clone()))
            .collect();

        let count_keepers = |roster: &Roster| -> HashMap<String, usize> {
            let mut keepers_by_pos: HashMap<String, usize> = HashMap::new();
            for pid in &roster.players {
                let p = enrich_player_full(pid, &all_players, &stats_ctx);
                if p["dynasty_tier"].as_i64().unwrap_or(4) <= 2 {
                    *keepers_by_pos.entry(p["position"].as_str().unwrap_or("?").to_string()).or_default() += 1;
                }
            }
            keepers_by_pos
        };

        // League-average tier-1/2 depth per position, used as the "need" baseline instead
        // of a fixed constant — a 36-player dynasty roster with a taxi squad and a lean
        // 18-player redraft roster have very different notions of "enough" depth at a
        // position, so a hardcoded QB:2/RB:3/WR:4/TE:1 floor either never triggers (deep
        // leagues) or always triggers (shallow ones).
        const NEED_POSITIONS: &[&str] = &["QB", "RB", "WR", "TE"];
        let all_keeper_counts: Vec<HashMap<String, usize>> =
            rosters.iter().map(&count_keepers).collect();
        let league_avg: HashMap<&str, f64> = NEED_POSITIONS
            .iter()
            .map(|pos| {
                let total: usize = all_keeper_counts.iter().map(|m| m.get(*pos).copied().unwrap_or(0)).sum();
                (*pos, total as f64 / rosters.len().max(1) as f64)
            })
            .collect();

        // For each roster, build:
        //   needs:    positions meaningfully below the league-average keeper depth
        //   offerables: tier 3/4 players (this roster's own trade bait)
        let profile_roster = |roster: &Roster| -> (Vec<String>, Vec<serde_json::Value>) {
            let keepers_by_pos = count_keepers(roster);
            let needs: Vec<String> = NEED_POSITIONS
                .iter()
                .filter_map(|pos| {
                    let have = keepers_by_pos.get(*pos).copied().unwrap_or(0);
                    let threshold = (league_avg[pos] * 0.75).floor().max(1.0) as usize;
                    if have < threshold { Some(pos.to_string()) } else { None }
                })
                .collect();

            let offerables: Vec<serde_json::Value> = roster
                .players
                .iter()
                .map(|pid| enrich_player_full(pid, &all_players, &stats_ctx))
                .filter(|p| p["dynasty_tier"].as_i64().unwrap_or(4) >= 3)
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

            let partner_name = other
                .owner_id
                .as_deref()
                .and_then(|id| user_map.get(id))
                .cloned()
                .unwrap_or_else(|| format!("Roster {}", other.roster_id));

            // I have no positional need, but they do and I have surplus (tier 3/4) value
            // there — a sell-high candidate even with no need-for-need match. Offer my
            // best expendable piece per needed position for their youngest tier-1/2 asset
            // at that position (a get-younger swap), speculative if they have none.
            if they_give_me.is_empty() && my_needs.is_empty() && !i_give_them.is_empty() {
                let mut best_sell_by_pos: HashMap<&str, &serde_json::Value> = HashMap::new();
                for &p in &i_give_them {
                    let pos = p["position"].as_str().unwrap_or("?");
                    best_sell_by_pos
                        .entry(pos)
                        .and_modify(|cur| {
                            if p["dynasty_tier"].as_i64().unwrap_or(4)
                                < cur["dynasty_tier"].as_i64().unwrap_or(4)
                            {
                                *cur = p;
                            }
                        })
                        .or_insert(p);
                }

                for (pos, sell_piece) in best_sell_by_pos {
                    let ret = other
                        .players
                        .iter()
                        .map(|pid| enrich_player_full(pid, &all_players, &stats_ctx))
                        .filter(|p| {
                            p["position"].as_str() == Some(pos)
                                && p["dynasty_tier"].as_i64().unwrap_or(4) <= 2
                        })
                        .min_by_key(|p| p["age"].as_i64().unwrap_or(99));

                    let their_gain = sell_piece["dynasty_tier"].as_i64().unwrap_or(4) - 1;
                    let summary = match &ret {
                        Some(r) => format!(
                            "Sell {} {} (expendable, they need {pos}) for {} {pos}, \
                             their younger option at the position — not a strict need match for you.",
                            sell_piece["name"].as_str().unwrap_or("?"),
                            pos,
                            r["name"].as_str().unwrap_or("?"),
                        ),
                        None => format!(
                            "Sell {} {} to {partner_name} — they need {pos} and have no \
                             tier 1/2 depth there; ask for picks/futures instead of a like-for-like swap.",
                            sell_piece["name"].as_str().unwrap_or("?"),
                            pos,
                        ),
                    };

                    proposals.push(serde_json::json!({
                        "kind": "sell_high",
                        "partner": partner_name,
                        "partner_roster_id": other.roster_id,
                        "win_win_score": their_gain,
                        "my_needs": my_needs,
                        "their_needs": their_needs,
                        "you_give": [sell_piece],
                        "you_receive": ret.map(|r| vec![r]).unwrap_or_default(),
                        "summary": summary,
                    }));
                }
            }

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
                "kind": "need_match",
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
}
