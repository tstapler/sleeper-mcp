# Research: Technology Stack — waiver-intelligence

**Question**: which libraries/frameworks/versions/patterns apply; what new
dependencies are needed; what are current community-recommended versions.

## Headline finding

**No new crates are needed for any of the three epics.** The repo's existing
dependency set — sqlx/SQLite, chromiumoxide via the `BrowserSession` seam,
serde_json, chrono — already covers the unified cache, the bid-projection
arithmetic, and the new DOM write flow. This is additive work on top of
patterns the repo already has live and tested, not a new-stack decision.

## Current versions: Cargo.toml constraint vs Cargo.lock vs crates.io latest

Checked directly against the crates.io API (`curl
https://crates.io/api/v1/crates/<name>`, 2026-11-10), not web-search summaries
(the web search results for `rmcp`/`sqlx`/`chromiumoxide` were self-contradictory
across stale docs.rs/algolia snapshots — crates.io's own API is the only
VERIFIED source below).

| Crate | `Cargo.toml` constraint | `Cargo.lock` (resolved) | crates.io latest (2026-11-10) | Action |
|---|---|---|---|---|
| `tokio` | `"1"` (full) | 1.52.3 | 1.54.1 | None — constraint already floats within 1.x; next `cargo update` picks it up. |
| `serde` | `"1"` (derive) | 1.0.228 | 1.0.229 | None — same as above. |
| `serde_json` | `"1"` | 1.0.149 | 1.0.154 | None. |
| `sqlx` | `"0.8"` (runtime-tokio, sqlite) | 0.8.6 | **0.9.0** | No action for this feature. 0.9 is a new major-ish line past the pinned `"0.8"` constraint; nothing in the three epics (plain `SqliteConnectOptions`/`SqlitePoolOptions`, `query`/`query_as`, `INSERT OR REPLACE`, `ALTER TABLE ADD COLUMN` idempotent-migration pattern) needs anything 0.9 adds. Treat as a separate, deliberate upgrade later, not bundled into this feature. |
| `chromiumoxide` | `"0.9.1"` (exact) | 0.9.1 | 0.9.1 | **Already current** — repo is pinned to the latest release. |
| `rmcp` | `"0.1"` (server, transport-io) | 0.1.5 | **3.9.0** | Large gap (0.1→3.x is several breaking majors: tool macros, transport API, `tool_box` attribute surface have all changed upstream). Out of scope to migrate as part of this feature — new tools (cache-refresh, bid-projection, claim-edit/cancel) should be added using the *existing* 0.1.5 `#[tool(tool_box)]` pattern already proven in `src/mcp.rs` (`waiver_claim`/`lineup_set`). Flagging the gap for a future standalone upgrade project, not blocking this one. |
| `chrono` | `"0.4"` (serde) | 0.4.44 | (0.4.x current line) | None. |
| `anyhow`/`thiserror` | `"1"` | 1.0.102 / 1.0.69 (+transitive 2.0.18) | — | None relevant. |

**Community-recommended pattern note**: sqlx 0.8/0.9 and chromiumoxide 0.9.x
are both the maintained-current lines for their purpose (sqlx = async
SQL toolkit w/ compile-time-optional query checking; chromiumoxide = async
CDP-based Chrome driver, tokio-only) — no alternative crate is better-fit here
given the repo already has working, tested integration code against both.

## Epic 1 — Unified read-through cache

**No new crate.** The repo already has two independent precedents to merge,
not invent from scratch:

1. **Row-level `fetched_at` + caller-supplied max-age**, already live in
   `src/cli/sync.rs:294-314` (`maybe_sync`): reads a stored `chrono`
   timestamp, computes `chrono::Utc::now() - synced_at`, compares
   `age.num_seconds() > max_age_secs`. This *is* the per-call
   staleness-tolerance mechanism the requirements doc asks for — it just
   needs generalizing from "league sync" to every cached data kind
   (transactions, roster snapshots, FAAB-remaining derivations).
2. **`(value, is_fresh: bool)` return shape**, already live in
   `src/cache.rs:71-98` (`HttpCache::get`): returns the cached row plus
   whether it's within TTL, leaving the caller to decide whether to
   revalidate. The new unified cache's read API should return the same
   shape — `(CachedRow, is_fresh)` or `Option<(Row, Staleness)>` — so a
   caller can inspect why a refresh did/didn't happen (requirements'
   Observability Requirements: log cache-hit vs missing-row vs
   stale-per-tolerance vs force-refresh).

`StatsDb` (`src/db.rs`) already has the right shape for "one cache layer,
two sources": the `transactions` table has `roster_ids`/`waiver_bid` columns
added via the idempotent `ALTER TABLE ... ADD COLUMN` + `PRAGMA table_info`
check (`src/db.rs:144-160`) specifically so public-REST-sourced and
browser-GraphQL-sourced rows share one schema — the doc comment on
`TransactionRow` (`src/db.rs:32-35`) already states this is the merge point.
The `fetched_at` column already exists on `transactions`, `roster_snapshots`,
and `leagues`. Epic 1's work is: (a) add a `status_checked_at` or similar
column distinct from `fetched_at` for pending-claim-status freshness
specifically (status can go stale independent of row insertion time), (b)
add a small staleness-tolerance type/enum (e.g. `enum Freshness { Any,
MaxAge(Duration), Force }`) threaded through the read methods, (c) wire the
whole-league browser-GraphQL backfill (generalizing
`pending_transactions_via_seam`, which today takes one `roster_id` — see
Epic 3 note below) as the second sync source alongside the existing REST
sync in `src/cli/sync.rs`.

