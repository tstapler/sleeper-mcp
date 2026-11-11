# Adversarial Review: waiver-intelligence

**Date**: 2026-11-10 (re-review after repair pass)
**Verdict**: CONCERNS

All three original Blockers are closed by the repair pass. Three items from the original Concerns
list are resolved as a side effect of the same edits (DOM-click-only success reporting, the
idempotency test gap, and one cosmetic diagram label); two Concerns and one Minor from the original
review remain open and are carried forward unchanged below. No new Blockers or Concerns were found
in the repaired sections.

**Arithmetic re-check**: the coordinator's recomputed Effort Estimate totals (`plan.md:995-1001,
1027-1029`) were independently rederived from the 20-row table (`plan.md:972-993`) and match exactly:
counts are 20 stories / 29 tasks / 1 XS + 12 S + 7 M (verified by summing the Tasks and Cost-class
columns row by row); raw tokens `20k + 12×55k + 7×130k = 1,590,000 ≈ 1.59M`; CU pre-verification
(weight `1+4×0.12=1.48` per token) `29.6k + 12×81.4k + 7×192.4k = 2,353,200 ≈ 2.353M`; CU total
`2,353,200 × 1.75 = 4,118,100 ≈ 4.12M`; overrun checkpoint `4,118,100 × 1.25 ≈ 5.15M`. Every figure
matches the coordinator's stated numbers; no arithmetic error found.

## Blockers Resolved (originally 3, now 0 open)

- [x] **"Claim frequency" missing signal** — fixed. `ClaimFrequencySignal { roster_id,
  claims_this_season, position: Option<String> }` added to the Domain Glossary
  (`plan.md:76`), `BidProjection` now requires five signals (`plan.md:72`), new Task 3.1.2b
  (`plan.md:741-750`) computes it from `list_transactions` filtered in Rust (every status, not just
  `complete`, per requirements.md's "how often do they even submit a claim"), and Story 3.1.4's AC
  (`plan.md:790-799`) requires `claim_frequency` as a non-optional 5th field with an updated
  `render_short()` example naming it explicitly.
- [x] **Unreachable/unspecified REST-vs-browser populator dispatch** — fixed. Story 2.1.2 now
  carries an explicit `(row state, kind_filter) → populator` decision table (`plan.md:496-500`) plus
  a 4th AC (`plan.md:484-490`) that exercises the REST branch directly (`kind_filter:
  Some("free_agent")` + empty cache → `analysis.sync_transaction_history`, never the browser). The
  plan is honest about scope: an explicit "Honest note on reachability" paragraph (`plan.md:502-508`)
  states that every other `kind_filter` only ever reaches the browser populator — stated as an
  accepted asymmetry rather than hidden. Task 2.1.2a's description (`plan.md:510-520`) ties the
  dispatch directly to the table rather than vague prose. This resolves the blocker as asked:
  concretely specified, and the one reachable branch is now tested.
- [x] **"Never claim success it can't prove" asserted only in prose** — fixed. `EditCancelOutcome
  { Confirmed | Ambiguous { reason: String } }` added to the Domain Glossary (`plan.md:69`), a new
  AC on Story 4.2.1 (`plan.md:880-888`) requires a `FakeSession` where the click succeeds but the
  confirmation `eval_string` call fails to yield `Ok(Ambiguous{..})`, never `Err`, and new Task 4.2.1b
  (`plan.md:896-914`) adds the outcome type, the post-click confirmation read inside
  `edit_cancel_claim_via_seam`, a new triage fn `edit_cancel_ambiguous_outcome_triage`, and a unit
  test against `FakeSession`. This now matches the other two edit/cancel disciplines
  (`claim_row_disambiguator`, Task 1.4.1a; `edit_cancel_already_resolved_triage`, Task 4.1.1a) in
  having concrete, tested implementations rather than prose-only assertions.

## Concerns

- [x] **Opponent-budget tiebreak awareness — resolved by the Phase 4 pre-mortem repair pass**
  (postdates this review's run). New Task 3.1.3b adds `competing_roster_faab: Vec<(i64,
  FaabRemaining)>` to `BidProjection`, computed for the 1-2 highest-pull competing rosters via the
  same Task 3.1.3a logic. requirements.md's Scope §2 was also amended to name the underlying
  cross-roster demand signal explicitly. No remaining gap.
- [ ] **No graceful-degradation path when the browser/GraphQL launch itself fails during a
  triggered refresh.** Carried forward unchanged — confirmed via a fresh search of the repaired
  plan (`grep -niE "fallback|degrad|browser is unavailable"` against `plan.md` returns no hits in
  Phase 2 or Phase 4). features.md §4's unstated need (research/features.md:221-228) — fall back to
  REST-only data with an explicit "may be missing pending/rejected rows" label, or surface a "run
  `sleeper browser login`" message — still has no corresponding task in `read_through`,
  `run_league_transactions_sync`, or `run_edit_cancel_claim`.
- [ ] **Story 2.3.1's "no browser launch" AC is still a wall-clock timing heuristic.** Carried
  forward unchanged — `plan.md:646-647` still reads "with no browser launch (no Chrome process
  spawned — verified by the command completing in well under the time a browser launch takes)," the
  same flaky-test shape flagged previously. — **Recommendation**: unchanged — assert on a
  call-count/mock (e.g. a spy confirming the browser-sync fn was never invoked) instead of duration.

### Resolved since the prior review (not carried forward as open)

- **DOM-click-only success reporting** (was a Concern) — now fixed as part of the Blocker-3 repair:
  Story 4.3.1's AC (`plan.md:920-934`) ties `"claim cancelled"` to a post-write
  `read_through(AlwaysLive)` re-check, printing the ambiguous/triage text instead whenever the
  re-check doesn't confirm the new state. Task 4.1.1a (`plan.md:854-860`) wires the same symmetric
  post-write check into `run_edit_cancel_claim`.
- **Story 2.2.2's idempotency claim wasn't exercised by its own Given/When/Then** (was a Concern) —
  now fixed: the AC (`plan.md:608-618`) adds a second `When` ("`run_league_transactions_sync` runs a
  **second** time immediately after with no new data") asserting `rows_changed == 0`, which the
  original single-run AC could not have failed on even if idempotency were broken.
- **Dependency diagram mislabels Story 2.1.2 as "Epic 2.1.2"** (was a Minor) — now fixed: the
  diagram (`plan.md:214-218`) correctly reads "Story 2.1.2 read_through orchestrator."

## Minors

- No retry/backoff policy is defined for transient browser failures (slow page render, selector not
  yet present) in either the whole-league backfill (Epic 2.2) or the edit/cancel flow (Epic 4.2).
  Carried forward unchanged — consistent with the existing claim/lineup_set flows' precedent (not a
  new regression), but pitfalls.md §5 notes this feature's write/backfill paths will run far more
  often than those one-shot flows, making the absence more likely to be hit in practice.
