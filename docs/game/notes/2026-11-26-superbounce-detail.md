# Superbounce: detail moved off the page

Evidence and detail for [superbounce.md](../superbounce.md).

## 2026-11-26: detail moved off the design page

Read at commit `c7ed1e40c`. These blocks are the page's text at that commit, verbatim, moved here when the page was cut down to its design. Reproduce with `git show c7ed1e40c:docs/game/superbounce.md`. They describe the code as it was at that commit; the page states the current design.

### Summary: full bullets

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
  ([contacts](../superbounce.md#the-contacts-on-a-bounce-tick-trace)). **Trace**
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
  ([playback](../superbounce.md#the-forced-crouch-leaves-state-trace-lab-playback),
  [pair word](../superbounce.md#the-pair-word-trace-lab-playback)). **Trace**
- A primed landing bounces when the sweep of the biped's collision agent
  is on, the swept capsule's axis goes through a floor polygon close to
  one of its edges, and the body moves across that edge
  ([the bounce rule](../superbounce.md#the-bounce-rule-code-trace)). **Code**, **Trace**
- Only the touchdown tick needs the word at −3. At 0 the pair pass widens
  its tolerance on that tick and catches the floor 2.7 cm above it. At −3
  it makes no contact, the capsule drops 14 to 23 cm into the two facing
  walls of the slot at the seam, and the solver's two opposing pushes of
  about 77,600 leave the upward part of the tilted face's normal as the
  launch ([touchdown tick](../superbounce.md#the-touchdown-tick-trace-stepped)). **Trace**

### The bounce rule: sweep timing and search agreement

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

### Forced crouch: fit bit and crouch rate

The "standing capsule fits" bit is output flags bit 1 of the character
physics update (`0x001e55d0`). After the mode handler runs it builds a
standing-height capsule at the biped (`0x002de0a0`) and tests it against
the world (`0x001c5210`, `0x001e59cd`); no hit sets the bit. This runs
every tick, airborne or not.

The crouch amount (`unit+0x2c4`) moves toward the request at the unit
definition's rate (`+0x250`, 5/s for `masterchief_mp`) in
`0x000dd7d0`, and is stored after physics in `0x000dd990`.

### The crouch height term: formula and numbers

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

### The melee-lunge hitbox offset: lunge details

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

### Traces: outcomes and the five bounces

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

### Zanzibar's window: prime, glass, landing

The user played the community's window bounce on Zanzibar on the lab
(job 0043, input recording `zanzibar-window`), read about every 12 ticks
through `superbounce.state` and `world.object`
([the note](../notes/superbounce-spots.md#2026-11-21-zanzibars-window-played)).
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
  ([collision.md](../../formats/collision.md#what-the-engine-does-with-a-bsp)).
  **[guess]** until `world.breakable` reads it in a game: the pieces' flags
  and indices are not read offline. The scan takes every surface as solid,
  so it sees the whole pane as a wall and no lip; the lip is a piece left
  above the gap after the break. The bots break the pieces in their walk
  in first and leave the one above ([bots.md](../bots.md#breaking-the-glass)).
  The way out of the `PROVEN` row is back into the room (+x), so the
  prime starts on the room's floor; the jump went out of the window,
  (−0.69, −0.72).
- **The landing.** The floor below is z 2.10 meeting a 17.6° ramp
  (normal (0, 0.3033, 0.9529)) along y −26.70, rising south to z 2.40.
  That crack is one concave crease, not two joints, so it is no
  [slot](../superbounce.md#bounce). The first try fell from z 5.5 at −4.5 units/s near
  (4.73, −26.15) and made a small pop. The second fell straight onto
  (4.876, −26.748), 5 cm on the ramp side of the crease, and went
  through the floor (z 2.86, then 0.44, then the player died).
- **A second prime.** The same session crouched at (8.07, −3.01), z
  4.51, three times. Twice the word turned −3 while walking out
  (ticks about 8071 and 8655), within 0.5 of the rest point; the first
  time it did not.

### The contacts on a bounce tick: tables and impulses

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

### Looking for state a forced crouch leaves behind: findings

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

### The forced crouch leaves state: recipe, playback, measurements

Measured 2026-11-21 on a second Mac through the lab (`h2 lab run --serve
--level ascension -- --fps 30`, [lab.md](../../runtime/lab.md)), driven by
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
| B, D | teleport to where the player stood after the second crouch, then the recorded walk and jump | 0 of 6 | none |

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

### The pair word: readings, word through playback, setting it

Measured 2026-11-21 through the lab on a build with `havok.pair.set`
([agent-api.md](../../runtime/agent-api.md#havokpairset)), same recording and
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
  world[+0x248]` ([havok-runtime.md](../../engine/havok-runtime.md)).

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
| none (as teleported, 0) | 0 of 6 | none |
| `00000000` (a write of the same value) | 0 of 1 | none |
| `fffffffd` (−3), held to the landing | 3 of 3 | +12.31 to +12.45 |
| `fffffffd`, gone by the end of the run | 0 of 1 | none |

All of them landed at (−10.25, 4.04, −3.25) at −5.4 units/s, where the
player's own bounce launched at +12.4. A teleport keeps the pair and its
word (a word written in one run was still there after the next run's
teleport), and keeps the body's velocity. A body that stands still for a
while has no level pair (it sleeps), and the pair it gets on waking
starts at 0. Those runs had nothing to write.

### The pair word: where -3 comes from

**Where −3 comes from** (Code, [havok-runtime.md](../../engine/havok-runtime.md)).
The word is the pair's tolerance level in the pair pass `FUN_00317950`.
At 0 or more it widens when contact points fall outside the tolerance and
narrows when they fit. −3 is the give-up value. `FUN_00316460` writes it
when a point is still outside after widening (`0x00316755`,
`0x00316958`), and from then on the pass runs that pair's agent at a
tolerance of −1000. Nothing re-arms −3; only −2 goes back to 0. A capsule
standing up into the corner's ceiling is deep in the geometry, which fits
the give-up path. **Code** for the paths; **Trace** that it happens on
standing up.

### The touchdown tick: tables and contacts

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

### The geometry: surfaces and capsule

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

### Spots in the geometry: input and ranking details

capsule is `masterchief_mp`'s: radius 0.175, standing 0.725, crouched
0.5, its bottom at the unit's origin. Walkable floor is `nz > 0.7`.
Polygons are bucketed on a 1-unit grid, so each test only meets its
neighbours. Bounce spots, slots and candidates together, are ranked by
score alone, best first (at most 4096, a bound no map reaches, so no
cap drops a high-scoring candidate); crouch spots are kept by
[the stricter rule](../superbounce.md#crouch-spots-the-stricter-rule).

### Bounce: geometry at Ascension and the guide's wording

collision: the joint where two collision surfaces meet. The touchdown
tick's two contacts at Ascension (y 4.105 and 3.939) are Havok's
contacts with floor edges, not with walls. The floor there is a strip
from y 3.8 to 4.0 with two joints: a concave crease at y 3.800, where
the ramp (normal z 0.8396) comes down to it, and a flat seam at y 4.000
with the next floor piece ([geometry](../superbounce.md#the-geometry-code-from-the-map)).
The two contacts straddle y 4.02 and the stepped run's origin was
y 4.04: the capsule sinks into the flat seam, between the edges of the
two coplanar pieces. A contact with the ramp would carry the ramp's
normal and none did; the bots' one learned Ascension site settled its
aim at y 3.985 ([bounce-search.md](../bounce-search.md)). So a spot's
target is the joint the pinch sits on, not the strip between: that
collision edge itself ([the target](../superbounce.md#the-target-the-collision-edge)).

The community guide's Ascension spot says "jump onto the line where the
ramp meets the ground" ([known
spots](../notes/2026-11-21-known-superbounce-spots.md)), the crease. The
capsule says otherwise: centred at y 4.02 its lower sphere (radius
0.175) reaches y 3.845, short of the crease, and however deep it sinks
its centre stays over 0.22 from the crease line, so neither the crease
nor the ramp is touched. The guide names the line a player sees: the
seam between coplanar pieces is not drawn, and the two lines are 0.2
apart, about 60 cm. Both stay targets for the sim to settle: the slot
on the seam, and the crease's own [candidate](../superbounce.md#single-creases) on
y 3.800.

### Bounce: scan rules, reported fields, merging

The scan finds every joint between two walkable polygons: edges on one
line (to 1 cm) that overlap, the overlap's ends taken as vertices (one
polygon's or the other's, as the map stores them). A joint is a seam when the two normals
agree (dot at least 0.999), a concave crease when the neighbour rises
away from the edge, and is dropped when convex. A slot is two joints
that are parallel (horizontal directions' dot at least 0.98), between
0.02 and the capsule's diameter (0.35) apart, overlapping by at least
0.05, with one floor between them (a polygon of each that is the same
or coplanar), at least one of them a crease, and the floor between
rising at most 45°. Seam and crease, two creases (a flat-bottomed
trough) and a seam up a ramp beside its foot all qualify
(`bounce.rs`, `strip`).

What it reports:

- **edge**: the target, the seam's collision edge: its two end
  vertices to the float (Ascension: (−11.10, 4.000, −3.20) to (−9.98,
  4.000, −3.20)). A trough has no seam and nothing measured: its
  sharper crease's edge.
- **stand**: the aim point, on the edge across the middle of the two
  joints' overlap, origin 0.01 above the floor (Ascension: (−10.54,
  4.00, −3.19)). A learned or partial line of the map's learned file
  moves it, see below.
- **launch**: 111.3 × the sum over the two joints of `1 − cos(dihedral)`
  (0 for a seam): the launch at the [speed threshold](../superbounce-spots.md#speed). One
  point calibrates it: Ascension's crease, ramp normal z 0.8396 against
  flat floor, gives the 17.85 units/s the body gained on the touchdown
  tick. A trough's two creases add. Strips under 3 units/s are dropped;
  estimates are capped at 60 and scored down past it.
- **drift**: across the strip, from the crease toward the other joint
  (the sharper crease of two): +y at Ascension, the way every traced
  bounce came in.
- **need**, **fall**: none (0; `null` in `--json`, `unmeasured` in the
  listing). The scan measures no speed and no drop; the game reads
  [one threshold for every seam](../superbounce-spots.md#speed). `--json` also says whether a
  spot is a `candidate`.
- **score**: 0.4, plus 0.3 × the overlap over a diameter (to 1), plus
  0.3 for a crease and a seam (the traced shape) or 0.15 for two
  creases.

Strips along one line (drift to 5°, edge across to 1 cm, height to
10 cm) that share an end vertex or overlap (to 1 cm) are merged into
one spot, so a floor split into pieces gives one edge, from the first
edge's start vertex to the last's end vertex. No gap is bridged, so the
merged edge's ends are still vertices. Its aim is the better-scored
strip's.

### Bounce: the sim's answer first, Ascension example

proves a spot ([Verifying spots](../superbounce.md#verifying-spots)). A line belongs to
one spot: of those it is about, the one whose target line passes
nearest its aim; a verified entry likewise proves only the spot whose
target line passes nearest its working landing. The game applies it where
its spot list is built (`superbounce_mode`, after the verified file);
the bots read the same aim from the line's knobs (`Site::target`). At
Ascension the learned line (standing at y 3.88, from an older scan's
stand) has its aim at (−10.45, 3.985), on the seam, and the verified
entry's landing is at y 3.976: the slot takes both, so it is the proven
strip, marked at y 3.985, and the crease stays a candidate.

### Single creases: Zanzibar window crease

The Zanzibar window landing ([Zanzibar's
window](../superbounce.md#zanzibars-window-trace-lab-play)) is one concave crease, a flat
floor meeting a 17.6° ramp along y −26.700 (x 1.80 to 6.00, z 2.10),
with no second joint, so no slot. Every concave joint between two

### Single creases: field detail, Zanzibar pair

- **edge**: the crease's collision edge, its two end vertices to the
  float (Zanzibar: (1.80, −26.70, 2.10) to (6.00, −26.70, 2.10)).
  **drift**: across it onto the flatter floor.
- **stand**: the crease's middle, on the line, origin 0.01 above it
  (Zanzibar: (3.90, −26.70, 2.11)).
- **launch**: 111.3 × its own `1 − cos(dihedral)`, the slot calibration
  with no second joint: 5.2 units/s at Zanzibar, the scale of the small
  pop its first try made. Under 3 units/s (a dihedral under about 13°)
  it is no candidate.
- **need**, **fall**: none. The one [catch](../superbounce.md#the-target-the-collision-edge)
  holds the Zanzibar landing that fell through, 4.8 cm off the edge.
- **score**: 0.3, plus 0.2 × its length over a diameter (to 1), plus
  0.1 × its crease over Ascension's (to 1): at most 0.6, under a slot of
  the traced shape. The Zanzibar window scores 0.53.

Zanzibar's window (the guide's spot 18) is a pair: the prime is the
hand-measured place under the glass shard, (5.22, −26.285, 5.11), a
`superbounce::PROVEN` place that makes a proven crouch spot the scan
can't find itself (the glass is an object), and the landing is this
crease candidate on y −26.70. The window sill over the crease's end
(x 5.20 to 5.40) covers too little of its 4.2 to drop it as
[covered](../superbounce-spots.md#covered-seams).

### Single creases: Vs, groove and converging scoring

**Vs.** Two shapes are a V (`Shape::Vee`, `shape` `vee`):

- **A groove**: two ramps meeting at the bottom, each tilted over 5° and
  rising away from the joint (their horizontal normals point at it from
  either side), Sanctuary's (`deltatap`) bounces. It scores 0.5, plus
  0.25 × its length fit, plus 0.2 × its crease fit, at most 0.95, over
  every other single crease. A ramp steepening (both floors rising the
  same way) and a flat floor at a ramp's foot are plain creases.
- **Converging creases**: two single creases (or grooves) that share an
  end vertex, the tip, at an opening under 90° (`vee::OPEN_DOT`) and not
  one line carried on: the Zanzibar V. A crease is an arm of one V at
  most, the most acute pair first. Its parts score 0.5, plus 0.45 × the
  two creases' mean fit (a crease's score over its 0.3 floor, out of
  0.3), and at least either crease's own.

The landing that counts mirrors the approach: played at Zanzibar, a
player coming in from one side of the V lands on the arm across the
bisector, not on the tip. So a V is three targets, each a candidate on

### Single creases: arm record, training, map pack maps

An arm's `arm` (`superbounce::Arm`) records the V's bisector, through
`pivot` (the tip, or the bottom line's middle) along `bisector` (into
the opening; a groove's along its bottom line), and `from`, the side the
arm is approached from: the half-plane on the far side of the bisector
(`Arm::approached_from`). The two arms' `from` are mirrors of each
other. `h2 superbounce scan` prints it (`arm, approached from (x, y)
of (pivot)`, and `arm` in `--json`), and `<map>.spots` stores it.

The sim's training flies each arm from its `from` side and once from its
own side as a control, to confirm or refute the mirror
([bounce-search.md, "Vs: the mirror and its control"](../bounce-search.md#vs-the-mirror-and-its-control)).
In play, once an arm is proven, the mode marks the arm across the
bisector from the player: a player on side A is marked arm B
(`superbounce::facing`, `superbounce_mode::for_player`). Before any
proof no part is marked, as no candidate is. Sanctuary,
Turf and Warlock are Multiplayer Map Pack maps, not on the retail disc:
their scans need the map pack's `.map` files (`deltatap.map`), and no
`deltatap.spots` exists until `h2 bots nav deltatap` runs on them.

### Single creases: Monster bounces detail

**Monster bounces.** Halopedia's "monster bounces" are a thin seam over
many grid lines, or a V of converging lines. A spot whose target line
has at least 2 other concave creases running into it, each ending on
the line and not parallel to it (`bounce.rs`, `converging`), scores at
least 0.85, 0.05 more for each further one to 0.95: ranked with the Vs.
Coplanar seams ending on the line are not counted, as a floor's
triangulation ends them on every joint; so a thin seam crossed by grid
lines is not told apart from its tessellation. The vertices where
many such lines meet are [busy points](../superbounce.md#busy-points), spots of their
own.

Creases and Vs are merged along their line like slots, and ranked with
the slots and busy points by score: no cap of their own. They are not strips to the
crouch rules: a candidate neither keeps a crouch spot "by a strip" nor
scores one up. `--explain` lists every concave crease within its radius
with its dihedral and the candidate it makes, or why none
(`superbounce::explain_bounces`), then the bounce spots there.

### Busy points: fields, score, minimums

- **edge**: the vertex twice, to the float: the vertex is the target,
  and the [catch](../superbounce.md#the-target-the-collision-edge) round it a disc.
- **stand**: on the vertex, origin 0.01 above it. **drift**: none.
- **launch**: unmeasured: the sharpest crease between its floors under
  the slot calibration (111.3 × `1 − cos`), at least 3 for a flat one.
- **need**, **fall**: none.
- **score**: by the edges and the planes that meet there. A point for
  each edge past 4 (to 6 points) and for each distinct plane past the
  first (to 3; normals' dot under 0.999), and 0.1 plus 0.05 a point:
  0.1 to 0.55, under every slot and most creases.

A busy point needs at least 4 edges and 2 points to be a spot
(`busy::MIN_POINTS`, a TODO to raise if a map lists too many): six flat
edges (a triangulated grid's vertex), five over two planes, or four over
three. A plain flat four-edge grid corner scores 0.1 and is left out, so
a floor's every vertex does not flood the list; there is no cap by
count. Busy points are candidates like creases: the sim's training flies
them, and play, the HUD and the assist leave them out until a game run
proves one.

### The target: the catch and measured values

There is no band and no width. One tolerance remains, the catch: how
far either side of the edge, horizontally, a landing's origin still
counts as on it, `superbounce::CATCH`, 0.1. It is used only for the hit
or miss verdict (`superbounce::catch_miss`: the game's landing
classifier and lock, the verifier and the bots' `missed`). The sim's
[seam drops](../../runtime/sim.md#seam-drops) measure it (`drops.json`'s
`catch`); 0.1 holds the measured Ascension origin (y 4.04,
0.04 off the seam), the bots' learned aim (y 3.985) and the second
Zanzibar window try (4.8 cm off its crease).

### The landing point: Ascension example, drift

A verified entry's own point is where the run stood, not where it
landed, so an entry with no offset on the edge gives no landing.
Ascension's proven seam: the entry stands at (−10.538, 3.880), its drop
landed (0.081, 0.096) on, and the seam's `stand` is (−10.457, 3.976),
0.024 before the edge at y 4.000. A game run's landing outranks the
sim's aim, which outranks the scan's.

A measured landing is aimed at as it is. A scan's `stand` is on the
edge, and the point to come down on is 0.1 before it along the seam's
`drift`, the side the landing comes from (`superbounce_assist::aim_at`,
`AIM_BACK`); a V's arm has its own drift, away from the side it is
approached from, so each arm's point is on its approach side. Along a
long strip the point slides to where the fall comes down, flown on the
[flight model](../superbounce-guide.md#the-flight-model) with the stick held as it is, and
keeps its place across the strip (`stand_near`).

Every recorded bounce came down moving across the seam along its drift,
at 0.3 to 1.3 units/s. A landing in the catch that comes down slower
along the drift than 0.3, still or against it, wants a push that way
(`superbounce_assist::wants_drift`, `DRIFT_MIN`): the readout's steer
line says `(arrow) Drift` in place of `On seam`. It is a hint, not a
verdict: the lock and the landing's result do not read it. The
[approach guide](../superbounce.md#the-approach-guide)'s arrow is lit at the full-launch
minimum, 0.15 units/s along the drift (`rule::DRIFT_FULL`): 0.1 can
bounce, 0.15 gives the full launch
([notes/2026-11-23-why-bounces-work.md](../notes/2026-11-23-why-bounces-work.md)).

### Covered seams: rule detail and figures

A seam under a low ceiling, indoors, can't launch anyone. Every bounce
spot, slot, crease, V or busy point, is dropped when at least half the points along
its target line (each 0.25, and both ends) have structure over them
within 5.06 of the floor (`bounce.rs`, `covered`): a column cast up from
each point, where any face that isn't vertical counts, an upper floor as
much as a ceiling. 5.06 is the climb of a typical launch, 6 units/s
(4.34 under the [speed model](../superbounce-spots.md#speed)'s gravity), plus the standing body
(0.725) (`superbounce::speed::cover`). The best measured launch, 13.6
units/s, climbs 22 units and would drop seams under any roof; most
bounces launch at 4 to 6. `--explain` names a covered crease: `covered:
structure 3.00 over it`.

### Speed: retail rule, guide posture, estimate

That estimate is not the retail rule. Near 5.4 the sweep is a band one
tick of gravity wide and the useful apex-to-floor drop is 3.35 to 3.60;
faster falls bounce by another route ([the bounce
rule](../superbounce.md#the-bounce-rule-code-trace)). The airborne HUD uses
`rule::predict`, with no 5.4-or-faster check, and the bots' launch-floor
search aims for a 3.45 drop. The navpoint, landing tracker, assist and
training score still read the estimate. `speed.rs` does not model the
threshold phase or the fast route.

The community guides' landing posture fits the physical rule: "land
vertically, no buttons", "release forward in mid-air", walk forward only
with no strafing. They control the fall and keep the capsule on the line;
none names a speed a spot needs.

No spot carries a guessed `need` or `fall`: a spot has one only when a
game run measured it. The game fills in the estimate where its spot list
is built (`superbounce::speed::apply` in `superbounce_mode`): each strip
needs 5.4 and a 2.43 drop. The bots' sites take that estimate as their
`need` (`Site::from_spot`), while their takeoff gate uses the bounce rule.

### Crouch: measured corner at Ascension

The measured forced crouch at Ascension, origin (−10.91, 0.72, −1.16),
is a corner, not a low ceiling. A small prop box (x −11.4 to −11.0,
y 0.11 to 0.61) overhangs its lower part by 5 cm, with a 45° lip
underneath (normal `(0, 0.7071, −0.7071)`, z −0.73 to −0.68). The lip's
corner is 0.142 from the capsule's axis: the standing capsule's top
reaches into it, the crouched one stays under it.

### Crouch: capsule test and region rules

height minus `r`, as the engine builds it ([crouch height
term](../superbounce.md#the-crouch-height-term-code)); its top is at 0.725 standing, the
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
rule](../superbounce.md#crouch-spots-the-stricter-rule) cuts them to the ones kept.

### Crouch: stand, out, lip, edge, score, explain

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

### The superbounce game type: chain, scoring notes

at 1, and a launch over 16 world units a second (`MONSTER_VZ`) is a
Monster, so the script's counts are the medals' (checked tick for tick
on every chain scenario, `superbounce_chain_twin.rs`). Every bounce
scores, a bounce off a seam no marked strip accounts for too. A kill
scores nothing:

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

### The superbounce game type: state, spots, bots, HUD

**State.** On or off is decided when a game starts and again every 30
ticks (so a guest's late host block counts); off, a tick costs a counter
and a flag. On, each tick reads the first local player's unit, its
origin (object `+0x64`, `superbounce_mode::OBJECT_POSITION`: the point a
teleport places and the frame of every spot's heights), how far its
bounding sphere's centre (`+0x30`, `OBJECT_CENTRE`) sits over that
origin, whether its biped is airborne (biped flags bit 0, as
`world.object`'s `physics.airborne` reads it), its velocity, the stick
it holds (the unit's throttle, turned into world axes by the aim's yaw,
for the [flight model](../superbounce-guide.md#the-flight-model)), its
aim (the vector, and as a yaw: the first-person camera looks along it),
its flashlight action and the word of its
one pair against the level (pair `+0x0c`,
`live_telemetry::havok_agents::fixed_pair_word`, which checks handles and
islands and decodes no collision-agent cache). A word of −3 is primed. It
publishes that with the nearest bounce spot; the HUD, the lines and
`superbounce.state` ([agent-api.md](../../runtime/agent-api.md#superbounce-the-superbounce-game-type))
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
([bots.md](../bots.md#practising-bounces)). The nearest
bounce spot is the one with the nearest edge.

**The HUD.** Drawn by Halo 2's own HUD through the HUD kit
([hud-kit.md](../hud-kit.md), "The superbounce HUD on the kit"), for the
first local player, in one block at the bottom left beside the motion
tracker: nine charge segments round the tracker's left, bottom and
right sides, cut from the energy sword's battery strip, which fill a
side for each of the recipe's steps and wipe back as the charge runs
out ("The charge segments" in hud-kit.md); Assault's red arming bar
over the readout's rows, filled by the recipe's steps, with a short
ending when primed
("The arming bar" in hud-kit.md); "Primed!" in the
notification list, as Assault's "Bomb armed!";
the step's verb on the game's prompt line ("Hold (crouch) and walk under", "Let go
of (crouch)", "Walk out to stand up"); and the game's navpoint over a
crouch spot while not primed, with the distance in metres, and over the
chosen strip while primed, with the drop in metres
([Floor marks](../superbounce.md#floor-marks)).
`crates/extensions/game/src/superbounce_hud.rs` registers it with the
kit and feeds it each tick from the mode's state (`view`); with the mode
off every element is hidden. Launch and drop are on the strips' labels
in the world and in the agent API, not on the HUD.

### The superbounce game type: readout notes

Nothing of the mode's HUD is in the middle of the screen but the arming
bar, at Assault's place, and the game's navpoints, which are where their
spots are. The readout's three navpoints are the seam
[targets](../superbounce.md#targets). Each row but the targets row is an
[option](../superbounce.md#the-huds-options) of the variant: the targets row answers the
player's own press.

### The superbounce game type: preview, tutorial

`h2 hud preview` draws it offline over the retail HUD, from the disc or
the synthetic map, with the kit's checks (hud-kit.md, "The preview and
the checks").

**The tutorial.** Four steps in the game's training box, two centred
lines each. The box is in the middle of the screen, so they show only
when `H2_SUPERBOUNCE_TUTORIAL=1` ([switches](../../runtime/switches.md))
asks for them:

1. Crouch, walk in under a lip, let go, then walk out.
2. You are primed. Do not crouch again.
3. Follow the navpoint. Jump from the Jump mark onto the Land mark.
4. Jump or fall onto the strip. Fall Assist (Controls) can steer.

No step asks for a button: the landing is the player's own, and the
last step names the opt-in [fall assist](../superbounce.md#hold-x-the-fall-assist) once,
by its row.

Priming moves step 1 to 2, a bounce spot within 8 m moves 2 to 3, a
bounce landing or 20 s moves 3 to 4, and a bounce or 15 s closes 4
(`hud_kit_core::huds::superbounce::Tutorial`), in game seconds: 600 and
450 ticks at any frame rate. A training text of the
game's own takes the box while it shows. Closing the last step sets the
`superbounce.tutorial_seen` setting.

### The superbounce game type: lines

- The spot is the navpoint's: not primed, the nearest crouch spot;
  primed, the chosen strip, which the
  [fall assist](../superbounce.md#hold-x-the-fall-assist) steers onto.
- No other spot has a marker: no field of rings, no lines between
  spots. While primed, each seam of the targets' set has a line on its
  collision edge ([Seam lines](../superbounce.md#seam-lines)). The whole level's spots
  are on the flashlight
  ([The bounce view](../superbounce.md#the-bounce-view)), when the player asks.
- Not primed and further than 6 world units from the next crouch spot
  (`NEAR_RANGE`), nothing is drawn in the world; the HUD's navpoint
  leads there.
- The marker does not fade with distance: it shows from across the map,
  under the HUD's navpoint.
- Lines are in the user's HUD colour and labels in the HUD's font
  (`crates/extensions/game/src/superbounce_font.rs`, below). The lock is
  the one exception: red, as the game's reticle is over an enemy in
  range.
- The marks are rebuilt each guest tick (30 Hz); what moves between
  ticks (the lock-on closing in, the pulses) moves by the wall clock at
  each present, so it is as smooth at 60 or 120 frames a second as at 30
  ([high-framerate.md](../high-framerate.md)).

### The superbounce game type: in the air

the navpoint and the tutorial step (`superbounce_hud::in_air`); the
readout's target navpoints and charge segments stay. The
world keeps the chosen strip's rings and line and the lock-on, and no
label. The mode draws no prompt over a strip:
`crates/extensions/game/src/superbounce_prompts.rs` keeps `prompt_for`'s
table (`Hold X`, `Guiding`, `Jump`, `Drop 3 m`) and its label rules,
which nothing draws.

### Targets: modes tail, switching the kinds

On a map with nothing proven or measured, `common` shows no target. The
list is classed once per scan, file change and mode.

**Switching the kinds in the game.** Two taps of the flashlight within
0.4 s (12 ticks from one let-go to the next, `DOUBLE_TICKS`) move the
mode on: common, likely, all and round again
(`superbounce_targets::Switch`, `TargetMode::next`). The variant's
option is where a game starts; the player's switch lasts that game. The
readout's targets row, over the arming bar, always says which kinds are
shown, how many seams of them the map has, and how to change them, the
flashlight's glyph twice: `Targets: Likely (4)  (W)(W)`, with X's glyph
on Bumper Jumper ([hud-kit.md](../hud-kit.md#the-superbounce-readout)). It
shows once the scan is ready and the map has a spot; `common` on a map
with nothing proven reads `Targets: Common (0)`. A switch names the new
kinds on the row at once and flashes it for two seconds. The
flashlight is the mode's one button: the game binds it on every input
set and layout, a tap is the bounce view's pulse and a hold its marks
and the tag, and after two taps a biped's light is as it was.

### Targets: navpoints

**Navpoints.** Two seams have one: the chosen strip (the mode's own,
on the ground) and the tag. The other seams of the set are
[seam lines](../superbounce.md#seam-lines), not navpoints: `sbr.target.1` and
`sbr.target.2` are the [second bounce](../superbounce-game-type.md#the-second-bounce)'s ranked
seams, in the flight after a bounce, and are empty otherwise
(`superbounce_mode::on_tick`). `superbounce_targets::choose` still
works out the two nearest seams the player can reach from their height
(`reachable`: the feet at least the seam's `fall` above its `stand`,
less 0.05), which the approach guide falls back on.

### Seam lines: table

| Seam | Line |
|---|---|
| the chosen strip | 4 px, full alpha at any distance, with a tick across each end as wide as the catch (0.1 either side) |
| any other seam of the set | 2.5 px at 0.6 alpha, fading from 25 units out to nothing at 45 |
| a seam with no length (a busy point, a V's tip) | a cross on its vertex, 0.1 each way |

A V's arm has its line only from across its bisector, as its marks.
Switching the targets' kinds changes the lines at once: the set is the
one the readout's `Targets` row names.

### The faced seam: heading, cost, kept

- **The heading** is the aim's yaw; moving over 0.5 units/s, the aim
  and the way of travel together.
- **A seam's cost** is the angle between the heading and the nearest
  point of its edge, in radians, plus 0.02 for each unit of distance
  to that point. Seams over 50 degrees off the heading or over 25 units
  away are not faced; one whose edge is within 0.5 is, whatever the
  heading. The lowest cost wins.
- **Kept.** The seam faced the tick before is 0.15 radians ahead, so
  the choice does not flicker as the aim crosses between two. With no
  seam in the cone the last one stays while it is in the set, and in
  the air it never changes: a jump keeps the seam it left for.

With no seam ever faced, the chosen strip is the nearest strip the
player's floor is high enough for (`superbounce_charge::nav_target`).

### The faced seam: the tag

**The tag.** Holding the flashlight (8 ticks, the hold that opens
[the bounce view](../superbounce.md#the-bounce-view)) while aiming at one of the mode's
seams tags it (`Tagger`): the seam nearest the aim's line from the eye,
within 6 degrees or the angle 0.4 units spans at its distance (`aimed`).
The tagged seam keeps a navpoint (`sbr.tag`) wherever the player goes,
reachable or not. Holding on the tagged seam clears the tag; a hold
aimed at no seam changes nothing; a new game clears it. While primed and
not falling onto a strip, a tagged seam that is one of the
[marked](../superbounce.md#the-superbounce-game-type) strips is the chosen strip
(`superbounce_targets::chosen`), so the mode's navpoint, the take-off,
the world marker and the [fall assist](../superbounce.md#hold-x-the-fall-assist) follow
the tag.

`superbounce.state` has the mode in effect, its count, the tag and the
marked points as `targets`
([agent-api.md](../../runtime/agent-api.md#superbounce-the-superbounce-game-type)).

### The second bounce: when, reached, ranked, shown

In the flight after a bounce the readout shows where that flight can
bounce again, the best seam first
(`crates/extensions/game/src/superbounce_second.rs`).

**When.** The first local player is in the air and their
[chain](../superbounce.md#medals) is live (`window`): from the launch to the next
touchdown, rising or falling.

**Reached.** A seam of the [targets](../superbounce.md#targets)' mode is reached
(`judge`) when:

- the flight, flown on the [flight model](../superbounce-guide.md#the-flight-model) with the
  stick held as it is, comes down to the seam's height within 8 s, at
  the speed the seam needs or faster (the touchdown speed of its last
  free row against `need_of`, the [one threshold](../superbounce-spots.md#speed) for a scanned
  strip). A seam over the apex, or just under it, is not;
- the pursuit of the seam's [landing point](../superbounce-spots.md#the-landing-point) nearest
  that landing (`superbounce_assist::stand_near`, `aim_at`;
  `superbounce_flight::pursue`) comes down in the seam's catch: the
  stick, as measured, can get it there;
- no floor under the flight's apex covers the point
  (`superbounce_assist::covered`);
- a V's arm, only from across its bisector.

**Ranked.** Each reached seam has a cost (`Ranked::cost`), lower first:
the steer's share of the stick's reach (0 to 1) plus half the seam's
need over the touchdown speed (0.5 with no speed to spare, less with
more). So a seam the flight is already coming down on outranks one that
needs steering, and of two as easy to steer onto the faster landing
wins. The two lowest are kept (`rank`), and of seams within 1 unit of
each other only the better.

**Shown**, all by the game's HUD:

- the readout's two target navpoints are on the ranked seams, the best
  on `sbr.target.1` (`over`); the tag keeps its own;
- the readout's last row lists them in order, each a rank, an arrow for
  where the seam is against the aim and its distance:
  `1 (arrow) 12 m  2 (arrow) 30 m`;
- the fall row is the best seam's line-up
  (`superbounce_readout::line_up`): the bar is the touchdown speed there
  over its need, and beside it `On seam` when the flight comes down in
  its catch with the stick held as it is, `(arrow) Drift` when it does so without the seam's
  drift, else an arrow for the way to steer to its
  [landing point](../superbounce-spots.md#the-landing-point) and how far.

With no seam reached the readout is as on any other fall. The game draws
no number on a navpoint, so the order is the row's; a navpoint's look
cannot be changed until its draw record is decoded. The variant's
`second_bounce` [option](../superbounce.md#the-huds-options) turns all of it off.
`superbounce.state` has the ranked seams as `targets.second`.

### The approach guide: marks table

| Mark | Where | Says |
|---|---|---|
| the pipper, a small ring with a cross | where the player's path touches down, on the seam's floor | in the air, the landing itself; on the ground, only while the cue counts to a jump's line, where that jump lands |
| the box, four corner brackets | on the seam's collision edge, at the point of it beside the landing point ([The landing point](../superbounce-spots.md#the-landing-point)), turned along the edge, 0.6 long and as wide as the catch (0.2): the edge runs through its middle, under the [seam line](../superbounce.md#seam-lines) | dim while the landing is off the seam, lit when it is lined up, and in a fall the phase verdict: green `Will bounce`, or `Too shallow`, `Too high`, `Off edge`, `No drift`, `Don't crouch`; under it, before the jump, the [jump cue](../superbounce.md#the-jump-cue) (`Climb up`, `Walk to line`, `Jump in 0.7 s`, green `Jump now`, `Walk off`) |
| the ticks, three dashes | from the pipper toward the box | how far off the landing is: they shorten as the two close and go when the landing is caught |

### The approach guide: which seam, when, impact

**Which seam** (`superbounce_guide::target`). The seam a fall is
predicted onto (the state's `landing`); else, while primed, the chosen
strip (the mode's navpoint: the tag, else [the faced seam](../superbounce-game-type.md#the-faced-seam)); else the nearest of the tagged seam and the
target navpoints' seams within 6 units ([Targets](../superbounce.md#targets)).

**When** (`superbounce_guide::strength`). While primed, at any
distance; not primed, never: before the charge there is nothing to land.
The pipper shows only while the
landing is within 3 units of the seam's point: the floor it is predicted
on is the seam's own, which says nothing further out. The marks are
fullscreen only, as the readout is, and the variant's `approach_guide`
[option](../superbounce.md#the-huds-options) turns them off.

**The impact** (`superbounce_guide::impact`). The body's free flight is
flown on the [flight model](../superbounce-guide.md#the-flight-model) with the stick the player
holds: each tick the fall speed gains 4.171 / 30 units/s and the
sideways velocity steps toward what the stick asks for. The flight ends
on the last tick whose next would be at or under the height the seam
stops the body at (`Seam::contact`): that tick's row is the **last free
row**, its place is the pipper's and its fall speed is the touchdown
speed. A test steps a drop from each of the fall model's heights and
gets the model's row and speed. On the ground the flight is the jump
pressed on the takeoff's line, and only while the cue counts to it
(`Takeoff::leaves_at`): the run carried to the line and 5 ticks on, 3.08
units/s up (`bots::bounce_route::JUMP_VZ`), the stick held that keeps
the run going. Otherwise there is no pipper and no ticks on the ground:
the floor underfoot is not the seam's, so a jump from anywhere else says
nothing. The flight is the body's origin (object `+0x64`), the frame of
the spots' heights; the bounding sphere's centre rides over it by the
lift the mode reads (about 0.4, `CENTRE_LIFT` when it cannot be read).
`Seam::contact` gives the height the centre is stopped at, and the
flight takes the lift off it: off the catch, the floor, with the origin
0.03 over the edge's height (`FALL_CLEARANCE`); in the catch, the
drops' measured rule, which is in the centre's frame: 0.03 over the
edge, or where the far face stops it first (**Depth**, below). No
other geometry is probed, so a landing on something else on the way is
not seen, which is why the pipper stays by the seam.

### The approach guide: depth detail

**Depth** (`superbounce_guide::depth`). The takeoff solver and the legacy
chance estimate a landing in the catch by how low the last free row ends
over the seam: low enough, and the next tick goes deep into it
([notes/2026-11-22-seam-drop-depth.md](../notes/2026-11-22-seam-drop-depth.md)).
The rule is in the seam's own frame, so it reads any seam with a drift:
with `past` the row's place past the edge along the drift (negative
before it) and `over` its height over the edge, both for the body's
bounding-sphere centre (the row's origin plus the lift: the drops'
records, which the numbers come from, are the centre's), the row is
**deep** when

```text
over − 0.75 × past < 0.445
```

and it is in the **landing window** when `past` is −0.07 to −0.01. The
three numbers are Ascension's proven seam's, measured there and nowhere
else (`SEAM_SLOPE`, `DEPTH_CUT`, `LANDING_WINDOW`): the note's cut
`z − 0.75·y < −5.755` and window y 3.93 to 3.99, against that seam's
edge at y 4.0 and z −3.2. The fall's tick phase sets the row's height,
and the phase is fixed once the body is in the air, so the depth is
known before the touchdown; about half the phases are deep. The seam
stops the body 0.135 under the cut (`DEPTH_BAND`), which is what makes a
row the last free one by the edge; that number is inferred from the
note's lowest and highest last free rows, not measured on its own.

### The approach guide: the call, state, supposed

**The call** (`superbounce_guide::phase_call`), what the box says. Each
airborne tick the mode reads the floor polygons within 4 units
(`Room::floors_near`) and runs `rule::predict` from the body's origin and
velocity with the held air stick. The `phase` keeps the verdict, the drop
from the apex to the origin standing on the deciding floor, and that
point's distance to the floor polygon's nearest edge.

| Predictor's outcome, or no landing by the seam | The box |
|---|---|
| outside the catch, or no landing | dim, off line |
| `bounce` or `bounce_fast` | green, `Will bounce`, the pipper green with it |
| `stopped` or `landing`: the sweep never came on before the floor | `Too shallow` |
| `caught`: the sweep came on too high | `Too high` |
| `pop`: through the floor away from an edge | `Off edge` |
| `level_pinch`: on an edge with too little drift across it | `No drift` |
| crouch held while primed and airborne | `Don't crouch` |
| `no_floor`, no polygons, or on the ground | lined up, no label |

On the ground the call stops at lined up: the landing is the jump's from
the line at the player's own speed, and its phase moves with every step.
There is no touchdown-speed cutoff and no percentage. The readout's bar
is full while the drop is in the 3.35 to 3.60 window and fades over the
next unit outside it; its info row says `Drop 10.5 m  Edge 0.06 m`. **Depth**, above, remains the takeoff solver's
heuristic and the agent API's legacy chance, not the airborne HUD call.

Fullscreen, while the guide is on a seam the [lock-on](../superbounce.md#lock-on)'s
brackets are not drawn (`superbounce_world::lock_shown`): the guide's
box is on the seam in their place, and its green says the fall will bounce. In
split screen the lock-on's brackets are drawn.

`superbounce.state` returns the guide as `guide`: the seam's point, the
call, the pipper, the last free row with its ticks and speed, `caught`,
`deep` with the margin under the cut and the row's place past the edge,
the speed along the drift, the fade, in a fall the [chance](../superbounce-guide.md#the-chance),
`chance` (0 to 1) and `chance_capped` (the scan-only cap cut it), and on
the ground the [jump cue](../superbounce.md#the-jump-cue): `jump_in` (ticks to `Jump
now`, while counting), `cue` (the box's words) and `takeoff` (the
takeoff, `walk` true while the cue is not a count). On the ground `deep`
is the depth of the jump from the line, an estimate that is not on the
box.

Supposed, until a lab run: that the cut and the window hold on seams
other than Ascension's and above 5.39 units/s, the fastest drop
measured; that a jump leaves the ground at 3.08 units/s with the run's
speed kept, 5 ticks after the press; where the seam stops the body
(`DEPTH_BAND`); and the chance's two softnesses and its cap. A
`superbounce.state` poll through a fall beside `sim.drops`' last free
row for the same drop checks the row and the call.

### The takeoff: lip, two flights, on the map

**The lip.** Where to look comes first from a run: the strip's kept
ghost or verified run (`superbounce_ghost::lip`, [Ghost runs](../superbounce.md#ghost-runs)),
the last ground frame before the flight that touched down, the way and
speed of its first tick in the air, and whether that tick was a jump.
Without one, or when its lip is on no floor, the map is searched
(`superbounce_world::lip`): from the strip's `stand` against its drift
first, then in a fan 30 degrees at a time round it to straight behind
(`LIP_FAN`; round the way to the player for a seam with no drift), in
0.05 steps (`TAKEOFF_STEP`) up to 4 units (`TAKEOFF_REACH`), the first
floor at least the strip's `fall` above it (less 0.05) and at most a
unit over that (`TAKEOFF_ABOVE`) is the lip, heading back at the strip.
The player's height does not limit the search, so from the seam's own
floor the ledge to climb to is found. A strip a standing jump reaches
(fall at most 0.05) takes the prompts' take-off, 0.2 back from `stand`
against the drift.

**The two flights.** The lip is put on the floor under it
(`superbounce_capsule::Room::floor_at`, within 0.1, `SAME_FLOOR`; none
there, no takeoff). Both flights are at the run's speed, the run's own,
else 0.8 units/s (`WALK_UP`, a walking approach), along the lip's
heading, flown on the [flight model](../superbounce-guide.md#the-flight-model) holding the
stick that keeps that run going (`superbounce_guide::solve`), as the
[impact](../superbounce.md#the-approach-guide) is:

- **a walk-off** from the lip, when it is an edge: the floor 0.1 past it
  (`EDGE_PROBE`) is not its own;
- **a jump** from each place back along the lip's floor, 0.005 apart
  (`TAKEOFF_STEP`), up to 4 units (`JUMP_REACH`) or the floor's end: the
  first band of them, from the lip back, whose landing is good. A press
  leaves the floor 5 ticks later (`PRESS_LEAD`, the drill runs' 4 to 6,
  [notes/2026-11-21-bot-bounce-sim.md](../notes/2026-11-21-bot-bounce-sim.md)),
  so the line is the band moved back by the ground of 5 ticks: where to
  press. Only near the landing window is every step tried: a stretch of
  0.04 (`TAKEOFF_COARSE`) whose two ends land clear of the window on the
  same side, by 0.12 (`TAKEOFF_CLEAR`), is passed over, and a test
  checks the band is the step-by-step scan's.

The one kept is the first of: a walk-off the box would call `Will
bounce`, a jump band that would, a walk-off that lands in the seam's
catch at its need (`superbounce_charge::need_of`), a jump band that
does. With none, the lip itself is kept unchecked, as the run left it,
else a walk-off from an edge and a jump from anywhere else: still the
place to go, drawn dim. A jump's flight is fixed by where it leaves the
floor, so the landing window, 0.06 wide, is a band of takeoffs just as
wide; the line is for the run's speed, and at another speed the pipper
shows where the jump comes down instead.

**On the map** (`superbounce_world::takeoff_figure`). Primed on the
ground, a bar 0.6 long across the run at the takeoff, an arrow just past
it the way to leave, and `Jump from here` or `Drop from here` on it.
While the player's floor is too low for the strip (feet under its
`fall` above `stand`, less 0.05) the pin and the HUD's navpoint
(`superbounce_mode::navpoint`, the state's `navpoint`) are on the
takeoff, and the strip has no `Land` label ([Floor marks](../superbounce.md#floor-marks)).

### The jump cue: table, count, steady

Before the jump the box says what to do, by where the player is against
the fixed takeoff (`superbounce_guide::Takeoff::cue`, `Cue`):

| Cue | When |
|---|---|
| `Climb up` | the feet are more than 0.3 (`LEVEL`) under the takeoff's floor |
| `Walk to line` | more than that over it; or on it, a jump's, and not running at its line |
| `Jump in 0.7 s` | on it, running at a jump's line: at 0.05 units/s or more along its heading (`MOVING`), at most 45 degrees off it, within 0.5 of its centre across the run (`LINE_ACROSS`), and short of the band's far end |
| `Jump now` | the same, within a tick of the band (`NOW_EARLY`) or in it |
| `Walk off` | on a walk-off's floor |

**The count.** `Jump in N`, N the time to the line's near end at the
current speed along its heading, in tenths of a second rounded up
(`Jump in 0.7 s`): at 30 ticks a second a tenth is three ticks, so the
count steps smoothly and its last value before `Jump now` is `0.1`.
`Jump now`, in the go green with the pipper, shows from one tick before
the player reaches the band until they leave it, the tolerance for the
eye and the thumb.

The cue's five words are `superbounce_guide::Cue::text`; the HUD kit's
box shows `Walk to line`, the count and `Jump now`
(`superbounce_guide::kit_cue`), and the bar on the floor says where.

**Steady** (`settle`). The cue is settled against the last tick's on
the same seam (`Guide::settled`, in the mode): a count a tenth back up,
`Jump now` against a count of `0.1`, and a count against no line or
`Walk to line` are each held for at most 3 ticks (`CUE_GRACE`), so
jitter in the player's speed does not flicker the box. A count down, a
new line, `Walk to line`, `Climb up`, `Walk off` and the end of `Jump
now` show at once; `Jump now` never outlasts its band.

### The chance: confidence table, cap

**The chance**

How sure the depth heuristic is in a fall while primed on a seam with a
drift (`superbounce_guide::chance`). This is a legacy agent-API estimate,
not the predictor's phase verdict. The HUD shows no percentage: the box
carries the verdict alone and the readout's fall row the steer text
alone. The agent API reports the chance from 0 to 1.

**In the air** the fall is all but decided, so it is a confidence
(`confidence`): how far the predicted last free row is from the two
lines that decide the bounce, against how well those lines are known.
With `margin` the row's depth under the cut over 0.02 (`DEPTH_SOFT`)
and `inside` its distance inside the nearer end of the landing window
over 0.01 (`WINDOW_SOFT`):

```text
confidence = 0.5 + 0.45 × tanh(min(margin, inside))
```

| The last free row | Confidence |
|---|---|
| deep and in the window's middle | 95% (`CONFIDENCE_TOP`) |
| 0.02 under the cut, or 0.01 inside an end of the window | 84% |
| on the cut, or on an end of the window | 50% |
| 0.003 under or over the cut, the width the cut is measured to | 57%, 43% |
| 0.05 over the cut, or 0.03 outside the window | 5 to 6% |
| outside the catch | 5% |

A fall slower than 5.2 units/s has no chance (`DEPTH_SPEED`). This cutoff
belongs only to the legacy estimate; the airborne phase verdict has none.

**Capped by evidence** (`cap`). On a seam with a measured landing
(`Spot::landed`: a verified entry's or a learned line's, [The landing
point](../superbounce-spots.md#the-landing-point)) the percentage is as worked out. On a seam
that is the scan's alone it is 60% at most (`SCAN_ONLY_CAP`): the cut
and the window are Ascension's proven seam's and are measured nowhere
else
([notes/2026-11-22-seam-drop-depth.md](../notes/2026-11-22-seam-drop-depth.md)),
so there the rule itself is a guess.

The two softnesses are inferred, not measured. The cut is found to
±0.003 and the window's low end to 0.01 (y 3.93 bounced, 3.92 did
not); how far the predicted row is from the row the game takes is not
measured, and `DEPTH_SOFT` stands in for it.

### The HUD's options: how the host reads them

The host reads them by name where it decides the mode is on, at a game's
start and every 30 ticks (`superbounce_options::read`): the active
variant's extension block (`variant_ext::in_game`), its script
reference, the script's IR (`rules::header::superbounce_options`,
`Ir::option_value`). A script that declares none of them takes the defaults.
`superbounce.state` returns the HUD's as `options`, and `pogo.state` the
stick's two. The stick's are also read by the spawn's weapon pick, from
guest memory alone ([pogo-stick.md](../pogo-stick.md#the-loadout)).

### Hold X: when, which strip, steer, button, prompt, where it sits

**When it applies.** The mode is on, the setting is on, the first local
player's X is held
(pad X, keyboard E by default; the `BUTTON` constant), the player is
airborne and falling (vertical speed at most 0.5 units/s), and a bounce spot
passes the reach cone (`judge`), each part flown on the
[flight model](../superbounce-guide.md#the-flight-model):

- it is below the player, and the flight with the stick held as it is
  comes down to its height within 3 s (`MAX_TIME`);
- its point to land on is within 3 units sideways. A strip bounces along
  its whole length, so the point is the stand point moved along the
  strip's edge to where that flight comes down, a capsule's
  radius (0.175) inside its ends (`stand_near`): a fall onto one end of a
  long strip is not pulled to its middle;
- the pursuit of the aim (`superbounce_flight::pursue`) comes down
  within 0.4 units of it (`HOLD_SLACK`): the stick, as measured, can make
  up the rest in the time left. The cone is wider the longer the fall;
- no floor is between the player and the point (`covered`): the highest
  floor under it, up to 0.5 over the player's feet
  (`superbounce_capsule::Room::floor_at`), is no more than 0.3 above it.
  A strip under a slab, a bridge or the ledge the player left is refused;
- nothing solid stands between them: the sight line from 0.3 over the
  player's origin to 0.25 over the point is clear of the scan's collision
  (`superbounce::view::Geometry`).

**Which strip.** Every bounce strip whose edge is within 6 units sideways
is judged, and of those that pass (`choose`):

1. the strip the read before steered onto, while it still passes, so the
   target never changes in the middle of a fall;
2. else the strip the HUD marks, the navpoint's and the world marker's
   ([Floor marks](../superbounce.md#floor-marks); `set_marked`, from the mode each tick),
   so the pull is toward the mark the player sees;
3. else the strip the held landing misses by least, one the fall is
   fast enough for (touchdown speed at least its `need`) before one it is
   too slow for.

`superbounce.assist` returns the choice: the reason, each strip judged
with its verdict, and the read the assist last engaged on
([agent-api.md](../../runtime/agent-api.md#superbounce-the-superbounce-game-type)).

**The steer.** The aim is the strip's
[landing point](../superbounce-spots.md#the-landing-point) (`aim_at`: a measured landing as it
is, a scan's point 0.1 units back along the spot's drift). Each read
the stick is the [flight model](../superbounce-guide.md#the-flight-model)'s pursuit of it
(`superbounce_flight::pursue`): the velocity that covers the rest of the
way in the ticks left, as a stick, in world axes. In the air the stick
asks for a velocity, not a push, so `world_to_stick` turns it as it is
into the left stick in the aim's frame (forward is `(cos yaw, sin yaw)`,
right `(sin yaw, -cos yaw)`), cut back to the unit disc. Read again each
tick, the pursuit corrects what the last one left: a simulated fall
against an air stick 0.7 to 1.4 times the measured one still lands
within 0.1 of the aim.

**The button.** While the assist is engaged X reads as released, so the
button does not reload or pick up. Once it has engaged, X stays released
until the player lets go, so a landing or a strip going out of reach does
not hand the game a fresh press. With no strip in reach, X and the stick
pass through unchanged.

**The prompt.** With the setting on, the assist's choice is also what the
world prompts are drawn from, whether or not X is held: the window reads
the player's body and makes the tap's own choice on each frame
(`choose_now`, keeping the strip the tap is steering onto), so `Hold X`
appears on the strip the stick would be steered onto before the button
is pressed, and turns to `Guiding` once the
tap reports it is engaged (`assisting`; [the prompts](../superbounce.md#the-superbounce-game-type)).
Off, there is no choice: no prompt in the air. The [lock-on](../superbounce.md#lock-on)
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

The air stick's numbers are the bots' measured ones
([The flight model](../superbounce-guide.md#the-flight-model)); the guest's own dead zone on a
replaced stick is not measured, and is the first thing to check in a lab
run.

### The flight model: air stick numbers, held, pursued

The air stick's numbers are the bots' drill runs'
(`bots::bounce_route`, [notes/2026-11-21-bot-bounce-sim.md](../notes/2026-11-21-bot-bounce-sim.md)):
0.035 units/s a tick in every run, braking and pushing, and flights held
at 0.9 to 1.3 under a full stick. So the air does not keep a run: a
run-jump at 2.0 units/s and 3.08 up, which would coast 2.87 on in the 43
ticks back to its floor, brakes to 1.2 with the stick held on (2.01 on)
and toward a stop with it let go (1.76), and a test holds those numbers.
The flight
ends on the last free row: the last tick whose next would be at or under
the surface.

- **Held** (`hold`): the stick the player holds, read from the unit's
  throttle each tick and turned into world axes by the aim's yaw
  (`held`). Without a read, the stick that keeps the velocity
  (`keep`: the velocity over 1.2, cut to the unit disc).
- **Pursued** (`pursue`): the stick each tick that heads for a point:
  the velocity that covers the rest of the way in the ticks left of a
  plumb fall to it, as a stick. It ends on the point when the stick can
  get there, and as near as it can when it cannot; that miss is what the
  fall assist and the second bounce gate on, and its first stick is the
  assist's and the landing's `steer`.

The flight is the body's origin (object `+0x64`). The bounding sphere's
centre (`+0x30`) rides about 0.4 over it and moves with the pose; the
approach guide's [depth](../superbounce.md#the-approach-guide) rule is measured on the
centre, so the guide adds the
lift the mode reads to a row before it reads the depth.

### Open: the six open items

- **The last 5 units/s.** The upward part of the two impulses on the
  touchdown tick gives 23 units/s over the body's mass; the body gains
  17.85 ([touchdown tick](../superbounce.md#the-touchdown-tick-trace-stepped)). What the
  virtual `+0x74` before the −3 write does isn't read either.
- **What drops the pair.** Entering a crouch and the body going to
  sleep both rebuild it at 0, which ends the effect. How long a stand it
  takes to sleep isn't measured.
- **60 frames a second.** The player reports no bounces at 60. That isn't
  measured here. The sweep's threshold is a speed and its length one
  tick ([the bounce rule](../superbounce.md#the-bounce-rule-code-trace)), so a 60 Hz tick
  would move every window.
- **The launch rule.** [The bounce rule](../superbounce.md#the-bounce-rule-code-trace)
  says when the two contacts pinch. How the convex agent (`0x002eeb40`)
  picks the sideways normal, and how the solver (the step at
  `0x00311100`) turns two impulses of 24,000 to 134,000 into 18.5 to 22
  units/s, are not decoded.
- **Lunges.** Whether a landing on the crease during the 1.5 s after a
  lunge (body offset from the origin) bounces differently is not
  traced.
- **Other bounce spots.** Every edge between two floor polygons bounces
  on Ascension ([the bounce rule](../superbounce.md#the-bounce-rule-code-trace)). Other
  maps and other bipeds are not measured.

### Charge: pair word amount and trace

**Charge**

The pair word has no amount. It is 0 through a crouch, forced or not,
and −3 from the first tick of the stand-up out of a forced crouch, where
the crouch lets go at the band's way out, 6 cm from where the body
rested in the measured corner (**Trace**, the note). So "how charged" is
two things the engine does have:

### Charge: how long it will last

- **How long it will last.** A primed body that stands still sleeps
  after 38 to 40 ticks and wakes with a new pair at 0 (**Trace**). The
  mode counts the ticks the body's speed is under 0.03 units/s, back to
  0 when it moves; the charge left is 1 less that count over 38. A
  crouch ends the charge at once (**Trace**), so a primed player whose
  request is set is `lost`, as is one whose word leaves −3. `lost` shows
  for 30 ticks.

### Charge: loss reason table, landing not a reason

| Reason | When |
|---|---|
| `crouched` | the crouch request is set |
| `stood_still` | the word left −3 after 30 still ticks or more (`STOOD_STILL_TICKS`): the body slept. 30 is under the 38 to 40 measured, so a sleep a few ticks early still reads as one |
| `unknown` | the word left −3 for another reason: a respawn, a vehicle, the pair gone, or a sleep the still count missed |

A landing is not a reason. Nothing measured has a landing use the
charge up: the word stays −3 after a bounce until the body sleeps (the
note), and a landing that does not bounce is filed as a landing, not a
loss.

### Charge: the two non-engine inputs, detail

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

### Crouch spots: the stricter rule, rule detail and explain output

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
   ([Zanzibar's window](../superbounce.md#zanzibars-window-trace-lab-play)).
5. **Stand at the way out.** `stand` is the region's exit sample nearest
   the nearest strip, not the sample nearest the centre: the word turns
   at the way out. The spot also carries `out` and `lip`
   ([Crouch](../superbounce-spots.md#crouch)).

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
([Verifying spots](../superbounce.md#verifying-spots)), which marks the spots that primed
`proven` and the ones that never did `failed`.

### Crouch spots in the world: marker sizes and fades

- **The marker.** One ring of the capsule's radius (0.175) round
  `stand`, 0.02 above the floor: the size of the player's own feet,
  where to be. A pin stands over it from 0.2 to 0.8 above the floor with
  `Crouch` at its top (`marker`); it fades out as the eye comes within
  2.5 units of its top and is gone within 1.5 (`near_fade`), so it never
  runs off the top of the screen. The verb with the button is on the
  HUD's prompt line.
- **Under the lip.** The ring pulses while the phase is `under` or
  `held` and the player is within 1 unit of the spot, sideways.
- **The approach arrow.** One arrowhead on the floor 0.15 outside the
  ring along `out` (`arrowhead`), pointing in toward the ring until the
  phase is `held`, then out: walk in, then walk out.

The marker appears once the player is within 6 world units of the spot
(`NEAR_RANGE`). No other crouch spot is drawn. A `likely` spot that is
next has the same marker as a `proven` one. While primed, and while
airborne, no crouch spot is drawn: a crouch now would lose the charge,
and a crouch is for the ground.

### Landings: on course, what happened, through the floor, off-spot detail

- **On course.** The state's `landing` (`superbounce_assist::predict_on`)
  is there whenever the player is airborne, rising or falling, primed or not:
  of the strips below with no floor between
  ([the assist's rule](../superbounce.md#hold-x-the-fall-assist)) and a point within
  3 units of the landing with the stick held as it is, the one it lands nearest the slot of,
  with its `verdict`
  (`OnSlot`, `Fixable` by the assist's reach gate, else `Miss`), `speed`
  and `need`, and the strip's `edge`, `stand` and `fall`. `miss` is how
  far outside the edge's catch it lands (0 when caught), and it is
  `locked` when it is `OnSlot` and `speed` is at least `need`
  (`superbounce_charge::Predicted`).
- **What happened.** A touchdown is a tick whose previous tick was
  airborne and falling at 1 unit/s or more and which has stopped falling
  (over −0.5), launched, or landed; the origin tested is the previous
  tick's, before the contact moves it. The launch is the best vertical
  speed of the touchdown tick and the 3 after
  (`superbounce_charge::LAUNCH_TICKS`, `Pending`): the push can show a
  tick or two after the contact, with the touchdown tick itself near 0.
  The landing is judged and filed when that window closes
  (`superbounce_charge::judge`), so the HUD, the steps and the medals
  only ever see the settled verdict. Within 0.5 of a strip it is
  `bounce` when the launch reaches 3.5 units/s
  (`superbounce_charge::BOUNCE_VZ`), else the first of `not primed` (the
  word was not −3 on the tick before), `missed` (the origin was outside
  the edge's catch), `too slow` (the speed in was under `need`), `no
  bounce`. Off every strip a primed launch that reaches 3.5 is a
  `bounce` all the same, and any other touchdown there files nothing.
  3.5 is the sim's seam drops' `bounce_vz`, the one value both use
  (`sim_bench::drops::BOUNCE_VZ`): over a standing jump's launch, 3.08
  (`bots::bounce_route::JUMP_VZ`), so a hop on landing is not a bounce,
  and under every measured bounce, which launch at 4 and over
  ([Covered seams](../superbounce-spots.md#covered-seams)). The HUD says it in the notification list, and
  `superbounce.state` returns the last landing as `last_landing`: the
  outcome, the touchdown's game tick and the origin.
- **Through the floor.** A primed fall can go through a seam's floor
  instead of bouncing (Zanzibar's crack, [Zanzibar's
  window](../superbounce.md#zanzibars-window-trace-lab-play); a seam on Ascension). It
  never stops falling, or stops for a tick and sinks on, so no window
  closes on it; it is filed at once as `through`
  (`superbounce_charge::through`, `LandingResult::Through`), on the
  tick the origin crosses from at or above the edge's height less 0.3
  (`SINK`) to below it, within 0.3 of the edge horizontally
  (`THROUGH_RADIUS`), moving down (the tick before falling at 1 unit/s
  or more, or a touchdown by the strip open) and primed (the word −3 on
  the tick before or on that touchdown's tick before). The height is
  the edge's at its point nearest the origin. 0.3 is deeper than any
  measured bounce's sink (0.07 to 0.23 under the floor, [Traces](../superbounce.md#traces-trace)
  and [the touchdown tick](../superbounce.md#the-touchdown-tick-trace-stepped)), so a
  bounce's dip is never one; the radius is the catch (0.1) and the
  capsule's radius (0.175), rounded, and the Zanzibar fall came down
  0.048 off its crease. A touchdown the fall follows was the seam hit:
  `through` replaces its verdict, with its tick and origin; otherwise
  the tick is the crossing's and the origin the tick before's. A
  touchdown under that floor within 0.5 of the edge after it is the
  same fall, not a landing. The speed in, the fall carried down to the
  edge's height, is on `Through::speed_in` (`Tracker::last_through`).
- **Off-spot bounces.** A primed bounce no marked strip accounts for
  (none within 0.5, or the origin outside the catch of the one there
  is) is a seam the marks don't know yet. The mode keeps the first local
  player's last 16 of the game (`superbounce_mode::OFF_SPOT_MAX`) on
  `superbounce.state`'s `off_spot`: the touchdown's tick, the origin, the
  edge of the bounce spot within 0.5 (the marked strip's, else the
  nearest of the whole scan, proven or not; absent with none), the speed
  in and the launch. They are for harvesting: the verified file is
  keyed by a scanned spot and written by runs that try it
  ([Verifying spots](../superbounce.md#verifying-spots)), so one bounce in play proves
  nothing there.

### Floor marks: land, fade, occlusion detail

- **Land.** The chosen strip has one ring of the capsule's radius
  (0.175) round `stand` and the strip's own line through it, 0.26 each
  way along `edge` (at most half the edge; `strip_bar`), which says
  which way the strip runs. On the ground, from a floor high enough, a
  pin stands over it with `Land, 3 m` (the drop, `fall` in whole metres;
  `Land` alone for a standing jump). From a floor too low, with a
  takeoff found, the pin and the navpoint are on the
  [takeoff](../superbounce.md#the-takeoff) and the strip has no label. In the air it has
  no pin and no label: the [lock-on](../superbounce.md#lock-on) is the slot's outline,
  and locked, the mark turns red with the lock.
- **Near the eye** the ring, the line and the pin fade out: full from
  2.5 units, gone within 1.5 (`near_fade`), so a ring at the feet does
  not fill the screen.
- **The guide's box.** While the [approach guide](../superbounce.md#the-approach-guide)
  is on the strip, its box is the strip's only mark: no ring, line or
  pin.
- **The takeoff.** On the ground the [takeoff](../superbounce.md#the-takeoff)'s bar,
  `Jump from here` or `Drop from here`.
- **Behind geometry.** A mark the map hides from the eye (the segment
  from the eye, 0.62 over the feet, to the mark's anchor crosses a
  collision polygon short of its last 0.15,
  `superbounce_capsule::Room::blocked`) is drawn dim, with no label
  (`superbounce_world::occlude`), so it does not read as being on the
  near surface.
- **No other strip** is drawn.

The scanner cannot know steps that need an object (break the window,
crouch under the shard) or a turn before the jump.

### Lock-on: bracket geometry, colours, reticle, font detail

four corner brackets on the floor round the point of the crease nearest the
landing, 0.3 along it and 0.2 across when closed. They open by 1.5 per
unit the landing misses the edge's catch by (at most 0.9 wider) and move toward
the landing by half the miss (at most 0.5), so they close in as the
landing comes onto the crease. A box that appears starts 0.6 wider and
closes in.

**Locked** is the landing's `locked`: within the edge's catch and at least the
touchdown speed the strip needs. Locked, the brackets and the landing
mark are red (`RED`, 1.0, 0.275, 0.196) with `Locked` over them. Red is
the one colour of ours: the game's red reticle means "this will hit".

**Wall clock, not frames.** The box and the lock are worked out per
guest tick from the state. Between ticks the brackets ease to each
tick's box over 0.12 s by the wall clock (`Ease`, smoothstep from what
was shown when the tick's box came), evaluated at each present's time
(`ImageMarks::visit`'s `now`), so they move the same at 30, 60 or 120
frames a second.

**The reticle.** Nothing is drawn over the reticle: the lock is the
brackets on the floor and the readout's fall row
(`superbounce_hud::reticle_lock_now` says whether it holds).
The game's own reticle is not turned red and no guest hook is involved:
turning it red through its widget inputs needs a hook after the HUD's
widget input call (`HUD_WIDGET_INPUTS`, `0x0013ad48`), which puts a
guest hook on the HUD. The tested
pieces of that route stay (`superbounce_hud::autoaim_entry`,
`autoaim_exit`; input 18 is the unit's "autoaimed" input by Assembly's
`nhdt` list). The player's aim-assist target (`touch_glue::on_target`)
is not written: it is input to the simulation and the network.

The colours are the game's: lines take the user's HUD colour, the
three floats at `0x00468c84` (the human triple `0x0013927e` returns),
read from the guest each frame (`superbounce_font::hud_colour`), but the
the go green. The Elite triple and the `hudg` timer-warning colour are not
used. Labels are drawn in the disc's HUD font (font 6) with its button
glyphs, in sentence case, numbers in whole metres. No letter shape is
ours.

**The font.** `superbounce_font::Capture::keep`, called by
`PackageRewrite::observing_fonts` after the `glyphs` kit
(`crates/apps/boot/src/packages.rs`), keeps `font_table.txt` and the file on
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

### Checking it on the lab: procedures

**Checking it on the lab**

Each of these is a lab run through the agent API
([agent-api.md](../../runtime/agent-api.md), [lab.md](../../runtime/lab.md)) at
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
  ([Verifying spots](../superbounce.md#verifying-spots)).
- **The landing report.** The recorded jump with the word at −3, at 0,
  from a lower take-off and 0.3 to the side: `bounce`, `not primed`,
  `too slow` and `missed` in turn. `superbounce.verify {"kind":"drop"}`
  with `prime` `set` and `none` gives the same verdicts from a fall.

No game was run for this section.

### Verifying spots: batch detail

`dry_run` lists, which `spots` indexes count in; failed spots are left out
unless named or `max_spots` is given), or over explicit `points`. An
agent ticker then runs the trials one after another on one seat, a tick
at a time, because a batch runs far longer than a deferred answer may
wait. `superbounce.verify_status` reports progress and, at the end, a
verdict and one line a spot; `superbounce.verify_cancel` stops it. A
batch needs the mode on (a game whose script requires
`hud.superbounce`) and its scan ready (`superbounce.state`'s
`scan.status`), except over
explicit points. A new map, or the game ending, fails a running batch.

### Verifying spots: the seat detail

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

### Verifying spots: crouch, drop, route trials and verdict line

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
the slot, `across` the spot's target line toward the drift (default
−0.1 to 0.1 in 0.05 steps, centred on the seam) and `along` it (default 0), and a `height` over the
strip's stand height (default 1.5, 2.5 and 3.5). The seat is teleported
onto the point, settles with the word written (`prime`: `set` −3, `none`
0, `keep` left alone), is lifted straight up, and falls; the word is
written again each tick there is a pair. Contact is the first tick the
vertical speed is back over −0.5 after falling faster than −1; the speed
in is the tick before's, the speed out the best of the contact tick and
the three after. The landing is classified as the HUD's are
(`superbounce_charge::classify`): `bounce` (out at 3.5 units/s or more),
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

### Verifying spots: verified file format

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
landing point from the working height ([bots.md](../bots.md)). A person can
edit the file; a later batch replaces the entries it tried.

No game has run a batch yet: the defaults (the grid, the speeds, the
tick counts) are first guesses to tune on the lab.

### Ghost runs: recording detail

**Recording.** The mode's tick hands every player's reads and the medal
chain's awards to the recorder. It records the bots (phantoms; they
practise bounces while the mode is on, [bots.md](../bots.md#practising-bounces))
and, while `superbounce.verify` runs, the local player's trials. Each
such player has a track of its last minute of ticks: the origin, the
aim's yaw, the vertical speed, the airborne bit, the level pair's word
and the crouch request (unit flags bit 23). A teleport (more than 3
units in a tick), a new unit or a gap in the ticks starts it again.
When the chain files a bounce ([Medals](../superbounce.md#medals)), 3 ticks after its
touchdown as its launch window closes, the track counts the flight from
the touchdown (its launch the best vertical speed since), follows the
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
- the spot is the strip within 0.5 of the touchdown origin, its `stand`;
  a bounce with no strip that near makes no run.

The inputs come from the unit, not the pad, so every seat is recorded
the same way: the crouch request is the crouch the game acted on (a
forced crouch keeps it), and the jump is the take-off it caused.

### Ghost runs: best run and file format

**The best run of a spot** is kept in the map's ghosts file,
`<map>.ghosts` in the bots directory beside the `.spots` and `.verified`
files. A run that primed itself (a crouch before the touchdown) beats
one whose word was set for it (a verifier's drop); then the shorter run
wins; then the higher launch. A kept run also marks its spot proven in
the verified file ([Verifying spots](../superbounce.md#verifying-spots)), landing offset
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

### Ghost runs: showing, playback, drawing and lab check

**Showing one.** With the feature on, only the chosen strip's run is
offered (`showable`: the navpoint's strip while primed, its run's spot
within 0.3 of the strip's `stand`). The player looks at it: its `stand`
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
[bounce view](../superbounce.md#the-bounce-view). The tap reads the first local player's
button-layout byte from the profile (`layout_glue`'s settings record,
`+4`) every read (`publish_layout`, `button_for`); a layout it cannot
read, or any other layout, is X. The prompt shows the chosen button's
glyph: "Press (B) to show ghost" on Bumper Jumper. The fall assist reads X
only in the air, so a press counts only while the player is on the ground
(`press_counts`); a press in the air is spent, not kept for the landing,
and the two never clash. On the keyboard it is X's key.

**Playback runs on the clock.** The frames are 1/30 s apart (the guest
tick is 30 Hz at every display rate, [high-framerate.md](../high-framerate.md)).
The world-line source captures the run, its start time and the viewer
with each image; each present asks for the pose at its own present time
and gets the origin and facing interpolated between the two frames
around it. At 60 or 120 frames a second each presented frame shows its
own pose, at the recorded speed. The figure holds on its last frame for
a second, then goes.

**What is drawn**: the figure alone, an outline flat to the viewer at
35% of the HUD's colour, as tall as the standing capsule (as the
crouched one while the run crouched), with a tick on the floor the way
it faces (`marks`).

`superbounce.ghosts` lists the map's runs, the one looked at, the prompt
and the playback; `superbounce.ghost_show` plays one by its index or
stops it ([agent-api.md](../../runtime/agent-api.md#superbounce-the-superbounce-game-type));
with the feature off it refuses to play one.

No game has recorded or shown a run yet. To check one on the lab: turn
the mode on with bots practising, wait for `superbounce.ghosts`'s
`recorded` to count one, then `superbounce.ghost_show {"spot": 0}` and
read `playing` until `done`; the figure itself needs a windowed run.

### The rule: touchdown, bounce detail

- **A touchdown** is the one [Landings](../superbounce-landings.md#landings) uses
  (`superbounce_charge::touchdown`): the tick before was airborne and
  falling at 1 unit/s or more, and this tick has stopped falling (over
  −0.5), launched, or landed. Here it counts anywhere on the map, by a
  strip or not.
- **A bounce** is a touchdown the landing classifier files as `bounce`
  once its launch window closes ([Landings](../superbounce-landings.md#landings): the best
  vertical speed of the touchdown tick and the 3 after reaches 3.5
  units/s, `superbounce_charge::judge`), with the word at −3 on the
  tick before. The chain holds each touchdown that could be one
  (primed, or in a live chain) for that window, so the medal comes 3
  ticks after the contact. Every measured bounce had the word ([the
  touchdown tick](../superbounce.md#the-touchdown-tick-trace-stepped): 0 of 4 without
  it), and the test keeps an explosion or a lift from counting. It
  counts wherever it happened: off a marked strip or not. A bounce by a
  strip names it, the nearest within 0.5
  (`superbounce_charge::landing_spot`), by `superbounce_chain::spot_id`,
  and a running rules script hears it as `on bounce`
  ([the game type](../superbounce.md#the-superbounce-game-type)). A bounce off every
  strip earns its medals and is recorded with no strip (`Award::spot`
  `None`); `on bounce` does not hear it, as it has no strip id to give,
  and `on launch` hears it with the strip `none`.

### The rule: Monster Bounce thresholds and community definition

  under it: 7.97 to 13.22 over the six traced bounces
  ([Traces](../superbounce.md#traces-trace)) and 12.31 to 12.45 with the word set
  ([the pair word](../superbounce.md#the-pair-word-trace-lab-playback)); 16 is about a
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

### The rule: chain end conditions

- **The chain** is a count per player. A bounce while the chain is live
  adds one; a bounce while it is not makes it 1. Each bounce makes the
  chain live. It stops being live at the first of:
  - a touchdown that is not a bounce (a plain landing, or a caught
    one, which leaves the airborne bit set);
  - a tick with the airborne bit clear, from the third tick after the
    bounce's touchdown on, the tick its window closes (`CLEAR_FROM`; a
    soft landing at the top of the flight is no touchdown);
  - a fall through a strip's floor (`through`, [Landings](../superbounce-landings.md#landings)),
    which arms a Bounced (below);
  - the player's unit changing or going (a death, a respawn, a
    vehicle), or its origin moving more than 3 units in a tick
    (`TELEPORT`: a teleporter, `sim.state.set`);
  - the mode going off, or the game ending.

### The rule: no clock, multi-kill comparison

So "right into" the next bounce means the next thing the player's feet
do is bounce. There is no clock. Halo 2's multi-kill count has one: the
kill adds to the count when the last kill's tick is inside a window the
caller passes, and restarts it at 1 otherwise (**Code** `0x001ea733` to
`0x001ea751`; the count is the stat row's multiple-kill counter,
[live-gamestate.md](../../live/live-gamestate.md)). A flight off a +12.4
launch is 6 s back to the same floor and 18 units high (gravity 4.15),
longer than a multi-kill's 4 s, and a flight cannot be stretched, so
the ground is the better test.

### The rule: medal counts and copies

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

### The rule: Bounced rules

**Bounced** (`bounced`) is not on the chain's table: it is its own
event, `Event::Bounced`. It is earned when, for a player who was
attempting a bounce, the unit goes (the player's unit reads none) or is
replaced by another (a death and a respawn on one tick), and either of
two things led to it. The first is a fall off the map: on the tick
before the unit went,

- the unit was airborne, and
- falling at 1 unit/s or more (`TOUCHDOWN_VZ`, the touchdown's own
  test), and
- the chain was live, or the word was −3 (primed: the player was going
  for a bounce).

The second is a fall through a strip's floor: the `through` of
[Landings](../superbounce-landings.md#landings) (`superbounce_charge::through`: the origin crosses
0.3 under the edge within 0.3 of it, moving down, primed or in a live
chain) arms a Bounced, and the unit going within 300 game ticks
(`superbounce_chain::BOUNCED_TICKS`, 10 s) earns it, whatever the
airborne bit, the word or the vertical speed read by then. A body that
goes through a seam ends up under the world: the level pair is gone or
rebuilt, the body slides or rests on something there with no fall
speed, and the Guardians kill it a second or more later, so the
first rule never holds on its last tick. The 10 s covers the Guardians,
a kill barrier and a fatal landing below. A bounce disarms it, as does
a teleport (`TELEPORT`), and so does living past the 300 ticks: the
player dropped through to a floor below and survived. A touchdown under
that floor within 0.5 of the edge is the same fall and is not judged.
The touchdown the fall through follows, if one was open, was the seam
hit: it is dropped, not judged. The fall through ends the chain, and the
award's `chain` is the count of the chain it ended, 0 if none was live.

So the first rule's window is one tick: the last tick the unit was read,
which must be a primed or chained fall. A fall out of the map is a death
in the air, with no touchdown before it; a death on the ground, a death
climbing off a bounce, or a death of an unprimed player falling earns
none, unless a fall through armed it. Its award's `chain` is the live
chain's count, 0 if there was none, and the chain ends. One death earns
one Bounced: where both rules hold, the armed one gives it, with the
chain it ended. What killed the player is not read: a primed
player shot dead in a fall earns it too, as does one who drops off a
ledge while primed (supposed: that falling out of the map kills the
unit and clears the player's unit before the next tick, rather than
leaving a body in the air for some ticks). A vehicle entered in a
primed fall reads as a new unit and earns it as well; the mode's
strips are floors, where a vehicle mid-fall is rare.

### The rule: chain purity (kept short)

The chain is pure and has no guest access of its own: a `Chain` per
player slot stepped with that player's tick (`Medals::step`), so its
tests are lists of ticks. It is not in a snapshot: after a state load
or a Theater seek a chain in flight starts again at 1.

### What is known about a second bounce

A chain needs a bounce with no crouch between it and the last one.

- The word is still −3 after the bounce tick (**Trace**, [the touchdown
  tick](../superbounce.md#the-touchdown-tick-trace-stepped)).
- The word stayed −3 through 175 ticks of walking, a run-up and a jump
  (**Trace**, [the pair word](../superbounce.md#the-pair-word-trace-lab-playback)), and
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

### On the HUD: line text and glyphs

the line of the highest medal the award earned (`Superbounce!`, `Double
bounce!`, `Triple bounce!`, `Quad bounce!`, `Chain bounce!`,
`Bouncetacular!`, `Bounced!`, `Monster bounce!`) behind its medal glyph
(U+E2A0 to U+E2A6, and U+E2A8 for Monster Bounce, new art in the two HUD
message fonts), queued through the game's own
insert (`Handle::notify`). Halo 2 shows a multi-kill the same way,
to the earner only and through the same routine, so the line's font,
colour, place and fade are the game's. No sound. A bot's medals and
another console's players' are recorded and not shown. The glyphs, the

### Recording: events, film, Live, site, Theater

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
  [film-format.md](../film-format.md) lists the kinds.
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
  account's row ([live-api.md](../../live/live-api.md#stats-summary)). Live
  projects the medals from them at ingest with the Tracker's projection
  (`film::events::medals::Projection`) and serves them in the game's and
  the account's `medals` ([live-api.md](../../live/live-api.md#medals)). The
  site reads those for the carnage report, a game's preview, the medal
  chest and the service record; it does not count medals itself. A bot's
  bounces are in the game's record and the summary's totals, and in no
  player's row.
- **The site's pictures.** A medal's badge is the art pack's file named
  after its id (`web/public/art/medals/<id>.webp` and `@2x`, original
  art, `web/public/art/ART.md`), or the game's own medal from the
  owner's disc where the server has game art
  ([web-viewer.md](../../live/web-viewer.md), `medalKey`). The disc has no
  bounce medal, so `medalKey` gives none for the eight ids; the pack's
  eight are drawn from the HUD glyphs' pixels (`scripts/superbounce-medals.py`
  makes the glyphs; the site's files are their nearest-neighbour upscale)
  and listed in `PACK_MEDALS` (`web/src/lib/art.ts`).
- **Theater.** A film played in the game runs the simulation again,
  and the mode with it when the film's variant turns it on, so the
  lines show as they did (supposed: a mode forced off by
  `superbounce.set` is not in the film). The film's
  summary carries the counts to the film list and the post-game panels
  (`theater_ui::adapter`), and the web Theater reads the `Superbounce` and
  `Bounced` records as it reads the kills, and derives their medals.

### The agent API: field detail

| `players` | a row for each player slot with a chain or a medal this game: `player` (the slot), `chain` (the count of its last chain), `live` (whether the next bounce adds to it) and `counts` (each medal id's total this game) |
| `recent` | the last 8 awards, oldest first: `player`, `tick`, `chain`, `medals` (the ids the award earned, lowest first) and, for a bounce, `strips` (the bounce strips within 0.5 of its touchdown) |

### The agent API: medal_test detail

`superbounce.medal_test {"chain": N}` (with `"monster": true` for a
Monster Bounce) or `{"bounced": true}` (access:
act) files a bounce at that count, or a Bounced, for the first local
player with a unit, after the chain's rule: the HUD line and `medals`
as a real one gives them, and nothing sent to the film or Live. It is how a run checks the line without five bounces in a
row, as `superbounce.assist_test` holds X without a pad.

### Checking the medals on the lab

Lab runs through the agent API at 30 frames a second, as
[above](../superbounce-landings.md#checking-it-on-the-lab):

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

### The bounce view: mockups

the map-view set of 2026-11-21 (concepts 1, 3, 4 and 6).

### The bounce view: input table detail

| Nothing | Two blips on the tracker, drawn as the game's own are, filled dots: the next spot (the navpoint's) is a dot with a ring round it, and the spot after it, the landing a next crouch spot pairs with, a smaller, fainter dot. One out of the tracker's range sits on its rim, toward the spot. No other spot is on the tracker (`tracker_blips`, `TRACKER_MARKS`). Hidden while the variant turns the tracker off. |
| Tap the flashlight | A scan pulse. A ring leaves the player at 40 units a second and crosses the tracker; each spot in the world lights as it passes, settles and is gone 4 s after it lit. |
| Hold the flashlight | Every spot on the level is marked in the world: a small upright ring or diamond with its distance in metres (none past 40 m), dashed and dimmer when a wall is between it and the player, a chevron over or under it for a floor above or below, and a dashed line from each crouch spot to its landing. The tracker grows to twice its size into a map: the floors from the collision within 50 m and from 2.5 units below to 1.5 above the player's feet, their outline, the spots and pairs, every proven spot out of range on its rim, turned so ahead is up. Let go and it shrinks back. |
| Tap the flashlight twice | Each tap pulses, and the pair switches the kinds of seam the seam lines show ([Targets](../superbounce.md#targets)). |

Proven spots are drawn at full strength, likely ones at half, as the
mode's other marks. Everything is in the HUD's colour (`0x00468c84`,
`superbounce_font::hud_colour`), labels in the disc's HUD font.

### The bounce view: the button

(`crates/extensions/h1-runtime/src/h1_scope_zoom.rs`). A layout that
moves the flashlight (Bumper Jumper puts it on X) moves the view with
it. The press is read, never taken from the game: on a converted Halo 1
map the light and the view come on together. A press let go within 8
ticks (0.27 s) is a tap; held 8 ticks it is a hold until let go
(`TAP_TICKS`). The ghost runs take X on the ground, and B on Bumper
Jumper, where the flashlight is X ([Ghost runs](../superbounce.md#ghost-runs)), so a press
never does both.

### The bounce view: wall, floors, tracker, frame rate

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
at `0x004b9dd8`; [hud-kit.md](../hud-kit.md#anchors-the-safe-area-and-scaling)).
A blip at the range's edge sits 35 HUD pixels from the centre (S: the
blip disc's own radius is not decoded). The range is the `hudg`'s
`radar_range` (`+0x290`), as the bots' radar reads it, or 25 m without
one. The host moves the HUD's bottom-left group for the window's aspect
and the HUD Position setting; the view reads that fit
(`render_aspect::hud_bottom_left_fit`, the first view's) and applies it,
so its marks stay on the tracker in Centred and Pinned. In a split
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

### Checking the bounce view on the lab

- **The button.** On Default, White taps a pulse and holds the map; on
  Bumper Jumper the same on X; on a converted Halo 1 map the light comes
  on too. `superbounce.view`'s `button` follows the press.
- **The tracker's place.** `render.frame` with blips and the map shown,
  4:3, 16:9 Centred and 16:9 Pinned: the blips sit on the tracker and
  turn with the game's own on a fast turn.
- **The blip disc's radius.** A bot at the edge of the tracker's range
  against a spot blip at the same distance.
- **Behind a wall.** `superbounce.view_test {"mode":"hold"}` on
  Zanzibar from a spot inside the base: `hidden` against what a look
  round says.
- **Frame time.** `profile` while held on Zanzibar.

No game was run for this section.

### Lines condensed on the page

Single lines whose names or values the page no longer carries.

- (`crates/live/superbounce`, `h2 superbounce scan`), and [Medals](../superbounce.md#medals),
- tick's gravity, is over 5.4465 units/s (the wrapper shape's `+0x20`,
- tick length × 0.15 (`0x004407b8`).
- (`superbounce::map_polys`). A surface's polygon is its ring by the
- line with a hit for a bounce spot, that line's aim (`ax ay`) outranks
- `crease` in `--json` and `superbounce.spots`; `bounce.rs`, `crease`).
- lower sphere rests on the floor (`r / nz − r` above it, plus 0.01). The
- `variant_ext::Decision::superbounce`, with or without the `rules`
- running rules script first: `on touchdown(p, down, up)` for a landing,
- `on launch(p, spot, speed)` for a landing judged a bounce, `on
- (`superbounce_chain::spot_id`), `chain` its count and `monster` whether
| targets | the scan is ready and the map has a spot | the kinds of seam the [targets](../superbounce.md#targets) are on, how many of them, and the flashlight's glyph twice, the double tap that switches them: `Targets: Likely (4)  (W)(W)`; it flashes for two seconds after a switch |
| speed | moving | an arrow for the way of travel against the aim and the horizontal speed in metres a second, `(arrow) 7.2 m/s`; while falling, the fall speed after it |
| fall | airborne and falling toward a strip (the state's `landing`) | a bar, the touchdown speed the fall gives over the speed the strip needs, full when fast enough; beside it `On seam` when the landing with the stick held as it is ([the flight model](../superbounce-guide.md#the-flight-model)) is caught and drifting the seam's way, `(arrow) Drift` when it is caught and is not, `Out of reach` (no arrow, the bar kept) when it is off the seam and the stick cannot bring it in (the prediction's `Course::Miss`), else an arrow for the way to push and how far the landing is from the seam's [landing point](../superbounce-spots.md#the-landing-point), `(arrow) 0.40 m`; no percentage follows |
| miss | after a landing by a strip that did not bounce: held three seconds, then until the next real fall toward a strip (falling at 1 unit/s or faster with a landing predicted), five seconds at most; a bounce takes it down at once. A hop, a jump or a step off a kerb in those three seconds leaves it, as does a landing with no line to give. Between lives its clock stops, so it stays through the respawn countdown | `Not primed`, `Missed seam by 0.31 m` (the origin's distance to the edge), `Too slow: 12.4 of 16.5 m/s` (the speed in and the strip's need), `No bounce: on seam at 17.2 m/s`, or `Fell through at 17.2 m/s`, which outranks the others: it replaces any line, and only a bounce replaces it. In the hold another miss replaces the line only with a higher speed in |
- from `crates/extensions/game/src/superbounce_world.rs` (`frame`,
- `targets`, `figures`, and the lock-on's `LockMarks`): a bold ring on the
- subset of the scan (`superbounce_mode::marked`, applied where the map's
- a strip is marked by its `verified` flag (`superbounce_mode::verified`:
| learned | the sim's training closed on it: a `learned` line with a hit in the map's learned file ([bounce-search.md](../bounce-search.md)) |
- `guide`; the state's `phase` also feeds the readout when the guide is
- (`superbounce_guide::place`, placed each tick by the mode,
- `superbounce_mode::takeoff_for`, into the state's `takeoff`).
- (`content/modes/rules/superbounce.rules`), carried in the
| `velocity` | bool, on | no speed row |
| `alignment` | bool, on | no fall row: no bar, no steer line |
| `miss_info` | bool, on | no miss row |
| `medals` | bool, on | no medal line in the notification list and no announcer line; the chain, the score and the recorded events are unchanged ([Medals](../superbounce.md#medals)) |
| `charge_segments` | bool, on | no segments round the motion tracker ([hud-kit.md](../hud-kit.md), "The charge segments"); priming is the arming bar, the prompt line and the notifications |
| `pogo` | bool, on | no Pogo Stick: the secondary weapon is the variant's own ([pogo-stick.md](../pogo-stick.md)) |
| `pogo_style` | `unsc`, `covenant` or `forerunner`; `unsc` | picks the stick's look; it has no off |
- (`superbounce.fall_assist`, off by default, a row at the end of the
- Controls list that a press turns on or off; `crates/extensions/game/src/setting_defs.rs`,
- `crates/extensions/game/src/video_screen.rs`), holding X in the air
- asks for X. The setting is read live (`follow_setting`); the
- `superbounce-assist` feature in `H2_FEATURES` stays the kill switch, which
- is `crates/extensions/game/src/superbounce_assist.rs`; `superbounce.assist`
- and `superbounce.assist_test`
- model (`crates/extensions/game/src/superbounce_flight.rs`): the approach
- 1. the fall speed gains 4.171 / 30 units/s (`FALL_GRAVITY`, the sim's
- fall model, `sim_bench::drops`);
- (`AIR_TOP`) by at most 0.035 units/s (`AIR_ACCEL`, 1.05 units/s²);
- a stick under 0.2 (`MIN_STICK`) is no stick, so a let-go stick
- one catch) is built in `crates/live/superbounce`. The state is built:
- `superbounce_mode::State` carries `charge`, `verb`,
- `nav_target`, `landing`, `last_landing`, `off_spot`, `still_ticks` and
- `standing_fits` (Code: `crates/extensions/game/src/superbounce_charge.rs`
- `superbounce_capsule.rs` for the capsule test, `superbounce_assist.rs`'s
- `predict` for the predicted landing), and `superbounce.state` returns
- "Lock-on") is built in `superbounce_world.rs`.
- reason and tick, while lost), `still_ticks`, `verb` (`LetGoOfCrouch`
- while `under`, `WalkOutToStand` while `held`, `KeepMoving`, and
- seam in its place (`superbounce_world::lock_shown`). The lock-on is
- AI-generated placeholders (`art/announcer`); how they queue with the
- `0x000c5115` and `0x000c52be` and the converted Halo 1 scope reads
- `pulse`, `hold`), the button, the pulse's age and front, the growth, the
