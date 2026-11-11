# ADR-002: Read-through cache orchestration lives in `src/cli/browser/transactions.rs`

**Status**: Accepted
**Date**: 2026-11-10
**Project**: waiver-intelligence

## Context

The unified cache (plan.md Phase 2) needs one place that decides, per read, whether to serve from
SQLite or trigger a live refresh (REST or authenticated browser), and that write-through the result
before returning. That orchestration logic needs both `AnalysisService` (REST client + `StatsDb`)
and `Config`/the browser-launch capability — a combination no existing module has, and the three
candidate locations have materially different blast radii.

## Decision

Place it in a new file, `src/cli/browser/transactions.rs`.

`cli::browser::*` is already the project's one established home for "needs both REST/DB and a
browser launch, reused by both the CLI wrapper and the MCP tool layer." This is not incidental: the
two existing write-action MCP tools (`waiver_claim`, `lineup_set`, `src/mcp.rs:521-553`) already
bypass `AnalysisService` entirely and call `crate::cli::browser::run_claim(&self.config,
&self.svc.client, req)` directly, and `src/cli/browser/actions.rs`'s own doc comment (`actions.rs:1-9`)
explicitly states it serves MCP tools too, despite the `cli::` prefix in its module path. Putting the
new orchestrator here extends a pattern already proven at this exact seam, rather than introducing a
new one.

## Alternatives considered

1. **Extend `src/analysis/` / `AnalysisService`.** Rejected: `AnalysisService`
   (`src/analysis/mod.rs:28-32`) deliberately holds only `Arc<SleeperClient>` and `Arc<StatsDb>` —
   every existing consumer (every non-write MCP tool, every CLI `roster_cmds`/`dynasty_cmds` command)
   depends on it without needing a `Config` or any browser-launch capability. Adding `read_through`
   here would force a browser dependency onto `AnalysisService` itself, which every one of its ~15
   existing read-only call sites would then transitively carry, for no benefit to them.
2. **New top-level module, e.g. `src/transactions.rs`.** Rejected: the project already has exactly
   one place for "integration glue spanning REST+DB+browser, reused by CLI and MCP" —
   `cli::browser/`. A fourth top-level module would duplicate that role under a different name for no
   structural reason, and would need its own decision about whether it's allowed to import
   `crate::browser` types directly (today only `cli::browser::*` and nothing else does) or go through
   yet another indirection.
3. **A new submodule directly under `src/browser/` (e.g. `src/browser/cache.rs`).** Considered and
   rejected without a separate alternatives-table row because it inverts a layering invariant the
   prior `undetected-chromedriver-rust` architecture doc already established: `src/browser/` is the
   transport-hiding seam (`BrowserSession` trait only); it has no concept of `StatsDb` or
   `AnalysisService` today, and introducing one would mean the seam's own module now depends
   "upward" on the DB/analysis layer it's supposed to be agnostic to.

## Consequences

- `StatsDb` stays a "dumb," source-agnostic repository — it has no opinion about *where* a
  `TransactionRow` came from (its own doc comment, `db.rs:32-35`, already states this) and gains none
  here either; the "which source, is it stale" decision lives entirely in the orchestrator, not in
  `StatsDb` itself. This preserves the repository/orchestration boundary Clean Architecture asks for
  (architecture.md §3's explicit warning: "don't let `StatsDb` grow a `read_through` method that
  launches browsers").
- Every future read that needs "cache, or live if needed" logic (not just transactions) that also
  needs a browser populator has a natural home to extend, rather than inventing a fifth location.
- The file will accumulate more than just `read_through` over this project's life (Stories 2.2.2's
  `run_league_transactions_sync`, potentially Phase 4's pre-write re-check helper) — if it grows past
  a few hundred lines, split it the same way `cli/browser/actions.rs` vs. `cli/browser/pending.rs`
  already split by concern, not by introducing a new top-level module.
