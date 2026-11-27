# Why a superbounce works or not: evidence

Evidence for [superbounce.md](../superbounce.md) ("The bounce rule") and
[bounce-search.md](../../runtime/bounce-search.md). Nothing here states
what our code does; the design pages do.

## 2026-11-23: the tick of contact, read from the image and stepped in a game

Commit `94583b978`. Retail 1.0 `default.xbe` through `$H2_DISC`, read with
`h2 disasm`. The game is the fast-tier image at
`/Volumes/Builds/h2recomp/image` (`H2_OPT=2`, `H2_TIER=fast`), run headless
with `h2 serve` on Ascension and stepped one tick at a time through the
agent API. The trial data is the 60-site Ascension search
(`h2 sim bounces full.toml`, 130,388 trials). No build and no new
instrumentation. Scripts, traces and disassembly are under
`$RUSH_KEEP/sb-why/` (the coordinator's session keep), not in the
repository.

Evidence labels: **Code** (read from the disassembly at the address
given), **Trace** (measured in the stepped game), **Data** (the search's
trials), **Open** (not established).

### The answer

A superbounce needs four things on one tick. Call it the bounce tick.

1. **The pair is primed** (level pair word −3), so the narrowphase accepts
   any penetration and never moves the body back to the floor.
2. **The sweep is on.** The biped's collision agent extends the capsule
   along its motion for the tick when the body's speed is over
   5.4465 units/s. The speed tested is the one the tick moves with: the
   speed the tick starts with plus one tick of gravity (0.139).
3. **The swept capsule's axis goes through a floor polygon near one of
   its edges.** With the axis's lowest point a depth `cb` below the
   polygon and the crossing point a distance `de` from the polygon's
   nearest edge, the contact normal is sideways when `de < cb` and
   straight up otherwise.
4. **The body moves across that edge**, at 0.1 units/s or more when the
   fall starts (0.035 a tick is lost in the air).

Then two contacts face each other across the edge: the polygon the axis
goes through pushes the body out through its nearest edge, and the
polygon on the other side of that edge pushes it back. They are 0.11 to
0.27 deep and cannot both be satisfied. The solver answers with two equal
impulses of 23,000 to 134,000. The contact that pushes the way the body
drifts has its normal at right angles to the velocity, so it leans up by
`drift speed / fall speed`; the other is level. The sideways parts
cancel and the lean leaves 18.5 to 22 units/s upward.

A landing fails when any one of the four is missing, and each failure
has its own look:

| Missing | What happens | Outcome |
|---|---|---|
| the prime (word 0) | the body goes into the floor for one tick, then the pair pass puts it back on the floor at rest; the word goes 0, 1, 0 | landing |
| the sweep, with the body in the floor | one contact, normal up, 0.08 deep, impulse about 740 | stops 4 to 9 cm in the floor |
| the sweep was already on the tick before | the swept capsule reaches the floor from above; one contact, normal up | caught 5 to 15 cm over the floor |
| the edge (`de > cb`) | one contact, normal up, 0.20 to 0.33 deep, impulse about 760 | a pop of +1.5 to +2.4 |
| the drift | two level contacts, impulses of 133,000, no upward part | stops in the floor |

### Why the fall speed has a window

A falling body gains 0.139 units/s a tick, so every fall has exactly one
tick on which the sweep comes on: the tick it starts at a speed from
5.3075 to 5.4465. The search's bounces sit in that band and nowhere else
(**Data**, total speed at the start of the contact tick, bodies 3 to
15 cm in the floor with drift over 0.15):

| Speed | Bounced |
|---|---|
| 5.16 to 5.29 | 0 of 1,253 |
| 5.30 | 11 of 507 |
| 5.31 | 534 of 632 |
| 5.32 to 5.44 | 4,514 of 5,229 |
| 5.45 | 13 of 15 |
| over 5.45 | 0 of 3 |

The tick before it, the sweep is off and the capsule is tested where it
stands. Nothing is in contact when its bottom is over 0.03 from the
floor at the start of the tick (**Trace**: origin 0.040 over the floor
lands, 0.045 goes through), so the body falls 0.18 and ends up to 0.14
inside the floor with nothing pushing on it. The next tick is the one
the sweep comes on, and the swept axis finds itself through the polygon.

So for a fall near the threshold the bounce is set two ticks back:

- the tick that starts at 5.168 to 5.3075 has to start with the origin
  0.045 to about 0.17 over the floor;
- it ends 0.01 to 0.14 inside the floor;
- the next tick, starting at 5.3075 to 5.4465, bounces if `de < cb`,
  where `cb` is 0.019 plus the depth inside the floor.

Which tick of a fall is in that band is set by the drop. From rest, the
band's tick comes 3.43 down. The share of phases that bounce, by the
whole drop from the apex and the distance from the edge (the rule above,
no game run):

| Drop from the apex | `de` 0.02 | 0.06 | 0.10 | 0.14 |
|---|---|---|---|---|
| 3.25 and under | 0% | 0% | 0% | 0% |
| 3.35 | 19% | 19% | 19% | 8% |
| 3.40 | 48% | 48% | 32% | 10% |
| 3.45 | 77% | 55% | 33% | 11% |
| 3.50 | 72% | 50% | 28% | 9% |
| 3.55 | 45% | 23% | 4% | 0% |
| 3.60 | 18% | 0% | 0% | 0% |
| 3.75 to 5.5 | 0% | 0% | 0% | 0% |
| 8 | 18% | 4% | 0% | 0% |
| 10 | 26% | 14% | 0% | 0% |
| 13 | 36% | 24% | 13% | 2% |

This is the recipe's 2.43 drop under a jump of about 1, and why the
seam drops bounced from 3.45 and not from 3.65 or 3.82
([2026-11-22-seam-drop-depth.md](2026-11-22-seam-drop-depth.md)).

**Faster falls bounce from above.** Over 5.4465 the sweep is on every
tick and reaches the floor before the body does. The axis goes through
the floor when the origin is under `speed / 30 − 0.165` over it, which is
0.02 at 5.6, 0.14 at 9 and 0.27 at 13. Under that height and within `cb`
of an edge it pinches as above. Over it the capsule's skin meets the
floor first and the body is caught in the air. **Trace**, primed, 0.03
from the ramp's foot:

| Speed in | Origin over the floor | Edge | Result |
|---|---|---|---|
| 9.0 | 0.06 | 0.03 | launch +10.68; normals (0, −1, 0.029) and (0, 1, 0), impulses 98,311 and 98,238 |
| 9.0 | 0.06 | 0.03, word 0 | landing; word 0, 1, 2, 1, 0 |
| 9.0 | 0.06 | 0.20 | pop +1.83; one contact, normal up, 0.25 deep |
| 9.0 | 0.20 | 0.03 | caught at +0.23; normal up |
| 13.0 | 0.10 | 0.06 | launch +8.75; impulses 114,325 and 114,172 |

The launch falls as the fall speed rises: the solver adds 18.5 at 5.4,
19.8 at 9 and 21.9 at 13, and the fall speed comes off it.

### The code paths

**The pair pass is the narrowphase** (`0x00317950`, **Code**). For each
pair of an island it sets the collision mode to 2 and a tolerance, calls
the pair's agent (virtual `+0x10`) into a local collector of `0x30`-byte
points, then hands the collector to the pair's contact manager (virtual
`+0x0c`, `0x00317b8d`).

