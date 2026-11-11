//! In-memory seam double: proves CLI paths compile and behave with no Chrome.
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::browser::{other, BoxFuture, BrowserSession};
use crate::error::Result;

pub struct FakeSession {
    logged_in: bool,
    transport_error: bool,
    split_signals: bool,
    eval_calls: AtomicUsize,
    dump: tokio::sync::Mutex<String>,
    /// Canned `eval_string` responses, consumed in order across successive
    /// calls within one flow (e.g. a candidate-row read followed by a
    /// distinct post-click confirmation read). A call past the end of the
    /// list repeats the last entry rather than erroring, so existing
    /// single-response fixtures (flows that call `eval_string` more than
    /// once) keep their prior behavior unchanged.
    eval_string_responses: tokio::sync::Mutex<Vec<String>>,
    eval_string_call_count: AtomicUsize,
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
            eval_string_responses: tokio::sync::Mutex::new(vec![r#"{"data":{}}"#.to_string()]),
            eval_string_call_count: AtomicUsize::new(0),
            closed: false,
        }
    }

    /// Canned response for every [`BrowserSession::eval_string`] call —
    /// used to fixture the GraphQL `league_transactions_filtered` reply in
    /// tests without a live Chrome.
    pub fn with_eval_string_response(json: &str) -> Self {
        Self::with_eval_string_responses(&[json])
    }

    /// Canned responses consumed in order across successive `eval_string`
    /// calls within one flow (Task 1.4.1b: the claims-panel read needs a
    /// different canned reply than the post-click confirmation read). The
    /// last entry repeats for any call beyond the list's length.
    pub fn with_eval_string_responses(jsons: &[&str]) -> Self {
        let s = Self::logged_in();
        *s.eval_string_responses
            .try_lock()
            .expect("fresh fixture, no contention") = jsons.iter().map(|j| j.to_string()).collect();
        s
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

    fn eval_string<'a>(&'a self, _js: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            if self.transport_error {
                return Err(other("fake transport error"));
            }
            let responses = self.eval_string_responses.lock().await;
            let idx = self.eval_string_call_count.fetch_add(1, Ordering::SeqCst);
            Ok(responses
                .get(idx)
                .or_else(|| responses.last())
                .cloned()
                .unwrap_or_default())
        })
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

    fn type_into_selector<'a>(&'a self, _selector: &'a str, _text: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }

    fn click_in_row<'a>(
        &'a self,
        _row_selector: &'a str,
        _needle: &'a str,
        _target_selector: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::super::debug::debug_browser_line;
    use super::FakeSession;
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
