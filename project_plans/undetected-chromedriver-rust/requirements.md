# Requirements: undetected-chromedriver-rust

**Date**: 2026-09-08
**Type**: feature addition (browser-backend migration inside the existing CLI)
**Complexity**: 3 — system design

## Problem Statement

`sleeper browser login` / `status` (driven by `chromiumoxide` in `src/browser.rs`) carries headless-automation fingerprints that keep Sleeper's behavioral bot score bad: a human solving the CAPTCHA once lands in a repeating challenge loop instead of a clean session (`docs/tasks/browser-automation-handoff.md:43-58`). The user needs an undetected-chromedriver-style stealth layer ported to Rust — a reusable patch/launch path inside this repo — so a human-authenticated session works challenge-free and stays alive in the persistent profile.

## Baseline

Today `SleeperBrowser::launch` (`src/browser.rs:30-42`) starts stock Chrome via `chromiumoxide` against `<data_dir>/chrome-profile` (`src/config.rs:40-44`); `wait_for_manual_login` (`src/browser.rs:84-100`) polls the visible-"Log In"-button check (`src/browser.rs:56-72`) while a human logs in by hand in a headed window (`src/cli.rs:326-336`); `status` does a headless read-only check (`src/cli.rs:339-349`). This works but re-triggers bot challenges, and scripted form-fill was already tried and removed as the root cause (`docs/tasks/browser-automation-handoff.md:43-58`).

## Users / Consumers

Single operator (Tyler) running `sleeper browser ...` on Linux; downstream consumers are the future `waiver-claim`, lineup, and trade flows that will drive the same backend through `SleeperBrowser`.

## Success Metrics

- `sleeper browser login` on a fresh profile completes one manual login with no repeating challenge loop, and `sleeper browser status` then reports logged-in on consecutive runs (fresh + authenticated profiles, ≥3 consecutive challenge-free runs each).
- `cargo build --release --bin sleeper` succeeds with direct `chromiumoxide` use removed or isolated behind the new backend.
- No orphaned Chrome/driver processes remain after normal `login`, `status`, or failure paths (verified via process table, not just exit codes).
- Regression anchor: today's manual-login-then-poll behavior and `SLEEPER_BROWSER_DEBUG` dump aid keep working through the swap.

## Appetite

Large (3–6 weeks). Scope must fit: full waiver/lineup/trade roadmap rides the new backend, but if the stealth port overruns, cut automation scope (lineups/trades first) — not login/status parity, and do not move the deadline.

## Constraints

- Single maintainer; Linux primary target.
- Login stays 100% manual — human types credentials and solves CAPTCHA in the visible window. Nothing scripts credential entry or CAPTCHA solving (standing policy from `docs/tasks/browser-automation-handoff.md:16-20`, re-confirmed; the new authorization covers backend stealth selection only).
- Profile dir stays owned/configured by `Config::chrome_profile_dir()`; no migration unless explicitly planned.
- In-Rust implementation inside `sleeper-mcp` (a reusable crate/module, not a separate repo or long-lived service — user rejected the separate-backend option).
- Chrome/driver version-pinning strategy must be defined so auto-updates can't silently invalidate patches.

## Non-functional Requirements

- **Performance SLO**: not specified — `login` is human-paced; `status` should stay within a few seconds of today's headless check.
- **Scalability**: not applicable — single operator, one browser instance at a time.
- **Security classification**: confidential (backend touches an authenticated web session; session cookies live in `chrome-profile`).
- **Data residency**: no special requirements.

## Scope

### In Scope

- Rust port of undetected-chromedriver-style stealth (nodriver approach): stealth launch flags + CDP-behavior hygiene scripts + UA/client-hints coherence + Chrome-for-Testing (CfT) version pinning, wired into a `SleeperBrowser` backend that preserves `launch` / `is_logged_in` / `wait_for_manual_login` / `close` / `debug_dump` semantics. Explicitly: no binary patching of any kind — neither the CfT Chrome binary nor any driver binary is patched in place or copied-and-patched (no `cdc_` fingerprint removal on binaries; there is no chromedriver binary in this CDP stack).
- `login` + `status` parity on fresh and authenticated profiles, challenge-free.
- Clean shutdown on all paths (no orphaned Chrome/driver processes).
- Waiver claims, then lineups and trades, automated against the human-authenticated session through the new backend (motivating case: Carter/Emmanwori moves, league `1352883872131481600`).
- Live-UI verification before each automated action; selectors are never guessed.
- `sleeper browser doctor` is the authorized version-skew diagnostic and `sleeper browser status --json` is authorized as additive machine-readable output (human default output unchanged when the flag is absent).

