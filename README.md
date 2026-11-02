# sleeper-mcp

Rust CLI + MCP server for Sleeper fantasy sports. Two binaries built from one codebase: `sleeper` (terminal) and `sleeper-mcp` (stdio MCP server for Claude/Goose).

## Build

```bash
cargo build --release
# binaries at target/release/sleeper and target/release/sleeper-mcp
```

Requires: Rust 1.76+, no external system deps (SQLite is bundled via sqlx).

## Configuration

```bash
export SLEEPER_DATA_DIR=/path/to/cache   # default: ./cache
```

Two SQLite databases are created automatically under `SLEEPER_DATA_DIR`:
- `http_cache.db` — SHA256-keyed HTTP response cache with conditional GET support
- `stats.db` — permanent weekly player stats store (completed weeks never re-fetched)

## CLI (`sleeper`)

```
sleeper <COMMAND>

Commands:
  user              Get a Sleeper user by username or ID
  leagues           List all leagues for a user/season
  league            Get league details
  nfl-state         Current NFL season state (week, type)
  standings         League standings sorted by wins/points
  rosters           All rosters in a league
  matchups          Matchup overview for a specific week
  start-sit         Start/sit recommendations for a roster
  waivers           Waiver wire recommendations (trending adds not yet rostered)
  trending          Trending players across all Sleeper leagues
  trade             Analyze a trade by player IDs
  transactions      Recent transactions for a league week
  search            Search NFL players by name/position
  stats             Player stats for a specific week
  available         Unrostered players ranked by season fantasy points
  roster            Roster details with player names, positions, injury status
  needs             Roster needs analysis: strengths, weaknesses, injury risks
  dynasty           Dynasty tier report: cornerstone/keeper/trade/cut classification
  dynasty-scout     Young unrostered players sorted by age — dynasty waiver targets
```

### Common invocations

```bash
# Season state
sleeper nfl-state

# Your league standings
sleeper standings --league-id 1124822842475569152

# Your roster (tstapler = roster_id 11)
sleeper roster --league-id 1124822842475569152 --roster-id 11

# Dynasty tier report for your roster
sleeper dynasty --league-id 1124822842475569152 --roster-id 11

# Find young unrostered players to target on waivers (dynasty)
sleeper dynasty-scout --league-id 1124822842475569152 --max-age 25 --limit 30

# Available unrostered RBs ranked by season points
sleeper available --league-id 1124822842475569152 --position RB

# Waiver recommendations (trending adds not on your rosters)
sleeper waivers --league-id 1124822842475569152

# Analyze a trade (player IDs from search)
sleeper trade --giving 4046 --receiving 6794 7564
```

## MCP Server (`sleeper-mcp`)

The MCP server exposes 19 tools over stdio, consumed by Claude Code (or any MCP client).

### Claude Code setup

`.mcp.json` in the repo root points to the release binary:

```json
{
  "mcpServers": {
    "sleeper": {
      "command": "/path/to/sleeper-mcp/target/release/sleeper-mcp",
      "args": [],
      "env": {
        "SLEEPER_DATA_DIR": "/path/to/sleeper-mcp/cache"
      }
    }
  }
}
```

After `cargo build --release`, restart Claude Code and the `sleeper_*` tools appear automatically.

### MCP Tools

| Tool | Description |
|------|-------------|
| `get_user` | User by username or ID |
| `get_user_leagues` | All leagues for a user/season |
| `get_league` | League details |
| `get_nfl_state` | Current week, season type |
| `get_league_standings` | Standings sorted by wins/points |
| `get_league_rosters` | All rosters |
| `get_matchups` | Matchup overview for a week |
| `get_start_sit_recommendations` | Start/sit by projected points |
| `get_waiver_recommendations` | Trending adds not yet rostered |
| `get_trending_players` | Trending adds/drops platform-wide |
| `analyze_trade` | Trade analysis by player IDs |
| `get_recent_transactions` | Transactions for a league week |
| `search_players` | Player search by name/position |
| `get_player_stats` | Stats for a specific week |
| `get_league_drafts` | All drafts for a league |
| `get_draft_picks` | All picks for a specific draft |
| `get_roster_with_names` | Roster with player names and injury status |
| `analyze_roster_needs` | Positional strengths, weaknesses, priorities |
| `dynasty_report` | Dynasty tier classification for a roster |
| `dynasty_scout` | Young unrostered targets (dynasty waivers) |

## Dynasty Workflow

This is a dynasty league (Big Blue 2, 12-team Half-PPR, `league_id=1124822842475569152`). Run this workflow each offseason:

### 1. Tier your roster

```bash
sleeper dynasty --league-id 1124822842475569152 --roster-id 11
```

Dynasty tiers are position-aware:

| Tier | Label | QB | RB | WR/TE/IDP |
|------|-------|----|----|-----------|
| 1 | Cornerstone | ≤26 | ≤24 | ≤25 |
| 2 | Keeper | 27–33 | 25–27 | 26–29 |
| 3 | Trade candidate | 34–36 | 28–30 | 30–33 |
| 4 | Cut / move on | 37+ | 31+ | 34+ |

