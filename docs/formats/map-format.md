# Halo 2 retail Xbox cache files (version 8)

Status: Built. Reader and lossless writer in `crates/formats/map`; tag codecs in `crates/formats/tags`. Switch: none.

This describes the reader and lossless writer in `crates/formats/map`, not an on-disk Rust struct ABI.
All integers below are little-endian. Offsets are hexadecimal and relative to
one `.map` file, including when the map is read inside an ISO.

Evidence labels:

- **V — verified-on-disc:** the stated structural property was checked against
  the owner's retail disc. This is evidence for that build, not a guarantee
  about every modded or regional map.
- **E — engine:** read from the retail XBE with `h2 disasm`.
- **S — unverified:** not independently validated on the disc or in the
  engine; kept only where the text needs it.
- **Design:** an implementation choice or recommendation, not a format fact.

No map bytes, tag dumps, tag-name lists, localized strings, or extracted assets
are included here or in tests. The tests construct invented maps in memory.

## Sources and reproducibility

The one outside reference is Iron_Forge's *Halo 2 .map file format
whitepaper*, revision 0.9b, dated 2004-11-21, preserved at
[xboxdevwiki revision 5696][whitepaper]. It explains the two magic values,
name tables, and top-two-bit raw-resource selectors. Its unknown fields and
speculation are not promoted to verified facts. The wiki is pinned by
revision ID, since it has no Git commit.

## Header and compression

**V:** the header is `0x800` bytes. The numeric FourCCs `head` and `foot` appear
as bytes `daeh` at `0` and `toof` at `0x7fc`. Version is 8 and the supported build
is `02.09.27.09809`. This reader also checks the build: version alone cannot
separate Xbox retail from Vista.

**V:** the retail maps checked here are directly seekable without whole-file
compression. Header offsets reach intelligible indexes, names and metadata in
the original disc bytes. All declared tag payloads, including BSP/lightmap
payloads, were readable without a decompression step. Do not apply Halo 1 Xbox
map decompression rules here. This does not mean individual texture, audio,
animation or model resources lack their own encodings/compression. Those
resource encodings are **S**, outside this reader's implementation.

| Offset | Type/size | Meaning | Evidence |
|---|---|---|---|
| `000` | FourCC | `head` | V |
| `004` | u32 | Cache version, 8 | V |
| `008` | u32 | Declared file length | V |
| `010` | u32 | Absolute file offset of index header (`I`) | V |
| `014` | u32 | Bytes from index header to ordinary metadata (`D`) | V |
| `018` | u32 | Ordinary metadata byte length | V, E `0x122a53` |
| `01c` | u32 | Total metadata memory size: index, largest BSP, ordinary metadata | V, E `0x122947` |
| `120` | char[32] | NUL-terminated build string | V |
| `140` | u32 (engine: i16, see caveats under "Lossless writing") | 0 campaign, 1 multiplayer, 2 menu, 3 shared, 4 campaign-shared | V |
| `158` | u32 | Low-detail bitmap data offset | V |
| `15c` | u32 | Low-detail bitmap data size | V |
| `160` | u32 | Auxiliary fixed-width string slots offset, stride `0x80` | S |
| `164` | u32 | String-ID entry count | V |
| `168` | u32 | String-ID string blob length | V |
| `16c` | u32 | String-ID offset-array file offset | V |
| `170` | u32 | String-ID blob file offset | V |
| `198` | char[32] | NUL-terminated internal map name | V |
| `1bc` | char[256] | NUL-terminated scenario path | V |
| `2c0` | u32 | Tag-name entry count | V |
| `2c4` | u32 | Tag-name blob file offset | V |
| `2c8` | u32 | Tag-name blob byte length | V |
| `2cc` | u32 | Tag-name offset-array file offset | V |
| `2d0` | u32 | Copy-time XOR checksum; explicit validation via `Map::validate_checksum` | E + V |
| `7fc` | FourCC | `foot` | V |

