# Build vs. Buy — waiver-intelligence

Research for Phase 2 (SDD) covering the three epics in
`project_plans/waiver-intelligence/requirements.md`: (1) unified read-through
cache with per-call staleness tolerance, (2) FAAB bid-projection heuristic,
(3) browser DOM edit/cancel of a pending waiver claim.

## Current-state baseline (what already exists in this repo)

- `src/cache.rs` (`HttpCache`) — SQLite-backed, per-row `ttl_secs`, `is_fresh`
  check against `cached_at`, ETag/Last-Modified revalidation, a `ttl_override`
  param on write. This is a hand-rolled read-through cache with a *per-call*
  staleness override already — one layer of exactly what Epic 1 asks for,
  just scoped to raw HTTP bodies.
- `src/db.rs` (`StatsDb`) — permanent SQLite store (player stats, leagues,
  transactions). `league_synced_at()` returns a timestamp; `src/cli/sync.rs:300`
  (`maybe_sync`) does the staleness check itself: caller passes `max_age_secs`,
  compares against `chrono::Utc::now() - synced_at`. This is the *exact*
  "staleness tolerance as a caller-supplied parameter, not a global constant"
  pattern Epic 1 requires — already proven in production for one table.
- `transactions` table already has a `waiver_bid` column populated only by
  the authenticated-browser path, merged via `COALESCE(excluded.waiver_bid,
  transactions.waiver_bid)` on conflict so a public-REST re-sync can't
  clobber a bid the browser path already recorded (`src/db.rs:446-483`). This
  source-merge logic is business-specific — no generic cache crate knows this
  rule.
- `dynasty_scout`/`dynasty_tier`/`search_rank` (`src/analysis/dynasty.rs`,
  `src/mcp.rs:436`) is the existing player-value signal Epic 2 is told to
  reuse, plus `get_trade_value` (FantasyCalc-backed, `src/mcp.rs:449`).
- Epic 3's pattern is fully established by `claim_player_via_seam` /
  `lineup_set_via_seam` (`src/browser/session/flows.rs:49-121`): a
  `BrowserSession` trait seam (`click_selector`/`wait_for_selector`/
  `eval_bool`/`type_into_selector`/`click_in_row`), a live-verified selector
  bundle in its own `mod ..._selectors`, a `FakeSession` for unit tests
  (`src/browser/session/fake.rs`), and "LIVE WRITE ACTION" framing in the MCP
  tool description (`src/mcp.rs:516`, `:533`). Edit/cancel is new selectors on
  the same seam, not a new architecture.

This baseline matters for every verdict below: the two cache mechanisms this
feature needs to unify already exist as two *separate, slightly different*
hand-rolled implementations in this codebase. The job is reconciling them
into one, not introducing caching to a codebase that has none.

## 1. Existing OSS library/framework

### 1a. Generic read-through / stale-while-revalidate cache over SQLite

