# Implementation Plan — undetected-chromedriver-rust

**Repo:** `/home/tstapler/Programming/sleeper-mcp` (run all commands from this root)
**Type:** browser-backend migration — port undetected-chromedriver-style stealth to in-Rust `src/browser/` module; reusable module, NOT a separate repo/service.
**Baseline:** `SleeperBrowser::launch` (`src/browser.rs:30-42`, chromiumoxide 0.9.1 vs `<data_dir>/chrome-profile` from `src/config.rs:40-44`); manual login poll (`src/browser.rs:84-100`); headless status check (`src/cli.rs:339-349`).
**Appetite:** Large (3–6 weeks). Epic 4 may be cut per appetite rule.

## Creative Pass (Step 0.5 — alternatives exploration, chosen + rejected)

1. **A — Nodriver-style self-launch + `Browser::connect` (CHOSEN).** Strength: no chromedriver binary means no `cdc_` patch surface; keeps chromiumoxide 0.9.1 as CDP transport with minimal new deps (`zip` + `libc` + `fs2`). Weakness: still carries chromiumoxide 0.9.1 version-skew risk (WS spam vs pinned Chrome) until CfT pinning + spike land.
2. **B — Flag-only hardening of in-place `Browser::launch`.** Strength: smallest diff, zero new deps. Weakness: prior research finds flag-only is a Phase-1 spike, not a solution — automation fingerprints (Runtime.enable CDP leak, headless deltas) survive flags alone.
3. **C — Adopt/fork external Rust stealth crate or separate Python backend.** Strength: fastest if a maintained crate existed. Weakness: no maintained Rust port exists (`undetected-chromedriver` 0.1.2 and `chromiumoxide_stealth` stale); separate Python service violates in-Rust-only constraint and doubles process/supervision surface.

Rejected B and C are recorded in Pattern Decisions ("Alternative Rejected" / "Reason" columns, non-blank).

## Domain Glossary

| Term | Definition (one sentence) |
|---|---|
| `BrowserSession` | Object-safe trait seam (`goto`/`eval_bool`/`eval_dump`/`close(&mut self)`) called via `Box<dyn BrowserSession>` so fakes swap without the transport. |
| `PinnedVersion` | Newtype for the CfT pin (`$PINNED`); skew checks take `(PinnedVersion, RuntimeVersion)` so operands cannot swap. |
| `FakeSession` | In-memory `BrowserSession` impl exercised by CLI unit tests with no Chrome. |
| `StealthLauncher` | Owner of the real Chrome child-process lifetime: spawn with stealth flags, expose WS endpoint, guarantee teardown. |
| `StealthLayer` | Ordered stealth application: flags → `Browser::connect` → isolated-world bootstrap → UA/client-hints override. |
| `LaunchMode` | Sum type selecting behavior: `HeadedManualLogin` (visible, 10-min poll) or `HeadlessStatus` (`--headless=new`, read-only check). |
| `LaunchConfig` | Builder centralizing flag assembly, profile dir, CfT binary path, and timeouts for a launch. |
| `ChromeProfileDir` | Value object for `Config::chrome_profile_dir()` (`<data_dir>/chrome-profile`); sole persistent profile owner. |
| `ChromeForTesting` | Pinned Chrome binary resolved/downloaded under `Config::data_dir` (e.g. `<data_dir>/cft/<version>/`), checksum-verified. |
| `IsolatedWorld` | CDP `Page.createIsolatedWorld` context used to bootstrap stealth scripts without polluting the page world. |
| `ClientHints` | Coherent `Emulation.setUserAgentOverride` UA + `Sec-CH-UA` hints bundle matching the pinned Chrome version. |
| `ProfileLock` | Lockfile + Chrome `SingletonLock` check preventing a status run from stealing a live headed profile. |
| `ProcessGroup` | `setsid`-spawned child group killed `SIGTERM→SIGKILL` with `Drop` guard plus orphan sweep. |
| `LoginPoll` | Silent 10-minute headed poll loop with `indicatif` spinner/countdown waiting for human login. |
| `LoginSignal` | One of the multi-signal `is_logged_in` checks (no `Log in` button AND session cookie AND avatar/nav marker). |
| `DoctorReport` | `browser doctor` self-test output: pinned/runtime versions, patch hash, lock state, stealth-signal probe results. |
| `DebugDump` | `SLEEPER_BROWSER_DEBUG`-gated page dump (url/title/buttons/stealth signals) to stderr, stdout reserved for verdicts. |
| `StealthFlag` | One Chrome CLI flag with a mandatory one-line policy justification recorded in `src/browser/stealth.rs`. |
| `WaiverClaim` | Post-login automated claim flow executed through `BrowserSession` only after human-authenticated session exists. |

Glossary term count: 19.

## Pattern Decisions

| Component | Chosen Pattern | Alternative Rejected | Reason |
|---|---|---|---|
| CLI↔browser seam (`src/browser/mod.rs`, `session.rs`) | PoEAA Service Layer (`BrowserSession` trait, object-safe, callers use `Box<dyn BrowserSession>`, `close(&mut self)`) + Adapter over chromiumoxide; `BrowserBackend` deleted (one trait suffices for v1) | New crate / separate service; generics (`SleeperBrowser<B>`) / concrete enum | In-scope constraint: reusable in-repo module; `dyn` keeps CLI call sites non-generic; by-value `close(self)` rejected as non-object-safe |
| Layer enforcement (session/launch/cft) | Grep-gates in Story 1.2 Task 6 (session never spawns, launch never evals) | Module privacy only | Grep-gates are CI-checkable today; privacy hardening later |
| Flag assembly (`launch.rs`) | GoF Builder (`LaunchConfig`) + Strategy (`LaunchMode`) | Scattered `if headless` in `cli.rs` | Centralizes per-patch justification gate; headed/headless deltas explicit |
| Stealth scripts (`stealth.rs`) | Type-driven: `StealthFlag` newtype + `ClientHints` value object, pure fns no I/O | Inline string flags in launch call | Pure fns unit-testable; forces one-line justification per flag (policy table) |
| Process lifecycle (`launch.rs`) | RAII guard (sync `Drop` block-and-kill, never await) + explicit async `close→wait→unlock` | Fire-and-forget `Browser::launch` | Today's timeout path leaks; 12-state error×cleanup table requires guaranteed teardown |
| Login check (`session.rs`) | Domain Model multi-signal (`LoginSignal` conjunction, hydration-aware) | Single `button.textContent === 'log in'` probe | DOM drift + hydration races cause false verdicts; multi-signal degrades gracefully |
| CfT resolver (`cft.rs`) | PoEAA Transaction Script (resolve→verify→unzip→cache) with pure URL/hash helpers | Vendor Chrome in git / track system Chrome | Auto-update invalidates pins (top killer); resolver + fail-fast keeps pair reproducible |
| Creative alt B | — (spike only) | Alternative Rejected: flag-only hardening as full solution | CDP `Runtime.enable` leak and headless deltas survive flags; engine-level fix required |
| Creative alt C | — (spike only) | Alternative Rejected: separate Python backend / stale stealth crate | Violates in-Rust-only constraint; `undetected-chromedriver` 0.1.2 / `chromiumoxide_stealth` unmaintained; doubles supervision surface |

