//! Browser-automation command surface: the login flow plus shared
//! launch/triage plumbing (`login.rs`), the `status` render built on that
//! plumbing (`status.rs`), the `doctor` self-test (`doctor.rs`, also built
//! on it), and the waiver-claim/lineup-set/trade-propose DOM actions
//! (`actions.rs`).
use crate::config::Config;
use crate::error::Result;

use super::BrowserAction;

mod actions;
mod doctor;
pub mod login;
pub mod status;

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

/// Crate-wide (not just `cli`-wide): the `waiver_claim`/`lineup_set` MCP
/// tools in `src/mcp.rs` call these shared cores directly, bypassing the
/// CLI's printing/process-exit wrapper.
pub(crate) use actions::{run_claim, run_lineup_set, ClaimRequest};