### 2. Scout waiver targets

```bash
# All positions, age ≤ 25
sleeper dynasty-scout --league-id 1124822842475569152 --max-age 25

# WRs only
sleeper dynasty-scout --league-id 1124822842475569152 --position WR --max-age 26
```

### 3. Check availability

```bash
sleeper available --league-id 1124822842475569152 --position RB
```

### 4. Evaluate trades

Use `analyze_trade` (MCP) or `sleeper trade` (CLI) with the player IDs from search results.

## 2026 Rookie Draft Prep

**Season status (May 2026):** 2025 dynasty season is complete. tstapler finished 12th (last place) with a 9–19 record, 1396.86 pts. The 2026 rookie draft has not yet been created in Sleeper but will open before the NFL season.

**Draft slot:** tstapler likely holds **pick 1.01** (last-place finish in a 12-team league gets the first pick). Verify once the draft is created:

```bash
sleeper leagues --user-id 999021676069273600 --season 2026
# then get the draft_id and check:
sleeper drafts --league-id 1124822842475569152
```

### 2026 Dynasty Rookie Rankings (post-NFL Draft consensus, May 2026)

**Tier 1 — Take at 1.01–1.04**

| Rank | Player | Pos | Team | Notes |
|------|--------|-----|------|-------|
| 1 | Jeremiyah Love | RB | TBD | Unanimous 1.01 in 1QB leagues (97% of drafts). First consensus RB1 since Jonathan Taylor 2020. |
| 2 | Carnell Tate | WR | Tennessee Titans | Cam Ward's WR1. Top target share, immediate starter. |
| 3 | Jordyn Tyson | WR | New Orleans Saints | 8th overall pick. Chris Olave leaves after 2026 → could be outright WR1 by 2027. |
| 4 | Makai Lemon | WR | Philadelphia Eagles | Elite supporting cast. Some experts rank above Tate by August. |

**Tier 2 — Strong value (1.05–1.09)**

| Rank | Player | Pos | Team | Notes |
|------|--------|-----|------|-------|
| 5 | Seth McGowan | RB | TBD | Three-down potential. Experts significantly above market ADP. |
| 6 | Kaelon Black | RB | TBD | Breakout candidate / high-quality handcuff. |
| 7 | Emmett Johnson | RB | Kansas City Chiefs | Clear RB2 behind starter; massive injury-replacement value in KC offense. |

**Tier 3 — Round 2 targets**

| Rank | Player | Pos | Notes |
|------|--------|-----|-------|
| 8 | Malachi Fields | WR | 6-5, 218 lbs, 38" vertical. Elite athletic profile, longer development. |
| 9 | Eli Raridon | TE | New England Patriots. Round 3 pick. Heir to Hunter Henry; attached to Drake Maye long-term. |
| 10 | Drew Allar | QB (SF) | Pittsburgh Steelers. Round 3. Strong arm + mobility. Experts 10 ADP spots above market. |

**Class summary:** WR is the deepest position. RB has Love at the top but falls off sharply. TE class is weak. In a 1QB half-PPR league: **take Love at 1.01**, then target Tyson or Tate at 1.03–1.04, then load up on WR depth in round 2.

### Fernando Mendoza (QB, Raiders) — Superflex note

If your league were Superflex, Mendoza would compete for 1.01. In 1QB, he's a non-factor — skip.

## Architecture

```
src/
  lib.rs              module re-exports
  main.rs             sleeper CLI (clap derive)
  bin/sleeper_mcp.rs  MCP server entry (rmcp stdio)
  models.rs           serde structs for all Sleeper API types
  config.rs           Config (SLEEPER_DATA_DIR or ./cache)
  error.rs            thiserror Error enum + Result<T>
  client.rs           SleeperClient: reqwest + SQLite cache + rate limiter
  cache.rs            HttpCache: SHA256-keyed SQLite response cache
  rate_limit.rs       RateLimiter: goroutine actor (tokio::spawn + mpsc)
  db.rs               StatsDb: permanent weekly stats (SQLite)
  analysis.rs         AnalysisService: standings, matchups, trade, dynasty
  cli.rs              19 CLI commands (indicatif + console colors)
  mcp.rs              19 MCP tools (rmcp #[tool] macros)
```

**Concurrency:** All parallel API calls use `tokio::spawn` → `JoinHandle`. The rate limiter is a goroutine actor: callers send oneshot reply channels via an mpsc inbox; the actor grants tokens and replies. `Arc<SleeperClient>` is cloned cheaply into each task.

## Key IDs

| Item | Value |
|------|-------|
| User ID | 999021676069273600 |
| Username | tstapler |
| Primary league | Big Blue 2 |
| League ID | 1124822842475569152 |
| Roster ID | 11 |
| Format | 12-team Half-PPR dynasty |
