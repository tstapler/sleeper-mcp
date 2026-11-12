use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// User
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub user_id: String,
    pub username: Option<String>,
    pub display_name: String,
    pub avatar: Option<String>,
    #[serde(default, deserialize_with = "null_as_empty_map")]
    pub metadata: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// League
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LeagueSettings {
    #[serde(default)]
    pub num_teams: i32,
    #[serde(default)]
    pub best_ball: i32,
    #[serde(default)]
    pub waiver_type: i32,
    pub playoff_teams: Option<i32>,
    pub draft_rounds: Option<i32>,
    // Named out of `extra` purely for ergonomic access (`league.settings.trade_deadline`
    // instead of `.extra.get("trade_deadline")`) — `extra`'s `#[serde(flatten)]` already
    // captured every one of these before this change, so nothing was ever actually lost
    // on sync; this just stops the common ones from requiring an untyped JSON lookup.
    pub playoff_week_start: Option<i32>,
    pub playoff_type: Option<i32>,
    pub playoff_round_type: Option<i32>,
    pub playoff_seed_type: Option<i32>,
    pub trade_deadline: Option<i32>,
    pub trade_review_days: Option<i32>,
    pub disable_trades: Option<i32>,
    pub pick_trading: Option<i32>,
    pub waiver_budget: Option<i32>,
    pub waiver_clear_days: Option<i32>,
    pub waiver_day_of_week: Option<i32>,
    pub waiver_bid_min: Option<i32>,
    pub taxi_slots: Option<i32>,
    pub taxi_years: Option<i32>,
    pub taxi_deadline: Option<i32>,
    pub taxi_allow_vets: Option<i32>,
    pub reserve_slots: Option<i32>,
    pub max_keepers: Option<i32>,
    pub max_subs: Option<i32>,
    pub disable_adds: Option<i32>,
    pub veto_votes_needed: Option<i32>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct League {
    pub league_id: String,
    pub name: String,
    pub season: String,
    pub status: String,
    pub sport: String,
    #[serde(default)]
    pub settings: LeagueSettings,
    #[serde(default)]
    pub total_rosters: i32,
    pub draft_id: Option<String>,
    pub avatar: Option<String>,
    pub scoring_settings: Option<HashMap<String, Value>>,
    #[serde(default)]
    pub roster_positions: Vec<String>,
}

impl League {
    /// Derive scoring type from the `rec` setting: 0 = Standard, 0.5 = Half PPR, 1.0 = PPR.
    pub fn scoring_type(&self) -> &'static str {
        let rec = self
            .scoring_settings
            .as_ref()
            .and_then(|s| s.get("rec"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        if rec >= 1.0 {
            "PPR"
        } else if rec >= 0.5 {
            "Half PPR"
        } else {
            "Standard"
        }
    }

    pub fn is_superflex(&self) -> bool {
        self.roster_positions.iter().any(|p| p == "SUPER_FLEX")
    }

    /// Raw points-per-reception value (0.0/0.5/1.0), for APIs that want the number
    /// rather than the Standard/Half PPR/PPR label.
    pub fn ppr(&self) -> f64 {
        self.scoring_settings
            .as_ref()
            .and_then(|s| s.get("rec"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    }

    pub fn roster_slot_counts(&self) -> HashMap<String, usize> {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for pos in &self.roster_positions {
            if pos != "BN" {
                *counts.entry(pos.clone()).or_default() += 1;
            }
        }
        counts
    }
}

// ---------------------------------------------------------------------------
// Roster
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RosterSettings {
    #[serde(default)]
    pub wins: i32,
    #[serde(default)]
    pub losses: i32,
    #[serde(default)]
    pub ties: i32,
    /// Whole-number part of fantasy points for (Sleeper splits integer + decimal).
    #[serde(default, rename = "fpts")]
    pub fpts_whole: i32,
    #[serde(default)]
    pub fpts_decimal: i32,
    #[serde(default)]
    pub fpts_against: i32,
    #[serde(default)]
    pub fpts_against_decimal: i32,
}

impl RosterSettings {
    pub fn total_fpts(&self) -> f64 {
        self.fpts_whole as f64 + self.fpts_decimal as f64 / 100.0
    }
    pub fn total_fpts_against(&self) -> f64 {
        self.fpts_against as f64 + self.fpts_against_decimal as f64 / 100.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Roster {
    pub roster_id: i32,
    pub owner_id: Option<String>,
    pub league_id: String,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub players: Vec<String>,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub starters: Vec<String>,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub reserve: Vec<String>,
    pub taxi: Option<Vec<String>>,
    #[serde(default)]
    pub settings: RosterSettings,
}

// ---------------------------------------------------------------------------
// Matchup
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Matchup {
    pub matchup_id: Option<i32>,
    pub roster_id: i32,
    #[serde(default)]
    pub points: f64,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub players: Vec<String>,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub starters: Vec<String>,
    #[serde(default)]
    pub players_points: HashMap<String, f64>,
}

// ---------------------------------------------------------------------------
// Transaction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub transaction_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    pub created: i64,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub roster_ids: Vec<i32>,
    pub adds: Option<HashMap<String, i32>>,
    pub drops: Option<HashMap<String, i32>>,
}

// ---------------------------------------------------------------------------
// NFL State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NflState {
    pub season: String,
    pub week: i32,
    pub season_type: String,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// Trending player
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendingPlayer {
    pub player_id: String,
    pub count: i64,
}

// ---------------------------------------------------------------------------
// Draft
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Draft {
    pub draft_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    pub sport: String,
    pub season: String,
    pub season_type: String,
    pub league_id: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftPick {
    pub round: i32,
    pub roster_id: Option<i32>,
    pub player_id: String,
    pub picked_by: String,
    pub pick_no: i32,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// One future-pick ownership record, as returned by
/// `/league/{id}/traded_picks` — live-verified (2026-11-11) to already
/// collapse a pick's full trade chain into its current holder: `roster_id`
/// is the pick's *original* owner (what identifies which pick this is),
/// `owner_id` is who holds it now, `previous_owner_id` is whoever last held
/// it (not necessarily the original owner). Only picks that have moved at
/// least once appear here at all — an untraded pick has no row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradedPick {
    pub season: String,
    pub round: i32,
    pub roster_id: i32,
    pub previous_owner_id: i32,
    pub owner_id: i32,
}

// ---------------------------------------------------------------------------
// Player info (from the big /players/nfl endpoint)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlayerInfo {
    pub full_name: Option<String>,
    pub position: Option<String>,
    pub team: Option<String>,
    pub status: Option<String>,
    pub injury_status: Option<String>,
}

// ---------------------------------------------------------------------------
// News item (RotowWire / ESPN)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsItem {
    pub player_name: String,
    pub headline: String,
    pub body: String,
    pub date: String,
    pub source: String,
}

// ---------------------------------------------------------------------------
// Serde helper: deserialize null JSON value as an empty Vec.
// ---------------------------------------------------------------------------

fn null_as_empty<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let opt: Option<Vec<T>> = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

fn null_as_empty_map<'de, D, K, V>(
    deserializer: D,
) -> std::result::Result<HashMap<K, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    K: Deserialize<'de> + std::hash::Hash + Eq,
    V: Deserialize<'de>,
{
    let opt: Option<HashMap<K, V>> = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}
