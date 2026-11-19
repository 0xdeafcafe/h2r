# Halo 1 models and animations as Halo 2 tags (`crates/h1models`)

Status: **Built** (2026-11-16) for Halo 1 Xbox `mode` -> Halo 2 `mode` and
`antr` -> `jmad`, including compressed (keyframed) animations
(2026-11-17). It is proven by decoding the converted tags through
`crates/tags` and comparing them with the source. No converted tag has been
loaded by the game yet ("Needs a game run").

This is the model and animation step of the Halo 1 campaign work
([campaign.md](campaign.md) §2). The Halo 1 cache container is in
[map-format.md](map-format.md), the Halo 2 targets in
[map-format.md](../formats/map-format.md) ("Render-model geometry"),
[structure-bsp.md](../formats/structure-bsp.md) and [animation.md](../formats/animation.md), and
the builder the output feeds in [map-builder.md](../formats/map-builder.md).

No map bytes, tag dumps or extracted assets are in the repository. The
tests build invented tags in memory. The `#[ignore]` tests read real maps at
run time and print only counts and error figures.

## Evidence labels

- **V1:** checked on the owner's Halo 1 Xbox multiplayer maps
  (`beavercreek.map`, `carousel.map`) by the ignored tests: every one of
  their 103 `mode` and 39 `antr` tags reads and converts, and the values
  read pass the stated checks.
- **V2:** checked on retail Halo 2 `mode` tags (`lockout.map`) by the
  ignored test `retail_h2_mode_layouts`.
- **V3:** checked against the `jmad` mode/class/type/action trees of every
  map on the owner's retail disc (30 maps, 2,268 graphs) by the ignored test
  `retail_h2_animation_names` in `crates/h1models/src/names.rs` (2026-11-17).
- **V4:** checked on every map of the owner's Halo 1 disc (24 maps, 628
  graphs, 186 compressed animations) by the ignored test
  `h1_disc_compressed_animations_decode` (2026-11-17).
- **V5:** checked on every biped and first-person model of the disc's a10
  and Blood Gulch by the ignored tests in `crates/h1models/src/skin_tests.rs`
  (2026-11-18), which print only counts and error figures.
- **X:** read from the Halo 1 Xbox XBE's own code (disassembled from
  `default.xbe` on the disc; addresses are virtual addresses in that XBE).
- **S:** field order and meaning unverified, checked against the data
  where a V label says so.
- **Design:** an implementation choice.

## Halo 1 Xbox `mode` (render model)

Xbox maps use the `mode` group. There are no `mod2` (gbxmodel) tags on the
two maps. Xbox `mode` differs from PC `mod2` in its vertex form and in having
no local node table. Every block is the usual 12-byte block: count, address,
unused.

### Header (`0xe8`)

| Offset | Field | Evidence |
|---|---|---|
| `00` | u32 flags | S |
| `04` | u32 node list checksum (animations carry the same value) | V1 (equal on the cyborg's model and animations) |
| `08` | f32 x5 LOD cutoffs, super high to super low | V1 (5, 50, 120, 250, 400 on the cyborg) |
| `1c` | i16 x5 LOD node counts | S |
| `30` | f32 x2 base map u, v scale; texture coordinates are multiplied by it | S, V1 (finite) |
| `ac` | markers, stride `0x40` | V1 |
| `b8` | nodes, stride `0x9c` | V1 |
| `c4` | regions, stride `0x4c` | V1 |
| `d0` | geometries, stride `0x30` | V1 |
| `dc` | shaders, stride `0x20`: tag reference (`00`), i16 permutation (`10`) | V1 (references name `soso`/`schi`/... tags) |

**Marker** (`0x40`): `char[32]` name, `34` block of instances (`0x20`):
i8 region, i8 permutation, i8 node, pad, f32x3 translation (`04`), f32x4
rotation (`10`). In a cache file the permutations' own marker blocks are
empty and every marker is here. V1.

**Node** (`0x9c`): `char[32]` name, i16 next sibling (`20`), first child
(`22`), parent (`24`), f32x3 default translation (`28`), f32x4 default
rotation `i, j, k, w` (`34`), f32 distance from parent (`44`). From `48`,
cache-computed values: 8 zero words, then at `68` the inverse of the
node's model-space rest frame (f32 scale 1, three f32x3 vectors that are
the inverse's **columns**, f32x3 position). The reader does not use them;
the skin test checks against them. V1: links in range, unit rotations;
V5: the inverse at `68`.

**Rotations are the conjugate of Halo 2's.** Every Halo 1 rotation (node
defaults, marker instances, animation frames) is the conjugate of the
quaternion Halo 2's rule (`mode::quat_matrix`, the rule the retail
`mode` inverse frames fix, V2) means. V5: composing the disc's bipeds'
nodes by Halo 2's rule puts their vertices about 3x farther from their own
node than the conjugate does (cyborg 0.267 against 0.084 units, grunt
0.275 against 0.090, elite 0.388 against 0.076), and only the conjugate
reproduces Halo 1's cached inverse frames at node `68` (to 7e-7 on every
biped). `h1::h2_rotation` converts; **the reader returns every rotation
already in Halo 2's convention**, so every consumer (the `mode` and
`jmad` writers, `hlmt` nodes, h1objects' physics and marker placement)
composes with Halo 2's rule. Before converter version 54 the rotations
were copied unconverted: the rest pose still matched (the inverse frames
were computed with the same wrong rule), but every animated pose put each
body part in the wrong place, up to 1.2 units off on the cyborg (the
"exploded" bipeds and first-person arms).

**Region** (`0x4c`): `char[32]` name, block of permutations at `40`.
**Permutation** (`0x58`): `char[32]` name, u32 flags (`20`), i16 x5
geometry index per LOD at `40`: super low, low, medium, high, super high.
V1: each is -1 or a geometry. On the cyborg the order runs 4, 3, 2, 1, 0,
so geometry 0 is the most detailed.

**Geometry** (`0x30`): u32 flags, block of parts at `24`.

**Part** (`0x68`):

| Offset | Field | Evidence |
|---|---|---|
| `00` | u32 flags | S |
| `04` | i16 shader index; `06`, `07` i8 previous/next filthy part | V1 (index in range) |
| `08` | i16 x2 centroid primary/secondary node, `0c` f32 x2 their weights | V1 (weights sum to 1) |
| `14` | f32x3 centroid | S |
| `20`, `2c`, `38` | blocks of uncompressed vertices (`0x44`), compressed vertices (`0x20`), strip rows (`6`). **Empty in cache files** | V1 |
| `44` | u32: 1 on every part (index buffer type) | V1, not interpreted |
| `48` | u32: strip length **less two** | V1: with this count every index is below the vertex count and the strip is followed by `ffff` padding |
| `4c` | u32: address of the strip's u16 indices | V1 |
| `50` | u32: address of this part's entry in the index table (index header `+0x1c`) | V1, not needed |
| `54` | u32 vertex type; 5 on every part (compressed) | V1 |
| `58` | u32 vertex count | V1 |
| `60` | u32 offset of the vertices in the model data | V1 (consecutive parts differ by count x 32), not needed |
| `64` | u32: address of this part's entry in the vertex table (index header `+0x14`); the entry's `+4` is the vertices' address | V1 |

