# Architecture Research: waiver-intelligence

Date: 2026-11-10 | Builds on: `project_plans/undetected-chromedriver-rust/research/architecture.md`
(browser seam layering — cited, not re-derived) and this project's own
`research/features.md` (caching/freshness precedent, data-flow edge cases).

## 0. What the prior docs already settled (not re-derived here)

- **Browser seam** (architecture.md §2-3): `BrowserSession` trait (`src/browser/mod.rs:102-250`)
  is the only surface anything outside `src/browser/` may program against;
  flow logic lives in `src/browser/session/flows.rs`, never as trait methods
  (enforced by `session_should_expose_primitives_only_when_claim_gated`,
  `src/browser/mod.rs:297-325`). This still holds for the new edit/cancel
  flow — see §4.
- **Reusable-core pattern**: `cli::browser::{actions,pending}.rs` functions
  (`run_claim`, `run_lineup_set`, `run_pending_transactions`) take `Config`
  (+ `SleeperClient` where needed), launch the browser, run a seam flow, close
  on every branch, and return `Result` with no printing/process-exit — shared
  verbatim by the CLI wrapper and the MCP tool (`src/mcp.rs:516-565`). This is
  the integration pattern the new cache orchestrator and edit/cancel action
  reuse, not a new shape.
- **Freshness/coverage precedent** (`features.md` §1): three existing shapes
  — `HttpCache`'s write-time TTL (`src/cache.rs:88`), `weeks_fetched`'s
  completeness ledger (`src/db.rs:91-96`, `210-218`), `roster_snapshots`'
  append-only history (`src/db.rs:114-126`). None is read-time
  tolerance-as-parameter; the new cache needs that as a genuine extension.
- **Upsert gaps already found** (`features.md` §1, §3b): no coverage ledger
  for `transactions` (unlike `weeks_fetched`), and `status` is unconditionally
  overwritten on conflict (`src/db.rs:460`) with no monotonicity guard, unlike
  `waiver_bid`'s COALESCE (`db.rs:465`). I confirm both by reading the SQL
  directly (`src/db.rs:451-467`) and treat them as **open defects to close in
  Phase 3**, not re-litigate — see §2.

## 1. Applicable architectural patterns

**Read-through cache with explicit-refresh escape hatch** (Cache-Aside /
Read-Through, PoEAA "Lazy Load" variant): the orchestrator checks the cache
first, serves it if within the caller's tolerance, and on miss/stale/forced
triggers exactly one of two populator strategies (REST or browser) chosen by
*which kind of truth the caller needs* — not a cache-aside that always hits
the same backend. This is Strategy pattern over the populator, Cache-Aside
over the read path.

**Adapter/Facade seam, one level up from the browser seam.** The existing
browser seam (`BrowserSession`) answers "how do I click things on a page
without leaking chromiumoxide." The new orchestrator needs a *second*,
higher-altitude seam: "how do I get transaction data without the caller
knowing or caring whether it came from SQLite, REST, or an authenticated
browser." Same pattern, one layer up — a `TransactionSource` is conceptually
`Cache | Rest | Browser`, and the orchestrator is the Facade hiding which one
actually ran.

**Coverage ledger as a first-class value, not an inference.** `is_week_cached`
(`db.rs:210-218`) already establishes "ask a ledger, don't infer completeness
from row presence." The transactions cache needs the identical shape
(`features.md` §1, §3a) — this is mechanical reuse of an existing pattern, not
a new one.

**Event-sourced status via monotonic timestamp, not last-write-wins.** The
`status_updated` field is already fetched by the GraphQL query
(`flows.rs:182`) but not persisted or used for ordering. Treating it as the
upsert's tie-breaker (CAS-by-timestamp, a well-known pattern for
multi-writer convergence) closes the race `features.md` §3b identifies
without inventing new infrastructure — same COALESCE idiom already proven
for `waiver_bid`.

## 2. Data flow: two sync sources into one `transactions` table

**Confirmed sound:**
- Single row per `transaction_id` forever (`PRIMARY KEY`, `db.rs:128`) +
  `ON CONFLICT ... DO UPDATE` (`db.rs:451-467`) — no duplicate accumulation
  across sources or re-syncs (`features.md` §3c).
- `waiver_bid` COALESCE-on-conflict (`db.rs:465`) correctly lets a
  bid-carrying browser row survive a bid-blind REST re-sync of the same
  transaction, verified by reading the SQL, not just its comment.
