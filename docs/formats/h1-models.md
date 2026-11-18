# Halo 1 models and animations as Halo 2 tags (`crates/h1models`)

Status: **Built** (2026-11-16) for Halo 1 Xbox `mode` -> Halo 2 `mode` and
`antr` -> `jmad`, including compressed (keyframed) animations
(2026-11-17). It is proven by decoding the converted tags through
`crates/tags` and comparing them with the source. No converted tag has been
loaded by the game yet ("Needs a game run").

This is the model and animation step of the Halo 1 campaign work
([campaigns.md](../game/campaigns.md) §2). The Halo 1 cache container is in
[h1-map-format.md](h1-map-format.md), the Halo 2 targets in
[map-format.md](map-format.md) ("Render-model geometry"),
[structure-bsp.md](structure-bsp.md) and [animation.md](animation.md), and
the builder the output feeds in [map-builder.md](map-builder.md).

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
- **S:** field order and meaning recalled from general Halo modding
  knowledge (the Halo 1 and Halo 2 editing kits' field names), then checked
  against the data where a V label says so. No community code or
  definition file (Invader, Assembly plugins, OpenH2) was opened or copied
  for this page.
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
| `00` | objects block | S |
| `0c` | units block (24 on the cyborg) | V1 (count) |
| `18` | weapons block | S |
| `24` | vehicles block | S |
| `30` | devices block | S |
| `3c` | unit damage block | S |
| `48` | first-person weapons block | S |
| `54` | sound references block | S |
| `60` | f32 limp body node radius; `64` u16 flags | S |
| `68` | nodes, stride `0x40` | V1 |
| `74` | animations, stride `0xb4` | V1 |

The units, weapons and vehicles blocks map fixed engine slots to animation
indices. They are not read: names are mapped from the animations' own names
instead (below). **Some graphs have no node block** (the warthog, ghost,
scorpion and the scenery on beavercreek): they animate their model's
skeleton, and the converter takes the nodes from the model.

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
| Nodes (`0x60`) | name, parent `04`, first child `06`, next sibling `08`, import index -1 `0a`, default translation `0c`, rotation `18`, **default scale 1 at `28`, inverse forward/left/up at `2c`/`38`/`44`, inverse position at `50`, distance from parent at `5c`** | V2: on every lockout node, `2c`..`5c` invert the model-space default pose computed from `0c`/`18`, in our convention (vectors are the columns of R^T); `28` is 1 |
| Marker groups (`58`, `0xc`) | one per Halo 1 marker name: name, block of markers (`0x24`): i8 region (-1 = any), i8 permutation (-1 = any), i8 node, pad, translation `04`, rotation `10`, f32 `20` written 0 | V2: all 922 lockout markers sit in named groups at `58`, with a node in range and a unit rotation; `20` is 0 on 842 and nonzero on 80 (likely a scale; meaning not established) |
| Materials (`60`, `0x20`) | first reference null, second the caller's `shad` for that shader index, or null | map-format.md |
| Header `44` | 0, as on every retail `mode` (Halo 1's node list checksum was written here until converter version 38; one that fell in the map's address range read as a stray pointer and a fresh level refused the tag) | V |
| Section groups (`34`, `0xc`) | one: detail levels `0x3f` (all six) at `00`, compound nodes (`04` block) empty; the LOD bytes at `3c`..`41` stay 0, naming it | V: all 181 of 05a's `mode`s have exactly this. E: `0x00137650` reads `groups[byte[3c + lod]]`'s `+4` (compound node count) with no count check and adds the node count; an empty block faulted at `0x0013767a` (a30's second crash) |

Header flags (`04`): left 0. Retail sets `0x8` on all and `0x4` on 167
of 181 (not the first-person models); with `0x4` set, `0x00137650`
skips the section group and counts one. Meaning not established.

Left empty: the node
map block at `50`, errors, PRT info and render leaves, and the section
blocks the decoder does not interpret (`10`, `28`, `30`, `44`, `54`, `5c`).

### `jmad`

| What | How |
|---|---|
| Nodes (`0x20`) | name, next sibling, first child, parent, joint flags, base vector, vector range from the graph (or the model, for graphs without nodes) |
| Animations (`0x6c`) | name (below), node list checksum, type, node count, frame count, flag `0x10` (in a resource), resource 0, data offset, static pose -1, both link indices -1, frame info type 0 |
| Data | one raw resource for the whole graph (`0x200`-aligned and padded). Base animations: static stream, codec 1, holding every channel Halo 1 keeps in default data. Every animation: animated stream, codec 3, holding the channels Halo 1 animates (i16 rotations, which is Halo 1's own form; float translations and scales). Then both flag sets (each 3 arrays), set sizes `3 x ((n + 31) >> 3 & !3)` |
| Modes tree | mode -> class -> type -> actions from the mapped names; action graph index -1 |
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
| `first-person <action>` (a weapon's first-person graph) | mode, class and type `any`, animation `first_person:<action>`; `melee` becomes `melee_strike_1`; the graph's `+0x54` the one pair (`any`, `any`) | V3 (every retail first-person graph on Coagulation, 34, has the one mode `any` and that pair; h1objects `first_person_graphs_take_retail_modes`, which lists the converted actions retail has and has not: 19 shared on Blood Gulch, and `ammunition`, `enter`, `firing`, `light_off`, `moving`, `overcharged_jitter`, `overheating_again`, `overlays`, `reload_full2`, `stealth_melee` it lacks) |
| `%n` | `:var<n>` on the action | Design; retail variant suffixes (`:var0`, ...) sit on animation names, never on tree actions |

**No Halo 2 equivalent, or not mapped:**

- Unit damage (`h-kill`, `s-kill`, `s-ping`, with direction and region)
  stays one action from all its words (`h_kill:back_gut`): V3 that no
  retail tree holds `h_kill`, `s_kill` or `s_ping` (nor any mode for them).
  Retail death vocabulary is `dead`, `mortal_wound`, `landing_dead`,
  `airborne_dead`, `thrash_front/back`, `resurrect_front/back`, driven by
  the engine's damage tables rather than name lookup.
- Vehicle overlays (`suspension left back`, `vehicle steering`,
  `vehicle ground-speed`, `tread pitch ...`, `firing`, `ground speed`,
  `acc-front-back` and kin) also stay one word-run action: V3 that retail
  vehicle graphs carry only `combat:opening`/`combat:closing` — no
  `suspension` or `vehicle` mode, no `steering` or `ground_speed` action
  exists anywhere. Halo 2 drives suspension and steering outside the jmad
  tree.
- Seats that keep their Halo 1 label: `P-riderLM`, `P-riderLB`,
  `P-riderRM`, `P-riderRB` (their retail row number among `pelican_p_l01..05`
  is not recoverable from the trees) and the dropship seats
  `CD-passengerL01`/`CD-passenger01` (retail phantom seats are columns
  `phantom_p_a01..c03`, with no left/right correspondence).
- The engine-slot structure of Halo 1 graphs (units/weapons/vehicles
  blocks) is not converted. Halo 2's tree comes from names only.
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

**Skinned round trip** (V5, `h1_disc_bipeds_skin`, converter version
54). Each model and its graph are converted, built into a map and decoded
through `crates/tags`; every vertex is skinned the Halo 2 way (decoded
position, node indices through the section node map, weights, the
decoded jmad frame composed by Halo 2's rule, times the inverse frame the
`mode` stores at node `+0x2c`) and compared with the Halo 1 vertex skinned
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