The checksum has executable and disc evidence: see "Lossless writing and bounded edits" below. The old
whitepaper description (XOR u32 words after the header) omits the routine's
qword parity seed and incomplete-tail behavior. Other header fields remain
uninterpreted; the writer preserves them, rather than treating them as padding.

## Strings, names and groups

**V:** both indexed string tables use a u32 offset array and a blob of
NUL-terminated strings. Each offset is relative to its own blob, not to the map
or a virtual base. Names are ordered by tag-index slot and contain full paths
without class extensions. Names can repeat across classes, so a name alone
is not a guaranteed unique key. The reader reports an ambiguous-name error
and offers `(name, group)` lookup.

**Design:** an offset of `0xffffffff` is treated as no string; the API preserves it as
`None`. A completely absent name table is also supported for inspection of
stripped maps. These cases are covered synthetically, not evidenced by the
retail disc. Empty strings remain distinct from missing strings.

**V:** a StringID's low 24 bits select a string and its high 8 bits carry its
length; every locale ID on the disc resolves to a name of exactly the encoded
length ([localization.md](localization.md)). The API exposes indexed strings
rather than promising a complete StringID semantic validator. Localized UI text is separate: language
information is in `matg`, with nine language records (descriptors start at `+0x188`; `+0x190` is the count field eight bytes in), stride `0x1c`,
and `unic` selects ranges. The later [localization decoder and evidence](localization.md) covers all nine
languages, compiled `unic` ranges, and the executable's wide-text conversion.

**V:** classes are FourCCs stored as little-endian integers: e.g. `scnr` is bytes
`rncs` on disk. Preserve punctuation and spaces (`snd!`, `ugh!`), rather than
normalizing classes as identifiers. A group entry is 12 bytes: class, parent,
grandparent; absent ancestors use `0xffffffff`. The file supplies the hierarchy.
Examples of class meanings are scenario (`scnr`), globals (`matg`), bitmap
(`bitm`), scenario structure BSP (`sbsp`) and lightmap (`ltmp`).

## Index, root tags and address translation

**V:** at header offset `I` there is a `0x20`-byte index header:

| Relative offset | Meaning |
|---|---|
| `00` | Virtual pointer to group table (`G`) |
| `04` | Group entry count |
| `08` | Virtual pointer to tag-entry table (`T`) |
| `0c` | Scenario's full salted datum |
| `10` | Globals' full salted datum |
| `14` | Uninterpreted u32 |
| `18` | Tag slot count |
| `1c` | Numeric `tags`, disk bytes `sgat` |

**V:** groups begin immediately after this header. A tag entry is 16 bytes:
class at `+0`, full datum at `+4`, virtual metadata address at `+8`, metadata
size at `+0xc`. Low 16 datum bits select the slot; upper 16 bits are its salt.
The reader compares the entire datum, so a stale reference does not select a
new tag in the same slot. Root datums resolve to their expected `scnr` and
`matg` classes. Neither root's slot nor its literal ID is hard-coded.

**V:** there are two ordinary translations, not one magic constant:

```text
index_virtual_base = G - 0x20
primary_magic      = index_virtual_base - I
entry_table_file    = T - primary_magic
metadata_file      = I + D
metadata_base      = first tag entry's metadata address
secondary_magic    = metadata_base - metadata_file
metadata_offset(p) = p - secondary_magic
```

The first tag anchors ordinary metadata. All its addresses and payload ranges
were checked against the header's declared metadata region. Index pointers
are translated only within the index region. `AddressSpace` stores a virtual
base, file start and size, avoiding accidental signed/wrapping subtraction.

