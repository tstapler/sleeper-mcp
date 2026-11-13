//! Waiver-claim/lineup-set/trade-propose DOM actions: launch through the
//! seam, resolve player_ids to display names (the live DOM has none to
//! select on), run the live-verified flow, and report triage on failure.
//!
//! `run_claim`/`run_lineup_set` are the reusable cores (no printing, no
//! process exit) shared by the CLI commands below and by the `waiver_claim`/
//! `lineup_set` MCP tools (`src/mcp.rs`) — the only two write actions live
//! enough to expose there; trade stays CLI-only since it never leaves the
//! disabled stub (Epic 4 cut, Q5-equivalent never resolved).
use console::style;

use crate::client::SleeperClient;
use crate::config::Config;
use crate::error::Result;
use crate::models::Roster;

use super::super::WaiverAction;
use super::login::{
    debug_detail, exit_on_action_error, launch_failure_error, launch_failure_exit, launch_session,
    triage_error, LaunchOptions,
};

/// Claim-flow failure triage (Story 3.2, live automation): stderr-only,
/// never a stdout verdict. Raw detail (selector/timeout specifics) rides
/// behind `SLEEPER_BROWSER_DEBUG` via `debug_detail`.
fn claim_failed_triage() -> String {
    "Waiver claim failed — the player may already be claimed, the roster may be full, or the live UI may have changed. Re-run with SLEEPER_BROWSER_DEBUG=1 for detail.".to_string()
}

/// Lineup-flow failure triage (Story 4.1, live automation): stderr-only,
/// never a stdout verdict. Raw detail rides behind `SLEEPER_BROWSER_DEBUG`.
fn lineup_failed_triage() -> String {
    "Lineup swap failed — the player may not be eligible for that slot, or the live UI may have changed. Re-run with SLEEPER_BROWSER_DEBUG=1 for detail.".to_string()
}

/// Epic 4 stub triage (cut per appetite rule): stderr-only, never stdout.
fn trade_disabled_triage() -> String {
    "selectors live-unverified — trade automation disabled (Epic 4 deferred per appetite cut rule; see plan.md). Live-verify the trade selector bundle first, then re-run `sleeper trade-propose`.".to_string()
}

pub(crate) async fn cmd_waiver(config: &Config, client: &SleeperClient, action: WaiverAction) -> Result<()> {
    match action {
        WaiverAction::Claim { league, player, drop, bid } => {
            let req = ClaimRequest { league: &league, player_id: &player, drop_id: drop.as_deref(), bid };
            cmd_waiver_claim(config, client, req).await
        }
        WaiverAction::Pending { league, roster } => {
            super::pending::cmd_pending_waivers(config, &league, roster).await
        }
        // Epic 3.2: intercepted in `cli::run` before dispatch reaches this
        // function — bid-projection is pure AnalysisService analysis, not a
        // browser action, and this function only has `client`, not `svc`.
        WaiverAction::BidProjection { .. } => {
            unreachable!("BidProjection is handled in cli::run before reaching cmd_waiver")
        }
        // Epic 4.3: also intercepted in `cli::run` before dispatch reaches
        // this function, the same reason as `BidProjection` above —
        // `run_edit_cancel_claim` needs `AnalysisService`, which this
        // function only has as `client`, not `svc`.
        WaiverAction::Edit { .. } | WaiverAction::Cancel { .. } => {
            unreachable!("Edit/Cancel are handled in cli::run before reaching cmd_waiver")
        }
    }
}

/// Params for [`run_claim`] — grouped rather than positional since callers
/// (the CLI command and the `waiver_claim` MCP tool) each assemble these
/// from a different source shape (a `WaiverAction::Claim` vs a deserialized
/// tool-params struct).
pub(crate) struct ClaimRequest<'a> {
    pub(crate) league: &'a str,
    pub(crate) player_id: &'a str,
    pub(crate) drop_id: Option<&'a str>,
    pub(crate) bid: u32,
}

