# Feature Landscape: undetected-chromedriver-rust

Date: 2026-09-08 | Phase: SDD-2 Agent 2 (Features)

## 1. Similar features in this codebase

- `src/browser.rs` — `SleeperBrowser::launch(profile_dir, headless)` starts stock
  Chrome via chromiumoxide 0.9.1 against `<data_dir>/chrome-profile`
  (`Config::chrome_profile_dir()`); `is_logged_in()` (visible-"Log In"-button probe),
  `wait_for_manual_login()` (5s poll loop), `close()` (clean CDP shutdown),
  `debug_dump()` (URL/title/button dump, gated on `SLEEPER_BROWSER_DEBUG`).
- `src/cli.rs` — `sleeper browser login` (always headed, 10-min manual window,
  `cmd_browser_login` ~L326) and `sleeper browser status` (headless read-only check,
  `cmd_browser_status` ~L339). No waiver/lineup/trade commands exist yet — the stealth
  backend is the foundation those ride on.
- `docs/tasks/browser-automation-handoff.md` — full history: scripted login was tried,
  caused repeating CAPTCHA loops (behavioral bot score, not a one-off gate), and was
  deleted in favor of 100%-manual login. Also notes chromiumoxide WS
  `Message`-enum log spam from CDP drift vs installed Chrome 146 (silenced via tracing
  directive in `src/main.rs`), i.e. version skew is already biting.
- **Policy tension (must resolve in design):** the handoff doc records a standing rule —
  "Login is 100% manual… anti-detection tooling (undetected-chromedriver, stealth
  patches, fingerprint spoofing) was explicitly requested twice and declined both
  times… a fixed policy line, not a preference to revisit." The new requirements.md
  explicitly requests undetected-chromedriver-style stealth. The design must reconcile
  these: the defensible reading is stealth-launch for a *human-driven* session
  (remove automation fingerprints so a real person's manual login isn't mis-scored)
  with the no-credential/CAPTCHA-scripting ban intact — but this needs explicit owner
  sign-off, not an assumption.

## 2. Industry landscape

| Project | Mechanism | Rust-relevant takeaway |
|---|---|---|
| undetected-chromedriver (Python/Selenium) | Downloads matching chromedriver, **binary-patches** `$cdc_*` / `cdc_…` injection blobs in place (same-length replacement), launches with `--disable-blink-features=AutomationControlled`, suppresses automation infobar | Patch-target list + version-matching downloader are the portable ideas; binary patching is brittle per Chrome release and needs a re-verify step |
| puppeteer-extra-plugin-stealth (~19 evasion modules) | `evaluateOnNewDocument` JS patches: `navigator.webdriver`, `window.chrome` (+`runtime`/`app`/`csi`/`loadTimes`), plugins, languages, permissions, WebGL vendor, UA, iframe `contentWindow`, outer dimensions | Taxonomy of in-page tells to replicate via CDP `Page.addScriptToEvaluateOnNewDocument`; each patch is a maintenance liability as Chrome changes |
| playwright-stealth / `playwright-extra` | Port of the above to Playwright | Same as above; community notes the package is stale vs 2026 detection |
| rebrowser-patches / patchright / nodriver / puppeteer-real-browser (2026 state of art) | Fix the **`Runtime.enable` CDP leak** at the driver/protocol layer instead of in-page JS | Key architectural lesson: in-page patches can't hide CDP-transport artifacts (`Runtime.executionContextCreated`, `console.debug` getter side effects). Prefer launch-flag/engine-level fixes over JS spoofing where possible |
| `--headless=new` + `--disable-blink-features=AutomationControlled` | Engine-level: new headless runs the real browser UI stack; the blink flag stops `navigator.webdriver` at the source | Highest-leverage single fix; leaves no redefined-getter artifact for `Object.getOwnPropertyDescriptor` audits to find |

Cross-cutting 2026 findings: `navigator.webdriver=false` (patched) vs `undefined`
(absent) matters — detectors check for the patch shape; proxies fail
`Function.prototype.toString` `[native code]` audits; UA must agree with
`Sec-CH-UA` Client Hints + `userAgentData`; `navigator.permissions` must agree with
`Notification.permission`; plugin arrays must survive `for…in`/`for…of` iteration;
`window.chrome` structure drifts per Chrome version (copy from a real Chrome);
patches belong at context level (iframe inheritance); validate weekly against
bot.sannysoft.com / creepjs / browserscan.