On retail maps the ordinary metadata ends the file exactly
(`I + index size + D = file size`). Some community-modded maps (metadata
injected by a modding tool, as in the Phantom Mappack's) grow `+0x18` and
`+0x1c` by more than the bytes the tool wrote, so the declared region runs a
few pages past the end of the file while every tag still lies inside it. The
reader keeps the region it can read, cut at the declared file end
(`Map::metadata_size`), and checks every tag against that; the writer and the
builder's decomposition grow and rebuild from it, so an edited copy's header
is consistent again.

**S:** null slots use an absent class/datum. Null addresses can be zero or
`0xffffffff`; they are not file offset zero. The parser preserves tags without
local data and returns `NoTagData` on payload reads. Missing first-slot metadata
anchors are rejected instead of guessing a magic from a later tag. Null slots,
stripped names, root lookups and stale handles are tested synthetically.

## BSP and lightmap metadata

**V:** BSP/lightmap tag-index addresses are zero on this disc. The scenario's
block at `+0x210` (count) / `+0x214` (ordinary metadata pointer) supplies their
locations. Descriptor stride is `0x44`, with file offset at `+0`, size at `+4`,
virtual base at `+8`, BSP datum at `+0x14`, lightmap datum at `+0x1c`.

**V:** each BSP region starts with a 16-byte header: used size, BSP virtual
address, lightmap virtual address, and the `sbsp` signature. The BSP address
is base plus 16. When a lightmap exists, the interval from BSP address to
lightmap address is the BSP payload, and the remaining used bytes are the
lightmap payload. Each tag retains this region's address translation.
Different BSPs can reuse virtual memory, so ordinary magic or an address-only
global lookup is insufficient.

**V:** the shared resource caches have BSP data with a null lightmap datum and
zero lightmap address. In that case the BSP uses the remaining used region,
not `lightmap_address - bsp_address`. A synthetic regression covers it.
An absent descriptor file offset is skipped (**S**, synthetic coverage only).

## Raw/shared resources

**S**, independently range-checked for bitmap LOD data (**V**): raw pointers
are file selectors, not tag-memory pointers. Decode the high two bits and
mask with `0x3fffffff`; do not subtract either magic.

| High bits | File |
|---|---|
| `00` | Current map |
| `01` | `mainmenu.map` |
| `10` | `shared.map` |
| `11` | `single_player_shared.map` |

The selector rule comes from the [whitepaper][whitepaper]. `RawPointer::decode`
preserves file identity and offset and treats `0xffffffff` as absent. It does
not silently open files, relocate the pointer, or decode resource contents.

**V:** a bitmap tag has an image block at `+0x44`,
stride `0x74`; its first three LOD pointers at image `+0x1c/+0x20/+0x24` pair with
sizes at `+0x34/+0x38/+0x3c`. Every nonempty, non-null range found by that walk
fit the selected disc file, with all four selectors observed. This verifies
pointer routing/ranges, not rendered pixels, codec correctness, or sound/model
resource layouts. Those remain **S** or unimplemented.

## API, validation and limits

**Design:** `Map::open(path)` and `Map::from_reader(reader)` read standalone
maps; `Map::from_region(reader, base, length)` reads ISO entries without copying
raw map data. The latter takes bounds from `xiso` and never reads beyond the
map's container region or declared size. Index/string tables are loaded once;
payload reads are lazy. Errors use `thiserror`; unsafe code is forbidden.

```rust
use map::{Map, TagQuery, FourCC};

let mut cache = Map::open("/path/to/your/<map>.map")?;
for tag in cache.tags() {
    println!("{} {} {:?}", tag.id, tag.group, tag.name);
}
if let Some(tag) = cache.scenario() {
    let id = tag.id;
    let bytes = cache.read_tag_bytes(id)?;
    // Decode fields explicitly, using tag.location.space for pointers.
}
// Qualified name lookup is useful when multiple classes share a path:
let query = TagQuery::NameInGroup("example\\asset", FourCC(*b"bitm"));
let _ = cache.tag(query);
# Ok::<(), map::Error>(())
```

**Design:** `h2 maps [disc.iso]` lists headers and counts for disc maps.
`h2 tags <map-name-or-file> [group] [--disc disc.iso]` lists tag IDs, classes,
resolved file offsets, payload lengths and names, followed by whole-map group
counts. Without an explicit disc, these use the existing `H2_DISC`/`disc/`
selection. A local map path does not need a disc. Group filters use the printed
FourCC order. Neither command writes game data.

**V:** validation opened every map on the supplied disc, parsed both indexed
string tables and the class hierarchy, resolved scenario/globals and BSPs,
and read every tag's resolved payload. Standalone opening was also exercised
using a map extracted only into session `$TMPDIR`. Per-map counts and detailed
validation output stay outside the repository. This does not test gameplay,
in-engine checksum rejection, resource decoding, or arbitrary modded caches.
The writer checks the recovered checksum independently, as described below.

**Design:** tests are entirely synthetic. They cover ordinary and BSP address
spaces, absent lightmaps, sparse/null entries, duplicate names across groups,
stale salts, corrupted ranges/counts, malformed/unterminated strings, wrong
platform/version, truncated files, and container-boundary escapes. Eager table
buffers and total decoded string bytes per table are each capped at 64 MiB;
tag slots are capped at 65,536. Payload lengths are checked against their
region and allocated with fallible reservation. The lossless writer is described below.

## Tag definitions

The typed per-group definitions (every field named or an explicit `unknown`
span) are in [tag-definitions.md](tag-definitions.md); the layout inference
that finds pointers, references and string IDs is in
[tag-schema.md](tag-schema.md). The generated codecs for the records below
are in `crates/formats/tags/schema.rs` and `crates/formats/tags/build.rs`. Cache bytes are
never cast into `#[repr(C)]` structs: disk offsets, alignment, little-endian
fields, salted references and region-dependent pointers need explicit
decoding.

[whitepaper]: https://xboxdevwiki.net/index.php?title=Halo_2&oldid=5696


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
`i = 0..8` (E `0x1a01c3`, [localization.md](localization.md)). `+0x190` is the
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

The [lighting decoder](lighting.md) additionally verifies three sky-child
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

`validate_resources(&ResourceSizes)` additionally requires lengths for every
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
cargo h2 tag <map> '<folder>\<subfolder>\<tag-name>' scnr --disc <disc.iso>
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

To display a model, run `cargo run -p viewer -- <map> <tag>`. To decode every
`mode` tag on the disc, run `cargo run -p viewer -- --all all`. The `--all`
run exits nonzero if any of these happen:

- a map fails to open, or a tag fails to decode;
- any zero-tolerance check fails (triangle counts, tiling, weights, material
  references);
- any thresholded counter exceeds its limit in `viewer/src/validate.rs`.

### Evidence and names

Evidence labels in this section:

- **E**: the engine's own code reads the field. Addresses are in the retail XBE
  and can be checked with `cargo h2 disasm`.
- **D**: disc evidence, checked on all 4,222 `mode` tags in all 30 maps by
  `viewer --all`.

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



## Lossless writing and bounded edits

**Design:** `Map::write_to(&mut impl Write)` streams the entire bounded input,
including header, index and null slots, both indexed string tables, auxiliary
fixed strings, BSPs, local raw/resource bytes, unknown sections, internal gaps,
and physical trailing padding. It does not decode/re-encode opaque data or
repair an unedited checksum. `Map::from_region` retains the supplied physical
length separately from the declared header length: normal payload reads still
stop at the declared length; writing and copy-time checksumming stop at the
physical length. Neither operation reads the next ISO entry. The caller must
supply the actual file/entry length, not an entire remaining disc region.
Only a 1 MiB streaming buffer is needed in addition to reader tables.

The writer is a pass-through plus bounded patches, not a rebuild: it copies the
source and applies only queued patches, appended name tables and the header
fields listed below. Any "byte-identical round trip" claim in this document means
this pass-through of an unedited map, not a re-encode of decoded structures.

There is no canonicalization exception: unchanged writes are byte-identical,
including invalid recorded checksum values if the caller opened a damaged map.
Call `validate_checksum()` explicitly when integrity is required.

**Caveats.** (1) How the engine uses the tag-name table at runtime is unverified
(finding 4): the writer keeps it consistent with this crate's reader, and nothing
here shows the retail engine reads or needs it. (2) The engine reads the cache
type at header `+0x140` as an `i16` (`0x1228a3`), but this crate reads it as a
`u32`; they agree for the small values retail maps use. (3) The editor rejects
sources whose string or name tables sit inside the first `0x800` header bytes
(the header is rewritten), and patches that overlap the tag index or any
string/name table, because either would yield output the reader rejects.

### Checksum evidence from the retail executable

**E** here means independently inspected with this repository's `h2 disasm`
on the owner's read-only `disc/default.xbe` (SHA-256
`03215919bb7163259257d361f4c7bf802a7ab12aa85e2689436369b5c427935d`).
No community checksum code or map bytes are included in the implementation
or fixtures.

