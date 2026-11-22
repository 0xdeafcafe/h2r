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

### Zanzibar's window (Trace, lab play)

The user played the community's window bounce on Zanzibar on the lab
(job 0043, input recording `zanzibar-window`), read about every 12 ticks
through `superbounce.state` and `world.object`
([the note](notes/superbounce-spots.md#2026-11-21-zanzibars-window-played)).
The recipe: in the base facing the beach, the rear right inner window,
break the glass, crouch under the shard left of the gap, let go, turn
right, jump, fall onto the crack below.

- **The prime.** The player crouched at (5.22, −26.285) on the window
  sill (z 5.10). The lip that forces the crouch is the broken glass, an
  object, not the structure or instanced geometry, so the scan sees none
  there. The phase went `under`, then `held`, and the word turned −3 on
  the jump, while still forced crouched: not on a walk out.
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
- **need**: that touchdown speed `v`, downward, in units/s: 4.28 at
  Ascension.
- **width**: the gap between the two joints, the slot's real width
  (0.2 at Ascension).
- **score**: 0.4, plus 0.3 × the overlap over a diameter (to 1), plus
  0.3 for a crease and a seam (the traced shape) or 0.15 for two
  creases.

Strips along one line (drift to 5°, position across to 5 cm, height to
10 cm) that touch or overlap within 10 cm are merged into one spot, so a
floor split into pieces gives one edge. The merged spot keeps the
smallest `fall`, with the `need` and `width` it came from.

#### Single creases

The Zanzibar window landing ([Zanzibar's
window](#zanzibars-window-trace-lab-play)) is one concave crease, a flat
floor meeting a 17.6° ramp, with no second joint, so no slot. Every
concave joint between two walkable floors is also a candidate of its own
(`Shape::Crease`, `shape` `crease` in `--json` and `superbounce.spots`;
`bounce.rs`, `crease`), for a game run to try:

- **edge**: the crease, at floor height. **drift**: across it onto the
  flatter floor.
- **launch**: 111.3 × its own `1 − cos(dihedral)`, the slot calibration
  with no second joint: 5.2 units/s at Zanzibar, the scale of the small
  pop its first try made. Under 3 units/s (a dihedral under about 13°)
  it is no candidate.
- **stand**: where the capsule's lower sphere rests touching both
  floors, its centre `r · tan(θ/2)` out over the flatter one (2.7 cm at
  Zanzibar), at that floor's height plus 0.01.
- **need**, **fall**: the slot rule with no gap: the sphere touches both
  floors at once, so the landing needs only the 0.04 of sink, 2.4
  units/s, and `fall` is 0. A guess the Zanzibar run does not support:
  its pop came after a 3.4 fall.
- **width**: 0.1, a band guessed for the ribbon and the verifier.
- **score**: 0.3, plus 0.2 × its length over a diameter (to 1), plus
  0.1 × its crease over Ascension's (to 1): at most 0.6, under a slot of
  the traced shape.

Creases are merged along their line like slots, and the best 128 are
kept before they join the slots under the cap. They are not strips to
the crouch rules: a crease neither keeps a crouch spot "by a strip" nor
scores one up. `--explain` lists every concave crease within its radius
with its dihedral and the candidate it makes, or why none
(`superbounce::explain_bounces`), then the bounce spots there.

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

The regions that pass become spots, and the [stricter
rule](#crouch-spots-the-stricter-rule) cuts them to the ones kept.

- **stand**: the region's exit sample (a forced sample with standing
  room next door) nearest the nearest bounce spot's stand, straight line:
  the way out you leave by to go and bounce, where the word turns. At
  Ascension that is the prop's corner, where the player stood. With no
  bounce spot on the geometry, the exit sample nearest the region's
  centre.
- **out**: the unit direction (X and Y) from `stand` to the standing room
  beside it: the sum of the lattice sides with standing room.
- **lip**: up to 8 edges, nearest first, of the faces the standing
  capsule hits at `stand`: of each face, its edge nearest the capsule's
  segment, at its own height, clipped to 0.5 each side of its point
  nearest the capsule (`superbounce::Lip`).
- **width**: the widest band measured at the probed exits.
- **edge**: across the largest group of exits that have standing room
  the same way, between its two ends: where a player walks in crouched
  and can't stand up.
- **score**: 0.4, plus 0.2 × the widest band over 0.15 (to 1), plus
  0.1 × the region's length along its principal axis (to 1), plus 0.3 ×
  (1 − the distance from the region's centre to the nearest bounce stand
  over 15) when one is within 15. You prime, then go and bounce, so a
  crouch spot near a strip ranks first. A proven spot scores 1.

`fall`, `launch`, `need` and `drift` are 0.

`--explain x,y,z[,r]` (`superbounce::explain`) prints the map's starting
locations and measured priming places, then every forced-crouch region
with a sample within r (default 1.0) of the point, before the filters
and through them: its sample count and bounds, the band width at each
probed exit, its length, whether the walk reaches open floor and where
(or why not), each stricter rule's verdict, why it was dropped, the
better spot that suppressed it, its score and its status. It is the tool
for tuning these rules against a real map.

## The superbounce game type

A built-in variant, Superbounce, for practising: Bounce Practice's
rules (no score or time limit, 3 s respawn,
overshields, damage resistance, a magnum, nothing on the map) plus a HUD
and lines over the map. It is listed under **Tweakers**, the last row of
the game type list (after Territories), and not under Slayer; underneath
it is a Slayer variant ([community-gametypes.md](community-gametypes.md#tweakers)).
Its entry in `crates/extensions/game/src/community_variants.rs` is
`Category::Tweakers(Tweak::Superbounce)`: its record's extension block
names the Superbounce rules script (mode 2, the script's hash prefix and
default options, `community_variants::superbounce_ext`), and
`crate::tweakers` writes that block into its built-in record, so a game
on it and a saved copy of it name the script. The script is
`crates/formats/rules/src/builtin/superbounce.rules`, compiled into the
game: `list tweakers` files the variant under Tweakers and
`requires hud.superbounce` turns the mode on
([rules-engine.md](rules-engine.md#firefight-ai-and-bounces)). It scores
bounces alone, which the script works out from what the medal chain
files ([Medals](#medals)); a kill scores nothing:

| Event | Points, by default | Option |
|---|---|---|
| a bounce | `bounce_points` (1) times its place in its chain, at most 6: a single 1, a double 2 (3 for the chain of two), the sixth and every bounce after it 6 | `bounce_points`, 0 to 5; 0 scores no bounce |
| a Monster Bounce | twice the bounce's, after the cap: a Monster single 2, a Monster eighth of a chain 12 | `monster_double`, on |
| a Bounced | −1, once: the suicide point while `suicide_costs` is on, else the Bounced penalty | `bounced_costs`, on |
| a suicide (a death with no killer, or by the player's own hand) | −1 | `suicide_costs`, on |
| a kill | nothing | none |

The cap is Bouncetacular's count, so a long chain cannot run away. A
kill scores nothing, so the board counts bounces; a betrayal costs
nothing either. The suicide point stays, so dying on purpose to reset a
bounce attempt is not free. A
Bounced is a death with no killer too, so while `suicide_costs` is on,
`on kill` takes its point and `on bounced` takes none: a Bounced costs one
point either way (supposed: that the game reports a fall out of the map as
a death with no killer; a kill feed line on a lab run would show it).

The mode is on when the variant's extension block names a rules script
that requires `hud.superbounce` (`rules::header::script_asks_superbounce`,
in `variant_ext::Decision::superbounce`, with or without the `rules`
feature, on a guest that applies its host's block too), when the block
has the superbounce flag (bit 2 of `+0x10f`,
[game-options.md](game-options.md)), when the active variant's name is
the built-in Superbounce's (a fallback for a game whose block did not
arrive or is not applied, as in a System Link game, where no machine
applies the block), or with
`H2_SUPERBOUNCE=1` ([switches](../runtime/switches.md)). The built-in
sets no flag; the flag stays a field of the block that a hand-made
variant may set. An agent can force it on or off in any game with
`superbounce.set`
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type)). It is
`crates/extensions/game/src/superbounce_mode.rs`.

Each bounce the medal chain files goes to a running rules script as
`on bounce(p, spot, chain, monster)`, `spot` the strip's id
(`superbounce_chain::spot_id`), `chain` its count and `monster` whether
it is a Monster Bounce; each Bounced goes as `on bounced(p)`.

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
BSP's collision geometry on a background thread (`superbounce::scan_in`
over the map's context, `superbounce::map_context`: its starting
locations and proven places, "Spots in the geometry"); the result is kept
per map name, BSP and the map file's header checksum, so an override with
the same name is scanned again, and a scan still running when the map is
left is not started twice. The map's verified file ("Verifying spots") is
applied to the result, and again whenever the file changes: a `proven`
spot scores 1, a `failed` one is kept for `superbounce.verify` but hidden
from the HUD, the lines and the overlay. The feature is presentation-only (a recorded film's hook digest
ignores it) and snapshot-stateless: nothing it holds needs saving.

**The bots.** While the mode is on the bots fight no one and practise
bounces instead, spread over the map's sites, proven ones first
([bots.md](bots.md#practising-bounces)). The nearest
bounce spot is the one with the nearest edge.

**The HUD.** Drawn by Halo 2's own HUD through the HUD kit
([hud-kit.md](hud-kit.md), "The superbounce HUD on the kit"), for the
first local player: a short grenade-style pill after the grenade row,
lit while primed, with the `^` icon where a third count would be; under
it a charge meter cut from the energy sword's battery strip, filled by
the time the charge has left; Assault's red arming bar low in the
middle, filled by the recipe's steps, with a short ending when primed
("The arming bar" in hud-kit.md); "Charging..." and "Primed!" in the
notification list, as Assault's "Bomb is arming..." and "Bomb armed!";
the step's verb on the game's prompt line ("Hold (crouch) and walk under", "Let go
of (crouch)", "Walk out to stand up"); and the game's navpoint over a
crouch spot while not primed, with the distance in metres, and over the
chosen strip while primed, with the drop in metres
([Floor marks](#floor-marks)).
`crates/extensions/game/src/superbounce_hud.rs` registers it with the
kit and feeds it each tick from the mode's state (`view`); with the mode
off every element is hidden. Launch and drop are on the strips' labels
in the world and in the agent API, not on the HUD.

```sh
h2 hud preview superbounce --state all --mode all --out DIR
```

`h2 hud preview` draws it offline over the retail HUD, from the disc or
the synthetic map, with the kit's checks (hud-kit.md, "The preview and
the checks").

**The tutorial.** The first time the mode is on, four steps show in the
game's training box, two centred lines each:

1. Crouch, walk in under a lip, let go, then walk out.
2. You are primed. Do not crouch again.
3. Follow the navpoint. Jump from the Jump mark onto the Land mark.
4. Jump or fall onto the strip. Fall Assist (Controls) can steer.

No step asks for a button: the landing is the player's own, and the
last step names the opt-in [fall assist](#hold-x-the-fall-assist) once,
by its row.

Priming moves step 1 to 2, a bounce spot within 8 m moves 2 to 3, a
bounce landing or 20 s moves 3 to 4, and a bounce or 15 s closes 4
(`hud_kit_core::huds::superbounce::Tutorial`), in game seconds: 600 and
450 ticks at any frame rate. A training text of the
game's own takes the box while it shows. Closing the last step sets the
`superbounce.tutorial_seen` setting; setting it back to false
(`h2 settings set superbounce.tutorial_seen false`) shows the steps again
the next time the game starts, and `H2_SUPERBOUNCE_TUTORIAL=1`
([switches](../runtime/switches.md)) shows them whatever it says.

**The lines.** The spots are small marks on the map, drawn by the
window's world-line pass with no depth test (the styles are described in
[the agent API](../runtime/agent-api.md#drawing-world-space-lines-over-the-frame)),
from `crates/extensions/game/src/superbounce_world.rs` (`frame`, and the
lock-on's `LockMarks`): the chosen strip's landing mark and its
take-off, the other strips' creases, the crouch spots' footprints, small
diamonds far away, and while falling the lock-on, as
[Floor marks](#floor-marks), [Lock-on](#lock-on) and
[Crouch spots in the world](#crouch-spots-in-the-world) describe.

- Only the 40 spots nearest the player are drawn, nearest first by the
  middle of their edges, and the chosen strip whatever its distance.
  Everything fades out between 25 and 45 world units from the camera.
- While the player is not primed a strip is at 35%; primed, at full
  strength, so a bright strip means a landing there launches.
- Lines are in the user's HUD colour and labels in the HUD's font
  (`crates/extensions/game/src/superbounce_font.rs`, below). The lock is
  the one exception: red, as the game's reticle is over an enemy in
  range.
- The marks are rebuilt each guest tick (30 Hz); what moves between
  ticks (the lock-on closing in, the pulses) moves by the wall clock at
  each present, so it is as smooth at 60 or 120 frames a second as at 30
  ([high-framerate.md](high-framerate.md)).

**The prompts.** While the player is primed and in the air, the window
also draws big, centred, shadowed labels with a pulse, on the strip they
say something about (`crates/extensions/game/src/superbounce_prompts.rs`).
On the ground the [floor marks](#floor-marks) say where to jump from and
land; `prompt_for` keeps the ground rules below as its table, and the
mode draws none of them:

- With the [fall assist](#hold-x-the-fall-assist) on, airborne and
  falling toward a strip it can steer onto (its pick), `Hold X` (the X
  button's glyph) floats 0.3 units above the
  strip's stand point. It grows as the player nears: 1x at 6 units and
  more, 3x at 0.5 and less, linear between (`label_scale`). A guide runs
  along the floor from where the player lands if nothing is done to the
  stand point; the landing itself is the [lock-on](#lock-on).
- Once X is held and the assist is steering, the label reads `Guiding`
  and stops pulsing.
- With the assist off (the default) nothing is said in the air and no
  strip is anyone's target: the landing is the player's, and what the
  map shows of it is the [lock-on](#lock-on).
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

An opt-in help: with the player's **Fall Assist** setting on
(`superbounce.fall_assist`, off by default, a row at the end of the
Controls list that a press turns on or off; `crates/extensions/game/src/setting_defs.rs`,
`crates/extensions/game/src/video_screen.rs`), holding X in the air
steers the player onto a bounce strip, so the landing is on the strip,
moving slowly along its drift, the way every traced bounce came in. Off,
the tap never reads X, nothing steers, and nothing on the map or the HUD
asks for X. The setting is read live (`follow_setting`); the
`superbounce-assist` feature in `H2_FEATURES` stays the kill switch, which
leaves the tap out whatever the setting says. It works on input, not
physics: the assist replaces the left stick
the guest reads and takes X out of it, and writes nothing to the world. It
is `crates/extensions/game/src/superbounce_assist.rs`; `superbounce.assist`
and `superbounce.assist_test`
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type))
show it and hold the button without a pad.

**When it applies.** The mode is on, the setting is on, the first local
player's X is held
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

**The prompt.** With the setting on, the assist's pick is also what the
world prompts are drawn from, whether or not X is held: the window reads
the player's body and runs `pick` on each frame, so `Hold X` appears on
the strip before the button is pressed, and turns to `Guiding` once the
tap reports it is engaged (`assisting`; [the prompts](#the-superbounce-game-type)).
Off, there is no pick: no prompt in the air. The [lock-on](#lock-on)
does not depend on the assist: it closes in on the predicted landing
either way.

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

What the mode shows about the three steps of a bounce: getting the
charge, keeping it, and landing it. The HUD parts (the arming bar, the
meter under the pill, the prompt line, the notifications, the navpoint)
are in [hud-kit.md](hud-kit.md), "The charge meter" and "The arming
bar"; this is the state they
read, the scanner's rules and the world overlay. The numbers are in
[notes/superbounce-spots.md](notes/superbounce-spots.md).

The scanner's part (the stricter crouch rule, and the strip's `width`
and `need`) is built in `crates/live/superbounce`. The state is built:
`superbounce_mode::State` carries `charge`, `verb`,
`nav_target`, `landing`, `last_landing`, `still_ticks` and
`standing_fits` (Code: `crates/extensions/game/src/superbounce_charge.rs`
for the tracker and the landing classifier,
`superbounce_capsule.rs` for the capsule test, `superbounce_assist.rs`'s
`predict` for the predicted landing), and `superbounce.state` returns
them ([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type)).
The world overlay ("Crouch spots in the world", "Floor marks" and
"Lock-on") is built in `superbounce_world.rs`.

### Charge

The pair word has no amount. It is 0 through a crouch, forced or not,
and −3 from the first tick of the stand-up out of a forced crouch, where
the crouch lets go at the band's way out, 6 cm from where the body
rested in the measured corner (**Trace**, the note). So "how charged" is
two things the engine does have:

- **How far through the recipe** (crouch, walk in, let go, walk out:
  one phase a step). `near` (crouched, the crouch request
  set, within 1.5 of a crouch spot's stand point), `under` (crouched with
  crouch held and the standing capsule does not fit), `held` (the crouch
  button is up and the request, unit flags bit 23, is still set: the
  engine keeps it only while standing does not fit, [forced
  crouch](#forced-crouch-code)), `primed` (the word is −3 and the request
  is clear).
- **How long it will last.** A primed body that stands still sleeps
  after 38 to 40 ticks and wakes with a new pair at 0 (**Trace**). The
  mode counts the ticks the body's speed is under 0.03 units/s, back to
  0 when it moves; the charge left is 1 less that count over 38. A
  crouch ends the charge at once (**Trace**), so a primed player whose
  request is set is `lost`, as is one whose word leaves −3. `lost` shows
  for 30 ticks.

**Why it went.** The tracker gives each loss a reason, read on the tick
the charge goes (`superbounce_charge::LostReason`):

| Reason | When |
|---|---|
| `crouched` | the crouch request is set |
| `stood_still` | the word left −3 after 30 still ticks or more (`STOOD_STILL_TICKS`): the body slept. 30 is under the 38 to 40 measured, so a sleep a few ticks early still reads as one |
| `unknown` | the word left −3 for another reason: a respawn, a vehicle, the pair gone, or a sleep the still count missed |

A landing is not a reason. Nothing measured has a landing use the
charge up: the word stays −3 after a bounce until the body sleeps (the
note), and a landing that does not bounce is filed as a landing, not a
loss.

**The warning and the new charge.** From 19 still ticks while primed
(`WARN_STILL_TICKS`, half of 38, where the fill is 0.5) the verb is
`KeepMoving`. For 150 ticks after a loss (`AGAIN_TICKS`: the `lost`
second and four more), while the phase is `lost` or hidden, the verb is
`ChargeAgain`; a crouch step (`near`, `under`, `held`) takes the line
from it.

`superbounce_mode`'s state carries it as `charge` (the phase, the fill:
0, 1/3, 2/3, then the charge left, 0 while lost; and the loss, its
reason and tick, while lost), `still_ticks`, `verb` (`LetGoOfCrouch`
while `under`, `WalkOutToStand` while `held`, `KeepMoving`, and
`ChargeAgain` with the loss) and `standing_fits`. `superbounce.state`
returns them, the reason as `lost_reason`, and `world.object`'s
`physics` block has `standing_fits` for bipeds.

Two inputs are not the engine's own:

- **The standing capsule.** The engine's answer, output flags bit 1 of
  the character physics update (`0x001e55d0`), is not kept in a field
  found so far, so `under` and `standing_fits` come from the mode's own
  test (`superbounce_capsule.rs`): the scan's standing capsule (bottom
  at the origin, 0.725 tall, radius 0.175, in deeper than 5 mm, upward
  faces at the feet are support) against the structure BSP's collision
  polygons, which the scan keeps in a 2-unit grid. It exists only while
  the mode is on and its scan is ready (**S**: that it agrees with the
  engine's bit at the measured corner). Reading the engine's bit needs a
  hook at `0x000e28f4` or the output pointer of `0x001e55d0`, and is
  **Open**.
- **The crouch button.** `held` needs to know the button is up. The fall
  assist's input tap (`superbounce_assist.rs`) reads it from the pad the
  guest gets on the first controller, through the player's action table
  (`0x51ea18 + 8 + 10`, crouch's slot; the Default layout's left-stick
  click when it cannot be read), with the agent's `input.pad` steps and
  before an `H2_INPUT_REPLAY`'s. Without the tap (the
  `superbounce-assist` feature off) the button is unknown and the phase
  goes from `under` to `primed` with no `held`.

Not a charge, and not shown as one: how long the crouch is held (nothing
counts it), and how deep the standing capsule is in the lip (most bands
are far deeper than the one that is known to prime, the note).

### Crouch spots: the stricter rule

The scan keeps a forced-crouch region by the rules of
[Crouch](#crouch), then cuts the spots by these, in this order
(`crouch.rs`, `select`):

1. **In play.** The stand point is no more than 1.0 below the lowest of
   the scenario's player starting locations (`scnr+0x100`, the position
   at `+0x00`), counting only those over the scanned geometry's bounds,
   since a scenario's starting locations are for all its BSPs. With none
   there the rule is off. Regions under the map are dropped (5 of
   Ascension's 21).
2. **By a strip.** A bounce spot's stand is within 8 units, straight
   line: a rule, where the score only prefers a strip within 15. If
   fewer than 3 spots are kept by the rules above and below, the spots
   nearest a strip are added, nearest first, through the same spacing,
   until 3 are (Coagulation). With no strip on the geometry that is the 3
   best by score.
3. **Spread out.** A spot within 3.0 of a better one is dropped, and the
   12 best are kept.
4. **Proven first.** A measured priming place (`superbounce::PROVEN`,
   looked up by the map's header name, or the list in the
   `superbounce::Context` handed to `scan_in`) is kept whatever
   the other rules say, before the others, so it crowds them out rather
   than the other way round, and it scores 1, so it ranks first. A spot
   with a sample within 0.3 of one is `proven`; the others are `likely`.
   A place no region is near (its lip is an object the scan doesn't
   read) is a proven spot of its own: `stand` where it was measured, its
   `out` from the table, its edge a 0.4 line across that, and its note
   in `--explain`. The places: the Ascension corner (−10.91, 0.72,
   −1.16), which by score alone ranks 13th of 21; Zanzibar's window sill
   (5.22, −26.285, 5.11), under broken glass (break it first; the word
   turns on the jump); and Zanzibar's walk-out place (8.07, −3.01, 4.51)
   ([Zanzibar's window](#zanzibars-window-trace-lab-play)).
5. **Stand at the way out.** `stand` is the region's exit sample nearest
   the nearest strip, not the sample nearest the centre: the word turns
   at the way out. The spot also carries `out` and `lip`
   ([Crouch](#crouch)).

`superbounce::scan_map` and `explain_map` read the starting locations
and the proven places from the map (`superbounce::map_context`);
`superbounce::scan` and `explain` on bare polygons have neither, so the
in-play rule is off and nothing is proven.

There is no depth rule: a cut by the standing capsule's overlap drops
almost nothing and has no measurement behind it (the note).
`--explain` prints, per region, each rule's verdict in this order (`in
play`, `by a strip`, `spread out`, `proven`, `stand at the way out`),
with the lowest starting location, the nearest strip and its distance,
the better spot that crowded it out, and its status. Expected on the
disc's maps: 7 to 12 a map, 3 on a map whose strips are far from its
lips (an estimate from the note's table; `h2 superbounce scan <map>
--json` counts them).

Which `likely` spots prime is a game run's to say: `superbounce.verify`
tries each kept spot in the game and writes the map's verified file
([Verifying spots](#verifying-spots)), which marks the spots that primed
`proven` and the ones that never did `failed`.

### Crouch spots in the world

One spot is one figure on the floor, drawn by the window's world-line
pass (`crates/extensions/game/src/superbounce_world.rs`, the geometry;
`crates/apps/app/src/overlay/marks.rs`, the drawing):

- **The footprint.** A ring of the capsule's radius (0.175) round
  `stand`, 0.02 above the floor (`ring`): the size of the player's own
  feet, where to be. It is thin while the player is elsewhere, and bold
  and pulsing while the phase is `under` or `held`. The phase belongs to
  the nearest crouch spot within 1 unit of the player, sideways.
- **The approach arrow.** One arrowhead on the floor 0.15 outside the
  ring along `out` (`arrowhead`), pointing in toward the ring until the
  phase is `held`, then out: walk in, then walk out.
- **The label.** `Crouch` 0.15 over the lip (0.8 over `stand` without
  one) of the nearest spot within 3 units; none on the others. The verb
  with the button is on the HUD's prompt line.
- **Far away.** Past 10 units the spot is a small upright diamond facing
  the player instead (`diamond`), 0.3 over `stand`, its size 0.012 of
  its distance so it stays the same size on screen (`far`,
  `diamond_half`).

A `likely` spot is drawn at half strength, a `proven` one at full. While
primed every crouch figure drops to a fifth and the strips come up, as
the strips are dimmed while not primed: what is bright is what to do
next, and a crouch now would lose the charge.

### Landings

The scanner's bounce spot carries `width` (the gap between its two
joints) and `need` (the touchdown speed its `fall` is worked from;
[Bounce](#bounce)). How a landing is shown is in
[Floor marks](#floor-marks) and [Lock-on](#lock-on); this is the state
under them.

- **On course.** The state's `landing` (`superbounce_assist::predict`)
  is there whenever the player is airborne and falling, primed or not:
  of the strips below whose stand point the coasting landing is within
  3 units of, the one it lands nearest the slot of, with its `verdict`
  (`OnSlot`, `Fixable` by the assist's reach gate, else `Miss`), `speed`
  and `need`, and the strip's `edge`, `stand`, `width` and `fall`.
  `miss` is how far outside the slot it lands (0 inside), and it is
  `locked` when it is `OnSlot` and `speed` is at least `need`
  (`superbounce_charge::Predicted`).
- **What happened.** On the tick a primed or unprimed player touches
  down within 0.5 of a strip the mode files the landing: `bounce` when
  the vertical speed comes out over 8 units/s, else the first of
  `not primed` (the word was not −3 on the tick before), `missed` (the
  origin was outside the slot), `too slow` (the speed in was under
  `need`), `no bounce`. A touchdown is a tick whose previous tick was
  airborne and falling at 1 unit/s or more and which has stopped falling
  (over −0.5), launched, or landed; the origin tested is the previous
  tick's, before the contact moves it. A bounce flashes the strip once
  and the others dim it for a second. The HUD says it in the
  notification list, and `superbounce.state` returns the last landing as
  `last_landing`: the outcome, the game tick and the origin.

### Floor marks

Two small marks say where to jump from and where to land, on the
**chosen strip**: the navpoint's (`superbounce_charge::nav_target`), the
nearest strip the player's floor is high enough for (feet at least
`fall` above `stand`, less 0.05; a standing-jump strip always is), else
the nearest strip, and while falling onto a strip that strip
(`nav_to_landing`). The HUD's navpoint is over the same strip with the
drop in metres ([hud-kit.md](hud-kit.md#the-navpoint)). The marks are
drawn while primed; not primed, there is no chosen strip.

- **Land.** The crease as a short line along `edge`, 0.26 each way from
  `stand` (at most half the edge), with a cap across each end
  (`crease_mark`); a ring of the capsule's radius round `stand` and a
  centre dot. `Land, 3 m` beside it (the drop, `fall` in whole metres;
  `Land` alone for a standing jump), on the ground only: in the air the
  [lock-on](#lock-on) speaks. Locked, the mark turns red with the lock.
- **Jump.** On the ground, where to take off (`superbounce_world::takeoff`,
  the mode's `takeoff`): a 0.25 ring with two small feet in it, three
  chevrons toward the landing (fainter nearest the ring), and `Jump`
  with the A button's glyph, pulsing once the player is within 0.15 of
  it. For a standing jump (fall at most 0.05) it is 0.2 back from
  `stand` against the drift, and has no chevrons (the landing is under
  the ring). For a fall it is found on the map: out from `stand` toward
  the player (against the drift when the player is over the strip), in
  0.05 steps up to 4 units, the first floor at least the fall above the
  strip (less 0.05) and at most 0.5 above the player's feet
  (`superbounce_capsule::Room::floor_at`, faces looking up more than
  0.7) is the lip of the ledge to come from; the take-off is a capsule
  radius back from the lip, on its floor. No such floor, no take-off
  mark. `superbounce.state` returns it as `takeoff`.
- **Other strips.** The crease line alone, at the strip's strength (35%
  unprimed, half when the floor is too low). Past 10 units, a small
  upright diamond instead, as far crouch spots have.

The scanner cannot know steps that need an object (break the window,
crouch under the shard) or a turn before the jump; the take-off is where
the geometry says the drop starts, not a recorded recipe.

### Lock-on

While primed, airborne and falling onto a predicted landing, the crease
is a target (`superbounce_world::lock_box`, `brackets`): four corner
brackets on the floor round the point of the crease nearest the
landing, 0.3 along it and 0.2 across when closed. They open by 1.5 per
unit the landing misses the slot by (at most 0.9 wider) and move toward
the landing by half the miss (at most 0.5), so they close in as the
landing comes onto the crease. A box that appears starts 0.6 wider and
closes in.

**Locked** is the landing's `locked`: in the slot and at least the
touchdown speed the strip needs. Locked, the brackets and the landing
mark are red (`RED`, 1.0, 0.275, 0.196) with `Locked` over them, and
the reticle goes red (below). Red is the
one colour of ours: the game's red reticle means "this will hit".

**Wall clock, not frames.** The box and the lock are worked out per
guest tick from the state. Between ticks the brackets ease to each
tick's box over 0.12 s by the wall clock (`Ease`, smoothstep from what
was shown when the tick's box came), evaluated at each present's time
(`ImageMarks::visit`'s `now`), so they move the same at 30, 60 or 120
frames a second.

**The reticle.** While the lock holds
(`superbounce_hud::reticle_lock_now`, set each tick), the host draws a
red reticle over the game's own: a ring and four ticks in the lock's
red (`RED`), in screen space as a `gfx::world_lines::HudMark` from the
world-line source `superbounce_reticle::Source`
(`crates/extensions/game/src/superbounce_reticle.rs`). The ring is
11 HUD pixels in radius, about the circle of the assault and battle
rifles' reticles, with a tick at each side from 14 to 20 pixels out,
inside the 64 by 64 `rects::RETICLE` the HUD kit keeps clear. It is
centred on the first view's centre, `(320, 240)` on the fullscreen
frame, with the host's HUD fit about that point
(`render_aspect::hud_centre_fit`: the widescreen squeeze, pinned or
centred, and the first view of a split screen, as the tracker marks
follow `hud_bottom_left_fit`). It fades in and out over 0.1 s by the
wall clock (`superbounce_reticle::Fade`, smoothstep, turning around from
the level it shows), evaluated at each present's `now`, so it is smooth
at any frame rate. Nothing is drawn with the mode off.

The game's own reticle is not turned red and no guest hook is involved:
turning it red through its widget inputs needs a hook after the HUD's
widget input call (`HUD_WIDGET_INPUTS`, `0x0013ad48`), which takes the
HUD off the pipelined render thread (`render_thread.rs`). The tested
pieces of that route stay (`superbounce_hud::autoaim_entry`,
`autoaim_exit`; input 18 is the unit's "autoaimed" input by Assembly's
`nhdt` list). The player's aim-assist target (`touch_glue::on_target`)
is not written: it is input to the simulation and the network.

The colours are the game's: lines take the user's HUD colour, the
three floats at `0x00468c84` (the human triple `0x0013927e` returns),
read from the guest each frame (`superbounce_font::hud_colour`), but the
lock's red. The Elite triple and the `hudg` timer-warning colour are not
used. Labels are drawn in the disc's HUD font (font 6) with its button
glyphs, in sentence case, numbers in whole metres. No letter shape is
ours.

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
- **The arming bar.** With the same playback at 30, 60 and 120 frames a
  second, take `render.frame` shots through the primed tick: the bar at
  2/3 before it, full by 130 ms, wider and taller at about 210 ms, gone
  at 290 to 360 ms, narrowing from 430 ms and gone at 600 ms, the same
  at each rate. With the swell missing the effects hook did not fire for
  the bar (hud-kit.md, runtime step 4): the plain ending, gone at 430 ms.
- **The time left.** Set −3 with `havok.pair.set`, stand still, and read
  `havok.agents` until the pair goes: the fill reaches 0 on that tick,
  within 2. Then creep at 0.02, 0.05 and 0.1 units/s for the speed
  limit.
- **Each kept crouch spot, and each strip.** `superbounce.verify`
  ([Verifying spots](#verifying-spots)).
- **The landing report.** The recorded jump with the word at −3, at 0,
  from a lower take-off and 0.3 to the side: `bounce`, `not primed`,
  `too slow` and `missed` in turn. `superbounce.verify {"kind":"drop"}`
  with `prime` `set` and `none` gives the same verdicts from a fall.

No game was run for this section.

## Verifying spots

`superbounce.verify` tries the loaded map's scanned spots in the running
game and says which work
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type)).
It is `crates/extensions/game/src/superbounce_verify.rs`: the plan in
`plan.rs`, a trial tick by tick in `trial.rs`, the verdicts in
`summary.rs`.

**A batch.** One call plans a batch of one kind (`crouch`, `drop` or
`route`) over the scan's spots of that kind, best score first (the order
`dry_run` lists, which `spots` indexes count in; failed spots are left out
unless named or `max_spots` is given), or over explicit `points`. An
agent ticker then runs the trials one after another on one seat, a tick
at a time, because a batch runs far longer than a deferred answer may
wait. `superbounce.verify_status` reports progress and, at the end, a
verdict and one line a spot; `superbounce.verify_cancel` stops it. A
batch needs the mode on and its scan ready (`superbounce.set
{"on":true}`, then `superbounce.state`'s `scan.status`), except over
explicit points. A new map, or the game ending, fails a running batch.

**The seat.** `player.0` (the default) is driven through the first
controller's pad: the fall assist's input tap replaces the whole pad
while a trial runs, so it needs the `superbounce-assist` feature. A
brain-driven bot (by name, handle or its `player.<n>`) is driven by
holding its pad (`bots::phantom::Phantoms::hold_pad`): its brain's own
pad is ignored until the trial lets go. Trials start by teleport (the
game's own, through the agent's calls queue) and clear or set the level
pair word directly (`live_telemetry::havok_agents::set_pair_word`); they
do not use snapshots, which `state.save` refuses in a game that is not
quiescent. A teleport keeps the pair word and the velocity, so each
trial clears the word after it lands and settles before it moves.

**Crouch trials** follow the recipe: crouch, walk in, let go, walk out.
Each starts outside the lip on the approach side: `distance` out from
`stand` against the way in, which is the way out (`out`) reversed and
turned by `turn` degrees, moved by `offset`, facing in. The default grid
is turns 0, 25 and −25 by distances 0.3 and 0.6. Then:

1. settle `settle_ticks` (20) with the word written 0;
2. press crouch and stand still until the crouch is down (at least 6
   ticks, at most 20);
3. crouch-walk in at 0.6 of full stick toward a point 0.06 past `stand`,
   until within 0.08 of it (at most 60 ticks);
4. hold crouch there `hold_ticks` (15), pushing in gently;
5. let go, still pushing in, for 6 ticks; the crouch request still set
   then is `forced`;
6. walk out along `out` at full stick for `out_ticks` (30).

The word reading −3 from step 5 on primes the trial and ends it. A trial
records whether it crouched first, whether the standing capsule failed to
fit at some tick (`under`, the mode's capsule test), `forced`, how close
it got to `stand`, and the ticks from letting go to the prime and where.
A trial that did not prime fails as `not_under` (never within 0.2 of
`stand`), `not_crouched`, `not_forced` or `forced_no_word`.

**Drop trials** fall onto a strip with the word set. Each is a point in
the slot, `across` the midline toward the drift (default −0.05 to 0.15
in 0.05 steps) and `along` it (default 0), and a `height` over the
strip's stand height (default 1.5, 2.5 and 3.5). The seat is teleported
onto the point, settles with the word written (`prime`: `set` −3, `none`
0, `keep` left alone), is lifted straight up, and falls; the word is
written again each tick there is a pair. Contact is the first tick the
vertical speed is back over −0.5 after falling faster than −1; the speed
in is the tick before's, the speed out the best of the contact tick and
the three after. The landing is classified as the HUD's are
(`superbounce_charge::classify`): `bounce` (out over 8 units/s),
`not_primed`, `missed`, `too_slow` or `no_bounce`; `no_touchdown` when
it never lands within `fall_ticks` (120). `prime` `none` is the control:
a strip that launches with it is not a superbounce.

**Route trials** order a bot's whole bounce route (`bots.bounce_to`) to a
point `ROUTE_RISE` (6) units straight up from each strip's stand, or to
`to`, `repeat` times (3), and watch it to its end (`route_ticks`, 3600).
A route works when it ends `done` with a touchdown that launched; the
trial carries the route's log, its touchdown and where it stopped.
Routes write nothing to the verified file.

**The verdict.** A spot one trial worked for is `proven`; one whose
trials all ran and none worked is `failed`; else `untried` (the batch
ended first, or only the setup failed: a refused teleport, the seat
gone). The best crouch trial is the quickest prime, the best drop the
biggest launch. Each spot's line reads like `crouch #3 (-10.91, 0.72,
-1.16): primed 4/9, best offset (0.06, -0.03) yaw -121 from 0.30 out;
failed: forced_no_word 5`.

**The verified file.** A batch of the scan's spots that ends (not
cancelled) merges its proven and failed spots into
`<data>/bots/<map>.verified` (`superbounce::verified`; written by
`bots::spots::store_verified`):

```text
# superbounce verified v1
crouch proven -10.910 0.720 -1.160 dx=0.060 dy=-0.030 yaw=-121.0 trials=9 ok=4
bounce failed -10.620 3.880 -3.190 dx=0.000 dy=0.000 trials=15 ok=0 best=1.71
```

A line is the kind, the verdict and the spot's `stand` as the scan gave
it, then `key=value` fields: `dx`, `dy` (a crouch's working start, a
drop's landing point, from `stand`), `yaw` (the working way in, degrees),
`height` (the working drop height), `trials`, `ok` and `best` (the
biggest launch seen). An entry is about the spot of its kind within 0.3
of its point sideways and 0.5 up or down. The scan's readers apply it:
the mode's spots (failed ones hidden, "The superbounce game type"),
`superbounce.spots` (`status`), and the bots, which route by proven spots
first, never by failed ones, start a walking prime where the working
trial started and along its way in, and aim a drop at the working
landing point from the working height ([bots.md](bots.md)). A person can
edit the file; a later batch replaces the entries it tried.

No game has run a batch yet: the defaults (the grid, the speeds, the
tick counts) are first guesses to tune on the lab.

## Ghost runs

A ghost run is a recorded run that bounced off a spot, replayed in the
world when the player asks for it: an outline figure in the HUD's colour
walks the run, the button the run pressed shows over where it pressed
it, a dotted trail shows the path and a ring marks the touchdown. It is
`crates/extensions/game/src/superbounce_ghost.rs`; the file format is
`superbounce::ghost` (`crates/live/superbounce/src/ghost.rs`).

**Recording.** The mode's tick hands every player's reads and the medal
chain's awards to the recorder. It records the bots (phantoms; they
practise bounces while the mode is on, [bots.md](bots.md#practising-bounces))
and, while `superbounce.verify` runs, the local player's trials. Each
such player has a track of its last minute of ticks: the origin, the
aim's yaw, the vertical speed, the airborne bit, the level pair's word
and the crouch request (unit flags bit 23). A teleport (more than 3
units in a tick), a new unit or a gap in the ticks starts it again.
When the chain files a bounce ([Medals](#medals)), the track follows the
climb to its top (the first tick not rising, or 90 ticks) and makes the
run:

- it starts at the crouch that primed the word: the run of crouched
  ticks that ends at most 300 ticks before the first primed tick (or on
  it), else that first primed tick, with 30 ticks before it for the
  approach;
- it ends at the top of the bounce;
- each tick is a frame: the origin, the facing, and flags for the
  crouch request, the jump's take-off (off the ground at 1 unit/s up or
  more, before the bounce), airborne and primed;
- the spot is the strip within 0.5 of the touchdown origin, its `stand`.

The inputs come from the unit, not the pad, so every seat is recorded
the same way: the crouch request is the crouch the game acted on (a
forced crouch keeps it), and the jump is the take-off it caused.

**The best run of a spot** is kept in the map's ghosts file,
`<map>.ghosts` in the bots directory beside the `.spots` and `.verified`
files. A run that primed itself (a crouch before the touchdown) beats
one whose word was set for it (a verifier's drop); then the shorter run
wins; then the higher launch. A kept run also marks its spot proven in
the verified file ([Verifying spots](#verifying-spots)), landing offset
and launch with it, unless the spot is proven already. A ghosts file
that cannot be read (another version, a damaged line) is named in the
log and is not written over.

The file is text, versioned by its first line:

```text
# superbounce ghosts v1
ghost -10.620 3.880 -3.190 source=bot launch=12.31 touchdown=212 frames=230
f -10.910 0.720 -1.160 -121.0 c
f -10.905 0.718 -1.160 -120.5 ca
```

A `ghost` line gives the spot's `stand` (matched to a spot as a verified
entry is, within 0.3 across and 0.5 up), `source` (`bot`, `verify` or
`player`), `launch`, `touchdown` (the frame of the bounce tick) and how
many `f` lines follow. A frame is x, y, z, the facing in degrees and its
flags: `c` crouch, `j` jump, `a` airborne, `p` primed, `-` none. A run
is at most 1800 frames.

**Showing one.** The player looks at a spot that has a run: its `stand`
within 15 units of the eye (0.62 over the origin), and within 6 degrees
of the aim, or the angle a 0.4-unit mark spans when that is wider. The
prompt line says "Press (X) to show ghost". The prompt line's
recipe steps come first; the ghost's prompt replaces only "Hold (crouch)
and walk under". A press plays the run once; a press while it plays
stops it ("Press (X) to stop ghost"); a press after it ends plays
it again. Leaving the map or the mode stops it.

The button is X (`superbounce_ghost::BUTTON`, read by the fall assist's
input tap), except on the Bumper Jumper layout, where it is B
(`BUTTON_BUMPER_JUMPER`): there X is the flashlight, which drives the
[bounce view](#the-bounce-view). The tap reads the first local player's
button-layout byte from the profile (`layout_glue`'s settings record,
`+4`) every read (`publish_layout`, `button_for`); a layout it cannot
read, or any other layout, is X. The prompt shows the chosen button's
glyph: "Press (B) to show ghost" on Bumper Jumper. The fall assist reads X
only in the air, so a press counts only while the player is on the ground
(`press_counts`); a press in the air is spent, not kept for the landing,
and the two never clash. On the keyboard it is X's key.

**Playback runs on the clock.** The frames are 1/30 s apart (the guest
tick is 30 Hz at every display rate, [high-framerate.md](high-framerate.md)).
The world-line source captures the run, its start time and the viewer
with each image; each present asks for the pose at its own present time
and gets the origin and facing interpolated between the two frames
around it. At 60 or 120 frames a second each presented frame shows its
own pose, at the recorded speed. The figure holds on its last frame for
a second, then goes.

**What is drawn**, all in the HUD's colour ([Crouch spots in the
world](#crouch-spots-in-the-world)):

- the figure, an outline flat to the viewer, as tall as the standing
  capsule (as the crouched one while the run crouched), with a tick on
  the floor the way it faces;
- the glyph of each button the run pressed (A for the jump, the left
  stick for crouch) over where it pressed it, for a second of playback;
- the run's path as a dotted line, every second tick;
- a ring the capsule's size where it touched down, pulsing.

`superbounce.ghosts` lists the map's runs, the one looked at, the prompt
and the playback; `superbounce.ghost_show` plays one by its index or
stops it ([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type)).

No game has recorded or shown a run yet. To check one on the lab: turn
the mode on with bots practising, wait for `superbounce.ghosts`'s
`recorded` to count one, then `superbounce.ghost_show {"spot": 0}` and
read `playing` until `done`; the figure itself needs a windowed run.

## Medals

Eight medals: **Superbounce** for every bounce; **Double Bounce**,
**Triple Bounce**, **Quad Bounce**, **Chain Bounce** and
**Bouncetacular** for the second to the sixth bounce with no ground
between them; **Monster Bounce**, for a bounce that launches well above
a normal one; and **Bounced**, for a fall out of the map during a
bounce. They are awarded to every player in the game, bots included,
while the mode is on. The player sees a medal as Halo 2 shows its own, a
line of the notification list with a medal glyph ([hud-kit.md](hud-kit.md),
"The medals"); the film and Live count them. No announcer: the medals
are silent.

Code: the game's `crates/extensions/game/src/superbounce_chain.rs` (the chain, the
table, the reads), stepped from `superbounce_mode`'s tick; tests in
`superbounce_chain_tests.rs`.
The record and the counts: `live_proto::film::events::medals`, the
`Superbounce` and `Bounced` events in `live_proto::game`, `live-server`,
and the site (`web/src/state/derive.ts`), in [Recording](#recording).

### The rule

The mode watches every player that has a biped, each tick: the unit, its
position and velocity, the airborne bit and the level pair's word, the
fields `superbounce_charge::Tick` reads for the first local player
([the state](#the-superbounce-game-type)); `superbounce_chain::read_players`.
Sixteen pair reads a tick at most, only while the mode is on.

- **A touchdown** is the one [Landings](#landings) uses
  (`superbounce_charge::touchdown`): the tick before was airborne and
  falling at 1 unit/s or more, and this tick has stopped falling (over
  −0.5), launched, or landed. Here it counts anywhere on the map, by a
  strip or not.
- **A bounce** is a touchdown the landing classifier files as `bounce`
  (within 0.5 of a strip, out at over 8 units/s,
  `superbounce_charge::strip_landing`), with the word at −3 on the tick
  before. Every measured bounce had the word ([the touchdown
  tick](#the-touchdown-tick-trace-stepped): 0 of 4 without it), and the
  test keeps an explosion or a lift beside a strip from counting.
  The bounce names its strip, the nearest within 0.5
  (`superbounce_charge::landing_spot`), by `superbounce_chain::spot_id`,
  and a running rules script hears it as `on bounce`
  ([the game type](#the-superbounce-game-type)).
- **A Monster Bounce** is a bounce whose vertical speed comes out over
  16 units/s (`superbounce_chain::MONSTER_VZ`), the speed the landing
  classifier reads (`strip_landing`'s `vz`). Every measured launch was
  under it: 7.97 to 13.22 over the six traced bounces
  ([Traces](#traces-trace)) and 12.31 to 12.45 with the word set
  ([the pair word](#the-pair-word-trace-lab-playback)); 16 is about a
  fifth over the top of them. The number is reasoned from those, not
  measured: no Monster has been traced (**Open**). The community's
  definition (Halo Fandom wiki and Halopedia, "Super bouncing") is a
  superbounce that launches higher than a normal one, off a seam thin
  enough to fall through onto a second set of gridlines, or between
  gridlines that fan out from one point in a V; neither page gives a
  number. Geometry alone does not tell a V from seams that cross, which
  launch as a normal bounce, so the test is the launch alone. Each
  bounce's award keeps how many bounce strips were within 0.5 of the
  touchdown (`Award::strips`, `superbounce.state`'s `recent`), for
  comparing Monsters against their geometry later; nothing reads it.
- **The chain** is a count per player. A bounce while the chain is live
  adds one; a bounce while it is not makes it 1. Each bounce makes the
  chain live. It stops being live at the first of:
  - a touchdown that is not a bounce (a plain landing, or a caught
    one, which leaves the airborne bit set);
  - a tick with the airborne bit clear, from the third tick after the
    bounce on (`CLEAR_FROM`; a soft landing at the top of the flight is
    no touchdown);
  - the player's unit changing or going (a death, a respawn, a
    vehicle), or its origin moving more than 3 units in a tick
    (`TELEPORT`: a teleporter, `sim.state.set`);
  - the mode going off, or the game ending.

So "right into" the next bounce means the next thing the player's feet
do is bounce. There is no clock. Halo 2's multi-kill count has one: the
kill adds to the count when the last kill's tick is inside a window the
caller passes, and restarts it at 1 otherwise (**Code** `0x001ea733` to
`0x001ea751`; the count is the stat row's multiple-kill counter,
[live-gamestate.md](../live/live-gamestate.md)). A flight off a +12.4
launch is 6 s back to the same floor and 18 units high (gravity 4.15),
longer than a multi-kill's 4 s, and a flight cannot be stretched, so
the ground is the better test.

Medals are given as the count is reached, as the game gives its
multi-kills:

| Chain count | Medals | Id |
|---|---|---|
| 1 | Superbounce | `superbounce` |
| 2 | Superbounce, Double Bounce | `double-bounce` |
| 3 | Superbounce, Triple Bounce | `triple-bounce` |
| 4 | Superbounce, Quad Bounce | `quad-bounce` |
| 5 | Superbounce, Chain Bounce | `chain-bounce` |
| 6 | Superbounce, Bouncetacular | `bouncetacular` |
| 7 and over | Superbounce | |
| any, launched over 16 units/s | the above, and Monster Bounce | `monster-bounce` |

Superbounce is every bounce, so its count is the player's bounces. A
chain gives each of the other five once, as the count passes it: a
chain of eight is eight Superbounces and one each of the five tiers.
The table is one function, `bounce_medals(chain)`. Its canonical home is
`crates/live/live-proto/src/film/events/medals.rs`, used by the film and
Live; the site has a copy in `web/src/state/derive.ts`, as it has of the
kill medals. The game, which does not depend on Live, has its own copy
in `superbounce_chain::bounce_medals`, kept to the same ids
(`medals_by_id`), for the HUD and the agent API. Monster Bounce is not
in the count: `bounce_medals_with(chain, monster)` adds it last, so it is
the line a Monster shows, and the bounce's event carries the flag. The
live and site lists of the eight ids are `BOUNCE_MEDALS` (`medals.rs`,
`derive.ts`).

**Bounced** (`bounced`) is not on the chain's table: it is its own
event, `Event::Bounced`. It is earned when, for a player who was
attempting a bounce, the unit goes (the player's unit reads none) or is
replaced by another (a death and a respawn on one tick), and on the
tick before that:

- the unit was airborne, and
- falling at 1 unit/s or more (`TOUCHDOWN_VZ`, the touchdown's own
  test), and
- the chain was live, or the word was −3 (primed: the player was going
  for a bounce).

So the window is one tick: the last tick the unit was read, which must
be a primed or chained fall. A fall out of the map is a death in the
air, with no touchdown before it; a death on the ground, a death
climbing off a bounce, or a death of an unprimed player falling earns
none. The award's `chain` is the live chain's count, 0 if there was
none, and the chain ends. What killed the player is not read: a primed
player shot dead in a fall earns it too, as does one who drops off a
ledge while primed (supposed: that falling out of the map kills the
unit and clears the player's unit before the next tick, rather than
leaving a body in the air for some ticks). A vehicle entered in a
primed fall reads as a new unit and earns it as well; the mode's
strips are floors, where a vehicle mid-fall is rare.

The chain is pure and has no guest access of its own: a `Chain` per
player slot stepped with that player's tick (`Medals::step`), so its
tests are lists of ticks. It is not in a snapshot: after a state load
or a Theater seek a chain in flight starts again at 1.

### What is known about a second bounce

A chain needs a bounce with no crouch between it and the last one.

- The word is still −3 after the bounce tick (**Trace**, [the touchdown
  tick](#the-touchdown-tick-trace-stepped)).
- The word stayed −3 through 175 ticks of walking, a run-up and a jump
  (**Trace**, [the pair word](#the-pair-word-trace-lab-playback)), and
  nothing in the pair pass takes −3 back; only a crouch or a sleep
  rebuilds the pair at 0 (**Code**).
- So a player who neither crouches nor stands still is primed for the
  next landing, and a chain needs no new crouch. Supposed: no trace
  follows the word through a flight after a bounce. The first trace's
  bounces at ticks 11274, 11770 and 12008 were each within 28 s of the
  last crouch, which fits one crouch for several bounces; which crouch
  came before which was not recorded.
- Every measured bounce came in at −5.3 to −5.4 units/s. A landing off a
  bounce comes in at 8 to 13 and more. Whether the crease launches at
  that speed, and how hard, is **Open**: it decides whether a chain
  happens at all on the traced patch.

### On the HUD

One notification per award for a local player, to that player's view:
the line of the highest medal the award earned (`Superbounce!`, `Double
bounce!`, `Triple bounce!`, `Quad bounce!`, `Chain bounce!`,
`Bouncetacular!`, `Bounced!`, `Monster bounce!`) behind its medal glyph
(U+E2A0 to U+E2A6, and U+E2A8 for Monster Bounce, new art in the two HUD
message fonts), queued through the game's own
insert (`Handle::notify`). Halo 2 shows a multi-kill the same way,
to the earner only and through the same routine, so the line's font,
colour, place and fade are the game's. No sound. A bot's medals and
another console's players' are recorded and not shown. The glyphs, the
other choices for the picture and the preview states are in
[hud-kit.md](hud-kit.md), "The medals" and "Game events and medals".

### Recording

The game says that a bounce happened, what the chain's count was and
whether it was a Monster; the medals are worked out from those wherever
they are shown, as kill medals are worked out from kills.

- **The events.** `Superbounce`, with `tick`, `player` (the slot),
  `chain` (1 and up) and `monster` (true for a Monster Bounce; absent,
  false, otherwise), and `Bounced`, with `tick`, `player` and the `chain`
  it ended (0 if none): game events
  beside `PlayerKilled` (`live_proto::game`, with their validation: a
  player of the game, a chain of at least 1). The mode sends them for
  every player it watches through the telemetry session that sends the
  kills (`Telemetry::superbounce` and `Telemetry::bounced` in
  `crates/live/live-telemetry/src/hooks.rs`, which read the game tick and
  queue `Session::superbounce` and `Session::bounced`), on the console
  that records the game: the host, or the one console of a local game.
  `superbounce_chain::record` makes the calls for each tick's awards,
  from `superbounce_mode`'s tick with the `Telemetry` the boot passes to
  `superbounce_mode::install`; it skips a console whose simulation
  world is a synchronous or distributed client (`superbounce_chain::records`),
  where the game's own damage code, and so its kills, do not run.
  `superbounce.medal_test`'s awards are not recorded.
  Like a kill, neither is sent in a cinematic, after the end, or for a
  slot that is not in the match. A full outbox may drop them, as it drops
  kills. A guest's own HUD line comes from its own simulation of its
  players; where the two disagree the record is the host's.
- **The film.** `film::events::Tracker` takes the events as it takes a
  kill: it writes a `Superbounce` (or `Bounced`) record at the match tick,
  then a `Medal` record for each medal of that count and flag, or `bounced` (the
  `Medal` kind as it is: `player`, `earned_at`, and the id in `medal`),
  and adds them to the counts the summary's `medals` map carries
  (`medals::Tally`). Both are stats records: `Record::decode_telemetry`
  gives them to Live, and `film::privacy` keeps them in the private import
  rows. The film's version stays 3: `Medal`'s `medal` and the summary's
  map are strings, and a reader that does not know the kinds passes over
  them (**Code**: an older `Record::decode_telemetry` returns nothing for
  an unknown kind, and an older `film::privacy`, which cannot decode the
  record, keeps it private). An older film has no bounces.
  [film-format.md](film-format.md) lists the kinds.
- **Live.** `Superbounce` and `Bounced` are in `game::CLIENT_GAME_EVENTS`,
  so the host may append them and the server keeps them in the game's
  events, in both homes (`live-server`; the Worker runs the same `Live`
  over its SQL store, `live-worker/src/sqlstore.rs`). Only the host
  reports them (`games.rs`, 403 for anyone else). A film's import
  carries them like kills and checks them against the ones sent live. A
  server refuses a whole batch holding a kind it does not know, and the
  console drops a refused batch with the kills beside it, so the server
  is released before the consoles that send them. A game's snapshot
  serves its last 100 `bounces` (each with `monster` when true) and `bounced` beside its `kills`; the
  stats summary counts `bounces` and `bounced` in the totals and in an
  account's row ([live-api.md](../live/live-api.md#stats-summary)). Live
  projects the medals from them at ingest with the Tracker's projection
  (`film::events::medals::Projection`) and serves them in the game's and
  the account's `medals` ([live-api.md](../live/live-api.md#medals)). The
  site reads those for the carnage report, a game's preview, the medal
  chest and the service record; it does not count medals itself. A bot's
  bounces are in the game's record and the summary's totals, and in no
  player's row.
- **The site's pictures.** A medal's badge is the art pack's file named
  after its id (`web/public/art/medals/<id>.webp` and `@2x`, original
  art, `web/public/art/ART.md`), or the game's own medal from the
  owner's disc where the server has game art
  ([web-viewer.md](../live/web-viewer.md), `medalKey`). The disc has no
  bounce medal, so `medalKey` gives none for the eight ids; the pack's
  eight are drawn from the HUD glyphs' pixels (`scripts/superbounce-medals.py`
  makes the glyphs; the site's files are their nearest-neighbour upscale)
  and listed in `PACK_MEDALS` (`web/src/lib/art.ts`).
- **Theater.** A film played in the game runs the simulation again,
  and the mode with it when the film's variant turns it on, so the
  lines show as they did (supposed: a mode forced on by
  `H2_SUPERBOUNCE` or `superbounce.set` is not in the film). The film's
  summary carries the counts to the film list and the post-game panels
  (`theater_ui::adapter`), and the web Theater reads the `Superbounce` and
  `Bounced` records as it reads the kills, and derives their medals.

### The agent API

`superbounce.state` ([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type))
has `medals`:

| Field | Meaning |
|---|---|
| `players` | a row for each player slot with a chain or a medal this game: `player` (the slot), `chain` (the count of its last chain), `live` (whether the next bounce adds to it) and `counts` (each medal id's total this game) |
| `recent` | the last 8 awards, oldest first: `player`, `tick`, `chain`, `medals` (the ids the award earned, lowest first) and, for a bounce, `strips` (the bounce strips within 0.5 of its touchdown) |

`superbounce.medal_test {"chain": N}` (with `"monster": true` for a
Monster Bounce) or `{"bounced": true}` (access:
act) files a bounce at that count, or a Bounced, for the first local
player with a unit, after the chain's rule: the HUD line and `medals`
as a real one gives them, and nothing sent to the film or Live. It is how a run checks the line without five bounces in a
row, as `superbounce.assist_test` holds X without a pad.

### Checking the medals on the lab

Lab runs through the agent API at 30 frames a second, as
[above](#checking-it-on-the-lab):

- **The word in flight.** The recorded bounce, then `havok.agents`
  every tick to the next touchdown: the word stays −3 and the pair is
  kept at the top of the flight.
- **A second bounce.** From a bounce, steer back onto the strip
  (`superbounce.assist_test`) and read `last_landing` and `medals`:
  `bounce` and a chain of 2, or what the landing was filed as and the
  speed it came in at.
- **The airborne bit** on the bounce tick and the two after
  (`world.object`), which the chain's three-tick rule supposes.
- **What ends a chain.** A bounce, then a landing off the strip, a
  caught landing, a death and a teleport: `live` goes false on that
  tick each time.
- **Host and guest.** A guest's bounce read on both consoles: the
  host's `medals` has it at the same tick, or the record and the
  guest's HUD differ and by how much.
- **Bounced.** A primed jump off the map's edge: `recent` gets a
  `bounced` award at the tick the unit went, and `world.object` on the
  player's unit over the fall says whether the unit clears on the death
  tick or the body stays read for some ticks.
- **The line.** `superbounce.medal_test` at 1 to 7 and with `bounced`,
  with `render.frame`, against `h2 hud preview superbounce`'s `bounced`
  and `medal-` states: the glyph in its own colours, the line's place
  and fade.

No game was run for this section.

## The bounce view

The spots across the whole level, on the flashlight, and the nearby ones
on the motion tracker. Built and not run in a game. Code:
`crates/extensions/game/src/superbounce_view.rs` (the button, the pulse,
the tracker, the drawing, the agent methods),
`crates/live/superbounce/src/view.rs` (the line-of-sight test, the floors
for the map, the pairing). Tests: `superbounce_view_tests.rs` beside it,
and the tests in `view.rs`. Design mockups and the choice among them:
the map-view set of 2026-11-21 (concepts 1, 3, 4 and 6).

| Input | What shows |
|---|---|
| Nothing | Spots within the tracker's range are blips on it: a ring for a crouch spot, a filled diamond for a landing, a small chevron over or under one on a floor above or below, a ring round the navpoint's. The nearest 6 out of range are chevrons on the rim pointing at them. Hidden while the variant turns the tracker off. |
| Tap the flashlight | A scan pulse. A ring leaves the player at 40 units a second and crosses the tracker; each spot in the world lights as it passes, settles and is gone 4 s after it lit. |
| Hold the flashlight | Every spot on the level is marked in the world: a small upright ring or diamond with its distance in metres (none past 40 m), dashed and dimmer when a wall is between it and the player, a chevron over or under it for a floor above or below, and a dashed line from each crouch spot to its landing. The tracker grows to twice its size into a map: the floors from the collision within 50 m and from 2.5 units below to 1.5 above the player's feet, their outline, the spots and pairs, every proven spot out of range on its rim, turned so ahead is up. Let go and it shrinks back. |

Proven spots are drawn at full strength, likely ones at half, as the
mode's other marks. Everything is in the HUD's colour (`0x00468c84`,
`superbounce_font::hud_colour`), labels in the disc's HUD font.

**The button** is the flashlight action, not the White button: the
unit control flags at unit `+0x148`, bit `0x4`, which Halo 2 tests at
`0x000c5115` and `0x000c52be` and the converted Halo 1 scope reads
(`crates/extensions/h1-runtime/src/h1_scope_zoom.rs`). A layout that
moves the flashlight (Bumper Jumper puts it on X) moves the view with
it. The press is read, never taken from the game: on a converted Halo 1
map the light and the view come on together. A press let go within 8
ticks (0.27 s) is a tap; held 8 ticks it is a hold until let go
(`TAP_TICKS`). The ghost runs take X on the ground, and B on Bumper
Jumper, where the flashlight is X ([Ghost runs](#ghost-runs)), so a press
never does both.

**Behind a wall.** One sight line a spot, from the eye (0.62 over the
feet) to 0.25 over the spot's stand point, against every collision
polygon of the scanned structure BSP, ignoring a hit within 0.05 of
either end. The polygons are gridded in 4-unit cells once a map, on the
scan's thread (`superbounce::view::Geometry`, built with the capsule
test's `Room`). Each tick tests a few spots round the list: 6 while
marks show, 2 otherwise (`SightCache`). A mark is wholly solid or wholly
dashed; nothing cuts it at a wall's edge.

**Another floor.** A spot more than 0.9 units over or under the
player's feet is on a floor above or below (`superbounce::view::level`).

**The pairs.** A crouch spot pairs with the nearest landing within 8
units, stand to stand (`superbounce::view::pairs`).

**The map's floors** are the collision polygons whose normal looks up
more than 0.7, their outline the floor edges no other floor shares (the
foot of a wall, the lip of a drop), matched at 1 cm. Higher floors are
drawn over lower ones and brighter. Stacked floors within the band are
drawn on top of each other.

**The tracker's place** is the retail tracker's, `rects::TRACKER` of the
HUD kit (x 47..130, y 360..443 at 640 by 480 fullscreen, centre 40.5
right and 42.5 up from the HUD frame's bottom-left corner, which is read
at `0x004b9dd8`; [hud-kit.md](hud-kit.md#anchors-the-safe-area-and-scaling)).
A blip at the range's edge sits 35 HUD pixels from the centre (S: the
blip disc's own radius is not decoded). The range is the `hudg`'s
`radar_range` (`+0x290`), as the bots' radar reads it, or 25 m without
one. The host moves the HUD's bottom-left group for the window's aspect
and the HUD Position setting; the view reads that fit
(`render_aspect::hud_bottom_left_fit`, the first view's) and applies it,
so its marks stay on the tracker in Centered and Pinned. In a split
screen it follows the view at the frame's top left (S: the first local
player's). The map's growth keeps the bottom-left corner of its box on
the tracker's; it covers the shield meter while held, which stays where
it is.

The marks over the tracker are host marks, drawn by the window's mark
pass in the image's HUD space (`gfx::world_lines::HudMark`), not HUD kit
elements: the kit only places the game's own widgets, and it has none for
a polygon or a map.

**Any frame rate.** The button, the sight tests, the blips (at the
tick's yaw, so they turn with the game's own blips) and the map are made
once a 30 Hz tick, with the instants the pulse and the growth started.
What is drawn is a function of that and the `now` the window gives each
present: the same `now` gives the same picture however many presents
there are, and the pulse, the fades and the growth run on the wall clock.

**The agent API.** `superbounce.view` (read) says the mode (`none`,
`pulse`, `hold`), the button, the pulse's age and front, the growth, the
spots marked and how many are behind a wall, the tracker's blips and the
map's counts. `superbounce.view_test {"mode": "pulse"|"hold"|"none"}`
(act) drives it with no pad
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type)).

### Checking the bounce view on the lab

- **The button.** On Default, White taps a pulse and holds the map; on
  Bumper Jumper the same on X; on a converted Halo 1 map the light comes
  on too. `superbounce.view`'s `button` follows the press.
- **The tracker's place.** `render.frame` with blips and the map shown,
  4:3, 16:9 Centered and 16:9 Pinned: the blips sit on the tracker and
  turn with the game's own on a fast turn.
- **The blip disc's radius.** A bot at the edge of the tracker's range
  against a spot blip at the same distance.
- **Behind a wall.** `superbounce.view_test {"mode":"hold"}` on
  Zanzibar from a spot inside the base: `hidden` against what a look
  round says.
- **Frame time.** `profile` while held on Zanzibar.

No game was run for this section.
