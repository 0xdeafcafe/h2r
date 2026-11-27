# Halo 2 retail Xbox cache files (version 8)

Status: Built. Reader and lossless writer in `crates/formats/map`; tag codecs in `crates/formats/tags`. Switch: none.

This describes the reader and lossless writer in `crates/formats/map`, not an on-disk Rust struct ABI.
All integers below are little-endian. Offsets are hexadecimal and relative to
one `.map` file, including when the map is read inside an ISO.

Evidence labels:

- **V - verified-on-disc:** the stated structural property was checked against
  the owner's retail disc. This is evidence for that build, not a guarantee
  about every modded or regional map.
- **E - engine:** read from the retail XBE with `h2 disasm`.
- **S - unverified:** not independently validated on the disc or in the
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
qword parity seed and incomplete-tail behaviour. Other header fields remain
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
length ([localisation.md](localization.md)). The API exposes indexed strings
rather than promising a complete StringID semantic validator. Localized UI text is separate: language
information is in `matg`, with nine language records (descriptors start at `+0x188`; `+0x190` is the count field eight bytes in), stride `0x1c`,
and `unic` selects ranges. The later [localisation decoder and evidence](localization.md) covers all nine
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
are translated within the index region, except in maps the builder grew
([map-builder.md](map-builder.md), "Adding tags to a pinned map"): there
the entry table or the group table can sit at the end of the ordinary
metadata (base `index_virtual_base + +0x1c - +0x18`). When `T < G`, the
group table is the moved one and `index_virtual_base = T - 0x20`. `AddressSpace` stores a virtual
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

Moved to [Map format: record codecs and render-model geometry](map-format-records.md#generated-record-codecs).

### Vertex-buffer types

Moved to [map-format-records.md](map-format-records.md#vertex-buffer-types).

## Render-model geometry (`mode` sections, parts, vertices)

Moved to [Map format: record codecs and render-model geometry](map-format-records.md#render-model-geometry-mode-sections-parts-vertices).

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

There is no canonicalisation exception: unchanged writes are byte-identical,
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
patch bytes within that region in one transaction. `patch_bsp(space,
address, bytes)` uses a BSP region resolved from the source map. Its
file offset distinguishes BSP regions that reuse virtual addresses.
The writer checks the range and protects the region header.

`patch_raw(block, block_size, offset, bytes)` is the fixed-size patch for raw
data (section geometry, bitmap pixels): `bytes` at `offset` into the
`block_size` bytes a raw pointer names. The block must be in this file (a
pointer into `mainmenu.map` or a shared cache is refused), inside the file,
past the header, and clear of the ordinary metadata, every BSP region, the
index and the string and name tables. The Modeling tool's in-place writes use
it (`docs/editor/modeling-targets.md`).

### The tag arena

The loader `0x1228f0` takes a map's `+0x1c` from the low end of one physical
arena; the game carves its other state down from the high end, and after the
load `0x12c1e0` gives every page left between the two cursors to one pool
(`0x13d8b0`, a page cache with no fixed size). So tag growth and that pool
share the arena, and a bigger arena is more room for both. Our runtime grows
it by `map::arena::GROWTH` (8 MiB) at the high end, and guest RAM by the
same, so nothing a map names moves and what lies after the arena keeps
retail's room (E, `h2 disasm 0x12b400`, `0x12c1e0`, `0x13d8b0`).

| | Retail | Ours |
|---|---|---|
| Guest RAM | `0x4000000` (64 MiB) | `0x4800000` (72 MiB), `runtime::guest::RAM_SIZE` |
| Arena (physical) | `0x61000..0x3145000` | `0x61000..0x3945000`, `map::arena::ARENA` |
| Arena size | `0x30e4000` | `0x38e4000` |
| Contiguous window | `0x80061000..0x83145000` | `0x80061000..0x83945000` |
| Tag cache base | `0x80061000` | `0x80061000` (unchanged) |
| After the arena (image, stacks, kernel) | `0x3145000..0x4000000` | `0x3945000..0x4800000` |
| `max_metadata_size` | largest retail `+0x1c` of the type | that plus `0x800000` |

The end and the size occur in the image only as two immediates in
`0x12b400` (`mov eax,3145000h` at `0x12b41d`, `push 30E4000h` at
`0x12b423`); no other code names them, and no code masks addresses to
64 MiB (searched by immediate: no `and` with `0x3ffffff`, no compare with
`0x84000000`). The `tag-arena` feature (`crates/extensions/game/src/tag_arena.rs`)
makes the two patches at run time:

| Patch | Hook | Retail | Ours |
|---|---|---|---|
| `arena size` | `before` `XPhysicalAlloc` `0x2d156a`, the call with size `0x30e4000` at `0x61000` | `0x30e4000` | `0x38e4000` |
| `arena cursors` | `after` `0x12b400`: high bound `0x4e6428`, depth-0 high cursor `0x4e6440` | `0x3145000` | `0x3945000` |

The size patch goes through the kernel, so it maps and books every page of
the grown arena (XPhysicalProtect on a carved piece needs that). 8 MiB is
the margin: about 50 times what Waterworks' Firefight kit was over, and many
times the largest DLC map's excess over retail (Turf, about 380 KB), while
the extra guest RAM stays small for snapshots and forks.

**Test:** `crates/formats/map/src/arena.rs` (layout: page alignment, fixed
base, room after the arena), `crates/extensions/game/src/tag_arena.rs`
(only the retail request and cursors are patched).

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
the same type on the owner's disc (V, `writer::retail_max_metadata_size`):
campaign `0xc24000`, multiplayer `0x62c000`, main menu `0x1d3000`, shared
`0x445000`, campaign-shared `0x69e000`, plus the arena's growth
(`0x800000`, "The tag arena"), so the pool keeps at least retail's worst
case. [scenario-editor.md](../editor/scenario-editor.md) §6 has the details and
the open question.

### Inserting raw data

`append_raw(bytes, align)` (`crates/formats/map/src/writer.rs`) queues new
raw data and returns its raw pointer; the caller points a tag word at it.
The bytes go where the source's tag index starts, as the builder places
raw data ([map-builder.md](map-builder.md), "Layout"): on write the index
and the ordinary metadata follow them, `+0x10` and `+8` grow by the bytes
padded to `0x1000`, and every queued patch at or past the index moves with
it. Raw pointers, BSP descriptors and every virtual address stay valid,
because what they name lies before the index; the metadata still ends the
file, so `append_metadata` works in the same edit and on the result.
`Map::raw_append_refusal` names why a map cannot take it: the metadata
does not end the file, the file has bytes past its declared length, or a
BSP region or a string or name table lies past the index. A block must end
below 2^30 (the raw pointer's offset bits) and the file stay under the
per-type cache size limit.

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
