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
        Ok(()) => Ok(()),
        Err(e) if e.to_string() == DEAD_SESSION_HINT => Err(e),
        Err(e) => Err(crate::browser::other(triage_error(
            &claim_failed_triage(),
            &debug_detail(&e.to_string()),
        ))),
    }
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
        Ok(()) => Ok(()),
        Err(e) if e.to_string() == DEAD_SESSION_HINT => Err(e),
        Err(e) => Err(crate::browser::other(triage_error(
            &lineup_failed_triage(),
            &debug_detail(&e.to_string()),
        ))),
    }
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
