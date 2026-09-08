# UX Design: undetected-chromedriver-rust

Date: 2026-09-08 | Status: design (pre-implementation) | Operator: single (Tyler, Linux)
Sources: `project_plans/undetected-chromedriver-rust/requirements.md`,
`project_plans/undetected-chromedriver-rust/research/ux.md`,
`src/cli.rs:165-190, 314-350`, `src/browser.rs`.

Framing (non-negotiable): login stays **100% manual** — the human types
credentials and solves any CAPTCHA/verification in the visible window, on
purpose. Nothing in this design scripts, hints at scripting, or times the
human step. The stealth backend only changes *how Chrome launches*, never
*who logs in*.

Color contract (kept, not extended with new colors): green = authenticated /
persisted, yellow = actionable unauthenticated, red = error, dim = transient
progress. `console` auto-strips on non-TTY / `NO_COLOR` — no extra work.

Stream contract: final verdict (`Logged in.` / `Not logged in…`) → **stdout**
(pipeable, one line). Progress / spinner / backend internals → **stderr**.
`SLEEPER_BROWSER_DEBUG=1` dump → stderr (existing behavior, extended).

## 1. User-facing surfaces (condensed)

### Surface A — `sleeper browser login`, fast path (already logged in)

Sample (stdout):

```
Already logged in — session persisted from a previous run.
```

(green via `console::style(...).green()`)

Acceptance criteria:

- Fast path checks `is_logged_in()` first and exits without starting the
  10-minute poll or spinner.
- Verdict is exactly one line on stdout, green; nothing on stderr except
  debug-gated output.
- Browser process group is closed before exit (no orphaned window).
- Exit code 0.

### Surface B — `sleeper browser login`, wait flow (interactive — full treatment in §2)

Sample (stdout once, then live spinner on stderr):

```
Log into Sleeper by hand in the window that just opened — including any CAPTCHA. This step isn't automated, on purpose; waiting up to 10 minutes…
⠋ waiting for manual login… (2m31s elapsed, 7m29s left)
```

Acceptance criteria: see §2 + §4 (spinner countdown, early-exit success,
corrected timeout copy directing to `login`).

### Surface C — `sleeper browser status` (human default)

Sample (stdout, one line):

```
Logged in.
```

(green) or:

```
Not logged in — run `sleeper browser login`.
```

(yellow)

Acceptance criteria:

- Verdict stays exactly one line on stdout; green vs. yellow per contract.
- No spinner, no poll — headless read-only check completes in seconds.
- Progress/backend detail (if any) goes to stderr, never stdout.
- Browser closed on all branches, including `is_logged_in` transport errors.
- Exit codes (matching implementation/plan.md Story 2.4): 0 logged-in /
  1 not-logged-in / 2 version-skew / 3 profile-lock-busy / 4 check-unknown,
  so scripts can gate on the cause. Unknown always exits 4, never 1 or 3.

### Verdict enum → copy map (`LoggedIn` / `LoggedOut` / `Unknown`)

| Variant | Stdout (one line) | Stderr | Exit |
|---|---|---|---|
| `LoggedIn` | `Logged in.` (green) | progress only | 0 |
| `LoggedOut` | `Not logged in — run \`sleeper browser login\`.` (yellow) | progress only | 1 |
| `Unknown` (transport error) | nothing | `Login check failed (<detail>) — re-run \`sleeper browser status\`.` (red) | 4 |
| `Unknown` (signal split/timeout) | nothing | `Login check unknown (<which-signal>) — re-run \`sleeper browser status\`; re-login only if persistent.` (red) | 4 |

### Surface D — `sleeper browser status --json` (proposed, additive)

Sample (stdout, pure JSON, no styling):

```json
{"logged_in":true,"chrome_version":"126.0.6478.126","chrome_runtime_version":"126.0.6478.126","profile":"/home/tyler/.local/share/sleeper/chrome-profile"}
```

Acceptance criteria:

- Flag is additive: default human output byte-identical when flag absent.
- stdout carries only the JSON object (pipeable to `jq`); human verdict and
  spinner (if any) go to stderr or are suppressed.
- Object always contains `logged_in`, `chrome_version` (pinned CfT), `chrome_runtime_version` (actually launched runtime),
  `profile`; versions are `"unknown"` rather than omitted when undetectable.
- Exit code 0 logged-in / 1 not-logged-in / 2 skew / 3 lock-busy /
  4 unknown, enabling
  `sleeper browser status --json || sleeper browser login` gates.
- Transport/check failure exits non-zero with `{"logged_in":false,"error":"…"}`
  shape, never a bare stack trace on stdout.

### Surface E — `sleeper browser doctor` (proposed self-test)

Sample (stdout/stderr split — checks to stdout, hints to stderr):

