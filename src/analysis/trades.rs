//! Trade analysis: a single give/receive PPG comparison, real dynasty trade
//! values from FantasyCalc, and the league-wide win-win trade scanner.
use std::collections::HashMap;

use serde_json::Value;

use crate::error::Result;
use crate::models::Roster;

use super::dynasty::{enrich_player, enrich_player_full};
use super::picks::{self, PickDescriptor, PickValuationContext};
use super::{best_cached_season, AnalysisService};

/// Median FantasyCalc value within each `dynasty_tier` bucket (1-4), computed
/// fresh from the live FantasyCalc dataset itself rather than hardcoded
/// constants that would drift as the market moves. Used as the heuristic
/// value for a player FantasyCalc doesn't cover — not an IDP-specific
/// branch: any uncovered player (IDP, or an obscure offensive player) falls
/// back the same way, keyed only on which tier their own age/experience/
/// position curve already places them in.
fn tier_value_medians(all_values: &[Value], all_players: &HashMap<String, Value>) -> HashMap<i64, f64> {
    let mut by_tier: HashMap<i64, Vec<f64>> = HashMap::new();
    for v in all_values {
        let Some(sleeper_id) = v["player"]["sleeperId"].as_str() else { continue };
        let Some(value) = v["value"].as_f64() else { continue };
        let tier = enrich_player(sleeper_id, all_players)["dynasty_tier"].as_i64().unwrap_or(4);
        by_tier.entry(tier).or_default().push(value);
    }
    by_tier
        .into_iter()
        .map(|(tier, mut values)| {
            values.sort_by(|a, b| a.partial_cmp(b).unwrap());
            (tier, values[values.len() / 2])
        })
        .collect()
}

/// Nearest tier (searching outward, cheaper first) that actually has a
/// median — guards the rare case a tier bucket comes up empty (e.g. a thin
/// FantasyCalc snapshot), rather than silently returning no value at all.
fn nearest_tier_median(medians: &HashMap<i64, f64>, tier: i64) -> Option<f64> {
    (0..4).find_map(|offset| {
        medians.get(&(tier + offset)).or_else(|| medians.get(&(tier - offset))).copied()
    })
}

/// One player's row in `trade_value_check`'s output: a real FantasyCalc
/// price when it has one, else the tier-median heuristic (see
/// `tier_value_medians`'s doc comment for why — no source prices IDP).
fn summarize_trade_value(
    pid: &str,
    all_players: &HashMap<String, Value>,
    by_sleeper_id: &HashMap<&str, &Value>,
    tier_medians: &HashMap<i64, f64>,
) -> serde_json::Value {
    let name = all_players
        .get(pid)
        .and_then(|p| p.get("full_name"))
        .and_then(Value::as_str)
        .unwrap_or(pid);
    match by_sleeper_id.get(pid) {
        Some(v) => serde_json::json!({
            "player_id": pid,
            "name": name,
            "position": v["player"]["position"],
            "value": v["value"],
            "value_source": "fantasycalc",
            "overall_rank": v["overallRank"],
            "position_rank": v["positionRank"],
            "found": true,
        }),
        None => {
            let enriched = enrich_player(pid, all_players);
            let tier = enriched["dynasty_tier"].as_i64().unwrap_or(4);
            let heuristic_value = nearest_tier_median(tier_medians, tier);
            serde_json::json!({
                "player_id": pid,
                "name": name,
                "position": enriched["position"],
                "value": heuristic_value,
                "value_source": "heuristic_tier_median",
                "dynasty_tier": tier,
                "search_rank": enriched["search_rank"],
                "found": false,
            })
        }
    }
}

