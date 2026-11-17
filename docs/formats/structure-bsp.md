# Structure BSP render geometry and lightmaps

This covers `crates/tags/src/geometry.rs` (the one geometry-section decoder,
shared with render models) and `crates/tags/src/structure.rs` (structure BSP
`sbsp` and lightmap `ltmp` render blocks). It extends
[map-format.md](map-format.md), whose evidence labels and address rules apply;
the render-model side is in map-format.md, "Render-model geometry". All
offsets are hexadecimal and little-endian. No game bytes, dumps or per-map
listings are in the repository; tests build invented data.

## Provenance

Every table below was derived from two sources only:

- **E — engine code:** instructions in the retail XBE, read with
  `cargo h2 disasm [iso] <addr> [count]`. The address cited is the instruction
  that reads or writes the field. Names are ours, not recovered symbols.
- **V — disc check:** a structural property checked on every campaign,
  multiplayer and main-menu map of the owner's disc by
  `cargo run -p viewer --release --bin bsp -- --all` (and, for model
  sections, `viewer --all all`).

No community plugin, XML layout or tag-definition file was opened or copied for
these tables, and none is a build input. Field names are descriptive labels
chosen here. A name is used only when E or V supports it; otherwise the field
is `unknown_*` or preserved in a `raw` byte array.

Engine anchors used throughout:

| Address | Role (our name) | Evidence used |
|---|---|---|
| `0x00041040` | `submit_structure_or_model_geometry` | Cluster/instance/definition strides and runtime section pointers; part kind test. |
| `0x00223c70` | geometry block fixup | Block-info, resource-entry and vertex-buffer-descriptor layouts. |
| `0x0012e210`, `0x0012e290` | geometry cache unload callbacks (registered by `0x0012d8b0`) | Runtime-offset, loaded-flag and cache-handle fields of block info. |
| `0x0004d2e4`..`0x0004d420` | lightmap selection for a structure render key | Lightmap group, render-info, bucket and bucket-reference layouts. |
| `0x000d170d`..`0x000d173e` | same tables, second consumer | Render-info layouts (cluster and instance). |

`0x004e0348` holds the active BSP pointer and `0x004e0344` the active lightmap
pointer, as already recorded in [engine-map.md](../engine/engine-map.md).

## Structure BSP blocks

| Field | Meaning | Evidence |
|---|---|---|
| `sbsp+08` u32 | Structure checksum; lightmap groups repeat it | V (76 of 78 lightmap groups match their scenario-paired BSP; see below) |
| `sbsp+9c` block, stride `b0` | Clusters | E `0x0004109e` (`[bsp+0A0h]`), `0x000410ac` (`imul 0B0h`) |
| `sbsp+a4` block | Materials; only the count is read | V: every part's material index is below it |
| `sbsp+138` block, stride `c8` | Instanced geometry definitions | E `0x000410ca` (`[bsp+13Ch]`), `0x000410d0` (`imul 0C8h`) |
| `sbsp+140` block, stride `58` | Instanced geometry placements | E `0x000410b4` (`[bsp+144h]`), `0x000410c2` (`imul 58h`) |

Other BSP header fields remain as described (or left opaque) in map-format.md.
Material elements, portals, sky ownership and visibility data are not decoded
here; a flat or normal-shaded shell does not need them.

### Geometry header (clusters, definitions, lightmap geometry)

Clusters and definitions both start with the same `0x54` bytes:

| Offset | Meaning | Evidence |
|---|---|---|
| `00` u16 | Total vertex count | V: equals every vertex stream's count |
| `02` u16 | Total triangle count | V: index count is exactly 3 × this |
| `04` u16 | Total part count | V: equals the loaded part count |
| `06`, `08` u16 | Shadow-casting triangle and part counts | D: Halo 2's section info order; V: equal to the totals on retail multiplayer clusters checked |
| `0c` u16 | `unknown_0c`, by Halo 2's order the opaque vertex count. No engine use found | V: ≤ total; every instance's lightmap bucket range fits when sized by this count, while 6 ranges overflow using the total |
| `0e` u16 | Opaque part count | D, as `06` |
| `12` u16 | Shadow-casting rigid triangle count | D, as `06` |
| `16` u16 | Compression flags: bit 2, the lightmap texture coordinate is compressed (stream `1f`) | D; V: 4 on every drawn retail multiplayer cluster but two |
| `24` u16 | Subpart count | V: equals the loaded subpart count |
| `26` u16 | Lighting flags: bit 0 lightmap texture coordinates, bit 1 incident radiance (stream `30`) | D; V: 3 on every drawn retail multiplayer cluster but two |
| other bytes to `28` | Not interpreted (`0a`, `10`, `14`, `18..23` zero on retail multiplayer clusters) | Preserved in `SectionInfo::raw` |
| `28`..`4c` | Geometry block info (below) | E `0x00223c70` |
| `4c` block | Runtime section pointer, filled after loading | E `0x000410d6` reads `+50h`; `0x00223c73`..`0x00223c87` writes count 1 and the pointer; V: empty on disc whenever a raw block exists |

