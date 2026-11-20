# The loose tag repository (`tags::repo`)

Status: Built for extraction, the file format, closure and building a map
from scratch. Proven offline on the owner's disc (below); the Halo 1
converter assembles its levels from the repository
(`crates/tools/h1campaign/src/scratch.rs`), and the builder's open points are in
[map-builder.md](map-builder.md), "Not verified".
Code: `crates/formats/tags/src/repo.rs`. Switch: none.
Evidence labels are those of
[map-format.md](map-format.md): **V** checked on the owner's disc, **Design**
an implementation choice.

A retail map is a closed, laid-out bundle of tags. The repository takes the
bundle apart: every tag of every map becomes one file, keyed by group and
path, with nothing in it that depends on where it sat. A map is then built
from any set of those files, by following references from a scenario. This
is what a converted Halo 1 level needs: its own tags plus the Halo 2 tags
the engine requires, laid out from scratch, with no retail map pinned
underneath ([tag-schema.md](tag-schema.md), "Fresh maps").

The repository is game data. It lives outside git (under the data
directory, e.g. `<data>/tags/`, or wherever `--out` points). Nothing of its
contents, hashes or lists is committed.

## The model

```rust
use tags::repo::{closure, to_tag_set, BuildOptions, MapSource, TagRepo, TagSource, TagKey};

let mut repo = TagRepo::open(dir)?;                        // or MapSource::new(schema, map, &mut shared)?
let tag = repo.read(&TagKey::new(group, path))?;           // Option<LooseTag>
let c = closure(&mut repo, &[globals_key, scenario_key])?; // every tag reached; c.missing
let (set, report) = to_tag_set(&mut repo, &c.tags, &opts)?; // a map::build::TagSet, nothing pinned
let (bytes, _) = set.build()?;
// a retail map's own scenario, with its manifest's options and variants:
let (set, report, closure) = tags::repo::build::from_repo(dir, "scenarios\\multi\\lockout\\lockout", None)?;
```

`TagSource` is the one trait over both kinds of source: `MapSource` (a
retail map, read with `tags::schema`) and `TagRepo` (a repository on disk).
A tool that assembles a map takes `&mut dyn TagSource` and works on either.

A `LooseTag` is a key (`TagKey`: group and full path) and a body: a tree,
or an alias of another tag. A tree is a list of **nodes** (arrays), root
first. A node is its element bytes, count, element size, alignment and its
layout-dependent fields by byte offset:

| Field | Width | Holds |
|---|---|---|
| `Block(node)` | 8 | the count (kept in the bytes), then the child node |
| `Data(node)` | 8 | a data reference: byte length, then the child node |
| `Ref(key)` | 4 | a datum, as the (group, path) of the tag it names: a tag reference's second word, or a bare datum. A tag reference's group word stays in the bytes |
| `String(text)` | 4 | a string ID, as its text |
| `Raw(index)` | 4 | a raw resource: origin, length and SHA-256 |
| `Bsp` | 12 | a scenario BSP descriptor; the builder fills it for the `sbsp` its element names |

Field bytes are zero in the node; everything else is the tag's own bytes.
An array the cache compiler shared inside one tag is one node reached
twice. Where a tag cannot be taken exactly, its tree lists **losses**, one
line each (below).

## The file format (version 1)

One file per tag. Little-endian. A 16-byte header, then sections, each a
`u32` count and its entries:

| Part | Contents |
|---|---|
| header | `"H2TG"`, `u16` version (1), `u16` kind (0 tree, 1 alias), `u32` group, `u32` flags (bit 0: the tree lives in a BSP region) |
| strings | `u16` length + UTF-8 each. String 0 is the tag's own path |
| alias only | `u32` owner group, `u32` owner path (string index), `u32` offset; end of file |
| refs | `u32` group, `u32` path (string index) |
| raws | `u8` file (0 local, 1 `mainmenu`, 2 `shared`, 3 `single_player_shared`), 3 zero bytes, `u32` offset in that cache (0 for local), `u32` length, 32-byte SHA-256 |
| losses | `u32` string index each |
| nodes | `u32` data offset, `u32` count, `u32` element size, `u32` align, `u32` first fixup, `u32` fixup count |
| fixups | `u32` offset in the node, `u8` kind, 3 zero bytes, `u32` value |
| data | every node's bytes, each at its alignment from the start of data |

Fixup kinds: 0 block and 1 data reference (value: the child node), 2 datum
(ref index), 3 string ID (string index), 4 raw (raw index), 5 BSP
descriptor. In the data each field holds what its fixup names, so the bytes
are position independent on their own: a block's address word is its
child's offset from the start of data, a datum word the ref index, a string
ID the string index, a raw pointer the raw index. A reader that finds the
two disagreeing, a count larger than the file, or trailing bytes refuses the
file. Encoding is deterministic: the same tag gives the same bytes, so files
compare by content (`format::encode`, `format::decode`).