impl AnalysisService {
    /// Resolves and values every requested pick, extracted out of
    /// `trade_value_check` so that function stays focused on the
    /// player-pricing path it already had.
    async fn resolve_pick_values(
        &self,
        league_id: &str,
        playoff_teams: Option<i32>,
        all_values: &[Value],
        by_sleeper_id: &HashMap<&str, &Value>,
        picks_requested: &[PickDescriptor],
    ) -> Result<Vec<Value>> {
        if picks_requested.is_empty() {
            return Ok(Vec::new());
        }
        let (rosters, _users) = self.fetch_rosters_and_users(league_id).await?;
        let traded_picks = self.client.get_traded_picks(league_id).await?;
        let state = self.client.get_nfl_state().await?;
        let draft_order = picks::project_draft_order(&rosters);
        let tercile_year = picks::tercile_year(all_values);
        let ctx = PickValuationContext {
            traded_picks: &traded_picks,
            draft_order: &draft_order,
            by_sleeper_id,
            tercile_year: tercile_year.as_deref(),
            playoff_teams,
            standings_snapshot_week: state.week,
        };
        Ok(picks_requested.iter().map(|pd| picks::summarize_pick_value(pd, &ctx)).collect())
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
    /// players and/or draft picks. Auto-derives numQbs/ppr from the league's actual settings so
    /// values match its format (e.g. superflex leagues value QBs much higher than 1-QB leagues).
    ///
    /// No major dynasty value source (FantasyCalc, DynastyProcess, KeepTradeCut — all checked
    /// directly) prices IDP players at all; there is no market-consensus number to fall back
    /// to. For any player FantasyCalc doesn't cover (IDP, or an obscure offensive player),
    /// `value` is instead this project's own heuristic: the median FantasyCalc value of
    /// offensive players sharing that player's `dynasty_tier`, computed live from the same
    /// fetch rather than hardcoded. `found: false` still means "not in FantasyCalc's own
    /// dataset" — `value_source` distinguishes a real market price from this estimate.
    ///
    /// Picks get the analogous treatment: FantasyCalc prices a generic "2027 2nd (Mid)" slot,
    /// not a specific team's pick, so `picks.rs` resolves current ownership (trades already
    /// made) and projects which third of the round the owning roster will likely land in from
    /// this league's own current standings — only for the one upcoming draft year FantasyCalc
    /// splits into Early/Mid/Late at all; further-out years get the plain generic value instead.
    /// Rosters/traded-picks/NFL-state are only fetched when `picks` is non-empty.
    pub async fn trade_value_check(
        &self,
        league_id: &str,
        player_ids: &[String],
        picks_requested: &[PickDescriptor],
    ) -> Result<serde_json::Value> {
        let league = self.client.get_league(league_id).await?;
        let num_qbs: u32 = if league.is_superflex() { 2 } else { 1 };
        let ppr = league.ppr();

        let all_values = self
            .client
            .get_trade_values(true, num_qbs, 12, ppr)
            .await?;
        let all_players = self.client.get_all_players("nfl").await?;
        let tier_medians = tier_value_medians(&all_values, &all_players);

        let by_sleeper_id: HashMap<&str, &Value> = all_values
            .iter()
            .filter_map(|v| v["player"]["sleeperId"].as_str().map(|id| (id, v)))
            .collect();

        let players: Vec<_> = player_ids
            .iter()
            .map(|pid| {
                summarize_trade_value(pid, &all_players, &by_sleeper_id, &tier_medians)
            })
            .collect();

        let pick_rows = self
            .resolve_pick_values(
                league_id,
                league.settings.playoff_teams,
                &all_values,
                &by_sleeper_id,
                picks_requested,
            )
            .await?;

        Ok(serde_json::json!({
            "source": "fantasycalc.com",
            "is_dynasty": true,
            "num_qbs": num_qbs,
            "ppr": ppr,
            "note": "value_source \"fantasycalc\" is a real market price; \"heuristic_tier_median\" \
                     is this project's own estimate (no dynasty value source prices IDP at all) — \
                     treat it as a rough same-tier comparison, not a market consensus. For picks, \
                     \"pick_slot_projection\" is this league's own standings-based slot estimate (will \
                     move as standings change), \"pick_slot_generic\" is a year+round value too far out \
                     to project a slot for, and \"unpriced\" is outside FantasyCalc's coverage entirely.",
            "players": players,
            "picks": pick_rows,
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

#[cfg(test)]
mod idp_heuristic_value_tests {
    use super::{nearest_tier_median, summarize_trade_value, tier_value_medians};
    use std::collections::HashMap;

    fn player(name: &str, age: i64, years_exp: i64, team: &str, position: &str, search_rank: i64) -> serde_json::Value {
        serde_json::json!({
            "full_name": name, "age": age, "years_exp": years_exp,
            "team": team, "position": position, "search_rank": search_rank,
        })
    }

    fn fantasycalc_entry(sleeper_id: &str, position: &str, value: f64, overall_rank: i64) -> serde_json::Value {
        serde_json::json!({
            "player": {"sleeperId": sleeper_id, "position": position},
            "value": value, "overallRank": overall_rank, "positionRank": 1,
        })
    }

    #[test]
    fn tier_value_medians_should_bucket_offensive_players_by_their_own_dynasty_tier() {
        let mut all_players = HashMap::new();
        // Two aging RBs (years_exp high enough to land in tier 4 regardless of age)
        // land in the same tier bucket as each other, not mixed with a tier-1 rookie.
        all_players.insert("1".to_string(), player("Old RB", 30, 10, "KC", "RB", 500));
        all_players.insert("2".to_string(), player("Older RB", 32, 12, "KC", "RB", 600));
        all_players.insert("3".to_string(), player("Rookie WR", 22, 0, "KC", "WR", 5));
        let all_values = vec![
            fantasycalc_entry("1", "RB", 100.0, 200),
            fantasycalc_entry("2", "RB", 200.0, 150),
            fantasycalc_entry("3", "WR", 8000.0, 1),
        ];
        let medians = tier_value_medians(&all_values, &all_players);
        // Both tier-4 RBs' median must come from each other, not the tier-1 rookie.
        let tier4 = medians.get(&4).copied().expect("tier 4 bucket populated");
        assert!(
            (90.0..=200.0).contains(&tier4),
            "tier-4 median should sit between the two tier-4 values, got {tier4}"
        );
        assert_eq!(medians.get(&1).copied(), Some(8000.0), "tier-1 bucket must not blend with tier 4");
    }

    #[test]
    fn nearest_tier_median_should_fall_back_to_the_closest_populated_tier() {
        let mut medians = HashMap::new();
        medians.insert(2, 500.0);
        // Tier 3 has no data at all — must fall back to the nearest tier (2), not vanish.
        assert_eq!(nearest_tier_median(&medians, 3), Some(500.0));
        assert_eq!(nearest_tier_median(&medians, 7), None, "no tier within range should return None, not panic");
    }

    #[test]
    fn summarize_trade_value_should_give_an_idp_player_a_heuristic_value_not_null() {
        let mut all_players = HashMap::new();
        // IDP player: FantasyCalc has no entry for them at all.
        all_players.insert("idp1".to_string(), player("Sample LB", 26, 5, "KC", "LB", 166));
        all_players.insert("1".to_string(), player("Old RB", 30, 10, "KC", "RB", 500));
        let all_values = vec![fantasycalc_entry("1", "RB", 150.0, 200)];
        let by_sleeper_id: HashMap<&str, &serde_json::Value> =
            all_values.iter().map(|v| (v["player"]["sleeperId"].as_str().unwrap(), v)).collect();
        let tier_medians = tier_value_medians(&all_values, &all_players);

        let row = summarize_trade_value("idp1", &all_players, &by_sleeper_id, &tier_medians);
        assert_eq!(row["found"], false, "IDP player is still not in FantasyCalc's own dataset");
        assert_eq!(row["value_source"], "heuristic_tier_median");
        assert!(!row["value"].is_null(), "IDP player must get a usable value, not null, got {row}");
        assert_eq!(row["value"], 150.0, "a lone tier-4 RB's value becomes the tier-4 median the LB shares");
    }

    #[test]
    fn summarize_trade_value_should_use_the_real_fantasycalc_price_when_found() {
        let mut all_players = HashMap::new();
        all_players.insert("1".to_string(), player("Found RB", 25, 3, "KC", "RB", 50));
        let all_values = vec![fantasycalc_entry("1", "RB", 777.0, 10)];
        let by_sleeper_id: HashMap<&str, &serde_json::Value> =
            all_values.iter().map(|v| (v["player"]["sleeperId"].as_str().unwrap(), v)).collect();
        let tier_medians = tier_value_medians(&all_values, &all_players);

        let row = summarize_trade_value("1", &all_players, &by_sleeper_id, &tier_medians);
        assert_eq!(row["found"], true);
        assert_eq!(row["value_source"], "fantasycalc");
        assert_eq!(row["value"], 777.0, "a found player's value must be FantasyCalc's own number, not the heuristic");
    }
}
