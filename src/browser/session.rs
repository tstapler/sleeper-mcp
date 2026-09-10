//! The transport-crate implementation of the `BrowserSession` seam.
//!
//! This is the ONLY module that owns transport types. No child-process
//! spawning here (layer gate: spawning lives in `launch.rs` only).

use std::sync::atomic::{AtomicUsize, Ordering};

use chromiumoxide::{Browser, Page};
use futures::StreamExt;

use crate::browser::stealth::{isolated_world_bootstrap_js, ClientHints, PinnedVersion};
use crate::browser::{other, BoxFuture, BrowserSession};
use crate::error::Result;

pub struct SleeperBrowser {
    browser: Browser,
    _handler: tokio::task::JoinHandle<()>,
    page: Page,
}

impl SleeperBrowser {
    /// Attach to an already-running Chrome (self-launched by
    /// `StealthLauncher::spawn`) and apply the stealth bootstrap.
    /// The stock launch constructor is intentionally absent: every launch
    /// goes through `launch.rs` so flags, endpoint parsing, and teardown
    /// stay central.
    pub async fn connect(endpoint: &str) -> Result<Self> {
        let (browser, mut handler) =
            Browser::connect(endpoint.to_string()).await.map_err(other)?;
        let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });
        let page = browser.new_page("https://sleeper.com").await.map_err(other)?;
        let this = Self { browser, _handler: handler_task, page };
        this.bootstrap_stealth().await?;
        Ok(this)
    }

    /// Post-connect stealth bootstrap (Story 1.3 Task 5). Best-effort note:
    /// v1 applies the evaluate-subset of `isolated_world_bootstrap_js`
    /// immediately; the full CDP sequence (isolated world +
    /// `addScriptToEvaluateOnNewDocument` + UA override) lands with CfT
    /// pinning in Epic 2.
    async fn bootstrap_stealth(&self) -> Result<()> {
        let js = isolated_world_bootstrap_js(&ClientHints::for_version(PinnedVersion::pinned()));
        let ok: bool =
            self.page.evaluate(js.as_str()).await.map_err(other)?.into_value().map_err(other)?;
        if !ok {
            return Err(other("stealth bootstrap probe failed after connect"));
        }
        Ok(())
    }

    pub fn page(&self) -> &Page {
        &self.page
    }
}

