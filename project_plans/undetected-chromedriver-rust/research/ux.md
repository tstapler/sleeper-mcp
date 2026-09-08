# UX Research: undetected-chromedriver-rust (Agent 5)

Date: 2026-09-08 | Scope: single-operator CLI (`sleeper browser login` / `status`)
Sources: `src/cli.rs:319-350`, `src/browser.rs:1-127`, `src/config.rs:40-44`, `src/error.rs`, existing `console` + `indicatif` usage.

## 1. Current baseline (what the operator sees today)

**`sleeper browser login`** (`src/cli.rs:326-337`):
1. Launches visible Chrome via `SleeperBrowser::launch(profile_dir, false)`.
2. Fast path: `is_logged_in()` → green `Already logged in — session persisted from a previous run.`
3. Slow path: plain `println!("Log into Sleeper by hand in the window that just opened — including any CAPTCHA. This step isn't automated, on purpose; waiting up to 10 minutes…")`, then silent 10-min poll (`wait_for_manual_login`, 5s interval, `src/browser.rs:84-100`).
4. Success: green `Logged in. Session persisted for future runs.` Failure: `Error::Other("still not logged in after 600s — finish logging in in the browser window, then rerun \`sleeper browser status\`")`.

**`sleeper browser status`** (`src/cli.rs:339-350`): headless launch, optional `SLEEPER_BROWSER_DEBUG` dump to stderr, then green `Logged in.` / yellow `Not logged in — run \`sleeper browser login\`.`

**Gaps found (evidence-backed):**
- The 10-minute wait is **silent** — no spinner, countdown, or elapsed-time feedback. Every other long operation in the CLI uses `indicatif::ProgressBar` with `{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len}` (`cmd_sync_stats`, `do_sync`). The login wait is the only long blocking UX without one.
- Timeout error is the only re-run guidance, and it points at `status` (read-only check) rather than `login` (the fix). If the operator actually timed out without logging in, the actionable next step is `sleeper browser login` again, not `status`.
- `is_logged_in()` failure vs. not-logged-in are indistinguishable — any `Error::Other` from CDP/evaluate propagates as a raw anyhow string with no actionable prefix (no `Error` variant for browser failures; `src/error.rs` has `Api/RateLimited/NotFound/Http/Database/Json/Join/Io/Other` only).
- No `--json` / machine-readable mode anywhere in the CLI; all browser output is human-styled `console::style` strings. Scripting `status` (e.g., gate waiver runs on login) currently requires string-matching.

## 2. Comparable CLI UX patterns for manual-login-wait flows

The correct family is **device-code / manual-auth-wait flows** (gh `auth login`, aws sso, tailscale login, `fly auth`), not progress bars:

| Pattern | How it applies here |
|---|---|
| **Print the ask once, then show liveness** | Keep the single instruction line, then start an `indicatif` spinner (already in deps) with elapsed time: `waiting for manual login… (2m31s elapsed, 7m29s left)`. Update message per poll tick, not per second — matches the 5s poll cadence, avoids log spam. |
| **Countdown + explicit deadline** | Today's "waiting up to 10 minutes…" states the budget but never shows it counting down. Spinner message should show remaining time so the operator can decide to keep waiting vs. abort. |
| **Early-exit on success** | Already works (poll returns `Ok` immediately). Add `spinner.finish_with_message(green "Logged in…")` so success is visually distinct from timeout — consistent with `pb.finish_with_message("Done")` in sync flows. |
| **Timeout message = diagnosis + next command** | Follow the repo's existing convention (`resolve_rid`: `"No roster found for '…' in league …. Run \`sleeper sync …\` first"`). Timeout should name what was observed ("no authenticated session detected after 10m") + the exact re-run command. Current text misdirects to `status`; should direct to `login`. |
| **Keep stdin free / Ctrl-C safe** | Poll loop must remain interruptible and still close the browser (zero-orphan requirement). Today's `browser.close().await` on the success path is fine; ensure the Ctrl-C / timeout paths also close before exit — currently the timeout `Err` propagates out of `cmd_browser_login` **without** calling `close()`, leaking the headed window. UX implication: operator aborts, window stays open, next `login` hits a locked profile (see §5). |

