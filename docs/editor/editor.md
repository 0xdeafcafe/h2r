# h2-editor

Status: Built.
Code: `crates/apps/studio/src/ui.rs`, `crates/apps/studio/src/app.rs`.
Switch: none.

Studio is the native Rust editor for maps, tags, voices, scripts, game
variants, playlists, screens and HUDs. Dear ImGui supplies the docked
windows. The front document determines which editor the window draws.
Follow the [editor guide](editor-guide.md) for the weapon, voice and
game type walkthroughs.

Nothing in the editor contains game data. Map names and everything else
it shows come from your own disc at run time.

Agents drive the editor through its agent API, [agent-api.md](agent-api.md):
every edit the window makes as a JSON method on a local socket, answered
by the open window (the user sees the edits land) or by `h2-editor
--serve` with no window; `h2 studio` is the client.

## Pages

| Page | Holds |
|---|---|
| This page | The tools, running it, the window, the document header, UI Screens, HUD, modeling, the code map and headless screenshots. |
| [editor-scenario.md](editor-scenario.md) | Scenario: the Maps list, packages, Halo 1, the Map Pack, what the viewport shows. |
| [editor-play.md](editor-play.md) | Play and pair, embedded play, game variants, playlists. |
| [editor-tags.md](editor-tags.md) | Tags: field names, registry, document, bundles, windows and panel. |

## Running it

```
cargo run --release -p studio -- [--disc <iso> | --map <file.map> | --folder <maps dir>] [--halo1 <iso|maps dir>] [--mappack <iso|folder>] [--hopper <file>|default] [--data <dir>]
```

The package is `studio`; its binary is `h2-editor`. With no source it
opens `H2_DISC`, else `disc/halo2.iso` when it exists
(`editor_core::default_disc`); with none, it starts empty and **File >
Open disc or map...** opens one later. A disc image lists every
`/maps/*.map`; a loose `.map` lists itself, and its directory's
`mainmenu.map`, `shared.map` and `single_player_shared.map` serve as the
shared caches. The source is read-only.

A Halo 1 source opens beside it: `--halo1`, else `$H1_ISO`, `$H1_MAP_DIR`
or the repository's `disc/halo1.iso` (`editor_core::halo1::default_source`).
**Open...** and a dropped path try Halo 1 first (`H1Source::open`: a disc
whose XBE is Halo: Combat Evolved's, a Halo 1 `.map`, or a folder holding
any), then Halo 2.

The Halo 2 Map Pack opens beside the disc, as its own source
(`editor_core::mappack`): `--mappack`, else
`$H2_MAPPACK_ISO` (`$H2_MAPPACK` is `h2 play`'s on/off switch, and `0`,
`false`, `no` or `off` turns the lookups below off too), else
`mappack.iso` beside the Halo 2 disc that was opened (any case, else the
first other `.iso` there whose contents are the map pack's; so it works
outside the repository), else the repository's `disc/mappack.iso`. A map
pack is told by its contents, never its name: an XISO whose
`/demos/dlc/<8 hex digits>/` directories hold a `ContentMeta.xbx` and
`.map` files (`xiso::mappack`, the layout in
[map-pack.md](../game/map-pack.md) section 1), or a folder with the same
tree (an extracted disc, or its `dlc` directory). **Open...** and a
dropped path try it first, so opening the map pack ISO by hand adds it
and leaves the Halo 2 source as it was. Its nine maps are ordinary Halo 2
cache maps: they open and edit like the disc's, and read their shared
caches (`shared`, `mainmenu`, `single_player_shared`) from the open Halo 2
source (`MapSource::shared`, used by `geometry::shared_sources`), so open
the Halo 2 disc too for their textures and geometry.

`--data` is the runtime's data root, as `h2 play --data`; the default is
the runtime's, `~/Library/Application Support/h2recomp/data` (`content::store::default_data_root`).
Saved game variants (`U:`) and map packages (`maps/`) go there, so a run
with the same `--data` finds them. The dock layout persists in
`<default data root>/h2-editor.ini`, whatever `--data` says (View > Reset
layout restores the default). The file's `[Studio][Tags]` section, after
Dear ImGui's own, keeps the field panel's **Show unnamed fields** and
**Show as** choices (`fields::Settings`, written on quit by `main.rs`).

## The window