- Word 0 or more (`0x00317a00..0x00317a29`): the tolerance is
  `world[+0x244] + word × world[+0x248]`. A point whose distance
  (`+0x0c`) is under it (`0x00317a63..0x00317a6e`) raises the word by one
  and calls `0x00316460` with the next tolerance
  (`0x00317ac1..0x00317ae7`). That call is the recovery: the stepped
  unprimed landing goes 8 cm into the floor on one tick and is at rest on
  the floor on the next, with the word at 1 (**Trace**). When every point
  fits one level lower the word goes down (`0x00317a78..0x00317ab9`).
- Word negative (`0x003179fa`, `0x00317af1..0x00317b06`): the tolerance
  is −1000.0 (`0xC47A0000`). No point is under it, so `0x00316460` is
  never called. Only −2 is tested again (`0x00317b22..0x00317b67`).

So the word is the pair's allowed penetration, and −3 allows all of it.
The values at `world[+0x244]` and `[+0x248]` come from the world's
construction info (`0x0031088d..0x0031089d`, info `+0x54` and `+0x58`)
and are not read. **Open**

**The agent of a biped against a level polygon** is vtable `0x00412290`,
whose slot at `+0x10` is `0x002db2b0` (**Code**). Before it calls the
convex agent (`0x002eeb40`, `0x002db5e0`) it decides the sweep for each
side whose shape is the biped's wrapper (type `0x17`):

