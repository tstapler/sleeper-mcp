# Implementation Plan: waiver-intelligence

**Feature**: Unified read-through transaction cache (REST + authenticated-browser GraphQL), a FAAB
bid-projection model grounded in this league's own cached history, and a live edit/cancel action for
a pending waiver claim — for the `sleeper-mcp` CLI/MCP server against league `1352883872131481600`
("Big Blue 2: Electric Bluegaloo Dynasty").
**Date**: 2026-11-10
**Status**: Ready for implementation
**ADRs**: ADR-001 (staleness-tolerance-as-parameter), ADR-002 (orchestration-layer placement),
ADR-003 (status-monotonicity-guard mechanism), ADR-004 (edit/cancel row-identity disambiguation
without a new `BrowserSession` primitive)

---

## Step 0.5 — Alternatives considered before committing

Three places in this design had a real fork. Each is recorded here once; the chosen option is
carried into Pattern Decisions below, not repeated.

1. **How the two sync sources become one cache.** (a) *Read-through cache, Strategy-pattern
   populator chosen by data kind* — reuses `src/cache.rs`'s `is_fresh` shape and `src/cli/sync.rs`'s
   caller-supplied-tolerance shape, zero new dependency. Weakness: two freshness idioms
   (write-time TTL vs. read-time tolerance) now coexist in the codebase until `HttpCache` is ever
   revisited. (b) *Adopt a generic SWR crate (`cacheflight`)* — real stale-while-revalidate
   semantics with jitter. Weakness: solves thundering-herd dedup this single-operator tool doesn't
   have, and still needs the exact same per-kind-default/COALESCE glue hand-written on top — net
   more code, not less. (c) *No cache — always call live* — simplest to reason about. Weakness:
   defeats the explicit requirement (zero-call repeat reads) and reintroduces a browser launch per
   question. **Chosen: (a).**
2. **Where bid-projection logic lives.** (a) *New `src/analysis/bid_projection.rs`, 7th module* —
   matches the existing one-file-per-analytical-question grain. Weakness: one more file to keep in
   sync with `AnalysisService`'s shared helpers. (b) *Extend `waivers.rs`* — fewer files. Weakness:
   mixes "what's on the wire" with "how much should I pay," the exact anti-pattern the other five
   modules avoid, and risks `waivers.rs` becoming a dumping ground. (c) *Extend `roster.rs`* (it
   already owns `sync_transaction_history`) — colocates with the data it reads. Weakness: `roster.rs`
   is about roster/league composition, not financial modeling; same granularity violation as (b).
   **Chosen: (a).**
3. **How edit/cancel picks the right row among several pending claims.** (a) *No new
   `BrowserSession` trait primitive — read all candidate rows via `eval_string`, disambiguate in Rust
   (exact-text + secondary disambiguator), fail closed on 0/>1 matches, click via a second
   `eval_string` call targeting the resolved index* — respects the test-enforced trait-primitive
   boundary (`src/browser/mod.rs:297-325`). Weakness: more JS embedded in `flows.rs` than the
   existing flows needed. (b) *Add a new `click_exact_row` trait primitive* — cleaner call site.
   Weakness: violates `session_should_expose_primitives_only_when_claim_gated`
   (`src/browser/mod.rs:297-325`), which asserts flow logic never becomes a trait method; would
   require weakening a test that exists specifically to keep this boundary. (c) *Loosen
   `click_in_row`'s matching to exact-text globally* — one shared primitive, no new code paths.
   Weakness: changes behavior for `claim_player_via_seam`/`lineup_set_via_seam`, two live-verified
   flows this project has no mandate to touch or re-verify. **Chosen: (a)** — see ADR-004.

---

## Domain Glossary

