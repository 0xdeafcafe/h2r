# Map format: record codecs and render-model geometry

Generated record codecs and `mode` geometry, split from [map-format.md](map-format.md).

| Section |
|---|
| [Generated record codecs](#generated-record-codecs) |
| [Render-model geometry (`mode` sections, parts, vertices)](#render-model-geometry-mode-sections-parts-vertices) |

Part of [map-format.md](map-format.md).

## Generated record codecs

This section records the implementation in `crates/formats/tags`. For block claims,
**V means count × stride fits in bounds on every disc instance**, with the
additional checks stated below. **S** marks a meaning that is unverified on the
disc and in the engine.

**Design:** the scenario, globals, BSP and model child blocks are not
catalogued, because bounds fitting the disc is not enough to justify a name
or a stride. Those areas are coalesced `Bytes` fields. Only bitmap child records are interpreted. Observed-empty slots
remain as `unsupported_<offset>` guards because the strict decoder actively
checks them; there is no guessed stride or semantic name for those slots.

This is a manually authored, limited binary interface and codec. Disc checks
corroborate particular numerical properties; they do not prove field semantics.

### Per-table evidence

No semantic field in the retained tables is claimed to have been discovered
from disc alone.

| Retained table | Fields | Evidence from the disc; remaining limits |
|---|---|---|
| `BitmapHeader` | root offsets and scalars (the Halo 1 converter's) | `0x4c` prefix fits; counts/pointers at `0x3c/0x44` give bounded, aligned sequence/image spans. Scalar editor settings remain S. |
| `BitmapImage` | format/flag fields and resource field offsets | Repeated signatures, `0x74` stride, separate depth/flag bytes, three selected LOD ranges and low-detail ranges fit. Format, swizzle, dimensions and mip semantics are S until pixel decoding/rendering corroborates them. |
| `BitmapSequence` | label/image-range fields and sprite descriptor | `0x3c` count × stride fits; sprite spans are bounded/aligned. Label and image-range interpretations are S. |
| `BitmapSprite` | index/rectangle/registration fields | `0x20` count × stride fits. Coordinates have not been rendered; those meanings remain S. |
| `ScenarioHeader` | retained scalars, nine references, three data refs, observed-empty offsets | Reference IDs/classes resolve, data byte extents fit, guards have zero counts. Scalar meanings are S; all other child-block regions are opaque bytes. |
| `GlobalsHeader` | language scalar, two references, empty guard | References resolve and the guard is empty. Language semantics remain S; child-block regions are opaque. |
| `LocaleDescriptor` | runtime words, counts, file offsets | Nine slots from `0x188` through `0x284` fit exactly; nonempty index/data extents fit the file. Runtime-word meanings are S; localized strings are not decoded. |
| `BspHeader` | retained scalars, one reference, four data refs, observed-empty offsets | Reference resolves; data ranges fit the BSP's own address space and tag payload; guards are empty. Geometry/physics scalar meanings remain S. Child-block regions are opaque. |
| `RenderModelHeader` | name and two empty guards; the rest opaque | Name StringID index/encoded length resolves; guards are empty. Other scalar meanings remain S; geometry child-block regions are opaque. |

**Locale offsets:** full descriptors start at `matg + 0x188 + i * 0x1c`, for
`i = 0..8` (E `0x1a01c3`, [localisation.md](localization.md)). `+0x190` is the
**count field**, eight bytes into the first descriptor, after two runtime
pointer words at `0x188` and `0x18c`. In the full record, count is `+8`, data
size `+0xc`, index file offset `+0x10`, data file offset `+0x14`, and loaded
state `+0x18`. The language selector is a separate scalar at `matg +0xac`; it
does not account for the eight-byte difference.

### Schema and generation

`crates/formats/tags/schema.rs` declares records, offsets, field kinds, and the three
reviewed bitmap block strides in Rust. Unreviewed gaps become coalesced `Bytes`
fields; they are not scanned for plausible pointers. `build.rs` checks record
limits, duplicate names, overlapping fields, and nonzero reviewed strides.
Child-block declarations name their element record, whose fixed size must
match the stride. Generated `OUT_DIR/records.rs` contains multi-line Rust.

Each generated record has fixed `SIZE`, field metadata, `decode` and
`encode_into`. Compile-time assertions tie schema field widths to codec widths.
Enums/flags preserve unknown values and retain labels for readable output.
References and data references are separate eight-byte wire types; resource
file selectors are distinct from metadata virtual addresses. Native struct
size and `repr(C)` do not participate in decoding.

Uninterpreted bytes round-trip, and dumps summarize opaque spans by byte length.
Codecs accept exactly their fixed size; the loader selects a prefix of the
larger tag payload. Encoding edits one fixed record: it does not relocate blocks,
resize a cache, update checksums or save a map. Hand-written domain types sit
above generated records. Derived image fields describe the decoded snapshot;
editing a wire record does not automatically regenerate that snapshot.

| Record | Fixed bytes | Coverage |
|---|---:|---|
| BitmapHeader | `0x4c` | Root fields; both child blocks |
| BitmapImage | `0x74` | Dimensions, formats/flags, mips, three LODs, low-detail fields |
| BitmapSequence | `0x3c` | Label/image range, sprite block |
| BitmapSprite | `0x20` | Image index, rectangle, registration |
| ScenarioHeader | `0x3e0` | Retained scalars/references/data refs, nine empty guards; other spans opaque |
| GlobalsHeader | `0x284` | Language, references, one empty guard; other spans opaque |
| LocaleDescriptor | `0x1c` | Nine embedded records, file range checks |
| BspHeader | `0x23c` | Retained scalars/reference/data refs, seven empty guards; other spans opaque |
| RenderModelHeader | `0x84` | Name, two empty guards; other spans opaque |

### Validation modes and limits

`DecodedTag::read` uses `DecodeMode::Strict`. A nonzero count in an `EmptyBlock`
guard returns `UnsupportedBlock`, wrapped with record/field context.
`DecodedTag::read_with_mode(..., DecodeMode::Lenient)` instead preserves that
unsupported descriptor and adds a `DecodeWarning` containing record, field,
count and address. It does **not** dereference the unknown pointer or count it
as a verified nonempty block. Callers must inspect `DecodedTag::warnings`.
Reviewed ranges, references and StringIDs still fail on corruption in both modes.
Leniency does not add support for cache builds rejected by `map`.

These modes apply only to declared guards. Opaque byte spans have no interpreted
block descriptors and are preserved in both modes. In particular, no claim is made to validate every top-level pointer in scenario/globals/BSP/model.
Adding such coverage requires individually reviewed child definitions and
evidence.

**V: count × stride fits in bounds on every disc instance** for the three
reviewed bitmap block kinds. Nonempty pointers are also four-byte aligned.
Span ends are checked against both the correct virtual address region and the
owning tag payload; bitmap children are decoded at their declared fixed sizes.
Total decoded sprite count is capped at `payload_len / 0x20` so repeated,
aliased sprite spans cannot produce quadratic allocations. Bounded aliasing is
allowed; arbitrary next-block order or non-overlap is not assumed.

The [lighting decoder](lighting.md) also verifies three sky-child
aliases into other sky payloads and one zero-sized fog identity alias on the
retail disc. Those reads require a same-class owning payload in the identical
address space; zero-sized roots require an exact metadata address match.
They are explicitly budgeted exceptions to own-payload-only traversal, not a
general fallback to arbitrary bytes elsewhere in metadata. See lighting.md
for the field evidence, limits and synthetic rejection cases.

**V for the retained checks:** all five supported groups in all 30 maps pass
strict decoding. Root codecs round-trip exactly, retained data references fit,
root tag references resolve full salted IDs with concrete/parent/grandparent
class compatibility, and model StringID lengths match. Empty guards confirm
only zero counts, never a stride. The verifier reports aggregate coverage.

`validate_resources(&ResourceSizes)` also requires lengths for every
selected resource file and checks full LOD/low-detail extents. The disc verifier
supplies those lengths. This proves file containment, not a valid texture;
pixels, mip packing, swizzling, palettes and cube faces still need review.
Localized text, geometry, physics, materials and scripting bodies remain
undecoded. No game-derived fixtures, strings or dumps are checked in.

```sh
cargo test -p map -p tags -p cli
```

Run the aggregate checker against your own disc:

```sh
cargo run -p tags --example verify_disc -- disc/halo2.iso
```

Dump one tag to the terminal, quoting its full cache path:

```sh
cargo h2-tools tag <map> '<folder>\<subfolder>\<tag-name>' scnr --disc <disc.iso>
```

`h2 tag <map-name-or-file> <tag-name> [group] [--lenient] [--disc disc.iso]`
reports ambiguity instead of selecting an arbitrary class. `--lenient` prints
warnings for unsupported populated guards. Unknown enum values remain numeric;
known values use metadata labels. Scalar arrays stay on one line, null tag
references print `none`, and zero-count blocks collapse to an `empty_blocks`
summary. External resource files are not opened by this command. Keep redirected
dumps under `$TMPDIR`, never in the repo.

Synthetic tests cover sprite alias amplification, exact sizes and round trips,
opaque preservation, strict/lenient guards, reviewed-block alignment, tag class
mismatch and inheritance, model StringID length mismatch, low-detail integer
overflow, complete resource extents, BSP-specific address translation, all nine
locale slots, and readable generated-code/record formatting.


## Render-model geometry (`mode` sections, parts, vertices)

The model reader is in `crates/formats/tags/src/model.rs`, and its tag records are in
the delimited "render_model geometry" section of `crates/formats/tags/schema.rs`. Raw
section blocks are loaded by the shared geometry decoder,
`crates/formats/tags/src/geometry.rs`, which structure BSPs and lightmaps also use.
Its block info, resource, part, subpart and descriptor layouts, and the
hostile-input rules every owner gets, are documented once in
[structure-bsp.md](structure-bsp.md). This section covers what is
model-specific: the `MODEL_SECTION` layout, compression, skinning and the
model cross-checks. The root
`RenderModelHeader` record above is unchanged: its child regions stay opaque
there. `ModeGeometryHeader` is a second view of the same `0x84` bytes, used
only by the geometry decoder.

Studio's model viewer displays a model. The `D` evidence
below came from a sweep of every `mode` tag on the disc by a standalone viewer
that has been removed (`git log -- crates/tools/viewer`); nothing repeats it.

### Evidence and names

Evidence labels in this section:

- **E**: the engine's own code reads the field. Addresses are in the retail XBE
  and can be checked with `cargo h2 disasm`.
- **D**: disc evidence, checked on all 4,222 `mode` tags in all 30 maps by
  the removed viewer's `--all` sweep.

Names follow two rules:

1. A name describes what the evidence shows. Examples are the section's
   vertex and part counts (equal to the raw block's), the node links (all in
   range), the two compression flags (bit 0 ⇔ s16 positions, bit 1 ⇔ s16
   texcoords), `indices`, `node_map`, the triangle-list part flag, subparts,
   the classification labels, and the vertex types `OneNode` through
   `FourNodes`. Names in the shared decoder follow the same rule (see
   structure-bsp.md).
2. A field with no evidence for its meaning is called `unknown_<offset>`.

These names were deliberately not used, because nothing cited here supports
them:

- part kind names other than "skipped by engine" (0 and 5);
- classification names such as "rigid boned";
- node rest-frame and scale fields;
- the visibility block (`0x10`) and the other uninterpreted section blocks.

Node `+0x5c` is not the translation length (V: 1,614 nodes disagree); that
word is part of `unknown_028`.

### Records

| Record (stride) | Evidence | Notes |
|---|---|---|
| `ModeGeometryHeader` (`0x84`) | Sections at `+0x24`, stride `0x5c`: **E** (`0x41076`-`0x41086` read `[tag+0x28]` + index×`0x5c`). Compression infos `+0x14`, regions `+0x1c`, nodes `+0x48`, materials `+0x60`: **D** | Every block's count×stride fits the tag payload. |
| `ModeCompression` (`0x38`) | **D** | Exactly one per model. Holds position bounds (x/y/z min,max), then texcoord u/v, then a second texcoord pair. Decoded attributes stay inside the bounds: 3 of 8.4M are marginal outliers, which the threshold allows. |
| `ModeRegion` (`0x10`), `ModePermutation` (`0x10`) | **D** | Six i16 section indices per permutation, LOD 0 (lowest) to 5 (highest). Each is `-1` or in range. Triangle counts rise with the index on the models checked. The region's `+4/+6` are unknown. |
| `ModeSection` (`0x5c`) | Stride and `+0x34`: **E**. Counts, classification, flags: **D**. `+0x38`..`+0x5c`: the shared geometry block info (**E** `0x00223c70`) | `+0x30`/`+0x34` are the runtime section block (count, pointer); both are zero on disc. Vertex and part counts equal the raw block's. Classification 1/2/3 occurs only with no-node, one-node and weighted vertex types respectively. Compression bit 0 ⇔ s16 positions and bit 1 ⇔ s16 texcoords, on every section. The block info's owner is the tag's own ID and its runtime offset is `-8` (pointing at `+0x30`) on every section. |
| `ModeNode` (`0x60`) | **D** | Links are in range. The 14 words from `+0x28` are not interpreted. |
| `ModeMaterial` (`0x20`) | **D** | Two tag references. The first is always null. The second always resolves to a `shad` tag; some store group `0xffffffff` with a live ID, so only the ID is required to resolve. |

The raw block, resources, parts, subparts and vertex-buffer descriptors are
the shared layouts in [structure-bsp.md](structure-bsp.md). The pointer
selects the local, mainmenu, shared or single-player-shared map with the usual
top-two-bit rule; all four occur. `MODEL_SECTION`, the `0x70`-byte section
struct:

| Field | Element | Evidence |
|---|---|---|
| `0x00` block | Part, `0x48` | **E** (`[section_data+4]`, index×`0x48` at `0x410da`-`0x410e0`) |
| `0x08` block | Subpart, 8 | **D**: element size 8. Every subpart names its own part and each part's run tiles the block, as in structure sections. Strip parts keep empty placeholder subparts (first 0, count 0), so here a subpart need only be empty or inside its part. |
| `0x20` block | u16 index | **D** |
| `0x38` block | Vertex-buffer descriptor, `0x20` | **E** `0x00223ced` |
| `0x64` block | u8 node map | **D**: entries index the model's nodes |
| `0x10`, `0x44`, `0x54`, `0x5c` blocks | Element sizes `0x14`, `0x44`, 4, 2 | **D** (element size only); checked like any block resource, not interpreted |

Parts: kind 0 and 5 are skipped by the engine (**E** `0x410fc`-`0x41129`).
Part flag `0x8` marks a triangle **list**. Without it, the range is a triangle
**strip** with degenerate joins. With that rule, the triangle count derived
from the indices exactly equals the stored count on all 27,319 sections; the
same flag is set on every structure part, which are all lists. Part materials
are below the model's material count, or `0xffff`.

### Vertex-buffer types

These are **D**: inferred from stride, section classification, compression
flags and value checks. No engine table has been located yet.

| Type | Stride | Layout | Checks |
|---|---:|---|---|
| `0x01` | 12 | f32×3 position | Only in sections without position compression; must be finite. |
| `0x02` | 6 | s16×3 position | Classification 1; maps into the compression bounds. |
| `0x04` | 8 | s16×3 position, u8 node, u8 zero | Classification 2. |
| `0x06` | 12 | s16×3, 2 zero bytes, u8×2 nodes, u8×2 weights | Weights sum to 255. |
| `0x08` | 12 | s16×3, u8×3 nodes, u8×3 weights | Weights sum to 255. |
| `0x0a` | 16 | s16×3, 2 zero bytes, u8×4 nodes, u8×4 weights | Weights sum to 255. |
| `0x18` | 8 | f32×2 texcoord | Only without texcoord compression. |
| `0x19` | 4 | s16×2 texcoord | |
| `0x1b` | 12 | 3 × NORMPACKED3 (11:11:10, x in the low bits) | Word 0 is the normal: its mean dot with the face normal derived from the strip is +0.88 over 19.8M corners (words 1 and 2: about 0). 99.9% are unit length. |
| `0x2f`, `0x30` | 3, 4 | unknown | 129 sections; preserved as bytes. |

### Decoding rules

- Each section has exactly one position stream, and the shared decoder
  allows at most one stream per role (texcoord, tangent frame, and so on).
  Retail data always has a position, a texcoord and a tangent-frame stream.
- Compressed values are `s16 / 32767`, clamped to -1, then mapped to
  `(v+1)/2 × (max-min) + min`. Non-finite bounds or float attributes are
  rejected.
- Node indices index the section's node map when it has one, otherwise the
  model's nodes. 1,399 vertices in first-person models reference nodes beyond
  their own model's list, probably a skeleton shared at run time. These are
  reported, and the run fails only above 0.1%.
- Strip winding: even triangles are `(a,b,c)` and odd ones `(a,c,b)`. This
  agrees with the decoded normals.
- Vertices with node data are stored in model space at the rest pose. The
  viewer draws them without node transforms, and parts land in place.

### Limits

The shared decoder's limits apply (structure-bsp.md, "Decoder rules"): a raw
block is at most 16 MiB, resources are disjoint and uniquely keyed, decoded
tables borrow the one loaded block, tag-block reads are charged to a
payload-sized budget, and counts are only used once the bytes behind them
have been checked.

Not interpreted:

- shaders, which are references only;
- the second texcoord set;
- the unknown raw blocks and the two unknown vertex types;
- markers and the remaining header blocks.

The verifier checks structure. Beyond the shapes inspected, it does not prove
how models should render. Screenshots and validation output stay outside the
repository.
