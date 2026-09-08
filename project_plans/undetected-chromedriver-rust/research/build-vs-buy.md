# Build vs. Buy — undetected-chromedriver-rust
**Date**: 2026-09-08 | **Agent**: SDD Phase 2, Agent 6 | **Repo**: sleeper-mcp

## Key architectural fact (frames everything below)
`SleeperBrowser::launch` (`src/browser.rs:30-42`) drives stock Chrome via **chromiumoxide 0.9.1 over CDP** — there is **no chromedriver binary** in the loop. Python `undetected-chromedriver`'s signature trick (rewriting `cdc_` strings inside the chromedriver binary before launch) therefore has **nothing to patch** here. Only its *second* layer — launch flags, preference hygiene, JS-evasion scripts — transfers to this stack. Any "port" is really a re-implementation of the stealth-launch + stealth-CDP layers, not a line-for-line binary-patch port.

Baseline verified: `BrowserConfig::builder().user_data_dir(profile_dir)` with `with_head()` iff headed (`src/browser.rs:33-36`); profile dir from `Config::chrome_profile_dir()` = `<data_dir>/chrome-profile` (`src/config.rs:40-44`); manual poll loop (`src/browser.rs:84-100`); headless status path honoring `SLEEPER_BROWSER_DEBUG` (`src/cli.rs:339-349`).

## Option 1 — Existing OSS Rust library (buy / adopt)
Candidates found via crates.io + GitHub search, status as of 2026-09-08:

| Crate | Maintenance | License | Fit |
|---|---|---|---|
| `undetected-chromedriver` 0.1.2 (Ulyssedev, thirtyfour-based) | **Stale** — last release 2023-09-13; ~63 stars. Depends on thirtyfour 0.31 + reqwest 0.11 (both far behind current). | Unknown/unspecified on docs.rs page | Poor — drags in Selenium/chromedriver stack we don't use; would force a backend swap, not a hardening. |
| `chromiumoxide_stealth` 2024-07-28 | **Stale/abandoned** — 3 commits, 6 stars, 4 forks; no release since Jul 2024. Thin wrapper injecting selenium-stealth/puppeteer-extra-stealth-derived scripts via `inject(&page)`. | Unknown | Poor — unmaintained JS-evasion snapshots rot fast against Chrome's 4-week release cadence; pins us to someone's dead fork. |
| `chaser-oxide` 0.2.4 (crates.io Apr 2026; repo created 2026-01-01, ~275★/46 forks, ~489 commits) | **Actively maintained in 2026** — newest by far; claims protocol-level CDP stealth, native host-OS fingerprint profiles, tested vs. Turnstile/WAF/bot.sannysoft/CreepJS. | MIT OR Apache-2.0 (fits this repo) | Best fit on paper — drop-in chromiumoxide fork. **Caveats**: (a) commercial halo (`chaser.sh` infra platform — evaluate whether stealth stays OSS or drifts paywalled); (b) repo appears mirrored under several near-identical forks (`ccheshirecat` canonical vs `0xchasercat`, `xsxz01`, `jason-ni`, `penumbra-x` copies with identical READMEs/commit counts — astroturfing/SEO smell, so pin the canonical repo + audit before trusting); (c) young (8 months) — no track record of surviving a full year of Chrome releases; (d) scoped to CDP/JS signals, says nothing about TLS/JA3 or headless-GPU signals. |
| `headless_chrome` 1.x (rust-headless-chrome, ~2.9k★, 855 commits, active into 2026) | Maintained, MIT | MIT | Wrong direction — equally detectable stock launcher, sync API would force rewriting async `SleeperBrowser`; buys nothing on stealth. |
| `browser_oxide` (yfedoseev, Apr 2026, 91★, 4 commits) | Experimental from-scratch engine (deno_core, no Chromium) | Apache-2.0 | Not viable — immortal-beta layout engine cannot render sleeper.com's real SPA/CAPTCHA widgets; fails the "human solves CAPTCHA in a real window" requirement by design. |

**Pros**: fastest path if a maintained crate fits; upstream absorbs Chrome-release churn; permissive licenses available.
**Cons**: nothing maintained *and* trustworthy except chaser-oxide, which is young and commercially entangled; the two older "undetected/stealth" crates are dead and would pin stale dependency trees (thirtyfour 0.31, reqwest 0.11) into the release build.
**Verdict**: **Viable** — *only* chaser-oxide qualifies, as a spike candidate, not a committed dependency. The other crates are **Not recommended**.

## Option 2 — Flag-only hardening of chromiumoxide (no binary patch)
What it is: keep chromiumoxide; fix the launch (`--disable-blink-features=AutomationControlled`, strip `--enable-automation`, `excludeSwitches: [enable-automation]`, `useAutomationExtension: false`, realistic UA/viewport/timezone/locale, persistent `user_data_dir` already owned by `Config`) plus CDP hygiene (`Page.addScriptToEvaluateOnNewDocument` evasions for `navigator.webdriver`, `chrome.runtime`, plugins, WebGL vendor, `User-Agent-CH` consistency).