Cluster-only fields:

| Offset | Meaning | Evidence |
|---|---|---|
| `54` f32×6 | Bounds: x min, x max, y min, y max, z min, z max | V: finite, and no loaded vertex position lies outside its cluster's bounds (tolerance 0.01) |
| `6c` i8 | First sky index into `scnr+0x8`; -1 falls back to `6e` unless that sky's `+0x10` bit 4 is set, then to the scenario BSP entry's sky (`+0x40`) | E `0x00130f60` |
| `6d`..`71` u8 | `0xff`, the sky index (E `0x0014b38d`), then `0xff` ×3 on retail multiplayer clusters (`6c` is `0xff` or 0) | V |
| `72`, `74` i16 | Background sound and sound environment: indices into the structure's own palettes `sbsp+0xd4` (`0x64` each) and `+0xdc` (`0x48` each), -1 none. The ambience pass reads the listener's cluster (its collision leaf's, `sbsp+0x2c` `i16` at `+0`) and keeps 64-entry arrays, so an index must be below 64 | E `0x0011bf46`, `0x0018d730`..`0x0018d8a5`, `0x0018ddf0`..`0x0018de0d`; V: all 4,191 retail clusters in range; on all 160 structures the two palettes name the scenario's (`scnr+0x250`, `+0x258`) entry for entry (`h1convert/tests/h2_sbsp.rs`) |
| `78`, `7a` i16 | Ambience and reverb sound cluster (`sbsp+0x148`, `+0x150`) | E `0x0018d7b4`, `0x0018dd10` (unchecked) |
| `84` block, 8 | Predicted resources: u16 type, u16 index, tag datum. Type 0 is image *index* of a bitmap (E `0x0016e656`, unchecked against the image count), 3 cluster *index*'s geometry block (E `0x0016e68e`), 2 a render model, 4 and 6..8 instanced and lightmap geometry buckets (E `0x0016e770`..`0x0016e8f0`, which index the lightmap group's tables unchecked). The engine predicts the camera cluster's list and those of the clusters it sees (E `0x0003f495`, `0x0003f4da` → `0x0016e5e0`) | E; V: every retail multiplayer cluster has one, including its own type 3 and type 7 entries and its shaders' bitmaps |
| `8c` block, u16 | Portals | V |
| `94` u32 | The structure checksum (`sbsp+0x08`) | V: equal on every retail multiplayer cluster |

Placements:

| Offset | Meaning | Evidence |
|---|---|---|
| `00` f32 | Uniform scale | V: finite and positive on all 12,961 placements |
| `04`, `10`, `1c` f32×3 | Forward, left, up basis rows | V: orthonormal and right-handed (tolerance 1e-3) on every placement |
| `28` f32×3 | Translation | V: finite; rendered instances sit on their host geometry |
| `34` i16 | Definition index | E `0x000410c5` (`movsx word [inst+34h]`); V: always in range |

World position = `translation + scale × (x·forward + y·left + z·up)`. The
split into a scalar and a rigid basis is V (the checks above, which the
validator enforces), not E; nothing is claimed for the bytes from `+36`.

### Geometry block info (`0x24` bytes)