![The editor window over Ascension with seven regions boxed and numbered in orange: 1 the menu bar, 2 document tabs and game link, 3 Scene, 4 Viewport, 5 Inspector, 6 Log, 7 the status bar](../images/editor/layout-regions.jpg)

*The headless renderer's `--callout` labels the menu bar, document bar,
Scene, viewport, inspector, Log and status bar.*

- **Main menu**: File (open a disc or map, open a folder of maps, **Save**,
  **Export map...**, open a package, the front editor's file
  commands, quit), Edit (**Go to anything...**, undo, redo, duplicate,
  delete, allow growing full blocks), View (frame the map, frame the
  selection, **Editors** (start the map, variant, playlist, UI screen,
  HUD or Textures editor), gizmo mode (move, rotate, scale) and local or
  world axes, grid and angle snapping, reset layout), and Game (Play, Stop, Pair,
  Unpair, Pause, Step, Resume and Edit).
- **The bar** (`ui/bar.rs`) has no tool tabs. The window draws the
  front document's editor: `App::surface` (`app/shell.rs`, `Surface`) is
  read from the tool the front tab brings forward, `Action::Show` asks
  for one, and `ui.rs` draws by `Surface`. `ToolKind` stays in `app` and
  the API as the holder of each document's state. The script editor is
  the `.rules` document (`Doc::Rules`): a tab from the first open of a
  script, drawn beside the variant's settings, closed without dropping
  its text.
- **Go to anything** (Cmd/Ctrl+P, `ui/palette.rs`) searches the tree's
  tags and the set's packages and items, as well as voices, maps,
  placements and commands (`app::commands::palette`). Use Up or Down
  to move, Enter to open or run the selection, and Esc to close.
- **Document tabs** (`ui/doc_tabs.rs`, `app/shell.rs`) identify open
  documents. Each tab has a close button. A `*` marks an unsaved
  document, and tabs that do not fit are under **More**.
- **The game link** sits to the right of the tabs. Its pill shows the pairing and whether tag and placement edits can reach
  the game.
  The controls beside it provide Play or Stop, Pause or Resume, Step and
  the Edit switch as they become available. The link's width is measured
  first, and the document tabs take what the link leaves.
- **Left dock**: the map's editor has a full-height column with the
  **Browser**, **Scene** and **Tags** tabs above the bottom dock. The
  other editors have their own windows in the left, centre and right
  regions (`ui/layout.rs`). The Map
  windows keep the `outliner`, `viewport` and `inspector` IDs used by saved layouts.
- **Scene** (`ui/outliner.rs`): the placement blocks as
  a tree, a filter (every word must match the block, the tag name or
  `#<index>`; blocks with no match hide, the rest open, with "x of y"
  counts), Duplicate and Delete. Click selects one entry, Cmd/Ctrl+click
  adds or removes one, Shift+click takes the shown run from the selected
  entry. Right-click an entry for Duplicate, Delete, Add and "Place a
  copy with the mouse"; right-click a block for Add and "Place new with
  the mouse" (the next click in the viewport puts it where the cursor
  meets the level). A button is disabled, with the reason as its
  tooltip, when the editor refuses (scenario-editor.md §6). Double-click
  an entry to frame the camera on it. The filter and the Duplicate and
  Delete row stay pinned at the top; the tree scrolls in one body
  (`theme::panel_body`).
