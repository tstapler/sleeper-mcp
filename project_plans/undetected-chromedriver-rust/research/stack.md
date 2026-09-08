# Stack Research: undetected-chromedriver-rust

Date: 2026-09-08. Baseline: `chromiumoxide 0.9.1` + stock Chrome, profile at
`Config::chrome_profile_dir()` (`src/config.rs:40-44`), launch in
`src/browser.rs:30-42`, manual-login poll `src/browser.rs:84-100`.

## (a) What Python undetected-chromedriver actually does

Upstream: `ultrafunkamsterdam/undetected-chromedriver` (~12.8k stars, GPL-3.0;
PyPI v3.5.5 is py3.8–3.11-only due to distutils; py3.12+ must install from git
master). Official successor is **`nodriver`** (same author, ~4.7k stars,
AGPL-3.0): drops Selenium + chromedriver entirely, drives Chrome over raw CDP
async. 2026 community consensus (dataresearchtools comparison): UC = legacy /
Selenium-compat choice; nodriver = lightweight Python CDP middle ground;
Patchright (patched Playwright fork) = strongest on hard targets. For this
repo (in-Rust only, no new service), the portable lesson is nodriver's, not
UC's: **remove the driver binary, launch real Chrome with clean flags, talk
CDP directly**.

Enumerated patch targets (from `patcher.py`, `__init__.py`, and secondary
writeups; generations per decodo/2026-01-06 guide):

1. **Binary `cdc_` patch (Gen-1 API signals).** `patcher.py` scans the
   downloaded chromedriver binary for the `cdc_` / `$cdc_` injection-code
   block (the JS variables chromedriver injects into every page, e.g.
   `cdc_adoQpoasnfa76pfcZLmcfl_*`, matched by regex
   `.+_.+_(Array|Promise|Symbol)`) and replaces bytes with same-length
   padding so page-side scans find no match. Done on-the-fly at init, not
   shipped pre-patched.
2. **Launch-flag stripping.** Removes `--enable-automation` and the
   `AutomationControlled` blink feature before Chrome starts; sets a normal
   UA and ordinary profile prefs so the session resembles a regular browse.
3. **Driver auto-management.** Detects installed Chrome major version,
   downloads the matching chromedriver (pre-M115 via
   `chromedriver.storage.googleapis.com`, post-M115 via Chrome-for-Testing
   endpoints), caches under `~/.undetected_chromedriver`.
4. **CDP script injection (`Page.addScriptToEvaluateOnNewDocument`).**
   `Chrome.execute_cdp_cmd` / `click_safe()` paths inject JS on new
   documents (hide `navigator.webdriver`, clean up leaked vars). Known Gen-2
   weakness: `Runtime.enable`-style CDP calls are themselves detectable;
   UC does not solve this.
5. **selenium-stealth-style JS evasions (the layer UC only partly covers).**
   Reference list: `navigator.webdriver=false`, `chrome`/`cdc_` runtime
   cleanup, plugins/languages/permissions spoofing, WebGL vendor/renderer,
   `navigator.platform`/`hardwareConcurrency`/`deviceMemory` consistency.
6. **Explicit non-goals / limits.** Does not hide IP (needs residential
   proxy); headless officially "unsupported / WIP, will break randomly";
   prefers official `google-chrome` binary over Chromium builds; Gen-3
   (TLS fingerprint, behavior/ML) untouched. Maintainer has throttled the
   issue tracker (Discussions only).

Implication for the port: replicate targets 1–2 by **elimination, not
patching** — no chromedriver binary at all means no `cdc_` block, no
`--enable-automation`, no `navigator.webdriver=true` to clean up. Target 5
(CDP `Page.addScriptToEvaluateOnNewDocument` / `Emulation.*` + isolated
worlds) is the code to actually write. Targets 3/6 become the
version-pinning design (see (d)).

## (b) Rust Chrome-automation crates (2026-09 status)

