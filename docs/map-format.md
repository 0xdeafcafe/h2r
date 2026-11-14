# Halo 2 retail Xbox cache files (version 8)

This describes the reader in `crates/map`, not an on-disk Rust struct ABI.
All integers below are little-endian. Offsets are hexadecimal and relative to
one `.map` file, including when the map is read inside an ISO.

Evidence labels:

- **V — verified-on-disc:** the stated structural property was checked against
  the owner's retail disc, with the public sources below as reference. This is
  evidence for that build, not a guarantee about every modded or regional map.
- **S — from-source:** public documentation/code describes this, but this work
  did not independently validate its semantics on the disc.
- **Design:** an implementation choice or recommendation, not a format fact.

No map bytes, tag dumps, tag-name lists, localized strings, or extracted assets
are included here or in tests. The tests construct invented maps in memory.

## Sources and reproducibility

Assembly is pinned to commit
[`fd14ef3c46135e9422966c6a64cd926fb0bd23fb`](https://github.com/XboxChaos/Assembly/tree/fd14ef3c46135e9422966c6a64cd926fb0bd23fb).
The following are links to that exact revision, not moving branch URLs:

- [Xbox core layouts][core]: header, index, groups, tags, BSP header, locale structures.
- [Xbox scenario layouts][scenario]: scenario BSP descriptors and their stride.
- [SecondGenCacheFile.cs][cache]: primary/secondary translation and BSP fixups.
- [SecondGenHeader.cs][header]: index versus tag-data regions and string tables.
- [SecondGenTagTable.cs][tags]: tag/group index interpretation and root datums.
- [SecondGenTag.cs][tag]: null addresses and metadata source distinctions.
- [Engines.xml][engines]: retail Xbox identification and Xbox-specific plugins
  with common Halo 2 plugins as fallback; Vista also uses version 8.
- [LengthBasedStringIDResolver.cs][stringids]: the string-ID encoding used by Assembly.
- [CacheFileType.cs][types]: numerical cache types.
- [Xbox bitmap layout][bitmap]: image block and raw LOD pointers.

The independent historical reference is Iron_Forge's *Halo 2 .map file format
whitepaper*, revision 0.9b, dated 2004-11-21, preserved at
[xboxdevwiki revision 5696][whitepaper]. It explains the two magic values,
name tables, and top-two-bit raw-resource selectors. Its unknown fields and
speculation are not promoted to verified facts. In particular its older
`tagIDStart` label is superseded here by the explicit globals datum in Assembly.
The wiki is pinned by revision ID, since it has no Git commit.

These sources were inspected for format facts. No community implementation
bodies or XML files were copied into this project.

## Header and compression

**V:** the header is `0x800` bytes. The numeric FourCCs `head` and `foot` appear
as bytes `daeh` at `0` and `toof` at `0x7fc`. Version is 8 and the supported build
is `02.09.27.09809`. This reader also checks the build: version alone cannot
separate Xbox retail from Vista. [Core layouts][core], [engine definitions][engines].

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
| `018` | u32 | Ordinary metadata byte length | V |
| `01c` | u32 | Total metadata memory size, including BSP space | S |
| `120` | char[32] | NUL-terminated build string | V |
| `140` | u32 | 0 campaign, 1 multiplayer, 2 menu, 3 shared, 4 campaign-shared | V |
| `144` | u32 | CRC checksum field (not the XOR field below) | S |
| `154` | u32 | Low-detail bitmap data count | S |
| `158` | u32 | Low-detail bitmap data offset | V |
| `15c` | u32 | Low-detail bitmap data size | V |
| `160` | u32 | Auxiliary fixed-width string slots offset, stride `0x80` | S |
| `164` | u32 | String-ID entry count | V |
| `168` | u32 | String-ID string blob length | V |
| `16c` | u32 | String-ID offset-array file offset | V |
| `170` | u32 | String-ID blob file offset | V |
| `178`, `17c` | u32 each | Cache build date halves | S |
| `198` | char[32] | NUL-terminated internal map name | V |
| `1bc` | char[256] | NUL-terminated scenario path | V |
| `2c0` | u32 | Tag-name entry count | V |
| `2c4` | u32 | Tag-name blob file offset | V |
| `2c8` | u32 | Tag-name blob byte length | V |
| `2cc` | u32 | Tag-name offset-array file offset | V |
| `2d0` | u32 | XOR checksum, not validated by this reader | S |
| `7fc` | FourCC | `foot` | V |

Sources for this table: [core layouts][core], [header interpretation][header],
[cache type enumeration][types]. The XOR algorithm (XOR u32 words after the
header) is **S**, from the [whitepaper][whitepaper]; checksum correctness and
runtime rejection behavior have not been tested. Other header fields remain
uninterpreted; they should not be assumed to be padding that a writer may erase.

## Strings, names and groups

**V:** both indexed string tables use a u32 offset array and a blob of
NUL-terminated strings. Each offset is relative to its own blob, not to the map
or a virtual base. Names are ordered by tag-index slot and contain full paths
without class extensions. Names can repeat across classes, so a name alone
is not a guaranteed unique key. The reader reports an ambiguous-name error
and offers `(name, group)` lookup. [Core layouts][core], [tag table][tags].

**S:** an offset of `0xffffffff` means no string; the API preserves it as
`None`. A completely absent name table is also supported for inspection of
stripped maps. These cases are covered synthetically, not evidenced by the
retail disc. Empty strings remain distinct from missing strings.

**S:** Assembly models a StringID as the low 24 bits selecting a string and the
high 8 bits carrying its length. The public API currently exposes indexed
strings rather than promising a complete StringID semantic validator.
[Length-based resolver][stringids]. Localized UI text is separate: language
information is in `matg`, with nine language records at `+0x190`, stride `0x1c`,
and `unic` selects ranges. Locale decoding is not implemented or verified.
[Core layouts][core].

**V:** classes are FourCCs stored as little-endian integers: e.g. `scnr` is bytes
`rncs` on disk. Preserve punctuation and spaces (`snd!`, `ugh!`), rather than
normalizing classes as identifiers. A group entry is 12 bytes: class, parent,
grandparent; absent ancestors use `0xffffffff`. The file supplies the hierarchy.
Examples of class meanings are scenario (`scnr`), globals (`matg`), bitmap
(`bitm`), scenario structure BSP (`sbsp`) and lightmap (`ltmp`).
[Core layouts][core], [tag table][tags].

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
[Core layouts][core], [tag table][tags].

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
[Cache loader][cache], [header interpretation][header].

**S:** null slots use an absent class/datum. Null addresses can be zero or
`0xffffffff`; they are not file offset zero. The parser preserves tags without
local data and returns `NoTagData` on payload reads. Missing first-slot metadata
anchors are rejected instead of guessing a magic from a later tag. Null slots,
stripped names, root lookups and stale handles are tested synthetically.
[Tag representation][tag].

## BSP and lightmap metadata

**V:** BSP/lightmap tag-index addresses are zero on this disc. The scenario's
block at `+0x210` (count) / `+0x214` (ordinary metadata pointer) supplies their
locations. Descriptor stride is `0x44`, with file offset at `+0`, size at `+4`,
virtual base at `+8`, BSP datum at `+0x14`, lightmap datum at `+0x1c`.
[Scenario layouts][scenario].

**V:** each BSP region starts with a 16-byte header: used size, BSP virtual
address, lightmap virtual address, and the `sbsp` signature. The BSP address
is base plus 16. When a lightmap exists, the interval from BSP address to
lightmap address is the BSP payload, and the remaining used bytes are the
lightmap payload. Each tag retains this region's address translation.
Different BSPs can reuse virtual memory, so ordinary magic or an address-only
global lookup is insufficient. [Core layouts][core], [BSP fixups][cache].

**V:** the shared resource caches have BSP data with a null lightmap datum and
zero lightmap address. In that case the BSP uses the remaining used region,
not `lightmap_address - bsp_address`. This corrects an assumption in Assembly's
size calculation for this special case. A synthetic regression covers it.
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

**V:** the [Xbox bitmap layout][bitmap] supplies an image block at tag `+0x44`,
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

let mut cache = Map::open("/path/to/your/mainmenu.map")?;
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
checksum enforcement, resource decoding, or arbitrary modded caches.

**Design:** tests are entirely synthetic. They cover ordinary and BSP address
spaces, absent lightmaps, sparse/null entries, duplicate names across groups,
stale salts, corrupted ranges/counts, malformed/unterminated strings, wrong
platform/version, truncated files, and container-boundary escapes. Eager table
buffers and total decoded string bytes per table are each capped at 64 MiB;
tag slots are capped at 65,536. Payload lengths are checked against their
region and allocated with fallible reservation. There is no cache writer yet.

## Rust tag definitions for an editable engine

**Design recommendation:** use a small, reviewed schema as the source of truth
and generate checked binary readers/writers and field metadata from it. Include
field name, offset, scalar/enum type, array/block stride, tag-reference class,
address kind, build applicability, and preserved unknown byte ranges. Generate
ordinary named Rust structs for understood values; use `TagId`, typed tag
references, string IDs, raw-resource pointers and `Vec<T>` blocks, not opaque
integer pointers in the editor model.

Use pinned community XMLs as research/import material, applying Xbox overrides
before common Halo 2 fallback definitions. Review both accuracy and licensing
before vendoring or generating distributed code/data from them: Assembly's
repository is GPL-licensed while this workspace declares MIT/Apache-2.0.
The current independent reader does not vendor those files. A mechanical bulk
import should not quietly become the project's permanent API.

Start with hand-reviewed `scnr`, `matg`, `bitm` and BSP records, then expand the
schema as needed. Hand-write semantic validation and editable domain models
above generated codecs. Keep padding/unknown fields for eventual round trips,
retain the provenance of each layout, and test codecs with synthetic fixtures.
Use private disc-backed checks locally for additional evidence.

Do not cast cache bytes into `#[repr(C)]` structs: disk offsets, alignment,
little-endian fields, salted references, and region-dependent pointers require
explicit decoding. `repr(C)` can be useful later at the guest-engine ABI boundary;
it is not the editable asset model. This keeps readable Rust names and useful
editor metadata without hand-maintaining hundreds of repetitive offset readers.

[core]: https://github.com/XboxChaos/Assembly/blob/fd14ef3c46135e9422966c6a64cd926fb0bd23fb/src/Blamite/Formats/Halo2Xbox/Layouts/H2X_Layouts_Core.xml
[scenario]: https://github.com/XboxChaos/Assembly/blob/fd14ef3c46135e9422966c6a64cd926fb0bd23fb/src/Blamite/Formats/Halo2Xbox/Layouts/H2X_Layouts_Scenario.xml
[cache]: https://github.com/XboxChaos/Assembly/blob/fd14ef3c46135e9422966c6a64cd926fb0bd23fb/src/Blamite/Blam/SecondGen/SecondGenCacheFile.cs
[header]: https://github.com/XboxChaos/Assembly/blob/fd14ef3c46135e9422966c6a64cd926fb0bd23fb/src/Blamite/Blam/SecondGen/Structures/SecondGenHeader.cs
[tags]: https://github.com/XboxChaos/Assembly/blob/fd14ef3c46135e9422966c6a64cd926fb0bd23fb/src/Blamite/Blam/SecondGen/Structures/SecondGenTagTable.cs
[tag]: https://github.com/XboxChaos/Assembly/blob/fd14ef3c46135e9422966c6a64cd926fb0bd23fb/src/Blamite/Blam/SecondGen/Structures/SecondGenTag.cs
[engines]: https://github.com/XboxChaos/Assembly/blob/fd14ef3c46135e9422966c6a64cd926fb0bd23fb/src/Blamite/Formats/Engines.xml
[stringids]: https://github.com/XboxChaos/Assembly/blob/fd14ef3c46135e9422966c6a64cd926fb0bd23fb/src/Blamite/Blam/LengthBasedStringIDResolver.cs
[types]: https://github.com/XboxChaos/Assembly/blob/fd14ef3c46135e9422966c6a64cd926fb0bd23fb/src/Blamite/Blam/CacheFileType.cs
[bitmap]: https://github.com/XboxChaos/Assembly/blob/fd14ef3c46135e9422966c6a64cd926fb0bd23fb/src/Assembly/Plugins/Halo2Xbox/bitm.xml
[whitepaper]: https://xboxdevwiki.net/index.php?title=Halo_2&oldid=5696