The vertex and index tables have one 12-byte entry per part (index header
`+0x10` gives the count). Only the address at `+4` means anything; the
other words are uninitialised (`cacacaca` and similar). The reader still
accepts the in-tag vertex and strip blocks when they are populated, as in
source tags (not seen in cache files).

**Compressed vertex** (32 bytes; V1: unit normals, nodes in range, weights
in [0, 1]):

| Offset | Field |
|---|---|
| `00` | f32x3 position, model space at the rest pose |
| `0c`, `10`, `14` | normal, binormal, tangent: packed 11:11:10 (x: signed 11 bits at bit 0, y: 11 at bit 11, z: 10 at bit 22; divided by 1023, 1023, 511) |
| `18` | i16 x2 texture coordinate, divided by 32767 |
| `1c`, `1d` | i8 node 0, node 1, **stored times three**; negative means none |
| `1e` | i16 weight of node 0, divided by 32767; node 1 gets the rest |

**Strips.** Indices form one strip per part with degenerate joins. Halo 1
strips have the **opposite parity** to Halo 2's: the faces that agree with
their normals are `(a, c, b)` for even triangles and `(a, b, c)` for odd
ones. V1: with Halo 2's rule, 99% of triangles on the cyborg, assault rifle
and warthog face away from their own normals. With this rule, 52 of 5,692
(cyborg), 0 of 940 (assault rifle) and 38 of 7,597 (warthog) do.

## Halo 1 Xbox `antr` (animation graph)

### Header (`0x80`)

