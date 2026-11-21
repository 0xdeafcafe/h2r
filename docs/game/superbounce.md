# Superbounce

Status: Reference, retail research with traces, except [Spots in the
geometry](#spots-in-the-geometry), which is the design of our scanner
(`crates/live/superbounce`, `h2 superbounce scan`).

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
  exact rule that sets the launch speed is in Havok's contact solver and
  is not decoded. **Code** for where; **Open** for the rule.
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
- Only the touchdown tick needs the word at −3. At 0 the pair pass widens
  its tolerance on that tick and catches the floor 2.7 cm above it. At −3
  it makes no contact, the capsule drops 14 to 23 cm into the two facing
  walls of the slot at the seam, and the solver's two opposing pushes of
  about 77,600 leave the upward part of the tilted face's normal as the
  launch ([touchdown tick](#the-touchdown-tick-trace-stepped)). **Trace**

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
| B, D | teleport to where the player stood after the second crouch, then the recorded walk and jump | 0 of 6 | — |

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
([agent-api.md](../runtime/agent-api.md#havokpairset)), same recording and
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
| none (as teleported, 0) | 0 of 6 | — |
| `00000000` (a write of the same value) | 0 of 1 | — |
| `fffffffd` (−3), held to the landing | 3 of 3 | +12.31 to +12.45 |
| `fffffffd`, gone by the end of the run | 0 of 1 | — |

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

## The geometry (Code, from the map)

The structure collision BSP of `ascension` at the patch (surface indices
in BSP 0):

- floor at z −3.2 in two coplanar pieces: a strip from y 3.8 to 4.0
  (surface 5383) and the main floor from y 4.0 (surface 5222);
- at y 3.8 a ramp (surfaces 5219 to 5221, normal `(0, 0.54, 0.84)`)
  rises toward −y, so the floor and the ramp meet in a concave crease;
- the ramp is a thin slab: its underside (surfaces 4431, 4432, 4445) is
  about 0.3 below it, and a vertical face (surface 4227) runs down from
  the floor at y 3.8.

The capsule radius is 0.175. At the bounce positions (y 3.86 to 4.01) the
capsule's bottom touches both the crease at y 3.8 and the coplanar seam
at y 4.0. The edges are shared: there is no crack. The "caught" landings
rest on the ramp side of the crease.

## Spots in the geometry

`crates/live/superbounce` looks for places like the two measured
Ascension spots in a map's collision: strips a primed landing could
launch out of, and places that force a crouch and so prime the pair
word. It is a heuristic over the geometry, calibrated on those two
spots, not a model of the solver. A spot it finds is a place to try,
not a measured bounce; only a game run makes it an entry in
`crates/live/bots/src/glitches/spots.rs`.

```sh
h2 superbounce scan ascension             # BSP 0 of the disc map, best first
h2 superbounce scan path/to/map.map --bsp 1 --json
h2 superbounce scan ascension --near -10.62,4.0,-3.4   # the polygons near a point
h2 superbounce scan ascension --explain -10.91,0.72,-1.16  # crouch regions near a point, judged
```

The input is every surface of the structure collision BSP and of each
instanced geometry placement's collision BSP, in world space
(`superbounce::map_polys`). A surface's polygon is its ring by the
engine's walk, and its normal is its plane, negated when bit 15 of the
plane word is set. Every surface is kept whatever its flags: two-sided,
invisible, climbable and breakable surfaces all stop a biped. The
capsule is `masterchief_mp`'s: radius 0.175, standing 0.725, crouched
0.5, its bottom at the unit's origin. Walkable floor is `nz > 0.7`.
Polygons are bucketed on a 1-unit grid, so each test only meets its
neighbours. Each kind is capped at 512 spots, best score first.

### Bounce

The touchdown tick's two contacts at Ascension (y 4.105 and 3.939) are
Havok's contacts with floor edges, not with walls. The floor there is a
strip from y 3.8 to 4.0 between a concave crease, where the ramp
(normal z 0.8396) comes down to it, and a flat seam with the next floor
piece ([geometry](#the-geometry-code-from-the-map)). A capsule the floor
doesn't catch sinks onto both edges at once, and the crease's tilt is
what is left upward.

The scan finds every joint between two walkable polygons: edges on one
line (to 1 cm) that overlap. A joint is a seam when the two normals
agree (dot at least 0.999), a concave crease when the neighbour rises
away from the edge, and is dropped when convex. A bounce spot is two
joints that are parallel (horizontal directions' dot at least 0.98),
between 0.02 and the capsule's diameter (0.35) apart, overlapping by at
least 0.05, with one floor between them (a polygon of each that is the
same or coplanar), at least one of them a crease, and the floor between
rising at most 45°. Seam and crease, two creases (a V-groove) and a
seam up a ramp beside its foot all qualify.

What it reports:

- **edge**: the midline between the two joints over their overlap, at
  floor height.
- **launch**: 111.3 × the sum over the two joints of `1 − cos(dihedral)`
  (0 for a seam). One point calibrates it: Ascension's crease, ramp
  normal z 0.8396 against flat floor, gives the 17.85 units/s the body
  gained on the touchdown tick. A V-groove's two creases add. Strips
  under 3 units/s are dropped; estimates are capped at 60 and scored
  down past it.
- **drift**: across the strip, from the crease toward the other joint
  (the sharper crease of two): +y at Ascension, the way every traced
  bounce came in.
- **stand**: on the midline's middle, 0.02 back toward the crease, origin
  0.01 above the floor. At Ascension that is y 3.88, where the measured
  spot stands.
- **fall**: how much further than a standing jump the player must fall.
  The touchdown tick moves the capsule `v / 30` down, so over a tick's
  start times half the landings sink at least `v / 60`. With the floor
  not caught, the capsule's bottom reaches both edges `r − √(r² −
  (gap/2)²)` below the floor (0.031 on the 0.2 strip); the shallowest
  traced bounce sank 0.070, 0.04 more. So the landing needs `v = 60 ×
  (first touch + 0.04)`, and `fall = (v² − 3²) / (2 × 4.15)`, at least 0.
  3 units/s is a standing jump's landing ("lands at −3"); 4.15 units/s²
  is the gravity that fits the recipe's jump, a 2.43 drop landing at
  5.4. Ascension comes out at about 1.1 units; the recipe drops 2.43.
- **score**: 0.4, plus 0.3 × the overlap over a diameter (to 1), plus
  0.3 for a crease and a seam (the traced shape) or 0.15 for two
  creases.

Strips along one line (drift to 5°, position across to 5 cm, height to
10 cm) that touch or overlap within 10 cm are merged into one spot, so a
floor split into pieces gives one edge.

### Crouch

The measured forced crouch at Ascension, origin (−10.91, 0.72, −1.16),
is a corner, not a low ceiling. A small prop box (x −11.4 to −11.0,
y 0.11 to 0.61) overhangs its lower part by 5 cm, with a 45° lip
underneath (normal `(0, 0.7071, −0.7071)`, z −0.73 to −0.68). The lip's
corner is 0.142 from the capsule's axis: the standing capsule's top
reaches into it, the crouched one stays under it.

So the scan tests the capsule itself. Candidate origins sit on a
world-aligned 4 cm lattice over walkable floor, only within a radius
(horizontally) of something that isn't walkable floor and reaches up to
standing height plus a radius over it. At each, the origin is where the
lower sphere rests on the floor (`r / nz − r` above it, plus 0.01). The
capsule's segment runs from the origin plus `r` to the origin plus its
height minus `r`, as the engine builds it ([crouch height
term](#the-crouch-height-term-code)); its top is at 0.725 standing, the
height `crates/live/bots/src/nav.rs` `standing_clear` checks. A capsule
hits a polygon it is more than 0.005 into, except a face looking up
(`nz > 0.3`) whose nearest point is under the lower sphere's centre,
which is support. A sample where the standing capsule hits and the
crouched one doesn't is a forced crouch.

Forced samples that neighbour each other (8-neighbours, floors within
0.15) are one region. Most regions are no use to a player: hair-thin
bands at the edges of low ledges and props, where only the crouched
capsule's rounded top slips under (under a ledge 0.4 up, 0.157 to 0.17
from its edge), and pockets inside props. A region is a spot only when:

- **it is more than a speck**: at least 2 samples;
- **it touches standing room**: some of its samples have a neighbour
  where standing is clear (its exits);
- **it is reachable**: walking clear samples from its exits reaches a
  lattice cell with no sample that is open floor, walkable floor with no
  walkable face (a prop's top) within 1.075 over it. A region walled in
  and roofed, inside a prop, never does;
- **it is wide enough to stand in**: at up to 24 of its exits, the band
  is probed both ways along each lattice axis to where the crouch is no
  longer forced (1 cm steps, then three halvings), and the narrower axis
  is its width there; along one axis alone, a band's end would measure
  its length. It must be at least 4 cm wide at one of them. The
  Ascension lip's band measures 5 cm, its overhang; a diagonal band
  measures up to √2 times its width.

There is no length rule. On the map the measured corner is a pocket two
samples (4 cm) long, between the lip's corner and a wall at x −11.10,
0.18 from the axis.

Then a spot within 1.5 of a better one is dropped, and the 30 best are
kept.

- **stand**: when a bounce spot's stand is within 15 (straight line, not
  walking distance) of the region's centre, the region's sample nearest
  the nearest such stand: the end you leave from to go and bounce. At
  Ascension that is the prop's corner, where the player stood. Otherwise
  the sample nearest the region's centre.
- **edge**: across the largest group of exits that have standing room
  the same way, between its two ends: where a player walks in crouched
  and can't stand up.
- **score**: 0.4, plus 0.2 × the widest band over 0.15 (to 1), plus
  0.1 × the region's length along its principal axis (to 1), plus 0.3 × (1 − the distance to
  that bounce stand over 15) when there is one. You prime, then go and
  bounce, so a crouch spot near a strip ranks first.

`fall`, `launch` and `drift` are 0.

`--explain x,y,z[,r]` (`superbounce::explain`) prints every forced-crouch
region with a sample within r (default 1.0) of the point, before the
filters and through them: its sample count and bounds, the band width at
each probed exit, its length, whether the walk reaches open floor and
where (or why not), why it was dropped, the better spot that suppressed
it, and its score. It is the tool for tuning these rules against a real
map.

## The superbounce game type

A built-in Slayer variant, Superbounce, for practising: Bounce Practice's
rules (no score or time limit, 3 s respawn, no suicide penalty,
overshields, damage resistance, a magnum, nothing on the map) plus a HUD
and lines over the map. The mode is on when the active variant is the
built-in Superbounce (told by its name,
`crates/extensions/game/src/community_variants.rs` has no field for the
flag), when the variant's extension block has the superbounce flag (bit 2
of `+0x10f`, [game-options.md](game-options.md)), or with
`H2_SUPERBOUNCE=1` ([switches](../runtime/switches.md)). An agent can
force it on or off in any game with `superbounce.set`
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type)). It is
`crates/extensions/game/src/superbounce_mode.rs`.

**State.** On or off is decided when a game starts and again every 30
ticks (so a guest's late host block counts); off, a tick costs a counter
and a flag. On, each tick reads the first local player's unit, its
position, whether its biped is airborne (biped flags bit 0, as
`world.object`'s `physics.airborne` reads it), its vertical velocity, its
aim as a yaw (the first-person camera looks along it) and the word of its
one pair against the level (pair `+0x0c`,
`live_telemetry::havok_agents::fixed_pair_word`, which checks handles and
islands and decodes no collision-agent cache). A word of −3 is primed. It
publishes that with the nearest bounce spot; the HUD, the lines and
`superbounce.state` ([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type))
all read it.

**Spots.** The first tick on a map and structure BSP starts a scan of the
BSP's collision geometry on a background thread (`superbounce::scan_map`,
"Spots in the geometry"); the result is kept per map name, BSP and the map
file's header checksum, so an override with the same name is scanned
again, and a scan still running when the map is left is not started
twice. The feature is presentation-only (a recorded film's hook digest
ignores it) and snapshot-stateless: nothing it holds needs saving. The nearest
bounce spot is the one with the nearest edge.

**The HUD.** A small glass panel at the right of the game image, a little
over a fifth of the way down: translucent dark blue with clipped corners,
thin cyan edges and a soft glow, all drawn in code
(`crates/apps/app/src/superbounce_hud.rs`, on the shell's CPU rasteriser
`crates/apps/app/src/hud_shapes.rs`). It shows whether or not the
watermark does (F3 does not hide it), and also with `--no-watermark`.

- **Primed.** A ring with an upward chevron, lit cyan and pulsing while
  primed, with `PRIMED -3`; dim grey with `NOT PRIMED` otherwise.
- **Launch.** A slanted gauge filled by the nearest spot's estimated
  launch, 0 to 30 units/s (full past 30), ticked every 5, with the
  number.
- **The spot.** Its distance in metres (a world unit is 3.048 m), a small
  arrow turned toward it from where the camera looks (none without an
  aim, or standing on it), and the fall it needs, `drop 1.1` (world
  units).
- **The scan.** `scanning map...` with a spinner until the scan is done;
  a failed scan says why, and a map without spots says so.
- **The hint.** While primed and airborne, `HOLD X - guide onto strip`.

The panel is drawn in design units, a pixel each at 720 lines: its height
over 720, rounded to a whole number so the 8x8 letters stay sharp (1 at
720p, 2 at 1080p and 1440p, 3 at 4K), and smaller if the panel would not
fit the image's width. The image is redrawn only when what it shows
changes: the inputs are rounded to what is printed, and the pulse and the
spinner step at most 15 times a second.

**The tutorial.** The first time the mode is on, four cards show under the
panel, each a short text and a small diagram:

1. Find a cyan line, a forced-crouch spot; walk in crouched, then stand up.
2. The HUD lights up PRIMED: the collision is in its give-up state (−3);
   crouching again loses it.
3. Find a glowing strip: green means a standing jump is enough, red a big
   drop.
4. Jump or fall onto it, holding X to guide yourself on.

Priming moves card 1 to 2, a bounce spot within 8 m moves 2 to 3, a bounce
(the vertical speed passing 8 units/s upward) or 20 s moves 3 to 4, and a
bounce or 15 s closes 4. F4 moves a card on (and reaches the game only
while no card shows). Closing the last card sets the
`superbounce.tutorial_seen` setting; setting it back to false
(`h2 settings set superbounce.tutorial_seen false`) shows the cards again
the next time the mode turns on, and `H2_SUPERBOUNCE_TUTORIAL=1`
([switches](../runtime/switches.md)) shows them whatever it says.

```sh
h2 superbounce hud-preview --out /tmp/sb-hud              # every state
h2 superbounce hud-preview --state primed --out /tmp/sb-hud
```

`hud-preview` draws the HUD with the window's code over a plain backdrop,
one PNG a state at 1280x720 (and `primed` at 1920x1080 too): `scanning`,
`not-primed`, `primed`, `primed-airborne` and `tutorial-1` to
`tutorial-4`.

**The lines.** The spots are figures on the map, drawn by the window's
world-line pass with no depth test (the styles are described in
[the agent API](../runtime/agent-api.md#drawing-world-space-lines-over-the-frame)),
from `crates/extensions/game/src/superbounce_world.rs` (`frame`): a crouch
spot's footprint, lip and way, a bounce strip's slot and post, and the
predicted landing, as [Crouch spots in the world](#crouch-spots-in-the-world)
and [Landings](#landings) describe.

- Only the 40 spots nearest the player are drawn, nearest first by the
  middle of their edges. Everything fades out between 25 and 45 world
  units from the camera.
- While the player is primed a strip pulses and its drift chevrons flow;
  while not it is at 35% and still, so a bright, moving strip means a
  landing there launches. The strip the assist would steer onto is drawn
  at full strength with a bolder, faster-pulsing core.
- Lines are in the user's HUD colour and labels in the HUD's font
  (`crates/extensions/game/src/superbounce_font.rs`, below).

**The prompts.** While the player is primed, the window also draws big,
centred, shadowed labels with a pulse, on the spots they say something
about (`crates/extensions/game/src/superbounce_prompts.rs`):

- Airborne and falling toward a strip the assist can steer onto (its
  pick), `Hold X` (the X button's glyph) floats 0.3 units above the
  strip's stand point. It grows as the player nears: 1x at 6 units and
  more, 3x at 0.5 and less, linear between (`label_scale`). A guide runs
  along the floor from where the player lands if nothing is done to the
  stand point; the landing itself is the predicted-landing ring
  ([Landings](#landings)).
- Once X is held and the assist is steering, the label reads `Guiding`
  and stops pulsing.
- On the ground, within 1.5 units of a strip whose fall is at most 0.05 (a
  standing jump does it), `Jump` is drawn at the take-off spot, 0.2 units
  back from the stand point against the drift (`takeoff`), with a small
  ring on the floor there. Within 0.1 units of the take-off spot it
  flashes `Jump now`. Only the nearest such strip gets one.
- On the ground, within 3 units of a strip whose fall is more than 0.3,
  `Drop 3 m` (the fall in whole metres, `whole_metres`) is drawn over it,
  so the player knows to come from above. Only the nearest gets one.

The prompts are in sentence case, in the HUD's colour and font, the
words told apart by what they say and how they pulse, not by colour.

`prompt_for` decides what a strip says from whether the player is primed,
airborne, targeted and engaged, the strip's fall and the distances to its
stand point and take-off spot. Strips more than a metre above or below the
player on the ground are ignored. Only the labels that are drawn are
formatted.

### Hold X: the fall assist

Holding X in the air steers the player onto a bounce strip, so the landing
is on the strip, moving slowly along its drift, the way every traced bounce
came in. It works on input, not physics: the assist replaces the left stick
the guest reads and takes X out of it, and writes nothing to the world. It
is `crates/extensions/game/src/superbounce_assist.rs`; `superbounce.assist`
and `superbounce.assist_test`
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type))
show it and hold the button without a pad.

**When it applies.** The mode is on, the first local player's X is held
(pad X, keyboard E by default; the `BUTTON` constant), the player is
airborne and falling (vertical speed at most 0.5 units/s), and a bounce spot
passes the reach cone:

- its stand point is below the player and within 3 units sideways;
- the time to fall to its height is at most 3 s (`fall_time`, from the
  height and vertical speed under gravity 4.15 units/s²);
- the landing the player would have if the stick were let go (`predicted_landing`,
  coasting for that time) is within what the stick can move in the time left:
  half the air acceleration times the fall time squared, plus 0.4 units. The
  cone is wider the longer the fall.

Of the spots that pass, the one the coasting landing misses by least is
steered onto.

**The steer.** The aim is the stand point, 0.1 units back along the spot's
drift, crossed at 0.8 units/s along the drift. A PD controller (`steer`,
5 rad/s, damping 0.8) tracks the line that arrives there: the position the
player would be at, the time left before touchdown, moving at the arrival
velocity. It asks for a horizontal acceleration in world axes;
`world_to_stick` turns it into the left stick in the aim's frame (forward is
`(cos yaw, sin yaw)`, right `(sin yaw, -cos yaw)`) by dividing by the air
acceleration (`AIR_ACCEL`, 3 units/s², an estimate of what a full stick
does in the air) and cutting it back to the unit disc. Without a stick that
strong the player misses; with one stronger the gains still land close
(simulated at 0.7 to 1.4 times the estimate).

**The button.** While the assist is engaged X reads as released, so the
button does not reload or pick up. Once it has engaged, X stays released
until the player lets go, so a landing or a strip going out of reach does
not hand the game a fresh press. With no strip in reach, X and the stick
pass through unchanged.

**The prompt.** The assist's pick is also what the world prompts are
drawn from, whether or not X is held: the window reads the player's body
and runs `pick` on each frame, so `HOLD X` appears on the strip before the
button is pressed, and turns to `GUIDING` once the tap reports it is
engaged (`assisting`; [the prompts](#the-superbounce-game-type)).

**Where it sits.** An input tap at `XInputGetState`
(`AssistTap`, installed by `register_late` before the agent's tap), so
the agent's `input.pad` merges first and the assist sees that pad. With the
mode off a pad read costs one atomic load. The tap changes the packet before
the recording or replay tap under it sees it: a film or input log records
the pad the guest got, and a replay overwrites the packet, so the assist
never runs in a replay. The `superbounce-assist` feature has no guest hooks,
so a film's hook digest does not include it, and it is snapshot-stateless:
the swallowed-button latch and the test hold are momentary.

The gains, the air acceleration and the stick's dead zone in the guest
have not been measured in the game; they are the first thing to tune in a
lab run.

## Open

- **The last 5 units/s.** The upward part of the two impulses on the
  touchdown tick gives 23 units/s over the body's mass; the body gains
  17.85 ([touchdown tick](#the-touchdown-tick-trace-stepped)). What the
  virtual `+0x74` before the −3 write does isn't read either.
- **What drops the pair.** Entering a crouch and the body going to
  sleep both rebuild it at 0, which ends the effect. How long a stand it
  takes to sleep isn't measured.
- **60 frames a second.** The player reports no bounces at 60. That isn't
  measured here, and how a frame rate would reach the 30 Hz tick isn't
  known.
- **The launch rule.** It is Havok's contact response for a capsule
  pushed into a concave crease next to a coplanar seam. Decoding it means
  reading the solver in the Havok library code (the step at `0x00311100`
  and its contact managers).
- **Lunges.** Whether a landing on the crease during the 1.5 s after a
  lunge (body offset from the origin) bounces differently is not
  traced.
- **Other bounce spots.** Only this patch was traced. Whether every
  superbounce spot is a crease next to a seam is not checked. The
  [scan](#spots-in-the-geometry) finds candidates of that shape; none is
  traced.

## Charge, crouch spots and landings

Design; the world overlay ([Crouch spots in the world](#crouch-spots-in-the-world),
[Landings](#landings) but "What happened") is built, the rest not. What
the mode shows next about the three steps of a bounce: getting the charge, keeping it, and landing it. The HUD parts
(the meter under the pill, the prompt line, the notifications, the
navpoint) are in [hud-kit.md](hud-kit.md), "The charge meter"; this is
the state they read, the scanner's rules and the world overlay. The
numbers are in [notes/superbounce-spots.md](notes/superbounce-spots.md).

### Charge

The pair word has no amount. It is 0 through a crouch, forced or not,
and −3 from the first tick of the stand-up out of a forced crouch, where
the crouch lets go at the band's way out, 6 cm from where the body
rested in the measured corner (**Trace**, the note). So "how charged" is
two things the engine does have:

- **How far through the recipe.** `near` (crouched by a crouch spot),
  `under` (crouched and the standing capsule does not fit: output bit 1
  of the character physics update, [forced crouch](#forced-crouch-code)),
  `held` (the crouch button is up and the request, unit flags bit 23, is
  still set), `primed` (the word is −3).
- **How long it will last.** A primed body that stands still sleeps
  after 38 to 40 ticks and wakes with a new pair at 0 (**Trace**). The
  mode counts the ticks the body's speed is under 0.03 units/s, back to
  0 when it moves; the charge left is 1 less that count over 38. A
  crouch ends the charge at once (**Trace**).

`superbounce_mode`'s state carries it as `charge`: the phase, the fill
(0, 1/3, 2/3, then the charge left), the still ticks and the bit.
`superbounce.state` returns it, and `world.object`'s `physics` block
gains `standing_fits`, read where the biped physics update tests it
(`0x000e28f4`).

Not a charge, and not shown as one: how long the crouch is held (nothing
counts it), and how deep the standing capsule is in the lip (most bands
are far deeper than the one that is known to prime, the note).

### Crouch spots: the stricter rule

The scan keeps a forced-crouch region by the rules of
[Crouch](#crouch), then:

1. **In play.** The stand point is no more than 1.0 below the lowest of
   the scenario's player starting locations. Regions under the map are
   dropped (5 of Ascension's 21).
2. **By a strip.** A bounce spot's stand is within 8 units, straight
   line: a rule, where the score only prefers a strip within 15. If
   fewer than 3 spots pass on a map, the 3 nearest a strip are kept
   (Coagulation).
3. **Spread out.** A spot within 3.0 of a better one is dropped, and the
   12 best are kept.
4. **Proven first.** A measured priming place
   (`crates/live/bots/src/glitches/spots.rs`, a list beside the bounce
   spots, given to the scan as points) is kept whatever the other rules
   say and ranks first. A spot within 0.3 of one is `proven`; the others
   are `likely`. The Ascension corner is the one proven spot, and it
   ranks 13th of 21 by score alone.
5. **Stand at the way out.** `stand` is the region's exit sample (a
   forced sample with standing room next door) nearest the nearest
   strip, not the sample nearest the centre: the word turns at the way
   out. The spot also reports `out`, the unit direction from `stand` to
   that standing room, and `lip`, up to 8 edges of the faces the
   standing capsule hits there, at their own height.

There is no depth rule: a cut by the standing capsule's overlap drops
almost nothing and has no measurement behind it (the note).
`--explain` prints, per region, each rule's verdict in this order, with
the lowest starting location, the nearest strip and its distance, the
better spot that crowded it out, and its tier. Expected on the disc's
maps: 7 to 12 a map, 3 on a map whose strips are far from its lips
(**S**, the note's table; `h2 superbounce scan <map> --json` counts
them).

Which `likely` spots prime is a game run's to say. A lab pass walks
each kept spot (below) and its result is a row in the proven list or a
row in a not-priming list the scan drops.

### Crouch spots in the world

One spot is one figure on the floor, drawn by the window's world-line
pass (`crates/extensions/game/src/superbounce_world.rs`, the geometry;
`crates/apps/app/src/overlay/marks.rs`, the drawing):

- **The footprint.** A ring of the capsule's radius (0.175) round
  `stand`, 0.02 above the floor (`ring`): the size of the player's own
  feet, where to be. It is thin while the player is elsewhere, and bold
  and pulsing while the phase is `under` or `held`. The phase belongs to
  the nearest crouch spot within 1 unit of the player, sideways.
- **The lip.** The `lip` edges, a thin line where the scan found them, at
  head height: what to get under.
- **The way.** Floor chevrons along `out`, from 1.2 out to just outside
  the ring (`chevrons`), flowing in toward the ring until the phase is
  `held`, then flowing out: walk in, then walk out. They travel at 0.4
  units/s, 0.25 apart.
- **The label.** `Crouch` 0.15 over the lip (0.8 over `stand` without
  one) of the nearest spot within 3 units; none on the others. The verb
  with the button is on the HUD's prompt line.

Only the nearest 3 get the lip and the way; the rest are the ring alone.
A `likely` spot is drawn at half strength, a `proven` one at full. While
primed every crouch figure drops to a fifth and the strips come up, as
the strips are dimmed while not primed: what is bright is what to do
next, and a crouch now would lose the charge.

### Landings

The scanner's bounce spot gains `width` (the gap between its two
joints) and `need` (the touchdown speed its `fall` is worked from).

The figures are `crates/extensions/game/src/superbounce_world.rs`'s.

- **Where.** The floor ribbon is the slot itself: `width` across, the
  overlap long, under the line down its middle; a cap across each end
  closes it and a tick across marks `stand` (`slot_marks`).
- **How high.** A thin post stands at `stand` to `fall` above the strip,
  with a small cross at its top (`post`): jump from a floor at or above
  it. Its label is the height in whole metres (`3 m`, at least 1), on the
  nearest 3 strips. A strip a standing jump reaches (`fall` at most
  0.05) has no post.
- **From here?** A strip's strength says whether the player's own floor
  is high enough (`high_enough`): full when their feet (the unit's
  origin) are at least `fall` above `stand` or it needs no fall, half
  when they must climb; and 35% of that while not primed
  (`strip_strength`). Every strip is the same colour, so the player does
  not have to turn a colour into a height.
- **On course.** While primed, airborne and moving down, the landing the
  state predicts (`landing`) is drawn where it is: a ring of the capsule's
  radius when it is inside the slot, the ring broken into every other
  segment when the assist can still bring it in (`Hold X`), and a cross
  when it cannot. 0.35 to its side, across the line from the player, a
  0.5-high post fills from the floor with the touchdown speed over `need`
  (`speed_fill`), full at `need`.
- **What happened.** On the tick a primed or unprimed player touches
  down within 0.5 of a strip the mode files the landing: `bounce` when
  the vertical speed comes out over 8 units/s, else the first of
  `not primed` (the word was not −3 on the tick before), `missed` (the
  origin was outside the slot, with the distance), `too slow` (the speed
  in was under `need`), `no bounce`. A bounce flashes the strip once and
  the others dim it for a second. The HUD says it in the notification
  list, and `superbounce.state` returns the last landing (the outcome,
  the strip, the speed in and out, the miss).

The colours are the game's: lines take the user's HUD colour, the
three floats at `0x00468c84` (the human triple `0x0013927e` returns),
read from the guest each frame (`superbounce_font::hud_colour`). The
Elite triple and the `hudg` timer-warning colour are not used: a miss is
told by its cross, not its colour. Labels are drawn in the disc's HUD
font (font 6) with its button glyphs, in sentence case, numbers in whole
metres. No colour or letter shape is ours.

**The font.** `superbounce_font::Capture`, chained after the font
rewrites in the boot runner's VFS (`crates/apps/boot/src/lib.rs`),
serves every file unchanged and keeps `font_table.txt` and the file on
its line 6, counted from 0, as the game reads them: the HUD's font, with
the icon sets' and bot mark's glyphs in it. The first frame that draws
packs its printable ASCII and button glyphs (U+E100 to U+E111) into a
512-wide atlas (`gfx::world_lines::LabelFont`, built in
`superbounce_font::atlas`) and publishes it. The window uploads the
atlas once per font and draws each glyph as a textured quad in the same
draw as the lines, laid out as the engine lays text out (kerning,
`x_offset`, `y_offset` from the ascent). A plain glyph takes the label's
colour; a glyph with coloured pixels (the face buttons) keeps its own. A
font pixel is a HUD pixel (the HUD is 480 lines) at `label_scale` 2, the
size the world labels use. Centred labels have a black drop shadow one
font pixel down and right; the HUD's own shadow colour
(`[0x004686d4]`) is not read.

Without the font (a host that does not mount the disc through the boot
runner, or a font that does not parse) the labels are drawn in the
window's 8x8 debug font, the X button is written `X`, and the log says
`superbounce: world labels use the 8x8 debug font and button letters`
with the reason, once. Without the HUD colour the lines are white and
the log says so, once.

### Checking it on the lab

Each of these is a lab run through the agent API
([agent-api.md](../runtime/agent-api.md), [lab.md](../runtime/lab.md)) at
30 frames a second, read every tick at pinned ticks:

- **The phases.** Play back the recorded forced crouch (above, "The
  recipe") and read `superbounce.state` with `world.object` and
  `havok.agents`: the phase is `under` from the first tick the standing
  capsule does not fit, `held` from the tick the button is up, `primed`
  on the tick the word is −3.
- **The time left.** Set −3 with `havok.pair.set`, stand still, and read
  `havok.agents` until the pair goes: the fill reaches 0 on that tick,
  within 2. Then creep at 0.02, 0.05 and 0.1 units/s for the speed
  limit.
- **Each kept crouch spot.** `sim.state.set` to 0.3 out along `out`,
  facing in; `input.pad` crouch held and the stick in, crouch released,
  then the stick out; read the request bit and the word. A spot that
  holds the request and turns the word is proven. The measured corner
  was not reached by scripted sticks, only by the recorded input, so a
  miss here is not proof a spot fails: try three approach angles.
- **The landing report.** The recorded jump with the word at −3, at 0,
  from a lower take-off and 0.3 to the side: `bounce`, `not primed`,
  `too slow` and `missed` in turn.

No game was run for this section.