- Schema already supports multi-season (`season` is a column, not an
  assumed singleton) and multi-type filtering (`list_transactions`'
  `kind_filter`, `db.rs:489-505`) — both open questions in requirements.md
  (recency weighting, trade-vs-waiver filtering) are schema-ready today.

**Confirmed missing (both already surfaced in `features.md`, restated here
as the Phase 3 blocking items, since the architecture can't be called sound
without a plan to close them):**
1. **No sync-run ledger.** `list_transactions` has no way to answer "is this
   league's cache missing anything" or "did a sync just run and find
   nothing new" — the requirement's own Observability Requirements section
   ("a 'nothing to do' re-sync is visibly cheap") is unsatisfiable without
   one. Add `transaction_sync_runs(league_id, season, source, roster_id
   NULLABLE, synced_at, rows_fetched, rows_changed)`, modeled on
   `weeks_fetched`, with the same idempotent `ALTER TABLE` / `PRAGMA
   table_info` migration guard already used for `roster_ids`/`waiver_bid`
   (`db.rs:150-160`).
2. **`status` has no monotonicity guard** (`db.rs:460`). Add `status_updated
   INTEGER` (the GraphQL epoch field already in the query shape,
   `flows.rs:182`, just not persisted) and change the conflict clause to
   `status = CASE WHEN excluded.status_updated >= transactions.status_updated
   THEN excluded.status ELSE transactions.status END` (NULL-from-REST
   treated as "no claim to staleness," i.e. always loses to any
   GraphQL-sourced value — REST never carries this field, so the CASE must
   treat `excluded.status_updated IS NULL` as "don't touch status" rather
   than as epoch 0).
3. **Pending→complete transition is read-triggered, not push-based.** Since
   Sleeper exposes no webhook, a cancelled/rejected claim's status only
   updates on the *next browser read that happens to ask about it* — this is
   inherent to the problem (not a bug to fix), but the staleness-tolerance
   design (§3) must make "pending claim status" kind 0-tolerance by default
   so a caller who actually needs current truth never silently gets a stale
   "pending" from cache.

**Verdict: the upsert design is sound for what it already does (dedup,
bid-preservation); it is incomplete for what the new read-through cache
needs on top of it (coverage answerability, status ordering).** Both gaps
are additive migrations (new column, new table), consistent with the
project's Risk Control section ("nothing existing is removed or changed in
a way that can't just not be called").

## 3. Where the orchestration layer lives

**Recommendation: a new module, `src/cli/browser/transactions.rs`, not
`src/analysis/` and not a new standalone top-level module.**

Why not `src/analysis/`: `AnalysisService` (`src/analysis/mod.rs:28-32`) is
deliberately REST+DB only — it holds `Arc<SleeperClient>` and `Arc<StatsDb>`,
nothing browser-related, and every existing consumer (CLI `roster_cmds`,
`dynasty_cmds`, every non-write MCP tool) depends on it without pulling in
`Config` or any browser-launch capability. `src/mcp.rs:521-564` confirms this
split is load-bearing at the integration layer, not incidental: the two
write-action MCP tools (`waiver_claim`, `lineup_set`) bypass
`self.svc` (`AnalysisService`) entirely and call `crate::cli::browser::run_claim(&self.config,
&self.svc.client, req)` directly — `cli::browser::*` is already the
established home for "needs both REST/DB *and* a browser launch," despite
the module path implying CLI-only. Making the orchestrator live there keeps
that invariant intact: a reader who only needs REST+cache data never forces
a `Config`/browser dependency onto `AnalysisService`'s existing call sites.

Why not a new top-level module (e.g. `src/transactions.rs`): the project
already has exactly one place for "integration glue that straddles
REST+DB+browser, reused by CLI wrapper and MCP tool" — `cli::browser/`. A
fourth top-level module would duplicate that role under a different name
for no structural reason; the existing doc comment in `actions.rs:1-9`
explicitly describes itself as serving MCP tools too, so the directory name
is already understood project-wide as "integration layer," not "CLI only."

