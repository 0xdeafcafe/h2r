# Halo 1 objects: building Halo 2 tags

How the converted object tags are built (Design), split from [objects.md](objects.md).

| Section |
|---|
| [Building Halo 2 tags (Design)](#building-halo-2-tags-design) |

Part of [objects.md](objects.md).

## Building Halo 2 tags (Design)

**Scenery has no template**: the `scen`, its
`hlmt` and its `mode`'s material list are written field by field from
typed definitions, with the base's scenery role words in the unknown
spans and Halo 1's model and bitmaps as predicted resources
([tag-definitions.md](../formats/tag-definitions.md), "Scenery from definitions").

**Weapons, equipment, projectiles and damage have no template**: they are built from their typed definitions,
with Halo 1's values and no template bytes ([tag-definitions.md](../formats/tag-definitions.md),
Weapons, equipment, projectiles and damage). The template rows below
for `weap`, `eqip` and `proj` only name the Halo 2 counterpart whose
references (HUD, effects, sounds, melee damage) fill the slots Halo 1
has nothing converted for.

**Vehicle drive.** Each vehicle comes from the `vehi` definition with
Halo 1's model, collision, seats, markers and powered mass points, on the
drive tuning of the matching Halo 2 vehicle: the warthog and rocket
warthog on the warthog, the ghost on the ghost, the scorpion on the
scorpion, the banshee on the banshee (`rebuild::DRIVE_COUNTERPARTS`,
declared per vehicle as `SWAPS` is). The counterpart's gears, engine,
steering, ground and hover laws and point tuning are copied from the base
map; the powered mass points name and place the points. A vehicle with no
counterpart is a named error, except a prop a cutscene or recorded
animation moves, a scripted set piece, a dropship and a turret, which keep
Halo 1's speeds and turn limits. Vehicles have no template row. [Drive
counterparts](role-audit.md#native-engine-fields).

**Vehicle seats.** A Halo 1 seat pairs with a counterpart seat by kind
(driver, gunner, passenger, read from the seat flags) and then order
within the kind; extra seats take the last of their kind, a kind the
counterpart lacks takes its first seat (`rebuild::PAIR_SEATS`,
`rebuild::counterpart_seat`). The seat takes the counterpart's entry
prompt, HUD interface, AI role and built-in gunner
(`rebuild::SEAT_PROMPT_FIELDS`), and its entry radius, facing angle and
relative-velocity limit (`rebuild::SEAT_ENTRY_FIELDS`). Halo 1 supplies
the turn rates, as the upper bounds of `yaw_rate_bounds` and
`pitch_rate_bounds` (`h1::Seat::rates`, `+0x7c` and `+0x80`, inferred
from the camera marker at `+0x84`). The entry cone stays pi on every seat
(Entry facing, below). Only a turret, a vehicle with no counterpart,
holds `rebuild::TURRET_SEAT_ROLE`; every seat holds no hidden region.

**Lifepod engine audio.** The a30 entry pod attaches the Halo 1
`sound\sfx\vehicles\lifeboat` loop. Its marker is empty and its primary
scale is zero. The rebuilt attachment names that level's converted loop,
whose track plays `lifeboat_entry`. The other lifepod variants have no
source engine attachment. The campaign disc test compares the ordered
loop attachments of converted vehicles with their source and checks a30's
entry pod explicitly. [Campaign audio evidence](sounds.md#campaign-loops-and-cache-slots-built).
### Native pickups and scripted props

`h1objects::convert_powerups` builds Halo 1's overshield, active
camouflage and health pack as fresh `eqip` and model chains.
The source provides the bounding sphere, pickup time, model, collision,
pickup sound and effect references. The paired retail `eqip` tags on
`shared` and `single_player_shared` establish the item sort word and
powerup fields (V). Overshield is type 2 and 60 seconds, camouflage type 3
and 45 seconds, and health type 5 with a positive time. Halo 2's pickup
routine reads those fields at `0x00151eff` and `0x00151f3e`, dispatches
shield at `0x00151f4b`, health at `0x00151f6a` and camouflage at
`0x00151f85` (E). The multiplayer conversion rebuilds the collections in
both settings ([multiplayer.md](multiplayer.md#what-isnt-placed-and-why)).

In `h1objects::scripted_props` and the campaign placement writer, b30's
seven boxes are dynamic `bloc` instances with the Halo 1
storage crate's model and collision. Halo 1's vehicle tag references a
`phys` for the mass and leaves the object's radius at zero. The rebuilt
crate takes its bounds from its model and its shape and inertia from
the collision hull. Halo 2's `crate_space` on 05a establishes a
dynamic `bloc` with one object-space body (V). a10's Halo ring is a
`scen`, as in Halo 2's 00a. The ring and boxes keep their scenario names,
transforms and script-controlled creation.

a10's seated props and aircraft: the cryotube and chairs
keep their source stationary vehicle type 6 and seats. a10's scripts call
`unit_enter_vehicle` or `vehicle_load_magic` on them (V).
Their collision bodies follow the model's nodes, including the cryotube
door. The fighter-bomber is a type 3 aircraft for `recording_play`. Its
`phys` has eighteen mass point spheres, which form one native compound
body. Halo 1 supplies mass, centre of mass and inertia.

Lifepods: the flying pods use Halo 1's type 5, model,
collision, seats and flight rates. Where Halo 1 supplies `phys` (the
plain pod and a30 entry pod), the collision-derived rigid body keeps its
mass, centre of mass and inertia tensor. The flight hook applies H1's positive-radius lifepod controller at
`0x37760` when the source supplies two powered groups.
The docked pod keeps its stationary body and source lights. a30's entry pod
attaches its own `lifeboat` loop, including the source track samples,
through `h1_assets::sounds::looping`, and its reentry fire through the native
`effe` attachment on `burn`. The plain and docked tags carry no engine
loop (V). Offline checks are in `vehicle_rebuild_disc` and
`h1campaign/tests/disc.rs`.

**Where a vehicle meets the ground.** Three
things the engine reads at a vehicle's base, proved in
`crates/tools/h1objects/tests/vehicle_ground_disc.rs`:

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

**The Blood Gulch ghost.** The converted `mode` keeps the ghost's `__base`
and `~damaged` permutations in that order and its `hlmt` has no variants and
no damage sections (`hlmt +0x60` empty), so nothing selects the damaged
model: E, the region permutation update (`0x0016d660`) gathers candidates
only from the object's variant (`0x0016dad0`, through `hlmt +0x54`), and with
none the region keeps permutation 0, the intact one. At rest the ghost lies
on its hull, as in Halo 1: its `phmo` is Halo 1's collision with Halo 1's
`phys` mass, and neither physics shape hovers without a driver. Driven, a
hover point whose marker is on any node but the root lifts nothing, and every
friction and hover marker of every retail vehicle is on node 0, so every
powered point's marker is (tag-definitions.md, "Every vehicle from
definitions"; `crates/tools/h1objects/tests/vehicle_rebuild_disc.rs`,
`every_powered_point_rides_the_root_node`). The hover points' positions match
the retail ghost's within 0.06. The game-run figures are in
[notes/objects.md](notes/objects.md).

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
output's export (tests:
`crates/tools/h1objects/tests/object_functions_disc.rs`).

**Periodic functions.** Halo 1's function kinds 2 to 11 are waves:
cosine, diagonal wave and slide, each with a variable-period form, then
noise, jitter, wander and spark. Halo 2's periodic blob has the same
twelve wave slots (`0x00468d48`), and a kind converts to its own slot.

| Step | Halo 1 | Halo 2 (`0x000ba8c0`) |
|---|---|---|
| Argument | game seconds over the period | game seconds (`0x000ba9cb`), frequency `1 / period` |
| Wave | table *k* | wave *k*, set 0, unranged (range input is 1.0) |
| Scale by an input | times the input, then bounded | bounded, then times the import (`0x000baa27`) |
| Bounds and result scale | clip, normalise or scale to fit | set 0's minimum and maximum, the header range |

The two orders agree when the bounds start at 0 and the scaled wave
never clips (scale to fit, or result scale at most the upper bound);
any other scaled wave is refused by name. So are a scaled period, a
wobble, an invert (Halo 2's time path skips it, `0x000ba95e`), a map-to
or a threshold on a wave, and kind 1 (zero) (`functions::periodic`).
Unit test: every kind against a Halo 1 reference over the same table
(`halo1_periodic_functions_match_halo1_through_halo2s_evaluator`).

Not verified: that Halo 1's table bytes equal Halo 2's
(`halo1_wave_table_is_halo2s` searches Halo 1's executable for Halo 2's
slots 1 to 11), and that Halo 1 gives a wave no per-object phase (Halo 2
adds none, as record flag bit 3 is clear). The disc test
`periodic_and_input_objects_convert_or_name_their_reason` converts the
needler, the crashed lifepod, the burning flame and the monitor, and
lists the waves they export.

Weapons, equipment, grenade projectiles and bipeds name the **retail
Halo 2 tag that plays the same role** (`convert::TEMPLATES`, first match;
`template_candidates` lists every match in table order and the converter
takes the first the base holds): assault rifle on the SMG, pistol on the
magnum, plasma rifle, needler, grenades on Halo 2's grenades, health pack
on the overshield, cyborg on `masterchief_mp`. For campaign bases
(05a_deltaapproach): placed elites, grunts, jackals and engineers on
Halo 2's elite, grunt, jackal and drone (`bugger`), every other placed biped
(marines, Keyes, the pilot) on `masterchief`. Vehicles (the pelican, Covenant
dropship, wraith, turrets, lifepods), scenery and devices have no template.
A lifepod is Halo 1's escape pod, a small flying craft, not Halo 2's
`insertion_pod`, the ODST drop pod: the scripts fly it with recordings, load
units into it, open, close, attach and animate it, and never hand it to the
player (`lifepod_use` in h1campaign's survey, below). Template child blocks
that are not rebuilt keep their retail addresses (aliasing, as retail does
13,099 times), so the tags must go into a `TagSet` made from that base map.

Lifepod use (**S**, `lifepod_use`): a30's `lifepod_entry`
(5 placements, the intro pods) is named in `recording_play` (5),
`recording_play_and_delete` (6), `unit_enter_vehicle` (2),
`object_teleport` (16), `objects_attach` (3) and `custom_animation` (1)
calls; a10's `lifepod_docked` and `lifepod` in `objects_attach`,
`custom_animation`, `unit_open`/`unit_close` and teleports; b30's and
a30's plain `lifepod` palette entries have no placement. No call drives
one with a player.

**Model chain.** `hlmt` = the template's `hlmt` root with: render model
-> converted `mode` (`h1_assets::models`, no shaders yet), collision -> converted
`coll`, animation -> converted `jmad` (`h1_assets::models`; null when the Halo 1
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
for one (`h1_assets::models::mode::hlmt_nodes`): name, parent, first child, next
sibling, `+0xa` zero, default translation and rotation, scale 1, the
inverse model-space basis and position. V (05a: SMG, warthog, banshee,
Master Chief): a retail `hlmt` node is its `mode` node's first `0x5c`
bytes with `+0xa` zero. E: `object_new` reserves `0x34` bytes of node
matrices per `hlmt` node, at least one (`0x000b7ef3`, `0x000b800b`,
object `+0x114`); the model, graph and physics write one per *their*
node. With the nodes emptied, every multi-node object would write past its
body into the next object in the pool, faulting on the clobbered header of
the next item (`0x0010b96e`, `mov edx,[ecx+0BCh]` with the definition
pointer null).

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

**Collision for Havok: BSP physics.** An object with a `coll` but neither
a `phmo` nor BSP physics slots is invisible to Havok, so the player would
walk through it (a10's round oxygen tanks, barricades and computer banks are
scenery with collision). Havok moves the player's biped, vehicles and loose
items, and it sees an object through the `hlmt`'s `phmo` or, with none,
through the `coll` permutation's BSP physics slots: one `0x70`-byte slot
per BSP, a Havok shape over the BSP inside a MOPP tree whose code keys
the BSP's surfaces by index ([collision.md](../formats/collision.md), "BSP
physics"). Retail writes the slots exactly for the `coll`s of scenery
with no `phmo` (V, every map on the disc: a Delta Halo statue, trees;
not weapons, equipment, devices, bipeds or crates). So:

- a scenery object (it has no `phmo`) gets its `coll` with one slot per
  BSP: the box of the BSP's vertices at `+0x10`/`+0x20`, the `coll`'s own
  datum at `+0x30`, and MOPP code from `crates/formats/mopp` over the BSP's
  surfaces, keyed `0..n` (`coll::bsp_mopp`), at `+0x64`;
- every other object gets its `coll` without slots, as retail. A Halo 1
  `coll` that both kinds use is built twice (the second named with
  `_bsp_physics` or `_phmo`).

**Shared blocks and b40.** A BSP's eight child blocks and a slot's MOPP
code are shared by content (`Block::shared`, a hash of the bytes): Halo 1
levels carry identical collision under several names (b40's three
granite boulders), and the slots alone add 37,280 bytes on b40 (38
slots; a10 13,152 for 15), which would put b40 0x8000 over the tag memory
bound even at the last lean step. With sharing b40 converts at lean 1
(the empty squads dropped) with tag memory `0xc22000` of `0xc24000`. A shared block can lie outside its tag's payload, so
`tags::collision::Collision::read` decodes against the whole metadata
when a block is out of the tag's range (`Collision::decode_at`).

**Vehicles meet bipeds through their `phmo`.** A vehicle's wheels and
treads are in its body above the hull's lowest point where its retail
role's physics models have wheel shapes (the scorpion's treads; not the
warthog's tires; tag-definitions.md, `convert::held_wheels`), so a walking
biped cannot enter the converted treads ([notes/objects.md](notes/objects.md)).

**Scenery has no body; placed scenery is structure.** The engine gives no
scenery a Havok body, from a `phmo` or otherwise (rigid bodies: only
bipeds, vehicles, machines, crates and creatures get them,
[collision.md](../formats/collision.md)), so neither the BSP slots nor a
built `phmo` would make a scenery object solid; the player put inside
`boulder_doublewide` or `boulder_crouch` drops to the ground through it
([tag-definitions.md](../formats/tag-definitions.md), "Scenery from
definitions"). Retail Blood Gulch's base keeps its rocks in the structure
and its one solid prop as a crate with a fixed body. So scenery has no
`phmo` and its `coll` keeps its BSP physics slots, as retail scenery with
no `phmo`, and every scenery placement there from the start whose object
has a collision model has that collision moved to its place and merged into
the level's collision BSP (`h1convert::scenery_collision`,
[multiplayer.md](multiplayer.md), "Collision"), the way retail builds its
rocks into the structure; the object still draws as scenery. So Havok
meets the rocks and trees through the structure's MOPP code, and the
engine's own BSP tests (bullets, projectiles, a biped's ground checks)
through the structure's 3D tree. Which objects is decided by the `coll`,
not the name. Proof: `crates/tools/h1objects/tests/scenery_collision_disc.rs`
(inside `boulder_doublewide` is solid in converted Blood Gulch, a ray from
above stops on its top).

**`hlmt` region bytes.** Each `hlmt` region (`0x10`) says which `coll`
region it uses (`i8` at `+4`) and which physics region (`+5`); each
permutation (`8`) its `coll` permutation (`+5`) and physics permutation
(`+6`); -1 is none (V, retail; [collision.md](../formats/collision.md)). They are
set by `convert::region_map`: the `coll`
region of the same name (string ID form), else the one at the same
index, else -1; the permutation likewise, then the region's first.
Physics bytes stay 0 (retail never holds -1 there on an object with no
`phmo`). A `coll` region no render region selects is listed in
the report.

**Physics.** A rebuilt vehicle's `phmo` is built from Halo 1's collision
and `phys` mass (`h1objects::physics`; [tag-definitions.md](../formats/tag-definitions.md),
"Every vehicle from definitions"). A biped borrows its template's `phmo`,
which binds rigid bodies to Halo 2's node layout, which the converted model
does not share (open question below).

### Halo 2 offsets written

| Tag | Offset | Field | Evidence |
|---|---|---|---|
| `obje` | `04`, `08` | bounding radius, offset | E |
| `obje` | `30`, `34` | default variant, model | E |
| `weap` | `150`..`164` | heat recovery, overheated, detonation threshold, detonation fraction, heat loss per second, illumination | S, V2 (set on the plasma rifle, zero on the SMG; the overheated effect follows at `16c`) |
| `weap` | `208`..`218` | `cone_angle`, `cone_range`, `pull_angle`, `pull_range`, `bend_angle` | E |
| `weap` | `2c0` | magazines, `0x5c` | V2 (stride: magazine objects start `0x5c` after) |
| magazine | `04`, `06`, `08`, `0a`, `10`, `14` | rounds recharged, total initial, total maximum, loaded maximum, reload time, rounds reloaded | S, V2 (the loaded maximum and rounds reloaded agree on every magazine) |
| magazine | `0c` | the most rounds held in reserve: total maximum less loaded maximum | E (`0x101a10` caps the reserve with it, [skulls.md](../game/skulls.md)), V (all seven retail magazines on Coagulation). Written 0 it would leave no reserve and the HUD count 0; written, the Blood Gulch assault rifle spawns with 180 in reserve, and emptying the magazine and reloading leaves 120 (Run) |
| `weap` | `2c8` | triggers of stride `0x40`. Input and behaviour at `04/06`, primary and secondary barrels at `08/0a`, charging action at `18`, charge times, action and illumination at `1c..2c`, charging effect at `30` | V on Coagulation and Blood Gulch. See “Firing a rebuilt weapon” below. |
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

### Firing a rebuilt weapon

The H1 disc has 316 weapon occurrences
across 24 maps, covering 23 distinct paths. Blood Gulch's shotgun and assault rifle both
have zero H1 trigger flags. The shotgun's weapon type is 1 at H1 `weap +4e2`,
also the type the two engines check for shell reloads (H1 `0xbf70f`, H2
`0x10271d`). The rebuild carries this to H2 `+292` and emits one shot per
press for its primary barrel. H1 flag 3 selects this mode on the sniper,
rocket launcher and Scorpion cannon. A charged trigger taps barrel 0 and
discharges the last barrel, including the single-barrel fuel rods. H2
checks its charging action at `0xfe955`. The charging effect comes from
H1 trigger `+5c`.

| Blood Gulch | AR | Shotgun |
|---|---|---|
| Input and cadence | Held, 15 rounds/s | One press, one shell/s |
| Ammunition and projectiles | 1 round, 1 bullet | 1 shell, 15 pellets |
| Spread | 2°–6.5°, rises in 0.6 s, recovers in 1 s | 10° at both error bounds |
| Magazine and reload | 60 rounds, 3.4 s | 12 shells, 0.4 s per shell |
| Projectile | 324 world units/s, 40 world units maximum | 140 to 100 world units/s, air slowdown from 1.5 to 3 world units |

H2 reads minimum and maximum shots per fire at barrel `+1c/1e`, recovery
at `+20`, and multiplayer prediction at `+34` (`0x10224f`, `0x103ec7`,
`0x10429b`). Coagulation's native shotgun has one shot per fire and ten
pellets. Its SMG and shotgun use continuous and discrete prediction,
respectively. The converted barrels keep H1's magazine cost and projectile
count independently of these fields.

H2 blends two spread profiles (`0xff474`, `0x104dd1`). Both carry H1's
bounds and acceleration/decay times and their reciprocal rates. The AR's
camera shake comes from its own firing damage effect, including the 0.05 s
duration and 0.5° rotational shake. H1 spin crossover, per-shot battery
cost and age-dependent cadence, cooling and misfire fields also survive
conversion. Barrel flags for partial ammunition, muzzle origins, zoom error,
aiming direction and shared projectile direction move from H1 bits
2/5/10/11/12 to H2 bits 2/3/5/6/7. The direct barrel damage reference at
`+94` has a null datum. H2 checks for `-1` after the projectile loop at
`0x1053f6`.

**Variable-speed H1 projectiles.** H1 computes constant acceleration
from the two speeds and the current medium's distance bounds at `0xc1a60`
and `0xc1ac0`. At impact it normalizes the measured speed between those
bounds at `0xc2de1`. H2 instead interpolates speed linearly with distance.
The converter retains the native air-distance bounds and their reciprocal
for effect inputs. It marks the zero guidance scalar at `proj +198` with
the negative-zero bit pattern. H2 compares it numerically at `0xf88df`. The 725 local projectile occurrences inspected on
the H2 disc contained no such marker. At the velocity-length call returning
to `0xf966a`, the simulation integrates the H1 curve over the collision
substep, supplies its mean velocity for the sweep and its end velocity for
the next step. At the damage initializer returning to `0xfad79`, it scales
direct impact damage by impact speed and retains material attenuation.
Both guided angular velocity bounds receive H1's single value.

`weapon_rebuild_disc` compares the firing fields, firing feedback and
projectile targets against each original weapon on the campaign and
multiplayer maps. Its Blood Gulch case also checks the retail Coagulation
trigger and barrel layouts.

### Weapon class and type (`weap +0x288`, `+0x28c`, **Built**)

Moved to [Halo 1 objects: weapon and model details](objects-build-weapons.md#weapon-class-and-type-weap-0x288-0x28c-built).

### Model materials (`hlmt +0x58`, **Built**)

Moved to [Halo 1 objects: weapon and model details](objects-build-weapons.md#model-materials-hlmt-0x58-built).

### Cut-out model shaders (**Built**)

Moved to [Halo 1 objects: weapon and model details](objects-build-weapons.md#cut-out-model-shaders-built).

### Change colours (`obje +0xac`, **Built**)

Moved to [Halo 1 objects: weapon and model details](objects-build-weapons.md#change-colours-obje-0xac-built).

### Weapon zoom

Moved to [Halo 1 objects: weapon and model details](objects-build-weapons.md#weapon-zoom).

### Ammunition counter (**Built**)

Moved to [Halo 1 objects: weapon and model details](objects-build-weapons.md#ammunition-counter-built).

### Tuning carried over

Moved to [Halo 1 objects: weapon and model details](objects-build-weapons.md#tuning-carried-over).

### Blood Gulch hit registration (**Built**)

Moved to [Halo 1 objects: weapon and model details](objects-build-weapons.md#blood-gulch-hit-registration-built).
