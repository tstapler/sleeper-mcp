# Pitfalls & Risks: undetected-chromedriver-rust

Date: 2026-09-08 | Agent 4 (Phase 2) | Repo: `sleeper-mcp`, project `undetected-chromedriver-rust`
Codebase anchor: `src/browser.rs:30-42` (launch), `src/browser.rs:57-72` (`is_logged_in`), `src/browser.rs:84-100` (manual-login poll), `src/config.rs:42-44` (profile dir), `Cargo.toml:35` (`chromiumoxide = "0.9.1"`).

## 1. undetected-chromedriver failure modes (what breaks in Python — will break in Rust too)

### 1.1 Chrome auto-update invalidates the patch (highest-frequency failure)
- UC works by binary-patching chromedriver (rename `cdc_`-prefixed vars to random strings) + launch flags (`--disable-blink-features=AutomationControlled`). The patch is byte-offset / string-literal dependent.
- Every Chrome major (~4 weeks) can move offsets or add new markers. Symptom class from 2023–2026 reports: `version mismatch after Chrome auto-updates`, driver fails to start, or silently re-detected (stuck on "Just a moment...", 403, Turnstile spinning forever).
- Sources (2026): NSLSolver "UC Not Working 2026" guide; webscraping.cc 2026 guide ("breaks whenever Chrome ships a version that outpaces the patch"); QuanticData 2026 ("tightly coupled to installed Chrome version… when Chrome updates the pairing can break").
- **Design against:** Chrome/driver version-pinning is mandatory, not optional. Pin via Chrome-for-Testing (CfT) endpoints, verify `chrome --version == chromedriver --version` at launch, fail fast with actionable message. Never auto-download "latest" at runtime. Never patch the user's system Chrome in place.

### 1.2 Patch-offset drift per release
- Even with matched versions, each new Chrome/Driver release can rename/add detection strings (`cdc_`, `$cdc_`, `$chrome_`), change `navigator.webdriver` plumbing, or alter headless UA (`HeadlessChrome` token). UC history shows repeated emergency fixes (Chrome 110 headless surprise; v3.4.0 rewrite from "remove/rename vars" to "prevent injection in the first place").
- A Rust port that hardcodes byte patterns will rot the same way.
- **Design against:** patch table externalized per (chrome, driver) version tuple with checksums; startup self-test (scan patched binary for residual `cdc_` literals); integration test that greps `document` keys in a live page; CI job that probes latest CfT release and opens an update task on drift.

### 1.3 Driver/Chrome version skew
- Two skew axes: (a) system Chrome auto-updates under a pinned driver; (b) fetcher downloads driver for wrong channel (stable vs beta). Both produce either launch crash or silent re-detection.
- **Design against:** own the whole chain — download both binaries from CfT `known-good-versions` JSON into `<data_dir>/stealth/` (NOT the user profile dir), record versions in a lockfile, refuse to launch on skew.

### 1.4 Detection arms-race maintenance burden (2026 status: UC losing ground)
- 2026 consensus: UC "not dead, but edge worn thin" — still strips oldest layer (`navigator.webdriver`, `cdc_` grep), increasingly fails against Turnstile / hard managed challenge / CDP-runtime probing / TLS JA3/JA4 fingerprinting. Maintenance has slowed; same maintainer (ultrafunkamsterdam, ~12.8k stars / 331 commits) now points new work at **nodriver** (drives Chrome directly, no Selenium/ChromeDriver layer, far fewer CDP traces).
- Implication for a Rust port: cloning UC's 2021–2023 patch set reproduces a backend already behind. The durable direction is CDP-minimal (skip `Runtime.enable`, isolated worlds via `Page.createIsolatedWorld`, defer debugger attach) per rebrowser/nodriver/selenium-driverless — not just `cdc_` renaming.
- **Design against:** scope the crate as "fingerprint-removal + CDP-minimal driver," with a maintenance plan (owner, update cadence tied to Chrome releases, deprecation trigger if pass-rate drops). Success criterion "≥3 consecutive runs" must be re-run after each Chrome bump, not once.

### 1.5 Headless-vs-headed gap
- Headless adds tells: `plugins.length == 0`, missing codecs, `Notification.permission == "denied"`, SwiftShader WebGL vendor, `HeadlessChrome` UA, differing `window.chrome.runtime`. `sleeper browser status` currently runs headless (`src/cli.rs:339-349`); login runs headed. A stealth port must treat these as two separate profiles to validate.
- **Design against:** validate challenge-free on BOTH paths; headed login must never silently fall back to headless flags.

## 2. Rust-specific risks

### 2.1 Patching a binary from Rust safely + reversibly
- Pitfalls: patching in place corrupts the cached driver on partial write; concurrent launches race on the same file; no rollback leaves a half-patched binary that fails opaquely next run.
- **Design against:** copy-on-write — copy pristine driver to versioned path (`<data_dir>/stealth/chromedriver-<ver>-patched`), patch the copy, verify (no residual marker + sha256 of result recorded), atomic rename. Keep pristine copy; `Drop`/uninstall restores or deletes patched copy only. Patch function must be pure + idempotent (re-patching a patched binary is a no-op or error, never double-mangle). Unit-test on fixture binaries, never on live system Chrome.

