# Research: Feature Landscape — waiver-intelligence

## 1. Existing caching/freshness precedent in this codebase

Three freshness patterns already exist. None of them is a generic
"staleness-tolerance parameter" — the new design should extend the pattern,
not invent a fourth shape.

**`src/cache.rs` (`HttpCache`, `http_cache` table).** Per-row TTL stored
alongside the data (`ttl_secs` column, `cached_at` timestamp), freshness
computed as `(now - cached_at) < ttl_secs` ([`src/cache.rs:88`](/home/tstapler/Programming/sleeper-mcp/src/cache.rs)).
`get()` returns `(CachedResponse, is_fresh: bool)` — staleness is surfaced to
the caller as a bool, not hidden; the caller decides what to do with a stale
hit (the doc comment at `cache.rs:68-70` says the caller should
conditionally revalidate via ETag/Last-Modified). `ttl_override` on `set()`
already lets a *writer* pick a non-default TTL per entry
([`cache.rs:100-112`](/home/tstapler/Programming/sleeper-mcp/src/cache.rs)). This is the closest existing thing to "staleness
tolerance is a parameter, not a constant" — but it's TTL-at-write-time, not
tolerance-at-read-time. The new unified cache needs the inverse: one stored
`fetched_at` per row (as today), and the *tolerance* supplied by the reader
at query time (`max_staleness: Duration`), not baked in at write time. That's
a real extension, not a drop-in reuse — flag this for Phase 3 design.

**`src/db.rs` `weeks_fetched` ([`db.rs:91-96`](/home/tstapler/Programming/sleeper-mcp/src/db.rs)).** A coverage ledger
separate from the data itself: `(season, week) -> fetched_at`. `is_week_cached()`
([`db.rs:210-218`](/home/tstapler/Programming/sleeper-mcp/src/db.rs)) answers "do we have this slice at all,"
independent of any TTL — once fetched, a week of stats is permanently valid
(final box scores don't change). This is a **completeness ledger**, not a
freshness ledger, and it's the direct precedent for the "cache coverage is
incomplete" problem called out in Feasibility Risks (see §3 below): a boolean
per (unit-of-work) that the read path checks before trusting an aggregate.

**`src/db.rs` `roster_snapshots` ([`db.rs:114-126`](/home/tstapler/Programming/sleeper-mcp/src/db.rs)).** Append-only
history (PK includes `fetched_at`, so every sync adds a new row rather than
overwriting) with a `_latest` index for "give me the newest one." This is the
precedent for anything that needs a time series (e.g., FAAB-remaining
trend-over-season, or roster bidding-aggressiveness-over-time) rather than a
point-in-time value. `transactions` itself doesn't need this shape (a
transaction has one lifecycle, see §3), but a future "FAAB balance over time"
materialized view would.

**Verdict:** extend, don't invent. The new unified cache's core table
(`transactions`, already migrated — see below) should carry `fetched_at` per
row as it already does; add a `source` column (`rest` | `graphql`) for
observability; add a coverage ledger modeled on `weeks_fetched` — e.g.
`transaction_sync_runs(league_id, season, source, roster_id NULL-able,
fetched_at, rows_fetched)` — so "do we have *full* history for this roster"
is a lookup, not an inference from row presence. No such coverage ledger
exists today for `transactions`; this is a gap, not an oversight, and it's
exactly what Feasibility Risk "FAAB-remaining accuracy" is asking Phase 3 to
close.

## 2. Industry precedent: FAAB bid-sizing and waiver-wire-assistant UX

Web research (FantasyPros, RotoWire, RotoBaller, ESPN, 4for4 — see citations
below) surfaces a consistent signal set across commercial FAAB tools and
strategy guides. None of these expose an API; they're guides/calculators a
human reads, but the signals they weight map directly onto this project's
scope:

- **League-wide demand estimation** — "how many other teams want this
  player" (positional need across rosters, not just the caller's). This is
  exactly `analyze_roster_needs`-style context pulled per-roster, matching
  this repo's existing recommendation-discipline rule (`CLAUDE.md`'s
  league-context step) — industry practice and this repo's own standing
  process converge here.