| Option | Pros | Cons | Verdict |
|---|---|---|---|
| **Hand-roll** (extend `src/cache.rs`'s pattern to a generic `freshness(kind, key, tolerance) -> Fresh/Stale/Missing` helper over the existing tables) | Reuses the exact pattern already proven twice in this repo; keeps the COALESCE source-merge rule (business logic no generic crate knows about) in the same place as the staleness check; zero new dependency; trivially testable (pure function of three i64s) | None material | **Recommended** |
| `cacheflight` (crates.io, SWR + XFetch + cold-miss dedup, store-agnostic) | Real SWR semantics, early-expiration jitter | Store-agnostic means it still needs a SQLite backend written by hand — doesn't remove the SQLite code, just wraps it in another abstraction; XFetch/jitter are for high-QPS multi-client dedup (thundering herd), which doesn't exist here (single operator, single process, interactive calls); would sit *beside* `src/db.rs`'s domain tables rather than inside them, since this feature's staleness check is per-data-kind (pending-claim vs. bid-history) not per-cache-key the way a generic SWR cache assumes | **Not recommended** — solves a concurrency problem this tool doesn't have, at the cost of a second cache abstraction next to the one in `src/cache.rs` |
| `sqlite-cache` (crates.io, SQLite-backed KV cache) | Does give a SQLite-backed store directly | No SWR/staleness-tolerance concept — it's a plain KV cache; last released 2024, low adoption (checked via crates.io/lib.rs listing); would still need the per-call staleness-tolerance logic hand-written on top, so it saves nothing over extending `src/cache.rs` | **Not recommended** |
| `moka` / in-memory caches (`moka`, `stretto`) | Mature, widely used, great for hot in-memory LRU/TTL | Wrong layer entirely — this cache must survive process restarts (single operator, interactive CLI invocations, not a long-running server) and be queryable directly via `sqlite3 stats.db` for debugging (an explicit existing habit in this codebase, per `src/db.rs:445` comment). An in-memory cache buys nothing the SQLite table doesn't already give for free | **Not recommended** |

**Why build wins here**: the staleness-tolerance requirement is not "is this
row younger than N seconds" in isolation — it's "is this row younger than N
seconds, where N depends on the *data kind* (pending-claim status vs.
bid-history) and the merge-on-write rule depends on *data provenance*
(public REST vs. browser GraphQL)." That logic is domain-specific to this
project's two data sources and already half-written in two places
(`src/cache.rs`, `src/cli/sync.rs`). No crate can know the COALESCE rule or
the per-kind default table; adopting one would mean writing just as much
glue code to bridge it to this schema, with a new dependency and a new
mental model on top.

### 1b. Fantasy-sports/FAAB-projection crate

Confirmed via crates.io/web search: **nothing exists.** Search for "FAAB
bid fantasy football crates.io", "rust fantasy football projection
library" returned only editorial content (RotoBaller weekly columns) and
generic fantasy-football API wrapper crates (league data fetchers), none
with any bid-projection, auction-value, or budget-modeling logic. This is
expected — FAAB bid sizing is a narrow, league-norm-dependent heuristic
(this league's own historical aggressiveness, not a transferable model), not
a general algorithm worth packaging. **Build is correct here by default**,
not by elimination of close competitors.

### 1c. Browser DOM-automation crate beyond chromiumoxide

Already a dependency: `chromiumoxide = "0.9.1"` (CDP-based), the foundation
for the project's entire stealth/non-synthetic-input policy
(`src/browser/session/stealth.rs`). Alternatives surveyed:

| Option | Pros | Cons | Verdict |
|---|---|---|---|
| **Extend existing `BrowserSession` seam + chromiumoxide** | Zero new dependency; the edit/cancel flow needs the same primitives (`click_selector`, `wait_for_selector`, `click_in_row`) already built for claim/lineup; `FakeSession` test harness already exists | None | **Recommended** |
| `fantoccini` / `thirtyfour` (WebDriver-protocol crates) | Higher-level ergonomics for some flows | WebDriver, not CDP — would require a `chromedriver`/`geckodriver` process alongside Chrome, a second automation stack, and does not reuse any of `stealth.rs`'s CDP-specific anti-detection work; a second DOM-automation dependency for one new flow | **Not recommended** |
| `headless_chrome` crate | CDP-based like chromiumoxide | Direct competitor to the already-adopted crate for the exact same role; swapping mid-project for one new flow is pure churn with no capability gain | **Not recommended** |

Nothing new is needed for Epic 3's *library* layer — the open problem
(per Rabbit Holes) is selector discovery against the live DOM, not tooling.

## 2. SaaS / managed API

| Option | Pros | Cons | Verdict |
|---|---|---|---|
| **Sleeper's own "Suggested FAAB Bids"** (native feature, toggleable per-league in general settings — [support.sleeper.com/en/articles/12111984](https://support.sleeper.com/en/articles/12111984-suggested-faab-bids)) | Already live in the UI this project's browser seam visits; zero integration cost beyond reading it off the DOM if ever wanted | It's a generic per-player suggested *range*, not personalized to this league's 12-roster bidding history, remaining-budget state, or claim frequency — the three signals Epic 2 explicitly requires; no API, DOM-read-only, and not the kind of structured data a projection model should be *built from* (no documented methodology) | **Not recommended as a replacement** — could be read opportunistically as one more signal later, but doesn't cover the requirement |
| **FantasyPros Waiver Planner** (ML-based winning-bid prediction, [support.fantasypros.com/.../Waiver-Planner](https://support.fantasypros.com/hc/en-us/articles/40901168902811-What-is-Waiver-Planner-and-How-Does-it-Work)) | Real ML model, covers Sleeper, factors league scoring + roster need + win-bid prediction — closest commercial match to this epic's intent | Requires a paid subscription tier (exact tier gated behind their site, not confirmed free); "Submit Plan" write-back needs *their* Chrome extension, a second, unrelated automation surface this project would have no control over and that conflicts with this project's own DOM-write policy (`element.click()`-only, no synthetic input — FantasyPros' extension internals are unverified against that bar); API access is listed as a premium feature marked "personal use only" with no confirmed endpoint for Waiver-Planner bid data specifically (unverified — would need a support ticket to confirm); introduces a second paid dependency and external-service coupling for a single-league personal tool where the win is marginal (12 rosters, ~150-200 transactions/season, well within what a local heuristic can model directly from already-cached data) | **Viable but not recommended** — the marginal lift over a cache-grounded local heuristic doesn't justify the cost/lock-in for a single-operator, single-league tool; revisit only if the in-house model's miss rate against actual winning bids proves bad enough to justify a subscription |
| **FantasyPros API (general)** | Documented API access tier exists | Confirmed to exist but un-confirmed to expose Waiver Planner's bid-prediction output specifically (the search surfaced the "API Access" feature on a pricing page, not endpoint docs) — would need direct vendor confirmation before counting on it at all | **Not recommended** without further vendor contact; out of scope to chase for a personal tool |

**Verdict**: build. This is a single dynasty league with ~150-200
transactions/season — the FAAB-projection requirement (player value ×
league bidding norms × remaining budget × claim frequency) is well within
what the already-cached transaction history supports directly, and every
SaaS option either doesn't cover personalized league history (Sleeper's own
suggestion) or adds cost/lock-in disproportionate to a 12-roster personal
tool (FantasyPros).

## 3. LLM-generated vs. battle-tested library (staleness/cache-freshness logic)

**Verdict: hand-rolled is not reckless here — it's the right level of
tooling, and the project has already validated this twice.**

The actual complexity of the staleness check is: compare `now - cached_at`
against a caller-supplied (or per-kind-default) tolerance in seconds, decide
`Fresh`/`Stale`/`Missing`, and on a live refresh, write back with the
source-specific merge rule (COALESCE for `waiver_bid`). That's:

- **Single SQLite table per data kind**, no sharding, no distributed
  invalidation.
- **Single process**, interactive CLI/MCP calls — no concurrent-writer race
  the check needs to defend against beyond what sqlx's WAL mode (already
  configured, `SqliteJournalMode::Wal` in both `src/cache.rs:36` and
  `src/db.rs:75`) already serializes.
- **No cache eviction policy needed** — a season's data (~150-200 rows) never
  needs to be evicted for space; "staleness" here means "should I trust this
  row's *content*," not "should I free this slot."

A battle-tested caching library (Moka, cacheflight, etc.) earns its
complexity budget when it's solving thundering-herd dedup, LRU eviction
under memory pressure, or multi-node cache coherency — none of which apply.
Pulling one in here would mean learning its API, bridging its abstraction to
a SQLite-column-per-table scheme it wasn't designed for, and still writing
the exact same per-kind-default lookup table this project needs regardless.
The two existing hand-rolled instances (`HttpCache`, `maybe_sync`) are each
under 30 lines of actual freshness logic and have run in production for this
project already — extending that shape to a third (generalized,
per-data-kind) table is proportionate, not reckless. The one place rigor
*is* warranted: the per-kind default tolerances need unit tests pinning the
`Fresh`/`Stale`/`Missing` boundary (off-by-one on the comparison operator is
the realistic bug class here), which is cheap regardless of hand-rolled vs.
library.

## 4. Fork or adapt

GitHub/web search for an existing "Sleeper waiver assistant" or "FAAB bid
calculator" project:

| Project | Relevance | Forkable? |
|---|---|---|
| **Fantasy GM** (`github.com/younim2837/fantasy-football-gm`, per a Glama MCP listing — MIT-licensed, reads Sleeper league settings to produce remaining budget / preferred bid / bid range / walk-away price, handles waiver-priority leagues separately) | Closest match found; algorithm ideas (walk-away price as a distinct number from preferred bid) worth looking at conceptually | Not directly forkable — this project's differentiator is the *authenticated GraphQL* data source (full league bid history, not just the public API), which Fantasy GM has no equivalent of; its budget math is a reasonable idea to borrow, not code to reuse, since the input data model is different (unverified against the actual repo — found via a secondary listing, not inspected directly; worth a direct look at algorithm structure only, not a merge/fork) |
| `anthonybaldwin/sleeper-api-mcp`, other Sleeper MCP wrappers (TypeScript) | Sleeper API wrappers with waiver-recommendation tool names | Different language (TypeScript vs. this project's Rust), different data source (public API only, same gap this project's requirements doc already identifies), no FAAB-specific calculator found in their tool descriptions | Not forkable, not closely relevant |
| **GarbageTime Pros** (Chrome extension, Sleeper FAAB averages) | Shows this exact niche ("FAAB averages for your Sleeper league") is a validated real use case elsewhere | Closed-source — nothing to inspect | N/A |

**Verdict**: nothing directly forkable. Fantasy GM is worth a five-minute
skim for the "walk-away price" framing (a ceiling distinct from the
suggested bid) as a possible addition to Epic 2's signal set, flagged as an
open idea for Phase 3 planning rather than adopted code — this project's
authenticated-GraphQL-sourced full bid history is a strictly richer input
than anything these projects work from, so the model built here should
exceed what they do by default, not need to copy their structure.

## Summary

| Epic | Verdict |
|---|---|
| Unified read-through cache | **Build** — extends two already-proven hand-rolled patterns (`src/cache.rs`, `src/cli/sync.rs`'s `maybe_sync`) into one generalized per-data-kind staleness helper. No crate fits; none would reduce code written. |
| FAAB bid-projection model | **Build** — no OSS crate exists; no SaaS option covers personalized league history at a cost/lock-in proportionate to a single-league personal tool; nothing directly forkable, though Fantasy GM's walk-away-price framing is worth a look for Phase 3. |
| Browser DOM edit/cancel write-action | **Build on existing seam** — chromiumoxide + the `BrowserSession` trait already used by `claim_player_via_seam`/`lineup_set_via_seam`; no new dependency; the real work is live-selector verification (Feasibility Risks), not tooling choice. |
