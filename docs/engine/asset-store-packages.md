# The asset store: mods, packages and identity

Status: Design (see asset-store.md for what is built).
Part of [asset-store.md](asset-store.md): how packages lay over the store, how a conflict resolves, and how a level is named on the network and in films.

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


## Related

- [asset-store.md](asset-store.md): the page this was split from.