- **Remaining-budget-relative bidding, not absolute-dollar bidding** — bid
  sizing is framed as "% of remaining FAAB," not a flat dollar figure,
  because $20 means different things to a team with $150 left vs. $30 left.
  The projection model's output should report bid-as-%-of-remaining
  alongside the raw dollar number.
  **Opponent-budget tiebreak awareness** — ESPN's example (bid $12 not $13
  when the next-highest likely bidder only has $12) implies the model should
  expose *other rosters'* estimated remaining FAAB as a visible signal, not
  just the caller's own — which this project's cache-sourced
  per-roster-bid-history already supports once FAAB-remaining is computed
  per roster, not just for "us."
- **Time-decay / urgency** — remaining budget is worth less as the season
  progresses (bye weeks, playoff push, unused FAAB doesn't carry over).
  Several sources argue for front-loading spend on high-confidence adds
  early and treating late-season dollars as "block" tools (small bids to
  deny an opponent) rather than "acquire" tools. This maps onto the
  open-question in requirements.md about recency-weighting — not just
  season-over-season recency, but within-season recency (a Week 3 bid
  pattern is less informative in Week 14).
- **Dynasty-specific data point (FantasyPros, $1,000-budget leagues):**
  reported winning bids skew low — "the bulk of winning bids were $25 or
  less," Week 1 median winning bid ~$15 — i.e., aggressive bids are the
  exception, not the norm, in dynasty specifically (as opposed to redraft,
  where early-season FAAB is often spent more aggressively). Since this
  project's one active league is dynasty, the projection model's prior
  should skew toward the league's own historical bid distribution (which the
  cache now captures) rather than a generic redraft-oriented heuristic.
- **Blind-bid dynamics**: because FAAB is a sealed-bid auction, "what would
  I pay" and "what will it take to win" are different questions — a
  projection tool that only models player value (the existing
  `dynasty_scout` signal) without the *league's own* bid-clearing-price
  history would systematically mis-price. This validates the requirement's
  insistence on combining perceived value + this league's own historical
  bidding patterns, not value alone.

No dedicated public "FAAB calculator" API or OSS tool turned up in search —
the space is guides and rules of thumb, not software. That's a point in
favor of this project's approach (a projection grounded in the league's own
cached history) being more rigorous than what's commercially available, not
a sign of a precedent to copy.

