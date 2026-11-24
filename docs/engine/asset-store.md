# The asset store: levels assembled from shared assets

Status: Design. The tag repository, the package build and the per-map rewrite it stands on are built ("What exists"), and so are slices AS1 to AS4, AS6 and AS7, and the store side of AS10 (a converter's map kept as a level of the store): the skeleton and extent table, `h2 tags reassemble`, the store-backed map file behind `store-maps`, extraction on compile, level images kept by `LevelId`, mod patches with the conflict table, and their Studio side (fork, revert, diff, **Changed by**, the conflict rows in Problems), proven on invented maps; the disc runs of AS1 to AS4 are not done ("Not verified"). The normal forms AS5 hashes with are `tags::repo::normal`, here for `was` and `replaces`. "Decisions" lists the owner's choices.
Code: `crates/formats/tags/src/repo` (the store's files and extraction; `repo/skeleton.rs` the skeleton, the extent table and `StoreFile`; `repo/compile.rs` extraction on compile), `crates/apps/cli/src/store_prepare.rs` (the CLI's side of it), `crates/runtime/vfs/src/store_map.rs` (the map file served from the store), `crates/apps/boot/src/store_status.rs` (`store.status`), `crates/tools/workspace/src/level.rs` (level images), `crates/tools/workspace/src/converted.rs` (converted levels), `crates/formats/content/src/id.rs` (`LevelId`), `crates/formats/content/src/package/` (packages and mods), `crates/tools/workspace/src/{build,edit,delta,onto,load,assets}.rs` (the conflict table, `from`, `replaces`, `tag-delta`, the store's bytes, the asset hashes a build checks against), `crates/apps/boot/src/packages.rs` (the game's side). Tests: each slice names its own.
Switch: `store-maps` (`H2_FEATURES`, off until slice AS2 is accepted; [switches.md](../runtime/switches.md)).

Every tag of every map on the user's discs is extracted once into one
deduplicated store on the user's machine. A level the game loads is
assembled from the store: its manifest names assets, and the game is
given a map file built from them. An asset many maps share is stored
once, so a change to it shows in every level that names it, unless the
change is scoped to one map (a fork). New content (the Pogo Stick, a
Forge kit, a community mod) is more assets and patches laid over the
store by packages, with no map file rewritten per package. The rules
that must hold: the store is game data under the data root and nothing
of it, its hashes or its lists is in Git; a package or a mod never holds
bytes from a disc; a level with nothing laid over it is served byte for
byte as the disc has it.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).

## What exists

| Need | What there is | State |
|---|---|---|
| Every tag as a position-independent file, raw resources by hash, a manifest per map | `tags::repo::{TagRepo, LooseTag, store_map}`, `h2 tags extract` ([tag-repo.md](../formats/tag-repo.md)) | built; the whole disc extracts with no tag refused |
| Re-extraction of only what moved | `extract.toml`, `tags::repo::extract::{plan, update}`, `TagRepo::settle` | built |
| The store in Studio: first start, `tags_root`, the browser over it | `crates/apps/studio/src/api/workspace.rs`, `app/assets.rs` ([tag-workspace.md](../editor/tag-workspace.md), W3) | built |
| Content as packages: manifest, `PackageId`, the set and its order, builders, edit lines with `was` | `content::package`, `crates/tools/workspace` ([studio-redesign.md](../editor/studio-redesign.md), "Packages") | built (PK1 to PK12) |
| Packages applied to a map the game reads | `workspace::PackageRewrite` over `workspace::build_map`: the whole map read, rebuilt onto its pinned tags, and kept as a whole map under `<data>/packages/cache/` | built |
| A map file served from something other than a file | `vfs::packages` (byte runs over a base as it streams), `vfs::sim_store` (a map served from recorded ranges, byte for byte at the same offsets) | built |
| A stale cache slot dropped when its source changes | `crates/extensions/game/src/map_slots.rs` ([cache-slots.md](../runtime/cache-slots.md), "Changed maps") | built |
| Live edits of a loaded map's tags | `crates/extensions/hot-tags`, `edit.batch` ([live-mirror.md](../editor/live-mirror.md)) | built |
| A level's identity on Live and in a film | `content::MapId`, the content set of `PackageId`s, a film's `DESC` map digest ([maps.md](../game/maps.md), [film-format.md](../game/film-format.md)) | built, the content set's session check is PK14 |
| A map given back from the store byte for byte | `maps/<map>.skel` and `.extents`, `tags::repo::StoreFile`, `TagRepo::check_reassembly` at extraction, `h2 tags reassemble` ([tag-repo.md](../formats/tag-repo.md)) | built (AS1); not run on the disc |
| The disc's maps served from the store | `vfs::store_map::StoreMapBackend` under `store-maps`, `store.status` | built (AS2); no boot from the store yet |
| The store kept current by `h2 build`, the pre-boot step and Studio | `tags::repo::compile::prepare`, `cli::store_prepare` | built (AS3); not run on the disc |
| A layered level as an image kept by its `LevelId` | `workspace::level::{assemble, StoreLevels}`, `content::LevelId`, `StoreMapBackend::with_levels`, `<data>/store/images/` | built (AS4); not run on the disc |
| Mod patches: a `set` line's `from`, `replaces`, the `tag-delta` item, the conflict table and a refused package off for one level with its reason | `workspace::{build_map_settled, delta, store_findings}`, `workspace::level::assemble_strict`, `content::package::Refusal`, `Report::refused`, `<data>/store/images/<cache key>.set` | built (AS6); on invented maps |
| A file of a package that is a raw resource or a tag file of the user's own store | `workspace::store_findings`, `h2 package check --store <data>/tags`, Studio's `package.check`, `package.read` and Problems (the store is `tags_root`) | built (AS6, AS7) |
| Mods in Studio: a fork for one map, its revert, a package's diff, **Changed by** in load order and the conflict rows in Problems | `studio::app::mods::{fork, revert, diff, changed_by}`, `asset.fork`, `asset.revert`, `package.diff`, `asset.links`' `changed_by`, `Problems::levels` | built (AS7); on invented maps |
| A converter's map kept as a level of the store, found by its stem and assembled like a disc level | `workspace::converted::{store_converted, is_stored, marked, levels}`, `<store>/maps/<stem>.converted` | built (AS10, store side); on invented maps; the converters do not call it yet |

What is missing: a boot that reads the executable, movies and fonts from
`<data>/disc/` and the maps from the store with no disc image open (the
guest still opens the image); the level tags (`sbsp`, `ltmp`, `scnr`) are lossy in
the store, so a retail level is its skeleton and cannot be rebuilt from
tags; with `store-maps` off, packages still reach the game as
whole-map caches; an asset's
identity differs between maps for reasons that are not content (a
sound's indices into its map's sound table, until the catalogue is current on the machine); a whole-asset override
(`replaces`) is built for `tag` items, and the `bitmap`, `model`, `sound`
and `animation` builders refuse it; a delta is made on the tag as the
build holds it, not on the normal form AS5 gives.

## How it works

### The store

