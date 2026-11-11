# UX Design: waiver-intelligence

**Surface**: `sleeper` CLI (clap) + MCP tools consumed by an AI assistant. No GUI — every
surface below is a condensed entry (sample + acceptance criteria), per research/ux.md's own
finding that this feature has zero interactive/wireframe-shaped surfaces. "Loading" means "a
browser launch is in flight" (several seconds); there is no other async wait in this feature.

Voice/format baseline (do not redesign, match exactly): `triage_error()` triage-first/raw-detail-
after-`—` ([login.rs:26-32](../../../../src/cli/browser/login.rs#L26-L32)); `DoctorReport::render()`'s
`✓`/`✗`-per-line-with-fix ([doctor.rs:20-26](../../../../src/cli/browser/doctor.rs#L20-L26));
`PendingTransaction::render()`'s compact bracket-status line
([pending.rs:31-41](../../../../src/cli/browser/pending.rs#L31-L41)); `maybe_sync()`'s silent-
unless-something-changed convention ([sync.rs:293-314](../../../../src/cli/sync.rs#L293-L314));
MCP's `"LIVE WRITE ACTION — <blunt-what-it-does> ... Returns an error (never a false success) if
..."` description prefix ([mcp.rs:516](../../../../src/mcp.rs#L516)).

Worked example used throughout: league `1352883872131481600`, roster 5, transaction_id `"1"`
(Jacoby Brissett, player_id `3257`, QB, pending waiver claim, bid $78) — the same example
plan.md's Given-When-Thens use.

---

## 1. CLI `sleeper transactions` — cache read (Story 2.3.1)

```
$ sleeper transactions --league 1352883872131481600 --max-age-secs 3600
[pending] waiver leg 9 — add Jacoby Brissett (QB - NE) (bid $78), drop none
[complete] waiver leg 8 — add D. Johnson (RB - DEN) (bid $12), drop K. Walker (RB - SEA)
[cancelled] waiver leg 7 — add M. Evans (WR - TB) (bid $0), drop none
```

No rows: `no cached transactions` (stdout, matches `pending.rs`'s `no pending waiver claims`
empty-state shape).

**Interaction flow**: `--max-age-secs N` sets `StalenessTolerance::MaxAge(N)`; omitted defaults
per-row to the status-branch policy (terminal rows: `CachedOnly`, effectively infinite — see
Surface 7; `pending` rows: live-checked only inside the Tuesday processing window, §ux.md
research). `--force` forces `StalenessTolerance::AlwaysLive` for every row regardless of status
(triggers Surface 10's browser-launch notice). A cache miss for a never-synced league also forces
one live backfill regardless of flag.

**Acceptance criteria**:
- Running with a warm cache and every row's status satisfying the tolerance prints every cached
  row via `render()` and performs zero browser launches.
- `--force` always launches a browser once (even if every row is already within tolerance) and
  prints Surface 10's dim progress line to stderr before the rows print to stdout.
- An empty cache prints `no cached transactions`, not an empty string or a panic.
- Output is one line per transaction, newest-leg-first, identical field order to
  `PendingTransaction::render()` (`[status] kind leg N — add X (bid $N), drop Y`).
- stdout carries only data rows; any staleness/refresh narration goes to stderr (dim/cyan), never
  interleaved into stdout (breaks piping to `grep`/`jq`).

---

## 2. MCP `get_transaction_history` tool (Story 2.3.2)

```json
{
  "league_id": "1352883872131481600",
  "transactions": [
    {
      "transaction_id": "1",
      "kind": "waiver",
      "status": "pending",
      "leg": 9,
      "waiver_bid": 78,
      "adds": ["Jacoby Brissett (QB - NE)"],
      "drops": [],
      "source": "graphql",
      "fetched_at": "2026-11-10T14:02:11Z",
      "status_updated": 1781900000
    }
  ]
}
```

**Interaction flow**: same `read_through` call as Surface 1, called with
`{ league_id, kind_filter, max_age_secs }`. Read-only — no "LIVE WRITE ACTION" prefix in its tool
description, since it can serve entirely from cache.

**Clock note**: `fetched_at` and `status_updated` are different clocks for different purposes —
`fetched_at` is an ISO-8601 timestamp marking when this tool's own cache last wrote the row;
`status_updated` is the raw GraphQL epoch field marking when Sleeper itself last changed the
transaction's status. Worth stating explicitly, since this JSON has no surrounding prose for an
LLM assistant to infer the distinction from.

**Acceptance criteria**:
- Every row carries `source` (`"rest"` or `"graphql"`) and `fetched_at`, per research/ux.md §1's
  structured-not-prose requirement — never a pre-rendered sentence.
- `kind_filter: "waiver"` returns only `kind == "waiver"` rows; omitted returns all three kinds
  (`waiver`, `trade`, `free_agent`).
- The tool never triggers a live browser call implicitly for terminal-status rows — only
  `max_age_secs: 0` (or a future explicit force param) does, consistent with Surface 1.
- A parse failure on the underlying GraphQL response (schema drift) returns `Err` with the exact
  `pending_failed_triage()`-style message, not a partial/empty JSON array presented as complete.

---

## 3. CLI `sleeper waiver bid-projection` — short line (Story 3.2.1)

```
$ sleeper waiver bid-projection --league 1352883872131481600 --player 3257 --roster 5
suggested bid $23 — Brissett QB tier 2 (search_rank 48), league pays ~$18-28 for tier-2 QB
waivers, roster 5 has ~$142 FAAB left (verified), has filed 6 QB claims this season
```

With `--full`: prints the same JSON shown in Surface 4.

**Interaction flow**: wraps `project_waiver_bid` (Epic 3.1). Reads cached signals only — no
browser launch, ever (bid-projection is pure analysis over already-synced data plus the existing
`enrich_player` dynasty signal).

**Acceptance criteria**:
- Default output is exactly one line, matching the `render()` short-form from Story 3.1.4 — no
  multi-paragraph explanation by default (research/ux.md §2's "don't over-explain" finding).
- The short line names all four signals' headline values (player tier, league norm range,
  target-roster FAAB remaining, target-roster claim frequency) — never just the number.
- `--full` prints the full structured object (Surface 4's shape) for drill-down, never a
  second, differently-worded prose summary.
- A target roster with zero cached transaction history still prints a short line, with the FAAB-
  remaining clause replaced by Surface 9's degraded-signal note — never a crash or a silently
  confident wrong number.
- When `suggested_bid` is `low_confidence` (per `BidProjection`'s flag — `Floor`-confidence FAAB or
  a bid-history `sample_size` under 5), the short line is prefixed with a visible caveat marker
  (e.g. `"low-confidence — "`) ahead of `suggested bid $N` — never the unqualified shape reserved
  for a well-supported figure (pre-mortem P1 #1).

---

## 4. MCP `project_waiver_bid` tool — full object (Story 3.2.2)

```json
{
  "suggested_bid": 23,
  "player_id": "3257",
  "roster_id": 5,
  "perceived_value": { "dynasty_tier": 2, "search_rank": 48 },
  "league_bidding_norms": {
    "position": "QB",
    "sample": "11 QB waiver bids this season",
    "range": [18, 28],
    "median": 22
  },
  "target_roster_faab": {
    "amount": 142,
    "confidence": { "kind": "verified", "as_of": "2026-11-10T14:02:11Z" }
  },
  "target_roster_claim_frequency": { "roster_id": 5, "claims_this_season": 6, "position": "QB" },
  "competing_roster_faab": [
    { "roster_id": 9, "amount": 34, "confidence": { "kind": "verified", "as_of": "2026-11-10T14:02:11Z" } }
  ],
  "low_confidence": false
}
```

**Acceptance criteria**:
- Returns the full `BidProjection` struct verbatim (Story 3.2.2's stated requirement) — the
  assistant caller never has to re-derive a signal the tool already computed.
- `target_roster_faab.confidence` is a tagged variant (`Verified{as_of}` vs. `Floor{reason}` per
  `CoverageConfidence`), never a bare boolean — the reader must be able to tell "trust this
  number" from "this is a floor, not a verified remaining balance" without inferring it from
  absence.
- No single opaque "confidence" float appears anywhere in the object (research/ux.md §2's explicit
  anti-pattern) — every claim is backed by a named sample size or range instead.
- `league_bidding_norms.sample` names a concrete count ("11 QB waiver bids this season"), not a
  percentage or score, consistent with the "honest small-sample caveat beats fake precision"
  finding.
- `competing_roster_faab` lists the 1-2 rosters most likely to compete for this claim (by
  positional demand), each with its own `FaabRemaining` (`amount`/`confidence`) — not just the
  target roster's own figure (pre-mortem P1 #1).

---

## 5. CLI `sleeper waiver edit` / `sleeper waiver cancel` (Story 4.3.1, LIVE WRITE ACTION)

```
$ sleeper waiver cancel --league 1352883872131481600 --transaction 1
claim cancelled
```

Failure (already resolved — see Surface 11 for the exact branch logic):
```
$ sleeper waiver cancel --league 1352883872131481600 --transaction 1
claim 1 is no longer pending (now: complete) — nothing to edit/cancel. Run `sleeper transactions
--league 1352883872131481600` to see its resolved outcome.
```

`sleeper waiver edit --league 1352883872131481600 --transaction 1 --bid 85` follows the same
success/failure shape, printing `claim bid updated to $85` on success.

**Interaction flow**: `run_edit_cancel_claim` — pre-write status re-check (`AlwaysLive`, scoped to
one transaction_id) → if still `pending`, launch → `edit_cancel_claim_via_seam` → triage → close
every branch. Always launches a browser (there is no cached path for a live write) — Surface 10's
progress line always appears first.

**Acceptance criteria**:
- A still-pending claim either prints `claim cancelled`/`claim bid updated to $N` on success or
  the exact triage text from Story 4.1.1/research/ux.md §4c on failure — never a bare stack trace on
  stdout, matching `triage_error`'s existing convention.
- An already-resolved claim never reaches the live DOM write — the pre-check short-circuits and
  prints the resolved-status triage line (Surface 11) naming the re-run command
  (`sleeper transactions --league <id>`).
- A dead session surfaces the same yellow Surface-C re-login hint `login.rs`/`doctor.rs` already
  use, not new copy.
- Exit code is non-zero on any failure branch (resolved-claim, dead-session, DOM-write-failed) and
  zero only on confirmed success.

---

## 6. MCP `edit_waiver_claim` / `cancel_waiver_claim` tools (Story 4.3.2, LIVE WRITE ACTION)

Tool description (verbatim convention, both tools):
```
LIVE WRITE ACTION — cancels a real pending waiver claim against sleeper.com (not a simulation:
frees the FAAB that claim had committed, immediately, with no confirm step). Launches a headless
Chrome using the operator's already-authenticated profile; requires `sleeper browser login` to
have been run by hand at least once first. Returns an error (never a false success) if the claim
is no longer pending, the session is not logged in, the target claim could not be matched to
exactly one row in the live claims panel, the claims panel's expected structure could not be found
at all (the live UI may have changed since the last verification), the target row moved before the
write could fire (safe — nothing changed), or the outcome is unknown (e.g. a timeout between the
DOM click and confirmation).
```

Success:
```json
{ "claim_cancelled": true, "league_id": "1352883872131481600", "transaction_id": "1" }
```

Already-resolved failure (the pre-write re-check, `AlwaysLive`-scoped to this one transaction_id,
finds the claim is no longer `pending` — short-circuits before any browser DOM write is attempted;
same copy source as Surface 5's CLI rendering, one copy source, two renderers):
```json
{
  "error": "cancel for transaction 1 is no longer pending (now: complete) — nothing to edit/cancel. Re-run get_transaction_history or get_pending_waivers to see its resolved outcome."
}
```

No-unique-row-match failure (`claim_row_disambiguator` returns 0 or >1 candidates — a read-time
row-match failure, before any click is attempted; same case as Surface 11's CLI rendering — one
copy source, two renderers):
```json
{
  "error": "cancel for transaction 1 could not be matched to exactly one row in the live claims panel (2 candidates matched) — nothing was clicked. Re-run get_transaction_history to see current state, then retry naming the exact claim, or cancel/edit by hand in the Sleeper app this one time."
}
```

Selector/DOM-drift failure (the claims panel's expected structure wasn't found at all — a
genuinely different situation from the no-unique-row-match failure above, which *does* find
candidate rows and only fails to tell which one matches; this is the same "the live UI may have
changed since the last verification" bucket `waiver_claim`/`lineup_set` already use, here meaning
the DOM shape itself, not which row is the right one — rendered via the new generic catch-all
triage fn `edit_cancel_failed_triage()` (plan.md Task 4.3.1a/4.3.2a), the same shape
`claim_failed_triage()`/`lineup_failed_triage()` already use for their own generic flow failures;
this is not a `sleeper browser doctor` health check — plan.md's Observability Plan explicitly does
not add a new doctor check for selector drift, since it isn't a session/login-health check in the
sense `doctor.rs` covers — so the real recovery action is the debug-detail re-run shown here):
```json
{
  "error": "cancel for transaction 1 failed — the live claims panel may have changed since the last verification, or the selector bundle needs live re-verification. Re-run with SLEEPER_BROWSER_DEBUG=1 for detail."
}
```

Unknown-outcome failure (DOM write timed out — `EditCancelOutcome::Ambiguous`):
```json
{
  "error": "cancel for transaction 1 timed out between click and confirmation — outcome unknown. Re-check via get_transaction_history or get_pending_waivers before retrying; do not assume the claim is still active."
}
```

Aborted-before-write failure (target row moved before the click fired — safe, nothing changed —
`EditCancelOutcome::AbortedBeforeWrite`):
```json
{
  "error": "cancel for transaction 1 aborted — the target claim moved in the live claims panel before the cancel could fire. Nothing was clicked; the claim is unchanged. Safe to retry: re-check via get_transaction_history or get_pending_waivers and retry."
}
```

**Acceptance criteria**:
- Both tool descriptions carry the "LIVE WRITE ACTION" prefix and the blunt real-money framing,
  matching `waiver_claim`/`lineup_set`'s existing convention byte-for-byte in structure (prefix,
  blunt consequence, auth precondition, error-never-false-success clause).
- A no-unique-row-match failure (0 or >1 candidates, before any click) returns `Err` naming the
  candidate count and the retry/manual-fallback step, worded from the same copy source as Surface
  11's CLI rendering — one copy source, two renderers, never independently-drifting wording.
- A selector/DOM-drift failure (the claims panel's expected structure not found at all) is never
  worded like the no-unique-row-match failure above — the two are distinct situations ("found rows,
  couldn't tell which one" vs. "couldn't find the expected structure at all") and are never
  collapsed into the same copy.
- An ambiguous-outcome failure (timeout mid-write) never returns `claim_cancelled: true` and never
  returns a generic "cancel failed" — it names the specific re-check tools
  (`get_transaction_history`/`get_pending_waivers`) and states the outcome is unknown, per Story
  4.3.2's stated acceptance criterion.
- An aborted-before-write failure (Task 4.2.1c's click-stage re-assertion) is never worded like the
  ambiguous-outcome failure above — it states plainly that nothing was clicked and the claim is
  unchanged, never "outcome unknown," since this case is a provable fact, not a genuine unknown.
- An already-resolved claim returns `Err` with the same resolved-status triage text as Surface 5,
  not a different MCP-specific wording — one copy source, two renderers.
- `edit_waiver_claim`'s params require the new bid amount; a missing/invalid bid is a validation
  `Err` before any browser launch, not a launch followed by a DOM-level failure.

---

## 7. Cross-cutting: terminal-vs-pending staleness (no dedicated command — governs Surfaces 1/2)

Not a separate command; this is the policy both read surfaces apply silently. Named here because
it's the single most load-bearing UX decision in the feature (requirements.md's resolved Open
Question).

**Acceptance criteria**:
- A `complete`/`rejected`/`cancelled` row is served from cache with zero live check regardless of
  age — these statuses never flip once written.
- A `pending` row outside the league's Tuesday waiver-processing window is served from cache with
  a long tolerance (it mechanically cannot have changed except via this tool's own cancel/edit
  action, which already invalidates it on write).
- A `pending` row read inside the processing window defaults to `AlwaysLive` unless the caller
  explicitly supplies a looser tolerance.
- This policy is invisible to the caller in the common case — no per-call flag is required to get
  the correct behavior; it only becomes visible as the `source`/`fetched_at` fields (Surface 2) or
  a `--force` override (Surface 1).

---

## 8. Cross-cutting cold-start / incomplete-coverage state (governs Surfaces 3/4)

CLI:
```
✗ no cached FAAB history for roster 5 — remaining budget is an estimate, not verified
```
(one line, inserted into the short-line render in place of a confident number; matches
`doctor.rs`'s `✗` idiom for "what do I actually have.")

MCP (inside `project_waiver_bid`'s `target_roster_faab`):
```json
{ "amount": null, "confidence": { "kind": "floor", "reason": "no transaction_sync_runs row for roster 5 — season waiver budget assumed, no won-bid deductions applied" } }
```

**Acceptance criteria**:
- A roster with zero coverage never reports `amount` as a plain number with no qualifier — it
  is `null` (MCP) or flagged `✗`/estimate (CLI), paired with the reason.
- Signals that don't depend on the missing coverage (`perceived_value`,
  `league_bidding_norms`) still render normally — the degradation is scoped to the one signal
  that's actually unverified, not the whole output.
- A *partial* backfill (some but not all of the season's bids synced) is distinguishable from
  *zero* history — `confidence.kind: "floor"` with a reason naming what's missing, not collapsed
  into the same message as true zero-history.

---

## 9. Cross-cutting dead-session-on-live-refresh (governs Surfaces 1/2-via-max_age_secs-0/5/6)

Reuses `DEAD_SESSION_HINT` / `launch_failure_error` verbatim — no new copy.

**Acceptance criteria**:
- A read that *prefers* fresh data (Surface 1 `--force`, Surface 2's `get_transaction_history`
  called with `max_age_secs: 0`) falls back to the last-known cached value with an explicit
  staleness flag when the session is dead — it does not hard-fail a call that could otherwise have
  succeeded from cache. Bid-projection (Surface 3) never triggers a live browser call at all — it is
  pure cached-signal analysis, so this bullet does not apply to it.
- A read that *requires* live truth (pending-claim status re-check ahead of an edit/cancel write,
  Surface 11) hard-fails with the existing yellow Surface-C re-login hint — it never silently
  substitutes a stale "pending" and lets a write proceed against it.
- The re-login hint text is byte-identical to the existing `doctor.rs`/`pending.rs` dead-session
  copy — no new session-dead message is invented for this feature.

---

## 10. Cross-cutting browser-launch progress notice (the "loading state")

```
Checking live claim status…
```
(dim, stderr, matches `sync.rs`'s `style(...).dim()` convention — printed once per browser
launch, before the multi-second wait, never animated/ticking).

**Acceptance criteria**:
- Printed exactly once per browser launch (not per retry, not per poll tick) — a user watching
  stderr sees one line, then either the result or a triage error, never a stream of identical
  lines.
- Routed to stderr only — never appears in stdout, so `sleeper transactions ... | jq` sees only
  data rows even when a force-refresh triggers a launch.
- Omitted entirely when the call is served from cache — this notice exists only when a browser is
  actually about to launch, never printed speculatively.
- For MCP (no stderr an assistant reads live), the equivalent signal is the `source: "graphql"` /
  `fetched_at: <now>` fields in the response after the fact — MCP tools never emit a mid-call
  progress message, since the assistant only sees the final result.

---

## 11. Cross-cutting: claim already resolved / no unique row match / aborted-before-write / ambiguous outcome / selector drift (edit/cancel failure surface)

Already-resolved (shown in Surfaces 5/6):
```
claim 1 is no longer pending (now: complete) — nothing to edit/cancel. Run `sleeper transactions
--league 1352883872131481600` to see its resolved outcome.
```

No unique row match (`claim_row_disambiguator` returns 0 or >1 candidates — e.g. the
live DOM shows two pending claims that both resolve to the same fallback key because
`transaction_id` isn't directly clickable and the secondary disambiguator also ties):
```
claim 1 could not be matched to exactly one row in the live claims panel (2 candidates matched) —
nothing was clicked. Re-run `sleeper transactions --league 1352883872131481600` and retry with
--transaction naming the exact claim, or cancel/edit by hand in the Sleeper app this one time.
```

Row moved between resolution and click — `EditCancelOutcome::AbortedBeforeWrite` (the click-stage
re-assertion inside `edit_cancel_claim_via_seam`, Task 4.2.1c, finds the claims panel changed since
the initial read — e.g. another claim resolved and the DOM reordered — immediately before the
click would fire; distinct from the row-disambiguator case above, which fails before the seam flow
ever attempts a click at all):
```
claim 1 moved in the live claims panel before the cancel could fire — nothing was clicked, the
claim is unchanged. Safe to retry: re-run `sleeper transactions --league 1352883872131481600` and
retry the cancel/edit.
```

Outcome unknown — click fired but the confirmation read failed — `EditCancelOutcome::Ambiguous`
(the post-click confirmation read inside `edit_cancel_claim_via_seam`, Task 4.2.1b, times out or
errors *after* the cancel/edit click already fired; distinct from both cases above, which fail
before any click is attempted — same case as Surface 6's "Unknown-outcome failure," one copy
source, two renderers):
```
claim 1 timed out between the cancel click and its confirmation — outcome unknown. Re-run `sleeper
transactions --league 1352883872131481600` or `sleeper waiver pending --league 1352883872131481600
--roster 5` before retrying; do not assume the claim is still active.
```

Selector/DOM-drift — the claims panel's expected structure wasn't found at all, a flow-level
failure that never reaches any of the three typed `EditCancelOutcome` checkpoints above (rendered
via the generic catch-all triage fn `edit_cancel_failed_triage()`, plan.md Task 4.3.1a/4.3.2a — the
same shape `claim_failed_triage()`/`lineup_failed_triage()` already use for their own generic flow
failures; same case as Surface 6's "Selector/DOM-drift failure," one copy source, two renderers;
this is not a `sleeper browser doctor` health check — plan.md's Observability Plan does not add one
for selector drift — so the real recovery action is the debug-detail re-run, never `doctor`):
```
Edit/cancel failed — the live claims panel may have changed since the last verification, or the
selector bundle needs live re-verification. Re-run with SLEEPER_BROWSER_DEBUG=1 for detail.
```

**Acceptance criteria**:
- The pre-write status re-check always runs before any DOM write is attempted — never after a
  failed write, per Story 4.1.1's explicit ordering requirement.
- "Already resolved," "no unique row match," "aborted-before-write," "ambiguous outcome," and
  "selector/DOM-drift" are five distinguishable failure messages — the first names the real current
  status, the second names the candidate count, the third states plainly that the row moved and
  nothing was clicked, the fourth states the outcome is unknown and must be re-checked, the fifth is
  the generic catch-all for a flow-level failure that never reached any of the other four
  checkpoints; none is collapsed into another's wording or into a generic "edit/cancel failed" with
  no distinguishing detail.
- A 0-candidate or >1-candidate match fails closed (no click attempted) — `claim_row_disambiguator`
  never guesses by picking the first/last match.
- `AbortedBeforeWrite` and `Ambiguous` (both this surface, mirrored on Surface 6) are never rendered
  with the same copy: `AbortedBeforeWrite` states confidently that nothing changed and retrying is
  safe, because nothing was clicked; `Ambiguous` states the outcome is unknown and must be
  re-checked before retrying, because the click already fired. Collapsing them would misrepresent
  the safe case as risky, or the risky case as safe.
- The selector/DOM-drift case is never worded like `Ambiguous` or `AbortedBeforeWrite` — it is a
  flow-level failure before the seam flow ever reaches a typed `EditCancelOutcome`, so it never
  claims either a provable "nothing changed" or a provable "click fired, outcome unknown."
- All five messages name a concrete next step (a re-run command, the `SLEEPER_BROWSER_DEBUG=1`
  detail flag, or — only for the genuinely unresolvable no-unique-row-match case — the Sleeper app
  as a one-time fallback) — no dead end, and never `sleeper browser doctor` (plan.md's Observability
  Plan does not add a doctor check for this failure class).

---

## UX Acceptance Criteria (feature-wide)

1. A `sleeper transactions` run completes and prints every cached row in 1 round-trip (no browser
   launch) when every row's status satisfies its staleness policy (Surface 7) — verified by the
   command returning before a browser-launch-duration elapses.
2. A `sleeper waiver bid-projection` run never launches a browser — it is pure cached-signal
   analysis; this is tested by confirming the command completes in sub-second time against a warm
   cache with no Chrome process spawned.
3. Every error state across Surfaces 5/6/9/11 names a concrete next step (a re-run command, the
   re-login flow, or — only for the genuinely unresolvable no-unique-match case — the Sleeper app
   by hand) — no dead ends, matching the Risk Control requirement.
4. No MCP tool response ever contains a bare confidence percentage or unexplained score — every
   number-with-uncertainty is paired with a named sample size, range, or tagged
   `CoverageConfidence` variant (Surfaces 4, 8).
5. Every LIVE WRITE ACTION tool description (Surface 6) states, verbatim-structured like
   `waiver_claim`/`lineup_set`: what it really does, the auth precondition, and that it returns an
   error rather than a false success — never silently available as if it were a read.
6. A cold-start/partial-coverage signal gap (Surface 8) degrades only the specific dependent
   signal, never the whole bid-projection output, and never presents an unverified number with the
   same formatting as a verified one.
7. stdout/stderr separation holds across every CLI surface: stdout carries only data rows or the
   final success/failure line; all staleness/progress/refresh narration (Surfaces 1, 10) goes to
   stderr.
8. An already-resolved claim (Surface 11) is caught by the pre-write re-check and never reaches a
   live DOM write — verified by the DOM-write function never being invoked when the pre-check
   returns a non-`pending` status.
9. Accessibility: N/A (no GUI) — consistent with research/ux.md §3.
