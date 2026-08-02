# Big Blue 2: Electric Boogaloo Dynasty — 2026 Rookie Draft

League ID `1352883872131481600` · Draft ID `1352883872144039936` · Roster ID `5` (tstapler)
12-team Superflex, Half-PPR, TE premium (+0.5/rec), IDP (1 flex), linear (non-snake) 4-round draft.

See the [league wiki index](README.md) for team pages and the trade log.

## Team strategy going in

- **Real need:** young RB (only RJ Harvey, Will Shipley pre-draft). WR looked deep but aging
  (Evans 32, Metcalf 28, Kittle/Kelce declining at TE). QB is surplus (Burrow/Fields/Tyrod) —
  tradeable, especially once other teams show SF desperation.
- **Approach:** best-player-available weighted by need early, shift to pure value + trade-capital
  accumulation once the top tiers dry up. Don't force positions the class doesn't support.
- **Pick ownership** (acquired via trades on top of the natural 4): 1.03 (own), 1.09 (from jrolles),
  2.03/#15 (own), 3.02/#26 (from dehnfranklincj), 3.03/#27 (own), 3.09/#33 (from jrolles), 4.03/#39 (own).

## Key tooling lesson (2026-07-21)

`dynasty_scout`/`dynasty_report` originally sorted rookies by age only — every player with
`years_exp <= 1` collapsed to the same Tier 1 bucket with zero differentiation. Fixed by wiring in
Sleeper's own `search_rank` signal (see `src/analysis.rs::prospect_sort_key`, tests in
`prospect_sort_tests`). **Gotcha:** `search_rank` alone doesn't confirm draft-pool eligibility —
Abdul Carter (search_rank 144, looked like a great value) turned out to be a 2025 draftee
(`years_exp: 1`) not actually in this rookie draft's pool. Always sanity-check `years_exp == 0`
for anyone who looks like a steal, and cross-check against the live Sleeper draft room screen if
something seems off.

## Pick log

| Pick | Player | Pos | Rationale |
|---|---|---|---|
| 1.03 (#3) | KC Concepcion | WR | Top remaining Tier-1 WR after Love/Tate gone; picked over Jordyn Tyson for injury-risk reasons (ACL) given roster already carries age/injury risk elsewhere |
| 1.09 (#9) | Emmett Johnson | RB | Real rookie-rank RB3-8 tier (corrected after initially misjudging Nicholas Singleton as comparable — verified against live rankings) |
| 2.03 (#15) | Jonah Coleman | RB | Best remaining skill player by search_rank (118); also gives us both sides of Denver's backfield alongside our own RJ Harvey |
| 3.02 (#26) | Kaytron Allen | RB | Best remaining by search_rank (157) once WR/TE tiers were exhausted |
| 3.03 (#27) | Rueben Bain | DL | Best true-rookie-eligible player left (152) after Abdul Carter turned out ineligible |
| 3.09 (#33) | Caleb Downs | DB | Best true-rookie IDP left (163); RB/WR pools dry — pivoted to trade-capital accumulation |
| 4.03 (#39) | Josiah Trotter | LB | Final pick; matched board (rank 181, best remaining true-rookie IDP) — draft complete for this roster |

## Team composition check-in (after pick #33)

RB is now stacked past starting relevance: Harvey, Shipley, Johnson, Coleman, Allen — 5 young
backs competing for 2 RB + up to 2 FLEX spots. IDP also building surplus (Anderson, Bolton,
Bonitto, Bain, Downs) against a single IDP_FLEX slot. WR — the actual pre-draft priority for a
youth refresh — only got one add (Concepcion).

**Conclusion:** the class itself can't fix the WR gap anymore at this draft position. Post-draft
plan is to use the RB/IDP surplus + tradeable QB (Fields) to buy a real WR2/3 from a rebuilding
team, not to force more skill picks late.

`find_win_win_trades` came back empty (`my_needs: []`) — the tool's tier-based need model doesn't
flag "aging WR corps" as a weakness since we're tier-full everywhere. This is a known blind spot:
manual roster scanning is still required to actually find a WR-needy trade partner.

## Final haul

Concepcion (WR), Emmett Johnson (RB), Jonah Coleman (RB), Kaytron Allen (RB), Rueben Bain (DL),
Caleb Downs (DB), Josiah Trotter (LB). Draft complete for roster 5 as of 2026-07-29.

## Open items (post-draft)

- [ ] Scan other rosters manually for a WR-needy trade partner, shop Fields + RB/IDP surplus
- [ ] Revisit the 5-6 dead-roster free agents ([[Big Blue 2 Dynasty League]] in the personal wiki)
      — drop only when there's an actual add queued, not proactively
- [ ] Decide keeper/taxi assignments for the new rookie class before roster lock
