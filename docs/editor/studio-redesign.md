# Studio, redesigned: one tag tree, packages, and the animation and sound editors

Status: Design. Nothing on this page is built except what "What exists" names and the slices marked built; the slices at the end are the order to build it in.
Code: `crates/apps/studio/src` (the shell, `ui.rs`, `ui/layout.rs`, `app.rs`), `crates/apps/studio-proto/src` (the API's wire), `crates/formats/content/src/package/` (manifest, id, set), `crates/tools/workspace` (builders and the build), `crates/apps/boot/src/packages.rs` (new: the game's side). Tests: each slice names its own.
Switch: none.

Studio is one window over one tree: every tag of every map on the
user's discs, extracted, with our packages laid over it. There are no
tools and no modes. Opening a tag opens the editor for what it is: a
sound tag is the sound editor, an animation graph the animation editor,
a scenario the map editor with its viewport. Every piece of content we
add to the game is a **package**: a folder with a manifest that the game
build, the game's boot, Studio and Live all read the same way. Halo 1
maps, the Pogo Stick,
our menu screens, the HUD kit's HUDs, the Map Pack's rotations and the
game types (Firefight and Siege among them) stop being separate injection paths in
`boot` and become packages in one load order. The rules that must hold:
a package holds our own content and patches, never bytes from a disc;
every action in the window is a method of the agent API
([agent-api.md](agent-api.md)); Studio stays native Rust on Dear ImGui.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).
"**E first**" marks a step that needs a read of the retail executable
before its slice starts.

## What exists

| Part | State | Where |
|---|---|---|
| The window: the document bar, View > Editors, a dock layout per surface, Go to anything, the game link (SH5) | built | `ui/bar.rs`, `ui/layout.rs`, `ui/palette.rs`, `app::Surface` |
| One command layer for the window and the API | built | `App::dispatch`, `App::perform` (`app.rs`), `api/mod.rs` |
| Document tabs: a tab per open document, each with its own undo stack; a tag's editor from its group; `shell.tabs`, `shell.open`, `shell.focus`, `shell.close` (SH1) | built | `app/shell.rs`, `ui/doc_tabs.rs`, `api/shell.rs`, `TagDoc::undo_tag` |
| The browser's rows: the Tags root by path with variants over `TagRepo`, the Packages and Live roots, Kind, Map, search, badges, `asset.list`, `asset.thumb` (a package's PNG bitmap item, a bitmap tag of the open map), `asset.copy`, `asset.where_used`, `asset.edits`, `asset.drop` (SH2, FL13, FL16, FL17) | built | `app/assets.rs`, `app/assets/{browse,thumbs,drop}.rs`, `api/assets.rs` |
| The browser panel: the three roots, the Kind and Map pickers, the search box, rows with badges and variant lines, open by double-click, **New from this...**, **Open as fields**, the **Where used** and **Show edits** views, bitmap thumbnails, a dropped source file (SH2, FL11, FL13, FL15 to FL17) | built | `ui/browser.rs`, `Action::OpenAsset`, `Action::CreateAsset`, `Action::OpenFields`, `Action::WhereUsed`, `Action::ShowEdits`, `Action::DropFile` |
| Forge map variants as documents saved as `map-variant` items, and the Forge palette as a Browser kind (FL14, FL18) | built | `app/forge_variants.rs`, `app/assets/forge.rs`, `ui/forge_variant.rs`, `api/forge_variants.rs` ([studio-flow.md](studio-flow.md), "Forge variants and items") |
| The kind registry: which editor and preview a tag group gets | built | `app/kinds.rs`, `tag.kinds` |
| The field panel and its row model | built | `fields.rs`, `fields/row.rs` |
| Pairing, the live mirror, `pair.preview`, `pair.audio` | built | `play/`, `api/preview.rs`, `api/pair.rs` |
| Map packages: a base plus patch layers, `MapId`, the resolver, Live sharing, mod packs | built | `crates/formats/content`, `crates/runtime/vfs/src/packages.rs`, [maps.md](../game/maps.md) |
| Sound audition (A1), mouth curves read (A2), sound measurement | built | `app/audition.rs`, `ui/sound.rs`, `api/sound.rs`, `api/sound_measure.rs`, `tags::sound::Gestalt::mouth_curve` |
| Announcer lines and voice cloning | built | [announcer.md](announcer.md), [voice-cloning.md](voice-cloning.md) |
| Animation graphs read and sampled, every retail codec, and a graph's clips posed on the model that uses it (M1) | built | `crates/formats/tags/src/anim.rs`, `anim_sample.rs` ([animation.md](../formats/animation.md)); the pose view `crates/apps/studio/src/{app,ui,api}/anim.rs` |
| Animation key editing, curve handles, gizmo keying, and in-memory clip checks (AN3) | built | `crates/tools/anim-edit`, Studio `app/anim/document.rs`, `ui/anim.rs`, `ui/anim/edit.rs` and `api/anim/edit.rs` |
| Animation package source and graph writeback (AN4) | built | `crates/tools/anim-edit/src/source.rs`, `crates/tools/workspace/src/animation.rs`, Studio `app/anim/save.rs` and `api/anim/save.rs` |
| A `jmad` stream encoder (static stream and the 8-byte quantised codec), which the Halo 1 converter calls | built (AN2) | `crates/formats/tags/src/anim_write.rs`; the converter's graph assembly stays in `crates/tools/h1-assets/src/models/jmad.rs` |
| An Xbox ADPCM encoder; a `snd!` writer with the gestalt merge | built, the second inside the converter | `crates/platform/audio/src/adpcm.rs` (`encode`), `crates/tools/h1-assets/src/sounds/merge.rs` |
| Packages: `content::package::{Manifest, Lock, PackageId, Set}`, validation, `requires`, `after`, `set.toml`, `core` and `ui` always on, the load order, the content set | built | `crates/formats/content/src/package/` |
| The package build: the builder registry (`tag`, `edits`, `bitmap`, `model`, `animation`, `sound`, `data`, `hud`, `screens`, `screen-edits`, `rules`, `variant`, `playlist`, `map-variant`), `build` of ordered packages on a `TagSet`, `build_map` on a map file, conflicts, the cache key, `Layered`, `h2 package check\|build` | built (PK2) | `crates/tools/workspace`, `crates/apps/h2-tools/src/package_cmd.rs` |
| Mod patches: a `set` line's `from`, `replaces`, the `tag-delta` item, the conflict table (`build_map_settled`), the asset hashes a `was` names (`workspace::assets`), a file that is the store's refused (`h2 package check --store`) | built ([asset-store.md](../engine/asset-store.md), AS6); Studio's side (fork, revert, diff, **Changed by**, conflict rows in Problems) is AS7, built | `crates/tools/workspace/src/{assets,build,delta,edit,load,onto}.rs` |
| The game's side: the baked set (`content/`), `<data>/packages/`, `set.toml`, features, `PackageRewrite` with its cache, `packages.list` | built (PK3); `content/pogo` and `content/ui` supply per-map items | `crates/tools/workspace/src/{game,rewrite}.rs`, `crates/apps/boot/src/packages.rs`, `crates/apps/boot/build.rs` |
| Projects, the other kits as build steps | design | [tag-workspace.md](tag-workspace.md) |

Each piece of new content reaches the game by a path of its own
(**Code**, `crates/apps/boot/src/lib.rs`, the `vfs::Chain` of rewrites and
the calls before it):