### Out of Scope

- Scripting credential entry or CAPTCHA/verification solving — explicit exclusion, stays manual forever per standing policy.
- A separate backend repo/project or a long-lived local service API (rejected scope option).
- SaaS/proxy or residential-IP workarounds for bot scoring.
- Changes to Sleeper's public read-only API client (`src/client.rs`).

## Rabbit Holes

- Porting undetected-chromedriver's stealth layers to Rust: the port covers the stealth-launch/CDP-hygiene layers (launch-flag set + CDP evasion scripts + UA coherence), not a binary patcher — no Chrome-binary patch targets exist in this stack. Load-bearing vs cargo-cult analysis applies to flags/scripts, timeboxed against the success metric.
- CDP detection surface is a moving target (runtime flags, `navigator.webdriver`, headless-mode tells); hardening can turn into endless flag-whack-a-mole — timebox it against the success metric.
- Sleeper DOM drift for waiver/lineup/trade flows: each flow must be re-verified live; never build selectors from memory.

## Alternatives Considered

- Flag-only hardening of the existing `chromiumoxide` launch (no binary patch) — cheaper, may not clear behavioral scoring; research must say whether it suffices.
- Separate Python backend (undetected-chromedriver stock) + thin Rust CLI delegation (handoff option A) — rejected by user in favor of an in-Rust port.
- Adopting an existing Rust stealth crate if research finds a maintained one — preferred over a from-scratch port if fit is good.

## Feasibility Risks

- No known maintained Rust port of undetected-chromedriver; the port surface (binary patch + driver management + stealth flags) is estimated, not measured.
- Chrome auto-update invalidating a pinned patch; driver/Chrome version skew breaking launches.
- Headless-mode detection differing from headed — `status` (headless) may need its own stealth proof.
- Sleeper UI changes invalidating login-state detection (`is_logged_in` selector) or future claim/lineup/trade selectors.

## Observability Requirements

Structured `tracing` logs at backend launch, login-poll transitions, status outcomes, and claim/lineup/trade submissions; error paths log error + profile/Chrome-version context. Preserve the `SLEEPER_BROWSER_DEBUG=1` page-dump aid through the swap. No metrics server or oncall alerts — local single-operator CLI.

## Risk Control

Not gated (single-operator CLI, no flag). Rollback: git revert restores the `chromiumoxide` implementation; keep that path building until the new backend passes the success metric. Rollout: verify fresh-profile → authenticated-profile → waiver-claim in that order; each stage gates the next.

## Amendment (2026-09-08): no-binary-patching reconciliation

The "Chrome binary/launch patching" wording in the original In Scope bullet is superseded: per `research/build-vs-buy.md` + `research/stack.md` (nodriver-style self-launch + `Browser::connect`, no chromedriver binary in this CDP stack), stealth = launch flags + CDP hygiene + CfT pinning only; the CfT Chrome binary is never patched. This reconciliation is on record for the Q-policy sign-off (plan.md Story 2.5 Task 0).

## Open Questions

1. RESOLVED (2026-09-08): patch targets = the launch-flag set + CDP evasion scripts + UA/client-hints bundle — no binary targets. Load-bearing vs cargo-cult analysis applies to flags/scripts only (see amendment note below).
2. New Rust crate vs module inside `src/` — and crate name/layout? (for planning)
3. Replace `chromiumoxide` entirely or keep it as the CDP transport under a stealth launch layer? (for research)
4. Pinned Chrome/driver versions and update cadence? (for planning)
5. Exact `waiver-claim` CLI shape and live-verified selectors? *(unresolved after Phase 2 research — requires live Sleeper UI; carried to plan.md Unresolved Questions, gated on live-UI verification during implementation)*