| Crate | Latest (verified) | Status / fit as CDP transport |
|---|---|---|
| `chromiumoxide` (+ `chromiumoxide_cdp`, `_types`, `_pdl`, `_fetcher`) | **0.9.1 (2026-02-25)**; 0.9.0 removed async-std, edition 2024, CDP r1566079, reqwest 0.13 / thiserror v2; ~1.4k stars, 23k dl/mo | Current baseline, actively maintained (mattsse). Async tokio-native, full typed CDP codegen (~60k lines), `Browser::launch` + `connect` to existing debugger URL, `Page::evaluate`. **Best keep-as-transport candidate**: stealth launch layer can build its own `BrowserConfig` (custom exe, args, remote-debugging port) and keep `Page` API for waiver/lineup flows. |
| `headless_chrome` | **1.0.21 (2026-02-03)** / 1.0.22 docs build 2026-06-11; org `rust-headless-chrome`, ~2.9k stars, sync API over threads | Puppeteer-equivalent; has `fetch` feature (auto-download known-good Chromium) and headful option. Sync API fights the repo's tokio shape; less CDP coverage than chromiumoxide. Fallback only. |
| `fantoccini` | **0.22.0 (2025-06-14)**, ~2k stars, 32k dl/mo | WebDriver protocol (needs chromedriver/geckodriver on port 4444). **Architecturally wrong here**: reintroduces exactly the driver binary UC patches. Reject as transport. |
| `thirtyfour` | **0.36.1 (2025-07-06)**, ~1.4k stars, 23k dl/mo; v0.29+ backend switched to fantoccini; has `WebDriver::managed`, limited CDP support + `cdp-events` WS feature | Same WebDriver objection as fantoccini. Only relevant as prior art: `Ulyssedev` crate below is built on thirtyfour 0.31. |
| `chromiumoxide_cdp` standalone | versioned with parent (0.9.1) | Only useful if the design splits launch (custom) from protocol types; normally just comes along with `chromiumoxide`. |

Recommendation: **keep `chromiumoxide` 0.9.1 as the CDP transport under a new
stealth launch layer** (answers open question 3 with "keep, isolate launch").
It already supports `Browser::connect` (attach to a self-launched Chrome with
`--remote-debugging-port`), custom `chrome_executable`, and extra args — i.e.
the nodriver pattern without changing `Page`-level call sites. `headless_chrome`
is the only credible alternative transport and loses on async fit + CDP
coverage; WebDriver-based crates are disqualified by the stealth goal.

## (c) Maintained Rust stealth / undetected crates

- **`undetected-chromedriver` 0.1.2 (crates.io, 2023-09-13; Ulyssedev, 63
  stars).** Thirtyfour-based port of UC (binary patch + driver download).
  **Stale (no release in ~3y), pins thirtyfour 0.31 / reqwest 0.11 / zip 0.6**
  — dependency conflict with this repo's reqwest 0.12 and modern thirtyfour.
  Useful only as patch-logic reference, not a dependency.
- **`zendriver-rs` (TurtIeSocks, created 2026-05-24, 14 stars, Apache-2.0,
  MSRV 1.85, workspace `zendriver` + `stealth`/`fetcher`/`interception`/
  `cloudflare`/`imperva`/`datadome` features, MCP server).** Async-first raw-CDP
  port of Python zendriver, stealth-by-default (UA/platform spoof, WebGL tiers,
  482-device catalogue, isolated worlds). **Closest live prior art** for the
  "stealth launch + CDP transport" shape, but brand-new / low adoption and a
  whole-framework dependency — vendor the *pattern* (feature-gated stealth
  module, isolated-world bootstrap via `Page.createIsolatedWorld`), not the
  crate.
- **`chaser-oxide` (0xchasercat, created 2026-01-01, 275 stars).**
  **Fork of chromiumoxide itself** with protocol-level stealth
  (`apply_native_profile()`: live `Browser.getVersion` + host OS/RAM
  autodetect, `Emulation.setUserAgentOverride` with full `UserAgentMetadata`
  so `Sec-CH-UA-*` stays coherent; `enable_stealth_mode()`: `cdc_`/
  `__webdriver` cleanup, `webdriver=false`, HeadlessChrome→Chrome UA;
  isolated-world bootstrap; `new_headless_mode()`). Directly compatible
  mental model with this repo's transport. Watch closely; depending on its
  crates.io maturity it is either a dependency candidate or (more likely,
  given "single maintainer, in-Rust, reusable module" constraints) the
  template for an in-repo `src/stealth.rs` that issues the same CDP calls
  through chromiumoxide 0.9.1's typed API.

