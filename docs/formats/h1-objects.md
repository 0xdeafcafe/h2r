# Halo 1 sandbox objects as Halo 2 tags (`crates/h1objects`)

Status: **Built** (2026-11-17) for reading every Halo 1 Xbox object tag on
the 13 multiplayer maps, and for building Blood Gulch's arsenal (assault
rifle, pistol, plasma rifle, needler, frag and plasma grenades, health
pack, cyborg, warthog, ghost) as Halo 2 tags on Coagulation. The built
tags read back through `crates/tags`. No converted object has been loaded
by the game yet ("Needs a game run").

This is the Halo 1 sandbox of [h1-multiplayer.md](../game/h1-multiplayer.md)
(phase 4) and the object step of [campaigns.md](../game/campaigns.md) §2.
Physics is Halo 2's (decision 1); Halo 1's health is kept (decision 2,
[health.md](../game/health.md)). Models and animations come from
[h1-models.md](h1-models.md); the Halo 2 object family is in
[objects.md](objects.md), collision in [collision.md](collision.md).

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
- **S:** field order and meaning recalled from general Halo modding
  knowledge (the editing kits' field names), then placed by the data. No
  community code or definition file was opened or copied.
- **E:** from [objects.md](objects.md) (engine code).
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
| `308` | u32 flags; `30c` label | S |
| `338` | f32 ready time, `33c` ready effect | S, V1 |
| `34c`..`360` | heat recovery threshold, overheated threshold, detonation threshold, detonation fraction, heat loss per second, illumination | S; V1 (set on the plasma rifle, zero on the assault rifle) |
| `374`, `384` | overheated, detonation effects | S |
| `394`, `3a4` | melee damage, melee response (`jpt!`) | V1 |
| `3bc` | actor firing parameters (`actv`) | V1 |
| `3da` | i16 magnification levels, `3dc` f32x2 magnification range | V1 (nonzero only on zooming weapons) |
| `3e4`..`3f4` | autoaim angle, autoaim range, magnetism angle, magnetism range, deviation angle | S, V1 (angles in radians, ranges in world units) |
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

### `bipd`, `vehi`, `scen`

Read as the `obje` header (plus every reference, `refs::scan`, for the
units). Their tuning is not read yet (below).

### `coll` (root `0x298`)

| Offset | Field | Evidence |
|---|---|---|
| `08` | f32 maximum body vitality; `cc` maximum shield vitality | V1 (nonzero on the cyborg) |
| `234` | materials (`0x48`, name at `+0`) | V1 |
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

`Options::rebuild` (off by default) builds scenery with no template:
the `scen`, its `hlmt` and its `mode`'s material list are written field by
field from typed definitions. See [tag-definitions.md](tag-definitions.md)
for the field-by-field comparison with the template path below.

**Vehicles have no template since converter version 49**: every Halo 1
vehicle is built from the `vehi` definition, with its friction and hover
points from Halo 1's powered mass points and its gears from its type's
role ([tag-definitions.md](tag-definitions.md), "Every vehicle from
definitions"). The vehicle rows described below (warthog, ghost,
scorpion, banshee, pelican, dropship, wraith, gun turret, lifepods) and
the lifepods' quiet stand-in are history, kept for the record.

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
the BSP's surfaces by index ([collision.md](collision.md), "BSP
physics"). Retail writes the slots exactly for the `coll`s of scenery
with no `phmo` (V, every map on the disc: a Delta Halo statue, trees;
not weapons, equipment, devices, bipeds or crates). So:

- a scenery object (every converted one: the template has no `phmo`)
  gets its `coll` with one slot per BSP: the box of the BSP's vertices at
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

No `phmo` is built: Halo 2's own static scenery (the statue the scenery
template is) has none, and blocks through these slots.

**`hlmt` region bytes.** Each `hlmt` region (`0x10`) says which `coll`
region it uses (`i8` at `+4`) and which physics region (`+5`); each
permutation (`8`) its `coll` permutation (`+5`) and physics permutation
(`+6`); -1 is none (V, retail; [collision.md](collision.md)). Until
version 18 these were all 0. Now (`convert::region_map`): the `coll`
region of the same name (string ID form), else the one at the same
index, else -1; the permutation likewise, then the region's first.
Physics bytes stay 0 (retail never holds -1 there on an object with no
`phmo`). A `coll` region no render region selects is listed in
the report.

**Physics.** No `phmo` is built from Halo 1's `phys` (BSP physics, above,
covers objects without one): Halo 2's physics
models are Havok shapes with runtime post-processing ([collision.md](collision.md)),
and a faithful one from mass-point spheres is a separate job. The
borrowed `phmo` binds rigid bodies to Halo 2's node layout, which the
converted model does not share (open question below).

### Halo 2 offsets written

| Tag | Offset | Field | Evidence |
|---|---|---|---|
| `obje` | `04`, `08` | bounding radius, offset | E |
| `obje` | `30`, `34` | default variant, model | E |
| `weap` | `150`..`164` | heat recovery, overheated, detonation threshold, detonation fraction, heat loss per second, illumination | S, V2 (set on the plasma rifle, zero on the SMG; the overheated effect follows at `16c`) |
| `weap` | `208`..`218` | autoaim angle and range, magnetism angle and range, deviation | E |
| `weap` | `2c0` | magazines, `0x5c` | V2 (stride: magazine objects start `0x5c` after) |
| magazine | `04`, `06`, `08`, `0a`, `10`, `14` | rounds recharged, total initial, total maximum, loaded maximum, reload time, rounds reloaded | S, V2 (the loaded maximum and rounds reloaded agree on every magazine) |
| `weap` | `2d0` | barrels, `0xec` | E (objects.md) |
| barrel | `04`, `0c`, `10` | rounds per second (2), acceleration, deceleration time | S, V2 |
| barrel | `28`, `2a`, `2c`, `2e` | magazine, rounds per shot, minimum rounds loaded, rounds between tracers | S, V2 |
| barrel | `38`, `3c` | error acceleration, deceleration time | S |
| barrel | `68`, `6a`, `6c` | distribution function, projectiles per shot, distribution angle | S, V2 |
| barrel | `70`, `74` | minimum error, error angle (2) | S, V2 (radians) |
| barrel | `7c`, `8c`, `a4` | first-person offset, projectile, heat per round | S, V2 (`8c` is a `proj` reference) |
| `proj` | `c0`..`e4` | detonation timer starts, impact noise, AI perception radius, collision radius, arming time, danger radius, timer (2), minimum velocity, maximum range, detonation noise | S, V2 (grenade-only fields zero on bullets) |
| `proj` | `164`..`184` | air gravity scale, air damage range, water gravity scale, water damage range, initial, final, guided angular velocity | S, V2 (bullet speeds hundreds per second; grenade gravity set) |
| `eqip` | `12c`, `130` | powerup type, powerup time | [health.md](../game/health.md) |

Halo 1 speeds are multiplied by 30 (per tick to per second).

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
reload time, every weapon's error angles and autoaim ranges, the plasma
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
- **Projectile:** material responses and every effect, sound and contrail
  (`cont`): the template's.
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
[campaigns.md](../game/campaigns.md) §9 converts a30 onto Delta Halo
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

## Needs a game run

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
- Template seats, turrets (Halo 2's warthog gunner is a separate turret
  vehicle) and animation names against the converted `jmad`.