1. It reads the body's linear velocity (motion `+0x40`) and squares it
   (`0x002db331..0x002db355`).
2. It compares that with wrapper `+0x20` (`0x002db359..0x002db365`). At
   or under it the wrapper's offset (`+0x10`) is zeroed
   (`0x002db419..0x002db41c`).
3. Over it, and with `0x00279670` true for the body's object, the offset
   is the velocity turned into the body's frame times the input's
   `+0x10` (`0x002db37c..0x002db406`). In the game it reads
   (0.009, 0, −0.185) at 5.56 down: one tick, 1/30 (**Trace**,
   `world.raw`'s `shape` part).
4. Which sides sweep is a mask (1 for A, 2 for B) kept at agent `+0xf0`.
   When the mask differs from the tick before's, or a sweep is on, the
   cached features are seeded again and the cached separation bound is
   set to −1 (`0x002db549..0x002db5cb`).

The wrapper's support query adds the offset, so the convex agent sees a
capsule stretched along the tick's motion. The threshold is set by the
wrapper's constructor (`0x002dbfc0`): `+0x24` is a speed and `+0x20` its
square. On `masterchief_mp` they read 5.4465 and 29.6644 (**Trace**).
Where the caller gets 5.4465, and what `+0x28` (1.3616, a quarter of it)
and `+0x2c` (16.686) do, are not read. **Open**

**The sideways normal** comes out of the convex agent's penetration
case, which is not decoded. **Open** for the code. The rule `de < cb` is
measured:

- **Trace**: at the foot of a ramp, with the origin 0.080 inside the
  floor (`cb` 0.099), edge distances 0.03, 0.06 and 0.09 launch at +13.01
  and 0.12 and 0.16 pop at +1.97. At 0.030 inside (`cb` 0.049), 0.03
  launches and 0.06 does not.
- **Data**: over the 1,471 first-round trials of one variant whose axis goes through a
  floor polygon on a swept tick, the bounces fill the half of the
  `de`, `cb` plane under the diagonal and none lie over it: with `cb`
  0.06 to 0.08, 43 of 50 bounce at `de` under 0.02, 32 of 45 at 0.04 to
  0.06, 9 of 37 at 0.06 to 0.08 and 0 of 29 over 0.08.

The contact depths fit it: the polygon the axis is through reports the
capsule's radius plus `de` (0.203, 0.233 and 0.263 at 0.03, 0.06 and
0.09) and its neighbour 0.199 minus `de` (0.170, 0.140, 0.110).

**The launch** is the solver's (the step at `0x00311100`), not decoded.
**Open** for the code; `rule::launch` fits it to these numbers. Measured at 5.39 in, 0.06 from the edge: drift 0.10 leaves
+0.07, 0.15 leaves +6.2, 0.20 leaves +9.4, 0.30 leaves +13.0 and 0.60
leaves +12.6. The launch barely changes with `de` or with the depth
(+12.93 to +13.01 over every run that bounced at drift 0.30).

### What each input does

| Input | Effect | Window |
|---|---|---|
| pair word | −3 or no bounce; set by a forced crouch, lost when the pair is rebuilt | exact |
| speed at the start of the bounce tick | the sweep must come on this tick, or have been on with the body low enough | 5.3075 to 5.4465 (one tick of gravity wide), or over 5.4465 with the origin under `speed / 30 − 0.165` |
| height of the origin over the floor, one tick earlier | over the contact tolerance, and low enough to end inside | 0.045 to about 0.17; a fall moves 0.18 a tick, so it is the tick phase |
| distance from the nearest edge of the polygon under the axis | `de < cb` | 0.02 to 0.16, wider the deeper the body is |
| speed across that edge | the lean of the normal | 0.1 or more at the last airborne row; under 0.15 the launch is weak |
| speed along the edge, facing, where along the edge | none found, beyond their part in the total speed | none |
| crouch | a crouch rebuilds the pair at 0, which ends the prime | must not change |

