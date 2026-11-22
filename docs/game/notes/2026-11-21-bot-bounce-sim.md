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

## 2026-11-22: after the walk and prime fixes (`c3981045a`), still no bounce

The same batch at `c3981045a`, run by the coordinator:

```
map        tries  bounced  primes  why not (top 3)
ascension    117        0     110  pair_lost 50, missed 27, path_lost_jump_crouch 16
lockout      129        0      99  no_drop 45, pair_lost 40, path_lost_stuck_crouch 18
midship      164        0     102  pair_lost 56, path_lost_stuck_crouch 48, prime_failed 20
zanzibar       0        0       0  (no tries at all)
(all)        410        0     311  pair_lost 146, path_lost_stuck_crouch 67, no_drop 46
```

Read from the runs' `report.json` (`state_components_v1`: position, velocity, crouch and flags by tick, the first 12268 ticks of each run). The word isn't in the state components, and the route's touchdowns (`Touchdown::word`) aren't in the report, so nothing below reads the word itself. The scripts were one-off Python over those files and aren't kept.

**Ascension's measured site, against the recipe.** At `c3981045a` the bots took the measured spot (−10.62, 3.88, −3.19) as a standing jump (`Site::measured`: fall 0). What `ascension-s1` player 2 did, ticks 3390 to 3754, against the recipe (superbounce.md, "The forced crouch leaves state"):

| | Recipe | The bot |
|---|---|---|
| After the prime | walk to the upper level, (−8.2, 2.4, −0.72) | walks down the ramp from the corner to the stand, (−10.68, 3.86, −3.19) |
| The jump | a running jump from (−8.4, 2.5, −0.76), heading (−0.8, 0.6) at about 1.6 units/s | a standing jump on the stand after 17 still ticks: +3.08 up, no sideways speed |
| In the air | easing off the stick | 0.15 of stick toward +y: no sideways speed at all |
| Touchdown | (−10.2 to −10.7, 3.93 to 4.04, −3.19) at −5.4, moving slowly toward +y (0.27 to 1.29) | where it jumped, at −2.9 (once −3.04, sinking 0.07) |
| Landings a prime | one | three to five, then the word reads lost and it walks back up to prime again |

Every one of the site's tries in the three Ascension runs ended `pair_lost` (6, 8, 7) or `path_lost_jump_crouch` (4, s1). A standing jump lands at −3 and never bounces there; the site needs the 2.43 drop. Gravity in these runs, from player 0's standing jumps (ticks 1619 to 1662): +3.08 to −2.90 in 43 ticks, 4.14 units/s², the scan's 4.15. From −0.76 a jump of +3.08 lands on −3.19 at (3.08² + 2 × 4.15 × 2.43)^½ = 5.44; a walk-off lands at 4.49.

