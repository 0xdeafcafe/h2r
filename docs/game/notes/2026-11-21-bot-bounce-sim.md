# The bots' superbounces in sim mode: practice runs and tuning sweeps

Evidence for [bots.md](../bots.md), "Tuning the bounce", and [sim.md](../../runtime/sim.md), "Bounce practice". Nothing here states what the code does; the design pages do.

## 2026-11-21: harness written, no numbers yet

Written at the `sb6-bottrain` branch (off `2c6b9ff41`). No run has been made: the tables are empty until the runs below fill them. Each filled entry names the commit, the machine (the quiet laptop, through `h2 lab`), `H2_OPT`, and the exact command.

### Before a run

```sh
# Once per map: the nav mesh and the spot scan (<map>.spots) in the data's bots/.
h2 bots nav zanzibar
h2 bots nav ascension
h2 bots nav lockout
h2 bots nav midship
# The image, built once (sim never builds); H2_OPT the same on every sim command.
H2_OBJ_CACHE=~/Library/Caches/h2recomp/obj H2_OPT=2 h2 build
```

### Commands

```sh
# One practice: four bots, Ascension, 18000 ticks (10 game minutes), unpaced.
H2_OPT=2 h2 sim run docs/runtime/sim-examples/superbounce-practice.toml \
    --report /tmp/bounce-practice.json

# Twelve practices: four maps by three seeds, one arm.
H2_OPT=2 h2 sim batch docs/runtime/sim-examples/superbounce-batch.toml --parallel 4

# The sweep: a coordinate search over six fields, two passes, each candidate the twelve runs.
H2_OPT=2 h2 sim sweep docs/runtime/sim-examples/superbounce-sweep.toml --dry-run
H2_OPT=2 h2 sim sweep docs/runtime/sim-examples/superbounce-sweep.toml --parallel 4

# What the runs proved and the ghosts they kept, into the shared bots/.
h2 sim harvest docs/runtime/sim-examples/results/bounce-practice --dry-run
h2 sim harvest docs/runtime/sim-examples/results/bounce-practice
```

The expected cost of a run is a guess until the first run times it. It is the boot (about 20 s of setup in other sim runs) plus 18000 ticks at the sim's unpaced speed with four bots. The sweep's dry run lists the candidates. With these axes the first pass is c000 plus at most 12 candidates (two new values on each of six axes), each candidate twelve runs.

### What to look at

- `report.json`'s `bounce` block per run: `totals.attempts` (the practice tried at all), `primes` against `attempts` (the prime works), `bounced_tries`/`bounce_rate`, `launch` (height from the touchdown), `why` (what fails most), `first_bounce_ticks` per bot.
- `compare.txt`'s `bounce practice` table: each map's tries, rate, launch and the top three reasons.
- `sweep.md`: the ranked candidates and the winner's `[bounce]` block. If it beats c000 on every map with enough tries, copy its numbers into `bots::tuning::BOUNCE_DEFAULT` (`crates/live/bots/src/tuning.rs`). Then confirm in a game that the bots play the new numbers.
- `attempts` of 0 on a map: no scan (`<map>.spots` missing from the data's `bots/`), or no crouch spot near a site. The run's `log.txt` names the spots loaded (`bots: <map>: N superbounce spots from <map>.spots`).

### One practice (superbounce-practice.toml)

| Commit | Machine | Wall s | Ticks | Tries | Primes | Bounced | Rate | Launch mean/best | Bounced deaths | Top reasons |
|---|---|---:|---:|---:|---:|---:|---:|---|---:|---|
| | | | | | | | | | | |

### The batch, per map (superbounce-batch.toml)

| Map | Runs | Tries | Primes | Bounced | Rate | Chain bounces | Launch mean/best | Bounced deaths | Top reasons |
|---|---:|---:|---:|---:|---:|---:|---|---:|---|
| zanzibar | | | | | | | | | |
| ascension | | | | | | | | | |
| lockout | | | | | | | | | |
| midship | | | | | | | | | |

### The sweep (superbounce-sweep.toml)

| Rank | Id | Tuning | Tries | Rate | Launch mean | Launch best | Chain bounces | Bounced |
|---:|---|---|---:|---:|---:|---:|---:|---:|
| | c000 | built-in | | | | | | |

The winner per map against c000:

| Map | c000 rate | Winner rate | c000 launch | Winner launch |
|---|---:|---:|---:|---:|
| zanzibar | | | | |
| ascension | | | | |
| lockout | | | | |
| midship | | | | |

## 2026-11-22: before the reachability fix (no bot reaches its spots)

Measured at `ed34fe894` (`sb6-bottrain`), **before** the practice paired sites with reachable crouch spots and walks gained the last stretch to a goal off the mesh. The batch is `superbounce-batch.toml` (four bots a run, four maps by three seeds, 18000 ticks each, the built-in bounce tuning), reproduced with the batch command above. No bot bounced:

```
map        tries  bounced  primes  why not (top 3)
ascension    467        0      68  goal_off_mesh 158, no_route 118, path_lost 93
lockout     1204        0       0  goal_off_mesh 787, no_route 254, no_path 133
midship      679        0      96  no_route 465, path_lost 96, missed 28
zanzibar    1391        0       0  no_route 994, goal_off_mesh 397
(all)       3741        0     164  no_route 1831, goal_off_mesh 1356, path_lost 191
```

The one practice on Ascension (`superbounce-practice.toml`) gave 143 tries: goal_off_mesh 52, no_route 34, prime_failed 28, not_primed 15, path_lost 13. Lockout and Zanzibar primed nothing.

What the code did then, at `ed34fe894`:

- `goal_off_mesh` (`nav/route.rs:82`): the walk's corridor ends at the mesh point nearest an off-mesh goal, pulled 0.3 inside the edge (`nav.rs:600`), and the walk failed when that end was more than its tolerance (0.3, 3D) from the goal. A scanned stand, its approach and a crouch spot's start sit on seams and edges, so they failed. The planner's walk measure (`bounce_route.rs:58`, `nav.path`) snapped silently within 3 units, so it planned walks that then failed.
- `no_route` (`mind/order.rs:273`): the practice planned with `plan_with` in Fun mode (`order.rs:263`), which drops a site more than 60 units from the bot (`bounce_route.rs:451`) and a route with more than 60 units of walking (`bounce_route.rs:493`), while the practice picked sites within 120 (`practice.rs:27`, `practice.rs:317`). The crouch spot was one of the 4 nearest the bot (`bounce_route.rs:318`), not one near the site, and nothing checked reachability before a try was spent (`practice.rs:447`).
- `no_path` (`nav/route.rs:34`): the walk excludes teleporter and crouch-jump links, but the planner's measure took them (`bounce_route.rs:58`).
- `path_lost` (`nav/route.rs:91`): the follower gave up, by its stuck check (`nav.rs:1261`: 30 ticks without getting nearer, a jump, 30 more) or its fall check (`nav.rs:1232`). Read from the code, not traced: the arrival test was 3D (`nav/route.rs:73`), so a body at the corridor's end near a goal at a seam's height, under the floor beside it, could stand short of the tolerance and stall there.


## 2026-11-22: after the reachability fix (`91ff0ee7b`), still no bounce

The same batch at `91ff0ee7b` (sites paired with reachable crouch spots, the last stretch to a goal off the mesh), run by the coordinator:

```
map        tries  bounced  primes  why not (top 3)
ascension    208        0     210  not_primed 102, path_lost 89, no_bounce 8
lockout      269        0     114  path_lost 128, prime_failed 74, not_primed 67
midship      304        0     123  path_lost 148, not_primed 85, prime_failed 30
zanzibar     727        0       0  path_lost 645, prime_failed 82
(all)       1508        0     447  path_lost 1010, not_primed 254, prime_failed 189
```

`no_route` and `goal_off_mesh` are gone. Read from the runs' `report.json` (`state_components_v1`: each player's position, crouch and flags by tick) and the code, at `91ff0ee7b`:

- **Zanzibar.** 169 spots (BSP 0, the only structure BSP the map has), and nearly every site is within 60 units of the two proven crouch places in `superbounce::PROVEN`, which pair first. The window one (5.22, −26.285, 5.11) is on a sill, its lip the window's glass (an object, broken first in the recipe), its way out off the sill. In `zanzibar-s1` player 0 walks up to the sill (tick 933), walks its prime's start off the edge and falls 3 units to z 2.11 (ticks 982 to 1023), then crouches on the floor below, where nothing forces it: `prime_failed`. Others stand at (2.0, −27.5, 2.4) under the window and jump in place, as the follower's stuck jump does (927 still ticks there in one run), then the route ends `path_lost`. Which link it was at is not read; the split reasons will say. The BSP-0 scan is not the cause.
- **Ascension's measured site** (−10.62, 3.88, −3.19): 23 tries, 23 `path_lost` in `ascension-s1`. The bots prime in the measured corner (crouch, stand up at (−10.91, 0.75)), then stand on the ramp at (−10.6, 3.0, −2.6): the approach, 0.8 behind the crease, keeps the crease's height, 0.6 under the ramp's floor there, so the walk never arrives.
- **not_primed.** Before a jump the bots stood still for 50 to 99 ticks most often (in `ascension-s1`, 184 of 251 jumps followed a still spell of 50 ticks or more; 33 of them 90 to 99 ticks, the settle's give-up time), at the sites' stands (−8.3, 3.7, −3.0). A primed body sleeps after 38 still ticks and wakes at 0 (superbounce.md, "Charge"). The settling stick near the stand moved the body too little to count, and the settle waited out its 90 ticks.
- **prime_failed** elsewhere: scanned crouch spots whose recipe does not turn the word (lockout 74, midship 30), and a word lost on the walk out.
- **no_bounce** (8, Ascension): landings at the stand, primed, that did not launch: the stand's place within the slot, the jump's drift (0.15) and the approach line are the sweep's (`superbounce-sweep.toml`) once bounces happen. Nothing here changes them.

Changed after this, in the next commit: the walk's goal put on the mesh's floor up to a standing height over it, the last stretch only over the collision's floor, the walk planned again three times when lost and its reason split by check and leg, no headroom stop within 0.5 of a walk's goal, a settle that has not moved for 12 ticks jumps, a word lost while settling primes again, and crouch spots and sites that keep failing given up for the map. The next batch's numbers go below.
