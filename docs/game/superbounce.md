# Superbounce

Status: Reference. Retail research with traces; nothing here is our code.

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
  stays there until the pair is rebuilt, which a teleport does. Played
  back with the player's recorded input from before the crouches, the
  landing bounced 4 times in 4. From a teleport after them, the same
  landing at the same place and speed never bounced (0 of 7). Setting the
  word to −3 on a teleported body (`havok.pair.set`) made it bounce every
  time the word held to the landing (3 of 3), at +12.3, the player's own
  launch ([playback](#the-forced-crouch-leaves-state-trace-lab-playback),
  [pair word](#the-pair-word-trace-lab-playback)). **Trace**; **Open** for
  why a negative word launches

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
player's own bounce launched at +12.4. A body that stands still for a
while after a teleport has no level pair to write (three runs of seven
found none 20 ticks after the teleport), and a pair that is rebuilt
comes back at 0.

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

## Open

- **Why a pair at −3 launches.** At −3 the pair pass runs the agent at a
  tolerance of −1000 ([pair word](#the-pair-word-trace-lab-playback)).
  One reading is that the capsule then gets no early contact points from
  the level and sinks 7 to 14 cm into the crease before the solver
  pushes it out, but which contact points the agent keeps at −1000, and
  what the virtual `+0x74` before the write does, aren't read.
- **What drops the pair.** Entering a crouch and a long stand both
  rebuild it at 0, which ends the effect. How long a stand it takes isn't
  measured.
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
  superbounce spot is a crease next to a seam is not checked.