Sources: [4for4 — Ultimate Guide to Winning the Waiver Wire 2026](https://www.4for4.com/2026/preseason/ultimate-guide-winning-waiver-wire-2026), [FantasyPros — FAAB Waiver Wire Strategy & Advice](https://www.fantasypros.com/2024/08/faab-waiver-wire-strategy-advice-fantasy-football/amp/), [FantasyPros — Fantasy Football FAAB Guide & Strategy](https://www.fantasypros.com/2022/08/fantasy-football-faab-guide-strategy-2022), [RotoWire — The Long Game: FAAB Strategy in Keeper Leagues](https://www.rotowire.com/baseball/showArticle.htm?id=17344), [ESPN — Spend Your FAAB](https://africa.espn.com/fantasy/football/story/_/page/veteranmoves0910/spend-your-faab-boss-week-2), [FantasyPros — Waiver Wire Pickups to Win Championships](https://www.fantasypros.com/2025/09/fantasy-football-waiver-wire-pickups-win-championships/amp/).

## 3. Edge cases in this project's own data

**3a. Partial browser-sync history → confidently-wrong FAAB-remaining.**
Confirmed gap: there is no coverage ledger for `transactions` today (see §1).
`roster_snapshots` and `weeks_fetched` both have one; `transactions` does
not. If FAAB-remaining is computed as `waiver_budget - SUM(waiver_bid WHERE
roster_id = X AND status = 'complete')` straight off the cache, a roster
whose browser-GraphQL history was only ever fetched for *some* weeks (e.g.
the live-verified reads so far were single-roster, single-fetch, not a full
backfill — see `pending_transactions_via_seam`'s `roster_id_filters: [N]`
signature at [`flows.rs:163-168`](/home/tstapler/Programming/sleeper-mcp/src/browser/session/flows.rs)) will silently under-count
bids and over-report remaining budget. **Recommendation:** the
bid-projection tool must refuse to present a point estimate for
FAAB-remaining without first checking a coverage signal (e.g. "do we have a
`transaction_sync_runs` row proving a full-season, all-type backfill ran for
this league/season"); absent that, it should either (a) label the number
"lower bound, cache incomplete" or (b) trigger a live refresh before
answering, per the Observability Requirements section's existing mandate
("every cache read logs... why"). This is a direct, mechanical extension of
`is_week_cached()`'s pattern — a boolean gate before trusting an aggregate —
applied to a different unit (whole-league-whole-season transaction coverage
instead of one stat-week).

**3b. Same `transaction_id` via both REST and GraphQL sync, different
richness.** Confirmed sufficient at the SQL layer for the *bid* field: the
existing migration ([`db.rs:446-483`](/home/tstapler/Programming/sleeper-mcp/src/db.rs)) already does
`ON CONFLICT(transaction_id) DO UPDATE SET ... waiver_bid = COALESCE(excluded.waiver_bid,
transactions.waiver_bid)` — a REST-sourced re-sync (which never carries a bid)
cannot erase a GraphQL-sourced bid already on file, confirmed by reading the
SQL directly, not just the comment above it. **Gap found:** `status` has no
equivalent guard — it's unconditionally overwritten by `excluded.status` on
every conflict ([`db.rs:460`](/home/tstapler/Programming/sleeper-mcp/src/db.rs)). Since the GraphQL payload already
carries a `status_updated` field (visible in the query shape at
[`flows.rs:182`](/home/tstapler/Programming/sleeper-mcp/src/browser/session/flows.rs): `status status_updated`) that is **not yet
persisted** in the `transactions` table, there is no way today to detect an
out-of-order write regressing status (e.g., a delayed/retried sync
overwriting `cancelled` back to `pending` because its own fetch happened to
run after a fresher one but was *scheduled* before it, or because the public
REST sync and a roster-scoped GraphQL poll race). **Recommendation for
Phase 3:** add a `status_updated` column (`INTEGER`, the GraphQL epoch) and
change the upsert to `status = CASE WHEN excluded.status_updated >=
transactions.status_updated THEN excluded.status ELSE transactions.status
END` (or simply skip the whole row update when the incoming write is older),
mirroring the already-successful COALESCE pattern instead of inventing a new
one.

**3c. Cancelled-after-pending — does cache flip status or accumulate
duplicate rows?** Confirmed: no duplicate accumulation risk exists, because
`transaction_id TEXT PRIMARY KEY` ([`db.rs:128`](/home/tstapler/Programming/sleeper-mcp/src/db.rs)) and the upsert is
`INSERT ... ON CONFLICT(transaction_id) DO UPDATE`, i.e., one row per
transaction forever — this is unlike `roster_snapshots`, which is
deliberately append-only via a composite PK that *includes* `fetched_at`
([`db.rs:123`](/home/tstapler/Programming/sleeper-mcp/src/db.rs)). So a cancelled-after-pending claim correctly
flips the single row's `status` column on the next sync that observes it —
*given* the ordering gap in 3b doesn't bite. The two findings are linked:
single-row-per-transaction is the right shape (no dupes), but without a
monotonicity guard on `status`, "correctly flips" is only true when syncs
happen to run in wall-clock order, which an async multi-source
(REST-sync-interleaved-with-browser-sync) design cannot guarantee.

**3d. Other edge cases worth flagging for Phase 3/4, not yet designed for:**
- **Trade transactions with multiple participating rosters**
  (`roster_ids` is a JSON array, [`db.rs:477`](/home/tstapler/Programming/sleeper-mcp/src/db.rs)) — a cancelled
  trade affects FAAB-remaining for zero rosters (trades don't spend FAAB)
  but *does* need the same status-flip correctness as a waiver claim; the
  bid-projection model must filter `type = 'waiver'` before summing
  `waiver_bid`, which the Out-of-Scope section already says it will (trade
  data cached but not projected) — just confirming the schema supports that
  filter today (`list_transactions`'s `kind_filter` at [`db.rs:489-505`](/home/tstapler/Programming/sleeper-mcp/src/db.rs)
  already does this).
- **A claim that is edited (bid changed) while still pending** — same
  `transaction_id`, new `waiver_bid` — correctly handled by the COALESCE
  *only* in the REST-can't-erase-GraphQL direction; a *new, lower* GraphQL
  bid should legitimately overwrite an old higher one (user lowered their
  own bid), but `COALESCE(excluded.waiver_bid, transactions.waiver_bid)`
  treats any non-NULL incoming value as authoritative regardless of source
  or recency — today that's fine because only GraphQL ever supplies a
  non-NULL bid, so there's no conflicting-non-NULL case yet, but the same
  `status_updated`-style monotonicity guard from 3b should cover bid amount
  too once edit/cancel (epic 3) starts writing bid changes back.
- **Zero pending claims / empty league-wide backfill result** — the GraphQL
  seam fails loud today on an `errors` array
  (`parse_pending_transactions`, referenced in Rabbit Holes) but the
  generalized whole-league fetch should be checked for the same
  fail-loud behavior on an empty-but-200 response vs. a schema-shape
  change (undocumented endpoint risk, explicitly flagged in
  requirements.md's Rabbit Holes).

## 4. Unstated needs beyond the explicit requirements

- **A "why is this number what it is" audit trail**, not just the signals
  list the requirements already ask for (Observability Requirements:
  "output includes the signals it used"). Given 3a/3b/3c above, the
  projection output should also surface *cache coverage confidence*
  explicitly (e.g. "FAAB-remaining for roster 7 based on 9/12 synced weeks —
  treat as a floor, not exact") — this is a need the user's own Feasibility
  Risk ("FAAB-remaining accuracy") implies but the Scope section doesn't
  yet assign to a specific field in the tool's return value.
- **Idempotent re-run safety for the coverage ledger itself** — whatever
  `transaction_sync_runs`-equivalent table gets designed needs the same
  idempotent-migration treatment already used for `roster_ids`/`waiver_bid`
  (`PRAGMA table_info` check before `ALTER TABLE`, [`db.rs:150-160`](/home/tstapler/Programming/sleeper-mcp/src/db.rs)) —
  this is process precedent, not a new idea, but worth stating as a
  Phase 3 constraint so it doesn't get reinvented differently.
- **A single "staleness tolerance" vocabulary shared across CLI/MCP/cache
  layers** — the requirements already call for a per-call parameter; the
  unstated need is that this parameter should use the *same* unit/shape
  wherever a caller sets it (CLI flag, MCP tool param, internal cache API)
  rather than three different ad hoc representations drifting apart over
  time, matching how `waiver_claim`/`lineup_set` already share one
  "LIVE WRITE ACTION" framing convention across CLI and MCP surfaces today.
- **Graceful degradation when the browser-GraphQL path is unavailable**
  (not logged in, selector drift, schema change) — the user's own
  Feasibility Risks call this out as a breakage risk for the *write* path
  (edit/cancel) and the *read* seam, but the unstated need is that the
  *read-through cache* should still answer from REST-sourced data alone
  when GraphQL is down, clearly labeled as "may be missing pending/rejected
  rows" rather than erroring the whole query — consistent with "fail loud on
  parse, don't fail loud on absence of the richer source."

## 5. Summary of what to carry into Phase 3 (plan)

1. Extend `weeks_fetched`'s coverage-ledger pattern to transactions (new
   `transaction_sync_runs`-style table); do not invent a new freshness
   primitive beyond that and the existing TTL/`fetched_at` shapes.
2. Add `status_updated` (and eventually bid-amount provenance) with a
   monotonicity guard in the upsert, closing the ordering gap identified in
   `db.rs:460`'s unconditional `status = excluded.status`.
3. Bid-projection signals to implement, per industry precedent: perceived
   value (existing `dynasty_scout`), league-wide demand/need (existing
   `analyze_roster_needs`-style per-roster pull), this league's own
   historical bid-clearing prices by position (new, from cache), bid as
   %-of-remaining-FAAB for both caller and likely competing rosters (new),
   and an explicit cache-coverage-confidence flag on the FAAB-remaining
   component specifically.
4. Treat `staleness_tolerance` as one shared type/unit across CLI, MCP, and
   the cache's internal API, read-time-supplied rather than write-time-baked
   (contrast with `HttpCache`'s current write-time TTL).