No stats/caching crate (e.g. `moka`, `cached`) is warranted — row count is
~150-200 transactions for one league, one season; a plain SQLite table with
an indexed `fetched_at`/`status_checked_at` column, queried with `sqlx`, is
sufficient and consistent with the rest of the codebase's "SQLite is the
whole persistence layer" convention (no Redis/embedded-KV precedent exists
or is needed here).

## Epic 2 — FAAB bid-projection model

**Confirmed: no ML, no stats crate.** This is small-N arithmetic over
already-cached data:

- Perceived player value: reuse `dynasty_tier`/`search_rank` straight from
  `src/analysis/dynasty.rs` (`enrich_player`, `dynasty_tier_for_pos`) — no
  new signal source.
- League bidding norms / per-roster aggressiveness: `GROUP BY roster_id`
  mean/median/percentile over the cached `transactions.waiver_bid` column —
  plain SQL aggregation (`sqlx::query_as` with `AVG`/`COUNT`, or pull rows
  and compute in Rust) or `.sort()` + index into a `Vec<f64>` for a
  percentile. 12 rosters × ~150-200 rows total does not justify pulling in
  `statrs` or any stats crate — hand-rolled mean/percentile is a few lines
  and already how `src/analysis/*.rs` does every other ranking (e.g.
  `available_players`'s `sort_by`/`partial_cmp` in
  `src/analysis/waivers.rs:110-116`).
- Remaining FAAB: `league.settings` (`LeagueSettings` in `src/models.rs`,
  which already `#[serde(flatten)]`s unmodeled fields including
  `waiver_budget` into `extra: HashMap<String, Value>` — no new fetch
  needed, just add an explicit `waiver_budget` field or read it via
  `extra.get("waiver_budget")`) minus `SUM(waiver_bid) WHERE status =
  'complete' AND roster_ids CONTAINS roster_id` from the cache. The
  "partial-history cache must not silently present a confident wrong
  number" rabbit hole (requirements doc) is a correctness/completeness
  check on the sync side (Epic 1), not a stack question — e.g. track a
  per-roster `backfill_complete: bool` or season-coverage marker rather than
  assuming any non-empty cache is a full history.

This confirms the requirements doc's own framing ("no ML needed... arithmetic/
heuristic") — there is nothing fancier the stack needs here.

## Epic 3 — Browser DOM write-action flow (edit/cancel pending claim)

**No new crate.** Follow the exact `src/browser/session/flows.rs` pattern
already used for `claim_player_via_seam`/`lineup_set_via_seam`:

- A new `claim_edit_selectors` (or similar) private `mod` block at the top
  of `flows.rs`, matching the `claim_selectors`/`lineup_selectors` shape —
  selectors must carry the same "live-verified against real sleeper.com,
  never guessed" doc-comment discipline those two already use, but since
  Epic 3's selectors are **not yet verified** (per requirements' Feasibility
  Risks / Open Questions), the new module's doc comment must say so
  explicitly and the flow function should fail loud rather than guess, same
  as `trade_propose_via_seam`'s STUB pattern (`src/browser/session/flows.rs:128-145`)
  until live-verified on Tyler's own machine.
- The flow function itself goes through the same `BrowserSession` trait
  primitives only — `goto`, `wait_for_selector`, `click_selector`,
  `click_in_row`, `eval_bool`/`eval_string`, `type_into_selector`
  (`src/browser/mod.rs:102-148`). These already enforce the
  `element.click()`/`dispatchEvent`-only constraint (no CDP `Input.*`) at
  the trait-object level, so the new flow gets that constraint for free by
  construction rather than needing to re-litigate it.
- Test it against `FakeSession` (`src/browser/session/fake.rs`), which
  already supports canned `eval_string` responses
  (`with_eval_string_response`) for fixturing a GraphQL-shaped reply, and
  logged-in/logged-out/transport-error/signal-split doubles for the
  triage-path tests — the same harness `tests/browser_claim.rs` and
  `tests/browser_flows.rs` already exercise.
- CLI + MCP wiring follows `src/cli/browser/pending.rs`'s
  `run_pending_transactions` → `cmd_pending_waivers` shape (launch session →
  call seam flow → close session on every branch → triage error vs. dead
  session vs. success) and `src/mcp.rs`'s `waiver_claim`/`lineup_set`
  `#[tool(description = "LIVE WRITE ACTION — ...")]` framing
  (`src/mcp.rs:516-538`) — the new edit/cancel tool must carry the same loud
  "LIVE WRITE ACTION" prefix per the requirements doc's Risk Control section.
- Auth stays entirely in-page: reuse `pending_transactions_via_seam`'s exact
  localStorage-token-read-and-use-inside-`eval_string`-JS approach
  (`src/browser/session/flows.rs:163-205`) for any new write call that needs
  the GraphQL mutation (if claim edit/cancel turns out to go through
  `league_transactions_filtered`'s GraphQL mutation counterpart rather than
  pure DOM clicks — undetermined until the live selector/network discovery
  happens). Either way, the token never crosses into a Rust value, consistent
  with the constraint already enforced for reads.

## Summary for Phase 3 planning

- Dependencies to add to `Cargo.toml`: **none**.
- Versions to bump: **none required**; `sqlx` 0.8→0.9 and `rmcp` 0.1→3.x are
  both real, large upgrades available upstream but neither is needed by this
  feature and both are flagged as separate future work, not bundled here.
- New code is additive within existing modules/patterns: `src/db.rs` (new
  columns + staleness-aware read methods on `StatsDb`), `src/cache.rs`-style
  `is_fresh` return shape generalized, `src/analysis/waivers.rs` (new
  bid-projection function, plain arithmetic), `src/browser/session/flows.rs`
  (new flow + selector module, gated unverified), `src/cli/browser/*.rs` +
  `src/mcp.rs` (new CLI subcommand + MCP tool, LIVE WRITE ACTION framing).