impl BrowserSession for SleeperBrowser {
    fn goto<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.page.goto(url).await.map_err(other)?;
            Ok(())
        })
    }

    fn eval_bool<'a>(&'a self, js: &'a str) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let v: bool =
                self.page.evaluate(js).await.map_err(other)?.into_value().map_err(other)?;
            Ok(v)
        })
    }

    fn eval_dump<'a>(&'a self) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let dump: String = self
                .page
                .evaluate(
                    r#"JSON.stringify({
                        url: location.href,
                        title: document.title,
                        buttons: Array.from(document.querySelectorAll('button')).map(b => ({
                            text: b.textContent.trim(),
                            visible: b.offsetParent !== null,
                        })),
                        ua: navigator.userAgent,
                        webdriver: navigator.webdriver,
                        languages: navigator.languages,
                        plugins: navigator.plugins.length,
                    })"#,
                )
                .await
                .map_err(other)?
                .into_value()
                .map_err(other)?;
            Ok(dump)
        })
    }

    /// CDP-minimal hygiene: N signal reads collapse into ONE `evaluate`
    /// round-trip. BEFORE (baseline `is_logged_in` per poll tick): goto +
    /// `wait_for_navigation` + 500ms sleep + 1 evaluate ≈ 3 transport
    /// round-trips per tick (STATIC estimate — live WS-event counts
    /// PENDING-OPERATOR, Story 1.4). AFTER: goto + 1 batched evaluate ≈ 2
    /// round-trips per tick, and multi-signal reads never multiply the
    /// evaluate count. Debugger attach stays deferred: nothing here calls
    /// `Runtime.enable` explicitly.
    fn eval_batch<'a>(&'a self, scripts: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<bool>>> {
        Box::pin(async move {
            let fns: Vec<String> =
                scripts.iter().map(|s| format!("(function(){{ return !!({s}); }})")).collect();
            let wrapped =
                format!("JSON.stringify(([{fns}]).map(f => {{ try {{ return f(); }} catch(e) {{ return false; }} }}))", fns = fns.join(","));
            let json: String =
                self.page.evaluate(wrapped.as_str()).await.map_err(other)?.into_value().map_err(other)?;
            let verdicts: Vec<bool> = serde_json::from_str(&json).map_err(other)?;
            Ok(verdicts)
        })
    }

    /// Generic click over the caller-supplied selector only. The selector
    /// is JSON-escaped into `querySelector`; no selector is hardcoded here
    /// (Q5 gate: never guess selectors in code).
    fn click_selector<'a>(&'a self, selector: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let sel = serde_json::to_string(selector).map_err(other)?;
            let js = format!(
                "(function(){{ const el = document.querySelector({sel}); if (!el) {{ throw new Error('no element matches selector'); }} el.click(); return true; }})()"
            );
            let ok: bool =
                self.page.evaluate(js.as_str()).await.map_err(other)?.into_value().map_err(other)?;
            if !ok {
                return Err(other("click primitive reported failure"));
            }
            Ok(())
        })
    }

    /// Generic wait over the caller-supplied selector: polls every `100ms`
    /// until match or `timeout_ms`, then `Err`. No hardcoded selectors.
    fn wait_for_selector<'a>(
        &'a self,
        selector: &'a str,
        timeout_ms: u64,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let sel = serde_json::to_string(selector).map_err(other)?;
            let js = format!("!!document.querySelector({sel})");
            let deadline =
                tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
            loop {
                if self.eval_bool(&js).await? {
                    return Ok(());
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(other(format!("selector not found within {timeout_ms}ms")));
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.browser.close().await.map_err(other)?;
            self.browser.wait().await.map_err(other)?;
            Ok(())
        })
    }
}

/// Redact session-shaped secrets from debug output: `key=value` pairs for
/// cookie/token/auth-adjacent keys become `key=[redacted]`, and
/// `Bearer <token>` becomes `Bearer [redacted]`. Debug dumps must never
/// leak cookies, tokens, or credential-adjacent values.
pub fn redact_secrets(s: &str) -> String {
    const KEYS: &[&str] = &[
        "session", "token", "cookie", "auth", "password", "secret", "credential",
    ];
    let mut out = s.to_string();
    for key in KEYS {
        for sep in ["=", ": "] {
            let needle = format!("{key}{sep}");
            let mut search_from = 0;
            loop {
                let hay = out.to_lowercase();
                let Some(rel) = hay[search_from..].find(&needle) else {
                    break;
                };
                let start = search_from + rel + needle.len();
                let bytes = out.as_bytes();
                let mut end = start;
                while end < bytes.len()
                    && !bytes[end].is_ascii_whitespace()
                    && bytes[end] != b','
                    && bytes[end] != b';'
                    && bytes[end] != b'"'
                    && bytes[end] != b'}'
                {
                    end += 1;
                }
                if end > start {
                    out.replace_range(start..end, "[redacted]");
                    search_from = start + "[redacted]".len();
                } else {
                    search_from = start;
                }
            }
        }
    }
    // Bearer tokens without a key prefix.
    let mut search_from = 0;
    loop {
        let hay = out.to_lowercase();
        let Some(rel) = hay[search_from..].find("bearer ") else {
            break;
        };
        let start = search_from + rel + "bearer ".len();
        let bytes = out.as_bytes();
        let mut end = start;
        while end < bytes.len()
            && !bytes[end].is_ascii_whitespace()
            && bytes[end] != b','
            && bytes[end] != b'"'
        {
            end += 1;
        }
        if end > start {
            out.replace_range(start..end, "[redacted]");
            search_from = start + "[redacted]".len();
        } else {
            search_from = start;
        }
    }
    out
}

