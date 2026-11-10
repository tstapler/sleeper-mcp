//! `BrowserSession` seam: the only interface `cli.rs` programs against.
//!
//! Transport types (`Browser`, `Page`) are confined to `session.rs` (impl)
//! and `launch.rs` owns the child process. Nothing outside `src/browser/`
//! imports the CDP transport crate — enforced by
//! `seam_should_expose_only_dyn_browser_session_when_chromiumoxide_hidden`.
//!
//! Async without `async-trait`: trait methods return boxed futures over
//! concrete types, which keeps the trait object-safe so callers hold
//! `Box<dyn BrowserSession>`. `close` takes `&mut self` (by-value `close`
//! is not object-safe).

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::error::{Error, Result};

pub mod cft;
pub mod launch;
pub mod session;
pub mod stealth;

pub use cft::{check_skew, RuntimeVersion};
pub use launch::{LaunchConfig, LaunchMode, LaunchedBrowser, StealthLauncher};
pub use session::{FakeSession, SleeperBrowser};
pub use stealth::{ClientHints, PinnedVersion, StealthFlag, PINNED_CHROME_VERSION};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub(crate) fn other<E: std::fmt::Display>(e: E) -> Error {
    Error::Other(anyhow::anyhow!(e.to_string()))
}

/// Tri-state login verdict. `Unknown` never guesses (Epic 2 wires exit 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginVerdict {
    LoggedIn,
    LoggedOut,
    Unknown,
}

/// Single-selector probe (baseline semantics, kept as ONE of the three
/// multi-signal checks in Story 2.3 — Epic-1 behavior preserved verbatim).
pub const LOGIN_PROBE_JS: &str = r#"!Array.from(document.querySelectorAll('button'))
    .some(b => b.offsetParent !== null && b.textContent.trim().toLowerCase() === 'log in')"#;

/// Session-marker probe: real persisted-auth localStorage keys.
/// Root-cause fix (2026-11-10): the prior check (`document.cookie.match(/session/i)`
/// or `localStorage.getItem('sleeper_session')`) was an unverified Story 2.3
/// placeholder. Live inspection of an authenticated profile's storage (dumped
/// key list + presence checks via read-only CDP `Runtime.evaluate`, never
/// the raw token value) found no `sleeper_session` key and no cookie whose
/// NAME contains "session" — the cookie branch's one observed `true` was a
/// coincidental substring match inside an unrelated analytics cookie's
/// opaque value, not a real signal. Sleeper's actual client auth keys are
/// `token`/`user_id` in `localStorage`, confirmed present (347-char token)
/// on a freshly relaunched Chrome pointed at the same profile, i.e. they
/// really do survive a full browser restart.
pub const LOGIN_SESSION_MARKER_JS: &str =
    r#"!!(localStorage.getItem('token') || localStorage.getItem('user_id'))"#;

/// No-signup-CTA probe: complements [`LOGIN_PROBE_JS`]'s "no visible Log In
/// button" with "no visible Sign Up button" — both auth CTAs show together
/// when logged out, neither when logged in (verified live). Root-cause fix
/// (2026-11-10): the prior selector (`nav [data-avatar]`, `.nav-profile-item`,
/// etc.) chased DOM elements that only exist on `/leagues/*` sub-pages, never
/// on the bare `https://sleeper.com` root that `goto` always lands on before
/// this probe runs — so it read `false` even fully authenticated, and the
/// unanimous-agreement verdict never reached `LoggedIn`.
pub const LOGIN_NO_SIGNUP_CTA_JS: &str = r#"!Array.from(document.querySelectorAll('button'))
    .some(b => b.offsetParent !== null && b.textContent.trim().toLowerCase() === 'sign up')"#;

/// Probe bundle read in ONE transport call via `eval_batch` (CDP hygiene).
/// Conjunction: LoggedIn only when ALL THREE agree; any split → Unknown.
pub const LOGIN_PROBES: &[&str] = &[
    LOGIN_PROBE_JS,
    LOGIN_SESSION_MARKER_JS,
    LOGIN_NO_SIGNUP_CTA_JS,
];

/// Pure verdict over one batch of signal reads: unanimous-true → LoggedIn,
/// unanimous-false → LoggedOut, anything else (split/empty) → Unknown.
/// Never guesses.
pub fn verdict_from_signals(signals: &[bool]) -> LoginVerdict {
    if signals.is_empty() {
        return LoginVerdict::Unknown;
    }
    if signals.iter().all(|s| *s) {
        LoginVerdict::LoggedIn
    } else if signals.iter().all(|s| !*s) {
        LoginVerdict::LoggedOut
    } else {
        LoginVerdict::Unknown
    }
}

