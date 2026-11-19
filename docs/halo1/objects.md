# Halo 1 sandbox objects as Halo 2 tags (`crates/h1objects`)

Status: **Built** (2026-11-17) for reading every Halo 1 Xbox object tag on
the 13 multiplayer maps, and for building Blood Gulch's arsenal (assault
rifle, pistol, plasma rifle, needler, frag and plasma grenades, health
pack, cyborg, warthog, ghost) as Halo 2 tags on Coagulation. The built
tags read back through `crates/tags`. No converted object has been loaded
by the game yet ("Needs a game run").

This is the Halo 1 sandbox of [multiplayer.md](multiplayer.md)
(phase 4) and the object step of [campaign.md](campaign.md) §2.
Physics is Halo 2's (decision 1); Halo 1's health is kept (decision 2,
[health.md](health.md)). Models and animations come from
[models.md](models.md); the Halo 2 object family is in
[objects.md](../formats/objects.md), collision in [collision.md](../formats/collision.md).

No map bytes, tag dumps or extracted assets are in the repository. The
unit tests use invented data; the `#[ignore]` tests read the discs at run
time and print counts, names and values only.

## Evidence labels

- **V1:** checked on every object tag of the 13 Halo 1 Xbox multiplayer
  maps by `every_object_tag_on_every_multiplayer_map_reads`: 195 `weap`,
  208 `proj`, 182 `eqip`, 26 `bipd`, 39 `vehi`, 364 `coll`, 39 `phys`,
  and the references of 4,137 `effe`, 858 `jpt!`, 4,110 `snd!`, 104 `cont`
  and 391 `ligh`. Checks: the object type matches the group, model-chain
  references have their group, every collision BSP passes
  `CollisionBsp::validate`, node/sphere indices are in range, masses and
  radii are sane.
- **V2:** the value pattern on retail Halo 2 tags (Coagulation): the same
  field holds the expected value on the SMG, magnum, plasma rifle and the
  grenades (for example the magazine's loaded maximum and the
  barrel's heat per round are nonzero exactly where the weapon has a
  magazine or heat).
- **S:** field order and meaning unverified, placed by the data.
- **E:** from [objects.md](../formats/objects.md) (engine code).
- **Design:** a choice made here.

## Reading Halo 1

Halo 1 references are 16 bytes (group, name address, name length, datum);
blocks are 12 (count, address, unused). The root size of each group is
where its first child block starts (V1).

**References without a layout (`refs::scan`).** A 16-byte run counts as a
reference when its datum names a tag of that group (or a child group) and
its name address is that tag's name, or when it is a null reference of a
group the map uses (no name). This is how `effe`, `jpt!`, `snd!`, `cont`
and `ligh` are read (references only, not their bodies). Note that many
null references in cache files have a zero group word, so they are not
found; the typed readers below read nulls by offset.

### `obje` (`0x17c`), shared

| Offset | Field | Evidence |
|---|---|---|
| `00` | u16 object type: bipd, vehi, weap, eqip, garb, proj, scen, mach, ctrl, lifi, plac, ssce | V1 (matches the group on every tag) |
| `02` | u16 flags | S |
| `04` | f32 bounding radius, `08` f32x3 bounding offset | S, V1 (finite) |
| `28` | `mode`, `38` `antr`, `70` `coll`, `80` `phys` | V1 |
| `90` | modifier shader, `a0` creation effect | S |
| `140` | attachments (`0x48`, reference at `+0`), `14c` widgets (`0x20`), `158` functions, `164` change colours, `170` predicted resources | S, V1 (blocks in range) |

### `item` (`0x17c..0x308`)

`248` material effects (`foot`), `258` collision sound, `2e8` detonating
effect, `2f8` detonation effect (V1: groups).

### `weap` (root `0x508`)

| Offset | Field | Evidence |
|---|---|---|
| `308` | u32 flags; `30c` label (`char[32]`): the weapon type the units' graphs animate it by (`stand rifle ar idle`): `ar`, `hp`, `sg`, `pr`, `rl`, `sr`, `pp`, `ne`, `ft`; `cg`, `cannon`, `gun` on vehicle guns | S; V1 (every Blood Gulch `weap`) |
| `338` | f32 ready time, `33c` ready effect | S, V1 |
| `34c`..`360` | heat recovery threshold, overheated threshold, detonation threshold, detonation fraction, heat loss per second, illumination | S; V1 (set on the plasma rifle, zero on the assault rifle) |
| `374`, `384` | overheated, detonation effects | S |
| `394`, `3a4` | melee damage, melee response (`jpt!`) | V1 |
| `3bc` | actor firing parameters (`actv`) | V1 |
| `3da` | i16 magnification levels, `3dc` f32x2 magnification range | V1 (nonzero only on zooming weapons) |
| `3e4`..`3f4` | `cone_angle`, `cone_range`, `pull_angle`, `pull_range`, `bend_angle` (the Halo 2 `weap +0x208..+0x218` order) | S, V1 (angles in radians, ranges in world units) |
| `418`, `41c`, `420`, `430` | light power on/off time and effects | S, V1 |
| `45c`, `46c` | first-person model, its animations | V1 |
| `480` | HUD interface (`wphi`); `490`, `4a0`, `4b0` pickup, zoom in, zoom out sounds | V1 |
| `4e2` | i16 weapon type | S |
| `4e4`, `4f0`, `4fc` | predicted resources, magazines (`0x70`), triggers (`0x114`) | V1 |

**Magazine (`0x70`):** `04` i16 rounds recharged, `06` total initial, `08`
total maximum, `0a` loaded maximum, `14` f32 reload time, `18` i16 rounds
reloaded, `1c` f32 chamber time, `38` reloading effect, `48` chambering
effect, `64` magazine objects (`0x1c`: i16 rounds, `+0xc` `eqip`). V1
(rounds reloaded equals the loaded maximum on the assault rifle).

**Trigger (`0x114`):** `04` f32x2 rounds per second, `0c`/`10`
acceleration and deceleration time, `20` i16 magazine, `22` rounds per
shot, `24` minimum rounds loaded, `26` rounds between tracers, `2e` firing
noise, `38`/`3c` error acceleration and deceleration time, `50` charging
effect, `6c` i16 distribution function, `6e` i16 projectiles per shot,
`70` distribution angle, `78` minimum error, `7c` f32x2 error angle, `84`
first-person offset, `94` projectile, `b8` heat per round, `108` firing
effects (`0x84`). V1 (`94` is a `proj` on every trigger with one), S
otherwise.

### `proj` (root `0x24c`)