## The repository on disk

```text
<repo>/repo.txt                  "h2 tag repository", format version
<repo>/groups.txt                group table: class parent grandparent
<repo>/strings.txt               the string IDs every extracted map starts with, in order
<repo>/tags/<path>.<group>       a tag: the first content stored for the key
<repo>/tags/<path>.<group>~<h>   another content for the same key (a variant)
<repo>/raw/<hh>/<sha256>         raw resource bytes, by hash
<repo>/maps/<map>.txt            what a retail map held (manifest)
<repo>/maps/<map>.header         its 0x800-byte header
<repo>/conflicts.txt             per key with variants, the maps that differ (h2 tags extract)
```

Path backslashes become directories. A group whose bytes are not all
letters, digits or `!_-+.` (`<fx>`, `$#!+`) is written as 8 hex digits, as in
the schema file. `<h>` is the first 12 hex digits of the SHA-256 of the
variant's file. A manifest lists the map's cache type, name, scenario,
globals, low-detail region (by raw hash), index word `+0x14`, every tag in
slot order with its variant (or `-`), and the tags that could not be
extracted.

**Variants (Design).** A key's main file is the first content extracted
for it: the shared caches first (`mainmenu`, `shared`,
`single_player_shared`), then the other maps in name order. A map whose tag
has other contents gets a variant file, and its manifest names it.
`TagRepo::for_map(dir, map)` reads that map's variants, so a retail map
rebuilds from exactly its own tags; `TagRepo::open` reads main files only,
which is what a new map (a converted Halo 1 level) builds from.
`conflicts.txt` lists every key with variants and the maps that have them.
Identical tags are stored once whatever map they came from.

Why tags differ between maps, **V** (lockout against `01a_tutorial`):
sounds (`snd!`) index their map's own sound gestalt (`ugh!`), so the same
sound has other indices in each map; bitmaps, particles (`prt3`), strings
(`unic`) and vertex declarations (`vrtx`) differ in data too. The
first-content rule keeps one main file per key; for a new map that mixes
tags from many maps, the per-map couplings (below, "Open") decide which
variant is right.

## Extraction (`h2 tags extract`)

```sh
h2 tags extract disc/halo2.iso --out ~/h2-data/tags [--maps lockout,01a_tutorial] [--verify]
```

Every map on the disc (or the `.map` files given, with shared caches found
next to them), shared caches first. Each tag is walked with its group's
layout from `tags::schema` (nothing else):

- a tag `Relocator::check` accepts (99.8 % of retail tags) is taken whole:
  every array inside it tiles it, no word outside a known field holds an
  address;
- a tag in a BSP region (`sbsp`, `ltmp`) is checked the same way against its
  region's bytes;
- a tag the whole-tag check refuses (every `scnr`, some `ugh!`, `sbsp`,
  `ltmp`, a few others) is taken **field by field**: no word of the root
  outside a block field may hold an address, and each root block is checked
  on its own. In these, raw fields are trusted (a pointer into `shared.map`
  has the selector bits of a metadata address, which made every
  multiplayer `ugh!` refuse). A block the scenario editor grew outside every
  tag is read from the file when its layout has no block or raw field. A
  block that still fails is left empty and becomes a loss line. Because
  nothing then checks that the arrays found cover the tag, the nonzero bytes
  no array reaches are counted and recorded as a loss too;
- a tag with no bytes of its own (index size 0) at the start of another tag
  is an alias of it. One that starts inside another tag gets a copy of what
  its own layout reaches (a depth-first owner has no fixed offset for it).

Datums naming no tag become null. A raw resource's length is its paired
size field when the schema has one, plus two pairs the inference misses: a
bitmap's level 0 (`bitm [0x44] 0x1c`, size at `0x34`) and a sound gestalt
chunk (`ugh! [0x40] 0x0`, size at `+4` without its top bit). Paired sizes in
`matg` and `scnr` are noise (one instance per map) and only a lower bound.
Otherwise the length runs to the next raw start of that file (every raw
pointer any of its tags holds, its BSP regions, the low-detail region, the
tag index). Resources in the shared caches are read from them and stored
like local ones, with their origin kept. A shared cache's own resources
are named by that cache and offset too, as every level names them, so a
tag reads the same from the cache and from a level. `--verify` reads every stored file
back and checks it decodes to the same tag.

## Building (`h2 tags build`)