**Pros**: smallest diff (stays in `src/browser.rs` launch path); zero new dependencies/licenses; preserves `Config::chrome_profile_dir()` ownership, manual-login policy, and `SLEEPER_BROWSER_DEBUG`; easy to A/B against the ≥3-consecutive-run success bar.
**Cons**: honest assessment — this clears only **Generation-1** signals (`navigator.webdriver`, AutomationControlled blink feature). It does nothing about Gen-2 (CDP `Runtime.enable` / domain-list fingerprinting — chromiumoxide *is* a CDP client, so its protocol footprint is intrinsic) or Gen-3 (TLS/JA3, headless GPU/software-GL strings, behavioral timing). Public 2025–2026 writeups (Zenrows, Decodo, Starlog) converge: flag-only passes bot.sannysoft's webdriver row while failing the rest, and headless stays "might work but will break randomly" even in UC upstream. Sleeper's challenge *loops* (rather than failing once) suggests score-based detection, exactly the kind flag-only rarely moves.
**Verdict**: **Viable as Phase-1 spike, Not recommended as the whole solution.** Cheap enough to try first (days, not weeks) and worth keeping regardless — but do not declare success on flags alone; require the fresh+authenticated ≥3-run bar before stopping.

## Option 3 — LLM-generated port of undetected-chromedriver's patch logic vs. battle-tested upstream
**Pros**: in-repo, in-Rust, no new dependency or vendor tie; can scope the port to exactly what a CDP stack needs (launch-arg hygiene + version-pinned launcher + evasion scripts), skipping the inapplicable binary-patch half.
**Cons** (the weightier side): (a) **correctness risk** — bespoke patch/evasion code is *adversarial* code; an LLM can reproduce UC's 2023-era evasions fluently while missing that Chrome changed the surface (e.g. `headless=new` vs old headless, UA-CH adoption, `cdc_` rotation). There is no ground truth except live challenge testing, which is slow and flaky. (b) **maintenance burden** — Chrome ships every ~4 weeks; UC upstream itself breaks on major bumps (3.1.0+ refactor) and needs active monitoring. A single maintainer hand-rolling this owns a permanent treadmill: pin Chrome version, re-verify ≥3 runs per bump, diff evasions against puppeteer-extra-stealth upstream. (c) **policy edge** — the repo already draws a line at bot-detection evasion for the login step (`src/browser.rs:74-83`); a stealth backend must stay scoped to *restoring a normal headed browser footprint for a human-operated session*, not scripting around challenges. Bespoke code makes that line harder to review than a pinned upstream diff.
**Verdict**: **Viable with guardrails, not as first choice.** If built, build only the CDP-applicable half (launcher + hygiene + script injection + Chrome/driver version pinning), never a chromedriver binary patcher; and budget the ongoing release-tracking cost explicitly (this is where the 3–6-week appetite goes, plus ~hours per Chrome bump forever).

## Option 4 — Fork-or-adapt a close Rust codebase
- **chaser-oxide** (chromiumoxide fork, protocol-level stealth): closest in-tree option — same async/CDP API shape, so `SleeperBrowser` internals change minimally. Adapt path: spike-swap `chromiumoxide` → `chaser-oxide`, run the fresh/authenticated matrices, keep the chromiumoxide path behind a feature flag until the bar passes. Risks are Option 1's caveats (youth, chaser.sh commercial orbit, fork-network smell — vendor-pin and audit).
- **`chromiumoxide_stealth` / thirtyfour-based `undetected-chromedriver`**: not close enough — dead (2023/2024), wrong layer (page-script-only or Selenium-stack), would cost more in dependency archaeology than writing the launcher ourselves.
- **`spider_chrome`** (maintained chromiumoxide fork in the spider ecosystem): worth a look during the spike as a *neutral* fork alternative if chaser-oxide's vendor tie concerns hold up — but it is crawl-oriented, with no stealth claims, so expect flag-level gains only.
- Upstream `chromiumoxide` itself (Feb 2026 release, maintained): the fallback — hardened in place per Option 2.

**Pros**: starts from code that already tracks CDP/Chromium churn; smallest bespoke surface.
**Cons**: inherits the fork's bus factor and roadmap; API drift from upstream chromiumoxide over time.
**Verdict**: **Recommended** — spike chaser-oxide first (timeboxed, ≤1 week), fallback to hardened in-place chromiumoxide.

## Excluded (one line each, per scope)
- **SaaS/managed browser or proxy APIs** (e.g. ScrapingBee/Zenrows-style): out of scope — constraints require in-Rust, no SaaS/proxy workarounds.
- **Separate Python service** (e.g. sidecar running Python `undetected-chromedriver`/`nodriver`): out of scope — already rejected by the user; evaluate in-Rust options only.

## Recommendation
**Fork-or-adapt, in this order: (1) timeboxed chaser-oxide spike → (2) flag+hygiene hardening of in-place chromiumoxide → (3) bespoke port of only the CDP-applicable UC logic if both fail.** Rationale: the binary-patch core of Python UC is inapplicable to a CDP stack, so "port UC" was never the real choice — the real choice is *who maintains the stealth-launch/CDP-hygiene layer against Chrome's release train*. A maintained chromiumoxide-shaped fork (chaser-oxide, MIT/Apache, active 2026) is the only candidate that answers that; flag-only hardening is cheap and worth doing either way but honestly insufficient alone; bespoke code is the fallback that maximizes maintainer burden and should be scoped to the smallest layer that clears the ≥3-run bar. Keep `Config::chrome_profile_dir()` ownership, 100%-manual login, `SLEEPER_BROWSER_DEBUG`, zero-orphan `close()` semantics, and Chrome version-pinning invariant through all three paths.
