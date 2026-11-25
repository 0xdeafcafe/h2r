# Bots fighting like people: round 1 on Midship

Evidence for [bots.md](../bots.md), "Finishing and breaking off" and §3.6
("Trips that fail", the slow-bolt reach). Nothing here states what the code
does; the design page does.

## 2026-11-23: one 5-minute round per game type, before the fixes

Measured by the coordinator with `h2 sim run` on the fast tier (unpaced,
seed 11, 9000 ticks), before branch `worktree-agent-abc8be8054d2ed029`
(its base is `75b0c2b4d`). Midship, eight phantoms on Legendary, all arm
`base`. Each run's `report.json` holds `players[].doing` (ticks per
activity), `weapon_accuracy` (shots and hits by range: close under 3
units, mid 3 to 12, long beyond, `crates/extensions/game/src/bot_accuracy.rs`),
`combat`, `stuck_ticks` and `repaths`.

To reproduce: a `match.toml` with `map = "midship"`, `game_type` slayer or
juggernaut, `seed = 11`, `max_ticks = 9000`, `time_limit_s = 300`,
`pace = "unpaced"` and eight `[[bot]]` phantoms at `skill = "legendary"`
([sim.md](../../runtime/sim.md)), then `h2 sim run match.toml --report report.json`.

**Team Slayer 4v4.** Score 6 to 11, 17 kills. Every bot held the plasma
rifle (Midship's start weapon) nearly all match.

| Bot | K/D | Shots | Acc | Fight | Support | Rearm | Recover | Chase | Melee | Stuck |
|---|---|---|---|---|---|---|---|---|---|---|
| Kestrel | 1/2 | 455 | 0.16 | 45% | 29% | 19% | 4% | 1% | 0 | 38 s |
| Moth | 2/3 | 481 | 0.18 | 47% | 22% | 12% | 15% | 0% | 0 | 13 s |
| Saber | 2/2 | 517 | 0.24 | 47% | 24% | 14% | 12% | 0% | 0 | 23 s |
| Pike | 2/3 | 509 | 0.20 | 53% | 19% | 14% | 12% | 0% | 0 | 14 s |
| Wren | 2/4 | 550 | 0.19 | 50% | 28% | 12% | 7% | 0% | 0 | 23 s |
| Gully | 3/0 | 391 | 0.22 | 31% | 6% | 56% | 5% | 0% | 0 | 3 s |
| Thorn | 2/4 | 356 | 0.19 | 47% | 30% | 15% | 4% | 1% | 0 | 30 s |
| Ember | 4/1 | 617 | 0.18 | 47% | 22% | 22% | 7% | 0% | 0 | 14 s |

Plasma rifle hits by range, all eight bots together: close 90 of 214
(0.42), mid 492 of 2940 (0.17), long 1 of 260 (under 0.01). No bot pressed melee
all match. Gully never died, never switched weapons and spent one stretch
of about 70 s (ticks 4386 to 6514) in `rearm` without picking anything up.

**Juggernaut, eight players.** Total score 6. Shots 402 to 787 per bot at
0.10 to 0.27; `rearm` 12 to 36%, `supply` 8 to 17%; objective ticks 0 to
2 s for every bot (`play` only while the Juggernaut was in view).
