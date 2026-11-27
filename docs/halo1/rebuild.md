# Halo 1 objects rebuilt from definitions (`h1objects::rebuild`)

Status: Built, not run in the game. How the Halo 1 converter writes scenery,
vehicles, bipeds, devices, physics models, effects, weapons and particles
from the typed definitions of [tag-definitions.md](../formats/tag-definitions.md)
instead of patching a Halo 2 template. The sections below keep their
evidence labels: **E** engine code, **V** checked on the owner's disc, **D**
a naming or interpretation choice (labels of
[map-format.md](../formats/map-format.md)). Dated game runs and the
disc measurements behind them are in [notes/rebuild.md](notes/rebuild.md).

## Scenery from definitions (`h1objects::rebuild::scenery`)

Every Halo 1 scenery object becomes three tags written from the
definitions (`rebuild::scenery`, `Converter::scenery`), with no retail
template read:

- **`scen`**: `default_record`, then the object type (6), Halo 1's
  bounding radius and offset, the model, the predicted resources (the
  converted `mode` as kind 2 index -1, then every converted bitmap its
  shaders draw as kind 0, as devices and vehicles), the change colours,
  and the attachments and creation effect
  ([Object attachments](objects.md#object-attachments)).
- **`hlmt`**: the render, collision and animation models, no physics
  model, the nodes (`h1models::mode::hlmt_nodes`) and the regions with the
  collision and physics region and permutation of each (`region_map`).
- **`mode`**: converted by `h1models::mode`, written from zero, its
  **material list** rewritten from the definition: one material per
  Halo 1 shader, the shader set or null, everything else at its default.

**Physics.** Converted scenery has no `phmo`; its `coll` keeps one BSP
physics slot per BSP, as retail scenery with no `phmo` does, and each
placement's collision is merged into the level's instead
(`h1convert::scenery_collision`, docs/halo1/multiplayer.md, "Collision").
Retail holds both forms (V: 05a 11 scenery with collision and no `phmo`,
5 with one; beavercreek 6 and 3; coagulation 0 and 6). The engine never
gives scenery a Havok body, so a `phmo` on scenery would be inert. E:
the only place an object gets a physics component and rigid bodies from
its `phmo` is `0x001c35f0` (it calls `0x001cf0b0` for the component at
`0x001c3667`, then `0x001cf120`), and it returns at `0x001c361c` unless
the object's type (object table entry `+3`) is in the mask `0x1883`:
biped 0, vehicle 1, machine 7, crate 11, creature 12. Scenery (6) never
passes. `0x001cf120` itself switches on the type (byte table
`0x001cf20c`, jump table `0x001cf1f4`) and has cases for exactly those
five; every other type's index is 5, whose jump-table entry is zero. The
other callers of `0x001cf120` (`0x000c54d1` for units, `0x000f1053`,
`0x001c37d2`, `0x001d3ffb`) rebuild a component the object already has.
Rigid bodies are built in two places only, the level's at `0x001c3c32`
and an object component's at `0x001d72c2`. Run: a converted Blood Gulch
lists no Havok body for any scenery placement and a player put at a
boulder's middle drops through it ([notes/rebuild.md](notes/rebuild.md),
"Scenery has no Havok body").

Retail does not rely on scenery for solid props either. V, coagulation
(Blood Gulch's base): no tag on the map names a rock or a boulder, so
its rocks are structure; its scenery are the flag and teleporter bases,
bushes and trees; and its one solid placed prop, the `powerhaus`, is a
crate (`bloc`) whose `phmo` body holds motion `5` (Havok motion 7,
fixed), flags 0, on node 0. A converted Halo 1 scenery object with solid
collision is therefore solid to bipeds and vehicles as structure
(instanced geometry merged into the level's collision); a crate with a
fixed body is not built.

**Proof, V** (`tests/rebuild_disc.rs`):

```sh
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso \
  cargo test --release -p h1objects --test it -- rebuild_disc:: --skip unit_rebuild_disc:: --ignored --nocapture
```

- `every_scenery_rebuilds_from_its_definition`: every scenery object
  (the ten levels on 05a, the multiplayer maps on coagulation and
  beavercreek) converts; `scen`, `hlmt` and `mode` read back through the
  definitions, write the same again, pin nothing, and name only converted
  tags. Every scenery shader belongs to the tree; no report holds a
  template stopgap; the type, bounds, model chain (collision and
  animation exactly where Halo 1 has them) and region count are Halo 1's;
  every `coll` has a BSP physics slot per BSP; every model is predicted
  first, its bitmaps after.
- `retail_scenery_has_bsp_physics_and_predicts_its_model`: the retail
  facts above (both physics forms, the model predicted first, kind words
  `0xffff0002` for `mode` and `0` for `bitm`).

## Vehicles with no template (`h1objects::rebuild::vehicle`)

A Halo 1 vehicle is written from its `vehi` definition whether or not a
Halo 2 vehicle resembles it. a10 has five with no Halo 2 counterpart: the
cryotube, the pilot and pod chairs, the fighterbomber and the Halo ring.
The scripts seat the player in the cryotube and open it as the opening
cinematic ends (`unit_open cryotube_1`); a name left unplaced would fault
there, which `h1scripts::compile::NONE_UNSAFE` guards against for any
unplaced name.

- **Object header**: object type 1, Halo 1's bounding radius and offset,
  the model, and the predicted resources: the converted `mode` (`+0` kind
  2, `+2` -1), then every converted bitmap its shaders draw (kind 0, index
  0). V: every retail vehicle on the disc lists some, its `mode` once in
  that form and first, its bitmaps in that form.
- **`hlmt` and `mode` materials**: as rebuilt scenery (above). Collision is
  Halo 1's converted `coll`; the animation graph Halo 1's converted
  `antr`; the physics model built from Halo 1's collision (below).
- **Physics model**: `vehicle_new` (`0x000eeb80`) returns false when the
  model's `hlmt +0x20` physics model is null (`cmp [hlmt+0x24],-1` at
  `0x000eebcb`), and `object_new` then deletes the vehicle, so every
  vehicle has one. It is Halo 1's collision as Havok shapes (Physics
  models from Halo 1, below): one rigid body on the object (node -1)
  holding every collision node's solid cells, placed by the model's
  default pose, of Halo 1's `phys` mass. A vehicle with no solid collision
  (a10's fighterbomber and Halo ring have no `coll`) takes the stand-in
  from `rebuild::sphere_physics`: one body (node -1), one 0.05 world unit
  sphere of 1000 kg on the origin, one material and one region.
- **Vehicle type** (`vehi +0x1f0`, named below): Halo 1's own type, read
  from its `vehi`. `ROLE_VEHICLE_TYPE` (turret, Halo 2's stationary
  vehicle) is the type written for a unit with no Halo 1 vehicle type of
  its own. Turrets hold no wheels, treads, hover points or engine raw
  (`+0x268`, `+0x2e8`, `+0x2f0` empty on every one), and their one
  physics element (`+0x2f8`) is zero but for four `-1` halves
  (`TURRET_PHYSICS_ROLE`, written when a vehicle has no powered mass
  points).
- **Seats** (`+0x1c8`): one per Halo 1 seat (Halo 1's `unit +0x2e4`,
  `0x11c` each, V on a10, b30 and c10: label `+4`, marker `+0x24`, camera
  marker `+0x84`, each 32 characters). The Halo 2 seat takes Halo 1's
  `label` as the rider's animation mode (`names::mode_name`: a10's
  `CT-driver` is `c_turret_ap_d`; the scripts' seat text maps to the same
  mode, `h1scripts::seats`), the marker at `+8`
  and the camera marker at `+0x60` (D: retail's values there are marker
  names, `driver`, `gunner`, `camera_rider`).
- **Entry words** (`TURRET_SEAT_ROLE`): a Halo 1 seat has no entry
  distance or angle, and the engine admits no rider at zero
  (`0x000c9040`), so every seat takes the one set of entry radius, cone
  angle, facing angle and maximum relative velocity that retail's turret
  seats hold, and `-1` for the invisible seat region.
  `tests/unit_rebuild_disc.rs`, `turret_role_is_the_retail_turrets_majority`,
  recomputes the table from every distinct retail turret.
- **Everything else** is the definition's default: zero, null, empty.
  Blocks left empty that some retail vehicles fill: the unit's camera
  tracks and HUD, the seats' HUD, damage. A Halo 1 vehicle with no seats
  (a10's Halo ring, b30's falling box) has none in Halo 2 either; three
  retail vehicles on 06a_sentinelwalls have none, so
  `every_campaign_level_converts`' sweep accepts an empty `vehi +0x1c8`.
  Halo 1's seat tuning that has no established Halo 2 home is not carried
  (listed in the object report).

Fields named for this (V, the disc proof below round-trips all 71 retail
vehicles byte-identical with no pinned values):

| Field | Evidence |
|---|---|
| `vehi +0x1f0` `vehicle_type` (`enum16`: tank, jeep, type_2, dropship, hover, flier, turret) | E: objects.md (vehicle type). V: the names by the retail vehicles holding each value: 0 scorpion, 1 warthog, 3 pelican, phantom and gravity throne, 4 ghost, wraith, spectre and creep, 5 banshee, 6 the turrets; none holds 2 |
| `vehi +0x1c8` seat `label` | V: the scenario's seat-mapping masks select seats by it (`h1scripts::seats`, on 05a) |
| `vehi +0x284` `suspension_sound`, `+0x294` `foot_ref_294` | V: objects.md lists snd! and foot here; the group word is `snd!`/none and `foot` and the datum null on every retail vehicle, so the schema could not see them. As unknown spans a fresh tag held zero, which names tag 0 |

Proof:

```sh
cargo test --release -p h1objects --test it unit_rebuild_disc:: -- --ignored --nocapture
cargo test --release -p h1campaign --test disc a10_cryotubes -- --ignored --nocapture
```

- `a10_vehicles_without_templates_are_rebuilt`: all five a10 vehicles
  convert; each reads back through the definition and writes the same
  tree; nothing is pinned; the predicted resources are the tree's `mode`
  then its bitmaps; the model chain (`hlmt`, `mode`, `coll`,
  `jmad`) is in the tree; the `hlmt` has the model's nodes; one seat per
  Halo 1 seat with its label and marker; the one physics element; the two
  null references null.
- `a10_cryotubes_are_placed_and_built_from_their_definition` (pinned and
  fresh): `cryotube_1` and `cryotube_2` name vehicle placements whose
  palette entry is the cryotube; read back out of the converted map it
  decodes and encodes byte-identical, its `hlmt`, `mode`, `coll` and
  `jmad` are tags in the map, and it has one seat, labelled
  `c_turret_ap_d`, at marker `driver`.

What to watch in the game: the player seated in the cryotube during
a10's opening, and getting out when the scripts open it. The first
suspects if that goes wrong: the physics model (a dynamic body: a
vehicle whose shape is not on a floor may drop or drift); and
the rider's pose, since retail seat labels
(`warthog_d`) also read like the riders' animation mode names (D) and the
player's graph may have no mode for the cryotube's seat.

## Every vehicle from definitions (`h1objects::rebuild::vehicle`)

Moved to [Rebuilt Halo 1 objects: vehicles and bipeds](rebuild-vehicles.md#every-vehicle-from-definitions-h1objectsrebuildvehicle).

## Bipeds from definitions (`h1objects::rebuild::biped`)

Moved to [Rebuilt Halo 1 objects: vehicles and bipeds](rebuild-vehicles.md#bipeds-from-definitions-h1objectsrebuildbiped).

## Devices (`mach`, `ctrl`; `h1objects::rebuild::device`)

`crates/formats/tags/src/defs/devi.txt`: `struct devi` (`0..0x11c`, the object
header and the device layer) and the `mach` (`0x134`) and `ctrl`
(`0x140`) roots. All 135 retail machines and 14 controls round-trip
byte-identical. The disc has no `lifi` tags, so there is no `lifi`
definition. Named fields and their evidence are in the file; the ones
the conversion writes:

| Field | Evidence |
|---|---|
| `devi +0xbc` `device_flags` | E: object_new reads it for device types; bit 0 set on the retail devices whose position wraps |
| `devi +0xc0..+0xd4` six rates: power, power acceleration, position, position acceleration, depowered position, depowered position acceleration | E `0x00115431`/`0x00115437`: the position step is `position_rate` and `depowered_position_rate` blended by the device's power, times the seconds per tick, so they are rates per second. V: no retail device holds 0 in an acceleration slot; 900 stands for "at once" |
| `devi +0x10c` `delay_time`, `+0x118` `activation_radius` | V: set on retail doors only, in seconds and world units, the values Halo 1's doors hold |
| `mach +0x11c` `machine_type` (door, platform, gear) | E `0x001153ee`: type 2 wraps the position at 1.0; `0x0011557e`: type 0 runs the door timer. V: retail doors 0, lifts and rails 1, flywheels and hinges 2 |
| `mach +0x11e` `machine_flags`, `+0x120` `door_open_time`, `+0x12c` `collision_response`, `+0x12e` `elevator_node` | E reads (objects.md); V: Halo 1's doors and lifts hold the same values at `mach +0x292`, `+0x294`, `+0x2e8`, `+0x2ea` |
| `ctrl +0x11c` `control_type`, `+0x11e` `control_trigger`, `+0x120` `control_value`, `+0x124` `action_string` | E reads (objects.md) |

Halo 1's devices (`h1objects::h1::Device`, V across the ten levels):
the device layer is `0x17c..0x290` (flags `+0x17c`; six times in seconds
`+0x180..+0x198`, in the order of Halo 2's rates; delay `+0x200`;
activation radius `+0x21c`), then the machine's type `+0x290`, flags
`+0x292`, door open time `+0x294`, collision response `+0x2e8` and
elevator node `+0x2ea`, or the control's type, trigger and value at
`+0x290..+0x298`. A time `t` becomes the rate `1/t`; a zero time is a
zero rate, or 900 in an acceleration slot. A light fixture (`lifi`) is
written as a machine of type platform (no behaviour of its own); its
light is not carried. Sounds and effects stay null (Halo 1's are `effe`
tags the converter does not build). Havok, which moves the player, sees a
device only through a physics model, and Halo 1's have none; without
one a closed door would show, animate and stop bullets (its `coll`) but
not block the player. A device with
solid collision therefore gets a `phmo` built from it (below): one rigid body per
collision node, on the model node of the same name, so each part moves
with its node as the door opens, as the disc's doors have (V:
`high_small_door`, a body on each of its three moving nodes). Counted
per level, 86 of the ten levels' 156 device tags get one; the rest have
no collision or none with solid space (most light fixtures).

## Physics models from Halo 1 (`h1objects::physics`, `rebuild::physics_model`)

A Halo 1 collision BSP's 3D tree ends in leaves (open space) and missing
children, which are solid (`h1objects::query`). Every root-to-missing
path is a set of half-spaces; clipped to the BSP's vertex box, their
intersection is a convex cell, and the cells together are exactly the
solid space. `physics::solid_cells` walks the tree and returns each
non-empty cell: its corners (the intersections of three of its planes
inside all of them), its face planes, volume, centroid and inertia.
`collision_disc.rs` (`halo1_solid_space_becomes_physics_pieces`) checks
this on every collision model of the ten levels (16,189 cells): just
outside a surface Halo 1's own tree finds open space 51,729 times and
solid 179, just inside solid 50,591 times and open 1,317 (parts thinner
than the probe step, open meshes); at 142,528 points through the BSP
boxes the tree and the cells agree on all but 117 (points on a face).

`rebuild::physics_model` writes the `phmo` from the definition: each
cell a polyhedron, a body with several a list of them, then the bodies,
one material, one region and permutation naming every body, and the
model's nodes (parent, next sibling, first child) when a body names one.
The layout follows the disc's polyhedron physics models (V:
`high_small_door`, `ghost`, `pelican`):

- a polyhedron of up to 12 corners keeps them inline at `+0x60` in up to
  three groups of four transposed points (capacity `0x80000003`); a
  bigger one takes its groups from the root `four_vectors` block, in
  polyhedron order (only those consume it), capacity `0x80000000 |
  groups`. Planes come from the root `plane_equations` block, in order,
  for every polyhedron; `+0xfc` is the plane count with bit 31 set.
- a list of up to four children keeps them inline (capacity
  `0x80000004`); a longer one takes them from `list_children`, capacity
  `0x80000000 | n`. Retail rigid bodies name a list directly 125 times,
  so no MOPP is written over it.
- shape header `+8` 1.0, `+0xc` 0.9, `+0x10` 0.6, `+0x14` the piece's
  volume, `+0x18` its mass; rigid body `+0x1a` 2, `+0x1e` 5 (retail
  bodies hold 1 to 6 there, 5 on most doors and vehicles; meaning not
  established), `+0x20` 1.0, `+0x28` 0.05; body mass, centre of mass and
  inertia are the pieces' at the body's density.
- the material is `hard_metal_thick_cov` with header `+6` 76, the pair
  the disc's `high_small_door` holds (D: `+6` reads as an index into a
  global material table that is not named here). Halo 1's collision
  materials are not carried.