```
✓ chrome pinned 126.0.6478.126 at <data_dir>/cft/126.0.6478.126/chrome (match)
✓ chrome runtime 126.0.6478.126 (match, skew-free)
✓ stealth flag set a1b2c3d4 applied (hash match, no binary patching)
✓ profile /home/tyler/.local/share/sleeper/chrome-profile (unlocked)
✓ headless login check: logged in
```

Acceptance criteria:

- One `✓`/`✗` line per check (chrome pinned/runtime, flag-set hash, profile, login check);
  each `✗` line names the fix + exact re-run command (no dead ends).
- Read-only where possible: never deletes the profile dir; reports reaped
  orphan PIDs instead of silently killing.
- Non-zero exit on any `✗`; zero only when all checks pass.
- Reuses Surface F message shapes for identical causes (single copy source).

### Surface F — error / triage messages (all failures)

Sample (stderr, red):

```
CfT Chrome runtime v127 vs pinned v126 — version skew. Re-run `sleeper browser login` to re-resolve the pin; if it persists, update the pin to 127.
```

Acceptance criteria:

- Every error names what happened + whose action fixes it + which command —
   wait vs. re-run vs. update are never conflated (skew/flag-set = update, never
   "wait").
- Raw CDP/anyhow detail follows `—` or hides behind `SLEEPER_BROWSER_DEBUG`;
  the triage line always comes first.
- Timeout copy directs to `login`, never `status` (fixes current
  `src/browser.rs:96-99` misdirection).
- Challenge-loop recurrence is framed as tool news, not operator failure
  (see state 8 below).

### Surface G — `SLEEPER_BROWSER_DEBUG` dump (existing aid, extended)

Sample (stderr):

```
DEBUG dump: {"url":"https://sleeper.com/","title":"Sleeper","buttons":[{"text":"Log In","visible":true}]}
DEBUG browser: chrome pinned v126.0.6478.126, runtime v126.0.6478.126, flag set a1b2c3d4, cdp port 41233, profile /home/tyler/.local/share/sleeper/chrome-profile, cleanup: reaped pid 0
```

Acceptance criteria:

- Var name `SLEEPER_BROWSER_DEBUG` unchanged; existing page-dump line kept.
- Adds one `DEBUG browser:` line with pinned/runtime chrome versions, flag-set hash, CDP port,
  profile path, cleanup actions.
- All output to stderr; stdout verdict lines unaffected.
- Never prints session cookies, tokens, or credential-adjacent values.

## 2. Login wait flow (full treatment — the only interactive surface)

### ASCII flow: operator action ↔ CLI response

```
OPERATOR                        CLI (stdout = ○, stderr = ●)
────────                        ─────────────────────────────
$ sleeper browser login
                                ○ (launch headed Chrome, open sleeper.com)
                                ○ Already logged in — session persisted
                                    from a previous run.  [green, exit 0]
                                    ── OR slow path: ──
                                ○ Log into Sleeper by hand in the window that
                                  just opened — including any CAPTCHA. This
                                  step isn't automated, on purpose; waiting
                                  up to 10 minutes…
                                ● ⠋ waiting for manual login… (0m00s elapsed,
                                  10m00s left)   [dim spinner, stderr]
        │ types credentials,
        │ solves CAPTCHA by hand
        │ in the visible window
        ▼
  [poll tick every 5s: spinner message
   updates elapsed/left on stderr only —
   no per-tick stdout lines]
                                ● ⠙ waiting for manual login… (2m31s elapsed,
                                  7m29s left)
        │ login completes
        ▼
  [next poll tick sees session]
                                ● ✔ Logged in. Session persisted for future runs.
                                    [green finish_with_message, stderr+stdout]
                                ○ Logged in. Session persisted for future runs.
                                    [green, stdout verdict — VERBATIM sentence]
                                     [close Chrome process group, exit 0]

  ── OR timeout path (no login in 600s): ──
                                ● ✘ still waiting stopped — no authenticated
                                  session detected after 10m [abandon spinner]
                                ○ Still not logged in after 10m — finish logging
                                  in in the browser window, then re-run
                                  `sleeper browser login`. [yellow, stdout]
                                     [close headed Chrome process group, exit non-zero]

  ── OR Ctrl-C path: ──
        │ presses Ctrl-C
        ▼
                                ● login wait cancelled — closing browser…
                                     [close Chrome process group, exit 130]
```

Notes:

- The ask prints **once** (stdout); liveness lives on the spinner (stderr),
  updated per 5s poll tick — matching the existing poll cadence, no log spam.
- Success uses `spinner.finish_with_message(green …)`; timeout uses
  `abandon_with_message` so scrollback shows the outcome after the spinner dies.
- Verbatim sentence preserved: `Session persisted for future runs.` appears in
  the success verdict exactly as today — it *is* the durability promise (one
  manual login → green `status` across runs).
