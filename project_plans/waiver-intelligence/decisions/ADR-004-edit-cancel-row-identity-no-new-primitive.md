# ADR-004: Edit/cancel row identity is resolved without adding a `BrowserSession` trait primitive

**Status**: Accepted
**Date**: 2026-11-10
**Project**: waiver-intelligence

## Context

The existing `BrowserSession::click_in_row` primitive (`src/browser/mod.rs:140-145`,
`src/browser/session/live.rs:200-226`) finds the **first** row whose `textContent` contains a needle
substring, and clicks it. That's safe for `claim_player_via_seam`/`lineup_set_via_seam` because each
operates on a freshly-filtered or UI-narrowed single-target DOM state (a search box result list; a
lineup swap mode that marks only eligible rows `.valid`).

The edit/cancel flow is structurally different: it presumably opens a panel listing *every* currently
pending claim at once, unfiltered. pitfalls.md §3 identifies the concrete risk directly: two pending
claims whose rendered text both contain a shared substring (e.g. a claim naming "Jacoby Brissett" and
an unrelated claim whose text happens to contain "Brissett" too) would make `click_in_row` silently
act on the wrong row — reporting success because *a* row matched, with no visible error. Because an
edit/cancel panel plausibly has an edit control and a cancel control sitting adjacent in the same row,
a near-miss here is higher-stakes than the existing flows' near-misses: a mis-click could cancel a
claim the operator meant to edit, which is irreversible by this tool.

Separately, `src/browser/mod.rs:297-325`'s test `session_should_expose_primitives_only_when_claim_gated`
asserts the trait exposes only small, generic primitives and that flow-specific logic (claim, lineup,
trade) never becomes a trait method — a deliberate, test-enforced architectural boundary from the
prior `undetected-chromedriver-rust` project.

## Decision

Resolve row identity **without adding a new `BrowserSession` trait method.** The flow function
(`edit_cancel_claim_via_seam`, `src/browser/session/flows.rs`) uses the existing generic `eval_string`
primitive to read back a JSON-serializable description of every candidate row in the claims panel
(index, visible text, and a `transaction_id` if the DOM exposes it in any attribute — unverified until
live discovery), passes that to a new pure function `claim_row_disambiguator` (no I/O, fully
unit-testable against fixtures), which resolves to exactly one row index or returns `Err` on zero or
more than one match — preferring an exact `transaction_id` match when available, falling back to a
secondary disambiguator (bid amount, add/drop pair) only when it isn't, and **never** falling back to
"closest match" or "first match." Only after that Rust-side resolution does the flow issue a second
`eval_string` call that clicks the exact resolved index (e.g.
`document.querySelectorAll('.claim-row')[3].querySelector('.cancel-button').click()`), never routing
through `click_in_row`'s substring-first-match semantics for this flow.

## Alternatives considered

1. **Add a new `BrowserSession::click_exact_row` (or similar) trait primitive** taking an exact-match
   requirement and failing closed on ambiguity, as a direct sibling to `click_in_row`. Rejected: this
   would require either weakening `session_should_expose_primitives_only_when_claim_gated`'s assertion
   list (`src/browser/mod.rs:304-309`) to allow a sixth primitive whose entire reason to exist is one
   flow's identity-disambiguation need, or adding it anyway and accepting that the test now encodes a
   primitive that is *not* claim-gated-generic in the sense the other five are (each of the existing
   five is reused across ≥2 flows; this one would exist for exactly one). Since `eval_string` already
   exists and is generic enough to express the exact click once the index is known in Rust, there is
   no capability gap to fill with a new primitive — only a convenience one, which isn't worth the
   boundary erosion.
2. **Loosen `click_in_row` itself to exact-text matching globally**, fixing the ambiguity risk for
   every caller at once. Rejected: this changes behavior for `claim_player_via_seam` and
   `lineup_set_via_seam`, two flows already live-verified against real sleeper.com DOM
   (`flows.rs:48-86`, `94-126`) — a substring match may be load-bearing there for reasons not visible
   from the code alone (e.g. the live DOM's actual rendered text including extra whitespace/badges a
   caller's exact string wouldn't match). Re-verifying two already-correct flows to fix a defect in a
   third, not-yet-built one is a disproportionate blast radius for this project to take on.
3. **Accept `click_in_row`'s first-match semantics for edit/cancel too, and rely on operator review of
   a confirmation dialog to catch a mis-click.** Rejected: requirements.md's Risk Control section
   treats this kind of write action as something that must fail loud, not something a human safety
   net is expected to catch after the fact — and pitfalls.md §3 is explicit that a mis-click here
   "would report success," meaning there is no confirmation signal distinguishing a correct click from
   a wrong one for either the tool or a human watching it run headless.

## Consequences

- `claim_row_disambiguator` is pure and unit-testable without any browser — plan.md Story 1.4.1's
  acceptance criteria exercise it directly against fixture JSON, before any live selector exists.
  This means the disambiguation *logic* is fully verified well ahead of the one piece that genuinely
  can't be verified in this sandbox (the actual selector bundle / whether the DOM exposes
  `transaction_id` anywhere at all).
- If live selector discovery (the Unresolved Question this ADR cannot close) finds that the DOM
  exposes **no** stable per-row identifier at all — not even indirectly via bid amount + player name
  — the secondary-disambiguator fallback may need to be redesigned once that's known. This ADR's
  decision (no new trait primitive, fail closed) still holds in that case; only the *disambiguator's
  own fallback key* would need revisiting, not the architectural boundary.
- Every future write-action flow needing to select among several similar rows (none currently
  planned) should reuse `claim_row_disambiguator`'s pattern — read-all-via-`eval_string` +
  Rust-side-resolve + exact-click — rather than reaching for `click_in_row` by default once more than
  one row can plausibly match.