| Offset | Field | Evidence |
|---|---|---|
| `00` | objects block, `0x14`: i16 animation, u16 function (0..3, the object's A..D output), u16 controls | V (Blood Gulch: the warthog's one element names `firing` with function 3; the scorpion's names no animation) |
| `0c` | units block (24 on the cyborg), `0x64`: label, aiming screen `20` (`0x18`), unit slots `40`, classes `58` (`0xbc`: label, slots `98`, types `b0`, the class's aiming screen `60`) | V1 (count); V (the warthog's, ghost's and scorpion's `fixed` class screens) |
| `18` | weapons block | S |
| `24` | vehicles block, `0x74`: steering screen `00` (`0x18`), vehicle slots `5c` (i16 per `h1::VEHICLE_SLOTS`), suspension `68` (`0x14`: i16 mass point, i16 animation, f32 full extension and full compression ground depths, negative) | V (Blood Gulch's warthog, ghost, scorpion) |
| `30` | devices block | S |
| `3c` | unit damage block | S |
| `48` | first-person weapons block, `0x1c`: 16 bytes, then at `10` a block of i16 animation indices, one per first-person slot (`h1::FIRST_PERSON_SLOTS`) | V (all 11 first-person graphs on Blood Gulch: one element; each slot's animation carries the slot's label wherever the two agree) |
| `54` | sound references block | S |
| `60` | f32 limp body node radius; `64` u16 flags | S |
| `68` | nodes, stride `0x40` | V1 |
| `74` | animations, stride `0xb4` | V1 |

**First-person slots (converter version 76).** Halo 1 plays a weapon's
first-person animations through the `48` table, by slot, and a slot may
hold an animation labelled for another: the assault rifle's
`reload-empty` slot holds its `reload-full` (it has no `reload-empty`),
the shotgun's and flamethrower's `reload-full` their `reload-empty`, the
assault rifle's `light-on` its `light-off`. Halo 2 asks a first-person
graph for actions by name, so the converted tree also names each filled
slot's action for the animation it holds (`jmad::first_person_slots`),
unless an animation of that label already has it. Before this the
assault rifle had no `reload_empty` action. Slot words 6 (`light-on`) and
19 (`misfire-2`) are S: no Blood Gulch graph fills them with an animation
of that label.

**The assault rifle's firing (converter version 79).** Its `fire-1` slot
(2) is empty, and its 21-frame base animation `first-person firing` fills
no slot: of every first-person graph on Blood Gulch, a10, c10 and d40 it is
the only one with a firing animation and an empty `fire-1` slot (the
flamethrower, ball and flag have no firing animation at all). Halo 1 plays
by slot, so it plays nothing on the assault rifle's shots (I). Halo 2
gives every weapon that fires a `fire_1` (13 of the Chief's 17 retail
first-person graphs on Coagulation; the four without, flag, ball, bomb and
energy sword, do not fire), so the converted graph names `fire_1` for
`first-person firing` when no slot or label took it
(`jmad::first_person_fire`). Before, the converted assault rifle fired
with no recoil animation.

**First-person overlays (converter version 79).** Halo 1's first-person
overlay animations (type 1: every graph's `moving` and `overlays`, the
needler's `ammunition`, the plasma pistol's `overcharged-jitter`) go to
their type's overlays block (`+0xc`, 8 bytes each: string ID, graph index
-1, animation index), not its actions (`jmad::first_person_overlays`). E:
the overlay lookup `0x001dbc80` searches the type's `+0xc` with stride 8
through `0x001dd560` (`0x001dbd01..0x001dbd12`), and the first-person
setup asks it for `overcharged_jitter`, `overlays`, `ammunition` and
`pitch_and_turn` (`0x0016821f..0x00168282`, through `0x001cb520` and
`0x001cb9d0`) and for `moving` (`0x001672d6`). V: every retail first-person
graph on Coagulation keeps `moving`, `overlays` and `pitch_and_turn` (the
needler also `ammunition`, some `sprint`) there and none of them among its
actions. Until version 79 they were actions, where the engine never looks
for an overlay. Halo 1's `overlays` keeps its name; Halo 2's
`pitch_and_turn` has no Halo 1 animation (which of Halo 2's two 9-frame
overlays Halo 1's 9-frame `overlays` matches is not established). Third-
person graphs keep their overlays among the actions, as before (retail
keeps those in `+0xc` too: `aim_still_up`, `look`, ...; not changed here).

The units, weapons and vehicles blocks map fixed engine slots to animation
indices. A third-person graph's tree comes from the animations' own names
(below); a vehicle graph also reads its vehicles block, objects block and
screens (Vehicle graphs, next). **Some graphs have no node block** (the warthog, ghost,
scorpion and the scenery on beavercreek): they animate their model's
skeleton, and the converter takes the nodes from the model.

**Vehicle graphs (converter version 84).** Halo 2 animates a vehicle
by lookups the converted graphs never answered: until version 84 the
Halo 1 warthog's, ghost's and scorpion's animations were actions named
for their labels (`vehicle:steering`, `suspension:left_back`,
`combat:fixed:aim_still`, `firing`), which no engine lookup asks for, so
no converted vehicle turned its wheels, sprang, aimed its gun or spun its
barrels. Version 84 lays a graph with Halo 1's vehicles block out as
retail's are (`jmad::vehicle_plan`):

- **Vehicle slots → overlays of `any:any:any`.** E: the vehicle
  animation update `0x000f0760` asks for the overlays `steering`, `roll`,
  `throttle`, `velocity`, `ground_speed` and `ground_speed_rear` through
  `0x001cb9d0` under the current mode and class; Halo 2's string table
  holds `steering` .. `unoccupied` as IDs `0x78`..`0x7f`, the order of
  Halo 1's eight vehicle slots. V: the retail warthog keeps `steering`,
  `ground_speed` and `ground_speed_rear` in `any:any:any`'s overlays.
  The warthog's slots give `steering` and `ground_speed`; the ghost's
  `steering`.
- **Blend screens (`+0x24`, `0x1c`: label, then Halo 1's 24-byte
  screen).** An animation's `+0x12` names its screen. V: the retail
  warthog's `steering` names screen `steering`; the chaingun turret's
  `combat:fixed:any` `aim_still_up` names `aiming`, whose 24 bytes equal
  the Halo 1 warthog's `fixed` class screen. Every retail animation with a
  screen is codec 8 (float rotations), so the converter writes those so.
  The vehicles block's screen is `steering`'s, each class's screen goes
  on its `aim_still_up` / `aim_move_up`.
- **Aim names.** E: the unit aim update asks for `aim_still_up`
  (`0x001134a3`). Halo 1's `stand fixed aim-still` becomes
  `combat:fixed:any:aim_still_up` (vehicle graphs only; biped aim is not
  converted).
- **Overlays, not actions.** Every type 1 animation of a vehicle graph
  goes to its type's overlays (`+0xc`): V, no retail vehicle tree holds an
  overlay among its actions, and no base animation among its overlays
  (`retail_vehicle_graphs`; the overlays may also hold a replacement,
  type 2: the wraith's `any:any:any` `crouch`). E: `acc_*` are looked up
  through `0x1cc1a0` → `0x1dbc80`, the overlay search. What type 2 does in
  the engine is not established; no Halo 1 vehicle graph has one (86 on
  the disc's campaign and multiplayer maps hold types 0 and 1 only), so
  the converter needs no rule for it.
- **Suspension (`+0x3c`, `0x28`).** E: `0x001dceb0` copies per element the
  animation (`+4`), marker (`+8`), offset (`+0xc`), full extension
  (`+0x10`) and full compression (`+0x14`) depths, region (`+0x18`) and
  the destroyed numbers (`+0x1c`..`+0x24`); `0x000f6670` finds the marker
  (`0x000b8d30`; no marker, no suspension), casts down from it by the
  extension depth and plays the overlay at `1 - (d - comp) / (ext -
  comp)`, clamped. So depths are positive distances below the marker. V:
  the retail warthog's four are 0.308 / 0.132; Halo 1's warthog's
  -0.314 / -0.135, written negated. The marker is the powered mass
  point's (each powered point gets a marker of its name on the converted
  model); an element whose mass point has none is reported
  (`suspension_without_marker`, `no_home`) and dropped.
- **Object overlays (`+0x44`, `0x14`: label, animation, controls,
  function name).** E: the unit's function lookup `0x000c6b90` answers
  `barrel_spin` (`0x000c6d41`) from its current weapon (`+0x212`,
  `+0x218`; the weapon's own at `0x000ff7b0`). V: the retail chaingun's
  `firing` names `barrel_spin`, controls 0. Halo 1's warthog drives
  `firing` from its D output, an accumulating function not converted, so
  the converted overlay follows `barrel_spin`.
- **Weapon pairs (`+0x54`).** (`any`, `any`) and each (type, class) Halo
  1's slot tables list: the warthog's (`cg`, `fixed`). The gun's own
  class comes from the same tables when no animation name votes for it
  (`Converter::weapon_class`): the warthog gun, label `cg`, had no class
  and so no aim until version 84.

Not converted: the ghost's `suspension` overlay (no slot or suspension
element names it; which Halo 2 lookup it matches is not established), its
`stand fixed c ghost gun fire-1`, and the scorpion's `tread pitch ...`
overlays, which stay overlays under their names. Tests: h1models
`a_vehicle_graph_takes_halo_2s_vehicle_layout`,
`reads_a_vehicle_graph_s_tables`; h1objects
`vehicle_animation_disc` (Blood Gulch on Coagulation, and the retail
layout).

**Node** (`0x40`): `char[32]` name, i16 next sibling (`20`), first child
(`22`), parent (`24`), u32 joint flags (`28`), f32x3 base vector (`2c`),
f32 vector range (`38`). V1 (links).

### Animation (`0xb4`)

| Offset | Field | Evidence |
|---|---|---|
| `00` | `char[32]` name | V1 |
| `20` | u16 type: 0 base, 1 overlay, 2 replacement | V1 (cyborg: 130 / 48 / 17) |
| `22` | u16 frame count | V1 |
| `24` | u16 frame size | V1: equals 8 x rotations + 12 x translations + 4 x scales on every animation |
| `26` | u16 frame info type: 0 none, 1 dx dy, 2 dx dy dyaw, 3 dx dy dz dyaw | S; nonzero on 109 cyborg animations |
| `28` | u32 node list checksum | V1 |
| `2c` | u16 node count | V1 (equals the graph's or the model's) |
| `2e` | u16 loop frame | S |
| `38` | i16 next animation (random permutation chain) | S |
| `3a` | u16 flags; bit 0 compressed data | X (`0x000a2d60` tests it), V4 |
| `3c` | i16 sound, `3e` sound frame | S |
| `48` | tag data: frame info (root movement) | V1 (size 0 when the type is 0) |
| `5c` | u64 translation flags, one bit per node | V1 (frame sizes) |
| `6c` | u64 rotation flags | V1 |
| `7c` | u64 scale flags | V1 |
| `88` | u32 offset of the compressed data within the frame data | X, V4 (0 on all 186 compressed animations) |
| `8c` | tag data: default data | V1 |
| `a0` | tag data: frame data, frame count x frame size bytes | V1 |

Tag data is 20 bytes: size, flags, file offset, address (`+0xc`), unused.
**Flag bits beyond the node count hold junk** (the cyborg's B-driver enter
has bits 19 and 20 set with 19 nodes). The reader masks them; with the
mask, every frame size matches.

**Frame data.** For each frame, for each node in order: the rotation if
flagged (i16 x4, `i, j, k, w`, normalised after reading), then the
translation if flagged (f32x3), then the scale if flagged (f32). V1: the
other order (all rotations first) puts float bits where unit quaternions
should be. **Default data** holds, per node in the same order, the channels
the node does *not* animate, and is constant over the animation.

### Compressed animations

186 animations on the disc have flag bit 0 set, all on cinematic graphs
(`cinematics\animations\...`: the pilot, captain, chief, Cortana,
marines and others on a10, a30, a50, b40, c40, d20 and d40). None occur in
the multiplayer maps. They are keyframed rather than stored per frame;
`crates/h1models/src/compressed.rs` decodes them to the same per-frame
transforms as the uncompressed path, and they convert through the same
`jmad` path.

**Which animations.** The engine (`0x000a2d60`) calls an animation
compressed when flag bit 0 is set and the byte at `0x00204b6f` (the
`model_animation_compression` global, 1 in the XBE) is nonzero. X. The
reader goes by the flag alone. Compressed animations have **no default
data** (size 0) and their frame size still counts the animated channels
as if uncompressed (8 x rotations + 12 x translations + 4 x scales). V4.

**Where.** The data starts at frame data + `[+88]`. The frame data's size
is exactly the compressed data's. X (`0x000a2d90`: `[anim+ac] + [anim+88]`),
V4.

**Header** (`0x2c`), eleven u32 offsets from the start of the data, then
the rotation channel table:

| Offset | Points to | Element |
|---|---|---|
| `00` | rotation key times | u16 |
| `04` | rotation statics, **one per node** | 48-bit rotation |
| `08` | rotation key values | 48-bit rotation |
| `0c` | translation channel table | u32 |
| `10` | translation key times | u16 |
| `14` | translation statics, **one per node** | f32x3 |
| `18` | translation key values | f32x3 |
| `1c` | scale channel table | u32 |
| `20` | scale key times | u16 |
| `24` | scale statics, **one per scale channel** | f32 |
| `28` | scale key values | f32 |
| `2c` | (inline) rotation channel table | u32 |

X: the rotation, translation and scale samplers (`0x000a3750`,
`0x000a2ee0`, `0x000a2d90`) read these offsets. V4: on all 186 the regions
are contiguous in this order with no padding, `[00]` is `0x2c` + 4 x
rotation channels, the tables hold 4 bytes per animated channel, the
statics 6 x nodes, 12 x nodes and 4 x scale channels, and the data ends
where the scale keys do.

**Channels.** A channel is a flagged (node, kind) pair, numbered per kind in
node order. Its table entry is a u32: key count in the low 12 bits, index
of its first key (into that kind's times and values) in the high 20 bits.
X (`and 0xfff`, `shr 0xc`). V4: every key index is in range; times
strictly increase, never include frame 0, and every keyed channel's last
key is the last frame (frame count - 1). 17 of 3,031 channels have no
keys.

**Sampling** at frame `f` (X, the three samplers):

- No keys: the static.
- Before the first key: blend from the static (the value at frame 0) to
  the first key over frames 0 .. first.
- On a key: that key. Between keys `k` and `k + 1`: blend by
  `(f - t[k]) / (t[k+1] - t[k])` (binary search, `0x000a29e0`).
- Past the last key, between it and frame `t[last] + 1`, the engine blends
  back toward the static (it samples fractional frames). With integer
  frames and the last key on the last frame this never applies. The
  decoder holds the last key there (Design); V4 that it never happens.
- Blends: translations and scales linearly. Rotations componentwise on the
  unnormalised values, negating the second when their dot product is
  negative, then normalised (`0x000b0e70`, `0x000b4320`; a zero result
  becomes identity).

The rotation and translation statics are indexed by **node**, the scale
statics by **channel**. X: the rotation and translation samplers index
their statics with their fourth argument (the node), the scale sampler
with its third (the channel). V4: the statics' sizes agree (the pilot's
`aye_aye_sir`: 21 rotation statics, 10 rotation channels, no scale
statics).

**Undriven channels** take the node's rotation and translation static and
scale 1. X (`0x000a4d1d`: the default-pose path reads `[+04]` and `[+14]`
by node and stores 1.0 for scale).

**48-bit rotation.** Three little-endian u16s `w0 w1 w2`, read as one
48-bit number `w0 << 32 | w1 << 16 | w2`: `i` is bits 36..47, `j` 24..35,
`k` 12..23, `w` 0..11, each a signed 12-bit value. Each is widened to 16
bits by repeating its top four bits below it (`v << 4 | v >> 8`) and
divided by 32767; the result is normalised. X (`0x000a2b70`; the
constant at `0x001f0ae8` is 1/32767).

**Measured** (V4):

| Check | Result |
|---|---|
| Animations, frames, keys | 186, 31,596, 202,404 |
| Frames per animation | the header's frame count, every one |
| Rotations | unit, every frame of every node |
| Translations, scales | finite |
| a50's chief `B30HoloMapShort` (compressed) against a30's copy on the b30 graph (uncompressed, same 130 frames and channels) | max 0.0026 rad, translation 1.3e-4 |
| a10's chief x30 `stand rifle idle` (compressed) against the cyborg's (uncompressed, same 59 frames) | max 0.0028 rad, translation 0 |
| Converted to `jmad` and decoded by `crates/tags` | max 6.1e-5 rad, translation and scale 0 |

The two uncompressed copies agree to under 0.003 rad, which is the
keyframe reduction's own loss. Undriven channels sit at the rest pose of
the model with the same node list for 1,426 of 2,007 undriven rotation
channels (within 0.01 rad), but not generally on these cinematic graphs (the uncompressed
animations on the same graphs, such as the pilot's `death_pose`, hold
non-rest poses there too), so the rest pose is not a reference here.

## Halo 2 output

### `mode`

Built as a `map::build` tree (header `0x84`) plus one raw block per
section:

| What | How | Evidence for the Halo 2 side |
|---|---|---|
| Name | string ID of the tag path's last component | map-format.md |
| Compression | one record: position bounds over every vertex, texture coordinate bounds (after the base map scale); a flat axis is widened by 1e-3 | map-format.md |
| Regions, permutations | names copied. Halo 1 LODs (super low..super high) fill Halo 2 LOD 0..4, and super high fills LOD 5 too. Region `+4` = -1, `+6` = 0 | map-format.md |
| Sections | one per Halo 1 geometry | |
| Section record | classification (`00`, `18`): 2 if every vertex has one node, 3 if any has two; vertex, triangle and part counts; max nodes per vertex (`14`); compression flags 3 (`1a`); node map size at `24` and `25`, subpart count at `28`, rigid node -1 at `2c` (S); block info with the tag's own datum, runtime offset -8, `1f` = 1, handle `ffffffff` | map-format.md, structure-bsp.md; `24`..`2c` S |
| Raw block | `blkh`, section struct `0x70`, resources each behind `rsrc`, `blkf`. Resources: parts (`00`), subparts (`08`), indices (`20`), 3 descriptors (`38`), node map (`64`), 3 vertex streams | structure-bsp.md |
| Parts | one per Halo 1 part, a **triangle list** (flag `0x8`): the strip unrolled with the parity above and degenerates dropped. Kind 2 when its shader is a `soso`, else 4. Material = Halo 1 shader index. `0e` max nodes, `0f` 1, centroid at `10`, its nodes (section-local) at `1c`, weights at `20` (S) | structure-bsp.md; kinds: only "0 and 5 not drawn" is E |
| Subparts | one per part covering its indices, `+4` = `ffff` | |
| Node map | the nodes this geometry's vertices use, ascending | map-format.md |
| Vertices | type `04` (s16 position, u8 node) or `06` (s16 position, two u8 nodes, u8 weights summing to 255); `19` s16 texcoords; `1b` normal, binormal, tangent repacked 11:11:10 | map-format.md |
| Nodes (`0x60`) | name, parent `04`, first child `06`, next sibling `08`, import index -1 `0a`, default translation `0c`, rotation `18`, **default scale 1 at `28`, inverse forward/left/up at `2c`/`38`/`44`, inverse position at `50`, distance from parent at `5c`** | V2: on every lockout node, `2c`..`5c` invert the model-space default pose computed from `0c`/`18`: the three vectors are the inverse's columns (the rows of the model-space rotation R); `28` is 1. V5: the same on every node of 05a's marine and grunt (`retail_h2_bipeds_pose_sanely`, rest frame times stored inverse within 5e-7). Until converter version 59 the converter wrote R's columns there, which skins a vertex of a rotated node by R twice: the twisted shards a30's marines and the first-person arms were drawn as |
| Marker groups (`58`, `0xc`) | one per Halo 1 marker name, then one per engine marker name the model has a Halo 1 marker for ("Marker names" below): name, block of markers (`0x24`): i8 region (-1 = any), i8 permutation (-1 = any), i8 node, pad, translation `04`, rotation `10`, f32 `20` written 0 | V2: all 922 lockout markers sit in named groups at `58`, with a node in range and a unit rotation; `20` is 0 on 842 and nonzero on 80 (likely a scale; meaning not established) |
| Materials (`60`, `0x20`) | first reference null, second the caller's `shad` for that shader index, or null | map-format.md |
| Header `44` | 0, as on every retail `mode` (Halo 1's node list checksum was written here until converter version 38; one that fell in the map's address range read as a stray pointer and a fresh level refused the tag) | V |
| Section groups (`34`, `0xc`) | one: detail levels `0x3f` (all six) at `00`, compound nodes (`04` block) empty; the LOD bytes at `3c`..`41` stay 0, naming it | V: all 181 of 05a's `mode`s have exactly this. E: `0x00137650` reads `groups[byte[3c + lod]]`'s `+4` (compound node count) with no count check and adds the node count; an empty block faulted at `0x0013767a` (a30's second crash) |

Header flags (`04`): left 0. Retail sets `0x8` on all and `0x4` on 167
of 181 (on Coagulation: `0x4` on 103 of 121 `mode`s, 17 of the 22
first-person ones; `0x8` on all 121); with `0x4` set, `0x00137650`
skips the section group and counts one. Its callers are the object draw
`0x0004b5d0` (`0x0004b854`) and `0x0003e393`, which size a per-draw
record from the count. Without the bit the count is the section group's
compound nodes plus the node count, which every converted model has (one
group, no compound nodes), so the general path is taken and holds (every
converted model draws). No reader of `0x8` was found. Left 0: setting
`0x4` only changes the count, and nothing drawn is known to need it.

Left empty: the node
map block at `50`, errors, PRT info and render leaves, and the section
blocks the decoder does not interpret (`10`, `28`, `30`, `44`, `54`, `5c`).

### The emblem (**Design**, converter version 84)

Halo 2 draws a player's emblem as a part of the body's model. The Chief's
`mc_emblem` (stem `opaque\emblem_overlay_simple`, alpha blended on layer
2) samples `ui\global_bitmaps\emblems\foreground` with its border
clamped (sampler address 4), so only texture coordinates 0 to 1 show the
emblem. Its 12 animated properties read the inputs
`emblem_foreground_index`, `emblem_toggle_foreground` and `emblem_active`.
The function evaluator `0x0022ace7` answers those for the player being
drawn (`[0x510c4c]+0x1b4` via `0x0014cca0`, or `+0x1c0` via
`0x0014ccf0`), and the colours are engine constants (E). The shader
carries everything; the model only gives it a place. The converted
multiplayer maps keep `mc_emblem`, because the base keeps the Chief.

Halo 1's cyborg has no such part. The converter adds one to a player
biped's model (`h1models::emblem`, called from `Converter::model` for a
`bipd` that `health_hud::is_player_biped` names):

- `place` casts a ray in the default pose. It starts 0.03 down the anchor
  node's own x axis, which is the bone's length, from the node, and runs
  along a model-space direction. It takes the farthest hit within reach
  on triangles whose three vertices are bound mostly to the anchor, so
  the outer surface, in the super-high geometry of the first permutation
  hit.
- `part` lays a square there: the hit triangle's normal turned to the
  ray's side, the wanted up laid onto the surface, lifted 0.002 off it,
  texture coordinates 0 to 1 (divided by the base map scale, which
  `convert_model` multiplies back), every vertex bound to the anchor
  alone. It is wound the way the hit triangle is relative to its vertex
  normals, so it faces out as the model's own triangles do.
- `add_emblem` puts the square in each detail level's geometry of that
  permutation. It adds a shader entry of group `embl` (not `soso`, so the
  part is drawn as the transparent kind, after the opaque body), whose
  material the converter names as the base's `mc_emblem`.

Default placement (`LEFT_SHOULDER`, a choice to confirm): the outside of
the left shoulder plate. The anchor is `bip01 l upperarm`, the direction
model +y (Halo 1 models face +x with z up, so +y is the biped's left),
up +z, a square of 0.07 (about 21 cm) and a reach of 0.3. On Blood
Gulch's cyborg the joint is at (-0.008, 0.09, 0.569) and the arm's x axis
is (0.01, 0.19, -0.98). The right shoulder or the chest is a change of
the placement's constants. The model is converted once per `mode` tag,
so every object drawing the cyborg model wears the emblem; on
multiplayer maps that is the player bipeds alone. Proof: the unit tests
in `h1models/src/emblem.rs` (an invented arm) and
`h1objects/tests/model_shaders_disc.rs`
`bloodgulch_cyborg_wears_the_chiefs_emblem_on_its_left_shoulder`. The
size, orientation and look need a live check.

### Marker names

Halo 2's engine finds a marker by string ID through `0x000b8d30`, which
takes the first marker group of that name (`0x001d8f00`, a linear scan;
an empty name is the object's origin, a name not found fails). Most names
come from tags (seat, attachment, effect and unit marker fields), and the
converted tags write Halo 1's names, which the converted model keeps. A
few are written in the engine's code; every string ID pushed to a marker
lookup in the XBE (E), each also a marker of some retail model on
Coagulation (V):

| Name | Use (E) |
|---|---|
| `left_hand` | where a thrown grenade leaves (`0x000e7730`, from `0x000e77f8`) |
| `head`, `body` | `head` at 20 sites, `body` at 6 (units, AI, camera; uses not traced one by one) |
| `primary_trigger` | `0x0023bd20`, `0x0023be75` |
| `ground_point` | `0x0010c238`, `0x0010d061`, `0x0010d27d`, `0x0010d74f` (items) |
| `hover_thrusters` | `0x000f60b8`, `0x000f636b` (vehicles) |
| `drop`, `drop_left`, `drop_right` | `0x000ce704` via `0x000ce6b0` (units) |
| `melee` | `0x000cf240` (units) |
| `fp_body_cam` | `0x000de1dc` |
| `bumper` | `0x0022a185` |
| `flashlight` | `0x0016632b`, first person, via `0x0016898b` |

Halo 1 writes marker names with spaces (`left hand`, `primary trigger`).
A converted model carries each engine name it has a Halo 1 marker for in
Halo 2's form (lower case, `_` for spaces and `-`) as an extra group with
the same instances (`mode::marker_aliases`); Halo 1's names stay, so the
seat, attachment and unit fields the converter writes still resolve. The
disc test `every_engine_marker_resolves_on_the_converted_objects` checks
both on every converted biped, weapon, vehicle and equipment of every
multiplayer map. Before this the cyborg had no `left_hand`, so its
grenades left from its origin (inferred: the lookup's fallback).

### `jmad`

| What | How |
|---|---|
| Nodes (`0x20`) | name, next sibling, first child, parent, joint flags, base vector, vector range from the graph (or the model, for graphs without nodes) |
| Animations (`0x6c`) | name (below), node list checksum, type, node count, frame count, flag `0x10` (in a resource), resource 0, data offset, static pose -1, both link indices -1, frame info type 0 |
| Data | one raw resource for the whole graph (`0x200`-aligned and padded). Base animations: static stream, codec 1, holding every channel Halo 1 keeps in default data. Every animation: animated stream, codec 3, holding the channels Halo 1 animates (i16 rotations, which is Halo 1's own form; float translations and scales). Then both flag sets (each 3 arrays), set sizes `3 x ((n + 31) >> 3 & !3)` |
| Modes tree | mode -> class -> type -> actions from the mapped names; action graph index -1. **Every level in ascending string-ID order** (converter version 64), laid out so when the map is built (`map::build::Block::sorted_by_string_id`): the engine finds a mode, class, type and action by binary search over signed string IDs (E `0x001dd560`, from the graph lookup `0x001db170`; every retail graph on Coagulation is so ordered, V). Through version 63 the levels were in name order, which hid names from the search: on Blood Gulch (version 60) the cyborg's graph found `combat:<class>:<type>:idle` for none of the Halo 1 weapons, so the player could neither start with nor pick up any of them ([objects.md](objects.md), "Holding is finding"). The tree also carries Halo 1's **borrowed slots**: Halo 1 finds a unit's animations through slot tables (`antr +0x0c` units, `0x64` each, label at +0, weapon classes at +0x58; a class is `0xbc`, label at +0, slot indices (`i16`) at +0x98, types at +0xb0; a type is `0x3c`, label at +0, slots at +0x30), and a slot may name another class's animation (every cyborg class's throw slot is `stand rifle throw-grenade`). Each such slot becomes an action of the borrowing mode, class and type naming the same animation, unless that path already has the action |
| Resource (`0x14`) | the tag's datum, size, local raw pointer, `ffffffff`, 0 |

Left empty: sound and effect references and events, blend screens,
inheritance, the `+0x54` string-ID pairs (count >= 1 on every retail graph;
meaning unknown; written only on first-person graphs, as (`any`, `any`)), static poses, and the header word at `+8`.

## Animation names

A Halo 1 name is split on spaces: `<unit> [<weapon class> [<weapon type>]]
<action>`, with a `%n` suffix for a random variant. The Halo 2 name is
`mode:class:type:action`, leaving out `any` segments. `-` becomes `_` and
names are lower-cased. A name that an earlier animation already produced
gets `:var1`, `:var2`, ... (Halo 1 repeats names for permutations: three on
the cyborg).

| Halo 1 | Halo 2 | Status |
|---|---|---|
| unit `stand` | mode `combat` | V3 (`combat:rifle:any:idle` and the movement actions on the retail masterchief/elite graphs) |
| unit `alert` | mode `combat` (no alert mode exists in retail) | V3; collides with `stand`, variants result |
| unit `crouch` | `crouch` | V3 (`crouch:rifle:any:idle`) |
| `W-driver`, `W-gunner`, `W-passenger` | `warthog_d`, `warthog_g`, `warthog_p` | V3 (`warthog_g:fixed:any:idle` proves the gunner seat and the `fixed` class together) |
| `G-driver`, `B-driver`, `scorpion-driver` | `ghost_d`, `banshee_d`, `scorpion_d` | V3 (all three are modes of the retail biped graphs) |
| `CT-driver` | `c_turret_ap_d` | V3 for the name (`c_turret_ap` is the retail covenant turret, its jmad on every multiplayer map); the choice between it and the rarer `c_turret_mp_d` is S |
| `GT-gunner` | `guntower_d` | V3 for the name (03a elite graph); the choice is S (retail also has `h_turret_ap_d`/`h_turret_mp_d`) |
| `P-riderLF`, `P-riderRF` | `pelican_p_l01`, `pelican_p_r01` | V3 for the names; 01 = front row is S (retail numbers each side's seats 01..05) |
| `scorpionLF` | `scorpion_p_lf` | V3 (`scorpion_p_lf:any:any:enter`; retail also has `_p_lb`, `_p_rf`, `_p_rb`, which no Halo 1 name feeds) |
| weapon `unarmed`, `pistol`, `rifle`, `missile`, `fixed` | the same class | V3 (classes of the retail biped graphs; `fixed` under `warthog_g`) |
| third word before the action (`stand rifle sg reload-1`) | weapon type | V3 for the labels Halo 1 and Halo 2 share (`pr`, `sg`, `sr`, `hp`, `ne`, `pp`, `rl`, `f`); `ar` has no Halo 2 type (the assault rifle became several weapons) |
| action `reload` / `reload-1` | `reload_full` | V3 that `reload_full` is the retail action and `reload_1` exists nowhere; retail reloads are first-person-only, so in a third-person graph no engine lookup uses it |
| action `melee` | `melee` (unchanged) | V3 (`combat:rifle:any:melee`, `combat:pistol:any:melee` on the retail elite; `melee_strike_1..3` are first-person actions only) |
| action `throw-grenade` | `throw_grenade` | V3 (`combat:sword:any:throw_grenade` on the masterchief) |
| vehicle actions `stand opening` / `stand closing` | `combat:opening` / `combat:closing` | V3 — the only actions any retail vehicle graph carries (banshee, scorpion, pelican, `c_turret_ap`, the phantom and wraith turrets) |
| `first-person <action>` (a weapon's first-person graph) | mode, class and type `any`, animation `first_person:<action>`; `melee` becomes `melee_strike_1`; the graph's `+0x54` the one pair (`any`, `any`) | V3 (every retail first-person graph on Coagulation, 34, has the one mode `any` and that pair; h1objects `first_person_graphs_take_retail_modes`, which lists the converted actions retail has and has not: 19 shared on Blood Gulch, and `ammunition`, `enter`, `firing`, `light_off`, `moving`, `overcharged_jitter`, `overheating_again`, `overlays`, `reload_full2`, `stealth_melee` it lacks; since converter version 79 `ammunition`, `moving`, `overcharged_jitter` and `overlays` are overlays, not actions, and the assault rifle has `fire_1`: First-person overlays and The assault rifle's firing above) |
| `%n` | `:var<n>` on the action | Design; retail variant suffixes (`:var0`, ...) sit on animation names, never on tree actions |

**No Halo 2 equivalent, or not mapped:**

- Unit damage (`h-kill`, `s-kill`, `s-ping`, with direction and region)
  stays one action from all its words (`h_kill:back_gut`): V3 that no
  retail tree holds `h_kill`, `s_kill` or `s_ping` (nor any mode for them).
  Retail death vocabulary is `dead`, `mortal_wound`, `landing_dead`,
  `airborne_dead`, `thrash_front/back`, `resurrect_front/back`, driven by
  the engine's damage tables rather than name lookup.
- Vehicle overlays (`suspension left back`, `vehicle steering`,
  `vehicle ground-speed`, `firing`, `acc-front-back` and kin): V3 that
  retail vehicle graphs carry only `combat:opening`/`combat:closing` as
  actions, no `suspension` or `vehicle` mode. Halo 2 finds the rest as
  overlays, in the suspension and object overlay blocks and by blend
  screen; since converter version 84 the converter puts them there
  (Vehicle graphs, above).
- Seats that keep their Halo 1 label: `P-riderLM`, `P-riderLB`,
  `P-riderRM`, `P-riderRB` (their retail row number among `pelican_p_l01..05`
  is not recoverable from the trees) and the dropship seats
  `CD-passengerL01`/`CD-passenger01` (retail phantom seats are columns
  `phantom_p_a01..c03`, with no left/right correspondence).
- The engine-slot structure of Halo 1 graphs (units/weapons blocks) is
  not converted beyond borrowed slots, aiming screens and weapon pairs;
  Halo 2's tree comes from names. The vehicles and objects blocks are
  (Vehicle graphs, above).
- Root movement (frame info) is dropped: 109 of the cyborg's 195
  animations have it. Halo 2 has a movement section after the flag sets
  that `crates/tags` does not decode yet.
- Overlay and replacement animations keep only their animated channels.
  Their Halo 1 default data is mostly the rest pose (1,567 non-identity
  channels on the cyborg, 202 on the warthog), and is dropped.
- Sounds, the loop frame, key frames, foot frames and permutation chains
  (`next animation`) are not carried over.

## API

```rust
use h1models::{graph_from_map, model_from_map, mode::ModelOptions};

let map = h1map::H1Map::from_reader(file)?;
let tag = map.find(h1map::Group::new(b"mode"), "characters\\cyborg\\cyborg").unwrap();
let opts = ModelOptions { shaders: vec![/* H2 shad datum per H1 shader index */] };
let (h1_model, converted) = model_from_map(&map, tag, &opts)?;
let mode_id = converted.install(&mut tag_set, "characters\\cyborg\\cyborg")?;

let antr = map.find(h1map::Group::new(b"antr"), "vehicles\\warthog\\warthog").unwrap();
let model = map.find(h1map::Group::new(b"mode"), "vehicles\\warthog\\warthog");
let (h1_graph, report, converted) = graph_from_map(&map, antr, model)?;
let jmad_id = converted.install(&mut tag_set, "vehicles\\warthog\\warthog")?;
```

A `Converted` is `{ group, root: map::build::Block, raw: Vec<(bytes,
align)> }`. Its raw fixups use `RawRef::Local(RawId(i))` with `i` indexing
its own `raw`, and `SELF_ID` stands for its own datum. `install` places the
raw resources in the set, rewrites both, declares the group, and adds the
tag. The lower-level pieces are `h1::Model::read`, `h1::Graph::read` (any
`h1::Memory`), `mode::convert_model` and `jmad::convert_graph`.
`jmad::GraphReport` lists each name mapping, the variants, and what was
dropped. `jmad::animation_names` gives each animation's Halo 2 name (and
whether it is a variant) without converting, so a script that names a
Halo 1 animation can name the converted one (h1campaign's cutscenes).

## Tests and measured error

`cargo test -p h1models` runs the synthetic tests. The real-data tests
build a v8 map in memory with `map::build`, open it with `map::Map`,
decode through `tags::model::RenderModelGeometry` and
`tags::anim::AnimationGraph`, and compare with the Halo 1 source:

```sh
H1_MAPS=<dir with beavercreek.map, carousel.map> cargo test -p h1models --release -- --ignored --nocapture
H2_DISC=disc/halo2.iso [H2_MAP=lockout] cargo test -p h1models --release -- --ignored retail_h2 --nocapture
H1_ISO=disc/halo1.iso cargo test -p h1models --release -- --ignored h1_disc --nocapture
```

Results on 2026-11-16 (beavercreek), every vertex and triangle of every
geometry, all LODs:

| Model | Nodes | Markers | Geometries | Vertices | Triangles | Max position error | Texcoord | Normal | Weight |
|---|---|---|---|---|---|---|---|---|---|
| cyborg | 19 | 8 | 5 | 6,025 | 5,692 | 5.4e-6 | 7.6e-6 | 0 | 2.0e-3 (u8) |
| assault rifle | 3 | 9 | 3 | 1,075 | 940 | 2.2e-6 | 7.6e-6 | 0 | 0 |
| warthog | 18 | 26 | 20 | 10,396 | 7,597 | 1.5e-5 | 1.5e-5 | 0 | 2.0e-3 (u8) |

Node names, hierarchy, default transforms, region and permutation names,
LOD sections, marker names and instances (exact), and per-triangle indices
all match.

**Retail control** (V5, `retail_h2_bipeds_pose_sanely`, 05a). The Halo 2
side of the round trip below is first held to retail data, so the round
trip cannot share an assumption with the converter: a retail biped's rest
frames times the inverse frames its `mode` stores, read as columns, are the
identity on every node (within 5e-7; read as rows, as the converter wrote
them until version 59, off by up to 2.0), and its own retail animations,
decoded by `crates/tags` and composed the same way, stretch the mesh's
triangle edges little (median 1.0, 99th percentile 1.5 to 2.4) and less
than the same frames with conjugated rotations (2.6 to 3.5), so the decoded
quaternions are used as they are.

**Skinned round trip** (V5, `h1_disc_bipeds_skin`, converter version
59). Each model and its graph are converted, built into a map and decoded
through `crates/tags`; every vertex is skinned the Halo 2 way (decoded
position, node indices through the section node map, weights, the
decoded jmad frame composed by Halo 2's rule, times the inverse frame the
`mode` stores at node `+0x2c`, read as the control reads retail's) and compared with the Halo 1 vertex skinned
the Halo 1 way (its frame composed by Halo 1's rule, times Halo 1's own
cached inverse). Bind pose, and the middle frame of about seven base
animations per graph:

| Model | Vertices | Our inverse frames vs Halo 1's | Bind pose | Posed, version 53 | Posed, version 54 |
|---|---|---|---|---|---|
| cyborg | 6,025 | 4.2e-7 (was 1.2) | 5.9e-6 | 0.64 to 1.16 | 1.4e-4 to 2.8e-4 |
| marine (armored) | 13,666 | 4.8e-7 (was 1.7) | 7.0e-6 | 0.53 to 0.85 | 7.3e-5 to 1.8e-4 |
| crewman | 1,578 | 4.8e-7 | 6.9e-6 | 0.53 to 0.85 | 7.4e-5 to 1.8e-4 |
| elite | 10,818 | 7.2e-7 (was 1.9) | 7.1e-6 | 0.54 to 1.00 | 1.4e-4 to 3.5e-4 |
| grunt | 8,065 | 3.5e-7 (was 1.9) | 6.8e-6 | 0.41 to 1.15 | 9.0e-5 to 2.8e-4 |
| captain, pilot, Cortana | 1,036 to 1,747 | under 7e-7 | under 7e-6 | 0.15 to 0.78 | under 2e-4 |
| cyborg first-person arms | 1,440 | 8.9e-8 | 2.9e-6 | (no graph of its own) | |

Errors are the largest vertex distance in world units (a biped is about
0.7 tall); the vertices travel up to 0.93 in those frames. What is left
is the i16 quantisation of positions and rotations. Every bind-pose vertex
stays within the model's Halo 1 bounding radius.

Animations, every frame of every animation, each channel the conversion
carries:

| Graph | Animations | Frames | Max rotation error | Translation | Scale |
|---|---|---|---|---|---|
| cyborg | 195 | 5,779 | 6.0e-5 rad | 0 | 0 |
| warthog | 11 | 43 | 2.1e-5 rad | 0 | 0 |

The rotation error is the i16 re-quantisation of normalised quaternions.

### First-person graphs

**The engine binds by name** (E). When a weapon's first-person state is
set up (`0x00167e86`), `0x001dd4c0` builds one table per model, the
arms (the player representation's `+0x0`, `0x00168083`) and the weapon's
first-person model (`weap +0x2a8` element `+0x0`, `0x001680eb`): for each
`mode` node (`+0x48`, stride `0x60`), the index of the `jmad` node
(`+0x0c`, stride `0x20`) whose string-ID name is equal, else -1; it
returns whether every node was found, which sets the state's flag `4`
(arms) or `2` (weapon). `0x0016774f` then seeds each bound graph node's
pose from the model node's default rotation and translation. Node order
does not matter: retail graphs list the arms' 37 nodes first, then the
weapon's and `camera_control` (V2, 14 graphs on 05a), but that is the
tool's order, not a rule the engine needs. Halo 1's graphs interleave
them (the assault rifle's `frame gun` is node 7) and are kept in their
order.

What broke the first person was the arms: the converted game used Halo
2's, whose nodes (`l_upperarm`, ...) match no node of a Halo 1 graph
(`frame l upperarm`, ...). From converter version 54 the Halo 1 sandbox
and campaign use Halo 1's arms (docs/halo1/multiplayer.md,
"First-person arms"), whose 37 names all occur in every Halo 1
first-person graph.

V5 (`h1_disc_first_person_skin`): Halo 1's arms and five weapons'
first-person models on Blood Gulch, with each weapon's converted graph,
bound by name, posed at the middle frame of base animations and skinned
both ways as for the bipeds. Every node of both models finds its graph
node. Largest vertex error per weapon over 5 to 8 animations: assault
rifle 1.2e-4, pistol 1.5e-4, plasma rifle 5.4e-5, needler 6.0e-5,
rocket launcher 6.3e-5 (the i16 quantisation). Until converter version
59 the arms and the weapons' first-person models carried the transposed
inverse frames (Nodes, above), so the engine drew them as a large flat
shape over the view.

**The first-person body** (the representation's `+0x8`) stays Halo 2's,
and the engine skips it for a Halo 1 player (E). The first-person draw
list (`0x00165f16`) reads the representation's arms (`+0x4`) and body
(`+0xc`) datums (`0x00165fdb`, `0x00165fde`), adds the arms when the
state's flag `4` is set and each weapon when its flag `2` is, and adds the
body last, posed with the third-person unit's own node matrices
(`0x000b8c00`), only when the body's `mode` and the unit's have the same
node count (`+0x48`, `0x001661d8`) and the same `+0x44` (`0x001661e0`).
Halo 2's body has the Chief's 33 nodes and his `+0x44` (V5, 05a); a converted Halo 1 biped (the
cyborg, 19 nodes) does not, so the body is never drawn. Halo 1 has no
first-person body.

## Needs a game run

- Whether the engine draws a model whose PRT and other
  unread header blocks are empty, with header flags 0, and whether it needs the section blocks
  the decoder skips.
- Part kinds 2 and 4 for opaque and transparent, the part fields from
  `0e`, and the section record's `24`..`2c`.
- Marker group order: retail may expect it sorted by name.
- Marker rotations take the same conjugation as nodes and animations
  (V5 proves it only for nodes and frames).
- Overlay semantics: Halo 1 overlays are copied verbatim (frame 0 is not
  subtracted). The engine may expect a different reference.
- Every name in the table above.
