# Halo 1 Xbox cache files (version 5)

This describes the reader in `crates/h1map`, step 1 of the campaigns work
([campaigns.md](../game/campaigns.md)): Halo: Combat Evolved's Xbox maps,
read so they can later be converted into Halo 2 v8 maps
([map-format.md](map-format.md)). It is not an on-disk Rust struct ABI.
All integers are little-endian. Offsets are hexadecimal. Unless a section
says otherwise, they are offsets into the **decompressed** file: the
`0x800`-byte header followed by the inflated body.

Evidence labels, as in map-format.md:

- **V — verified-on-disc:** checked against the owner's retail Halo 1 maps
  by the ignored tests `real_maps_read_and_their_multiplayer_parts_and_collision_check_out`
  (maps as stored, from `$H1_MAP_DIR`) and `real_disc_every_map_reads` (the
  whole disc, `$H1_ISO`).
- **S — from-source:** public documentation describes this, and the reader
  relies on it, but it has not yet been checked on the disc.
- **Design:** an implementation choice, not a format fact.

**Status (2026-11-16):** checked on two retail multiplayer maps
(`beavercreek`, `carousel`), cut from a partial download of the disc as
stored (compressed). What they confirm is marked V; the rest stays S until
the whole disc (campaign and UI maps) is checked.

No map bytes, tag dumps, tag-name lists or extracted assets are included
here or in tests. The tests build invented maps and discs in memory
(`h1map::testing`).

## Sources

- The Reclaimers Library (c20), *Maps* page, sections "Map header", "Tag
  index header", "Tag array entry", "Compressed maps" and "Tag space":
  <https://c20.reclaimers.net/h1/maps/> (read 2026-11-16; the wiki has no
  revision IDs to pin).
- c20, *scenario* tag page, the `structure bsps` block's fields:
  <https://c20.reclaimers.net/h1/tags/scenario/>.
- SnowyMouse, *Halo map file structure*, revision 2.1, Open Carnage topic
  6693, credited by c20.