- **Panels on short windows.** A panel scrolls as a whole: its lists
  and trees draw into the one body, and two lists in a panel are two
  folding headers. Only a panel with rows pinned above its list (the
  filter and command row of the Scene, Variants and Playlists lists, the
  Halo 1 outliner's banner) puts the list in a child, and
  `theme::panel_body` gives it the height left under them, never less
  than four lines. A list inside a dialog or popup, the command palette's
  results, has a height from its rows, at most seven, and never over half
  the viewport (`theme::bounded_list`). The helpers are the only callers
  of `child_window` in `ui.rs` and `ui/`, apart from `ui/tags.rs` and
  `fields.rs` (the check is `child_windows_come_from_the_panel_helpers` in
  `headless.rs`); `every_panel_scrolls_as_a_whole_on_a_short_window` draws
  each tool at 1000 by 420 and lists the child windows it finds. The
  design is in [tag-editing.md](tag-editing.md), "Panels on short
  windows".
- **Viewport** (centre): the map's art, its sky and a marker per
  placement, drawn by `scenario_edit::render::Viewport` into an offscreen
  texture that ImGui shows as an image. Left click picks, Cmd/Ctrl+click
  adds to or takes from the selection, double-click focuses (a
  placement, or the point of the level under the cursor). The camera
  (`camera.rs`, plain maths with unit tests): hold the right button to
  look and fly with WASD, Q down and E up, the wheel setting the fly
  speed (also adjustable in the toolbar) and Shift four times faster. Alt+left
  drag orbits about the selection, else about the point of the level
  under the cursor; middle drag pans; the wheel dollies; F frames the
  selection to its size. Motion eases in and out by elapsed time, the
  same at any frame rate, and the camera glides to a new focus. The
  selection has an ImGuizmo gizmo: W moves, E rotates (facing-only
  placements turn about z), R scales (objects with a scale field); X
  switches local and world axes. Toolbar icons show their shortcuts on
  hover. Grid and angle snapping each have a toggle and an adjacent
  step dropdown. They apply to the gizmo and to placing with the mouse. Other selected placements ride along with the
  primary's move and turn; one drag is one undo step.
- **Inspector** (right, `ui/inspector.rs`): the selection's typed fields
  under Transform, Object, Spawn and Settings. Position is x, y, z;
  rotation yaw, pitch and roll in degrees; facing in degrees; the
  palette index and tag references are searchable combos of tag names;
  flags are a checkbox per bit and the netgame flag type a named combo;
  other integers are bounded drags (team, respawn time). Read-only fields
  are disabled; each field's tooltip gives its offset and evidence. With
  several placements selected it shows the fields they all have; a field
  whose values differ is marked `*`, and an edit (one component, one bit
  or the whole value) applies to all of them as one undo step
  (`Session::edit_selection`).
  For one placement with an object or item reference, **Tags** shows the
  tag chain from that placement. Open a tag node to edit its fields with
  the Tags tool's panel (`fields::panel`). Named links lead to the object
  model and other dependencies, with other links under **All references**.
  The **Show in** button opens a tag whose metadata lives in a shared cache
  in the Tags tool, read-only. An edit affects every use of the tag. **Undo
  tag edit** and **Redo tag edit** step through the tag's history, its
  fields and its editor's steps alike ([studio-flow.md](studio-flow.md),
  "Shared patterns"). The map save includes the tag edits alongside the
  placement edits.
  A reference field's **Show this tag** button focuses the target in the
  inspector and keeps the placement selected. Breadcrumbs lead back to
  any ancestor. **Back** and **Forward**, Alt+Left and Alt+Right, and the
  mouse's back and forward buttons step through tag and placement
  history. Returning to a field opens its block and scrolls to it.
- **Game** and **Log** (bottom). Game shows the session, player and
  advanced pairing controls. Log shows every action's result, with
  errors in red.
- **Status bar**: the latest message, a hint for the viewport or toolbar
  control under the pointer, and the frame rate.
- **Unsaved changes**: anything that would drop a tool's unsaved edits
  asks first, naming the tool: another source or map (scenario), another,
  new or imported variant, or an entry's variant opened from a playlist
  (variants), another or new playlist file (playlists). Save (Save as...
  for an untitled playlist file), Discard or Cancel.
  Quitting asks about each tool with unsaved edits in turn.

Shortcuts include Cmd/Ctrl+O to open, Cmd/Ctrl+S to save, Shift+Cmd/Ctrl+S
to export a map or save a playlist to a chosen path, Cmd/Ctrl+P to open Go to
anything, Cmd/Ctrl+Z to undo and Shift+Cmd/Ctrl+Z or
Cmd/Ctrl+Y redo (in the front document), F5 Play, Shift+F5 stop the
game the editor started, and Cmd/Ctrl+Q quit. In the scenario tool,
Cmd/Ctrl+D duplicate and Delete or Backspace delete (the whole
selection), F frame the selection, Home frame all, W/E/R gizmo mode, X
local or world axes, and T (over the viewport, paired) teleport the player
to the cursor.
Dropping a disc image, map or folder on the window
opens it.

## Document header and links

