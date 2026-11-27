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

## Traces (Trace)

`world.object` carries a `physics` block for units (crouch, crouch
request, mode, airborne bit and ticks, raw flag words;
[agent-api.md](../runtime/agent-api.md)). Polling `object player.0` on
one `h2 agent -` connection gives one reply per tick, stamped with the
tick.

Ascension, `masterchief_mp`, a patch of floor around `(-10.5, 3.9)`,
z −3.19 standing. Falls onto it were all at −5.3 units/s (the same jump
height), with three outcomes over 20 landings:

| Outcome | Count | What the trace shows |
|---|---|---|
| landing | 8 | stops at the floor, airborne bit clears |
| caught | 7 | stops 5 to 15 cm above the floor, still airborne |
| bounce | 5 | sinks 7 to 14 cm below the floor, then launched |

The five bounces:

| Tick | y in (units/s) | z below floor | z velocity in | z velocity out |
|---|---|---|---|---|
| 6723 | 0.27 | 0.070 | −5.40 | 13.22 |
| 13232 | 0.77 | 0.136 | −5.26 | 11.17 |
| 11274 | 0.89 | 0.114 | −5.26 | 10.39 |
| 11770 | 1.25 | 0.119 | −5.26 | 8.01 |
| 12008 | 1.29 | 0.122 | −5.26 | 7.97 |

- The launch is not a multiple of the fall speed (constant here) and does
  not follow the sink depth.
- It falls with the speed toward +y going in: about 14.9 − 5.3 × that
  speed, r = −0.99 over five points. A fit, not the engine's rule.
- Every bounce turns the y velocity negative (−0.4 to −1.0): the push
  has a small −y component.
- Crouch was 0 with no request on every bounce tick.

A third trace (fresh game, tick 2928) adds a sixth bounce: −5.40 in,
0.084 below the floor, +11.89 out, 359 ticks after a forced crouch that
had ended on the ground at tick 2569. Unit flags were back to
`00000003` and the hitbox offset 0 from then on. Two melee swings with
no target did not lunge (mode stayed 1, offset 0).

### Zanzibar's window (Trace, lab play)