Recommendation: reuse `indicatif` spinner style already in the tree (no new dep), `console::style` colors already in the tree (no new dep).

## 3. Operator mental model: challenge-free vs. challenge loop

**Today's model (broken):** operator logs in by hand, solves one CAPTCHA, expects persistence — but the next launch re-challenges (automation fingerprint in chromiumoxide launch). The symptom ("CAPTCHA solved but keeps looping") is indistinguishable from "I did the login wrong," so the operator blames themselves and retries the human step that was never the problem.

**Target model (stealth backend):** one manual login → durable profile → `status` stays green across runs. The UX must teach this in one line:
- `login` success copy should state durability explicitly (it already does: "Session persisted for future runs." — **keep this sentence verbatim** post-migration so the ≥3-run success criterion is visible in the output itself).
- `status` is the cheap confidence check before waiver/lineup/trade automation. Its output is the operator's pre-flight signal; keep it to one line, keep the green/yellow color contract.

**Version-mismatch / patch-failure errors — the wait / re-run / update triage:**
The new backend adds failure modes the operator has never seen (Chrome auto-updated under the pinned driver, stealth patch failed to apply, chromedriver port taken). Each error must answer three questions in order: **what happened, whose action fixes it, which command to run**. Proposed message contract (all via new `Error::Browser`-style variant or prefixed `Other`, stderr, red via `console`):

