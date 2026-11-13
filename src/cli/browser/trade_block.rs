//! Trade Block read path (discovery, 2026-11-11): Sleeper has no dedicated
//! trade-block query. Captured the real GraphQL traffic the `/trades` page
//! fires and found it's `settings.otb`/`settings.otb_added_at` on the
//! existing `league_players` query (`league_players_via_seam`,
//! `src/browser/session/flows.rs`) — see that function's doc comment for
//! the full discovery. Unlike `league_transactions_filtered`, this query
//! carries no `player_map`, and Sleeper's own API does not filter stale
//! flags (a player since traded/dropped keeps a leftover `otb` value) — both
//! resolved here against already-public REST data, no second authenticated
//! fetch needed.
use std::collections::HashMap;

use crate::analysis::AnalysisService;
use crate::browser::session::league_players_via_seam;
use crate::config::Config;
use crate::error::Result;

use super::login::{debug_detail, launch_failure_error, launch_session, triage_error, LaunchOptions};
use super::pending::graphql_error_message;

/// One player currently on the trade block, fully resolved.
#[derive(Debug, PartialEq, serde::Serialize)]
pub(crate) struct TradeBlockPlayer {
    pub(crate) player_id: String,
    pub(crate) name: String,
    pub(crate) position: String,
    pub(crate) team: String,
    pub(crate) roster_id: i64,
    pub(crate) owner_name: String,
    pub(crate) flagged_at: String,
}

/// Pure, no I/O: parses the raw `league_players` GraphQL response into
/// `(player_id, otb_roster_id, otb_added_at_ms)` triples, keeping only rows
/// that actually carry an `otb` flag at all. Does NOT cross-reference
/// current ownership — that needs the (separately fetched) roster list, so
/// it stays a caller concern, same split as `parse_pending_transactions`
/// leaving name resolution to its own `player_map`.
pub(crate) fn parse_otb_rows(raw: &str) -> Result<Vec<(String, i64, i64)>> {
    let v: serde_json::Value = serde_json::from_str(raw).map_err(crate::browser::other)?;
    if let Some(msg) = graphql_error_message(&v) {
        return Err(crate::browser::other(format!("league_players query failed: {msg}")));
    }
    let entries = v.pointer("/data/league_players").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    Ok(entries
        .into_iter()
        .filter_map(|e| {
            let player_id = e.get("player_id")?.as_str()?.to_string();
            let otb_roster = e["settings"]["otb"].as_i64()?;
            let added_at = e["settings"]["otb_added_at"].as_i64().unwrap_or(0);
            Some((player_id, otb_roster, added_at))
        })
        .collect())
}

/// Launches a headless session, fetches raw `league_players`, and parses
/// the otb rows — the one piece that genuinely needs the authenticated
/// browser seam. Ownership/name/owner resolution happens in
/// `run_trade_block`, against already-public REST data.
async fn fetch_otb_rows(config: &Config, league_id: &str) -> Result<Vec<(String, i64, i64)>> {
    let (mut session, launched) = match launch_session(config, LaunchOptions::headless()).await {
        Ok(pair) => pair,
        Err(e) => return Err(launch_failure_error(&e)),
    };
    let outcome = league_players_via_seam(&*session, league_id).await;
    session.close().await?;
    launched.close().await?;
    match outcome {
        Ok(raw) => parse_otb_rows(&raw),
        Err(e) => Err(crate::browser::other(triage_error(
            "could not read the trade block — the live `/trades` page's GraphQL shape may have changed",
            &debug_detail(&e.to_string()),
        ))),
    }
}