The store is the tag repository at `<data>/tags/` (Studio's `tags_root`;
on the owner's machine a folder on the builds volume), with three
additions. One store serves the game, Studio, the tools and every
worktree on the machine; only the extractor writes to it.

```text
<store>/tags/<path>.<group>[~<h>]   an asset: a loose tag (tag-repo.md, "The repository on disk")
<store>/raw/<hh>/<sha256>           a raw resource, by hash
<store>/maps/<map>.txt              a level's manifest
<store>/maps/<map>.skel             its skeleton
<store>/maps/<map>.extents          its extent table
<store>/assets.idx                  the catalogue (new; derived, deleted and rebuilt freely)
<store>/extract.toml                the disc's identity and the extraction versions
<data>/store/layers/<PackageId>/    tags a package's builders made, in the store's format
<data>/store/images/<LevelId>.img   level images, with <LevelId>.extents
```

**An asset** is a tag: its key is its group and path (`TagKey`), its
content a `LooseTag`. Identity has two parts, and they answer different
questions:

- the **key** says which asset it is. Packages, edit lines, references
  between tags and Studio's browser name assets by key;
- the **asset hash** says which content. It is the SHA-256 of the tag's
  normal form: its bytes with references by key, string IDs by text, raw
  resources by the hash of their bytes (`workspace::tag_hash`), and the
  per-map couplings taken out ("Normal forms").

**Deduplication.** A raw resource is one file whatever maps hold it. A
tag whose file is identical in several maps is one file
(`store_map`). A key whose content differs between maps keeps a variant
file per content, and each map's manifest says which it reads
(`TagRepo::for_map`). The disc's own variants are the first forks.

**Normal forms.** Half of the disc's tags are variants, and most of
those are not different content: a sound holds indices into its map's
sound table (`ugh!`), a bitmap an index into its map's low-detail
region, and a raw resource with no stored length carries the padding
that follows it ([tag-repo.md](../formats/tag-repo.md), "Open").
`tags::repo::normal` (new) gives each group with such a coupling a
normal form: a sound with the table rows it names written out in place
of their indices, a bitmap with its low-detail pixels by hash, a raw
resource cut to its real length where the group's layout gives one. The
asset hash is over that form. Two maps then disagree about an asset only
when its content differs, and that count is what `assets.idx` records.
The stored files stay as extracted, so a retail map still reads exactly
its own bytes.

**The catalogue** (`assets.idx`) is built from the manifests: for each
key its asset hashes, the maps reading each, and the keys that
reference it. It answers Studio's **In maps** and **Where used** rows
and the assembler's "which levels does this change touch".

**A level's manifest** names what the level is made of:

- the level's name, class, scenario and globals, and every tag in slot
  order as key and variant (as built, `maps/<map>.txt`);
- its **skeleton**: the bytes of the map file that are not raw
  resources, at their offsets: the header, the BSP regions, the string
  tables, the tag index and the tag metadata. `maps/<map>.skel` holds
  them, deflate-compressed;
- its **extent table**: every other byte range of the file as
  `(offset, length, raw hash)` (`maps/<map>.extents`, text). Each local
  raw resource a tag names is an extent of its own file; a range no tag
  names (padding, bytes past the declared size) is stored in `raw/` by
  hash as an extent of its own ([tag-repo.md](../formats/tag-repo.md),
  "Skeletons and extent tables").

The rule the skeleton and the extents keep: together they give the disc
map back byte for byte. The extractor reads every map back through them
(`tags::repo::StoreFile`) and checks the SHA-256 against the map file's
before it records the map (`TagRepo::check_reassembly`); a store
extracted before skeletons existed is taken again map by map
(`extract::Why::Skeleton`). The skeleton is what makes the store whole
while the level tags are lossy: nothing the schema cannot describe is
lost, because it is still there as the disc's bytes.

**Forks.** A fork is a change with a map scope. It is never a copy in
the store: it is an item of a package whose `maps` names one map
("Mods and packages"). Studio's **Fork for this map** (`asset.fork`,
built) writes that item into the edit target. A change with no map scope
applies in every level that names the asset. **Copy into package**
(`asset.copy`, built) makes a new asset under a new key, which a level
uses when a reference is pointed at it.

### Extraction on compile

`h2 build` extracts. After the game image is built it calls
`tags::repo::compile::prepare` on the disc it was given, into the store
at `<data>/tags`, with progress on stderr (`cli::store_prepare::on_build`;
a failure fails the build; the slim host build does not extract).
`h2 play`, `h2 run` and `h2 serve` call it before boot, beside
`package_prepare::before_boot` (`store_prepare::before_boot`), and never
fail the boot: a problem is a line on stderr. Studio's extraction thread
(`workspace.extract` and the later start's background plan) is the same
call. `prepare` takes the store's lock (`extract.lock`; a second caller
waits, then finds nothing stale), hashes each map file through
`<store>/stamps.txt`, runs `extract::plan`, and runs `extract::update`
when the plan is not empty.

With nothing stale the plan opens no map and costs a stat of each map
file: a map file's recorded hash is trusted while the size and
modification time of the file it is read from (the image, for an entry),
and the entry's offset and length, match their stamp (the stamp rule of
`workspace::unit_key`). A stale store extracts before the boot, with a
progress line per map.

- **Incremental.** The plan's cases are the built ones: a new or changed
  map file is taken whole (and hashed once), a group whose layout hash
  moved re-extracts that group's tags only, an extractor or format
  version change takes everything, and a stopped run redoes one map. A
  map file only touched is hashed again, and plans nothing.
- **From the store itself.** Because the skeleton and extents give each
  map back exactly, `prepare` called with no disc image reads the maps
  from the store (`compile::Reader`, a `Read + Seek` over a disc region or
  a `StoreFile`, behind `MapSource`). A reset empties `maps/`, so the
  skeletons and extent tables it reads are first copied to
  `<store>/reextract/`, which a stopped run leaves for the next one and a
  finished run removes. A map the store has no skeleton for, with no disc
  image, is an error that names it and says the image is needed. The disc
  image is never written or deleted, and is read only when the store is
  stale in a way the store cannot answer.