| Cause | Operator action | Message shape |
|---|---|---|
| Chrome binary missing / wrong channel | install or point at Chrome | `Chrome not found at <path>. Install Chrome stable <pinned> or set CHROME_PATH. See \`sleeper browser status\` after installing.` |
| Driver/Chrome version skew | re-run (auto re-pin) or update Chrome | `Chrome vX vs driver vY — mismatch. Re-run \`sleeper browser login\` to re-pin the driver; if it persists, update Chrome to <pinned>.` (never "wait" — waiting can't fix skew) |
| Stealth patch failed to apply | update `sleeper` or Chrome | `Stealth patches failed against Chrome vX (patch set vY). Update sleeper and retry; if it persists, pin Chrome to <last-known-good>.` |
| Profile locked (previous window still open / orphan) | close window or re-run after cleanup | `Profile <dir> is locked — another Chrome window is probably open. Close it and re-run \`sleeper browser login\`. Cleaned up orphan PID <n> automatically.` |
| Timeout with no login | re-run `login` (NOT `status`) | `Still not logged in after 10m — finish logging in in the browser window, then re-run \`sleeper browser login\`.` (fix the current misdirection at `src/browser.rs:96-99`) |
| Challenge/CAPTCHA still appearing post-migration | informational, not operator fault | `Login page is still showing a challenge after N polls. This is unexpected with the stealth backend — solve it once by hand if you can; if it loops, file the Chrome/driver versions from \`sleeper browser status --debug\`.` |

Key distinction to encode in copy: **wait** (login poll, patch download) vs. **re-run** (timeout, lock, transient CDP) vs. **update** (version skew, patch failure). Never emit a bare anyhow/CDP error for these — wrap with the triage line first, raw detail after `—` or behind `SLEEPER_BROWSER_DEBUG`.

## 4. Terminal-output clarity

- **Color contract (keep, extend):** green = authenticated/persisted (`Already logged in…`, `Logged in…`), yellow = actionable unauthenticated (`Not logged in — run …`), red = errors via `Error` Display, dim = transient progress (spinner, `Syncing league settings…`). New backend messages must reuse these, not invent new colors. Respect `console`'s auto `NO_COLOR`/non-TTY stripping — already free via the crate.
- **stdout vs. stderr:** human status lines go to stdout today (`println!`); the debug dump already goes to stderr (`eprintln!("DEBUG dump: …")`, `src/browser.rs:123`). Keep that split: progress/spinner → stderr (so stdout stays pipeable), final verdict (`Logged in.` / `Not logged in…`) → stdout. Backend internals (driver version, patch set, CDP port, cleanup actions) → stderr, gated verbose by extending the existing `SLEEPER_BROWSER_DEBUG` env aid (requirement: aid preserved — keep the var name, add `driver v…, chrome v…, profile …` lines to the dump).
- **Machine-readable `status`:** additive `--json` (or `--format json`) flag on `browser status` only — e.g. `{"logged_in":true,"chrome_version":"…","driver_version":"…","profile":"…"}` on stdout, exit code 0 logged-in / 3 (or non-zero) not-logged-in so `sleeper browser status --json || sleeper browser login` gates scripts. Do NOT restyle existing human output; downstream scripting is new surface, default stays human.
- **Spinner discipline:** use the same `ProgressStyle` template family as sync commands for visual consistency; `finish_with_message` on success, `abandon_with_message` on timeout so scrollback shows the outcome. No per-poll `println!` — one live line.

## 5. Error states needing graceful handling (UX × cleanup table)

| # | State | Detection point | UX (message + color + next command) | Cleanup requirement |
|---|---|---|---|---|
| 1 | Chrome missing | launch, before driver spawn | red `Chrome not found…` + install/pin guidance; exit non-zero, no spinner started | nothing to clean; do not create half-profile |
| 2 | Driver/Chrome skew | driver handshake / session new | red mismatch line (wait→never; re-run/update); include both versions | kill spawned driver process before exit |
| 3 | Stealth patch failure | patch-apply step | red patch-failure line + last-known-good Chrome version | kill driver + browser; leave profile untouched (don't corrupt persisted login) |
| 4 | Profile locked | launch (SingletonLock) | yellow/red lock line naming `<data_dir>/chrome-profile` (from `Config::chrome_profile_dir`) + "close the other window" | attempt orphan reaping, report PIDs reaped; never delete the profile dir |
| 5 | Timeout, no login | poll deadline (600s) | yellow timeout line directing to `login` (fix current `status` misdirection) | **close headed browser** — today's early-return/Err paths skip `close()`; the migration must close-or-kill on every exit path |
| 6 | Orphan cleanup failure | close/kill path | red warning naming PID + `kill -9 <pid>` fallback instruction; still exit non-zero so scripts don't assume clean state | log what was reaped vs. what survived |
| 7 | `status` headless ambiguous | `is_logged_in` evaluate error | distinguish "check failed" (red, re-run `status`) from "not logged in" (yellow, run `login`) — today both collapse; new `Error` variant should separate transport errors from negative auth result | close headless browser in both branches |
| 8 | No display / headless-only env | headed `login` launch | red `no display` line suggesting SSH-X/real display or postponing login; don't start a 10-min doomed poll | exit before poll starts |

Cross-cutting: every `login`/`status` exit path (success, timeout, Ctrl-C, error) must close or kill browser + driver (zero-orphan success criterion). Prefer `Drop`-guard / scopeguard over remembering `close().await` at each return — today's leak (timeout path skips `close`) is the exact bug shape to design out.

## 6. Job-to-be-done lens

- **Functional job:** "Log in once, by hand, and never think about auth again — so waiver/lineup/trade automation just runs." Success = `login` once → `status` green forever (until Sleeper expires the session). UX corollary: after first green `status`, the browser should disappear from the operator's mental model; `status` output stays one line, and downstream commands should surface the yellow `Not logged in — run \`sleeper browser login\`` hint themselves when they detect a dead session rather than dumping CDP errors.
- **Emotional job:** "Confidence the session won't challenge-loop at 11:55pm before waivers clear." The anxiety is time-bound (waiver deadlines) and blame-ambiguous (is it me or the tool?). UX corollary: (a) `status` is the pre-deadline ritual — it must be fast (headless, no 10-min wait) and unambiguous (green vs. yellow, plus `--json` for scripted gates); (b) any challenge-loop recurrence must be framed as tool news, not operator failure ("unexpected with the stealth backend… file versions"), with the versions printed by the same command; (c) first-run-after-migration copy should explicitly promise the new model in one clause, e.g. keep `Session persisted for future runs.` and add nothing longer — the durability claim *is* the emotional payoff.
- **Standing policy constraint:** login stays 100% manual — no credential/CAPTCHA scripting. The UX must never hint at automation of that step ("type credentials… solve any CAPTCHA… by hand" framing in `src/browser.rs:74-83` docs and the `Login` clap help at `src/cli.rs:180-182` already sets this; keep both wordings through the migration).
