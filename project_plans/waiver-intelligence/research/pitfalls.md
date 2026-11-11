# Research: Known Pitfalls and Risks — waiver-intelligence

**Phase**: 2 — Research
**Scope**: what commonly goes wrong with this type of feature, risks specific
to this stack, and what the design should explicitly defend against. Grounded
in the current codebase (`src/db.rs`, `src/cache.rs`, `src/rate_limit.rs`,
`src/browser/session/{flows,live,mod}.rs`, `src/cli/browser/pending.rs`,
`src/mcp.rs`, `src/analysis/dynasty.rs`) as of this session, commit `93e8863`.

## 1. Cache invalidation / staleness-policy pitfalls

### 1.1 The design already has a concrete non-monotonic-write bug surface

`StatsDb::upsert_transaction` (`src/db.rs:451-466`) upserts on `transaction_id`
with per-column conflict resolution:

```
status     = excluded.status,                                    -- plain overwrite
...
waiver_bid = COALESCE(excluded.waiver_bid, transactions.waiver_bid), -- never erased
```

`waiver_bid` is deliberately non-destructive (comment at `src/db.rs:447-450`
explains why: REST sync must never erase a bid the browser path recorded).
`status` is **not** given the same protection — it's a straight overwrite from
whichever source wrote last. That's correct only if transaction status is
monotonic in wall-clock-of-fetch order, which it is **not** guaranteed to be:

- The public REST sync and the browser-GraphQL sync poll two different
  Sleeper backends (REST API vs. the app's own GraphQL resolver) that are not
  guaranteed to be read-consistent with each other. If the GraphQL path is
  served from a replica that lags the REST path's data store (or vice versa),
  a sync run can write `status="pending"` over a row a more-recent REST sync
  already correctly marked `status="complete"` — silently regressing a
  terminal transaction back to non-terminal with no error, no log line, and
  no way for a reader to know it happened.
- Design against this explicitly: either (a) treat `status` with the same
  COALESCE-style guard but keyed on a monotonic lattice (`pending < complete`,
  `pending < rejected`, `pending < cancelled`; never overwrite a terminal
  status with `pending`), or (b) compare `fetched_at` of the incoming write
  against the existing row and reject a write that is *older* than what's
  there) — and (c) log any write that would downgrade a terminal status, even
  if you choose to allow it, so it's visible rather than silent (ties into the
  Observability Requirements in requirements.md, which only currently asks for
  rows-fetched-vs-changed, not status-regression detection).

### 1.2 "Fresh enough" per the caller's tolerance is not the same as "fresh enough" per reality

The requirements (and `src/cache.rs`'s existing `HttpCache` precedent, which
this unified cache should generalize, not literally reuse) make staleness a
`now - fetched_at < tolerance` check against the **last time this row was
written**, not against when the underlying fact last changed. The two are
different distributions:

- A pending claim's status can change in Sleeper's backend (waiver processed)
  at any point in the waiver window, not on a schedule aligned with this
  tool's fetches. A read with `tolerance = 6h` that happens to land 2 minutes
  after Sleeper actually processed the claim will report "pending" with full
  confidence and zero indication that the number is wrong — this is not a
  degraded/slow-path response, it looks identical to a correct one.