- `0x123110` is the checksum function. `0x123137`–`0x12313a` loads the byte
  length and shifts right by three. It processes complete 8-byte units only;
  a final 0–7 bytes is ignored, not padded or handled as one more dword.
- `0x123117`–`0x123133` constructs seed bytes `01 02 04 08 10 20 40 80`.
  `0x123143`–`0x12314b` selects that seed if the qword count is odd, otherwise
  zero. `0x123160` XORs each qword; `0x123168`–`0x123171` folds the two dwords.
  Equivalently, XOR each qword's two little-endian dwords and `0x88442211`.
  For a body divisible by 16, the seed cancels, leaving the familiar u32 XOR.
- The copy state machine starts at `0x2141f0`. It reads the header to
  `0x55b4f0` at `0x214312`–`0x214322`. It obtains the physical source length
  through the call at `0x2143ff` and stores it at `0x55bcfc` (the queued
  callback `0x1a15d0` calls the file-size routine `0x2d2715`). It clears the
  checksum accumulator at `0x214444` and starts body copying at offset `0x800`
  at `0x214524`. `0x21455b`–`0x214572` clips each copy to physical EOF.
- `0x2145dd` calls `0x123110` on the bytes actually copied, and `0x2145e8`
  XORs the result into the accumulator. `0x214616` compares the final value
  against `0x55b7c0`, i.e. header base `0x55b4f0 + 0x2d0`, and `0x21461c`
  sets the success flag from equality. Thus physical trailing storage is
  included; the header and its checksum field are excluded.

