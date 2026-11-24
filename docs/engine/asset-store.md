# The asset store: levels assembled from shared assets

Status: Design. The tag repository, the package build and the per-map rewrite it stands on are built ("What exists"); the skeleton, the store-backed map file, level images, mod patches and `LevelId` are not. "Decisions" lists what the owner has to choose.
Code: `crates/formats/tags/src/repo` (the store's files and extraction), `crates/runtime/vfs/src/store_map.rs` (new: the map file served from the store), `crates/tools/workspace/src/level.rs` (new: level manifests and images), `crates/formats/content/src/package/` (packages and mods), `crates/apps/boot/src/packages.rs` (the game's side). Tests: each slice names its own.
Switch: `store-maps` (a feature, on once slice AS2 is accepted), to be listed in [switches.md](../runtime/switches.md).

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

What is missing: the store cannot give a map back exactly (`sbsp`,
`ltmp` and `scnr` are lossy in it), the game reads maps from the disc
and from whole-map caches, an asset's identity differs between maps for
reasons that are not content (a sound's indices into its map's sound
table), and a mod can only add tags or set fields.

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
<store>/maps/<map>.skel             its skeleton (new)
<store>/maps/<map>.extents          its extent table (new)
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
  resources, at their offsets: the header, the BSP regions, the tag
  index and the tag metadata. `maps/<map>.skel` holds them, compressed;
- its **extent table**: every other byte range of the file as
  `(offset, length, raw hash)`. The extractor's raw starts already tile
  the raw region; a range no tag names is an extent of its own.

The rule the skeleton and the extents keep: together they give the disc
map back byte for byte. The extractor checks this for every map against
the file hash in `extract.toml` before it records the map
(`TagRepo::reassemble`, new). The skeleton is what makes the store whole
while the level tags are lossy: nothing the schema cannot describe is
lost, because it is still there as the disc's bytes.

**Forks.** A fork is a change with a map scope. It is never a copy in
the store: it is an item of a package whose `maps` names one map
("Mods and packages"). Studio's **Fork for this map** (`asset.fork`,
new) writes that item into the edit target. A change with no map scope
applies in every level that names the asset. **Copy into package**
(`asset.copy`, built) makes a new asset under a new key, which a level
uses when a reference is pointed at it.

### Extraction on compile

`h2 build` extracts. After the game image is built it runs
`tags::repo::extract::update` on the disc it was given, into the store,
with progress on stderr, and writes the catalogue. `h2 play`, `h2 run`
and `h2 serve` run the plan before boot, beside
`package_prepare::before_boot`; with nothing stale the plan opens no map
(`extract::plan`) and costs a stat of each map file, since a map's
recorded hash is trusted while its size and modification time match
(the stamp rule of `workspace::unit_key`). A stale store extracts before
the boot, with a progress line; the app shows it on its first-run
screen. Studio's first start is the same call (`workspace.extract`).

- **Incremental.** The plan's cases are the built ones: a new or changed
  map file is taken whole, a group whose layout hash moved re-extracts
  that group's tags only, an extractor or format version change takes
  everything, and a stopped run redoes one map.
- **From the store itself.** Because the skeleton and extents give each
  map back exactly, a re-extraction after the first reads the maps from
  the store (`MapSource` over `store_map::StoreFile`) when the disc is
  not there. The disc image is needed once.
- **The rest of the disc.** The executable, the movies and the font
  files are not assets. The first extraction copies them under
  `<data>/disc/` (about 560 MB, **V** by the directory of the owner's
  image), so a boot needs no disc image at all. Decision 1.
- **Other discs.** The Map Pack's maps and the converted Halo 1 levels
  enter the store through the same `store_map`, as the output of their
  `convert` items ("What it replaces").
- **Agents and worktrees** share the one store read-only. A lane never
  extracts into a path of its own.

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

**A level with layers** (a package item whose `maps` takes it) gets an
image from `workspace::level::assemble` (new):

1. Resolve the layers for the level: the store read for that map, then
   each package of the set in load order ("Mods and packages").
2. If the image cache holds the level's `LevelId` ("Identity"), use it.
3. Otherwise run the built per-map build (`workspace::build_map`) on
   the skeleton as a decomposed map: every tag pinned
   (`TagSet::from_map`), the tags the layers change replaced, new tags
   added, the per-map couplings merged (the sound table, the low-detail
   ranges). Raw resources stay names: a new or changed resource is
   stored in `raw/` by hash and gets an extent past the end of the
   file; nothing already in the store is rewritten.