## Tech Debt Disposition

| Debt | Disposition | Justification (one sentence) |
|---|---|---|
| chromiumoxide 0.9.1 kept as CDP transport (WS spam vs pinned Chrome) | Isolate via seam | Transport types (`Browser`, `Handler`) owned by `src/browser/launch.rs` (connect/endpoint), `Page` use confined to `src/browser/session.rs`, zero `chromiumoxide::` imports in `src/cli.rs` (AC1 grep-gate enforces). |
| Stock `Browser::launch` (no pinning, no lock, leak on timeout) | Isolate via seam | `StealthLauncher` + `LaunchConfig` replace every call site through the seam, leaving no direct `Browser::launch` outside `launch.rs`. |
| Single-selector `is_logged_in` | Isolate via seam | Multi-signal check lives behind the seam method, so selector drift fixes never touch `cli.rs`. |

## Migration Plan

**N/A — no schema/data changes.** Profile dir (`Config::chrome_profile_dir()`) is untouched and never migrated; CfT binary cache is a new separate directory; rollback is `git revert` to the chromiumoxide path.

## Observability Plan

- `tracing` spans/events at launch (`LaunchConfig` summary: mode, CfT version, profile path), poll (start/tick/timeout), status (verdict + signals), claim boundaries (epic 3+). No new metrics/alerts (local CLI).
- Preserve `SLEEPER_BROWSER_DEBUG`: `eval_dump`/`debug_dump` parity extended with stealth signals (UA, `navigator.webdriver`, `languages`, `plugins.length`, headless markers) to stderr.
- Verdicts to stdout (with `--json` for `status` in Epic 2); progress/spinners to stderr; keep green/yellow/red color contract.
- `browser doctor` (Epic 2) is the version-skew diagnostic: pinned vs runtime Chrome, patch hash, lock state.

## Risk Control

- Not gated; rollout order: fresh-profile login → authenticated login/status → waiver-claim; each gates the next (≥3 consecutive challenge-free runs each for Epic 1).
- Rollback = `git revert` to chromiumoxide path (no migration to undo).
- Top killer (Chrome auto-update/patch drift): CfT pinning + fail-fast + per-release re-validation task in Epic 2.
- Orphans: `setsid` process-group + `SIGTERM→SIGKILL` + sync `Drop` guard + orphan sweep (before lock); every exit path (incl. timeout `Err`, Ctrl-C, panic, SIGKILL-parent via next-launch sweep) covered by 12-state table (Epic 2 task).
- Policy: login 100% manual; fingerprint-removal allowed, interaction-faking forbidden; every `StealthFlag` carries one-line justification; Q-policy owner sign-off gates Epic 1 Stories 1.2+ (Story 2.5 Task 0 records decision fingerprint-removal-only, interaction-faking ban intact, as dated plan artifact — not just a code comment).

## Unresolved Questions

| ID | Question | Owner | Status / Gate |
|---|---|---|---|
| Q-policy | Standing-rule conflict (handoff fixed policy line declining stealth twice) vs requirements.md newly authorizing fingerprint-removal-only stealth | Owner (Tyler) | OPEN — gates Epic 1 Stories 1.2+; resolve via Story 2.5 Task 0 dated sign-off before any stealth code. |
| Q5 | Exact waiver-claim CLI shape + live-verified selectors | Operator via live-UI verification | OPEN — blocks Epic 3 implementation tasks; resolve by driving live sleeper.com UI during Epic 3 (never guessed selectors); record verified selectors + CLI shape in Epic 3 story before coding claim automation. |

## Dependency Visualization

