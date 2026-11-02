//! Start/sit recommendations: greedy slot-fill lineup optimization from
//! weekly projections.
use std::collections::HashSet;

use serde_json::Value;

use crate::error::Result;

use super::AnalysisService;

impl AnalysisService {
    // -----------------------------------------------------------------------
    // Start/sit recommendations
    // -----------------------------------------------------------------------

    pub async fn start_sit(
        &self,
        league_id: &str,
        roster_id: i32,
        week: i32,
    ) -> Result<serde_json::Value> {
        let state_h = self.spawn_nfl_state();
        let rosters_h = self.spawn_rosters(league_id);
        let league_h = self.spawn_league(league_id);
        let players_h = self.spawn_all_players();

        let state = state_h.await??;
        let rosters = rosters_h.await??;
        let league = league_h.await??;
        let all_players = players_h.await??;

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

        // Split the roster into players eligible to be started (Active, and not
        // Out/IR/PUP) vs. players who can't play this week regardless of projection.
        struct Candidate {
            pid: String,
            position: String,
            points: f64,
        }
        let mut available: Vec<Candidate> = Vec::new();
        let mut unavailable: Vec<serde_json::Value> = Vec::new();

        for pid in &roster.players {
            let obj = all_players.get(pid).and_then(Value::as_object);
            let position = obj
                .and_then(|m| m.get("position"))
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            let status = obj
                .and_then(|m| m.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("Active");
            let injury_status = obj.and_then(|m| m.get("injury_status")).and_then(Value::as_str);
            let points = proj_pts(pid);

            if status != "Active" || matches!(injury_status, Some("Out") | Some("IR") | Some("PUP")) {
                unavailable.push(serde_json::json!({
                    "player_id": pid,
                    "position": position,
                    "status": status,
                    "injury_status": injury_status,
                    "projected_points": points,
                    "reason": "excluded — not active/eligible to play this week",
                }));
            } else {
                available.push(Candidate { pid: pid.clone(), position, points });
            }
        }

        // Slots in most-restrictive-first order so a dedicated QB/RB/WR/TE/IDP slot
        // never loses its best fit to a wider FLEX/SUPER_FLEX/IDP_FLEX slot filled first.
        let mut slots: Vec<&str> = league
            .roster_positions
            .iter()
            .map(String::as_str)
            .filter(|s| *s != "BN")
            .collect();
        slots.sort_by_key(|s| slot_eligible_positions(s).len());

        let mut used: HashSet<usize> = HashSet::new();
        let mut lineup: Vec<serde_json::Value> = Vec::new();

        for slot in &slots {
            let eligible = slot_eligible_positions(slot);
            let pick = available
                .iter()
                .enumerate()
                .filter(|(i, c)| !used.contains(i) && eligible.contains(&c.position.as_str()))
                .max_by(|(_, a), (_, b)| a.points.partial_cmp(&b.points).unwrap_or(std::cmp::Ordering::Equal));

            match pick {
                Some((i, c)) => {
                    used.insert(i);
                    lineup.push(serde_json::json!({
                        "slot": slot,
                        "player_id": c.pid.clone(),
                        "position": c.position.clone(),
                        "projected_points": c.points,
                    }));
                }
                None => {
                    lineup.push(serde_json::json!({
                        "slot": slot,
                        "player_id": null,
                        "position": null,
                        "projected_points": 0.0,
                        "note": "no eligible rostered player available for this slot",
                    }));
                }
            }
        }

        let mut bench: Vec<_> = available
            .iter()
            .enumerate()
            .filter(|(i, _)| !used.contains(i))
            .map(|(_, c)| serde_json::json!({
                "player_id": c.pid.clone(),
                "position": c.position.clone(),
                "projected_points": c.points,
            }))
            .collect();
        bench.sort_by(|a, b| {
            b["projected_points"]
                .as_f64()
                .partial_cmp(&a["projected_points"].as_f64())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut all_ranked: Vec<_> = roster
            .players
            .iter()
            .map(|pid| serde_json::json!({
                "player_id": pid,
                "projected_points": proj_pts(pid),
            }))
            .collect();
        all_ranked.sort_by(|a, b| {
            b["projected_points"]
                .as_f64()
                .partial_cmp(&a["projected_points"].as_f64())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(serde_json::json!({
            "roster_id": roster_id,
            "week": week,
            "current_starters": roster.starters,
            "recommended_lineup": lineup,
            "recommended_starters": lineup.iter()
                .filter_map(|s| s["player_id"].as_str())
                .collect::<Vec<_>>(),
            "recommended_bench": bench,
            "excluded_unavailable": unavailable,
            "all_players_ranked": all_ranked,
        }))
    }
}

/// Which Sleeper roster positions a given lineup slot accepts. `FLEX`/`SUPER_FLEX`
/// cover offense; `IDP_FLEX` is disjoint (defense only) — Sleeper leagues don't mix them.
fn slot_eligible_positions(slot: &str) -> Vec<&str> {
    match slot {
        "FLEX" => vec!["RB", "WR", "TE"],
        "WRRB_FLEX" => vec!["RB", "WR"],
        "WRTE_FLEX" | "REC_FLEX" => vec!["WR", "TE"],
        "SUPER_FLEX" => vec!["QB", "RB", "WR", "TE"],
        "IDP_FLEX" => vec!["DB", "DL", "LB"],
        other => vec![other],
    }
}