| Offset | Field | Evidence |
|---|---|---|
| `17c` | u32 flags; `180` i16 detonation timer starts, `182` impact noise | S |
| `18c` | super detonation; `1ac` effect | V1 |
| `19c`..`1a8` | AI perception radius, collision radius, arming time, danger radius | S, V1 (set on grenades) |
| `1bc` | f32x2 timer | S |
| `1c4` | minimum velocity, `1c8` maximum range | S, V1 (equal to the Halo 2 template's on bullets) |
| `1cc` | air gravity scale, `1d0` air damage range, `1d8` water gravity scale, `1dc` water damage range | S, V1 |
| `1e4`, `1e8`, `1ec` | initial velocity, final velocity, guided angular velocity: **per tick** | S; V1: bullet values are a thirtieth of a plausible speed per second |
| `1f0` | i16 detonation noise; `1f4` detonation started; `204` flyby sound; `214` attached detonation damage; `224` impact damage | V1 |
| `240` | material responses (`0xa0`, 33 on every projectile) | V1 |

### `eqip` (root `0x3b0`)

`308` i16 powerup type (0 none, 1 double speed, 2 overshield, 3 active
camouflage, 4 full-spectrum vision, **5 health**, 6 grenade), `30a` i16
grenade type, `30c` f32 powerup time, `310` pickup sound. V1 (the health
pack is type 5 with a positive time; the grenades are type 6).

### `effe` and attachments

Read field by field since converter version 50 (`h1::Effect`,
`h1::attachments`): layout and evidence in
[tag-definitions.md](../formats/tag-definitions.md), "Effects from definitions".

### `part` (root `0x164`) as `prt3`

`h1objects::particles::H1Particle` and `Converter::particle`: the stem
by the framebuffer blend function at `+0xda` (0 alpha blend, 3 additive,
7 alpha-multiply-add: V, the particles both games ship pair that way; 6,
on three particles, is drawn additive, unproven), the bitmap (`+0x4`) as
the stem's `alpha_map_texture` with the compiled shader built from the
stem ([tag-definitions.md](../formats/tag-definitions.md), "The compiled particle
shader"), Halo 1's sprite sequences (`+0x98` first, `+0x9a + +0x9c`
counts, one picked per particle), the collision and death effects
(`+0x48`, `+0x58`). The five properties and the input mask, vertex
shader, quad and UV window are those of the base's static particles
(white, opaque, unit size, unrotated, first sprite; the most common
set). Lifespan, fade, radius and frame animation, physics and the rest
are emitter-side in Halo 2.

### Effect particles (event `+0x38`, `0xe8`) as particle systems

Since converter version 58 (`particles::effect_particles`,
`particle_system`, `Converter::effect_particle_systems`): each Halo 1
effect particle whose `part` converts becomes a particle system of its
event, one emitter on the proven slots ([effects.md](../formats/effects.md),
"Emitter"). The record (V, `halo1_effect_particle_survey`): `+0x8`
location, `+0x54` the `part`, `+0x6c`/`+0x6e` count bounds, `+0x70`
distribution radius, `+0x84` velocity bounds, `+0x8c` velocity cone
(radians), `+0x90` angular velocity bounds, `+0xa0` radius bounds,
`+0xb0`/`+0xc0` tint bounds (alpha, red, green, blue), `+0xe0`/`+0xe4`
scale words.

- Count: the engine spawns `rate x 30 x dt` per step, `dt` the event's
  step in seconds (E, the tick length `[0x510c54]+4` cut to the event's
  remaining time), so the rate is Halo 1's count over 30 times the event's
  duration, floored at a tick; an event with particles shorter than a
  tick lasts one (retail's usual duration). Halo 1 creates the count at
  once; over a longer event Halo 2 spreads it.
- Lifespan (the `part`'s), speed (velocity), cone (degrees), size (twice
  the radius: unproven), tint (packed ARGB: unproven) and alpha: Halo 1's
  midpoints, as constants.
- The words no slot names (system flags, fade and LOD distances; emitter
  `+0x9c..+0xb8`; the constants' input words) are the base's most common
  (`EmitterRole`).
- Not carried: distribution radius, relative direction and offset,
  angular velocity, the `part`'s physics (`pmov` null), the scale words.
- A particle's collision or death effect that leads back to the effect
  being converted is left out.


### `pctl` (particle systems) as particle systems

Since converter version 59 (`particles::particle_system_types`,
`Converter::particle_system_states`): an effect part naming a Halo 1
`pctl` becomes particle systems of its event, after the event's own
particles. The layout (V, all 28 distinct tags,
`halo1_pctl_structure_survey`): a `0x68` root (`+0x38` point physics,
`+0x48` update physics, `+0x50` 4-byte constants, `+0x5c` particle types
of `0x80`); a type's `+0x0` name, `+0x24` initial count, `+0x2c` radius,
`+0x54` creation physics (0, 1, 2) and `+0x5c` its 4-byte constants,
`+0x68` system states (`0xc0`), `+0x74` particle states (`0x178`); a
particle state's `+0x0` name, `+0x20` duration and `+0x28` transition
bounds, `+0x30` bitmap, `+0x40` sequence index, `+0x48` radius bounds,
`+0x58` rotation rate bounds, `+0x60`/`+0x70` colour bounds, `+0x84`
point physics, and from `+0xdc` `part`'s shader words (`+0xe2` the blend
function: 298 fire states 3, 104 smoke states 0).

Each particle state that lasts becomes its own system, its `prt3` built
from it as from a `part`, all created with the part: the type's initial
count, the state's duration plus transition as lifespan, its radius and
colour. Halo 1 moves each particle through the states in order (fire
turning to smoke); here every state's particles appear at once. Not
carried: the state sequence, the creation physics and its constants (no
proven meaning), the system states, the rotation rate. The particles
start still at the part's location.

### `cont` (root `0x144`) and contrail attachments

`h1objects::particles::H1Contrail` (since converter version 55): Halo 2's
`cont` is Halo 1's compacted, span for span (V: the sniper, needler and
plasma contrails of both games word for word, `particles_disc.rs`
`contrail_survey`). `particles::CONT_SPANS` and `CONT_POINT_SPANS` list
the spans; the layout is in [tag-definitions.md](../formats/tag-definitions.md),
"Particles and contrails". Halo 2 effect parts never name a `cont` (the
part groups on the disc are `deca`, `snd!`, `obje`, `ligh`, `lens`,
`jpt!`, `MGS2`, `tdtl`): contrails are object attachments, so a Halo 1
attachment that names one converts it (`Converter::contrail`), on
vehicles with the other attachments and on rebuilt projectiles, whose
attachment block is replaced by Halo 1's when any of them converts. A
point's physics (`pphy`) is not converted: null.

### `bipd`, `vehi`, `scen`

Read as the `obje` header (plus every reference, `refs::scan`, for the
units). Their tuning is not read yet (below).

### `coll` (root `0x298`)

| Offset | Field | Evidence |
|---|---|---|
| `08` | f32 maximum body vitality; `cc` maximum shield vitality | V1 (nonzero on the cyborg) |
| `234` | materials (`0x48`, name at `+0`, `u16` type at `+0x24`: an index into Halo 1's material types, `arsenal::MATERIALS` order) | V1 (Blood Gulch: boulders stone 2, trees wood 4, the cyborg cyborg armor 21, the warthog rubber 8, metal 7 and glass 9) |
| `240` | regions (`0x54`): name, `+0x38` destroyed effect, `+0x48` permutations (`0x20`, a name) | V1 |
| `24c` | modifiers (`0x34`), counted only | S |
| `280` | pathfinding spheres (`0x20`): i16 node (-1 none), `+0x10` centre, `+0x1c` radius | V1 |
| `28c` | nodes (`0x40`): name, `+0x20` i16 region, `+0x22` parent, `+0x24` next sibling, `+0x26` first child, `+0x34` BSPs (`0x60`, the structure's collision BSP form) | V1 (every BSP validates) |

A node's BSP `i` belongs to permutation `i` of its region (V1: never more
BSPs than permutations).

### `phys` (root `0x80`)

`08` mass, `0c` centre of mass, `18` density, `1c` gravity scale, `50`
moments, `5c` inertia matrices (2 x `0x24`), `68` powered mass points
(`0x80`), `74` mass points (`0x80`: name, `+0x20` powered mass point,
`+0x22` model node, `+0x2c` mass, `+0x34` density, `+0x38` position,
`+0x68` radius). V1 (strides; finite positions).

## Building Halo 2 tags (Design)

**Scenery has no template since converter version 50**: the `scen`, its
`hlmt` and its `mode`'s material list are written field by field from
typed definitions, with the base's scenery role words in the unknown
spans and Halo 1's model and bitmaps as predicted resources
([tag-definitions.md](../formats/tag-definitions.md), "Scenery from definitions").
The scenery template (Delta Halo's `statue_shard_large`) described below
is history.

**Weapons, equipment, projectiles and damage have no template since
converter version 51**: they are built from their typed definitions,
with Halo 1's values and no template bytes ([tag-definitions.md](../formats/tag-definitions.md),
Weapons, equipment, projectiles and damage). The template rows below
for `weap`, `eqip` and `proj` only name the Halo 2 counterpart whose
references (HUD, effects, sounds, melee damage) fill the slots Halo 1
has nothing converted for.

**Vehicles have no template since converter version 49**: every Halo 1
vehicle is built from the `vehi` definition, with its friction and hover
points from Halo 1's powered mass points and its gears from its type's
role ([tag-definitions.md](../formats/tag-definitions.md), "Every vehicle from
definitions"). The vehicle rows described below (warthog, ghost,
scorpion, banshee, pelican, dropship, wraith, gun turret, lifepods) and
the lifepods' quiet stand-in are history, kept for the record.

**Where a vehicle meets the ground (converter version 63).** Three
things the engine reads at a vehicle's base, proved in
`h1objects/tests/vehicle_ground_disc.rs`:

- **Bounding sphere.** Halo 1 centres every vehicle's sphere on its
  origin (`obje +0x08` zero on all of them), and its origin is where the
  wheels or hull touch the ground: every Blood Gulch vehicle placement
  is 1 mm above the floor. Halo 2 lights an object from that centre
  (E: `0x000d1850` passes `obj +0x30` to `0x000d2a50`, which casts from
  0.01 above it straight down 10 units, `0x004406e4` and `0x0045dc70`,
  and reads the lightmap where the ray meets the structure) and finds
  its cluster from it, then from the origin (`0x000b8600`: `obj +0x40`,
  then `obj +0x64`; with neither in a cluster the object is outside the
  map, `obj +4` bit 18, and the player's action search `0x00151110`,
  which walks the clusters round the player, never offers it). A centre
  a few millimetres under the ground leaves the vehicle unlit and,
  outside the map, unboardable. The retail warthog's centre is 0.29
  above its origin, inside the body; the ghost, banshee, scorpion and
  wraith keep theirs on the origin (the wraith's 3 cm under its model),
  so retail holds no general rule, and a centre on the contact point is
  a centimetre of sinking from dark. A rebuilt vehicle's sphere is its model's: the middle of the box
  round every vertex, reaching the farthest one
  (`rebuild::model_sphere`).
- **Entry facing.** `0x000c9040` admits a rider only when the entry
  marker's forward points back at the rider's eye within the seat's
  `+0x9c`. Retail entry markers face out of their door (the warthog's
  `driver_enter`: (0.15, 0.85, 0.5)); Halo 1's `driver enter` faces the
  vehicle's forward, so with the warthog role's 70 degrees a rider square
  beside the door was refused; only one ahead of it, looking back, got in. Rebuilt seats hold pi there
  (`rebuild::SEAT_ENTRY_FACING`), as the retail turrets do.
- **Spawn height.** The ghost's hull reaches 0.143 below its origin, so
  at Halo 1's placement it began 0.14 into the floor, where Halo 1's
  hover mass points lifted it and Halo 2 left it. The Halo 1 sandbox
  raises each vehicle spawn by how far its model reaches below its
  origin (`h1convert::convert::halo1::vehicle_depth`). The Halo 2
  sandbox raises each retail vehicle the same way, by the lowest z of
  its render model's position bounds
  (`h1convert::placements::retail_vehicle_depth`: the retail ghost
  0.15, the warthog 0.02, the wraith none).

**Functions and what they scale (vehicles).** Halo 1 scales an
attachment (a looping engine sound, a headlight) by one of the object's
four outputs (attachment `+0x30`: 1 to 4, A to D); output *k* is the
object's function *k*, on every Halo 1 vehicle "one" scaled by an input
and bounded. An input is the vehicle's source where it names one
(`vehi +0x31c`), else the unit's (`unit +0x198`): the warthog's
headlights the unit's driver's seat power, its taillights the vehicle's
jump, its engine the vehicle's engine hack; the pelican's three sounds
engine hack, hover and thrust. Halo 2 names it instead: the attachment's
`+0x10` is evaluated (`0x000bab40`) as `one`/`zero`, then as an import
name of the object's type handlers (`0x00108d90`: vehicle `0x000f6d10`,
unit `0x000c6b90`, object `0x000bcc20`), then as a function's export
(`obje +0x64`, `+8`); a name nothing knows is 0, and a looping sound at 0
is off. So each Halo 1 function becomes a Halo 2 function
(`h1objects::functions`): its input's Halo 2 import name (Halo 2 keeps
Halo 1's names: the retail warthog's taillights scale by `jump`), export
`a_out` to `d_out`, Halo 1's invert and always-active flags, turn-off by
the export of the function Halo 1 names, and data an identity or linear
function whose range is Halo 1's bounds. Each attachment then names its
output's export. An output that doesn't convert leaves its attachments
unscaled (1, as before), with a note. Before this, every rebuilt
vehicle's attachments were unscaled: the engine sounds a constant hum
and the lights always on (tests: `h1objects/tests/object_functions_disc.rs`).

Each object is built on the **retail Halo 2 tag that plays the same role**
in the base map (`convert::TEMPLATES`): assault rifle on the SMG, pistol
on the magnum, plasma rifle, needler, grenades on Halo 2's grenades,
health pack on the overshield, cyborg on `masterchief_mp`, warthog and
ghost on Halo 2's. For campaign bases (05a_deltaapproach): pelican on the
pelican, Covenant dropship on the phantom, wraith on the wraith, the
Covenant gun turret on `c_turret_ap`, lifepods on the pelican (by role:
Halo 1's lifepod is the escape pod, a small flying craft, not Halo 2's
`insertion_pod`, the ODST drop pod. The scripts fly it with recordings,
load units into it, open, close, attach and animate it, and never hand it
to the player: `lifepod_use` in h1campaign's survey, below); placed elites, grunts, jackals and
engineers on Halo 2's elite, grunt, jackal and drone, every other placed
biped (marines, Keyes, the pilot) on `masterchief`; scenery on Delta
Halo's `statue_shard_large` (a static rock with collision and no `phmo`). The template's root is copied; the fields below get
Halo 1's values; the model reference becomes a converted `hlmt`; the rest
stays the template's. Template child blocks that are not rebuilt keep
their retail addresses (aliasing, as retail does 13,099 times), so the
tags must go into a `TagSet` made from that base map.

Through version 47, lifepods kept none of the pelican's looping sounds
(`convert::QUIET_STAND_INS`, removed with the vehicle template path). The pelican's attachments (`obje +0x94`,
`0x18` each) loop six sounds, engine, hover and banking on each side,
scaled by its `engine_audio`, `hover_audio` and `banking_audio`
functions; on a lifepod they hummed the pelican's engine at every
placement, all level (a10 at converter version 14: "repeated
wurrrring"). A lifepod gets its own copy of the attachments without the
`lsnd` elements (its lights and thruster effects stay the template's)
and a report line per sound dropped. Halo 1's own looping sounds are not
converted, so lifepods are quiet. Other craft keep their template's
sounds: those are the same vehicle's.

Lifepod use (**S**, `lifepod_use`, 2026-11-17): a30's `lifepod_entry`
(5 placements, the intro pods) is named in `recording_play` (5),
`recording_play_and_delete` (6), `unit_enter_vehicle` (2),
`object_teleport` (16), `objects_attach` (3) and `custom_animation` (1)
calls; a10's `lifepod_docked` and `lifepod` in `objects_attach`,
`custom_animation`, `unit_open`/`unit_close` and teleports; b30's and
a30's plain `lifepod` palette entries have no placement. No call drives
one with a player. So the template is a flying unit with passenger
seats that a recording can fly: the pelican, on 05a; the model and
animations stay Halo 1's.

**Model chain.** `hlmt` = the template's `hlmt` root with: render model
-> converted `mode` (`h1models`, no shaders yet), collision -> converted
`coll`, animation -> converted `jmad` (`h1models`; null when the Halo 1
object has none, as on the weapons), **physics model: the template's
`phmo`, borrowed** (warthog's for the warthog, ghost's for the ghost,
`masterchief`'s for the cyborg; Coagulation's weapons and equipment have
none). The borrowed `phmo` is dropped (null) when its rigid bodies
(`phmo +0x38`, `0x90` each, node `i16` first) name a node past the
converted model's count: the physics model writes a matrix per rigid
body's node into the object's `hlmt`-sized array (Nodes, below). That is
why humans take the Chief, whose `phmo` stays inside a Halo 1 human's 20
nodes, and not the marine (node 36); grunts, monitors, flood forms and
small devices then have no physics model. A vehicle left with none gets one built
from its Halo 1 collision (tag-definitions.md, Physics models from Halo
1), since `vehicle_new` refuses a model without one. Variants are emptied (count 0); regions are rebuilt from the
converted model's region and permutation names. `obje default_variant`
is set to 0.

**Nodes** (`hlmt +0x78`, `0x5c` each) are the converted `mode`'s, one
for one (`h1models::mode::hlmt_nodes`): name, parent, first child, next
sibling, `+0xa` zero, default translation and rotation, scale 1, the
inverse model-space basis and position. V (05a: SMG, warthog, banshee,
Master Chief): a retail `hlmt` node is its `mode` node's first `0x5c`
bytes with `+0xa` zero. E: `object_new` reserves `0x34` bytes of node
matrices per `hlmt` node, at least one (`0x000b7ef3`, `0x000b800b`,
object `+0x114`); the model, graph and physics write one per *their*
node. With the nodes emptied (the first version), every multi-node
object wrote past its body into the next object in the pool; a30's
first game run faulted on the clobbered header of the next item
(`0x0010b96e`, `mov edx,[ecx+0BCh]` with the definition pointer null).

**`coll`** is built from scratch: materials and names as string IDs
(lower case, spaces to underscores), each region permutation holding the
BSP of every node of that region for that permutation, each BSP the same
narrowing copy as the structure's (`h1convert::collision::convert`) with
the node index in the BSP's leading word (V: on retail objects the word
is a node index in range, `0, 2, 3` on a five-node weapon; the engine's
use is not established). Pathfinding spheres and nodes copy across.
Halo 1's and Halo 2's collision BSPs hold the same tree; the difference
is the strides (Halo 1 `12, 16, 8, 8, 20, 12, 24, 16` per element of
the eight blocks, Halo 2 `8, 16, 4, 4, 16, 8, 12, 16`: 32-bit indices
narrowed to 24-bit 3D children and 16-bit flagged indices), and Halo 2's
`0x44`-byte element puts a word before the eight blocks. Proof:
`campaign_collision_converts_and_answers_as_halo1` (below).

**Collision for Havok: BSP physics** (2026-11-17, converter version 18).
Until version 18 every converted object had its `coll` (BSPs, regions,
nodes) but scenery, whose template has no `phmo`, had no BSP physics
slots either, so Havok could not see it and the player walked through
a10's props (the round oxygen tanks, barricades, computer banks: a10 has
no tag named crate; its scenery with collision is ten tags). Havok moves the player's biped, vehicles and loose
items, and it sees an object through the `hlmt`'s `phmo` or, with none,
through the `coll` permutation's BSP physics slots: one `0x70`-byte slot
per BSP, a Havok shape over the BSP inside a MOPP tree whose code keys
the BSP's surfaces by index ([collision.md](../formats/collision.md), "BSP
physics"). Retail writes the slots exactly for the `coll`s of scenery
with no `phmo` (V, every map on the disc: a Delta Halo statue, trees;
not weapons, equipment, devices, bipeds or crates). So:

- a scenery object with no `phmo` (through version 84 every converted
  one; since 85 only those whose Halo 1 collision has no solid cell or
  whose base has no scenery `phmo`) gets its `coll` with one slot per BSP: the box of the BSP's vertices at
  `+0x10`/`+0x20`, the `coll`'s own datum at `+0x30`, and MOPP code from
  `crates/mopp` over the BSP's surfaces, keyed `0..n` (`coll::bsp_mopp`),
  at `+0x64`;
- every other object gets its `coll` without slots, as retail. A Halo 1
  `coll` that both kinds use is built twice (the second named with
  `_bsp_physics` or `_phmo`).

**Shared blocks and b40.** A BSP's eight child blocks and a slot's MOPP
code are shared by content (`Block::shared`, a hash of the bytes): Halo 1
levels carry identical collision under several names (b40's three
granite boulders), and the slots alone add 37,280 bytes on b40 (38
slots; a10 13,152 for 15), which put b40 0x8000 over the tag memory
bound even at the last lean step. With sharing b40 converts at lean 1
(as before: the empty squads dropped) with tag memory `0xc22000` of
`0xc24000`. A shared block can lie outside its tag's payload, so
`tags::collision::Collision::read` decodes against the whole metadata
when a block is out of the tag's range (`Collision::decode_at`).

Through version 84 no scenery `phmo` was built: Halo 2's own static
scenery (the statue the scenery template is) has none and blocks through
these slots. But the player walked through converted Blood Gulch's
boulders with their slots in place, so since version 85 scenery with
solid Halo 1 collision gets a built `phmo` and no slots
([tag-definitions.md](../formats/tag-definitions.md), "Scenery from
definitions"). Vehicles likewise meet bipeds through their `phmo`: the
converted scorpion's treads, in its `coll` but left out of its `phmo`
through version 85, were walked through; since 86 a vehicle's wheels and
treads are in its body above the hull's lowest point where its retail
role's physics models have wheel shapes (the scorpion's treads; not the
warthog's tires; tag-definitions.md, `convert::held_wheels`).

**`hlmt` region bytes.** Each `hlmt` region (`0x10`) says which `coll`
region it uses (`i8` at `+4`) and which physics region (`+5`); each
permutation (`8`) its `coll` permutation (`+5`) and physics permutation
(`+6`); -1 is none (V, retail; [collision.md](../formats/collision.md)). Until
version 18 these were all 0. Now (`convert::region_map`): the `coll`
region of the same name (string ID form), else the one at the same
index, else -1; the permutation likewise, then the region's first.
Physics bytes stay 0 (retail never holds -1 there on an object with no
`phmo`). A `coll` region no render region selects is listed in
the report.

**Physics.** No `phmo` is built from Halo 1's `phys` (BSP physics, above,
covers objects without one): Halo 2's physics
models are Havok shapes with runtime post-processing ([collision.md](../formats/collision.md)),
and a faithful one from mass-point spheres is a separate job. The
borrowed `phmo` binds rigid bodies to Halo 2's node layout, which the
converted model does not share (open question below).

### Halo 2 offsets written

| Tag | Offset | Field | Evidence |
|---|---|---|---|
| `obje` | `04`, `08` | bounding radius, offset | E |
| `obje` | `30`, `34` | default variant, model | E |
| `weap` | `150`..`164` | heat recovery, overheated, detonation threshold, detonation fraction, heat loss per second, illumination | S, V2 (set on the plasma rifle, zero on the SMG; the overheated effect follows at `16c`) |
| `weap` | `208`..`218` | `cone_angle`, `cone_range`, `pull_angle`, `pull_range`, `bend_angle` | E |
| `weap` | `2c0` | magazines, `0x5c` | V2 (stride: magazine objects start `0x5c` after) |
| magazine | `04`, `06`, `08`, `0a`, `10`, `14` | rounds recharged, total initial, total maximum, loaded maximum, reload time, rounds reloaded | S, V2 (the loaded maximum and rounds reloaded agree on every magazine) |
| magazine | `0c` | the most rounds held in reserve: total maximum less loaded maximum | E (`0x101a10` caps the reserve with it, [skulls.md](../game/skulls.md)), V (all seven retail magazines on Coagulation). Written 0 until converter version 76: no reserve, the HUD count 0. Live (version 77, Blood Gulch): the assault rifle spawns with 180 in reserve, and emptying the magazine and reloading leaves 120 |
| `weap` | `2c8` | triggers, `0x40`: `04` i16 input, `06` i16 firing behaviour (0 automatic, 1 once per press, 2 charging), `08`/`0a` i16 primary and secondary barrel (-1 none), `0c` i16 (1, 0, 2 with those behaviours, but 0 on the wraith's automatic minigun) | V (every retail weapon with a barrel has one; 27 on Coagulation). Built from Halo 1's triggers (`arsenal::h2_triggers`): one per Halo 1 trigger (at most two, inputs 0 and 1), once per press where Halo 1's trigger flag `0x8` is set (the rocket launcher, sniper rifle, scorpion cannon), automatic otherwise; a charging first trigger (`+0x48` > 0, the plasma pistol) with a second is one charging trigger whose secondary barrel is the second. None were written from the rebuild (`dac88db6`) to converter version 76, so no converted Halo 1 weapon fired. The charging trigger also carries Halo 1's charge after its charging time (`+0x4c..+0x5c`: charged time, overcharged action, charged illumination, spew time) as `+0x20..+0x2c` (`arsenal::H1_TRIGGER_CHARGED`; the plasma pistol's 45, 0, 1.0, 0; retail's holds 0, 0, 1.0, 0; S for the names); before converter version 79 they were 0. The charging effect (`+0x30`) is not carried: no Blood Gulch trigger names one. `+0x10..+0x18` (retail's plasma pistol: 0.0667, 0.5, and the i16 pair 1, 0) have no Halo 1 field and stay 0 |
| `weap` | `2d0` | barrels, `0xec` | E (objects.md) |
| barrel | `04`, `0c`, `10` | rounds per second (2), acceleration, deceleration time | S, V2 |
| barrel | `28`, `2a`, `2c`, `2e` | magazine, rounds per shot, minimum rounds loaded, rounds between tracers | S, V2 |
| barrel | `38`, `3c` | error acceleration, deceleration time | S |
| barrel | `68`, `6a`, `6c` | distribution function, projectiles per shot, distribution angle | S, V2 |
| barrel | `70`, `74` | minimum error, error angle (2) | S, V2 (radians) |
| barrel | `7c`, `8c`, `a4` | first-person offset, projectile, heat per round | S, V2 (`8c` is a `proj` reference) |
| `proj` | `c0`..`e4` | detonation timer starts, impact noise, AI perception radius, collision radius, arming time, danger radius, timer (2), minimum velocity, maximum range, detonation noise | S, V2 (grenade-only fields zero on bullets) |
| `proj` | `164`..`184` | air gravity scale, air damage range, water gravity scale, water damage range, initial, final, guided angular velocity | S, V2 (bullet speeds hundreds per second; grenade gravity set) |
| `eqip` | `12c`, `130` | powerup type, powerup time | [health.md](health.md) |

Halo 1 speeds are multiplied by 30 (per tick to per second).

### Weapon class and type (`weap +0x288`, `+0x28c`, **Built**, converter version 55)

A unit takes a weapon, picked up or given at spawn, only when its
animation graph holds it. `0x000cd6a0` (called by `unit_add_weapon`
`0x000cd0c0` at `0x000cd12e`, and on the pickup paths at `0x000e8a63` and
`0x000ea475`) passes the weapon's class (`+0x288`) and type (`+0x28c`)
string IDs, with mode `current` (sid `0x101`) or the seat's, to
`0x0010fcd0`; that asks `0x001cc3f0` for action `idle` (sid `0xc`) and
refuses the weapon when there is none (E). Retail: the SMG `rifle`/`smg`,
the magnum `pistol`/`hp`, the rocket launcher `missile`/`rl`, vehicle guns
`fixed`/`gun` (V, Coagulation). The rebuilt weapons left both null, so in
the Halo 1 sandbox the player could pick nothing up and so never fired.

Halo 1 keeps only the type on the weapon (its label, `+0x30c`); the class
is where the units' graphs put that type (`stand rifle ar idle`). The
converter (`Converter::weapon_class`) takes, over every animation of every
graph on the map, the class most of them name the label under, through
the same naming the converted `jmad` tree uses (`h1models::names`): the
assault rifle `rifle`/`ar`, pistol `pistol`/`hp`, shotgun `rifle`/`sg`,
plasma rifle `rifle`/`pr`, sniper rifle `rifle`/`sr`, rocket launcher
`missile`/`rl`, needler `pistol`/`ne`, plasma pistol `pistol`/`pp`. A label
no graph holds (Derelict's flamethrower, `ft`) gets neither, with a
report line. V: `tests/sandbox_disc.rs`
(`every_halo1_weapon_is_held_by_the_halo1_player_graph`): on every
multiplayer map, every Halo 1 hand weapon of the Halo 1 sandbox names a
class and type the converted player biped's graph holds in mode `combat`.

**Holding is finding** (converter version 64). The graph lookup
`0x001db170` finds the mode, class, type and action each by binary
search over signed string IDs (`0x001dd560`; the type falls back to
`any`), so a graph holds a weapon only if every level of its tree is in
string-ID order. Through version 63 the converted trees were in name
order, which hides names from the search: on Blood Gulch at version 60
the cyborg's graph found `idle` and `throw_grenade` for none of the eight
Halo 1 weapons (a scratch probe over the imported map, the search as
below), and all 13 converted graphs (the cyborg's, the eight first-person
graphs, the vehicles') were out of order while all 59 retail ones were in
order. So the cyborg refused every weapon:
the Halo 1 sandbox's "no starting weapon", "can't pick up weapons" and,
with nothing held, no first-person arms or weapon. The test held only "is in the tree".
Now the trees are laid out by string ID when the map is built
([models.md](models.md), `jmad`), and the disc tests look each weapon up
as the engine does: `idle` under `combat`, its class and its type, and
`idle` in its first-person graph, and `throw_grenade` under `combat`,
its class and its type. Halo 1 has no class fallback for the throw: it
finds a unit's animations through slot tables (per unit, weapon class
and type, a list of animation indices), and on the cyborg every class's
throw slot names `stand rifle throw-grenade` (V: Blood Gulch's cyborg
`antr`, every unit and class). The converter carries those slots over:
a slot naming another class's animation becomes an action of the
borrowing class naming the same animation (`jmad::borrow_slots`), so the
pistol, plasma pistol, needler and rocket launcher throw with the rifle's
animation, as in Halo 1. The disc test fails on any weapon of any
multiplayer map whose path finds no throw.

### Model materials (`hlmt +0x58`, **Built**, converter version 54)

`hlmt +0x58` holds the model's materials, `0x14` each. The engine maps a
hit on an object to a global material through it: `0x000d8ae0` (ecx the
object, esi an index, eax the output) goes object tag `+0x38` (the `hlmt`)
to `+0x5c` (this block's address) and reads element `esi`'s `u16` at
`+0x10`, with no bound check (E). Its one caller, `0x001695fb` in
`0x00169510`, tests a ray against an object's model and passes the hit's
material index. An empty block faults there reading `[0 + esi*0x14 +
0x10]`: the Halo 1 sandbox's crash at pc `0x000d8b33`, address `0x10`,
after a few seconds of play (the player on the health pack), since every
converted `hlmt` left the block empty.

Retail (V, `tests/hlmt_materials_disc.rs` on coagulation, beavercreek and
05a, 100 `hlmt`s, 197 elements):

| Offset | Field | Evidence |
|---|---|---|
| `+0` | name: the `coll`'s material `k` (every `hlmt` with a `coll`: 80 of 80 in order, never fewer elements) | V |
| `+4` | two `i16`s; `0xffff0000` most often (67 of 197) | V |
| `+8` | low `i16` `k`, high `-1` (185 of 197) or 0 | V |
| `+0xc` | the global material's name (empty on some; then `+0` names it) | V |
| `+0x10` | `u16`: the index in the globals' materials (`matg +0x150`) of the `+0xc` name, else of `+0` (197 of 197); high half 0 | V |

A `phmo`'s shapes name material indices below the count on 60 of 62
(retail's exceptions: powerhaus, pelican_rear_gun, invisible_box, the
phantom).

The converter (`rebuild::model_materials`) writes one element per
converted `coll` material, in order: the material's name, `+4`
`0xffff0000`, `+8` `k | 0xffff0000`, the Halo 2 global material of its
Halo 1 type (`arsenal::MATERIALS`) and that material's index in the base's
globals. A `phmo` the converter builds (its shapes name material 0) adds
an element for each of its materials past the `coll`'s; a borrowed retail
`phmo` keeps the template's elements past the `coll`'s, each named as its
global material. `h1objects::verify::hlmt_materials` checks the rule on a
built map; `tests/sandbox_disc.rs`
(`every_hlmt_holds_its_materials_in_both_sandboxes`) and
`h1campaign/tests/disc.rs` (`every_campaign_level_converts`) run it on
every converted map against what the retail base already breaks.

### Change colours (`obje +0xac`, **Built**, converter version 84)

Halo 2 colours a player by its object's change colours, not by its
shader. `object_new` keeps `0x18` bytes per change colour at object
`+0x124` (initial and current RGB). `0x000bacc0` sets one (esi the index,
edi the RGB; bound-checked against the definition's count, both halves
written), and `0x000bad50` reads it (E). `0x0014bfc0` writes the player's
four colours into slots 0 to 3 through `0x000bacc0`. The colours come from
`0x0015f330` and `0x0007f790`, which turn appearance bytes 0 to 3
(primary, secondary, tertiary, quaternary) into RGB through the palette at
`0x001a06a0`. In a team game `0x0007f790` makes slot 0 the team's colour
(`0x0007f720`) and slot 1 the player's own primary. When that fails, and
for every object that isn't a player, `0x000be240` resets the colours
from the definition: white (`[0x468710]`) unless the slot has initial
permutations. Then it picks one by weight among those whose variant
(`+0x1c`) is 0 or the object's, and a colour between its bounds
(`0x000be40e..0x000be44b`). The `obje` block at `+0xac` (`0x10` each)
holds those permutations (`block_0`, `0x20` each: `+0` weight, `+4` lower
RGB, `+0x10` upper RGB, `+0x1c` variant string id) and the functions
(`block_8`) (E). So the count must reach the slots the shaders read.

The shader reads them as constants. The Chief's stem
`opaque\tex_bump_env_four_change_color` draws base x detail x 2 x the
product over `k` of `lerp(1, colour k, change map channel k)`: red slot
0, green 1, blue 2, alpha 3 (V2, its colour pass on coagulation's three
instances; the layout and their values: `h1convert::shad`,
`TEX_BUMP_ENV_FOUR_CHANGE_COLOR`).

Halo 1 (V, every object tag on the disc's Blood Gulch, a10 and the
multiplayer maps): a `soso` names its change colour source at `+0x4c` (0
none, 1 to 4 = A to D) and masks it with its multipurpose map's blue (on
the cyborg it covers exactly the white armour plates; red is the
reflection mask, green near 0). An object's change colours are at
`obje +0x164` (`0x2c` each: `+0` darken by, `+2` scale by, `+4` flags,
`+8` and `+0x14` the function's bounds, `+0x20` permutations of `0x1c`:
weight, lower, upper). The cyborg's armour, visor and first-person hands
read C, which is Halo 1's player colour (the campaign cyborg's C is the
Chief's green; the multiplayer cyborg's A and B have only functions).
The scenery, the Scorpion's darkened hull (grey 0.45), the Covenant and
doors read A; marines' faces, the flood and the crewman's flesh B; the
cyborgs, the cryo tube and the cyborg helmet C. No object reads both A
and C.

The converter (**Design**):

- A `soso` with a change colour that isn't cut out is drawn with
  `tex_bump_env_four_change_color` (the black default cube when it has no
  reflection, as retail's `elite_fp_leather` is). Its change map is the
  multipurpose blue moved into the channel of the Halo 2 slot
  (`h1bitmaps::mask::channel_into`, DXT5, tag
  `<mask>_change_color_<slot>`); every other channel is 0, so the shader
  multiplies the rest by 1. A cut-out one (the trees' leaves) stays
  without: there is no alpha-tested change-colour stem in the base.
- Slots: A and C go to 0, B to 1, D to 2
  (`h1convert::shad::halo2_change_colour_slot`). C is the player's colour
  in both games, and no object reads both A and C, so one object's slot 0
  is C when its model's shaders read C and A otherwise
  (`h1::halo2_slots`).
- Every object with a Halo 1 change colour, or whose model reads C, gets
  four elements (`rebuild::object_change_colors`): the player writer
  needs slots 0 to 3. Each holds the Halo 1 permutations as initial
  permutations (weight, lower, upper; variant 0). Halo 1's darken-by and
  scale-by functions are not carried.

So the cyborg's armour plates take the player's primary colour, or the
team's in a team game. Halo 1's cyborg has one mask, so the secondary
colour has no area to show on. Halo 2 does not team-colour vehicles; a
vehicle keeps its own colour (the Scorpion's grey). Proof:
`h1bitmaps` `a_multipurpose_blue_becomes_one_change_colour_channel`,
`h1convert` `halo1_change_colours_go_to_halo2_slots` and
`the_change_colour_layout_puts_the_mask_and_the_reflection_where_the_stem_reads_them`,
`h1objects` `change_colours_go_to_halo2_slots_with_c_first_when_read`,
`rebuilt_scenery_carries_its_change_colours`, and the disc tests
`tests/model_shaders_disc.rs` (`bloodgulch_cyborg_and_scorpion_take_change_colours`)
and `tests/rebuild_disc.rs`.

### Tuning carried over

Every field in the table above is written from Halo 1 for each object:
the weapon's heat and aim assist, each magazine's counts and reload time,
each trigger's rate of fire, error, spread and projectile (as a barrel),
each projectile's radii, timer, range, gravity, damage ranges and
velocities, the health pack's powerup type and amount, and every object's
bounding sphere. The values themselves are retail data and are not
committed: `bloodgulch_sandbox_converts_and_reads_back` prints, for every
object, each field with the Halo 1 value, the value written and the
template value it replaced (`ObjectReport::tuning`). On Blood Gulch the
rows that change play most are the assault rifle's reserve ammunition and
reload time, every weapon's error angles and cone ranges, the plasma
rifle's heat, the needler's magazine and rate of fire, and every bullet's
speed (Halo 1's are slower than the Halo 2 templates').

## What has no Halo 2 home here

- **Damage** (`jpt!` amounts): the projectile keeps the template's damage
  effects. Halo 1's `jpt!` bodies are not read.
- **Vitality:** Halo 1 `coll` body and shield vitality
  and its damage modifiers; Halo 2 keeps damage in the `hlmt`, whose
  layout is not reviewed. The template's damage info stays.
- **Unit and vehicle tuning:** speeds, acceleration, seats, cameras,
  boarding; `bipd`/`vehi` keep the template's. Halo 1 `phys` (mass points)
  is read but not used: the template's `phmo` is.
- **Weapon:** zoom levels (template's kept; the magnum happens to be 2x
  like the pistol), ready time, chamber time, charging (plasma pistol),
  first-person model and animations, the HUD (`wphi` vs `nhdt`), firing
  effects and sounds: all the template's.
- **Projectile:** material responses and every effect and sound: the
  template's. Contrails are Halo 1's (above).
- **Equipment:** non-health powerup and grenade type values (template's).
- **Grenade throwing** reads the projectile from the globals' grenade
  block (`matg`), which this crate does not touch; the converted grenade
  projectiles are built but only thrown once the globals point at them.

Stopgap references (every retail effect, sound, damage effect, HUD and
`phmo` kept from a template) are listed per object in
`ObjectReport::stopgaps`; Blood Gulch's proof set keeps 93.

**Campaign bases** have no multiplayer biped or powerups. Each Halo 1
tag tries every matching row in table order (`template_candidates`) and
takes the first the base holds: the biped falls back to
`objects\characters\masterchief\masterchief`, the health pack to the frag
grenade (its powerup type and time are still Halo 1's;
[campaign.md](campaign.md) §9 converts a30 onto Delta Halo
this way).

## API

```rust
use h1objects::{base::Base, convert::Converter};

let mut h2 = map::Map::from_reader(base_bytes)?;          // e.g. coagulation
let base = Base::read(&mut h2)?;
let mut conv = Converter::new(&h1_map, &base);
let weap = conv.object(h1_map.find(h1map::Group::new(b"weap"), "weapons\\assault rifle\\assault rifle").unwrap())?;
let tree = conv.tree;                                      // tags + per-object reports
let mut set = map::build::TagSet::from_map(&mut h2)?;
let ids = tree.install_over(&mut set, &donor_slots)?;      // or tree.install(&mut set) on an unpinned set
```

- `h1::{Weapon, Projectile, Equipment, Unit, Collision, Physics}::read(map,
  tag)`, `h1::H1Object::read` for any object tag, `refs::scan` for any tag.
- `coll::convert_collision(&Collision, bsp_physics) -> (Converted,
  CollisionReport)`; `coll::bsp_mopp`, `coll::bsp_box` for one BSP.
- `query::{locate, ray}`: the 3D tree's answer for a point (leaf or
  solid) and the first solid point along a segment, on either game's BSP
  (`query::Tree` for `tags::collision::Bsp` and `h1map::bsp::CollisionBsp`).
- `Converter::object(tag)` converts an object (and its projectiles, model,
  collision and animations, each once) into `Converter::tree`, a `Tree` of
  `(name, h1models::Converted)` whose cross references are placeholders
  (`convert::local(k)`).
- `Tree::install(set)` adds slots (fine on a `TagSet::new`).
  `Tree::install_over(set, donors)` reuses existing slots: **a pinned base
  cannot grow its tag index** (`TagSet::build` refuses), so on a base map
  the tags must take over slots the converted map no longer uses. A
  pinned donor's bytes stay in place as unowned metadata, so blocks other
  tags share with it remain valid. The proof uses Coagulation's 86
  `scenarios\` bitmaps for its 52 tags.

## Tests

```sh
cargo test -p h1objects                       # synthetic: templates, coll, tree install
H1_ISO=<halo1 iso> H2_DISC=disc/halo2.iso cargo test -p h1objects --test disc -- --ignored --nocapture
```

`tests/collision_disc.rs` (`H2_MAPS=` limits the Halo 2 maps,
`H1_LEVELS=` the Halo 1 levels, default a10 and b40):

- `retail_static_scenery_carries_bsp_physics`: the slot rule, each
  slot's code keying `0..surfaces` and its box holding the BSP, on every
  object of every Halo 2 map; prints the `hlmt` region bytes seen.
- `campaign_collision_converts_and_answers_as_halo1`: every `coll` on the
  level converted with BSP physics, built, read back and validated; on
  every BSP, 64 rays and 64 points through its box (a quarter wider) give
  the same answer from the read-back Halo 2 tree as from Halo 1's, and
  every surface within a centimetre of a ray's entry point is a key the
  slot's code emits there. A crate's level walk through its middle stops.
- `level_objects_select_collision_with_bsp_physics` (`H1_LEVEL=`,
  default a10): the level's objects through the converter on 05a: slots
  on scenery `coll`s, region 0 of every `hlmt` with a `coll` selects a
  collision region; prints the bytes the slots add.

`bloodgulch_sandbox_converts_and_reads_back` converts the twelve tags of
the proof set (52 Halo 2 tags with their models, collisions, animations,
projectiles and `hlmt`s), installs them over donor slots in Coagulation,
builds the map, and reads it back: every object decodes through
`tags::object::Context::resolve_object` with its `hlmt` -> `mode` (all),
`coll` (all with a Halo 1 collision model), `jmad` (cyborg, warthog,
ghost) and `phmo` (cyborg, warthog, ghost); every converted `coll` passes
`tags::collision::Collision::validate`; and every reference word pair in
the new tags (136) names a tag of that group in the built map.

`tests/particles_disc.rs`: `every_pctl_part_becomes_particle_systems`:
every `pctl` effect part (201 over the conversions) becomes one system
per lasting particle state (402), on that state's `prt3`, at the part's
location, the type's count over the event, the state's lifespan and size.
`every_effect_particle_becomes_a_particle_system`:
every Halo 1 effect particle whose `part` converts (42,266 over the
campaign and multiplayer conversions) is a particle system of its event
on the converted `prt3`, at its location, its rate times 30 times the
event's duration its count, its speed, cone and size Halo 1's.
`tests/particles_disc.rs`: `every_particle_converts`: every Halo 1
`part` of the campaign and multiplayer maps (2,301 conversions) reads
back and writes the same, names its blend function's stem, its bitmap as
parameter and compiled bitmap, Halo 1's sequences and converted effects.
`retail_particle_shaders_compile_from_their_stems`: the compiled shader
rule on 533 retail particles.

`tests/particles_disc.rs`: `every_contrail_converts`: every Halo 1
`cont` of the campaign and multiplayer maps (191) converts, reads back
through the definition with Halo 1's spans at Halo 2's offsets and its
bitmap converted; every rebuilt projectile and vehicle whose Halo 1
attachments name a contrail (236 attachments) names the converted one.

## Needs a game run

- Particle systems (`pctl`): every state at once instead of in
  sequence, and still particles (no creation physics): whether explosions
  read as explosions.
- Particles: the spawn count over an event, the size factor (twice the
  radius), the packed tint, null `pmov` (no gravity or drag), the base's
  role words on the systems and emitters.
- Contrails: Halo 1's spans as Halo 2's (the flags and the root's
  zeroed `+0x3c..+0x60`), and points with no physics (`pphy` null).

- That the engine accepts objects whose `hlmt` has no variants
  and regions renamed from Halo 1, and whose template damage info names
  regions the converted model lacks.
- The borrowed `phmo`s against Halo 1 skeletons (node counts and names
  differ: rigid bodies may attach to the wrong nodes or none).
- That the S offsets above mean what the values suggest (heat, magazine,
  barrel and projectile fields), and the per-tick to per-second factor.
- Halo 1's tuning on Halo 2's physics: projectile gravity (Halo 1 bullets
  have gravity scale 1, Halo 2's 0), maximum range 0 on the plasma bolt.
- The `coll` BSP leading word as a node index, and Halo 1 material names
  as Halo 2 materials (the global material table will not know them).
- That the player stops at a converted crate (BSP physics slots, version
  18), and that Havok bodies (grenades, vehicles) bounce off scenery.
  **Not so on converted Blood Gulch** (converter version 83, game run
  2026-11-19): the player walked straight through `boulder` (placement at
  (37.93, -88.69, 0.09); the path passed 0.23 from its centre, its
  collision box half 0.67 by 0.60) with no Havok contact but the level's.
  The boulder's `coll` has its BSP physics slot, and the engine had
  written the slot's vtables at load (`0x00456194` at `+0`, `0x00455540`
  at `+0x40`, `0x00455f58` at `+0x50`), yet `havok.bodies` lists no
  body owned by any of Blood Gulch's 50 scenery placements (only the two
  fixed bodies with no owner), and a10 holds none either. So the slots
  alone do not put converted scenery in the Havok world. Retail
  Coagulation, Blood Gulch's base, gives every scenery with collision a
  `phmo` (6 of 6, V: `h2 object coagulation ...`; the trees' rigid bodies
  on node 0 or -1). Since converter version 85 scenery with solid Halo 1
  collision gets one, built as devices' are, with the base's scenery
  body words (tag-definitions.md, "Scenery from definitions", Physics).
  Which object code adds a slots-only scenery's shapes to the world is
  not found (the slot shapes are built by `0x00246c60` from the `coll`
  post-process `0x002122b0`; no reader of slot `+0x40` or `+0x50` was
  found in a sweep of `0x70`-stride loops).
- Template seats, turrets (Halo 2's warthog gunner is a separate turret
  vehicle) and animation names against the converted `jmad`.