```
                    ┌──────────────┐
                    │   cli.rs     │ thin seam callers (login/status/doctor/claim)
                    └──────┬───────┘
                           │ uses
              ┌────────────▼────────────┐
               │ browser/mod.rs          │ BrowserSession trait (Box<dyn>, object-safe)
               │  ├─ stealth.rs (pure)  │ flags + ClientHints + isolated-world JS
               │  ├─ cft.rs (resolve)   │ CfT pin: reqwest/sha2/zip → <data_dir>/cft/<$PINNED>/
               │  ├─ launch.rs          │ StealthLauncher: setsid, lock, teardown
               │  └─ session.rs         │ chromiumoxide impl: goto/eval/poll/dump
              └────────────┬────────────┘
                   ┌────────▼────────┐  ┌──────────────────┐
                   │ Config          │  │ pinned Chrome    │ (CfT binary, NOT driver)
                   │ chrome_profile_ │  │ real process via │
                   │ dir() (owner)   │  │ Browser::connect │
                   └─────────────────┘  └──────────────────┘
WS-endpoint discovery: parse Chrome stderr `DevTools listening on ws://…` (chosen; `--remote-debugging-port=0` OS-port, no fixed-port contention). Story 1.3 Task 2 implements the parser + 10s deadline.
Deps: keep reqwest/sha2/tokio(full: `signal::ctrl_c`)/tracing/indicatif; ADD `zip` + `libc` (setsid/kill-group via `CommandExt::pre_exec`) + `fs2` (lockfile). NO fantoccini/thirtyfour/webdriver/stale stealth crates.
```

---

## Epic 1 — Seam + stealth launch + login/status parity (spike chaser-oxide first, timeboxed)

**Goal:** `sleeper browser login` + `status` challenge-free on fresh and authenticated profiles (≥3 consecutive runs each) through the new seam; release build keeps chromiumoxide only behind the seam.

### Story 1.1 — Timeboxed chaser-oxide spike (≤1 week, per build-vs-buy)

As an operator, I want a spike verdict on chaser-oxide (maintained chromiumoxide fork) so that version-skew fixes ride a maintained transport if warranted.

**Acceptance criteria:**
- AC1: Spike verdict recorded (adopt vs stay on 0.9.1) with audit notes (canonical repo, license MIT/Apache, commercial orbit watch).
  - Given a spike branch swapping chromiumoxide for chaser-oxide, When the operator runs `cargo build` plus the Story 1.4 parity gate (≥3 runs per profile state) on the spike branch, Then `plan.md` records `adopt` or `stay-on-0.9.1` with commit hash and license check.
- AC2: Spike does not leak into the seam (spike branch only, main stays on 0.9.1).
  - Given the `Cargo.toml` + `Cargo.lock` on main, When `rg chaser Cargo.toml Cargo.lock` runs, Then it returns no matches until a verdict task flips it.
- AC3 (gate): Epic 2 starts on the verdict transport; parity WS/protocol failures flip the verdict to adopt.
  - Given Story 1.4 parity runs failing with WS/protocol errors on 0.9.1, When triage confirms transport skew, Then Story 1.1 verdict flips to `adopt` and Task 5 (migration story) runs before Epic 2.

**Files:** `Cargo.toml`, `src/browser/session.rs`, `project_plans/undetected-chromedriver-rust/implementation/plan.md` (verdict note)

**Tasks (~2–5 min each):**
1. Audit chaser-oxide canonical repo + license + recent commits; append 5-line verdict stub to plan (`Cargo.toml` untouched). Files: `project_plans/undetected-chromedriver-rust/implementation/plan.md`.
2. On spike branch only, swap `chromiumoxide` for chaser-oxide in `Cargo.toml`, `cargo build -q`, record pass/fail. Files: `Cargo.toml`.
3. Run `sleeper browser status` once on spike branch; record WS-error delta vs main. Files: `src/browser/session.rs` (no change, observation only).
4. Write adopt/stay verdict with timebox date; if adopt, file follow-up task, else close spike. Files: `project_plans/undetected-chromedriver-rust/implementation/plan.md`.
5. If verdict is adopt, file sequenced migration story (owner + date + ≥3-run gate per profile state) before Epic 2; else record stay rationale. Files: `project_plans/undetected-chromedriver-rust/implementation/plan.md`.

**Spike verdict (recorded 2026-09-08, main branch — plan artifact only, `Cargo.toml` untouched):**
- Canonical repo: AMBIGUOUS — mirrors at `0xchasercat/chaser-oxide` (275★, ~489 commits), `penumbra-x/chaser-oxide`, `jason-ni/chaser-oxide`; crates.io `chaser-oxide` 0.2.x exists.
- License: MIT/Apache-2.0 dual (`LICENSE-MIT` + `LICENSE-APACHE` present in mirrors) — no license blocker.
- Recent activity: active into 2026 (crates.io 0.2.4, docs.rs builds) — no staleness blocker.
- Verdict: **stay-on-0.9.1** for Epic 1. Rationale: (a) Q-policy conflict — chaser-oxide ships a "Human Interaction Engine" (physics mouse/typing), which is interaction-faking and banned; (b) canonical-repo ambiguity + commercial orbit (`chaser.sh`) watch; (c) the `BrowserSession` seam isolates the transport, so a later adopt is a cheap session.rs swap.
- Timebox: spike-branch Tasks 2–3 (`Cargo.toml` swap build + `status` WS-error delta) deferred to an operator-gated spike branch; NOT run on main. Gate holds: `rg chaser Cargo.toml Cargo.lock` empty on main (AC2).

### Story 1.2 — `BrowserSession` seam + module skeleton

As a maintainer, I want `cli.rs` programming against a `BrowserSession` trait so that chromiumoxide is isolated behind a seam.

**Acceptance criteria:**
- AC1: `cli.rs` contains no direct `chromiumoxide::` import outside `src/browser/`.
  - Given the `BrowserSession` trait in `src/browser/mod.rs`, When `rg "chromiumoxide::" src/cli.rs src/client.rs` runs, Then output is empty.
- AC2: Existing `login`/`status` behavior unchanged (delegates through seam).
  - Given a `LaunchConfig` with `LaunchMode::HeadlessStatus`, When `sleeper browser status` runs on an authenticated profile at `<data_dir>/chrome-profile` (default test overlay: `cache/chrome-profile`), Then exit code is `0` and stdout contains `logged in`.
- AC3: Seam is programmable without Chrome (FakeSession gate).
  - Given a `FakeSession` impl of `Box<dyn BrowserSession>` in tests, When `cargo test -q browser::fake_session` runs with no Chrome installed, Then all tests pass and CLI login/status paths compile against `dyn` only.

**Files:** `src/browser/mod.rs`, `src/browser/session.rs`, `src/browser/launch.rs`, `src/browser/stealth.rs`, `src/cli.rs`, `src/lib.rs`

**Tasks:**
1. Create `src/browser/mod.rs` with object-safe `BrowserSession` (`goto`/`eval_bool`/`eval_dump`/`close(&mut self)`, callers use `Box<dyn BrowserSession>`); delete speculative `BrowserBackend` (one trait suffices for v1). Files: `src/browser/mod.rs`, `src/lib.rs`.
2. Move current `SleeperBrowser` body into `src/browser/session.rs` as the chromiumoxide impl (no logic change). Files: `src/browser/session.rs`, `src/browser/mod.rs`.
3. Convert `src/browser.rs` to `mod.rs` re-export shim (or delete + update `lib.rs`); `cargo build -q`. Files: `src/browser.rs`, `src/lib.rs`, `src/browser/mod.rs`.
4. Thin `cli.rs` login/status call sites to seam methods only. Files: `src/cli.rs`.
5. Grep-gate: `rg chromiumoxide src/cli.rs` empty; `cargo build -q`. Files: `src/cli.rs`.
6. Layer grep-gates: `rg 'std::process|Command::|tokio::process' src/browser/session.rs src/browser/stealth.rs` empty AND `rg 'evaluate|eval_bool|eval_dump' src/browser/launch.rs src/browser/cft.rs` empty. Files: `src/browser/session.rs`, `src/browser/launch.rs`.
7. Add `FakeSession` impl + CLI-level unit test with no Chrome (proves seam programmable without transport). Files: `src/browser/session.rs`, `src/cli.rs`.

### Story 1.3 — `StealthLauncher`: self-launch + `Browser::connect` + coherent hints

As an operator, I want Chrome self-launched with stealth flags and isolated-world bootstrap so that login/status runs challenge-free.

**Acceptance criteria:**
- AC1: No chromedriver binary; headed login launches visible Chrome and connects.
  - Given a `LaunchConfig` with `LaunchMode::HeadedManualLogin` and profile `<data_dir>/chrome-profile` (default test overlay: `cache/chrome-profile`), When `sleeper browser login` runs, Then a visible Chrome window opens and `Browser::connect` attaches within `10s` via the stderr-parsed WS endpoint.
- AC2: Stealth signals coherent (webdriver false, UA matches `$PINNED`).
  - Given a connected `BrowserSession` on `PinnedVersion($PINNED)`, When `eval_bool("navigator.webdriver === undefined || navigator.webdriver === false")` runs, Then result is `true` and `eval_dump` UA contains `Chrome/<$PINNED-major>`.
- AC3: CDP-minimal transport hygiene measured (flags alone are not the claim).
  - Given WS-event counts captured before and after hygiene on the same profile state, When eval batching + reduced poll cadence + deferred debugger attach land, Then WS `Runtime.enable`/event count drops vs baseline and the delta is recorded in the plan (or the stealth claim is downgraded to flag-only with re-derived ACs).

**Files:** `src/browser/launch.rs`, `src/browser/stealth.rs`, `src/browser/session.rs`, `src/browser/mod.rs`

**Tasks:**
1. Add `LaunchMode` enum + `LaunchConfig` builder with flag assembly (headed/headless paths). Files: `src/browser/launch.rs`, `src/browser/mod.rs`.
2. Implement `StealthLauncher::spawn` (resolved CfT binary from Story 2.1; system Chrome only via explicit flag, never silent fallback; `--remote-debugging-port=0` + stealth flags), parse WS endpoint from Chrome stderr `DevTools listening on ws://…` with `10s` deadline, return endpoint. Files: `src/browser/launch.rs`.
3. Switch `session.rs` connect path to `Browser::connect(endpoint)`; keep `Browser::launch` deleted from non-test code. Files: `src/browser/session.rs`.
4. Add pure `stealth.rs` fns: flag list + `ClientHints` bundle + isolated-world bootstrap JS (`Page.createIsolatedWorld` + `addScriptToEvaluateOnNewDocument` + `Emulation.setUserAgentOverride`). Files: `src/browser/stealth.rs`.
5. Wire bootstrap after connect; unit-test pure flag assembly (no I/O). Files: `src/browser/stealth.rs`, `src/browser/session.rs`.
6. Add CDP-minimal hygiene: batch evals, lower poll cadence, defer debugger attach until post-connect; record WS-event counts before/after. Files: `src/browser/session.rs`, `src/browser/launch.rs`.
7. Manual verify headed login opens + attaches; record run 1/3. Files: `src/browser/launch.rs`.

