# The editor's agent API: workspace, browser and packages

Status: Built. Code: `crates/apps/studio/src/api`, `crates/apps/studio-proto/src`. Tests: in-source unit tests.
Switch: `H2_STUDIO` (the socket path).

The tag workspace, projects, the Browser, Forge variants, packages, problems and the package-building methods. The index is [agent-api.md](agent-api.md).

## The tag workspace

The tags tree (docs/editor/tag-workspace.md): every tag of the disc as
loose files under the data root, what every map is built from.
`workspace.status` reports it: the root (the `tags_root` setting,
`<data>/tags/` unless set, saved with the window layout), the maps
extracted with their lossy and refused counts, the plan's stale parts,
an extraction in progress and what the last one did, the sizes of
everything the workspace holds (the tree, each layer under
`<data>/workspace/`, each project's build cache, each package under
`<data>/maps/`), and `start_page`, whether the first start's window is
showing. `workspace.extract {disc}` runs the plan: `disc` is an image, a
`.map` file or a folder of maps (the recorded disc, `H2_DISC` or
`disc/halo2.iso` when not given), `maps` limits it to those, `verify`
reads every stored file back, and `root` sets the tree's root - refused
inside a Git checkout, for the tree is game data and never sits in a
repository. It answers at once with the maps it will read; the progress
is in `workspace.status` and the window's status line, and work
continues while the tree fills. An unchanged disc is a no-op, and a
later start of the editor runs the plan against the recorded disc in the
background. `workspace.prune {dry_run}` removes raw resources no tag
file or manifest names, generated layers no project asks for and build
caches unused for a week - with `dry_run`, what would go; the outputs of
deleted projects are the projects' and are not pruned. The tree itself
is never pruned in part: it is deleted whole and extracted again.
`workspace.import` (planned) stores a map as an import layer (W9).

The window's first start - no tree, no source open other than a disc
image - is one step: choose the disc (`H2_DISC` and `disc/halo2.iso`
offered), see the size and the time, extract every map with the status
line's progress bar. It makes the same calls.

```sh
h2 studio workspace
h2 studio workspace.extract '{"disc":"/Volumes/halo2.iso"}'
h2 studio workspace
h2 studio workspace.prune '{"dry_run": true}'
```

The first start and the plan run headless on an invented map
(`app::workspace::tests`: a root inside a Git checkout refused, the
background plan extracting a changed disc, the sizes walked, prune
keeping what a tag file or a manifest names); the API's tests
(`api/tests.rs`) do the same through the dispatcher, and every method's
example runs in `every_method_in_methods_has_a_test`.

## Projects and edit files

A project is a package's `map` item: one map to build from the tree, its
import layers, the authored layer (the `tag` items the map takes), its
edit files, its kits and the other items it takes
(docs/editor/tag-workspace.md, "Projects"; code
`workspace::project`, `workspace::project_build`, `api/projects.rs`).
Each call reads the files again. The roots are the `tags_root` setting,
the data root and the checkout Studio was built from.

