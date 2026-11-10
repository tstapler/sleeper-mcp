//! Roster/league composition: a single roster resolved to player names, a
//! whole-league snapshot (persisted as a time series), positional-needs
//! analysis, and transaction-history sync.
use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

use crate::error::Result;
use crate::models::League;

use super::dynasty::enrich_player;
use super::AnalysisService;

impl AnalysisService {
    // -----------------------------------------------------------------------
    // Roster with player names
    // -----------------------------------------------------------------------

    pub async fn roster_with_names(
        &self,
        league_id: &str,
        roster_id: i32,
    ) -> Result<serde_json::Value> {
        let (rosters, users, all_players) = self.fetch_rosters_users_players(league_id).await?;

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
            "taxi": roster.taxi.iter().flatten().map(|p| enrich(p)).collect::<Vec<_>>(),
            "record": {
                "wins": roster.settings.wins,
                "losses": roster.settings.losses,
                "ties": roster.settings.ties,
                "fpts": roster.settings.total_fpts(),
            },
        }))
    }

    // -----------------------------------------------------------------------
    // League snapshot — every roster, resolved and position-tallied, for an
    // at-a-glance view across the whole league (not just one roster).
    // -----------------------------------------------------------------------

    /// Rich, name-resolved view of every roster in the league: owner, record,
    /// per-position counts, and enriched (name/position/age/tier/injury)
    /// player lists split by starters/bench/taxi/reserve. Also appends one
    /// row per roster to `roster_snapshots` (`self.db`) so the same data is
    /// queryable as a time series later — callers that only want the JSON
    /// (no persistence) should read `roster.players` etc. directly instead.
    pub async fn league_snapshot(&self, league_id: &str) -> Result<serde_json::Value> {
        let (rosters, users, all_players, league) =
            self.fetch_rosters_users_players_league(league_id).await?;

        let user_map: HashMap<_, _> =
            users.iter().map(|u| (u.user_id.clone(), u.display_name.clone())).collect();
        let enrich = |pid: &str| enrich_player(pid, &all_players);

        let mut roster_summaries = Vec::with_capacity(rosters.len());
        for r in &rosters {
            let owner_name = r
                .owner_id
                .as_deref()
                .and_then(|id| user_map.get(id))
                .cloned()
                .unwrap_or_else(|| "Unknown".to_string());

            let taxi_ids: Vec<&String> = r.taxi.iter().flatten().collect();
            let bench: Vec<serde_json::Value> = r
                .players
                .iter()
                .filter(|p| {
                    !r.starters.contains(p) && !r.reserve.contains(p) && !taxi_ids.contains(p)
                })
                .map(|p| enrich(p))
                .collect();
            let starters: Vec<_> = r.starters.iter().map(|p| enrich(p)).collect();
            let taxi: Vec<_> = taxi_ids.iter().map(|p| enrich(p)).collect();
            let reserve: Vec<_> = r.reserve.iter().map(|p| enrich(p)).collect();

            let mut position_counts: std::collections::BTreeMap<String, usize> =
                Default::default();
            for p in &r.players {
                let pos = enrich(p)["position"].as_str().unwrap_or("?").to_string();
                *position_counts.entry(pos).or_default() += 1;
            }

            let summary = serde_json::json!({
                "roster_id": r.roster_id,
                "owner": owner_name,
                "record": {
                    "wins": r.settings.wins,
                    "losses": r.settings.losses,
                    "ties": r.settings.ties,
                    "fpts": r.settings.total_fpts(),
                },
                "position_counts": position_counts,
                "starters": starters,
                "bench": bench,
                "taxi": taxi,
                "reserve": reserve,
            });

            self.db
                .upsert_roster_snapshot(crate::db::RosterSnapshotRow {
                    league_id,
                    roster_id: r.roster_id,
                    season: &league.season,
                    owner: Some(owner_name.as_str()),
                    wins: r.settings.wins,
                    losses: r.settings.losses,
                    data: &summary,
                })
                .await?;

            roster_summaries.push(summary);
        }

        Ok(serde_json::json!({
            "league_id": league_id,
            "league_name": league.name,
            "season": league.season,
            "fetched_at": chrono::Utc::now().to_rfc3339(),
            "rosters": roster_summaries,
        }))
    }

    /// Fetch + resolve + persist transaction history for `weeks` (idempotent
    /// per transaction_id — safe to re-run over already-synced weeks).
    /// `adds`/`drops` are stored pre-resolved (name/position/roster owner),
    /// not bare player_ids, so `sqlite3 stats.db` queries need no join.
    /// Returns the number of transactions stored.
    pub async fn sync_transaction_history(
        &self,
        league_id: &str,
        weeks: &[i32],
    ) -> Result<usize> {
        let players_h = self.spawn_all_players();
        let league_h = self.spawn_league(league_id);
        let members = self.db.list_league_members(league_id).await?;

        let all_players = players_h.await??;
        let season = league_h.await??.season;
        let owner_by_roster: HashMap<i32, String> = members
            .into_iter()
            .map(|(roster_id, _user_id, username, display_name)| {
                (roster_id, display_name.or(username).unwrap_or_else(|| format!("roster {roster_id}")))
            })
            .collect();

        let resolve_side = |side: &HashMap<String, i32>| -> serde_json::Value {
            side.iter()
                .map(|(pid, roster_id)| {
                    let e = enrich_player(pid, &all_players);
                    serde_json::json!({
                        "player_id": pid,
                        "name": e["name"],
                        "position": e["position"],
                        "roster_id": roster_id,
                        "owner": owner_by_roster.get(roster_id).cloned().unwrap_or_default(),
                    })
                })
                .collect()
        };

        let mut stored = 0usize;
        for &week in weeks {
            let txns = self.client.get_league_transactions(league_id, week).await?;
            for t in &txns {
                let adds = t.adds.clone().unwrap_or_default();
                let drops = t.drops.clone().unwrap_or_default();
                self.db
                    .upsert_transaction(crate::db::TransactionRow {
                        transaction_id: &t.transaction_id,
                        league_id,
                        season: &season,
                        week,
                        kind: &t.kind,
                        status: Some(t.status.as_str()),
                        created: t.created,
                        adds: &resolve_side(&adds),
                        drops: &resolve_side(&drops),
                        roster_ids: &serde_json::json!(t.roster_ids),
                        waiver_bid: None,
                    })
                    .await?;
                stored += 1;
            }
        }
        Ok(stored)
    }

    // -----------------------------------------------------------------------
    // Roster needs analysis
    // -----------------------------------------------------------------------

    pub async fn analyze_roster_needs(
        &self,
        league_id: &str,
        roster_id: i32,
    ) -> Result<serde_json::Value> {
        let league_h = self.spawn_league(league_id);
        let data = self.roster_with_names(league_id, roster_id).await?;
        let league = league_h.await??;

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

        // Active-roster capacity: taxi/reserve squads are separate pools (per taxi_slots/
        // reserve_slots) that don't count against roster_positions' starter+bench slots.
        let taxi = data["taxi"].as_array().cloned().unwrap_or_default();
        let reserve_used = data["reserve"].as_array().map(Vec::len).unwrap_or(0);
        let capacity = roster_capacity(players.len(), taxi.len(), reserve_used, &league);
        let (taxi_ineligible, taxi_ineligible_msgs) = taxi_ineligibility(&taxi, &league.settings.extra);

        let mut priorities = weaknesses.clone();
        priorities.extend(capacity.warnings());
        priorities.extend(taxi_ineligible_msgs);
        priorities.extend(drop_recs);

        Ok(serde_json::json!({
            "owner": data["owner"],
            "position_counts": pos_counts,
            "strengths": strengths,
            "weaknesses": weaknesses,
            "injured_or_out": injured,
            "no_team_free_agents": no_team,
            "taxi_ineligible": taxi_ineligible,
            "roster_capacity": capacity,
            "suggested_priorities": priorities,
        }))
    }
}