```sh
h2 tags build ~/h2-data/tags 'scenarios\multi\lockout\lockout' --out ~/h2-testmaps/repo/lockout.map [--map lockout] [--compare disc/halo2.iso]
```

`build::from_repo` takes the options from the manifest of `--map` (default:
the map whose scenario it is), opens the repository for that map's
variants, closes over references from the globals and the scenario, and
`to_tag_set` lays it out with nothing pinned:

- slot 0 is the globals, then the closure in breadth-first order. Datums are
  the builder's (`salt_base + slot`);
- string IDs: the table starts with `strings.txt` (the IDs every extracted
  map begins with, so engine-known IDs keep their indices), the rest are
  interned as met;
- every tree becomes a `Built` block tree. Each node gets a sharing key from
  its content, so identical arrays are placed once, also between tags, as
  the retail compiler does;
- local raw resources are copied in, once per hash, at `0x200`. Pointers
  into the shared caches stay pointers;
- each scenario BSP descriptor names an `sbsp` and `ltmp`: that pair becomes
  a built BSP region and the descriptor's three words its `Bsp` fixups. A BSP
  tag no descriptor names gets no data (reported);
- the header template (dependency identities and other fields the builder
  does not own) is the map's retail header, and the low-detail region its
  own.

**Onto the disc map** (`--onto disc.iso`, `build::onto_retail`): the disc
map decomposed with every tag pinned (`TagSet::from_map`), then every tag
the schema can move (`Relocator::movable`, slot 0 apart) that the
repository holds exactly is replaced by the repository's tree, laid out
anew in reverse slot order. References resolve to the disc's datums by key,
string IDs to its table by text, and local raw resources point at the disc
file's copy (matched by hash). The BSP regions, the scenario and every
lossy tag stay the disc's bytes. This is the load test of the repository's
contents that does not depend on the parts the schema cannot rebuild yet
(the BSPs); the from-scratch build is the test of everything.