`checksum::engine_xor` exposes the exact buffer routine.
`checksum::XorChecksum` permits arbitrary host chunk boundaries, buffering a
partial qword until the next call. `Map::computed_checksum` covers the physical
body; `Map::validate_checksum` compares it with the recorded value. Host
streaming equals the engine's XOR of copy chunks when its intermediate chunks
are qword-aligned. The six copy-buffer sizes at `0x448ff0`, selected at
`0x214545`, are
`0x40000` and five `0x100000` entries, all divisible by eight. All tested files
are sector-aligned. Synthetic tests distinguish the odd-qword seed, ignored tails,
and a changed physical tail from the simpler folklore algorithm.

### Header signatures and acceptance boundaries

**E:** `0x122870`, called during the copy at `0x21437c`, checks numeric `head`
and `foot` at `+0` / `+0x7fc` (`0x122880`, `0x122888`), nonnegative declared
size bounded by `0x20800000` (`0x122894`–`0x1228a1`), cache type 0–4 and its
size limit (`0x1228a3`–`0x1228bf`), internal name length below 32
(`0x1228c1`–`0x1228dc`), and version 8 (`0x1228de`). The size-limit switch at
`0x214ed0` allows campaign `0x11800000`, multiplayer/menu `0x05000000`, shared
`0x0b400000`, and campaign-shared `0x20800000` bytes. Edits enforce these limits.
The ordinary load path at `0x122934` calls `0x12130` on header `+0x120`, comparing
its build string against the accepted build table.

The copy also compares three present dependency identities (8-byte values at
header `+0x180`, `+0x188`, `+0x190`, gated by bytes `+0x174`–`+0x176`) with
loaded-cache identities at `0x214390`–`0x2143d1`. The writer preserves those,
the build string, the `+0x178` build identity, the `+0x144` field, and all
uninterpreted header bytes. No cryptographic map signing is implemented or
claimed. These inspected paths establish structural/signature markers and XOR
validation, not a proof that every engine/network validation path accepts a
mod. Renaming or patching shared caches does not automatically update dependent
maps; this API leaves build identities intact. Resource decoding and gameplay
validation remain separate work.