/// Pure `DEBUG browser:` context line (ux.md Surface G): pinned/runtime
/// chrome, flag-set hash, CDP port, profile path, cleanup/reaped PIDs.
/// Callers print it to stderr with secrets already redacted.
pub fn debug_browser_line(
    pinned: &str,
    runtime: &str,
    flag_set_hash: &str,
    cdp_port: &str,
    profile: &std::path::Path,
    reaped_pids: &[u32],
) -> String {
    let pids = if reaped_pids.is_empty() {
        "0".to_string()
    } else {
        reaped_pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",")
    };
    format!(
        "DEBUG browser: chrome pinned v{pinned}, runtime v{runtime}, flag set {flag_set_hash}, cdp port {cdp_port}, profile {}, cleanup: reaped pid {pids}",
        profile.display()
    )
}

/// Stealth-signal health over a page dump (seam-level parse, beside
/// `eval_dump`/`redact_secrets`): returns a short signal summary plus whether
/// the webdriver marker came back clean (`false`/`undefined`). Callers only
/// render the result; parsing never lives in the CLI.
pub fn stealth_health(dump: &str) -> (String, bool) {
    let clean =
        dump.contains("webdriver") && (dump.contains("false") || dump.contains("undefined"));
    let summary = if clean {
        "webdriver: false".to_string()
    } else {
        "webdriver: missing/tampered".to_string()
    };
    (summary, clean)
}

/// In-memory seam double: proves CLI paths compile and behave with no Chrome.
pub struct FakeSession {
    logged_in: bool,
    transport_error: bool,
    split_signals: bool,
    eval_calls: AtomicUsize,
    dump: tokio::sync::Mutex<String>,
    closed: bool,
}

impl FakeSession {
    pub fn logged_in() -> Self {
        Self {
            logged_in: true,
            transport_error: false,
            split_signals: false,
            eval_calls: AtomicUsize::new(0),
            dump: tokio::sync::Mutex::new(
                r#"{"url":"https://sleeper.com/","title":"Sleeper","webdriver: false","ua: Chrome/126","languages: en-US","plugins: 5"}"#.to_string(),
            ),
            closed: false,
        }
    }

    pub fn logged_out() -> Self {
        let mut s = Self::logged_in();
        s.logged_in = false;
        s
    }

    pub fn with_transport_error() -> Self {
        let mut s = Self::logged_in();
        s.transport_error = true;
        s
    }

    /// Split-signal double: the three login signals disagree (one misses),
    /// so the tri-state verdict must be `Unknown`, never a guess.
    pub fn with_signal_split() -> Self {
        let mut s = Self::logged_in();
        s.split_signals = true;
        s
    }

    /// Number of transport calls issued (hygiene counter).
    pub fn eval_calls(&self) -> usize {
        self.eval_calls.load(Ordering::SeqCst)
    }
}