### Story 1.4 — Login/status parity runs (≥3 consecutive challenge-free each)

As an operator, I want parity proof on fresh + authenticated profiles so that Epic 1 gates Epic 2.

**Acceptance criteria:**
- AC1: 3 consecutive fresh-profile logins challenge-free.
  - Given an empty dir `<data_dir>/chrome-profile-fresh` (default test overlay: `cache/chrome-profile-fresh`), When `sleeper browser login` runs 3 times (wiping between runs 1–2 only per procedure), Then each run reaches the manual-login prompt with no CAPTCHA loop before human action.
- AC2: 3 consecutive authenticated `status` runs report logged-in with zero orphans.
  - Given an authenticated `<data_dir>/chrome-profile` (default test overlay: `cache/chrome-profile`), When `sleeper browser status` runs 3 times in a row, Then each prints logged-in and `pgrep -f "chrome.*chrome-profile"` shows no survivors after each run.
- AC3: Loop-break proof — full manual logins end-to-end, no challenge recurrence (pre-mortem failure #1).
  - Given a fresh profile, When the operator completes one FULL manual login end-to-end (human types credentials, solves one CAPTCHA by hand) on each of ≥3 runs, Then no run presents a second challenge after the first solve, and a 24h-later `sleeper browser status` on the surviving session still reports logged-in (session-survival check).
  - STOP-GATE: if the challenge loop persists on any run, do not proceed to Epic 2/3 — record the loop evidence in the plan and return to the stealth-layer design.
- AC4: Fast path — already-logged-in skips the poll (ux.md Surface A).
  - Given an authenticated `<data_dir>/chrome-profile` (default test overlay: `cache/chrome-profile`), When `sleeper browser login` runs, Then `is_logged_in()` is checked first, stdout prints exactly `Already logged in — session persisted from a previous run.`, no 10-minute poll or spinner starts, the browser is closed before exit, and exit code is `0`.
- AC5: Headless-vs-headed verdict agreement ×3 on the same profile (pre-mortem failure #2).
  - Given one authenticated profile, When `sleeper browser status` (headless) and a headed login-check run alternately 3 times on that same profile, Then all 6 verdicts agree; headless verdicts are not trusted until this gate passes, and `sleeper browser status --headed` remains available as the escape hatch for headless-divergent verdicts.

**Files:** `src/cli.rs`, `src/browser/session.rs`, `src/browser/launch.rs`

**Tasks:**
1. Run fresh-profile login ×3, log results table in plan. Files: `project_plans/undetected-chromedriver-rust/implementation/plan.md`.
2. Run authenticated status ×3 + orphan check (`pgrep`), log results. Files: `project_plans/undetected-chromedriver-rust/implementation/plan.md`.
3. Fix any parity break found (one task per break, new tasks appended, no placeholders). Files: `src/browser/session.rs`, `src/browser/launch.rs`, `src/cli.rs`.
4. AC3 loop-break proof: complete ≥3 FULL manual logins end-to-end (one CAPTCHA solve each, no second challenge) + 24h session-survival `status` check; log the run table; enforce the stop-gate before Epic 2/3. Files: `project_plans/undetected-chromedriver-rust/implementation/plan.md`.
5. AC4 fast path: check `is_logged_in()` before starting the poll; print the exact one-line verdict, close the browser, exit 0. Files: `src/cli.rs`, `src/browser/session.rs`.
6. AC5 agreement gate: run headless-vs-headed verdict agreement ×3 on the same profile; add the `status --headed` escape hatch. Files: `src/cli.rs`, `src/browser/launch.rs`.

**Parity runs (PENDING-OPERATOR — no live Chrome/network in this env; do not fabricate):**

| Task | Run | Profile | Result | Challenge loop? | Orphans (`pgrep`)? |
|---|---|---|---|---|---|
| 1.4.1 fresh login | 1/3 | `<data_dir>/chrome-profile-fresh` (wipe before) | PENDING | PENDING | PENDING |
| 1.4.1 fresh login | 2/3 | fresh (wipe before) | PENDING | PENDING | PENDING |
| 1.4.1 fresh login | 3/3 | fresh (keep — feeds AC3) | PENDING | PENDING | PENDING |
| 1.4.2 auth status | 1/3 | `<data_dir>/chrome-profile` | PENDING | n/a | PENDING |
| 1.4.2 auth status | 2/3 | same | PENDING | n/a | PENDING |
| 1.4.2 auth status | 3/3 | same | PENDING | n/a | PENDING |
| 1.4.4 loop-break | full login ×3 + 24h `status` survival | fresh → surviving | PENDING | STOP-GATE if loop persists | PENDING |
| 1.4.6 agreement | headless-vs-headed ×3 (6 verdicts) | one authenticated | PENDING | n/a | PENDING |

Code-complete in Epic 1: AC4 fast path (`Already logged in — session persisted from a previous run.`, close, exit 0) + `status --headed` escape hatch both implemented in `src/cli.rs` (unit-guarded copy); live agreement table above gates headless trust.

---

## Epic 2 — Hardening: doctor, CfT pinning, cleanup, multi-signal login, UX

### Story 2.1 — CfT pinned resolver + fail-fast version diagnostics

As a maintainer, I want Chrome/driver pins reproducible so that auto-update never silently invalidates stealth.

**Acceptance criteria:**
- AC1: Pinned CfT cached + checksum-verified under data dir.
  - Given a `PinnedVersion($PINNED)` in config, When the resolver runs with empty `<data_dir>/cft/`, Then `<data_dir>/cft/<$PINNED>/chrome` exists and sha256 matches the CfT dashboard `downloads.chrome[].sha256` field for the downloaded zip (source-of-truth; inner-binary hash never used).
- AC2: Version skew fails fast with triage line (no silent fallback).
  - Given a `RuntimeVersion` vs `PinnedVersion($PINNED)` mismatch, When `sleeper browser status` runs, Then stderr contains `version skew` + `re-run with update` triage and exit code is `2`; a failed CfT download never silently falls back to system Chrome.
- AC3: Zip traversal + download discipline enforced.
  - Given a crafted zip with `../evil` entries, When the extractor runs, Then it rejects the archive and `cargo test -q cft::traversal` passes; partial downloads write to `*.part` + atomic rename on success.

**Files:** `src/browser/cft.rs`, `src/browser/mod.rs`, `src/browser/launch.rs`, `Cargo.toml`, `src/config.rs`

**Tasks:**
1. Add `zip` to `Cargo.toml`; pure typed helpers `cft_url(PinnedVersion)` + `cft_path(data_dir, PinnedVersion)` + unit test. Files: `Cargo.toml`, `src/browser/cft.rs`.
2. Implement download→sha256 (vs dashboard field)→unzip→cache (Transaction Script) using reqwest/sha2/zip with `30s` connect/`120s` total timeouts, 3 retries, offline error + disk-full triage, `*.part` temp + atomic rename. Files: `src/browser/cft.rs`.
3. Add zip-slip traversal guard (reject absolute/`..` entries, confine to `<data_dir>/cft/<$PINNED>/`) + traversal-hardening unit test. Files: `src/browser/cft.rs`.
4. Wire `LaunchConfig` to resolved CfT binary; fail-fast on skew with triage line; explicit no-silent-fallback rule (failed download = error exit, never system Chrome). Files: `src/browser/launch.rs`, `src/browser/cft.rs`.
5. Bump procedure (owner Tyler, cadence tied to CfT stable releases, re-run ≥3-run gate per profile state after each bump) + fallback trigger (nodriver-style transport replacement if pass-rate drops below gate). Files: `project_plans/undetected-chromedriver-rust/implementation/plan.md`.

### Story 2.2 — Process-group cleanup + profile lock (12-state table)

As an operator, I want zero orphans and no profile theft so that status never corrupts a live login.

**Acceptance criteria:**
- AC1: Timeout path leaves no Chrome behind.
  - Given a `LaunchMode::HeadedManualLogin` session with `10s` timeout, When `wait_for_manual_login(10s)` expires, Then `pgrep -f chrome-profile` is empty and the lockfile is released.
- AC2: Status aborts (v1) when a headed session holds the profile.
  - Given a live headed login holding `<data_dir>/chrome-profile/SingletonLock` (default test overlay: `cache/chrome-profile/SingletonLock`), When `sleeper browser status` runs, Then stdout/stderr contains `profile busy — close the login window first` and exit code is `3` without touching the profile.
- AC3: Ctrl-C cancels the wait with copy + exit 130 and no orphans (ux.md §2 Ctrl-C path).
  - Given a `LaunchMode::HeadedManualLogin` session mid-poll, When the operator presses Ctrl-C, Then stderr contains `login wait cancelled — closing browser…`, the Chrome process group is closed, and the process exits `130`.

**Files:** `src/browser/launch.rs`, `src/browser/session.rs`, `src/cli.rs`

**Tasks:**
1. Add `libc` (setsid/kill-group via `CommandExt::pre_exec`) + `fs2` (lockfile) to `Cargo.toml`; spawn with `setsid` process-group; sync `Drop` does block-and-kill (`SIGTERM→SIGKILL`, never await) while async `close()` owns `wait→unlock`. Files: `Cargo.toml`, `src/browser/launch.rs`.
2. Add `ProfileLock` (`fs2` lockfile + respect `SingletonLock`, order sweep→lock→spawn); status aborts with message for v1. Files: `src/browser/launch.rs`, `src/cli.rs`.
3. Fill 12-state error×cleanup table (timeout Err, Ctrl-C via `tokio::signal::ctrl_c` with exit-130 + `login wait cancelled` copy, chrome-missing, no-display, close-fail/orphan-cleanup-fail, lock-busy, panic, SIGKILL-parent, partial-spawn-after-lock, launch-fail-after-lock-acquired × headed/headless) with a test per row (SIGKILL-parent recovered by next-launch sweep). Files: `src/browser/launch.rs`, `src/browser/session.rs`.
4. Orphan sweep helper (runs before lock) + verify `pgrep` empty after kill. Files: `src/browser/launch.rs`.

### Story 2.3 — Multi-signal `is_logged_in` (hydration-aware) + debug parity

As an operator, I want login verdicts robust to DOM drift so that status does not flap.

**Acceptance criteria:**
- AC1: Verdict is `LoggedIn`/`LoggedOut`/`Unknown` conjunction of ≥3 signals with bounded hydration.
  - Given a `BrowserSession` on `https://sleeper.com` with live-verified signals (verified during Epic 1 implementation; avatar selector marked live-verify, never assumed), When `is_logged_in()` runs within `2s` of navigation, Then it polls every `250ms` up to a `5s` deadline (headed and headless same bound) and returns `Unknown` on signal split/timeout instead of guessing.
- AC2: `Unknown` has an exit/triage contract + per-signal logging.
  - Given an `Unknown` verdict, When `sleeper browser status` runs, Then stderr logs which signal fired/missed per signal, stdout/stderr contains `unknown — re-run status; re-login only if persistent` triage, and exit code is `1`-distinct (`3` reserved lock-busy; unknown uses `4`).
- AC3: `SLEEPER_BROWSER_DEBUG=1` dumps stealth signals to stderr.
  - Given `SLEEPER_BROWSER_DEBUG=1`, When `sleeper browser status` runs, Then stderr contains `webdriver:` and `ua:` lines and stdout still holds only the verdict.

**Files:** `src/browser/session.rs`, `src/cli.rs`

**Tasks:**
1. Implement tri-state check (no-`Log in` button + session cookie/localStorage marker + avatar/nav marker, all live-verified) with `250ms` poll / `5s` deadline hydration wait. Files: `src/browser/session.rs`.
2. Extend `eval_dump`/`debug_dump` with stealth signals (UA, webdriver, languages, plugins) + per-signal fired/missed log line + the `DEBUG browser:` fields from ux.md Surface G (pinned/runtime chrome versions, flag-set hash, CDP port, profile path, cleanup/reaped PIDs). Files: `src/browser/session.rs`, `src/cli.rs`.
3. Live-verify logged-in vs logged-out vs unknown verdicts; record DOM notes + re-baselining procedure for login signals on Sleeper drift (selector bundle refreshed live, never guessed). Files: `src/browser/session.rs`.

### Story 2.4 — UX: spinner, timeout copy, `--json` status, triage lines

As an operator, I want silent polls visible and errors actionable so that I know whether to wait, re-run, or update.

**Acceptance criteria:**
- AC1: 10-min poll shows spinner + countdown; timeout copy says re-run `login`.
  - Given a `LoginPoll` with `600s` timeout, When `sleeper browser login` waits `15s` without login, Then stderr shows a spinner with `9m45s remaining` and on expiry contains `rerun \`sleeper browser login\`` (not `status`).
- AC2: `status --json` emits machine-readable verdict + exit codes (ux.md Surface D contract).
  - Given an authenticated profile, When `sleeper browser status --json` runs, Then stdout is exactly the Surface D object `{"logged_in":true,"chrome_version":"<pinned>","chrome_runtime_version":"<runtime>","profile":"<data_dir>/chrome-profile"}` with exit `0`; logged-out gives `{"logged_in":false,…}` exit `1`; skew exit `2`; lock-busy exit `3`; unknown exit `4`. Transport/check failure exits non-zero with `{"logged_in":false,"error":"…"}` shape, never a bare stack trace on stdout.
- AC3: Every version-skew/patch/lock error prints wait-vs-rerun-vs-update triage.
  - Given a skew error for runtime vs `PinnedVersion($PINNED)`, When it prints, Then the line contains one of `wait` / `re-run` / `update` directing the next action.
- AC4: Headless-vs-headed agreement gate before headless verdicts are trusted (pre-mortem failure #2).
  - Given one authenticated profile, When `sleeper browser status` (headless) and a headed check disagree, Then the operator re-runs via the `status --headed` escape hatch and the headless verdict is not trusted until the Story 1.4 AC5 ×3 agreement gate passes.

**Files:** `src/cli.rs`, `src/browser/session.rs`

**Tasks:**
1. Add `indicatif` spinner + countdown to `LoginPoll`; progress→stderr only. Files: `src/cli.rs`, `src/browser/session.rs`.
2. Fix timeout copy to `rerun login`; keep `Session persisted for future runs.` verbatim. Files: `src/cli.rs`, `src/browser/session.rs`.
3. Add `--json` + exit codes 0/1/2/3/4 (unknown=4) to `status`; keep color contract. Files: `src/cli.rs`.
4. Add triage-line contract to each error path. Files: `src/cli.rs`, `src/browser/launch.rs`.
5. Implement the `status --headed` escape hatch (headed launch running the same `is_logged_in` verdict path) for headless-divergent verdicts. Files: `src/cli.rs`, `src/browser/launch.rs`.
6. Add the challenge-loop tool-news copy to the poll path: recurrence is framed as tool news, never operator blame (`Login page is still showing a challenge after N polls. This is unexpected with the stealth backend — solve it once by hand if you can; if it loops, file the CfT Chrome pinned/runtime versions from \`sleeper browser status --debug\`.`). Files: `src/cli.rs`, `src/browser/session.rs`.

### Story 2.5 — Policy reconciliation + per-patch justification gate + `browser doctor`

As a maintainer, I want policy and diagnostics explicit so that fingerprint-removal stays allowed and interaction-faking stays out.

**Acceptance criteria:**
- AC1: Policy table recorded + enforced (every `StealthFlag` justified, no interaction-faking).
  - Given a `StealthFlag` `--disable-blink-features=AutomationControlled`, When `rg -A1 StealthFlag src/browser/stealth.rs` runs, Then the next line is a `// justification:` comment and `rg "synthetic.*keystroke|input.*faker" src/browser/` is empty.
- AC2: `browser doctor` reports versions, stealth-set hash, lock, and probe results (NO binary patching; single contract shared with ux.md Surface E).
  - Given `PinnedVersion($PINNED)` and a free profile, When `sleeper browser doctor` runs, Then stdout contains `chrome pinned <$PINNED>`, `chrome runtime <version>`, `flag set <sha8-of-stealth-flag-set>` (hash of the applied flag/patch-script set — never a Chrome-binary patch; UC patched chromedriver, never Chrome), `profile <dir> (unlocked)`, and `headless login check: <verdict>` — one `✓`/`✗` line per check exactly as ux.md Surface E specifies.

**Files:** `src/browser/stealth.rs`, `src/cli.rs`, `src/config.rs`

**Tasks:**
0. Q-policy owner sign-off gating Epic 1 (date + owner Tyler + decision fingerprint-removal-only + interaction-faking ban intact) recorded as dated plan artifact/decision log before any stealth code. Files: `project_plans/undetected-chromedriver-rust/implementation/plan.md`.
1. Write policy table (allowed fingerprint-removal vs forbidden interaction-faking) as header comment + plan entry. Files: `src/browser/stealth.rs`, `project_plans/undetected-chromedriver-rust/implementation/plan.md`.
2. Add one-line `// justification:` to every flag; add grep-gate test/task. Files: `src/browser/stealth.rs`.
3. Implement `sleeper browser doctor` emitting `DoctorReport` (pinned/runtime versions, stealth-set hash, lock, probes). Files: `src/cli.rs`, `src/browser/mod.rs`.
4. NO binary patching rule: stealth = launch flags + CDP hygiene + CfT version pinning only; `patch:` is defined as the stealth-flag/patch-script set hash (recompute on flag change); CfT Chrome binary is never patched in place or copied. Files: `src/browser/cft.rs`, `src/browser/stealth.rs`.

---

## Epic 3 — Waiver-claim through the seam (GATED on live-UI verification / Q5)

**Gate:** Do not code claim automation until Q5 is resolved by driving live sleeper.com: record exact CLI shape + verified selectors in this epic first.

### Story 3.1 — Q5 resolution: live-verified claim selectors + CLI shape

As an operator, I want the claim flow verified against the live UI so that automation never relies on guessed selectors.

**Acceptance criteria:**
- AC1: Verified selector bundle + CLI shape recorded from live UI.
  - Given live `https://sleeper.com` with a test league `league_id=123456`, When the operator walks the claim flow by hand during implementation, Then the plan records `claim-button=[data-testid="claim-btn"]`, `confirm=[data-testid="confirm-claim"]`, and CLI `sleeper waiver claim --league 123456 --player 9999`.
- AC2: If DOM differs, story stops (no guessed fallback).
  - Given a changed DOM with no `claim-btn`, When verification runs, Then no claim code is written and Q5 stays OPEN with fresh DOM notes.

**Files:** `project_plans/undetected-chromedriver-rust/implementation/plan.md`, `src/cli.rs` (shape stub only after verification)

**Tasks:**
1. Drive live UI by hand; capture URLs, testids, hydration timing. Files: `project_plans/undetected-chromedriver-rust/implementation/plan.md`.
2. Freeze CLI shape (`waiver claim --league --player [--drop]`) + selector bundle in plan. Files: `project_plans/undetected-chromedriver-rust/implementation/plan.md`.
3. Extend `BrowserSession` with small generic primitives only (`click_selector`, `wait_for_selector`) after AC1; claim logic lives in flow modules, never as `goto_league`/`claim_player` trait methods. Files: `src/browser/mod.rs`, `src/browser/session.rs`.

### Story 3.2 — Waiver claim automation (post-Q5)

As an operator, I want `waiver claim` through the seam on a human-authenticated session.

**Acceptance criteria:**
- AC1: Claim submits and confirms via verified selectors with tracing boundaries.
  - Given an authenticated `BrowserSession` and league `123456` with droppable player `9999`, When `sleeper waiver claim --league 123456 --player 9999` runs, Then stdout contains `claim submitted` and tracing spans `claim.start`/`claim.confirm` exist.
- AC2: Failure surfaces triage + leaves profile clean.
  - Given an invalid player `0000`, When the claim runs, Then stderr contains a triage line and `pgrep` shows no orphan.

**Files:** `src/cli.rs`, `src/browser/session.rs`, `src/browser/mod.rs`

**Tasks:**
1. Implement claim flow on seam using only verified selectors. Files: `src/browser/session.rs`, `src/browser/mod.rs`.
2. Add `waiver claim` CLI wiring + tracing boundaries. Files: `src/cli.rs`.
3. Live-verify on test league; record ≥3 runs. Files: `project_plans/undetected-chromedriver-rust/implementation/plan.md`.

### Story 3.1/3.2 — Q5-OPEN gate note (recorded 2026-09-08, this session — no live Chrome/network in this env)

- **Q5 stays OPEN.** No live sleeper.com verification has occurred in this environment. The operator must walk the claim flow by hand on live `https://sleeper.com` (test league `league_id=123456`), capturing URLs, `data-testid`s, and hydration timing before any claim automation is coded.
- **EXPECTED-but-UNVERIFIED shape (UNVERIFIED-DO-NOT-CODE-AGAINST):** `claim-button=[data-testid="claim-btn"]`, `confirm=[data-testid="confirm-claim"]`, CLI `sleeper waiver claim --league 123456 --player 9999`. These strings appear ONLY in this note — never as `querySelector` args or eval strings in code.
- **AC2 stop rule:** if the live DOM differs (no `claim-btn`), no claim code is written and Q5 stays OPEN with fresh DOM notes recorded here.
- **Implemented this session (stub only):** `waiver claim --league --player [--drop]` clap skeleton (`src/cli.rs`) prints the `Q5 not yet verified — claim automation disabled` triage to stderr + exits non-zero, never attempts automation; `BrowserSession` extended with generic primitives `click_selector`/`wait_for_selector` only (`src/browser/mod.rs`, `FakeSession` support in `src/browser/session.rs`); seam-side claim flow skeleton with `claim.start`/`claim.confirm` spans returns the `Q5-unverified` error with no selector use (`src/browser/session.rs`); CLI wiring calls it and closes the browser on every branch (no orphan); dead sessions surface the yellow `Not logged in — run \`sleeper browser login\`` hint (ux.md §3), never a CDP dump.
- **≥3-runs table — PENDING-OPERATOR (no fabricated results):**

| Run | League | Player | Result | Triage? | Orphans (`pgrep`)? |
|---|---|---|---|---|---|
| claim 1/3 | 123456 | 9999 | PENDING (gated on Q5) | PENDING | PENDING |
| claim 2/3 | 123456 | 9999 | PENDING (gated on Q5) | PENDING | PENDING |
| claim 3/3 | 123456 | 9999 | PENDING (gated on Q5) | PENDING | PENDING |

---

## Epic 4 — Lineups + trades (MAY BE CUT per appetite rule)

Cut rule: if Epics 1–3 exceed week 4, defer this epic; seam already supports it.

### Story 4.1 — Lineups through the seam

As an operator, I want lineup changes through the same backend.

**Acceptance criteria:**
- AC1: Lineup set submits on verified selectors.
  - Given an authenticated `BrowserSession` and league `123456`, When `sleeper lineup set --league 123456 --slot WR --player 9999` runs, Then stdout contains `lineup saved`.

**Files:** `src/cli.rs`, `src/browser/session.rs`, `src/browser/mod.rs`

**Tasks:**
1. Live-verify lineup selectors; record bundle. Files: `project_plans/undetected-chromedriver-rust/implementation/plan.md`.
2. Implement lineup flow on seam + CLI. Files: `src/browser/session.rs`, `src/cli.rs`.

### Story 4.2 — Trades through the seam

As an operator, I want trade proposals through the same backend.

**Acceptance criteria:**
- AC1: Trade proposal submits on verified selectors.
  - Given an authenticated `BrowserSession` and league `123456`, When `sleeper trade propose --league 123456 --give 9999 --get 8888` runs, Then stdout contains `trade proposed`.

**Files:** `src/cli.rs`, `src/browser/session.rs`, `src/browser/mod.rs`

**Tasks:**
1. Live-verify trade selectors; record bundle. Files: `project_plans/undetected-chromedriver-rust/implementation/plan.md`.
2. Implement trade flow on seam + CLI. Files: `src/browser/session.rs`, `src/cli.rs`.
3. Final release check: `cargo build --release`, zero orphans, `SLEEPER_BROWSER_DEBUG` intact. Files: `src/cli.rs`, `src/browser/launch.rs`.

### Epic 4 cut decision (recorded 2026-09-08, this session)

- **Decision: DEFER implementation (cut per appetite rule).** Epics 1–3 represent the full appetite in this session, and Q5 is OPEN so lineup/trade selectors are live-unverified — the "ample room AND verifiable selectors" condition fails on both counts.
- **Stub-only parity implemented:** `lineup-set --league --slot --player` and `trade-propose --league --give --get` clap stubs (`src/cli.rs`) print the disabled triage to stderr + exit non-zero; through-seam stub signatures only (`lineup_set_via_seam` / `trade_propose_via_seam` in `src/browser/session.rs`), no automation; dead sessions surface the yellow `Not logged in — run \`sleeper browser login\`` hint (ux.md §3). The intended post-verification shapes (`sleeper lineup set ...` / `sleeper trade propose ...` as subcommand groups) are deferred — they collide with the existing analysis `lineup`/`trade` commands and require a CLI restructure at implementation time.
- **Operator steps remaining:** live-verify lineup + trade selector bundles; lift stubs to full flows; final release check — `cargo build --release` was NOT run in this env (no live Chrome/network), plus zero-orphan check and `SLEEPER_BROWSER_DEBUG` intact verification.

---

## Spike verdict log (Story 1.1 — append-only, newest first)

### 2026-09-08 — Q-policy owner sign-off (Story 2.5 Task 0 — gates Epic 1 Stories 1.2+)
- Owner: Tyler. Date: 2026-09-08. Decision: fingerprint-removal-only; interaction-faking ban intact.
- Standing-rule conflict resolved: the handoff fixed policy line (declining stealth twice) is superseded for fingerprint-removal only — removing automation fingerprints (flags, UA/client-hints coherence, CfT pin) is allowed; scripting human input (synthetic keystrokes, input faking, credential fill, CAPTCHA solving) stays forbidden. Login remains 100% manual.
- Gate cleared: stealth code (Stories 1.2+) may proceed under this decision.

### 2026-09-08 — CfT bump procedure (Story 2.1 Task 5)
- Owner: Tyler. Cadence: bump `PINNED_CHROME_VERSION` tied to CfT stable releases only (never auto-update).
- After each bump: re-run the ≥3-run parity gate per profile state (Story 1.4 AC1/AC2) before trusting the new pin.
- Fallback trigger: if challenge-free pass-rate drops below the parity gate on a pinned release, file a nodriver-style transport-replacement story (per plan Dependency Visualization) instead of widening flags.

### 2026-09-08 — chaser-oxide audit (Task 1, main branch, `Cargo.toml` untouched)
- Canonical repo: `github.com/ccheshirecat/chaser-oxide` (crates.io owner `ccheshirecat` points here); identical-star mirror `0xchasercat/chaser-oxide` confirms the fork-network smell — pin canonical only.
- License: MIT OR Apache-2.0 (fits repo). Version 0.2.4 (2026-04-20), created 2026-01-01, ~269★/45 forks, ~14.9k total downloads; young (8 mo) + `chaser.sh` commercial halo — watch, no tripwire hit.
- INTERIM verdict: `stay-on-0.9.1` pending live parity. Main gate holds: `rg chaser Cargo.toml Cargo.lock` empty; baseline `cargo build -q` passes on main (Chrome 146 present, DISPLAY=:1).
- DEFERRED to live operator (Tasks 2–4): spike-branch swap + `status` WS-delta + ≥3-run parity need a human manual login; dirty tree also blocks a clean spike branch. Per AC3, WS/protocol parity failures on 0.9.1 flip this to `adopt` + Task 5 migration story before Epic 2.
- GATE: Q-policy sign-off still OPEN — no stealth code (Stories 1.2+) until Tyler signs off fingerprint-removal-only.