4. Write `<LevelId>.img` and `<LevelId>.extents`. The header checksum
   covers the logical file and is made from a checksum kept per raw
   file, so the build reads no raw bytes (**S**: that the retail
   checksum composes over extents at their alignment; AS4 proves it on
   invented maps and against the disc's own header values).

This is what `PackageRewrite` does on first read of a map, less the
copy of the map's raw region and the whole-map cache file.

- **Load time.** A cached image costs nothing extra: the guest's copy
  into its slot paces the load, as it does for a disc map
  ([cache-slots.md](../runtime/cache-slots.md), "Copy speed on the
  loading screen"). A missing image costs one `onto` build of the
  level's metadata, which runs at prepare (`boot::packages::prepare_maps`)
  for the maps of the current set, not on the loading screen. The
  build's incremental cache ([tag-workspace.md](../editor/tag-workspace.md),
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
  `TagSet::check_tag_memory` refuses an image over the retail bound for
  its type and names the assets that grew it
  ([scenario-editor.md](../editor/scenario-editor.md), section 6).
- **Levels built whole.** A level of ours (the stage, a converted Halo 1
  map) has no skeleton: its image is a `fresh` build from the manifest's
  closure. A retail level builds that way once its level tags extract
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
| A field patch | `edits` (built) | key, and per line the field path with `from`, the value it replaces (new) | our values only |
| A structural patch | `edits` `block` lines (built) | key and `was`, the asset hash of the base | the entries added |
| A whole-asset override | any tag-making item (`tag`, `model`, `bitmap`, `sound`, `animation`) with `replaces = "<asset hash>"` (new) | key and that hash | an authored source: YAML, PNG, WAV, glTF |
| A delta | `tag-delta` (new) | key and `was` | a copy-and-insert delta against the base's normal form: the new bytes and ranges of the old, as a BPS patch holds |
| A new asset | `tag`, `model`, `bitmap`, `sound`, `animation`, `screens`, `hud`, `rules` (built) | a key no layer below has | the source |
| A whole-level patch | `map-patch` (built) | the base map's SHA-256 | a PPF, IPS or BPS file |

Naming a target by key and base hash is what lets a mod be shared: the
file says which of the user's own assets it changes and holds only the
change. `from` on a `set` line checks one field, so two mods that set
different fields of one asset both apply, and a line applies to each
map's variant of the asset in which the field has that value.

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
unchanged everywhere else.

**Conflicts** (`workspace::level::assemble`, reported in
`Report::overrides` and `Report::refused`):

| Case | Result |
|---|---|
| Two packages set different fields of one asset | both apply |
| Two packages set the same field | the later package's value; the override is listed with both names |
| A `from`, `was` or `replaces` that does not match what the store holds | the item is for other content (another pressing, a changed base): the package is off for that level with the reason |
| A package patches an asset an earlier package overrides whole | applies only when the patch's `was` names the override's hash and the package `requires` the other; otherwise the later package is off, naming both |
| Two packages override the same asset whole, or add the same key | the later package is off, naming both |
| A patch whose key is missing | off, with the key |

A package is never loaded in part: one refused item puts the whole
package off for the level, with a line in `packages.list` and Problems.
A first-party package (`content/`) that refuses stops the build
instead, as `h2 package check` does.

**Enable and disable.** `set.toml` holds what is off and the order
(`package.enable`, `package.order`). The store is not touched by any
mod, so nothing is extracted again; a level's `LevelId` moves, the
image for the other set is still in the cache, and the change shows at
the next load of the level.

**In Studio.** The Packages root lists the set; a package opens as a
document (built). New for mods:

- `package.diff {id, map?}`: per asset, the fields before and after,
  the assets added, overridden and forked, and the levels touched;
- the inspector's **Changed by** row names every package with a patch
  on the asset in front, in order, with the winning value;
- `package.check` runs the conflict table against the set and refuses a
  file whose SHA-256 is a raw resource or a tag file of the store, so
  the user's own store is the retail list on their machine;
- `asset.fork {tag, map}` and `asset.revert {tag, map?}` add and remove
  scoped items in the edit target;
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

`assets` is the manifest after layering, sorted by key; `packages` is
the ordered content set for the level; `builders` is
`workspace::BUILDERS_VERSION`. Assembly is deterministic, so two
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
| `h2 build`, `h2 play`, `h2 run`, `h2 serve`, Studio | `tags::repo::extract::update`, then `store::catalogue::write` | the plan, the extraction, the catalogue |
| The VFS | `vfs::store_map::StoreFile::{len, read_at}` | a level's bytes from an image or a skeleton and the raw files |
| `boot::packages::prepare_maps`, Studio's `package.build` | `workspace::level::assemble(store, set, level) -> Level {id, image, extents, report}` | the image for a level under a set |
| The game's agent API | `packages.list` (built), `store.status` (new): the store's path, maps, sizes, images, what is stale | read |
| Studio's API | `workspace.status`, `workspace.extract`, `workspace.prune` (built); `asset.fork`, `asset.revert`, `package.diff`, `package.import` (new) | above |
| `h2 tags` | `extract --update` (built), `reassemble <map> -o FILE`, `level <map> [--set FILE]` (new) | the coordinator's and CI's entry |

## Rules and limits

- The store, the layers and the images are game data under the data
  root. `workspace` refuses a root inside a Git checkout. No hash, list
  or count of the store's contents is committed.
- The store is written by the extractor only. A change is always an
  item of a package.
- A package holds no disc bytes: `package.check` and Live's upload both
  refuse a blob whose hash is retail.
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
7. **The checksum over extents** (step 4 of assembly) is supposed.
8. **A wrong store.** A store extracted from a damaged or patched image
   makes a level no peer shares. `LevelId` catches it at the join and
   the mismatch names the assets.

**The first proving slice is AS1 with AS2**: the skeleton and extents,
`reassemble` equal to the disc for all 30 maps, and the game booting a
retail level through `vfs::store_map` with the disc's maps hidden. It
changes no tag and no identity, needs no builder work, and settles
risks 1 and 6 and the real size of the store.

## Build slices

One lane each; a lane writes code and tests and the coordinator runs
them. Tests are headless on invented maps unless marked disc
(`#[ignore]`). Most valuable first, within what each needs.

| # | Slice | Needs | Files | Tests | Accepted when |
|---|---|---|---|---|---|
| AS1 | **Skeleton and extents.** The extractor writes `maps/<map>.skel` and `.extents`; `TagRepo::reassemble`; the plan re-extracts a map lacking them; `h2 tags reassemble` | nothing | `crates/formats/tags/src/repo/{skeleton.rs (new), extract.rs, store.rs, tests.rs}`, `crates/apps/h2-tools/src/tag_repo_cmd.rs`, [tag-repo.md](../formats/tag-repo.md) | an invented map with shared pointers, an unnamed raw range and two BSPs reassembles to its bytes; a missing raw file is a named error; disc: all 30 maps reassemble to their SHA-256 | the disc test passes and prints the store's size by part |
| AS2 | **The map file from the store.** `vfs::store_map` (length, ranged reads, a level by name), wired into boot's backend chain under `store-maps`; `store.status` | AS1 | `crates/runtime/vfs/src/{store_map.rs (new), lib.rs}`, `crates/runtime/vfs/tests/store_map.rs` (new), `crates/apps/boot/src/lib.rs`, `crates/extensions/agent` (`store.status`), [agent-api.md](../runtime/agent-api.md), [switches.md](../runtime/switches.md) | reads across extent borders equal the reassembled bytes at every offset class; a store lacking a level falls to a named error, never to another file; `map_changed` fires when a level's extents change | a retail level boots and plays from the store with the image's maps hidden, and its film's map digest equals the disc run's |
| AS3 | **Extraction on compile.** `h2 build` and the pre-boot plan call `extract::update`; the stat fast path; the non-map files copied to `<data>/disc/`; re-extraction from the store; progress | AS1 | `crates/apps/cli/src/{build_cmd, package_prepare.rs}`, `crates/formats/tags/src/repo/extract.rs`, [building.md](../runtime/building.md), [storage.md](../runtime/storage.md) | unchanged input opens no map; a touched map file is hashed once and taken alone; a moved layout hash re-extracts from the store with no disc; an interrupted run resumes | `h2 build` on a fresh data root leaves a store that AS1's disc test accepts, and a second run does nothing |
| AS4 | **Level images.** `workspace::level::{assemble, LevelId}`: `build_map` on the skeleton, raw as extents, the checksum from per-file sums, the image cache and its cap; `PackageRewrite` and `<data>/packages/cache/` removed | AS2 | `crates/tools/workspace/src/{level.rs (new), rewrite.rs, build.rs, game.rs}`, `crates/formats/content/src/id.rs`, `crates/apps/boot/src/packages.rs`, [studio-redesign.md](../editor/studio-redesign.md) | the image and extents read back equal `build_map`'s whole map byte for byte; golden `LevelId`s on invented hashes; a set change gives another id and the first image is still a hit; an image over the tag memory bound is refused naming the assets | a multiplayer level with the Pogo loads from an image and `<data>/packages/cache/` is not written |
| AS5 | **Normal forms and the catalogue.** `tags::repo::normal` for sounds, bitmaps and padded raw; the asset hash; `assets.idx`; Studio's In maps and Where used from it | AS1 | `crates/formats/tags/src/repo/{normal.rs (new), catalogue.rs (new)}`, `crates/tools/workspace/src/hash.rs`, `crates/apps/studio/src/app/assets.rs`, `docs/formats/notes/` (the variant counts) | two invented maps with one sound at different table indices give one asset hash and another when a sample differs; the catalogue rebuilt from manifests equals the one kept | the note records the disc's variant counts by group before and after |
| AS6 | **Mod patches.** `from` on `set` lines, `replaces`, the `tag-delta` item, the conflict table, off-with-reason for installed packages, the retail refusal against the store | AS4, AS5 | `crates/formats/content/src/package/{manifest.rs, set.rs}`, `crates/tools/workspace/src/{edit.rs, delta.rs (new), level.rs, builders.rs}`, `crates/apps/h2-tools/src/package_cmd.rs`, [studio-redesign.md](../editor/studio-redesign.md), [maps.md](../game/maps.md) | one test per row of the conflict table; a delta applied to its base gives the changed tag and to another base is refused; a package holding a store blob is refused by file name | two invented mods on one weapon's different fields both show in the assembled level |
| AS7 | **Mods in Studio.** `package.diff`, `asset.fork`, `asset.revert`, **Changed by** in order, the conflict rows in Problems | AS6 | `crates/apps/studio/src/{api/packages.rs, api/assets.rs, app/package_doc.rs, ui/browser.rs}`, `studio-proto`, [agent-api.md](../editor/agent-api.md) | every method's example; a fork changes one level's `LevelId` and no other's; revert removes the item and its line | a field forked for one map shows there and nowhere else, in the window and through the API |
| AS8 | **Identity on Live and in films.** `level_id` in the session and the join check with the asset diff (with PK14); `DESC.level_id`; `prepare_map` by `LevelId`; film-named images kept; the cross-platform determinism test | AS4; PK13 for Live | `crates/live/live-proto`, `crates/live/live-hle/src/maps.rs`, `crates/extensions/game/src/theater/metadata.rs`, `crates/live/film/src`, [film-format.md](../game/film-format.md), [live-api.md](../live/live-api.md) | on the fake server a joiner with another asset fails before the map load naming it; a film recorded on an assembled level verifies, and after a builder bump plays from its kept image | two machines of different architectures print the same image hash for one `LevelId` |
| AS9 | **Mod import.** `package.import` and `h2 live mods import` write tag-level packages through the store, `map-patch` as the fallback | AS6 | `crates/tools/workspace/src/import_map.rs` (new), `crates/apps/cli/src/live_cmd`, `crates/apps/studio/src/api/packages.rs`, [maps.md](../game/maps.md) | an invented patched map imports to a package that assembles to the same tags; a patch touching the scenario's lossy part stays a `map-patch` and says so | two imported mods of one base map load together |
| AS10 | **Converted and imported levels in the store.** The `convert` items store their output with `store_map`; a level with no skeleton assembles `fresh`; `<data>/maps/` derived packages and `<data>/campaigns/` whole maps removed | AS4 | `crates/tools/workspace/src/{convert.rs, maps.rs, level.rs}`, `crates/apps/cli/src/{h1_import.rs, mappack_cmd.rs}`, [multiplayer.md](../halo1/multiplayer.md), [map-pack.md](../game/map-pack.md) | an invented converted level stores only tags the store lacks and assembles to the map the converter wrote | a converted level loads from the store; the note has the disk size before and after |
| AS11 | **The renderer on asset hashes.** The image's datum table; loaders keyed by asset hash; a model read from the store by key; the Monitor's catalogue by `(key, asset hash)` | AS4, AS5 | `crates/rewrites/native-render/src/{model_load.rs, models.rs, load.rs}`, `crates/extensions/forge/src/look/census.rs`, `crates/apps/cli/src/forge_looks.rs`, [native-objects.md](../graphics/native-objects.md), [forge-monitor-appearance.md](../game/forge-monitor-appearance.md) | an invented model shared by two levels uploads once across a level change; a model in no loaded level resolves from the store | a campaign body draws on a multiplayer level with no other map file opened |
| AS12 | **Forge on the store.** A Forge document as a layer; the materialised map and its package removed | AS4 | `crates/tools/forge-build`, `crates/formats/content/src/forge.rs`, `crates/extensions/forge`, [forge-design.md](../game/forge-design.md) | the image for an invented document equals the materialised map's tags | a saved Forge variant loads with no `.map` written |
| AS13 | **Bundles from the store.** The host data bundle and the device packers (`h2host::layout`) take images and a subset of the store | AS4 | `crates/live/host-data/src/bundle`, `crates/apps/h2host/src/layout.rs`, [dedicated-host.md](../live/dedicated-host.md) | an invented bundle serves every range its coverage names and refuses one it lacks by name | a host plays a match from a bundle built with no disc image mounted |
| AS14 (later) | **Retail levels built whole.** With W10, a retail level assembles `fresh` and its skeleton is dropped | W10 | as W10 | as W10 | a retail level with no skeleton loads |
| AS15 (later, **E first**) | **No slot copy.** The guest's slot answered as warm and its reads routed to the store file | AS2; a read of the status and resource-read functions | `crates/extensions/game/src/map_copy.rs` | the kernel cache-slot test with the route on | a level load copies nothing and `<data>/disk/` shrinks |

AS1, AS3 and AS5 run together; AS2 follows AS1; AS4 follows AS2; AS6,
AS8, AS10, AS11, AS12 and AS13 follow AS4 side by side.

## Decisions

Each is the owner's. The recommendation is first.

1. **Is the disc image needed after the first extraction?**
   (a) No: the store reassembles every map, and the executable, movies
   and fonts are copied to the data root. (b) Yes: the store is a cache
   and a boot still opens the image. Recommended: (a). It is what makes
   the store smaller than what it replaces.
2. **Where extraction runs.** (a) `h2 build`, and the pre-boot plan as a
   check that extracts when stale. (b) Studio's first start only, the
   game using the disc until then. Recommended: (a), "on compile" as
   asked, with Studio calling the same code.
3. **What an edit in Studio touches by default.** (a) Every level that
   names the asset, with **Fork for this map** as the explicit other
   choice. (b) The open level only, with "apply everywhere" explicit.
   Recommended: (a).
4. **When two mods collide.** (a) Different fields both apply, the same
   field goes to the later package and is listed, and a structural
   clash puts the later package off with the reason. (b) Any clash
   stops the load. (c) The later package always wins. Recommended: (a).
5. **What a shared mod may hold.** (a) Field patches, authored
   overrides, new assets and `tag-delta` items, with any blob matching
   a retail hash refused. (b) No deltas: only fields and authored
   sources. Recommended: (a). A delta stands where a `map-patch` stands
   already, and imported community mods need it.
6. **Community mods already imported.** (a) Convert to tag-level
   packages at import, `map-patch` when a mod cannot be said that way.
   (b) Leave them whole-map patches. Recommended: (a); nothing is
   published, so nothing is migrated.
7. **What gates a session, and old films.** (a) `LevelId` gates joins
   and names a film's level; `MapId` and `PackageId` stay the names of
   things on Live; images a saved film names are kept. (b) Keep
   `map_id` plus the content set, and refuse a film after a builder
   change. Recommended: (a).
8. **Client-only assets in `LevelId`.** (a) Every asset counts until a
   lockstep run shows a client asset cannot move the simulation.
   (b) Leave client assets out from the start. Recommended: (a).
9. **Converted Halo 1 levels.** (a) Into the store after AS4 (AS10).
   (b) Leave them whole maps. Recommended: (a): on the owner's machine
   they are 4.3 GB of whole maps.
10. **Raw resources on disk.** (a) One file per resource, as built,
    with a pack file per level only if AS2 measures the reads as slow.
    (b) Pack files from the start. (c) Compressed. Recommended: (a).

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
- `LevelId`, the conflict table, `from`, `replaces`, `tag-delta`, the
  image and the extent shim are **Design**.

## Not verified

- That the header checksum of a level can be made from per-file sums.
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
