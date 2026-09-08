# Architecture Research: undetected-chromedriver-rust

Date: 2026-09-08 | Area: `src/browser.rs` (127 lines) + `src/cli.rs` browser commands (`cmd_browser_login`/`cmd_browser_status`, cli.rs:319-350)

## 1. Current architecture (baseline)

- `SleeperBrowser::launch(profile_dir, headless)` (browser.rs:30-42): builds `chromiumoxide::BrowserConfig` with `user_data_dir`, optionally `with_head()`, then `Browser::launch` (which spawns stock Chrome with default automation flags), spawns the handler pump task, opens one `Page` on `https://sleeper.com`.
- Session ops (`is_logged_in`, `wait_for_manual_login`, `debug_dump`) all call `Page::evaluate` / `goto` directly — concrete `chromiumoxide::Page` type leaks into every caller.
- `src/cli.rs` calls the concrete `SleeperBrowser::launch` directly; `Config::chrome_profile_dir()` (config.rs:42-44) is the only indirection, and it only supplies a path.
- `close()` (browser.rs:50-54) does `browser.close()` + `browser.wait()` — correct ordering today, but no `Drop` guard, no lock file, no kill-on-abort path.

## 2. Applicable patterns

### Adapter / Facade seam (required)
Introduce a `BrowserBackend` trait (Facade over "whatever drives Chrome") so waiver/lineup/trade flows depend on the seam, not on a driver crate:

```rust
// src/browser/mod.rs (proposed)
#[async_trait] // or plain async-in-trait, edition 2021 + recent tokio OK
pub trait BrowserSession: Send {
    async fn goto(&self, url: &str) -> Result<()>;
    async fn eval_bool(&self, js: &str) -> Result<bool>;
    async fn eval_dump(&self, js: &str) -> Result<String>;
    async fn close(self) -> Result<()>;
}
pub struct SleeperBrowser<B: BrowserSession> { inner: B, page_url: ... }
```

- Concrete impl `ChromiumOxideSession` wraps `Browser`+`Page` today; a future `StealthSession` (same trait) can swap transports without touching `cli.rs` or the waiver flow.
- This is Adapter (wraps chromiumoxide types in app-owned API) + Facade (one narrow surface: goto/eval/status/close) + Dependency Inversion (high-level flows depend on the trait).

### Strategy for headed-vs-headless
`launch(profile_dir, headless: bool)` is already a boolean Strategy selector. Formalize it:

```rust
pub enum LaunchMode { HeadedManualLogin, HeadlessStatus }
pub struct LaunchConfig { pub profile_dir: PathBuf, pub mode: LaunchMode, pub chrome_bin: PathBuf, pub driver_version: String }
```

- `HeadedManualLogin` → visible window, no `--headless`, human solves CAPTCHA (standing policy preserved).
- `HeadlessStatus` → `--headless=new` + stealth flags; used only for `is_logged_in` polling.
- Builder (`LaunchConfig::builder()`) centralizes flag assembly so Phase 3 has exactly one place to pin/audit Chrome args. Keep the `bool` signature on `SleeperBrowser::launch` as a thin shim during migration to avoid breaking `cli.rs`.

### Three-layer separation (patch vs launch vs session)
Port of undetected-chromedriver maps to three modules with one-way dependencies:

| Layer | Responsibility | Proposed location |
|---|---|---|
| **patch-layer** | Static stealth artifacts: Chrome arg blocklist/removal (`--disable-blink-features=AutomationControlled`, exclude `enable-automation` switch), `cdc_` variable patch or equivalent JS-object hiding, viewport/lang/timezone normalization constants | `src/browser/stealth.rs` (pure functions: `fn stealth_args() -> Vec<OsString>`, `fn patch_script() -> &str`) — unit-testable with no Chrome |
| **launch-layer** | Owns process lifecycle: resolve + pin chrome binary, create profile dir, acquire lock, spawn Chrome with `--remote-debugging-port`, wait for DevTools endpoint, connect CDP, guarantee teardown | `src/browser/launch.rs` (`StealthLauncher`, `LaunchConfig`, `ProfileLock`) |
| **session-layer** | Post-connect behavior: goto/eval/login-poll/debug-dump, i.e. today's `SleeperBrowser` body minus launch | `src/browser/session.rs` (trait + impls) |

Rule: session-layer never spawns processes; launch-layer never evaluates JS; patch-layer has no I/O. This makes the "≥3 consecutive challenge-free runs" success criterion attributable (patch vs flake in launcher).

## 3. Integration points