/// Full trade-block read: authenticated otb-flagged rows, cross-referenced
/// against current roster ownership (dropping stale flags — live-verified
/// 2026-11-11: 4 of 21 raw flags in this exact league were stale) and
/// resolved to real names/positions/teams/owners via already-fetched public
/// REST data. Sorted most-recently-flagged first.
pub(crate) async fn run_trade_block(
    config: &Config,
    analysis: &AnalysisService,
    league_id: &str,
) -> Result<Vec<TradeBlockPlayer>> {
    let raw_rows = fetch_otb_rows(config, league_id).await?;

    let rosters = analysis.client.get_league_rosters(league_id).await?;
    let users = analysis.client.get_league_users(league_id).await?;
    let all_players = analysis.client.get_all_players("nfl").await?;

    let owner_of: HashMap<&str, i64> =
        rosters.iter().flat_map(|r| r.players.iter().map(move |pid| (pid.as_str(), r.roster_id as i64))).collect();
    let roster_owner_user: HashMap<i64, &str> = rosters
        .iter()
        .filter_map(|r| Some((r.roster_id as i64, r.owner_id.as_deref()?)))
        .collect();
    let display_name_of: HashMap<&str, &str> =
        users.iter().map(|u| (u.user_id.as_str(), u.display_name.as_str())).collect();

    let mut out: Vec<TradeBlockPlayer> = raw_rows
        .into_iter()
        .filter(|(pid, otb_roster, _)| owner_of.get(pid.as_str()) == Some(otb_roster))
        .map(|(pid, roster_id, added_at_ms)| {
            let p = all_players.get(&pid);
            let name = p.and_then(|p| p.get("full_name")).and_then(|v| v.as_str()).unwrap_or(&pid).to_string();
            let position = p.and_then(|p| p.get("position")).and_then(|v| v.as_str()).unwrap_or("?").to_string();
            let team = p.and_then(|p| p.get("team")).and_then(|v| v.as_str()).unwrap_or("FA").to_string();
            let owner_name = roster_owner_user
                .get(&roster_id)
                .and_then(|uid| display_name_of.get(uid))
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("Roster {roster_id}"));
            let flagged_at = chrono::DateTime::from_timestamp_millis(added_at_ms)
                .map(|t| t.to_rfc3339())
                .unwrap_or_default();
            TradeBlockPlayer { player_id: pid, name, position, team, roster_id, owner_name, flagged_at }
        })
        .collect();
    out.sort_by(|a, b| b.flagged_at.cmp(&a.flagged_at));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_otb_rows_should_only_keep_rows_that_carry_an_otb_flag() {
        let raw = r#"{"data":{"league_players":[
            {"player_id":"1","settings":{"otb":5,"otb_added_at":1000}},
            {"player_id":"2","settings":{"waiver_clears_at":999}},
            {"player_id":"3","settings":{}}
        ]}}"#;
        let rows = parse_otb_rows(raw).expect("must parse");
        assert_eq!(rows, vec![("1".to_string(), 5, 1000)]);
    }

    #[test]
    fn parse_otb_rows_should_surface_graphql_error_when_unauthorized() {
        let raw = r#"{"errors":[{"message":"Unauthorized"}]}"#;
        let err = parse_otb_rows(raw).expect_err("must error");
        assert!(err.to_string().contains("Unauthorized"));
    }

    #[test]
    fn parse_otb_rows_should_return_empty_when_no_player_carries_a_flag() {
        let raw = r#"{"data":{"league_players":[{"player_id":"1","settings":{}}]}}"#;
        assert_eq!(parse_otb_rows(raw).expect("must parse"), vec![]);
    }

    /// Operator-gated live run: real browser + real league. Live-verified
    /// 2026-11-11 against this exact league -- 21 raw otb flags, 4 stale
    /// (cross-referenced against current ownership and correctly dropped),
    /// 17 real entries matching the live `/trades` page's own rendering
    /// exactly. Re-run by hand with `cargo test --lib -- --ignored
    /// trade_block_live --nocapture` whenever this flow needs re-checking
    /// against a live session.
    #[tokio::test]
    #[ignore = "operator-gated: needs live Chrome + authenticated profile"]
    async fn trade_block_live_should_return_real_entries_from_a_real_league() {
        let config = Config::default();
        let client = crate::client::SleeperClient::new(&config).await.expect("client must build");
        let db = crate::db::StatsDb::open(&config.stats_db_path()).await.expect("db must open");
        let analysis = AnalysisService::new(client, db);

        let players = run_trade_block(&config, &analysis, "1352883872131481600")
            .await
            .expect("trade block must resolve against a live, logged-in session");
        println!("{} players on the trade block", players.len());
        for p in &players {
            println!("{:?}", p);
        }
        assert!(!players.is_empty(), "this league's real trade block is not empty");
    }
}