- **The rest of the disc.** The executable, the movies and the font
  files are not assets. After an extraction from an image,
  `store_prepare::copy_disc_files` copies every file of the image that is
  not a map under `<data>/disc/` (about 560 MB, **V** by the directory of
  the owner's image), once per image: `copied.txt` holds the image's
  stamp and is written after the last file, so a stopped copy continues.
  Decision 1. A boot that reads them instead of the image is not wired
  (see "What is missing").
- **Other discs.** The Map Pack's maps and the converted Halo 1 levels
  enter the store through the same `store_map`, as the output of their
  `convert` items ("What it replaces").
- **Agents and worktrees** share the one store read-only. A lane never
  extracts into a path of its own.

The catalogue (`assets.idx`) is written by the same call once AS5 builds
it.

**Size.** On the owner's Halo 2 image the 30 map files are 4,101 MB,
672 MB of it the three resource caches (**V**, the image's directory).
The store for the same maps is 171 MB of tag files and 2,480 MB of raw
resources (**V**, [tag-repo.md](../formats/tag-repo.md), "The whole
disc"). Skeletons add the non-raw part of each map. On the two maps
measured that part is about 7 MB of a 32 MB multiplayer map and about
15 MB of a 65 MB campaign map (disc size less the local raw the rebuild
copies, same page); 27 levels at that rate are 250 to 400 MB before
compression (**S**: slice AS1 reports the real figure). So the store is
about 3.0 GB against 4.1 GB of maps, and it replaces more than the
maps:

| Held on the owner's machine | Size | With the store |
|---|---|---|
| The Halo 2 image | 4.66 GB | 0.56 GB of non-map files, once extracted |
| Converted Halo 1 maps and campaign, whole maps under `<data>/maps/` and `<data>/campaigns/` | 0.67 GB and 3.6 GB | layers holding only what differs from the store (**S**: not measured) |
| A rebuilt whole map per map a package touches (`<data>/packages/cache/`), every multiplayer map for the Pogo | about the size of the maps touched | one image of 5 to 15 MB each |
| Materialised patched maps (`<data>/maps/cache/`, capped at 4 GiB) | up to the cap | images |
| The guest's cache slots (`<data>/disk/`) | 1.1 GB | the same: the guest still copies a level into its slot |

The sizes in the first column are `ls` and `du` on the owner's data
root and image, not a controlled measurement.

### Runtime assembly

The guest engine is not changed. It asks for `maps\<name>.map`, copies
that file into a cache slot, loads the tag index and metadata from the
slot into its tag memory, and reads raw resources and BSP regions from
the slot's file as it runs ([cache-slots.md](../runtime/cache-slots.md)).
The store gives it that file in two parts, and neither is a hook in the
guest:

1. **A level image** holds the part of the file with addresses in it:
   header, BSP regions, index and metadata. It is prebuilt and cached.
2. **An extent shim in the VFS** serves the rest. `vfs::store_map` is a
   backend for map paths: `stat` gives the logical length, `read_at`
   answers a range from the image or from `raw/<hh>/<sha256>` through
   the extent table, as `vfs::sim_store` answers from its range index.
   No raw byte is copied to build a level.

**A level with nothing over it** is its skeleton and its extents: the
disc's bytes, the disc's SHA-256, the retail `MapId`. No build runs.
`StoreMapBackend` serves it under the `store-maps` feature from the
store at `<data>/tags`, below every other map layer
(`game::map_override::mount`): the disc's map files are never read, a
map the store lacks is `ObjectNameNotFound` with a line naming the store
and the map, and a raw file that is missing fails the read naming its
hash. The listing of `maps` is the store's maps. A map's extent table
that changes while the game runs is a changed map (`map_changed`),
served anew from its next open with no handle on the old one.

**A level with layers** (a package item whose `maps` takes it) gets an
image from `workspace::level::assemble`:

1. Resolve the layers for the level: the packages of the set, in load
   order, with an item for it, chosen as the rewrite chooses them
   (`rewrite::applied_to`: a package whose added tags the level holds
   already is left out). With none, the level is its skeleton, as above,
   and its identity the retail `MapId`.
2. Settle them under the conflict table ("Conflicts"): every package the
   level refuses is left out, whole, and the rest are the ones the level
   takes. Which those are is learned by building once and kept as the
   level's **settlement**, `<data>/store/images/<cache key>.set` (the key
   is over the base file and every package with an item for the level), so
   an open that finds it names the id without building. An image already
   under the id of the whole set says none is refused.
3. If `<data>/store/images/` holds the level's `LevelId` ("Identity"),
   use it.
4. Otherwise run the built per-map build (`workspace::build_map_settled`,
   the same items as `build_map`) on the map the store gives back: every
   tag pinned (`TagSet::from_map`), the tags the layers change replaced,
   new tags added. The builder writes the header checksum over the whole
   file. A build over the tag memory bound is refused
   (`level::check_tag_memory`, below).
5. Split the result (`level::split`): a range equal to one of the base's
   extents at the same offset stays that extent and names the raw file
   the store has; a raw resource the build placed is stored in `raw/` by
   hash, once, and named the same way; every other byte is the image's
   skeleton. Write `<LevelId>.img` (a skeleton file) and then
   `<LevelId>.extents`, the table that vouches for it, so a stopped write
   is built again.

This is what `PackageRewrite` does on first read of a map, less the
whole-map cache file: the image keeps the skeleton part only, and the
raw region is not stored again. The build still reads the level's bytes
once from the store; a build of the metadata alone, with the checksum
made from a checksum kept per raw file, is later work (**S**: that the
retail checksum composes over extents at their alignment).

With `store-maps` on, the boot gives `workspace::level::StoreLevels` to
the VFS (`vfs::Config::store_levels`). `StoreMapBackend` asks it for each
level as it is first opened and serves the image through `StoreFile`
over the store's `raw/`, as it serves a retail level; a level whose image
cannot be made is served as the store has it, with a line on stderr and
the reason in `packages.list`; a level with a package off for it is served
as it takes it, each reason a line on stderr and in `packages.list`
(`maps.<map>.refused`). The boot names its baked packages to
`StoreLevels::strict`: one of them refused stops the level's build, as a
failure does, where an installed package refused is left out.
`PackageRewrite` is then `levels_from_store`: it rebuilds no map and still
runs the kits and the observers. With `store-maps` off, `PackageRewrite`
and its whole-map cache serve the set, a refused package off for the map
with its reason in its record and the cached map keyed by what the map
takes; they go when the store is the default.

- **Load time.** A cached image costs nothing extra: the guest's copy
  into its slot paces the load, as it does for a disc map
  ([cache-slots.md](../runtime/cache-slots.md), "Copy speed on the
  loading screen"). A missing image costs one build of the level at its
  first open (`StoreLevels`), as a `PackageRewrite` cache miss does.
  Building at prepare (`boot::packages::prepare_maps`) for the maps of
  the current set, off the loading screen, is later work. The build's
  incremental cache ([tag-workspace.md](../editor/tag-workspace.md),
  "Build speed") keeps a one-asset change to the rebuilt tag and the
  address pass.
- **BSP switching.** BSP regions are in the image at the offsets the
  scenario's descriptors give, so the guest's switch reads them from its
  slot as it does from a disc map. A level whose structure is edited
  gets that region rebuilt by the modeling writers
  ([modeling-targets.md](../editor/modeling-targets.md)); the others
  stay the skeleton's bytes.
- **Raw reads** (geometry, bitmaps, sound) go to the slot's file, which
  the guest filled through the extent shim. A resource the level shares
  with a resource cache keeps its pointer into that cache (the selector
  bits of [map-format.md](../formats/map-format.md), "Raw/shared
  resources"); the caches are levels of the store too and are served
  the same way. A resource from a cache the level does not load (a
  campaign asset in a multiplayer level) is given a local extent, which
  costs an entry in a table and no bytes.
- **Slots.** A slot is reused by path alone, so a level whose `LevelId`
  moved is dropped from its slot by `map_slots` through
  `Vfs::map_changed`, as a changed package map is.
- **Tag memory.** A level's metadata must fit the guest's tag memory.
  `level::check_tag_memory` refuses an image over the retail bound for
  its type (or the base's own total, when that is larger) and names the
  tags the packages added and changed
  ([scenario-editor.md](../editor/scenario-editor.md), section 6).
- **The cap.** After a build, `level::trim` keeps `<data>/store/images/`
  under 4 GiB (`DEFAULT_IMAGE_CAP`), least recently used first (a hit
  renews the table's modification time), sparing the images the boot
  serves.
- **Converted levels.** A map a converter wrote (a converted Halo 1
  map) is kept by `workspace::converted::store_converted`: its tags go
  through `store_map` (a tag the store holds is not written again, a key
  held with other contents is a variant only this level reads), its
  skeleton and extents through `store_skeleton`, and the store must give
  the map back byte for byte. A `<stem>.converted` marker in
  `<store>/maps/` (converter and file hash) is written last; a stem a disc
  extraction made is never overwritten, and a converted level whose file is
  the converter's current output is left alone (`is_stored`). Nothing
  falls back: a stem that is not a plain lower-case name, a map that does
  not read, a pointer into a shared cache the caller cannot give, a tag the
  extractor refuses and a store that does not give the map back each stop
  the level with an error naming it (a converted Halo 1 level never takes
  a Halo 2 tag silently). The level then assembles as a disc level does
  (`level::assemble`). A re-extraction of the disc under a new extractor
  version empties `<store>/maps/`, markers included, and the converter's
  unit runs again.
- **Levels built whole.** A level of ours (the stage) has no skeleton: its
  image is a `fresh` build from the manifest's closure. A retail level builds that way once its level tags extract
  without loss (W10 of [tag-workspace.md](../editor/tag-workspace.md)).

**The renderer.** The native loaders read a level's models, world and
textures from its tags and name GPU resources by tag datum and map
epoch ([framescene.md](../graphics/framescene.md), section 4.2). An
image carries a side table, datum to `(key, asset hash)`, so the loaders
key what they upload by asset hash: an asset two levels share is not
decoded and uploaded again on a level change, and a model no loaded
level holds is read from the store by key (`native_render::model_load`
over a `TagSource`).

### Mods and packages

A mod is a package ([studio-redesign.md](../editor/studio-redesign.md),
"Packages"): the same `package.toml`, lock, `PackageId`, set order and
Live routes. There is one format. A package is **patches** to assets
the store has and **new assets**; it never holds disc bytes.

| Part | Item | Names its target by | Holds |
|---|---|---|---|
| A field patch | `edits` (built) | key, and per line the field path with `from`, the value it replaces (built) | our values only |
| A structural patch | `edits` `block` lines (built) | key and `was`, the asset hash of the base | the entries added |
| A whole-asset override | a `tag` item with `replaces = "<asset hash>"` (built). The manifest lets a `model`, `bitmap`, `sound` or `animation` item carry one too; their builders refuse it by name | key and that hash | an authored source: YAML |
| A delta | `tag-delta` (built) | key and `was` (optional) | a BPS patch from the base's image to the changed asset's: the new bytes and ranges of the old |
| A new asset | `tag`, `model`, `bitmap`, `sound`, `animation`, `screens`, `hud`, `rules` (built) | a key no layer below has | the source |
| A whole-level patch | `map-patch` (built) | the base map's SHA-256 | a PPF, IPS or BPS file |

Naming a target by key and asset hash is what lets a mod be shared: the
file says which of the user's own assets it changes and holds only the
change. `from` on a `set` line checks one field, so two mods that set
different fields of one asset both apply, and a line applies to each
map's variant of the asset in which the field has that value. The check
is against the tag as the build first found it, so the later of two
packages setting one field still applies over the earlier's line: its
value is in the level and the override is listed. A `replaces` takes the
asset at its key and leaves it the same tag (what names it still reaches
it) with the authored content; `like` may name that asset, so an override
is the asset with fields changed.

**The asset hash** a `was` or `replaces` names is `tags::repo::normal::hash`
over the tag's normal form, read for the level (a form depends on the
map's tables: its sound gestalt, its low-detail region, the raw files a
padded resource is cut by). A build asks `workspace::assets::OfMap`: the
store's catalogue (`assets.idx`) answers for the base's own tags, and a
base with no catalogue, or a tag a package wrote, is read from the map's
bytes and given its form. The asset's hash as an override leaves it is in
`Report::replaced`, for a later patch's `was`. A build given no assets
(`h2 package build`, Studio's `package.build`) checks against a tag's own
hash (`workspace::tag_hash`), as a build on an invented map does.

**An image** (`workspace::delta::image`) is an asset as the build holds
it, with nothing that depends on where it sits: its arrays in tree order,
each its bytes and the fields the builder fills in, by what they name
(tags by group and path, string IDs by text, raw resources by the hash of
their bytes or by the cache and offset they sit at). A `tag-delta` item's
source is a BPS patch (`content::patch::write::bps`) from the base's
image to the changed asset's, so it holds the bytes the author changed and
ranges of the user's own asset. `was` is checked as an `edits` entry's
is; the patch's own source size and CRC32 must then match the image, so a
patch made on other bytes is refused too, and an item without `was` stands
on that check alone. The image the patch writes is read back with every
length, index and alignment checked, names no raw resource the map does
not hold, and gives the changed asset its own arrays (it keeps no sharing
key).

**Layering and order.** A level reads its assets through
`workspace::Layered`, first hit wins, top first: the user's local
packages, packages installed from Live, the baked packages, then the
store read for that map. Within the packages the order is the set's:
`requires` and `after`, then `set.toml`, then id (`content::package::Set`).
Patches are applied bottom up, each to what the layers below give.
Tags a package's builders make that do not depend on the level are
built once into `<data>/store/layers/<PackageId>/` and named by every
level that takes them. A builder that reads the level (a weapon that
borrows the map's first-person view, a shader written on the map's
template) runs per level, cached by the hashes of what it read.

**Per-map scope.** An item's `maps` (`"multiplayer"`, `"campaign"`,
`"all"`, or names) is its scope. A mod that changes an asset for one
map only is an item with that map's name: a fork. The asset is
unchanged everywhere else: a fork moves its level's `LevelId` and no
other's, and a mod refused on one level for a variant of the asset
applies on the others.

**Conflicts** (`workspace::level::assemble` over
`workspace::build_map_settled`, reported in `Report::overrides` and
`Report::refused`):

| Case | Result |
|---|---|
| Two packages set different fields of one asset | both apply |
| Two packages set the same field | the later package's value; the override is listed with both names |
| A `from`, `was` or `replaces` that does not match what the store holds | the item is for other content (another pressing, a changed base): the package is off for that level with the reason, which names the key and both hashes |
| A package patches an asset an earlier package overrides whole | applies only when the patch's `was` names the override's hash and the package `requires` the other; otherwise the later package is off, naming both |
| Two packages override the same asset whole, or add the same key | the later package is off, naming both |
| A patch whose key is missing | off, with the key |
| A package that `requires` one off for the level | off too, naming what it requires |
| A whole override follows a patch of the same asset | the override takes the asset; each field the earlier package set is listed as overridden |

A refusal is an item's, and it is the package's: one refused item puts
the whole package off for the level, and nothing of it applies, down to
an item that ran before the refused one (the map is built again without
the package). The reason is a line on stderr and in `packages.list`
(`maps.<map>.refused`); Studio's `package.diff` with a map reads the same rows and Problems lists them ("In Studio"). A refused
package moves neither the level's `LevelId` nor its image: both are over
the packages the level takes, so a peer without the mod holds the same
level. A first-party package (`content/`) that refuses stops the level's
build instead, as a failure does, and every refusal stops `h2 package
build` and Studio's `package.build` with the item's line. `h2 package
check` names without a map what two packages settle between themselves:
two that override one asset whole, or add one tag path, for maps that can
meet.

**Enable and disable.** `set.toml` holds what is off and the order
(`package.enable`, `package.order`). The store is not touched by any
mod, so nothing is extracted again; a level's `LevelId` moves, the
image for the other set is still in the cache, and the change shows at
the next load of the level.

**In Studio.** The Packages root lists the set; a package opens as a
document (built). For mods (`app::mods`, `api/assets.rs`,
`api/packages.rs`):

- `package.diff {id?, map?}`: per asset, the fields the package sets
  (each line's `to`, and its `from` when the line names the value it
  replaces) and its block lines, the assets it adds, overrides whole
  (`replaces`) or patches (`tag-delta`), which items are forks, and the
  levels it changes. With `map` and the map's file the set is built over
  that map (`workspace::build_map_settled`) and `conflicts` holds the
  conflict table's rows (`refused` with the reason, `override` with both
  package names); `involves` marks the rows that name this package, and
  the rows replace the last ones read for that level in Problems;
- the inspector's **Changed by** row, and `asset.links`' `changed_by`,
  name every package with a patch on the asset in front, in load order,
  with each field's value and whether it wins (a later package that sets
  the field, or overrides the asset whole, takes it);
- `package.check`, `package.read` and Problems refuse a file whose
  SHA-256 is a raw resource or a tag file of the store, so the user's own
  store is the retail list on their machine (`workspace::store_findings`,
  with `tags_root` as the store; `h2 package check --store <data>/tags`
  runs the same), and name two packages that override one asset whole
  for maps that can meet;
- `asset.fork {tag, map, package?, set?}` writes the tag's lines into
  `edits/<map>.yaml` of the package (the edit target by default), an
  `edits` item whose `maps` names that map alone. Without `set` they are
  the lines the package's other `edits` items that reach the map have for
  the tag; a tag with none, a tag forked already, a built-in package and a
  map that cannot name a file are refused. `asset.revert {tag, map?,
  package?}` takes the tag's lines out: with `map` only the item that names
  that map alone, without every `edits` item with an entry for the tag;
  an item left with no entry goes with its file. The inspector's **Fork
  for this map** and **Revert for this map** (`Action::ForkAsset`,
  `Action::RevertAsset`) make the same calls on the open map;
- **Import mod...** (`package.import`) turns a patched map into a
  package: the patch is applied to the base the store reassembles, the
  result is stored as a layer (`store_map` keeps only what differs),
  and the difference is written as `tag-delta`, `replaces` and new
  items. A change the delta cannot say (in a lossy level tag) leaves
  the mod a `map-patch`, which takes the whole level and composes with
  nothing.

**What the package system becomes.** `PackageRewrite` and its whole-map
cache become `level::assemble` and the image cache. `map` items and the
derived map packages they write become level manifests in the store.
`map-patch` stays as the fallback above. `MapId` stays the name of a
map package on Live; what gates a session is `LevelId`. `PackageId`,
the lock, signing, `/v1/packages` and the content set are as designed
(PK13, PK14). The `pogo` package is unchanged as a source: its items
are the same files, and its tags are a layer instead of a rewrite of
each multiplayer map.

### Identity on the network and in films

**`LevelId`** is the SHA-256 of the canonical JSON
(`content::canonicalize`) of what a level is assembled from:

```json
{"builders": 7, "format": 1, "level": "<name>", "skeleton": "<sha256 or null>",
 "assets": [["<group>:<path>", "<asset hash>"], ...],
 "packages": ["<PackageId>", ...]}
```

`content::LevelId` makes it from `content::LevelInputs`;
`workspace::level::level_id` fills them. `packages` is the ordered list
of packages laid over the level; `builders` is
`workspace::BUILDERS_VERSION`; `skeleton` is the SHA-256 of the map file
the store's skeleton gives back. `assets` is the manifest after
layering, sorted by key. For a level with a skeleton the skeleton's hash
stands for every asset the store gives it, and the packages' ids for
what they add and change, so `assets` is empty; it lists asset hashes
once the catalogue gives them without reading each tag (AS5). Assembly is deterministic, so two
machines with the same `LevelId` hold the same image bytes
(**Test**, as `workspace::cache_key`'s builds are; across platforms it
is **S**, slice AS8's test). A level with nothing over it also has a
`LevelId`; its file hash is the disc's.

- **What peers compare.** A session carries `level_id` and the content
  set's `PackageId`s. A joiner assembles the level and compares. On a
  mismatch the host's manifest is fetched and the join fails naming the
  first assets that differ and whether a package or the disc supplies
  them, before any map load. A missing package is fetched and the level
  assembled again (PK14's flow).
- **Scope.** Every asset counts. A `client` item that only replaces
  pixels or samples still moves `LevelId`, because it can move tag
  addresses in guest memory. Laying the `sim` assets out first and the
  `client` ones after them, and leaving client assets out of the id, is
  a later relaxation that needs a lockstep run with and without the item
  ([studio-redesign.md](../editor/studio-redesign.md), "Contents").
- **Peers without a disc.** A dedicated host has no disc and no store.
  Its data bundle (`crates/live/host-data`) holds, per level, the image
  and the ranges its coverage reads, packed as the sim store it already
  uses (`vfs::sim_store`); the bundle's manifest carries each
  `LevelId`. The store's hashes never leave the machine: Live serves
  package blobs only and refuses a blob on the retail list.
- **Films.** `DESC` gains `level_id` and the content set beside the map
  digest it holds ([film-format.md](../game/film-format.md)). The digest
  stays the hash of the mounted bytes (or of the recorded ranges on a
  sparse host), so a film of a level with nothing over it verifies
  against the disc's map. Playback (`theater::metadata::prepare_map`) assembles
  the level by `level_id`, fetching packages by `PackageId`, and checks
  the digest. The image cache keeps every image a saved film names, so
  a local film plays after `BUILDERS_VERSION` moves; a film from another
  machine whose image cannot be assembled is refused with the builder
  versions named. Decision 7.

## What it replaces

| Path | Becomes |
|---|---|
| The Pogo and every per-map item: `PackageRewrite` rebuilding each multiplayer map and keeping a whole copy | a layer built once; each level's image names its tags |
| Forge kits: a materialised `.map` per document ([forge-design.md](../game/forge-design.md), "Materialising") | the base level's manifest plus the kit's layer and the document's placements; an image per document hash |
| The Monitor's body from a campaign map: a host model list that opens another map file ([forge-monitor-appearance.md](../game/forge-monitor-appearance.md), "Gaps") | the model read from the store by key; a catalogue entry is `(key, asset hash)` in place of `(key, source map SHA-256)`. A body as a real object is an asset added to the level, within tag memory |
| Studio edits: an edit line, then a rebuilt map | the edit line; the paired game takes it at once through `edit.batch`, and every other level that names the asset takes it at its next load because its `LevelId` moved |
| Community mod packs: one whole-map patch per map | tag-level packages that compose, with `map-patch` as the fallback |
| Converted Halo 1 maps and the Map Pack: whole maps as package layers | levels of the store; the Halo 2 assets they borrow are stored once |
| `<data>/packages/cache/`, `<data>/maps/cache/`, the derived maps under `<data>/maps/` | `<data>/store/images/`, trimmed by least recent use to a cap, never an image a saved film names |

## Interfaces

| Caller | Call | Does |
|---|---|---|
| `h2 build`, `h2 play`, `h2 run`, `h2 serve`, Studio | `tags::repo::compile::prepare` (the plan and `extract::update`; `store::catalogue::write` with AS5) | the stamps, the plan, the extraction |
| The VFS | `vfs::store_map::StoreMapBackend` over `StoreFile::{len, read_at}` | a level's bytes from an image or a skeleton and the raw files |
| `StoreLevels` (the VFS's, at a level's first open); Studio's `package.build` (later) | `workspace::level::assemble(store, images, packages, level) -> Level {id, map_id, image, packages, held, refused, report}`; `assemble_strict(.., strict)` | the image for a level under a set |
| The game's agent API | `packages.list`, `store.status` (built): the store's path, whether its maps are served, each map's length, SHA-256, skeleton size and extents, what is stale, and the level images | read |
| `h2 package check --store <data>/tags` | `workspace::{check_package, check_set, store_findings}` | what is wrong with a package before any map is built, and a file that is the store's |
| Studio's API | `workspace.status`, `workspace.extract`, `workspace.prune`, `asset.fork`, `asset.revert`, `package.diff` (built); `package.import` (new) | above |
| `h2 tags` | `extract --update`, `reassemble MAP --out FILE`, `reassemble --all [--compare DISC]` (built), `level <map> [--set FILE]` (new) | the coordinator's and CI's entry |

## Rules and limits

- The store, the layers and the images are game data under the data
  root. `workspace` refuses a root inside a Git checkout. No hash, list
  or count of the store's contents is committed.
- The store is written by the extractor only. A change is always an
  item of a package.
- A package holds no disc bytes: `h2 package check --store <data>/tags`
  refuses a file whose SHA-256 is a raw resource or a tag file of the
  user's own store, and Live's upload refuses a blob on the operator's
  retail list ([maps.md](../game/maps.md), section 5).
- A level with nothing over it is the disc's bytes. Anything else has
  an image, and a level never mixes the two.
- A refused package is off whole, with its reason. Nothing is applied
  in part and nothing is dropped quietly.
- Nothing is taken from a leaked build: not a layout, not a tool's
  behaviour.
- A network game needs equal `LevelId`s. A local unsigned package with
  a `sim` item refuses a Live session (`boot::packages::network_refusal`).

## Risks, and what is proven first

1. **The guest accepts a store-backed file.** Removed by construction
   in the first slice: the file served is the disc's bytes, checked by
   SHA-256, so every guest check, the cache slots and the film digest
   see what they saw.
2. **Tag memory.** Retail multiplayer maps have 0.1 to 1.5 MiB of room
   under the retail bound and one has none (**V**,
   [scenario-editor.md](../editor/scenario-editor.md), section 6). A
   store makes adding assets easy; the guest's memory does not grow.
   The engine's own bound is not measured.
3. **Lossy level tags.** Until `sbsp`, `ltmp` and `scnr` extract whole,
   a retail level needs its skeleton, a change to those tags goes
   through the writers that know them, and a mod that changes them
   stays a `map-patch`.
4. **Per-map couplings.** The sound table and the low-detail region are
   merged per level. The normal form has to take them out of the asset
   hash without losing them, or "one asset" stays 57,000 sound variants
   (**V**, [tag-repo.md](../formats/tag-repo.md), "The whole disc").
5. **Determinism.** Two machines must assemble the same bytes from the
   same `LevelId`, on every platform, or `LevelId` cannot gate a
   session.
6. **Read cost.** A level's raw region is thousands of small files.
   The guest's slot copy reads them in file order; whether opening them
   is slower than one contiguous read is measured in AS2, and a pack
   file per level is the answer if it is.
7. **The checksum over extents** is supposed. An image is built from
   the whole map, so the builder's checksum holds; a build of the
   metadata alone needs it.
8. **A wrong store.** A store extracted from a damaged or patched image
   makes a level no peer shares. `LevelId` catches it at the join and
   the mismatch names the assets.

**The first proving slice is AS1 with AS2**: the skeleton and extents,
`reassemble` equal to the disc for all 30 maps, and the game booting a
retail level through `vfs::store_map` with the disc's maps hidden. It
changes no tag and no identity, needs no builder work, and settles
risks 1 and 6 and the real size of the store. The disc check:

```sh
h2 tags extract disc/halo2.iso --out <data>/tags --update
h2 tags reassemble --repo <data>/tags --all --compare disc/halo2.iso
H2_FEATURES=store-maps h2 run ...   # then: h2 agent store.status
```

## Build slices

One lane each; a lane writes code and tests and the coordinator runs
them. Tests are headless on invented maps unless marked disc
(`#[ignore]`). Most valuable first, within what each needs.

| # | Slice | Needs | Files | Tests | Accepted when |
|---|---|---|---|---|---|
| AS1 | **Skeleton and extents.** The extractor writes `maps/<map>.skel` and `.extents`; `TagRepo::reassemble`; the plan re-extracts a map lacking them; `h2 tags reassemble` | nothing | `crates/formats/tags/src/repo/{skeleton.rs (new), extract.rs, store.rs, tests.rs}`, `crates/apps/h2-tools/src/tag_repo_cmd.rs`, [tag-repo.md](../formats/tag-repo.md) | an invented map with shared pointers, an unnamed raw range and two BSPs reassembles to its bytes; a missing raw file is a named error; disc: all 30 maps reassemble to their SHA-256 | the disc test passes and prints the store's size by part |
| AS2 | **The map file from the store.** `vfs::store_map` (length, ranged reads, a level by name), wired into boot's backend chain under `store-maps`; `store.status` | AS1 | `crates/runtime/vfs/src/{store_map.rs (new), lib.rs}`, `crates/runtime/vfs/tests/store_map.rs` (new), `crates/apps/boot/src/{lib.rs, store_status.rs (new)}`, `crates/extensions/game/src/map_override.rs`, [agent-api.md](../runtime/agent-api.md), [switches.md](../runtime/switches.md) | reads across extent borders equal the reassembled bytes at every offset class; a store lacking a level falls to a named error, never to another file; `map_changed` fires when a level's extents change | a retail level boots and plays from the store with the image's maps hidden, and its film's map digest equals the disc run's |
| AS3 | **Extraction on compile.** `h2 build` and the pre-boot plan call `extract::update`; the stat fast path; the non-map files copied to `<data>/disc/`; re-extraction from the store; progress | AS1 | `crates/apps/cli/src/{store_prepare.rs, boot_cmd.rs, package_prepare.rs}`, `crates/formats/tags/src/repo/compile.rs`, `crates/apps/studio/src/app/workspace.rs`, [tag-repo.md](../formats/tag-repo.md), [building.md](../runtime/building.md), [storage.md](../runtime/storage.md) | unchanged input opens no map; a touched map file is hashed once and taken alone; a moved layout hash re-extracts from the store with no disc; an interrupted run resumes | `h2 build` on a fresh data root leaves a store that AS1's disc test accepts, and a second run does nothing |
| AS4 | **Level images.** `workspace::level::{assemble, LevelId}`: `build_map` on the skeleton, raw as extents, the checksum from per-file sums, the image cache and its cap; `PackageRewrite` and `<data>/packages/cache/` removed | AS2 | `crates/tools/workspace/src/{level.rs (new), rewrite.rs, build.rs, game.rs}`, `crates/formats/content/src/id.rs`, `crates/apps/boot/src/packages.rs`, [studio-redesign.md](../editor/studio-redesign.md) | the image and extents read back equal `build_map`'s whole map byte for byte; golden `LevelId`s on invented hashes; a set change gives another id and the first image is still a hit; an image over the tag memory bound is refused naming the assets | a multiplayer level with the Pogo loads from an image and `<data>/packages/cache/` is not written |
| AS5 | **Normal forms and the catalogue.** `tags::repo::normal` for sounds, bitmaps and padded raw; the asset hash; `assets.idx`; Studio's In maps and Where used from it | AS1 | `crates/formats/tags/src/repo/{normal.rs (new), catalogue.rs (new)}`, `crates/tools/workspace/src/hash.rs`, `crates/apps/studio/src/app/assets.rs`, `docs/formats/notes/` (the variant counts) | two invented maps with one sound at different table indices give one asset hash and another when a sample differs; the catalogue rebuilt from manifests equals the one kept | the note records the disc's variant counts by group before and after |
| AS6 | **Mod patches.** `from` on `set` lines, `replaces`, the `tag-delta` item, the conflict table, off-with-reason for installed packages, the retail refusal against the store | AS4, AS5 | `crates/formats/content/src/package/{manifest.rs, set.rs}`, `crates/tools/workspace/src/{edit.rs, delta.rs (new), level.rs, builders.rs}`, `crates/apps/h2-tools/src/package_cmd.rs`, [studio-redesign.md](../editor/studio-redesign.md), [maps.md](../game/maps.md) | one test per row of the conflict table; a delta applied to its base gives the changed tag and to another base is refused; a package holding a store blob is refused by file name | two invented mods on one weapon's different fields both show in the assembled level |
| AS7 | **Mods in Studio.** `package.diff`, `asset.fork`, `asset.revert`, **Changed by** in order, the conflict rows in Problems | AS6 | `crates/apps/studio/src/{api/packages.rs, api/assets.rs, app/{mods.rs (new), package_doc.rs, links.rs, problems.rs}, ui/inspector.rs}`, `studio-proto`, [agent-api.md](../editor/agent-api.md) | every method's example; a fork changes one level's `LevelId` and no other's; revert removes the item and its line | a field forked for one map shows there and nowhere else, in the window and through the API |
| AS8 | **Identity on Live and in films.** `level_id` in the session and the join check with the asset diff (with PK14); `DESC.level_id`; `prepare_map` by `LevelId`; film-named images kept; the cross-platform determinism test | AS4; PK13 for Live | `crates/live/live-proto`, `crates/live/live-hle/src/maps.rs`, `crates/extensions/game/src/theater/metadata.rs`, `crates/live/film/src`, [film-format.md](../game/film-format.md), [live-api.md](../live/live-api.md) | on the fake server a joiner with another asset fails before the map load naming it; a film recorded on an assembled level verifies, and after a builder bump plays from its kept image | two machines of different architectures print the same image hash for one `LevelId` |
| AS9 | **Mod import.** `package.import` and `h2 live mods import` write tag-level packages through the store, `map-patch` as the fallback | AS6 | `crates/tools/workspace/src/import_map.rs` (new), `crates/apps/cli/src/live_cmd`, `crates/apps/studio/src/api/packages.rs`, [maps.md](../game/maps.md) | an invented patched map imports to a package that assembles to the same tags; a patch touching the scenario's lossy part stays a `map-patch` and says so | two imported mods of one base map load together |
| AS10 | **Converted and imported levels in the store.** `workspace::converted::store_converted` keeps a converter's map as a level (**built**); the `convert` items call it with their output and `<data>/maps/` derived packages and `<data>/campaigns/` whole maps are removed (**not built**: the Halo 1 and Map Pack converters, the boot's map list and the campaign picker still read the files) | AS4 | `crates/tools/workspace/src/{converted.rs, convert.rs, maps.rs}`, `crates/apps/cli/src/{h1_import.rs, mappack_cmd.rs}`, [multiplayer.md](../halo1/multiplayer.md), [map-pack.md](../game/map-pack.md) | an invented converted level stores only tags the store lacks and assembles to the map the converter wrote | a converted level loads from the store; the note has the disk size before and after |
| AS11 | **The renderer on asset hashes.** The image's datum table; loaders keyed by asset hash; a model read from the store by key; the Monitor's catalogue by `(key, asset hash)` | AS4, AS5 | `crates/rewrites/native-render/src/{model_load.rs, models.rs, load.rs}`, `crates/extensions/forge/src/look/census.rs`, `crates/apps/cli/src/forge_looks.rs`, [native-objects.md](../graphics/native-objects.md), [forge-monitor-appearance.md](../game/forge-monitor-appearance.md) | an invented model shared by two levels uploads once across a level change; a model in no loaded level resolves from the store | a campaign body draws on a multiplayer level with no other map file opened |
| AS12 | **Forge on the store.** A Forge document as a layer; the materialised map and its package removed | AS4 | `crates/tools/forge-build`, `crates/formats/content/src/forge.rs`, `crates/extensions/forge`, [forge-design.md](../game/forge-design.md) | the image for an invented document equals the materialised map's tags | a saved Forge variant loads with no `.map` written |
| AS13 | **Bundles from the store.** The host data bundle and the device packers (`h2host::layout`) take images and a subset of the store | AS4 | `crates/live/host-data/src/bundle`, `crates/apps/h2host/src/layout.rs`, [dedicated-host.md](../live/dedicated-host.md) | an invented bundle serves every range its coverage names and refuses one it lacks by name | a host plays a match from a bundle built with no disc image mounted |
| AS14 (later) | **Retail levels built whole.** With W10, a retail level assembles `fresh` and its skeleton is dropped | W10 | as W10 | as W10 | a retail level with no skeleton loads |
| AS15 (later, **E first**) | **No slot copy.** The guest's slot answered as warm and its reads routed to the store file | AS2; a read of the status and resource-read functions | `crates/extensions/game/src/map_copy.rs` | the kernel cache-slot test with the route on | a level load copies nothing and `<data>/disk/` shrinks |

AS1 to AS4, AS6 and AS7 are built, and AS10's `store_converted`; the disc checks of AS1 to AS4 are the
coordinator's (above). AS4's build reads the level whole and builds at
first open, and `PackageRewrite` with `<data>/packages/cache/` stays for
`store-maps` off.
AS1, AS3 and AS5 run together; AS2 follows AS1; AS4 follows AS2; AS6,
AS8, AS10, AS11, AS12 and AS13 follow AS4 side by side.

## Decisions

1. **The disc image after the first extraction.** A boot does not open
   it: the store reassembles every map, and the executable, movies and
   fonts are copied to the data root. The image is never deleted; it is
   read again only when the store has to be extracted again.
2. **Where extraction runs.** `h2 build`, with the pre-boot plan as a
   check that extracts when the store is stale. Studio calls the same
   code.
3. **What an edit in Studio touches.** Every level that names the
   asset. **Fork for this map** is the explicit other choice.
4. **When two mods collide.** Different fields both apply. The same
   field goes to the later package and is listed. A structural clash
   turns the later package off with the reason.
5. **What a shared mod may hold.** Field patches, authored overrides,
   new assets and `tag-delta` items. Any blob matching a retail hash is
   refused.
6. **Community mods already imported.** They are converted to
   tag-level packages at import, with `map-patch` when a mod cannot be
   said that way.
7. **What gates a session, and films.** `LevelId` gates joins and names
   a film's level. `MapId` and `PackageId` stay the names of things on
   Live. The images a saved film names are kept.
8. **Client-only assets in `LevelId`.** Every asset counts until a
   lockstep run shows a client asset cannot move the simulation.
9. **Converted Halo 1 levels.** They go into the store after AS4
   (AS10).
10. **Raw resources on disk.** One file per resource, as built, with a
    pack file per level only if AS2 measures the reads as slow.

## Evidence

- The whole disc extracts with no tag refused; 22,430 keys, 172,129
  tags, half of them variants, most of those sounds and bitmaps;
  171 MB of tag files and 2,480 MB of raw resources (**V**,
  [tag-repo.md](../formats/tag-repo.md), "The whole disc").
- `sbsp`, `ltmp` and every `scnr` are lossy in the store (**V**, the
  same page, "Open"), which is why a retail level keeps a skeleton.
- A retail map rebuilt onto its pinned tags with its movable tags moved
  runs in the game (**Run**, [tag-schema.md](../formats/tag-schema.md),
  "Load test"); `PackageRewrite` serves every package item that way
  (**Code**, `crates/tools/workspace/src/rewrite.rs`).
- The VFS serves a map from ranges at their offsets
  (**Code**, `crates/runtime/vfs/src/sim_store.rs`) and lays runs over
  a base as it streams (**Code**, `crates/runtime/vfs/src/packages.rs`).
- The guest reuses a cache slot by path alone and reads resources from
  the slot's file after the copy (**E**, the research points of
  [cache-slots.md](../runtime/cache-slots.md), "Changed maps").
- The native object path names a model by tag datum and map epoch
  (**Code**, [framescene.md](../graphics/framescene.md), section 4.2).
- Map file sizes on the image, and the sizes under the owner's data
  root (**V**: the image's directory listing, `du`).
- Skeleton sizes, the store's total, and the converted levels' size as
  layers are **S**: AS1 and AS10 measure them.
- The skeleton, the extent table, `StoreFile` and `StoreMapBackend` give
  invented maps back byte for byte at every offset class (**Test**,
  `crates/formats/tags/src/repo/tests.rs`,
  `crates/runtime/vfs/tests/store_map.rs`).
- Unchanged input opens no map and hashes none; a touched file is
  hashed once and plans nothing; a changed one is taken alone; a moved
  layout hash and a version change re-extract from the store with no
  disc, from the stash after a stopped reset; a stopped run resumes
  (**Test**, `crates/formats/tags/src/repo/tests.rs`, `compile_*`;
  `crates/apps/cli/src/store_prepare.rs`).
- `LevelId` gives fixed ids for invented inputs and moves with each
  input; a level image reads back equal to `build_map`'s map byte for
  byte and names the base's raw file where the bytes are unchanged; a
  set change gives another id and the first image is still a hit
  (**Test**, `crates/formats/content/src/id.rs`,
  `crates/tools/workspace/src/level_tests.rs`,
  `crates/runtime/vfs/tests/store_map.rs`).
- Two invented mods on one weapon's different fields both show in the
  assembled level; a `from` applies where the field holds its value and
  turns its package off for a level whose variant holds another; a fork
  moves its level's `LevelId` and no other's; a refused package moves
  neither a level's `LevelId` nor its image, and its settlement keeps the
  next open from building; one test per row of the conflict table, with a
  package that requires one off for the level off too and named, and a
  refusal of ours stopping the build; a delta applied to its base gives
  the changed tag and is refused on another base or when its patch was
  made on other bytes; a package holding a store blob is refused by file
  name (**Test**, `crates/tools/workspace/src/mod_tests.rs`,
  `delta.rs`, `rewrite_tests.rs`,
  `crates/formats/content/src/package/tests.rs`,
  `crates/apps/h2-tools/src/package_cmd.rs`).

## Not verified

- That all 30 disc maps reassemble to their SHA-256, and a retail level
  boots and plays under `store-maps` with a film digest equal to the
  disc run's (AS1 and AS2's acceptance).
- That `h2 build` on a fresh data root leaves a store AS1's disc test
  accepts and a second run does nothing, and the time of the stat fast
  path over the 30 maps.
- That the copy of the disc's non-map files is the 560 MB estimated.
- That the header checksum of a level can be made from per-file sums.
- That a multiplayer level with the Pogo loads from an image under
  `store-maps` (AS4's acceptance).
- That the guest's slot copy through thousands of small files is as
  fast as from one file.
- How much room the engine's own tag memory bound leaves past the
  retail one.
- That assembly gives the same bytes on every platform.
- Which groups a client item may change without moving the simulation.
- The disk size of converted Halo 1 levels as layers.
- That a re-extraction under a better layout leaves every installed
  mod's `was`, `from` and `replaces` matching, since the asset hash is
  over a normal form that a layout change can move.
- That an image a delta is made on (`workspace::delta::image`: the asset
  as the build holds it) is the normal form AS5 hashes. Until it is, a
  delta made on one map's variant of an asset with a per-map coupling (a
  sound, a bitmap) is refused on another's.
- That the catalogue and the map's own bytes give one asset hash beyond
  the invented map's (`workspace::assets`).
- That an asset's hash as an override leaves it (`Report::replaced`) is
  what AS9's importer computes for the same content, so a patch over an
  override written there is taken.
- `replaces` on `bitmap`, `model`, `sound` and `animation` items: the
  manifest takes it and their builders refuse it by name.
- That a mod's patch reaches the game on a running level under
  `store-maps`, and that a refused mod is off where a player expects to
  see the reason.
- That a film recorded on an assembled level seeks and verifies as one
  recorded on a disc map does.

## Related

- [tag-repo.md](../formats/tag-repo.md), [tag-schema.md](../formats/tag-schema.md), [map-builder.md](../formats/map-builder.md), [map-format.md](../formats/map-format.md): the store's files, layouts, the builder, the cache file
- [tag-workspace.md](../editor/tag-workspace.md), [studio-redesign.md](../editor/studio-redesign.md): the workspace and packages this page builds on
- [maps.md](../game/maps.md), [map-pack.md](../game/map-pack.md): map packages, `MapId`, community mods
- [cache-slots.md](../runtime/cache-slots.md), [storage.md](../runtime/storage.md): the guest's copy of a map and the data root
- [pogo-stick.md](../game/pogo-stick.md), [forge-design.md](../game/forge-design.md), [forge-monitor-appearance.md](../game/forge-monitor-appearance.md): the content paths it simplifies
- [native-renderer.md](../graphics/native-renderer.md), [framescene.md](../graphics/framescene.md): resources by tag datum
- [film-format.md](../game/film-format.md), [live-api.md](../live/live-api.md), [dedicated-host.md](../live/dedicated-host.md): identity in films, sessions and hosts
- [live-mirror.md](../editor/live-mirror.md), [hot-iteration.md](../graphics/hot-iteration.md): edits on a running game
