# ADR-001: Keep chromiumoxide 0.9.1 as CDP Transport with Nodriver-Style Launch

**Status:** Accepted
**Date:** 2026-09-08
**Context:** `src/browser.rs` drives stock Chrome via `chromiumoxide::Browser::launch` (0.9.1). Automation fingerprints persist. Alternatives: fantoccini/thirtyfour/webdriver (new driver binary + `cdc_` patch surface), stale stealth crates (`undetected-chromedriver` 0.1.2, `chromiumoxide_stealth`), separate Python backend.
**Decision:** Keep chromiumoxide 0.9.1 as the CDP transport only. Launch real Chrome ourselves (`StealthLauncher`) with stealth flags and attach via `Browser::connect` (nodriver pattern). No chromedriver binary, so no `cdc_` block to patch. Stealth applied via `Emulation.setUserAgentOverride` (coherent client hints) + `Page.createIsolatedWorld` bootstrap + `Page.addScriptToEvaluateOnNewDocument`. Isolate all chromiumoxide use behind `BrowserSession`/`BrowserBackend` seam (`src/browser/`).
**Consequences:** Minimal new deps; known risk carried: chromiumoxide 0.9.1 WS spam vs Chrome 146 version skew → mitigated by CfT pinning (ADR-002) + `browser doctor` diagnostics + timeboxed chaser-oxide spike (Epic 1). Removal/upgrade of chromiumoxide later requires only seam + `session.rs` change.
**Policy note:** Engine-level launch/CDP hardening only. No interaction-faking; login stays 100% manual.