The user played the community's window bounce on Zanzibar on the lab
(job 0043, input recording `zanzibar-window`), read about every 12 ticks
through `superbounce.state` and `world.object`
([the note](notes/superbounce-spots.md#2026-11-21-zanzibars-window-played)).
The recipe: in the base facing the beach, the rear right inner window,
break the glass, crouch under the shard left of the gap, let go, turn
right, jump, fall onto the crack below.

- **The prime.** The player crouched at (5.22, −26.285) on the window
  sill (z 5.10). The lip that forces the crouch is a piece of the broken
  glass. The phase went `under`, then `held`, and the word turned −3 on
  the jump, while still forced crouched: not on a walk out.
- **The glass.** The pane is in the structure BSP: `--near` lists
  two-sided vertical polygons in the plane x 5.30 filling both openings
  of the window (y −24.5 to −25.5 and −26.1 to −27.1, z 5.10 to 6.30),
  cut into irregular pieces. The map has the breakable-surface
  definition `globals\glass` (`bsdt`) and its effects, and no scenery or
  crate named for glass or a window. So the pieces are breakable surfaces
  (flag bit 3, a breakable index each), skipped in collision once broken
  ([collision.md](../formats/collision.md#what-the-engine-does-with-a-bsp)).
  **[guess]** until `world.breakable` reads it in a game: the pieces' flags
  and indices are not read offline. The scan takes every surface as solid,
  so it sees the whole pane as a wall and no lip; the lip is a piece left
  above the gap after the break. The bots break the pieces in their walk
  in first and leave the one above ([bots.md](bots.md#breaking-the-glass)).
  The way out of the `PROVEN` row is back into the room (+x), so the
  prime starts on the room's floor; the jump went out of the window,
  (−0.69, −0.72).
- **The landing.** The floor below is z 2.10 meeting a 17.6° ramp
  (normal (0, 0.3033, 0.9529)) along y −26.70, rising south to z 2.40.
  That crack is one concave crease, not two joints, so it is no
  [slot](#bounce). The first try fell from z 5.5 at −4.5 units/s near
  (4.73, −26.15) and made a small pop. The second fell straight onto
  (4.876, −26.748), 5 cm on the ramp side of the crease, and went
  through the floor (z 2.86, then 0.44, then the player died).
- **A second prime.** The same session crouched at (8.07, −3.01), z
  4.51, three times. Twice the word turned −3 while walking out
  (ticks about 8071 and 8655), within 0.5 of the rest point; the first
  time it did not.

Both prime places are rows in `superbounce::PROVEN`, and the crease is
what [single creases](#single-creases) look for.

### The contacts on a bounce tick (Trace)

A fourth trace recorded `trace.start player.0` (every tick,
taken inside the game) next to `havok.contacts {object: player.0}`. It
bounced at tick 5846: −5.26 in, 12.5 cm below the floor, +9.39 out,
standing, 315 ticks after a forced crouch had ended. Havok reported
contacts between the player's body and the level (the fixed body) on
that tick only, two points:

| Point (x, y, z) | Normal | Distance | Impulse |
|---|---|---|---|
| (−10.624, 4.117, −3.200) | (0, −1, 0) | −0.253 | 9317 |
| (−10.623, 3.941, −3.211) | (0, 0.983, 0.183) | −0.140 | 9250 |

- Both pushes are nearly horizontal and face each other: the capsule,
  once below the floor plane, is pinched between a face looking −y (at
  the seam, y 4.12) and one looking +y tilted 10.6° up (y 3.94). It is
  in 25 and 14 cm deep.
- The solver answers with two almost equal impulses. Their y parts
  nearly cancel (−224); the tilt leaves 1697 up.
- The body's mass is 125 (`havok.bodies`, inverse mass 0.008), so the
  sum is a velocity change of (0, −1.8, +13.6). The trace shows
  (0, −2.1, +14.8) with gravity's tick taken out: the same direction,
  the reported impulses about 10% short (perhaps the last solver
  iteration's share, not the tick's whole push). **Open**
- So the launch is not a push off the floor. It is the vertical part of
  two large opposing pushes out of a pinch, and it grows with how deep
  the capsule gets in. That fits the fall-speed fit above: more speed
  toward +y drives the capsule into the −y face first and changes the
  split.
- Standing on the floor gives no contacts at all (843 of about 3900
  replies had any, all in the air near geometry); the forced crouch
  (ticks 4432 to 4792, 5453 to 5531) changed nothing: the Havok body's
  position equals the drawn one on every tick, crouched or not, by
  construction (the object's position is copied from the body).

### Looking for state a forced crouch leaves behind (Trace)

Players report that a bounce only happens after a forced crouch some time
before the jump (not during it). A fifth trace looked for
such a state with `world.raw` (the player object, its physics component
record, Havok body, motion and shape as bytes) before, during and after
a forced crouch, and every tick while the player jumped at the patch:

- The biped's Havok shape (vtable `0x004122f0`, radius 0.175 at `+0x0c`)
  is not rebuilt by a crouch: its pointer at `+0x30` moves from the
  standing pill (`0x80393700` in that run) to the crouching pill (`+0x50`)
  and back. After the crouch it is the standing pill again, and the
  shape's bytes equal the ones before.
- In the player object, the only word changed by the crouch and kept
  after it is `+0x2c` (with `+0x28`), which moves with the object's
  location in the map, not with crouching.
- 15 landings at the patch, each after a forced crouch: 1 bounce (9.2 cm
  in, +2.6 out), 6 caught, 8 plain landings, two of them 13.6 and 14 cm
  in. No body, physics component or shape word differed on the tick
  before the bounce from the ticks before the others.
- 5 landings after a respawn with no crouch, at the same fall speed:
  none bounced (3 landings, 2 caught). Too few to compare.

So far the state, if there is one, is not in the object, the body or the
shape. The remaining candidate is Havok's collision agents: the per-pair
caches between the body and the level (closest features, welding), which
a shape swap mid-contact could leave stale. **Open**

### The forced crouch leaves state (Trace, lab playback)

Measured 2026-11-21 on a second Mac through the lab (`h2 lab run --serve
--level ascension -- --fps 30`, [lab.md](../runtime/lab.md)), driven by
`h2 lab agent`. The lab runs at 30 frames a second. The player reported
that bounces don't happen at 60, and every run here was at 30.

**The recipe**, from two recordings of the player (`input.record` and
`trace.start` together):

1. Forced crouch. Stand at (−10.85, 1.16, −1.45) facing yaw −121°, click
   crouch (left stick) and crouch-walk at full stick into the corner at
   (−10.91, 0.72, −1.16), turning slightly toward it. Release crouch
   while still pushing. The crouch request stays set with the button up
   (the standing capsule doesn't fit) for as long as you stay. Turn about
   67° to yaw −66° while pushing forward, and the player slides out and
   stands up in six ticks.
2. Walk to the upper level, about (−8.2, 2.4, −0.72), by way of
   (−10.2, −0.3), (−9.5, −0.5), (−8.5, −0.2), (−8.0, 0.8), (−7.8, 1.5).
3. A running jump from about (−8.4, 2.5, −0.76), heading (−0.8, 0.6) at
   about 1.6 units/s, easing off the stick in the air.
4. It lands on (−10.2 to −10.7, 3.93 to 4.04, −3.19) at −5.4 units/s,
   the fall speed after that drop. All the player's bounces here landed
   like this. In the player's control run, 7 of 44 such jumps bounced.

A stand-in for each step without the player's own input never worked.
Crouching anywhere but in the corner doesn't catch the request. Scripted
stick timings walk past it. A standing jump at the patch lands at −3 and
is never a test.

**Playback.** `input.pad` steps rebuilt tick for tick from the recorded
input log (one step per tick read), from a `sim.state.set` teleport to
where the recording starts. A playback is repeatable: two runs of the
same steps gave the same launch to 0.01 units/s.

| Run | Start | Bounced | Launch |
|---|---|---|---|
| A, C | teleport before both forced crouches, then the recorded crouches, walk and jump | 4 of 4 | +11.4 to +12.0 |
| B, D | teleport to where the player stood after the second crouch, then the recorded walk and jump | 0 of 6 | - |

Four of the six B and D runs landed at (−10.26, 4.02, −3.18) at −5.3 to
−5.4, the place and speed of the bounces. The B and D runs came right
after runs that had crouched, so each body had crouched and was then
teleported. The teleport is `object_teleport`'s body (`0x29ffb0`), which
places the unit through `0x14e200`.

Also measured:

- **The animation counter at biped `+0x3a8` is not the state.** The
  biped physics update (`0x000e2678`) counts ticks in which the current
  animation is idle (`0x0400000C`) while the wanted group (`+0x34a`) is 1
  (moving or airborne). The count resets to 0 on the first idle tick
  after the animation was anything else (`+0x3a9`, set from `0x000dd300`
  each tick). While count × tick length is under 0.1 s (`0x45DC68`) it
  forces the switch to the wanted state. In play it runs between 0 and
  about 30 all the time, crouched or not. Writing 0 to it (`world.poke`)
  just before the jump of run C didn't change the launch (+11.52, the
  same as A).
- **A pause at an unpinned point can move the path.** One
  `session.pause` placed by position in a script of reads took the next
  landing off the bounce. A pause at a tick pinned with `session.wait
  {until: {tick}}` didn't move the landing. Pin reads and writes to a tick
  (`input.pad {at_tick}` for the steps, `session.wait` for the read).
- `state.save` is refused on this build ("not quiescent", seven features
  not yet audited for snapshots), so every run starts from a teleport.

### The pair word (Trace, lab playback)

Measured 2026-11-21 through the lab on a build with `havok.pair.set`
([agent-api.md](../runtime/agent-api-world.md#havokpairset)), same recording and
playback as above. Scripts and results are in the lab session's keep
directory, not in the repository.

**Two readings at one tick.** `world.raw`, `havok.contacts` and
`havok.agents`, taken at the same tick of the recording in a primed
playback and in a teleported one, both with the player standing still at
(−7.99, 2.49). The primed body bounced afterward and the teleported one
didn't. Positions agreed to 0.05. Two differences stand out:

- The body's rotation (motion `+0x30`, and the matrix at `+0x80`) is
  identity in the primed body and a yaw in the teleported one: the
  teleport turns the body, play doesn't. Setting it to identity after the
  teleport didn't make a bounce (0 of 2).
- `havok.agents` lists one pair against the level for the player. Its
  `pair_word` is `fffffffd` (−3) in the primed body and `00000000` in the
  teleported one. A negative word puts the pair in the mode that sets its
  input threshold to −1000 instead of `world[+0x244] + word ×
  world[+0x248]` ([havok-runtime.md](../engine/havok-runtime.md)).

Copying every other differing word of the object, physics component,
entity, motion and shape onto the teleported body at that tick (697
words, pointers and handles left out) didn't make a bounce (0 of 4).

**The word through a playback** (`havok.agents` every tick):

| Recording tick | Crouch | Word |
|---|---|---|
| start | 0 | 0 |
| 6127, first forced crouch | 1.0 | the pair is gone for two ticks, then back at 0 |
| 6159, standing up in the corner | 0.83 | −3 |
| 6441 to 6579, second forced crouch | 1.0, then 0.83 | gone, 0, then −3 again |
| 6579 to 6754, walk, run-up, jump | 0 | −3 |
| 6754, the bounce, +11.5 | 0 | −3 |

In a teleported playback the word is 0 to the landing, then 1 for a tick
on touchdown, then 0. No bounce.

**Setting it.** A teleport to where the player stood still before the
jump (recording tick 6684), then the recorded jump from there:

| Word set to | Bounced | Launch |
|---|---|---|
| none (as teleported, 0) | 0 of 6 | - |
| `00000000` (a write of the same value) | 0 of 1 | - |
| `fffffffd` (−3), held to the landing | 3 of 3 | +12.31 to +12.45 |
| `fffffffd`, gone by the end of the run | 0 of 1 | - |

All of them landed at (−10.25, 4.04, −3.25) at −5.4 units/s, where the
player's own bounce launched at +12.4. A teleport keeps the pair and its
word (a word written in one run was still there after the next run's
teleport), and keeps the body's velocity. A body that stands still for a
while has no level pair (it sleeps), and the pair it gets on waking
starts at 0. Those runs had nothing to write.

**Where −3 comes from** (Code, [havok-runtime.md](../engine/havok-runtime.md)).
The word is the pair's tolerance level in the pair pass `FUN_00317950`.
At 0 or more it widens when contact points fall outside the tolerance and
narrows when they fit. −3 is the give-up value. `FUN_00316460` writes it
when a point is still outside after widening (`0x00316755`,
`0x00316958`), and from then on the pass runs that pair's agent at a
tolerance of −1000. Nothing re-arms −3; only −2 goes back to 0. A capsule
standing up into the corner's ceiling is deep in the geometry, which fits
the give-up path. **Code** for the paths; **Trace** that it happens on
standing up.

### The touchdown tick (Trace, stepped)

Measured 2026-11-21 through the lab (job 0025, branch `sb-lab2`: the
bounded pipeline prewarm `241622789` with `havok.pair.set`), with the
game paused and advanced one tick at a time (`session.pause`, then
`session.step {ticks: 1}` per tick, with `havok.pair.set`, `world.object`,
`havok.agents` and `havok.contacts` between ticks). Each run teleports to
where the player stood at recording tick 6684, waits for the body to
stand, steps 30 ticks, sets the word to 0, queues the recorded input from
6684 and steps through the jump. Pad steps advance with ticks, so the
path is the same from run to run. Ticks below are recording ticks; the
capsule first meets the floor on 6753.

| Word | Bounced |
|---|---|
| 0 throughout | 0 of 2 |
| −3 from 6684 | 1 of 1 |
| −3 written before tick 6713, 6733, 6748, 6750 or 6752 | 5 of 5 |
| −3 from 6684, 0 written before 6750, 6751, 6752 or 6753 | 0 of 4 |
| −3 from 6684, 0 written before 6754 | 1 of 1 |

Runs whose body had no level pair to write are left out (four of 17).
The word decides the bounce on tick 6753 alone. On that tick, in the two
kinds of run, with everything before it the same (z −3.253, vertical
speed −5.4):

| | Word after the tick | Contacts with the level after the tick | Vertical speed |
|---|---|---|---|
| word 0 | 1 (widened) | one, normal (0, 0, 1), 0.027 above the floor, impulse 509 | −0.04 |
| word −3 | −3 | two, normal (0, −1, 0) at y 4.105, 0.231 deep, impulse 77,576; normal (0, 0.9993, 0.0372) at y 3.939, 0.141 deep, impulse 77,566 | +12.45 |

At 0 the pass widens the pair's tolerance and catches the floor before
the capsule reaches it. At −3 it makes no point with the floor, the
capsule ends the tick inside the slot between two walls 17 cm apart, and
the solver pushes it out of both at once. The sideways parts cancel. The
second wall leans 2.1° up, and the upward part of the two impulses is
2,886, which over the body's mass (125, `havok.bodies`) is 23 units/s
against the 17.85 the body gained. How the solver's reported impulse
turns into that velocity, and so where the other 5 units/s go, isn't
read. The scripts and replies are in the lab session's keep directory.

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