- the shapes' Havok headers and pointers stay zero; the engine writes
  them at load (`0x001ea9c0`).
- no block of a `phmo` is shared with another tag
  (`tags::repo::build::IN_PLACE`). The loader turns each rigid body's and
  list child's `{type, index}` reference into a pointer in the block
  itself, so a block two tags share is resolved twice and the second
  pass reads a pointer as a reference: the resolver (`0x001eaf30`)
  returns null for it, and the list's bounding box faults reading its
  first child (`0x002fc139`). A fresh map shares equal blocks between
  tags by content, so tags with equal list children (a10's cryotube
  vehicle and machine) or equal rigid bodies (three a10 doors) would
  share. `physics_models_load_as_the_engine_reads_them`
  (h1campaign `disc.rs`) checks every converted map's physics models
  against the loader: no rewritten block shared, every reference in
  range, the list-children, four-vector and plane runs filling their
  blocks.

A device's mass is its cells' volume at 1000 kg per cubic metre
(`convert::DENSITY`, per cubic world unit). A body of more than 64 cells
(`physics::MAX_PIECES`; no built body on the ten levels reaches it, the
most is b30's falling box with 62) keeps one enclosing piece
per node instead: the intersection of the cells' own faces that hold
every corner and of 26 axis, edge and corner planes moved out to the
farthest corner. Not done: the `block_30` pairs retail doors hold
(adjacent bodies' nodes, D), constraints, and Halo 1's `phys` mass
points.

Placement (`h1campaign::scenario`): Halo 1's machines (`+0x294`, `0x40`
each), controls (`+0x2ac`, `0x40`) and light fixtures (`+0x2c4`, `0x58`)
and their palettes land in Halo 2's machine (`+0xa8`, `0x48`) and
control (`+0xb8`, `0x44`) blocks, fixtures after the machines. After the
object fields both games hold the power group, position group (`i16`
indices into the device groups, -1 none), the device flags and the
machine's or control's flags: Halo 1 at `+0x28`, Halo 2 at `+0x34`. The
device groups are `h1scripts::level`'s (`+0xa0`, in Halo 1's order); a
conversion that leaves out the level data (`Lean::LevelData`) writes
every group index as -1.

