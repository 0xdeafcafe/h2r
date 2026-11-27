# h2-editor's agent API

Status: Built. Code: `crates/apps/studio-proto/src`, `crates/apps/studio/src`. Tests: in-source unit tests.
Switch: `H2_STUDIO` (the socket path; `crates/apps/studio-proto/src/lib.rs`).

An agent (or a script, or a test) edits what the editor edits: maps and
their tags, scenario placements, game variants, playlists and UI screens,
through JSON methods on a local socket. The style is the game's agent API
([agent-api.md](../runtime/agent-api.md), `h2 agent`): JSON-RPC 2.0, one
object per line, short forms on the command line, scripts on stdin, and a
`session.methods` call that lists every method with its params and result.

Status: first slice built (`crates/apps/studio/src/api`, `crates/apps/studio-proto`).
The Tags tool (`tag.*`), the HUD tool (`hud.*`) and the models provider
(`model.*`) answer through providers. `model.colors`, `model.emblem` and
`custom.*` are planned: listed by `session.methods` as `planned` and
answering `NOT_YET` until their tools register.

## Running it

Two ways to have an editor to talk to:

- **The window listens.** `h2-editor` opens its socket at start-up and
  answers between frames, on the documents the window shows, so the user
  sees each edit land (the outliner, inspector, viewport and title update;
  the log has a line per call that logs). `--no-listen` turns it off. When
  another editor already listens on the path, the window starts without
  the API and says so in its log.
- **Headless.** `h2-editor --serve [--socket PATH] [--data DIR] [--map
  file.map | --disc iso ...]` runs the same API with no window and no GPU
  session. Every start-up flag still applies (`--map`, `--pick`,
  `--hopper`, ...), so a run can begin with a map open. Between requests
  it runs the game link as the window does between frames, so it pairs,
  auto-pairs with the one game running, and answers `pair.*` and
  `mirror.*`. It runs until killed.

The socket is `--socket`, else `$H2_STUDIO`, else `studio.sock` in the
game's cache directory beside `agent.sock`. The editor accepts local AF_UNIX
connections. The socket has mode 0600 on Unix. On Windows 10 version 1803
or later it inherits the directory's ACL. The client refuses a `tcp://` path.

The client is `h2 studio` (or `h2-editor call`, the same code):

```
h2 studio status
h2 studio methods
h2 studio source ~/maps/example.map
h2 studio objects netgame_flags
h2 studio set netgame_flags:0 position [4,5,6]
h2 studio diff
h2 studio save-as /tmp/edited.map
h2 studio scenario.add '{"block":"weapons","like":0,"position":[1,2,3]}'
printf 'undo map\nredo map\ndocs\n' | h2 studio -
```

A call is a method and JSON params, or a short form (`h2 studio --help`
lists them). Results print as pretty JSON, one compact line per call in a
`-` script. An error prints to stderr as JSON (`{"code", "message",
"data"}`) and exits 1; a script stops at its first error.

## The command layer

Every edit the window makes is an `Action` on the `App`
(`crates/apps/studio/src/app.rs`). The window turns clicks into actions and
`App::dispatch`es them (an action that would drop unsaved edits waits for
the prompt). The API turns calls into the same actions and
`App::perform`s them: the action runs through the same `App::run`, and
the caller gets the log lines it wrote or why it failed. An action that
would drop unsaved edits is refused (`UNSAVED`) unless the call passes
`"discard": true`; the API never holds one for a prompt.

What goes through actions, from both sides:

| Edit | Action | Window | API |
|---|---|---|---|
| Open a source, map, package, Halo 1 map | `OpenSource`, `SelectMap`, `OpenPackageEntry` | menus, Browser, map grid | `source.open`, `doc.open` |
| A placement's field | `EditSelection {field, edit, merge}` (every selected placement), `SetField {block, index, field, value, merge}` (one), `SealEdit` | the inspector dispatches `EditSelection` (a drag merges under one key, sealed when it ends) | `scenario.set`, `tag.set` |
| Any tag's field or block | `Tag(TagAction)` built by `app::tag_api::{set_field, block}`; each change is then written as an edit line to the edit target | the Tags tool | `tag.set`, `tag.block.*` |
| Inspect tag fields | read the registry layout | Tags panel and inspector | `tag.fields {tag, path?, unnamed?}` |
| Show unnamed fields, Show as, focus a field, the filter row | `App::tag_fields` (`fields::Settings`), `Navigate(Go)`, `fields::find` | the Tags tool's **Show unnamed fields**, **Show as**, **Search fields** and **Changed only** | `tag.view`, `tag.find` |
| What a field's pick dialog lists | `fields::picker::{values, bits, kinds, tags, views}` | the field panel's pick dialog | `tag.choices {tag, path, query?, kind?, limit?}` |
| An element's or field's bytes; put a field back to the file's value | `tag_api::{bytes_of, reset}` | Data and bytes: **Bytes**, **Reset to file value** | `tag.bytes {tag, path?}`, `tag.reset {tag, path}` |
| Move and turn a placement | `MovePlacement` (one step); `GestureSet {position, ypr}`, `GestureScale`, `EndGesture` (a drag: one step, a multi-selection riding along) | the viewport gizmo dispatches the gesture actions every frame of a drag | `scenario.move` |
| Add, duplicate, delete | `Add`, `DuplicateEntry`, `DeleteEntry` | outliner, toolbar, keys | `scenario.add`, `.duplicate`, `.delete`, `tag.block.*` |
| Select, frame | `Select`, `FocusSelected`, `FrameAll` | Scene, viewport, keys | `scenario.select`, `view.camera` |
| Select a trigger volume | `SelectVolume` (clears the placements; selecting a placement clears it), `FocusSelected` | a click on a box the viewport shows, the Scene tab's Volumes group, the inspector's Frame | `volume.select {index?, focus?}` |
| Move, turn and resize a trigger volume | `SetVolumeFrame {index, frame, merge}` (one step, or one per merge run); `VolumeGesture(frame)`, `EndGesture` (a gizmo drag: one step) | the viewport gizmo (move, turn, and scale along each axis), the inspector's centre, size and facing | `volume.set` |
| Select a level collision surface | `SelectSurface` (clears the placements and the volume), `FocusSelected` | a click on a surface a collision layer shows when nothing else is under the cursor | `collision.select {index?, focus?}`, `collision.at {..., select: true}` |
| Select a pathfinding sector, bot polygon, bot link or spot | `SelectNav` (clears the placements, the volume and the surface), `FocusSelected` | a click on what a Navigation layer shows when no placement or volume is under the cursor | `nav.pick {kind, index, focus?}`, `nav.pick {..., select: true}` |
| Add a trigger volume, make a kill volume, remove either | `AddVolume {frame, kill}` (with `kill`, the volume and a kill trigger naming it in one step), `RemoveVolume(kind, index)`; `Delete` on a selected volume | the Scene tab's Volumes menu, the inspector's Delete and a kill trigger's Remove, the Delete key | `volume.add`, `volume.remove` |
| Set fly speed | `SetFlySpeed` | viewport toolbar, wheel while flying | `view.camera {fly_speed}` |
| Show or hide viewport layers | `SetLayers`, `SetOverlay` | Show popover, View > Halo 1 overlay | `view.layers` |
| Choose what the level is drawn with | `SetLighting` | the toolbar's Lighting menu | `view.lighting` |
| Navigate tag fields | `Navigate` | inspector links and breadcrumbs | `nav.go`, `nav.back`, `nav.forward` |
| Undo, redo | `Undo`, `Redo` (the front document: the tool's, or the tag tab in front), `UndoDoc(doc)`, `RedoDoc(doc)`, `UndoIn(tool)`, `RedoIn(tool)` | Edit menu, keys, every editor's Undo and Redo button | `doc.undo`, `doc.redo` |
| Save | `Save`, `SaveMapAs`, `SaveVariant`, `SaveHopper(As)`, `ExportVariant` | File menu, shortcuts, prompt | `doc.save`, `doc.save_as` |
| Open as fields | `OpenFields {asset, map}` | a Browser tag row's menu, **Fields** in the header's **Open with** | `shell.open {asset, fields: true}` |
| Drop a source file on a package | `DropFile {file, package}` | a WAV, a PNG or a model YAML dropped on the window: the package is the one the Browser's Packages root shows or has selected, else the edit target | `asset.drop` |
| Where used, Show edits | `WhereUsed {asset}`, `ShowEdits {asset}`, `ClearBrowse` | a Browser row's menu, the header's **Links** menu, the Browser view's **Clear** | `asset.where_used`, `asset.edits` |
| A Forge map variant's field, its save | `Forge(ForgeAction::{Set, Save, Select})`, `UndoDoc(Doc::Forge)`, `RedoDoc(Doc::Forge)` | the Forge variant window's field boxes and rows, the header's Undo, Redo and Save | `forge.variant.set`, `forge.variant.save`, `doc.undo`, `doc.redo` |
| Go to anything | `OpenAsset` for tags, packages and items, `GoTo` for placements, existing map-open and menu actions for the other entries | Edit menu, Cmd/Ctrl+P | `ui.palette {query, run?}` |
| Variant edits | `OpenVariant`, `NewVariant`, `NewVariantFromScript`, `CopyBuiltin`, `SetVariantName`, `SetSetting`, `SetScriptOption`, `ImportVariant*`, `DeleteVariant` | the variant tool | `doc.open`, `variant.*` |
| Playlist edits | `EditPlaylist(PlaylistEdit)`, `SetEntryVariant` | the title and description fields, the table's row buttons, weights, Add, the unused slot's "Make a … playlist" buttons and "Remove playlist" apply the same `PlaylistEdit`s | `playlist.set`, `playlist.entry`, `playlist.entry_variant` |
| UI screens | `UiPick`, `UiPress`, `UiBrowse` | the UI Screens tool | `ui.pick`, `ui.press`, `ui.select_folder` |
| UI screen edits | `UiEdit(EditAction)`, `UndoIn(UiScreens)`, `RedoIn(UiScreens)` | the UI Screens tool's canvas drags, handles, nudges, inline text, Selected section and Save | `ui.select`, `ui.move`, `ui.set_rect`, `ui.set_text`, `ui.hide`, `ui.duplicate`, `ui.set_string`, `ui.revert`, `ui.undo`, `ui.redo`, `ui.save` |
| Playtest | `play::playtest::decide` | F5, Play button, Game menu and confirmation dialog | `pair.playtest {confirm?, prompt?}` |
| Playtest settings | `play::pair::Bridge` | Game menu and Game tab spawn picker | `pair.settings {spawn?, pause_while_editing?}` |
| Live selection | `play::select` | click a live object, Aim select in the bar or Game menu | `mirror.pick`, `mirror.aim_select` |
| Shared camera and player moves | `play::presence` | viewport camera menu, T and Shift+T | `mirror.camera`, `mirror.player` |
| Game link | `play::pair::Request` | game link bar, Game menu and Game panel | `pair.connect`, `pair.disconnect`, `pair.auto`, `pair.play`, `pair.stop`, `pair.control`, `mirror.control`, `pair.edit` |

The playlist fields apply their `PlaylistEdit`s inside the frame's
`HopperSession::edit_with`, so a typing run stays one undo step (merged
under the field's key) where the API's `playlist.set` is one step per
call. The playlist table's map pickers do not go through actions; they edit
the working copy in place the same way.

The tests check that the window's path and the API's path make the same
document: the same scenario changes and byte-identical saved maps for a
dragged field against `scenario.set`, and equal playlists for the row
buttons against `playlist.entry` (`api::tests::the_window_and_the_api_make_identical_documents`).
Tag edits are the Tags tool's own `TagAction`s, so they are the same by
construction.

## Documents

A `doc` param names one: `map` (the scenario tool's map), `tags` (the
Tags tool's edits to the same map: their own undo stack; they save and
diff with the map), `variant`, `playlists`, `ui`, `hud` (the HUD tool's
map: its widget edits, with their own undo stack and save), `scripts`
(the Scripts tool's view of the open map: read only, never dirty, and
`doc.save` and `doc.save_as` refuse it; its undo stack is the compiles,
as `script.undo`'s). Without one, a `doc.*` call means the tool shown.
`doc.list` says, for each, whether it is open, its name, dirty, and
whether it can undo and redo. An open tag tab (below) is a document too:
its id is a `doc`, and `doc.list` lists it after the tools' documents.

### Document tabs

Every open document is a tab in the bar (`app/shell.rs`,
`ui/doc_tabs.rs`). A tool's document (`map`, `variant`, `playlists`,
`ui`, `hud`) gets its tab when its tool opens it and loses it when the
tool closes it. A tag of the open map gets a tab from `shell.open` or a
double-click in the Tags tree; its id is its `group:path`, as `tag.*`
takes it, and its editor is the one its group's row of the kind registry
names (`tag.kinds`): bitmaps open in the Textures tool, the scenario tag
is the map's own tab, the rest open in the Tags tool's field panel. A
tag tab belongs to the map it was opened on and closes with it.

Each tab has its own undo stack:

- **What it holds.** A tag's steps in the order they were made, wherever each is kept: its writes in the map's tag document (`TagDoc::undo_tag`), a sound's mouth curves (`sound.mouth`), a bitmap's pixels and shape (`bitmap.*`), an animation graph's documents (`anim.*`).
- **Order.** Two tag tabs undo and redo their own in any order, and a new edit drops its own tag's redo and keeps the others'. Undo takes the newest step not undone. Redo takes the oldest undone one, unless a newer step of the document was made after it was undone: that step made it wrong.
- **Refusal.** An undo is `REFUSED` while a later edit of another tag writes the same bytes, or both added strings; that one is undone first.
- **Which stack.** Undo and Redo in the window, and every editor's Undo and Redo button, take the front tab's stack while the Tags tool shows its tag, or the canvas bitmap's in the Textures tool; else the tool's.
- **Closing.** Closing a tag tab keeps its edits: they are the map's, saved or dropped with it.

| Method | Does |
|---|---|
| `shell.tabs` | the tabs in order and the `front` one; per tab its `doc` id, `kind`, `title`, the `tool` it shows in, `dirty`, `can_undo`, `can_redo`, and for a tag its `editor`, `id`, `group` and `map` |
| `shell.open {asset?, fields?, tag?, map?, discard?}` | opens a tag of the open map (with `map`, that map opens first; refused over unsaved edits unless `discard`), or brings a map's tab forward; `asset` takes a `tag:`, `voice:`, `item:` or `package:` id. A package opens in the `package` tab. A voice opens on Setup while it has no dataset. A package item opens the editor of what it builds, with `item:` tab IDs for scripts and variants. A local authored tag, WAV or model can open in a source tab before its tag is available in a map. Playlist fragments are refused. `forge:<map>:<file>` opens a Forge map variant. With `fields`, a `tag:` asset opens as fields (the Browser's **Open as fields**): its tab shows the field panel in place of its group's editor, a bitmap's leaving the Textures canvas and a sound's the sound editor, and says `editor: field_panel` and `fields: true` until the tag is opened without `fields` or the tab closes; a scenario and an asset that is no tag are refused. The call answers the `tab`, the `front` and the `log` |
| `shell.focus {doc}` | brings a tab forward: its document shows in its editor |
| `shell.close {doc, discard?}` | closes a tab: a tool's document closes (`UNSAVED` over edits unless `discard`), a tag's tab closes keeping its edits; the nearest tab left open, to its left first, comes forward |

The tab strip's clicks run the same commands (`Action::OpenDoc`,
`FocusDoc`, `CloseDoc`, `UndoDoc`, `RedoDoc`), so the window and the API
leave the same tabs
(`api::tests::shell_tests::the_window_commands_and_shell_calls_leave_the_same_state`,
`headless::tests::the_tab_strip_clicks_and_shell_calls_leave_the_same_state`).
Short forms: `tabs`, `focus <doc>`, `close <doc>`.

