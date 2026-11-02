//! Live-verified DOM-action flows (waiver claim, lineup swap, trade-propose
//! stub) — built entirely on the generic seam primitives from `mod.rs`
//! (`click_selector`/`wait_for_selector`/`type_into_selector`/`click_in_row`),
//! never on transport types.
use crate::browser::{other, BrowserSession};
use crate::error::Result;

use super::debug::DEAD_SESSION_HINT;

/// Selector bundle live-verified against real sleeper.com rows (plan.md
/// Story 3.1, 2026-09-11) — never guessed. The player list is
/// `react-virtualized` (off-screen rows aren't in the DOM at all), so the
/// search box is the only reliable way to get a row mounted before touching
/// it.
mod claim_selectors {
    pub const PLAYER_SEARCH_INPUT: &str = ".player-search input";
    pub const PLAYER_LIST_ITEM: &str = ".player-list-item";
    pub const ACTION_BUTTON: &str = ".player-list-item a.player-action-button";
    pub const ON_WAIVERS_PROBE_JS: &str =
        "!!document.querySelector('.player-list-item a.player-action-button.waiver')";
    pub const MODAL_ROOT: &str = ".add-player-panel";
    pub const DROP_ROW: &str = ".add-player-panel a.team-roster-item";
    pub const BID_INPUT: &str = ".add-player-panel .bid-input-container input";
    pub const SUBMIT_BUTTON: &str = ".add-player-panel button";
    pub const SUBMIT_WAIVER_TEXT: &str = "Make Waiver Bid";
    pub const SUBMIT_ADD_TEXT: &str = "Add Player";
}

/// Selector bundle for the Team/lineup page, spot-checked 2026-09-11
/// (plan.md Epic 4 lineup mechanism spot-check) then fully verified before
/// this flow was written: clicking a starter's position link enters an
/// in-place swap-select mode (no modal) — the clicked starter and every
/// eligible bench player get marked `.selected`/`.valid` respectively, an
/// ineligible bench player gets `.invalid`. Clicking a `.valid` row's link
/// completes the swap immediately — there is no separate save/confirm step.
mod lineup_selectors {
    pub const ROSTER_ROW: &str = ".team-roster-item";
    pub const ROW_LINK: &str = "a.cell-position";
    pub const VALID_TARGET_ROW: &str = ".team-roster-item.valid";
}

/// Story 3.2 seam-side claim flow, live-verified 2026-09-11 (plan.md Epic 3,
/// Story 3.1 selector bundle). `player_name`/`drop_player_name` are the
/// resolved Sleeper display names (callers resolve player_id -> full_name
/// before crossing the seam — the DOM has no player_id to select on).
/// `bid_amount` is ignored on the free-agent (non-waiver) path, since that
/// modal has no bid field.
pub async fn claim_player_via_seam(
    session: &dyn BrowserSession,
    league_id: &str,
    player_name: &str,
    bid_amount: u32,
    drop_player_name: Option<&str>,
) -> Result<()> {
    use claim_selectors::*;

    let start = tracing::info_span!("claim.start", league = league_id, player = player_name);
    let _enter = start.enter();
    if !session.is_logged_in().await.unwrap_or(false) {
        return Err(other(DEAD_SESSION_HINT));
    }

    session.goto(&format!("https://sleeper.com/leagues/{league_id}/players")).await?;
    session.wait_for_selector(PLAYER_SEARCH_INPUT, 10_000).await?;
    session.type_into_selector(PLAYER_SEARCH_INPUT, player_name).await?;
    session.wait_for_selector(PLAYER_LIST_ITEM, 5_000).await?;

    let on_waivers = session.eval_bool(ON_WAIVERS_PROBE_JS).await?;
    session.click_selector(ACTION_BUTTON).await?;
    session.wait_for_selector(MODAL_ROOT, 5_000).await?;

    if let Some(drop_name) = drop_player_name {
        session.click_in_row(DROP_ROW, drop_name, "").await?;
    }

    let confirm = tracing::info_span!("claim.confirm", drop = ?drop_player_name, bid = bid_amount, on_waivers);
    let _enter_confirm = confirm.enter();
    if on_waivers {
        session.type_into_selector(BID_INPUT, &bid_amount.to_string()).await?;
        session.click_in_row(SUBMIT_BUTTON, SUBMIT_WAIVER_TEXT, "").await?;
    } else {
        session.click_in_row(SUBMIT_BUTTON, SUBMIT_ADD_TEXT, "").await?;
    }
    tracing::info!("claim submitted");
    Ok(())
}

/// Story 4.1 seam-side lineup-swap flow, live-verified 2026-09-11 (plan.md
/// Epic 4 lineup mechanism spot-check). `out_player_name`/`in_player_name`
/// are resolved Sleeper display names (callers resolve player_id ->
/// full_name before crossing the seam, same as the claim flow). There is no
/// confirm step on the live UI — clicking the bench replacement's row
/// completes the swap immediately.
pub async fn lineup_set_via_seam(
    session: &dyn BrowserSession,
    league_id: &str,
    out_player_name: &str,
    in_player_name: &str,
) -> Result<()> {
    use lineup_selectors::*;

    let start = tracing::info_span!(
        "lineup.start",
        league = league_id,
        out = out_player_name,
        r#in = in_player_name
    );
    let _enter = start.enter();
    if !session.is_logged_in().await.unwrap_or(false) {
        return Err(other(DEAD_SESSION_HINT));
    }

    session.goto(&format!("https://sleeper.com/leagues/{league_id}/team")).await?;
    session.wait_for_selector(ROSTER_ROW, 10_000).await?;
    // Enter select mode: click the currently-started player's position link.
    session.click_in_row(ROSTER_ROW, out_player_name, ROW_LINK).await?;
    // Only an eligible bench row carries `.valid` — an ineligible in_player
    // simply won't match here, failing closed instead of clicking the wrong row.
    session.wait_for_selector(VALID_TARGET_ROW, 5_000).await?;

    let confirm = tracing::info_span!("lineup.confirm", out = out_player_name, r#in = in_player_name);
    let _enter_confirm = confirm.enter();
    session.click_in_row(VALID_TARGET_ROW, in_player_name, ROW_LINK).await?;
    tracing::info!("lineup saved");
    Ok(())
}

/// Story 4.x seam-side stub — STUB ONLY (Epic 4 cut per appetite rule;
/// selectors live-unverified). Through-seam signature, no automation.
pub async fn trade_propose_via_seam(
    session: &dyn BrowserSession,
    league_id: &str,
    give: &str,
    get: &str,
) -> Result<()> {
    let start = tracing::info_span!("trade.start", league = league_id, give = give, get = get);
    let _enter = start.enter();
    tracing::info!("trade gated: Epic 4 cut, no automation attempted");
    if !session.is_logged_in().await.unwrap_or(false) {
        return Err(other(DEAD_SESSION_HINT));
    }
    Err(other(
        "selectors live-unverified — trade automation disabled (Epic 4 deferred; see plan.md cut note)",
    ))
}