| Offset | Meaning | Evidence |
|---|---|---|
| `00` u32 | Raw pointer to the block (file selector in the top two bits, as map-format.md); `0xffffffff` = none | V: every structure and bucket pointer selects the local map and fits |
| `04` u32 | Block size | V: equals `8 + section + resources + 4`; 0 exactly when the pointer is absent |
| `08` u32 | Section struct size | E `0x00223cb7` (resource base = buffer + 8 + this) |
| `0c` u32 | Resource area size | V: repeated in the `blkh` header |
| `10` block, stride `10` | Block resources | E `0x00223c8a`, `0x00223ca1`, `0x00223d61` |
| `18` u32 | Owner tag datum | V: the owning BSP, lightmap or model tag when a raw block exists; `0xffffffff` (or 0 in an all-zero record) otherwise |
| `1c` i16 | Offset from this record to the runtime section block | E `0x00223c73`, `0x0012e363`; V: `0x24` for structure and bucket blocks, `-8` for model sections |
| `1e` u8 | Runtime "loaded" flag | E `0x0012e31d`, `0x0012e379`; V: 0 for every raw block |
| `1f` u8 | Not interpreted | V: 1 when a raw block exists |
| `20` u32 | Runtime cache handle | E `0x0012e380`; V: `0xffffffff` for every raw block |

Lightmap cluster and instance geometry never has a raw block on the disc. Most
of those headers keep a nonzero section and resource size with no resources.
In 44 of them (in-tag sections) the runtime block at `4c` points into the tag
with count 1, and the block info may carry resource entries and runtime state.
These are range-checked, kept in `GeometryHeader::embedded_section` and not
decoded. Anywhere else, an absent block must have no resources.

### Raw block

A raw block is `blkh` (FourCC), u32 resource size, the section struct, the
resource area, then `blkf`. Every resource is preceded by an `rsrc` FourCC and
its offset is relative to the start of the resource area. E: the fixup adds 8
to skip the header (`0x00223c7e`, `0x00223cc0`). V: framing and markers hold on
every loaded block. The FourCC values are verified framing, not engine checks;
the engine does not test them. The decoder refuses blocks over 16 MiB before
reading them.

### Block resource (`0x10` bytes)

| Offset | Meaning | Evidence |
|---|---|---|
| `00` u8 | Kind: 0 tag block, 1 data reference, 2 vertex data | E `0x00223cc4`..`0x00223ccd`; other kinds are rejected |
| `01` u8 | Not read by the fixup | V: 2 for blocks and vertex data, 4 for data |
| `02` u16 | Not read by the fixup | V: equals `04` in every raw block (enforced there; in-tag sections differ) |
| `04` u16 | Offset of the patched field in the section struct | E `0x00223cda`, `0x00223d35`, `0x00223d40` |
| `06` u16 | Block: element size (count = size / this). Vertex data: descriptor index in the block at `04`. Data: `0xffff` | E `0x00223ccf`, `0x00223d44`..`0x00223d4e` |
| `08` u32 | Byte size | E `0x00223cf2`, `0x00223d39` |
| `0c` u32 | Offset in the resource area; `0xffffffff` skips the entry | E `0x00223ca8`..`0x00223cb1` |

The engine also accepts vertex data with index `0xffff` (the field is itself a
descriptor). No disc block uses it, so the decoder rejects it.

### Decoder rules

The rules below hold on every disc block and are enforced in `geometry.rs`, so
every owner's layout gets them:

- A block resource's patched field lies inside the section struct, and the
  struct's count there equals size / element size.
- Present resources never share a byte, counting their `rsrc` markers. No two
  share a key: (kind, field) for blocks and data, (field, descriptor index)
  for vertex data. Every vertex resource names a descriptor that exists.
- Decoded tables are borrowed ranges of the one loaded block; no resource is
  copied per descriptor. Tag-block reads are charged to a byte budget equal
  to the payload length (V: the largest BSP uses 0.13 of it), so aliased
  tables cannot multiply decoded output.
- Vertex streams: a type with a known stride must use it. No two streams in a
  section share a role (position, texcoord, tangent frame, lightmap texcoord,
  or the same unknown type). All streams agree on the vertex count, which
  must equal the owner's own count where it has one. A section with no streams
  must have count 0, no parts and no indices (V: 95 clusters).
- Every index is below the vertex count.
- Parts tile the index block in order without gaps or overlap. A part flagged
  as a triangle list has a multiple of 3 indices. Each part's subparts are a
  run that tiles the subpart block in order, and every subpart names its own
  part. In structure sections the subparts also tile the part's index range;
  in model sections they are empty or inside the part.

### Section struct and its blocks

