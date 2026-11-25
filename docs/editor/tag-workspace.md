# The tag workspace: Studio as the whole tools suite

Status: Design. The tag repository it stands on is built, with its extraction versions ([tag-repo.md](../formats/tag-repo.md)), and Studio's first start is built (W3: the start page, `workspace.status`, `workspace.extract` and `workspace.prune`, the game's `maps.load` rescan). The workspace's projects, the build and the rest of Studio's side are not. "Open questions" lists what the owner still has to choose.
Code: `crates/formats/tags/src/repo` (the tree, extraction, the build), `crates/tools/workspace` (layers, projects, the incremental build), `crates/apps/studio/src/api/workspace.rs` (`workspace.*`, `project.*`), `crates/apps/h2-tools/src/tag_repo_cmd.rs` (`h2 tags`). Tests: `crates/formats/tags/src/repo/tests.rs`, `crates/tools/workspace/src/tests.rs`, `crates/formats/tags/tests/tag_repo.rs` (`#[ignore]`, the disc).
Switch: none.

Studio is the one tool for content: it edits levels (the scenario tool),
edits tags (the Tags tool), views and imports assets, and builds maps.
Those were four or five separate programs for the retail game. The move
that makes it one is the source of truth: content stops being patches to
`.map` files and becomes a tree of loose tags on the user's machine, plus
our own sources in this repository. The first time Studio starts, the user
gives it their disc and it extracts every tag; it extracts again when our
knowledge of the formats grows. Maps are built from the tree. The rule
that must hold: the tree is game data, lives under the data root, and
nothing of it, its hashes or its lists is ever in Git.

The suite is four more pages on this one: the [content stage](content-stage.md),
the first map built this way; [live editors](live-editors.md), an editor
and a preview for every kind of tag; the [game view](game-view.md), the
game's picture inside Studio; and [playtest](playtest.md), bot matches
on the map being edited. "Build slices" orders the slices of all five.

## What exists

| Need | What there is | State |
|---|---|---|
| A loose tag, position independent, one file per tag | `tags::repo::LooseTag`, `format::{encode, decode}` | built; deterministic bytes |
| The tree on disk, raw resources by hash, per-map manifests and variants | `tags::repo::TagRepo`, `store_map`, `h2 tags extract` | built; the whole disc extracts with no tag refused ([tag-repo.md](../formats/tag-repo.md), "Proof") |
| Extraction versions: re-extract only what a better layout or a changed disc moved, resumable per map | `extract.toml`, `tags::repo::extract::{plan, update}`, `TagRepo::settle`, `h2 tags extract --update` | built; tested on invented maps, not yet run on the disc ([tag-repo.md](../formats/tag-repo.md), "Versions") |
| One trait over a map and a tree | `tags::repo::TagSource` (`MapSource`, `TagRepo`) | built |
| A map from a set of tags, by closure from the globals and a scenario | `closure`, `build::to_tag_set`, `build::from_repo`, `map::build::TagSet::build`, `h2 tags build` | built; two retail maps rebuild with every reached tag identical in canonical form |
| A retail map with only its movable tags rebuilt | `build::onto_retail` (`--onto`) | built; a map with its movable tags moved ran in the game ([tag-schema.md](../formats/tag-schema.md), "Load test") |
| A map of ours over a retail base's globals | `h1convert::scratch::{build, LevelSource}`, `h1campaign::scratch` | built; converted Halo 1 maps are the only from-scratch maps the game has loaded |
| Typed tag fields with defaults | `tags::defs` ([tag-definitions.md](../formats/tag-definitions.md)) | built for the groups the Halo 1 converter writes |
| Writers for our own assets | `h1_assets::models::mode::convert_model` (a `mode`), `collision_build` (a collision BSP), `mopp::build_structure`, `pathfind::generate`, `tags::rgba_bitmap` and `pogo_kit::texture::bitmap` (a `bitm`), `h1convert::shad::Library` (a `shad` on a retail template), `h1sounds::merge_sound` (the gestalt) | built, each inside the converter or a kit |
| Content added to a map | each kit patches a map: `pogo_kit::inject::add_pogo`, `firefight_kit::build_package`, `ui_kit::inject::tag::add_screens`, `campaign_cards::add_cards`, the font and gear rewrites; most run as a `vfs::Rewrite` when the game first reads the map | built; no shared shape |
| Tag editing | Studio's Tags and scenario tools on an open `.map`, saved as a patch package ([scenario-editor.md](scenario-editor.md)); `tag.export` and `tag.import` move `.h2tag` bundles between maps and only replace tags a map already has | built |

