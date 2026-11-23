# The forked bounce search on Ascension's first 60 seams

Evidence for [bounce-search.md](../../runtime/bounce-search.md). Nothing here states what the code does; the design page does.

## 2026-11-23: 60 sites, 4 provable

Run on master before `273410756`; the log names no commit and no machine. Fast tier (`tier: running the fast module /Volumes/Builds/h2recomp/image/fast/libh2game.dylib` in each run's log). The log shows 5 runs of 12 sites each in 15 seats; it does not show the command line.

The config is the example's with `verify = "all"`: `map = "ascension"`, `targets = "scan"`, `max_targets = 60`, `points = 400`, `rounds = 3`, `refine = 200`, the default axes and the three default variants (fall, hop, slept).

```sh
h2 sim bounces docs/runtime/sim-examples/ascension-bounces.toml --parallel 4
```

With `verify = "all"` added to the `[bounces]` table to check every batch, as this run did. `--parallel 4` splits the 60 sites into 4 runs of 15; `--parallel 5` gives this run's 5 runs of 12. The run's log, config and results are `$RUSH_KEEP/sb-bounces/full.log`, `full.toml` and `results/full/` (`bounces.json`, `bounces.md`, `bounces.verified`, each run's `report.json` and `log.txt`), under the coordinator's session keep.

### Verdicts

| Verdict | Sites |
|---|---|
| provable | 4 |
| fragile | 53 |
| none | 1 (s055-crease) |
| untested | 2 (s021-crease, s054-vee) |

The provable sites:

| Site | Ease | Position room (cm) | Press ticks |
|---|---|---|---|
| s007-slot | 0.92 | 101 | 26 |
| s032-vee | 0.91 | 71 | 11 |
| s038-vee | 0.86 | 55 | 8 |
| s051-vee | 0.73 | 56 | 41 |

Each window rests on 8 points, the `min_support`. Their speed along the drift runs over the whole sampled range (0 to 0.8 units/s), every phase bounces at three of them (s051-vee: 89%), and every bounce of theirs bounced on all three variants (reproducible 100%).

- **Fragile sites mostly have a small provable share.** 49 of the 53 have 1 to 6% of their points bouncing on every variant. s031-crease has 0.3% (17 of 477 points), and s023-vee, s040-crease and s043-vee have none. None has a window with 8 points of support.
- **The best launches are s000, s001 and s002 (all slots), at 15.87 to 16.05 units/s.** Every other site that bounced has a best launch of 12.25 to 13.97.
- **Reproducibility.** 49 sites are at 100%; s009-slot (91%), s048-crease (91%), s005-slot (98%), s052-crease (98%) and s010-slot (99%) have points that bounced on some variants only. s023-vee, s040-crease and s043-vee are at 0%: nothing bounced on every variant.
- **Unprimed points:** 928 run with the word 0, none bounced.

### Trials and the fork

- 130,388 trials of 182,760 planned (the most refinement could add): 19,539 bounced, 106,684 did not, 4,165 failed their setup, none lost the prime.
- Of the setup failures, 3,079 are a teleport that landed off its start with its aim corrected: 2,763 of them 1 unit off or more, the rest under 1 unit. 1,073 never touched down in 7 rows, 11 died. More than half of the setup failures (2,332) are on four sites: s055-crease (1,167 of its 1,200 trials), s023-vee (430), s040-crease (408) and s043-vee (327).
- **Anchors.** 258 anchor trials came out as their drops (no bounce). 78 failed their setup on a teleport that landed off its start (such as 1.000 straight up) with its aim corrected, and 2 never touched down in 6 rows. So 129 of the 169 variant bases agree with their anchor and 40 have none to compare (`drop = anchor` in the table).
- **Forks:** 24 captures, 20,215 restores, one wave each, 183,323 ticks: 9.1 rows a wave. 42 captures or restores answered busy, the last `kernel: thread 2: continuation feature: async entry -> 0x00137fe0`. Replayed waves that ended in the same guest RAM: 12 of 12. Restores checked against their capture: 2,445 of 2,445 equal.

### Time

| Run | Sites | Batches | Waves | Ticks | Trials | Wall (s) |
|---|---|---|---|---|---|---|
| 0 | 12 | 2 | 5,235 | 47,449 | 26,587 | 1,819 |
| 1 | 12 | 1 | 2,643 | 24,134 | 24,375 | 1,110 |
| 2 | 12 | 1 | 2,644 | 27,018 | 28,932 | 1,287 |
| 3 | 12 | 2 | 5,193 | 44,293 | 25,896 | 1,867 |
| 4 | 12 | 2 | 4,500 | 40,429 | 24,598 | 1,651 |

The batch took 3,691 s. A run's wall time over its waves is 0.35 to 0.49 s a wave, setup and calibration included. Runs whose 12 sites needed two batches (sites under 1.2 units apart) ran about twice the waves.

### Harness notes

- **Out of step.** Four bases were lost to a release one to three ticks after the planned one: s001-slot on fall (released at tick 125, planned 124), s006-slot on hop (239, planned 236), s053-vee on fall (301, planned 298) and s049-crease on fall (137, planned 135). Read from the code at that commit, a teleport's answer comes one tick after its ask (the agent's calls run at the start of the next tick), so a late release is a late arm: a held body that is not still on its arm row arms only after 3 still rows (`drops::SETTLE_STILL`), and one to three ticks late is that range. The message did not name the arm tick, so the run does not show it directly.
- **s040-crease on fall:** `base: ended early`. Its drop finished before the capture tick.
- **s021-crease and s054-vee:** every variant's calibration was refused with `teleport: the player has no unit now (between lives)`.
- **The placement.** Every run noted `teleport: the placement put the origin 0.033 off (x +0.000 y +0.000 z +0.033) from where it was asked`: the game's own lift (`[0x47f05c]`, 0.0328). Its trials asked against it; no trial that asked again landed, so none was off by that lift alone.