`--compare` reads the built map back and extracts every tag again, then
compares each with the disc map's: canonical forms (`repo::canonical_with`:
every element's bytes with fields blanked, references by key, strings by
text, shared-cache pointers by place) and every local raw resource byte for
byte (the disc's length, read at the new pointer). A tag with losses is
compared without its loss lines (both sides lose the same fields).

## Proof

```sh
H2_DISC=disc/halo2.iso cargo test --release -p tags --test tag_repo extract -- --ignored --nocapture
# H2_REPO_MAPS=all (default lockout,01a_tutorial), H2_REPO_DIR=keep it,
# H2_REPO_COUNT_RAW=1 size raw without writing it (skips the rebuild)
cargo test -p tags --lib repo   # unit tests, no disc
```

Unit tests: every kind of tag file round-trips and re-encodes to the same
bytes; truncated, unknown-version and foreign files are refused; closure
follows references and reports missing tags; loose tags built into a map
and read back with a schema extract to the same canonical form and raw
bytes; a repository keeps a variant and reads it for its map only.

**Extraction and rebuild, V** (2026-11-17, lockout and `01a_tutorial`
into one repository; every stored file read back and decoded equal):

| | `01a_tutorial` (campaign) | `lockout` (multiplayer) |
|---|---|---|
| Tags extracted | 2,830 of 2,830, none refused | 4,512 of 4,512, none refused |
| Stored | 2,830 new | 2,777 new, 964 identical to `01a`'s, 771 variants |
| Lossy | `scnr`, `sbsp`, `ltmp`, one `coll` (drum) | `scnr`, `sbsp`, `ltmp` |
| Raw resources | 6,352 (151 MB) | 5,017 more (129 MB) |
| Extraction | 7.7 s | 5.3 s |
| Rebuilt by closure | 2,690 tags (140 not reached), 1 BSP region, 1,723 raw copied (50 MB), 4,606 shared-cache pointers | 4,429 tags (83 not reached), 1 BSP region, 632 raw copied (25 MB), 8,921 shared-cache pointers |
| File | 54 MB (disc 65 MB), metadata `0x33a000` | 31 MB (disc 32 MB), metadata `0x3a9000` |
| Compared with the disc | **2,690 of 2,690 identical**, 1,825 local raw resources byte for byte | **4,429 of 4,429 identical**, 659 local raw resources byte for byte |
| Onto the disc map (`--onto`) | 2,538 tags from the repository (10 aliases), 292 pinned, raw pointers kept; 65 MB; **2,830 of 2,830 identical** | 4,177 tags from the repository (32 aliases), 335 pinned, raw pointers kept; 32 MB; **4,512 of 4,512 identical** |

"Identical" is the canonical form and raw bytes; for the lossy tags it
means both sides lose the same (see "Open"). Variants between the two maps,
by group (variant/instances): `snd!` 367/1817, `bitm` 243/1301, `prt3`
77/290, `unic` 36/89, `vrtx` 17/146, `effe` 11/782, `weap` 5/40, `nhdt` 3/35,
and one or two each of `<fx>`, `bipd`, `char`, `jmad`, `jpt!`, `matg`,
`mulg`, `phmo`, `udlg`, `ugh!`.

**The whole disc, V** (2026-11-17, all 30 maps, `H2_REPO_MAPS=all
H2_REPO_COUNT_RAW=1`: tag files written, raw resources hashed and counted
but not written, for lack of disk space): 172,129 tags extracted in 93 s,
**none refused**, every file read back and decoded equal. 22,430 keys;
63,480 tags identical to their key's main file, 86,219 kept as variants.
247 tags are lossy (every `scnr`, most `sbsp` and `ltmp`, and a few `coll`,
`prt3`, `stem`, `spas`, `jmad`). Size: **171 MB of tag files plus 86,638
raw resources, 2,480 MB**, about 2.65 GB in all.

Variants by group (variant/instances): `snd!` 57,517/65,309, `bitm`
18,516/25,214, `prt3` 3,437/4,988, `phmo` 1,739/2,273, `unic` 1,185/1,404,
`vrtx` 719/2,261, `effe` 599/14,785, `nhdt` 273/637, `weap` 251/822,
`jmad` 240/2,268, `hlmt` 213/3,743, `bipd` 199/242, `jpt!` 193/2,722,
`foot` 190/1,698, `char` 140/322, `shad` 128/11,495, `udlg` 115/294, `coll`
95/2,842, `pmov` 86/1,791, `<fx>` 58/180, `proj` 52/725, `vehi` 34/412,
`ugh!`, `matg`, `mulg` 29/30 each (one per map), fewer than 25 each for the
rest. A first run that stored a shared cache's own resources as local
(where every level names them by cache and offset) had `mode` at 2,696 and
`jmad` at 1,299 variants; naming them the same way brought those to 11 and
240.

Tags the closure does not reach (not in the rebuilt map) are only `bitm`,
`shad`, `prt3`, `stem`, `spas`, `pmov`, `vrtx`, `deca`, `PRTM`: lockout 83,
`01a` 140. Nothing names them by datum; whether the engine finds any by
name is the open question `h1convert::fresh` keeps them for
(`KEEP_PREFIXES`). The game run decides.

## Open

- **BSP tags are lossy.** The schema's `sbsp` and `ltmp` layouts come from
  one instance per BSP and are weak. Lockout's `sbsp` loses its block
  `+0x138` (13 elements; `tags::structure` knows it as the instanced
  geometry definitions, `0xc8` bytes each per the XBE's `imul 0C8h`, while
  the schema says `0x11`) and 380 KB of nonzero bytes no array reaches; its
  `ltmp` misses 33 KB. A rebuilt BSP is therefore not the retail one, and the
  comparison cannot see that (both sides lose the same). The fix is better
  `sbsp`/`ltmp` layouts: `tags::structure` already has the cluster (`0xb0`),
  instanced definition (`0xc8`) and geometry header (`0x54`) strides.
- **`scnr` is lossy too**: lockout 8.9 KB and `01a_tutorial` 97 KB of
  nonzero bytes no schema block reaches (as in `h1convert::fresh`). One
  `coll` (the 55-gallon drum) loses a block with an undescribed pointer.
- **Per-map couplings.** A `snd!` indexes its map's sound gestalt (`ugh!`),
  and bitmaps index their map's low-detail region (header `+0x158`). A
  retail map rebuilds from its own variants, so both stay right; a new map
  mixing tags from several maps needs the gestalt merged
  (`h1sounds::merge_sound`) and the low-detail ranges re-based. The main
  file of a key is just the first content seen.
- **Raw lengths** without a stored size run to the next resource of the
  file, so padding comes along and the same resource can differ between
  maps (a source of variants).
- **Tags reached by name**, not by datum, are not in a closure (above).
  `h1convert::fresh` keeps `shaders\` and `rasterizer\` for that reason.
- **Raw in the shared caches** is stored too (with its origin), but a build
  keeps the pointer: a tag from a campaign map that points into
  `single_player_shared.map` must not go into a multiplayer map, which does
  not load that cache.
- The builder's datums (`salt_base + slot`), the zero index tail and a
  retail header template are untested in game for maps built from scratch
  ([map-builder.md](map-builder.md), "Not verified").
- The map pack's maps are not extracted yet (its shared caches are the
  disc's).