What is missing is the part between: layers over the tree, a project that
says which map to build from what, edits kept as data instead of as
patched bytes, the kits as steps of one build, a build fast enough to run
on every save, and Studio opening a project instead of a map.

## How it works

### The tree and its versions

The tree is `tags::repo`'s repository, in the layout
[tag-repo.md](../formats/tag-repo.md) records. Its root is Studio's
`tags_root` setting: `<data>/tags/` unless set, and on the owner's
machine a folder on the builds volume. It is read-only to every
tool but the extractor: a cache that the disc and the extractor
reproduce. Beside it the extractor keeps `extract.toml`
([tag-repo.md](../formats/tag-repo.md), "Versions"):

- `tags::repo::EXTRACT_VERSION` and the tag file format version the
  tree was extracted under;
- the disc's identity (the SHA-256 of each map file, as packages name a
  base) and which maps are extracted;
- per map and group, the layout hash (`tags::schema::hash`) its tags
  were extracted under, and its counts of tags, lossy tags and refused
  tags.

`tags::repo::extract::plan` compares that file with the running build
and the disc and gives the work: a map never extracted, a map whose file
changed, and for a group whose layout hash moved, that group's tags
only. A format or extractor version change re-extracts everything.
Extraction is per map and resumable; a map is recorded after its
manifest, so an interrupted run redoes one map. This is "the engine has new
things": a better `sbsp` layout, a newly described block or a new
raw-size pairing shows up as a moved layout hash and the tags that gain
from it are taken again.

**First start.** With no tree, Studio's start page is one step: choose
the disc (an image or an extracted folder; `H2_DISC` and
`disc/halo2.iso` are offered), see the size and time
([tag-repo.md](../formats/tag-repo.md), "The whole disc"), and extract
every map with a progress bar. On later starts Studio runs the plan and shows a bar in
the status line while stale parts re-extract in the background; work
continues on the tree as it is. Headless, `workspace.extract` does the
same.

