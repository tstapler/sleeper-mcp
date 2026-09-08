# Pre-mortem: undetected-chromedriver-rust
**Date**: 2026-09-08
## Failure Modes
| # | Failure | First Symptom | Prevention | Severity |
|---|---------|---------------|------------|----------|
| 1 | Stealth clears `navigator.webdriver`/flag rows but Sleeper's behavioral/score-based challenge loop persists — shipped, problem unsolved. | Fresh-profile login reaches the manual prompt cleanly, but after the human solves one CAPTCHA a second challenge appears (Story 1.4 run 1/3 already looping). | Plan.md Story 1.4: add AC3 "loop-break proof" — ≥3 runs must each complete one full manual login end-to-end plus a 24h session-survival check, with an explicit stop-gate (do not proceed to Epic 2/3) if the loop persists. | P1 |
| 2 | Headless `status` diverges from headed `login` — green status while login loops, so the operator trusts a lie on waiver night. | `sleeper browser status` reports logged-in while a headed `login` on the same profile immediately shows a challenge (or vice versa); `doctor` probes still pass. | Plan.md Stories 1.4/2.4: add headless-vs-headed verdict-agreement gate (×3 on the same profile) plus a `status --headed` escape hatch before headless verdicts are trusted. | P1 |
| 3 | CfT pin goes stale (or re-download fails) and fail-fast exit 2 bricks launches the night waivers clear. | `status` exits 2 with `version skew` on waiver night after a pin bump or failed CfT download; `doctor` shows pinned≠runtime and no launch is possible. | Plan.md Story 2.1 AC2: on skew/download failure fall back to the last-known-good cached CfT binary (keep N-1 pins cached) with a loud warning — exit 2 only if no cached binary launches; never fail onto system Chrome. | P2 |
| 4 | Ctrl-C orphan/stale-lock poisoning: a killed run leaves `SingletonLock`/lockfile behind and every later `status` aborts "profile busy" with no Chrome alive. | After Ctrl-C during `login`, all later `status` runs exit 3 `profile busy — close the login window first` while `pgrep -f chrome-profile` is empty. | Plan.md Story 2.2 Task 2: lock-busy path must validate the holder PID is alive before aborting — stale holder → sweep, remove stale lock, and proceed with a warning. | P2 |
| 5 | `is_logged_in` AND-conjunction false-negatives flap to `Unknown`, prompting needless re-logins that burn the warm profile. | `status` flaps logged-in → unknown → logged-in across consecutive runs with no human action; stderr shows 1/3 signals missed near the 5s hydration deadline. | Plan.md Story 2.3 AC1: change AND-conjunction to scored quorum (2/3 = LoggedIn, 0/3 = LoggedOut, else Unknown) and harden the exit-4 copy to forbid suggesting re-login on `Unknown`. | P2 |
## P1 Items (address before implementation)
- [ ] <failure #1> — add Story 1.4 AC3 loop-break proof (full manual login ×3 + 24h survival) with stop-gate before Epic 2
- [ ] <failure #2> — add headless-vs-headed verdict-agreement gate (×3) plus `status --headed` escape hatch

## Evaluated, not top-5
- Chaser-oxide spike verdict wrong: rejected — Story 1.1 AC3 already flips the verdict on WS/protocol failures, and fork commercial-rot is a >1-month horizon, not a first-month killer.
- WS-endpoint stderr-parse fragility (`DevTools listening` format change): possible but the log line has been stable for years; covered by Story 1.3 Task 2's deadline error path. Watch-item only.