**Code** in `crates/apps/studio/src/ui/doc_header.rs` draws the shared
header for the front document. `App::doc_asset` resolves its tag,
package item, package or voice id when one is available. The trail
buttons and **In Browser** locate the row, and the kind button also
filters the Browser by kind. For tags, the badge describes the source
or package overlay. The **into:** line reports the edit target and
document dirty state separately. **<** and **>** walk the document
trail (`app/navigation.rs`, `app/shell.rs`).

`app/links.rs` reads forward and reverse tag references, the enabled
package manifests, variant and script item relationships, and the
voice rules that match sound paths. **Links** shows **Uses** and
**Used by**, up to 20 rows per role, with the source of the link on
hover. **Show all in the Browser** passes the full section's asset ids
to the Browser. Tag rows dispatch `Action::OpenAsset`. Other rows
locate the asset in the Browser. **Open with** offers the editor
entries from the link table and disables entries with a refusal or
without a wired action. [`asset.links`](agent-api.md#the-browser) exposes
the links, editor choices and read problems over the socket.

### Shared patterns

| Pattern | Window and API behaviour | Code under `crates/apps/studio/src/` |
|---|---|---|
| **Preview** | The header and **Preview** menu share the target picker and **In game**. `preview.target` reads or changes the front tab's target. `pair.preview {doc?}` runs the document's route. Supported tags use the paired game, variants launch Studio's game with the current record, and scripts compile and reload into Studio's game. | `ui/preview_switch.rs`, `app/preview_switch.rs`, `api/preview.rs` |
| **Check** | The header counts the front asset's findings. Clicking **Check** refreshes Problems. `check.run {asset}` returns the requested asset's rows from a refresh, and `problems.list {asset}` filters the list. Script findings can open the affected line. | `ui/doc_header.rs`, `app/problems.rs`, `api/checks.rs` |
| **Apply** | Edits act on the working document. The header and Cmd/Ctrl+S share `App::doc_save_action`. Scripts, variants, playlist files, selected animation clips and generated voice lines save through the edit target. Without a target, a package save opens the picker. Map saves follow `App::save_target`, and export controls write standalone files. | `app/doc_save.rs`, `app/edit_target.rs`, `app.rs` |
| **Undo** | The header runs `Action::UndoDoc` and `Action::RedoDoc` for the front document. The tag document's step counter orders field edits with specialized steps such as mouth curves, bitmap edits and animation edits. The map routes its history to the active modeling or map script context when applicable. | `app/shell.rs`, `app/tag_doc.rs` |

The header's **Revert** is disabled. The variant Inspector offers its
own revert action. **Copy into package...** writes a tag recipe and
leaves the source tab in front. Opening the copy requires a map that
contains it. The **Packages** root lists packages and their items.
Package check, build and play are socket methods. See
[Packages](agent-api.md#packages).

### Not verified

- The copied weapon and the replacement sound in a game running a map
  built with their package items.
- The full variant launch and script reload in Studio's game.

## Scenario

Moved to [editor-scenario.md](editor-scenario.md#scenario).

### Packages and Halo 1 in the Maps list

Moved to [editor-scenario.md](editor-scenario.md#packages-and-halo-1-in-the-maps-list).

### The Maps list's cards

Moved to [editor-scenario.md](editor-scenario.md#the-maps-lists-cards).

### The Map Pack

Moved to [editor-scenario.md](editor-scenario.md#the-map-pack).

### New maps

Moved to [editor-scenario.md](editor-scenario.md#new-maps).

### What the viewport shows, and what it does not yet

Moved to [editor-scenario.md](editor-scenario.md#what-the-viewport-shows-and-what-it-does-not-yet).

## Play and pair

Moved to [editor-play.md](editor-play.md#play-and-pair).

### Embedded play (not built)

Moved to [editor-play.md](editor-play.md#embedded-play-not-built).

## Game variants

Moved to [editor-play.md](editor-play.md#game-variants).

## Playlists

Moved to [editor-play.md](editor-play.md#playlists).

## Tags

Moved to [editor-tags.md](editor-tags.md#tags).

### Field names

Moved to [editor-tags.md](editor-tags.md#field-names).

### The field registry

Moved to [editor-tags.md](editor-tags.md#the-field-registry).

### The document

Moved to [editor-tags.md](editor-tags.md#the-document).

### Browsing other maps and where tags live

Moved to [editor-tags.md](editor-tags.md#browsing-other-maps-and-where-tags-live).

### Tag bundles

Moved to [editor-tags.md](editor-tags.md#tag-bundles).

### The windows and the panel

Moved to [editor-tags.md](editor-tags.md#the-windows-and-the-panel).

## UI Screens

The UI builder's preview, checks and pages (docs/game/ui-builder.md) in
the editor. `app/ui_screens.rs` (`UiScreens`) holds the state, with no
UI; `ui/ui_screens.rs` draws it.

- **The tree**: two trees of folders under one filter.
  `boot::ui_kit::screens::NAMES` with each page's named states
  (`Page::states`) is grouped by `app::ui_screens::browse::ours`: the
  first dotted segment of a screen's name is its folder, a bare name that
  heads others joins them, a lone name stays at the root, and `design.*`
  and `layout.*` are the folders "Design system" and "Layouts", listed
  first. The disc's retail screens
  (`preview::Disc::retail_screens`, every `wgit` tag of `mainmenu.map`)
  nest by tag path (`browse::retail`) with the folders every tag shares
  stripped. A filter match shows its folders open.
- **The grid**: clicking a folder, or the root "Our screens" or
  "Retail screens", shows everything under it as tiles
  (`app/ui_screens/gallery.rs`, `ui/ui_screens/gallery.rs`;
  `UiScreens::gallery.at` is the `Browse` shown), by folder under
  subheadings, with a breadcrumb back. Each tile is the screen's name, a
  badge for its findings (ours; retail screens are drawn, not checked)
  and its thumbnail: ours in their first state (or every state with "All
  states"), retail as `Disc::render_retail` draws them. Thumbnails take
  the picture's CPU path (`render_view`, the same checks) on a fresh page
  so the one shown is not disturbed, are shrunk to fit 320 by 240 and
  cached per pick until the disc changes. Only tiles on screen draw, top
  first, for about 12 ms a frame (`UiScreens::draw_thumbs`), then upload
  as `Rgba8Unorm` textures (as the picture's), those off screen dropped
  past 96. Clicking a tile is `Action::UiPick`, which shows the picture
  with a breadcrumb back to its folder (`Action::UiBrowse`). The
  agent API reaches the grid with `ui.select_folder` and its contact sheet
  with `ui.sheet` ([agent-api.md](agent-api.md#ui-screens-folders)).
- **The picture**: a native pick is `screens::live(name)` gone to its
  state; its spec becomes a `View` (`view_of`) drawn by
  `preview::render_view` from the editor's disc (`App::sync_ui_disc`: the
  open source when it is a disc image) or the synthetic source. A retail
  pick is `Disc::render_retail` (pane 0, in the screen's sample state:
  docs/game/ui-builder.md "Retail screens in a believable state"), not
  checked. The canvas goes to a `Rgba8Unorm` texture registered with the
  renderer (not sRGB, as the viewport's), re-uploaded when the shot's
  `rev` changes.
- **Overlays**, clipped to the picture: widget rectangles
  (`Widget::pixels`), the rules' title-safe and reserved areas, and the
  findings' widgets (`layout::Issue::ids`) in red; a finding hovered in
  the inspector outlines its widgets thicker. The widget under the mouse
  is the topmost whose rectangle holds it (`UiScreens::widget_at`); the
  inspector shows its entry of `View::json`.
- **The pad** sends `Action::UiPress(Input)` to the `Live` page; the
  effect goes to the log. List screens step their real `ListClass`
  over an in-memory profile store, so nothing is written.
- **Export** (`Action::UiExport(dir)`): the PNG, the JSON and
  `preview::write_sheet`'s files, refused inside a Git checkout (any
  folder with `.git` at or above it).
- **Editing** (`Action::UiEdit`, `app/ui_screens/edit.rs`): select,
  drag, resize, nudge and retype widgets on the picture
  (`ui/ui_screens/edit.rs`), with a **Selected** section and a widget
  list in the inspector. Our screens' edits change their
  `content/ui/screens/<name>.yaml` docs (`edit::ours`, saved into the
  checkout and reloaded when the files change on disk); a retail
  screen's are tag writes on the open `mainmenu` map (`edit::retail`,
  `App::tags`), saved with the map. A retail pick is drawn with its
  edits (`Disc::render_retail_edited`: the places moved, and the pane's
  text, button and bitmap blocks as the map's tag document has them)
  and has a view (`Disc::retail_view_edited`) for the overlays and
  edits. The Selected section's Hide and Duplicate buttons and
  `ui.set_string` are tag edits past a rect (`edit::retail_tag`). The design, the
  refusals and the `ui.*` edit methods are in docs/game/ui-builder.md
  "Editing in the editor".
- **The transport** (`ui/ui_screens/transport.rs`): the kit's Open and
  Close transitions played, looped or scrubbed, 0.25 to 2 times, with
  an onion skin; `ui.animate` writes the same transition as a filmstrip.

**Why the studio depends on `boot`.** The pure parts (view, spec,
preview, checks) could move to a crate of their own, but the pages can't:
a list page steps the screen's real `ListClass`, rows and stores, which
are `boot`'s. Moving them all would split the screens from their glue, so
the studio takes `boot` as a library (it already builds for `h2`; no
game module is built or loaded).

`--ui-screen <screen>/<state>` (the state defaults to the first) and
`--ui-press up,a,...` set the tool up for a headless screenshot.

Tests: `app::ui_screens::browse::tests` (the grouping, retail nesting,
sections, filter, crumbs), `app::ui_screens::gallery::tests` (sections,
the thumbnail budget and cache, the contact sheet), `api::tests`
(`ui_screens_lists_the_folders_of_both_trees`,
`ui_select_folder_shows_the_grid_...`, `ui_sheet_renders_...`),
`app::ui_screens::tests` (the tree, drawing without a disc,
presses, hover and JSON, findings with their widgets, export and its
refusal; `with_the_disc` is ignored without `H2_DISC`), and
`headless::tests::the_ui_screens_tool_steps_a_page_through_its_widgets`,
which clicks the tree, the pad and hovers the title through the
test-engine hooks.

## HUD

The developer's page is [hud-editor.md](hud-editor.md): the placement
rules and their proof, the checks, the save, and how the widget fields
use the generic field panel (`fields.rs`, named by
`tags/src/defs/nhdt.txt`) with the HUD tool's own history. `app/hud.rs` (`HudTool`) holds the tool's state
and its `Hud*` actions. `ui/hud.rs` draws it and uses the shared map
sections for the source and package picker. The logic lives in
`crates/tools/hud-edit`.

## Modeling

The developer's page is [modeling.md](modeling.md), with the targets and
the save in [modeling-targets.md](modeling-targets.md). `app/modeling.rs`
(`Modeling`, `ModelAction`) holds the geometry session over the open map
and hands its writes to the map's save; `ui/modeling.rs` draws the map
inspector's Modeling section; `api/geometry.rs` is `geometry.*` and
`mesh.*`. The viewport redraws open structure targets through
`Viewport::set_native_section`. The logic lives in
`crates/tools/geometry-edit` over `crates/tools/mesh-edit`.

## Code

The tools' logic lives in UI-free crates with their own tests; the
studio crate only draws them and turns input into actions.

- `crates/tools/editor-core`: map sources (`MapSource`: a disc image, a loose
  `.map` with its siblings, or a folder of maps; `map_names()`,
  `open_map(name)`, `resources(name)`), the snapshot `UndoStack`
  (coalescing by `u64` keys), `Status`, `variant_sources` (the
  variant schema, XBE save names and save storage for a source),
  `packages` (the packages under a data root, Halo 1 origins),
  `halo1` (Halo 1 sources and what is shown of a map) and `tag_bundle`
  (the tag bundle format, [Tag bundles](#tag-bundles)).
- `crates/tools/geometry-edit`: the Modeling tool's targets
  (`Target`, `list`), `GeometrySession` (meshes, undo, op log, write
  classes, `changes`, `previews`) and the in-place section rewrite.
- `crates/tools/scenario-edit`: `Session` holds the scenario editor's state and
  every command (open, select, add, remove, plane drags, gizmo gestures,
  undo, save, packages); `session/multi.rs` holds the multi-selection
  and its edits (`edit_selection`, `remove_selection`,
  `duplicate_selection`, `add_entry_at`, `surface_hit`). `render` and `geometry` draw it offscreen with
  wgpu (the camera gives separate view and projection matrices for
  ImGuizmo; `geometry::decompose` turns a gizmo matrix back into position
  and yaw/pitch/roll).
- `crates/formats/variants`: the variant record, schema, saves and
  `VariantEditor` (state and commands).
- `crates/tools/hud-edit`: the HUD tool's logic (the `nhdt` layout and field
  tables, placement, the preview, checks, `HudSession`;
  [hud-editor.md](hud-editor.md)).
- `crates/tools/hopper-edit`: the playlist editor's logic (working copy,
  validation, presets, catalogue, the variant picker's sources) and
  `HopperSession` (file handling, undo).
- `crates/apps/studio`:
  - `app.rs`: `App`, the state (each tool's session) and `Action`s
    (open, select map, save, packages, undo/redo, select, delete,
    duplicate, add; variants; playlist files and entries; quit), with the
    unsaved-changes guard (`dispatch` holds an action that would drop a
    tool's edits; `resolve` answers and re-dispatches it). `app/library.rs`
    holds the packages, the Halo 1 source and the overlay. Tested without
    a window (`app/tests.rs`).
  - `ui.rs`: the ImGui shell and the scenario tool's windows; it only
    turns input into `Action`s and draws `App`. `ui/inspector.rs` and
    `ui/outliner.rs` hold the inspector's typed fields and the
    outliner's filter and click rules; `ui/variants.rs`,
    `ui/playlists.rs`, `ui/ui_screens.rs` and `ui/tags.rs` draw the
    other tools.
  - `app/tag_doc.rs`, `app/tag_api.rs`, `fields.rs`: the Tags tool's
    document, its plain-function API and the reusable field panel;
    `app/tag_browse.rs` (browsing other maps, previews),
    `app/tag_tree.rs` (search and the tree), `app/tag_bundle.rs` (export
    and import), `app/shared_refs.rs` (where tags live) ([Tags](#tags)).
  - `app/scripts.rs`, `ui/scripts.rs`, `api/scripts.rs`: the Scripts tab
    of the Map tool, read only ([rich-editors.md](rich-editors.md#scripts)).
  - `app/convert.rs`: Convert to Halo 2 (the background build of a new
    map and its plain-words errors). `app/editing.rs`: multi-selection
    delete and duplicate, placing at the cursor, framing the selection.
  - `camera.rs`: the camera controller's maths (fly, orbit, pan, dolly,
    easing, framing), with unit tests and no UI.
  - `viewport.rs`: the viewport texture (`register_external_texture`),
    camera input, picking, placing, snapping and the gizmo.
  - `play.rs` and `play/`: Play and pair (above): the `h2 play` command
    and socket discovery (`launch`), the agent link thread (`link`), the
    tag and placement diffs sent as `edit.batch` (`tag_sync`, `sync`). `ui/bar.rs` draws the
    link and transport. `ui/play.rs` draws the Game panel and the live
    viewport frame.
  - `theme.rs`: colours, spacing, fonts (Inter 15 pt with a Lucide icon
    subset merged in; Dear ImGui 1.92 rasterises at the framebuffer scale,
    so text is crisp on HiDPI).
  - `headless.rs`: renders real frames of the UI offscreen and writes PNGs.
  - `script.rs`, `probe.rs`: the `--do` steps and the widget lookup behind
    them (below).

## Headless screenshots

```
cargo run --release -p studio -- --disc <iso> --pick <map> --screenshot out.png [--size 1600x1000] [--scale 2]
```

renders six frames of the real UI offscreen (no window) and writes the
last. The flags that set the scene up:

| Flag | Effect |
| --- | --- |
| `--pick <map>` | open this map in the scenario tool |
| `--tool tags --tag <group>:<path>` | focus the Map tool's Tags tab and show a tag of the open map |
| `--tags-map <map>` | browse another map of the source in the Tags tool (`shared`, `single_player_shared`); `--tag` then picks one of its tags |
| `--halo1 <iso\|dir>` | open a Halo 1 source (else `$H1_ISO`, `$H1_MAP_DIR`, `disc/halo1.iso`) |
| `--mappack <iso\|dir>` | open the Map Pack (else `$H2_MAPPACK_ISO`, `mappack.iso` beside the disc, `disc/mappack.iso`) |
| `--halo1-map <map>` | show a map of it, read-only |
| `--package <dir>[#H1\|#H2]` | open a package from the Maps list (a converted map's variant by its sandbox) |
| `--convert h2\|h1` | with `--halo1-map`, run Convert to Halo 2 with that sandbox, wait for it and open the result (writes under `--data`) |
| `--overlay on\|off` | switch the Halo 1 overlay |
| `--select <block>:<index>` | select a placement (`vehicles:0`, `netgameequipment:3`) |
| `--focus` | frame the camera on the selection (F) |
| `--camera <yaw>,<pitch>,<distance>` | orbit the camera (degrees, world units), after `--focus` |
| `--tool scenario\|scripts\|variants\|playlists\|ui-screens\|hud` | show that tool (`scenario` selects Map; `scripts` its Scripts tab) |
| `--hs-doc <file>` | name builtins in the Scripts tab from this script documentation listing ([rich-editors.md](rich-editors.md#scripts)) |
| `--hud-map <map>`, `--hud-package <key>` | open a map of the source (or a package) in the HUD tool |
| `--hud <nhdt>` | show that HUD (its full name, or its last part: `masterchief`) |
| `--hud-widget <name>` | select that widget |
| `--ui-screen <screen>/<state>` | show a screen of ours in the UI Screens tool |
| `--ui-press <inputs>` | then press these on it (`up,right,a`) |
| `--variant <n>` | open built-in variant `n` |
| `--copy-variant <name>` | copy it to an editable variant named `<name>` |
| `--hopper <file>\|default --playlist <1..16>` | open a playlist file (or the default lineup) at a playlist |
| `--preset <n>` | pick historical preset `n` for that playlist |
| `--dialog edit-target\|unsaved` | open the first-edit picker (the edit target), or nudge the selection and ask to quit (the unsaved-changes window) |
| `--nudge <x>,<y>,<z>` | move the selection by that much, as an edit |
| `--page <title>` | in the variant tool, open only the settings pages whose title contains it |
| `--no-disc` | open no disc and no Halo 1 source (the first run) |

Then, to show a state the flags can't set up, scripted input runs over
the settled frames, and the written frame can be cropped and annotated:

| Flag | Effect |
| --- | --- |
| `--do '<step>; <step>...'` | `hover`, `click`, `rclick`, `dclick` or `move` a target; `down`, `up` (the left button, for drags); `type <text>`; `key <name>[*n]`; `wait <frames>`; `sleep <ms>` (wall time, for background work such as a conversion); `wheel <dy>` |
| `--crop <target>` (repeatable) | write only the union of these targets' rectangles |
| `--pad <points>` | grow the crop by this much (default 8) |
| `--crop-height <points>` | make the crop this tall from its top |
| `--callout <target>=<badge>` (repeatable) | draw an orange box round the target with the badge (a number) on it |

A target (`studio::script`) is a point `x,y`, a window `win:<name>`
(`win:###inspector`, `win:##Menu`, `win:##Tooltip`, `win:##Combo`,
`win:##Popup`), or a widget by label, `[<window>>]<label>[#n]` (`View`,
`Rotate (E)`, `##setting-2`, `###playlists-entries>Edit#0`), each optionally
`@dx,dy` from its top left rather than its centre. Widgets are found by
label through Dear ImGui's test-engine hooks (`studio::probe`; the
`test-engine` feature of `dear-imgui-rs`): `ItemAdd` gives each item's
rectangle and window, `ItemInfo` its label. Combos never report a label,
so the labels a script names are also hashed on each item's window ID
stack and matched by ID. The hooks only record in headless renders. Set
`H2_EDITOR_PROBE=1` to print every window and widget the last frame drew,
to write a script against. A scripted click on the former Scenario row
selects **Map**.

```
h2-editor --disc disc/halo2.iso --pick ascension --select netgameequipment:0 \
  --do 'click View; hover Rotate gizmo' --crop View --crop 'win:##Menu' --screenshot menu-view.png
```

![The View menu open with Rotate gizmo highlighted, cropped to the menu](../images/editor/menu-view.png)

*What that command writes: the View menu opened and an item hovered by
the script, cropped to the menu.*

`scripts/editor-screenshots.sh` renders the user guide's pictures this way
from your disc, into `docs/images/editor/` (editor-guide.md §12).
`headless::tests::renders_the_studio_offscreen` does the same over the
scenario crate's invented test map, and skips when the machine has no
wgpu adapter.