**Concretely:**
```rust
// src/cli/browser/transactions.rs (new)
pub(crate) enum StalenessTolerance {
    AlwaysLive,                 // pending-claim status: read-now-or-wrong
    MaxAge(chrono::Duration),   // caller-supplied; per-kind defaults chosen at call sites
    CachedOnly,                 // never trigger a live call, even on total miss
}

pub(crate) async fn read_through(
    config: &Config,
    analysis: &AnalysisService,       // existing REST+DB service — reused, not duplicated
    league_id: &str,
    kind_filter: Option<&str>,
    tolerance: StalenessTolerance,
    force_refresh: bool,
) -> Result<Vec<crate::db::CachedTransaction>> { /* ... */ }
```
It decides cache-vs-live by consulting the new `transaction_sync_runs`
ledger (§2.1), calls `analysis.sync_transaction_history(league_id, weeks)`
(existing, `src/analysis/roster.rs:162-220`) for the REST populator, and a
new `run_league_transactions_sync(config, league_id)` (same file, mirroring
`run_pending_transactions`'s shape in `cli/browser/pending.rs:105-130`) for
the browser populator — which itself calls the new generalized
`league_transactions_via_seam` (§4). Every live path ends by writing through
`StatsDb::upsert_transaction` and a `transaction_sync_runs` row, so the next
reader benefits regardless of which path triggered the refresh (requirements
§ Cache architecture, verbatim).

This keeps `StatsDb` (`src/db.rs`) a dumb, source-agnostic repository —
it already has no opinion about *where* a `TransactionRow` came from
(`TransactionRow`'s own doc comment, `db.rs:32-35`, states this explicitly)
— and keeps that invariant by putting the "which source, is it stale"
decision in the orchestrator, not in `StatsDb` itself. Don't let `StatsDb`
grow a `read_through` method that launches browsers; it would collapse the
repository/orchestration boundary Clean Architecture asks you to keep.

## 4. Bid-projection logic placement

**Recommendation: new file `src/analysis/bid_projection.rs`, as a 7th module
beside `dynasty.rs`/`lineup.rs`/`players.rs`/`roster.rs`/`trades.rs`/
`waivers.rs` — not an extension of `waivers.rs`.**

`waivers.rs` (233 lines, `src/analysis/waivers.rs`) today covers exactly one
concern per its own doc comment: *wire-scanning* — unrostered players ranked
by points (`available_players`), trending add/drop signals
(`waiver_recommendations`), raw transaction listing (`recent_transactions`).
Bid-projection is a *different* concern — financial modeling across a
roster's historical spend and remaining budget — that happens to read the
same `transactions` table. Folding it into `waivers.rs` would mix "what's
out there" with "how much should I pay," violating the single-concern
granularity the other five modules already establish (each file = one
analytical question, not one data source). A dedicated file matches the
existing pattern and keeps `waivers.rs` from becoming the catch-all for
anything waiver-adjacent.

The new module composes, rather than duplicates, existing pieces:
- **Perceived value**: reuses `dynasty.rs`'s `enrich_player`/`dynasty_tier`/
  `search_rank` (`dynasty.rs:173-191`, `196-241`) — already the signal
  requirements.md names explicitly.
- **League-wide demand**: reuses `roster.rs::analyze_roster_needs`-style
  per-roster pulls (requirements §2, `features.md` §2's "league-wide demand
  estimation" precedent) — call it per roster, don't reimplement positional
  counting.
- **This league's bid history + FAAB-remaining**: new logic over
  `self.db.list_transactions(league_id, Some("waiver"))` (existing,
  `db.rs:489-525`), filtering `status == "complete"` and summing `waiver_bid`
  per `roster_id`, subtracted from the season `waiver_budget` setting.
  **Caveat to encode in the output, not just the comment** (per
  `features.md` §3a/§4): this sum is a floor, not an exact figure, unless a
  `transaction_sync_runs` check (§2.1) confirms full-season browser coverage
  for every roster being summed — the tool must label the number
  accordingly rather than present a confident wrong one, per requirements'
  own Feasibility Risk.
- **`waiver_budget` per roster**: note the GraphQL query already selects a
  `waiver_budget` field per transaction (`flows.rs:182`) — check at Phase 3
  whether that is a league-level constant or carries per-roster-season state
  more current than the typed `League`/`LeagueSettings` model
  (`src/models.rs:39-54`, which has no `waiver_budget` field today — it only
  exists in the raw JSON, confirmed by its absence from the `grep` over
  `models.rs`). If so, prefer it over hardcoding the league settings value,
  since it's observed per-transaction ground truth, not a fetched-once
  constant.

`AnalysisService` itself needs no new fields for this — `bid_projection.rs`
is `impl AnalysisService` like its six siblings, using the existing
`self.client`/`self.db`, with no `Config`/browser dependency (the projection
is read-only over cache; it never itself triggers a live refresh — that's
the orchestrator's job, called by the CLI/MCP command *before* invoking the
projection, same separation as §3).

## 5. Integration points for the new edit/cancel browser flow

**`src/browser/mod.rs`'s `BrowserSession` trait needs no new primitives.**
The existing five (`click_selector`, `wait_for_selector`, `type_into_selector`,
`click_in_row`, `eval_string`) already cover every DOM shape the edit/cancel
flow plausibly needs: open the claims panel (`click_selector` on a nav
element, generalizing the `claim_player_via_seam` pattern at
`flows.rs:48-86`), find the specific pending claim among a list
(`click_in_row`, exactly as already used to pick a drop candidate at
`flows.rs:73` and a lineup target at `flows.rs:116/123`), change the bid
value (`type_into_selector`, as used for `BID_INPUT` at `flows.rs:79`), and
confirm a cancel (`click_selector`/`click_in_row` again for any "are you
sure" dialog). Adding a trait method here would violate the test-enforced
invariant at `src/browser/mod.rs:297-325`
(`session_should_expose_primitives_only_when_claim_gated`) that flow logic
stays out of the trait — don't propose one in Phase 3 planning unless live
selector discovery turns up a DOM interaction none of the five can express
(e.g., a native `<select>` needing `eval_string`-driven option selection,
which `eval_string` already covers generically).

**New flow function**: `edit_cancel_claim_via_seam` in
`src/browser/session/flows.rs`, sibling to `claim_player_via_seam`/
`lineup_set_via_seam`/`pending_transactions_via_seam`. Needs its own
`mod edit_claim_selectors { ... }` block (same shape as `claim_selectors`/
`lineup_selectors`, `flows.rs:15-40`) — **entirely unverified until live-UI
discovery** (requirements' Rabbit Holes / Feasibility Risks are explicit
that this cannot happen in this sandbox). Phase 3 planning should produce
candidate selectors flagged unverified, not block planning on them.

**New reusable core**: `run_edit_cancel_claim` in a new
`src/cli/browser/actions.rs` function (or a sibling file if `actions.rs`
gets crowded — it's 188 lines today, still room), mirroring `run_claim`'s
exact shape (`actions.rs:69-97`): launch → resolve any player name needed →
call the seam flow → close on every branch → triage-wrap errors. Needs a new
triage message (`edit_cancel_failed_triage`, same pattern as
`claim_failed_triage`/`lineup_failed_triage`, `actions.rs:25-33`).

**New generalized browser-GraphQL read**: `league_transactions_via_seam` in
`flows.rs`, generalizing `pending_transactions_via_seam` (`flows.rs:163-205`)
by (a) dropping the single-`roster_id_filters` constraint — pass `[]` for
all rosters, matching the whole-league scope the cache needs — and (b)
widening `type_filters` to all three kinds and `status_filters` to `[]`
(already "all" today per the existing query shape) rather than hardcoding
`["waiver"]` as the CLI/MCP-facing `pending_transactions_via_seam` currently
does at its call sites (`pending.rs:136`, `mcp.rs:561`). Reuse
`parse_pending_transactions`'s shape (`pending.rs:58-95`) but rename/extend
it to also capture `status_updated` and `roster_ids` per-entry (needed for
§2's monotonicity fix and the unified cache's `roster_ids` column) — this
is schema-shape work, not new infrastructure, and should keep the same
"fail loud on an `errors` array" discipline (`pending.rs:60-68`) the
Rabbit Holes section asks for.

**CLI-wrapper pattern**: a new `WaiverAction::Edit`/`WaiverAction::Cancel`
variant alongside the existing `Claim`/`Pending` (`actions.rs:40-50`),
dispatched through `cmd_waiver` the same way, with the same "LIVE WRITE
ACTION" framing `waiver_claim`/`lineup_set` already carry in their MCP tool
descriptions (`mcp.rs:516`, `533`) — per requirements' Risk Control section,
this is non-negotiable, not a style preference.

## 6. SOLID/DDD disposition

**No Refactor-first anywhere in this touched area.** Nothing here is a
hotspot by the prior doc's own criteria (size × dependents × churn): the
browser seam is already isolated (prior doc §7, `architecture.md:77-79`,
disposition "Isolate via seam," unchanged by this project); `StatsDb` at 572
lines spans four aggregates (stats, leagues, roster snapshots, transactions)
— broader than ideal (borderline SRP: a future split into per-aggregate
repos is plausible) but not yet large or contended enough to justify a
rewrite before shipping additive columns/tables onto it; `analysis/`'s
six-module split is already the right-grained pattern (§4 extends it with a
seventh module, not a restructure).

**Disposition by area:**
- **`src/db.rs` (transactions table)**: **Extend as-is.** Two additive
  migrations (§2: `status_updated` column, `transaction_sync_runs` table),
  same idempotent-`ALTER TABLE` discipline already in place (`db.rs:150-160`).
  Flag for a *future*, not this, phase: if `StatsDb` keeps accreting
  unrelated aggregates, split into `StatsRepo`/`LeagueRepo`/`TransactionRepo`
  — not warranted yet at 572 lines with clear per-method grouping.
- **`src/analysis/`**: **Extend as-is.** New `bid_projection.rs` module,
  same shape as its six siblings; no changes to `AnalysisService`'s fields.
- **`src/browser/` seam + `src/cli/browser/`**: **Isolate via seam**
  (continuing the prior doc's own disposition, not a new one) — new flow
  function in `flows.rs`, new reusable core in `cli/browser/`, zero trait
  changes, zero changes to existing flows.
- **Orchestration layer (§3)**: new addition, not a refactor of anything
  existing — introduce it as `cli::browser::transactions`, which is net-new
  code riding an existing architectural seam (REST+DB+browser integration
  layer), not a violation to fix.

## 7. EventStorming grammar: multi-actor/multi-step business rules

| Domain Event | Policy Trigger | Command | Actor/System |
|---|---|---|---|
| Operator wants a bid number for player X | — | `ComputeBidProjection` | Operator, via CLI/MCP |
| `BidProjectionComputed` (number + signals returned) | Recommendation-discipline gate (`CLAUDE.md`: adversarial critique + league context + external grounding) | *(no automatic command — human review required)* | Operator (manual; this project ships the signal, never the auto-pilot, per Out-of-Scope) |
| Operator decides to act | — | `SubmitWaiverClaim` | Operator, via existing `waiver_claim` (CLI/MCP) |
| `ClaimSubmitted` | — | *(writes directly to sleeper.com; no local persistence step — Sleeper is system of record for the claim itself)* | sleeper-mcp (browser write) → Sleeper platform |
| Caller's staleness tolerance is violated on a cache read | `CacheMissOrStalePolicy` (§3's `read_through`) | `TriggerLiveRefresh` (REST *or* browser, chosen by data kind) | sleeper-mcp (orchestrator), no human involved |
| `LiveRefreshCompleted` (rows fetched, rows changed) | Upsert policy (§2: COALESCE bid, monotonic status) | `PersistTransactionRows` + `RecordSyncRun` | sleeper-mcp → `StatsDb` |
| League's waiver window opens (`waiver_day_of_week`) | Sleeper's own internal scheduling — **external, not modeled by this system** | *(Sleeper processes all pending claims for the league)* | Sleeper platform |
| A previously-pending claim's status actually changes (won/rejected) | Detected only on the *next* live browser read that happens to ask — no push/webhook exists | `RefreshPendingClaimStatus` (same `read_through`/`AlwaysLive` path) | sleeper-mcp, caller-triggered only (requirements' Risk Control: no background timer) |
| Operator wants to change or cancel a pending claim | — | `EditPendingClaim` / `CancelPendingClaim` (new, this project) | Operator, via CLI/MCP, gated "LIVE WRITE ACTION" |
| Edit/cancel selector bundle not yet live-verified | Feasibility blocker — cannot resolve in this sandbox | *(implementation proceeds against `FakeSession`; live action stays disabled/unverified until operator runs it on own machine)* | Operator, own machine, real display |

The middle three rows are the load-bearing ones for this project's design:
the system has **no way to observe** "Sleeper processed the waivers" as an
event — it only ever finds out by *asking again*, and the staleness-tolerance
mechanism (§2 item 3, §3) exists specifically to make that re-asking
economical (don't ask when a cheap-enough cached answer exists) without ever
silently serving a wrong "is this still pending" answer (0-tolerance for
that one kind, by design, not by omission).