| Term | Definition | Notes |
|------|-----------|-------|
| `StalenessTolerance` | Enum (`AlwaysLive \| MaxAge(chrono::Duration) \| CachedOnly`) a caller passes to `read_through` to say how stale an answer it will accept. | `src/cli/browser/transactions.rs`; read-time, not write-time (contrast `HttpCache`'s TTL). |
| `read_through` | The cache-aside entry point: checks the cache against a `StalenessTolerance`, triggers exactly one populator (REST or browser) on miss/stale/forced, always writes through before returning. | `src/cli/browser/transactions.rs`. |
| `default_tolerance_for` | New pure function (no I/O): `fn(status: &str, now: DateTime<Utc>, waiver_day_of_week: u8, daily_waivers: bool) -> StalenessTolerance` — the Tuesday-window-aware default a caller gets when it supplies no explicit tolerance (requirements.md's resolved Open Question, ux.md §7). | `src/cli/browser/transactions.rs`; unit-testable without a browser/DB. |
| `TransactionSource` | Which backend wrote a given cached row: `"rest"` or `"graphql"` (stored as the new `transactions.source` column, not a Rust enum crossing the DB boundary). | Observability field per features.md §4. |
| `TransactionRow` | Existing write-side struct for `StatsDb::upsert_transaction`; gains `status_updated: Option<i64>` and `source: &str`. | `src/db.rs:36-48`. |
| `CachedTransaction` | Existing read-side struct for `StatsDb::list_transactions`; gains `fetched_at`, `status_updated`, `source` — currently missing even `fetched_at`, a read-side gap this plan closes (§2.1.1). | `src/db.rs:52-64`. |
| `status_updated` | New `INTEGER` column (GraphQL epoch field, already fetched at `flows.rs:182` but never persisted) used as the CAS tie-breaker for the `status` column. | `src/db.rs`. |
| `transaction_sync_runs` | New coverage-ledger table, modeled on `weeks_fetched` (`db.rs:91-96`), answering "did a full-league backfill from this source ever complete for this league/season," not just "do rows exist." | `src/db.rs`. |
| `TransactionSyncRun` | Rust struct mirroring one row of `transaction_sync_runs` (`league_id`, `season`, `source`, `roster_scope`, `synced_at`, `rows_fetched`, `rows_changed`). | New in `src/db.rs`. |
| `roster_scope` | `transaction_sync_runs` column: `"all"` for a whole-league GraphQL backfill, or a specific roster_id (as text) for a narrower sync (e.g. the existing single-roster pending read). | Determines whether a FAAB-remaining computation can trust full coverage. |
| `league_transactions_via_seam` | New seam flow generalizing `pending_transactions_via_seam` (`flows.rs:163-205`) to fetch the whole league, all types, all statuses, in one round-trip. | `src/browser/session/flows.rs`. |
| `edit_cancel_claim_via_seam` | New seam flow for editing a bid or cancelling a pending claim; selectors unverified until live-run. Returns `EditCancelOutcome`, not a bare `Result<()>` — a pre-click, same-`eval_string`-call identity re-assertion (pre-mortem P1 #2) feeds `AbortedBeforeWrite` (nothing clicked, claim provably unchanged), and a post-click confirmation read feeds `Ambiguous` (the click fired, outcome genuinely unknown) — distinct situations, never collapsed into one meaning, and never a silent wrong-row click or an unprovable success either way. | `src/browser/session/flows.rs`. |
| `EditCancelAction` | New enum (`EditBid(u32) \| Cancel`) distinguishing the two write operations the one flow function supports. | `src/browser/session/flows.rs`. |
| `EditCancelOutcome` | New enum (`Confirmed \| AbortedBeforeWrite { reason: String } \| Ambiguous { reason: String }`) — the "never claim success it can't prove" discipline's concrete type (adversarial review Blocker 3), distinguishing two situations with different real risk: `AbortedBeforeWrite` means the pre-click row-identity re-assertion (Task 4.2.1c) failed to find a single exact match, so nothing was clicked and the claim is provably unchanged — safe to retry immediately. `Ambiguous` means the click already fired and the post-click confirmation read (Task 4.2.1b) came back unverified (e.g. the confirmation `eval_string` call itself failed or timed out) — the outcome is genuinely unknown, real risk in a blind retry. Neither collapses into a bare `Err` that implies nothing happened. | `src/browser/session/flows.rs`. |
| `run_edit_cancel_claim` | New reusable core (launch → pre-write status re-check → seam flow (returns `EditCancelOutcome`) → post-write `read_through(AlwaysLive)` re-check → close-every-branch → triage), shared by CLI and MCP, mirroring `run_claim` (`actions.rs:69-97`). | New file or addition to `src/cli/browser/actions.rs`. |
| `WaiverAction::Edit` / `WaiverAction::Cancel` | New CLI subcommand variants alongside existing `Claim`/`Pending` (`actions.rs:40-50`). | `src/cli/mod.rs` (wherever `WaiverAction` is defined) + `actions.rs`. |
| `BidProjection` | New struct: the bid-projection tool's full output — five named signals with real values and sample sizes, a `competing_roster_faab` list (the 1-2 likeliest competing rosters' own `FaabRemaining`, pre-mortem P1 #1), a `low_confidence` flag gating the short-render caveat, plus a suggested dollar figure and bid-as-%-of-remaining-FAAB. | `src/analysis/bid_projection.rs`. |
| `FaabRemaining` | New struct: `{ amount: i64, confidence: CoverageConfidence }` — never a bare number. | `src/analysis/bid_projection.rs`. |
| `CoverageConfidence` | New enum (`Verified { as_of: DateTime<Utc> } \| Floor { reason: String }`) — replaces a bare bool so a caller/reader can see *why* a number might be a lower bound. | `src/analysis/bid_projection.rs`. |
| `BidHistorySignal` | New struct: `{ mean_winning_bid: f64, sample_size: usize, position: String }` — this league's own historical bid-clearing price for a position, tagged with `n` so thin samples visibly read as thin. | `src/analysis/bid_projection.rs`. |
| `ClaimFrequencySignal` | New struct: `{ roster_id: i64, claims_this_season: usize, position: Option<String> }` — how often *this specific roster* has submitted a waiver claim this season, the 5th bid-projection signal named explicitly in requirements.md's Success Metrics ("claim frequency", requirements.md:48-51) and Scope §2 ("how often do they even submit a claim", requirements.md:138) but missing from the original 4-signal `BidProjection` design. | `src/analysis/bid_projection.rs`. |
| `has_full_league_graphql_coverage` | New `StatsDb` query: most recent `transaction_sync_runs` row with `source='graphql', roster_scope='all'` for `(league_id, season)`, or `None`. | `src/db.rs`. |
| `claim_row_disambiguator` | New pure function (no I/O): given the JSON array `eval_string` returns for all candidate claim rows plus a target `(transaction_id_or_fallback_key)`, returns the single matching row index or `Err` on 0/>1 matches. | `src/browser/session/flows.rs` (private fn) — unit-testable without a browser. |
| `BrowserSession` | Existing seam trait (`src/browser/mod.rs:102-149`); **gains no new methods** in this project (ADR-004). | Unchanged. |
| `AnalysisService` | Existing REST+DB service (`src/analysis/mod.rs:28-32`); gains the `bid_projection` module as a 7th `impl` file, no new fields. | Unchanged struct. |

---

## Pattern Decisions

| Component | Pattern Chosen | Source | Alternative Rejected | Reason |
|-----------|---------------|--------|---------------------|--------|
| Unified transaction cache | Cache-Aside / Read-Through (PoEAA "Lazy Load" variant) with Strategy over the populator (REST vs. browser, chosen by `StalenessTolerance` + data kind) | architecture.md §1 | `cacheflight` (generic SWR crate) | Solves thundering-herd dedup this single-operator tool doesn't have; still needs the same hand-written per-kind-default/COALESCE glue on top (build-vs-buy.md §1a). |
| Orchestration layer | Facade, one layer above the existing `BrowserSession` seam — `src/cli/browser/transactions.rs` | architecture.md §3 | New top-level `src/transactions.rs` module | `cli::browser/` is already project-wide understood as "REST+DB+browser integration layer, serves MCP too" (`actions.rs:1-9`'s own doc comment); a fourth top-level module duplicates that role under a different name. |
| Orchestration layer (cont.) | — | — | Extend `AnalysisService`/`src/analysis/` | `AnalysisService` is deliberately REST+DB-only (`analysis/mod.rs:28-32`); giving it a browser-launch dependency would force that cost onto every existing non-write call site. |
| Bid-projection logic | New 7th `AnalysisService` module, `src/analysis/bid_projection.rs` | architecture.md §4 | Extend `src/analysis/waivers.rs` | Mixes "what's on the wire" (waivers.rs's actual concern) with "how much should I pay" — violates the one-file-per-analytical-question grain the other five modules keep. |
| `transactions.status` write conflict | Event-sourced CAS on a monotonic timestamp (`status_updated`), NULL-from-REST never touches `status` | architecture.md §2, pitfalls.md §1.1 | Compare incoming `fetched_at` against the existing row's `fetched_at` and reject older writes outright | Rejects the whole row (losing a legitimately-concurrent `roster_ids`/`adds`/`drops` update) instead of just gating the one column that actually needs ordering; `waiver_bid`'s COALESCE precedent already shows per-column guards are the established idiom here. |
| Transaction coverage | Dedicated ledger table (`transaction_sync_runs`), modeled on `weeks_fetched` | architecture.md §2 item 1, features.md §1 | Per-`(league_id, roster_id, season)` leg-range coverage table (pitfalls.md §1.3's finer-grained proposal) | The whole-league GraphQL query (`leg_filters: []`, `roster_id_filters: []`) is a single round-trip covering every roster and leg at once — a `roster_scope='all'` row per sync run answers the real question ("did a full-league backfill ever complete") without the bookkeeping overhead of 12 per-roster rows that would always change in lockstep. |
| SQLite write concurrency (two independent writers into `transactions`) | `busy_timeout` on `StatsDb`'s existing `SqliteConnectOptions`, keep `max_connections(5)` | pitfalls.md §5 | Serialize all `StatsDb` writes through `max_connections(1)`, mirroring `rate_limit.rs:121` | `StatsDb` serves many concurrent *readers* across all six existing `AnalysisService` modules; dropping to one connection would serialize unrelated reads too. `busy_timeout` makes a genuinely concurrent writer retry-wait instead of erroring, which is the actual failure mode this feature introduces. |
| SQLite write concurrency (cont.) | — | — | New, third `SqlitePool` dedicated to the transactions cache | Would multiply the surfaces where a busy-timeout gap shows up independently (pitfalls.md §5); the transactions table already lives in `StatsDb`'s pool — no reason to split it out. |
| Edit/cancel row selection | No new trait primitive; `eval_string`-driven read-all-candidates + Rust-side exact-match/secondary-disambiguator + fail-closed, then a targeted `eval_string` click | architecture.md §5, pitfalls.md §3, ADR-004 | New `BrowserSession::click_exact_row` primitive | Would violate the test-enforced invariant `session_should_expose_primitives_only_when_claim_gated` (`src/browser/mod.rs:297-325`) that flow logic never becomes a trait method. |
| Edit/cancel row selection (cont.) | — | — | Loosen `click_in_row`'s substring-match to exact-match globally | Changes behavior for the two already live-verified flows (`claim_player_via_seam`, `lineup_set_via_seam`) this project has no mandate to re-verify. |
| Bid-projection scoring | Hand-rolled weighted combination of 5 explicit signals (including per-roster claim frequency), each carrying its own sample size; no single opaque score | build-vs-buy.md §3, ux.md §2; requirements.md §Success Metrics, §Scope item 2 (claim frequency) | A statistical/ML model fit to this league's ~150-200 transactions | Confirmed overfitting risk at this sample size (pitfalls.md §4); no OSS/SaaS option exists anyway (build-vs-buy.md §1b, §2). |
| Bid-projection scoring (cont.) | — | — | Rank by `dynasty_tier`/`search_rank` alone (ignore league history) | Systematically mis-prices a sealed-bid auction — "what would I pay" and "what will it take to win" are different questions (features.md §2's "blind-bid dynamics" finding). |
| Staleness representation | Sum type (`StalenessTolerance` enum: `AlwaysLive \| MaxAge(Duration) \| CachedOnly`) | type-driven design (newtypes/sum-types) | `Option<i64>` seconds + a separate `force: bool` flag | The three states are mutually exclusive and the `Option`+`bool` pairing allows an illegal combination (`Some(0)` + `force: true` meaning two different things); the enum makes every call site's intent explicit and matches architecture.md §3's own signature. |
| FAAB-remaining confidence representation | Sum type (`CoverageConfidence::Verified{as_of} \| Floor{reason}`) | type-driven design | Bare `bool is_complete` field | A bool loses *why* (features.md §4's "unstated need"); the enum carries the reason string a reader/assistant needs to decide whether to trust the number. |

---

## Tech Debt Disposition

| Area | Existing Issue | Disposition | Justification |
|------|----------------|--------------|----------------|
| `src/db.rs` — `transactions` table | No coverage ledger (unlike `weeks_fetched`); `status` unconditionally overwritten on conflict with no monotonicity guard ([db.rs:460](../../../src/db.rs)) | **Extend as-is** — two additive migrations (`status_updated`/`source` columns, `transaction_sync_runs` table), same idempotent `PRAGMA table_info` + `ALTER TABLE` discipline already proven at `db.rs:150-160` | architecture.md §6: these are open defects to close, not a reason to refactor `StatsDb`'s shape; nothing else about the table is unsound (dedup via `PRIMARY KEY` + `ON CONFLICT` is already correct, confirmed by reading the SQL). |
| `src/db.rs` — `StatsDb` overall (572 lines, four aggregates: stats, leagues, roster snapshots, transactions) | Borderline single-responsibility breadth | **Extend as-is; flag for a future, not this, phase** | architecture.md §6: not yet large or contended enough (572 lines, clear per-method grouping) to justify a `StatsRepo`/`LeagueRepo`/`TransactionRepo` split before shipping additive columns/tables. |
| `src/analysis/` (six existing modules) | None — six-module split is already the right grain | **Extend as-is** — add `bid_projection.rs` as a 7th module | architecture.md §6: this project's own Epic 2 is itself evidence the grain is right (a new concern gets a new file, not a bolt-on). |
| `src/browser/` seam + `src/cli/browser/` | None — seam is already isolated per the prior `undetected-chromedriver-rust` architecture doc | **Isolate via seam** (continuing, not a new disposition) — new flow functions in `flows.rs`, new reusable cores in `cli/browser/`, zero trait changes | architecture.md §6: the whole point of the seam is that new DOM behavior is additive here by construction. |
| `src/cache.rs` (`HttpCache`) | Write-time TTL, not the read-time-tolerance shape this feature needs | **Leave alone; do not generalize it for this feature** | The unified transaction cache needs a genuinely different shape (read-time tolerance) from `HttpCache`'s write-time TTL (features.md §1) — forcing one to subsume the other would churn a working, unrelated cache (raw HTTP bodies) for no benefit to this project. Two freshness idioms coexisting is accepted technical debt, named here rather than silently duplicated. |
| `src/rate_limit.rs` | None — correct as-is for its own purpose | **No change** | Cited only as the stricter-pattern precedent for the `StatsDb` busy_timeout decision above; this feature does not touch rate limiting. |

---

## Migration Plan

- **Migration file**: no standalone migration file — this repo's existing pattern is inline,
  idempotent DDL inside `StatsDb::open` (`src/db.rs:82-160`), guarded by `PRAGMA table_info` checks
  for `ALTER TABLE` (no `IF NOT EXISTS` support in SQLite) and plain `CREATE TABLE IF NOT EXISTS`
  for new tables. This plan adds to that same function: two new columns on `transactions`
  (`status_updated INTEGER`, `source TEXT`) and one new table (`transaction_sync_runs`).
- **Reversibility**: forward-only by convention (matches the existing `roster_ids`/`waiver_bid`
  migration, `db.rs:150-160`, which has no down-script either). A new column defaults to `NULL` on
  existing rows, which the CAS logic already treats as "no claim to staleness" — existing rows are
  not corrupted by the migration running. Rollback = don't deploy the binary with the new code; the
  old binary ignores the new columns/table entirely (SQLite tolerates extra columns it doesn't
  `SELECT`).
- **Zero-downtime strategy**: not applicable in the distributed-systems sense — this is a
  single-operator, single-process local SQLite file (requirements.md § Data residency). The only
  "downtime" risk is a half-applied migration if the process is killed mid-`ALTER TABLE`; SQLite's
  own DDL atomicity (each statement is its own implicit transaction) already covers this, consistent
  with how `roster_ids`/`waiver_bid` already ships today.
- **Rollback procedure**: re-run an older `sleeper-mcp` binary against the same `stats.db` file; it
  ignores `status_updated`/`source`/`transaction_sync_runs` and continues operating on the columns
  it knows. No data loss, since nothing existing is dropped or renamed (Risk Control, requirements.md).

## Observability Plan

- **Logs**: every `read_through` call emits one `tracing::info_span!("transactions.read_through", …)`
  (mirroring `pending_transactions.start`'s existing shape, `flows.rs:169-175`) naming
  `cache_hit`/`cache_miss`/`stale_per_tolerance`/`force_refresh` and, for a `pending`-status row
  specifically, the row's actual age at read time (pitfalls.md §1.2's explicit ask — not just
  hit/miss, but how old the serving row actually was). Every sync run (`sync_transaction_history`
  or `league_transactions_via_seam`) logs rows fetched vs. rows changed at `info`, and — new — warns
  at `warn` if a `status` write would have downgraded a terminal status to `pending` but was rejected
  by the CAS guard (pitfalls.md §1.1's "log it even if you choose to allow it" — here we don't allow
  it, but the attempt is still worth a log line for drift detection). Debug-level: log the top-level
  key set of the first non-empty `league_transactions_filtered` entry per sync run, to catch a silent
  GraphQL schema drift before it trips a hard assertion (pitfalls.md §2.1).
- **Metrics**: none — this is an interactive, single-operator CLI/MCP tool with no metrics backend;
  `tracing` spans are the whole observability surface, consistent with every existing flow.
- **Alerts**: none — no always-on process to alert from. The equivalent is the `sleeper browser
  doctor`-style ✓/✗ report (`doctor.rs`) a human runs on demand; this plan does not add a new doctor
  check, since none of the new failure modes (stale cache, selector drift, cold-start history) are
  session/login-health checks in the sense `doctor.rs` already covers.

## Risk Control

- **Feature flag**: none needed — every new capability is additive (new tables/columns, new CLI
  subcommands, new MCP tools); nothing existing changes behavior unless the new code path is invoked
  (Risk Control, requirements.md). The one exception needing explicit gating is the edit/cancel live
  write action, which is gated the same way `waiver_claim`/`lineup_set` already are: a loud "LIVE
  WRITE ACTION" MCP tool description and `sleeper browser login` as a manual prerequisite — no
  separate on/off switch, consistent with the existing two write tools.
- **Rollback procedure**: see Migration Plan above (older binary, same DB file, no data loss).
- **Staged rollout**: sequence by risk, not by epic number (see Phase ordering below) — the
  status-monotonicity fix and the edit/cancel row-identity design land in Phase 1, before the bulk of
  the cache/projection code depends on either being right.

## Unresolved Questions

- [ ] Exact live-UI selector bundle for the claims panel (open claim list, identify a row, bid-edit
  field, cancel control) — blocks Story 4.2.1's live verification (not its `FakeSession`
  implementation, which proceeds now) — owner: Tyler, on his own machine, requires a real display.
- [ ] Whether Sleeper's `/graphql` ever returns a non-2xx HTTP status for an auth failure rather than
  a 200 with an `errors` array (pitfalls.md §2.2) — blocks hardening Story 2.2.1's error handling
  beyond what's already verified — owner: Tyler, requires a live authenticated session to probe.
  Current code (`flows.rs:195-199`, `.then(r => r.text())`) cannot distinguish these today; Story
  2.2.1 should capture `r.status` alongside the body as a defensive improvement regardless of the
  answer, and this question gates only whether that capture ever actually fires in practice.
- [ ] Recency weighting for the bid-projection model (season-over-season, or within-season) — explicitly
  deferred by Phase 2 research (only one season of data exists today); Story 3.1.2's aggregation
  query groups by `season` from day one so the hook exists, but no weighting logic is implemented —
  owner: Tyler, revisit once a second season of history accumulates.
- [ ] Whether the GraphQL `waiver_budget` field (per-transaction, `flows.rs:182`) is a league-level
  constant or carries more-current per-roster-season state than `LeagueSettings`'s fetched-once value
  (architecture.md §4 flagged this and planning dropped it without resolving it) — blocks a
  correctness hardening of Task 3.1.3a, not its initial implementation (which proceeds using the
  league-settings value as currently scoped). Resolution either way should add a `warn` log on
  disagreement between the two sources — owner: Tyler, needs a live transaction with a confirmed
  current FAAB state to compare against.

## Dependency Visualization

```
Phase 1 — Risk-first foundations
┌─────────────────────────┐   ┌──────────────────────────────┐
│ Epic 1.1 status-monotone │   │ Epic 1.4 edit/cancel design   │
│ guard (db.rs)            │   │ spike (flows.rs, fake.rs)     │
└────────────┬─────────────┘   └───────────────┬────────────────┘
             │ (same file: db.rs)              │ (same file: flows.rs)
┌────────────▼─────────────┐                   │
│ Epic 1.2 coverage ledger  │                   │
│ (db.rs)                   │                   │
└────────────┬─────────────┘                   │
┌────────────▼─────────────┐                   │
│ Epic 1.3 busy_timeout     │                   │
│ (db.rs)                   │                   │
└────────────┬─────────────┘                   │
             │                                  │
Phase 2 — Unified read-through cache            │
┌────────────▼─────────────┐    ┌───────────────▼────────────────┐
│ Epic 2.1 StalenessTolerance│   │ Epic 2.2 whole-league backfill  │
│ + extend read API (db.rs)  │   │ (flows.rs, pending.rs) — serial │
│                             │   │ after 1.4 (same file: flows.rs) │
└────────────┬───────────────┘   └───────────────┬──────────────────┘
             └───────────────┬───────────────────┘
                 ┌────────────▼─────────────┐
                 │ Story 2.1.2 read_through  │
                 │ orchestrator (new file:   │
                 │ cli/browser/transactions) │
                 └────────────┬─────────────┘
                 ┌────────────▼─────────────┐
                 │ Epic 2.3 CLI + MCP surface │
                 │ (parallel: different files)│
                 └────────────┬─────────────┘
                              │
Phase 3 — Bid-projection model
                 ┌────────────▼─────────────┐
                 │ Epic 3.1 bid_projection.rs │
                 │ 5 signals, serial (1 file) │
                 └────────────┬─────────────┘
                 ┌────────────▼─────────────┐
                 │ Epic 3.2 CLI + MCP surface │
                 │ (parallel: different files)│
                 └────────────┬─────────────┘
                              │
Phase 4 — Edit/cancel live write action
             ┌────────────────┼────────────────┐
┌────────────▼─────────────┐  │  ┌──────────────▼───────────────┐
│ Epic 4.1 pre-write status │  │  │ Epic 4.2 edit_cancel seam flow│
│ re-check (uses 2.1.2)     │  │  │ (flows.rs, builds on 1.4)     │
└────────────┬─────────────┘  │  └──────────────┬───────────────┘
             └─────────────────┴─────────────────┘
                 ┌────────────▼─────────────┐
                 │ Epic 4.3 CLI + MCP surface,│
                 │ LIVE WRITE ACTION framing  │
                 └───────────────────────────┘
```

---

## Phase 1: Risk-First Foundations

The two defects research independently found twice (status monotonicity, missing coverage ledger)
and the one unresolved design fork with no cheap way to retry (edit/cancel row identity) all land
here, before the bulk of Epic 1/2/3's code is built on top of assumptions about them.

### Epic 1.1: Transaction status monotonicity guard

**Goal**: close the defect at [`src/db.rs:460`](../../../src/db.rs) where `status` is unconditionally
overwritten on upsert conflict, with no protection against an out-of-order write regressing a
terminal status (`complete`/`rejected`/`cancelled`) back to `pending`.

#### Story 1.1.1: Add `status_updated`/`source` columns and a CAS-guarded upsert

**As a** cache maintainer, **I want** the `transactions.status` column to only move forward in
GraphQL-epoch order, **so that** a delayed or out-of-order sync write can never silently regress a
resolved claim back to `pending`.

**Acceptance Criteria**:
- A write with an older `status_updated` than the stored row never changes `status`.
  - *Given* `transactions` already holds transaction_id `"998877"` for roster 5 with
    `status="complete"`, `status_updated=1781900000`, *When* `upsert_transaction` is called again
    for `"998877"` with `status="pending"`, `status_updated=1781890000` (ten minutes earlier), *Then*
    the stored row's `status` remains `"complete"` and a `warn`-level log line records the rejected
    downgrade attempt.
- A REST-sourced write (which never carries `status_updated`) never touches `status` on conflict.
  - *Given* the same row above, *When* `sync_transaction_history` (REST path) re-syncs leg 9 and
    calls `upsert_transaction` with `status=Some("complete")`, `status_updated=None`, *Then* the
    stored `status`/`status_updated` are unchanged (REST's own value is simply never applied via the
    CASE branch, matching the existing `waiver_bid` COALESCE's "REST can't erase a browser-sourced
    value" idiom).
- A first-ever `status_updated` value from GraphQL is always accepted (no false "stale" rejection).
  - *Given* transaction_id `"1"` (the pending Jacoby Brissett claim, player_id `3257`, roster_id 5,
    bid $78) exists with `status_updated=NULL` (inserted by an earlier REST sync before any GraphQL
    read happened), *When* `league_transactions_via_seam`'s result is persisted with
    `status="pending"`, `status_updated=1781850000`, *Then* the stored row picks up both the new
    `status` and `status_updated`.

##### Task 1.1.1a: Add `status_updated`/`source` columns (cost class XS)
- Extend the existing `PRAGMA table_info('transactions')` idempotent-migration block
  (`src/db.rs:150-160`) with two more `has_col`/`ALTER TABLE` checks: `status_updated INTEGER`,
  `source TEXT`.
- Files: `src/db.rs`.

##### Task 1.1.1b: Rewrite `upsert_transaction`'s CASE logic; extend `TransactionRow` (cost class S)
- Add `status_updated: Option<i64>` and `source: &'a str` to `TransactionRow` (`src/db.rs:36-48`).
- Replace the plain `status = excluded.status` line (`db.rs:460`) and add a parallel
  `status_updated = CASE ...` clause:
  ```sql
  status = CASE
      WHEN excluded.status_updated IS NULL THEN transactions.status
      WHEN excluded.status_updated >= COALESCE(transactions.status_updated, 0)
          THEN excluded.status
      ELSE transactions.status
  END,
  status_updated = CASE
      WHEN excluded.status_updated IS NULL THEN transactions.status_updated
      WHEN excluded.status_updated >= COALESCE(transactions.status_updated, 0)
          THEN excluded.status_updated
      ELSE transactions.status_updated
  END,
  source = excluded.source,
  ```
  (`source` is a plain overwrite — it's observability metadata, not a correctness-bearing field, so
  it always reflects whichever sync last touched the row.)
- Update every existing call site of `TransactionRow` (`src/analysis/roster.rs:202-214`'s
  `sync_transaction_history`, which must now pass `status_updated: None, source: "rest"`) to compile.
- Files: `src/db.rs`, `src/analysis/roster.rs`.

##### Task 1.1.1c: Unit tests for the monotonicity guard (cost class XS)
- Add `#[cfg(test)]` cases in `src/db.rs` covering: older write rejected + warn logged, REST write
  never touches status, first-ever `status_updated` accepted (the three ACs above).
- Files: `src/db.rs`.

### Epic 1.2: Transaction coverage ledger

**Goal**: close the second defect research found — there is no way today to answer "is this
league's transaction cache missing anything," unlike `weeks_fetched`'s precedent for stat weeks.

#### Story 1.2.1: Add `transaction_sync_runs` and a coverage-check query

**As** the bid-projection tool (Phase 3) and the read-through orchestrator (Phase 2), **I want** a
first-class record of whether a full-league backfill ever completed, **so that** a FAAB-remaining
figure can be labeled a verified total rather than a silent floor.

**Acceptance Criteria**:
- Every successful sync run (REST or GraphQL) records one ledger row.
  - *Given* `league_transactions_via_seam` successfully fetches and persists 187 transactions for
    league `1352883872131481600`, season `2026`, *When* the sync completes, *Then* a
    `transaction_sync_runs` row is inserted with `source='graphql'`, `roster_scope='all'`,
    `rows_fetched=187`, `rows_changed=<N≤187>`, `synced_at=<now>`.
- A coverage query distinguishes "full-league GraphQL backfill exists" from "only REST/partial data."
  - *Given* `transaction_sync_runs` has only `source='rest'` rows for `(league_id=
    "1352883872131481600", season="2026")` (no `roster_scope='all', source='graphql'` row),
    *When* `has_full_league_graphql_coverage("1352883872131481600", "2026")` is called, *Then* it
    returns `None`.

##### Task 1.2.1a: `transaction_sync_runs` table + `TransactionSyncRun` struct + `record_sync_run`/`has_full_league_graphql_coverage` (cost class S)
- `CREATE TABLE IF NOT EXISTS transaction_sync_runs (league_id TEXT NOT NULL, season TEXT NOT NULL,
  source TEXT NOT NULL, roster_scope TEXT NOT NULL, synced_at TEXT NOT NULL, rows_fetched INTEGER
  NOT NULL, rows_changed INTEGER NOT NULL)` added to the same `CREATE TABLE IF NOT EXISTS` block as
  `weeks_fetched`/`transactions` (`src/db.rs:82-140`).
- `StatsDb::record_sync_run(&self, row: TransactionSyncRun) -> Result<()>` and
  `StatsDb::has_full_league_graphql_coverage(&self, league_id: &str, season: &str) ->
  Result<Option<chrono::DateTime<chrono::Utc>>>` (most recent matching `synced_at`, or `None`).
- Files: `src/db.rs`.

---

### Epic 1.3: SQLite concurrency hardening

#### Story 1.3.1: `busy_timeout` on `StatsDb`'s pool

**As** the system, **I want** a concurrent writer into `transactions` to retry-wait instead of
erroring, **so that** a stale-tolerance-triggered browser sync racing a REST sync never silently
fails to persist.

**Acceptance Criteria**:
- `StatsDb::open` configures a non-default busy timeout.
  - *Given* two `StatsDb` handles open against the same `stats.db` file (simulating a REST sync and a
    GraphQL sync running concurrently), *When* one holds the write lock for 200ms and the other
    attempts `upsert_transaction`, *Then* the second call blocks and succeeds rather than returning
    `SQLITE_BUSY`.

##### Task 1.3.1a: Add `.busy_timeout(Duration::from_secs(5))` to `SqliteConnectOptions` (cost class XS)
- `src/db.rs:72-75`'s `SqliteConnectOptions::new()...journal_mode(...)` chain gains
  `.busy_timeout(std::time::Duration::from_secs(5))`.
- Add a doc comment on `StatsDb::open` explaining why (pitfalls.md §5: two independent writers now
  exist against `transactions` for the first time), matching `rate_limit.rs:121`'s existing
  comment-the-concurrency-decision convention.
- Files: `src/db.rs`.

---

### Epic 1.4: Edit/cancel row-identity-disambiguation design spike

**Goal**: resolve the one design fork research flagged as genuinely open (pitfalls.md §3) — how the
edit/cancel flow picks the correct row among several pending claims — before any CLI/MCP wiring
depends on the answer. See ADR-004.

#### Story 1.4.1: `claim_row_disambiguator` + `edit_cancel_claim_via_seam` stub against `FakeSession`

**As a** developer wiring the edit/cancel write action in Phase 4, **I want** the row-selection logic
proven against fixtures now, **so that** Phase 4's CLI/MCP work only has to plug in live-verified
selectors, not redesign disambiguation under time pressure.

**Acceptance Criteria**:
- Given two pending claims whose rendered text both contain "Brissett" (e.g. a claim for Jacoby
  Brissett and an unrelated claim naming a "Brissett Jones"), the disambiguator refuses to pick
  either without a secondary disambiguator and returns `Err`.
  - *Given* `eval_string` returns
    `[{"index":0,"text":"Jacoby Brissett — bid $78","transaction_id":"1"},{"index":1,"text":"Brissett Jones — bid $12","transaction_id":"2"}]`,
    *When* `claim_row_disambiguator` is called with target `"1"` (the known `transaction_id`),
    *Then* it returns `Ok(0)` — resolved by the strong key, not the substring both rows share.
  - *Given* the same two rows, *When* called with a weak fallback key that matches both rows'
    rendered text (no `transaction_id` available), *Then* it returns `Err` naming both candidate
    indices, never guessing the first.
- `edit_cancel_claim_via_seam` is implemented against the seam's existing five primitives only (no
  new `BrowserSession` method), with its own `mod edit_claim_selectors` doc-commented as **unverified**
  (same STUB discipline as `trade_propose_via_seam`, `flows.rs:128-145`), and passes against
  `FakeSession` with a canned `eval_string` response.
  - *Given* a `FakeSession` configured with `with_eval_string_response` returning the two-claim JSON
    above, *When* `edit_cancel_claim_via_seam(&session, league_id, "1", EditCancelAction::Cancel)` is
    called, *Then* it resolves row index 0, attempts the cancel click sequence, and returns `Ok(())`
    against the fake (live selector correctness remains unverified — see Unresolved Questions).

##### Task 1.4.1a: `claim_row_disambiguator` pure function + unit tests (cost class S)
- New private fn in `flows.rs`: takes the parsed `Vec<CandidateRow>` (index, text, optional
  `transaction_id`) and a `target: RowTarget { TransactionId(String), Fallback { bid: Option<i64>,
  add_name: Option<String> } }`; returns `Result<usize>`, `Err` on 0 or >1 matches.
- Files: `src/browser/session/flows.rs`.

##### Task 1.4.1b: `edit_cancel_claim_via_seam` + `mod edit_claim_selectors` stub (cost class M)
- New flow function and selector module (candidate selectors only, explicitly marked unverified),
  following the `trade_propose_via_seam` STUB doc-comment pattern but — unlike that stub — fully
  exercised against `FakeSession` (not Epic-4-cut-disabled), since this project's appetite is full
  scope, not a cut.
- Extend `src/browser/session/fake.rs` if the existing `with_eval_string_response` fixture helper
  doesn't yet support returning different responses for sequential calls within one flow (open the
  claims panel's read vs. the post-click confirmation read may need two distinct canned responses).
- Files: `src/browser/session/flows.rs`, `src/browser/session/fake.rs`, new test module in
  `tests/browser_claim.rs` or a new `tests/browser_edit_cancel.rs`.

---

## Phase 2: Unified Read-Through Cache

### Epic 2.1: Staleness-tolerance type and cache read API

#### Story 2.1.1: `StalenessTolerance` enum; extend `CachedTransaction`/`list_transactions` with `fetched_at`; Tuesday-window-aware default tolerance

**As** the read-through orchestrator (Story 2.1.2), **I want** `list_transactions` to return each
row's `fetched_at`, `status_updated`, and `source`, **so that** a staleness decision can be made
without a second query — and **so that** a caller who supplies no explicit tolerance still gets the
league's own Tuesday-processing-window-aware default rather than a one-size-fits-all guess.

**Acceptance Criteria**:
- `CachedTransaction` includes `fetched_at: chrono::DateTime<chrono::Utc>`.
  - *Given* a row for transaction_id `"1"` was last written at `2026-11-04T18:02:00Z`, *When*
    `list_transactions("1352883872131481600", Some("waiver"))` is called, *Then* the returned
    `CachedTransaction` for `"1"` has `fetched_at == 2026-11-04T18:02:00Z` (today it has no such
    field at all — `db.rs:52-64`'s struct and `db.rs:489-525`'s `SELECT` both omit it).
- A caller supplying no explicit tolerance gets a `pending`-row default that depends on the
  league's actual waiver-processing day, not a fixed guess (cross-artifact BLOCKER: requirements.md's
  resolved Open Question and ux.md §7 both specify this, but no function in this plan implemented it).
  - *Given* league `1352883872131481600`'s `waiver_day_of_week=2` (Tuesday) and `daily_waivers=0`,
    and `now` falls on a Tuesday, *When* `default_tolerance_for("pending", now, 2, false)` is called,
    *Then* it returns `StalenessTolerance::AlwaysLive`.
  - *Given* the same league settings, *When* `now` falls on any other day of the week, *Then*
    `default_tolerance_for("pending", now, 2, false)` returns
    `StalenessTolerance::MaxAge(Duration::hours(24))` — a long, but not infinite, tolerance (ux.md
    §7: "a pending row outside the league's Tuesday waiver-processing window is served from cache
    with a long tolerance").

##### Task 2.1.1a: Extend `CachedTransaction` struct + `list_transactions` SELECT (cost class S)
- Add `fetched_at`, `status_updated: Option<i64>`, `source: Option<String>` fields; extend the
  `SELECT`/tuple-destructure in `list_transactions` (`db.rs:489-525`) to match.
- Files: `src/db.rs`.

##### Task 2.1.1b: `StalenessTolerance` enum (cost class XS)
- `pub(crate) enum StalenessTolerance { AlwaysLive, MaxAge(chrono::Duration), CachedOnly }` per
  architecture.md §3's signature, with a `fn is_fresh(&self, fetched_at: DateTime<Utc>, now:
  DateTime<Utc>) -> bool` method for the one comparison every call site needs.
- Files: `src/cli/browser/transactions.rs` (new file; create it here, populated further in 2.1.2).

##### Task 2.1.1c: `default_tolerance_for` pure function + wiring into the no-explicit-tolerance default path (cost class S)
- Root cause: `waiver_day_of_week`/`daily_waivers` (already fetched as part of league settings)
  appeared nowhere in plan.md's actual logic before this task, even though requirements.md's resolved
  Open Question and ux.md §7 both specify the Tuesday-window-aware default as the intended behavior.
- `pub(crate) fn default_tolerance_for(status: &str, now: DateTime<Utc>, waiver_day_of_week: u8,
  daily_waivers: bool) -> StalenessTolerance` — pure, no DB/browser I/O, unit-testable directly.
  Terminal statuses (`complete`/`rejected`/`cancelled`) return `StalenessTolerance::CachedOnly`
  (cache-valid forever, regardless of day). For `pending`: `daily_waivers == true`, or `now`'s
  weekday matches `waiver_day_of_week`, returns `AlwaysLive`; any other day returns
  `MaxAge(Duration::hours(24))`.
- Wire its use into every "no explicit tolerance supplied" call site: `sleeper transactions` (Task
  2.3.1a, when `--max-age-secs`/`--force` are both omitted) and `get_transaction_history` (Task
  2.3.2a, when `max_age_secs` is omitted) call `default_tolerance_for` per-row-status to compute the
  `tolerance` argument they pass into `read_through`, instead of defaulting to an arbitrary constant.
- Files: `src/cli/browser/transactions.rs`.

#### Story 2.1.2: `read_through` orchestrator

**As a** CLI/MCP caller, **I want** one function that serves cached transaction history or triggers
exactly the right live refresh, **so that** I never have to know which source populated the cache.

**Acceptance Criteria**:
- A `CachedOnly` read on a league with zero cached rows returns an empty result, never a live call.
  - *Given* an empty `stats.db` (no rows for league `1352883872131481600`), *When* `read_through`
    is called with `tolerance = StalenessTolerance::CachedOnly`, *Then* it returns `Ok(vec![])`
    with a `tracing::debug!` span noting `cache_miss, no_live_call (CachedOnly)` and performs no
    REST or browser call.
- An `AlwaysLive` read on a `pending` claim always triggers a refresh, even if the row was fetched
  one second ago.
  - *Given* transaction `"1"` (Jacoby Brissett, bid $78) cached one second ago with
    `status="pending"`, *When* `read_through` is called with `tolerance =
    StalenessTolerance::AlwaysLive` scoped to that transaction, *Then* it triggers
    `league_transactions_via_seam` regardless of the one-second age.
- A `MaxAge` read on a `complete` (terminal) row never triggers a refresh, no matter how old.
  - *Given* a `complete`-status transaction cached 90 days ago, *When* `read_through` is called with
    `tolerance = StalenessTolerance::MaxAge(Duration::hours(1))`, *Then* the terminal-status row is
    served from cache with no live call (ux.md §1: terminal rows are cache-valid forever — the
    `MaxAge` tolerance only gates `pending` rows).
- A `free_agent`-scoped refresh dispatches to REST, not the browser (the one case where REST is
  actually sufficient — see decision table below).
  - *Given* an empty cache for league `1352883872131481600`, *When* `read_through` is called with
    `kind_filter: Some("free_agent")` and `tolerance = StalenessTolerance::MaxAge(Duration::hours(1))`,
    *Then* it calls `analysis.sync_transaction_history` (REST), never `run_league_transactions_sync`
    (browser) — free-agent pickups never carry a FAAB bid and Sleeper processes them immediately
    (never `pending`), so REST's existing, already-correct data fully answers this read.

**Populator-dispatch decision table** (closing adversarial-review Blocker 2 — this is the plan's
headline Strategy-pattern decision, named in Step 0.5 fork #1 and Pattern Decisions, but until now
never stated as a concrete rule):

| Row state after cache lookup | `kind_filter` | Refresh triggered? | Populator | Why |
|---|---|---|---|---|
| Terminal status (`complete`/`rejected`/`cancelled`) present | any | Never (bypasses `tolerance`) | — | Cache-valid forever (3rd AC above). |
| `pending` present, or no row at all | `Some("free_agent")` | Per `tolerance`/`force_refresh` | **REST** (`analysis.sync_transaction_history`) | Free-agent transactions never carry a `waiver_bid` and are never `pending`/`rejected`/`cancelled` in practice (immediate processing) — REST's existing data is already sufficient; a browser launch would cost more for no additional truth. |
| `pending` present, or no row at all | `Some("waiver")`, `Some("trade")`, or `None` (unfiltered) | Per `tolerance`/`force_refresh` | **browser** (`run_league_transactions_sync`) | REST can never report a non-`complete` status or any bid amount at all (requirements.md Problem Statement) — the browser/GraphQL path is the only source of truth for these kinds once a refresh is warranted. |

**Honest note on reachability**: for every `kind_filter` other than `free_agent`, the REST branch of
this dispatch is never reached by design — not a bug, an accepted consequence of REST's own coverage
gap. The Strategy pattern's two populators are therefore asymmetric in practice: the browser
populator is exercised by every `waiver`/`trade`/unfiltered refresh (the common case for this
project), while the REST populator is load-bearing only for `free_agent` reads. This is recorded here
rather than left implicit, per the review's ask — it does not change the chosen design (REST is still
the right, cheaper populator for the one case it serves), only makes its actual reach explicit.

##### Task 2.1.2a: `read_through` function + populator dispatch (cost class M)
- `pub(crate) async fn read_through(config: &Config, analysis: &AnalysisService, league_id: &str,
  kind_filter: Option<&str>, tolerance: StalenessTolerance, force_refresh: bool) ->
  Result<Vec<CachedTransaction>>` per architecture.md §3's signature. Branches: terminal-status rows
  bypass `tolerance` entirely (always served from cache once present); `pending`-status rows and
  "no row at all" cases consult `tolerance`. On a triggered refresh, dispatches per the decision table
  above: `analysis.sync_transaction_history` (REST) when `kind_filter == Some("free_agent")`,
  otherwise the new `run_league_transactions_sync` (browser, Story 2.2.2) — then re-reads from cache
  before returning (requirements § Cache architecture: every live read refreshes the cache for the
  next reader).
- Files: `src/cli/browser/transactions.rs`.

### Epic 2.2: Whole-league GraphQL backfill

#### Story 2.2.1: Generalize `pending_transactions_via_seam` into `league_transactions_via_seam`

**As** the read-through orchestrator, **I want** one GraphQL call that returns the whole league's
transaction history (all rosters, all types, all statuses), **so that** a backfill is one round-trip,
not twelve.

**Acceptance Criteria**:
- The query drops the single-roster filter and widens type/status filters.
  - *Given* league `1352883872131481600`, *When* `league_transactions_via_seam(session,
    "1352883872131481600", leg_filters: [])` is called, *Then* the GraphQL query string sent
    contains `roster_id_filters: []` (not `[5]`), `type_filters: ["waiver","trade","free_agent"]`,
    `status_filters: []` — unlike today's `pending_transactions_via_seam`, which hardcodes a single
    `roster_id_filters: [roster_id]`.
- The response parser extracts `status_updated` and `roster_ids` per entry (needed by Epic 1.1/2.2.2).
  - *Given* a raw response entry `{"transaction_id":"1","status":"pending","status_updated":1781850000,
    "roster_ids":[5],"settings":{"waiver_bid":78},"adds":{"3257":5},...}` (the Jacoby Brissett claim),
    *When* parsed, *Then* the resulting record carries `status_updated == Some(1781850000)` and
    `roster_ids == vec![5]` — fields `parse_pending_transactions` (`pending.rs:58-95`) currently drops
    on the floor.
- A missing (not just empty) `data.league_transactions_filtered` key is a hard `Err`, distinct from a
  legitimately empty result.
  - *Given* a raw response `{"data":{}}` (key absent, no `errors` array — a shape-change signature per
    pitfalls.md §2.1), *When* parsed, *Then* the parser returns `Err` naming the missing key, not
    `Ok(vec![])` — closing the gap pitfalls.md §2.1 flags in the existing
    `.pointer(...).unwrap_or_default()` chain.
- A persisted GraphQL-sourced row's `adds`/`drops` JSON carries the same enriched
  `{player_id, name, position, roster_id, owner}` shape the REST path already produces (architecture
  review Blocker 1) — not the flattened display strings `parse_pending_transactions` emits today.
  - *Given* a raw response entry `{"transaction_id":"1","status":"pending","settings":
    {"waiver_bid":78},"adds":{"3257":5},"drops":{},...}` (the Jacoby Brissett claim, roster 5), *When*
    `run_league_transactions_sync` (Task 2.2.2a) persists it via Task 2.2.1c's resolver, *Then* the
    stored `transactions.adds` JSON for transaction_id `"1"` is
    `[{"player_id":"3257","name":"Jacoby Brissett","position":"QB","roster_id":5,"owner":"<roster 5's
    owner>"}]` — a `position` field is present and non-null, so Story 3.1.2's `GROUP BY position`
    aggregation can actually find this (bid-carrying) row.

##### Task 2.2.1a: `league_transactions_via_seam` in `flows.rs` (cost class M)
- New function generalizing `pending_transactions_via_seam` (`flows.rs:163-205`): drop the
  `roster_id` parameter (or make it `roster_id_filters: &[i64]`, empty = all), widen
  `type_filters`/`status_filters` defaults, same `localStorage`-token-read-entirely-in-page-JS
  discipline (unchanged — no new auth surface).
- Files: `src/browser/session/flows.rs`.

##### Task 2.2.1b: Extend the parser (rename/generalize `parse_pending_transactions`) (cost class M)
- Rename to `parse_league_transactions` (or add a second entry point sharing the core logic) in
  `src/cli/browser/pending.rs`: capture `status_updated`, `roster_ids`; add the missing-key-vs-empty-
  array distinction (positive assertion that `data.league_transactions_filtered` is present as an
  array, per pitfalls.md §2.1's recommendation); add a test for the missing-key case (not just the
  already-tested empty-array case at `pending.rs:187-191`). Also retain each entry's **raw**
  `adds`/`drops` maps (`HashMap<String, i32>`, player_id → roster_id — the same shape REST's
  `Transaction.adds`/`drops` already use, `models.rs:178-179`) on the returned record, alongside (not
  instead of) the existing flattened display-string `Vec<String>` the CLI render still needs — Task
  2.2.1c's enrichment step consumes the raw map, `PendingTransaction::render()` keeps consuming the
  display strings.
- Files: `src/cli/browser/pending.rs`.

##### Task 2.2.1c: GraphQL-side `resolve_side` enrichment, matching the REST shape (cost class S)
- Root cause (architecture review Blocker 1): the REST path's `resolve_side` closure
  (`roster.rs:180-193`) is the *only* code that builds the `{player_id, name, position, roster_id,
  owner}` shape `TransactionRow.adds`/`drops` needs, and it's private to `sync_transaction_history`.
  The GraphQL path's only existing parser, `parse_pending_transactions`, instead produces flattened
  display strings via `describe_player` against the GraphQL response's own thin `player_map` (which
  lacks `age`/`years_exp`/`search_rank` — not enough to call `enrich_player` directly). Rather than
  invent a second enrichment convention against `player_map`, promote `resolve_side` out of its
  closure into a reusable `pub(crate) fn resolve_side(side: &HashMap<String, i32>, all_players:
  &HashMap<String, serde_json::Value>, owner_by_roster: &HashMap<i32, String>) -> serde_json::Value`
  in `roster.rs`, and update `sync_transaction_history` to call it (no behavior change there).
- `run_league_transactions_sync` (Task 2.2.2a) fetches `all_players`/`owner_by_roster` the same way
  `sync_transaction_history` already does (`roster.rs:167-178`: `spawn_all_players()` +
  `list_league_members`), then calls this same `resolve_side` on each entry's raw `adds`/`drops` map
  (Task 2.2.1b) before building the `TransactionRow` — so a GraphQL-sourced row and a REST-sourced row
  go through the identical enrichment function, just fed from a different player/roster-id source map.
- Unit test: given a raw GraphQL entry's `adds: {"3257": 5}` plus a fixture `all_players`/
  `owner_by_roster`, assert `resolve_side(...)` returns JSON containing `"position": "QB"` for
  player_id `"3257"` — proves the AC above without a real browser.
- Files: `src/analysis/roster.rs` (promote `resolve_side` to `pub(crate)`), `src/cli/browser/
  transactions.rs` (or wherever Task 2.2.2a's `run_league_transactions_sync` lives).

#### Story 2.2.2: Persist the whole-league backfill and record the sync run

**As** the read-through orchestrator, **I want** a whole-league GraphQL backfill to write through
`upsert_transaction` and record a `transaction_sync_runs(roster_scope='all')` row, **so that** the
coverage ledger (Epic 1.2) reflects reality.

**Acceptance Criteria**:
- A successful backfill persists every transaction and exactly one coverage-ledger row, and a repeat
  backfill of the same data changes nothing.
  - *Given* `league_transactions_via_seam` returns 187 entries for league `1352883872131481600`,
    *When* `run_league_transactions_sync` processes the result, *Then* `transactions` has ≤187 rows
    changed (idempotent on re-run) and exactly one new `transaction_sync_runs` row with
    `source='graphql', roster_scope='all', rows_fetched=187`.
  - *Given* the same 187-entry result, *When* `run_league_transactions_sync` runs a **second** time
    immediately after with no new data, *Then* the second run's `rows_changed` is `0` (every
    `upsert_transaction` is a no-op ON CONFLICT with identical values) — proving idempotency, not just
    asserting it, since the first `When` alone can't distinguish "idempotent" from "ran once."
- The browser session closes on every branch, including a mid-parse failure.
  - *Given* `league_transactions_via_seam` returns malformed JSON triggering a parse `Err`, *When*
    `run_league_transactions_sync` runs, *Then* `session.close()`/`launched.close()` are still called
    before the `Err` propagates — no leaked headless Chrome process (matching `pending.rs:116-118`'s
    no-`?`-before-close discipline, which this new call must replicate, not assume).

##### Task 2.2.2a: `run_league_transactions_sync` core (cost class M)
- New function in `src/cli/browser/transactions.rs` (or `pending.rs`, whichever stays under
  5-file-touch scope): launch → `league_transactions_via_seam` → close-on-every-branch (`match`, not
  `?`) → on success, loop per entry: resolve `adds`/`drops` via Task 2.2.1c's shared `resolve_side`
  (not the display-string parser), then `upsert_transaction` (with `source: "graphql"`,
  `status_updated: Some(entry.status_updated)`) → `record_sync_run` with `roster_scope: "all"`.
- Files: `src/cli/browser/transactions.rs` (or `src/cli/browser/pending.rs`), `src/db.rs` (call
  sites only, no schema change here).

### Epic 2.3: CLI/MCP surface for unified cache reads

#### Story 2.3.1: CLI `sleeper transactions` subcommand

**As** the operator, **I want** a CLI command that reads the unified cache with a tolerance flag,
**so that** I can inspect transaction history without an assistant.

**Acceptance Criteria**:
- `sleeper transactions --league 1352883872131481600 --max-age-secs 3600` prints cached rows without
  a live call when the cache is warm and within tolerance.
  - *Given* a warm cache for league `1352883872131481600` synced 10 minutes ago, *When* the command
    above runs, *Then* it prints the Jacoby Brissett ($78, pending) row and every other cached row,
    with no browser launch — asserted directly via a call-count check (`FakeSession::eval_calls()
    == 0` / the browser-launch fn's spy count stays at 0), not a wall-clock timing heuristic.

##### Task 2.3.1a: `sleeper transactions` CLI command (cost class S)
- New `clap` subcommand wired to `read_through`, rendering via a `render()` method matching
  `PendingTransaction::render()`'s compact one-line-per-row convention (`pending.rs:31-42`).
- When `--max-age-secs` and `--force` are both omitted, calls `default_tolerance_for` (Task 2.1.1c)
  per row's status to compute the tolerance, instead of a fixed constant.
- Files: `src/cli/mod.rs` (or wherever the `sleeper` top-level `clap::Subcommand` enum lives — same
  file `WaiverAction` is defined in), `src/cli/browser/transactions.rs`.

#### Story 2.3.2: MCP tool for cache-backed transaction reads

**As** the assistant, **I want** an MCP tool that reads the unified cache, **so that** repeated
questions about transaction history never force a browser launch.

**Acceptance Criteria**:
- The tool returns structured JSON with `source`/`cached_at`/age-bucket fields per ux.md §1's
  structured-not-prose recommendation for MCP output.
  - *Given* the same warm cache, *When* the new tool is called with `league_id:
    "1352883872131481600"`, *Then* the JSON result includes, per row, `"source": "graphql"` or
    `"rest"` and `"fetched_at"`, not a formatted sentence.

##### Task 2.3.2a: `get_transaction_history` MCP tool (cost class S)
- New `#[tool]` method on `SleeperMcpServer`, read-only (not a LIVE WRITE ACTION), params
  `{ league_id, kind_filter: Option<String>, max_age_secs: Option<i64> }`.
- When `max_age_secs` is omitted, calls `default_tolerance_for` (Task 2.1.1c) per row's status, same
  as the CLI path.
- Files: `src/mcp.rs`.

---

## Phase 3: FAAB Bid-Projection Model

### Epic 3.1: `bid_projection.rs` core

#### Story 3.1.1: Perceived-value and league-wide-demand signals

**As the** bid-projection tool, **I want** the player's dynasty tier/search_rank and a per-roster
positional-demand pull, **so that** the first two of five required signals are real values, not
placeholders.

**Acceptance Criteria**:
- The output includes the target player's actual `dynasty_tier`/`search_rank`.
  - *Given* a projection for Jacoby Brissett (player_id `3257`, QB), *When* `project_waiver_bid` is
    called, *Then* the result's `perceived_value` signal includes `dynasty_tier` and `search_rank`
    sourced from `enrich_player` (`dynasty.rs:165-192`), not recomputed.
- League-wide demand is computed per-roster, not just for the caller.
  - *Given* league `1352883872131481600`'s 12 rosters, *When* demand for position `QB` is computed,
    *Then* the result reflects a pull across all 12 rosters' `analyze_roster_needs`-style positional
    counts (roster.rs:226+), not just roster 5's.

##### Task 3.1.1a: `bid_projection.rs` module skeleton + perceived-value signal (cost class S)
- New file, `impl AnalysisService` block; reuses `enrich_player`/`dynasty_tier_for_pos`
  (`dynasty.rs:165-192`) verbatim.
- Files: `src/analysis/bid_projection.rs`, `src/analysis/mod.rs` (add `mod bid_projection;`).

##### Task 3.1.1b: League-wide demand signal (cost class S)
- Loop `analyze_roster_needs`-equivalent logic (or call it directly if league-agnostic enough) across
  all rosters for the target position; aggregate into a simple per-position demand count/score.
- Files: `src/analysis/bid_projection.rs`.

#### Story 3.1.2: League's own historical bid-clearing-price and claim-frequency signals

**As the** bid-projection tool, **I want** this league's own historical winning bids by position
*and* how often the target roster itself actually submits a claim, both tagged with sample size,
**so that** a thin sample (e.g. n=3) never looks as confident as a thick one (n=22), and a roster's
bidding habits aren't inferred from the league-wide mean alone.

**Acceptance Criteria**:
- The signal reports `mean_winning_bid` and `sample_size` together, never the mean alone.
  - *Given* league `1352883872131481600` has 4 historical `complete` waiver transactions at QB with
    bids `[12, 8, 15, 5]`, *When* the QB bid-history signal is computed, *Then* it returns
    `BidHistorySignal { mean_winning_bid: 10.0, sample_size: 4, position: "QB" }` — the `n=4` is
    present in the struct, not just mentally implied.
- The aggregation groups by `season` (hook for future recency weighting), even though only one
  season exists today.
  - *Given* only season `"2026"` data exists, *When* the query runs, *Then* its `GROUP BY` includes
    `season`, confirmed by the SQL text, not just by today's single-season result happening to look
    the same either way.
- The target roster's own claim frequency is counted separately from the league-wide bid-history
  signal, as its own named, required signal (requirements.md's "claim frequency" — Success Metrics
  ~line 48-51, Scope §2 ~line 138 — not optional, not folded into `league_demand`).
  - *Given* roster 5 has submitted 7 `type='waiver'` transactions (any status) for league
    `1352883872131481600`, season `"2026"`, *When* the claim-frequency signal is computed for
    roster 5, *Then* it returns `ClaimFrequencySignal { roster_id: 5, claims_this_season: 7,
    position: None }` — a raw count, not a rate or percentile.
  - *Given* the same roster, *When* the signal is computed scoped to position `"QB"` (only claims
    whose `adds` enrichment names a QB target), *Then* `claims_this_season` reflects only the
    QB-targeting subset and `position` is `Some("QB")`.

##### Task 3.1.2a: Bid-history-by-position query + `BidHistorySignal` (cost class S)
- `SELECT position, AVG(waiver_bid), COUNT(*) FROM <resolved transactions> WHERE league_id=? AND
  season=? AND type='waiver' AND status='complete' GROUP BY position, season` — note `position`
  isn't a column on `transactions` directly; resolve it from the already-persisted, pre-resolved
  `adds` JSON blob (`roster.rs:180-193`'s `resolve_side` already stores `position` per add) via
  `list_transactions` + in-Rust aggregation, not raw SQL on a column that doesn't exist.
- Files: `src/analysis/bid_projection.rs`.

##### Task 3.1.2b: Claim-frequency-by-roster signal + `ClaimFrequencySignal` (cost class S)
- Count a given `roster_id`'s own `transactions` rows (`list_transactions`, filtered in Rust —
  same no-new-SQL-column discipline as Task 3.1.2a) where `type='waiver'` and `roster_ids` (JSON
  array) contains the target roster, optionally further filtered to rows whose `adds` enrichment
  names the target `position` (reuses the same per-add `position` field Task 3.1.2a reads, so no
  second data-shape dependency). Counts every status (`pending`/`complete`/`rejected`/`cancelled`),
  not just `complete` — "how often do they even submit a claim" (requirements.md §Scope item 2) asks
  about submission behavior, not win rate.
- `ClaimFrequencySignal { roster_id: i64, claims_this_season: usize, position: Option<String> }`.
- Files: `src/analysis/bid_projection.rs`.

#### Story 3.1.3: FAAB-remaining with explicit coverage confidence

**As the** bid-projection tool, **I want** a roster's remaining FAAB computed only with an explicit
confidence flag, **so that** a partially-backfilled cache never presents a confident wrong number.

**Acceptance Criteria**:
- A verified-coverage league reports `CoverageConfidence::Verified`.
  - *Given* `has_full_league_graphql_coverage("1352883872131481600", "2026")` returns
    `Some(<timestamp>)`, *When* roster 5's FAAB-remaining is computed (`waiver_budget=100` minus
    summed `complete`-status waiver bids for roster 5), *Then* the result's `confidence` is
    `CoverageConfidence::Verified { as_of: <timestamp> }`.
- A league with no full-league GraphQL backfill reports `Floor`, not a bare number.
  - *Given* `has_full_league_graphql_coverage` returns `None` (only REST-sourced, bid-blind
    `complete` rows exist, or only a single-roster `pending_transactions_via_seam` read ever ran),
    *When* roster 5's FAAB-remaining is computed, *Then* the result's `confidence` is
    `CoverageConfidence::Floor { reason: "no full-league GraphQL backfill recorded for season 2026"
    .to_string() }`, and the CLI/MCP rendering makes this unmistakable (ux.md §4a's "don't silently
    substitute a default with the same confidence" instinct).
- The projection surfaces competing-roster FAAB visibility instead of leaving it a permanently
  deferred CONCERN (pre-mortem P1 #1).
  - *Given* the same call for Jacoby Brissett (QB, roster 5), *When* `project_waiver_bid` computes
    league-wide demand (Task 3.1.1b) and identifies the 1-2 other rosters with the highest positional
    pull for QB, *Then* the result's `competing_roster_faab` field contains a `FaabRemaining` (same
    struct, same `CoverageConfidence` gating) for each of those 1-2 rosters, computed via the same
    logic as Task 3.1.3a, just scoped to a different `roster_id`.

##### Task 3.1.3a: `FaabRemaining`/`CoverageConfidence` + computation (cost class M)
- Reads `league.settings.extra.get("waiver_budget")` (no typed `waiver_budget` field exists on
  `LeagueSettings` today — confirmed absent from `src/models.rs:25-36`; read it from the `extra`
  flatten map) minus `SUM(waiver_bid) WHERE status='complete' AND roster_ids CONTAINS roster_id` from
  `list_transactions`, gated by `has_full_league_graphql_coverage`.
- Files: `src/analysis/bid_projection.rs`, `src/db.rs` (if a dedicated summing query is cheaper than
  pulling all rows into Rust — decide based on row count, ~150-200, either is fine; prefer summing in
  Rust over `list_transactions`'s existing output to avoid a new SQL query for a one-season dataset).

##### Task 3.1.3b: Competing-roster FAAB visibility (cost class S)
- Root cause (pre-mortem P1 #1 / adversarial-review Concern, carried forward twice unaddressed):
  `suggested_bid` only ever showed the *target* roster's own FAAB-remaining, with zero visibility
  into whether the 1-2 rosters most likely to actually compete for this claim can even afford to
  outbid it — a number that looks like "what it takes to win" when it's only ever "what I can
  afford."
- Reuses Task 3.1.3a's `FaabRemaining` computation verbatim, called once per competing roster_id
  (identified from Task 3.1.1b's per-position demand signal: the 1-2 non-target rosters with the
  highest positional pull for the target position), rather than inventing a second computation path.
- Adds `competing_roster_faab: Vec<(i64, FaabRemaining)>` to `BidProjection` (roster_id paired with
  its `FaabRemaining`, same `CoverageConfidence` gating as the target roster's own figure — a
  low-coverage league reports `Floor` for these too, never a silently-confident number).
- Files: `src/analysis/bid_projection.rs`.

#### Story 3.1.4: Compose the five signals into `BidProjection`; short/full render

**As a** CLI user or the assistant, **I want** one call that returns all five signals plus a
suggested number, **so that** I get the number and the reasoning behind it in one place.

**Acceptance Criteria**:
- The full struct includes all five named signals with real values — never a bare score.
  - *Given* a call for Jacoby Brissett (player_id `3257`) against roster 5 in league
    `1352883872131481600`, *When* `project_waiver_bid` returns, *Then* the result contains
    non-placeholder `perceived_value`, `league_demand`, `bid_history` (with `sample_size`),
    `claim_frequency` (with `claims_this_season`), and `faab_remaining` (with `confidence`) fields,
    plus a `suggested_bid: i64` and `suggested_bid_pct_of_remaining: f64`. `claim_frequency` is
    required in every response, not an optional/omittable field — it is one of the signals named in
    requirements.md's Success Metrics, not an extra.
- A short-line render matches the existing CLI voice (terse, one line, names the real numbers).
  - *Given* the same projection, *When* `.render_short()` is called, *Then* it produces something in
    the shape `"suggested bid $23 — QB market (n=4, $10 avg), roster 5 has $87 FAAB left (verified),
    league-wide QB demand: moderate, roster 5 has filed 7 QB claims this season"` — one line, real
    numbers, no bare confidence percentage (ux.md §2's explicit guidance against a pseudo-precise
    score).
- A low-confidence `suggested_bid` never renders identically to a well-supported one (pre-mortem P1
  #1 — anchoring risk).
  - *Given* `faab_remaining.confidence == CoverageConfidence::Floor { .. }` or
    `bid_history.sample_size < 5`, *When* `.render_short()` is called, *Then* the rendered line is
    prefixed with a visible caveat marker (e.g. `"low-confidence — "`) ahead of the `suggested bid
    $N` text — never the bare `"suggested bid $N — ..."` shape reserved for a well-supported figure.
    The full struct carries the same signal as a `low_confidence: bool` field (set by the same two
    conditions), so an MCP caller can detect it without re-deriving it from the individual signals.

##### Task 3.1.4a: `BidProjection` struct + composition + `render_short`/full serialize (cost class S)
- Composes all signals (Tasks 3.1.1a/b, 3.1.2a/b, 3.1.3a/b) into one struct, including the new
  `competing_roster_faab` list and a `low_confidence` flag (set when `faab_remaining.confidence ==
  Floor` or `bid_history.sample_size < 5`) that `render_short` checks to prepend the caveat marker.
- Files: `src/analysis/bid_projection.rs`.

### Epic 3.2: CLI/MCP wiring

#### Story 3.2.1: CLI `sleeper waiver bid-projection` command

**Acceptance Criteria**:
- `sleeper waiver bid-projection --league 1352883872131481600 --player 3257 --roster 5` prints the
  short-line render by default.
  - *Given* the command above, *When* run, *Then* stdout contains the short-line render from Story
    3.1.4, and a `--full` flag prints the full structured object for drill-down.

##### Task 3.2.1a: CLI subcommand wiring (cost class S)
- Files: `src/cli/mod.rs` (or the waiver-subcommand enum file), `src/cli/dynasty_cmds.rs` or a new
  sibling file (follow whichever existing file already hosts `dynasty_scout`-shaped commands).

#### Story 3.2.2: MCP `project_waiver_bid` tool

**Acceptance Criteria**:
- The tool returns the full structured object (not the short line) per ux.md §2's "don't hide the
  breakdown from the assistant that might need to defend the number."
  - *Given* the same inputs via MCP, *When* `project_waiver_bid` is called, *Then* the JSON result
    includes every field from `BidProjection`, not a pre-rendered string.

##### Task 3.2.2a: `project_waiver_bid` MCP tool (cost class S)
- Files: `src/mcp.rs`.

---

## Phase 4: Edit/Cancel Live Write Action

### Epic 4.1: Pre-write status re-check

#### Story 4.1.1: Re-check current claim status before any edit/cancel write

**As the** operator, **I want** the tool to confirm a claim is still pending before attempting to
edit or cancel it, **so that** I never fire a write against a claim Sleeper already processed.

**Acceptance Criteria**:
- If the claim already resolved, no DOM write is attempted and the real current state is reported.
  - *Given* transaction `"1"` (Jacoby Brissett, roster 5, bid $78) was `pending` an hour ago but a
    fresh `read_through(AlwaysLive)` check now shows `status="complete"` (Sleeper processed Tuesday's
    waiver run, `waiver_day_of_week=2`), *When* `run_edit_cancel_claim` is called to cancel it,
    *Then* it returns `Err` with triage text in the shape `"claim 1 is no longer pending (now:
    complete) — nothing to edit/cancel. Run \`sleeper transactions --league
    1352883872131481600\` to see its resolved outcome."` (ux.md §4c's exact pattern), and no browser
    DOM write action is attempted.
- If still pending, the write proceeds.
  - *Given* the same transaction still shows `status="pending"` on re-check, *When*
    `run_edit_cancel_claim` is called, *Then* it proceeds to Epic 4.2's live flow.

##### Task 4.1.1a: Pre-write re-check wired into `run_edit_cancel_claim` (cost class S)
- Calls `read_through(..., StalenessTolerance::AlwaysLive, force_refresh: false)` scoped to the one
  transaction_id before invoking the seam flow; new triage fn `edit_cancel_already_resolved_triage`
  matching `pending_failed_triage()`'s shape (`pending.rs:97-99`). `run_edit_cancel_claim` also gains
  a symmetric **post-write** `read_through(AlwaysLive)` re-check after the seam flow returns
  `EditCancelOutcome::Confirmed` (Task 4.2.1b) — wired here since this is where the function's overall
  launch→check→flow→triage shape lives, even though the outcome type it's checking is defined in 4.2.1b.
- Files: `src/cli/browser/actions.rs` (or a new `src/cli/browser/edit_cancel.rs` if `actions.rs`
  would exceed 5 files touched across this epic — `actions.rs` is 188 lines today, has room per
  architecture.md §5).

### Epic 4.2: Live DOM edit/cancel flow

#### Story 4.2.1: `edit_cancel_claim_via_seam` full implementation

**As the** operator, **I want** the live DOM flow to use the Phase 1.4 disambiguation design against
real (unverified) selectors, **so that** the only remaining risk is selector correctness, not logic.

**Acceptance Criteria**:
- Already covered by Story 1.4.1's ACs (the stub from Phase 1 is completed here, not redesigned) —
  this story's only new AC is that it compiles and passes against `FakeSession` end-to-end as part of
  the full `run_edit_cancel_claim` path, not in isolation.
  - *Given* the Phase 1.4 stub and Phase 4.1's pre-write re-check, *When* a full `run_edit_cancel_claim`
    call runs against `FakeSession` with a canned "still pending" response followed by a canned
    claims-panel response, *Then* the whole path (re-check → disambiguate → click sequence) returns
    `Ok(EditCancelOutcome::Confirmed)` without touching real Chrome.
- A mid-flow failure between the write click and its confirmation read returns a distinguishable
  "outcome unknown" result, never a bare `Err` that implies the write didn't happen (adversarial
  review Blocker 3; mirrors pitfalls.md §3's explicit finding and Story 4.3.2's AC, which this task
  makes implementable).
  - *Given* a `FakeSession` configured so the cancel-click `eval_string` call succeeds but the
    *following* confirmation-read `eval_string` call returns an error (simulating a timeout or
    dropped connection between click and confirmation), *When* `edit_cancel_claim_via_seam` runs,
    *Then* it returns `Ok(EditCancelOutcome::Ambiguous { reason: "<detail>" })`, not `Err` — the
    click already fired, so "no result" must never read as "nothing changed."
- The click-stage `eval_string` call re-asserts the target row's identity in the same call that
  performs the click — not a separate round-trip — so a row change between read-time resolution and
  click time is caught, never blindly clicked (architecture-review CONCERN, raised twice and never
  fixed; pre-mortem P1 #2). This is a different situation from the second AC above: here nothing has
  been clicked yet, so the claim is provably still in its original state — zero risk of having
  changed anything — and the outcome is `AbortedBeforeWrite`, never `Ambiguous`.
  - *Given* a `FakeSession` configured so the row at the resolved index no longer matches the target
    `transaction_id` (or fallback predicate) by the time the click-stage `eval_string` runs —
    simulating the claims panel re-rendering between `claim_row_disambiguator`'s read and the click
    (another claim resolved, the DOM reordered) — *When* `edit_cancel_claim_via_seam` runs, *Then* it
    returns `Ok(EditCancelOutcome::AbortedBeforeWrite { reason: "<detail>" })` and never performs the
    click — the re-check and the click are the same `eval_string` invocation, so a changed row is
    caught before any DOM mutation, not after. Nothing was clicked; the claim is unchanged — contrast
    the second AC's post-click case, where the click fired and the outcome is genuinely unknown.

##### Task 4.2.1a: Finish `edit_cancel_claim_via_seam` (bid-edit path) (cost class M)
- The Phase 1.4 stub covers the cancel path's shape; this task adds the edit-bid path
  (`EditCancelAction::EditBid(u32)`): type the new bid via `type_into_selector`, click a save control.
  Selectors remain unverified/candidate-only, doc-commented identically to the cancel path.
- Files: `src/browser/session/flows.rs`.

##### Task 4.2.1b: `EditCancelOutcome` + post-click confirmation read + ambiguous-outcome triage (cost class S)
- Root cause (adversarial review Blocker 3): this plan's other two edit/cancel disciplines each have
  a concrete, tested implementation — "fail closed on no unique match" → `claim_row_disambiguator` (Task
  1.4.1a) and "re-check before writing" → `edit_cancel_already_resolved_triage` (Task 4.1.1a) — but
  "never claim success it can't prove" was asserted only in Story 4.3.2's prose, with nothing building
  it.
- `EditCancelOutcome { Confirmed, AbortedBeforeWrite { reason: String }, Ambiguous { reason: String } }`
  in `flows.rs`, next to `EditCancelAction` (the `AbortedBeforeWrite` variant is Task 4.2.1c's — the
  enum is defined in one place, here, but populated by both tasks). After the write click (cancel or
  edit-bid), `edit_cancel_claim_via_seam` makes one more `eval_string` call reading back the claim
  row's state (e.g. the row is gone from the open-claims list, or shows the new bid) to confirm the
  write landed. A failure/unexpected-content on *that* read — not the write click itself — yields
  `Ambiguous { reason }`, never a bare `Err`.
- This post-click confirmation read is the *only* path that produces `Ambiguous` — the click has
  already fired by the time this check runs, so "outcome unknown" is the only honest label. Contrast
  Task 4.2.1c's pre-click re-assertion, which fails closed into `AbortedBeforeWrite` instead, because
  nothing has been clicked yet and the claim is provably unchanged — a different risk profile that
  must never be reported with the same copy.
- New triage fn `edit_cancel_ambiguous_outcome_triage` (sibling to `edit_cancel_already_resolved_triage`,
  Task 4.1.1a, and to Task 4.2.1c's `edit_cancel_aborted_before_write_triage`), producing text in the
  same "outcome unknown, re-check before retrying" shape pitfalls.md §3 asks for.
- Unit test against `FakeSession`: a canned response sequence where the click-stage `eval_string`
  succeeds and the confirmation-stage `eval_string` call errors; asserts the flow returns
  `Ok(EditCancelOutcome::Ambiguous { .. })`.
- Files: `src/browser/session/flows.rs`, `src/cli/browser/actions.rs` (or wherever Task 4.1.1a's
  triage fns live), new/extended test module from Task 1.4.1b.

##### Task 4.2.1c: Click-stage row-identity re-assertion (cost class S)
- Root cause (architecture-review CONCERN, raised twice, never fixed; pre-mortem P1 #2): row
  identity is resolved once via `claim_row_disambiguator` (Task 1.4.1a) at read time, but nothing
  re-verifies that identity immediately before the click fires — a DOM reorder between resolution and
  click could cancel/edit the wrong, irreversible claim. This is a distinct discipline from Task
  4.2.1b's *post*-click confirmation read (a separate `eval_string` call made *after* the click fires,
  to prove the write landed) — this task closes the gap *before* the click fires, and the two are not
  interchangeable: here nothing has been clicked yet, so the claim is provably still in its original
  state (zero risk of having changed anything), whereas Task 4.2.1b's case is a genuine unknown
  because the click already fired.
- The click-stage JS (the single `eval_string` call `edit_cancel_claim_via_seam` sends to perform the
  click) first re-asserts the target row's identity — by its `data-transaction-id` attribute (or the
  same disambiguation predicate `claim_row_disambiguator` used) — and only calls `.click()` if that
  re-check still finds exactly one matching row at the resolved index. If the re-check finds zero or
  more than one match, the JS returns a sentinel the Rust side maps to
  `EditCancelOutcome::AbortedBeforeWrite { reason: "row identity changed between resolution and
  click" }` instead of firing the click — all inside the one `eval_string` round-trip, so there is no
  window between re-check and click for the DOM to change again. *Given* this re-check fails to find
  a single exact match, *when* the outcome is reported, *then* the text states plainly that nothing
  was clicked and the claim is unchanged — never the post-click case's "outcome unknown" language,
  since nothing here is actually unknown.
- New triage fn `edit_cancel_aborted_before_write_triage` (sibling to Task 4.2.1b's
  `edit_cancel_ambiguous_outcome_triage`), with copy stating the target claim moved before the
  edit/cancel could fire and that it's safe to retry — never the `Ambiguous` triage's "outcome
  unknown, re-check before retrying" wording, since here the tool knows nothing changed.
- Unit test against `FakeSession`: canned response where the claims-panel read matches one row, but
  the click-stage `eval_string` response simulates that row no longer being present/matching; asserts
  `AbortedBeforeWrite`, never `Confirmed` and never `Ambiguous`, and asserts no click-producing side
  effect was recorded by the fake.
- Files: `src/browser/session/flows.rs`, `src/cli/browser/actions.rs` (triage fn), new/extended test
  module from Task 1.4.1b.

### Epic 4.3: CLI/MCP surface, LIVE WRITE ACTION framing

#### Story 4.3.1: `WaiverAction::Edit`/`WaiverAction::Cancel` CLI wiring

**Acceptance Criteria**:
- `sleeper waiver cancel --league 1352883872131481600 --transaction 1` triggers the full
  pre-check → flow → triage path and prints `"claim cancelled"` only once a post-write read-back
  confirms it — never on DOM click alone (adversarial review CONCERN, same discipline as Task 4.2.1b).
  - *Given* transaction `"1"` still pending, *When* the command runs against a real session and
    `edit_cancel_claim_via_seam` returns `EditCancelOutcome::Confirmed`, *Then* `run_edit_cancel_claim`
    performs one more `read_through(..., StalenessTolerance::AlwaysLive, force_refresh: false)` scoped
    to transaction `"1"`; only once that re-read shows `status != "pending"` (for cancel) or the new
    bid (for edit) does the CLI print `"claim cancelled"` — otherwise it prints the exact triage text
    from Story 4.1.1/research/ux.md §4b/§4c, never a bare stack trace on stdout (matching
    `triage_error`'s existing convention).
  - *Given* the same flow instead returns `EditCancelOutcome::Ambiguous { reason }` (Task 4.2.1b),
    *When* the command runs, *Then* it prints `edit_cancel_ambiguous_outcome_triage`'s "outcome
    unknown, re-check before retrying" text — never `"claim cancelled"` and never a bare failure that
    implies the claim is definitely still active.
  - *Given* the same flow instead returns `EditCancelOutcome::AbortedBeforeWrite { reason }` (Task
    4.2.1c), *When* the command runs, *Then* it prints `edit_cancel_aborted_before_write_triage`'s
    "target claim moved before the edit/cancel could fire — nothing changed, safe to retry" text —
    genuinely different copy from the `Ambiguous` branch's "outcome unknown" wording, since here
    nothing was clicked and the tool knows the claim is unchanged.
  - *Given* `edit_cancel_claim_via_seam` instead returns `Err` directly — never reaching any of the
    three typed `EditCancelOutcome` checkpoints (e.g. the claims panel's expected DOM structure
    could not be located at all) — *When* the command runs, *Then* it prints
    `edit_cancel_failed_triage()`'s text ("Edit/cancel failed — the live claims panel may have
    changed since the last verification, or the selector bundle needs live re-verification. Re-run
    with SLEEPER_BROWSER_DEBUG=1 for detail.") — the same generic catch-all shape
    `claim_failed_triage()`/`lineup_failed_triage()` already use for the other two write actions,
    never the `Ambiguous`/`AbortedBeforeWrite` copy above, since this case never reached either of
    those checkpoints, and never a suggestion to run `sleeper browser doctor` (plan.md's
    Observability Plan explicitly does not add a doctor check for selector drift).

##### Task 4.3.1a: `WaiverAction::Edit`/`Cancel` variants + `cmd_waiver` dispatch (cost class S)
- `run_edit_cancel_claim` (Task 4.1.1a/4.2.1b/4.2.1c) gains the post-write re-check described in the
  AC above before returning to this dispatch layer, so the CLI only has to render whichever of
  `{Confirmed-and-verified, Ambiguous, AbortedBeforeWrite, already-resolved, generic-flow-failure}`
  it's handed — no additional branching logic lives in `cmd_waiver` itself.
- New catch-all triage fn `edit_cancel_failed_triage()`, sibling to `claim_failed_triage()`/
  `lineup_failed_triage()` (`actions.rs:25-33`) and identical in shape: "Edit/cancel failed — the
  live claims panel may have changed since the last verification, or the selector bundle needs live
  re-verification. Re-run with SLEEPER_BROWSER_DEBUG=1 for detail." `run_edit_cancel_claim` returns
  this triage, wrapped exactly like `run_claim`'s generic `Err(e)` match arm (`actions.rs:89-96`),
  when `edit_cancel_claim_via_seam` itself returns `Err` — a flow-level failure (e.g. the claims
  panel's expected DOM structure couldn't be located at all) that never reached any of the three
  typed `EditCancelOutcome` checkpoints. This is a fourth, generic arm alongside the three
  typed-outcome arms — not a new `EditCancelOutcome` variant: the three variants exist specifically
  to distinguish safe-vs-risky retry semantics, and a selector-not-found/timeout failure is neither
  — it's the same "something went wrong before reaching a checkpoint" case `claim_failed_triage()`/
  `lineup_failed_triage()` already handle for the other two write actions. Per the Observability
  Plan, this is not a `sleeper browser doctor` check — selector drift isn't a session/login-health
  check in the sense `doctor.rs` covers — so the triage names `SLEEPER_BROWSER_DEBUG=1` as the real
  recovery action, never `sleeper browser doctor`.
- Files: `src/cli/mod.rs` (wherever `WaiverAction` lives), `src/cli/browser/actions.rs`.

#### Story 4.3.2: MCP `edit_waiver_claim`/`cancel_waiver_claim` tools

**Acceptance Criteria**:
- Both tools carry the same "LIVE WRITE ACTION" prefix convention as `waiver_claim`/`lineup_set`
  (`mcp.rs:516`, `mcp.rs:533`), plus the ambiguous-partial-failure caveat from pitfalls.md §3
  ("on any error, re-check pending-claim state before retrying").
  - *Given* the MCP tool descriptions are read by an assistant before calling them, *When*
    `cancel_waiver_claim` is invoked for transaction `"1"` and `edit_cancel_claim_via_seam` returns
    `EditCancelOutcome::Ambiguous { reason }` (Task 4.2.1b — e.g. the DOM write times out between
    click and confirmation), *Then* the tool returns `Err` with text stating the outcome is unknown
    and instructing a re-check via `get_transaction_history`/`get_pending_waivers` before retrying —
    never a bare "cancel failed" that implies the claim is definitely still active.
  - *Given* the same call instead returns `EditCancelOutcome::AbortedBeforeWrite { reason }` (Task
    4.2.1c — e.g. the claims panel reordered before the click fired), *Then* the tool returns `Err`
    with text stating the target claim moved before the write could fire, nothing changed, and it is
    safe to retry — genuinely different copy from the `Ambiguous` case's "outcome unknown," since
    here the tool knows nothing was altered.
  - *Given* the same call instead finds `edit_cancel_claim_via_seam` returning `Err` directly — a
    flow-level failure that never reached any of the three typed `EditCancelOutcome` checkpoints
    (e.g. the claims panel's expected DOM structure could not be located at all) — *Then* the tool
    returns `Err` with `edit_cancel_failed_triage()`'s text, the same generic catch-all copy source
    as Surface 11's CLI rendering (one copy source, two renderers) — never the `Ambiguous`/
    `AbortedBeforeWrite` wording above, and never a `sleeper browser doctor` suggestion (plan.md's
    Observability Plan does not add a doctor check for this failure class).

##### Task 4.3.2a: `edit_waiver_claim`/`cancel_waiver_claim` MCP tools (cost class S)
- Renders `EditCancelOutcome::Ambiguous` and `AbortedBeforeWrite` as distinct `Err` texts (per the
  two ACs above — the first says the outcome is unknown, the second says nothing changed);
  `Confirmed` (post-write-verified, per Task 4.1.1a/4.3.1a) as a plain success result; and a generic
  `Err` returned by `edit_cancel_claim_via_seam` itself (selector/DOM-shape drift, never reaching any
  of the three typed checkpoints) as `edit_cancel_failed_triage()`'s text (Task 4.3.1a) — the fourth,
  catch-all arm alongside the three typed-outcome arms, same copy source as the CLI (Surface 11),
  never a `sleeper browser doctor` suggestion.
- Files: `src/mcp.rs`.

---

## Effort estimate

Bands and weights are **INFERRED** per `/home/tstapler/.claude/skills/sdd/skills/ESTIMATION.md`;
recalibrate against the first real implementer run of this project. No fresh/cache token-type
breakdown is available ahead of execution, so CU here conservatively treats all non-output tokens as
fresh input (weight 1) and output at weight 5 — `CU ≈ tokens × (1 + 4s)` where `s` is the assumed
output share (12% for every implementation task below, per ESTIMATION.md's 10–15% default for
implementation runs).

| Story | Tasks | Cost class | Est. raw tokens | Output share | Est. CU | Wall-clock blocker |
|---|---|---|---|---|---|---|
| 1.1.1 status-monotonicity guard | 3 | S | 55k | 12% | 81.4k | — |
| 1.2.1 coverage ledger | 1 | S | 55k | 12% | 81.4k | — |
| 1.3.1 busy_timeout | 1 | XS | 20k | 12% | 29.6k | — |
| 1.4.1 edit/cancel disambiguation design + stub | 2 | M | 130k | 12% | 192.4k | — |
| 2.1.1 extend `CachedTransaction`/`list_transactions` + Tuesday-aware default-tolerance fn | 3 | M | 130k | 12% | 192.4k | — |
| 2.1.2 `read_through` orchestrator | 1 | M | 130k | 12% | 192.4k | — |
| 2.2.1 generalize GraphQL seam flow + parser + shared `resolve_side` enrichment | 3 | M | 130k | 12% | 192.4k | — |
| 2.2.2 persist backfill + record sync run | 1 | M | 130k | 12% | 192.4k | — |
| 2.3.1 CLI `transactions` subcommand | 1 | S | 55k | 12% | 81.4k | — |
| 2.3.2 MCP transaction-history tool | 1 | S | 55k | 12% | 81.4k | — |
| 3.1.1 perceived-value + league-demand signals | 2 | S | 55k | 12% | 81.4k | — |
| 3.1.2 bid-history-by-position + claim-frequency signals | 2 | M | 130k | 12% | 192.4k | — |
| 3.1.3 FAAB-remaining + coverage confidence + competing-roster FAAB visibility | 2 | M | 130k | 12% | 192.4k | — |
| 3.1.4 compose `BidProjection` + render | 1 | S | 55k | 12% | 81.4k | — |
| 3.2.1 CLI bid-projection command | 1 | S | 55k | 12% | 81.4k | — |
| 3.2.2 MCP `project_waiver_bid` tool | 1 | S | 55k | 12% | 81.4k | — |
| 4.1.1 pre-write status re-check | 1 | S | 55k | 12% | 81.4k | — |
| 4.2.1 finish `edit_cancel_claim_via_seam` + `EditCancelOutcome` confirmation read + click-stage identity re-assertion | 3 | M | 130k | 12% | 192.4k | Live-UI selector verification — owner: Tyler, own machine, real display |
| 4.3.1 CLI edit/cancel wiring | 1 | S | 55k | 12% | 81.4k | Waiver-window (Tuesday) cadence to live-test against a real pending claim — owner: Tyler |
| 4.3.2 MCP edit/cancel tools | 1 | S | 55k | 12% | 81.4k | — |

**Sum (20 stories, 32 tasks; 1 XS + 11 S + 8 M)** — recomputed after the Phase-4 validation repair
loop added Tasks 2.1.1c, 3.1.3b, and 4.2.1c (bumping Story 2.1.1 from class S to M for its new
Tuesday-aware default-tolerance function, and bringing Stories 3.1.3/4.2.1 to 2/3 tasks respectively
at unchanged class M), on top of the earlier Phase-3 repair loop's Tasks 2.2.1c, 3.1.2b, and 4.2.1b:
20k + 11×55k + 8×130k = **≈1.665M raw tokens** → 29.6k + 11×81.4k + 8×192.4k = **≈2.464M CU
pre-verification**.

**Verification multiplier**: ×1.75 (midpoint of the ESTIMATION.md ×1.5–2 band — spec-compliance +
code-quality review per story, `6-verify` layers, CI reruns) → **≈4.31M CU total**.

This lands at the low end of the **Large** band (3–15M CU per ESTIMATION.md's table), consistent
with requirements.md's own informational Size estimate — no scope change implied.

**Critical path in agent waves** (file-collision-aware — tasks touching the same file within a phase
serialize even when logically independent):

1. **Wave 1 (parallel)**: 1.1.1 (db.rs) ‖ 1.4.1 (flows.rs, fake.rs — different files, runs
   concurrently with the db.rs chain).
2. **Wave 2**: 1.2.1 (db.rs, after 1.1.1) ‖ 2.2.1 (flows.rs, after 1.4.1 — same-file serialization,
   not a logical dependency).
3. **Wave 3**: 1.3.1 (db.rs, after 1.2.1) ‖ 2.2.2 (pending.rs/transactions.rs, after 2.2.1).
4. **Wave 4**: 2.1.1 (db.rs, after 1.3.1 — all Phase-1 schema work must land first).
5. **Wave 5**: 2.1.2 (new file, after 2.1.1 and 2.2.2).
6. **Wave 6 (parallel)**: 2.3.1 ‖ 2.3.2 (different files, both after 2.1.2).
7. **Wave 7 (serial, one new file)**: 3.1.1 → 3.1.2 → 3.1.3 → 3.1.4 (all touch
   `bid_projection.rs`; depends on 2.1.2's cache + 1.2.1's coverage ledger).
8. **Wave 8 (parallel)**: 3.2.1 ‖ 3.2.2 (after 3.1.4).
9. **Wave 9 (parallel)**: 4.1.1 ‖ 4.2.1 (different files; both depend on 2.1.2 and 1.4.1
   respectively).
10. **Wave 10 (parallel)**: 4.3.1 ‖ 4.3.2 (after 4.1.1 and 4.2.1).

Ten waves on the critical path; most waves fan out two tasks, keeping wall-clock dominated by the
blockers below, not by agent-run count.

**Overrun checkpoint**: if cumulative spend exceeds **≈5.39M CU** (25% over the 4.31M CU table value)
for the stories completed so far, pause and report the delta with cause (wrong estimate vs. a real
surprise) — informational, not a cut trigger.

**Wall-clock blockers** (the only calendar-time items in this plan):
- Live-UI selector verification for the edit/cancel claims panel (Story 4.2.1 / Epic 4.3) — owner:
  Tyler, on his own machine with a real display; this sandbox cannot do it (requirements.md
  Feasibility Risks).
- The league's Tuesday waiver-processing window (`waiver_day_of_week=2`) gates live-testing
  cancel/edit against a genuinely pending real claim without waiting for the next cycle — owner:
  Tyler.
- Confirming whether Sleeper's `/graphql` ever returns a non-2xx HTTP status on auth failure
  (pitfalls.md §2.2) — owner: Tyler, needs a live authenticated session; can piggyback on the first
  blocker's session rather than requiring a separate one.

---
