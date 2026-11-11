//! `BrowserSession` implementations and the DOM-action flows built on them.
//!
//! - `live.rs` — the real, chromiumoxide-backed session (the ONLY file that
//!   owns transport types).
//! - `fake.rs` — the in-memory test double.
//! - `flows.rs` — waiver-claim/lineup-set/trade-propose, built on the seam's
//!   generic primitives.
//! - `debug.rs` — secret redaction, the debug context line, and stealth
//!   health parsing.
mod debug;
mod fake;
mod flows;
mod live;

pub use debug::{debug_browser_line, redact_secrets, stealth_health, DEAD_SESSION_HINT};
pub use fake::FakeSession;
pub use flows::{
    claim_player_via_seam, edit_cancel_claim_via_seam, lineup_set_via_seam,
    pending_transactions_via_seam, trade_propose_via_seam, EditCancelAction, EditCancelOutcome,
};
pub use live::SleeperBrowser;