impl BrowserSession for FakeSession {
    fn goto<'a>(&'a self, _url: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }

    fn eval_bool<'a>(&'a self, _js: &'a str) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            if self.transport_error {
                return Err(other("fake transport error"));
            }
            self.eval_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.logged_in)
        })
    }

    fn eval_dump<'a>(&'a self) -> BoxFuture<'a, Result<String>> {
        // tokio Mutex has no poison: lock() is infallible, so no expect() and
        // no panic path (this double ships in non-cfg(test) code).
        Box::pin(async move { Ok(self.dump.lock().await.clone()) })
    }

    fn eval_batch<'a>(&'a self, scripts: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<bool>>> {
        Box::pin(async move {
            // Hygiene contract: exactly ONE transport call for N reads, and a
            // transport failure surfaces immediately instead of swallowing.
            if self.transport_error {
                return Err(other("fake transport error"));
            }
            self.eval_calls.fetch_add(1, Ordering::SeqCst);
            if self.split_signals {
                // Disagreeing signals: two fire, one misses.
                const SPLIT: &[bool] = &[true, false, true];
                return Ok(scripts
                    .iter()
                    .enumerate()
                    .map(|(i, _)| SPLIT[i % SPLIT.len()])
                    .collect());
            }
            Ok(vec![self.logged_in; scripts.len()])
        })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if self.closed {
                return Err(other("browser already closed"));
            }
            self.closed = true;
            Ok(())
        })
    }

    fn click_selector<'a>(&'a self, _selector: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }

    fn wait_for_selector<'a>(
        &'a self,
        _selector: &'a str,
        _timeout_ms: u64,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Dead-session hint (ux.md §3 downstream surfacing): flow commands that
/// detect a dead session surface this yellow Surface C hint themselves
/// rather than dumping CDP errors.
pub const DEAD_SESSION_HINT: &str = "Not logged in — run `sleeper browser login`.";

/// Story 3.2 seam-side flow plumbing — STUB ONLY while Q5 is OPEN.
/// Through-seam signature (`&dyn BrowserSession`); the body performs NO
/// selector use and returns a `Q5-unverified` error. Tracing spans
/// `claim.start` / `claim.confirm` mark the future automation boundaries.
pub async fn claim_player_via_seam(
    session: &dyn BrowserSession,
    league_id: &str,
    player_id: &str,
    drop_player_id: Option<&str>,
) -> Result<()> {
    let start = tracing::info_span!("claim.start", league = league_id, player = player_id);
    let _enter = start.enter();
    tracing::info!("claim gated: Q5 OPEN, no automation attempted");
    if !session.is_logged_in().await.unwrap_or(false) {
        return Err(other(DEAD_SESSION_HINT));
    }
    let confirm = tracing::info_span!("claim.confirm", drop = ?drop_player_id);
    let _enter_confirm = confirm.enter();
    Err(other(
        "Q5 not yet verified — claim automation disabled (no live-verified selectors; see plan.md Epic 3)",
    ))
}

