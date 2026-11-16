# Building whole maps (`map::build`)

Status: **Built** (2026-11-16) for the cache layout, string IDs, tag
names, tag index, metadata with fixups, raw resources, BSP regions and
the checksum. Proven by synthetic tests and by retail round trips on all
30 maps of the owner's disc. Nothing here has been loaded by the game yet
(see "Not verified").

`MapEditor` ([map-format.md](map-format.md), "Lossless writing") patches
an existing map in place. The builder in `crates/map/src/build.rs` writes
a whole v8 cache from a `TagSet` instead. The Halo 1 campaign converter
([campaigns.md](../game/campaigns.md) on the `campaigns` branch) and maps
made from scratch ([maps.md](../game/maps.md) §4) both need it.

Evidence labels are those of [map-format.md](map-format.md): **V** checked
on the owner's disc, **Design** an implementation choice.

## The problem: absolute addresses

v8 metadata holds absolute values that depend on layout:

- tag-block and data-reference pointers (virtual addresses in the
  ordinary metadata region, or in a BSP's own region);
- tag references and datums (salted IDs from the tag index);
- string IDs (index into the string table, plus the string's length);
- raw-resource pointers (file offsets with a file selector in the top two
  bits);
- the scenario's BSP descriptors (file offset, size and virtual base of
  each BSP region).

A tag laid out at a new address only works if every one of these inside it
is known. `crates/tags` knows a few layouts completely (bitmaps); most
retail tags still have opaque spans that may hold pointers. So a tag set
cannot assume every tag is relocatable.

## The model (Design)

```rust
use map::build::{Block, RawRef, TagPayload, TagSet};
use map::{CacheType, FourCC};

let mut set = TagSet::new(CacheType::Campaign, "a30", "levels\\a30\\a30");
set.groups = retail_groups; // copy the group table from a retail map
let globals = set.add_tag(FourCC(*b"matg"), "globals\\globals", TagPayload::Built(globals_block))?;
let pixels = set.add_raw(bytes, 0x200);
let mut image = Block::zeroed(0x74);
image.raw(0x1c, RawRef::Local(pixels));
let mut bitmap = Block::zeroed(0x4c);
bitmap.block(0x44, 1, image);
let id = set.add_tag(FourCC(*b"bitm"), "levels\\a30\\bitmaps\\x", TagPayload::Built(bitmap))?;
set.globals = globals;
let layout = set.write_to(&mut out)?;
```

A `TagSet` holds everything a cache needs: cache type, map name, scenario
path, the group table, the string-ID table, the tag-index slots (each a
name and an optional tag), the scenario and globals datums, raw resources
and BSP sections. Each tag's payload is one of:

| Payload | Meaning |
|---|---|
| `Built(Block)` | A tree of blocks the builder lays out. Any address. |
| `Pinned { address, bytes }` | Opaque bytes that must stay at one ordinary-metadata address, because they may hold pointers nobody has described. |
| `Empty` | No ordinary metadata; index address and size are zero. Retail BSP and lightmap tags, whose bytes sit in a BSP region. |

A `Block` is bytes, an alignment (default 4) and a list of fixups. A fixup
names an offset in the block and what goes there:

| Fixup | Width | Written value |
|---|---|---|
| `Block { count, child }` | 8 | count, then the child's address (0 when empty) |
| `Data(child)` | 8 | the child's byte length, then its address |
| `Pointer(child)` | 4 | the child's address |
| `TagRef(id)` | 8 | the target's group, then its datum; null is `ffffffff ffffffff` |
| `TagId(id)` | 4 | the datum |
| `StringId(s)` | 4 | the string's ID, interning it if new |
| `Raw(RawRef)` | 4 | `Local(raw)` the placed file offset; `External { file, offset }` the offset with the selector of `mainmenu.map`, `shared.map` or `single_player_shared.map`; `Null` is `ffffffff` |
| `Bsp { section, field }` | 4 | a built BSP region's file offset, size or virtual base |

Children are laid out after their parent, depth first, each at its own
alignment, so a tag's tree is contiguous and the index size covers all of
it. The builder rejects fixups that overlap or run past their block, a
block count that does not divide its child's bytes, and references to
datums not in the set. Bytes without a fixup are copied unchanged, so a
converter writes scalars directly and leaves only layout-dependent fields
to fixups. `TagSet::add_tag` gives slot `n` the datum
`(salt_base + n) << 16 | n`.

Retail tags enter the set through `TagSet::from_map`, which pins every tag
at its original address and keeps everything else that pinned bytes may
depend on (next section). A tag whose layout is fully known can then be
turned into a `Built` tree and moves: `tags::rebuild::relocate_bitmap`
does this for bitmaps. It declines (returns `None`, leaving the tag
pinned) unless the header, sequences, sprites and images cover the tag's
payload exactly once, so no unknown pointer can be left behind. It can
also copy each image's first three local LODs into new raw resources.

## Layout

**V** (all 30 retail maps): the file is the header, then raw data, BSP
regions, the 128-byte string slots, the string-ID offsets and blob, the
tag-name blob and offsets, low-detail bitmaps and more raw data, then the
tag index, then the ordinary metadata, which ends the file. Tables are
`0x200`-aligned. The index loads at `0x80061000`; BSP regions load at
`0x80061000 + D` (header `+0x14`); the metadata at
`0x80061000 + +0x1c - +0x18`. The tag-entry table follows the group
table directly (`0x80061020 + 12 × groups`). The index region `D` is much
larger than its header, groups and entries: lockout's is `0x4a000`, of
which about `0x38000` bytes after the entries are nonzero and
uninterpreted.

The builder writes, in order:

1. the header (`0x800` bytes);
2. the pinned file prefix, if any (below);
3. local raw resources, each at its alignment and at least `0x200`;
   offsets must stay below 2^30;
4. built BSP regions, page aligned: a 16-byte header (used size, BSP
   address, lightmap address, `sbsp`), the BSP tree at base + 16, the
   lightmap tree straight after it (the reader sizes the BSP as lightmap
   address minus BSP address, so no padding may sit between), padded to a
   page. Every region uses the same virtual base, `0x80061000 + D`;
5. the 128-byte string slots, string offsets, string blob, name blob and
   name offsets, each `0x200`-aligned;
6. the tag index, `0x200`-aligned: header, groups, entries, padded to a
   page. Built BSP and lightmap tags get index address and size zero, as
   on retail maps; the reader finds them through the scenario's descriptor
   block, which the author fills with `Bsp` and `TagId` fixups;
7. the ordinary metadata, padded to a page.

The metadata base is `0x80061000 + D + largest BSP region`; header `+0x1c`
is `base - 0x80061000 + metadata size`. Slot 0 must be an ordinary
metadata tag: the reader anchors the region on slot 0's address, and the
builder puts it first. Built tags follow in slot order.

The header gets the signatures, version 8, the build string, the cache
type, every size and table offset above, the map name (under 32 bytes)
and scenario path (under 256), and the XOR checksum over everything after
the header. Every other header byte comes from the pinned header, or is
zero for a map built from scratch.

**Limits** (Design, from the engine checks in map-format.md): the file
must stay under the per-type cache size limit, and `+0x1c` under
`max_metadata_size(cache_type)` (the largest retail map of the type).
Slots are capped at `0xffff`. A string ID must be under 128 bytes (its
fixed slot) and its index under 2^24.

### String IDs

An ID is `index | length << 24`. Existing table entries keep their index,
so pinned bytes stay valid; strings named by `StringId` fixups that are
not in the table are appended in slot order, depth first, so output is
deterministic. The builder writes the offset array, the blob and the
fixed 128-byte slots (header `+0x160`) from that one list. **V:**
regenerating all three from the string list reproduces the retail bytes
on every map.

### Pinned maps (`TagSet::from_map`)

To keep opaque bytes valid, a decomposed map keeps:

- the header, as the template for fields the builder does not own;
- the whole file from `0x800` to the tag index: raw data, BSP regions,
  low-detail bitmaps, locale text and the old tables. File offsets inside
  pinned tags stay right because this prefix does not move;
- the whole index region, including the uninterpreted tail. The builder
  rewrites only the header, groups and entries, and refuses to grow them
  past the original entries;
- `D`, the largest BSP region and the metadata base and size, so BSP and
  metadata addresses do not move;
- nonzero metadata bytes that no tag owns (one range per retail map).

A rebuilt table that fits in its old place is written there (the rest
zeroed); one that grew goes after the prefix. Built tags are placed
first-fit into metadata no pinned bytes use (ranges of tags that became
built, padding, slack up to the old size), then after it. With
`reuse_free_space = false` every built tag goes after the old region.
`reverse_built = true` places built tags in reverse slot order (used to
move retail tags away from their old addresses while reusing freed space).
`check_tag_memory = false` skips the tag-memory bound below; only for
offline round trips of maps already at the bound.

## What is verified

Synthetic tests (`cargo test -p map build`, `cargo test -p tags rebuild`):

- a map built from scratch reads back through `map::Map`: header fields,
  retail layout rules, groups and parents, names (including a named null
  slot), string IDs and fixed slots, a nested block, a data reference, tag
  references and datums, local/external/null raw pointers, alignment, an
  empty block, a BSP with a lightmap resolved through the scenario's
  descriptor, and a valid checksum;
- builds are deterministic, and an all-pinned decomposition of a built
  map rebuilds it byte for byte;
- a pinned scenario replaced by a built one moves, gains a string and a
  raw resource, and the pinned BSP still resolves through the moved
  descriptor;
- rejection of undeclared groups, dangling references, out-of-range and
  overlapping fixups, long strings, mismatched block counts, wrong root
  classes, long names, 30-bit raw overflow, a BSP-only slot 0, and
  metadata over the tag-memory bound;
- a bitmap becomes a tree that moves with its pixels and decodes the
  same; a bitmap with uncovered bytes stays pinned.

Retail round trips (`#[ignore]`, in memory, nothing written to disk):

```sh
H2_DISC=disc/halo2.iso H2_BUILD_MAPS=all cargo test --release -p tags --test map_build -- --ignored --nocapture
```

`H2_BUILD_MAPS` takes comma-separated map names; the default is
`lockout,01a_tutorial`. Results on 2026-11-16, **V**:

- **All 30 maps**, decomposed with every tag pinned and rebuilt, are
  **byte-identical** to the disc: header, prefix, regenerated string and
  name tables, index and metadata.
- **All 30 maps**, with every bitmap (25,214 in total, none declined)
  rebuilt as a block tree and laid back first-fit with pixels in place,
  are also byte-identical. Every bitmap's pointers went through fixups.
- **18 maps** (8 campaign, 10 multiplayer) also pass with every bitmap
  appended at a new address and every local LOD's pixels copied to new
  raw resources: same tags, names, groups, roots and strings; every pinned
  tag byte-identical at its address; every tag of the five groups
  `crates/tags` decodes (`bitm`, `scnr`, `matg`, `sbsp`, `mode`) decodes
  to the same fields, bitmaps apart from block addresses and moved LOD
  pointers, whose pixel bytes match. The other 12 (`01b`, `03b`, `04b`,
  `06a`, `06b`, `07a`, `08b`, `mainmenu`, `shared`,
  `single_player_shared`, `waterworks`, `zanzibar`) exceed the
  tag-memory bound when bitmaps are appended, and some the file-size
  limit when pixels are copied; they pass with freed space reused.

Other disc facts the test prints: no retail tag lacks local data (every
tag a map uses has metadata in that map; shared maps are only reached
through raw pointers); retail salts start at `0xe174` for slot 0 and grow
with the slot but skip values (lockout slot 3 is `0xe178`).

## Load tests (`h2 map testbuild`)

Two maps for the game to load, through the `H2_MAP_DIR` override
([storage.md](../runtime/storage.md), "Map overrides"). Each goes in its
own folder under the disc's file name, so the override serves it for that
level:

```sh
cargo h2 map testbuild disc/halo2.iso --out ~/h2-testmaps --map lockout
```

| Folder | What changed from the disc's map | Risk it tests |
|---|---|---|
| `moved/` | Every bitmap rebuilt as a block tree and appended after the old metadata (all at new addresses), every local LOD's pixels copied to new raw data at new file offsets. Everything else pinned. | that the engine follows fixed-up pointers to moved tags and moved raw data; alignment of moved pixel data |
| `fresh/` | Every tag byte-identical at its address, but the index tail cleared to zero and the header rebuilt from zeros (only what the builder writes, plus the low-detail fields the bitmaps index into) | the index tail and the zeroed header fields of a map built from scratch |

The command checks each map reads back with the same tags and a valid
checksum before writing. `--force` replaces earlier output. If `fresh`
fails, bisect with `--keep-tail` (writes `fresh-keep-tail/`: header still
from scratch) and `--keep-header` (writes `fresh-keep-header/`: tail still
cleared). With both, the map is the disc's byte for byte. With either flag only the
fresh variant is written.

Salts are **not** tested: pinned tags hold datums in bytes nobody has
described, so renumbering them needs full relocation of every group. The
salt risk stays open until a map built entirely from built tags (the
Halo 1 converter's first level) loads.

Run each one (Lockout is multiplayer, so this starts a local Slayer game,
[level-boot.md](../runtime/level-boot.md)):

```sh
H2_MAP_DIR=~/h2-testmaps/moved cargo h2 play disc/halo2.iso --level lockout
H2_MAP_DIR=~/h2-testmaps/fresh cargo h2 play disc/halo2.iso --level lockout
# only if fresh fails:
cargo h2 map testbuild disc/halo2.iso --out ~/h2-testmaps --keep-tail
H2_MAP_DIR=~/h2-testmaps/fresh-keep-tail cargo h2 play disc/halo2.iso --level lockout
cargo h2 map testbuild disc/halo2.iso --out ~/h2-testmaps --keep-header
H2_MAP_DIR=~/h2-testmaps/fresh-keep-header cargo h2 play disc/halo2.iso --level lockout
```

**Success:** the log shows `map override: maps\lockout from …/lockout.map`
and `level boot: lockout loaded at present N`, and the level looks and
plays like the disc's: same textures everywhere (for `moved`, no black,
missing or garbled surfaces), no crash walking round the map.

**What a failure means:**

- `moved` fails to load, or textures are wrong or garbled, and `fresh`
  is fine: the engine doesn't accept moved bitmaps or moved pixel data.
  Garbled only at certain sizes points at raw alignment (`0x200` now).
  A crash means something outside the bitmap points into a bitmap's old
  bytes (aliasing), or the engine needs metadata in its original order.
- `fresh` fails but `fresh-keep-tail` works: the engine needs the index
  tail. Maps built from scratch must generate it; find out what it is
  first.
- `fresh` fails but `fresh-keep-header` works: the engine needs one of
  the header fields we leave zero (most likely the dependency identities
  at `+0x174..+0x198`, which the copy path compares with the shared maps).
- Both bisect variants fail: both parts matter; try with both flags (the
  disc's own bytes) to rule out the override itself.
- No `map override` line: the file name or folder is wrong; the override
  wasn't used.

## Not verified (needs a game run, or more work)

- **No built map has been loaded by the game.** The round trips prove the
  layout is the retail one and that our reader agrees, not that the engine
  accepts a map whose tags moved.
- **The index tail.** A map built from scratch has only the index header,
  groups and entries in its index region. Retail maps carry about
  `0x38000` more bytes there, loaded into memory before the BSP space.
  What they are, and whether the engine needs them, is unknown. Metadata
  words with values in that address range exist (thousands in `coll`, a
  few in `ugh!`, `bitm`, `scnr`, `shad` and UI tags), but the range also
  matches `shared.map` raw pointers (selector `10`, small offsets), so
  this is no evidence either way. This is the biggest risk for maps built
  from scratch.
- Header fields a from-scratch map leaves zero: `+0x144` (CRC), the build
  identities and dependency identities (`+0x174`–`+0x198`), low-detail
  bitmap fields. The engine compares dependency identities against the
  loaded shared caches; zero may mean "none" or may fail.
- Salts: the builder's `salt_base + slot` pattern is not retail's; whether
  the engine checks salts beyond equality is unknown.
- Alignment the hardware needs for moved raw data (the builder uses
  `0x200` for bitmap pixels).
- Aliasing: a pointer from a pinned tag into a moved tag's old bytes (the
  lighting decoder found sky children aliasing other sky payloads) breaks
  silently. Only bitmaps move today, and nothing is known to alias them.
- Layouts beyond bitmaps: [tag-schema.md](tag-schema.md) infers a layout
  for every group from all retail maps and relocates 90 % of retail tags
  (`tags::schema::Relocator`), proven offline by a shuffle of all 39 maps.
  Not yet loaded by the game. The Halo 1 converter builds tags from
  scratch, so it writes every field itself and does not depend on this.