1. **`Config::chrome_profile_dir()`** — stays the single owner of the profile path. Launch-layer takes it as input and must `create_dir_all` (as today) but add: (a) check `SingletonLock`/`SingletonSocket` before launching, (b) never delete or migrate the dir silently (login persistence is the product promise).
2. **`cmd_browser_login` / `cmd_browser_status` (cli.rs:326-350)** — become thin orchestrators over the seam: `launch → is_logged_in → wait_for_manual_login → close`. No new flags needed for login/status; waiver-claim CLI (open question 5) later adds `BrowserAction::Claim { ... }` calling the same `BrowserSession` trait — no driver import in `cli.rs` after migration (enforce via `rg 'chromiumoxide' src/cli.rs` == empty).
3. **Future waiver-claim flow** — rides `BrowserSession::goto/eval` plus 2–3 new methods (`click_selector`, `wait_for_selector`) added to the trait, not to the concrete type. Live selectors stay in the flow module, not in launch/patch layers.
4. **`SLEEPER_BROWSER_DEBUG` dump (cli.rs:341-343)** — preserve as a session-layer method (`debug_dump`) behind the trait so it works identically under stealth launch. Extend dump with `navigator.webdriver` + stealth-flag echo to aid Phase 3 verification.

## 4. Data-flow / consistency requirements

- **Profile ownership**: `Config` owns the path; launcher owns the lock; session borrows. Exactly one writer: launcher creates dir + lockfile (`<profile>/.sleeper-browser.lock` via `fs2`/`fslock`); second `launch` on a locked profile fails fast with "browser already running — close the login window first" instead of corrupting `Default/` leveldb.
- **Single-instance locking**: Chrome itself holds `SingletonLock`; our lockfile is the early, human-readable guard. `status` (headless) must not steal a headed login window's profile — if lock held, `status` should attach-or-report, never `--headless` over a live headed profile. Phase 3 must decide attach vs abort (recommend: abort with message for v1).
- **Shutdown ordering (zero orphans)**: `close()` → (1) close pages, (2) `browser.close()` (graceful), (3) `browser.wait()` (reap, as today), (4) release lockfile, (5) `Drop` impl that `try_close`/kill child + abort handler task if `close()` was skipped (panic/early return). Add a `Child`-handle or pid file owned by the launcher so a crashed CLI run can be reaped on next launch (kill stale pid if port unresponsive). Verify with `pgrep -f 'chrome.*chrome-profile'` empty after each run.
- **Version-pinning flow**: `LaunchConfig` carries `chrome_version` + `driver/cdp` expectation; launcher asserts `chrome --version` matches pin at startup and fails loudly on mismatch (constraint: Chrome/driver version-pinning required). Pin source: env var or const in `launch.rs`, surfaced in `--version`/debug dump.

## 5. Placement recommendation: module, not crate

**Recommend `src/browser/` module (split current `browser.rs` into `mod.rs` + `stealth.rs` + `launch.rs` + `session.rs`), NOT a new crate or workspace member.** Rationale: single binary crate, single maintainer, ~127-line starting point — a new crate adds workspace/publish/version overhead with zero second consumer (in-repo only is a hard constraint). A module keeps `crate::browser::SleeperBrowser` import paths stable, lets patch-layer unit tests live beside the code, and still allows extraction later if a second binary needs it. Only graduate to `crates/stealth_chrome` if a future MCP-server binary needs the backend without the CLI.

## 6. Does chromiumoxide stay?

**Recommend: keep chromiumoxide as the CDP transport under a stealth launcher for v1 (hybrid), isolate it behind the `BrowserSession` seam so it can be removed later.** Concretely: stop using `Browser::launch` (stock flags are the fingerprint source); instead the launch-layer spawns Chrome itself with stealth args + `--remote-debugging-port=N`, then `Browser::connect(debug_ws_url)` from chromiumoxide for CDP. This satisfies "release build with chromiumoxide removed/isolated" via the *isolated* branch (zero direct `chromiumoxide` imports outside `session.rs`/`launch.rs`), defers a full transport rewrite (custom websocket CDP client) to a follow-up, and keeps the `Page::evaluate` call sites working unchanged. Full removal is justified only if `Browser::connect` itself leaks automation signals — Phase 3 spike should test headed-manual-login with `connect` vs raw CDP and record the result.

## 7. SOLID / Clean disposition for Phase 3

**Disposition: Isolate via seam.** `SleeperBrowser` concretely couples session logic to `chromiumoxide::{Browser, Page}` (DIP violation — high-level login/waiver policy depends on a driver detail; OCP violation — every transport change edits the same struct), but at 127 lines with no dependents beyond two CLI functions it does not justify a Refactor-first rewrite; Phase 3 must introduce the `BrowserSession` trait + `LaunchConfig` boundary and route all new waiver/lineup/trade code through it while leaving existing method bodies intact.
