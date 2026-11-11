---
name: trade-evaluation
description: >-
  Trade evaluation and proposal checklist for Tyler's "Big Blue 2" Sleeper dynasty
  league (league_id 1352883872131481600, roster_id 5), using this repo's own
  MCP/CLI tools. Use whenever asked to evaluate a trade, find trade targets, check
  the trade block, or propose a trade to another team in this league.
---

# Trade Evaluation — Big Blue 2 Dynasty

12-team Superflex (`QB`+`SUPER_FLEX`), half-PPR, `IDP_FLEX`, dynasty. Trades are
two-sided in a way waiver claims aren't: a proposal that only helps roster_id 5 just
gets rejected or ignored. Everything below is in service of that — evaluate and frame
every trade from both sides, not just "is this good for me."

## 0. The trade block itself: **open item, not currently readable**

Sleeper has a real in-app Trade Block feature (players/picks a manager flags as
available, surfaced in league chat) — confirmed via Sleeper's own support docs. It is
**not exposed by the public REST API** at all (read-only API, no trade-block-contents
endpoint in the documented surface). Reading it would need the same kind of
authenticated-GraphQL-plus-browser work this repo already did for pending waiver
claims — and that work has **not been done** for the trade block. Don't assume it's
readable; if asked to check what's on another team's trade block, say plainly that
this repo can't see it yet and would need new browser-automation work (a new GraphQL
query to find, likely surfaced via the Team page's trade UI, mirroring how
`league_transactions_filtered` was discovered for waivers) before it could.

**Proposing a trade live is also not currently possible.** `trade_propose_via_seam` in
`src/browser/session/flows.rs` exists but is a hard-coded stub (Epic 4, cut per an
appetite rule) — it always returns an error, with a comment stating selectors are
live-unverified and no automation is attempted. Don't treat its existence as "half
built"; it's a placeholder signature only. A real trade proposal has to be sent by
Tyler himself in the Sleeper app; this skill is for building the case, not executing it.

## 1. Pull league-wide state first

- `mcp__sleeper-mcp__get_league_rosters` (all 12 rosters) — every team's full player
  list, starters, and `settings.wins`/`settings.losses`. This is the raw input for both
  "who needs what" and "who's contending vs. rebuilding" below.
- `mcp__sleeper-mcp__get_league_info` — confirm `is_superflex`/`roster_positions`
  again; trade value math depends on it (see §3).
- `mcp__sleeper-mcp__get_league_drafts` + `mcp__sleeper-mcp__get_draft_picks` — if a
  proposal involves picks, not just players, these give the actual draft/pick
  inventory (this league runs 4-round rookie drafts per `draft_rounds` in league info).

## 2. Generate first-pass candidates with `find_win_win_trades`, then verify every claim it makes

```
mcp__sleeper-mcp__find_win_win_trades(league_id, roster_id="5", limit=10)
```

This tool already does real need-matching — it returns `my_needs`/`their_needs` per
position, ranked `trade_proposals` with `win_win_score`, and concrete `you_give`/
`you_receive` player lists. Treat its output exactly like `dynasty_scout`'s tier/rank
in the waiver-side policy: **a starting signal, not a verdict.** Specific things to
check before trusting a proposal it surfaces:

- **It can disagree with `analyze_roster_needs` on the same position** — confirmed
  live: `find_win_win_trades` flagged `QB` as one of roster 5's needs, while
  `analyze_roster_needs` flags QB as a 3-player "surplus" the same week (see
  `weekly-lineup-check` §4 — the QB-quality trap generalizes here). When two tools
  disagree, that's a real signal to dig into player quality yourself, not something to
  average or pick whichever answer is more convenient.
- **`injury_status`/`risk_flag` fields are embedded in its player objects** — read
  them. A proposal built around a player currently `"injury_status": "Out"` (seen live:
  Mike Evans, Deebo Samuel both surfaced as trade-relevant while `Out`) needs an
  up-to-date web-search check on real recovery timeline before you'd actually value
  them at the shown `ppg`/`season_pts`, which reflect pre-injury/healthy-game averages.
- **`partner_roster_id`'s actual context matters more than the summary line.** Pull
  that specific roster from §1's `get_league_rosters` result and check their real
  win/loss record — a rebuilding team (low wins) wants youth/future value even if the
  tool's `summary` framed the trade around this-week need; a contender wants proven
  weekly production even at an age/injury-risk cost. Don't take the tool's own
  `summary` string as the full picture of what the other team actually wants.

## 3. Price it with `get_trade_value`, not raw `dynasty_tier`/`search_rank`

```
mcp__sleeper-mcp__get_trade_value(league_id, player_ids=[...both sides, one call...])
```

