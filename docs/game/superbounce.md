# Superbounce

Status: Reference, retail research with traces, except [Spots in the
geometry](#spots-in-the-geometry), which is the design of our scanner
(`crates/live/superbounce`, `h2 superbounce scan`), and [Medals](#medals),
whose rule, HUD line and agent API are built and not yet run in a game,
and whose recording in the film and Live, and the site's medals, are
built, [Ghost runs](#ghost-runs) and [The bounce view](#the-bounce-view),
built and not yet run in a game.

What the retail engine does when a player superbounces: what is
established, what is measured, and what is still open. The research is a
static reading of `default.xbe` (`h2 disasm`, no game run), then per-tick
traces of a player on Ascension through the agent API.

Evidence labels: **Code** (read from the disassembly at the address
given), **Trace** (measured in a live game, numbers below), **Open** (not
established).

## Summary

- A superbounce is a landing on a seam in the structure collision. The
  player's capsule ends up 7 to 14 cm below the floor, and the next tick
  its vertical velocity is +8 to +13 units/s instead of zero. **Trace**
- The push is velocity: the position change on the bounce tick is the new
  velocity times one tick, every time. **Trace**
- The bipeds' character physics has no capsule-against-world response of
  its own. The response comes from the Havok step (`0x001c3d30`). The
  launch speed is the contact solver's ([the launch](#the-launch-code-trace)):
  eight passes of a coupled two-row solve over the two faces of the pinch.
  **Code**, **Trace**
- On the bounce tick the body is pinched below the floor between two
  faces looking at each other, one tilted 10.6° up; the solver's two
  opposing impulses cancel sideways and leave about 14 units/s up
  ([contacts](#the-contacts-on-a-bounce-tick-trace)). **Trace**
- Forced crouch (staying crouched after releasing the button under a low
  ceiling) is real. So is a crouch term that moves a mid-air biped by its
  height change. Neither was involved in any recorded bounce: all five
  were standing, 3.5 to 28 seconds after the last crouch. **Code**, **Trace**
- A forced crouch leaves the player-vs-level collision pair's word (pair
  `+0x0c`) at −3, and that word is what makes the landing bounce.
  Standing up out of the forced crouch takes it from 0 to −3, and it
  stays there until the pair is rebuilt at 0 (a crouch, or the body going
  to sleep). Played back with the player's recorded input from before the
  crouches, the landing bounced 4 times in 4. From a teleport after them,
  the same landing at the same place and speed never bounced (0 of 7).
  Setting the word to −3 on a teleported body (`havok.pair.set`) made it
  bounce every time the word held to the landing, at +12.3 to +12.45, the
  player's own launch
  ([playback](#the-forced-crouch-leaves-state-trace-lab-playback),
  [pair word](#the-pair-word-trace-lab-playback)). **Trace**
- A primed landing bounces when the sweep of the biped's collision agent
  is on, the swept capsule's axis goes through a floor polygon close to
  one of its edges, and the body moves across that edge
  ([the bounce rule](#the-bounce-rule-code-trace)). **Code**, **Trace**
- Only the touchdown tick needs the word at −3. At 0 the pair pass widens
  its tolerance on that tick and catches the floor 2.7 cm above it. At −3
  it makes no contact, the capsule drops 14 to 23 cm into the two facing
  walls of the slot at the seam, and the solver's two opposing pushes of
  about 77,600 leave the upward part of the tilted face's normal as the
  launch ([touchdown tick](#the-touchdown-tick-trace-stepped)). **Trace**

## The bounce rule (Code, Trace)

What has to be true on the tick of contact, from the pair pass, the
biped's collision agent and stepped runs
([notes/2026-11-23-why-bounces-work.md](notes/2026-11-23-why-bounces-work.md)):

1. **The pair word is −3.** The pair pass (`0x00317950`) is the
   narrowphase. At a word of 0 or more it checks every contact point
   against the pair's allowed penetration and puts a body that is too
   deep back on the floor (`0x00316460`). At a negative word the
   allowance is −1000 and nothing is checked.
2. **The sweep is on.** The biped's agent (`0x002db2b0`) stretches the
   capsule along the tick's motion when the body's speed, with this
   tick's gravity, is over 5.4465 units/s (the wrapper shape's `+0x20`,
   29.6644, against the speed squared).
3. **The swept capsule's axis goes through a floor polygon nearer one
   of its edges than the axis reaches under it.** The contact normal is
   then sideways, out through that edge, and the polygon across the edge
   pushes back: the pinch of
   [the touchdown tick](#the-touchdown-tick-trace-stepped). Further from
   an edge the normal is straight up and the landing pops at +2.
4. **The body moves across the edge**, at 0.1 units/s or more. The
   normal on the side it moves toward leans up by the drift over the
   fall speed, and the lean is the launch.

A fall gains 0.139 units/s a tick, so the sweep comes on during exactly
one tick: the one that starts at 5.3075 to 5.4465. The tick before, the
capsule is tested where it stands and a primed body goes up to 0.14 into
the floor untouched. So near the threshold a bounce needs that earlier
tick to start with the origin 0.045 to 0.17 over the floor, which is the
drop: 3.35 to 3.60 from the apex, and at 3.45 about three phases in
four. A faster fall has the sweep on all the way down and bounces from
above when the origin starts the tick under `speed / 30 − 0.165` over
the floor: rarely under 6 units/s, a quarter of the phases at 10.

Any edge between two floor polygons does it, a coplanar seam as much as
a crease. The 57 Ascension sites of the search bounce alike. The rule,
stepped over a site's polygons with no game, agrees with 98.9% of the
search's 63,843 first-round trials.

`superbounce::rule::predict` is that stepped rule in Rust. Starting at the
body's origin and velocity, it adds gravity and the held air stick each
tick, then classifies the deciding contact as `bounce`, `bounce_fast`,
`caught`, `stopped`, `pop`, `level_pinch`, `landing` or `no_floor`. Its
`Verdict` carries the tick, row, speed and, for a crossing, the polygon,
edge distance, depth and speed across the edge. `rule::edge_under` finds
the nearest edge of the floor under a point. A drift of 0.15 units/s is
the full-launch minimum (`DRIFT_FULL`), and `DROP_MIN`, `DROP_BEST` and
`DROP_MAX` are 3.35, 3.45 and 3.60. The HUD, the bots' takeoff gate and
the search use these same constants and the same verdict.
`rule::launch(speed, across)` (and `Verdict::launch`) is the launch the
bounce gives, fitted to the traces; `rule::pinch` is the decoded solver
behind it ([the launch](#the-launch-code-trace)).

## The launch (Code, Trace)

The island solve (`0x0031c100`, [havok-step.md](../engine/havok-step.md#the-island-solve),
ported in `sim-havok`) turns the two contact points of the pinch into the
launch. Read from the image (`0x003101c0`, `0x001c2d40`, `0x0030fcb0`,
`0x0031f620`, `0x0031c940`) and checked against the traced impulses:

- **Constants.** 8 passes (info `+0x4c`); row target scale 0.6
  (`0x0044ae8c`); velocity scale 1; recovery per metre 8 × 30 = 240
  (`pair+0x0c`); recovery share 0.05 (`0x004155a4`), capped at one step
  (1/30); softness 1.19e-7 (`0x0041559c`); pair coupling 0.99
  (`W+0x178`). The biped's inverse mass is 0.008.
- **A fresh point's target.** The add-point method (`0x00318c10`) stores
  the stored recovery as the separation plus a velocity term, so the
  builder's error is the velocity term less 5% of the penetration: the
  row's target speed is `0.6 × 240 × 0.05 × |separation|` (about 7.2 per
  unit of depth) and does not recover the whole depth.
- **The two points are one pair.** A point flagged as paired couples with
  the one before it into a 2×2 block `[t, m; m, t]`, `t` the inverse
  mass plus softness, `m` the inverse mass times the normals' dot times
  0.99. Each pass adds `K⁻¹(target − v·n)` when both parts are positive,
  else the one row that is. A pass ends by taking the speed along each
  normal off its target; the step's velocity is the mean of the eight
  passes' velocities.
- **Why the impulses are huge and equal.** Two faces that nearly oppose
  make `K` nearly singular: `det = t²(1 − 0.99²cos²)`. With level normals
  the impulses are 133,000 each (traced 133,128 and 132,874); with the
  0.093 lean of `s007-slot` they are 26,100 and 26,200 (traced 24,308
  and 24,273). The 0.99 leaves the first pass short of the push the
  normals would cancel, and what is left over is along `z`: that is the
  launch, 12.60 stepped against +12.67 traced.
- **Why drift and not the edge.** Only the second normal's lean (drift
  over fall speed) and the two depths enter, and the depths sum to a near
  constant (radius 0.175 plus `de`, 0.199 less `de`), so the launch is
  flat in `de` (+13.11 at 0.03 and 0.12) and rises with drift as
  traced: +6.5, +10.1, +12.9 and +12.0 at drifts 0.15, 0.20, 0.30 and
  0.60 (traced +6.2, +9.4, +13.0, +12.6).

`rule::pinch` runs the solve; `rule::pinch_at(speed, across, de)` builds
the pinch from a fall. Tests: `pinch_matches_the_traced_impulses`,
`pinch_launch_follows_the_drift`.

**Not decoded.** The sideways normal itself, from the convex agent's
penetration case (`0x002eeb40`, [havok-convex.md](../engine/havok-convex.md)):
the model takes the second normal square to the velocity. The velocity
term of a fresh point (restitution, `0x00318e09`) is taken as zero. The
model disagrees with the faster falls, where the two depths are not
known: it gives +7.5 at 9 units/s and +0.1 at 13 against traced +10.68
and +8.75, so `rule::launch` keeps the fitted push for those.

## Forced crouch (Code)

The crouch request is unit flags (`unit+0x134`) bit 23. The biped physics
update (`0x000e24f0`) maintains it at `0x000e28f4`:

- pressing crouch sets it (`0x000e291b`);
- releasing crouch clears it (`0x000e2928`) only when the character
  physics output says the standing capsule fits;
- otherwise it stays set with the button up: forced crouch.

The "standing capsule fits" bit is output flags bit 1 of the character
physics update (`0x001e55d0`). After the mode handler runs it builds a
standing-height capsule at the biped (`0x002de0a0`) and tests it against
the world (`0x001c5210`, `0x001e59cd`); no hit sets the bit. This runs
every tick, airborne or not.

The crouch amount (`unit+0x2c4`) moves toward the request at the unit
definition's rate (`+0x250`, 5/s for `masterchief_mp`) in
`0x000dd7d0`, and is stored after physics in `0x000dd990`.

## The crouch height term (Code)

The capsule's bottom stays at the biped's origin. Crouching shortens it
from the top (`0x000df66e`: height = standing + (crouching − standing) ×
crouch − 2 × radius). A crouch change in the air would therefore move the
head, not the feet. To keep the head still, two places apply the height
change as a vertical speed, `(standing − crouching) × ticks per second ×
crouch change` (`0x000dd8e0`):

- before physics, `0x000e2a06` adds it to this tick's vertical movement
  (crouching in the air lifts the feet; this is the crouch-jump);
- after physics, `0x000ddaa3` subtracts it from the velocity read back
  from Havok.

Both apply only to a biped airborne for a few ticks (`0x000e4050`: biped
flags `+0x348` bit 0, `+0x399` airborne ticks over a threshold, ground
mode) or in a melee lunge (`0x000e0dc0`). For `masterchief_mp` (standing
0.725, crouching 0.5) it is about 1.1 units/s per tick of change and 6.75
over a full stand-up. In the traces it was zero on every bounce tick.

## The melee-lunge hitbox offset (Code)

The Havok body of a biped is not always at its origin. When the game
places it (`0x001d401a`), for a biped it subtracts `0x000e0d70`'s
offset from the matrix's z: biped `+0x3aa` (a signed tick counter) ×
tick length × 0.15 (`0x004407b8`).

- A melee lunge started while not crouching (`0x000e2941`) sets biped
  flags bits 13 and 14, shrinks the capsule to crouch height
  (`0x001c3770`) and sets the counter to minus (standing − crouching) /
  (tick length × 0.15): −44 ticks, a 0.22 offset, for `masterchief_mp`.
- Outside the lunge it steps one tick toward 0 per tick (`0x000e2870`),
  so the body slides back to the origin over about 1.5 s.
- When the lunge ends (`0x000e0c70`), a standing capsule is tested at the
  offset place (`0x001d5120`). If it does not fit, the crouch request is
  set instead: a second way into forced crouch.

In both traces the physics mode was 1 (ground) on every tick and bits 13
and 14 were never set: no lunge, so the offset was 0 on every bounce.
`world.object`'s `physics.hitbox_offset_ticks` and `hitbox_offset` show
it directly.

## Where the sections live

| Page | Covers |
|---|---|
| [superbounce-traces.md](superbounce-traces.md) | the traces behind the rules |

Each heading below is cited from code and moved to the page named after it.

## Traces (Trace)

Moved to [superbounce-traces.md](superbounce-traces.md).

### Zanzibar's window (Trace, lab play)

Moved to [superbounce-traces.md](superbounce-traces.md).

### The contacts on a bounce tick (Trace)

Moved to [superbounce-traces.md](superbounce-traces.md).

### Looking for state a forced crouch leaves behind (Trace)

Moved to [superbounce-traces.md](superbounce-traces.md).

### The forced crouch leaves state (Trace, lab playback)

Moved to [superbounce-traces.md](superbounce-traces.md).

### The pair word (Trace, lab playback)

Moved to [superbounce-traces.md](superbounce-traces.md).

### The touchdown tick (Trace, stepped)

Moved to [superbounce-traces.md](superbounce-traces.md).

## Pages

| Page | What |
|---|---|
| [Superbounce: the geometry and the spots](superbounce-spots.md) | Where bounces and crouch spots are in the map geometry. |
| [Superbounce: the game type](superbounce-game-type.md) | The superbounce game type: targets and the second bounce. |
| [Superbounce: the guide, HUD options and fall assist](superbounce-guide.md) | The approach guide, the HUD options, the hold-X fall assist, the flight model and what is open. |
| [Superbounce: charge, landings and verifying spots](superbounce-landings.md) | Charge, crouch spots, landings, floor marks, lock-on and how spots are verified. |
| [Superbounce: ghost runs, medals and the bounce view](superbounce-ghosts-medals.md) | Ghost runs, the medals, recording and the bounce view. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## The geometry (Code, from the map)

See [Superbounce: the geometry and the spots](superbounce-spots.md#the-geometry-code-from-the-map).

## Spots in the geometry

See [Superbounce: the geometry and the spots](superbounce-spots.md#spots-in-the-geometry).

### Bounce

See [Superbounce: the geometry and the spots](superbounce-spots.md#bounce).

#### Single creases

See [Superbounce: the geometry and the spots](superbounce-spots.md#single-creases).

#### Busy points

See [Superbounce: the geometry and the spots](superbounce-spots.md#busy-points).

#### The target: the collision edge

See [Superbounce: the geometry and the spots](superbounce-spots.md#the-target-the-collision-edge).

## The superbounce game type

See [Superbounce: the game type](superbounce-game-type.md#the-superbounce-game-type).

### Targets

See [Superbounce: the game type](superbounce-game-type.md#targets).

#### Seam lines

See [Superbounce: the game type](superbounce-game-type.md#seam-lines).

### The approach guide

See [Superbounce: the guide, HUD options and fall assist](superbounce-guide.md#the-approach-guide).

#### The takeoff

See [Superbounce: the guide, HUD options and fall assist](superbounce-guide.md#the-takeoff).

#### The jump cue

See [Superbounce: the guide, HUD options and fall assist](superbounce-guide.md#the-jump-cue).

### The HUD's options

See [Superbounce: the guide, HUD options and fall assist](superbounce-guide.md#the-huds-options).

### Hold X: the fall assist

See [Superbounce: the guide, HUD options and fall assist](superbounce-guide.md#hold-x-the-fall-assist).

## Open

See [Superbounce: the guide, HUD options and fall assist](superbounce-guide.md#open).

## Charge, crouch spots and landings

See [Superbounce: charge, landings and verifying spots](superbounce-landings.md#charge-crouch-spots-and-landings).

### Crouch spots: the stricter rule

See [Superbounce: charge, landings and verifying spots](superbounce-landings.md#crouch-spots-the-stricter-rule).

### Crouch spots in the world

See [Superbounce: charge, landings and verifying spots](superbounce-landings.md#crouch-spots-in-the-world).

### Floor marks

See [Superbounce: charge, landings and verifying spots](superbounce-landings.md#floor-marks).

### Lock-on

See [Superbounce: charge, landings and verifying spots](superbounce-landings.md#lock-on).

## Verifying spots

See [Superbounce: charge, landings and verifying spots](superbounce-landings.md#verifying-spots).

## Ghost runs

See [Superbounce: ghost runs, medals and the bounce view](superbounce-ghosts-medals.md#ghost-runs).

## Medals

See [Superbounce: ghost runs, medals and the bounce view](superbounce-ghosts-medals.md#medals).

### The rule

See [Superbounce: ghost runs, medals and the bounce view](superbounce-ghosts-medals.md#the-rule).

### Recording

See [Superbounce: ghost runs, medals and the bounce view](superbounce-ghosts-medals.md#recording).

## The bounce view

See [Superbounce: ghost runs, medals and the bounce view](superbounce-ghosts-medals.md#the-bounce-view).

#### In the air

See [superbounce-game-type](superbounce-game-type.md#in-the-air).

#### Monster bounces

See [superbounce-spots](superbounce-spots.md#monster-bounces).
