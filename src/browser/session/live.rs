//! The transport-crate implementation of the `BrowserSession` seam.
//!
//! This is the ONLY module that owns transport types. No child-process
//! spawning here (layer gate: spawning lives in `launch.rs` only).

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

    fn eval_string<'a>(&'a self, js: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let v: String =
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

    /// Sets the value via the native input-value-setter (React tracks the
    /// element's own setter, so a plain `.value =` is invisible to it) then
    /// dispatches `input`+`change`. Live-verified against `.player-search`
    /// and the FAAB bid field, both React-controlled inputs.
    fn type_into_selector<'a>(&'a self, selector: &'a str, text: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let sel = serde_json::to_string(selector).map_err(other)?;
            let val = serde_json::to_string(text).map_err(other)?;
            let js = format!(
                "(function(){{ const el = document.querySelector({sel}); if (!el) {{ throw new Error('no element matches selector'); }} \
                 const proto = el.tagName === 'TEXTAREA' ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype; \
                 const setter = Object.getOwnPropertyDescriptor(proto, 'value').set; \
                 setter.call(el, {val}); \
                 el.dispatchEvent(new Event('input', {{ bubbles: true }})); \
                 el.dispatchEvent(new Event('change', {{ bubbles: true }})); \
                 return true; }})()"
            );
            let ok: bool =
                self.page.evaluate(js.as_str()).await.map_err(other)?.into_value().map_err(other)?;
            if !ok {
                return Err(other("type primitive reported failure"));
            }
            Ok(())
        })
    }

    /// Finds the first element matching `row_selector` whose `textContent`
    /// contains `needle`, then clicks `target_selector` within it (or the
    /// row itself when `target_selector` is empty). Live-verified need: the
    /// add/claim submit button and lineup swap rows have no id/class/testid
    /// (or the name text lives outside the clickable link) — CSS alone
    /// cannot select them (plan.md Story 3.1/4.1).
    fn click_in_row<'a>(
        &'a self,
        row_selector: &'a str,
        needle: &'a str,
        target_selector: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let row_sel = serde_json::to_string(row_selector).map_err(other)?;
            let needle_json = serde_json::to_string(needle).map_err(other)?;
            let target_sel = serde_json::to_string(target_selector).map_err(other)?;
            let js = format!(
                "(function(){{ const needle = {needle_json}; const targetSel = {target_sel}; \
                 const row = Array.from(document.querySelectorAll({row_sel})) \
                     .find(e => e.textContent && e.textContent.includes(needle)); \
                 if (!row) {{ throw new Error('no row with matching text found'); }} \
                 const target = targetSel ? row.querySelector(targetSel) : row; \
                 if (!target) {{ throw new Error('no target element within matching row'); }} \
                 target.click(); return true; }})()"
            );
            let ok: bool =
                self.page.evaluate(js.as_str()).await.map_err(other)?.into_value().map_err(other)?;
            if !ok {
                return Err(other("click_in_row primitive reported failure"));
            }
            Ok(())
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