### API and relocation policy

```rust
let mut cache = map::Map::open(input)?;
cache.validate_checksum()?;
let id = cache.tag(map::TagQuery::NameInGroup(name, map::FourCC(*b"weap")))?.id;
let mut edit = cache.edit();
tags::edit::set_object_bounding_radius(&mut edit, id, 1.25)?;
edit.rename_tag(id, "mods\\example_weapon")?;
edit.write_to(&mut destination)?;
```

`MapEditor::patch_tag(id, relative_offset, bytes)` replaces a bounded fixed-size
range, checks the full datum salt, uses the tag's own ordinary/BSP address space,
and rejects overlapping patches. `read_tag_range` and `source` inspect the
original, not queued edits. Field semantics belong to the caller; the low-level
patch API does not validate arbitrary block pointers or references.
`tags::edit::set_object_bounding_radius` supplies one reviewed scalar operation:
object prefix `+4`, verified by `0x000be075`, finite and nonnegative new values,
and a known object class. Unknown bytes are retained.

`rename_tag` rebuilds the complete tag-name blob and offset array, including
names on null slots, and writes them at sector-aligned EOF. It updates header
`+8`, `+0x2c0`, `+0x2c4`, `+0x2c8`, `+0x2cc`, pads the new end to 512 bytes,
and refreshes `+0x2d0`. Old table bytes remain in place; nothing else shifts.
Missing names (`0xffffffff`), empty names, duplicate names, and stripped tables
are handled separately. Renaming does not change a tag's datum or resource
references. String IDs and fixed string slots are preserved exactly; changing
StringID lengths would require repairing references in opaque tags.

`patch_metadata(address, bytes)` is the same fixed-size patch addressed by
ordinary metadata virtual address instead of a tag and offset, so a caller can
patch bytes of any tag, or between tags, in one transaction.

### Growing the metadata

`append_metadata(bytes, align)` appends bytes after the last tag and returns
their virtual address; the caller then points a block at them with
`patch_metadata`. No tag owns the new bytes, and nothing that exists moves.

**V:** on every retail map on the owner's disc the ordinary metadata is the
last thing in the file (`+0x10 + +0x14 + +0x18 == +0x8`), the index loads at
virtual `0x80061000`, BSPs at `0x80061000 + D`, and the metadata at
`0x80061000 + +0x1c - +0x18`, with `+0x1c = D + largest BSP + +0x18`. Every
size is a multiple of `0x1000`.

**E:** the loader `0x1228f0` allocates `+0x1c` rounded up to a page from the
low end of the physical arena (`0x12296c` fails when that would cross the
high cursor), reads `roundup(+0x14, 0x200)` bytes of index from `+0x10` to the
base (`0x12299f`), and `roundup(+0x18, 0x200)` bytes of metadata from
`+0x10 + +0x14` to `base + +0x1c - +0x18` (`0x122a52`–`0x122a87`, using the
unrounded `+0x1c`). The read helper `0x122d60` fails on a short read. Only the
loader reads `+0x18`; `+0x1c` is also read by three "inside the tag cache"
range checks (`0x1d02d8`, `0x1d5059`, `0x279745`), which use the same value.

So growing `+0x18` and `+0x1c` by the same amount leaves the metadata base,
and with it every tag address, unchanged. On write the appended bytes are
padded with zeros to a multiple of `0x1000`, and `+8`, `+0x18` and `+0x1c`
grow by that; a renamed name table goes after them. `append_metadata` refuses
when the metadata does not end the file, when the file has physical bytes
past its declared length, when a BSP's address space would overlap the new
range, when the file would pass the per-type cache size limit, and when
`+0x1c` would pass `writer::max_metadata_size(cache_type)`.