/// Story 4.x seam-side stub — STUB ONLY (Epic 4 cut per appetite rule;
/// selectors live-unverified). Through-seam signature, no automation.
pub async fn lineup_set_via_seam(
    session: &dyn BrowserSession,
    league_id: &str,
    slot: &str,
    player_id: &str,
) -> Result<()> {
    let start =
        tracing::info_span!("lineup.start", league = league_id, slot = slot, player = player_id);
    let _enter = start.enter();
    tracing::info!("lineup gated: Epic 4 cut, no automation attempted");
    if !session.is_logged_in().await.unwrap_or(false) {
        return Err(other(DEAD_SESSION_HINT));
    }
    Err(other(
        "selectors live-unverified — lineup automation disabled (Epic 4 deferred; see plan.md cut note)",
    ))
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

#[cfg(test)]
mod tests {
    use super::{debug_browser_line, redact_secrets, FakeSession};
    use crate::browser::{verdict_from_signals, BrowserSession, LoginVerdict};

    #[tokio::test]
    async fn is_logged_in_should_return_logged_in_when_all_signals_agree() {
        let session = FakeSession::logged_in();
        let verdict = session.login_verdict().await.expect("verdict");
        assert_eq!(verdict, LoginVerdict::LoggedIn);
        // Pure conjunction agrees: unanimous-true is LoggedIn.
        assert_eq!(
            verdict_from_signals(&[true, true, true]),
            LoginVerdict::LoggedIn
        );
        assert_eq!(
            verdict_from_signals(&[false, false, false]),
            LoginVerdict::LoggedOut
        );
        // Bool seam stays consistent with the tri-state verdict.
        assert!(session.is_logged_in().await.expect("bool seam"));
    }

    #[tokio::test]
    async fn is_logged_in_should_return_unknown_when_signals_split_or_timeout() {
        let session = FakeSession::with_signal_split();
        // Split signals poll to the 5s deadline, then Unknown — never a guess.
        let verdict = session.login_verdict().await.expect("verdict");
        assert_eq!(verdict, LoginVerdict::Unknown);
        assert_eq!(
            verdict_from_signals(&[true, false, true]),
            LoginVerdict::Unknown
        );
        assert_eq!(verdict_from_signals(&[]), LoginVerdict::Unknown);
    }

    #[tokio::test]
    async fn debug_dump_should_include_stealth_signals_when_env_set() {
        let session = FakeSession::logged_in();
        let dump = session.eval_dump().await.expect("dump");
        for marker in ["webdriver:", "ua:", "languages:", "plugins:"] {
            assert!(
                dump.contains(marker),
                "debug dump must carry stealth signal {marker}, got {dump}"
            );
        }
        // stdout stays verdict-only: the dump itself is stderr material and
        // carries no verdict line.
        assert!(!dump.contains("Logged in."));
        let line = debug_browser_line(
            "126.0.6478.126",
            "126.0.6478.126",
            "a1b2c3d4",
            "41233",
            std::path::Path::new("/tmp/chrome-profile"),
            &[],
        );
        assert!(line.contains("DEBUG browser:"), "got {line}");
        assert!(line.contains("flag set a1b2c3d4"), "got {line}");
    }

    #[test]
    fn debug_dump_should_redact_when_session_values_present() {
        let raw = "cookies: session=abc123; auth: token=xyz789, Bearer sekrit-token headers";
        let clean = redact_secrets(raw);
        for leaked in ["abc123", "xyz789", "sekrit-token"] {
            assert!(
                !clean.contains(leaked),
                "redactor must strip session values, got {clean}"
            );
        }
        assert!(clean.contains("[redacted]"), "got {clean}");
    }

    #[tokio::test]
    async fn fake_session_should_satisfy_seam_contract_when_no_chrome_installed() {
        let mut session: Box<dyn BrowserSession> = Box::new(FakeSession::logged_in());
        session.goto("https://sleeper.com").await.expect("goto");
        assert!(session.eval_bool("true").await.expect("eval_bool"));
        let dump = session.eval_dump().await.expect("eval_dump");
        assert!(dump.contains("webdriver"), "dump carries stealth signals");
        let batch = session.eval_batch(&["true", "true", "true"]).await.expect("eval_batch");
        assert_eq!(batch, vec![true, true, true]);
        assert!(session.is_logged_in().await.expect("is_logged_in"));
        session.close().await.expect("close");
    }

    #[tokio::test]
    async fn close_should_return_error_when_called_twice_on_fake_session() {
        let mut session = FakeSession::logged_in();
        session.close().await.expect("first close ok");
        let err = session.close().await.expect_err("second close must Err, not panic");
        assert!(err.to_string().contains("already closed"));
    }

    #[tokio::test]
    async fn eval_batch_should_coalesce_polls_when_hygiene_enabled() {
        let session = FakeSession::logged_in();
        assert_eq!(session.eval_calls(), 0);
        let verdicts =
            session.eval_batch(&["true", "true", "true"]).await.expect("batch");
        assert_eq!(verdicts.len(), 3);
        assert_eq!(
            session.eval_calls(),
            1,
            "hygiene: 1 transport call for N signal reads, not N calls"
        );
    }

    #[tokio::test]
    async fn poll_should_stop_batching_when_transport_errors() {
        let session = FakeSession::with_transport_error();
        let err = session.eval_batch(&["true", "true"]).await.expect_err("batch must surface Err");
        assert!(err.to_string().contains("transport"));
        // Higher-level poll must propagate, never swallow into a false verdict.
        session.is_logged_in().await.expect_err("is_logged_in must propagate transport Err");
    }
}
