# UX Research: waiver-intelligence

**Surface**: `sleeper` CLI (clap) + MCP tool calls consumed by an AI assistant. No GUI. The
"screen" is either a terminal or an assistant's relayed text, so every pattern below is
evaluated for that surface specifically, not transplanted from a web/app UX source unchanged.

## 0. Existing conventions this must match (not redesign)

Read before writing any new copy — these are the established voice/format:

- `src/cli/browser/login.rs::triage_error(triage, raw_detail)` — triage line always first,
  raw detail after `—` (or hidden behind `SLEEPER_BROWSER_DEBUG`). Never a bare stack on stdout.
- `src/cli/browser/doctor.rs::DoctorReport::render()` — one `✓`/`✗` line per check, every `✗`
  names the fix + exact re-run command. [doctor.rs:20-34](src/cli/browser/doctor.rs#L20-L34)
- `src/cli/browser/pending.rs::PendingTransaction::render()` — `[status] kind leg N — add X (bid
  $N), drop Y`. Compact, one line per item, bracket-tagged status prefix.
- `src/cli/sync.rs::maybe_sync()` — silent when nothing changed; a cyan `↻ synced N new stat
  week(s)` only when work actually happened. [sync.rs:293-314](src/cli/sync.rs#L293-L314)
- `src/client.rs` read-through cache — `tracing::debug!(url, "cache_hit")` / `cache_miss`-style
  spans, `is_fresh` bool, `cached_at`/`ttl_secs` columns. [client.rs:56-75](src/client.rs#L56-L75)
- MCP tool descriptions: `"LIVE WRITE ACTION — <what it really does, bluntly: 'spends FAAB and
  changes the live roster immediately'> ... Returns an error (never a false success) if ..."`
  ([mcp.rs:516](src/mcp.rs#L516), [mcp.rs:533](src/mcp.rs#L533)).

The new freshness/confidence/bid-signal copy should read like it was written by the same person
who wrote those — terse, declarative, names the exact fix/re-run command, never apologetic.

## 1. Cache freshness / confidence-scored output — CLI and tool-output patterns

**Finding (web research, standard search, no single canonical source — synthesized from CLI
tool patterns and a Claude Code feature-request thread)**: the common structure across `scope-cli`,
`devgraph`, `req-cli`'s `stale.rs`, and a proposed Claude Code cache-status indicator is:

- **Default to silence; surface staleness only on request or past a threshold.** A relative
  timestamp lives in an explicit status/doctor-style command, not printed on every unrelated
  call. This repo already does this: `maybe_sync` is silent unless it actually found new data
  ([sync.rs:307-312](src/cli/sync.rs#L307-L312)) — the unified cache should keep that bar, not
  regress to printing "last synced Xm ago" on every read.
- **Flag real drift, not generic age.** `devgraph`'s status command lists something stale only
  if it actually changed since last build — age alone isn't the signal; whether the cached
  answer could now be wrong is. For this project: a `complete` transaction is permanently
  correct once synced (it never changes again), so its age is irrelevant to correctness — only
  `pending` status needs a true freshness check, because that's the only row that can flip
  under you.
  - **Recommendation**: don't apply one staleness policy to "transactions" as a monolith. Gate
    staleness checks on whether the row's `status` is terminal (`complete`, `rejected`,
    `cancelled` — immutable once written) vs. `pending` (mutable until it resolves). A terminal
    row is cache-valid forever; the cache doesn't need a TTL for it at all, just a "do I have
    it" check. Only `pending` rows need the staleness-tolerance parameter the requirements doc
    asks for.
- **Few discrete states, not a countdown.** The Claude Code proposal's green/yellow/red (fresh /
  expiring / cold) beats a live ticking clock in a CLI — nothing in a terminal should animate
  on its own. For MCP tool output (consumed by an assistant, not a human watching a screen), the
  state should be a plain field the assistant can read and decide whether to mention, not prose
  trying to persuade a human — e.g. `"source": "cache", "cached_at": "...", "age_bucket":
  "fresh"` as structured JSON, matching how `get_pending_waivers` already returns structured
  `pending_waivers` today ([mcp.rs:560-565](src/mcp.rs#L560-L565)) rather than a formatted
  sentence. The CLI-rendered version of the same data can fold that into one line, same as
  `PendingTransaction::render()` already folds `status`/`kind`/`leg` into one compact string.
- **Route warnings to the right channel.** The `burn` tool proposal explicitly routes staleness
  warnings to stderr so piped stdout stays clean — this codebase's existing pattern (dim/cyan
  `console::style` lines, [sync.rs:285-288](src/cli/sync.rs#L285-L288)) already separates
  progress narration from data output; a cache-staleness note belongs in that same dim/cyan
  stream, not mixed into the data rows.

**Sources**: no single canonical write-up found for this exact pattern; synthesized from
[scope-cli `status.rs`](https://docs.rs/crate/scope-cli/0.9.1/source/src/commands/status.rs),
[devgraph CLI status docs](https://devgraph.ameyalambat.com/docs/cli/status.md),
[req-cli `stale.rs`](https://docs.rs/crate/req-cli/0.5.0-rc.6/source/src/commands/stale.rs),
and a [Claude Code cache-status feature request](https://claudeissues.com/issue/52665-feature-request-surface-prompt-cache-expiry-status-in-cli-status-bar).

### Applied to this project's staleness-tolerance requirement

The requirements doc (§ Non-functional, "Staleness tolerance is a parameter, not a constant")
asks Phase 2 research to ground defaults in the league's own observed cadence rather than guess.
What's available to ground it in, from the requirements doc itself:

- `waiver_type: 2` (FAAB), `waiver_day_of_week: 2` (Tuesday), `daily_waivers: 0` (weekly, not
  daily) — so a `pending` claim cannot resolve at all except around the weekly Tuesday
  processing window. **A pending claim read any day other than Tuesday needs zero live
  refresh tolerance-wise — it mechanically can't have changed** unless the operator cancelled
  it themselves through this same tool. The one time staleness tolerance for `pending` status
  actually matters is a window around Tuesday's processing run.
- Default policy this suggests (for Phase 3 planning to size precisely): `pending`-status reads
  default to "always live" only within some window of the known processing day/time; off-window,
  even a `pending` row can be served from cache with a long tolerance, because the mechanism
  guarantees nothing changed it. This turns a blind "pending = always refresh" rule (expensive:
  a browser launch every single pending-status read) into a rule grounded in the league's actual
  settings, per the requirements doc's explicit ask — and the "edit/cancel a claim that already
  resolved" race (§4 below) is exactly the case this window exists to catch.
- Historical bid-pattern / FAAB-remaining reads have no such mechanical ceiling — they tolerate
  whatever the cache has, refreshed on explicit force or when genuinely missing.

## 2. FAAB bid-sizing trust — what makes a suggested number trustworthy vs. black-box

**Finding (web research, standard search — practitioner XAI/UX sources, 2025-2026)**:

- **List the actual factors, not a single opaque score.** The pattern that recurs across
  sources (Smashing Magazine's XAI piece, Google PAIR, Goji Labs) is feature-level attribution:
  show the inputs that drove the number, in the units a domain expert already thinks in — not
  an abstract 0-1 confidence float. For this project that maps directly onto what the
  requirements doc already names as the model's inputs: perceived player value (`dynasty_tier`/
  `search_rank`), the league's historical bidding norms for that position, the target roster's
  estimated remaining FAAB, and that roster's claim frequency. **The tool's output should list
  those four, with their actual values, not a single number.** This matches the Observability
  Requirement already in the spec ("output includes the signals it used, not just the number")
  — research confirms that requirement is the right one, not an afterthought.
- **Progressive disclosure, but the CLI/MCP version of it is just two render depths, not a UI
  affordance.** A web app can hide reasoning behind an expand click; a CLI/MCP tool can't. The
  equivalent here: the **short line** (`render()`-style, one line: `suggested bid $23 — $FA
  market tier 2, roster has $142 FAAB left, bids waiver claims ~40% of weeks`) is what a human
  reads by default, and the **full structured object** (every signal, every intermediate number:
  how "league bidding norms" was computed, what the comparable historical bids actually were) is
  what the assistant gets back from the MCP tool and can drill into if asked "why $23 and not
  $15." Don't force a human to read the full breakdown by default; don't hide it from the
  assistant that might need to defend the number.
- **More explanation is not strictly better — calibration matters more than volume.** Goji Labs'
  point (users can trust a system they understand less, as long as behavior is consistent) cuts
  against over-explaining every time. Concretely: don't repeat the full signal breakdown on every
  single waiver question in a conversation — once the assistant has seen the signals for a given
  target/roster pair this session, re-stating them on a follow-up question is noise. This is the
  same "don't nag" principle as §1's freshness guidance, applied to confidence/signal disclosure
  instead of staleness disclosure — both come down to: say it once, structurally, and let the
  reader (human or assistant) ask again if they need it restated.
- **Avoid a raw confidence percentage with no context.** Multiple sources flag that a bare "73%
  confidence" number reads as pseudo-precise and doesn't help a reader decide anything. For a
  single-season, single-league model (the explicit stated limitation — "being derived from a
  relatively short observation window... defaults chosen now may need revisiting") a plain
  caveat beats a fake-precise score: e.g. `"sample": "6 of this roster's last 8 waiver weeks"` is
  more honest and more actionable than `"confidence": 0.61`.
- **The real sanity-check mechanism for this tool is CLAUDE.md's existing recommendation-discipline
  rule**, not new UI — the project's own `CLAUDE.md` already mandates adversarial critique +
  league-context check + external grounding before any real roster-money action executes. The
  bid-projection tool's job per the requirements doc (Out of Scope: "this ships the capability and
  the projection signal, not an auto-pilot") is to **feed that existing review loop with real
  signals**, not to replace it with a trust score. Framing the tool output as "here are the
  signals, go sanity-check them" rather than "here's your answer" is consistent with both the
  recommendation-discipline rule and the XAI research above.

**Sources**:
[Smashing Magazine — Beyond The Black Box: Practical XAI For UX Practitioners](https://smashingmagazine.com/2025/12/beyond-black-box-practical-xai-ux-practitioners/),
[Google PAIR — Explainability + Trust](https://pair.withgoogle.com/chapter/explainability-trust/),
[Goji Labs — Explainable AI: What Users Actually Need](https://gojilabs.com/blog/explainable-ai-what-users-actually-need/),
[Jan Rummel — Trust & Explainability (progressive disclosure)](https://janrummel.github.io/ai-product-thinking/en/03-product-design/02-trust-explainability),
[Koji — AI Explainability Testing](https://www.koji.so/docs/ai-explainability-user-testing).

## 3. Accessibility — N/A (no GUI), skipped per the brief.

## 4. Error states and edge cases

### 4a. Zero transaction history for a roster (new to league / never synced)

This is a **cold-start** problem in the ML-UX literature sense (new entity, no behavioral
history), not a bug state. Research pattern (synthesized — no source addressed this exact
"insufficient data" messaging directly, see gap note below): segment the fallback rather than
either erroring or silently guessing.

- **Don't silently substitute a default and present it with the same confidence as a
  data-backed answer.** This is the single sharpest point from the cold-start research (one
  source: "a generic default can make the product seem unintelligent rather than neutral") and
  it's also exactly what the requirements doc's Rabbit Hole already warns about: "a
  partial-history cache must not silently present a confident, wrong remaining-budget number."
  Research confirms that instinct is correct, not overcautious.
- **Concrete behavior for this tool**: if a roster has zero cached transaction history, the
  bid-projection tool's signals section should say so explicitly per-signal — e.g.
  `"target_roster_faab_remaining": null, "note": "no transaction history cached for this
  roster — season_budget assumed, no won-bid deductions applied"` — rather than quietly reporting
  `season_waiver_budget` as if it were a verified remaining balance. The player-value signal
  (`dynasty_tier`/`search_rank`) is unaffected by this gap and should still render normally —
  don't degrade signals that don't depend on the missing data.
- **CLI rendering**: match the existing `doctor.rs` ✓/✗ idiom for "what do I actually have":
  a line like `✗ no cached FAAB history for roster 7 — remaining budget is an estimate, not
  verified` reads consistently with `✗ chrome runtime ... version skew` already in this codebase.

### 4b. Live browser session logged out when a refresh is attempted

This isn't a new problem — it's the exact failure mode `login.rs`/`doctor.rs` already triage.
Reuse, don't reinvent:

- `launch_session`/`launch_failure_error` already exist for this
  ([login.rs](src/cli/browser/login.rs)) and `pending.rs`'s `DEAD_SESSION_HINT` branch already
  distinguishes "session is dead, re-login" from "something else broke"
  ([pending.rs:119-128](src/cli/browser/pending.rs#L119-L128)). Any new live-refresh path
  (cache force-refresh, bid-projection's live FAAB check, edit/cancel) should route through the
  same `launch_session` → `DEAD_SESSION_HINT` → yellow "Surface C" re-login hint pattern rather
  than inventing new copy. The cache layer's job here is narrow: when a live refresh fails
  because the session is dead, **serve the last-known cached value with an explicit staleness
  flag** rather than propagating a bare error up through a read that could otherwise have
  succeeded from cache — the login failure should only hard-fail the specific call that demanded
  live truth (e.g. "is this still pending" — see §4c), not every read that merely prefers fresh
  data.

### 4c. Edit/cancel invoked on a claim that already resolved (the pending-vs-processed race)

This is the closest-matching researched pattern: **optimistic-action-vs-server-truth conflict**.
Synthesized guidance from Twilio's error docs, a frontend-patterns guide, and Mendix/SAP Fiori's
draft-handling copy, adapted to a CLI/MCP surface that has no "reload the page":

- **Check current state before attempting the write, not after it fails.** Twilio's guidance
  ("confirm the update is still needed and the desired change is not already present") maps
  directly: before firing the edit/cancel DOM action, re-fetch the claim's current status
  through the same cache-aware path the bid-projection/read tools use. If it's no longer
  `pending`, don't attempt the live DOM action at all — report what actually happened instead.
- **Don't call it a generic error.** Ayende's framing point (outdated-state conflicts aren't
  "errors," they're "the world moved") applies directly to fantasy-waiver semantics: a claim
  that already got processed isn't a bug, it's just Tuesday happening on schedule. Triage copy
  should read like the existing `pending_failed_triage()` pattern but name the real cause:
  `"claim {id} is no longer pending (now: {status}) — nothing to edit/cancel. Run \`sleeper
  browser pending-waivers\` to see its resolved outcome."` — same shape as
  `check_failed_triage()`'s `"... re-run \`sleeper browser status\`"` convention
  ([login.rs:19-21](src/cli/browser/login.rs#L19-L21)), pointed at the right re-run command.
- **No retry-with-overwrite.** The frontend-patterns source is explicit that the correct recovery
  from this conflict class is re-fetch-and-show-current-truth, never "retry the same write
  blindly" — directly relevant here because retrying a cancel against an already-processed claim
  isn't just confusing, it's acting on stale state for a real-money action, which is exactly what
  this repo's recommendation-discipline rule and "LIVE WRITE ACTION" framing exist to prevent.
  This also doubles as the mechanism that refreshes the cache: the pre-write status re-check is
  itself a cache-invalidating read, so the next reader gets the corrected status for free.

**Sources**:
[Twilio — error 45796: conflict with current resource state](https://live.docs.dev.twilio.com/docs/api/errors/45796.md),
[frontendpatternsdev — managing async UX states](https://frontendpatternsdev.vercel.app/guides/managing-async-ux-states),
[SAP Fiori — Draft Handling](https://www.sap.com/design-system/fiori-design-web/v1-120/foundations/best-practices/global-patterns/object-handling/draft-handling),
[Mendix — Optimistic Locking](https://docs.mendix.com/refguide/optimistic-locking/),
[Ayende Rahien — "the right thing and what the user expects"](https://ayende.net/blog/180257/the-right-thing-and-what-the-user-expect-to-happen-are-completely-unrelated).

**Gap, named rather than smoothed over**: no source directly addressed "insufficient-data
confidence messaging" for a cold-start entity (§4a) — that section is inferred from general
cold-start/ML-UX material (tianpan.co, shaped.ai) plus this project's own stated instincts in
the requirements doc, not from a source that tested the specific message. Treat §4a's concrete
copy as a reasonable default, not a verified-effective pattern.

## 5. Jobs-to-be-done: "tell me what to bid" and "let me change what I already bid"

**Finding**: no source addresses dynasty-league JTBD directly (searched; general JTBD framework
sources and generic fantasy-commissioner content only). The functional/emotional/social
three-way split itself is well-established (LogRocket, uxdesign.cc); the mapping below is this
agent's own application of that framework to the two specific jobs in scope, not a cited result.

**"Tell me what to bid" (bid-projection tool)**

- *Functional*: convert the dozen-plus facts a careful manager would otherwise hold in their
  head (own FAAB left, target's market tier, what this specific league usually pays for that
  tier, how aggressive the likely competing bidder is) into one number, faster than doing that
  math by hand before the Tuesday deadline.
- *Emotional*: relieve **second-guessing anxiety** specific to FAAB — unlike a snap lineup
  decision, a bid is irreversible and public (every other manager sees the winning bid amount
  after the fact), so the fear isn't "did I lose the player" alone, it's "did I overpay and
  signal I don't know the league's going rate." A trustworthy signal (§2) directly serves this —
  a black-box number makes the anxiety worse, not better, because it adds "do I even trust my
  own tool" on top of "did I bid right."
- *Social*: avoid looking like the manager who overpays or underpays relative to league norms —
  dynasty leagues are multi-season relationships (per the one grounded source on dynasty
  specifically, Footballguys' commissioner material: the role is long-term league stewardship,
  not a single season), so a visibly bad bid pattern is reputational across years, not just one
  week's outcome.

**"Let me change what I already bid" (edit/cancel)**

- *Functional*: correct a decision before it executes — today the only path is leaving this
  tool for the Sleeper app by hand (stated directly in the requirements doc's Baseline), so the
  job here is strictly "don't force a context-switch to fix my own mistake."
- *Emotional*: relief from the **lock-in dread** of a FAAB system specifically — once submitted,
  a bid is usually final until the processing window, so "can I still change this" is a real
  question with real stakes (wrong bid = wasted budget for a season with a hard cap), not idle
  tinkering. The tool answering that question accurately (§4c) matters more emotionally than the
  edit mechanism itself being convenient.
- *Social*: a private correction channel — fixing a misjudged bid through this tool rather than
  being seen fumbling it in the league's shared app (where some leagues show pending claims to
  commissioners/other managers) avoids a visible-to-others "take it back" moment. This is
  speculative relative to Sleeper's actual visibility model and should not be asserted as fact
  in any user-facing copy — named here only as a plausible "why" for including the feature, not
  a claim about Sleeper's UI.

**Source**: [LogRocket — 3 types of jobs to be done](https://blog.logrocket.com/product-management/3-types-of-jobs-to-be-done/)
for the functional/emotional/social framework itself;
[Footballguys — 2025 Dynasty Commissioner's Handbook](https://www.footballguys.com/article/2025-dynasty-commissioners-handbook-01)
for the dynasty-specific "multi-year stewardship" framing used in the social-job section above.
Everything else in §5 is this agent's inference from the requirements doc and general JTBD
structure, not a cited finding — flagged per the project's evidence-and-claims standard rather
than presented as researched fact.

## Summary of concrete recommendations for Phase 3 planning

1. Staleness policy should branch on transaction `status` (terminal = cache-valid forever, no
   TTL; `pending` = needs live-check only near the known Tuesday processing window), not apply
   one TTL to "transactions" as a category — grounded in `waiver_day_of_week`/`daily_waivers`
   already in hand, per the Rabbit Hole's explicit ask.
2. Cache/freshness state surfaces as structured fields in MCP tool JSON (`source`, `cached_at`,
   terminal-vs-pending) and as a dim/cyan one-line note in CLI output, matching
   `sync.rs`'s existing silent-unless-something-happened convention — never a per-call nag,
   never a live countdown.
3. Bid-projection output must list the four named signals with real values (not a single score),
   short-line for CLI/human, full structured object for the MCP/assistant caller — directly
   satisfies the Observability Requirement already in requirements.md, now backed by XAI
   research rather than asserted.
4. Missing-history and dead-session failures reuse existing triage idioms
   (`triage_error`/`DEAD_SESSION_HINT`/`check_failed_triage`) rather than inventing new copy;
   the only genuinely new triage case is "claim already resolved," which should re-check status
   before attempting any live write and report the real current state, not a generic error.
5. JTBD framing (anxiety over an irreversible, publicly-visible bid; dread of being locked into
   a mistake) argues for the bid tool erring toward more legible signals over a terser black-box
   number, and for the edit/cancel tool erring toward an accurate "can I still change this"
   answer over a fast but possibly-wrong one — consistent with, not in tension with, the
   project's existing recommendation-discipline and LIVE-WRITE-ACTION conventions.