Proof:

```sh
H2_DISC=... H2_DEFS_GROUPS=mach,ctrl cargo test --release -p tags --test tag_defs -- --ignored --nocapture
H1_ISO=... H2_DISC=... cargo test --release -p h1campaign --test disc every_device_placement -- --ignored --nocapture
```

`every_device_placement_lands_with_its_device_groups`: on every level,
every Halo 1 machine, control and fixture placement lands (1,596
machines, 840 fixtures, 77 controls on the disc), at Halo 1's position and
rotation, with its palette entry a `mach` or `ctrl` that re-encodes
byte-identical, its power and position groups Halo 1's and below the
converted device group count, and its flags; all 496 Halo 1 names of
devices name their placement (a fixture's names its machine).

## Effects from definitions (`h1objects::rebuild::effect`)

Moved to [Rebuilt Halo 1 objects: effects, weapons and particles](rebuild-effects.md#effects-from-definitions-h1objectsrebuildeffect).

## Weapons, equipment, projectiles and damage (`h1objects::arsenal`)

Moved to [Rebuilt Halo 1 objects: effects, weapons and particles](rebuild-effects.md#weapons-equipment-projectiles-and-damage-h1objectsarsenal).

## Particles and contrails (`prt3`, `cont`)

Moved to [Rebuilt Halo 1 objects: effects, weapons and particles](rebuild-effects.md#particles-and-contrails-prt3-cont).

## Which converted objects use the definitions

| Groups | Path | Why |
|---|---|---|
| `vehi` | definitions (only path) | no vehicle template path |
| `mach`, `ctrl`, Halo 1's `lifi` (as `mach`) | definitions (only path) | no template path |
| `phmo` of a rebuilt vehicle or device | definitions (Halo 1's collision as polyhedra; a stand-in sphere with no solid collision) | `vehicle_new` refuses a model with none; Havok sees a device only through one |
| `scen` | definitions (only path) | no scenery template path |
| `bipd` | definitions (only path) | Halo 1's values; the shield shader, damage sections and Halo 2 HUD view from the Halo 2 counterpart |
| `phmo` of a rebuilt biped | definitions (Halo 1's collision as polyhedra, a body per node, joined as a ragdoll) | Halo 1 gives no ragdoll |
| `weap`, `eqip`, `proj`, and the `jpt!` the projectiles deal | definitions (only path) | Halo 1's values; not run in the game |
| `effe` (effect parts, vehicles' attachments and creation effects) | definitions (only path) | no template path; sound, light, decal, damage, object and particle-system parts |
| `ligh`, `deca` (effect parts, attachments) | definitions (only path) | no template path |
| `lens` (lights', sky lights'), `garb` (effect parts) | definitions (only path) | no template path |