### 2.2 Process-group cleanup (orphans)
- Current `close()` (`src/browser.rs:50-54`: `browser.close()` + `browser.wait()`) only covers the graceful path. Chrome spawns a process tree (browser + renderers + GPU + crashpad); if the CLI is Ctrl-C'd, panics, or `close()` is skipped, children outlive the parent and hold the profile lock (`SingletonLock`), poisoning the next run.
- **Design against:** spawn Chrome in its own process group (`Command::pre_exec` + `setsid` on Linux), kill-by-group with timeout (SIGTERM → grace → SIGKILL), reaping zombies; wire Ctrl-C / panic path to the same cleanup; add an orphan-sweep at launch (detect stale lock + liveness check before deleting — never blindly `rm SingletonLock`); regression test: launch → SIGKILL parent → assert `pgrep -f chrome-profile` empty.

### 2.3 chromiumoxide 0.9.1 staleness — checked 2026-09-08
- Findings: `chromiumoxide = "0.9.1"` in `Cargo.toml:35` **is the latest documented release** (docs.rs shows 0.9.1; libs.rs/lib-releases note latest release Nov 28 2025; ~1.3k GitHub stars, maintainers mattsse/Sytten). So "stale version pin" is NOT the current problem — but structural staleness risks remain:
  - Small maintainer bus-factor; CDP bindings are ~60k lines of generated code (`chromiumoxide_cdp` build-time PDL regen) → slow to track new Chrome DevTools Protocol domains; compile times 3–5 min.
  - Known long-standing bugs: DevTools port parsed from Chrome stdout assuming **English locale** (non-English systems time out); tokio-only runtime; `Page::execute` escape hatch needed for missing commands.
  - CDP-full design: chromiumoxide issues `Runtime.enable`-style instrumentation by default — exactly the surface 2026 detectors probe (console-serialization side effects, `Runtime.evaluate` timing, `toString()` on patched natives). A stealth backend may need to bypass or wrap chromiumoxide's page/context creation, which fights the library's happy path.
- **Design against:** isolate chromiumoxide behind a trait (`BrowserBackend`) so it can be removed without touching `client.rs`/CLI flows (per requirements "removed/isolated"); budget for either (a) contributing CDP-minimal patches upstream, or (b) replacing transport with a minimal CDP client (e.g. `nodriver`-style) if stealth tests show chromiumoxide's chatter is itself the tell. Do NOT attempt both in one change.

### 2.4 Release-build + dependency weight
- `chromiumoxide_cdp` codegen bloats debug AND release builds; stripping it changes binary size/profile. Success criterion "release build with chromiumoxide removed/isolated" needs a feature-flag (`--no-default-features` / `backend-stealth` vs `backend-chromiumoxide`) so removal is verifiable (`cargo tree | grep chromiumoxide` must be empty in stealth profile).

## 3. Sleeper-side risks

### 3.1 Behavioral scoring vs one-off checks (load-bearing lesson, already in-repo)
- `src/browser.rs:77-82` documents the key prior: scripting the login form (native setter + synthetic event, no real keystroke timing) **re-triggered the bot challenge even after a human solved it once**. The detector scored ongoing behavior, not just the CAPTCHA moment.
- Consequence: any stealth technique that fakes interaction signals (synthetic typing cadence, mouse jitter, scroll timing tuned to "look human") repeats the same mistake and crosses into evasion the project explicitly rejected.
- **Design against:** post-login automation must use only coarse, human-initiated-equivalent actions (navigate, click own-team buttons); login + challenge stays 100% manual with real OS-level input. Measure success by "challenge never appears for a manual login," not "scripted login passes."

### 3.2 Selector drift in `is_logged_in`
- Current check (`src/browser.rs:61-66`): `button.textContent.trim().toLowerCase() === 'log in'` + `offsetParent !== null`. Brittle to: copy change ("Login"/"Sign in"), i18n, button rendered as `a`/`div`, shadow DOM, SPA hydration delay (500 ms sleep + `wait_for_navigation().ok()` swallows the failure).
- The requirements call out an earlier **case-sensitivity false-positive** in this function — same bug class will recur with whitespace/case/punctuation variants.
- **Design against:** multi-signal check (button text set incl. "log in"/"login"/"sign in" + authenticated marker like avatar/account menu + URL/session cookie presence), normalize (lowercase, collapse whitespace, strip punctuation), explicit wait-for-hydration (poll DOM ready + network idle, not fixed sleep), and keep `SLEEPER_BROWSER_DEBUG` dump (`debug_dump`, `src/browser.rs:103-125`) as the triage path. Log which signal fired.

