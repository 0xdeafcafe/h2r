# Halo 1 models: animation graphs and Halo 2 output

The Halo 1 `antr` graph and what it becomes in Halo 2, split from [models.md](models.md).

| Section |
|---|
| [Halo 1 Xbox `antr` (animation graph)](#halo-1-xbox-antr-animation-graph) |
| [Halo 2 output](#halo-2-output) |

Part of [models.md](models.md).

## Halo 1 Xbox `antr` (animation graph)

### Header (`0x80`)

| Offset | Field | Evidence |
|---|---|---|
| `00` | objects block, `0x14`: i16 animation, u16 function (0..3, the object's A..D output), u16 controls | V (Blood Gulch: the warthog's one element names `firing` with function 3; the scorpion's names no animation) |
| `0c` | units block (24 on the cyborg), `0x64`: label, aiming screen `20` (`0x18`), unit slots `40`, classes `58` (`0xbc`: label, slots `98`, types `b0`, the class's aiming screen `60`) | V1 (count); V (the warthog's, ghost's and scorpion's `fixed` class screens) |
| `18` | weapons block | S |
| `24` | vehicles block, `0x74`: steering screen `00` (`0x18`), vehicle slots `5c` (i16 per `h1::VEHICLE_SLOTS`), suspension `68` (`0x14`: i16 mass point, i16 animation, f32 full extension and full compression ground depths, negative) | V (Blood Gulch's warthog, ghost, scorpion) |
| `30` | devices block | S |
| `3c` | unit damage block of signed 16-bit animation indices, ordered by kind, direction and region | Blood Gulch cyborg, 4 × 4 × 11 |
| `48` | first-person weapons block, `0x1c`: 16 bytes, then at `10` a block of i16 animation indices, one per first-person slot (`h1::FIRST_PERSON_SLOTS`) | V (all 11 first-person graphs on Blood Gulch: one element; each slot's animation carries the slot's label wherever the two agree) |
| `54` | sound references block | S |
| `60` | f32 limp body node radius; `64` u16 flags | S |
| `68` | nodes, stride `0x40` | V1 |
| `74` | animations, stride `0xb4` | V1 |

**First-person slots.** Halo 1 plays a weapon's
first-person animations through the `48` table, by slot, and a slot may
hold an animation labelled for another: the assault rifle's
`reload-empty` slot holds its `reload-full` (it has no `reload-empty`),
the shotgun's and flamethrower's `reload-full` their `reload-empty`, the
assault rifle's `light-on` its `light-off`. Halo 2 asks a first-person
graph for actions by name, so the converted tree also names each filled
slot's action for the animation it holds (`jmad::first_person_slots`),
overriding the clip label. An empty source slot leaves its key empty.
The Halo 1 XBE's slot label table confirms the names of light-on (6)
and misfire-2 (19), even though Blood Gulch has no clips with those labels.

**First-person fire and per-shell reloads.** Blood Gulch's assault
rifle has a 21-frame clip named `first-person firing` and an empty firing
slot (2). The Halo 1 XBE selects slot 2 at `0xd70d8`. At
`0xd757b..0xd75c5` it reads the weapon graph's slot and returns on −1,
before the clip and frame writes at `0xd7609..0xd7611`. The converted
AR therefore leaves the current first-person animation playing when it
fires. Its third-person firing overlay comes from its populated weapon
slot 4. For per-shell reloads Halo 2 asks for `reload_enter`,
`reload_continue_empty`, `reload_continue_full` and `reload_exit`
(`0x166620..0x166637`). The shotgun supplies slots 23, 7, 8 and 25. Both
continuation slots point to its shell insertion. Halo 1 has separate dry
and loaded exits. Halo 2's single post-insertion exit uses the loaded
clip. Both source exits retain their own names. Halo 1 changes a ready
request on an overheated weapon to state 2 at `0xd7448..0xd7463`, selecting
slot 22 at `0xd712c`. Both plasma weapons thus bind `ready_overheated`
(`0x16664a`) to their `overheating-again` clip. Halo 1's melee slot 13
supplies both the generic and strike requests (`0x166638`, `0x16667a`).

**First-person overlays.** Halo 1's first-person
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
actions: the engine never looks for an overlay among the actions. Halo 1 requires at least nine frames for slot 4
(`0xd8284..0xd82aa`). Halo 2 makes the same check on `overlays`
(`0x1679a4..0x1679ae`) and samples its additional `pitch_and_turn` slot
independently (`0x16791b..0x16796b`).

- Standing, crouching and seated units bind by slot position in the unit
  (`+0x40`), class (`+0x98`) and weapon type (`+0x30`). For example,
  the missile class borrows one forward stun for several directions. The
  cyborg's alert rifle supplies the forward movement and moving aim
  override under `patrol`, as on Coagulation's Chief.
- Halo 2 searches whole-body actions at type `+0x04` (`0x1db170`) and
  overlays at `+0x0c` (`0x1dbd01..0x1dbd12`). The latter holds aim,
  fire and the reload and melee replacements. The grenade throw is a
  base action. The unit dispatch tries the overlay and then the action
  (`0x113f6f..0x113fff`). Both games' grenade release is frame 8.
- Look and aim carry the source yaw and pitch grids and use aligned codec
  8 frames. Classes carry their source hand/grip marker pairs. The node
  flags and graph masks at `+0x5c/+0x7c` select the left arm and remaining
  nodes in the cyborg's own node order. The retail Chief uses these masks
  during channel sampling (`0x1c7013..0x1c703a`).
- The source damage table supplies all 176 positions under the four keys
  `s_ping`, `h_ping`, `s_kill` and `h_kill`. Halo 2 looks up the key
  at type `+0x14`, then indexes direction and body region (`0x1dc808`).
  Cyborg permutations retain their head, next index and cumulative weight
  (`0x1dd6b9`). Cyborg and first-person clips carry their primary and
  secondary key frames in the frame-event block at `+0x4c`. Foot contact
  bytes `+0x40/+0x41` become event types 2 and 3. The Chief and cyborg
  turn left at frames 7/14 and right at frames 14/7.
- **Loop starts.** H1 `0xa318e..0xa31aa` bounds a positive loop
  frame by the last frame of the clip. H2 consumes the loop start at
  record `+0x2e` directly (`0x1c6810..0x1c6835`). Standing rifle
  airborne repeats from frame 40. The plasma pistol's nine-frame
  overheated clip specifies frame 34, so its effective loop starts at 8.
- The Halo 2 sandbox selects Coagulation's Chief and Elite. The disc
  cases in `sandbox_disc/player_animations.rs` check both representation
  paths, and compare Halo 1 slot indices and Halo 2 retail bindings in
  their respective sandboxes. `bloodgulch_player_frame_data` compares
  pose channels and root displacement with the Halo 1 disc.

**Some graphs have no node block** (the warthog, ghost,
scorpion and the scenery on beavercreek): they animate their model's
skeleton, and the converter takes the nodes from the model.

**Vehicle graphs.** Halo 2 animates a vehicle by lookups: a graph that
left the Halo 1 warthog's, ghost's and scorpion's animations as actions
named for their labels (`vehicle:steering`, `suspension:left_back`,
`combat:fixed:aim_still`, `firing`) would never be asked for them, and no
converted vehicle would turn its wheels, spring, aim its gun or spin its
barrels. A graph with Halo 1's vehicles block is laid out as retail's are
(`jmad::vehicle_plan`):

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
- **Codec 8 streams are 16-byte aligned.** E: codec 8's interpolated
  rotation function (`0x0028c530`, record 8 of the codec table at
  `0x47fb14`, slot `+0x1c`) loads each frame's four floats at
  `+0x20 + i * [+0x14] + frame * 16` of the stream with `movaps`
  (`0x0028c589`), which faults on an address that is not a multiple of
  16. V: every codec 8 stream on Coagulation is so aligned in its
  resource (`crates/tools/h1objects/tests/animation_codec_disc.rs`). So every
  blob starts 16-aligned and a static stream before a codec 8 stream is
  padded to 16 (its size field covers the padding); with 4-aligned blobs
  the game stops with `#GP at 0x28c589` when a vehicle's steering or aim
  overlay first plays.
  The vehicles block's screen is `steering`'s, each class's screen goes
  on its `aim_still_up` / `aim_move_up`.
- **Aim names.** E: the unit aim update asks for `aim_still_up`
  (`0x001134a3`). Halo 1's `stand fixed aim-still` becomes
  `combat:fixed:any:aim_still_up` (the cyborg uses its class screen as described above).
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
  model); an element whose mass point has none (unpowered, or out of
  range) is reported (`suspension_without_marker`) and the object
  converter refuses the graph with an error naming the map and the
  graph, not a dropped element. No retail vehicle has one (V: the
  warthog's four and the scorpion's eight are all powered).
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
  and so would have no aim.

Not converted: the ghost's `suspension` overlay (no slot or suspension
element names it; which Halo 2 lookup it matches is not established), its
`stand fixed c ghost gun fire-1`, and the scorpion's `tread pitch ...`
overlays, which stay overlays under their names. Tests:
`crates/tools/h1-assets/src/models/tests.rs`
`a_vehicle_graph_takes_halo_2s_vehicle_layout`,
`reads_a_vehicle_graph_s_tables`; `crates/tools/h1objects/tests`
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
| `2e` | loop start, signed i16 | H1 XBE `0xa3193..0xa31aa` |
| `34`, `36` | primary and secondary key frames | H1 XBE `0xa31f4..0xa3200` |
| `38`, `42`, `44` | i16 next and head indices, f32 cumulative weight | cyborg permutation chains |
| `40`, `41` | foot contact frames | cyborg and retail Chief turn cycles |
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
`crates/tools/h1-assets/src/models/compressed.rs` decodes them to the same per-frame
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
| Converted to `jmad` and decoded by `crates/formats/tags` | max 6.1e-5 rad, translation and scale 0 |

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
| Parts | one per Halo 1 part, a **triangle list** (flag `0x8`): the strip unrolled with the parity above and degenerates dropped. A sky's model instead keeps Halo 1's strip behind one repeat of its first index, no flag and an empty placeholder subpart, since the sky draw draws every part as a strip ([sky.md](sky.md), "Strips"; `ModelOptions::strips`). Kind 2 when its shader is a `soso`, else 4. Material = Halo 1 shader index. `0e` max nodes, `0f` 1, centroid at `10`, its nodes (section-local) at `1c`, weights at `20` (S) | structure-bsp.md; kinds: only "0 and 5 not drawn" is E |
| Subparts | one per part covering its indices, `+4` = `ffff` | |
| Node map | the nodes this geometry's vertices use, ascending | map-format.md |
| Vertices | type `04` (s16 position, u8 node) or `06` (s16 position, two u8 nodes, u8 weights summing to 255); `19` s16 texcoords; `1b` normal, binormal, tangent repacked 11:11:10 | map-format.md |
| Nodes (`0x60`) | name, parent `04`, first child `06`, next sibling `08`, import index -1 `0a`, default translation `0c`, rotation `18`, **default scale 1 at `28`, inverse forward/left/up at `2c`/`38`/`44`, inverse position at `50`, distance from parent at `5c`** | V2: on every lockout node, `2c`..`5c` invert the model-space default pose computed from `0c`/`18`: the three vectors are the inverse's columns (the rows of the model-space rotation R); `28` is 1. V5: the same on every node of 05a's marine and grunt (`retail_h2_bipeds_pose_sanely`, rest frame times stored inverse within 5e-7). R's columns there would skin a vertex of a rotated node by R twice (twisted shards on the biped models and the first-person arms) |
| Marker groups (`58`, `0xc`) | one per Halo 1 marker name, then one per engine marker name the model has a Halo 1 marker for ("Marker names" below): name, block of markers (`0x24`): i8 region (-1 = any), i8 permutation (-1 = any), i8 node, pad, translation `04`, rotation `10`, f32 `20` written 0 | V2: all 922 lockout markers sit in named groups at `58`, with a node in range and a unit rotation; `20` is 0 on 842 and nonzero on 80 (likely a scale; meaning not established) |
| Materials (`60`, `0x20`) | first reference null, second the caller's `shad` for that shader index, or null | map-format.md |
| Header `44` | 0, as on every retail `mode` (a Halo 1 node list checksum here that fell in the map's address range would read as a stray pointer, and a fresh level refused the tag) | V |
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

### The emblem (**Built**)

Halo 2 draws a player's emblem as a part of the body's model. The Chief's
`mc_emblem` (stem `opaque\emblem_overlay_simple`, alpha blended on layer
2) samples `ui\global_bitmaps\emblems\foreground` with its border
clamped (sampler address 4), so only texture coordinates 0 to 1 show the
emblem. Its 12 animated properties read the inputs
`emblem_foreground_index`, `emblem_toggle_foreground` and `emblem_active`.
The function evaluator `0x0022ace7` answers those for the player being
drawn (`[0x510c4c]+0x1b4` via `0x0014cca0`, or `+0x1c0` via
`0x0014ccf0`), and the colours are engine constants (E).

Halo 1's model draw at `0xa2449..0xa246d` selects the authored shader
for each geometry part. The source cyborg models have no player-emblem
part. The converter adds one to a player
biped's model (`h1_assets::models::emblem`, called from `Converter::model` for a
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
  material is a new `h1\player_emblem` instance built from the `shad`
  definition. It binds the native Halo 2 emblem program and foreground
  atlas, which have no Halo 1 equivalent. The bitmap animation maps the
  foreground input to the atlas's 64 frames; the two pixel-constant
  animations bind the foreground toggle and active flag. The build
  requires those draw dependencies by their full paths.

Default placement (`LEFT_SHOULDER`, a choice to confirm): the outside of
the left shoulder plate. The anchor is `bip01 l upperarm`, the direction
model +y (Halo 1 models face +x with z up, so +y is the biped's left),
up +z, a square of 0.07 (about 21 cm) and a reach of 0.3. On Blood
Gulch's cyborg the joint is at (-0.008, 0.09, 0.569) and the arm's x axis
is (0.01, 0.19, -0.98). The right shoulder or the chest is a change of
the placement's constants. The model is converted once per `mode` tag,
so every object drawing the cyborg model wears the emblem; on
multiplayer maps that is the player bipeds alone. Proof: the unit tests
in `crates/tools/h1-assets/src/models/emblem.rs` (an invented arm) and
`crates/tools/h1objects/tests/model_shaders_disc.rs`
`bloodgulch_cyborg_wears_the_chiefs_emblem_on_its_left_shoulder` hides the
base's `mc_emblem` instance and checks that the cyborg binds the rebuilt
material. `rebuilt_emblem_binds_the_player_inputs_at_the_native_pass`
checks the animation bindings and the foreground frame function
(`crates/tools/h1convert/src/shad.rs`).

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
| Animations (`0x6c`) | name (below), node list checksum, type, node count, frame count, flag `0x10` (in a resource), resource 0, data offset, static pose -1, source frame info type and biped permutation links and weights |
| Data | one raw resource for the whole graph (`0x200`-aligned and padded), each animation's blob at a 16-byte aligned offset. Base animations: static stream, codec 1, holding every channel Halo 1 keeps in default data (padded to 16 bytes before a codec 8 stream). Every animation: animated stream, codec 3, holding the channels Halo 1 animates (i16 rotations, which is Halo 1's own form; float translations and scales), or codec 8 (float rotations) on steering, look and aim screens. Then both flag sets (each 3 arrays), set sizes `3 x ((n + 31) >> 3 & !3)`. `tags::anim::AnimationGraph::engine_problems` checks each animation against the engine's reader |
| Modes tree | mode -> class -> type -> actions from the mapped names; action graph index -1. **Every level in ascending string-ID order**, laid out so when the map is built (`map::build::Block::sorted_by_string_id`): the engine finds a mode, class, type and action by binary search over signed string IDs (E `0x001dd560`, from the graph lookup `0x001db170`; every retail graph on Coagulation is so ordered, V). Levels in name order would hide names from the search: the cyborg's graph would find `combat:<class>:<type>:idle` for none of the Halo 1 weapons, so the player could neither start with nor pick up any of them ([objects.md](objects.md), "Holding is finding"). The cyborg's bindings come directly from the source slot tables (Multiplayer cyborg, above). Campaign bipeds bind their resting poses, locomotion and talk overlays by slot. |
| Resource (`0x14`) | the tag's datum, size, local raw pointer, `ffffffff`, 0 |

Animation sounds (first-person graphs): see
"Animation sounds" below. Left empty: effect references and events,
inheritance, static poses and the header word at `+8`. Cyborg and vehicle
graphs carry their blend screens and weapon pairs. First-person graphs
carry the single pair (`any`, `any`).