The speed windows are exact to the float: the test is `speed² >
29.6644`. The position windows are continuous, with edges a few
millimetres wide in the trials.

### Tick by tick

Each row is the state before a tick: `h` is the origin's height over the
site's point, the speed is vertical, and the contacts are the ones the
tick that follows makes (`world.raw`, `havok.agents`, `havok.contacts`
between `session.step {ticks: 1}` calls). Every run sets the word, flies
the body down a free fall to the first row by writing its velocity
(`world.poke`) and then lets go.

**`s007-slot`** (provable): a seam across a 26.6° ramp at y 10.8505,
0.15 over the ramp's foot. Asked: across 0.048, `h` 0.107, 5.225 down,
drift 0.58.

| Run | Row 0 | Row 1 | Row 2 | Contacts on the deciding tick |
|---|---|---|---|---|
| bounce | +0.107, −5.22 | −0.072, −5.36 | +0.350, **+12.67** | (0, 1, 0) 0.267 deep, 24,308; (0, −0.996, 0.093) 0.111 deep, 24,273 |
| 0.2 faster | +0.107, −5.42 | +0.104, −0.07 | +0.097, −0.21 | row 0's tick: the ramp's normal, 0.128 deep, 768 (sweep on, caught) |
| 0.2 slower | +0.107, −5.02 | −0.065, −5.16 | −0.069, −0.11 | row 1's tick: the ramp's normal, 0.116 deep, 726 (no sweep) |
| `h` 0.030 | +0.030, −5.22 | +0.006, −0.71 | −0.020, −0.79 | row 0's tick: the ramp's normal, 0.023 deep, 651 (inside the tolerance) |
| no drift | +0.107, −5.22 | −0.072, −5.36 | −0.067, +0.16 | (0, 1, 0) 0.233 deep, 133,128; (0, −1, 0) 0.137 deep, 132,874 |
| word 0 | +0.107, −5.22 | −0.072, −5.36 | +0.040, −0.67 | the ramp's normal, 0.004 over it, 476; word 0, 0, 1, 0 |

The bounce and the faster run differ in one bit on row 0's tick: the
bounce moves at 5.40 in all, whose square is under 29.6644, and the
faster run at 5.59, over it (`0x002db359`). The bounce and the slower
run differ in the same bit one tick later (5.53 against 5.33).
The bounce and the no-drift run make the same two contacts and differ
in the 0.093 lean of one normal.

**Three fragile sites**, the same six runs each:

| Site | Bounce | Faster | Slower | `h` 0.030 | No drift | Word 0 |
|---|---|---|---|---|---|---|
| `s001-slot` (0.127, 5.273, drift 0.39) | +13.61; (1, 0, 0) 0.255 and (−0.998, 0, 0.058) 0.120, 44,755 and 44,638 | caught at +0.125 | stops at −0.05 | lands | level normals, 133,547 and 132,430, +0.30 | lands, word 0, 0, 1, 0 |
| `s020-crease` (0.124, 5.176, drift 0.44) | +13.41; (−0.856, −0.513, 0.068) 0.239 and (0.857, 0.515, 0) 0.135, 36,614 and 36,628 | caught at +0.120 | stops at −0.05 | lands | level normals, 132,998 each | lands, word 0, 0, 1, 0, 1, 0 |
| `s025-vee` (0.042, 5.354, drift 0.50) | +12.27 on row 0's tick; (−0.229, −0.973, 0) 0.205 and (0.214, 0.974, 0.075) 0.166, 33,216 and 33,189 | caught a tick early | stops at −0.02 | +12.30 | level normals, 133,478 and 132,514 | +12.27; word 0 then −3 |

`s025-vee`'s rows start with the capsule already inside its sloping
floor, so the bounce tick is the first tick. Its unprimed run bounced:
the pair pass found the point too deep, `0x00316460` gave up and wrote
−3 on that tick, and the same tick's contacts pinched. A body placed
inside the floor primes itself. No trial of the search did so (0 of 928
unprimed trials bounced).