### 3.3 Warm-profile dependence
- 2026 guides agree a persistent `user-data-dir` with cookies/localStorage/prior clearance looks far more human than a fresh profile challenged every launch. The repo already does this (`chrome_profile_dir()`), but "challenge-free on fresh profile" is the harder criterion — fresh profiles may legitimately see one passive check.
- **Design against:** define success separately (fresh: challenge solvable manually once, then persists; authenticated: zero challenges across ≥3 runs). Never delete/migrate the profile as a "fix" for detection — that destroys the warm signal. Profile dir owned solely by `Config::chrome_profile_dir()`; stealth binaries/cache live elsewhere.

### 3.4 Headless `status` vs headed `login` divergence
- If `status` (headless) and `login` (headed) use different flag sets, a passing headed login proves nothing about headless status. Test both, log effective flags + versions on every run for post-mortem.

## 4. Policy guardrails (fingerprint-removal ✅ vs interaction-faking ❌)

Standing policy: login stays 100% manual; no credential/CAPTCHA scripting. The stealth line:

| Allowed (remove false automation tells) | Forbidden (fake human behavior / solve challenges) |
|---|---|
| Remove `--enable-automation`, `navigator.webdriver=true`, `cdc_` injection | Synthetic keystroke timing, mouse-movement jitter, scroll/click cadence tuning to lower a bot score |
| `--disable-blink-features=AutomationControlled`, real UA, consistent locale/TZ/viewport | Auto-solving CAPTCHA/Turnstile/proof-of-work, token minting, challenge-clearance replay via proxies |
| Populate `plugins`/`languages`/`window.chrome` to true-Chrome values; version-pinned CfT binaries | TLS/JA3 spoofing or header forgery whose purpose is impersonating a human session |
| CDP-minimal transport (fewer protocol artifacts) | SaaS/proxy challenge-solving services (explicitly out of scope) |

- **Flag any technique that "lowers the score by acting human"** (browser.rs:80-81 already names this as evasion). Fingerprint-removal makes the browser *not lie about being automated infrastructure*; interaction-faking makes it *lie about being a person*. Only the former is in scope.
- Concretely out of scope for this crate: credential fill, form submit scripting, CAPTCHA widgets, Turnstile `cf-turnstile-response` handling, `cf_clearance` harvesting, residential proxies.
- Design gate: every stealth flag/patch gets a one-line justification citing which false tell it removes; anything justified as "looks more human" is rejected in review.

## 5. Explicit "design against" checklist (for the plan phase)

1. Chrome auto-update breaking the patch → CfT pinning + version-skew fail-fast + per-release re-validation.
2. Patch-offset drift → externalized per-version patch table + binary self-test + live-page marker test.
3. Maintenance abandonment (UC-style) → owner + cadence + nodriver/CDP-minimal fallback trigger.
4. Unsafe/in-place patching → copy-on-write + checksum + atomic replace + restore.
5. Orphaned Chrome trees + profile lock poisoning → process groups + SIGTERM/SIGKILL + launch-time stale-lock triage.
6. chromiumoxide CDP chatter as its own tell → `BrowserBackend` trait isolation + feature-flagged removal.
7. `is_logged_in` false positives/negatives → multi-signal, normalized, hydration-aware check + debug dump preserved.
8. Fresh-vs-authenticated conflation → separate success criteria per profile state.
9. Behavioral-score regression (scripted login) → manual-login-only enforcement; no synthetic input timing.
10. Scope creep into interaction-faking → per-patch justification gate; credential/CAPTCHA/Turnstile/proxy work rejected.

## Sources (verified 2026-09-08 via WebSearch)

- NSLSolver, "undetected-chromedriver Not Working in 2026" (2026-06-16) — UC mechanism, CDP/TLS/higher-order tells, slowed maintenance → nodriver.
- webscraping.cc, "Undetected ChromeDriver — The 2026 Guide" (2026-05-06) — slow updates, breaks on Chrome version drift.
- QuanticData, "How to Use undetected-chromedriver" (2026-07-30) — Chrome/driver coupling.
- ultrafunkamsterdam/undetected-chromedriver (GitHub, ~12.8k★/331 commits) + PyPI — patch-and-download model, `TARGET_VERSION`.
- AlterLab (2026-02-07, 2026-05-14), Jibao Proxy (2026-06-06), Foil "CDP detection" (2026-07), DataDome (2026-06-01: Playwright 1.53 webdriver change; V8 May-2025 CDP-preview patches) — layered 2026 detection: `navigator.webdriver`, `cdc_` markers, plugins, headless flags, canvas/WebGL, TLS JA3/JA4, HTTP/2, CDP `Runtime.enable` side effects, coherence checks, behavioral scoring.
- chromiumoxide: docs.rs 0.9.1, crates.io/lib.rs (latest release Nov 28 2025), mattsse/chromiumoxide README (fetcher, English-locale port parsing, ~60k-line PDL codegen).
