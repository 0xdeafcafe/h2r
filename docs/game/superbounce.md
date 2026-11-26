# Superbounce

Status: Reference, retail research with traces, except
[Spots in the geometry](#spots-in-the-geometry) (the design of our scanner),
[Medals](#medals), [Ghost runs](#ghost-runs) and
[The bounce view](#the-bounce-view) (built, not yet run in a game).

What the retail engine does when a player superbounces: what is
established, what is measured, and what is still open. The research is a
static reading of `default.xbe` (`h2 disasm`) and per-tick traces of a
player on Ascension through the agent API. Traces and measured tables are in
the [detail note](notes/2026-11-26-superbounce-detail.md).

Evidence labels: **Code** (read from the disassembly at the address
given), **Trace** (measured in a live game), **Open** (not established).

## Summary

- A superbounce is a landing on a seam in the structure collision. The
  capsule ends up 7 to 14 cm below the floor and the next tick's vertical
  velocity is +8 to +13 units/s instead of zero. **Trace**
- The response comes from the Havok step (`0x001c3d30`), not from the
  biped's character physics. **Code**
- The body is pinched below the floor between two faces looking at each
  other; the solver's two opposing impulses cancel sideways and leave the
  upward part of the tilted face's normal
  ([contacts](#the-contacts-on-a-bounce-tick-trace)). **Trace**
- A forced crouch leaves the player-vs-level pair's word (pair `+0x0c`) at
  -3, and that word makes the landing bounce
  ([playback](#the-forced-crouch-leaves-state-trace-lab-playback),
  [pair word](#the-pair-word-trace-lab-playback)). **Trace**
- A primed landing bounces when the biped's sweep is on, the swept
  capsule's axis goes through a floor polygon close to an edge, and the
  body moves across that edge
  ([the bounce rule](#the-bounce-rule-code-trace)). **Code**, **Trace**
- Only the touchdown tick needs the word at -3
  ([touchdown tick](#the-touchdown-tick-trace-stepped)). **Trace**

Detail: see the [detail note](notes/2026-11-26-superbounce-detail.md),
"Summary".

## The bounce rule (Code, Trace)

What has to be true on the tick of contact
([notes/2026-11-23-why-bounces-work.md](notes/2026-11-23-why-bounces-work.md)):

1. **The pair word is -3.** The pair pass (`0x00317950`) is the
   narrowphase. At a word of 0 or more it checks every contact point
   against the pair's allowed penetration and puts a body that is too
   deep back on the floor (`0x00316460`). At a negative word the
   allowance is -1000 and nothing is checked.
2. **The sweep is on.** The biped's agent (`0x002db2b0`) stretches the
   capsule along the tick's motion when the body's speed, with this
   tick's gravity, is over 5.4465 units/s.
3. **The swept capsule's axis goes through a floor polygon nearer one
   of its edges than the axis reaches under it.** The contact normal is
   then sideways and the polygon across the edge pushes back: the pinch of
   [the touchdown tick](#the-touchdown-tick-trace-stepped). Further from
   an edge the landing pops at +2.
4. **The body moves across the edge**, at 0.1 units/s or more. The
   normal on that side leans up by the drift over the fall speed, and
   the lean is the launch.

Any edge between two floor polygons does it, coplanar or a crease. The
sweep comes on during one tick near the threshold, which sets the drop
window (3.35 to 3.60 from the apex). The rule agrees with 98.9% of the
search's first-round trials. Timing and rates: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "The bounce rule".

`superbounce::rule::predict` steps that rule in Rust from an origin and
velocity (gravity and the held air stick each tick) and classifies the
deciding contact as `bounce`, `bounce_fast`, `caught`, `stopped`, `pop`,
`level_pinch`, `landing` or `no_floor`. Its `Verdict` carries the tick, row,
speed and, for a crossing, the polygon, edge distance, depth and speed
across the edge. `rule::edge_under` finds the nearest edge under a point.
`DRIFT_FULL` is 0.15 units/s; `DROP_MIN`, `DROP_BEST`, `DROP_MAX` are 3.35,
3.45, 3.60. The HUD, the bots' takeoff gate and the search share them.

## Forced crouch (Code)

The crouch request is unit flags (`unit+0x134`) bit 23, kept by the biped
physics update (`0x000e24f0`) at `0x000e28f4`. Pressing crouch sets it
(`0x000e291b`); releasing clears it (`0x000e2928`) only when the character
physics output says the standing capsule fits, otherwise it stays set:
forced crouch. The fit bit is output flags bit 1 of the character physics
update (`0x001e55d0`), tested every tick at `0x002de0a0`, `0x001c5210`,
`0x001e59cd`. The crouch amount (`unit+0x2c4`) moves toward the request at
the unit definition's rate (`+0x250`) in `0x000dd7d0`, stored in
`0x000dd990`.

## The crouch height term (Code)

The capsule's bottom stays at the origin; crouching shortens it from the
top (`0x000df66e`). To keep the head still, two places apply the height
change as a vertical speed (`0x000dd8e0`): `0x000e2a06` adds it before
physics (the crouch-jump) and `0x000ddaa3` subtracts it from the velocity
read back from Havok. Both apply only to a biped airborne for a few ticks
(`0x000e4050`) or in a melee lunge (`0x000e0dc0`). It was zero on every
recorded bounce tick.

## The melee-lunge hitbox offset (Code)

The Havok body is offset below the origin by biped `+0x3aa` (a tick
counter) times tick length times 0.15, applied when the game places it
(`0x001d401a`, `0x000e0d70`). A lunge started standing (`0x000e2941`)
sets it; it decays to 0 (`0x000e2870`), and a lunge that ends where the
standing capsule does not fit sets the crouch request (`0x000e0c70`,
`0x001d5120`), a second way into forced crouch. No recorded bounce had a
lunge; `world.object`'s `physics.hitbox_offset_ticks` and `hitbox_offset`
show it. Numbers: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "The melee-lunge
hitbox offset".

## Traces (Trace)

`world.object` carries a `physics` block for units (crouch, crouch
request, mode, airborne bit and ticks, raw flag words;
[agent-api.md](../runtime/agent-api.md)). Polling `object player.0` on one
`h2 agent -` connection gives one reply per tick. Of 20 landings on one
Ascension patch (`masterchief_mp`, around `(-10.5, 3.9)`), 8 landed, 7 were
caught above the floor and 5 bounced; every bounce turned the y velocity
negative and none followed a recent crouch. The per-bounce tables are in the
[detail note](notes/2026-11-26-superbounce-detail.md), "Traces".

### Zanzibar's window (Trace, lab play)

The community's window bounce was played on the lab (job 0043, recording
`zanzibar-window`). The prime is a crouch under a broken-glass lip at
(5.22, -26.285); the word turned -3 on the jump. The landing is a single
concave crease, so it is no [slot](#bounce). Both prime places are rows in
`superbounce::PROVEN`, and the crease is what
[single creases](#single-creases) look for. Walkthrough: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "Zanzibar's window".

### The contacts on a bounce tick (Trace)

`trace.start player.0` beside `havok.contacts {object: player.0}` shows two
near-horizontal contacts with the level on the bounce tick only, facing
each other (one tilted 10.6° up) with near-equal impulses. Their sideways
parts cancel and the tilt leaves the launch, so the launch is the vertical
part of a pinch, not a push off the floor. How the impulses sum to the
measured velocity is **Open**. Tables: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "The contacts on a
bounce tick".

### Looking for state a forced crouch leaves behind (Trace)

`world.raw` before, during and after a forced crouch found no differing
word in the object, the body or the shape (the shape pointer swaps between
the standing and crouching pills and returns). The candidate left was
Havok's collision-agent caches. **Open** at that point; the pair word below
settled it. Detail: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "Looking for state a
forced crouch leaves behind".

### The forced crouch leaves state (Trace, lab playback)

Run on a second Mac through the lab (`h2 lab run --serve --level ascension
-- --fps 30`, [lab.md](../runtime/lab.md)) at 30 frames a second, driven by
`h2 lab agent`. The player's recorded input (`input.record`, `trace.start`)
was played back as `input.pad` steps from a `sim.state.set` teleport.
Played back from before the forced crouches, the landing bounced 4 of 4;
from a teleport after them, the same landing never bounced (0 of 6). Pin
reads and writes to a tick (`input.pad {at_tick}`, `session.wait`).
`state.save` is refused on this build. Recipe and tables: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "The forced crouch
leaves state".

### The pair word (Trace, lab playback)

`havok.agents` lists one pair against the level for the player. Its
`pair_word` is `fffffffd` (-3) in a primed body and `00000000` in a
teleported one ([agent-api.md](../runtime/agent-api.md#havokpairset),
[havok-runtime.md](../engine/havok-runtime.md)). Setting -3 with
`havok.pair.set` on a teleported body and holding it to the landing made it
bounce 3 of 3 at +12.31 to +12.45; clearing it first gave 0 of 1.

**Where -3 comes from.** The word is the pair's tolerance level in the pair
pass `FUN_00317950`: at 0 or more it widens and narrows with the contact
points; `FUN_00316460` writes -3 when a point is still outside after
widening (`0x00316755`, `0x00316958`); nothing re-arms it, only -2 goes back
to 0. Standing up into a corner's ceiling is deep in the geometry, which
fits. **Code** for the paths; **Trace** that it happens on standing up.
Tables: see the [detail note](notes/2026-11-26-superbounce-detail.md), "The
pair word".

### The touchdown tick (Trace, stepped)

Stepped one tick at a time through the lab (job 0025, `session.pause`,
`session.step {ticks: 1}`, `havok.pair.set`), the word decides the bounce
on the touchdown tick alone. At 0 the pass widens the tolerance and catches
the floor 2.7 cm above it. At -3 it makes no contact, the capsule drops into
the slot between two walls 17 cm apart, and the solver's two opposing
pushes (about 77,600 each) leave the upward part of the tilted face as the
launch, +12.45. Tables: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "The touchdown tick".

## The geometry (Code, from the map)

The structure collision BSP of `ascension` at the patch has a floor at z
-3.2 in two coplanar pieces (surfaces 5383, 5222) meeting a ramp at y 3.8
(surfaces 5219 to 5221) in a concave crease. At the bounce positions the
capsule (radius 0.175) touches both the crease and the coplanar seam;
"caught" landings rest on the ramp side. Surface list: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "The geometry".

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
(`superbounce::map_polys`), every surface kept whatever its flags. The
capsule is `masterchief_mp`'s, walkable floor is `nz > 0.7`, and polygons
are bucketed on a 1-unit grid. Bounce spots, slots and candidates are ranked
by score, best first; crouch spots are kept by
[the stricter rule](#crouch-spots-the-stricter-rule).

### Bounce

A superbounce is a primed landing on a **seam** in the structure
collision: the joint where two collision surfaces meet. At Ascension the
touchdown contacts are Havok's contacts with floor edges, not walls, so a
spot's target is the joint the pinch sits on, not the strip between: the
collision edge itself ([the target](#the-target-the-collision-edge)).
Geometry and the guide's wording: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "Bounce". Known
spots: [notes/2026-11-21-known-superbounce-spots.md](notes/2026-11-21-known-superbounce-spots.md).

The scan (`bounce.rs`, `strip`) finds every joint between two walkable
polygons and classifies it: a seam (normals agree), a concave crease, or
convex (dropped). A slot is two parallel joints, at most a capsule
diameter (0.35) apart, with one floor between them and at least one
crease. Each slot reports:

| Field | Meaning |
|---|---|
| `edge` | the seam's collision edge, two vertices to the float (a trough: its sharper crease) |
| `stand` | the aim point on the edge, origin 0.01 above the floor |
| `launch` | 111.3 × the sum of `1 − cos(dihedral)` over the joints, at the [speed threshold](#speed); under 3 dropped, capped at 60 |
| `drift` | across the strip, from the crease toward the other joint |
| `need`, `fall` | none: the game reads [one threshold for every seam](#speed) |
| `score` | 0.4, plus 0.3 × overlap over a diameter, plus 0.3 for crease and seam (0.15 for two creases) |

Strips along one line that share an end vertex or overlap are merged into
one spot with the better-scored strip's aim. Measurements and field
detail: see the [detail note](notes/2026-11-26-superbounce-detail.md),
"Bounce".

**The sim's answer first.** A `learned` or `partial` line with a hit in
the map's learned file ([bounce-search.md](bounce-search.md)) moves
`stand` onto its aim (`superbounce::learned::apply`, applied in
`superbounce_mode`; the bots read it through `Site::target`). The `edge`
stays the collision edge, and the line proves nothing: only a primed game
run does ([Verifying spots](#verifying-spots)).

#### Single creases

Every concave joint between two walkable floors is a spot of its own
(`Shape::Crease`, `shape` `crease`; `bounce.rs`, `crease`). A single
crease is a **candidate** (`Spot::candidate`): the sim's training flies
it (`h2 sim train`), and play, the HUD and the assist leave it out until
a game run or the training proves it ([which spots are
marked](#the-superbounce-game-type)). The Zanzibar window is one
([trace](#zanzibars-window-trace-lab-play)).

- **edge** is the crease's collision edge, **stand** its middle, 0.01
  above the line, **drift** across it onto the flatter floor.
- **launch** is 111.3 × its own `1 − cos(dihedral)`; under 3 units/s it is
  no candidate. **need**, **fall**: none.
- **score** is 0.3, plus 0.2 × length over a diameter, plus 0.1 × its
  crease over Ascension's: at most 0.6.

Zanzibar's window pairs a hand-measured prime place (a
`superbounce::PROVEN` place) with the crease landing. Worked numbers: see
the [detail note](notes/2026-11-26-superbounce-detail.md), "Single
creases".

**Vs.** Two shapes are a V (`Shape::Vee`, `shape` `vee`): a groove (two
ramps meeting at the bottom, Sanctuary's `deltatap`) and converging
creases (two creases sharing a tip at an opening under 90°,
`vee::OPEN_DOT`: the Zanzibar V). A player comes in from one side and
lands on the arm across the bisector, so a V is three candidate targets
(`crates/live/superbounce/src/vee.rs`, `parts`):

| Part | `edge` | `stand` | `drift` | `arm` |
|---|---|---|---|---|
| tip (converging) | the tip vertex twice | on it | in from the opening, toward the tip | none |
| bottom line (groove) | the bottom line's two vertices | its middle | across, onto the flatter floor | none |
| arm (converging) | the crease's own two vertices | its middle | across the bisector onto the arm | set |
| arm (groove) | the bottom line's two vertices | 0.05 across it onto the ramp (`vee::ARM_STAND`) | onto its ramp | set |

An arm's `arm` (`superbounce::Arm`) records the bisector (`pivot`,
`bisector`) and `from`, the side it is approached from
(`Arm::approached_from`). `h2 superbounce scan` prints it and
`<map>.spots` stores it. Training flies each arm from its `from` side and
once from its own as a control ([bounce-search.md, "Vs: the mirror and
its control"](bounce-search.md#vs-the-mirror-and-its-control)). In play a
proven arm is marked across the bisector from the player
(`superbounce::facing`, `superbounce_mode::for_player`). Scoring and
map-pack notes: see the [detail note](notes/2026-11-26-superbounce-detail.md),
"Single creases".

**Monster bounces.** A spot whose target line has at least 2 other
concave creases running into it (`bounce.rs`, `converging`) scores at
least 0.85, to 0.95, ranked with the Vs. `--explain` lists every concave
crease in its radius and the candidate it makes
(`superbounce::explain_bounces`).

#### Busy points

A floor vertex where many collision edges meet is a spot of its own
(`Shape::Busy`, `shape` `busy`; `busy.rs`). Its `edge` is the vertex
twice, `stand` is on the vertex, `drift` and `need`/`fall` are none. It
needs at least 4 edges and 2 points (`busy::MIN_POINTS`), scores 0.1 to
0.55 (under every slot and most creases), and is a candidate like a
crease. Point rules and launch: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "Busy points".

#### The target: the collision edge

A bounce spot's target is the collision edge, and its `edge` is the two
vertices to the float. Floor marks, the aligner and the assist aim at
that line. There is no band; one tolerance remains, the catch
(`superbounce::CATCH`, 0.1): how far either side of the edge a landing's
origin still counts as on it. It decides only hit or miss
(`superbounce::catch_miss`: the landing classifier and lock, the verifier
and the bots' `missed`) and is measured by the sim's [seam
drops](../runtime/sim.md#seam-drops) (`drops.json`'s `catch`). The
measured values it holds: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "The target".

#### The landing point

A seam's `stand` is the one point everything reads: the target navpoints
and tag ([Targets](#targets)), the readout's line-up, the [second
bounce](#the-second-bounce) and the [fall
assist](#hold-x-the-fall-assist). `superbounce_mode::with_verified` sets
it from the best data the seam has:

| Data | `stand` | `Spot::landed` |
|---|---|---|
| a proven seam's verified entry whose landing is on the edge to the catch and within 0.3 of the entry's point | the entry's point plus its `dx`, `dy` (`superbounce::verified::landings`, `land`) | set |
| else a `learned` or `partial` line with a hit and an aim | the line's aim (`superbounce::learned::apply`) | set |
| else | the scan's point on the edge | clear |

A measured landing is aimed at as it is; a scan's `stand` is aimed 0.1
back along `drift` (`superbounce_assist::aim_at`, `AIM_BACK`), and along
a long strip it slides to where the fall comes down (`stand_near`, on the
[flight model](#the-flight-model)). A landing in the catch slower than
0.3 along the drift wants a push that way
(`superbounce_assist::wants_drift`, `DRIFT_MIN`): the readout says `Drift`
for `On seam`; it is a hint, not a verdict. The [approach
guide](#the-approach-guide)'s arrow is lit at 0.15 units/s
(`rule::DRIFT_FULL`), the full-launch minimum
([why](notes/2026-11-23-why-bounces-work.md)). Ascension's worked
numbers: see the [detail note](notes/2026-11-26-superbounce-detail.md),
"The landing point".

#### Covered seams

A seam under a low ceiling can't launch anyone. A bounce spot is dropped
when at least half the points along its target line have structure over
them within 5.06 of the floor (`bounce.rs`, `covered`;
`superbounce::speed::cover`): the climb of a 6 units/s launch plus the
standing body (0.725). `--explain` names a covered crease. Worked
numbers: see the [detail note](notes/2026-11-26-superbounce-detail.md),
"Covered seams".

#### Speed

`crates/live/superbounce/src/speed.rs` keeps a single-speed estimate for
the assist, landing tracker and training scores: 5.4 units/s
(`BOUNCE_SPEED`) and a 2.43 drop to reach it (`speed::fall`, gravity
4.15); `speed::launch` adds the excess over 5.4. The sim's [seam
drops](../runtime/sim.md#seam-drops) measure the real speed and gain
(`drops.json`'s `bounce_speed`, `gain`).

The estimate is not the retail rule ([the bounce
rule](#the-bounce-rule-code-trace)). The airborne HUD uses
`rule::predict`, and the bots' launch-floor search aims for a 3.45 drop;
the navpoint, landing tracker, assist and training score still read the
estimate. No spot carries a guessed `need` or `fall`: the game fills in
the estimate where its spot list is built (`superbounce::speed::apply` in
`superbounce_mode`), and the bots' sites take it as `need`
(`Site::from_spot`), while their takeoff gate uses the bounce rule.
Guide posture and the band figures: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "Speed".

### Crouch

The forced crouch at Ascension is a corner under a prop's 45° lip, not a
low ceiling. The scan tests the capsule itself: candidate origins sit on
a 4 cm lattice over walkable floor near overhead geometry, the capsule's
segment is built as the engine builds it ([crouch height
term](#the-crouch-height-term-code)), and a sample where the standing
capsule hits and the crouched one doesn't is a forced crouch. Its top is
0.725 standing, the height `crates/live/bots/src/nav.rs` `standing_clear`
checks. Contact rules: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "Crouch".

Neighbouring forced samples form a region, and a region is a spot only
when it is at least 2 samples, touches standing room (its exits), is
reachable from open floor, and is at least 4 cm wide at one exit. There
is no length rule. The [stricter rule](#crouch-spots-the-stricter-rule)
cuts the spots to the ones kept.

- **stand**: the exit sample nearest the nearest bounce spot's stand
  (else the region's centre).
- **out**: the unit direction from `stand` to the standing room beside
  it.
- **lip**: up to 8 edges of the faces the standing capsule hits
  (`superbounce::Lip`).
- **edge**: across the largest group of exits that stand the same way.
- **score**: 0.4, plus width, length and nearness to a bounce stand
  terms; a proven spot scores 1. `fall`, `launch`, `need` and `drift` are
  0.

`--explain x,y,z[,r]` (`superbounce::explain`) prints every forced-crouch
region near a point with each filter's verdict. Score formula and explain
output: see the [detail note](notes/2026-11-26-superbounce-detail.md),
"Crouch".

## The superbounce game type

A built-in variant, Superbounce, for practising: Bounce Practice's rules
(no score or time limit, 3 s respawn, overshields, damage resistance, a
magnum) plus a HUD and lines over the map. It is listed under **Research
and Devolution** and is a Slayer variant underneath
([community-gametypes.md](community-gametypes.md#tweakers)). Its entry is
built from one of `rules::builtin::LISTED`
(`crates/extensions/game/src/community_variants.rs`,
[rules-variants.md](rules-variants.md#built-in-list-entries)), and
`community_variants::write_blocks` writes its block when the list loads.
The script is `content/modes/rules/superbounce.rules`: `type research`
files it, `requires hud.superbounce` turns the mode on
([rules-engine.md](rules-engine.md#firefight-ai-and-bounces)). It keeps
the bounce chain on what the medal chain ([Medals](#medals)) saw
(`on launch`, `on grounded`); a launch over 16 units/s (`MONSTER_VZ`) is
a Monster. The counts match the medals', checked tick for tick by
`superbounce_chain_twin.rs`. A kill scores nothing:

| Event | Points, by default | Option |
|---|---|---|
| a bounce | `bounce_points` (1) times its place in its chain, at most 6 | `bounce_points`, 0 to 5 |
| a Monster Bounce | twice the bounce's, after the cap | `monster_double`, on |
| a Bounced | −1, once (the suicide point if `suicide_costs` is on) | `bounced_costs`, on |
| a suicide | −1 | `suicide_costs`, on |
| a kill | nothing | none |

The cap is Bouncetacular's count. Worked scores and the Bounced
assumption: see the [detail note](notes/2026-11-26-superbounce-detail.md),
"The superbounce game type".

The mode is on only when the variant's extension block names a rules
script that requires `hud.superbounce`
(`rules::header::script_asks_superbounce`, in
`variant_ext::Decision::superbounce`). The built-in Superbounce and
Bounce Practice scripts do; no switch or name does. An agent can force
the mode off with `superbounce.set`, never on
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type)).
Code: `crates/extensions/game/src/superbounce_mode.rs`.

Each tick, what the medal chain saw goes to a running rules script first
(`on touchdown`, `on launch`, `on grounded`;
[rules-world-api.md](rules-world-api.md#physics)), then each bounce on a
marked strip goes as `on bounce(p, spot, chain, monster)` (`spot` from
`superbounce_chain::spot_id`) and each Bounced as `on bounced(p)`.

**State.** On or off is decided at game start and every 30 ticks. On,
each tick reads the first local player's unit (origin at object `+0x64`,
`OBJECT_POSITION`; sphere centre `+0x30`, `OBJECT_CENTRE`; airborne flag;
velocity; stick; aim; flashlight) and its pair word against the level
(`live_telemetry::havok_agents::fixed_pair_word`; −3 is primed). It
publishes that with the nearest bounce spot; the HUD, the lines and
`superbounce.state` read it. Field detail: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "State".

**Spots.** The first tick on a map and structure BSP starts a background
scan (`superbounce::scan_in`, `superbounce::map_context`), cached per map
name, BSP and header checksum. The map's verified file ("Verifying
spots") is applied to the result: a `proven` spot scores 1, a `failed`
one is hidden from the HUD, lines and overlay but kept for
`superbounce.verify`. The feature is presentation-only and
snapshot-stateless.

**The bots.** While the mode is on the bots practise bounces over the
map's sites, proven ones first ([bots.md](bots.md#practising-bounces)).

**The HUD.** Halo 2's own HUD through the HUD kit
([hud-kit.md](hud-kit.md), "The superbounce HUD on the kit"), for the
first local player, bottom left beside the motion tracker: charge
segments, an arming bar, "Primed!", the step's prompt line, and a
navpoint over the crouch spot (not primed) or the chosen strip (primed)
([Floor marks](#floor-marks)).
`crates/extensions/game/src/superbounce_hud.rs` registers it and feeds it
from the mode's state (`view`); with the mode off every element is
hidden. Element detail: see the [detail note](notes/2026-11-26-superbounce-detail.md),
"The HUD".

**The readout.** Three rows right of the motion tracker and a fourth over
the arming bar, fullscreen only
([hud-kit.md](hud-kit.md#the-superbounce-readout);
`crates/extensions/game/src/superbounce_readout.rs`):

| Row | Shows when | Says |
|---|---|---|
| targets | the scan is ready and the map has a spot | the kinds of seam the [targets](#targets) are on, their count, and the flashlight's double tap that switches them |
| speed | moving | the way of travel and horizontal speed in m/s |
| fall | airborne and falling toward a strip | a bar of touchdown speed over the strip's need, and `On seam`, `Drift`, `Out of reach` or an arrow and distance to the [landing point](#the-landing-point) |
| miss | after a landing by a strip that did not bounce | `Not primed`, `Missed seam by ...`, `Too slow: ...`, `No bounce: on seam at ...` or `Fell through at ...` |

Each row but the targets row is an [option](#the-huds-options) of the
variant. Hold times and replacement rules: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "The readout".

```sh
h2 hud preview superbounce --state all --mode all --out DIR
```

`h2 hud preview` draws it offline over the retail HUD with the kit's
checks (hud-kit.md, "The preview and the checks").

**The tutorial.** Four steps in the game's training box, shown only with
`H2_SUPERBOUNCE_TUTORIAL=1` ([switches](../runtime/switches.md)). Priming,
a bounce spot within 8 m, a bounce landing and timeouts advance it
(`hud_kit_core::huds::superbounce::Tutorial`); closing the last step sets
`superbounce.tutorial_seen`. Step texts: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "The tutorial".

**The lines.** One marker on the one spot the player is going for, drawn
by the window's world-line pass with no depth test
([styles](../runtime/agent-api.md#drawing-world-space-lines-over-the-frame)),
from `crates/extensions/game/src/superbounce_world.rs` (`frame`,
`targets`, `figures`, `LockMarks`): a ring, a pin and a label, as [Floor
marks](#floor-marks) and [Crouch spots in the
world](#crouch-spots-in-the-world) describe, and while falling the
[lock-on](#lock-on). Not primed and over 6 units from a crouch spot
(`NEAR_RANGE`), nothing is drawn; while primed each seam of the targets'
set has a [seam line](#seam-lines). Lines use the HUD colour and font
(`crates/extensions/game/src/superbounce_font.rs`), the lock is red, and
marks rebuild at 30 Hz while motion follows the wall clock
([high-framerate.md](high-framerate.md)).

**Which spots are marked.** `superbounce_mode::marked` picks the subset
the marker, navpoint, tracker, bounce view and fall assist agree on:

- a failed spot is never marked; every other crouch spot is;
- once any strip of the map is `verified` (`superbounce_mode::verified`,
  [Verifying spots](#verifying-spots)), only proved strips are marked;
- a strip with a `learned` or `partial` line is marked at its aim
  ([Bounce](#bounce));
- on a map with no proved strip, two-joint slots are marked and [single
  creases and Vs](#single-creases) are not;
- a proven V's arm is marked only for a player on its approach side
  (`superbounce_mode::for_player`).

`superbounce.verify` and the bots still see every spot.

**In the air.** The HUD kit's elements drop the arming bar, prompt line,
navpoint and tutorial step (`superbounce_hud::in_air`); the readout and
charge segments stay, and the world keeps the chosen strip's rings, line
and the lock-on. `crates/extensions/game/src/superbounce_prompts.rs`
keeps `prompt_for`'s table, which nothing draws.

### Targets

Seams get the game's own navpoint, the arrow pointing down at the seam's
[landing point](#the-landing-point) with the waypoint marker above it. The
game keeps an off-screen navpoint at the edge of the waypoint area, so a
target is found from any height or facing.
`crates/extensions/game/src/superbounce_targets.rs` says where they go; the
readout's three navpoints draw them
([hud-kit.md](hud-kit.md#the-superbounce-readout)).

**Classes.** Every bounce spot of the scan, with the map's verified and
learned files applied (`superbounce_mode::scanned_spots`), has one class
(`superbounce_targets::class`), the best its data gives:

| Class | Data |
|---|---|
| known | proven: a game run bounced on it ([Verifying spots](#verifying-spots)), or a measured place (`Status::Proven`) |
| learned | the sim's training closed on it ([bounce-search.md](bounce-search.md)) |
| partial | a `partial` line with a hit: it bounced, and the budget ran out |
| slot | the scan's two-joint slot, the shape measured on Ascension ([Bounce](#bounce)) |
| candidate | scan only: a [single crease](#single-creases), a V's arm or tip, a [busy point](#busy-points) |

A spot a game run failed has no class and is never a target.

**Modes.** The variant's `targets` option picks the classes shown
(`superbounce_targets::in_mode`), each mode holding the one before it:

| `targets` | Classes |
|---|---|
| `common` | known |
| `likely`, the default | known, learned, partial, slot |
| `all` | every class |

**Switching the kinds in the game.** Two flashlight taps within 0.4 s move
the mode on (`superbounce_targets::Switch`, `TargetMode::next`); the
readout's targets row names the kinds shown. Details: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "Targets".

**Navpoints.** Two seams have one: the chosen strip and the tag. The other
seams are [seam lines](#seam-lines); `sbr.target.1` and `sbr.target.2` are
the [second bounce](#the-second-bounce)'s ranked seams
(`superbounce_mode::on_tick`). Details: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "Targets".

#### Seam lines

While primed, every seam of the targets' set has a line on its collision
edge, 0.02 over it. The rules are
`crates/extensions/game/src/superbounce_seams.rs` (`lines`, `segments`);
`superbounce_world::frame` draws them (`seam_callouts`) through the
world-line pass. The chosen strip is drawn heavy, the others thin and
fading with distance, a seam with no length as a cross; a V's arm has its
line only from across its bisector. Widths and fades: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "Seam lines".

#### The faced seam

Primed, with no tag and no strip being fallen onto, the chosen strip is the
seam of the targets' set the player faces (`superbounce_seams::faced`,
`Facing`; `superbounce_mode::faced_target`). The navpoint, takeoff,
approach guide, seam line and fall assist follow it. A tag outranks it,
and so does the strip a flight is predicted onto. With no seam ever faced,
the chosen strip is the nearest one the floor is high enough for
(`superbounce_charge::nav_target`). The cost rule: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "The faced seam".

**The tag.** Holding the flashlight (8 ticks) while aiming at one of the
mode's seams tags it (`Tagger`, `aimed`). The tagged seam keeps a navpoint
(`sbr.tag`) and, while primed, is the chosen strip
(`superbounce_targets::chosen`). `superbounce.state` has the mode, its
count, the tag and the marked points as `targets`
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type)).

### The second bounce

In the flight after a bounce the readout shows where that flight can bounce
again, the best seam first
(`crates/extensions/game/src/superbounce_second.rs`). It applies while the
first local player is in the air with a live [chain](#medals) (`window`).

**Reached.** A seam of the [targets](#targets)' mode is reached (`judge`)
when the flight, flown on the [flight model](#the-flight-model), comes down
to its height within 8 s at the speed it needs (`need_of`), the pursuit of
its [landing point](#the-landing-point) comes down in its catch, and no
floor covers the point.

**Ranked.** `Ranked::cost` is the steer's share of the stick's reach plus
half the need over the touchdown speed; the two lowest are kept (`rank`).

**Shown.** The readout's two target navpoints, its last row of ranks, and
the fall row's line-up (`superbounce_readout::line_up`). The variant's
`second_bounce` [option](#the-huds-options) turns it off.
`superbounce.state` has the ranked seams as `targets.second`. Rows and
wording: see the [detail note](notes/2026-11-26-superbounce-detail.md),
"The second bounce".

### The approach guide

A pipper and a box on the seam the player is going for, drawn on the
geometry, and before the jump a cue on the box. They are world marks of the
readout HUD, built in the HUD kit
([hud-kit.md](hud-kit.md#the-approach-guide)); the rules are
`crates/extensions/game/src/superbounce_guide.rs`, pure, run each tick
(`superbounce_guide::guide_with_phase`, into the state's `guide`). Where to
leave from is a bar on the floor ([The takeoff](#the-takeoff)).

| Mark | Says |
|---|---|
| the pipper | where the path touches down on the seam's floor |
| the box, on the seam's collision edge | the landing against the seam; in a fall, the phase verdict |
| the ticks | how far off the landing is |
| the drift arrow | the way to be moving at the landing |

**Which seam** (`superbounce_guide::target`). The seam a fall is predicted
onto, else while primed the chosen strip, else the nearest tagged or
navpoint seam within 6 units.

**When** (`superbounce_guide::strength`). While primed, at any distance;
fullscreen only. The pipper shows only within 3 units of the seam's point.
The variant's `approach_guide` [option](#the-huds-options) turns the marks
off.

**The impact** (`superbounce_guide::impact`). The body's free flight is
flown on the [flight model](#the-flight-model) with the stick held; the
**last free row** gives the pipper's place and the touchdown speed.
On the ground the flight is the jump from the takeoff's line
(`Takeoff::leaves_at`). Frames and constants: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "The approach guide".

**Lined up** (`superbounce_guide::caught`). The last free row is within the
seam's catch of its edge (`superbounce::catch_miss`).

**Depth** (`superbounce_guide::depth`). The takeoff solver and the legacy
chance read a landing in the catch by how low the last free row ends over
the seam ([notes/2026-11-22-seam-drop-depth.md](notes/2026-11-22-seam-drop-depth.md)).
With `past` the row's place past the edge along the drift and `over` its
height over the edge, for the bounding-sphere centre, the row is **deep**
when

```text
over − 0.75 × past < 0.445
```

and in the **landing window** when `past` is −0.07 to −0.01. The numbers
(`SEAM_SLOPE`, `DEPTH_CUT`, `LANDING_WINDOW`, `DEPTH_BAND`) are Ascension's
proven seam's alone.

**The call** (`superbounce_guide::phase_call`), what the box says. Each
airborne tick the mode runs `rule::predict` on the floor polygons within 4
units (`Room::floors_near`); the outcomes map to `Will bounce`, `Too
shallow`, `Too high`, `Off edge`, `No drift`, `Don't crouch` or no label.
In split screen the lock-on's brackets are drawn; fullscreen the box
replaces them (`superbounce_world::lock_shown`).

`superbounce.state` returns the guide as `guide`, with the call, the
pipper, the last free row, `caught`, `deep`, `chance`, `chance_capped`,
`jump_in`, `cue` and `takeoff`. Outcome table, field meanings and the
suppositions awaiting a lab run: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "The approach guide".

#### The takeoff

The takeoff is a fixed place on a real floor to leave from for the chosen
strip, a jump pressed on a line or a walk off a lip
(`superbounce_guide::place`, `superbounce_mode::takeoff_for`, into the
state's `takeoff`).

**The lip.** It comes from the strip's kept ghost or verified run
(`superbounce_ghost::lip`, [Ghost runs](#ghost-runs)), else from a map
search (`superbounce_world::lip`).

**The two flights.** A walk-off and a jump band are flown on the
[flight model](#the-flight-model) (`superbounce_guide::solve`); a press
leaves the floor 5 ticks later (`PRESS_LEAD`,
[notes/2026-11-21-bot-bounce-sim.md](notes/2026-11-21-bot-bounce-sim.md)).
The first that would call `Will bounce`, else one that lands in the catch,
is kept.

**On the map** (`superbounce_world::takeoff_figure`). Primed on the ground,
a bar across the run, an arrow and `Jump from here` or `Drop from here`.
Search steps, constants and the choice order: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "The takeoff".

#### The jump cue

Before the jump the box says what to do
(`superbounce_guide::Takeoff::cue`, `Cue`): `Climb up`, `Walk to line`,
`Jump in 0.7 s`, `Jump now` or `Walk off`. The count is the time to the
line's near end in tenths of a second. `settle` holds a flickering cue for
at most 3 ticks (`CUE_GRACE`). Words: `superbounce_guide::Cue::text`; the
HUD kit's box uses `superbounce_guide::kit_cue`. Conditions: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "The jump cue".

#### The chance

A legacy agent-API estimate of how sure the depth heuristic is in a fall
(`superbounce_guide::chance`), 0 to 1. The HUD shows no percentage. In the
air it is a confidence (`confidence`) from the last free row's distance to
the cut and the window; none below 5.2 units/s (`DEPTH_SPEED`). On a seam
that is the scan's alone it is capped at 60% (`SCAN_ONLY_CAP`, `cap`).
Table and softnesses: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "The chance".

### The HUD's options

The HUD pieces a game shows are options of the Superbounce rules script
(`content/modes/rules/superbounce.rules`), carried in the variant record's
option bits after the four scoring options
([rules-engine.md](rules-engine.md#firefight-ai-and-bounces)), each on
unless a variant turns it off:

| Option | Type, default | Off |
|---|---|---|
| `targets` | `common`, `likely` or `all`; `likely` | picks the [targets](#targets)' classes a game starts with; no off |
| `velocity` | bool, on | no speed row |
| `alignment` | bool, on | no fall row |
| `miss_info` | bool, on | no miss row |
| `medals` | bool, on | no medal line or announcer line ([Medals](#medals)) |
| `second_bounce` | bool, on | no ranked seams ([The second bounce](#the-second-bounce)) |
| `approach_guide` | bool, on | no pipper, box, ticks, arrow, jump cue or percentage ([The approach guide](#the-approach-guide)) |
| `charge_segments` | bool, on | no segments round the motion tracker ([hud-kit.md](hud-kit.md), "The charge segments") |
| `pogo` | bool, on | no Pogo Stick ([pogo-stick.md](pogo-stick.md)) |
| `pogo_style` | `unsc`, `covenant` or `forerunner`; `unsc` | picks the stick's look; no off |

The host reads them by name at a game's start and every 30 ticks
(`superbounce_options::read`, via `variant_ext::in_game` and
`rules::header::superbounce_options`). A script that declares none takes the
defaults. `superbounce.state` returns the HUD's as `options`, and
`pogo.state` the stick's two
([pogo-stick.md](pogo-stick.md#the-loadout)).

### Hold X: the fall assist

An opt-in help: with the player's **Fall Assist** setting on
(`superbounce.fall_assist`, off by default; `setting_defs.rs`,
`video_screen.rs` in `crates/extensions/game/src/`), holding X in the air
steers the player onto a bounce strip. It works on input: the assist
replaces the left stick the guest reads and takes X out of it, and writes
nothing to the world. The setting is read live (`follow_setting`); the
`superbounce-assist` feature in `H2_FEATURES` is the kill switch. Code:
`crates/extensions/game/src/superbounce_assist.rs`; `superbounce.assist`
and `superbounce.assist_test`
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type)).

**When it applies.** The mode and setting are on, X is held, the player is
falling, and a spot passes the reach cone (`judge`): below the player,
reached within 3 s (`MAX_TIME`), a landing point within 3 units sideways
(`stand_near`), the pursuit (`superbounce_flight::pursue`) within 0.4
(`HOLD_SLACK`), no floor over it (`covered`) and a clear sight line.

**Which strip.** Within 6 units sideways (`choose`): the strip steered onto
before, else the marked strip (`set_marked`), else the one the held landing
misses by least.

**The steer.** The aim is the [landing point](#the-landing-point)
(`aim_at`); the stick is the pursuit of it, turned by `world_to_stick`.

**The button.** While engaged X reads as released until the player lets go.

**The prompt.** `Hold X` shows on the strip the tap would steer onto
(`choose_now`), and `Guiding` once it engages (`assisting`;
[the prompts](#the-superbounce-game-type)). The [lock-on](#lock-on) does not
depend on the assist.

**Where it sits.** An input tap at `XInputGetState` (`AssistTap`, installed
by `register_late`). The assist never runs in a replay, adds no guest hooks
and is snapshot-stateless. Numbers, strip rules and the lab check: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "Hold X: the fall
assist".

### The flight model

Every landing the mode predicts is flown one tick at a time on one model
(`crates/extensions/game/src/superbounce_flight.rs`): the approach guide's
[impact](#the-approach-guide) and [takeoff](#the-takeoff), the
[fall assist](#hold-x-the-fall-assist), the [landing](#landings) a fall is
headed for and the [second bounce](#the-second-bounce). Each tick:

1. the fall speed gains 4.171 / 30 units/s (`FALL_GRAVITY`);
2. the sideways velocity steps toward the stick times 1.2 units/s
   (`AIR_TOP`) by at most 0.035 units/s (`AIR_ACCEL`); a stick under 0.2
   (`MIN_STICK`) is no stick;
3. the body moves its new velocity over 30.

The flight ends on the last free row. `hold` flies the stick the player
holds (`held`, `keep`); `pursue` flies the stick that heads for a point.
The flight is the body's origin (object `+0x64`); the bounding sphere's
centre (`+0x30`) rides about 0.4 over it. The air stick's measurements: see
the [detail note](notes/2026-11-26-superbounce-detail.md), "The flight
model".

## Open

Not yet decoded or measured; each is an open question (detail: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "Open"):

- the last 5 units/s of the touchdown impulse
  ([touchdown tick](#the-touchdown-tick-trace-stepped));
- what drops the pair (crouch, sleep);
- bounces at 60 frames a second ([the bounce rule](#the-bounce-rule-code-trace));
- the launch rule: the convex agent (`0x002eeb40`) and the solver step
  (`0x00311100`);
- lunges;
- other bounce spots, maps and bipeds.

## Charge, crouch spots and landings

What the mode shows about the three steps of a bounce: getting the
charge, keeping it, and landing it. The HUD parts are in
[hud-kit.md](hud-kit.md), "The charge meter", "The arming bar" and "The
charge segments"; this is the state they read, the scanner's rules and the
world overlay. The numbers are in
[notes/superbounce-spots.md](notes/superbounce-spots.md).

The scanner's part (the stricter crouch rule, the strip's edge and the one
catch) is built in `crates/live/superbounce`. The state is
`superbounce_mode::State`: `charge`, `verb`, `nav_target`, `landing`,
`last_landing`, `off_spot`, `still_ticks` and `standing_fits`
(`crates/extensions/game/src/superbounce_charge.rs` for the tracker and
landing classifier, `superbounce_capsule.rs` for the capsule test,
`superbounce_assist.rs`'s `predict` for the predicted landing), and
`superbounce.state` returns them
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type)).
The world overlay ("Crouch spots in the world", "Floor marks" and
"Lock-on") is built in `superbounce_world.rs`.

### Charge

The pair word has no amount: it is 0 through a crouch and −3 from the first
tick of the stand-up out of a forced crouch. "How charged" is two things the
engine does have. Traces: see the
[detail note](notes/2026-11-26-superbounce-detail.md), "Charge".

- **How far through the recipe** (crouch, walk in, let go, walk out):
  `near` (crouched, request set, within 1.5 of a crouch spot's stand point),
  `under` (crouch held and the standing capsule does not fit), `held`
  (button up, request, unit flags bit 23, still set: [forced
  crouch](#forced-crouch-code)), `primed` (word −3, request clear).
- **How long it lasts.** A primed body that stands still sleeps after 38 to
  40 ticks and wakes with a new pair at 0. The mode counts ticks under 0.03
  units/s; the charge left is 1 less that count over 38. A crouch ends the
  charge at once, so a primed player whose request is set is `lost`, as is
  one whose word leaves −3. `lost` shows for 30 ticks.

**Why it went.** Each loss gets a reason on the tick the charge goes
(`superbounce_charge::LostReason`): `crouched` (request set), `stood_still`
(word left −3 after `STOOD_STILL_TICKS`, 30, still ticks) or `unknown`
(respawn, vehicle, pair gone). A landing is not a reason.

**The warning and the new charge.** From 19 still ticks while primed
(`WARN_STILL_TICKS`) the verb is `KeepMoving`. For 150 ticks after a loss
(`AGAIN_TICKS`) the verb is `ChargeAgain` until a crouch step takes the line.

`superbounce_mode`'s state carries `charge` (phase, fill, and while lost the
reason and tick), `still_ticks`, `verb` and `standing_fits`.
`superbounce.state` returns them (the reason as `lost_reason`), and
`world.object`'s `physics` block has `standing_fits` for bipeds.

Two inputs are not the engine's own:

- **The standing capsule.** The engine's answer (output flags bit 1 of the
  character physics update, `0x001e55d0`) is not kept in a known field, so
  `under` and `standing_fits` come from the mode's own test
  (`superbounce_capsule.rs`) against the structure BSP's collision polygons.
  It exists only while the mode is on and its scan is ready (**S**). Reading
  the engine's bit needs a hook at `0x000e28f4` or the output pointer of
  `0x001e55d0` (**Open**).
- **The crouch button.** The fall assist's input tap
  (`superbounce_assist.rs`) reads it through the player's action table
  (`0x51ea18 + 8 + 10`). Without the `superbounce-assist` feature the phase
  goes from `under` to `primed` with no `held`.

How long the crouch is held and how deep the capsule is in the lip are not
shown as a charge.

### Crouch spots: the stricter rule

The scan keeps a forced-crouch region by the rules of [Crouch](#crouch), then
cuts the spots in this order (`crouch.rs`, `select`):

1. **In play.** The stand point is within 1.0 of the lowest scenario player
   starting location (`scnr+0x100`) over the scanned bounds. No locations
   turns the rule off.
2. **By a strip.** A bounce spot's stand is within 8 units. Under 3 kept
   spots, the nearest to a strip are added until there are 3.
3. **Spread out.** A spot within 3.0 of a better one is dropped; the 12 best
   stay.
4. **Proven first.** A measured priming place (`superbounce::PROVEN`, or the
   list in `superbounce::Context` given to `scan_in`) is kept, scores 1 and
   ranks first. A spot with a sample within 0.3 of one is `proven`, the
   others `likely`. The places: the Ascension corner (−10.91, 0.72, −1.16),
   Zanzibar's window sill (5.22, −26.285, 5.11) and Zanzibar's walk-out
   place (8.07, −3.01, 4.51) ([Zanzibar's
   window](#zanzibars-window-trace-lab-play)).
5. **Stand at the way out.** `stand` is the exit sample nearest the nearest
   strip. The spot also carries `out` and `lip` ([Crouch](#crouch)).

`superbounce::scan_map` and `explain_map` read the starting locations and the
proven places from the map (`superbounce::map_context`); `scan` and `explain`
on bare polygons have neither. There is no depth rule. `--explain` prints each
rule's verdict per region; expect 7 to 12 spots a map (`h2 superbounce scan
<map> --json` counts them). Which `likely` spots prime is
`superbounce.verify`'s to say ([Verifying spots](#verifying-spots)).

### Crouch spots in the world

While not primed, the crouch spot the navpoint is over carries the marker,
drawn by the world-line pass
(`crates/extensions/game/src/superbounce_world.rs` for the geometry,
`crates/apps/app/src/overlay/marks.rs` for the drawing):

- **The marker.** A ring of the capsule's radius (0.175) round `stand`, and a
  pin over it with `Crouch`; both fade near the eye (`near_fade`).
- **Under the lip.** The ring pulses in phase `under` or `held` within 1 unit.
- **The approach arrow.** An arrowhead 0.15 outside the ring along `out`
  (`arrowhead`), pointing in until `held`, then out.

The marker appears within 6 units (`NEAR_RANGE`). No other crouch spot is
drawn, and none while primed or airborne.

### Landings

The scanner's bounce spot carries its collision edge as the target ([the
target](#the-target-the-collision-edge)); its `need` is the [one
threshold](#speed). How a landing is shown is in [Floor marks](#floor-marks)
and [Lock-on](#lock-on).

- **On course.** `landing` (`superbounce_assist::predict_on`) is present
  whenever the player is airborne: the strip it lands nearest, with `verdict`
  (`OnSlot`, `Fixable`, `Miss`), `speed`, `need`, `edge`, `stand`, `fall` and
  `miss`. It is `locked` when `OnSlot` and `speed` reaches `need`
  (`superbounce_charge::Predicted`).
- **What happened.** A touchdown is a falling tick that stops falling. The
  launch is the best vertical speed of that tick and the 3 after
  (`LAUNCH_TICKS`, `Pending`), and the landing is judged when the window
  closes (`superbounce_charge::judge`). Within 0.5 of a strip it is `bounce`
  at a launch of 3.5 units/s (`BOUNCE_VZ`, shared with
  `sim_bench::drops::BOUNCE_VZ`; a standing jump launches 3.08), else `not
  primed`, `missed`, `too slow` or `no bounce`. `superbounce.state` returns
  `last_landing`. See [Covered seams](#covered-seams).
- **Through the floor.** A primed fall that sinks through a seam instead of
  bouncing is filed at once as `through` (`superbounce_charge::through`,
  `LandingResult::Through`): the origin crosses below the edge less 0.3
  (`SINK`) within 0.3 horizontally (`THROUGH_RADIUS`) while primed and moving
  down. The speed in is `Through::speed_in` (`Tracker::last_through`). See
  [Traces](#traces-trace).
- **Off-spot bounces.** A primed bounce no marked strip accounts for is kept
  on `superbounce.state`'s `off_spot` (last 16, `OFF_SPOT_MAX`) for
  harvesting; one bounce in play proves nothing
  ([Verifying spots](#verifying-spots)).

### Floor marks

While primed the marker is on the **chosen strip**: the navpoint's
(`superbounce_charge::nav_target`), else the nearest strip, and while falling
the strip being landed on (`nav_to_landing`). The HUD's navpoint is over it
([hud-kit.md](hud-kit.md#the-navpoint)) and the [fall
assist](#hold-x-the-fall-assist) steers onto it. Only
[marked](#the-superbounce-game-type) strips can be chosen.

- **Land.** A ring round `stand`, the strip's line along `edge`
  (`strip_bar`), and on the ground a pin `Land, 3 m` (the drop in metres). From
  a floor too low the pin and navpoint are on the [takeoff](#the-takeoff). In
  the air the [lock-on](#lock-on) is the mark.
- **Near the eye** the marks fade (`near_fade`).
- **The guide's box.** While the [approach guide](#the-approach-guide) is on
  the strip, its box is the only mark.
- **The takeoff.** On the ground, the [takeoff](#the-takeoff)'s bar.
- **Behind geometry.** A mark hidden from the eye
  (`superbounce_capsule::Room::blocked`) is drawn dim and unlabelled
  (`superbounce_world::occlude`).

The scanner cannot know steps that need an object or a turn before the jump.

### Lock-on

While primed, airborne and falling onto a predicted landing, the crease is a
target (`superbounce_world::lock_box`, `brackets`), in split screen or with
the variant's `approach_guide` option off; otherwise the [approach
guide](#the-approach-guide)'s box replaces it (`lock_shown`). Four corner
brackets on the floor close in as the landing nears the crease.

**Locked** is the landing's `locked`; the brackets and landing mark turn red
(`RED`) with `Locked` over them.

**Wall clock, not frames.** The box is worked out per guest tick; the brackets
ease to it over 0.12 s by the wall clock (`Ease`, `ImageMarks::visit`), so
they move alike at any frame rate.

**The reticle.** Nothing is drawn over the game's reticle and no guest hook is
involved (`superbounce_hud::reticle_lock_now`). Turning it red needs a hook
after `HUD_WIDGET_INPUTS` (`0x0013ad48`); the tested pieces stay
(`superbounce_hud::autoaim_entry`, `autoaim_exit`). The aim-assist target
(`touch_glue::on_target`) is not written.

Lines take the user's HUD colour (three floats at `0x00468c84`, read by
`superbounce_font::hud_colour`). Labels use the disc's HUD font (font 6) in
sentence case, numbers in whole metres.

**The font.** `superbounce_font::Capture::keep`, called by
`PackageRewrite::observing_fonts` (`crates/apps/boot/src/packages.rs`), keeps
the HUD font; the first drawing frame packs it into an atlas
(`gfx::world_lines::LabelFont`, `superbounce_font::atlas`) the window uploads
once. Without the font the labels use the 8x8 debug font and the log says so
once; without the HUD colour the lines are white.

### Checking it on the lab

Each check is a lab run through the agent API
([agent-api.md](../runtime/agent-api.md), [lab.md](../runtime/lab.md)) at 30
frames a second, read at pinned ticks. Procedures: see the [detail
note](notes/2026-11-26-superbounce-detail.md), "Checking it on the lab".

- **The phases.** Play back the recorded forced crouch and read
  `superbounce.state`, `world.object` and `havok.agents`.
- **The arming bar.** `render.frame` shots through the primed tick at 30, 60
  and 120 frames a second.
- **The time left.** `havok.pair.set` −3, stand still, read `havok.agents`.
- **Each kept crouch spot and strip.** `superbounce.verify`
  ([Verifying spots](#verifying-spots)).
- **The landing report.** The recorded jump at −3, at 0, from a lower take-off
  and to the side; `superbounce.verify {"kind":"drop"}` gives the same
  verdicts.

No game was run for this section.

## Verifying spots

`superbounce.verify` tries the loaded map's scanned spots in the running game
and says which work
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type)).
It is `crates/extensions/game/src/superbounce_verify.rs`: the plan in
`plan.rs`, a trial in `trial.rs`, the verdicts in `summary.rs`.

**A batch.** One call plans a batch of one kind (`crouch`, `drop` or `route`)
over the scan's spots, best score first, or over explicit `points`. An agent
ticker runs the trials one at a time on one seat.
`superbounce.verify_status` reports progress and the verdict;
`superbounce.verify_cancel` stops it. A batch needs the mode on and its scan
ready (`scan.status`), except over explicit points. A new map or the game
ending fails it.

**The seat.** `player.0` is driven through the first controller's pad by the
fall assist's input tap (the `superbounce-assist` feature). A bot is driven by
`bots::phantom::Phantoms::hold_pad`. Trials start by teleport and set the pair
word directly (`live_telemetry::havok_agents::set_pair_word`); they do not use
snapshots.

**Crouch trials** follow the recipe (crouch, walk in, let go, walk out), from
a grid of turns (0, 25, −25) and distances (0.3, 0.6) outside the lip. The
word reading −3 after letting go primes the trial. A trial that did not prime
fails as `not_under`, `not_crouched`, `not_forced` or `forced_no_word`. Steps
and defaults: see the note.

**Drop trials** fall onto a strip with the word set, from a grid of `across`,
`along` and `height`; `prime` is `set`, `none` (the control) or `keep`. The
landing is classified as the HUD's are (`superbounce_charge::classify`), or
`no_touchdown` within `fall_ticks` (120).

**Route trials** order a bot's bounce route (`bots.bounce_to`) to a point
`ROUTE_RISE` (6) above each strip, or to `to`, and work when the route ends
`done` with a launching touchdown. They write nothing to the verified file.

**The verdict.** A spot one trial worked for is `proven`; one whose trials all
ran and none worked is `failed`; else `untried`.

**The verified file.** A finished batch of scan spots merges its proven and
failed spots into `<data>/bots/<map>.verified` (`superbounce::verified`,
`bots::spots::store_verified`): a `# superbounce verified v1` header, then a
line per spot (kind, verdict, `stand`, and `dx`, `dy`, `yaw`, `height`,
`trials`, `ok`, `best`). An entry applies to the spot of its kind within 0.3
sideways and 0.5 up or down. The mode's spots, `superbounce.spots` and the
bots read it ([bots.md](bots.md)); a person can edit it.

No game has run a batch yet; the defaults are first guesses.

## Ghost runs

A ghost run is a recorded run that bounced off a spot, replayed as one faint
outline figure in the HUD's colour. Showing one is the `superbounce-ghost`
feature, off by default ([switches](../runtime/switches.md)); runs are
recorded either way. The code is
`crates/extensions/game/src/superbounce_ghost.rs`; the file format is
`superbounce::ghost` (`crates/live/superbounce/src/ghost.rs`).

**Recording.** The mode's tick hands every player's reads to the recorder:
bots (phantoms, [bots.md](bots.md#practising-bounces)) and, during
`superbounce.verify`, the local player. Each has a track of its last minute.
When the medal chain files a bounce ([Medals](#medals)), the run is cut from
the priming crouch (30 ticks of approach before it) to the top of the climb,
one frame a tick, for the strip within 0.5 of the touchdown; no strip, no run.

**The best run of a spot** is kept in `<map>.ghosts` in the bots directory. A
self-primed run beats a set one, then the shorter, then the higher launch. A
kept run marks its spot proven in the verified file. An unreadable ghosts file
is named in the log and not overwritten. The text format is
`# superbounce ghosts v1`, a `ghost` line (`stand`, `source`, `launch`,
`touchdown`, frame count) and `f` lines (x, y, z, facing, flags `c j a p`), at
most 1800 frames; see the note.

**Showing one.** Only the chosen strip's run is offered (`showable`), when the
player looks at its `stand` within 15 units. The prompt reads "Press (X) to
show ghost"; a press plays, stops or replays it. The button is X
(`superbounce_ghost::BUTTON`), or B on the Bumper Jumper layout
(`BUTTON_BUMPER_JUMPER`, `publish_layout`, `button_for`), where X is the
flashlight for the [bounce view](#the-bounce-view). A press counts only on the
ground (`press_counts`).

**Playback runs on the clock.** Frames are 1/30 s apart
([high-framerate.md](high-framerate.md)); each present interpolates the pose
at its own time, and the figure holds a second on its last frame.

**What is drawn**: an outline at 35% of the HUD's colour, standing-capsule
tall, with a facing tick on the floor (`marks`).

`superbounce.ghosts` lists the runs and playback; `superbounce.ghost_show`
plays or stops one
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type)).
No game has recorded or shown a run yet.

## Medals

Eight medals: **Superbounce** for every bounce; **Double Bounce**, **Triple
Bounce**, **Quad Bounce**, **Chain Bounce** and **Bouncetacular** for the
second to sixth bounce with no ground between; **Monster Bounce** for a launch
well above a normal one; and **Bounced** for a fall out of the map or through
a strip's floor during a bounce. Every player in the game gets them, bots
included, while the mode is on. The HUD shows them as Halo 2 does
([hud-kit.md](hud-kit.md), "The medals"); the film and Live count them. The
announcer speaks each line but Bounced
([audio.md](../audio/audio.md#our-medals-announcer-lines));
`H2_FEATURES=-medal-announcer` turns it off.

Code: `crates/extensions/game/src/superbounce_chain.rs`, stepped from
`superbounce_mode`'s tick; tests in `superbounce_chain_tests.rs`. The record
and counts: `live_proto::film::events::medals`, the `Superbounce` and
`Bounced` events in `live_proto::game`, `live-server`, and
`web/src/state/derive.ts`, in [Recording](#recording).

### The rule

The mode watches every player that has a biped, each tick: unit, position,
velocity, the airborne bit and the level pair's word, the fields
`superbounce_charge::Tick` reads ([the state](#the-superbounce-game-type));
`superbounce_chain::read_players`. At most sixteen pair reads a tick, only
while the mode is on.

- **A touchdown** is the one [Landings](#landings) uses
  (`superbounce_charge::touchdown`), counted anywhere on the map.
- **A bounce** is a touchdown the landing classifier files as `bounce` once
  its launch window closes (`superbounce_charge::judge`, 3 ticks after
  contact), with the word at −3 the tick before. It counts on any floor. A
  bounce by a strip names the nearest within 0.5
  (`superbounce_charge::landing_spot`, `superbounce_chain::spot_id`) and
  `on bounce` hears it; a bounce off every strip earns its medals with no
  strip (`Award::spot` `None`).
- **A Monster Bounce** is a bounce launching over 16 units/s
  (`superbounce_chain::MONSTER_VZ`, read from `Pending::launch`). The
  threshold is reasoned, not measured (**Open**); `Award::strips` keeps the
  strip count for comparing later.
- **The chain** is a count per player. A bounce while it is live adds one,
  otherwise makes it 1. It stops at a touchdown that is not a bounce, a
  clear airborne bit from `CLEAR_FROM`, a fall through a strip's floor
  (`through`), a unit change or a move over `TELEPORT` (3 units a tick), or
  the mode or game ending.

There is no clock: the next thing the feet do must be a bounce. Retail's
multi-kill clock comparison is in the [detail note](notes/2026-11-26-superbounce-detail.md),
"The rule".

Medals are given as the count is reached:

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

The table is `bounce_medals(chain)`, canonical in
`crates/live/live-proto/src/film/events/medals.rs`, with copies in
`web/src/state/derive.ts` and `superbounce_chain::bounce_medals`;
`bounce_medals_with(chain, monster)` adds Monster Bounce last. The ids are
`BOUNCE_MEDALS`.

**Bounced** (`bounced`) is `Event::Bounced`, earned when a player attempting
a bounce loses the unit, by a fall off the map (airborne, falling at
`TOUCHDOWN_VZ` or more, chain live or word −3 on the last tick) or by a
fall through a strip's floor that arms it for `BOUNCED_TICKS` (300 ticks).
A bounce or teleport disarms it. One death earns one Bounced, whose
`chain` is the chain it ended. Rules and cases: [detail note](notes/2026-11-26-superbounce-detail.md),
"The rule".

The chain is pure, a `Chain` per slot stepped by `Medals::step`, so its
tests are lists of ticks. It is not in a snapshot: after a state load or a
Theater seek a chain in flight restarts at 1.

### What is known about a second bounce

A chain needs no new crouch: the word stays −3 after a bounce, so a player
who neither crouches nor stands still is primed for the next landing
(**Trace**, **Code**). Whether the crease launches a landing at 8 to 13
units/s is **Open**. Evidence: [detail note](notes/2026-11-26-superbounce-detail.md),
"What is known about a second bounce".

### On the HUD

One notification per award for a local player, unless the variant's `medals`
[option](#the-huds-options) is off: the highest medal's line (`Superbounce!`
to `Bouncetacular!`, `Bounced!`, `Monster bounce!`) behind its glyph, queued
through `Handle::notify`, the routine Halo 2 uses for multi-kills. No sound;
bots' and other consoles' medals are recorded, not shown. Glyphs and states:
[hud-kit.md](hud-kit.md), "The medals" and "Game events and medals".

### Recording

The game says a bounce happened, the chain count and whether it was a
Monster; medals are derived wherever shown.

- **The events.** `Superbounce` and `Bounced` are game events beside
  `PlayerKilled` (`live_proto::game`), sent by `Telemetry::superbounce` and
  `Telemetry::bounced` (`crates/live/live-telemetry/src/hooks.rs`) from
  `superbounce_chain::record` on the recording console only
  (`superbounce_chain::records`). `superbounce.medal_test` awards are not
  recorded.
- **The film.** `film::events::Tracker` writes a `Superbounce` or `Bounced`
  record and a `Medal` record per medal, and tallies them
  (`medals::Tally`). The film's version stays 3. See [film-format.md](film-format.md).
- **Live.** Both events are in `game::CLIENT_GAME_EVENTS`; only the host
  reports them. Live projects medals at ingest
  (`film::events::medals::Projection`); see
  [live-api.md](../live/live-api.md#stats-summary) and
  [live-api.md](../live/live-api.md#medals).
- **The site's pictures.** Badges are `web/public/art/medals/<id>.webp`,
  listed in `PACK_MEDALS` (`web/src/lib/art.ts`); the disc has none
  ([web-viewer.md](../live/web-viewer.md)).
- **Theater.** A film re-runs the simulation, so the mode shows the lines
  again; the summary feeds `theater_ui::adapter`.

Fields, validation and deploy order: [detail note](notes/2026-11-26-superbounce-detail.md),
"Recording".

### The agent API

`superbounce.state` ([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type))
has `medals`:

| Field | Meaning |
|---|---|
| `players` | per slot: `player`, `chain`, `live`, `counts` (medal id totals) |
| `recent` | the last 8 awards: `player`, `tick`, `chain`, `medals`, and for a bounce `strips` |

`superbounce.medal_test {"chain": N}` (with `"monster": true`) or
`{"bounced": true}` (access: act) files an award for the first local player
with a unit: the HUD line and `medals`, nothing sent to the film or Live.

### Checking the medals on the lab

Lab checks through the agent API at 30 frames a second, as
[above](#checking-it-on-the-lab): the word in flight, a second bounce, the
airborne bit, what ends a chain, host and guest, Bounced and the line. No game
was run for this section. The list: [detail note](notes/2026-11-26-superbounce-detail.md),
"Checking the medals on the lab".

## The bounce view

The spots across the level on the flashlight, the nearby ones on the motion
tracker. Built and not run in a game. Code:
`crates/extensions/game/src/superbounce_view.rs`,
`crates/live/superbounce/src/view.rs`. Tests: `superbounce_view_tests.rs`
and the tests in `view.rs`.

| Input | What shows |
|---|---|
| Nothing | Two tracker blips: the next spot and the one after (`tracker_blips`, `TRACKER_MARKS`) |
| Tap the flashlight | A scan pulse across the tracker; spots light as it passes |
| Hold the flashlight | Every spot marked in the world, and the tracker grows to a map |
| Tap twice | Each tap pulses; the pair switches the seam kinds ([Targets](#targets)) |

Proven spots draw at full strength, likely ones at half, in the HUD colour
(`superbounce_font::hud_colour`).

**The button** is the flashlight action (unit `+0x148`, bit `0x4`); a press
under `TAP_TICKS` (8) is a tap, else a hold.

**Behind a wall.** One sight line per spot against the structure BSP, gridded
once a map (`superbounce::view::Geometry`, `SightCache`).

**Another floor.** More than 0.9 units off the feet is another floor
(`superbounce::view::level`).

**The pairs.** A crouch spot pairs with the nearest landing within 8 units
(`superbounce::view::pairs`).

**The map's floors** are collision polygons facing up (normal over 0.7),
outlined by edges no other floor shares.

**The tracker's place** is `rects::TRACKER` of the HUD kit, scaled by
`render_aspect::hud_bottom_left_fit` ([hud-kit.md](hud-kit.md#anchors-the-safe-area-and-scaling));
the range is the `hudg`'s `radar_range`, or 25 m. The marks are host marks
(`gfx::world_lines::HudMark`), not HUD kit elements.

**Any frame rate.** State is made once a 30 Hz tick; drawing is a function
of that and the window's `now`.

**The agent API.** `superbounce.view` (read) and
`superbounce.view_test {"mode": "pulse"|"hold"|"none"}` (act)
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type)).

Dimensions, offsets and mode rows: [detail note](notes/2026-11-26-superbounce-detail.md),
"The bounce view".

### Checking the bounce view on the lab

Checks cover the button, the tracker's place at 4:3 and 16:9, the blip disc
radius, hidden spots on Zanzibar and frame time. No game was run for this
section. The list: [detail note](notes/2026-11-26-superbounce-detail.md),
"Checking the bounce view on the lab".
