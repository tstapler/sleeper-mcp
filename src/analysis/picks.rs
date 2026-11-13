//! Dynasty draft-pick valuation. No external source prices a *specific*
//! team's future pick — FantasyCalc prices generic "2027 2nd (Mid)"-style
//! slots, not "roster 7's 2027 2nd". This module bridges that gap: resolve
//! who actually owns the pick today (trades already made), project which
//! third of the round it will likely land in from this league's own
//! current standings, then anchor that projection to FantasyCalc's real
//! Early/Mid/Late values — same external-anchor-plus-in-house-bucketing
//! shape as `trades.rs`'s IDP tier-median heuristic.
use std::collections::HashMap;

use serde_json::Value;

use crate::models::{Roster, TradedPick};

/// Which third of the round a pick projects into. Confirmed empirically
/// (2026-11-11) against this league's real history — its 2026 draft_order
/// reverses its 2025 final standings (worse record -> earlier pick), the
/// standard dynasty convention, not a lottery. "Early" = the most valuable
/// slots in the round, assigned to the worst teams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tercile {
    Early,
    Mid,
    Late,
}

impl Tercile {
    /// FantasyCalc's own naming for this tercile, used to build its sleeperId.
    fn id_part(&self) -> &'static str {
        match self {
            Tercile::Early => "early",
            Tercile::Mid => "mid",
            Tercile::Late => "late",
        }
    }

    /// Human-readable label for output.
    pub fn label(self) -> &'static str {
        match self {
            Tercile::Early => "Early",
            Tercile::Mid => "Mid",
            Tercile::Late => "Late",
        }
    }
}

/// A pick identified the way Sleeper's own `traded_picks` endpoint does:
/// by season, round, and *original* roster — not by current owner, since
/// that's exactly what needs resolving.
#[derive(Debug, Clone)]
pub struct PickDescriptor {
    pub season: String,
    pub round: i32,
    pub original_roster_id: i32,
}

/// Resolves a pick's current owner. A matching `traded_picks` row wins;
/// absence means the pick has never changed hands, so it's still the
/// original roster's own.
pub fn resolve_pick_owner(traded_picks: &[TradedPick], season: &str, round: i32, original_roster_id: i32) -> i32 {
    traded_picks
        .iter()
        .find(|p| p.season == season && p.round == round && p.roster_id == original_roster_id)
        .map(|p| p.owner_id)
        .unwrap_or(original_roster_id)
}