/// Story 3.2 core: launches through the seam, resolves player_id(s) to
/// display names, calls the live-verified seam-side claim flow, and closes
/// the browser on every branch — no orphan, and no printing/process exit
/// (shared by the CLI command below and the `waiver_claim` MCP tool).
/// `Err`'s `Display` is already the full triage line callers should surface
/// verbatim (dead-session hint, or the claim-failed triage with debug detail).
pub(crate) async fn run_claim(
    config: &Config,
    client: &SleeperClient,
    req: ClaimRequest<'_>,
) -> Result<()> {
    use crate::browser::session::{claim_player_via_seam, DEAD_SESSION_HINT};
    let player_name = crate::cli::sync::resolve_player_name(client, req.player_id).await?;
    let drop_name = match req.drop_id {
        Some(id) => Some(crate::cli::sync::resolve_player_name(client, id).await?),
        None => None,
    };
    let (mut session, launched) = match launch_session(config, LaunchOptions::headless()).await {
        Ok(pair) => pair,
        Err(e) => return Err(launch_failure_error(&e)),
    };
    let outcome =
        claim_player_via_seam(&*session, req.league, &player_name, req.bid, drop_name.as_deref())
            .await;
    session.close().await?;
    launched.close().await?;
    match outcome {
        Ok(on_waivers) => {
            // A pending waiver claim hasn't moved anyone yet — nothing to
            // patch (and the pending-claims view is read live via the
            // browser seam each time anyway, so it's never cache-stale in
            // the first place). Only an instant free-agent add/drop is a
            // confirmed roster mutation worth reflecting early.
            if !on_waivers {
                apply_instant_add_patch(client, req.league, req.player_id, req.drop_id).await;
            }
            Ok(())
        }
        Err(e) if e.to_string() == DEAD_SESSION_HINT => Err(e),
        Err(e) => Err(crate::browser::other(triage_error(
            &claim_failed_triage(),
            &debug_detail(&e.to_string()),
        ))),
    }
}

/// Optimistic roster-cache patch for an instant (non-waiver) free-agent
/// add/drop. Anchors on `drop_id` — the one player we can be certain is
/// already in the cached roster data — to find which roster is "mine"
/// without needing a roster_id param this call site never had. A pure add
/// with no drop has no such anchor (the added player isn't on any cached
/// roster yet), so that case intentionally no-ops rather than guess which
/// of 12 rosters is the right one; it falls back to the existing
/// force-fresh/TTL-expiry path like before this feature existed.
async fn apply_instant_add_patch(
    client: &SleeperClient,
    league: &str,
    player_id: &str,
    drop_id: Option<&str>,
) {
    let Some(drop_id) = drop_id else { return };
    let player_id = player_id.to_string();
    let drop_id = drop_id.to_string();
    let result = client
        .patch_cached_rosters(league, move |rosters| apply_drop_add(rosters, &player_id, &drop_id))
        .await;
    if let Err(e) = result {
        tracing::debug!(error = %e, "optimistic_roster_patch_failed");
    }
}

/// Pure mutation for [`apply_instant_add_patch`]: finds the roster carrying
/// `drop_id` and moves it out, `player_id` in. No-ops if `drop_id` isn't on
/// any cached roster (the anchor this patch depends on is missing).
fn apply_drop_add(rosters: &mut [Roster], player_id: &str, drop_id: &str) {
    let Some(roster) = rosters.iter_mut().find(|r| r.players.iter().any(|p| p == drop_id)) else {
        return;
    };
    roster.players.retain(|p| p != drop_id);
    roster.starters.retain(|p| p != drop_id);
    roster.reserve.retain(|p| p != drop_id);
    if let Some(taxi) = roster.taxi.as_mut() {
        taxi.retain(|p| p != drop_id);
    }
    roster.players.push(player_id.to_string());
}

