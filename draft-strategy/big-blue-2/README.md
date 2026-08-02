# Big Blue 2: Electric Boogaloo Dynasty — Wiki

League ID `1352883872131481600` · 12-team Superflex, Half-PPR, TE premium, IDP (1 flex).

This is the durable, git-tracked home for team strategy, league scouting, and trade history —
kept up to date across the season instead of living only in chat. Update this index whenever a
new team page or trade file is added.

## Teams

| Roster | Owner | Notable |
|---|---|---|
| [1](teams/roster-01-jrolles.md) | jrolles | — |
| [2](teams/roster-02-jackiemoon1893.md) | Jackiemoon1893 | — |
| [3](teams/roster-03-blakemoen.md) | BlakeMoen | Thinnest IDP in the league (1 total) |
| [4](teams/roster-04-jacobverheyen.md) | jacobverheyen | Deepest roster overall (35+ rostered) |
| [5](teams/roster-05-tstapler.md) | **tstapler (us)** | See [2026 rookie draft log](2026-rookie-draft.md) |
| [6](teams/roster-06-skywalsher.md) | skywalsher | — |
| [7](teams/roster-07-mi-les.md) | Mi-les | Deepest QB room (7 total) |
| [8](teams/roster-08-beemachine.md) | BeeMachine | Thin IDP (3 total) |
| [9](teams/roster-09-ajtwarog.md) | ajtwarog | RB/WR imbalance, secondary trade candidate |
| [10](teams/roster-10-galphonse.md) | galphonse | Thin QB (3 total) |
| [11](teams/roster-11-dehnfranklincj.md) | dehnfranklincj | **RB=4/WR=15 — best trade fit in the league** |
| [12](teams/roster-12-3chainzzz.md) | 3chainzzz | Thinnest QB room (2 total) |

## Trades

| Date | Counterparty | Status | Summary |
|---|---|---|---|
| 2026-07-20 | [BlakeMoen](trades/2026-07-20-blakemoen-offer-declined.md) | Declined | 3 picks for our 1.03 — kept 1.03, took Concepcion |
| 2026-08-01 | [dehnfranklincj](trades/2026-08-01-proposed-dehnfranklincj-rb-for-wr.md) | Proposed, not sent | RB (Allen/Pollard) for WR (Johnston/Jeudy) |
| 2026-08-01 | [3chainzzz](trades/2026-08-01-proposed-3chainzzz-fields-for-wr.md) | Proposed, not sent | Fields for WR (Sutton/Shakir) — speculative |

## Other documents

- [2026 rookie draft log](2026-rookie-draft.md) — full pick-by-pick rationale, team composition
  checkpoints, and the `search_rank` tooling fix from this draft

## Data freshness

Team pages are point-in-time snapshots (`analyze_roster_needs` / `get_roster_with_names` calls),
dated at the top of each file. `analyze_roster_needs`'s tier-based need model has a known blind
spot — it returns `weaknesses: []` for every team in this league regardless of real imbalances
(see the 2026-08-01 team pages, all built from manual position-count inspection instead). Re-pull
a team's page before relying on it for a live trade decision if it's more than a couple weeks old.