- This is unavoidable with any TTL-style staleness model — it's the general
  "cache coherence vs. staleness tolerance" trade requirements.md explicitly
  accepts (correctness over speed is NOT required for every call; the
  per-call tolerance exists precisely so a caller can say "I need the real
  answer" by passing a near-zero tolerance). The risk is narrower than "staleness
  exists" — it's that **callers will default to a lenient tolerance out of habit
  and get silently wrong answers for exactly the question (`is this still
  pending`) that requirements.md calls out as read-now-or-wrong.** Concretely:
  any MCP tool/CLI command that answers "is claim X still pending" should default
  its own tolerance near zero (or force-refresh), independent of whatever
  default the general cache-read helper picks for other data kinds — don't let
  one generic "staleness_secs: Option<i64>" parameter with one global default
  cover both the pending-status question and the FAAB-history question, or
  whichever default wins will be wrong for the other use.
- Observability requirement to add: a resolved tolerance decision should log
  not just cache-hit-vs-refresh (already required) but **the row's actual
  age** at read time, so a pattern of "answers that were technically within
  tolerance but turned out wrong" is diagnosable after the fact by comparing
  against a later force-refresh of the same row.

### 1.3 "Remaining FAAB" computed from a cache with silently incomplete history

Remaining FAAB = season `waiver_budget` − sum of won-bid history for that
roster, read from the cache (`src/db.rs`'s `transactions` table, `waiver_bid`
column, filtered to `status="complete"` for that roster). The failure mode
flagged in requirements.md's Rabbit Holes is exactly right and worth making
concrete: this subtraction is only correct if the cache holds **every** won
bid for the roster, for the whole season. Two ways it silently doesn't:

- **Partial backfill.** If the browser-GraphQL whole-league backfill has only
  ever been run with a `leg_filters`/`roster_id_filters` scope narrower than
  "everyone, every week" (e.g. it was only ever run for the operator's own
  roster, or only for recent legs), rows for other rosters or early-season
  weeks simply don't exist — `SUM(waiver_bid)` over missing rows isn't NULL,
  it's just a smaller number than truth, and the resulting "remaining FAAB"
  is *too high* with no signal that anything is missing.
- **Silent coverage gap after a schema change** (see §2) — if a sync run
  starts silently returning 0 rows because the GraphQL shape changed in a way
  that doesn't trip the existing `errors`-array check (see §2.2), the cache
  simply stops growing and nobody notices because the read path has no
  concept of "do I have full-season coverage for this roster."
- Design against this: track coverage explicitly, not just row count — e.g. a
  `backfill_coverage` table/row per `(league_id, roster_id, season)` recording
  the leg/week range actually covered by a completed whole-league sync, so the
  FAAB-remaining calculation can check "do I have legs 1..current for every
  roster" before presenting a number, and the bid-projection tool's "signals
  used" output (already an explicit requirement) should surface *that*
  coverage check, not just the computed number — e.g. "remaining FAAB: $62
  (computed from 14/14 legs synced)" vs. "... (computed from 9/14 legs — may
  be stale/incomplete)." Without this, the tool will present a confident wrong
  number indistinguishable from a confident right one, which is the single
  worst failure mode for a feature whose whole point is replacing gut feel
  with a trustworthy number.

## 2. Risks of depending on an undocumented, reverse-engineered GraphQL endpoint + localStorage auth

### 2.1 Likely failure signature when Sleeper changes it: silent-wrong, not loud, by default

`parse_pending_transactions` (`src/cli/browser/pending.rs:58-95`) already does
the right thing for one specific failure mode — a top-level `errors` array —
and fails loud with `Err` naming the message
(`src/cli/browser/pending.rs:60-68`, tested at
`src/cli/browser/pending.rs:180-185`). But that only covers the case where
Sleeper's GraphQL server itself rejects the query. The more dangerous and more
likely drift mode is **a 200 response that parses fine but silently no longer
means what the code assumes**, and the current parser has no defense against
it:

```rust
let entries = v
    .pointer("/data/league_transactions_filtered")
    .and_then(|x| x.as_array())
    .cloned()
    .unwrap_or_default();                       // <-- missing-field and empty-result collapse to the same Vec::new()
```

If Sleeper renames `league_transactions_filtered` to something else, moves it
under a different `data` shape, or changes a field this code reads
positionally (`settings/waiver_bid`, `adds`/`drops` as keyed objects,
`player_map`), the `.pointer(...).unwrap_or_default()` chain degrades to "zero
transactions" or "zero waiver_bid" — which is **indistinguishable from the
true empty/zero case** (no pending claims; no bid on a free-agent-only claim).
This is the textbook silent-wrong-data failure mode for scraping an
undocumented endpoint: the loud failure (`errors` array, HTTP 4xx/5xx, JS
`fetch` throwing) is the *good* case because it's already handled; the bad
case is a shape change that still returns `200 {"data": {...}}` with a field
missing or renamed.

What to design against, concretely, before this project's generalization
(whole-league backfill) ships:
- **Assert expected top-level shape positively, not just absence-of-errors.**
  After the existing errors-array check, also require
  `data.league_transactions_filtered` to be present as an array (not just
  "treat missing as empty") — `Err` on a missing/null key distinctly from "key
  present, array empty." The existing test at
  `src/cli/browser/pending.rs:187-191` locks in exactly the collapse this
  flags (`"league_transactions_filtered":[]"` → `Vec::new()`), but there is no
  test for the key being *absent* — add one and make it assert `Err`, not
  `Ok(vec![])`, to cement the asymmetry into the test suite before the
  generalized backfill depends on it at scale.
- **Sanity-check one structural invariant per sync**, not just presence: e.g.
  if a transaction has `type == "waiver"` and `status == "complete"`, assert
  a `waiver_bid` was extractable (even if `0`); if that assertion starts
  failing across a sync run, that's the signature of a schema change on a
  field this code reads by path rather than a legitimately empty result —
  surface it as a warn-level log distinct from the normal "rows fetched vs.
  changed" observability line already required.
- **Version/fingerprint the query response shape** cheaply: log (at debug) the
  top-level key set of `data.league_transactions_filtered[0]` the first time
  each sync run sees a non-empty result, so a silent field rename shows up as
  a diffable log line even before it trips a hard assertion.

### 2.2 `localStorage` auth-token read is a second, independent drift surface

`pending_transactions_via_seam` (`src/browser/session/flows.rs:163-205`) reads
the token via a specific, undocumented fallback chain entirely inside page JS:

```js
let t; try { t = JSON.parse(localStorage.getItem('token_data')); } catch (e) { t = null; }
const tok = (t && (t.token || t.access_token || t.jwt)) || localStorage.getItem('token');
```

If Sleeper changes the storage key, the JSON shape, or starts requiring the
`Bearer ` prefix it currently explicitly rejects (comment at
`src/browser/session/flows.rs:155-158`), the symptom is **not** a crash — `tok`
becomes `undefined`/`null`, the `fetch` still fires with a missing/invalid
`authorization` header, and Sleeper's GraphQL almost certainly returns a 200
with an `errors: [{"message": "Unauthorized"}]` body — which the existing
parser *does* catch (§2.1's good case). So this particular drift mode is
actually already loud by the existing design, as long as the failure reaches
`parse_pending_transactions` rather than being swallowed earlier (e.g. by
`eval_string` throwing on an unexpected JS exception type, or by a change to
Sleeper's response envelope that doesn't use the `errors` key at all — some
GraphQL backends return `401` with no GraphQL envelope rather than a 200 with
an `errors` array, which is a different parse path this code does not visibly
handle — check `session.eval_string`'s handling of a non-2xx HTTP response in
the browser `fetch`, not just a GraphQL-level error).
- Concretely verify (as part of Phase 3 planning / Phase 6 verification, since
  it requires a live session): does Sleeper's `/graphql` ever return a non-2xx
  status for an auth failure rather than 200+errors? If so, `r.text()` in the
  current JS (`.then(r => r.text())`, `src/browser/session/flows.rs:195-199`)
  swallows the status code — the parser never even sees whether the HTTP
  layer rejected it, only the body. Capture and surface `r.status` alongside
  the body text so a 401 at the HTTP layer isn't indistinguishable from a 200
  with an empty/odd body.

## 3. Live DOM write-action risk: editing/cancelling an existing claim vs. submitting a new one

The verified flows (`claim_player_via_seam`,
`src/browser/session/flows.rs:48-86`; `lineup_set_via_seam`,
`src/browser/session/flows.rs:94-126`) both operate on a **freshly-navigated,
single-target** DOM state: claim flow searches for one player by name and acts
on the one resulting row; lineup flow clicks one starter, which the UI itself
then narrows to only `.valid` rows (fails closed per the comment at
`src/browser/session/flows.rs:117-118`). Edit/cancel is structurally different
and each difference is a new failure mode the existing two flows never had to
handle:

- **Selecting among several existing rows, not a freshly-filtered one.** The
  claims panel presumably lists *all* currently-pending claims at once (not
  filtered to a search term), so the generic `click_in_row` primitive
  (`src/browser/session/live.rs:200-226`) — which does
  `textContent.includes(needle)` and clicks the **first** matching row
  (`.find(...)`, `src/browser/session/live.rs:212-213`) — becomes a real
  mis-click risk this project must design around explicitly: if two pending
  claims share a player name substring (plausible: a claim that was already
  edited once may still show the old state briefly; a drop-and-add pair naming
  the same player on both sides; or simply two different claims both involving
  players whose names are substrings of each other, e.g. a first/last name
  overlap), the edit/cancel flow would act on the wrong claim with no visible
  error — it would report success because *a* row matched and was clicked.
  **Design against this with a stronger row-identity check than substring
  text match**: resolve the target row by `transaction_id` if the DOM exposes
  it anywhere (even in a data attribute or href), or at minimum require an
  exact-text match (not `.includes`) plus a secondary disambiguator (bid
  amount, add+drop pair together) passed by the caller, and **fail closed
  (error, not "closest match") when more than one row matches** rather than
  silently taking the first — this is the same "fail closed" instinct already
  applied to `lineup_set_via_seam`'s `.valid`-only targeting
  (`src/browser/session/flows.rs:117-118`) and should be stated as an explicit
  design requirement for edit/cancel, not left to whatever `click_in_row`
  already happens to do.
- **Adjacent edit/cancel controls raise the cost of a near-miss click.** The
  claim flow's one write action (submit) and the lineup flow's one click
  (swap) each have exactly one plausible target per step. An edit/cancel
  panel plausibly has an edit (pencil/bid field) control and a cancel/delete
  control sitting next to each other in the same row — a selector bundle that
  is even slightly too broad (e.g. matching "any button in this row" instead
  of the specific cancel icon) risks cancelling a claim the operator meant to
  edit, which is irreversible by this tool (there is no "undo cancel" — the
  operator would have to resubmit from scratch at the current, possibly
  different, bid landscape). This is a stronger argument than usual for
  requiring the live-UI selector-discovery step (already flagged as a Phase 3
  wall-clock blocker in requirements.md) to produce selectors scoped as
  narrowly as the lineup flow's `.team-roster-item.valid` pattern, not a loose
  "button in row" selector, and for the triage copy to make unmistakably
  clear which action (edit vs. cancel) is about to fire before it does.
- **Partial completion is a new state these flows don't have to handle today.**
  `claim_player_via_seam` and `lineup_set_via_seam` are each a short, mostly
  linear sequence ending in one submit click with no earlier step that leaves
  real-world state changed if a later step fails (searching/selecting a
  player doesn't submit anything; only the final `click_in_row(SUBMIT_BUTTON,
  ...)` does). An edit flow is different: if it's "open modal → type new bid →
  click save," a crash/timeout between typing the new bid and clicking save
  leaves the **live Sleeper claim unchanged** (safe — nothing submitted) but
  leaves **this tool's own return value ambiguous** — did the bid change or
  not? The existing pattern (`Err` from a failed step, never a false `Ok`) is
  still the right shape, but the recovery contract needs to be explicit: on
  any `Err` from the edit/cancel flow, the correct caller-visible statement is
  "unknown — re-read the claim's current state before assuming either
  outcome," not "the edit failed" (which implies the bid is still the old
  value — true most of the time, but not provably true, e.g. if the save
  click fired and the modal's confirmation simply timed out before the flow
  observed it). The MCP tool description should say this explicitly, the same
  way the existing `waiver_claim`/`lineup_set` descriptions already promise
  "never a false success" (`src/mcp.rs:516`) — extend that promise to "and on
  any error, re-check pending-claim state before retrying," since a retry
  after an ambiguous partial failure could double-submit a bid change.

## 4. Bid-projection-model risks

- **Overfitting a "suggested bid" to ~150–200 transactions from one league,
  one season.** The model's inputs — per-roster historical bid aggressiveness,
  position-specific bidding patterns, claim frequency — are each a thin slice
  of an already-small sample once split by roster × position. A roster that
  has made 3 waiver claims all season produces a "pattern" indistinguishable
  from noise; presenting a number derived from n=3 with the same confidence as
  one derived from n=30 misleads by construction. Design against this by
  **propagating sample size into the output, not just the number**: the
  "signals behind it" requirement (requirements.md Success Metrics,
  Observability Requirements) should include each signal's underlying n (e.g.
  "roster 7's average winning bid: $14, based on 4 historical wins" vs. "based
  on 22"), so a thin-sample suggestion visibly reads as thin rather than as
  confident as a well-supported one.
- **The model can encode the league's own bad habits as "the norm."** If this
  league's historical bidding has been systematically too low (nobody's ever
  had to outbid real competition) or too reactive (panic-bids after a big
  game), a model trained on "how has this league historically bid" will
  recommend *more of the same*, not a better bid — it optimizes for
  "consistent with history," which is a different goal from "likely to win
  the claim at the lowest sustainable cost," and the two only coincide if the
  history was already good. There's no fully worked guard for this without
  outside-the-league norms (which requirements.md explicitly puts out of scope
  — this model is intentionally in-league-only), so the honest mitigation is
  disclosure, not correction: the projection's "signals" output should
  distinguish "this league's historical norm for this position" from "the
  player's perceived value" (the `dynasty_tier`/`search_rank` signal, which is
  league-independent) so a reader can see when the two disagree — a
  suggested bid that's mostly "in-league norm" with little support from
  perceived value is a different kind of suggestion than one where both
  signals agree, and the output should let that show rather than collapsing
  both into one number.
- **Recency weighting is an open question the research is right to defer, but
  the hook should exist from day one.** With one season of data there's
  nothing to weight against yet, but if the schema doesn't carry season/date
  granularity cleanly into whatever aggregation the projection model reads
  (it does — `transactions.season`/`created` already exist, `src/db.rs:130,134`)
  a later recency-weighting pass would have to be retrofitted. Confirm the
  projection model's aggregation query groups/filters by season rather than
  treating the whole `transactions` table as one undifferentiated pool, even
  though today there's only one season's worth of rows — this is cheap to get
  right now and awkward to retrofit once a second season exists.

## 5. Rust/sqlx-specific pitfalls

- **This codebase already treats concurrent-SQLite-writes as a solved problem
  in one place but not another — the new cache should follow the stricter
  pattern, not the looser one.** `src/rate_limit.rs:116-121` explicitly caps
  its pool at `max_connections(1)` with the comment "serialise writes through
  a single connection" — a deliberate, documented acknowledgment that SQLite
  (even in WAL mode) only supports one writer at a time and that letting sqlx
  hand out multiple connections for a write-heavy table invites
  `SQLITE_BUSY`/lock contention. `StatsDb::open` (`src/db.rs:77-80`), which
  owns the `transactions` table this whole feature is built on, uses
  `max_connections(5)` with no equivalent comment or busy-timeout
  configuration visible in this codebase (`grep -rn busy_timeout src` found
  nothing). This feature introduces **two independent writers into the same
  table** (REST sync path, browser-GraphQL sync path) that can now run
  concurrently for the first time in a meaningful way (a backfill triggered by
  a stale-tolerance violation on one CLI/MCP call, while a scheduled or
  manual REST sync runs in another process or task) — exactly the scenario
  `rate_limit.rs`'s comment exists to prevent. Without either (a) an explicit
  busy_timeout on `StatsDb`'s `SqliteConnectOptions` (sqlx's default is short;
  verify the actual value rather than assume) or (b) serializing writes the
  same way `rate_limit.rs` does, concurrent writers risk an `Err` from sqlx on
  lock contention rather than a blocked-then-successful write — and because
  this is a read-through cache whose whole value proposition is "a live
  refresh always succeeds in updating the cache for the next reader," a sync
  that silently fails to persist its own write because of a transient lock
  conflict is a correctness bug, not just a performance one.
- **Two separate `SqlitePool`s already exist for what may be the same on-disk
  file** (`HttpCache` in `src/cache.rs` and `StatsDb` in `src/db.rs`, each with
  its own pool and its own WAL-mode connection options). If the unified cache
  this project builds is implemented as a third pool against the same file
  (or if the existing two pools are ever pointed at the same file), WAL mode
  handles multi-connection access fine for reads, but every pool opening its
  own connection multiplies the surfaces where a busy-timeout gap in §one pool
  shows up independently of the others — worth explicitly deciding whether the
  unified cache lives in `StatsDb`'s existing pool (reusing its connection
  settings) or a new one, rather than discovering the answer accidentally.
- **async/await pitfall specific to the browser-session seam**: every flow in
  `flows.rs` takes `&dyn BrowserSession` and the CLI wrapper pattern
  (`run_pending_transactions`, `src/cli/browser/pending.rs:105-130`) is
  careful to call `session.close()` / `launched.close()` on **every** branch
  before propagating the `Result` — note it does this via a `match` on the
  seam call's outcome rather than `?`, specifically so the browser always
  closes even on error. Any new edit/cancel flow or whole-league-backfill seam
  call must follow the same no-`?`-before-close discipline; a careless `let
  raw = seam_call().await?;` ahead of `session.close()` would leak a headless
  Chrome process on every error path, and because this project's write action
  runs more often (every stale-tolerance-triggered refresh, not just an
  explicit `sleeper browser login`), a leak here is more likely to be hit in
  practice than in the existing single-shot flows.