/// CLI wrapper: prints `claim submitted` on success; dead sessions get the
/// yellow Surface C hint (ux.md §3), everything else prints red and exits 1.
async fn cmd_waiver_claim(config: &Config, client: &SleeperClient, req: ClaimRequest<'_>) -> Result<()> {
    match run_claim(config, client, req).await {
        Ok(()) => {
            println!("claim submitted");
            Ok(())
        }
        Err(e) => exit_on_action_error(e),
    }
}

/// Story 4.1 core: launches through the seam, resolves both player IDs to
/// display names, calls the live lineup-swap flow, and closes the browser on
/// every branch — no orphan, and no printing/process exit (shared by the
/// CLI command below and the `lineup_set` MCP tool). `Err`'s `Display` is
/// already the full triage line callers should surface verbatim.
pub(crate) async fn run_lineup_set(
    config: &Config,
    client: &SleeperClient,
    league: &str,
    out_player_id: &str,
    in_player_id: &str,
) -> Result<()> {
    use crate::browser::session::{lineup_set_via_seam, DEAD_SESSION_HINT};
    let out_name = crate::cli::sync::resolve_player_name(client, out_player_id).await?;
    let in_name = crate::cli::sync::resolve_player_name(client, in_player_id).await?;
    let (mut session, launched) = match launch_session(config, LaunchOptions::headless()).await {
        Ok(pair) => pair,
        Err(e) => return Err(launch_failure_error(&e)),
    };
    let outcome = lineup_set_via_seam(&*session, league, &out_name, &in_name).await;
    session.close().await?;
    launched.close().await?;
    match outcome {
        Ok(()) => {
            apply_lineup_swap_patch(client, league, out_player_id, in_player_id).await;
            Ok(())
        }
        Err(e) if e.to_string() == DEAD_SESSION_HINT => Err(e),
        Err(e) => Err(crate::browser::other(triage_error(
            &lineup_failed_triage(),
            &debug_detail(&e.to_string()),
        ))),
    }
}

/// Optimistic roster-cache patch for a confirmed starter<->bench swap:
/// `lineup_set_via_seam` already live-verified `in_player_id` landed in a
/// real (non-bench) slot before returning `Ok`, so swapping the two ids at
/// `out_player_id`'s existing `starters` index is exact, not a guess.
///
/// Deliberately doesn't attempt IR/Taxi moves: those swap a player into
/// `reserve`/`taxi`, not `starters`, and this function has no reliable way
/// to tell which one without re-deriving the live DOM slot label — if
/// `out_player_id` isn't found in any roster's `starters` (the IR/Taxi
/// case), the mutation below is a no-op and the existing cache-TTL/
/// force-fresh behavior applies, same as before this feature existed.
async fn apply_lineup_swap_patch(client: &SleeperClient, league: &str, out_id: &str, in_id: &str) {
    let out_id = out_id.to_string();
    let in_id = in_id.to_string();
    let result = client
        .patch_cached_rosters(league, move |rosters| apply_starter_swap(rosters, &out_id, &in_id))
        .await;
    if let Err(e) = result {
        tracing::debug!(error = %e, "optimistic_roster_patch_failed");
    }
}

/// Pure mutation for [`apply_lineup_swap_patch`]: replaces `out_id` with
/// `in_id` at its existing index in whichever roster's `starters` carries
/// it. No-ops if `out_id` isn't in any roster's `starters` — the IR/Taxi
/// case this patch deliberately doesn't attempt (see that fn's doc comment).
fn apply_starter_swap(rosters: &mut [Roster], out_id: &str, in_id: &str) {
    let Some(roster) = rosters.iter_mut().find(|r| r.starters.iter().any(|p| p == out_id)) else {
        return;
    };
    let idx = roster.starters.iter().position(|p| p == out_id).expect("just matched");
    roster.starters[idx] = in_id.to_string();
}