Real FantasyCalc market-consensus values, confirmed **already superflex/PPR-aware**
for this exact league (`is_dynasty`/`num_qbs`/`ppr` fields in the response reflect this
league's actual settings, not generic defaults) — this is the right tool for "is this
fair," not `dynasty_tier`/`search_rank` (those are this repo's own coarse internal
buckets, fine for roster-need triage, too coarse for pricing a specific trade).

**Gap: IDP players return `found: false`** — FantasyCalc doesn't price them at all.
For any trade involving an IDP player (this roster carries several — DB/DL/LB), value
them by `dynasty_tier`/`search_rank` reasoning plus a web search for current
analyst consensus, same as the waiver-side policy requires; don't let a missing
`get_trade_value` entry read as "worthless," it just means this tool has no opinion.

## 4. Check real completed-trade history in *this* league for market calibration

```
mcp__sleeper-mcp__get_recent_transactions(league_id, week=<N>, type_filter="trade")
```

Per-week only (no single "all trades this season" call) — loop across the weeks
you care about. This gives actual prices this specific league's managers have already
agreed to, which can diverge from FantasyCalc's cross-league consensus (a league that's
historically RB-heavy in its trades, for example, will price RBs differently than the
market average). Use this to sanity-check §3's numbers against this league's own
revealed preferences, not as a replacement for them.

## 5. Apply the standing recommendation-discipline policy — adapted for two sides

This repo's `CLAUDE.md` ("Recommendation discipline (roster moves)") already requires
adversarial critique, league-wide context, and external grounding for any real roster
move, trades included. For a trade specifically, that means doing all three **for both
sides of the deal**, not just roster 5's:

1. **Adversarial critique, both directions.** State the opportunity cost of what
   roster 5 gives up (not just what it gains), *and* argue the other team's side — why
   would they actually say yes? A trade that only makes sense from one seat isn't a
   real proposal, it's a wishlist.
2. **League-wide context** (§1/§2) — confirmed startable-slot scarcity matters the same
   way it does for waivers (e.g. one `IDP_FLEX` slot across the whole league caps how
   much IDP depth any team can actually use, no matter how the raw dynasty values
   compare) — check this before assuming a lopsided-looking IDP-for-value trade is
   actually a real need-fill for either side.
3. **External grounding, every named player, both sides** — current injury/role/trade-
   value-consensus via web search, cited. This applies to the players coming back to
   roster 5 just as much as the ones leaving — don't ground only the side you're
   evaluating critically and wave through the other.

Only the resulting **verdict** needs to reach Tyler by default, per the existing
policy — the full roster tally, tool outputs, and citations are supporting evidence,
not something to dump in full unless asked.

## 6. Data-source inventory

| Need | Tool | Notes |
|---|---|---|
| All 12 rosters, win/loss records | `mcp__sleeper-mcp__get_league_rosters` | Required for both need-matching and contender/rebuilder read |
| League shape (superflex, slots, draft rounds) | `mcp__sleeper-mcp__get_league_info` | Confirm, don't assume |
| First-pass trade candidates | `mcp__sleeper-mcp__find_win_win_trades` | Starting signal only — §2's verification steps are mandatory, not optional |
| One-off trade analysis (last week's stats) | `mcp__sleeper-mcp__analyze_trade` | `giving_player_ids`/`receiving_player_ids`; optional `season`/`week` |
| Market-consensus dynasty value | `mcp__sleeper-mcp__get_trade_value` | Superflex-aware already; no IDP coverage (`found:false`) |
| This-league's own completed-trade prices | `mcp__sleeper-mcp__get_recent_transactions(type_filter="trade")` | Per-week call, loop as needed |
| Draft pick inventory | `mcp__sleeper-mcp__get_league_drafts` → `get_draft_picks(draft_id)` | Needed when a pick, not just a player, is part of the deal |
| Player identity/lookup | `mcp__sleeper-mcp__search_players` | Resolve a name to `player_id` or vice versa |
| Long-horizon dynasty scouting | `mcp__sleeper-mcp__dynasty_scout` / `get_player_report` | Coarser than `get_trade_value` — use for IDP or general roster-building context |
| This-week/current role, injury recovery truth | `WebSearch` | Mandatory per §5.3 for every named player on both sides |
| Trade-block contents (other teams' listed-available assets) | **none yet** | Confirmed not exposed by the public API; would need new browser-automation discovery work — say so plainly rather than guessing |
| Propose a trade live | **not available** | `trade_propose_via_seam` is a hard stub that always errors (Epic 4 cut) — Tyler sends real proposals himself in the app |