## 3. Edge cases & failure modes the design must handle

1. **Chrome auto-update invalidating patches** — silent Chrome upgrade changes
   injection blobs / `window.chrome` shape; design needs a pinned-version check and a
   patch-verification self-test that fails loudly, not a silent stealth regression.
2. **Driver/Chrome version skew** — already observed (chromiumoxide 0.9.1 CDP defs vs
   Chrome 146, WS spam). Pin Chrome + driver + CDP-bindings versions together; emit
   actionable mismatch diagnostics (expected vs found versions, what to install).
3. **Headless-vs-headed detection delta** — `status` runs headless, `login` headed;
   headless historically leaks (plugins=0, UA `HeadlessChrome`, outerWidth=0,
   missing GPU). `--headless=new` + flag parity + separate headless/headed acceptance
   runs required.
4. **Orphaned processes** — `close()` exists but panics/timeouts can strand Chrome.
   Need Drop-guard / kill-on-exit + startup sweep of stale PIDs holding the profile.
5. **Profile lock contention** — two `sleeper browser` invocations on one
   `<data_dir>/chrome-profile` → Chrome SingletonLock failure. Detect lock, fail with
   a clear message (no Chromium-internal error passthrough).
6. **Login-state selector drift** — `is_logged_in()` depends on literal "Log In"
   button text (already false-positived once on casing). Needs multi-signal check
   (URL + session cookie/localStorage + button probe) and a documented re-baselining
   procedure against the live DOM.
7. **Patch-shape detection** — every JS spoof is itself fingerprintable (proxies,
   `toString`, iteration, permissions mismatch, HTTP-vs-HTTPS `chrome.runtime`
   exposure). Prefer engine-level flags; keep in-page patch set minimal and versioned.
8. **CDP-transport artifacts** — `Runtime.evaluate` isolated worlds,
   `Runtime.enable` context-created events, `sourceURL` markers. Keep CDP traffic
   minimal during login/status; don't poll faster than needed.
9. **IP/behavioral layer (out of scope but must state)** — no browser patch fixes
   datacenter-IP reputation or bot-like timing; success criteria assume residential
   single-operator use, else challenge-free runs are unachievable regardless.

## 4. Unstated needs (gaps the design should close)

- **Version-mismatch diagnostics**: startup check printing Chrome version, driver
  version, CDP-binding version, patch fingerprint; fail fast with remediation text.
- **Patch-verification self-test**: e.g. `sleeper browser doctor` that launches,
  asserts `navigator.webdriver` absent/`undefined`, no `cdc_` globals, plugins sane,
  UA/Client-Hints consistent — the automated form of the ≥3-run acceptance gate.
- **Debug-dump parity**: `SLEEPER_BROWSER_DEBUG` aid must survive the migration and
  extend to stealth signals (dump webdriver value, cdc globals, UA, plugins length).
- **Policy reconciliation**: explicit owner decision recorded (stealth-for-manual-login
  vs prior blanket decline) before implementation; waiver-claim CLI shape and live
  selectors stay deferred until an authenticated session exists (per handoff §Next steps).
- **Release hygiene**: chromiumoxide removal/isolation verified in release build
  (dependency audit), plus a regression test that stock-launch fingerprints differ
  from stealth-launch fingerprints on the self-test page.

## Sources

- Codebase: `src/browser.rs`, `src/cli.rs` (BrowserAction/cmd_browser*),
  `src/config.rs` (`chrome_profile_dir`), `docs/tasks/browser-automation-handoff.md`.
- undetected-chromedriver `patcher.py` (ultrafunkamsterdam); Foil "Selenium/Puppeteer
  bot detection" series; crawlex.net puppeteer-stealth patch-by-patch (2026-04);
  HelperX "Headless Browser Detection in 2026"; BotCloud "Navigator.webdriver Runtime
  Artifacts"; Serpent API "Does puppeteer-extra Stealth Still Work in 2026";
  StealthCheck Playwright stealth test; feder-cr `invisible_playwright` cdc explainer.
