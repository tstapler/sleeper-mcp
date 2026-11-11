# Architecture Review: waiver-intelligence
**Date**: 2026-11-10 (re-review after repair pass)
**Verdict**: CONCERNS

**Review basis**: full read of the current `implementation/plan.md` (1043 lines), focused on the
single previously-BLOCKED item: Story/Task 2.2.1 (including new Task 2.2.1c), Task 2.2.2a, and
Story 3.1.2's bid-history-by-position signal, cross-checked against the live
`src/analysis/roster.rs:162-220` (`sync_transaction_history`'s `resolve_side` closure and
`enrich_player`/`spawn_all_players` signatures). The 4 Concerns and 3 Nitpicks from the original
review are carried forward unverified per instructions — none are touched by this repair pass,
which was scoped to Blocker 1 (plus an unrelated idempotency-AC gap in Story 2.2.2 and a diagram
mislabel, neither of which interacts with Blocker 1's fix).

## Blocker 1 — RESOLVED

Original blocker: Story 3.1.2's bid-history-by-position signal reads `position` out of the
persisted `adds`/`drops` JSON blob, but only the REST sync path's private `resolve_side` closure
(`roster.rs:180-193`) produced that enriched `{player_id, name, position, roster_id, owner}` shape;
the REST path never carries `waiver_bid`, so the only bid-bearing (GraphQL-sourced) rows would
persist with no `position` field, and Story 3.1.2's `GROUP BY position` aggregation would silently
return empty/wrong results.

**Fix as written now closes the gap.** The repair pass's chain:

1. **Task 2.2.1b** (plan.md:567-578) extends the GraphQL parser to retain each entry's **raw**
   `adds`/`drops` maps (`HashMap<String, i32>`, player_id → roster_id) alongside the existing
   flattened display-string `Vec<String>` — the same shape REST's `Transaction.adds`/`drops` already
   use (`models.rs:178-179`), confirmed type-compatible with `resolve_side`'s input parameter.
2. **Task 2.2.1c** (plan.md:580-600, new) promotes the REST path's private `resolve_side` closure to
   a reusable `pub(crate) fn resolve_side(side: &HashMap<String, i32>, all_players:
   &HashMap<String, serde_json::Value>, owner_by_roster: &HashMap<i32, String>) ->
   serde_json::Value` in `roster.rs`. I confirmed this signature matches the closure's actual
   captured types read live: `spawn_all_players()` returns `JoinHandle<Result<HashMap<String,
   Value>>>` (`src/analysis/mod.rs:186`) and `enrich_player(pid: &str, all_players: &HashMap<String,
   Value>)` (`src/analysis/dynasty.rs:165`) — exactly what `resolve_side` already calls internally
   (`roster.rs:183`). `sync_transaction_history` is updated to call the promoted fn with no behavior
   change. A unit test asserts `resolve_side(...)` returns `"position": "QB"` for a fixture entry —
   proving the fn works independent of any browser.
3. **Task 2.2.2a** (plan.md:625-632) explicitly mandates that `run_league_transactions_sync` fetches
   `all_players`/`owner_by_roster` the same way `sync_transaction_history` does, then resolves each
   entry's raw `adds`/`drops` map via **Task 2.2.1c's shared `resolve_side` — "not the display-string
   parser"** — before building `TransactionRow` and calling `upsert_transaction`. This is the load-
   bearing line: it's what actually wires the GraphQL backfill path to the same enrichment function
   as REST, using `all_players` (full player data, has `position`) rather than the GraphQL response's
   thin `player_map` that motivated the original blocker.
4. **Story 2.2.1's new AC** (plan.md:549-558) is a concrete Given-When-Then proving the output: a raw
   GraphQL entry `{"adds":{"3257":5}, "settings":{"waiver_bid":78}, ...}`, once persisted via Task
   2.2.1c's resolver, yields stored `transactions.adds` JSON of
   `[{"player_id":"3257","name":"Jacoby Brissett","position":"QB","roster_id":5,"owner":"<owner>"}]`
   — `position` present and non-null.

Since Story 3.1.2's query (Task 3.1.2a) reads `position` from exactly this persisted `adds` JSON via
`list_transactions` + in-Rust aggregation (unchanged logic, not touched by the repair), and that JSON
will now carry `position` for GraphQL-sourced rows too, Story 3.1.2's signal computation will receive
rows with a `position` field once implemented as written. The new claim-frequency signal (Task
3.1.2b, addressing a separate Blocker 2) reuses the same per-add `position` field and introduces no
new data-shape dependency, so it doesn't reopen this gap.

No residual gap found in this chain: the type signatures line up against the actual code (not just
plan prose), the task sequencing (Wave 2: 2.2.1 before Wave 3: 2.2.2, per the Dependency
Visualization) puts the resolver in place before the backfill task that needs it, and the new AC is
concrete and falsifiable rather than asserted.

## Concerns

*(carried forward from the original review, unchanged from original review, not re-litigated — none
intersect with the Blocker 1 fix verified above)*

- [x] **Epic 1.4/4.2 (`edit_cancel_claim_via_seam`)** — **resolved by the Phase 4 pre-mortem repair
  pass** (postdates this review's original run). New Task 4.2.1c has the click's own `eval_string`
  call re-verify row identity before clicking, returning the now-3-variant `EditCancelOutcome`'s
  `AbortedBeforeWrite { reason }` on a mismatch — distinct from `Ambiguous`, which a later triad-review
  UX pass confirmed was wrongly conflated with this provably-safe case in an earlier draft of the fix
  (now corrected: `AbortedBeforeWrite` means nothing was clicked; `Ambiguous` stays reserved for the
  genuinely-unknown post-click case). No remaining gap.
- [ ] **Story 2.1.2 (`read_through` orchestrator)** — its ACs describe launch-vs-no-launch behavior
  with no stated test strategy that doesn't require a real browser. Recommendation: split the pure
  decision (`{UseCache, PopulateRest, PopulateBrowser}` given cached rows/tolerance/force) into its
  own testable function, leaving only thin, untested wiring to the actual populator call.
- [x] **Task 3.1.3a (FAAB-remaining)** — **tracked, not fully resolved**: added to plan.md's
  Unresolved Questions (owner: Tyler) per this review's own recommendation, rather than left dropped
  between research and planning. The `warn`-on-disagreement logging suggestion is not yet a task —
  still open for whoever picks up Story 3.1.3 at implementation time.
- [ ] **`src/db.rs`/`StatsDb` size trajectory** — "extend as-is, flag for a future phase" has no
  concrete trigger threshold, and this plan adds more to the same file again. Recommendation: state a
  concrete line-count/aggregate-count trigger in the disposition table itself.

## Nitpicks

*(carried forward from the original review, unchanged, not re-litigated)*

- Raw `&str`/`String` IDs (`transaction_id`, `league_id`, `roster_id`-as-text) are primitive-obsessed
  by a strict type-driven-design read, but consistent with the existing codebase convention — not
  worth fixing piecemeal for just this feature.
- `CoverageConfidence::Floor { reason: String }` uses free text rather than a closed enum of reasons —
  fine for now, matches the codebase's existing String-based triage-message idiom, but would need to
  become structured if a caller ever needs to branch on *which* reason.
- `claim_row_disambiguator`'s `RowTarget::Fallback { bid: Option<i64>, add_name: Option<String> }` has
  no documented tie-break rule for when both fields are `None` — worth one line in the task
  description or eventual doc comment.