The decoder is told where each interpreted block lives with a
`SectionLayout`: field offsets, element strides and the struct size, which
must match the block info's. For clusters and definitions
(`STRUCTURE_SECTION`, a `0x44` byte struct):

| Field | Element | Evidence |
|---|---|---|
| `00` block | Part, `0x48` bytes | E `0x000410da`..`0x000410e0` (section `+4`, `index × 0x48`); V: resource element size `0x48` |
| `08` block | Subpart, 8 bytes | V: resource element size 8; tiling above |
| `20` block | u16 index | V: element size 2 |
| `38` block | Vertex-buffer descriptor, `0x20` bytes | E `0x00223ced` (`shl 5`); V |

Fields `28` (data) and `30` (u16 elements) are also patched by resources but
are not decoded. They are the section's visibility MOPP code (`28`, a data
resource, secondary `0xffff`: a Havok code object as in [mopp.md](mopp.md),
its keys the subpart indices 0..n, V on Coagulation) and a `u16` reorder
table (`30`); every retail structure section that has the code has both.
The engine draws a cluster for a set of volumes (E `0x00044e90`, from the
pass at `0x00044720`) by querying the code with the volumes' planes
(`0x002dd930`, interpreter `0x002dc220`), and `0x000458d0` turns the result
into the cluster's subpart mask (a bit per subpart, cluster `+0x24`):

- a leaf a volume cuts emits its key: bit *key* is set (`0x00045a34`);
- a subtree wholly inside emits the keys of its first and last leaf (walks
  `0x002ff570`, `0x002ff210`), read through the table with *n* the subpart
  count (`+0x08`): positions `table[n + first]..=table[n + last]`, and bit
  `table[p]` for each (`0x000459cb`).

So `table[0..n]` is the keys in leaf order, each once, and `table[n + key]`
the key's position: V on all 4095 retail sections with a code
(`h1convert/tests/render_fields.rs`, `retail_visibility_layout`). Retail
tables run on past `2n`; a table longer than `2n` (with render entry flag
`0x1000`) switches to a second code stored after the first, its start the
first code's size rounded up to 16 (`0x00044f7a`..`0x00044fac`, then
`0x00045a80`); that layout is not decoded. A table count of 0 sends the
cluster down the object branch, which reads the render entry's null object
(E `0x0004539c`); a mask count (`+0x24`) of 0 keeps the cluster out of the
pass. Lightmap buckets use an 8-byte struct whose only field is
a descriptor block at `00` (`BUCKET_SECTION`). Render models use a `0x70`
byte struct (`MODEL_SECTION`, map-format.md).

Part (`0x48`):

| Offset | Meaning | Evidence |
|---|---|---|
| `00` u16 | Kind | E `0x000410fc`..`0x00041129`: kinds 0 and 5 are not submitted; no other value is named |
| `02` u16 | Flags; bit 3 (`0x8`) marks a triangle list, otherwise a strip | V: every structure part has bit 3 and a multiple of 3 indices; with this rule the derived triangle count equals the stored count on every structure and model section |
| `04` u16 | Material index | V: below `sbsp+a4`'s count on every structure part, and below the model's material count on every model part |
| `06`, `08` u16 | First index, index count | V: tiling |
| `0a`, `0c` u16 | First subpart, subpart count | V: tiling |
| `10` f32×3 | Bounding sphere centre: the centre of the box at `30` | V: equal on every Coagulation part checked |
| `2c` f32 | Bounding sphere radius, never 0 on disc | E `0x00045da1` reads it; a radius ≤ 0 is replaced by a constant |
| `30` f32×6 | Box of the part's vertices: x min, x max, y min, y max, z min, z max | V: equal to the box computed from the part's indices |

On every retail multiplayer structure part (2,450) the kind is 2 and the
flags are `0xb`; `1c..2b` are zero. The rest of the part is preserved. Only
bit 3 of the flags is interpreted.

Subpart (8): u16 first index, u16 index count (V: tiling), u16 unknown
(`0xffff` in structure sections), u16 owning part (V: always the index of the
part whose run holds it; enforced).

**Triangle lists.** Indices are a list exactly when part flag bit 3 is set,
and every structure part sets it. This is V, not E: 2,358,082 of 2,358,576
non-degenerate triangles have counter-clockwise winding agreeing with their
first vertex's packed normal. A strip would alternate. The draw call's
primitive type has not been located in the engine.

