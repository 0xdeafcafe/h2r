# The asset store: risks, slices, decisions and evidence

Status: Design (see asset-store.md for what is built).
Part of [asset-store.md](asset-store.md): what is proven first, the build slices, the decisions taken and the evidence they rest on.

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
| AS10 | **Converted and imported levels in the store.** Halo 1 and Map Pack converters store their levels as conversion units. Halo 1's campaign and multiplayer maps are served from the store. | AS4 | `workspace::converted`, `cli::h1_import`, `cli::mappack_cmd`, [multiplayer.md](../halo1/multiplayer.md), [map-pack.md](../game/map-pack.md) | invented levels round-trip, a store reset invalidates converter output, and the VFS serves the selected sandbox | a converted level loads from the store and the note records disk usage |
| AS11 | **The renderer on asset hashes.** The image's datum table; loaders keyed by asset hash; a model read from the store by key; the Monitor's catalogue by `(key, asset hash)` | AS4, AS5 | `crates/rewrites/native-render/src/{model_load.rs, models.rs, load.rs}`, `crates/extensions/forge/src/look/census.rs`, `crates/apps/cli/src/forge_looks.rs`, [native-objects.md](../graphics/native-objects.md), [forge-monitor-appearance.md](../game/forge-monitor-appearance.md) | an invented model shared by two levels uploads once across a level change; a model in no loaded level resolves from the store | a campaign body draws on a multiplayer level with no other map file opened |
| AS12 | **Forge on the store.** A Forge document as a layer; the materialised map and its package removed | AS4 | `crates/tools/forge-build`, `crates/formats/content/src/forge.rs`, `crates/extensions/forge`, [forge-design.md](../game/forge-design.md) | the image for an invented document equals the materialised map's tags | a saved Forge variant loads with no `.map` written |
| AS13 | **Bundles from the store.** The host data bundle and the device packers (`h2host::layout`) take images and a subset of the store | AS4 | `crates/live/host-data/src/bundle`, `crates/apps/h2host/src/layout.rs`, [dedicated-host.md](../live/dedicated-host.md) | an invented bundle serves every range its coverage names and refuses one it lacks by name | a host plays a match from a bundle built with no disc image mounted |
| AS14 (later) | **Retail levels built whole.** With W10, a retail level assembles `fresh` and its skeleton is dropped | W10 | as W10 | as W10 | a retail level with no skeleton loads |
| AS15 (later, **E first**) | **No slot copy.** The guest's slot answered as warm and its reads routed to the store file | AS2; a read of the status and resource-read functions | `crates/extensions/game/src/map_copy.rs` | the kernel cache-slot test with the route on | a level load copies nothing and `<data>/disk/` shrinks |

AS1 to AS4, AS6, AS7 and AS10 are built. The disc checks of AS1 to AS4 are the
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


## Related

- [asset-store.md](asset-store.md): the page this was split from.