**missed** (44 over the batch, 27 on Ascension). Every site they came from was a scanned standing site (stands from z −0.8 to +1.06). Of the 279 touchdowns within 0.3 of those stands, 271 came from a standing jump with no sideways speed, at −2.90 (median; −1.20 to −3.04). They came down a median 0.087 from the stand (10% under 0.049, 10% over 0.18), and slid a median 0.014 (10% over 0.072) on the slope in the six ticks after, so the next jump, from where it came to rest, landed farther off: player 0's eight tries on (5.87, 6.36, 1.06) walked 0.014 a try down the crease. A crease's band is 0.1 (`superbounce::bounce`'s `CREASE_BAND`), so a landing more than 0.05 off is `missed`. What's wrong: the bot jumps straight up from wherever it stands, and the drift that should carry it onto the line doesn't move it. The 0.15 of stick it holds toward the drift in the air is inside the move stick's dead zone (`nav::route`'s 0.15): free fall in these runs does change sideways speed under a bigger stick (about 0.033 units/s a tick), but not under that one.

**pair_lost** (146). It is the route's reprime budget running out (`bounce_route::REPRIMES`, 2): a word read as anything but −3 on the ground after the prime, the third time. Of the 393 primes in the batch that were followed by standing jumps and then a walk back to a crouch spot, the jumps before the walk back numbered 3 to 5 in 278 (1 to 2 in 26, 6 or more in 89; the measured site's 53: 3 to 5 in 51). None crouched (a crouch starts the next window), and the longest still spell before the last jump was a median 16 ticks; 24 had 38 or more, where the body sleeps. So in most, neither of the known clearers (a crouch, sleep) came before the word was lost: it went after a few landings at −3 that didn't launch. Which part of a landing rebuilds the pair isn't read here; the drill below reads it with the word, one landing a try.

**Zanzibar** (0 tries). No practice event at all, and every bot stood where it spawned for the whole match (`zanzibar-s1`: positions unchanged from tick 42). A strike only follows a tried route (`Mind::practice_tally`), so none was made: the pick returned `Nothing` from the start. Every site there pairs first with the two proven crouch places (`superbounce::PROVEN`), the window sill (5.22, −26.285, 5.11) and (8.07, −3.01, 4.51); with neither paired no site has a crouch spot and nothing is picked. Not confirmed here: the runs say only that nothing was picked. The likeliest change since `91ff0ee7b` (727 tries, these two crouch places) is the last stretch's new floor check (`nav::route`'s `tail_ok`: collision floor under every 0.25), which a walk onto a sill, or off the mesh across an edge, now fails. The pick now says why in the log once a map (`bots: bounce practice: nothing to try: ...`, with each proven crouch spot's walks).

Changed after this, in the next commit on `sb6-bottrain`:

- The measured spot is the recipe's drop: `Bounce::takeoff` (−8.4, 2.5, −0.76), `Site::measured` falls 2.43 from it.
- A drop that a walk-off can't make fast enough is the recipe's running jump: run-up 0.6, the ground speed that carries the flight onto the aim (about 1.3 units/s here), the jump at the launch point, and in the air steering of where the landing comes down, letting go within 0.03. Sticks are at least 0.2.
- Strikes lapse after 2 minutes, what is given up comes back after 3, a proven crouch spot is never given up, and a pick that would find nothing for what was given up takes it all back.
- The bounce drill (`[drill]`, sim.md, "The bounce drill") and [superbounce-ground-truth.toml](../../runtime/sim-examples/superbounce-ground-truth.toml): one bot, the measured spot, each try from the takeoff with the word set to −3. Its `why` reads the approach and the jump alone; `pair_lost` there is the word lost between the start and the jump.

Not changed: the standing sites' drift (`[bounce] drift`, sweepable without a rebuild; the traced bounces came in at 0.27 to 1.29 units/s toward the drift), and the HUD's own spot data.

## 2026-11-22: the ground-truth drill at `66784c400`, no bounce

`h2 sim run docs/runtime/sim-examples/superbounce-ground-truth.toml` (one bot, Ascension, 9000 ticks), run by the coordinator; read from its log and `report.json`'s `bounce` block. 9 tries, all at the measured site (−10.62, 3.88, −3.19), `why` {missed: 9}, `primes` 0, `primes_read` 0, no bounce. The log, in order: `bots: bounce drill: no known site within 0.5 of [-10.62, 3.88, -3.19] (1 sites known)` after the match started and before the nav mesh loaded; the nav mesh loading; `sim: drill: the word was set to -3 at the start` (once: the note is said once a match, not once a write); nothing else about the tries.

What it does and doesn't show:

- The word got there. A drill try starts only once the word reads −3 at the start (`Mind::drill_next`), and a word read as anything else on the ground before the jump ends the try `pair_lost`, so `missed` × 9 means the word was −3 up to the takeoff. Spawning or respawning doesn't defeat it: the bot asks for the word whenever it stands at the start unprimed.
- `primes` and `primes_read` are 0 by design: they count primes at a crouch spot, and a drill's route starts primed. The route also said a word it started with was assumed rather than read (`OrderRun::new` set `read` false), so a set word left no trace at all in the report.
- The `no known site` line is the bot looking before the map's spots were all in (1 site known, not the measured one). It waits and looks again; the 9 tries are at the site, so it found it later. Nothing said so.
- `missed` is a touchdown more than 0.1 (half `SLOT_WIDTH`) from the aim, which was the stand. The traced bounces landed on x −10.2 to −10.7, y 3.93 to 4.04: 0.05 to 0.16 past the stand in y and up to 0.4 off it in x. The takeoff headed at the stand along (−0.85, 0.53), 5° off the recipe's (−0.8, 0.6). Where the bot came down isn't in the report.
- The unit tests' drop body moved at the stick's speed in the air too, so the air steering (which predicts the landing from the sideways speed held, as the game's body keeps it) swung the body back onto the ledge: three of `bounce_route`'s drop tests failed. The game's body keeps its speed in the air and the stick nudges it (about 1 unit/s² at a stick past the dead zone, the round-3 runs' free falls).

Changed after this, in the next commit on `sb6-bottrain`:

- The measured spot aims at the middle of the traced landings, (−10.45, 3.985, −3.19) (`Bounce::landing`, `Site::aim`): the takeoff then heads along (−0.81, 0.59), 1° off the recipe's, at about 1.23 units/s.
- A route started primed from a word read at −3 says it was read (`OrderRun::primed_read`), and the report's `armed` counts those tries.
- Each try logs one line, `bots: bounce try: ...`: the last jump's takeoff, ground and vertical speed, word, air stick, touchdown and result, and the results of the jumps before it (bots.md, "Practising bounces", "The try's line"). The drill says once which site it found.
- The drop tests' body keeps its sideways speed in the air (`TEST_AIR_ACCEL`, 1 unit/s²).

## 2026-11-22: the drill four ways

The ground-truth drill (`h2 sim run docs/runtime/sim-examples/superbounce-ground-truth.toml`, one bot, Ascension, 9000 ticks) at `47782ee8f`, run by the coordinator four ways; the logs and reports are the coordinator's `ground-truth{4,5,6,7}`. Every try started with the word read at −3 (`armed`). The `bots: bounce try:` lines give the bot's view; the per-tick reads below are from `report.json`'s `state_components_v1` (`player.00.pos`, `player.00.velocity`), with one-off Python that isn't kept.

| Run | Change | Tries | Bounced | Lift-off (trace) | Ground | Flight | Touchdown (trace) | In | Out |
|---|---|---:|---:|---|---:|---:|---|---:|---:|
| 4 | as committed: run stick `stick_for(want + 0.5 (want − have))`, run-up 0.6 | 10 | 0 | (−8.42, 2.62, −0.76) | 1.02 to 1.08 | 60 ticks | (−10.21, 3.85, −3.08), on the floor | 5.26 | +0.5 |
| 5 | gain 2.0 instead of 0.5 | 10 | 0 | (−8.49, 2.68, −0.77) | 0.9 to 1.75 | 60 | (−10.21, 3.86, −3.10) | 5.26 | +0.65 |
| 6 | run-up 1.5 | 8 | 0 | (−8.52, 2.55, −0.77) | 1.15 | 60 | (−10.22, 3.83, −3.09) | 5.26 | +0.58 |
| 7 | run stick forced to 1 (a diagnostic hack, reverted) | 14 | 7 | (−8.59, 2.77, −0.78) | 2.04 to 2.16 | 61 | (−10.13 to −10.32, 3.85 to 4.15, −3.31), in the slot | 5.40 | +2 to +13 |

Run 7: 13 practice bounces, chains {1: 5, 2: 1}, launches mean 15.4, best 20.9, one Bounced death, `why` {no_path: 1}; the first bot ghost, `ghost -10.538 3.880 -3.190 source=bot launch=11.93`. With the hack three `bounce_route` drop tests failed (tests.rs:745, :761, :796). Run 5 had one test failing too.

What the traces show:

- **Every try jumped, run 7 included.** Each lift-off has a vertical speed of +3.08 on its first tick (+2.80 once), the flights take 60 or 61 ticks to the touchdown (a jump of 3.08 falling 2.4 under 4.15 takes 2.05 s; a fall from rest takes 1.08 s), and the touchdowns come in at 5.26 to 5.40, a jump's (a fall from rest from −0.76 would land at 4.5 to 4.6). Run 7's try lines read `vz −0.04` because the line's takeoff is the tick A was pressed and the body left the floor 6 ticks later, outside the 6 ticks the line read; at 2 units/s it had run 0.3 on, past the edge (z −0.80 on the first tick up), and the jump went off as it left the floor. Run 4's jumps left 4 ticks after the press, 0.1 on. So the working recipe is a full-stick running jump, not a run-off.
- **The air stick changes the sideways speed by 0.035 units/s a tick**, braking and pushing, in all four runs. Under a stick pushing on at 0.6 to 1 (run 4's mean (−0.54, +0.22) toward the aim) the speed never rose past 1.27: it swung 0.92 to 1.27, 7 ticks up and 7 down, the swing of a steering some 4 ticks late. Run 7 came off the floor at 2.1 and braked at that rate to 0.40 by 45 ticks in, and a small stick on slowed it further before it grew: the stick drives the sideways speed toward the stick times about 1.2, so it can't push a slow flight on, and a small stick brakes.
- **Why runs 4 to 6 missed:** from 1.05 units/s the flight can't be pushed past about 1.2, so it came down 2.3 out, on the floor at (−10.21, 3.85, −3.08), 0.27 short of the aim, every try within 0.01 of the same point.
- **Why run 7 bounced half the time:** off the floor at 2 units/s, braked, it came down about 2.2 out, in the slot (z −3.31). The bounces came down at (−10.13, 3.85 to 3.87) with +10.5 to +12.6 out, (−10.29, 3.92) +13.1 and (−10.25, 4.07) +9.6; the misses at (−10.13, 3.88) came out at +5.5 to +7 (in the slot, short of a launch), and three at (−10.31 to −10.40, 4.14 to 4.23) at +2.0. The aim had moved to (−10.28, 3.91) after the first bounce, the verified entry's one landing.
- **The Bounced death:** the try at tick 4326 ran off the edge before its jump went off (+2.80, z −0.81 at lift-off), heading more toward +y; the air stick braked the whole flight to a standstill and it came down over (−10.19, 4.02) with no sideways speed, fell through the crease to −5.39 and died.
- Ground speed against the stick: 1.02 to 1.08 asking 0.6 to 0.8, 0.9 to 1.75 asking more (run 5), 1.96 to 2.16 at 1. A walk, then a run: not linear.
- Gravity, from every flight: 0.138 units/s a tick, 4.15 units/s².

Changed after this, in the next commit on `sb6-bottrain`:

- How a drop leaves the floor comes from the flight model, `bounce_route::drop_takeoff`: a run-off at full stick where it lands fast enough and the air stick can land it on the aim, then a slower run-off (`run_off_speed`, 0.6), then a full-stick running jump, then a slower one. Ascension's spot is the full-stick jump: its run-off lands at 4.49, under 5.4. The run's ground speed is the measured map (`ground_speed`), not a linear stick.
- A running jump presses A ahead of the launch point by the ground the body covers in 5 ticks (`INPUT_LAG`) and keeps running until the body leaves the floor.
- The air steering (`air_stick`) models the stick as driving the sideways speed toward 1.2 times the stick at 1.05 units/s² (`AIR_TOP`, `AIR_ACCEL`), predicts from where the body is once the stick acts, and asks for the speed that lands on the aim; a landing 0.3 past the aim gets the full stick (`AIR_PAST`). `air_gain` is its margin over just enough (1.2).
- A measured spot keeps its traced landing as the aim when a verified drop is merged in (`Site::verified`).
- A flight's vertical speed is read over 10 ticks from the press (`TAKEOFF_TICKS`).
- The drop tests' body is the drill's: the measured ground-speed map, a jump 5 ticks after the press, and the air stick driving the speed toward 1.2 times the stick.

## 2026-11-22: batch 4, the failures before the jump

The batch (`superbounce-batch.toml`, four bots a run, four maps by three seeds, 18000 ticks), run by the coordinator on `sb6-bottrain` into `docs/runtime/sim-examples/results/bounce-practice/` (its log is the coordinator's `bounce-sims/batch4.log`). The run records no commit; the fixes below branch from `4317badaf`.

| Map | Tries | Bounces | Primes | Top reasons |
|---|---:|---:|---:|---|
| ascension | 187 | 13 | 102 | path_lost_jump_crouch 95, pair_lost 25, missed 20 |
| lockout | 173 | 0 | 101 | pair_lost 52, prime_failed 44, path_lost_stuck_crouch 34 |
| midship | 185 | 0 | 109 | path_lost_stuck_crouch 69, pair_lost 48, missed 22 |
| zanzibar | 0 | 0 | 0 | — |
| (all) | 545 | 13 | 312 | pair_lost 125, path_lost_stuck_crouch 105, path_lost_jump_crouch 95 |

All 13 bounces are Ascension's measured drop. Most failures come before the jump, on the walk to the crouch spot or in the prime. The causes below come from the runs' `report.json` (`state_components_v1`: position, velocity, crouch and unit flags by tick), the `bots: bounce try:` lines and the maps' nav files (`H2BT`/`H2NV`, polys, edges and wall triangles). The reading was done with one-off Python that isn't kept.

- **path_lost_jump_crouch (Ascension, 95).** Three bots (s1 p0, s1 p2, s2 p0) were trapped on the ramp, mesh polys 1543 and 1544, near (−17.9, 1.3, 2.9). The walk down to their crouch spot took the Drop link 1543→1604 at the ramp's y = 1.2 edge. A rail the mesh lacks stands there: the bodies sit at y 1.285 to 1.385 against it. Each replan (3 a step) planned the same drop, and every try did the same, about 30 tries a bot for the rest of the match. 92 of the 95 ended at the route's step 0.
- **path_lost_stuck_crouch (midship 69, lockout 34).** The pattern is spells of exactly zero speed, with four jumps about 60 ticks apart. That is the follower's stuck, jump and replan cycle, and it happens at the same places, for example midship (11.12, 5.56, −0.29), (8.73, 7.04), (7.45, 7.20) and (8.03, 10.62). The walk's headroom check (`moves::headroom`) stops a standing walk before a low ceiling near the crouch spot, more than 0.5 from the goal (`END_HEADROOM` only covers the last 0.5). Not every case is proven: lockout's (5.98, −9.12) shows small speeds that are not zero, which fits a body rubbing a wall instead.
- **pair_lost (125).** Every one is at a standing site with 8 or more standing jumps (the try line keeps the last 8), all taken off at ground speed 0. The word is lost in the jump loop: in 64 of 125 it was unread at takeoff, and the rest landed `not_primed` after 1 to 3 jumps. It was never lost between the prime and the first takeoff, so the approach does not cause it. The hypothesis is that a jump straight up with no sideways speed lets the body sleep, while the drill's drops (sideways speed) kept −3 across a landing. Standing sites bounced 0 times. Nothing in the approach changes this. A standing-site drill is the experiment to run.
- **prime_failed (lockout 44).** lockout-s1 p1 at the crouch spot near (8.715, −2.915, −5.89): the crouched body stops against walls at x = 8.715 and y = −2.915 and never gets within the stand's reach. When it lets go of crouch, unit flag bit 23 clears within 6 ticks: standing fits there, so nothing forces the crouch. In the game this scanned spot does not prime. Each bot found that out for itself, 2 strikes each.
- **Zanzibar (0 tries).** The practice's log line: "157 sites, 12 crouch spots; ... no crouch spot 46, no approach 3, no walk to them 108". For the proven crouch spot at (8.04, −2.52, 4.51), a walk reaches its stand's site but no walk reaches its start from the bots. `pair_site` checks the walk from the crouch spot to the approach, which drops down from that floor, so the sites paired with it. The bots' walk to the crouch spot then failed, and the pick marked each site out of reach rather than the crouch spot. The proven crouch spots pair first, and a proven one is never given up, so all 108 sites stayed out of reach.

Changed after this, on `sb9-approach`:

- A walk lost at a jump or drop link is planned again around it: later walks of the route skip a jump, drop or crouch-jump link within 0.5 units sideways and 1 unit in height of where it gave up (`RouteWalk::failed_link`, `RouteWalk::new_avoiding`, `nav::route::AVOID_REACH`, `OrderRun::avoid`). Test: `a_walk_to_a_crouch_spot_goes_round_a_drop_it_cannot_make`.
- A walk to a crouch spot's start crouches under a ceiling the standing body does not fit, instead of stopping (`RouteWalk::ducking`). Walks on other legs still stop. Test: `a_walk_to_a_crouch_spot_ducks_under_a_low_ceiling_rather_than_stopping`.
- A failed prime counts for every bot on the map (`practice::PRIME_FAILS`), so a crouch spot whose primes failed twice between the bots is given up by each of them. Test: `a_crouch_spot_whose_primes_fail_for_two_bots_is_given_up_by_both`.
- When the walk to a paired crouch spot fails, the crouch spot is set aside for 600 ticks (`OUT_OF_REACH_TICKS`), not the site, and its sites pair again without it. The nothing-to-try line counts the crouch spots set aside. Test: `a_crouch_spot_no_walk_reaches_is_set_aside_and_its_site_paired_with_another`.
- `pair_lost` is unchanged.

What to look for in the next batch: Ascension's `path_lost_jump_crouch` should mostly go, and the bots on the ramp at (−18, 1.3) should take another way down or fail `no_path`. There should be fewer `path_lost_stuck_crouch` on midship, and the ones left (lockout's (5.98, −9.12)) are a different cause. Each bot should log fewer `prime_failed` at a scanned spot that doesn't prime. Zanzibar should try sites. If it still tries none, its nothing-to-try line shows how many crouch spots are set aside.