`doc.save_as` takes a path.

| Call | Writes |
|---|---|
| `doc.save` on a variant, game type script or playlist | the edit target, as a package item; returns `{saved: true, path, edit_target}` |
| `doc.save` on a base map | nothing new: the edits are already lines in the edit target (`app::edit_target`); returns `{saved: true, edit_target}` |
| `doc.save` on another map | the target `App::save_target` chooses: the open package, the last standalone output (**Export map...**) or the edit target |

With no target these answer `NO_TARGET`, as do `tag.set` and `tag.block.*` (the window asks for one). The API keeps map outputs outside Git checkouts and uses a destination separate from the base source. Renders (`ui.render`, `view.snapshot`) default to `studio/` in the cache directory and refuse paths in Git checkouts.

`doc.diff` is what a save would write against the document as opened:

- **Map:** each patched run with its address and the blocks that grew. Placement patches identify the block, entry and fields; a removal's count word shows as `count`. BSP patches also carry a `bsp_region_offset`, the map-relative file offset of their region.
- **Playlists:** which playlists changed and how.
- **Variant:** the settings that differ from the defaults.

`source.maps` returns `map_entries` for the open Halo 2 source and `halo1_maps` for the read-only Halo 1 source. Each entry has the file stem (`map`), display name (`label`) and section (`group`).