/// This league's projected draft order, worst team first (pick 1). Reverses
/// the same wins-then-points tie-break `AnalysisService::standings` already
/// uses for the opposite (best-first) ranking — picking a second, divergent
/// tie-break here would silently disagree with the standings this project
/// already shows the user elsewhere.
pub fn project_draft_order(rosters: &[Roster]) -> Vec<i32> {
    let mut ranked: Vec<&Roster> = rosters.iter().collect();
    ranked.sort_by(|a, b| {
        let by_wins = a.settings.wins.cmp(&b.settings.wins);
        if by_wins != std::cmp::Ordering::Equal {
            return by_wins;
        }
        a.settings
            .total_fpts()
            .partial_cmp(&b.settings.total_fpts())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ranked.into_iter().map(|r| r.roster_id).collect()
}

/// Which third of the round a roster's pick projects into, given the
/// league's draft-order projection (worst-first, per `project_draft_order`).
/// `None` only if `roster_id` isn't in `draft_order` at all.
pub fn tercile_for_roster(draft_order: &[i32], roster_id: i32) -> Option<Tercile> {
    let idx = draft_order.iter().position(|&r| r == roster_id)?;
    let n = draft_order.len();
    if n == 0 {
        return None;
    }
    let third = n.div_ceil(3).max(1);
    Some(if idx < third {
        Tercile::Early
    } else if idx < third * 2 {
        Tercile::Mid
    } else {
        Tercile::Late
    })
}

/// The one upcoming draft year FantasyCalc's PICK dataset splits into
/// Early/Mid/Late at all (every further-out year only gets a plain
/// year+round value) — detected live from the dataset's own entry names
/// rather than hardcoded, so this keeps working as years roll over without
/// a code change.
pub fn tercile_year(all_values: &[Value]) -> Option<String> {
    all_values.iter().find_map(|v| {
        if v["player"]["position"].as_str() != Some("PICK") {
            return None;
        }
        let name = v["player"]["name"].as_str()?;
        if !name.contains("(Early)") {
            return None;
        }
        name.split_whitespace().next().map(str::to_string)
    })
}

/// FantasyCalc's own `sleeperId` format for a pick — live-verified
/// (2026-11-11): tercile picks are `FP_{year}_{tercile}_{round-1}`
/// (0-indexed round), plain picks are `FP_{year}_{round}` (1-indexed).
pub fn pick_sleeper_id(year: &str, round: i32, tercile: Option<Tercile>) -> String {
    match tercile {
        Some(t) => format!("FP_{year}_{}_{}", t.id_part(), round - 1),
        None => format!("FP_{year}_{round}"),
    }
}

/// Everything `summarize_pick_value` needs besides the pick itself —
/// grouped so adding a new input doesn't grow a positional parameter list.
pub struct PickValuationContext<'a> {
    pub traded_picks: &'a [TradedPick],
    pub draft_order: &'a [i32],
    pub by_sleeper_id: &'a HashMap<&'a str, &'a Value>,
    pub tercile_year: Option<&'a str>,
    pub playoff_teams: Option<i32>,
    pub standings_snapshot_week: i32,
}

/// True when the owning roster's current rank is still within the playoff
/// field — if so, its eventual draft slot depends on playoff results too,
/// not just where the regular season leaves it (this league's draft order
/// reverses *final* standings, confirmed against its real history — see
/// `project_draft_order`'s doc comment).
fn is_playoff_contender(draft_order: &[i32], owner: i32, playoff_teams: Option<i32>) -> bool {
    let n = draft_order.len();
    draft_order
        .iter()
        .position(|&r| r == owner)
        .zip(playoff_teams)
        .is_some_and(|(idx, pt)| (n - idx) as i32 <= pt)
}

/// The caveat attached to a slot-projected (not generic) pick value.
fn projection_note(ctx: &PickValuationContext, owner: i32) -> String {
    let week = ctx.standings_snapshot_week;
    let base = format!(
        "Projected from the owning roster's current standing (week {week}) via this league's \
         own reverse-standings draft-order convention (confirmed against its real draft \
         history, not assumed) — not a fixed price."
    );
    if is_playoff_contender(ctx.draft_order, owner, ctx.playoff_teams) {
        format!(
            "{base} That roster is still mathematically playoff-eligible, so its final slot \
             depends on games (and possibly playoff results) still to be played, and will keep \
             moving."
        )
    } else {
        base
    }
}

fn unpriced_pick_row(pick: &PickDescriptor, owner: i32, label: &str) -> Value {
    serde_json::json!({
        "pick": label,
        "season": pick.season,
        "round": pick.round,
        "original_roster_id": pick.original_roster_id,
        "current_owner_roster_id": owner,
        "value": null,
        "value_source": "unpriced",
        "note": "Out of FantasyCalc's pick-pricing coverage (year or round beyond what their dataset prices).",
    })
}

/// One pick's row in `trade_value_check`'s output. Mirrors `trades.rs`'s
/// `summarize_trade_value` shape (`value`/`value_source`/`found`-style
/// fields) so a caller handling both lists doesn't need two different
/// conventions.
pub fn summarize_pick_value(pick: &PickDescriptor, ctx: &PickValuationContext) -> Value {
    let owner = resolve_pick_owner(ctx.traded_picks, &pick.season, pick.round, pick.original_roster_id);
    let label = format!("{} Round {}", pick.season, pick.round);

    let project_slot = ctx.tercile_year == Some(pick.season.as_str());
    let tercile = if project_slot { tercile_for_roster(ctx.draft_order, owner) } else { None };

    let sleeper_id = pick_sleeper_id(&pick.season, pick.round, tercile);
    let Some(entry) = ctx.by_sleeper_id.get(sleeper_id.as_str()) else {
        return unpriced_pick_row(pick, owner, &label);
    };

    let note = match tercile {
        None => "Too far out to project a draft slot — generic year+round value only, no \
                 standings-based adjustment."
            .to_string(),
        Some(_) => projection_note(ctx, owner),
    };

    serde_json::json!({
        "pick": label,
        "season": pick.season,
        "round": pick.round,
        "original_roster_id": pick.original_roster_id,
        "current_owner_roster_id": owner,
        "value": entry["value"],
        "value_source": if tercile.is_some() { "pick_slot_projection" } else { "pick_slot_generic" },
        "projected_tercile": tercile.map(Tercile::label),
        "note": note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster(roster_id: i32, wins: i32, fpts_whole: i32) -> Roster {
        Roster {
            roster_id,
            owner_id: None,
            league_id: "L".to_string(),
            players: vec![],
            starters: vec![],
            reserve: vec![],
            taxi: None,
            settings: crate::models::RosterSettings {
                wins,
                losses: 14 - wins,
                ties: 0,
                fpts_whole,
                fpts_decimal: 0,
                fpts_against: 0,
                fpts_against_decimal: 0,
            },
        }
    }

    fn traded(season: &str, round: i32, roster_id: i32, owner_id: i32) -> TradedPick {
        TradedPick {
            season: season.to_string(),
            round,
            roster_id,
            previous_owner_id: roster_id,
            owner_id,
        }
    }

    fn pick_entry(sleeper_id: &str, name: &str, value: f64) -> Value {
        serde_json::json!({
            "player": {"sleeperId": sleeper_id, "position": "PICK", "name": name},
            "value": value,
        })
    }

    /// Real finding this module is built on (2026-11-11): worse record ->
    /// earlier pick, reverse-standings, not a lottery.
    #[test]
    fn project_draft_order_should_rank_worst_record_first() {
        let rosters = vec![roster(1, 11, 2000), roster(2, 3, 1600), roster(3, 7, 1800)];
        let order = project_draft_order(&rosters);
        assert_eq!(order, vec![2, 3, 1], "worst record (fewest wins) must pick first");
    }

    #[test]
    fn project_draft_order_should_break_ties_by_lower_points_first() {
        let rosters = vec![roster(1, 5, 2000), roster(2, 5, 1500)];
        let order = project_draft_order(&rosters);
        assert_eq!(order, vec![2, 1], "equal wins: fewer points still picks earlier");
    }

    #[test]
    fn tercile_for_roster_should_bucket_into_thirds_of_the_draft_order() {
        let order = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        assert_eq!(tercile_for_roster(&order, 1), Some(Tercile::Early));
        assert_eq!(tercile_for_roster(&order, 4), Some(Tercile::Early));
        assert_eq!(tercile_for_roster(&order, 5), Some(Tercile::Mid));
        assert_eq!(tercile_for_roster(&order, 9), Some(Tercile::Late));
        assert_eq!(tercile_for_roster(&order, 12), Some(Tercile::Late));
    }

    #[test]
    fn tercile_for_roster_should_return_none_for_an_unknown_roster() {
        let order = vec![1, 2, 3];
        assert_eq!(tercile_for_roster(&order, 99), None);
    }

    #[test]
    fn resolve_pick_owner_should_return_current_owner_when_traded() {
        let picks = vec![traded("2027", 2, 7, 6)];
        assert_eq!(resolve_pick_owner(&picks, "2027", 2, 7), 6);
    }

    #[test]
    fn resolve_pick_owner_should_return_original_roster_when_never_traded() {
        let picks = vec![traded("2027", 2, 7, 6)];
        // Different round -> no match -> falls back to the original roster.
        assert_eq!(resolve_pick_owner(&picks, "2027", 3, 7), 7);
    }

    #[test]
    fn tercile_year_should_detect_the_one_year_with_early_mid_late_splits() {
        let values = vec![
            pick_entry("FP_2027_early_0", "2027 1st (Early)", 5000.0),
            pick_entry("FP_2028_1", "2028 1st", 2000.0),
        ];
        assert_eq!(tercile_year(&values), Some("2027".to_string()));
    }

    #[test]
    fn pick_sleeper_id_should_format_tercile_and_plain_picks_per_fantasycalc_convention() {
        assert_eq!(pick_sleeper_id("2027", 2, Some(Tercile::Early)), "FP_2027_early_1");
        assert_eq!(pick_sleeper_id("2028", 1, None), "FP_2028_1");
    }

    #[test]
    fn summarize_pick_value_should_project_tercile_for_the_current_owner_in_the_tercile_year() {
        // Roster 7 traded this pick away to roster 6; roster 6 is the worst
        // team in the league (picks first) -> Early tercile.
        let traded_picks = vec![traded("2027", 2, 7, 6)];
        let rosters = vec![roster(6, 2, 1400), roster(5, 10, 1900)];
        let draft_order = project_draft_order(&rosters);
        let values = [
            pick_entry("FP_2027_early_1", "2027 2nd (Early)", 2113.0),
            pick_entry("FP_2027_mid_1", "2027 2nd (Mid)", 1773.0),
        ];
        let by_sleeper_id: HashMap<&str, &Value> =
            values.iter().map(|v| (v["player"]["sleeperId"].as_str().unwrap(), v)).collect();

        let pick = PickDescriptor { season: "2027".to_string(), round: 2, original_roster_id: 7 };
        let ctx = PickValuationContext {
            traded_picks: &traded_picks,
            draft_order: &draft_order,
            by_sleeper_id: &by_sleeper_id,
            tercile_year: Some("2027"),
            playoff_teams: Some(6),
            standings_snapshot_week: 10,
        };
        let row = summarize_pick_value(&pick, &ctx);

        assert_eq!(row["current_owner_roster_id"], 6);
        assert_eq!(row["value"], 2113.0, "must use roster 6's projected (Early) tercile, not a generic value");
        assert_eq!(row["value_source"], "pick_slot_projection");
        assert_eq!(row["projected_tercile"], "Early");
    }

    #[test]
    fn summarize_pick_value_should_use_generic_value_outside_the_tercile_year() {
        let values = [pick_entry("FP_2028_3", "2028 3rd", 1023.0)];
        let by_sleeper_id: HashMap<&str, &Value> =
            values.iter().map(|v| (v["player"]["sleeperId"].as_str().unwrap(), v)).collect();

        let pick = PickDescriptor { season: "2028".to_string(), round: 3, original_roster_id: 2 };
        let ctx = PickValuationContext {
            traded_picks: &[],
            draft_order: &[],
            by_sleeper_id: &by_sleeper_id,
            tercile_year: Some("2027"),
            playoff_teams: Some(6),
            standings_snapshot_week: 10,
        };
        let row = summarize_pick_value(&pick, &ctx);

        assert_eq!(row["value"], 1023.0);
        assert_eq!(row["value_source"], "pick_slot_generic");
        assert!(row["projected_tercile"].is_null());
    }

    #[test]
    fn summarize_pick_value_should_mark_unpriced_when_fantasycalc_has_no_matching_entry() {
        let pick = PickDescriptor { season: "2031".to_string(), round: 9, original_roster_id: 2 };
        let ctx = PickValuationContext {
            traded_picks: &[],
            draft_order: &[],
            by_sleeper_id: &HashMap::new(),
            tercile_year: None,
            playoff_teams: None,
            standings_snapshot_week: 10,
        };
        let row = summarize_pick_value(&pick, &ctx);

        assert!(row["value"].is_null());
        assert_eq!(row["value_source"], "unpriced");
    }
}
