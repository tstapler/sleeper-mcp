# Requirements: waiver-intelligence

**Date**: 2026-11-10
**Type**: feature addition
**Complexity**: 3 — system design

## Problem Statement

Sleeper's public REST API (`/league/{id}/transactions/{round}`) only ever returns
`status: "complete"` transactions and never carries a FAAB bid amount
(live-verified this session, for every roster, not just the caller's own).
Pending, rejected, and cancelled transactions — and every historical bid amount —
are invisible to this tool except through the authenticated browser session,
which fires the same GraphQL query (`league_transactions_filtered`) the Sleeper
web app itself uses. Right now that path exists only as a narrow, single-purpose
read (one roster, pending waivers only, nothing persisted), there is no way to
size a bid beyond gut feel, and there is no way to edit or cancel an
already-submitted claim without leaving the tool entirely and using the Sleeper
app by hand.

## Baseline

Today, to review or change a pending waiver claim the user opens the Sleeper app
directly. To decide a bid amount they rely on their own memory of the league's
bidding norms and their own FAAB balance, tracked mentally or not at all. Every
time this tool's assistant wants current or historical transaction context it
either can't get it (public API gap) or must open a live, logged-in browser
session to get it — there is no local record, so the same ground gets re-fetched
on every ask, and older history (how aggressively has roster X bid historically,
how much FAAB does roster Y have left) is not captured anywhere this tool can
read.

## Users / Consumers

The operator (Tyler) via Claude Code / the `sleeper-mcp` MCP server, and the
`sleeper` CLI directly. Single dynasty league today; the mechanism is
league-generic (parameterized by `league_id`) but only one league is in active
use.

## Success Metrics

- A pending waiver claim's bid can be changed, or the claim cancelled, from this
  tool (CLI + MCP), where today the only path is the Sleeper app by hand.
- Repeated analysis of transaction history (bid patterns, FAAB remaining, claim
  frequency) runs against a local cache with zero additional API/browser calls
  once that cache is warm, where today every such question requires a fresh
  live call (or isn't answerable at all, for anything the public API hides).
- Asking "what should I bid on player X" returns a concrete number plus the
  signals behind it (perceived value, league bidding norms, the target roster's
  remaining budget and claim frequency), where today that number comes from
  nothing but intuition.
- A read (CLI, MCP tool, or future assistant turn) that only needs
  within-tolerance-fresh data never triggers a live browser/API call; one that
  needs current truth (e.g. "is this still pending") always does, and the
  result refreshes the cache for the next reader.

## Size

Large (3–15M CU) — informational estimate; three substantial, partly-independent
epics (unified read-through cache, FAAB bid-projection model, live waiver-claim
edit/cancel) plus the cross-cutting cache/staleness-policy design the user asked
for explicitly. Full scope is the default; this is not a gate.

**Wall-clock blockers**:
- Live-UI selector verification for the edit/cancel modal needs a real display
  and a human (same constraint that applied to the original claim/lineup_set
  work) — this sandbox has no display. Owner: Tyler, on his own machine.
- The Sleeper league's waiver window (Tuesdays, `waiver_day_of_week` setting)
  gates how soon a cancel/edit action can be live-tested against a real pending
  claim without waiting for the next cycle.

## Constraints

- Login/credential interaction stays 100% manual — no scripted keystrokes, no
  synthetic mouse/keyboard device events (CDP `Input.*`), ever. This is an
  existing, non-negotiable project policy (`src/browser/session/stealth.rs`
  policy table, Q-policy sign-off 2026-09-08).
- DOM actions (including any new edit/cancel flow) use `element.click()`/
  `dispatchEvent` only — the already-sanctioned primitives — never raw
  input-device simulation.
- The GraphQL auth token lives in the page's own `localStorage` and must never
  cross into a Rust value, a log line, or a tool's return value — read and used
  entirely inside the page's JS context (established pattern from this
  session's `pending_transactions_via_seam`).
- This repo's `CLAUDE.md` recommendation-discipline rule still gates any real
  roster-money action: adversarial critique + league-context + external
  grounding before anything actually executes against a real league. This
  project builds the tooling and the projection signal; it does not authorize
  skipping that review for any specific real bid.

## Non-functional Requirements

- **Performance SLO**: not latency-sensitive (interactive, single-operator
  tool) — correctness and staleness-bound freshness matter more than speed.
- **Scalability**: one league, 12 rosters, a season's worth of transactions
  (~150-200 rows observed); no scale concern.
- **Security classification**: internal/personal. No secrets persist in the
  local cache — only transaction/player/roster data, never the auth token.
- **Data residency**: not applicable (local SQLite file, operator's own
  machine).
- **Cache architecture** (explicit user requirement): one local cache layer
  serves BOTH the public-REST-sourced data and the authenticated-browser-GraphQL-sourced
  data uniformly — a reader should not need to know or care which source
  originally populated a row. The assistant/CLI/MCP layer reads through this
  cache by default and only falls through to a live call (API or browser) when
  the cached data is missing or outside the caller's staleness tolerance.
  Every live read that happens anyway refreshes the cache as a side effect, so
  the next reader benefits regardless of which path triggered the refresh.
- **Staleness tolerance is a parameter, not a constant**: different analyses
  need different freshness. Pending-claim status ("is this still pending") is
  read-now-or-wrong; historical bid-pattern/FAAB-remaining analysis can
  tolerate much older data. The mechanism must let a caller specify how stale
  is acceptable for its purpose, defaulting sensibly per data kind rather than
  one global TTL. What counts as "reasonable" per data kind is an empirical
  question — Phase 2 research should look at the actual observed cadence of
  each kind (how often a pending claim's status actually changes, how often
  new transactions appear, the league's own waiver-processing schedule from
  its `waiver_day_of_week`/`daily_waivers` settings) rather than picking a
  number out of the air.

## Scope

### In Scope

1. **Unified read-through cache** covering transaction history (complete,
   pending, rejected, cancelled — all four statuses, all three types: waiver,
   trade, free_agent) for the whole league, sourced from both the public REST
   sync path (existing) and the authenticated-browser GraphQL path (this
   session's `pending_transactions_via_seam`, generalized to fetch the
   whole-league history in one round-trip — already live-verified feasible).
   Explicit refresh capability (force a live re-fetch regardless of
   staleness) and a per-call staleness-tolerance parameter.
2. **FAAB bid-projection model**: for a target player, a suggested bid number
   plus the signals behind it — perceived player value (existing
   `dynasty_scout`/tier/search_rank signal), a cross-roster positional-demand
   pull (how many of the league's other rosters also need this position, not
   just the target roster's own perceived need), the league's historical
   bidding patterns from the cache (how aggressively has each roster bid, on
   what positions, how often do they even submit a claim), and each roster's
   estimated remaining FAAB this season (season `waiver_budget` setting minus
   its own won-bid history from the cache).
3. **Edit/cancel a pending waiver claim** as a new live write action (CLI
   command + MCP tool), following the existing claim/lineup_set
   launch→flow→triage→close pattern. Requires live-UI selector discovery for
   opening the claims panel and changing/cancelling an entry — not yet
   verified (see Feasibility Risks).

### Out of Scope

- Autonomously placing, changing, or cancelling a real bid without the
  existing recommendation-discipline review — this ships the capability and
  the projection signal, not an auto-pilot.
- Multi-league support beyond what already works generically (single league
  in active use; no need to special-case or optimize for more).
- Any form of scripted credential entry, CAPTCHA handling, or synthetic
  input-device simulation, for this or any other flow.
- Projecting bids for anything other than FAAB waiver claims (e.g. this does
  not model trade value or draft-pick value, even though `league_transactions_filtered`
  returns trade data too — that data is cached for completeness/future use
  but the projection model itself is waiver-bid-only).

## Rabbit Holes

- **Opening the live waiver-claims panel reliably.** Earlier ad hoc attempts
  this session to open it via blind DOM clicking were unreliable (a plain
  `.click()` did nothing; a synthesized `mousedown`/`mouseup`/`click` sequence
  sometimes advanced internal state without visibly opening the panel). The
  edit/cancel flow needs this solved properly — a verified selector bundle,
  not another guess — before any live automation is trusted.
- **Determining "reasonable" staleness defaults** requires real cadence data
  this project doesn't yet have in hand (e.g., exactly how often a pending
  claim's status flips, in wall-clock terms, relative to the league's waiver
  day). Research should ground this in the Sleeper settings already fetched
  this session (`waiver_type: 2` = FAAB, `waiver_day_of_week: 2`,
  `daily_waivers: 0` = weekly not daily) plus whatever transaction timestamp
  spread the cache itself reveals once backfilled — not an arbitrary guess.
- **GraphQL schema stability.** `league_transactions_filtered` is an
  undocumented, private endpoint reverse-engineered this session. It could
  change shape without notice. The parsing layer should fail loud (as the
  existing `parse_pending_transactions` already does on an `errors` array)
  rather than silently return wrong/partial data.
- **FAAB-remaining accuracy.** Computing a roster's remaining budget from
  summed historical bids assumes the cache has that roster's FULL season
  history, not just what happened to get synced. A partial-history cache must
  not silently present a confident, wrong remaining-budget number.

## Alternatives Considered

- Keep doing ad hoc, uncached browser reads per question (the status quo this
  session started from) — rejected: doesn't scale across a season, re-does
  the same login-gated browser launch for every question, and gives the user
  no bid-sizing signal at all.
- Hand-roll a fixed TTL per table instead of a caller-supplied staleness
  parameter — rejected per the user's explicit direction: different analyses
  have genuinely different freshness needs, and a single constant can't serve
  both "is this claim still pending" and "how has this roster bid all
  season."

## Feasibility Risks

- Live-UI verification for edit/cancel cannot happen in this sandbox (no
  display) — it can be implemented and unit-tested against `FakeSession`, but
  the real-DOM selector bundle needs the operator to run it on their own
  machine, the same gate the original claim/lineup_set work went through.
- The authenticated GraphQL path depends on reading `localStorage.token`
  inside the page's own JS — if Sleeper changes how/where it stores that
  token, the whole browser-sourced cache-refresh path breaks until
  re-verified live.
- "How often is reasonable" for staleness is being derived from a relatively
  short observation window (this project, one league, partial season) —
  defaults chosen now may need revisiting once more cadence data accumulates.

## Observability Requirements

- Every cache read logs (at trace/debug level, consistent with the existing
  `tracing` spans in `src/browser/session/flows.rs`) whether it was served
  from cache or triggered a live refresh, and why (missing row vs. stale per
  the caller's tolerance vs. explicit force-refresh).
- Every sync run (public-REST or browser-GraphQL) logs rows fetched vs. rows
  actually changed, so a "nothing to do" re-sync is visibly cheap and a real
  backfill is visibly not silent.
- The bid-projection tool's output includes the signals it used (not just the
  number), so a wrong suggestion can be diagnosed rather than taken on faith.

## Risk Control

- The edit/cancel write action ships behind the same kind of explicit,
  loud "LIVE WRITE ACTION" framing as `waiver_claim`/`lineup_set` already use
  in their MCP tool descriptions and triage copy — never silently available
  as if it were a read.
- No automatic/background cache refresh loop in this phase — refreshes happen
  on an explicit call (CLI/MCP invocation) or when a read's staleness
  tolerance is violated, never on a timer the user didn't ask for.
- Rollback is trivial: this is additive (new tables/columns via the existing
  idempotent migration pattern, new CLI subcommands, new MCP tools) — nothing
  existing is removed or changed in a way that can't just not be called.

## Open Questions

- ~~Exact staleness-tolerance defaults per data kind~~ — **resolved by Phase 2
  research** (UX agent): branch by transaction `status`, not one TTL. Terminal
  statuses (`complete`/`rejected`/`cancelled`) are cache-valid forever once
  synced; only `pending` rows can change, and only near the league's known
  Tuesday waiver-processing window (`waiver_day_of_week: 2`, `daily_waivers:
  0`) — so pending-status reads default to near-zero tolerance, everything
  else defaults to long/indefinite.
- Whether the bid-projection model should weight recency (this season's bids
  more than last season's, if/when multi-season history accumulates).
  *(unresolved after Phase 2 research — only one season of data exists; owner
  accepted the uncertainty, proceeding without recency weighting now but
  Phase 3 should leave the hook for it.)*
- Exact live-UI selector bundle for the claims panel / edit field / cancel
  control. *(unresolved after Phase 2 research — needs the operator's real
  display, same sandbox blocker as the original claim/lineup_set
  verification; owner accepted the uncertainty, proceeding with an
  implement-against-FakeSession/flag-as-unverified path.)*