- Invader (<https://github.com/SnowyMouse/invader>) is the reference
  implementation c20 credits. It is GPL: it may be cited, but none of its
  code or tag definition files may be copied here, and none were.

Field offsets inside tags (the scenario's BSP block) are not on c20's tag
page; those are marked S with a note.

Model and animation tag layouts (`mode`, `antr`) are in
[h1-models.md](h1-models.md), checked on real maps.
Bitmap tags (`bitm`) and where their pixels are in the file are in
[h1-bitmaps.md](h1-bitmaps.md), checked on every map of the disc.

## Header and compression

**S:** the header is `0x800` bytes and is stored uncompressed. `head`
(bytes `daeh`) at `0`, `foot` (bytes `toof`) at `0x7fc`. For cache version 5
(Xbox) everything after the header is one zlib stream. PC maps (version 7)
and Custom Edition (`0x261`) are not compressed and are refused by this
reader (`Error::Unsupported`).

| Offset | Type | Meaning | Evidence |
|---|---|---|---|
| `000` | FourCC | `head` | V |
| `004` | u32 | Cache version, 5 | V |
| `008` | u32 | Decompressed file length, header included | V: both maps inflate to exactly this less `0x800` |
| `00c` | u32 | "Padding length", Xbox only; kept as `field_0c` | S, meaning unverified: small values (`0x12d`, `0x336`) unrelated to the stored or inflated size |
| `010` | u32 | Tag data offset in the decompressed file (the tag index header) | V |
| `014` | u32 | Tag data size | V |
| `020` | char[32] | Scenario (map) name, NUL-terminated | V: equals the file name |
| `040` | char[32] | Build, `01.10.12.2276` on Xbox | V |
| `060` | u16 | Cache type: 0 campaign, 1 multiplayer, 2 UI | V for 1 (multiplayer); 0 and 2 S |
| `064` | u32 | CRC32 over BSPs, model data and tag data | S |
| `7fc` | FourCC | `foot` | V |

**Design:** the reader inflates the stream and requires exactly
`file_length - 0x800` bytes out of it (one byte more or less is an error),
then keeps header + body as one buffer, so header offsets work unchanged.
Bytes after the end of the zlib stream (sector padding) are ignored.
**V:** `file_length` counts the header. The two maps as stored end exactly
at the end of the zlib stream (no padding after it). `0x0c` is still open.

**S:** size limits by type: campaign 278 MiB, multiplayer 47 MiB, UI 35 MiB
(the disk cache partition each one is inflated into). The reader caps
`file_length` at 512 MiB (Design).

## Tag index header

At the header's tag data offset.

| Offset | Type | Meaning | Evidence |
|---|---|---|---|
| `00` | u32 | Address of the tag array | V |
| `04` | u32 | Scenario tag ID | V: tag 0, a `scnr` |
| `08` | u32 | Checksum | S |
| `0c` | u32 | Tag count (at most 65535) | V |
| `10` | u32 | Model part count | S (equals `18` on both maps) |
| `14` | u32 | Vertex data **address** (Xbox) | V: an address inside the tag data |
| `18` | u32 | Model part count, repeated | V: equals `10` |
| `1c` | u32 | Triangle (index) data **address** (Xbox) | V: an address inside the tag data |
| `20` | FourCC | `tags` | V (never at `24` on Xbox) |

**S:** on Xbox the model data is inside the tag data, so `14` and `1c` are
addresses; on PC they are file offsets, and a model data size pushes `tags`
to `24`. The reader accepts `tags` at `20` or `24` and keeps the fields
before it as `model_fields` (Design: the PC shape is tolerated, not needed).

## Addresses

**S:** the Xbox engine loads tag data at `0x803a6000`
(`XBOX_METADATA_BASE`): that address is the tag index header. So a tag
data address `a` is at file offset `tag_data_offset + (a - base)`, and must
fall inside the tag data size. The active BSP is loaded at the other end of
the 22 MiB tag space.

**Design:** the reader does not assume the base. The tag array directly
follows the index header, so it derives
`base = tag_array_address - index_header_len` from each file
(`H1Map::metadata_base`). The disc check asserts that this equals
`0x803a6000` for every map. **V** on both maps.

## Tag array

`tag count` entries of `0x20` bytes at the tag array address.

| Offset | Type | Meaning | Evidence |
|---|---|---|---|
| `00` | u32 | Group (FourCC as a u32: `scnr` is the bytes `rncs`) | V |
| `04` | u32 | Parent group, or `ffffffff` | V |
| `08` | u32 | Grandparent group, or `ffffffff` | S |
| `0c` | u32 | Tag ID: array index in the low 16 bits, salt above | V |
| `10` | u32 | Address of the tag path (NUL-terminated, no extension) | V |
| `14` | u32 | Address of the tag's data | V |
| `18` | u32 | External flag; padding on Xbox (no resource maps) | S |
| `1c` | u32 | Unused | S |

Group codes shorter than four characters are space-padded (`hmt `).

**Design:** the reader requires each ID's index to be its position in the
array, and the scenario ID to name a tag. A tag's data range runs from its
address to the next tag's data (by address), or to the end of the tag data;
external tags and addresses outside the tag data have no range. The real
size of a tag is its layout's business, which the converter will know.

## Tag blocks and references

**V:** a tag block field is 12 bytes: count, address of element 0, unused.
A tag reference field is 16 bytes: group, path address, path length, tag
ID. `H1Map::block` turns a block and an element size into the elements'
bytes; an empty block reads nothing, whatever its address.

## Structure BSPs

**S:** each BSP lives in its own region of the file, outside the tag data,
and is loaded at the end of tag space when it becomes active. The scenario's
`structure bsps` block holds, per BSP: BSP start (file offset), BSP size,
BSP address (where the region is loaded), and a reference to the `sbsp`
tag. c20 names these fields but gives no offsets.

| Where | Value | Evidence |
|---|---|---|
| Scenario tag | `structure bsps` block at `5a4` | V |
| Element | `0x20` bytes: `00` start, `04` size, `08` address, `0c` unused, `10` `sbsp` reference | V: the region starts at file offset `0x800`, straight after the header, and the reference names the map's `sbsp` |

`H1Map::structure_bsps` reads them, checks each reference names an `sbsp`
tag and each region lies inside the file, and `H1Map::bsp_bytes` returns a
region. Reading inside a BSP region (its header, and addresses relative to
the BSP address) is left to the converter.

### The BSP region

**V** (both maps): the region opens with a `0x18`-byte header. Addresses
inside a region are relative to the BSP address from the scenario (the
region is loaded there), not to the tag data base.

| Offset | Type | Meaning | Evidence |
|---|---|---|---|
| `00` | u32 | Address of the `sbsp` root struct, inside the region | V |
| `04` | u32, u32 | Count and address: rendered lightmap vertex data | V for count = the second pair's count; meaning S |
| `0c` | u32, u32 | Count and address, the same count again | V for the count; meaning S |
| `14` | FourCC | `sbsp` | V |

`H1Map::structure_bsp` returns the region as a `Space` (bytes at an
address) with its root struct. The tag array's `sbsp` entry has data
address 0; its data lives only in the region.

### `sbsp` root (`0x288` bytes)

Strides were read from the spacing of consecutive blocks (count × stride
equals the gap to the next block); "V" here means both maps agree.

| Offset | Field | Element | Evidence |
|---|---|---|---|
| `000` | Lightmap bitmap reference (`bitm`) | | V: resolves to a `bitm` |
| `0a4` | Collision materials | `0x14`: a shader reference, then 4 bytes | V: every reference names a shader (`shdr` child) |
| `0b0` | Collision BSP (one element on both maps) | `0x60` | V |
| `0bc` | Render BSP nodes | 6 | V (stride) |
| `0c8` | World bounds: x, y, z as (min, max) pairs | | V: every spawn point lies inside |
| `0e0` | Leaves | `0x10` | V (stride) |
| `0ec` | Leaf surfaces | 8 | V (stride) |
| `0f8` | Surfaces (three u16 vertex indices) | 6 | V (stride) |
| `104` | Lightmaps (one per lightmap page) | | V (count) |
| `134` | Clusters | | V (count) |
| `140` | Cluster data (data reference, 20 bytes) | | V (shape) |
| `154` | Cluster portals | | V (count) |
| `178`, `184`, `190` | Fog planes, fog regions, fog palette | | count only |
| `1b4` | Weather palette | | count only |
| `1e4` | Pathfinding surfaces | 1 | V (stride) |
| `1fc`, `208` | Background sound and sound environment palettes | | count only |
| `214` | Sound PAS data (data reference) | | shape only |
| `24c`, `258` | Detail objects, runtime decals | | count only |

### Render geometry

Read by `StructureBsp::lightmaps` and `render_surfaces`. **V** on both
maps: the materials' surface runs tile the render surfaces in order, every
index is below its material's vertex count, every vertex is inside the world
bounds, every packed normal is unit length (±0.02), and lightmap texture
coordinates are in [0, 1].

Render surface (`sbsp+f8`, 6 bytes): three `u16` vertex indices into the
material that owns the surface.

Lightmap (`sbsp+104`, `0x20` bytes): `i16` image index in the lightmap
bitmap at `00` (`-1`: no lightmap; its materials have no lightmap
vertices), materials block at `14`.

Material (`0x100` bytes):

| Offset | Field | Evidence |
|---|---|---|
| `00` | Shader reference (a `shdr` child: `senv`, `sgla`, ...) | V |
| `14`, `18` | `i32` first render surface, surface count | V: tiling |
| `28`.. | Ambient colour, distant lights, reflection tint, shadow vector and colour (Halo 1 lighting scalars) | S: values look like colours and unit directions; not used yet |
| `b0`, `b4` | `u16` rendered vertex type (1), `u32` vertex count | V for the count |
| `c0` | Address of the material's Xbox vertex buffer resource (12 bytes) in the region header's first array | V: in the array |
| `c4`, `c8` | `u16` lightmap vertex type (3), `u32` count (= vertex count) | V |
| `d4` | Address of the lightmap vertex buffer resource, in the second array | V |
| `ec` | Data reference (size, flags, file offset, address, unused) to the compressed vertices | V: size is `32 × count`, plus `8 × count` when lit |

The region header's two arrays (`04`, `0c`) hold one 12-byte Xbox vertex
buffer resource per material, `0x41` of them on beavercreek, matching its
65 materials (V for the count; the resource layout isn't read).

Compressed rendered vertex (32 bytes): position (3 × `f32`), normal,
binormal, tangent (each a `u32` packed signed 11:11:10: x in bits 0–10, y
11–21, z 22–31, each over its field's largest magnitude), texture
coordinate (2 × `f32`). Compressed lightmap vertex (8 bytes): packed
incident direction, lightmap coordinate (2 × `i16` over 32767). All rendered
vertices come first, then all lightmap vertices.

### Collision BSP (`0x60`)

Eight blocks, `{count, address, unused}` each:

| Block | Element | Layout | Evidence |
|---|---|---|---|
| `00` 3D nodes | 12 | `i32 plane`, `i32 back`, `i32 front`. Child: `-1` solid, bit 31 a leaf (low 31 bits), else a node | V: every spawn point (raised 0.3 m) walks to a leaf with the rule "front when `n.p - d >= 0`", and solid lies within 1 m below every one; with the children swapped all 133 spawn points are solid |
| `10` planes | 16 | `nx, ny, nz, d`, `n.p - d = 0` | V: unit normals |
| `20` leaves | 8 | `u16 flags`, `u16 2D reference count`, `i32 first 2D reference` | V: ranges in bounds |
| `30` 2D references | 8 | `i32 plane` (bit 31 flips it), `i32 node` (bit 31: a surface) | V: indices in range |
| `40` 2D nodes | 20 | `a, b, d`, `i32 back`, `i32 front` (bit 31: a surface) | V: indices in range |
| `50` surfaces | 12 | `i32 plane` (bit 31 flips), `i32 first edge`, `u8 flags`, `i8 breakable surface`, `u16 material` | V: below |
| `60` edges | 24 | `start vertex, end vertex, forward edge, reverse edge, left surface, right surface` (`i32`, `-1` none) | V: below |
| `70` vertices | 16 | `x, y, z`, `i32 first edge` | V |

Ring rule, the same as Halo 2's object collision ([collision.md](collision.md)):
from a surface's first edge, an edge whose right surface is this surface
gives its end vertex and continues at its reverse edge; otherwise its start
vertex and its forward edge, until the first edge comes round. **V:** on
both maps every ring closes and every ring vertex lies within 1 cm of its
surface's (flip-applied) plane. That one check confirms the plane, flip,
surface, edge and vertex layouts together. `CollisionBsp::validate` runs it.

The Halo 1 layout is Halo 2's with every index widened to `i32` (Halo 2
packs 3D children into 24 bits and the rest into `u16`, with bit 15 for the
flags Halo 1 keeps in bit 31).

## Scenario: multiplayer parts

Read by `H1Map::multiplayer`. Stride evidence as for the `sbsp` root.

| Offset | Block | Element | Evidence |
|---|---|---|---|
| `030` | Skies: a `sky ` reference each | `0x10` | V |
| `210`, `21c` | Scenery placements, palette | `0x48`, `0x30` | V (stride) |
| `240`, `24c` | Vehicle placements, palette | `0x78`, `0x30` | S: neither map has vehicles; the offsets follow the placement/palette pattern, which `2dc`/`2e8` (sound scenery, `0x28`) confirm |
| `354` | Player starting locations | `0x34` | V |
| `378` | Netgame flags | `0x94` | V |
| `384` | Netgame equipment | `0x90` | V |
| `390` | Starting equipment | `0xcc` | V (stride) |
| `3a8`, `3b4` | Decals, decal palette | `0x10`, `0x10` | V (stride) |

Player starting location (`0x34`): position `00`, facing `0c` (radians),
`u16` team `10`, `u16` BSP `12`, four `u16` game types `14`..`1a`, padding.
V: positions inside the BSP's bounds, facing in ±π, game types 12 and 13.

Netgame flag (`0x94`): position `00`, facing `0c`, `u16` kind `10`, `u16`
team (teleporter channel for teleporters) `12`, weapon group `itmc`
reference `14` (null on both maps), padding. Kinds: 0 CTF flag, 1 CTF
vehicle, 2 oddball, 3 race track, 4 race vehicle, 5 Vegas bank, 6 teleport
from, 7 teleport to, 8 hill flag. V: every kind on both maps is in 0..=8 and
teleporter pairs share a channel. The names are from c20; the values are
what the maps hold.

Netgame equipment (`0x90`): `u32` flags `00`, four `u16` game types `04`,
`u16` team `0c`, `u16` respawn seconds `0e`, padding to `40`, position
`40`, facing `4c`, item collection reference `50`, padding. V: every
reference names an `itmc`, every spawn sits in the map.

Game types (`u16`): 0 none, 1 CTF, 2 slayer, 3 oddball, 4 king of the hill,
5 race, 12 all, 13 all except CTF, 14 all except race and CTF (S: names from
c20; 12 and 13 seen).

Item collection (`itmc`): permutations block at `00` (`0x54` each: weight
`f32` at `20`, item reference at `24`, a `weap` or `eqip`), `u16` default
respawn seconds at `0c`. V.

## The disc

| Fact | Evidence |
|---|---|
| `default.xbe`'s certificate title ID is `4D530004` | S |
| Maps are files directly under `\maps\`, named `<scenario>.map` | S |
| Campaign levels: a10, a30, a50, b30, b40, c10, c20, c40, d20, d40 | S |
| `ui.map` holds the menus; Xbox has no `bitmaps.map`/`sounds.map` | S |

`h1map::disc` checks the title ID and lists the maps. `campaign_maps()`
needs every level and adds `ui`, `bitmaps` and `sounds` when present.

## Commands

- `h2 h1 info <iso|map> [map]`: with a disc and no map, the title and each
  map's (uncompressed) header; with a map, its header, tag index, base,
  structure BSPs and tag counts by group.
- `h2 h1 tags <iso|map> [map] [--group G]`: ID, group, data range and name
  of every tag (or one group's).
- `h2 h1 bitmap <iso|map> [map] <tag> --out DIR [--mips]`: one bitmap's
  images as PNGs ([h1-bitmaps.md](h1-bitmaps.md)).
- `h2 import halo1 [iso] [--data DIR] [--force] [--dry-run]`: checks the
  title and every campaign map's header, then copies the maps as stored
  (still compressed) into `<data>/campaigns/halo1/source/` and writes
  `manifest.txt` there: `<sha256>  <size>  <name>` per file. `--data`
  defaults to `$TMPDIR/h2boot-data` and the disc to `disc/halo1.iso`.

## Verifying on the disc

```
H1_MAP_DIR=/path/to/maps cargo test -p h1map --release -- --ignored real_maps --nocapture
H1_ISO=/path/to/halo1.iso cargo test -p h1map --release -- --ignored real_disc --nocapture
```

`real_maps_read_and_their_multiplayer_parts_and_collision_check_out` reads
every `*.map` in a folder (as stored on the disc), checks the header, index,
base and names as below, then reads the multiplayer parts and each BSP's
region, root and collision BSP: references resolve to the right groups, the
collision BSP validates (rings on planes), and every spawn point is in open
space with ground under it. Run it on the two multiplayer maps; it passes.

`real_disc_every_map_reads` opens every map on the disc and asserts: the
title ID; every campaign map present; each map inflates to exactly the
header's length; the derived base is `0x803a6000`; the header name matches
the file name; the scenario tag is a `scnr`; every tag name is non-empty
ASCII; every non-UI map has structure BSPs, each naming an `sbsp` tag, with
its region outside the tag data. It prints each map's header fields, base,
tag count, index header length, model fields and BSPs, from which this
page's table rows can be marked V.