A fragile site's bounce and its near misses split at the same branch as
the provable site's.

### Why 53 sites are fragile

They are not. The sites are alike: inside the speed and height windows
above, 54 of the 57 sites with trials bounce on 49 to 80% of their
trials, and the other three have under 180 such trials each. The verdict comes from the sampling and the report.

- **The speed axis is mostly dead.** It is sampled from 5.0 to 6.6 and
  bounces only from 5.168 to 5.4465, 17% of it. First-round bounces by
  asked speed: none under 5.10, 13% at 5.1 to 5.2, 29% at 5.2 to 5.3, 5%
  at 5.3 to 5.4, 1.5% at 5.4 to 5.5 and 4 of 15,000 over that. With the
  height, edge and drift windows the first round bounces on 2.9% of its
  points (1,870 of 63,843), the 1 to 6% the table shows.
- **A real fall is always in the band.** Every fall passes through
  5.168 to 5.3075 on one tick. What a player controls is where that tick
  starts, which is the drop height. The search's `depth` axis is that,
  and its `vz` axis has no counterpart in play.
- **The window is measured in the wrong space.** The report moves each
  point to its measured last airborne row (`report.rs`, `seen` and the
  `at[...]` writes). A bounce through the floor has its last row inside
  the floor at 5.26 to 5.44. A trial the floor only slowed (still under
  −0.5) has its last row several ticks on, at 0.5 to 2 units/s. So the
  failures are carried away from the bounces along the speed axis. The
  bounces' cluster is 1.8 human steps wide in speed, so no window can
  have a radius over 0.9, and the four provable sites have 0.92, 0.91,
  0.86 and 0.73. Whether a site gets a window is whether eight good
  points happen to sit in one cube.
- **Havok state does not matter.** The three variants agree on 42,787
  of 42,819 points. The 32 that differ are on 7 sites.

The table's `position cm`, `press ticks` and `phases` come from those
windows' bounds and carry no information: `s007-slot`'s speed bounds
read 0.50 to 6.60.

### The harness oddities

**The 0.033 placement note is not a bug.** It is the engine's lift in
`PLAYER_PLACE_NEAR`, the trials ask against it, and the stepped runs here
landed within 0.0012 of the row asked for.

**"Out of step" cost no provable site.** Five site-variants of 174 lost
their base to it (`s001` fall, `s006` hop, `s053` fall, `s049` fall) or
to an early end (`s040` fall). A lost variant removes a third of a
site's trials and no information, since the variants agree. The verdict
is over points, not trials. The releases are 1, 2 and 3 ticks late,
which fits a seat that waits for up to three still rows before it arms.
The stepped runs here do not use the hold and neither confirm nor refute
that cause.

**The placement's search costs four sites their trials.** This one is
not in the notes. `PLAYER_PLACE_NEAR` moves the unit when the standing
capsule does not fit where it is asked, by exactly 1.0 along an axis or
0.765 on a diagonal. The trial fails as `teleport: landed 1.000 off`.
3,079 of the 4,165 setup failures are these:

| Site | Setup failures | Verdict |
|---|---|---|
| `s055-crease` | 1,202 of 1,246 | `none` |
| `s040-crease` | 392 of 836 | fragile, 0% |
| `s023-vee` | 447 of 1,246 | fragile, 0% |
| `s043-vee` | 332 of 1,246 | fragile, 0% |
| `s018-vee`, `s031-crease`, `s014-slot` | 209, 152, 129 | fragile, 0 to 1% |

`s055-crease` is untested, not `none`: 44 trials ran. The fix is in the
report (`bounces/report.rs`): a site whose known points are a small
share of its points is `untested`, with the placement's miss as its
reason. A second teleport against a miss of 1.0 cannot work, so
`TrialRun::step` can fail such a trial at once and not ask again.

### The predictor

The rule above, stepped a tick at a time over the site's polygons, with
no game:

```text
each tick, from the row the trial starts on:
  v' = v + gravity                      # 4.171 / 30 down
  if |v'| > 5.4465:                     # the sweep is on
    A = origin + (0, 0, 0.165); E = A + v' / 30
    for each floor polygon the segment A..E crosses from above, inside it:
      cb = E's depth under the polygon
      de = the crossing point's distance to the polygon's nearest edge
      if de < 0.9 * cb and the speed across that edge >= 0.02: BOUNCE
    if a polygon was crossed: no bounce (a pop)
    if the capsule at E is within 0.03 of a floor: no bounce (caught)
  else:
    if the capsule is within 0.03 of a floor: no bounce (lands or stops)
  origin += v' / 30; v = v' with 0.035 off its horizontal part
```

The capsule's bottom is 0.01 under the origin and its radius is 0.175.
Floor polygons are those with a normal z over 0.2, taken from `h2
superbounce scan ascension --near`, whose vertices are printed to the
centimetre. The 0.9 and the 0.03 were chosen on the first round.

Against the search's sampled trials with an outcome (**Data**):

| Trials | Bounced | Predicted and bounced | Predicted, did not | Missed | Accuracy | Always "no" |
|---|---|---|---|---|---|---|
| first round, 63,843 (unbiased) | 1,870 | 1,756 | 566 | 114 | 98.9% | 97.1% |
| all rounds, 123,823 (refinement sits on the boundary) | 19,481 | 17,976 | 4,133 | 1,505 | 95.5% | 84.3% |

Of the bounces it finds 94% (first round) and 92% (all); of its
predictions 76% and 81% bounce. Per site the first-round accuracy is
97.8% to 100%, median 99.0%. The speed and height windows alone, with no
polygons, find 95% of the bounces and are right 36% of the time: the
edge test is what the polygons add. The misses left are near `de = cb`,
where centimetre vertices are too coarse, and landings the neighbour
polygon did not answer (at `cb` 0.139 and `de` 0.12 the stepped game
made one sideways contact and no pinch).

### Reproduce

```sh
# the trials
H2_OPT=2 h2 sim bounces full.toml --out /Volumes/Builds/h2recomp/image --tier fast --parallel 5

# a game to step
H2_LEVEL=ascension H2_OPT=2 H2_TIER=fast h2 serve $H2_DISC \
  --out /Volumes/Builds/h2recomp/image --tier fast --data <a data dir> --socket <sock>

# one stepped run: teleport above the row, pause, then per tick
#   world.poke (object +0x88 and motion +0x40: the velocity),
#   havok.pair.set {"object":"player.0","word":"fffffffd"},
#   session.step {"ticks":1}, world.raw, havok.agents, havok.contacts
python3 run2.py '{"sites":{"e":[[3.0,11.0,-2.0],[0,-1]]},
  "trials":[{"site":"e","h0":0.10,"vz":5.25,"vd":0.3,"across":-0.06}]}'

# the code
h2 disasm $H2_DISC 0x317950 330     # the pair pass
h2 disasm $H2_DISC 0x2db2b0 290     # the biped agent's sweep
h2 disasm $H2_DISC 0x2dbfc0 30      # the wrapper's threshold

# the predictor and its score
python3 load.py && python3 dump_polys.py && python3 pred3.py && python3 persite.py
python3 falls.py                    # the drop table
```

The scripts (`drv.py`, `trial.py`, `run2.py`, `geo.py`, `pred3.py`,
`falls.py`), the stepped traces (`traces/*.json`) and the disassembly are
in `$RUSH_KEEP/sb-why/`. A game left running ends its Slayer round and
returns to the menu; `game.load {"scenario":"ascension"}` starts another.

### Open

- The convex agent's penetration case (`0x002eeb40`): the code that
  picks the sideways normal, why one normal of the pair leans and the
  other is level, and when the neighbour polygon makes no contact.
- The solver's launch: why two impulses of 24,000 to 134,000 leave 18.5
  to 22 units/s.
- The values at `world[+0x244]` and `[+0x248]`, and what `0x00316460`
  does to the body before it gives up.
- Where 5.4465 comes from, and whether other bipeds have another value.
- Only Ascension and `masterchief_mp` were measured. The fast-fall
  rows are six stepped runs at one edge.
- 60 frames a second: the threshold is a speed and the sweep is one
  tick long, so a 60 Hz tick would change every window here. Not
  measured.
