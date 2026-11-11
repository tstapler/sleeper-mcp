# ADR-001: Staleness tolerance is a caller-supplied parameter, not a fixed TTL

**Status**: Accepted
**Date**: 2026-11-10
**Project**: waiver-intelligence

## Context

The unified transaction cache (plan.md Phase 2) serves two very different questions from the same
`transactions` table: "is this specific waiver claim still pending" (wrong answer if even slightly
stale) and "what has this league historically bid at QB" (correct forever once a `complete`
transaction is synced, since Sleeper transactions don't retroactively change once resolved).

The codebase already has two existing freshness mechanisms, neither of which fits both questions:

- `src/cache.rs`'s `HttpCache` — a per-row TTL chosen **at write time** (`ttl_override` on `set()`),
  checked as `(now - cached_at) < ttl_secs`. This bakes "how stale is acceptable" into the write,
  before anyone knows what question a later reader will ask.
- `src/cli/sync.rs`'s `maybe_sync` — a caller-supplied `max_age_secs` compared against
  `league_synced_at()`. Closer to what's needed, but it's a single `i64` applied uniformly to one
  table (`leagues`), with no concept of "this row is terminal, it can never go stale" vs. "this row
  is pending, it can flip at any moment."

requirements.md's own Non-functional Requirements section states this explicitly as a requirement,
not a nice-to-have: "Staleness tolerance is a parameter, not a constant... defaulting sensibly per
data kind rather than one global TTL."

## Decision

Introduce `StalenessTolerance` as a three-state enum (`AlwaysLive | MaxAge(chrono::Duration) |
CachedOnly`), supplied by the **caller at read time**, not baked into the row at write time. The
cache's own staleness *gate* branches first on the transaction's `status`:

- Terminal statuses (`complete`, `rejected`, `cancelled`) are cache-valid forever — `tolerance` is
  not even consulted once such a row exists, because the underlying fact cannot change after Sleeper
  resolves it.
- Only `pending` rows (or an outright cache miss) consult the caller's `tolerance`.

This is a genuine extension of the existing `HttpCache`/`maybe_sync` patterns, not a reuse of either
verbatim — see architecture.md §1 and build-vs-buy.md §3's confirmation that hand-rolling this
extension is proportionate (no crate solves this; it's ~30 lines of comparison logic either way).

## Alternatives considered

1. **One global `max_age_secs: i64` parameter, like `maybe_sync`'s today**, applied uniformly to
   every `list_transactions`/`read_through` call. Rejected: pitfalls.md §1.2 names the exact failure
   this causes — "callers will default to a lenient tolerance out of habit and get silently wrong
   answers for exactly the question (`is this claim still pending`) that requirements.md calls out as
   read-now-or-wrong." A single default cannot serve both the pending-status question and the
   historical-analysis question without one of them being wrong by construction.
2. **Fixed per-table TTL chosen at write time, `HttpCache`-style.** Rejected: the "right" TTL for a
   `pending` row depends on *when it's read relative to the league's Tuesday waiver-processing
   window* (`waiver_day_of_week=2`), not on when it was written — a write-time TTL can't express that
   without becoming a moving target recomputed on every write, which is strictly more complex than
   just asking the reader.
3. **Always live (no staleness concept at all).** Rejected outright by requirements.md's Success
   Metrics: repeated analysis must run against a local cache with zero additional calls once warm.

## Consequences

- Every call site (CLI flag, MCP tool param, internal `read_through` signature) must share the same
  `StalenessTolerance` vocabulary (features.md §4's "unstated need" — one shared type, not three ad
  hoc representations) rather than drifting into `Option<i64>` + `bool force` at some call sites and
  an enum at others.
- The terminal-status short-circuit means most reads never consult `tolerance` at all in practice —
  this is intentional (ux.md §1: "a `complete` transaction is permanently correct once synced, so its
  age is irrelevant to correctness"), but it means `tolerance` is easy to under-test if test fixtures
  only ever use terminal-status rows. Plan.md Story 2.1.2's acceptance criteria explicitly cover a
  `pending` row under `AlwaysLive` and a `complete` row under a short `MaxAge` to catch this.
- `HttpCache` is left unchanged (Tech Debt Disposition: "leave alone, do not generalize") — two
  freshness idioms now coexist in the codebase (write-time TTL for raw HTTP bodies, read-time
  tolerance for transactions). This is accepted technical debt, named here rather than silently
  duplicated; unifying them is explicitly out of scope for this project.