/// Object-safe browser seam. Implemented by `SleeperBrowser` (chromiumoxide)
/// and `FakeSession` (tests). Story 3.1 adds only small GENERIC selector
/// primitives (`click_selector`/`wait_for_selector`, gated on Q5) — flow
/// logic lives in flow modules, never as per-page trait methods.
pub trait BrowserSession: Send + Sync {
    fn goto<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<()>>;
    fn eval_bool<'a>(&'a self, js: &'a str) -> BoxFuture<'a, Result<bool>>;
    fn eval_dump<'a>(&'a self) -> BoxFuture<'a, Result<String>>;
    /// Generic click primitive (Story 3.1 Task 3): clicks the element
    /// matching the caller-supplied `selector`. No selectors are hardcoded
    /// here or in impls — Q5 forbids guessing selectors in code.
    fn click_selector<'a>(&'a self, selector: &'a str) -> BoxFuture<'a, Result<()>>;
    /// Generic wait primitive: resolves once `selector` matches, `Err` on
    /// timeout. Takes the selector as an argument; never hardcodes one.
    fn wait_for_selector<'a>(&'a self, selector: &'a str, timeout_ms: u64)
        -> BoxFuture<'a, Result<()>>;
    /// Generic type primitive (Story 3.2): sets the value of the element
    /// matching `selector` via the native property setter and dispatches
    /// `input`+`change` so React-controlled inputs (live-verified: the
    /// player search box and the FAAB bid field) pick up the change. No
    /// selector or text is hardcoded here.
    fn type_into_selector<'a>(&'a self, selector: &'a str, text: &'a str) -> BoxFuture<'a, Result<()>>;
    /// Generic scoped-click primitive (Story 3.2/4.1): finds the first
    /// element matching `row_selector` whose `textContent` contains
    /// `needle`, then clicks the element matching `target_selector` inside
    /// it (or the row itself when `target_selector` is empty). Covers every
    /// live-verified click CSS alone can't express (plan.md Story 3.1/4.1):
    /// an attribute-less submit button (`row_selector="button"`, empty
    /// target), a drop-candidate row that is itself the clickable link
    /// (`row_selector="a.team-roster-item"`, empty target), and a lineup
    /// row whose name text lives outside its clickable link
    /// (`row_selector=".team-roster-item"`, `target_selector="a.cell-position"`).
    /// No row/target selector or needle text is hardcoded here.
    fn click_in_row<'a>(
        &'a self,
        row_selector: &'a str,
        needle: &'a str,
        target_selector: &'a str,
    ) -> BoxFuture<'a, Result<()>>;
    /// Coalesced read: ONE transport call for N scripts (hygiene gate:
    /// `eval_batch_should_coalesce_polls_when_hygiene_enabled`).
    fn eval_batch<'a>(&'a self, scripts: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<bool>>>;
    fn close(&mut self) -> BoxFuture<'_, Result<()>>;

    /// Provided: batched single-call verdict (no per-signal round-trips).
    /// Navigates to the base URL first — correct for a one-shot status
    /// check, but never call this from a poll loop sharing a tab with an
    /// active human: use [`is_logged_in_here`](Self::is_logged_in_here)
    /// there instead.
    fn is_logged_in<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            self.goto("https://sleeper.com").await?;
            self.is_logged_in_here().await
        })
    }

    /// Provided: same three-signal verdict as [`is_logged_in`](Self::is_logged_in),
    /// without the navigation. Root-cause fix (2026-11-10): the manual-login
    /// poll in `cmd_browser_login` called `is_logged_in()` every 5s for up
    /// to 10 minutes against the SAME tab the human was actively driving —
    /// each tick's `goto("https://sleeper.com")` forced a full reload back
    /// to the home page mid-flow (mid-CAPTCHA, mid-passkey, wherever the
    /// user currently was), reported as "it keeps jumping out of login".
    /// Live passive-CDP capture confirmed the account was actually
    /// authenticating (an account-scoped `graphql` response came back
    /// mid-poll) — the repeated reload was the only thing disrupting it.
    fn is_logged_in_here<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let verdicts = self.eval_batch(LOGIN_PROBES).await?;
            Ok(verdict_from_signals(&verdicts) == LoginVerdict::LoggedIn)
        })
    }

    /// Provided: tri-state verdict with bounded hydration (Story 2.3 AC1).
    /// Polls every `250ms` up to a `5s` deadline (headed and headless share
    /// the bound); returns `Unknown` on signal split/timeout instead of
    /// guessing. Transport errors propagate as `Err` (caller prints the
    /// distinct `Login check failed` copy). Per-signal fired/missed lines go
    /// to stderr; stdout stays verdict-only.
    fn login_verdict<'a>(&'a self) -> BoxFuture<'a, Result<LoginVerdict>> {
        Box::pin(async move {
            self.goto("https://sleeper.com").await?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                let verdicts = self.eval_batch(LOGIN_PROBES).await?;
                let verdict = verdict_from_signals(&verdicts);
                if verdict != LoginVerdict::Unknown
                    || tokio::time::Instant::now() >= deadline
                {
                    for (idx, fired) in verdicts.iter().enumerate() {
                        eprintln!(
                            "login signal {}: {}",
                            idx,
                            if *fired { "fired" } else { "missed" }
                        );
                    }
                    // Observability boundary (plan.md Layer 3): verdict string
                    // + per-signal booleans only — never cookie/token values.
                    tracing::info!(verdict = ?verdict, signals = ?verdicts, "login.verdict");
                    return Ok(verdict);
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        })
    }

    /// Provided: manual-login poll. Human types credentials and solves any
    /// CAPTCHA by hand in the visible window; nothing here automates it.
    /// Expiry directs to re-run `sleeper browser login` (ux.md Surface F) —
    /// never `status`, which is a read-only check.
    fn wait_for_manual_login<'a>(&'a self, timeout_secs: u64) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Observability boundary (plan.md Layer 3): one span + start /
            // timeout events; per-tick progress stays on the spinner/stderr.
            let poll_span = tracing::info_span!("login.poll", timeout_s = timeout_secs);
            let _poll_enter = poll_span.enter();
            tracing::info!("login.poll start");
            if self.is_logged_in().await? {
                return Ok(());
            }
            self.goto("https://sleeper.com").await?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
            while tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if self.is_logged_in().await? {
                    return Ok(());
                }
            }
            tracing::info!("login.poll timeout");
            Err(other(format!(
                "still not logged in after {timeout_secs}s — finish logging in in the browser window, then re-run `sleeper browser login`"
            )))
        })
    }

    /// Provided: `SLEEPER_BROWSER_DEBUG` page dump to stderr.
    fn debug_dump<'a>(&'a self) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.goto("https://sleeper.com").await?;
            let dump = self.eval_dump().await?;
            eprintln!("DEBUG dump: {dump}");
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::session::FakeSession;
    use super::BrowserSession;

    #[test]
    fn seam_should_expose_only_dyn_browser_session_when_chromiumoxide_hidden() {
        // Object-safety: a fake must satisfy the seam as a trait object with
        // no transport types involved.
        let mut boxed: Box<dyn BrowserSession> = Box::new(FakeSession::logged_in());
        let _ = boxed.as_mut();
        // Layer gate, compile-coupled: no file under cli/ may import the transport.
        // NOTE: this file's own test name contains the bare crate word, so
        // the self-scan targets the import path (with `::`) specifically.
        //
        // `include_str!` needs a literal path per file (no glob), so cli.rs's
        // split into src/cli/ means every file under it must be listed here by
        // hand — add new ones to this list, or this gate silently stops
        // covering them.
        let cli = [
            include_str!("../cli/mod.rs"),
            include_str!("../cli/browser/mod.rs"),
            include_str!("../cli/browser/login.rs"),
            include_str!("../cli/browser/status.rs"),
            include_str!("../cli/browser/doctor.rs"),
            include_str!("../cli/browser/actions.rs"),
            include_str!("../cli/sync.rs"),
            include_str!("../cli/roster_cmds.rs"),
            include_str!("../cli/dynasty_cmds.rs"),
        ]
        .concat();
        assert!(
            !cli.contains("chromiumoxide"),
            "no file under src/cli/ may reference the transport; program against Box<dyn BrowserSession>"
        );
        let seam = include_str!("mod.rs");
        // Built dynamically so this gate cannot match its own source text.
        let import_path = format!("{}::", "chromiumoxide");
        assert!(
            !seam.contains(&import_path),
            "browser/mod.rs must not import the transport; impl lives in session.rs"
        );
    }

    #[test]
    fn session_should_expose_primitives_only_when_claim_gated() {
        // R22 happy path: the seam exposes only small generic primitives;
        // flow logic (claim/lineup/trade) lives in flow modules, not as
        // per-page trait methods — this held before Q5 resolved and still
        // holds now that Story 3.2 is implemented on top of these.
        let seam = include_str!("mod.rs");
        for primitive in [
            "click_selector",
            "wait_for_selector",
            "type_into_selector",
            "click_in_row",
        ] {
            assert!(
                seam.contains(primitive),
                "seam must expose generic primitive {primitive} (Story 3.1/3.2)"
            );
        }
        // Banned flow methods, built dynamically so this gate cannot match
        // its own source text.
        let banned = [["goto", "league"], ["claim", "player"]]
            .map(|parts| parts.join("_"));
        for method in &banned {
            assert!(
                !seam.contains(method),
                "seam must not expose flow method {method} as a trait method"
            );
        }
    }
}