- Timeout copy fix: current code says `then rerun \`sleeper browser status\``
  (`src/browser.rs:96-99`); `status` is a read-only check and cannot fix a
  missed login. New copy directs to `sleeper browser login`.
- stdout stays pipeable throughout: only the ask + final verdict touch stdout;
  spinner/countdown/progress stay on stderr (same `ProgressStyle` spinner family
  as `cmd_sync_stats` / `do_sync`).

### Error / edge handling (12-state table: 1–8 from research/ux.md §5, 9–12 from implementation/plan.md Story 2.2)

| # | State | Detection point | CLI response (color + next command) | Cleanup |
|---|---|---|---|---|
| 1 | Chrome missing | launch, before Chrome spawn | red (stderr) `Chrome not found at <path>. Install Chrome stable <pinned> or set CHROME_PATH. See \`sleeper browser status\` after installing.` — no spinner started, exit non-zero; no stdout verdict | nothing to clean; do not create half-profile |
| 2 | CfT Chrome skew | CfT resolve / runtime-vs-pin check | red (stderr) `CfT Chrome runtime vX vs pinned vY — version skew. Re-run \`sleeper browser login\` to re-resolve the pin; if it persists, update the pin to <runtime>.` (never "wait"); exit 2; no stdout verdict | kill spawned Chrome process group before exit |
| 3 | Stealth flag-set mismatch | launch, flag-set hash check | red (stderr) `Stealth flag set <sha8> mismatches expected <sha8> against CfT Chrome vX. Update sleeper and retry; if it persists, pin CfT Chrome to <last-known-good>.` exit non-zero; no stdout verdict | kill Chrome process group; leave profile untouched |
| 4 | Profile locked | launch (SingletonLock / lockfile) | yellow/red (stderr) `Profile <dir> is locked — another Chrome window is probably open. Close it and re-run \`sleeper browser login\`. Cleaned up orphan PID <n> automatically.` `login` exits non-zero; `status` aborts with `profile busy — close the login window first` and exit 3; no stdout verdict on either | reap orphans, report PIDs; never delete profile |
| 5 | Timeout, no login | poll deadline (600s) | yellow (stdout verdict + stderr abandon) `Still not logged in after 10m — finish logging in in the browser window, then re-run \`sleeper browser login\`.` exit non-zero | **close headed browser** (today's path skips `close()` — must fix) |
| 6 | Orphan cleanup failure | close/kill path | red (stderr) `Could not stop browser PID <pid> — run \`kill -9 <pid>\` then re-run \`sleeper browser login\`.`; exit non-zero; no stdout verdict | log reaped vs. surviving PIDs |
| 7 | `status` check ambiguous / unknown | `is_logged_in` evaluate error or signal split/timeout | red (stderr) `Login check failed (<detail>) — re-run \`sleeper browser status\`.` for transport errors; red (stderr) `Login check unknown (<which-signal>) — re-run \`sleeper browser status\`; re-login only if persistent.` for `Unknown` verdicts, exit 4; both distinct from yellow `Not logged in — run \`sleeper browser login\`.` (exit 1) | close headless browser in all branches |
| 8 | No display / headless-only env | headed `login` launch | red (stderr) `No display detected — \`sleeper browser login\` needs a visible window. Run on a machine with a display (or SSH -X) and retry.` — do not start doomed 10-min poll; exit non-zero; no stdout verdict | exit before poll starts |
| 9 | Panic mid-session | Rust panic while Chrome alive | sync `Drop` guard block-and-kills the process group (`SIGTERM→SIGKILL`, never await); the panicking process itself exits non-zero (panic) with no stdout verdict. Next launch prints dim (stderr) `Previous run ended unexpectedly — cleaned up browser PID <pid>. Continuing with launch…` then proceeds with its own normal flow/exit code | Drop-guard kill owns cleanup; next-launch sweep reaps anything the guard missed and reports PIDs |
| 10 | SIGKILL-parent (CLI killed -9) | no in-process handler possible; recovery only on next launch | killed process has no exit path (dies by signal, no verdict, no cleanup). Next launch runs the orphan sweep before locking and prints dim (stderr) `Cleaned up orphan browser PID <pid> from killed run.` then proceeds with its own normal flow/exit code; stdout verdict untouched | next-launch sweep reaps orphans, reports each reaped PID; never deletes profile |
| 11 | Partial-spawn-after-lock | lock acquired, Chrome spawn fails | red (stderr) `Chrome launch failed (<detail>) — profile lock released. Re-run \`sleeper browser login\`.` exit non-zero; no stdout verdict; no spinner left running | release the lockfile/`SingletonLock`; kill any half-spawned child; never leave a poisoned lock |
| 12 | Launch-fail-after-lock-acquired | lock acquired + spawn succeeded, later launch step fails (flag-set check / handshake / session-new) | red (stderr) `Launch failed (<step>: <detail>) — profile lock released, browser stopped. Re-run \`sleeper browser login\`.` exit non-zero, fail loud — never a silently held lock; no stdout verdict | release lock, kill Chrome process group, log reaped PIDs |

Cross-cutting: every `login`/`status` exit path (success, timeout, Ctrl-C,
panic, error, states 9–12) closes or kills the Chrome process group (zero-orphan
criterion). Sync `Drop` does block-and-kill (`SIGTERM→SIGKILL`, never await)
while async `close()` owns `wait→unlock`; launch order is sweep→lock→spawn so
a SIGKILL-parent or panic can only be recovered by the next launch's sweep.
Prefer a `Drop`-guard / scopeguard over per-return `close().await` — today's
leak (timeout `Err` propagates from `cmd_browser_login` without `close()`) is
the exact bug shape to design out. Challenge/CAPTCHA still looping
post-migration is state-8-adjacent copy, framed as tool news: `Login page is
still showing a challenge after N polls. This is unexpected with the stealth
backend — solve it once by hand if you can; if it loops, file the
Chrome pinned/runtime versions from \`sleeper browser status --debug\`.` — never
operator blame.

## 3. Downstream surfacing

Future `waiver-claim` / lineup / trade commands drive the same backend. When
they detect a dead session they surface the yellow Surface C hint themselves
(`Not logged in — run \`sleeper browser login\``) rather than dumping CDP
errors — so after the first green `status`, the browser disappears from the
operator's mental model and `status` becomes the pre-deadline ritual.

## 4. Testable UX acceptance criteria

1. Operator completes one manual login with a live countdown visible: spinner
   on stderr shows elapsed + remaining, updated per 5s poll tick; no per-tick
   stdout lines.
2. Early-exit success: logging in at minute 2 finishes the spinner with a
   green `finish_with_message` within ~5s (one poll interval), printing the
   verbatim verdict `Logged in. Session persisted for future runs.` on stdout.
3. Timeout at 600s abandons the spinner and prints a yellow verdict directing
   to `sleeper browser login` (never `status`); headed browser is closed.
4. Every error names what happened + whose action + which command — no dead
   ends: each of the 12 states in §2 prints its triage line with the exact
   re-run command before any raw detail; progress/errors stay on stderr and
   no failure path prints a stdout verdict.
5. `status` verdict stays one line on stdout (green `Logged in.` / yellow
   `Not logged in — run \`sleeper browser login\`.`); all progress and
   backend internals go to stderr.
6. `status --json` emits a single JSON object on stdout with
   `logged_in/chrome_version/chrome_runtime_version/profile`; exit 0 logged-in / 1
   not-logged-in / 2 skew / 3 lock-busy / 4 unknown; human default output
   unchanged when flag absent.
7. Check-failed vs. not-logged-in are distinguishable: transport errors print
   red `Login check failed … re-run \`sleeper browser status\``; `Unknown`
   verdicts print red `Login check unknown … re-run \`sleeper browser
   status\`; re-login only if persistent.` with exit 4; negative auth
   prints yellow `Not logged in — run \`sleeper browser login\`` with
   exit 1; lock-busy aborts with exit 3.
8. Zero orphans: success, timeout, Ctrl-C, panic (via Drop-guard kill),
   and all 12 error states leave no Chrome processes (verified via
   process table); SIGKILL-parent is recovered by the next launch's sweep,
   which reports each reaped PID.
9. `SLEEPER_BROWSER_DEBUG=1` preserves the existing page-dump line and adds
   chrome pinned/runtime/flag-set/CDP-port/profile/cleanup context, all on stderr, with
   no secrets.
10. 100%-manual-login framing holds everywhere: ask line keeps "by hand …
    including any CAPTCHA. This step isn't automated, on purpose"; no copy
    suggests automating credentials or CAPTCHA.
11. Panic mid-session: the Drop guard kills the Chrome process group without
    awaiting; the next launch reports the cleanup (`Previous run ended
    unexpectedly — cleaned up browser PID <pid>.`) on stderr and proceeds
    normally with no poisoned lock.
12. SIGKILL-parent: the next launch's sweep runs before locking and reports
    each reaped orphan PID (`Cleaned up orphan browser PID <pid> from
    killed run.`) on stderr; exit code follows the fresh launch outcome.
13. Partial-spawn-after-lock: Chrome spawn failure after lock acquisition
    releases the lock, kills any half-spawned child, prints a red
    lock-released + re-run line on stderr, exits non-zero, and leaves no
    poisoned lock for the next run.
14. Launch-fail-after-lock-acquired: any later launch-step failure with the
    lock held releases the lock, stops the browser, prints a red fail-loud
    lock-released + re-run line on stderr, exits non-zero — a silently held
    lock is never an acceptable end state.