That last bound is empirical. No constant in the loader limits `+0x1c`; the
real limit is the arena: after the load `0x12c1e0` gives every free page to
one pool, so growth shrinks that pool, and how much the high side has used by
then was not measured. The bound is the largest `+0x1c` of any retail map of
the same type on the owner's disc (V): campaign `0xc24000`, multiplayer
`0x62c000`, main menu `0x1d3000`, shared `0x445000`, campaign-shared
`0x69e000`. [scenario-editor.md](../editor/scenario-editor.md) §6 has the details and
the open question.

To write a whole map from a set of tags, with every layout-dependent field
recorded as a fixup, use the builder in [map-builder.md](map-builder.md)
instead.

This minimal relocation policy deliberately does not resize metadata blocks
in place, repack raw resources, compact old tables, or rewrite unknown pointer
fields; a grown block is a new copy at the end, pointed at by its caller.
Those operations need a complete relocation schema. The tag index and all
metadata/raw offsets remain valid because their original regions do not move.
The renamed table is bounded to the reader's 64 MiB string-table limit.

All writer APIs accept streams rather than opening output paths. The source
must remain unchanged throughout an edit/write and must not alias the output.
`Map::write_to` needs `Write`; edited output needs `Write + Seek` to backpatch the
checksum, starts at the current position, and leaves the cursor at the end.
The caller supplies an empty/truncated destination region, handles atomic
replacement, and flushes/syncs as needed. The demo uses `create_new` under
`$TMPDIR` and cannot overwrite its input. Tests never write under `disc/`.

### Reproduction

Run the public synthetic suite normally. The retail integration test is
`#[ignore]`-gated and requires an explicit `H2_DISC` ISO path, as existing disc
checks do. It uses `xiso::Xiso` and `Map::from_region`, writes one temporary map
at a time, byte-compares the full output with the corresponding ISO entry,
re-reads it, and verifies its checksum. On maps with a weapon it also patches
the radius, rebuilds the name offsets, re-reads both changes and checks the new
checksum. Temporary retail maps are removed after each successful case. No map
bytes or game-derived fixtures are committed.

```sh
H2_DISC=/path/to/halo2.iso CARGO_TARGET_DIR="$TMPDIR/map-write-target" cargo test -p tags --test map_write -- --ignored --nocapture
```

The demo accepts a standalone map or an ISO plus entry path. It retains its
output in `$TMPDIR`, prints its path, and checks the result after closing the
writer:

```sh
CARGO_TARGET_DIR="$TMPDIR/map-write-target" cargo run -p tags --example edit_map -- /path/to/input.map
```

Or run directly against the read-only ISO without extracting an input map:

```sh
CARGO_TARGET_DIR="$TMPDIR/map-write-target" cargo run -p tags --example edit_map -- /path/to/halo2.iso /maps/beavercreek.map
```

### Retail results

All 30 maps in the supplied retail ISO round-tripped **byte-identically**, with
no semantic-only exceptions. All recorded and recomputed checksums matched.
Every map also passed a weapon-radius patch plus tag-name relocation, followed
by re-reading the new field/name and validating its recomputed checksum.
Physical and declared lengths matched on this disc; synthetic tests separately
cover trailing physical padding and embedded-region boundaries.

The result is per map (exact round trip, checksum, edited and re-read); the
verifier prints the table for the owner's disc.

The standalone `edit_map` demo was also run directly on `/maps/beavercreek.map`:
a weapon's bounding radius field was changed, tag-name offsets were rebuilt,
and the saved map re-opened with a matching checksum. Its output lives only
under session `$TMPDIR`.

Validation: `cargo test -p map -p tags` (synthetic tests; the retail tests are
ignored by default); the explicit retail test above (30 maps / 30 edits);
`cargo clippy -p map -p tags --no-deps --all-targets -- -D warnings`; and
`cargo check --workspace --all-targets`. Commands use
`CARGO_TARGET_DIR=$TMPDIR/map-write-target`, with the configured rustc wrapper left on.
These are file/structure/checksum checks, not an in-game playthrough.
