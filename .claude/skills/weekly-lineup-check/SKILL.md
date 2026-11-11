---
name: weekly-lineup-check
description: >-
  Weekly lineup-setting and waiver-review checklist for Tyler's "Big Blue 2" Sleeper
  dynasty league (league_id 1352883872131481600, roster_id 5), using this repo's own
  CLI/MCP tools. Use before kickoff each week, or whenever asked to "check/set my
  lineup" or "what should we do for waivers" for this league.
---

# Weekly Lineup Check — Big Blue 2 Dynasty

This league is **Superflex** (`QB` + `SUPER_FLEX` slots — confirm via `get_league_info`,
don't assume) and half-PPR, 12 teams, with an `IDP_FLEX` slot. That shape changes what
"need" means here versus a standard 1-QB league — see the QB section below.

## 0. Pull current state first

Always re-fetch fresh — don't reason from memory of a prior week:

- `mcp__sleeper-mcp__get_roster_with_names` (roster_id 5) — current roster, starters,
  bench, taxi, reserve, each player's `injury_status`.
- `mcp__sleeper-mcp__get_league_info` — confirms `roster_positions`/`is_superflex`/
  `roster_slot_counts`. Don't hardcode slot counts from memory; league settings can change.
- `mcp__sleeper-mcp__get_nfl_state` — current week, so bye-week reasoning lines up.

## 1. Check every starter for a red flag

For each starter, check three things, in this order:

1. **Bye week.** A player's team can be on bye while Sleeper's own `injury_status` field
   stays `null` — the roster API doesn't surface bye directly. Cross-check via a live
   roster-row dump (`sleeper transactions`/a `BrowserSession.eval_string` read against
   `.team-roster-item` text, or just ask a web search for "<team> bye week <N> <season>")
   if `get_roster_with_names` doesn't make it obvious.
2. **`injury_status`.** `null` = fine. `Questionable` = don't swap preemptively; see §2.
   `Out`/`Doubtful`/`IR` = needs a bench replacement.
3. **Role uncertainty that `injury_status` won't show at all.** A backup QB/RB whose
   *team* has an unresolved starter situation (a starter's injury is "not settled," a
   committee backfield) carries real risk `injury_status: null` won't flag. Ground this
   with a web search on the player's actual team situation this week — don't trust the
   roster payload alone for this category.

## 2. `Questionable` players: don't swap early

A `Questionable` tag is usually a game-time call, and this league's roster construction
means the "healthy" bench alternative is often *also* hurt (confirmed happening twice
already this season: Mike Evans/Rashid Shaheed both `Out` at the same time as a
`Questionable` starter). Rather than guessing:

- Web-search the player's name + team + "injury report" + the current week for the
  actual practice/game designation. Results are frequently stale or conflate different
  weeks' injury scares for the same player — read dates carefully, don't average
  conflicting reports into a guess.
- If no clear answer exists yet, say so plainly and recommend checking again closer to
  kickoff rather than swapping on incomplete information — benching a player who ends up
  playing costs real points for no reason.
- Know the fallback *before* kickoff: find the right bench replacement by checking which
  **slot type** the Questionable player occupies (see §3) — a FLEX-slot player can be
  replaced by any FLEX-eligible bench player, not just same-position ones.

## 3. Know which slot type you're actually filling

`roster_positions` includes locked single-position slots (`QB`, `TE`, two `RB`, three
`WR`) *and* wide-eligibility slots (`FLEX` ×2, `SUPER_FLEX`, `IDP_FLEX`). The MCP
roster payload's `starters` array doesn't label which slot each player fills — infer it
from `roster_positions` order, or check the live roster page directly (a `.team-roster-item`
row's `.league-slot-position-square` div gives the real slot label, e.g. "WR" vs "BN").

This matters because a player who *looks* locked into a WR-only read might actually be in
a FLEX slot — meaning a healthy bench **RB or TE** is a legal replacement even though no
healthy bench WR exists. Don't assume the replacement pool is position-matched; check the
slot type first.

## 4. QB in Superflex: raw count lies — check confirmed-starter status, not roster depth

`analyze_roster_needs` flags QB as a "surplus" purely by headcount. In a Superflex league
that's the wrong signal — what matters is how many of your rostered QBs are **confirmed
starters this week**, not how many QBs you own. Found this the hard way: had 3 rostered
QBs flagged as a surplus, and all 3 were backups-or-worse — one on bye, one a
depth-chart QB2/3 only elevated if a specific starter sits, one a pure clipboard-holder.

For each rostered QB, before setting the lineup:
- Web-search "<player> <team> starting quarterback week <N> <season>" — don't rely on
  `dynasty_tier`/`search_rank` for in-season role, those are long-horizon value signals,
  not this-week status.
- Prefer whichever bench QB has a real (even conditional) path to playing meaningful
  snaps over one with none, even if the conditional one has lower `dynasty_tier` — a
  zero is a zero regardless of long-term value.
- If literally none of your rostered QBs has a real shot at playing, that's a genuine
  waiver-wire priority this week (see §6) — not a "surplus," regardless of what
  `analyze_roster_needs` says.

## 5. Check pending waivers before finalizing

Run `sleeper waiver pending --league 1352883872131481600 --roster 5` (needs a working
browser session — see §7 if it fails). A claim that resolves before kickoff changes
roster composition out from under a lineup decision made without checking it. This is
the *only* way to see pending claims and FAAB bid amounts — the public REST API
(`get_recent_transactions` etc.) never exposes `pending` status or bid amounts at all.

## 6. Waiver-wire review: follow the project's standing recommendation discipline

This repo's own `CLAUDE.md` ("Recommendation discipline (roster moves)") already sets
the required process for any real roster-move recommendation — adversarial critique of
opportunity cost, league-wide context via `get_league_rosters`, and external grounding
via web search, cited. Follow that in full; this skill doesn't duplicate it. Two
traps specific to this league worth naming here:

- **A position tool-flagged as "surplus" can still have a real quality gap** (§4's QB
  finding). Don't let a clean `analyze_roster_needs` output end the analysis — check
  whether the *players*, not just the slots, are actually good.
- **Don't double-spend FAAB on the same need.** Before recommending a new claim, check
  what's already pending (§5) — a claim already in flight (e.g. a newly-elevated
  Superflex streamer) can already cover the gap a fresh recommendation would also target.
  With a $100 season FAAB budget and claims sized $5–$80+, a few pending bids can eat
  most of what's left; confirm the real remaining budget before sizing a new one.

## 7. Setting the lineup: execute, then verify against the *live page*, not just the CLI's own message

```
sleeper lineup-set --league 1352883872131481600 --out <currently_occupying_player_id> --in <bench_player_id>
```

`--out` is whoever currently occupies the target slot (a started player, or an IR/Taxi
occupant); `--in` is the bench player moving into it. This same command handles IR/Taxi
moves too (live-verified 2026-11-11, round-tripped a real swap and back) — there's no
separate tool for it. To activate someone off IR/Taxi back to the bench, just reverse
which player_id is `--out` and which is `--in`.

**Never trust "lineup saved" alone.** This flow had two real, now-fixed bugs this
session (wrong name-matching against the live DOM, and closing the browser before the
debounced save network call fired — the latter made it print "lineup saved" while
silently doing nothing). The fix added a verification poll before the CLI returns
success, but still re-check independently when it matters:

- `get_roster_with_names` right after a swap can show the **old** state for a few
  minutes — that's Sleeper's own public REST API cache lag, not a failed swap. Don't
  panic and re-run the swap; wait a few minutes and re-check, or confirm against the
  live page directly if you need certainty immediately.
- If `lineup-set`/`waiver pending`/`browser status` fails with "Chrome exited before
  printing a DevTools endpoint" or "Login check failed (browser launch)": check for a
  stale `SingletonLock` in `cache/chrome-profile/` from an orphaned Chrome process
  (`ps aux | grep chrome-profile`; if the PID is dead, `rm cache/chrome-profile/SingletonLock`
  is safe — Chrome's own stale-lock recovery, not a destructive action on live data).
  This happened once already from a `timeout`-wrapped CLI call SIGKILLing the Rust
  process before its own cleanup-on-drop could run and close the child Chrome.

## 8. Data-source inventory (what to pull from, and why)

| Need | Tool/command | Notes |
|---|---|---|
| Current roster + injury tags | `mcp__sleeper-mcp__get_roster_with_names` | Public REST, cached briefly; no pending/bid data |
| League shape (superflex, slots) | `mcp__sleeper-mcp__get_league_info` | Confirm, don't assume |
| Current week/season state | `mcp__sleeper-mcp__get_nfl_state` | |
| Roster-need signal (count-based) | `mcp__sleeper-mcp__analyze_roster_needs` | Starting point only — §4/§6 traps |
| Trending adds/drops | `mcp__sleeper-mcp__get_waiver_recommendations` | League-wide Sleeper trend data, not this-league-specific |
| Other rosters (scarcity/FAAB context) | `mcp__sleeper-mcp__get_league_rosters` | Required by the CLAUDE.md recommendation-discipline policy |
| Pending claims + FAAB bids | `sleeper waiver pending --league <id> --roster 5` (CLI) | Needs a live, logged-in browser session; public API can't see this |
| Player identity/lookup | `mcp__sleeper-mcp__search_players` | Resolve a name to `player_id` or vice versa |
| Dynasty value/tier | `mcp__sleeper-mcp__dynasty_scout` / `get_player_report` | Long-horizon signal — not a this-week status source |
| This-week role/injury truth | `WebSearch` | The only reliable source for "is X actually starting this week" — always ground here before a start/sit or waiver call |
| Set a lineup | `sleeper lineup-set --league <id> --out <id> --in <id>` (CLI) | Verify per §7, don't trust the message alone |
| Bid projection | `sleeper waiver bid-projection --league <id> --player <id> --roster 5 [--full]` | Honest low-confidence/floor caveats built in — read them |