/// Active-roster/taxi/reserve occupancy vs. the league's configured limits.
#[derive(Serialize)]
struct RosterCapacity {
    active_count: usize,
    active_cap: usize,
    over_by: usize,
    taxi_used: usize,
    taxi_slots: u64,
    taxi_over_by: u64,
    reserve_used: usize,
    reserve_slots: u64,
    reserve_over_by: u64,
}

impl RosterCapacity {
    /// Human-readable warnings for any limit currently exceeded.
    fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        if self.over_by > 0 {
            w.push(format!(
                "Over the active-roster limit by {} ({}/{}, excluding taxi/reserve) — cut or move players to taxi/reserve",
                self.over_by, self.active_count, self.active_cap
            ));
        }
        if self.taxi_over_by > 0 {
            w.push(format!(
                "Over the taxi-squad limit by {} ({}/{})",
                self.taxi_over_by, self.taxi_used, self.taxi_slots
            ));
        }
        if self.reserve_over_by > 0 {
            w.push(format!(
                "Over the reserve-squad limit by {} ({}/{})",
                self.reserve_over_by, self.reserve_used, self.reserve_slots
            ));
        }
        w
    }
}

/// Active-roster capacity: taxi/reserve squads are separate pools (per taxi_slots/
/// reserve_slots) that don't count against roster_positions' starter+bench slots.
fn roster_capacity(total_players: usize, taxi_used: usize, reserve_used: usize, league: &League) -> RosterCapacity {
    let active_count = total_players.saturating_sub(taxi_used).saturating_sub(reserve_used);
    let active_cap = league.roster_positions.len();
    let taxi_slots = league.settings.extra.get("taxi_slots").and_then(Value::as_u64).unwrap_or(0);
    let reserve_slots = league.settings.extra.get("reserve_slots").and_then(Value::as_u64).unwrap_or(0);
    RosterCapacity {
        active_count,
        active_cap,
        over_by: active_count.saturating_sub(active_cap),
        taxi_used,
        taxi_slots,
        taxi_over_by: (taxi_used as u64).saturating_sub(taxi_slots),
        reserve_used,
        reserve_slots,
        reserve_over_by: (reserve_used as u64).saturating_sub(reserve_slots),
    }
}

/// Taxi eligibility: `taxi_allow_vets=0` restricts the taxi squad to rookies
/// (years_exp == 0); `taxi_allow_vets=1` raises the cap to `taxi_years` of experience.
/// Returns the ineligible players alongside one warning message per player.
fn taxi_ineligibility(taxi: &[Value], settings: &HashMap<String, Value>) -> (Vec<Value>, Vec<String>) {
    let taxi_allow_vets = settings.get("taxi_allow_vets").and_then(Value::as_u64).unwrap_or(0) != 0;
    let taxi_years = settings.get("taxi_years").and_then(Value::as_i64).unwrap_or(0);

    let ineligible: Vec<Value> = taxi
        .iter()
        .filter(|p| {
            let years_exp = p["years_exp"].as_i64().unwrap_or(0);
            if taxi_allow_vets { years_exp > taxi_years } else { years_exp > 0 }
        })
        .cloned()
        .collect();

    let messages = ineligible
        .iter()
        .map(|p| {
            format!(
                "Taxi-ineligible: {} has {} year(s) exp (limit {taxi_years}) — move to active roster or drop before the taxi deadline",
                p["name"].as_str().unwrap_or("?"),
                p["years_exp"].as_i64().unwrap_or(0),
            )
        })
        .collect();

    (ineligible, messages)
}
