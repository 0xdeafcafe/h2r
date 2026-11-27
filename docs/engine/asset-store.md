# The asset store: levels assembled from shared assets

Status: Design. The tag repository, the package build and the per-map rewrite it stands on are built ("What exists"), and so are slices AS1 to AS4, AS6 and AS7, and the store side of AS10 (a converter's map kept as a level of the store): the skeleton and extent table, `h2 tags reassemble`, the store-backed map file behind `store-maps`, extraction on compile, level images kept by `LevelId`, mod patches with the conflict table, and their Studio side (fork, revert, diff, **Changed by**, the conflict rows in Problems), proven on invented maps; the disc runs of AS1 to AS4 are not done ("Not verified"). The normal forms AS5 hashes with are `tags::repo::normal`, here for `was` and `replaces`, and the catalogue `tags::repo::catalogue` (`assets.idx`) is written by every extraction and read by Studio's asset browser. "Decisions" lists the owner's choices.
Code: `crates/formats/tags/src/repo` (the store's files and extraction; `repo/skeleton.rs` the skeleton, the extent table and `StoreFile`; `repo/compile.rs` extraction on compile), `crates/apps/cli/src/store_prepare.rs` (the CLI's side of it, with a line for the catalogue it wrote), `crates/apps/studio/src/app/assets.rs` (the browser's rows, from the catalogue), `crates/runtime/vfs/src/store_map.rs` (the map file served from the store), `crates/apps/boot/src/store_status.rs` (`store.status`), `crates/tools/workspace/src/level.rs` (level images), `crates/tools/workspace/src/converted.rs` (converted levels), `crates/formats/content/src/id.rs` (`LevelId`), `crates/formats/content/src/package/` (packages and mods), `crates/tools/workspace/src/{build,edit,delta,onto,load,assets}.rs` (the conflict table, `from`, `replaces`, `tag-delta`, the store's bytes, the asset hashes a build checks against), `crates/apps/boot/src/packages.rs` (the game's side). Tests: each slice names its own.
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
| Packages applied to a map the game reads | `workspace::PackageRewrite` over `workspace::build_map_streamed`: the map read by offset, rebuilt onto its pinned tags with its raw data and BSP regions left on the disc, and kept as an overlay under `<data>/packages/cache/` | built |
| A map file served from something other than a file | `vfs::packages` (byte runs over a base as it streams), `vfs::sim_store` (a map served from recorded ranges, byte for byte at the same offsets) | built |
| A stale cache slot dropped when its source changes | `crates/extensions/game/src/map_slots.rs` ([cache-slots.md](../runtime/cache-slots.md), "Changed maps") | built |
| Live edits of a loaded map's tags | `crates/extensions/hot-tags`, `edit.batch` ([live-mirror.md](../editor/live-mirror.md)) | built |
| A level's identity on Live and in a film | `content::MapId`, the content set of `PackageId`s, a film's `DESC` map digest ([maps.md](../game/maps.md), [film-format.md](../game/film-format.md)) | built, the content set's session check is PK14 |
| A map given back from the store byte for byte | `maps/<map>.skel` and `.extents`, `tags::repo::StoreFile`, `TagRepo::check_reassembly` at extraction, `h2 tags reassemble` ([tag-repo.md](../formats/tag-repo.md)) | built (AS1); not run on the disc |
| The disc's maps served from the store | `vfs::store_map::StoreMapBackend` under `store-maps`, `store.status` | built (AS2); no boot from the store yet |
| The store kept current by `h2 build`, the pre-boot step and Studio | `tags::repo::compile::prepare`, `cli::store_prepare` | built (AS3); not run on the disc |
| A layered level as an image kept by its `LevelId` | `workspace::level::{assemble, StoreLevels}`, `content::LevelId`, `StoreMapBackend::with_levels`, `<data>/store/images/` | built (AS4); not run on the disc |
| An asset's content whatever map it was read from, and the store's assets by that hash; Studio's **In maps** and **Where used** | `tags::repo::{normal, catalogue}`, `assets.idx`, `h2 tags catalogue`, `asset.list`'s `hash`, `variants` and `used` | built (AS5); the disc's variant counts are not measured |
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

The catalogue (`assets.idx`) is written by the same call, and
`cli::store_prepare` says in a line what it holds.

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
| An overlay per map a package touches (`<data>/packages/cache/`), every multiplayer map for the Pogo | the header, tables, index and metadata of the maps touched | one image of 5 to 15 MB each |
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
  map or a Map Pack level) is kept by `workspace::converted::store_converted`. Its tags go
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
  unit runs again. The multiplayer list and campaign picker use verified
  converter markers. Halo 1 sandbox and asset choices select distinct store
  stems. Campaign manifests, HUD data and pictures live under
  `<data>/campaigns/halo1/`. Multiplayer list metadata is under
  `<store>/halo1-mp/`. The VFS serves the selected level from the store.
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

In [asset-store-packages.md](asset-store-packages.md).

### Identity on the network and in films

In [asset-store-packages.md](asset-store-packages.md).

- **Conflicts.** In [asset-store-packages.md](asset-store-packages.md).

- **In Studio.** In [asset-store-packages.md](asset-store-packages.md).

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

In [asset-store-plan.md](asset-store-plan.md).

## Build slices

In [asset-store-plan.md](asset-store-plan.md).

## Decisions

In [asset-store-plan.md](asset-store-plan.md).

## Evidence

In [asset-store-plan.md](asset-store-plan.md).

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