No mature, widely-adopted Rust UC equivalent exists in 2026; the field is one
stale port plus two 2026 newcomers. That supports the requirements.md instinct
(reusable crate/**module inside this repo**, not an external dependency) and
open question 2 leaning toward **`src/` module first** (extract to crate only
if a second consumer appears).

## (d) Chromedriver / Chrome version management in Rust

Key fact: **if the design follows nodriver (no chromedriver), there is no
driver version to manage** — only the Chrome↔CDP-schema pairing, which
chromiumoxide pins at build time (CDP r1566079 in 0.9.1). What remains is
Chrome binary pinning:

- **Chrome-for-Testing (CfT) is the pinning mechanism** (M115+; old
  `chromedriver.storage.googleapis.com` per-version buckets are legacy).
  Dashboard: `googlechromelabs.github.io/chrome-for-testing`; JSON endpoints
  `last-known-good-versions.json`, `latest-versions-per-milestone.json`,
  `latest-patch-versions-per-build.json` (+ `-with-downloads` variants);
  assets `chrome` / `chromedriver` / `chrome-headless-shell` ×
  linux64/mac-arm64/mac-x64/win32/win64 (+ linux-arm64 since v153). Stable at
  research time: **152.0.7977.82 (r1669021)**. CfT builds disable auto-update
  and strip first-run surfaces — exactly the determinism the requirements
  ("Chrome/driver version-pinning required") ask for.
- **Rust-side options:** (i) `chromiumoxide_fetcher` / `headless_chrome`
  `fetch` feature / `zendriver-fetcher` — all download known-good builds, but
  each ties pinning to its own crate; (ii) thirtyfour `WebDriver::managed`
  (driver auto-manage, irrelevant if driverless); (iii) **preferred: small
  in-repo resolver** — pinned version constant (e.g. `PINNED_CHROME_*` in the
  stealth module or build script) → CfT `-with-downloads` JSON →
  `storage.googleapis.com/chrome-for-testing-public/…` zip → cache under
  `Config::data_dir` (next to `chrome-profile`), sha-checked, launched via
  explicit `chrome_executable` + `--remote-debugging-port`. Version bump =
  reviewed one-line change; CI runs pinned, scheduled job probes latest
  Stable. Reuse `reqwest 0.12` + `zip` already in/near the tree rather than
  adding a fetcher crate.
- Manual-login/headless note: UC and nodriver both warn headless is the
  leakiest mode; CfT's `chrome-headless-shell` vs full `chrome --headless=new`
  choice belongs in the stealth module with headed default for login and
  `new` headless only for `status` (matches current `cli.rs:327/340` split).

## Dependency / pattern summary

- **Keep:** `chromiumoxide 0.9.1` (+ `chromiumoxide_cdp` transitively) as CDP
  transport; `tokio`, `futures::StreamExt` handler pump (existing shape).
- **Add (minimal):** `zip` + already-present `reqwest`/`serde_json`/`sha2`
  for the CfT pinning resolver; `which` (chromiumoxide dep already) for
  system-Chrome fallback. Nothing else if stealth is hand-rolled CDP.
- **Do not add:** `fantoccini`/`thirtyfour`/`webdriver` (reintroduces driver
  binary); `undetected-chromedriver 0.1.2` (stale, conflicts); full
  `zendriver`/`chaser-oxide` frameworks (vendor the pattern instead).
- **Pattern:** `StealthLauncher` owns process lifetime (spawn real Chrome:
  `--remote-debugging-port`, `--no-first-run`, `--no-default-browser-check`,
  `--disable-blink-features=AutomationControlled`, profile dir from
  `Config::chrome_profile_dir`, stdout-inherit off per 0.9.0 behavior,
  Drop/kill + `SLEEPER_BROWSER_DEBUG` logging preserved) →
  `Browser::connect(debug_ws_url)` → bootstrap isolated world
  (`Page.createIsolatedWorld` + `addScriptToEvaluateOnNewDocument`) →
  `Emulation.setUserAgentOverride` with coherent client hints → existing
  `SleeperBrowser` page flows unchanged. New headless (`--headless=new`)
  only for status; login stays headed + 100% manual per policy.