Vertex-buffer descriptor (`0x20`): u8 type at `00`, u8 stride at `01`
(E `0x00223cf8`: count = (size − `[desc+8]`) / stride, written back to u16 at
`02`; `0x00223d0f`..`0x00223d30` build a D3D resource header at `14`). On disc
only `00`/`01` are nonzero. Vertex data for descriptor *i* is the kind-2
resource with that index. Buckets reserve two descriptors and zero the unused
one; the decoder skips zeroed descriptors that have no data.

Stream types in structure sections and buckets (all V; the engine's
type-to-declaration mapping is not yet located; model types are in
map-format.md):

| Type | Stride | Interpretation | Check |
|---|---|---|---|
| `01` | 12 | f32×3 position | Finite; inside cluster bounds; renders the level |
| `18` | 8 | f32×2 texture coordinate | Stride and count here. The name rests on model sections, where `18` occurs exactly when the texcoord-compression flag is clear |
| `1b` | 12 | Three packed vectors, signed 11:11:10 (x bits 0–10, y 11–21, z 22–31); the first is the normal | Every non-zero vector is unit length (± 0.02); 12,104 are exactly zero; winding agreement above |
| `1f` | 4 | Two normalized i16 lightmap coordinates | All values in [0, 1] |
| `30` | 4 | Packed 11:11:10 incident light direction, length ≤ 1 (D: Halo 2's lightmap incident radiance; present exactly when section lighting flag bit 1 is set, 398 of 400 retail multiplayer clusters) | Unpacks with length ≤ 1, not unit, not the normal |
| `2f` | 3 | Not interpreted (buckets only) | Present exactly when bucket flag bit 1 is set |

## Lightmap

| Field | Meaning | Evidence |
|---|---|---|
| `ltmp+80` block, stride `68` | Lightmap groups | E `0x0004d2e9` (`[ltmp+84h]`, first group used directly). E `0x00015c00`..`0x00015c78`: the palette set-up loop walks every group from `[ltmp+84h]`, advancing the group offset with `add esi, 68h` at `0x00015c6b` and comparing the iteration with `[ltmp+80h]`; its inner loop steps palettes by `0x400` (`0x00015c51`). An earlier `0x98` reading rested on where the following data begins in an inspected map; every disc lightmap has one group, so that bound could not tell the two apart. A synthetic two-group test does |

Group (`0x68`):

| Offset | Meaning | Evidence |
|---|---|---|
| `04` u32 | Structure checksum | V: equals `sbsp+08` for 76 of 78 groups |
| `08` block, stride `400` | 256-entry u32 palettes | V: 1024-byte spans |
| `10` block | Not reviewed | V: empty on every disc map |
| `18` tag ref | Lightmap bitmap (`bitm`) | V: resolves; every render-info bitmap index is below its image count |
| `20` block, stride `54` | Cluster geometry (geometry header) | V: no raw blocks, no owner |
| `28` block, stride 4 | Cluster render info | E `0x0004d3f3`, `0x000d1732` (`[group+2Ch]`, `index × 4`) |
| `30` block, stride `54` | Instance geometry (geometry header) | V: no raw blocks, no owner |
| `38` block | Not reviewed | V: nonempty in 26 groups; not decoded |
| `40` block, stride `38` | Geometry buckets | E `0x0004d319`, `0x0004d389` (`imul 38h`) |
| `48` block, stride 4 | Instance render info | E `0x0004d3c2`, `0x000d172b` (`[group+4Ch]`) |
| `50` block, stride `c` | Instance bucket references | E `0x0004d373`..`0x0004d37c` (`[group+54h]`, `index × 0xc`) |
| `58` block | Not reviewed | V: nonempty in 63 groups; not decoded |
| `60` block, stride `c` | Cluster bucket references | E `0x0004d2f2`..`0x0004d303` (`[group+64h]`, `index × 0xc`) |

Render info (4 bytes): i16 bitmap index at `00` (E `0x0004d3ff`), i8 palette
index at `02` (E `0x0004d406`, `cmp -1` at `0x0004d40a`); −1 means none. V:
every value is −1 or in range, and counts equal the BSP's cluster and
placement counts.

Bucket (`0x38`):

| Offset | Meaning | Evidence |
|---|---|---|
| `00` u16 | Flags. Bit 0: the engine patches the bucket's first vertex descriptor with a reference's offset only when set (E `0x0004d326`, `0x0004d396`); otherwise no lightmap stream is bound. Bit 1: V, set exactly on buckets that carry a type-`2f` stream | V: every bucket has bit 0 |
| `0c` | Geometry block info | V: runtime offset `0x24`, owned by the lightmap |
| `30` block | Runtime section block (E `0x0004d32b`/`0x0004d39b` read `+34h`) | V: empty on disc (enforced) |

Bucket reference (`0xc`):

| Offset | Meaning | Evidence |
|---|---|---|
| `00` u16 | `unknown_0` | V: always 0; no engine use found |
| `02` u16 | Bucket index | E `0x0004d382`, `0x0004d315`; V: in range |
| `04` u16 block | First-vertex offsets into the bucket's streams | See below |

The engine multiplies the selected offset by 4 and writes it into the
bucket's first descriptor (`0x0004d3a1`, `0x0004d331`). For an instance it
reads the first entry (E `0x0004d37f`, `0x0004d386`); for a cluster it reads
the entry chosen by a 9-bit field of the render key (E `0x0004d309`..
`0x0004d311`). Neither path tests for `0xffff`. The decoder's `None` for
`0xffff` is a disc observation only: 66 references hold it as their first
entry, and the validator skips those. The engine reads an entry
unconditionally, so an empty offsets block is rejected.

V, for buckets with flag bit 0: every instance reference has exactly one
offset, and its range (offset + the definition's `unknown_0c`) fits its
bucket. Every cluster-reference offset lies inside its bucket.

## Observations from the disc

- All structure and bucket geometry is local to its map; no BSP section uses
  either shared resource cache.
- Six definitions have no raw block.
- Two lightmap groups carry a checksum different from their scenario-paired
  BSP. One of them is empty. The validator counts these
  (at most 2 allowed) and does not check their per-cluster tables.
- Part kinds seen: 2, 3, 4, 5. Only "0 and 5 are not drawn" is engine-backed.

## Writing

`h1convert::structure` writes a structure BSP and lightmap in this layout
from a Halo 1 level, as built block trees for `map::build`
([map-builder.md](map-builder.md)): what each field holds and which are
left empty is in [h1-multiplayer.md](../game/h1-multiplayer.md), "The
converted structure BSP". Its collision BSP comes from the Halo 1 source
([collision.md](collision.md), `Bsp::encode_blocks`), its Havok MOPP code
from [mopp.md](mopp.md), and AI pathfinding can be generated from the
collision ([pathfinding.md](pathfinding.md)). The converted lightmap has one
group and no bitmap yet ([h1-bitmaps.md](h1-bitmaps.md)).

## Use

```sh
cargo run -p viewer --release --bin bsp -- --all
cargo run -p viewer --release --bin bsp -- <map>
cargo run -p viewer --release --bin bsp -- <map> --render "$OUT" --bsp 0 [--force]
```

The disc comes from `--disc`, `H2_DISC` or `disc/halo2.iso`, as for the
viewer. `--all` and single-map runs exit 1 if any check fails. Zero-tolerance
checks are: decode failures, tiling, non-local blocks, positions outside
cluster bounds, non-unit packed vectors, lightmap coordinates outside [0, 1],
non-rigid placements, bucket flag bit 1 against the `2f` stream, and instance
references without exactly one offset. Thresholds, in `viewer/src/bin/bsp.rs`,
cover the counters that are nonzero on the disc: zero packed vectors (≤ 0.2%),
winding disagreement (≤ 0.1%) and checksum-mismatched groups (≤ 2).

`--render` writes a top-down normal-coloured image and a perspective
Lambert-shaded image of one BSP's clusters and instanced geometry (software
rasterized, back faces culled). It uses the viewer's safe output rules: no
writing through symlinks, and no replacing a file without `--force`. Write
the images outside the repository: they are derived from game data.

```rust
use tags::structure::StructureRender;
let bsp = StructureRender::read(&mut map, bsp_id)?;
for cluster in &bsp.clusters {
    let materials = Some(bsp.material_count as usize);
    if let Some(section) = cluster.geometry.load(&mut raw, materials)? {
        let v = section.vertices(None)?; // positions, normals, lightmap uvs
        for part in section.parts.iter().filter(|p| p.is_drawn()) {
            let triangles = section.part_triangles(part);
        }
    }
}
```
