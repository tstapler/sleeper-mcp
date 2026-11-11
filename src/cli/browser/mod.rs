//! Browser-automation command surface: the login flow plus shared
//! launch/triage plumbing (`login.rs`), the `status` render built on that
//! plumbing (`status.rs`), the `doctor` self-test (`doctor.rs`, also built
//! on it), the waiver-claim/lineup-set/trade-propose DOM actions
//! (`actions.rs`), and the edit/cancel pre-write-recheck core
//! (`edit_cancel.rs`, Epic 4.1 — split out of `actions.rs` per plan.md Task
//! 4.1.1a once that file would exceed its file-size budget).
use crate::config::Config;
use crate::error::Result;

use super::BrowserAction;

mod actions;
mod doctor;
mod edit_cancel;
pub mod login;
mod pending;
pub mod status;
pub mod transactions;

pub(super) async fn cmd_browser(config: &Config, action: BrowserAction) -> Result<()> {
    match action {
        BrowserAction::Login { system_chrome, chrome_binary } => {
            login::cmd_browser_login(config, system_chrome, chrome_binary).await
        }
        BrowserAction::Status { headed, json, debug, system_chrome, chrome_binary } => {
            let opts = status::StatusOptions { headed, json, debug, system_chrome, chrome_binary };
            status::cmd_browser_status(config, opts).await
        }
        BrowserAction::Doctor => doctor::cmd_browser_doctor(config).await,
    }
}

pub(super) use actions::{cmd_lineup_set, cmd_trade_propose, cmd_waiver};

/// Crate-wide (not just `cli`-wide): the `waiver_claim`/`lineup_set`/
/// `get_pending_waivers` MCP tools in `src/mcp.rs` call these shared cores
/// directly, bypassing the CLI's printing/process-exit wrapper.
pub(crate) use actions::{run_claim, run_lineup_set, ClaimRequest};
/// `run_edit_cancel_claim`/`EditCancelRunOutcome` (Epic 4.1) plus Epic 4.3's
/// shared renderer (`render_edit_cancel_outcome`, called by both the CLI
/// dispatch below and the `edit_waiver_claim`/`cancel_waiver_claim` MCP
/// tools) and CLI wrapper (`cmd_waiver_edit_cancel`, called from
/// `cli::run`'s `WaiverAction::Edit`/`Cancel` interception — it needs
/// `AnalysisService`, which `cmd_waiver` above doesn't have).
/// `EditCancelRunOutcome` itself is only referenced from `mcp.rs`'s
/// `#[cfg(test)]` module (to construct fixtures for
/// `render_edit_cancel_outcome`) — real callers only ever see the `String`
/// `render_edit_cancel_outcome` returns — hence the explicit allow in a
/// non-test build.
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use edit_cancel::{
    cmd_waiver_edit_cancel, render_edit_cancel_outcome, run_edit_cancel_claim, EditCancelRunOutcome,
};
pub(crate) use pending::run_pending_transactions;