/// CLI wrapper: prints `lineup saved` on success; dead sessions get the
/// yellow Surface C hint (ux.md §3), everything else prints red and exits 1.
pub(crate) async fn cmd_lineup_set(
    config: &Config,
    client: &SleeperClient,
    league: &str,
    out: &str,
    in_: &str,
) -> Result<()> {
    match run_lineup_set(config, client, league, out, in_).await {
        Ok(()) => {
            println!("lineup saved");
            Ok(())
        }
        Err(e) => exit_on_action_error(e),
    }
}

/// Epic 4 stub wiring (cut per appetite rule): same launch → gated-flow →
/// triage → close discipline as the claim stub. Never attempts automation.
pub(crate) async fn cmd_trade_propose(
    config: &Config,
    league: &str,
    give: &str,
    get: &str,
) -> Result<()> {
    use crate::browser::session::{trade_propose_via_seam, DEAD_SESSION_HINT};
    let (mut session, launched) = match launch_session(config, LaunchOptions::headless()).await {
        Ok(pair) => pair,
        Err(e) => launch_failure_exit(&e),
    };
    let outcome = trade_propose_via_seam(&*session, league, give, get).await;
    session.close().await?;
    launched.close().await?;
    match outcome {
        Ok(()) => Ok(()), // unreachable while Epic 4 is cut (stub always Errs)
        Err(e) if e.to_string() == DEAD_SESSION_HINT => {
            eprintln!("{}", style(DEAD_SESSION_HINT).yellow());
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("{}", style(trade_disabled_triage()).red());
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_roster(roster_id: i32, players: &[&str], starters: &[&str]) -> Roster {
        Roster {
            roster_id,
            owner_id: None,
            league_id: "league1".to_string(),
            players: players.iter().map(|s| s.to_string()).collect(),
            starters: starters.iter().map(|s| s.to_string()).collect(),
            reserve: Vec::new(),
            taxi: None,
            settings: Default::default(),
        }
    }

    #[test]
    fn apply_drop_add_should_move_drop_id_out_and_player_id_in_when_drop_id_is_rostered() {
        let mut rosters = vec![
            fixture_roster(1, &["100", "200"], &["100"]),
            fixture_roster(2, &["300"], &["300"]),
        ];
        apply_drop_add(&mut rosters, "900", "200");
        assert_eq!(rosters[0].players, vec!["100".to_string(), "900".to_string()]);
        assert_eq!(rosters[0].starters, vec!["100".to_string()]);
        assert_eq!(rosters[1].players, vec!["300".to_string()]);
    }

    #[test]
    fn apply_drop_add_should_remove_drop_id_from_starters_when_it_was_starting() {
        let mut rosters = vec![fixture_roster(1, &["100"], &["100"])];
        apply_drop_add(&mut rosters, "900", "100");
        assert!(rosters[0].starters.is_empty());
        assert_eq!(rosters[0].players, vec!["900".to_string()]);
    }

    #[test]
    fn apply_drop_add_should_no_op_when_drop_id_is_not_on_any_cached_roster() {
        let mut rosters = vec![fixture_roster(1, &["100"], &["100"])];
        apply_drop_add(&mut rosters, "900", "999");
        assert_eq!(rosters[0].players, vec!["100".to_string()]);
    }

    #[test]
    fn apply_starter_swap_should_replace_out_id_at_its_existing_index_when_out_id_is_starting() {
        let mut rosters = vec![fixture_roster(1, &["100", "200"], &["100", "200"])];
        apply_starter_swap(&mut rosters, "200", "300");
        assert_eq!(rosters[0].starters, vec!["100".to_string(), "300".to_string()]);
    }

    #[test]
    fn apply_starter_swap_should_no_op_when_out_id_is_not_a_starter_the_ir_taxi_case() {
        let mut rosters = vec![fixture_roster(1, &["100", "200"], &["100"])];
        apply_starter_swap(&mut rosters, "200", "300");
        assert_eq!(rosters[0].starters, vec!["100".to_string()]);
    }
}
