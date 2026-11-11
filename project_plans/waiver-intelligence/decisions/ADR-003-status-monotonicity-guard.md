# ADR-003: Transaction `status` upsert uses a CAS-by-timestamp guard, not a last-write-wins overwrite

**Status**: Accepted
**Date**: 2026-11-10
**Project**: waiver-intelligence

## Context

`StatsDb::upsert_transaction`'s `ON CONFLICT(transaction_id) DO UPDATE` clause (`src/db.rs:451-467`)
treats every column differently depending on how trustworthy an overwrite is: `waiver_bid` is
`COALESCE`d so a bid-blind REST re-sync can never erase a bid the browser-GraphQL path already
recorded (`db.rs:465`, comment at `db.rs:447-450`). `status`, by contrast, is a plain
`status = excluded.status` (`db.rs:460`) — whichever sync writes last, wins, unconditionally.

This project introduces the first scenario where that matters: two independent writers (the existing
REST sync, and this project's new whole-league browser-GraphQL backfill) can now both touch the same
`transaction_id` in an order that isn't guaranteed to match wall-clock reality — e.g. two GraphQL
reads scheduled out of order, or a GraphQL read landing between two REST syncs. A plain overwrite can
silently regress a terminal status (`complete`, `rejected`, `cancelled`) back to `pending`, with no
error, no log, and no way for a caller to know it happened. architecture.md §2 and pitfalls.md §1.1
both independently identified this as a real, already-present code defect — not a hypothetical this
project introduces — architecture.md §0 explicitly calls it "an open defect to close in Phase 3," not
a design question still open.

## Decision

Add a `status_updated INTEGER` column (the GraphQL epoch field already fetched at `flows.rs:182` but
never persisted) and change the conflict clause to a CAS (compare-and-swap) guard keyed on it:

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
END
```

A write with no `status_updated` (the REST path never carries this field) never touches `status` on
conflict at all — it's treated as "no claim to staleness," which also means REST can never move
`status` backward *or* forward once a GraphQL-sourced value exists. A write with an older
`status_updated` than the stored row is rejected (status stays as-is); the attempt is logged at
`warn` so a pattern of rejected downgrades is diagnosable (closing pitfalls.md §1.1's explicit ask:
"log any write that would downgrade a terminal status, even if you choose to allow it" — here we
don't allow it, but the attempt is still visible).

## Alternatives considered

1. **Compare the incoming row's `fetched_at` against the existing row's `fetched_at`; reject the
   entire write if older.** Rejected: this would also reject legitimately-concurrent updates to
   `roster_ids`/`adds`/`drops` just because the write happened to arrive "late," even when `status`
   itself isn't in conflict. A per-column guard (matching `waiver_bid`'s existing COALESCE idiom)
   only blocks the one field that actually needs ordering.
2. **Treat `status` as monotonic on a lattice (`pending < complete`, `pending < rejected`, `pending <
   cancelled`) instead of by timestamp**, rejecting any write that would move backward on that
   lattice regardless of when it was observed. Rejected: this can't express a case the GraphQL field
   can — if Sleeper itself corrects a transaction (e.g. a `rejected` claim later gets manually
   reinstated as `complete` by a commissioner action, an edge case not ruled out by Sleeper's own
   semantics), a lattice-only guard would permanently refuse a legitimate later correction, whereas
   the timestamp guard accepts it because it actually did happen later.
3. **Log the regression but allow it anyway (visibility without prevention).** Rejected as
   insufficient on its own — pitfalls.md §1.1 raises this as the minimum bar, not the target; since
   the fix (a CAS guard) is no more code than logging-and-allowing, there's no reason to ship the
   weaker version. The chosen design does both: rejects *and* logs.

## Consequences

- Every write path that calls `upsert_transaction` must now pass `status_updated` honestly: `None`
  from the REST path (it never has this field — `src/analysis/roster.rs`'s `sync_transaction_history`
  call site), `Some(epoch)` from the GraphQL path. A future third data source that claims to carry a
  `status` must also carry a real `status_updated` or it will silently never be able to change
  `status` at all (treated identically to REST) — this is the correct conservative default (don't
  trust a source's status without knowing when it was observed), but should be stated explicitly in
  any future `TransactionRow`-producing code's doc comment, not rediscovered by reading the CASE
  clause.
- `source` (observability-only, plain overwrite, not CAS-guarded) is added alongside `status_updated`
  in the same migration, so a `sqlite3 stats.db` query can distinguish "this row's status came from
  REST vs. GraphQL" without inferring it from whether `waiver_bid`/`status_updated` happen to be
  non-NULL.