- The groups in those arrays are `multiplayer`, `campaign`, `other` and `shared_and_menu`, in that order. Each section is sorted by display name, using file names while the cards load or when a card is missing.
- `map_pack` and `packages` entries also report `group` (`map_pack`, `packages` or `halo1_sandbox`) and use display-name order within each group.
- `maps` remains the array of Halo 2 file stems in source order.

## Go to anything

`ui.palette {query}` returns `{entries, totals}` from `app::commands::palette`.

| Entry kind | Rows |
|---|---|
| `map` | the maps |
| `tag`, `package`, `item`, `voice` | the asset rows `asset.list` answers for the query: every tag of the tags tree, the set's packages and the items of each package that is on (`app::commands::palette_assets`), with no map open; a `voice` is a `voice:<id>` asset of the **Voices** kind and opens the voice document |
| `placement` | placements |
| `command` | the menus' own, one `Open editor: <name>` per editor of View > Editors |

Entries are `{kind, label, detail}` in the order shown by Cmd/Ctrl+P (the table's). Each kind has at most 20 entries; `totals` counts all matches by kind before that cap. Each word of the query must match the commands' labels and details, and is the same search `asset.list` runs on the asset rows.

Pass `run` with a zero-based index into the displayed entries to run the
action and return its index as `ran`. An out-of-range index returns
`INVALID_PARAMS`. Actions that would discard edits return `UNSAVED`. A
tag entry opens on the map that holds it (`Action::OpenAsset`); a
package's and an item's rows carry the same action, which opens their
documents as the slices of docs/editor/studio-flow.md teach it to.

## The design system

`ui.components` returns the catalogue of `content/ui/design.yaml`
(`ui_kit::components::catalogue_json`): `components` (each with `name`,
`role`, `font`, `colour`, `justify`, `variants` and its `specimen` page),
`layouts` (each `layout.<name>` page with its `kind` and `states`),
`palette` (`{name, rgb}`) and `fonts` (role to font index).

`ui.consistency {screen?}` checks our screens, or one, in every state
against the design system (`ui_kit::consistency::check`;
docs/game/ui-builder.md "Consistency") and returns
`{findings: [{screen, state, ids, rule, text}], checked}`: the findings
`consistency::ALLOWED` doesn't excuse, and how many screen states were
checked. A screen not in `NAMES` is `INVALID_PARAMS`.

Both read the kit
alone: no disc and no window state. The rules include the pattern rules
(`button`, `handled`, `arrows`) and the settings dialog's (`settings`,
drawn at every focus, and `help`; docs/game/ui-builder.md "The settings
dialog"). The `settings` component in `components` is the shared
settings dialog, its parts `settings.name`, `settings.about` and
`settings.help`.

`ui.widgets` answers `{widgets, findings: [{text, ids}], from_disc}` for
the picked screen in its state. The findings are
`ui_kit::findings::of_state`'s, the list `h2 ui preview` prints: each
line starts with its check (`check`, `content`, `layout`, `consistency`)
and covers the checks on what was drawn (`on_drawn`, `panel_shared`,
`covers_under`, `drawn_outside`; docs/game/ui-builder.md "The checks").

## UI Screens folders

`ui.screens` also returns `folders`: `ours` and `retail` are trees of
`{id, label, count, items: [{name, label}], folders: [...]}` (the
outliner's folders, from `app::ui_screens::browse`), and `browse` is the
grid the centre panel shows (`{root, folder, path}`, else `null`). Ours
group by the first dotted segment of the screen name (`design` and
`layout` are labelled "Design system" and "Layouts" and list first);
retail groups by tag path with the folders every tag shares stripped
(`retail.stripped`), and is `null` without a disc. A folder's text form is
`<root>/<id>`: `ours/forge`, `retail/game_shell/settings`.

`ui.select_folder {folder | root}` shows that folder's (or `ours` /
`retail`'s) grid of previews in the editor, as clicking it in the outliner
does, so `view.snapshot` of the window shows it. `folder` is the text form,
or an id under `root` when both are given. It answers `{browse, tiles,
states}`; an unknown folder is `INVALID_PARAMS`, retail without a disc
`FAILED`. `ui.pick` shows the picture again.

`ui.pick {screen, state?}` shows one of our screens in a state (default
its first); `ui.pick {retail, sample?}` shows a retail screen by tag
name, in its plain sample state or the sample variant `sample` names
(`variant:` in `assets/ui/retail-samples/`, docs/game/ui-builder.md
"Retail screens in a believable state"). It answers `{pick, summary}`;
`pick` is `{screen, state}` or `{retail}`, with `sample` when one is
named. An unknown screen or state, a `sample` the screen has no variant
of (the message lists the ones it has), or a `sample` with `screen` is
`INVALID_PARAMS`.

`ui.pick {screen, title, rows}` on `kit.picker` or `kit.picker.game` shows
the picker with a script's own offer: `title` and `rows` (1 to 16 strings;
the labels `offer_pick` and `pick_row` give, as `rules::world::choice` opens
them) in place of the kit's sample count rows. The offer is for that pick
alone; `title` without `rows`, `rows` on another screen, no rows, more than
16 and a row that is no string are `INVALID_PARAMS`.

`ui.sheet {folder?, root?, states?, columns?, out?}` writes that grid to
one labelled contact-sheet PNG (default: the grid shown; `states` draws
every state of our screens, not only each one's first; `columns` defaults
to 4; `out` defaults to `studio/ui/sheet-<path>.png` and is refused in a
Git checkout). It answers `{path, width, height, tiles: [{pick, name, x,
y, w, h, findings, found, checked, error?}]}`: each tile's picture
rectangle in the PNG, its finding count and `found`, one line per finding
as `ui.widgets` gives them (`checked` is false for retail screens, which
are drawn but not checked), and why it did not draw. Cells are drawn on
the CPU like `ui.render`, with the same checks, and never disturb the page
shown.


## Pages

| Page | Holds |
|---|---|
| [agent-api-methods.md](agent-api-methods.md) | The method reference: every namespace, its params and what it answers, for tags, scenarios, sounds, shaders, curves and placements. |
| [agent-api-pairing.md](agent-api-pairing.md) | Pairing with a running game: playtest, live tag editing, the Game picture, the mirror, viewport layers, volumes, collision, navigation, renders, and content previews in the paired game. |
| [agent-api-workspace.md](agent-api-workspace.md) | The tag workspace, projects, the Browser, Forge variants, packages, problems and the package-building methods. |
| [agent-api-media.md](agent-api-media.md) | Image generation, content checks, the announcer and voice methods. |
| [agent-api-extensions.md](agent-api-extensions.md) | The tools that register providers behind the API: tags, strings, recordings, scripts, curves, colours, shaders, UI edits, HUDs, modeling, textures, AI ground, siege layouts, new maps, game type markers and block-out pieces. |
| [agent-api-models.md](agent-api-models.md) | `model.*`, model poses, and the planned `model.colors`, `model.emblem` and `custom.*`. |

## Methods

Every namespace, its params and results: [agent-api-methods.md](agent-api-methods.md). `h2 studio methods` is the live list.

### Game picture

The Game panel's frames and the picture feed: [agent-api-pairing.md](agent-api-pairing.md) (the **Game picture** entry).

### The preview switch

Moved to [agent-api-pairing.md](agent-api-pairing.md#the-preview-switch).

### Previewing content in the paired game

Moved to [agent-api-pairing.md](agent-api-pairing.md#previewing-content-in-the-paired-game).

### Generating images

Moved to [agent-api-media.md](agent-api-media.md#generating-images).

### The tag workspace

Moved to [agent-api-workspace.md](agent-api-workspace.md#the-tag-workspace).

### Projects and edit files

Moved to [agent-api-workspace.md](agent-api-workspace.md#projects-and-edit-files).

### The browser

Moved to [agent-api-workspace.md](agent-api-workspace.md#the-browser).

### Forge variants and items

Moved to [agent-api-workspace.md](agent-api-workspace.md#forge-variants-and-items).

### Packages

Moved to [agent-api-workspace.md](agent-api-workspace.md#packages).

### Problems

Moved to [agent-api-workspace.md](agent-api-workspace.md#problems).

### Sounds into a package

Moved to [agent-api-workspace.md](agent-api-workspace.md#sounds-into-a-package).

### Models into a package

Moved to [agent-api-workspace.md](agent-api-workspace.md#models-into-a-package).

### Sound edits

Moved to [agent-api-workspace.md](agent-api-workspace.md#sound-edits).

### Pictures into a package

Moved to [agent-api-workspace.md](agent-api-workspace.md#pictures-into-a-package).

### Errors

Moved to [agent-api-workspace.md](agent-api-workspace.md#errors).

## Content checks

Moved to [agent-api-media.md](agent-api-media.md#content-checks).

## Announcer

Moved to [agent-api-media.md](agent-api-media.md#announcer).

## Voices

Moved to [agent-api-media.md](agent-api-media.md#voices).

## Extension points

Moved to [agent-api-extensions.md](agent-api-extensions.md#extension-points).

## AI ground

Moved to [agent-api-extensions.md](agent-api-extensions.md#ai-ground).

## Siege layouts

Moved to [agent-api-extensions.md](agent-api-extensions.md#siege-layouts).

## New maps

Moved to [agent-api-extensions.md](agent-api-extensions.md#new-maps).

## Game type markers

Moved to [agent-api-extensions.md](agent-api-extensions.md#game-type-markers).

## Block-out pieces

Moved to [agent-api-extensions.md](agent-api-extensions.md#block-out-pieces).

## Models and customisation

Moved to [agent-api-models.md](agent-api-models.md#models-and-customisation).

### `model.*`

Moved to [agent-api-models.md](agent-api-models.md#model).

### Model poses

Moved to [agent-api-models.md](agent-api-models.md#model-poses).

### `model.colors` and `model.emblem` (planned)

Moved to [agent-api-models.md](agent-api-models.md#modelcolors-and-modelemblem-planned).

### `custom.*`

Moved to [agent-api-models.md](agent-api-models.md#custom).

### Tests

Moved to [agent-api-models.md](agent-api-models.md#tests).

## Code

- `crates/apps/studio-proto`: method names, error codes, the socket path,
  the short forms and the client (`h2 studio`, `h2-editor call`). No
  editor dependencies, so the `h2` CLI links it without wgpu.
- `crates/apps/studio/src/api/mod.rs`: `METHODS` (every method's shape and
  example), `Api::call` (shape check, providers, built-ins),
  `Api::pump` (the window's between-frames answer).
- `api/server.rs`: the socket. `api/{docs,scenario,tags,variants,playlists,ui_screens,ui_browse,view}.rs`:
  the namespaces (`volume.*` is in `api/scenario.rs`, `collision.*` in `api/collision.rs`). `api/seams.rs`: providers; `api/tag_fields.rs`,
  `api/tag_io.rs`, `api/sound.rs`, `api/hud.rs` and `api/scripts.rs` the Tags,
  sound, HUD and Scripts tools', `api/geometry.rs` the Modeling tool's,
  `api/ui_edit.rs` the UI Screens tool's edits, `api/preview.rs` and
  `api/preview/run.rs` the previews in a paired game (tested in
  `api/preview/tests.rs` against the agent's real server with invented
  host methods). `api/sound_measure.rs` (with `sound_measure/meter.rs`,
  the verdict over the audio crate's meter), `api/ui_check.rs` and
  `api/checks.rs`: the content checks. `api/models.rs`: `model.*` (its
  renders through `scenario_edit::native_view::preview`).
  `api/markers.rs`: `markers.*`, over `scenario_edit::marker_view`.
  `api/anim.rs` and `api/anim/edit.rs`: `anim.*` over `app/anim.rs` and
  `app/anim/document.rs`. The Animation window in `ui/anim.rs` draws
  the same poses and edits the same `anim-edit::AnimDoc` tracks.
  `api/playtest.rs`: the playtest checks (`app/playtest_static.rs`) and
  the runner's methods (`app/playtest.rs`). `api/workspace.rs` (with
  `app/workspace.rs`: the tree's root, the plan, the extraction thread,
  the sizes and prune; `ui/workspace.rs`: the first start's page and the
  status line's bar): the tag workspace.
  host methods). `api/sound_measure.rs` (with `sound_measure/meter.rs`),
  `api/ui_check.rs` and `api/checks.rs`: the content checks. `api/voice.rs`
  (with `app/announcer.rs` and `src/voice*`): the Announcer.
  `api/voice_clone.rs` (with `app/voices.rs`, `voice/dataset.rs`,
  `voice/clone.rs`, `voice/sidecar.rs` and `sidecar/voice_clone.py`):
  Voices, tested in `api/voice_clone/tests.rs` with a fake sidecar.
- `app/commands.rs`: the actions added for the API and the window
  (`SetField`, `EditSelection`, `MovePlacement`, `EditPlaylist`,
  `UndoIn`), `PlaylistEdit`, and `App::perform`. The viewport gizmo
  dispatches `GestureSet`, `GestureScale` and `EndGesture` (one undo step
  per drag); the playlist title and description fields apply
  `PlaylistEdit`s; the inspector dispatches `EditSelection`.

Tests (`api::tests`, on the scenario crate's invented map, the playlist
crate's sample file and the fallback variant schema; outputs in the
system temp directory):

- every method in `session.methods` runs its own example on a fresh app
  (planned ones answer `NOT_YET` with their lane), and every example names
  only params its shape has;
- params are checked before anything runs;
- a headless editor on a socket opens a map, sets a field, diffs and saves,
  and the saved map differs from the source only in that field's bytes
  (and the header checksum); a notification gets no answer, a bad line a
  parse error; the socket is 0600 and goes with the listener;
- undo and redo through the API for the map, a variant and playlists;
- the window's path and the API's make identical documents, the gizmo's
  drag and `scenario.move` included (one undo step each), and the
  playlist text fields' edits and `playlist.set`;
- growth refused with its reason, unsaved edits never dropped without
  `discard`, outputs refused in a Git checkout or over the source;
- tags listed, read (raw and through the registry), referenced and
  followed; `tag.set` on a tag through the Tags tool, diffed, undone and
  redone as the `tags` document; a tag too short for its layout refused;
- a shared cache browsed (its tags listed and read, `tag.set` refused),
  back to the open map, a tag exported, edited and imported back over the
  edit as one undo step;
- the HUD tool through the API on `hud_edit`'s invented map: read, check,
  move (and in another mode), set, diff, undo and redo as the `hud`
  document, render (refused in a Git checkout), save as and re-open; and
  the window's actions and the API's make identical patches;
- a registered provider answers a planned method;
- the content checks on invented signals and layouts: a half-scale
  997 Hz sine reads -6.02 dBFS peak and -9.03 dBFS RMS and LUFS, a
  stereo one 3 dB louder, silence -120, a full-scale square every sample
  clipped, a WAV file against the invented map's sound `loud`
  (`api::sound_measure::tests`); a text past the title-safe area, one
  too long for its rect and two on each other named `ui.layout.unsafe`,
  `ui.layout.fit` and `ui.layout.text_overlap` (`api::ui_check::tests`);
  `check.run`'s order, counts and skips over invented checks
  (`api::checks::tests`);
- `view.layers` validates changes atomically, shares the Halo 1 menu switch,
  and changes the invented map’s snapshot pixels on a machine with a GPU;
- `volume.list` reads the invented map's two volumes, kill trigger and BSP
  switch, `volume.select` replaces the placements and frames the box, and
  the volume layers count the boxes and change the snapshot's pixels
  (`api::tests::volume_list_and_select_read_the_invented_volumes`); a
  click in the headless window selects a volume and brings its Scene row
  into view (`headless::tests::a_click_on_a_volume_selects_it`);
- `volume.set` moves, resizes and turns a box as one undo step each,
  `volume.remove` refuses the kill box naming its kill trigger and takes
  the kill trigger first, and `volume.add` with `kill` adds both entries
  and undoes as one step
  (`api::tests::volume_set_add_and_remove_edit_through_the_command_layer`);
- `collision.surfaces` reads the invented structure BSP's room, all
  collision only as nothing draws it, filtered and paged;
  `collision.at` finds the far wall by ray and by point with the layers
  hidden and selects it in place of the placements; `collision.select`
  frames a surface; the collision layers start hidden and count the
  surfaces (`api::tests::collision_surfaces_and_at_report_the_invented_rooms_walls`);
- `collision.surfaces` places the invented instance's quad and
  `collision.at` finds it; `collision.objects` places the invented
  scenery's `coll` cube on its second node and its `phmo` sphere on the
  root, and the layers count them
  (`api::tests::collision_objects_and_instances_report_the_invented_shapes`);
- `nav.pathfinding` reads the invented BSP's two sectors and its links,
  paged; `nav.pick` finds each sector by point and by ray, honours
  `shown` and `layer`, and selects and frames one; the Navigation layers
  start hidden, count the sectors, and change the snapshot's pixels
  (`api::tests::nav_pathfinding_and_pick_read_the_invented_sectors`);
- `nav.bots` reads an invented bot cache and its spots, verified,
  learned and ghosts files: links by kind, the drop one way, and each
  spot's state; `nav.pick` finds a spot and a polygon, and showing a bot
  layer loads the data again for a reopened map
  (`api::tests::nav_bots_reads_an_invented_cache_and_its_spot_files`);
  with no cache it builds the mesh in memory and writes nothing
  (`api::tests::nav_bots_builds_the_mesh_when_there_is_no_cache`);
- `playtest.static` over the invented map with sectors and an invented
  bots' mesh finds an off-mesh starting location, item and flag, an
  island, two opposing starting locations in sight and the missing
  Capture the Flag flags; with both starts on the mesh Slayer is ready;
  a start moved onto the island cannot reach the other and the room's
  wall hides it
  (`api::tests::playtest_tests::playtest_static_finds_each_problem_on_the_invented_map`);
  the checks themselves on invented layouts: islands, slivers, one-way
  drops, sight through a wall and by team, each game type's needs
  (`app::playtest_static::tests`);
- `playtest.compare` and `view.layers {diff}` over a fake sim: the
  baseline, a second run on a new build compared with it, the problems
  gained and lost, other seeds refused, a difference refused with nothing
  to compare, and the layers counting and drawing the marks
  (`api::tests::playtest_tests`, `app::playtest::tests`,
  `sim::spatial::compare::tests`, `viewport::playtest::tests`);
- `playtest.run`, `status`, `result` and `cancel` over a fake sim: the
  files written and read back by `h2 sim`'s batch planner, the presets'
  matches and seeds, only ready game types run, partial and whole
  results with their refused measures, a failed match listed, a repeat
  answered from the cache, a newer run cancelling the old one, and the
  refusal with no runner (`api::tests::playtest_tests`); the runner
  itself, its stale mark on a new build key and its cache keys
  (`app::playtest::tests`);
- `view.stats` of an invented scene reports its BSP's lighting, its
  triangles by blend, its materials and its sky's sun and fog
  (`api::view::tests`);
- the window's loop answers a client between frames.

`studio-proto`'s tests cover the short forms and the wire round trip.