| Method | Does | Answers |
|---|---|---|
| `project.list` | every `map` item of the packages `content/*/` in the checkout, then `<data>/packages/*/` | `{projects: [{name, path, origin, package, item, mode, base, error}]}` |
| `project.read {project, closure?}` | resolves a project (a map's name, or a package directory with one map item) without building it or writing anything | `{project, file, package, item, origin, layers, authored, edit_files, edited, kits, scenario_from, lossy, problems}` |
| `project.build {project}` | builds and writes the map package `<data>/maps/<name>/` with `receipt.json`; the authored layer is kept under `<data>/workspace/generated/` | `{dir, map_id, file, unchanged, took_ms, receipt}` |
| `edits.list` | every `assets/tag-edits/*.yaml`, then `<data>/workspace/edits/*.yaml` | `{files: [{name, origin, path, entries, tags, problems}]}` |
| `edits.check {project}` | applies every edit of the project once over its layers | `{project, files, edited, problems: [{file, line, tag, why}], overrides}` |
| `edits.set {file, tag, path, value, project?}` | writes one `set` line into local file `file`; `project` records the tag's hash as `was` | `{file, path, line, replaced, was}` |

`problems` in `project.read` are what would stop the build: a missing
tree, base map, import layer or edit file, an authored tag a lower layer
holds or two items add, an edit that does not apply, a kit this build
lacks, a `ground` file. A package that does not load stops every call
that loads a project, named. `lossy` (with `closure`, a fresh project whose
scenario is in the layers) lists the closure's tags the tree does not
hold exactly. `project.build` fails (`FAILED`) with the same words; an
unknown project is `INVALID_PARAMS`; `edits.set` refuses (`REFUSED`) a
dead path, a value the field does not take and an entry made on another
hash.

```sh
h2 studio project.list
h2 studio project.read '{"project":"stage","closure":true}'
h2 studio edits.set '{"file":"tuning","tag":"jpt!:objects\\weapons\\rifle\\damage","path":"damage_lower_bound","value":12.5,"project":"stage"}'
h2 studio edits.check '{"project":"stage"}'
h2 studio project.build '{"project":"stage"}'
```

The same calls are `h2 tags project list|show|build` and `h2 tags edits
list|check|set`. Tests: `workspace`'s `edits_tests.rs` and
`project_tests.rs` (invented trees), and
`projects_and_edit_files_resolve_over_an_invented_tree` in
`api/tests.rs`; every example runs in
`every_method_in_methods_has_a_test`, where `project.build` and
`edits.check` answer that there is no tree.

## The browser

`asset.*` is the browser's data (docs/editor/studio-redesign.md, "The
browser"; `app/assets.rs`). An asset is named by an id: `tag:<group>:<path>`
(a tag of the tree, or one a package adds), `package:<id>` or
`item:<package>:<n>` (the n-th item of a package's manifest), `voice:<id>`
or `forge:<map>:<file>` (a Forge map variant).

`asset.list {root?, package?, kind?, map?, query?, limit?}` answers
`{rows, total, limit, truncated, maps, kinds, problems}`. With `root`
`tags` (the default) a row is one tag path of the tags tree. A tag every
map holds alike is one row with `variants: []`; where maps hold different
contents the row's `variants` lists each with its `maps`. While the
store's catalogue (`assets.idx`) is current, a content is an asset by
asset hash (`hash` on a row with one; `hash`, `files`, `variant` and
`maps` on each entry of `variants`), so two files that differ only in
what ties them to their map are one content, and `used` names the keys
whose tags reference the tag. A tag the catalogue could not hash has no
asset and its files are its contents. Without a catalogue, or with a
stale one (a line in `problems`), a content is a file and an entry of
`variants` is
(`variant`, null for the main file, and its `maps`). A tag a package
adds is in the same list
(`new: true`, `origin: "package"`, badge `new in <package>`); a tag an
edit line of a package on touches has badge `edited by <package>` and
the packages in `packages`. `kind` is one of `kinds` (Maps, Objects,
Models, Animations, Bitmaps, Shaders, Effects, Sounds, Screens, HUDs,
Strings, Other, Voices) or a tag group; the kind `Voices` lists a
`voice:<id>` row for each character and the announcer, with `state` (`no
dataset`, `dataset`, `fine-tuned` or `announcer`), `minutes`,
`pending_asr` and `tuned`; `map` keeps what that map's manifest
holds, with the tags a package adds for it; every word of `query` must
match the path, group, kind or a package id; `package` keeps what that
package adds or edits. `root` `packages` lists the set (baked, then
`<data>/packages/`) with on or off and why; with `package` it lists that
package's items. On item rows, `kind` is the Browser kind of what the
item builds (including **Game types** and **Playlists**), and `item_kind`
is its manifest kind. Use `kind` to filter those rows. `root` `live` answers `offline: true`: the server's
offer is not held. A missing tree is refused with the way to make it
(`workspace.extract`).

`asset.thumb {asset, out?}` writes a PNG and answers its path: the
picture a Browser row draws (`app/assets/thumbs.rs`).

| Asset | PNG | Answer |
|---|---|---|
| a package's `bitmap` item with a PNG source | the source as it is | `{asset, path, bytes, source}` |
| a bitmap tag (`tag:bitm:<path>`) the open map holds | image 0 at its base level, 64 pixels on its longer side | `{asset, path, bytes, width, height, source}`, `source` the map |
| a bitmap no open map holds; any other tag (a model through `model.render`, a waveform, a screen) | none | `not-yet` |

`asset.drop {file, package?}` is a source file dropped on the window
(`app/assets/drop.rs`). It answers `{file, kind, tag, package, front, log}`.

| File | `kind` | `tag` | As |
|---|---|---|---|
| `.wav` | `sound` | `sounds\<name>` | `import.sound`; opens the source tab |
| `.png` | `bitmap` | `bitmaps\<name>` | `import.bitmap` |
| `.yaml`, `.yml` | `model` | `models\<name>\model` | `import.model`; opens the source tab |

`<name>` is the file's name in lower case with `-` for every run of other
characters. With no `package`, the method uses the edit target or returns
`NO_TARGET`. Any other file is `INVALID_PARAMS` (`source.open` opens a
map); a file the import refuses is refused the same way.

`asset.copy {asset, package?, to?}` is **New from this...**. It writes
`tags/<path>.<group>.yaml` holding only `like: '<group>:<path>'`, adds a
`tag` item to the package's `package.toml` and rewrites its
`package.lock`. The package is one under `<data>/packages/` (a baked one
is refused); the new path is `to`, or the source's with `_copy`; a path
the tree or the package already has is refused. The reply gives the
new source tab's `item:` id as `doc`. With no `package`, the method uses
the edit target or returns `NO_TARGET`.

`asset.links {asset}` answers what the asset uses, what uses it and which
editors open it (docs/editor/studio-flow.md, "Links"; `app/links.rs`):
`{asset, uses, used_by, changed_by, editors, problems}`. A link is `{asset, role, via}`;
`role` groups it (Model, Animations, Sounds, HUD, Projectile, Variant,
Voice, Package, or the target's kind). A tag's `uses` are the tags its file
references (`via` is `node <n> +<offset>`) and, for a dialogue sound, the
voice that says it (`voice:<character>`); its `used_by` are the tags that
reference it and the packages that add or edit it; its `changed_by` is every
package with a patch on it in load order, each `{package, item, asset, change,
maps, forked, blocks, fields: [{path, value, from?, wins}]}` (`change` is
`edit`, `patch` for a `tag-delta` or `replaces`; `wins` is false for a field
a later package sets or an asset a later package overrides whole;
`app/mods.rs`). A `variant` item uses the
`rules` item of its script key, which lists the variant under `used_by`.
An item lists the tag it adds or edits and its package; a package lists its
items and `requires`. `editors` is `[{id, label, why_not}]`. A tag file that
does not decode, or a missing tree, is named in `problems`. A bad id is
`INVALID_PARAMS`; an unknown package or item number is `REFUSED`.

`asset.where_used {asset}` and `asset.edits {asset}` are the Browser's link
views (`app/assets/browse.rs`): the rows replace the Browser's list, as the
menu entries **Where used** and **Show edits** leave them, and the call
answers `{asset, view, rows: [{asset, role, via}], total, problems}`.

| Method | `view` | Rows |
|---|---|---|
| `asset.where_used` | `where_used` | `used_by` of `asset.links`, for any asset id |
| `asset.edits` on a tag | `edits` | each item with a patch on it in load order: `role` is `edit`, `patch` or `replaces`, `via` is where the item reaches then its lines (`x = 1`, `(overridden)` for a line a later package takes) |
| `asset.edits` on a package | `edits` | its `edits` and `tag-delta` items and the items that replace an asset, `via` the source |

`asset.edits` on an item or a voice is `REFUSED`; a bad id is
`INVALID_PARAMS`. A refused call leaves the view as it was.

```sh
h2 studio asset.list '{"kind":"Objects","query":"rifle"}'
h2 studio asset.list '{"root":"packages","package":"pogo"}'
h2 studio asset.drop '{"file":"/tmp/boing.wav","package":"my-pack"}'
h2 studio asset.copy '{"asset":"tag:weap:objects\\weapons\\rifle","package":"my-pack"}'
h2 studio asset.links '{"asset":"tag:weap:objects\\weapons\\rifle"}'
h2 studio asset.where_used '{"asset":"tag:hlmt:objects\\weapons\\rifle_model"}'
h2 studio asset.edits '{"asset":"package:pogo"}'
```

## Forge variants and items

`forge.*` is the Forge map variants and the Forge palette
(docs/editor/studio-flow.md, "Forge variants and items";
`app/forge_variants.rs`, `app/assets/forge.rs`, `api/forge_variants.rs`).

| A variant | Asset id | `source` |
|---|---|---|
| a `map-variant` item of a package that is on | `item:<package>:<n>` | `package` |
| a file saved in the game, `<data>/map-variants/<map>/<file>.h2mapvariant` | `forge:<map>:<file>` | `saved` |
| a file fetched from Live; listed, does not open | `forge:<map>:<file>` | `live` |

`shell.open {asset}` opens either as a tab.

| Method | Does |
|---|---|
| `forge.variant.list {query?, limit?}` | the variants of every map, the items first: the rows of `asset.list {kind: "Forge variants"}`, `{asset, name, map, file, source, package, editable, things, id, base}` |
| `forge.variant.read {asset?}` | the variant's things: `{target, block, added, removed, item, piece, notes, fields: [{field, type, value}]}`, with `package`. With no `asset`, the open one, with its unsaved edits (`dirty`) |
| `forge.variant.set {asset?, target, field, value}` | sets a field the variant writes; one undo step of the document. `asset` opens that variant first |
| `forge.variant.save {package?}` | writes the open variant as a `map-variant` item of `package`, else of the edit target: `{asset, saved, dirty, edit_target, path, log}` |
| `forge.items {query?, map?, custom?, limit?}` | the Forge palette, the rows of `asset.list {kind: "Forge items"}`: tag rows with `forge: {menu, custom, source}` |

`forge.variant.save` writes the item the variant was opened from when it
is the package's. A variant of another package, or one saved in the game,
becomes a new item at `map-variants/<map>/<name>.h2mapvariant`, and
`asset` is that item's id from then on. No file under
`<data>/map-variants/` is written.

A thing's `target` is `added:<op>` for an entry the variant adds and
`base:<block>:<index>` for an entry of the base map it edits or removes.
`type` is `bool`, `int`, `float`, `vec2`, `vec3`, `vec4`, `text` or `tag`; a
tag's value is `<group>:<path>`, or null for none. `forge.variant.set`
replaces the value of the write that gives the field its value, so it adds
no op; the value keeps the field's type and the document must still pass
`forge_doc::validate`.

| Refusal | Code |
|---|---|
| no variant is open; a field the variant does not write; an entry it does not have; a variant from Live | `REFUSED` |
| `forge.variant.save` with no `package` and no edit target | `NO_TARGET` |
| a value of another type; a `target` that is not `added:<op>` or `base:<block>:<index>` | `INVALID_PARAMS` |

A variant with unsaved edits does not close, and another does not open over
it: save it or undo the edits.

`forge.items` rows by `forge.source`:

| `source` | Rows | `custom` |
|---|---|---|
| `disc` | every tag of the tree in a group Forge places: `weap`, `vehi`, `eqip`, `scen`, `bloc`, `mach`, `ctrl`, and the spawner collections `itmc` and `vehc` | false |
| `package` | the tags the packages that are on add in those groups | true |
| `blockout` | the kit's block-out set (`forge_shapes::starter`), `bloc` under `forge\blockout\` | true |

`asset.where_used` and `asset.links` on an item's `tag:` id list the
variants that place it (`role` `Forge variant`, `via` `<n> placed in <name>`);
`asset.links` on a variant lists the tags it places under `uses`.

```sh
h2 studio forge.variant.list '{}'
h2 studio shell.open '{"asset":"item:my-pack:0"}'
h2 studio forge.variant.set '{"target":"added:1","field":"position","value":[1.5,0,2]}'
h2 studio forge.variant.save '{}'
h2 studio forge.items '{"custom":true}'
h2 studio asset.where_used '{"asset":"tag:weap:objects\\weapons\\rifle\\battle_rifle\\battle_rifle"}'
```

`asset.fork {tag, map, package?, set?}` is Fork for this map
(docs/engine/asset-store.md, "In Studio"; `app/mods.rs`). `tag` is
`<group>:<path>` or `tag:<group>:<path>`; `package` is a local package, else
the edit target (`NO_TARGET`). The tag's lines go into `edits/<map>.yaml` of
the package, an `edits` item whose `maps` names `map` alone, so every other
level reads the asset as it was. Without `set` the lines are the ones the
package's other `edits` items that reach `map` have for the tag (the last to
set a path wins); with `set`, a list of `{path, value, from?}`, those lines are
put in by path. The answer is `{package, tag, map, file, item, lines,
from_items, created}`. A tag with no line to fork, a tag forked for the map
already (without `set`) and a built-in package are `REFUSED`; a map that cannot
name a file (letters, digits, `_` and `-`) or a tag that is not one is
`INVALID_PARAMS`. The lock is rewritten.

`asset.revert {tag, map?, package?}` takes the tag's lines out of the
package's `edits` items: with `map` only the item that names that map alone,
without every `edits` item with an entry for the tag. An item left with no
entry goes with its file. The answer is `{package, tag, map, removed: [{item,
file, maps, lines, item_removed}]}`; a tag with no line is `REFUSED`.

The tests (`api/tests/assets_tests.rs`, `api/tests/links_tests.rs`,
`api/tests/mods_tests.rs`) run over an invented tree and local packages.

## Packages

`package.*` is the package document (docs/editor/studio-redesign.md,
"Packages"; `app/package_doc.rs`, `api/packages.rs`). A package is a folder
under `<data>/packages/<id>/` with `package.toml`, its files and
`package.lock`; the built-in packages are listed and read but never written
(`REFUSED`, "built in"). The load order and what is off are
`<data>/packages/set.toml`; the edit target is `<data>/packages/target.txt`.

| Method | Answers |
|---|---|
| `package.list` | the set in load order, then the packages that are off, each `{id, name, package_id, origin, dir?, on, why?, always_on, signed, features, items, scopes, target, editable}`, with `problems`, `content_hash` and `target` |
| `package.read {id}` | the manifest as `package` and `credit`, each item with `index`, `asset`, `builder`, `builds`, `scope` and `changes_map`, the lock, and `problems` |
| `package.new {id, name}` | an empty package with its manifest and lock; a taken, malformed or always-on id is refused |
| `package.set {id, path, value?}` | a `[package]` field (`name`, `description`, `image`, `requires`, `after`, `features`) or `credit.<field>`; no value removes an optional one; `id` and `format` are not edited |
| `package.item.add {id?, item, at?}` | an item in its manifest form; its files are in the folder already, with no `..` or absolute path |
| `package.item.set {id?, index, path, value?}` | one field of an item (`kind`, `source`, `inputs`, `tag`, `maps`) |
| `package.item.remove {id?, index}` | the item leaves the manifest; its files stay |
| `package.target {id?, clear?}` | the edit target, read, set to a local package, or cleared |
| `package.enable {id, on}` | `set.toml`'s `off`; `left_out` lists the packages that `require` the one switched off |
| `package.order {ids}` | `set.toml`'s `order`; `loaded` is the resulting load order |
| `package.check {id?}` | `{ok, count, findings: [{kind, message, item?, file?, line?}]}`: `lock` (missing or stale), `files` (a cache map, a `.h2tag` bundle, disc files, model weights or a dataset folder), `store` (a file that is a raw resource or a tag file of the user's tag store, by SHA-256; `package.read`'s `problems` and Problems list it too), `item` (no builder, a missing or oversize source, a source its builder refuses), `set` (a requirement missing or off), `conflict` (a tag path another package adds) |
| `package.build {id?, map?, out?}` | for each map of the open source the package has an item for (or `map`), `{map, scope, touched, took_ms, tags, added, changed, overrides, bytes, problems}`; with `out`, each built map is written to `<out>/<map>.map` |
| `package.play {id, map}` | the build for `map`, then `game.load` on the paired game |
| `package.export {id, out}` | `{id, out, files}`; creates a new folder outside Git with the local package manifest, generated lock and named sources; refuses an existing destination, forbidden package files and tag-store bytes |
| `package.diff {id?, map?}` | `{id, map, assets, counts, levels, built, conflicts, problems}`: `assets` is one row per asset the package changes or adds, `{asset, item, item_asset, change, maps, forked, ...}` where `change` is `edit` (with `fields: [{path, to, from?}]`, `blocks: [{path, op}]` and `was?`), `patch` (a `tag-delta`, with `was`), `override` (a `replaces`, with its hash) or `added`; `counts` counts them and the forks; `levels` are the maps the package changes (the open source's maps by class, else the maps its items name). With `map` (of the open source) the whole set is built over it and `conflicts` is the conflict table's rows `{map, kind: refused \| override, package, why, involves, other?, tag?, path?}`; `built` is false, with the reason in `problems`, when the build did not run. A read that built keeps its rows as that level's in Problems |

Each write puts the new manifest through the set before it keeps it: a
manifest that does not read, a source that is not in the folder, a
`requires` cycle or a set `content::package::Set` refuses leaves the old
manifest in place and answers `REFUSED` with the reason. `package.enable`
and `package.order` refuse by name: `core` and `ui` cannot be switched off
(`SetError::AlwaysOn`), and an order that puts a package before one it
`requires` or follows names both. A `set.toml` written by hand that turns
`core` off is named in `package.list`'s `problems` and stops every write to
the set file. The edit calls that default `id` to the edit target answer
`NO_TARGET` when none is set. `package.build` needs a source (`NO_DOCUMENT`) and
refuses a package that is off; `package.diff` needs one only for a `map`. `package.play` needs a pairing and
refuses a game whose content hash (`packages.list`) differs from
`package.list`'s: the game reads its set at boot.

```sh
h2 studio package.new '{"id":"my-pack","name":"My pack"}'
h2 studio package.target '{"id":"my-pack"}'
h2 studio package.order '{"ids":["my-pack","pogo"]}'
h2 studio package.check '{"id":"my-pack"}'
h2 studio package.build '{"map":"lockout"}'
```

The tests (`api/tests/packages_tests.rs`) run over invented packages under
a temporary data root.

## Problems

`problems.list {source?, asset?}` is the Problems panel's list
(docs/editor/studio-redesign.md, "The shell"; `app/problems.rs`,
`api/problems.rs`), read again on every call. A row is
`{source, area, code, severity, message, target, path?, asset?, field?,
tool?, line?, col?}`, worst first. The sources:

| `source` | Rows from |
|---|---|
| `check` | `check.run`'s `models` and `sounds` checks, and the diagnostics of the script open in the script editor (area `scripts`, code `script.<code>`, target `<title>:<line>`, with `line` and `col`; the row opens the editor with the cursor there) |
| `ui` | `ui.check` |
| `hud` | `hud.check` and the HUD kit's check |
| `package` | the manifest checks `h2 package check` runs over the packages under `<data>/packages/`: an item with no builder, a missing or oversize source, a source its builder refuses, two items that add one tag path, a file that is a raw resource or a tag file of the user's tag store (`workspace::store_findings`), and the conflict rows of the last `package.diff` with a `map` for each level (`package.refused`, a warning: the package is off for that level, with the reason; `package.override`, a note: a later package's value for a field or an asset wins; target `<map>: <package>`, asset `package:<id>`). A refresh keeps those rows; the next diff of the level replaces them |
| `build` | what the game's package set refuses to load, as the boot log says it: an unreadable package, a refused id, a `requires` that is missing, a set that cannot be ordered |
| `ground` | the last AI ground build of the open map: a label it could not honour (`ground.label`, a warning, target `<label>[<index>]`) or why it failed (`ground.build`, an error); the tool is the scenario ([AI ground](agent-api-extensions.md#ai-ground)) |

Each row names what opens it: `asset` (`tag:<group>:<path>` for a tag of
the open map, `package:<id>` or `item:<package>:<n>`), `field` (the field
of the tag the finding names, from a model finding's `tag.set path`), or
`tool` (`ui-screens` or `hud` for a screen or a HUD, which are not
assets). The answer is `{problems, counts: {error, warning, note, total,
by_source}, ran: [{source, check, findings}], skipped: [{source, check,
why}]}`; a check that cannot run is under `skipped`. With `source` only
that source is read and the panel's other rows stay. The panel's
double-click on a row is `Action::OpenProblem`: a tag opens in its editor
with the field shown, a package or item shows in the browser, a screen or
a HUD shows its tool.

`asset` (an asset id) keeps the rows about that asset: those whose
`asset` is it, and for the HUD, a screen at its state and the open script
the rows their tool holds (`App::asset_problems`, `App::doc_problems`). The
number on the header's **Check** is the same rows' count
(`App::doc_problem_count`), so it equals this call's `counts.total` for
the document's asset. A HUD document has no asset id of its own when it
was opened from a map; its count is the `hud` rows.

```sh
h2 studio problems.list '{"source":"package"}'
```

The tests are `app/problems_tests.rs` (findings of two invented sources
list together, a row opens its document and field), `api/tests/problems_tests.rs`
and `headless::tests::the_problems_panel_lists_rows_and_a_double_click_opens_them`.

## Sounds into a package

`import.sound {file, package?, tag, class?}` (`api/import.rs`,
`app/sound_import.rs`) adds a WAV to a local package as a `sound` item that
writes the `snd!` `tag`. The WAV (16-bit PCM, mono or stereo, any rate) is
copied unchanged to `sounds/<last segment of tag>.wav`, the item is added to
`package.toml` and `package.lock` is rewritten. `class` is a sound class by
name (`weapon_fire`, `ui`) or number; the default is `ui` (51), and any
other is written to `sounds/<name>.sound.yaml`, a `data` item beside the
WAV. The answer is what the build will write: `rate` (22,050, 32,000 or
44,100 Hz, the nearest not below the clip's), `frames` at that rate,
`duration_ms` and the `bytes` of Xbox ADPCM, with `from` the clip's own
rate, channels and frames. The sound source tab shows the waveform and
plays the WAV. Its `item:` id is `doc` in the reply. With no `package`,
the method uses the edit target or returns `NO_TARGET`. A package that is baked, a tag the package adds
already or a file that is not a 16-bit PCM WAV is refused; a class that is
not one is `invalid params`.

```sh
h2 studio import.sound '{"file":"/tmp/boing.wav","package":"my-pack","tag":"sounds\\boing"}'
```

The build is `workspace::build` (`h2 package build`); the tests
(`api/tests/import_tests.rs`, `workspace/src/import_tests.rs`) run over
invented tones and an invented map.

## Models into a package

`import.model {file, package?, tag}` adds a primitive model YAML and the
sibling atlas PNG it names to a local package as `model` and `data`
items. The model builder checks both files before Studio writes them.
The destination tag is a path such as `models/invented/box`. The
result includes the package, the `tag:hlmt:` asset id, the YAML's
relative `source`, the model item's index and the package directory.
The model source tab shows the mesh as a wireframe and the result
includes its `item:` id as `doc`. With no `package`, the method uses
the edit target or returns `NO_TARGET`.

## Sound edits

`sound.edit`, `sound.render` and `sound.permutation` (`api/sound_edit.rs`,
`app/sound_edit.rs`, the `sound-edit` crate) change what a package's `sound`
item sounds like without touching its WAV. A sound is named by `package`
(a local package's id) and `source` (the item's WAV, e.g.
`sounds/boing.wav`, as `import.sound` answers it); a package that is built
in is refused, and so is a `source` that is not a `sound` item of it.

`sound.edit {package, source, op, ...}` changes the list of edits kept in
`<source>.sound.yaml` (a `data` item of the package, added with the first
edit, the lock rewritten). `op` is an edit to add (at `at`, the end by
default): `trim {start_ms, end_ms?}`, `fade_in {ms}`, `fade_out {ms}`,
`gain {db}`, `normalise {lufs, ceiling_dbfs?}` (the default ceiling is -1
dBFS) or `silence {start_ms, end_ms}`; or `remove {index}`,
`move {from, to}`, `undo` (drops the last edit and answers it as
`removed`), `clear` or `list`. Times are milliseconds into what the edits
before left. The list is rendered before it is written: an edit that does
not apply (a trim that leaves nothing, normalising silence, a gain past 60
dB) is refused by its place (`edit 1: ...`) and the package is left as it
was; a param the edit does not take is `invalid params`. With
`permutation: n` the edits are that permutation's. The answer is the list,
`changed` and the levels of the render (as `sound.measure`'s).

`sound.render {package, source, permutation?, upto?, out?, bins?}` renders
the WAV through the first `upto` edits (all by default) and answers the
levels `before` and in `render`, and the waveform `peaks`; with `out` it
writes a 16-bit PCM WAV there. `out` is refused inside the package, whose
WAVs are the sources, and nothing in the package is written.

`sound.permutation {package, source, op}` keeps the sound's other
permutations, `permutations:` of the same file: `add {file, weight?,
gain_db?, skip_fraction?}` (the file must be a WAV of the package, and
becomes a `sound` item of it), `remove {index}`, `set {index, weight?,
gain_db?, skip_fraction?}`, `move {from, to}` or `list`. Each row carries
the loudness of its file after its edits, or why it does not render, with
`source_row` for the source's own. The build writes the source's own
permutation; encoding the listed files into the `snd!` is not built.

```sh
h2 studio sound.edit '{"package":"my-pack","source":"sounds/boing.wav","op":"fade_out","ms":40}'
h2 studio sound.render '{"package":"my-pack","source":"sounds/boing.wav","out":"/tmp/boing.wav"}'
```

The tests are `api/tests/sound_edit_tests.rs` (each op over an invented
ramp, the source's bytes unchanged, undo and reorder, the refusals),
`workspace/src/import_tests.rs` (the build renders the list) and the
`sound-edit` crate's own.

## Pictures into a package

`import.bitmap {file, package, tag}` (`api/import.rs`,
`app/bitmap_import.rs`) adds a PNG to a local package as a `bitmap` item
that writes the new `bitm` `tag`. The PNG (any bit depth and colour type) is
copied unchanged to `bitmaps/<last segment of tag>.png`, the item is added
to `package.toml` and `package.lock` is rewritten. The answer is what the
`bitmap` builder writes (`workspace::bitmap`): one linear `argb8888` image
of the PNG's `width` and `height` with one mip, `bytes` of pixels, and
`class: "shape"`, because a new tag reaches a game by a build and a load,
never by a push ([live-editors.md](live-editors.md)). A package that is
baked, a tag the package adds already or a file that is not a PNG is
refused; a tag that is not a path is `invalid params`. A bitmap with mips or
another format is made by `bitmap.reshape` on the built tag.

```sh
h2 studio import.bitmap '{"file":"/tmp/icon.png","package":"my-pack","tag":"ui\\icon"}'
```

The tests are in `api/tests/import_tests.rs`, over an invented PNG.

## Errors

The JSON-RPC codes (`-32700` parse, `-32601` no such method, `-32602`
bad params), and ours (`studio_proto::code`):

| Code | Name | Meaning |
|---|---|---|
| 10 | `NO_DOCUMENT` | the call needs a document that isn't open |
| 11 | `REFUSED` | checked and refused, with the reason: growth, a built-in variant, a Git checkout, the map's source |
| 12 | `UNSAVED` | it would drop unsaved edits; save, or pass `"discard": true` |
| 13 | `NOT_YET` | a planned method; `data.lane` says who builds it |
| 14 | `FAILED` | it ran and failed (I/O, a map that doesn't read) |
| 15 | `NO_GPU` | nothing renders here |
| 16 | `NO_TARGET` | the call writes to the edit target and none is set (`package.target`) |