| Content | Source in Git | How it gets in |
|---|---|---|
| Halo 1 maps and campaign | the recipe, `content/halo1/convert/` (the bytes are the user's disc) | the `halo1` package's `convert` item, run before boot by `package_prepare::before_boot` (PK10, "Conversions"), writes converted levels to the tag store and campaign metadata to `<data>/campaigns/halo1/`; `campaigns::boot_state` and `new_maps::install_at_boot` list them |
| Pogo Stick | `content/pogo/` (the `pogo` package) | `workspace::PackageRewrite` (PK4; `content_kit::served` reads the weapons it left) |
| Menu screens, retail screen edits | `content/ui/` (the `ui` package) | `workspace::PackageRewrite` (PK5) |
| HUD kit HUDs | `hud` items (`content/{superbounce,forge,modes}/huds/`); their states in Rust (`crates/rewrites/hud-kit-core/src/views/`) | `hud_kit::install` in `boot/src/registration.rs`, compiled against the loaded map |
| Firefight | the ground generator in Rust (`crates/tools/firefight-kit`); the wave sets are `rules` items of `modes` (`content/modes/firefight/*.firefight.json`, PK12) | `firefight_kit::ground::build` writes a map package per base map when asked (`h2 firefight kit`, `boot/src/test_maps_recipes.rs`, `crates/live/host-data/src/bundle/kits.rs`); the sets are read by `firefight::builtin` and `workspace::wave_sets` |
| Forge | `.h2forge` documents | `forge_build::build` materialises a format 2 map package ([forge-design.md](../game/forge-design.md)) |
| Map Pack maps and rotations | none (the user's disc) | `h2 import-mappack` and the automatic install ([map-pack.md](../game/map-pack.md)); rotations in `crates/live/live-proto/src/hopper/defaults.rs` |
| Rules modes and variants | `rules` and `variant` items (`content/core/rules/`, `content/modes/rules/`) | `rules::builtin::Registry` from the set (`workspace::script_registry`); the constants of `rules::builtin` are the same files; list entries by `community_variants`, installed at boot with the set's registry |
| The default hopper's playlists | a `playlist` item (`content/modes/playlists/default.toml`, `content::playlist::Fragment`) | `workspace::hopper_fragments` lists a set's fragments for the server |
| Campaign cards, icon fonts, the bot gear glyph | `kit` items: `content/halo1/` (`campaign-cards`) and `content/core/` (`glyphs`); the cards and the icon sets are the user's, given by the boot (PK12) | `workspace::PackageRewrite` runs `campaign_cards::CardRewrite` on mainmenu, and `icons::font_rewrite` then `bot_mark::GearRewrite` on each font file |
| Announcer clips | `art/announcer/` | baked; one WAV per medal of ours (`art/announcer/README.md`) |
| Community mods | none (Live) | imported as map packages with credit (`h2 live mods import`), grouped as mod packs |

## The new UI

### The shell

One window and one layout. The tool tabs go and nothing takes their
place: what is on screen follows the document in front. Documents are
tabs in the centre.

```
+--------------------------------------------------------------------------------------+
| File  Edit  View  Package  Game  Help                                                |
+--------------------------------------------------------------------------------------+
| [Go to anything........]     into: pogo *  [v]          (o) Paired: h2 fast  > || >| |
+----------------+---------------------------------------------------+-----------------+
| BROWSER        | pogo_unsc.weap x | pogo_unsc.mode | lockout.scnr  | INSPECTOR       |
| [search......] +---------------------------------------------------+ pogo_unsc       |
| Kind: [All  v] |                                                   | Weapon          |
| Map:  [All  v] |                                                   | > Handling      |
| v Tags         |              the editor for this                  |   Ready time    |
|   v objects    |               kind of tag, and                    |   0.25 s        |
|     v weapons  |                its preview                        | > Barrel        |
|       > rifle  |                                                   | > Model         |
|       v pogo + |                                                   |   -> pogo_unsc  |
|   > sound      |                                                   |      .hlmt      |
|   > scenarios  |  Preview: (o) Studio  ( ) My game  ( ) Attached   |                 |
|   > ui         |                                                   | Where used (3)  |
| v Packages     |                                                   | Changed by: pogo|
|   > pogo  (*)  |                                                   | In maps: 12     |
+----------------+---------------------------------------------------+-----------------+
| Problems (2) | Timeline | Pending | Game | Log                                       |
|  ! pogo  weapons\pogo_unsc.weap  barrels[0].rounds_per_second: no such field         |
+--------------------------------------------------------------------------------------+
| pogo: 14 items, built 0.8 s ago                         hint under the pointer  60fps|
+--------------------------------------------------------------------------------------+
```

- **The bar** (`ui/bar.rs`). Left: Go to anything. Middle: the **edit
  target**, the package that receives every edit ("into: pogo"), with
  `*` while it has unsaved edits. Right: the game link pill and its
  Play, Pause and Step controls, as built.
- **Browser** (left). One tree for everything Studio can open, in place
  of the Maps list, the Tags tab, the Variants and Playlists lists, the
  screen folders and the HUD list.
- **Documents** (centre). One tab per open tag. Each has its own undo
  stack, as `doc.list` reports them. The editor is the one
  `app::kinds::KINDS` names for the tag's group; a group with no editor
  of its own gets the field panel full width.
- **Inspector** (right). The field panel's rows (`fields::rows`) for the
  selection in the front document, with **Where used**, **In maps** and
  **Changed by** (the packages whose edit lines touch the tag, in load
  order, with each field's value and the one that wins; **Fork for this
  map** and **Revert for this map** act on the open map in the edit
  target, `asset.fork` and `asset.revert`). Every
  editor is a second view of these rows, so any tag can always be edited
  field by field.
- **Bottom dock.** **Problems** (`ui/problems.rs`) is one list for every
  check and build error (`problems.list`); a double-click on a row opens
  its tag and field, its package or its tool. **Timeline** shows when the front document has time (an
  animation, a sound, a recording, a playtest run) and hides otherwise.
  **Pending**, **Game** and **Log** are the built panels, and so is
  **Playtest** (`ui/playtest.rs`), the result of the last playtest run
  ([playtest.md](playtest.md#the-layers-and-the-panel)).
- **Go to anything** (`ui/palette.rs`) searches every tag, package,
  item and command (`asset.list`'s rows), with no map open.

### The browser

The **Tags** root is the tag tree: every tag of every map of the Halo 2
disc, the Map Pack and the Halo 1 disc, extracted once on first start
into `<data>/tags/` (`tags::repo::TagRepo`,
[tag-repo.md](../formats/tag-repo.md); the first start is slice W3 of
[tag-workspace.md](tag-workspace.md)). It is one tree by tag path, as
the tags name themselves (`objects\weapons\...`, `sound\...`,
`scenarios\...`), not a list of maps. A tag that many maps hold is one
row; where maps hold different versions the row opens to the tree's
variants, each naming its maps. While the store's catalogue is current
the versions are assets by hash, so two files that differ only in what
ties them to their map are one
([asset-store.md](../engine/asset-store.md), "The catalogue"). Tags our
packages add are in the same
tree at their own paths, marked `+`. A map is not a container here: it
is its scenario tag, and opening that is opening the map.

Two pickers and a search box narrow the tree: **Kind** (Maps, Objects,
Models, Animations, Bitmaps, Shaders, Effects, Sounds, Screens, HUDs,
Strings, or a tag group) and **Map** (only what one map reaches, by the
closure from its scenario). Every word of the search must match.

The **Packages** root lists the set in load order, the edit target
first; a package opens to its items and source files, and holds what is
not a tag: `.rules` scripts, variants, playlists, recipes. **Live**
lists packages the server offers.

The panel (`ui/browser.rs`) is the `asset.list` rows drawn: the root
buttons, the Kind and Map pickers, the search box, then one line per tag
by path with its group, a `[n variants]` mark and its badge (**edited by
<package>** or **new in <package>**). A tag whose maps differ is followed
by one line per variant naming its maps. The list is read again when a
filter changes, on the refresh button and when the first-start page
comes or goes. Beside the root buttons are **Open...** and **Folder...**,
which choose the source the maps are read from; while no map is open the
viewport shows the source's map grid (`ui/map_list.rs`), which also
opens a map. A package's row opens to its items on a double-click, and
**All packages** goes back.

Double-click on a tag, or **Open** in its right-click menu, runs
`Action::OpenAsset`: it opens the map that holds the tag (the open map
when it holds it, else the first that does; a Halo 1 map shows
read-only), then the tag as a tab in the editor its group gets
(`app::assets`, `App::open_asset`; a scenario tag is the map's own tab).
A variant line opens the tag on that variant's first map. **New from
this...** asks for the new path and writes a `like:` tag to the edit
target via `asset.copy`. With no target, the edit-target picker opens.
The copy opens in its package source tab.

A row's right-click menu and the list itself hold the rest
([studio-flow.md](studio-flow.md), "Links" and "Interfaces"):

| Part | Does | Code |
|---|---|---|
| **Where used**, **Show edits** | replace the list with the asset's users, or the items that change it; **Clear** brings the list back | `app/assets/browse.rs` |
| **Open as fields** | opens a tag's tab in the field panel in place of its group's editor | `Action::OpenFields` |
| Thumbnails | a bitmap tag the open map holds, and a package's PNG bitmap item, draw their picture a line high; hovering shows it larger | `app/assets/thumbs.rs` |
| A dropped file | a WAV, a PNG or a model YAML dropped on the window becomes an item of the package the Packages root shows or has selected, else of the edit target; any other file opens as a source | `app/assets/drop.rs` |
| **Forge variants**, **Forge items** | two kinds of the Kind picker | `app/forge_variants.rs`, `app/assets/forge.rs` |

Folders in the tree are not built. A thumbnail for a model, a sound's
waveform or a screen is not built: `asset.thumb` answers `not-yet`. The
operating system gives a drop no row to land on, so the package is the
one the Browser shows, not the one under the pointer.

A loose `.map` (a community mod) opened from **File > Open** is stored
as an import layer (W9) and its tags join the tree under its name.

### Editing a disc asset

A disc asset opens editable. The first change adds an edit line to the
edit target (`edits/<name>.yaml`, the format of
[tag-workspace.md](tag-workspace.md), "Edits are data"); the inspector
marks the row and names the package. There is no "Save as package" form
and no standalone-map save: **Save** writes the package's files, and
**Export map...** (File menu, `doc.save_as`) writes a whole `.map` for use
outside the game. With no edit target set, the first edit asks for one (a
picker dialog: an existing package or **New package...**); the change
waits for the answer and runs when it is given, and the agent API answers
`NO_TARGET` instead of asking.

`app::edit_target` is the one path. The tag editor holds the bytes
(`TagDoc`); after every write, undo and redo `app::edit_lines` says what
differs from the file as `set` lines by field path, and the lines go to
the target as `edits/<map>.yaml`, an `edits` item for that map
(`package_doc::put_edits`, which locks the package). The window's field
row and `tag.set` both write bytes through `TagDoc`, so a field set either
way is the same line; the file is the journal, so an undo that returns a
field to the file's value takes its line out, after a save too. Lines of
tags the session has not changed stay as the file has them. What a `set`
line cannot say (a list that grew, shrank or moved, a data reference's
bytes, a field the definition does not name) is reported once in the log
and is not written; those edits are not in the package yet. The edit
target is `<data>/packages/target.txt`. Studio sets
`App::edits_to_target`; a document held by a test does not write into
`<data>/packages/`.

### Preview and the paired game

Every document's preview has one target switch, the three views of
[live-editors.md](live-editors.md): **Studio** (the native view or the
kind's own renderer), **My game** (a game Studio started) and
**Attached** (a game someone else started). The link pill, pairing and
Pending are the built ones ([live-mirror.md](live-mirror.md)). The
[Game panel](game-view.md) shows the game's frames and the preview
result. **In game** (Cmd/Ctrl+Enter) in the header and **Preview** menu
runs the document form of `pair.preview`. On a variant it starts
Studio's own game with the open record. On a script it reloads into
Studio's own game, starting it if needed. The
[agent API](agent-api.md#the-preview-switch) lists the paired kinds.

### The editor follows the tag

| Opening | Gives | With | Is today's |
|---|---|---|---|
| a scenario (`scnr`) | the map editor: the viewport with gizmo and layers | a **Scene** tab beside the browser; Pending and Game below; **Playtest** runs and their overlays ([playtest.md](playtest.md)) | Scenario tool, Scene, layers, collision and nav picks, the Halo 1 overlay, the Modeling section |
| an object (`weap`, `vehi`, `bipd`, `scen`, `eqip`, `proj`, `hlmt`) | the object editor: the model, variants, markers, the tuning sheet | the model preview | the Tags tool, `model.*` |
| a model (`mode`, `coll`, `phmo`) | the model view and the Modeling tools | | the Modeling section |
| an animation graph (`jmad`) | the animation editor | the Timeline | nothing (new) |
| a sound (`snd!`, `lsnd`, `udlg`) | the sound editor | the Timeline | the Sound window and the voice document |
| a bitmap (`bitm`) | the canvas: paint, import, export | | the Textures tool, image generation |
| a shader (`shad`, `stem`, `spas`) | the Shader tab and its preview | | the field panel |
| an effect (`effe`, `prt3` and their kin) | the effect editor | the Timeline; the paired game as preview | the field panel |
| a screen (`wgit`) or our screen YAML | the 640 by 480 canvas | Problems | the UI Screens tool |
| a HUD (`nhdt`) or a HUD kit file | the same canvas with the HUD's states | Problems | the HUD tool |
| a `.rules` script, a variant, a playlist file | the variant's settings, the script read-only, check and sim reports | Problems | the Variants and Playlists tools, the Scripts tab, the game type script window |
| a package | its manifest, items, order and build ("Studio's side") | Problems, Log | Save as package, the package cards, the Map Pack window |
| any other tag | the field panel | | the Tags tool |

Panels a document brings (Scene, Timeline) dock where the layout has
them and go when another document is in front; nothing else moves.

**A scenario open.**

```
+----------------+---------------------------------------------------+-----------------+
| BROWSER | SCENE| lockout.scnr x                                    | INSPECTOR       |
| [filter......] | [Move][Rotate][Scale] Snap 0.5  Show v  Lighting v| Weapon spawn #4 |
| v Weapons (12) |                                                   | Transform       |
|   #4 battle r. |                 3D viewport                       |  x 3.2 y 1 z 0.4|
| v Vehicles (0) |           placements, layers, gizmo               | Object          |
| v Spawns (24)  |                                                   |  battle_rifle > |
| v Volumes (3)  |                                                   | Tags            |
+----------------+---------------------------------------------------+-----------------+
```

**Scripts.** A `.rules` file opens in the built script editor
(`app/rules_edit.rs`, `ui/game_script.rs`: completion, rename, the
formatter, problems), with its options as a form, its check and
simulation reports (`rules::workbench`) and the header's **In game**
(`rules.reload` on Studio's own game). The same file is edited in VS Code or Zed through the
extensions ([rules-editors.md](rules-editors.md)); **Open in editor**
(`app/external.rs`) goes there and Studio follows the file on disk.

### Navigation

- Cmd/Ctrl+P is Go to anything; Cmd/Ctrl+Tab cycles documents;
  Alt+Left and Alt+Right step the navigation history across documents
  (`app/navigation.rs`).
- A reference row's **Show this tag** opens or focuses the target's
  document, so following references is how one moves from a weapon to
  its model, its animations and its sounds.
- One layout persists in `h2-editor.ini`; **View > Reset layout**
  restores it.

### The API

Every row above is a method. The shell's own, in
`crates/apps/studio/src/api/shell.rs` (new):

| Method | Does |
|---|---|
| `shell.tabs`, `shell.open {asset}`, `shell.focus {doc}`, `shell.close {doc, discard?}` | the document tabs; `doc.*` takes the same ids |
| `asset.list {root?, package?, kind?, map?, query?, limit?}` | the browser's rows with kind, origin badge and the packages that edit them |
| `asset.thumb {asset, out?}` | a thumbnail as a PNG path |
| `asset.copy {asset, package?, to?}` | New from this |
| `asset.where_used {asset}`, `asset.edits {asset}` | the Browser's link views |
| `asset.drop {file, package?}` | a dropped source file into a package |
| `shell.open {asset, fields: true}` | Open as fields |
| `forge.variant.*`, `forge.items` | Forge map variants and the Forge palette |
| `problems.list {source?}` | the Problems panel |
| `preview.target {doc?, target?}` | the preview switch |
| `package.*` | below, "Studio's side" |

`doc.open` loses its per-tool forms and takes an asset. `tag.*`,
`scenario.*`, `variant.*`, `playlist.*`, `ui.*`, `hud.*`, `pair.*` and
`mirror.*` keep their params; where they took `doc: "map"` they take a
document id.

### What goes away

- `ToolKind` as a mode, the tool tabs and the per-tool `ToolWindows`
  (`ui/layout.rs`): the editor comes from the tag's kind.
- Opening a map as the unit of work: the tree holds every map's tags
  at once.
- The Maps list (its window, the Map Pack group and the package rows)
  is gone: the browser. The map grid and its cards (`ui/map_list.rs`,
  `app/map_cards.rs`), the Tags tab's tree as a separate list and the
  per-tool list panels go with the slices that give their documents a
  place in the browser.
- **Save as package...**, the package form of `doc.save`, **Save as...**
  for a `.map`, `active.toml`: the edit target and **Export map...**.
- The floating Sound, Voices, Image and Map Pack windows: panes of the
  sound, bitmap and package documents.
- `.h2tag` bundles as a way to move content between maps
  (`app/tag_bundle.rs`): a bundle copies a map's bytes, so it cannot go
  in a package. Moving a tag is **New from this...**, which writes text.

## Packages

### The model

A package is a folder with `package.toml` and the files it names. It is
the unit of authoring, building, enabling, sharing and credit.

- Ours live in Git under `content/<id>/` and are baked into `h2`: boot's
  build script `include_bytes!`s every file of each folder that holds a
  `package.toml` (`crates/apps/boot/build.rs`), read back through
  `workspace::baked`.
- The user's live under `<data>/packages/<id>/`; so do packages
  installed from Live.
- A package holds **sources and data**, never code. What turns a source
  into tags is a **builder**: a Rust function in a closed registry
  (`workspace::builders`, new). A package names builders; it cannot add
  one. Behaviour stays where it is: game features in Rust
  (`pogo_ride.rs`, Forge) and `.rules` scripts in the rules VM. A
  package says which features it needs. A community package may use
  the data builders only; the `kit` builders are for ours.

```toml
# content/pogo/package.toml
[package]
format = 1
id = "pogo"                        # lowercase letters, digits, '-'; unique in a set
name = "Pogo Stick"
description = "The stick a Superbounce player carries, in three looks."
image = "preview.png"
requires = ["core"]                # must be present and load first
after = ["superbounce"]            # order only, when present
features = ["pogo"]                # game features the content needs

[credit]                           # live_proto::maps::Credit
authors = ["Alexander Forbes-Reed"]
notes = "Original shapes and paint."

[[item]]
kind = "model"                     # a builder
source = "models/unsc.yaml"
tag = 'objects\weapons\multiplayer\pogo\unsc\pogo_unsc'
maps = "multiplayer"               # where it is added

[[item]]
kind = "tag"
source = "tags/pogo_unsc.weap.yaml"
maps = "multiplayer"

[[item]]
kind = "sound"
source = "sounds/spring.wav"
tag = 'sound\weapons\pogo\spring'
maps = "multiplayer"
```

`maps` is `"multiplayer"`, `"campaign"`, `"mainmenu"`, `"shared"`,
`"all"` (when omitted) or a list of map names. Unknown keys are refused
(`deny_unknown_fields`, as `content::Manifest`).

### Contents

| `kind` | Source in the package | Builds | Scope |
|---|---|---|---|
| `tag` | `<path>.<group>.yaml`: `like:` a disc tag or the group's defaults, then fields by name ([tag-workspace.md](tag-workspace.md), "Authoring and importing"); with `replaces = "<asset hash>"`, the asset at `tag` overridden whole (built for `tag` items; the `model`, `bitmap`, `sound` and `animation` builders refuse `replaces` by name) | a new tag, or an asset overridden | sim |
| `edits` | an edit file: `set` and `block` lines on tags that exist; a line's `from` names the value it replaces | changed fields | sim |
| `model` | our primitive YAML (`content/pogo/models/*.yaml`'s form) or glTF | `bitm`, `shad`, `mode`, `phmo`, `hlmt` (built: the primitive YAML, with the atlas it names) | sim |
| `bitmap` | PNG | `bitm` | client |
| `tag-delta` | a BPS patch from an asset's image to the changed asset's ([asset-store.md](../engine/asset-store.md), "Mods and packages"); `tag` is the asset's path, the group the source's second extension (`a.jpt!.bps`), `was` the asset hash of its base (optional) | the patched asset | sim |
| `animation` | `.anim.yaml` with the graph, skeleton, key tracks and replace or add operation | animation records and streams in a `jmad` | sim |
| `sound` | WAV and an optional `.sound.yaml` (permutations, class, edits, mouth curve) | `snd!`, the gestalt merged (built: an item with a `tag` is written as a `snd!`; one with none is checked and a game feature plays the file) | client |
| `screens` | screen YAML and `design.yaml` | `wgit` tags and `wgtz` rows ([ui-in-maps.md](../game/ui-in-maps.md)) | client |
| `screen-edits` | retail screen edit files | changed `wgit` fields | client |
| `hud` | HUD kit YAML (a `hud_kit_core::model::Hud` as data) | HUD kit records | client |
| `strings` | YAML by name and language | `unic` rows | client |
| `rules` | `.rules`; `inputs` are joined after the source (CTF and Assault are a header then the shared body) | a compiled script in the rules registry (built: checked at build, read by `workspace::script_registry`; a `rules` item of `core` is a retail type's game) | sim |
| `variant` | a settings or scripted `.rules` file or a portable `.h2variant` record | the boot list reads script-based entries in manifest order | sim |
| `map-variant` | a `.h2mapvariant` file, a Forge document of the variant tier ([forge-map-variants.md](../game/forge-map-variants.md), "A map variant in a package") | no tag: the game lists it with the map variants saved on the machine (`workspace::map_variants`, `forge::files::variants::install_packaged`) and builds its map when a lobby picks it | sim |
| `playlist` | a TOML hopper fragment (`content::playlist::Fragment`) or an edited `multiplayer_hopper.dat` | the server reads TOML fragments through `workspace::hopper_fragments` | Live |
| `map` | `map.toml`: `mode` (`fresh`, `onto`), `base`, the new map's `name`, `scenario` and `kits` (`fresh`), and `ground` when it has AI; the items it takes are those whose `maps` names it | a derived map package under `<data>/maps/`, built at prepare; a `map` with `ground` has the AI ground built over the map it makes (built: `workspace::prepare`, `Fresh::ground`) | sim |
| `map-patch` | a PPF, IPS or BPS file, and in `inputs` one TOML file with the base's `map` name and `sha256` | a map package with that layer, the one `h2 live maps import` writes for the patch (built) | sim |
| `convert` | a recipe, `<name>.convert.toml`: a `converter` (`halo1`, `map-pack`) and its `[settings]`; the input is the user's own disc | map packages and campaign files under the data root, at prepare (built: `workspace::convert`, "Conversions") | sim |
| `data` | a file a named game feature reads (the Pogo's pose set) | handed to the feature at boot | per feature |
| `kit` | a named code builder's settings: `source` is `kits/<kit>.kit.yaml`, whose name says the kit (`campaign-cards`, `glyphs`); first-party packages only | what that builder adds to the files it is for; its inputs are the game's, so a build on a map leaves it out (`workspace::kits`) | per builder |

**Scope** is the builder's, not the author's: `sim` items change what the
simulation reads, so every peer of a networked game needs them; `client`
items change only what one player sees and hears; `Live` items are read
by the server. Which tag groups a client item may touch without moving
the sim hash is **S**: the check is a lockstep run with and without the
item ([live-coop.md](../live/live-coop.md)).

**What a package may not hold.** `package.check` refuses, naming the
file: a cache map; a `.h2tag` bundle; a `tag` item whose YAML carries raw
bytes; a `map-patch` without its base hash; anything under a voice
dataset or model weights folder ([voice-cloning.md](voice-cloning.md));
a file over the store's size cap. `h2 package check --store <data>/tags`
refuses a file whose SHA-256 is a raw resource or a tag file of the user's
own tag store (`workspace::store_findings`): no list of the disc's hashes
is shipped. Live refuses any blob whose hash is on the operator's retail
list, as it does for maps ([maps.md](../game/maps.md) section 5).

### Identity, dependencies and load order

- **`PackageId`** is the SHA-256 of the manifest's canonical JSON
  (`content::canonicalize`) with each `source` replaced by its file's
  SHA-256 and each `inputs` path paired with its SHA-256. An item's
  `replaces` and `was` are in its entry as the asset hash's lowercase text.
  Display fields and `[credit]` are omitted, the way
  `MapId` is made. The build writes the file hashes to `package.lock`
  beside the manifest; the lock is what is signed.
- **`requires`** names packages that must be in the set. A missing one
  disables the package with a line in Problems and the boot log; it is
  never loaded half.
- **Order**: a topological sort over `requires` and `after`, ties broken
  by the user's order (`<data>/packages/set.toml`, which also holds what
  is switched off), then by id. A cycle is refused, naming its members.
  Every package is on unless the set file turns it off. `core` and `ui`
  are always on (`content::package::ALWAYS_ON`): a set file whose `off`
  names one is refused by name (`SetError::AlwaysOn`), and they may not
  `require` anything, so no other package's absence disables them.
- **Conflicts.** In `h2 package build` and Studio's `package.build` two
  packages that add the same tag path stop the build, naming both. Two
  edit lines on the same field: the later package wins and the report
  lists the override (`Report::overrides`). An edit whose path or `was`
  hash does not match stops the build with the line. The game's build
  leaves a refused package off for the map with its reason instead
  ([asset-store.md](../engine/asset-store.md), "Conflicts").
- **The content set** of a game is the ordered list of the `PackageId`s
  that have a `sim` item. Its hash goes where `map_id` goes (below).

### The build

`workspace::build(set, packages, target, unpin)` applies the loaded
packages, in order, to a map's `map::build::TagSet` for one `Target` (the
map's name and class). An item runs when its `maps` takes the target;
its builder comes from the closed registry (`workspace::registry`: `tag`,
`edits`, `bitmap`, `model`, `sound`, `data`, `hud`, `screens`,
`screen-edits`, `rules`, `variant`, `playlist`, `map-variant`, `map`,
`map-patch`, `convert`, `kit`, `tag-delta`; the other kinds stop the build with "no
builder"). The `map` and
`map-patch` builders change no map: they build map packages at prepare (below). The
base is what is built onto: the result differs from it only in the tags
the items add or change, and a tag a decomposed map keeps pinned is laid
out anew (through `Unpin`, `MapUnpin` for a map file) only when an item
changes it. After an error the set is partly changed and is dropped.

- **`tag`** (`<path>.<group>.yaml`, the group the second extension):
  `like:` a tag of the same group in the map, else the group's defaults
  (`tags::defs::default_record`); `block:` lines (`{path, op: add|remove,
  like|index}`), applied first; `fields:` a mapping from field path
  (`a.b[2].c`) to value. A tag reference is `<group>:<path>` and names a
  tag the base or an earlier item has. `borrow:` takes fields from a tag
  the map already has, chosen by what it holds (`from: weap`, `where:`
  alternatives of text fields, `take:` field paths): the first alternative
  some tag matches, with every taken block non-empty, gives the values; no
  match stops the build. It is how a weapon takes the first-person view of
  the map's own weapon of a class. With `replaces = "<asset hash>"` the
  item takes the asset its `tag` names instead of adding a free path: the
  asset is in the map and hashes to it (`Cx::replace_target`), the tag
  keeps its datum, and the asset's hash as the item leaves it is kept for
  a later package's `was` (`Report::replaced`).
- **`edits`**: a YAML list of `{tag: <group>:<path>, was, block, set}`;
  `set` is a list of `{path, value}`. Values follow the field's type:
  numbers, enum and flag names or numbers, text for string IDs, lists for
  vectors, `<group>:<path>` for references, `null` to clear one. A block
  is changed with `block` lines. A line's `from` is the value it replaces,
  checked against the tag as the build first found it (over an earlier
  package's whole override, against that): a `from` that is not a value
  the field takes, or that the field does not hold, is a refusal.
- **`tag-delta`** (`workspace/src/delta.rs`): a BPS patch from the base
  asset's image to the changed asset's. An image is the asset as the build
  holds it (its arrays in tree order, each its bytes and the fields the
  builder fills in, by what they name); the patch's source size and CRC32
  must match it, so a patch made on other bytes is refused too, and the
  tree it writes is read back with every length, index and alignment
  checked. A raw resource the patch names is one the map holds.
- **`bitmap`**: a PNG (any depth or colour type) as one linear `ARGB8888`
  `bitm` with no mips, through `tags::rgba_bitmap::install`.
- **`screens`** (`workspace/src/screens.rs`) takes `design.yaml` as its
  source and names its screen YAML files in `inputs`. The map supplies the
  templates. The builder adds the screens in game ID order and lists them
  in the map's `wgtz` tags.
- **`screen-edits`** takes one retail screen edit file. Its `maps` list
  names the map in the file. In `content/ui`, these entries precede
  `screens` so the generator reads the edited templates.
- **`model`** (`workspace/src/model.rs`): the source is our primitive YAML
  (`content_kit::source`), its `atlas` names `<atlas>.png` beside it (a `data`
  item of the package, so the package's id covers it). The item's `tag` is
  `<folder>\<look>\<name>`; the builder adds `bitm` (the atlas as a DXT5 mip
  chain), `shad` (written on a shader template the base map has, with the
  atlas as its base map), `mode` and `hlmt` at that path, and `phmo` and
  `nhdt` at `<folder>\pogo`, shared by the package's looks and added by the
  first. What the map lends (the shader template, a global material) is
  read from the base map once by `build_map` (`Cx::shading`); a build with
  no map reader refuses a `model` item by name.
- **`sound`** (`workspace/src/sound.rs`, `workspace/src/import.rs`): a
  16-bit PCM WAV, mono or stereo, at any rate. With no `tag`, the clip is
  checked and no tag is added: the game feature that plays it reads the
  file. With a `tag`, the builder writes a `snd!` of that path: the clip
  is resampled (a Hann-windowed sinc, `audio::resample::resample_sinc`) to
  the nearest of 22,050, 32,000 and 44,100 Hz that is not below it,
  encoded as Xbox ADPCM (`audio::adpcm::encode`, whole 64-frame blocks, the
  last padded with its final frame), cut into `CHUNK_LEN` chunks that join
  the map as raw resources, and given one pitch range of one permutation
  in the map's `ugh!`. The gestalt is read from the set (or started
  empty), extended and written back in its own slot, so the rows of every
  sound already in it, and the raw its chunks name, stay as they were; the
  report lists it as changed, or as added when the build made it. The
  class is the default (`ui`, 51) or the `class:` of a sidecar,
  `<source stem>.sound.yaml`, which is a `data` item of the package so the
  package's id covers it. The `snd!` takes its distances from the class
  and has no cone.
- **`data`**: any file; no tag is added (a feature reads it, or another
  item's builder does).
- **`map`** and **`map-patch`** (`workspace/src/maps.rs`) change no map.
  `workspace::prepare(maps, packages, bases, fresh)` builds them to map
  packages in `<data>/maps/`. A `map` item's `map.toml` names `mode`,
  `base` (a disc map, read through `BaseMaps`), `name` (the new map, which
  is not the base), and for `fresh` a `scenario` and `kits` that a `Fresh`
  builder turns into a whole map (the kits live above `workspace`; none
  given stops the item with a named error). `onto` starts from the base
  as it is and needs at least one taken item. The items the map takes are
  every item that changes a map and lists the map's name in `maps`; they
  are applied through `build_map`, so the map is what the game would
  serve from the same items. The output is a derived package (the whole
  map as its one layer, `[list] base` the base) written to `<name>/`.
  A `map-patch` item's source is the patch as given and its one `inputs`
  file holds the base's `map` and `sha256` (and optionally `title`,
  `author`, `description`, `dir`); the output is `Manifest::retail` over
  that base with the patch as the one layer, so its `MapId` is the one
  `h2 live maps import` gives the same patch. It reads no disc. A `map`
  item with a `ground` file has the AI ground built over the map the item
  makes (`Fresh::ground`, `boot::map_ground`); its key holds the ground file
  and the generator's version.
  Each directory a build writes holds `built-from.toml`: the package and
  item that made it, the map's id and a key over the base map's hash, the
  package ids, the builders' version and (for `fresh`) `Fresh::version`.
  A directory that holds the key for the same item is not built again
  (`workspace::held_in`); one that holds another item's build, or none, is
  refused by name; the same item with another key is replaced.
  `PackageRewrite` serves a map a `map` item built without the items it
  took (`workspace::without_taken`), which are in its bytes already.
- **The stage** is `content/stage/` (baked): a `map` item whose
  `map.toml` is also the tag workspace's project for the tree build
  ([tag-workspace.md](tag-workspace.md), "Projects").
  `stage_kit::fresh::StageFresh` is its `Fresh`: it reads the base and the
  shared caches through `BaseMaps` and runs `stage_kit::map::build`; its
  version is the kit's and the layout's hash.
- **A test map** is a package of an `onto` `map` item and an `edits` item
  for the scenario (`workspace::test_map_package`). `scenario_edits(base,
  edited)` writes the edits between two map files' scenarios: `block`
  lines that remove or add placements at the end of a block, `set` lines
  for each field that differs, and a check that applying them gives the
  edited scenario (a field it cannot say as a value stops it, named).
  `boot::test_maps_recipes::recipe_package` runs a placement recipe on the
  base's bytes and returns that package; the Firefight probe is its kit's
  and is not one.
- **At boot** `boot::packages::prepare_maps` calls `workspace::prepare`
  with the disc's maps (through the VFS's disc backend) and `StageFresh`,
  after the set is read and before the content library reads
  `<data>/maps/`. An item that cannot build is a line on stderr.

`was`, and a `replaces`, name the asset hash of the tag as the build first
found it: `tags::repo::normal::hash` over its normal form, read for the
level, when the build is given the base's assets (`workspace::assets`:
`build_map_settled` is, from the store's catalogue or the map's bytes);
`workspace::tag_hash` (group, bytes and fixups, references by name, raw
resources by their bytes) when it is not, as `h2 package build` and a
build on an invented map are not. A tag added by an item has its own
hash. The error for a stale `was` gives both hashes and the line of the
entry.

A **refusal** is an item for other content (a `from`, `was` or `replaces`
that does not match, a patch of an asset an earlier package overrides
whole without naming its hash, a second override of one asset or a second
add of one tag path, a key the map lacks). `build` stops on it as on any
failure. `build_map_settled` leaves the package off for the map whole,
with its reason in `Report::refused`, and every package that requires one
off for the map too; a package in `strict` (ours) stops the build instead.
A package refused after one of its items ran is taken out by building the
map again without it.

A tag path two items add, or one an item adds that the base has, stops
the build and names both items. Two packages that set the same field
leave the later one's value and an entry in `Report::overrides`.

`workspace::cache_key(base, packages, target)` is the SHA-256 of the
base file's hash, the map's lowercased name and class, the builders'
version (`BUILDERS_VERSION`, bumped when a builder's output changes) and
the `PackageId`s of the packages with an item for the target, in order.

`Layered` is a `TagSource` over sources in order, first hit wins; a key
held by several is listed once.

`h2 package check <dir>...` reads package folders (or folders of them),
orders them as the game would (`content::package::Set`) and reports, one
line each: a manifest that does not read, a missing, stale or unlisted
`package.lock`, a cache map, tag bundle, disc file, model weights file or
dataset folder in the folder, an item with no builder, a missing or
oversize source (`MAX_FILE_BYTES`), a source its builder refuses (with its
line), a `replaces` its builder cannot act on, and two packages that add
the same tag or override the same asset whole for maps that can meet.
`h2 package check --store <data>/tags <dir>...` also names a file whose
SHA-256 is a raw resource or a tag file of the store.
`h2 package build <dir>...
--base <map> -o <out.map> [--map NAME] [--scope CLASS] [--no-lock]` runs
the same checks, applies the packages to the base map and writes the
result and each package's `package.lock`, then prints what was added,
changed and overridden, and the cache key.

### How the game consumes packages

`boot` builds one set, `workspace::GameSet` over a
`content::package::Set` (`boot::packages::at_boot`): the baked packages,
then each folder under `<data>/packages/` that holds a `package.toml`, by
name, then `set.toml`. A local package with a baked one's id is not
loaded; a `set.toml` that does not read, or that switches `core` or `ui`
off, is not used and every package is on. Each such line goes to stderr
and to `packages.list`. The set is used in four places, which replace the
paths in "What exists":

1. **Prepare**, before boot (`h2 play`, `h2 run`, `h2 serve`):
   `convert` items run when their input disc is present and their stamp
   is stale (`package_prepare::before_boot`, "Conversions"); at boot
   `map` and `map-patch` items build to map packages under
   `<data>/maps/`. `new_maps::install_at_boot`, the resolver and `MapId`
   are unchanged: a map package stays the form the VFS loads and Live
   names.
2. **Per-map items**: one `vfs::Rewrite`, `workspace::PackageRewrite`
   (`crates/tools/workspace/src/rewrite.rs`, made in
   `crates/apps/boot/src/packages.rs`). For a `maps\<name>.map` the game
   reads, it takes the packages with an item whose `maps` matches the
   name or the header's class, leaves out a package whose added tags the
   map already has (a map that holds the content), and runs the rest in
   order through `workspace::build_map_streamed`: the build
   `h2 package build` takes (`workspace::build_map`), with the same
   bytes, on the map read by offset. The map is decomposed with its file
   prefix (raw data, BSP regions) left on the disc
   (`map::build::TagSet::from_map_streamed`); the build holds the
   header, index, names and metadata, and serves an overlay over the
   map (`vfs::Rewritten::Over`), so a campaign map of any size takes its
   packages. A screen item, which pins the map again, lays the set out
   as an overlay over the map and decomposes that file the same way
   (`workspace::onto::lay_over`); the final overlay composes over it. A
   package whose item writes the prefix in memory is refused for that
   map by name. The overlay is kept in
   `<data>/packages/cache/<map sha>-<key>.splice` (16 hex digits of
   each, the key `workspace::cache_key`), its runs' bytes, with a
   `.json` receipt holding the full key, the splice's hash and where
   each run goes; a second read is a cache hit, and a changed map,
   package or builder builds anew. Test:
   `crates/tools/workspace/tests/streamed_rewrite.rs` (a 200 MiB map,
   byte-equal to the buffered build, peak memory bounded). A map that fails to
   build is served unchanged and named once on stderr; a package refused
   for the map (a stale `was`, a key it lacks) is off for it with its
   reason in `packages.list`, and the cached map is keyed by what the map
   takes. It is the only rewrite in boot's chain. The set's `kit` items run in it on the files
   they are for, with the inputs the boot gave it (`workspace::KitInputs`):
   `campaign-cards` runs `CardRewrite` on `maps\mainmenu.map` before the
   set's items, and `glyphs` runs the icon sets and then `GearRewrite` on
   each file of `maps\fonts`. `PackageRewrite::observing_fonts` hands each
   font file and the font table, as the game is given them, to
   `superbounce_font::Capture::keep`.
   `PackageRewrite::observed` hands
   each map, as the game is given it, to the boot's hook, which records
   the pogo stick's weapons (`content_kit::served::publish`).
   Boot reads one set for map preparation and mounting. The runtime
   rewrite and store level builder share the enabled package payloads.
   `packages.list` keeps the set's summary and reads map results from
   the served-map record. `vfs::RewriteBackend` keeps a 64 MiB cache for
   idle rewritten level payloads (held bytes), protecting open levels and retaining
   the shared resource maps separately. File metadata and directory
   listings reuse the rewritten sizes after eviction.
   With the maps served from the tag store (`store-maps`), the per-map
   items reach the game as level images instead: `vfs::store_map` asks
   `workspace::level::StoreLevels` for each level it opens, which builds
   the image with the same `build_map` and keeps it under
   `<data>/store/images/` by its `LevelId`
   ([asset-store.md](../engine/asset-store.md), "Runtime assembly"). The
   rewrite is then `PackageRewrite::levels_from_store`: it rebuilds no map
   and still runs the kits and the observers.
3. **Set items**: `rules` items fill the script registry
   (`rules::builtin::Registry`, whose baked form is `rules::builtin`'s
   constants); `variant` items are the built-in list entries
   `community_variants` builds (`workspace::script_registry`, installed by
   `boot::packages::at_boot`); `hud` items are what `hud_kit::install`
   compiles against the loaded map; `playlist` items are hopper fragments
   (`workspace::hopper_fragments`) that the server merges into the default hopper
   (`live-server/src/default_hopper.rs`).
4. **Features**: a package whose `features` names a switch that is off
   (`hooks::Switches::feature`, on unless switched off) is skipped whole,
   and so is a package that requires it, so turning `pogo` off still
   removes the stick. `core` and `ui` are never skipped for a feature.

Studio's build of a map (`package.build`) calls the same
`workspace::build`, with a package's sources read from its folder
instead of the baked copy, which is how a lane sees a changed atlas
without rebuilding `h2`. A dedicated host gets the same set in its data
bundle (`crates/live/host-data`).

This is the build of [tag-workspace.md](tag-workspace.md) with packages
as its unit: a project is a `map` item, a `ContentKit` is a builder, and
`KitRewrite` is `PackageRewrite`.

### Each content path as a package

| Package | Items | Replaces |
|---|---|---|
| `core` (built, PK8: the `rules` items) | `rules` (the seven retail types), `strings`, `kit: glyphs` (built, PK12: icon fonts, the gear) | `rules::builtin`'s engine constants, `icons::font_rewrite`, `bot_mark::GearRewrite` |
| `ui` (built, PK5) | `screens`, `screen-edits`; accepts `bitmap` items for art | `content/ui/` holds the design, screen docs and retail edits |
| `modes` (built, PK8: `variant`, `rules`, `playlist`, `hud`) | `rules` (the community modes, Siege, Firefight's director with its wave sets (built, PK12), which the director script is built from), `variant`, `playlist`, `hud`, `sound` (start lines), `map` (our Firefight and Siege maps, with `ground`) | the rest of the scripts of `rules::builtin` (now `content/modes/rules/`), `community_variants`' table of scripts, `crates/formats/firefight`'s built-in sets, `huds/firefight.rs` |
| `superbounce` | `rules`, `hud`, `sound` (the announcer clips), `screens` | `hud-kit-core/src/views/superbounce*.rs`' element lists, `art/announcer/` |
| `pogo` (built, PK4) | `model`, `tag` (the weapons), `sound` (the clips `pogo_sound.rs` plays), `data` (the atlases, the pose set `pogo_pose.rs` reads) | `art/pogo/`, `PogoRewrite`; `content_kit::{kit, mesh, source, texture}` is the `model` builder's code and `inject::add_pogo` the test's reference |
| `halo1` | `convert` (built, PK10: converter `halo1`, every multiplayer map in both sandboxes and the campaign), `kit: campaign-cards` (built, PK12), `screens` | `h1_import::auto_import` (gone), `CardRewrite` |
| `map-pack` (built, PK10: the `convert` item) | `convert` (converter `map-pack`: the nine maps and the title update from the user's Map Pack disc), `playlist` | `mappack_cmd::auto_install` at boot (`h2 import-mappack` and Live's capture still call it), the Map Pack rotations in `hopper/defaults.rs` |
| `stage` | `map` (`fresh`), `model`, `bitmap` | `content/stage/map.toml` ([content-stage.md](content-stage.md)) |
| a community mod pack | `map-patch` per map, `[credit]` | a Live mod pack's member manifests ([maps.md](../game/maps.md), "The mod repository") |
| a test map | `map` (`onto`) with `edits` | `scenario_edit::recipe` through `boot/src/test_maps_recipes.rs` ([test-maps.md](../game/test-maps.md)) |

**Game types are scripts.** Firefight and Siege are `rules` items like
Slayer: a script that awards points and declares its `type`, with no
retail game type under it
([game-types-without-carriers.md](../game/game-types-without-carriers.md)).
Firefight's wave sets are `rules` items of `modes`, and nothing is added to a map at boot for it. A
Firefight or Siege map is made in Forge or in Studio, and the AI ground
it needs (sectors, zones, squad slots, characters) is built on demand
when that map is made or edited
([game-types-without-carriers.md](../game/game-types-without-carriers.md),
"Maps for scripted games"). In Studio it is a `map` item with a
`ground` file (`workspace::GroundFile`, written by `ground.save`);
`firefight_kit`'s generator is its build step.

**Forge is not a package.** It is an engine feature
(`crates/extensions/forge`, [forge-design.md](../game/forge-design.md)):
its documents, materialiser and slot reservation stay code. Its screens
are data like every screen and live in `ui`; a map variant a player
makes in Forge is shared as a Forge document, as today.

### Conversions

A `convert` item holds the recipe only: which maps, which sandbox, which
base. The Halo 1 or Map Pack bytes come from the user's disc at prepare
time and the outputs stay under the data root. The converters stay Rust
(`crates/tools/h1convert`, `crates/apps/cli/src/h1_import.rs`,
`mappack_cmd.rs`); the package is what names and orders them, and what
other packages build on: an `edits` item whose `maps` names a converted
map is the overlay [modeling.md](modeling.md), "Converted Halo 1 maps"
describes.

- **The recipe** (`content/halo1/convert/halo1.convert.toml`,
  `content/map-pack/convert/map-pack.convert.toml`) names a `converter`
  from the closed list `workspace::CONVERTERS` and its `[settings]`. The
  `convert` builder checks it (`h2 package check`) and changes no map.
  `halo1` takes `campaign`, `halo2_assets`, `multiplayer` (true or false)
  and `sandboxes` (`"h1"`, `"h2"`) (`h1_import::package::Picks`);
  `map-pack` takes none. A key the converter does not know is an error.
- **The run** is `workspace::convert(data, packages, converters, force,
  say)`. A `workspace::Converter` finds its input (the disc beside the
  game disc), lists its units (a level, a map in one sandbox; the Map
  Pack is one unit) and converts the ones it is given, saying each as it
  finishes. `h2` has two: `h1_import::package::Halo1` (the import's jobs
  on its worker threads) and `mappack_cmd::package::MapPack`.
- **Stamps.** `<data>/packages/stamps/<package>.toml` holds, per item,
  the key each unit was converted with, or why it failed. A unit's key
  (`workspace::unit_key`) covers the converter's version
  (`h1_import::CONVERTER_VERSION`, the Map Pack stamp's version), the
  input's identity (the disc's size and modification time), the settings
  and the unit. The stamp is written after every unit, so a run that
  stops resumes with the units it had not reached; a unit whose key moved
  or whose output is gone converts again on its own; a unit that failed
  with the same key is not tried again until `--force`. The Halo 1
  import's own `import.stamp` and the Map Pack's `mappack.stamp` are kept
  in step, so `h2 import halo1` and `h2 import-mappack` share the work.
- **No disc.** A converter that finds no input gives its reason. With
  nothing converted before (no stamp entry, and `Converter::kept` false:
  no output of the converter's own command), the stamp records the
  reason as `off`, and `GameSet::read` lists the package off with it
  (`Off::Input`, "not converted: ..."). `GameSet::read_unprepared` is the
  set prepare runs on, so a disc that has arrived is found. Output
  converted before keeps the package on when the disc is taken away.
- **When it runs.** `h2 play`, `h2 run` and `h2 serve` call
  `package_prepare::before_boot` before they boot (in place of
  `h1_import::auto_import` and `mappack_cmd::auto_install`); it never
  stops the boot. `--no-halo1`, `--no-mappack`, `H2_HALO1` and
  `H2_MAPPACK` leave that converter out of the run, and its stamp as it
  was. `h2 package prepare [--disc] [--data] [--force] [--no-halo1]
  [--no-mappack]` runs it alone and prints what is off and what failed;
  it runs in `h2`, not `h2-tools`, because the converters link the disc
  readers (`tools::forwards`).

### Live: sharing, signing and credit

A package is published as a mod pack is today: files into the content
store by SHA-256, and the manifest with its lock as a document signed by
the author's key (the `MapPublished` flow of `live_proto::maps`).

- **Routes** ([live-api.md](../live/live-api.md), "Packages"):
  `PUT /v1/packages` takes the signed document, `GET /v1/packages`
  lists, `GET /v1/packages/{id}` gives the signed manifest and lock,
  blobs come from `/v1/content/{hash}`. A mod pack is a package whose
  items are `map-patch`; the mod pack routes go once `h2 live mods
  import` writes packages (PK14). Both homes run the same routes, and the
  upload check is one function (`live_proto::packages::check`) over each
  home's store (`Holdings`); the parity suite runs them.
- **Signing.** The server verifies the signature and every hash in the
  lock on upload: each must be stored (the store keeps blobs by their
  checked SHA-256), off the retail list and not taken down, and the lock
  must name exactly the manifest's files. A client verifies both again on install. Baked
  packages are trusted as part of the binary. A local, unsigned package
  loads in local games only: a Live or System Link session is refused
  while the set holds one with a `sim` item, naming the package
  (`workspace::GameSet::network_refusal`, `boot::packages::network_refusal`;
  the session's call to it comes with PK14).
- **Joining.** A session carries the content set's hash and its
  `PackageId`s beside `map_id`. A joiner missing one downloads it,
  rebuilds its cache and joins; one it cannot have (blocked, or its base
  is missing from the joiner's disc) fails the join with the reason,
  before any map load. Ranked hoppers pin the set.
- **Credit.** `[credit]` is `live_proto::maps::Credit`. The order of
  truth stays: an admin's correction (`PUT /v1/packages/{id}/credit`),
  then the publisher's signed `credit` in the document, then the
  manifest's. The publisher is shown as "uploaded by",
  never as the author. `docs/game/mod-credits.toml` and
  `mod-details.toml` keep their role for imported community packs.
- **Models.** A package with a `model` or `animation` item needs the
  uploader's `model_uploads` flag
  ([forge-features.md](../game/forge-features.md), "Custom models on
  Live"), set by `PUT /v1/admin/model-uploads/{gamertag}` (a
  `ModelUploadsSet` event). A `kit` item is refused: kits are ours.
- **In game.** The Mods screens (`mods_screen.rs`) list packages; a
  package's page lists its maps, modes and credit.

### Studio's side

A package opens as a document; the browser's Packages root is the set:

```
+----------------+---------------------------------------------------+-----------------+
| PACKAGES       | pogo  x                                           | INSPECTOR       |
|     core       | Pogo Stick             id pogo    [Check] [Build] | models/unsc.yaml|
|     ui         | requires: core         features: pogo             | kind   model    |
| [x] modes      +---------------------------------------------------+ tag    ...\pogo_|
| [x] superbounce| Items (14)                    kind     maps       |        unsc     |
| [x] pogo    *  |  models/unsc.yaml             model    multiplayer| maps   multipl. |
| [x] halo1      |  models/covenant.yaml         model    multiplayer| scope  sim      |
| [ ] map-pack(no|  tags/pogo_unsc.weap.yaml     tag      multiplayer| builds mode coll|
|      disc)     |  sounds/spring.wav            sound    multiplayer|        phmo hlmt|
| -- mine --     |  ...                                              |                 |
| [x] my-lockout | Credit: Alexander Forbes-Reed                     | [Open] [Remove] |
| -- Live --     | Last build: lockout 0.8 s, 31 tags, +212 KB       |                 |
|    Get more... | [Set as edit target]  [Publish...]  [Export...]   |                 |
+----------------+---------------------------------------------------+-----------------+
```

The list is the load order; dragging a row reorders within what
`requires` allows. A greyed row says why it is off. `core` and `ui`
have no checkbox.

| Method | Does |
|---|---|
| `package.list` | the set in order: id, name, `PackageId`, on or off and why, item counts, scopes |
| `package.read {id}` | the manifest, items with what each builds, the lock, problems |
| `package.new {id, name}`, `package.set {id, path, value}`, `package.item.add {id?, item, at?}`, `package.item.set {id?, index, path, value}`, `package.item.remove {id?, index}` | edits the manifest of a local package and rewrites its lock; a manifest the files or the set refuse is put back |
| `package.target {id?, clear?}` | reads, sets or clears the edit target (`<data>/packages/target.txt`); `id` is optional on the item calls, `package.check` and `package.build`, which then use it, and answer `NO_TARGET` with none |
| `package.enable {id, on}`, `package.order {ids}` | writes `set.toml`; `core` and `ui` are refused (`ALWAYS_ON`), as is an order that puts a package before one it `requires` or follows |
| `package.check {id?}` | the refusals above, missing sources, unknown builders, conflicts with the rest of the set |
| `package.build {id?, map?, out?}` | builds what the package and what it `requires` give a map of the open source, or every map it has an item for; `{maps: [{map, scope, touched, took_ms, tags, added, changed, overrides, bytes, problems}]}` |
| `package.play {id, map}` | builds, then loads the map in the paired game; refused when the game read another package set at boot |
| `package.publish {id}`, `package.install {id \| hash}`, `package.export {id, out}` | Live and a folder or archive |

`h2 package check|build|prepare|publish` is the same crate from the
command line, for the coordinator and CI.

## The animation editor

Opening a `jmad` gives the animation editor. It poses the graph's model,
edits clips on a timeline and saves authored keys to a local package.

```
+----------------+---------------------------------------------------+-----------------+
| BROWSER        | masterchief.jmad x            on: masterchief.hlmt| INSPECTOR       |
| Kind: Anim.    |                                                   | r_forearm       |
| v combat       |                 posed model                       | Rotation        |
|   rifle        |          (bones, markers, ground)                 |  y 12 p -4 r 0  |
|     idle       |                                                   | Translation     |
|     walk_front |   [|<] [<] [ > ] [>] [>|]  frame 14 / 42   30 Hz  |  0  0  0        |
|   > pistol     |                                                   | Key: linear [v] |
+----------------+---------------------------------------------------+-----------------+
| Timeline | Curves                              0    10    20    30    40             |
|  v pelvis        R T        o-----o-----------o---------------------o                |
|  v spine         R          o-----------o---------------------------o                |
|    r_forearm     R          o--o--o-----------o---------o-----------o                |
|  events          sound ^         effect ^                                            |
+--------------------------------------------------------------------------------------+
```

**Read and pose (slice M1 of [rich-editors.md](rich-editors.md), built).**
`tags::anim::AnimationGraph` lists modes, classes, types and actions;
`anim_sample::AnimationClip::sample` gives a pose at a frame for every
retail codec. The native view takes node matrices from the pose in place
of `rest_nodes` (`crates/tools/scenario-edit/src/native_view.rs`
`posed_nodes`, drawn by `native_view/preview.rs`'s posed render; the
Animation window and the `anim.list`/`anim.pose`/`anim.read` API over it are
`crates/apps/studio/src/{app,ui,api}/anim.rs`). The frame a decoded
rotation is in relative to the render model's skeleton was an open item
of [animation.md](../formats/animation.md); M1 settled it against the
engine's composition rule ([anim-sampling.md](../engine/anim-sampling.md)),
which the Halo 1 converter already writes to ([models.md](../halo1/models.md)):
the sampled channels merge with the render model node's defaults and
compose up its tree (animation.md, "Composing a pose"), an overlay
applying with the engine's scaled-add rule.

**Timeline (AN1 and AN3, built).** One row per node shows rotation,
translation and scale as lanes of key markers. The ruler uses frames
at the graph's 30 Hz. Click or drag to scrub. Play, step, loop and speed
use `app::anim::Transport`. An overlay can pose over a selected base
clip, shown beside it. The events row counts frame, sound and effect
events. Event and root-movement decoding are **E first** (see
[animation.md](../formats/animation.md), "Open items").

**The document (AN3, built).** `AnimDoc` in `crates/tools/anim-edit`
holds sparse keys per channel with step, linear or smooth outgoing
interpolation. Opening a retail clip reduces its dense frames with
`tags::anim_keys::reduce`. Its Ramer-Douglas-Peucker reduction measures
rotation as an angle, translation as a distance and scale as a
difference. The timeline shows the source tolerance. Each document has
its own undo and redo history, its steps numbered with the graph tab's
(`AnimDoc::share_steps`), so the window's Undo and Redo and the tab's take
them all in the order they were made
([studio-flow.md](studio-flow.md), "Shared patterns"). **Save to
package** writes a named `animation` item for the current map. Opening a source clip gives a
replacement item. A new or duplicated clip takes an action and can
specify a mode, class and weapon type.

**Curves (AN3, built).** The Curves tab shares the curve editor's plot
with a frame axis. It shows rotation as three unwrapped angles while
the document stores quaternions. Drag a key's component to edit its
value. Select a key next to a smooth segment to show its tangent
handles. One drag is one undo step.

**Editing keys (AN3, built).** Select a node in the model view or a
timeline row. Move, rotate or scale it with the gizmo to set the
corresponding channel at the scrub frame. For an overlay, the key stores
the change relative to its base. Shift-click keys in one lane to select
a range. Drag to move them, or copy, paste at the scrub head, scale in
time or delete. **New animation** makes an empty base clip on the
graph's skeleton. **Duplicate** makes a separate document from the
current keys. `anim.duplicate` can also start from the decoded source.

**Retarget (AN5, built as `anim.retarget`).** `anim_edit::retarget`
(`crates/tools/anim-edit/src/retarget.rs`) copies a document's sparse
channels onto another skeleton and keeps every key's frame and
interpolation. Nodes pair by case-insensitive name when the name is
unique on both skeletons (`h1-assets`' `models/names.rs` maps Halo 1
names the same way); explicit `{source, target}` pairs win, and a `null`
target drops a node. A base or replacement rotation becomes
`target_rest * inverse(source_rest) * key`, so a key equal to the source
rest lands on the target rest. A translation keeps its offset from the
rest pose, scaled by the ratio of the two local bone lengths (a root or
a zero-length bone keeps the offset). A scale scales by the ratio of the
rest scales. An overlay rotation and scale copy as they are; its
translation takes the length ratio. The reply's node table has one row
per source node with both names, then `unmatched_source`,
`unmatched_target` and `dropped_channels`. `preview` returns the table
without opening a document. Rest transforms come from the graph's model
(`to_model`, `from_model` pick one when several use the graph) and are
kept per graph, so a document can be retargeted from a map the Tags tool
no longer shows. In the Animation window, **Retarget from...** lists the
session's base documents (with their map) and the graph's own clips, and
makes a document on the shown graph from the one picked, with the name
field's name and the picked model's rest pose (`ui/anim.rs`,
`retarget_picker`).

**Import (AN5, built as `anim.import`).** `anim_edit::read_gltf`
(`crates/tools/anim-edit/src/gltf.rs`) reads one animation of a `.gltf`
(external or base64 buffers) or `.glb` as a clip on the file's node
tree, then the same retarget places it on the graph. Times round to
frames at 30 Hz, the file's axes become the engine's (the file's x, y, z
are the engine's y, z, x), `STEP`, `LINEAR` and `CUBICSPLINE` become step,
linear and smooth keys, and `matrix` nodes, sparse or normalised
accessors, morph weights and non-uniform scale are refused or listed in
`notes`. This is the "own animation" row of
[forge-features.md](../game/forge-features.md).

**Writing the tag (AN4, built).** The `animation` builder
(`crates/tools/workspace/src/animation.rs`) reads `.anim.yaml`, matches
the source skeleton to the graph by node order, name and parent, and
bakes integer frames with `anim_edit::encode`. A base clip puts constant
channels in codec 1 and moving channels in codec 3. Overlay and
replacement clips use codec 3 for their keyed channels. Unkeyed
channels keep their skeleton fallback.

The source names the graph path, clip name, frame count and nodes.
`operation` takes `{op: replace, animation: <clip name>}` or
`{op: add, mode, class, weapon_type?, action, kind?}`. A channel names
its `node` index and `channel` (rotation, translation or scale). Each
key has a `frame`, `value` and optional `interp` and `tangent`.
Values and tangents use the same coordinates as `anim.read`.

- **Replace** keeps the existing clip's index, kind, links, checksums
  and events. The builder allocates new raw bytes and changes the
  selected record's stream fields. If no other clip uses its resource
  slot, the builder reuses the slot. A shared slot and its bytes stay
  intact, and the edited clip gets a separate resource.
- **Add** appends a record and resource and adds the action under the
  selected mode, class and weapon type. Overlay actions join the
  overlay list. The lookup tables use the engine's packed string-ID
  order.

Both paths use the package build's `TagSet`, whether it starts from
built tags or decomposes an existing map. Builds refuse inheriting
graphs, shared caches, inline clip data and frame-info modes with root
movement. Replacing an aim screen or shortening a clip past a
retained event or loop start is also refused.

**In the running game.** A package build with new animation resources
takes effect on map load. The guest animation cache can keep its own
resident copy of the old resource while the map is loaded.
For live metadata edits, `hot_glue::tag_written` flushes the decoded
clips whose keys belong to the graph's current animation block and
any block it displaces. See [realtime-edits.md](realtime-edits.md),
"Tag-write notices".
`pair.preview {what: "animation", target, animation}` spawns the
object (`sim.spawn`), plays the animation on it through the game's
`world.object.animate` ([agent-api.md](../runtime/agent-api-input.md#worldobjectanimate))
and captures the frame after `ticks`
([agent-api.md](agent-api.md), "Previewing content in the paired game"). The game method
points the object's first animation channel at the named animation of its
own graph; whether the engine then plays it as an authored clip is
checked on a running game.

| Method | Does |
|---|---|
| `anim.list {tag}` | graph animations with frames, codecs and types, and the models that use the graph |
| `anim.pose {tag, animation, frame?, model?, out?}` or `anim.pose {doc, frame?, base?, model?, out?}` | sample a source clip or document, with a model render when available |
| `anim.read {tag, animation, tolerance?, keys?}` or `anim.read {doc, keys?}` | the source reduction and its document ID, or the document's current keys |
| `anim.key {doc, node, channel, frame, value?, interp?, remove?}` | set or remove a key |
| `anim.new {tag, name, frames}`, `anim.duplicate {tag, animation, name}` or `anim.duplicate {doc, name}` | create a new animation document |
| `anim.save {doc, package?, mode?, class?, weapon_type?, action?}` | save a map-scoped `.anim.yaml` item in the edit target or a specified local package |
| `anim.check {doc, model?}` | per-node codec quantisation and loop seam, and feet under the model-space ground plane |
| `anim.retarget {from, to, map?, name?, from_model?, to_model?, preview?}` | retarget an open document onto a graph; the node table and unmatched nodes |
| `anim.import {file, to?, clip?, map?, name?, to_model?, preview?}` | import a glTF clip onto a graph through the same retarget |

## The sound editor

Opening a sound tag gives the sound editor: the built audition (A1)
with editing around it.

```
+----------------+---------------------------------------------------+-----------------+
| BROWSER        | spring.snd! x                 [> Play] [Loop] [In game]  INSPECTOR  |
| Kind: Sounds   | Pitch range 0   permutations: 3                   | Permutation 1   |
| v pogo         |  [1] spring_a  0.42 s  w 1.0   -14.2 LUFS         | Weight   1.0    |
|   spring       |  [2] spring_b  0.39 s  w 1.0   -14.6 LUFS         | Gain     0 dB   |
|   compress     |  [3] spring_c  0.45 s  w 0.5   -13.9 LUFS         | Skip     0      |
|   settle       +---------------------------------------------------+ Class            |
| v announcer    |  waveform  ~~~~^^^^~~~~~~^^~~~~~~~~~~_____________|  weapon_fire  > |
|   double-bounce|  [Trim] [Fade in] [Fade out] [Gain] [Normalise]   | Distance        |
+----------------+---------------------------------------------------+  1 .. 9 wu      |
| Timeline         0.0      0.1      0.2      0.3      0.4           |                 |
|  mouth          ___/^^^^\__/^^\______                              | vs retail: ok   |
|  edits          trim 0.02..0.44   fade out 0.05                    |                 |
+--------------------------------------------------------------------------------------+
```

**Audition (built).** `app/audition.rs` decodes a permutation and plays
it through the game's own sink; `ui/sound.rs` is the sound document: it
draws the front tab's `snd!` while the Tags tool shows it (a browsed
map, which has no tabs, shows the sound selected there). A row for each
permutation says its place, length, integrated loudness and peak, and the
length of its mouth curve (`App::sound_rows`, the numbers
`sound.measure` gives each permutation); a permutation that does not
decode keeps its row with the reason. The waveform and the mouth track
sit under the rows on one time axis; under them **Preview on** picks a
biped to draw with its `talk` overlay at the playhead's weight (the face
preview, [rich-editors.md](rich-editors.md), slice A3). `sound.read`,
`sound.play`, `sound.stop`, `sound.mouth`, `sound.face` and
`sound.measure` are the API.

**Waveform editing (built).** Edits apply to a package's source WAV and are
kept as a list in `<name>.sound.yaml`, the sidecar of the `sound` item
(a `data` item of the package, added when the first edit is made): `trim`,
`fade_in`, `fade_out`, `gain`, `normalise` to a loudness target under a peak
ceiling, and `silence` of a range. The WAV is never rewritten; the edits
apply in order to its decoded samples (`sound_edit::render`), and the build
renders the list before it resamples and encodes
(`workspace/src/sound.rs`). Removing an edit, moving it and undoing the last
one are changes to the list, so the render before is the render after
them. A list that does not render (a trim that leaves nothing, normalising
silence) is refused by the place of the edit, and `package.check` names it
the same way. `crates/tools/sound-edit` holds the edits, the sidecar
document and a WAV reader and writer; the announcer's own trim and
normalise (`voice/audio.rs`) are not moved onto it. A disc sound is not
edited in place: **Copy
into** makes a `sound` item from a WAV the user supplies, and a disc
sound's fields (gain, distances, class) change through `edits`.

**Permutations (built as a list).** Other WAV files of the package are
listed after the source's own, each with a weight, a gain and a skip
fraction, and each its own edit list (`permutations:` in the sidecar). The
list shows each one's loudness after its edits, as `sound.measure` reads
it. The build writes the source's own permutation; encoding the listed
files into the `snd!` is not built.

**Mouth curves (slices A2 to A4).** The track under the waveform is the
permutation's mouth curve, one value per 30 Hz tick
([rich-editors.md](rich-editors.md), "Sounds and facial animation"). A2
reads it (built), A3 drives a picked model's `talk` overlay with it
(needs M1), A4 draws on it and fills it **From the sound**. For a
package sound the
curve is part of the `.sound.yaml` and the build writes it with the
gestalt's extra info; for a disc sound A4's in-place write stands.

**Import and encode (built).** `import.sound` takes a WAV at any rate
and adds it to a local package as a `sound` item (`app/sound_import.rs`):
the file is copied in unchanged under `sounds/`, and the manifest and lock
follow. The build (`workspace::import`) does the rest: resampled to a rate
the format field names, mono or stereo, encoded by `audio::adpcm::encode`,
split into chunks and written as a `snd!`, with its rows appended to the
map's gestalt by the Halo 1 converter's `ConvertedSound` path
(`h1_assets::sounds`; `workspace` reaches it through `content-kit`'s
dependency already). `merge_sound`, which takes a `snd!` and its rows from
another map's gestalt, is re-exported from `workspace::import` for the
import of a retail sound; the gestalt model itself stays in `h1-assets`,
which cannot depend on `workspace` (`workspace` depends on `content-kit`,
which depends on it). Looping sounds (`lsnd`)
take a start, a loop and an end file. The codec stream form (WMA) is
decoded for audition only; nothing encodes it. That every sound class,
dialogue included, plays from ADPCM is **S**: the converted Halo 1
campaign's dialogue is the evidence to check
([sounds.md](../halo1/sounds.md)).

**The announcer and cloned voices.** `voice.say` makes a line and
`voice.commit` puts it in the edit target as a `sound` item, in place of
copying into `art/announcer`. A cloned line
([voice-cloning.md](voice-cloning.md)) commits the same way and may ship
in a package; the dataset and the weights stay under the data root and
`package.check` refuses them.

**In the running game.** `pair.audio` measures what the game's mixer
does while a call makes the sound. **In game** on a sound document runs
`pair.preview {what: "sound", target}`: the package builds, the edit
reaches the game, the sound plays at the player and `pair.audio`'s
readings come back with the editor's own measure beside them. The
game's `audio.play` starts a sound tag at a player's unit through the
engine's own sound start ([agent-api.md](../runtime/agent-api-input.md#audioplay)).

| Method | Does |
|---|---|
| `sound.read`, `sound.play`, `sound.stop`, `sound.measure` | built |
| `sound.mouth {sound, permutation, set?}` | reads a curve (A2, built); the write is A4 |
| `sound.face {tag, at_ms, on}` | the face preview's weight and pose (A3, built) |
| `sound.edit {package, source, op, ...}`, `sound.render {package, source, out?}` | the edit list (built) and its result as numbers and a WAV (built) |
| `sound.permutation {package, source, op: add \| remove \| set \| move \| list}` | the permutation list (built) |
| `import.sound {file, package, tag, class?}` | a new `sound` item (built) |

## Other editors

Each is a document kind in `app::kinds::KINDS`, opened from the browser
as a tab, and each writes through
`TagDoc` into the edit target.

| Editor | Fits as | Stands on | First step |
|---|---|---|---|
| **Bitmaps** | the Textures tool as a document: paint, import, mips, sequences | `crates/tools/bitmap-edit` ([modeling.md](modeling.md), "Texture editing") | LE3 of [live-editors.md](live-editors.md) |
| **Shaders** | a Shader tab with a sphere, quad or object preview | `tags::shader`, rich-editors S1 to S6 | S1 |
| **Colours and curves** | pickers and plots inside any field panel | rich-editors C1 to C3, F1, F2 | X1 |
| **Effects and particles** | one document for an `effe` and what it spawns, with curves on the shared Timeline; the preview is the paired game, because the native view draws no effects | `tags::effects`, the game view | LE7; a host call that spawns an effect at a point is **E first** |
| **Objects** | one editor per object: model, variants, markers, attachments, the tuning sheet | `tags::object`, `model.*` | LE5 |
| **Scenario placement** | the map editor a scenario opens | `crates/tools/scenario-edit` | built; AI squads and zones wait on the engine's AI readers ([scenario-editor.md](scenario-editor.md) section 6) |
| **Level geometry and collision** | the Modeling tools in the map editor's viewport | [modeling.md](modeling.md), [realtime-edits.md](realtime-edits.md) | their own slices |
| **Scripts (HaloScript)** | read-only from the scenario's **Scripts** tab, edited outside | rich-editors H1 to H4 | H2 |
| **Recordings** | a document on the shared Timeline | rich-editors K1 to K4 | K1 |
| **Strings** | a table by name and language; a `strings` item for ours | `tags::localization` | N1 |
| **Sky and fog** | a tag view with the dome | `tags::lighting` | LE8 |

## Rules and limits

- No game data in a package, in Git or on Live. A package names disc
  content by tag path and map name, and a base by hash only in a
  `map-patch`.
- A package runs no code. Builders are ours and the registry is closed;
  `.rules` scripts run in the rules VM under its tick budget
  ([rules-engine.md](../game/rules-engine.md)).
- Menu and HUD content is data in map files. No builder draws a screen
  in Rust.
- A package is loaded whole or not at all: off for one map whole when it
  is refused there, never in part.
- An edit is never dropped quietly: a line that does not resolve stops the
  build, and a line or an item for other content turns its package off for
  that map with its reason (and stops `h2 package build`).
- The window has no action the API lacks. `session.methods` lists every
  method, and each slice's headless test drives its feature through the
  API.
- Nothing is carried over: saved patch packages, `active.toml`, `.h2tag`
  bundles and saved layouts of the old tools are not read.

## Slices

Sizes: S under 800 lines with tests, M under 2,500, L more. One lane
each. Every test is headless on invented data. `W`, `LE`, `GV` and
`PT` numbers are slices of [tag-workspace.md](tag-workspace.md),
[live-editors.md](live-editors.md), [game-view.md](game-view.md) and
[playtest.md](playtest.md); `M1`, `A2` to `A4`, `F1` and `X2` are
[rich-editors.md](rich-editors.md)'s. PK2, PK3 and PK7 are W2, W4 and W5
cut to the package model; PK4, PK5 and PK9 to PK12 are W8's migrations.
Rows are in build order.

| # | Slice | Size | Needs | Accepted when |
|---|---|---|---|---|
| SH1 | **Document tabs and editors by kind** (built: tabs, own undo, editor by group, `shell.*`; `ToolKind` still stands): `ToolKind` goes; a document id per open tag; the editor from `app::kinds::KINDS`; panels that follow the front document; `shell.tabs`, `shell.open`, `shell.focus`, `shell.close`; one layout | M | nothing | each kind's editor draws at 1000 by 420 with no stray child window (`every_panel_scrolls_as_a_whole_on_a_short_window`); two documents have two undo stacks; the Timeline shows for an invented sound and not for an invented weapon; the window's clicks and `shell.*` leave the same state |
| W3 | **First start and the tree in Studio** (built; tag-workspace; it needs W1 only here) | M | nothing | as there |
| PK1 | **Manifest and set** (built): `content::package::{Manifest, PackageId, Lock, Set}`, validation, the canonical id, `requires`, `after`, `set.toml`, the order | M | nothing | golden ids on invented hashes; unknown keys, a cycle and a missing `requires` are refused by name; display fields and credit do not move the id; the order is stable |
| PK2 | **Builders and the build** (built): `crates/tools/workspace` with the registry, `Layered`, `build` on a `TagSet`, conflicts, the cache key, the `tag`, `edits` and `bitmap` builders, `h2 package check` and `build` | M | PK1 | on an invented map: two packages build in order; the same tag path twice stops naming both; a stale `was` stops with its line; `onto` output differs from its base only in changed tags |
| SH2 | **The browser** (built: the rows and `asset.*`, `app/assets.rs`, the panel `ui/browser.rs` and the removal of the Maps list; the Tags tab list and the per-tool lists stay until their documents are kinds of the browser): the Tags root over `TagRepo` by path with variants, the Packages and Live roots, `asset.list`, `asset.thumb`, `asset.copy`, Kind, Map, search, badges; the Maps list removed | M | SH1, PK1, W3 | an invented tree of two maps and one package lists one row for a shared tag and two variants for one that differs; the Map picker gives one scenario's closure; each word of a query must match; a tag an edit line touches carries the package's badge; **New from this...** writes a `tag` item with no bytes of the source; the panel lists the tree's rows and a double-click on a scenario row opens the map's editor |
| PK3 | **The game's side** (built): `PackageRewrite` as the one rewrite, the baked set, `<data>/packages`, features, the cache, the game's `packages.list` | M | PK2 | the rewrite's bytes equal `workspace::build`'s; a second read is a cache hit; a package off in `set.toml` or with its feature off adds nothing; a map that holds the content is left alone |
| PK11 (built) | **HUDs as data** (built: the builder, the YAML and `views/`; `hud_kit::install` still reads the first-party YAML that `data::BUILTIN` embeds, and `workspace::huds` lists a set's HUDs for it): the `hud` builder, HUD kit YAML for the superbounce, Forge readout and Firefight HUDs in `content/{superbounce,forge,modes}/huds/`; the element lists in `hud-kit-core/src/huds/*.rs` deleted, the states that stay in `views/` | M | PK3 | each YAML is the `Hud` its Rust list built (`hud-kit-core/src/{expected,data_tests}.rs`), so it compiles to the same records (`hud-kit-core/src/tests.rs`' fixtures run on the YAML) |
| PK4 | **Pogo as a package** (built): `content/pogo/`, the `model` and `sound` builders, the weapon as a `tag` item; `PogoRewrite` deleted | M | PK3 | on `content_kit::fixture`'s map the package builds the tags `content_kit::inject::add_pogo` built, compared group by group |
| PK5 | **Screens as a package** (built): the always-on `content/ui/` package serves `screens` and `screen-edits` through `PackageRewrite`; `ui.save` writes our screen YAML into the checkout’s UI package | M | PK3 | `workspace::screen_tests` compare the package with applying the retail edits and calling `ui_kit::inject::tag::add_screens` on each invented UI map; the Studio save test checks that only the package screen source changes |
| PK6 (built: `app/package_doc.rs`, `api/packages.rs`) | **The package document and `package.*`**: list, read, new, set, items, target, enable, order, check, build, play, over the always-on rule `content::package::Set` keeps | M | PK2, SH1 | every method's example runs; a `set.toml` or a call that turns `core` off is refused by name; a reorder that breaks `requires` is refused; `package.check` names a cache map, a bundle and a dataset folder put in an invented package |
| PK7 (built: `app/edit_target.rs`, `app/edit_lines.rs`; the game's and Live's readers of `active.toml` stand) | **Edits go to the edit target**: the journal as edit lines, the first-edit picker, **Export map...**; the package form and `active.toml` deleted | M | PK6 | a field set in the window and by `tag.set` write the same line; undo across a save removes it; with no target the API answers `NO_TARGET` |
| SH3 | **Problems** (built: `app/problems.rs`, `ui/problems.rs`, `problems.list`): one panel and `problems.list` over `check.run`, `ui.check`, `hud.check`, `package.check` and build errors, each row opening its asset | S | SH1 | findings from two invented sources list together; opening a row focuses the document and the field |
| SH4 | **The preview switch** (built: `app/preview_switch.rs`, `ui/preview_switch.rs`, `api/preview.rs`; the Game panel shows the preview state and warnings): `preview.target`, **Preview in game** by kind over `pair.preview` | S | SH1 | on the fake game each kind sends the calls its `what` lists; a paused game refuses with the built message |
| GT0 to GT7, GE0 to GE5, GM1 to GM4 | **Game types without carriers**, no retail engine object, and maps for scripted games ([game-types-without-carriers.md](../game/game-types-without-carriers.md)) | S to L | GT6 needs PK11; GM2 needs PK9 | as there |
| PK8 (built: the items, `Registry`, `community_variants`' list from the set and the fragment's parse and tests; the default hopper in `live-proto` still builds its lineup in Rust) | **Rules, variants and playlists as items**: `content/core/` and `content/modes/`; the registry from the set; `community_variants` and the default hopper read it | M | PK3 | `crates/formats/rules/tests/modes.rs` pass with scripts from the set; a package off removes its list entries; the hopper built from fragments equals `hopper/defaults.rs`'s |
| PK9 (built) | **Whole maps** (`workspace::prepare`, `stage_kit::fresh::StageFresh`, `workspace::scenario_edits`, boot's `packages::prepare_maps`): `map` and `map-patch` items to map packages at prepare; the stage and the test-map recipes as packages | M | PK2 | an invented `fresh` and an invented `onto` item build; the `MapId` of a `map-patch` item equals the id of the map package the importer writes for the same patch |
| PK10 (built: `workspace::convert`, `cli/src/package_prepare.rs`, `content/{halo1,map-pack}/`; the Map Pack's `playlist` item waits for the hopper to read fragments) | **Conversions**: `convert` items, `h2 package prepare`, stamps; `content/halo1/` and `content/map-pack/` over the existing converters | M | PK9 | with an invented converter: no disc leaves the package off with its reason; a stale stamp reruns one map; an interrupted run resumes |
| PK12 (built) | **Kits** (built: `workspace::{kits, firefight_sets}`, `content/{core,halo1}/`, `content/modes/firefight/`; `firefight_kit::build_package` stays for `h2 firefight kit` and `host-data`'s bundle until that is moved to `ground::build`): `kit: campaign-cards`, `glyphs`; Firefight's director and wave sets as `rules` items of `modes`; `firefight_kit::build_package`'s boot and host-bundle callers go (GM1 of game-types-without-carriers builds ground on demand); boot's chain is `PackageRewrite` alone | M | PK9 | per kit, the map through the package equals the one its own caller made; `boot/src/lib.rs` builds no other rewrite |
| PK13 (built: `live_proto::packages`, `live-server/src/packages.rs`, the parity cases in `live-server/tests/support/packages.rs`; the mod pack routes stand until PK14's import writes packages) | **Live, server**: `/v1/packages`, the signed manifest and lock, the upload checks, credit, the model flag, both homes | M | PK1 | the parity suite: a bad signature, a lock hash that does not match, a retail hash and a model item without the flag are each refused; an admin's credit outranks the manifest's |
| PK14 (started: the install's checks, `live_proto::packages::{verify_answer, verify_file}`) | **Live, game and Studio**: install, the content set in a session, the join check, a session refused on `boot::packages::network_refusal`, the Mods screens, `package.publish` and `package.install`; `h2 live mods import` writes packages | M | PK13, PK3 | on the fake server: a joiner missing one package fetches it and joins; a blocked one fails before the map load with the reason; an imported pack holds patches and credit only |
| M1 (built) | **Model poses** (rich-editors) | M | nothing | as there |
| AN1 (built) | **The animation editor, read**: `anim.read`, the timeline rows, scrub, loop, the key reduction | M | M1, SH1 | an invented two-bone clip lists its channels; reduce then bake is within the tolerance at every frame |
| AN2 | **The encoder** (built): `tags::anim_write` (codecs 1 and 3) lifted from `h1-assets`, which calls it | M | nothing | `decode(encode(c))` equals `c` within quantisation on invented clips; the converter's `jmad` tests pass unchanged |
| AN3 (built) | **Keys and curves**: `crates/tools/anim-edit`, `AnimDoc`, `anim.key`, gizmo keying, the Curves tab, `anim.new`, `anim.duplicate`, `anim.check` | M | AN1 | a key set by the gizmo and by `anim.key` make the same document, a drag is one undo step, and a rotation survives the angle view unchanged. `api/anim/read_tests.rs`, `ui/anim/edit.rs` and `crates/tools/anim-edit/src/tests.rs` |
| AN4 (built) | **Write back**: `anim.save`, the `animation` builder, replace and add in a `jmad`, the refusals, the live metadata flush | M | AN2, AN3, X2, PK2 | an invented graph with one animation replaced keeps its other records and streams, an added one is reachable by its action, and an inheriting graph is refused by name. `crates/tools/workspace/src/animation/tests.rs` |
| AN5 (built) | **Retarget and glTF**: `anim.retarget`, `anim.import`, the node table | M | AN3 | a clip retargeted to a skeleton with one renamed and one longer bone keeps the end position within a bound; unmatched nodes are listed. `crates/tools/anim-edit/src/retarget/tests.rs`, `gltf/tests.rs` and `api/anim/transfer_tests.rs` |
| AN6 | **Animation in the game** (built: `world.object.animate`, `pair.preview {what: "animation"}`; the channel start is unconfirmed on a running game): the game's `world.object.animate`, `pair.preview {what: "animation"}` | S | AN4 | on the fake game the preview sends spawn, animate, wait and capture in order |
| A2, A3, A4 | **Mouth curves** (rich-editors) | S, S, M | A1; M1 for A3 | as there |
| SN1 (built) | **The sound editor as a document** (built: `ui/sound.rs` draws the front sound tab, `App::sound_rows`, `App::front_tag_doc`): the audition as a document, permutations with loudness, the mouth track row | S | SH1 | an invented sound opens as a tab; each permutation row carries `sound.measure`'s numbers |
| SN2 | **Import and encode** (built): `import.sound`, the `sound` builder, resample, ADPCM, the gestalt merge moved to `workspace::import` | M | PK2 | an invented WAV builds to a `snd!` that `app/audition.rs` decodes to the same samples within the codec's error; the gestalt of an invented map gains its rows and keeps the others |
| SN3 (built) | **Waveform edits** (built: `crates/tools/sound-edit`, `app/sound_edit.rs`, `api/sound_edit.rs`): the edit list, `sound.edit`, `sound.render`, permutations as files | M | SN1, SN2 | each op on an invented wave gives the expected samples; the source file's bytes never change; reorder and undo give the earlier render |
| SN4 (built: `App::voice_commit_sound`, `api/voice.rs`) | **Voices into packages**: `voice.commit` and cloned lines to the edit target | S | PK7 | a committed line is a `sound` item; a dataset or weights path is refused |
| SN5 | **Sound in the game** (built: the game's `audio.play`, `pair.preview {what: "sound"}`): the game's `audio.play` (**E first**), `pair.preview {what: "sound"}` | S | SN2, SH4 | on the fake game the preview plays, samples `audio.voices` and returns both measures |
| SH5 (built: no tool tabs; `ui.rs` and `ui/bar.rs` draw by `app::Surface`; `Doc::Rules`; the docs. `ToolKind` stands in `app` and the API; the screenshots list is not yet from documents) | **Old tools removed**: the tool tabs and floating windows; the script editor as the `.rules` document; [editor.md](editor.md) and [editor-guide.md](editor-guide.md) describe the tree and the editors | M | SH2, PK7, SN1 | `headless.rs`' screenshots list regenerates from documents; no `ToolKind` match is left in `ui.rs` |

SH1, W3, M1, PK2, GT0 and GT1 need nothing unbuilt and run together; SH2 follows SH1, PK1 and W3; PK3 follows PK2, and PK11,
PK4 and PK5 follow it side by side. The other editors' slices keep their own pages' order and
need only SH1 to open as documents.

## Evidence

- Every "today" row is read from the file it names (**Code**).
- The converter's `jmad` writer emits codecs 1 and 3 and the retail
  decoder reads its output (**Test**, `crates/tools/h1-assets/src/models/tests.rs`;
  [models.md](../halo1/models.md)).
- Converted Halo 1 maps and Firefight's per-map builds are already map packages the
  game loads by id (**Code**, `crates/apps/cli/src/h1_import.rs`,
  `crates/tools/firefight-kit/src/lib.rs`).
- That one rewrite over a pinned map gives the game what the four
  separate rewrites give it is **S**: PK4, PK5 and PK12 compare the
  bytes per kit.
- A mod's `from`, `replaces`, `tag-delta` and the conflict table, on an
  invented map: both of two mods on different fields show in the level, a
  refused package is off with its reason and moves neither the level's
  `LevelId` nor its image (**Test**,
  `crates/tools/workspace/src/{mod_tests,delta,assets,rewrite_tests}.rs`;
  [asset-store.md](../engine/asset-store.md), "Evidence").
- Which groups a `client` item may change without moving the sim hash is
  **S** (a lockstep run).
- Events and root movement in a `jmad`, the guest entry that starts an
  animation on an object, the one that plays a sound tag, and the one
  that spawns an effect are **E first**.

## Not verified

- That a map built by `PackageRewrite` with every first-party package
  loads and plays as today's chain's does.
- That an animation written by the editor plays in the game, and that a
  live `jmad` edit shows without a reload.
- That an imported ADPCM sound plays for every sound class.
- The time `PackageRewrite` adds to a first map read, and the cache's
  size.
- That a join which downloads a package finishes inside the lobby's
  timeouts.

## Related

- [tag-workspace.md](tag-workspace.md): the tree, layers, edit files and the build this stands on
- [live-editors.md](live-editors.md), [rich-editors.md](rich-editors.md), [game-view.md](game-view.md), [playtest.md](playtest.md), [content-stage.md](content-stage.md): the editors and views that open as documents
- [editor.md](editor.md), [agent-api.md](agent-api.md): the window and the API as built
- [game-types-without-carriers.md](../game/game-types-without-carriers.md): game types as scripts with a `type`, and their slices
- [maps.md](../game/maps.md): map packages, `MapId`, Live's store and credit
- [animation.md](../formats/animation.md), [sounds.md](../halo1/sounds.md): the formats the two new editors write
