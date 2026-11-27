# Superbounce: the geometry and the spots

Where bounces and crouch spots are in the map geometry. Part of [superbounce.md](superbounce.md).

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
neighbours. Bounce spots, slots and candidates together, are ranked by
score alone, best first (at most 4096, a bound no map reaches, so no
cap drops a high-scoring candidate); crouch spots are kept by
[the stricter rule](superbounce-landings.md#crouch-spots-the-stricter-rule).

### Bounce

A superbounce is a primed landing on a **seam** in the structure
collision: the joint where two collision surfaces meet. The touchdown
tick's two contacts at Ascension (y 4.105 and 3.939) are Havok's
contacts with floor edges, not with walls. The floor there is a strip
from y 3.8 to 4.0 with two joints: a concave crease at y 3.800, where
the ramp (normal z 0.8396) comes down to it, and a flat seam at y 4.000
with the next floor piece ([geometry](#the-geometry-code-from-the-map)).
The two contacts straddle y 4.02 and the stepped run's origin was
y 4.04: the capsule sinks into the flat seam, between the edges of the
two coplanar pieces. A contact with the ramp would carry the ramp's
normal and none did; the bots' one learned Ascension site settled its
aim at y 3.985 ([bounce-search.md](bounce-search.md)). So a spot's
target is the joint the pinch sits on, not the strip between: that
collision edge itself ([the target](#the-target-the-collision-edge)).

The community guide's Ascension spot says "jump onto the line where the
ramp meets the ground" ([known
spots](notes/2026-11-21-known-superbounce-spots.md)), the crease. The
capsule says otherwise: centred at y 4.02 its lower sphere (radius
0.175) reaches y 3.845, short of the crease, and however deep it sinks
its centre stays over 0.22 from the crease line, so neither the crease
nor the ramp is touched. The guide names the line a player sees: the
seam between coplanar pieces is not drawn, and the two lines are 0.2
apart, about 60 cm. Both stay targets for the sim to settle: the slot
on the seam, and the crease's own [candidate](#single-creases) on
y 3.800.

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
  (0 for a seam): the launch at the [speed threshold](#speed). One
  point calibrates it: Ascension's crease, ramp normal z 0.8396 against
  flat floor, gives the 17.85 units/s the body gained on the touchdown
  tick. A trough's two creases add. Strips under 3 units/s are dropped;
  estimates are capped at 60 and scored down past it.
- **drift**: across the strip, from the crease toward the other joint
  (the sharper crease of two): +y at Ascension, the way every traced
  bounce came in.
- **need**, **fall**: none (0; `null` in `--json`, `unmeasured` in the
  listing). The scan measures no speed and no drop; the game reads
  [one threshold for every seam](#speed). `--json` also says whether a
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

**The sim's answer first.** Where the map's learned file
([bounce-search.md](bounce-search.md)) has a `learned` or `partial`
line with a hit for a bounce spot, that line's aim (`ax ay`) outranks
the scan's: `superbounce::learned::apply` moves `stand` onto it. The
`edge` stays the collision edge: the aim says where on and beside it
to land, the edge is what the catch is measured from. It proves
nothing: the training writes the word, and only a primed game run
proves a spot ([Verifying spots](superbounce-landings.md#verifying-spots)). A line belongs to
one spot: of those it is about, the one whose target line passes
nearest its aim; a verified entry likewise proves only the spot whose
target line passes nearest its working landing. The game applies it where
its spot list is built (`superbounce_mode`, after the verified file);
the bots read the same aim from the line's knobs (`Site::target`). At
Ascension the learned line (standing at y 3.88, from an older scan's
stand) has its aim at (−10.45, 3.985), on the seam, and the verified
entry's landing is at y 3.976: the slot takes both, so it is the proven
strip, marked at y 3.985, and the crease stays a candidate.

#### Single creases

The Zanzibar window landing ([Zanzibar's
window](#zanzibars-window-trace-lab-play)) is one concave crease, a flat
floor meeting a 17.6° ramp along y −26.700 (x 1.80 to 6.00, z 2.10),
with no second joint, so no slot. Every concave joint between two
walkable floors is also a spot of its own (`Shape::Crease`, `shape`
`crease` in `--json` and `superbounce.spots`; `bounce.rs`, `crease`).
Nothing has measured one, so a single crease is a **candidate**
(`Spot::candidate`): the sim's training flies it (`h2 sim train`), and
play, the HUD and the assist leave it out until a game run or the
training proves it ([which spots are marked](superbounce.md#the-superbounce-game-type)).

- **edge**: the crease's collision edge, its two end vertices to the
  float (Zanzibar: (1.80, −26.70, 2.10) to (6.00, −26.70, 2.10)).
  **drift**: across it onto the flatter floor.
- **stand**: the crease's middle, on the line, origin 0.01 above it
  (Zanzibar: (3.90, −26.70, 2.11)).
- **launch**: 111.3 × its own `1 − cos(dihedral)`, the slot calibration
  with no second joint: 5.2 units/s at Zanzibar, the scale of the small
  pop its first try made. Under 3 units/s (a dihedral under about 13°)
  it is no candidate.
- **need**, **fall**: none. The one [catch](#the-target-the-collision-edge)
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
[covered](#covered-seams).

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
its own collision edge (`crates/live/superbounce/src/vee.rs`, `parts`):

| Part | `edge` | `stand` | `drift` | `arm` |
|---|---|---|---|---|
| tip (converging) | the tip vertex twice | on it | in from the opening, toward the tip | none |
| bottom line (groove) | the bottom line's two vertices | its middle | across, onto the flatter floor | none |
| arm (converging) | the crease's own two vertices | its middle | across the bisector onto the arm | set |
| arm (groove) | the bottom line's two vertices | 0.05 across it onto the ramp (`vee::ARM_STAND`, inside the catch) | onto its ramp | set |

An arm's `arm` (`superbounce::Arm`) records the V's bisector, through
`pivot` (the tip, or the bottom line's middle) along `bisector` (into
the opening; a groove's along its bottom line), and `from`, the side the
arm is approached from: the half-plane on the far side of the bisector
(`Arm::approached_from`). The two arms' `from` are mirrors of each
other. `h2 superbounce scan` prints it (`arm, approached from (x, y)
of (pivot)`, and `arm` in `--json`), and `<map>.spots` stores it.

The sim's training flies each arm from its `from` side and once from its
own side as a control, to confirm or refute the mirror
([bounce-search.md, "Vs: the mirror and its control"](bounce-search.md#vs-the-mirror-and-its-control)).
In play, once an arm is proven, the mode marks the arm across the
bisector from the player: a player on side A is marked arm B
(`superbounce::facing`, `superbounce_mode::for_player`). Before any
proof no part is marked, as no candidate is. Sanctuary,
Turf and Warlock are Multiplayer Map Pack maps, not on the retail disc:
their scans need the map pack's `.map` files (`deltatap.map`), and no
`deltatap.spots` exists until `h2 bots nav deltatap` runs on them.

#### Monster bounces

Halopedia's "monster bounces" are a thin seam over
many grid lines, or a V of converging lines. A spot whose target line
has at least 2 other concave creases running into it, each ending on
the line and not parallel to it (`bounce.rs`, `converging`), scores at
least 0.85, 0.05 more for each further one to 0.95: ranked with the Vs.
Coplanar seams ending on the line are not counted, as a floor's
triangulation ends them on every joint; so a thin seam crossed by grid
lines is not told apart from its tessellation. The vertices where
many such lines meet are [busy points](#busy-points), spots of their
own.

Creases and Vs are merged along their line like slots, and ranked with
the slots and busy points by score: no cap of their own. They are not strips to the
crouch rules: a candidate neither keeps a crouch spot "by a strip" nor
scores one up. `--explain` lists every concave crease within its radius
with its dihedral and the candidate it makes, or why none
(`superbounce::explain_bounces`), then the bounce spots there.

#### Busy points

A floor vertex where many collision edges meet is a spot of its own
(`Shape::Busy`, `shape` `busy`; `busy.rs`): the stressed, busy points
of the floor's mesh, where a landing meets the most edges at once.
Every edge of a walkable polygon ending at the vertex counts once,
coplanar triangulation edges as much as creases; edges under 2 cm are
slivers and don't. Vertices within a millimetre are one, and the
coordinate kept is the map's own vertex, unrounded.

- **edge**: the vertex twice, to the float: the vertex is the target,
  and the [catch](#the-target-the-collision-edge) round it a disc.
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

#### The target: the collision edge

A bounce spot's target is the collision edge itself, the segment
between two vertices, and its `edge` is those two vertices to the
float, as the map's collision stores them (a busy point's is its vertex
twice). Floor marks, the aligner and the assist aim at that line
(`stand`, on it, or a measured landing beside it, below), and `h2
superbounce scan` prints it with every digit the float has.

There is no band and no width. One tolerance remains, the catch: how
far either side of the edge, horizontally, a landing's origin still
counts as on it, `superbounce::CATCH`, 0.1. It is used only for the hit
or miss verdict (`superbounce::catch_miss`: the game's landing
classifier and lock, the verifier and the bots' `missed`). The sim's
[seam drops](../runtime/sim.md#seam-drops) measure it (`drops.json`'s
`catch`); 0.1 holds the measured Ascension origin (y 4.04,
0.04 off the seam), the bots' learned aim (y 3.985) and the second
Zanzibar window try (4.8 cm off its crease).

#### The landing point

Where to come down on a seam is one point, the spot's `stand`, and
everything that points at a seam reads it: the target navpoints and the
tag ([Targets](superbounce.md#targets)), the mode's own navpoint, the readout's
line-up, the [second bounce](superbounce-game-type.md#the-second-bounce)'s ranking and the
[fall assist](superbounce-guide.md#hold-x-the-fall-assist). The mode sets it where it
applies the map's files to the scan (`superbounce_mode::with_verified`),
by the best data the seam has:

| Data | `stand` | `Spot::landed` |
|---|---|---|
| a proven seam's verified entry whose landing is on the edge to the catch and within 0.3 of the entry's point | the entry's point plus its `dx`, `dy`: where the drop that bounced came down (`superbounce::verified::landings`, `land`) | set |
| else a `learned` or `partial` line with a hit and an aim | the line's aim (`superbounce::learned::apply`) | set |
| else | the scan's point on the edge | clear |

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
[flight model](superbounce-guide.md#the-flight-model) with the stick held as it is, and
keeps its place across the strip (`stand_near`).

Every recorded bounce came down moving across the seam along its drift,
at 0.3 to 1.3 units/s. A landing in the catch that comes down slower
along the drift than 0.3, still or against it, wants a push that way
(`superbounce_assist::wants_drift`, `DRIFT_MIN`): the readout's steer
line says `(arrow) Drift` in place of `On seam`. It is a hint, not a
verdict: the lock and the landing's result do not read it. The
[approach guide](superbounce-guide.md#the-approach-guide)'s arrow is lit at the full-launch
minimum, 0.15 units/s along the drift (`rule::DRIFT_FULL`): 0.1 can
bounce, 0.15 gives the full launch
([notes/2026-11-23-why-bounces-work.md](notes/2026-11-23-why-bounces-work.md)).

#### Covered seams

A seam under a low ceiling, indoors, can't launch anyone. Every bounce
spot, slot, crease, V or busy point, is dropped when at least half the points along
its target line (each 0.25, and both ends) have structure over them
within 5.06 of the floor (`bounce.rs`, `covered`): a column cast up from
each point, where any face that isn't vertical counts, an upper floor as
much as a ceiling. 5.06 is the climb of a typical launch, 6 units/s
(4.34 under the [speed model](#speed)'s gravity), plus the standing body
(0.725) (`superbounce::speed::cover`). The best measured launch, 13.6
units/s, climbs 22 units and would drop seams under any roof; most
bounces launch at 4 to 6. `--explain` names a covered crease: `covered:
structure 3.00 over it`.

#### Speed

`crates/live/superbounce/src/speed.rs` keeps the older single-speed
estimate for the assist, landing tracker and training scores: 5.4 units/s
(`BOUNCE_SPEED`), the slowest touchdown measured to bounce in the first
recipe, and a 2.43 drop to reach it (`speed::fall`, gravity 4.15). A
standing jump lands at 3 and never bounced in that recipe. `speed::launch`
adds 1 times the excess over 5.4 to the spot's launch (the gain is
unmeasured). The sim's [seam drops](../runtime/sim.md#seam-drops) measure
the speed and gain (`drops.json`'s `bounce_speed` and `gain`).

That estimate is not the retail rule. Near 5.4 the sweep is a band one
tick of gravity wide and the useful apex-to-floor drop is 3.35 to 3.60;
faster falls bounce by another route ([the bounce
rule](#the-bounce-rule-code-trace)). The airborne HUD uses
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