**Size and cleanup.** The whole disc is about 2.7 GB of tag files and
raw resources. The generated and imported layers, build caches and
outputs add to it under `<data>/workspace/`. The builds volume has a
quota, so the workspace accounts for itself: `workspace.status` gives
the bytes of the tree, each layer, each project's cache and each
output; `workspace.prune` removes raw resources no tag file or
manifest names, generated layers no project asks for (with no project
files, all of them; a project file's own fingerprints are the
projects', W2) and build caches unused for a week — the outputs of
deleted projects are named by the project files that build them and
wait for those (W2) — and `cargo xtask prune` calls it. The tree
itself is never pruned in part: it is deleted whole and extracted
again.

### Layers

A build reads tags through `workspace::Layered`, a `TagSource` over
sources in order, first hit wins:

1. **Edits** (below), applied to the tag the lower layers give.
2. **Authored tags**: tags made from sources in Git. A kit's or an
   importer's output from `art/` and YAML, kept under
   `<data>/workspace/generated/<fingerprint>/` in the tree's file
   format and remade when the fingerprint moves.
3. **Imported maps**: a community map or a map pack map stored into
   `<data>/workspace/imports/<name>/` by `store_map`, so only what
   differs from the tree is held.
4. **The tree**, read with the variants of the project's base map
   (`TagRepo::for_map`).

`LevelSource` in `h1convert::scratch` is this with two layers; the
converter moves onto `Layered`.

### Edits are data

A change to a tag that exists (a retail weapon's field, a placement, a
HUD widget) is a line in an edit file, not a patched map and not a copy
of the tag:

```yaml
# assets/tag-edits/<name>.yaml (ours, in Git) or <data>/workspace/edits/ (the user's)
- tag: <group>:<path>
  was: <content hash of the tag the edit was made on>   # local files only
  set:
    - {path: "barrels[0].rounds_per_second.upper", value: 12.0}
    - {path: rounds_per_second, from: 8.0, value: 10.0}   # a mod's line: the value it replaces
  block: [{path: "magazines", op: add, like: 0}]
```

`workspace::edits::apply` runs them on a `LooseTag` through the same
field paths as `tag.set` (`tags::defs`, `tags::schema`). An edit whose
path no longer resolves, or whose `was` no longer matches after a
re-extraction, is reported with the tag and line and the build stops:
no edit is dropped quietly. A line's `from` is checked against the tag
as the build first found it, so two mods that set different fields of one
asset both apply and the later of two that set one field wins; a `from`
that the field does not hold is for other content, and in the game's
build it turns the package off for that map with its reason
([asset-store.md](../engine/asset-store.md), "Mods and packages"). Files in Git hold our own numbers and names
only, as `content/ui/retail-edits/` does for screens
([ui-in-maps.md](../game/ui-in-maps.md), "Retail edits as data").

Studio's document stays what it is, an open map with a journal and
undo. What changes is what a save writes: the journal as edit lines in
the project's edit file, then a build. The scenario tool's placement
edits are edit lines on the `scnr`.

### Projects

A project is a small file naming one map to build. In the package
model of [studio-redesign.md](studio-redesign.md), "Packages", it is a
`map` item of a package and its fields are these:

```toml
# art/stage/project.toml, or <data>/workspace/projects/<name>.toml
name = "stage"
mode = "fresh"                    # or "onto"
base = "lockout"                  # the disc map: variants, globals, header template
scenario = 'scenarios\multi\stage\stage'
kits = ["stage", "pogo"]          # ContentKit names, in order
edits = ["assets/tag-edits/pogo-tuning.yaml"]
output = "package"                # a map package under <data>/maps/
```

Two modes, because the tree cannot yet give every tag exactly:

- **`onto`**: a retail level with changes. The disc map is taken with
  every tag pinned (`TagSet::from_map`), the tags the layers changed
  are replaced, new tags are added, kits run. The level's `sbsp`,
  `ltmp` and `scnr` stay the disc's bytes unless an edit touches them.
  This is `build::onto_retail` and what every kit does now, and it is
  the path proven in the game.
- **`fresh`**: a map whose level is ours (the stage, a converted Halo 1
  map, later a level made in Studio). Closure from the globals and the
  scenario over the layers, `to_tag_set`, nothing pinned, as
  `h1convert::scratch::build`. A fresh build refuses a closure that
  holds a lossy tag and names it, so a retail level builds fresh only
  once its level tags extract without loss.

`workspace::build(project) -> Built` in order: resolve the layers, apply
edits, make the tag set for the mode, run each kit's `install`, merge
the per-map couplings (the sound gestalt by `h1sounds::merge_sound`, the
low-detail bitmap ranges), check tag memory
(`TagSet::check_tag_memory`), `TagSet::build`, read the result back,
write the output and a receipt. The output is a map package: a `derived`
whole map for `fresh`, a patch against the disc map for `onto`
(`content::patch::diff`, as `firefight_kit::build_package`). The game
loads it by its id, or as a play copy of a disc level
([editor.md](editor.md#play-and-pair)).

### Kits are build steps

`content::kit::ContentKit` is the one shape for content added by code.
A package names one as a builder, and `KitRewrite` runs the whole
package set ([studio-redesign.md](studio-redesign.md), "How the game
consumes packages"):

```rust
pub trait ContentKit {
    fn name(&self) -> &'static str;
    fn wants(&self, map: &MapInfo) -> bool;
    fn fingerprint(&self, sources: &Sources) -> Sha256;
    fn install(&self, set: &mut TagSet, sources: &Sources) -> Result<Installed>; // tags added, Costs
}
```

`install` is what each kit already has (`TagSet` in, tags added). Two
callers run it: the workspace build, and one `vfs::Rewrite`
(`content::kit::KitRewrite`) that replaces each kit's own rewrite and
cache in `boot`. So a player's game keeps adding our content to the
user's disc maps on first read, with no tree, and Studio's build of the
same map runs the same code. `Sources` is the kit's baked files or a
directory in a working tree, which is how a lane sees a changed atlas
without rebuilding `h2`. A map that already holds a kit's root tag is
skipped by `KitRewrite`.

Migration, one kit at a time, each a move with no change in the tags
made: the Pogo, the Firefight kit, the UI screens and retail edits, the
campaign cards, the font and gear rewrites, Forge's build, test-map
recipes (a recipe becomes a project with edit lines). `scenario_edit`'s
patch packages stop being written by Studio once its save writes edit
lines; nothing is published, so no saved package is carried over.

### Authoring and importing

- **A tag as text.** `<path>.<group>.yaml` in a kit's source folder:
  `like:` (start from an extracted tag) or the group's defaults, then
  fields by `tags::defs` name and references by key. `workspace::author`
  turns it into a `LooseTag`. A weapon's definition then needs no Rust;
  `pogo_kit::kit::weapon` is the first to move.
- **Importers**, each a function from our sources to loose tags, called
  by kits and by `h2 tags import` and Studio's `import.*`: a mesh to
  `mode`, `coll` and `phmo`; a PNG to `bitm`; a WAV to `snd!` with the
  gestalt merged; a triangle soup to `sbsp` collision with MOPP and
  pathfinding. They are the converter's and the kits' writers lifted
  into `workspace::import` with one signature. The Tags tool's
  viewers (model, bitmap, sound, shader) are the suite's asset viewers
  ([rich-editors.md](rich-editors.md)).

### Build speed

A save builds, so the build is incremental (`workspace::cache`, under
`<data>/workspace/build/<project>/`):

- a tag's built block tree is kept by the hash of its loose file, its
  edits and its group's layout hash; an unchanged tag is not decoded
  or rebuilt;
- raw resources are placed by hash and the raw region of the last
  output is reused when its set did not change;
- the closure is kept with the keys it read and re-walked only from
  changed tags;
- layout, the index and the header are redone every time (they are
  address assignment over the kept trees);
- `onto` rebuilds only the tags that changed and keeps the disc map's
  bytes for the rest.

A build that changed only field bytes of tags the game holds is also
sent to a paired game as `edit.batch`, so the game shows it before the
map is written ([live-mirror.md](live-mirror.md)). Targets, until
`notes/tag-workspace.md` (new) records them (**S**): the stage under
1 s, a retail multiplayer map after a one-tag change under 3 s, a first
build of one under 15 s.

## Interfaces

Studio ([agent-api.md](agent-api.md)):

| Method | Access | Does |
|---|---|---|
| `workspace.status` | read | the tree (path, maps extracted, lossy counts), the plan's stale parts, an extraction in progress |
| `workspace.extract` | act | `{disc?, maps?, verify?}` runs the plan; progress in `workspace.status` |
| `workspace.import` | act | `{map \| path, name}` stores a map as an import layer |
| `project.list`, `project.read` | read | projects and what each resolves to: layers, kits, edit files, mode, the lossy tags that block `fresh` |
| `project.build` | act | `{project, sources?}` builds; `{map_id, file, took_ms, rebuilt, reused, costs, problems}` |
| `project.play` | act | builds, then loads the output in the paired game |
| `doc.open {project}` | act | opens a project's last build as the document; saves write edit lines |
| `import.model`, `import.bitmap`, `import.sound` | act | an importer on a source file, into the authored layer (`import.bitmap` and `import.sound` are built) |

The CLI gains `h2 tags extract --update`, `h2 tags build --project
FILE` and `h2 tags import`, over the same crate, for the coordinator
and CI. The game gains nothing but `maps.load` finding a package written
while it runs.

## Rules and limits

- The tree, the generated and imported layers, build caches and
  outputs live under the data root. `workspace` refuses a root inside
  a Git checkout. Git holds sources (`art/`, YAML, kits) and edit
  files of our own numbers and names.
- Nothing is taken from a leaked build: not a tag layout, not a tool's
  behaviour. Field names follow [tag-field-names.md](notes/tag-field-names.md).
- The game never needs the tree. Players get our content through
  `KitRewrite` on their own disc maps, and maps of ours as packages.
- A campaign tag that points into the campaign's shared cache is
  refused in a multiplayer project
  ([tag-repo.md](../formats/tag-repo.md), "Open").
- Tags the engine finds by name are not in a closure. A `fresh`
  project keeps `h1convert::fresh::{KEEP_GROUPS, KEEP_PREFIXES}` as
  roots.
- A built map's tag memory is checked against the retail bound for its
  type; the engine's own bound is not known.
- An edit is never dropped: a path or a `was` that does not match
  stops the build with the line.
- Opening a loose `.map` (a community mod) still works; saving one
  offers "import as a layer".

## Running built maps in the game

| Build | Evidence | Verdict |
|---|---|---|
| `onto` a retail map, movable tags rebuilt | ran in the game for 2,000 ticks ([tag-schema.md](../formats/tag-schema.md), "Load test"); every kit's rewrite is this with tags added | yes |
| `fresh`, a level of ours over retail globals | the converted Halo 1 maps, built from scratch with the builder's own datums, an empty index tail and a retail header template | yes for those; the stage is the second case and its hand-made BSP the new part |
| `fresh`, a retail level from the tree | every reached tag identical in canonical form, but `sbsp`, `ltmp` and `scnr` are lossy and the comparison cannot see what both sides lose ([tag-repo.md](../formats/tag-repo.md), "Open"); never loaded | no, until those layouts are complete. `onto` covers retail levels meanwhile |

## Risks

- **Lossy level tags.** Until `sbsp`, `ltmp` and `scnr` extract whole,
  retail levels stay `onto` and geometry edits go through
  [modeling-targets.md](modeling-targets.md)'s writers.
- **Per-map couplings.** A sound indexes its map's gestalt and a
  bitmap its map's low-detail region. Mixing tags from several maps
  needs both merged; the Halo 1 path does it for sounds and the
  generic pass is new.
- **Shared-cache resources.** A build keeps pointers into the shared
  caches and adds new resources locally. A project that changes a
  shared cache itself (the UI screens) builds that cache, and every
  map's dependency on it must still match.
- **Checksums and identities.** The builder writes the header checksum
  and takes dependency identities from the base's header. A play copy
  is checked against the retail payload checksum on load
  ([agent-api.md](../runtime/agent-api.md#test-maps)); which built
  outputs that check accepts is to be read before slice W3.
- **Tag memory.** One added tag can move the whole index table into
  metadata, and some retail maps have no room
  ([scenario-editor.md](scenario-editor.md), section 6).
- **Disk and time.** The whole disc is about 2.7 GB and a minute and a
  half to extract, more with raw written, on a volume with a quota
  ("Size and cleanup").
- **Pinned assumptions in kits.** A kit that finds its template by
  slot or address works on a pinned set and not on a fresh one; each
  migration checks both modes.

## Build slices

One lane each. A lane writes code and tests; the coordinator runs them.
`S` rows are the stage's, detailed in [content-stage.md](content-stage.md#build-slices).

| # | Slice | Owns | Tests | Accepted when |
|---|---|---|---|---|
| W1 | Extraction versions: `extract.toml`, per-group layout hashes, the plan, resumable per map, `h2 tags extract --update` | `crates/formats/tags/src/repo/{store.rs, extract.rs}`, `crates/formats/tags/src/schema` (the hash), `tag_repo_cmd.rs`, [tag-repo.md](../formats/tag-repo.md) | on invented maps: a moved layout hash re-extracts that group only; an interrupted run redoes one map; unchanged input is a no-op | the disc test extracts, then `--update` does nothing |
| S1, S2 | The stage's geometry and its map, built `fresh` from a `TagSource` | see the stage page | | the stage loads in the game |
| W2 | `Layered` and projects: the crate, the project file, `fresh` and `onto` builds, the package output, `h2 tags build --project` | `crates/tools/workspace/{Cargo.toml, src/lib.rs, layer.rs, project.rs, build.rs, tests.rs}`, `h1convert/src/scratch.rs` (onto `Layered`) | invented tree and layers: first hit wins; a lossy tag refuses `fresh` by name; `onto` output differs from its base only in changed tags; the stage's project builds the bytes S2 built | `h2 tags build --project art/stage/project.toml` writes the stage package |
| W3 | Studio's first start and `workspace.*` (built): the start page, the background plan, `tags_root`, status with sizes, `workspace.prune`, the game's `maps.load` rescan | `crates/apps/studio/src/{api/workspace.rs, ui/workspace.rs, app/workspace.rs}`, `studio-proto`, `crates/extensions/game/src/test_maps.rs`, [agent-api.md](agent-api.md), [editor-guide.md](editor-guide.md) | headless: no tree shows the start page; `workspace.extract` on an invented disc; a Git checkout refused; every method's example | a fresh data root extracts from the disc in the window, with progress |
| W4 | `ContentKit`, `KitRewrite`, the Pogo as the first kit, sources from a directory | `crates/formats/content/src/kit.rs`, `crates/tools/pogo-kit/src/{kit.rs, source.rs, rewrite.rs, inject.rs}`, `crates/apps/boot/src/lib.rs` (the chain), [pogo-stick.md](../game/pogo-stick.md) | baked and directory sources install the same tags; a map holding the kit is skipped; the rewrite's cache key is unchanged for unchanged input | the stage project with `kits = ["stage", "pogo"]` holds the Pogo |
| S3 to S8 | The stage's flow: game methods, `stage.*`, checks | see the stage page | | `stage.run` gives a verdict on the Pogo |
| W5 | Edit files and Studio's save: the format, `apply`, the journal to edit lines, `doc.open {project}`, `project.*` | `workspace/src/edits.rs`, `studio/src/{api/project.rs, app/project_doc.rs}`, [tag-editing.md](tag-editing.md), [scenario-editor.md](scenario-editor.md) | apply then diff gives the edit back; a stale `was` and a dead path stop with the line; the window's save and the API's write the same file; undo across a save | a field changed in the Tags tool is in the edit file, in the built map and in the paired game |
| W6 | The incremental cache and its measurements | `workspace/src/cache.rs`, `docs/editor/notes/tag-workspace.md` | a one-tag change rebuilds one tree; a changed layout hash drops that group's entries; cached and uncached builds are the same bytes | the note has the three timings |
| W7 | Tags as text and the importers' one signature | `workspace/src/{author.rs, import.rs}`, `pogo-kit` (its weapon as YAML), `tag_repo_cmd.rs` (`h2 tags import`), `studio/src/api/import.rs` | a text tag equals the Rust-built one byte for byte; unknown fields refused with the nearest name; each importer on an invented source | the Pogo's `weap` comes from YAML with the same tags as before |
| W8 | The other kits onto `ContentKit`, one lane per kit: Firefight, UI screens and retail edits, campaign cards, fonts and gear, Forge's build, test-map recipes | each kit's crate or module, `boot/src/lib.rs`, the kit's page | per kit: the map made through `KitRewrite` equals the one its own rewrite made | boot's chain is `KitRewrite` and the kits' own caches are gone |
| W9 | Imports as layers: community maps and the map pack | `workspace/src/import_map.rs`, `tags::repo::extract` (map pack caches), `studio/src/api/workspace.rs` | an invented mod map stores only its differing tags; a project over it builds | a community map opens as a project |
| W10 (later) | Level tags without loss: `sbsp`, `ltmp` and `scnr` layouts from `tags::structure`, so retail levels build `fresh` | `crates/formats/tags/src/{schema, structure.rs, repo/extract.rs}`, [tag-repo.md](../formats/tag-repo.md) | the disc test reports no lossy level tag on its maps; the rebuilt regions compare byte for byte in content | a retail multiplayer map built `fresh` loads in the game |

### Order across the pages

`S` is [content-stage.md](content-stage.md), `GV` [game-view.md](game-view.md),
`LE` [live-editors.md](live-editors.md), `PT` [playtest.md](playtest.md).
A wave's slices are independent of each other.

| Wave | Slices | Waits for |
|---|---|---|
| 1 | W1, S1, then S2 | nothing |
| 2 | GV1 (the picture), LE1 (registry and fan-out), PT1 (static checks), PT2 (spatial trace), PT3 (measures), S4 (`render.view`), S5 (`audio.meter`) | nothing; they start beside wave 1 |
| 3 | W2 (layers and projects), S3 (lighting and sky), PT8 (fresh maps) | S2 |
| 4 | W3 (first start), W4 (kits), GV2 (the panel, attached), GV3 (editor mode, the own game), PT4 (the runner), PT5 (overlays) | W2 for W3, W4 and PT4; GV1 for GV2 and GV3; PT3 for PT5 |
| 5 | S6 (`stage.*`), GV4 (input), GV5 (cameras), LE2 (the subject), W5 (edit files), PT6 (compare), PT7 (the pool) | GV3 for S6, GV4, GV5 and LE2; W4 for W5; PT4 for PT6 and PT7 |
| 6 | S7 (checks and `stage.run`), GV6 (edits with no Pending), W6 (incremental build), W7 (tags as text, importers), LE3 to LE8 (the editors) | S5, S6 and W4 for S7; W5 for GV6; LE1 and each editor's cited slice |
| 7 | S8 (panel, skill, budgets), W8 (the other kits), W9 (imports) | S7; W4 |
| Later | W10 (retail levels built fresh) | W1 |

## Decisions

- The suite covers the scenario editor, the tag editor, the asset
  editors and viewers and the map builder, each tag kind with an
  editor and a preview ([live-editors.md](live-editors.md)).
- A player's game keeps the boot-time rewrite (`KitRewrite`) and needs
  no tree and no Studio.
- The first start extracts every map.
- Edit files for retail tags are in Git, holding tag paths, field
  names and our numbers.
- Studio's save writes edit lines and builds. It writes no patch
  packages and migrates none.
- Retail levels stay `onto`. Building them `fresh` (W10) is a later
  goal.

## Open questions

1. **Our own tags in Git.** Sources only (YAML, art, kit code), with
   the loose tags generated locally. Or commit the generated loose
   tags of ours as well, for review?

## Evidence

- Extraction of the whole disc refuses no tag, and two maps rebuild
  from the tree with every reached tag identical in canonical form
  (**V**, `crates/formats/tags/tests/tag_repo.rs`, [tag-repo.md](../formats/tag-repo.md), "Proof").
- `sbsp`, `ltmp` and every `scnr` are lossy in the tree (**V**, the
  same page, "Open").
- A retail map with its movable tags at new addresses ran in the game
  (**Run**, [tag-schema.md](../formats/tag-schema.md), "Load test").
- From-scratch maps the game has loaded are the converted Halo 1 maps
  only (**Code**, [map-builder.md](../formats/map-builder.md), "Not verified").
- Every kit adds tags to a `TagSet` and most run as a `vfs::Rewrite`
  with a cache of their own (**Code**, `crates/tools/pogo-kit/src/rewrite.rs`,
  `crates/extensions/game/src/ui_kit/inject/rewrite.rs`,
  `crates/extensions/game/src/campaign_cards.rs`).
- `tag.import` replaces tags and does not add them (**Code**,
  `crates/apps/studio/src/api/tag_io.rs`).
- Build times are targets (**S**: W6's note).

## Not verified

- That `KitRewrite` over a fresh map and over a pinned map gives the
  game the same content for every kit.
- The time of a build, cold and incremental.
- That a re-extraction under a better layout leaves every edit file
  applying.
- That the play copy check accepts a built `onto` map of a disc level
  and a `fresh` package alike.
- That a retail level built `fresh` loads (W10).

## Related

- [studio-redesign.md](studio-redesign.md): packages, the unit this build runs on, and the window around it
- [content-stage.md](content-stage.md): the first project, and how content is checked on it
- [live-editors.md](live-editors.md), [game-view.md](game-view.md), [playtest.md](playtest.md): the rest of the suite
- [tag-repo.md](../formats/tag-repo.md), [tag-schema.md](../formats/tag-schema.md), [map-builder.md](../formats/map-builder.md), [tag-definitions.md](../formats/tag-definitions.md): the tree, layouts, the builder, typed fields
- [editor.md](editor.md), [tag-editing.md](tag-editing.md), [scenario-editor.md](scenario-editor.md), [rich-editors.md](rich-editors.md): Studio's tools
- [maps.md](../game/maps.md): packages
- [multiplayer.md](../halo1/multiplayer.md): the fresh build as the converter uses it
