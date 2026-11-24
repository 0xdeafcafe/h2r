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
| Any tag's field or block | `Tag(TagAction)` built by `app::tag_api::{set_field, block}` | the Tags tool | `tag.set`, `tag.block.*` |
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
| Undo, redo | `Undo`, `Redo` (the tool shown), `UndoIn(tool)`, `RedoIn(tool)` | Edit menu, keys | `doc.undo`, `doc.redo` |
| Save | `Save`, `SaveMapAs`, `SaveVariant`, `SaveHopper(As)`, `ExportVariant` | File menu, shortcuts, prompt | `doc.save`, `doc.save_as` |
| Go to anything | `GoTo` for tags and placements, existing map-open and menu actions for the other entries | Edit menu, Cmd/Ctrl+P | `ui.palette {query, run?}` |
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
(the Scripts tool's view of the open map: read only, never dirty;
`doc.save` and `doc.save_as` refuse it). Without one, a `doc.*` call means the tool shown.
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

Each tab has its own undo stack. A tag's is its tag's steps of the map's
tag document (`TagDoc::undo_tag`): two tag tabs undo and redo their own
edits in any order, and a new edit drops its own tag's redo and keeps
the others'. An undo is `REFUSED` while a later edit of another tag
writes the same bytes, or both added strings; that one is undone first.
Undo and Redo in the window take the front tab's stack while the Tags
tool shows its tag, else the tool's. Closing a tag tab keeps its edits:
they are the map's, saved or dropped with it.

| Method | Does |
|---|---|
| `shell.tabs` | the tabs in order and the `front` one; per tab its `doc` id, `kind`, `title`, the `tool` it shows in, `dirty`, `can_undo`, `can_redo`, and for a tag its `editor`, `id`, `group` and `map` |
| `shell.open {tag?, map?, discard?}` | opens a tag of the open map (with `map`, that map opens first; refused over unsaved edits unless `discard`), or brings a map's tab forward; answers the `tab`, the `front` and the `log` |
| `shell.focus {doc}` | brings a tab forward: its document shows in its editor |
| `shell.close {doc, discard?}` | closes a tab: a tool's document closes (`UNSAVED` over edits unless `discard`), a tag's tab closes keeping its edits; the nearest tab left open, to its left first, comes forward |

The tab strip's clicks run the same commands (`Action::OpenDoc`,
`FocusDoc`, `CloseDoc`, `UndoDoc`, `RedoDoc`), so the window and the API
leave the same tabs
(`api::tests::shell_tests::the_window_commands_and_shell_calls_leave_the_same_state`,
`headless::tests::the_tab_strip_clicks_and_shell_calls_leave_the_same_state`).
Short forms: `tabs`, `focus <doc>`, `close <doc>`.

`doc.save_as` takes a path. `doc.save` writes a variant to its save
storage under the data root or a playlist file to its own path. For a
map, `App::save_target` chooses the open package or the last standalone
output. For a base map it opens the package form and returns
`{saved: false, package_form: {name, dir}}`. The API keeps map outputs outside Git
checkouts and uses a destination separate from the base source. Renders
(`ui.render`, `view.snapshot`) default to `studio/` in the cache directory
and refuse paths in Git checkouts.

`doc.diff` is what a save would write against the document as opened.
For a map it lists each patched run with its address and the blocks that
grew. Placement patches identify the block, entry and fields. A
removal's count word shows as `count`. BSP patches also carry a
`bsp_region_offset`, the map-relative file offset of their region. For
playlists it lists which playlists changed and how. For a variant it
lists the settings that differ from the defaults.

`source.maps` returns `map_entries` for the open Halo 2 source and
`halo1_maps` for the read-only Halo 1 source. Each entry has the file
stem (`map`), display name (`label`) and section (`group`). The groups
in those arrays are `multiplayer`, `campaign`, `other` and
`shared_and_menu`, in that order. Each section is sorted by display
name, using file names while the cards load or when a card is missing.
`map_pack` and `packages` entries also report `group` (`map_pack`,
`packages` or `halo1_sandbox`) and use display-name order within each
group. `maps` remains the array of Halo 2 file stems in source order.

## Go to anything

`ui.palette {query}` returns `{entries, totals}` from `app::commands::palette`.
Entries are `{kind, label, detail}` in the order shown by Cmd/Ctrl+P:
`map`, `tag`, `placement`, then `command`. Each kind has at most 20
entries. `totals` counts all matches by kind before that cap. Each word
of the query must match and case is ignored.

Pass `run` with a zero-based index into the displayed entries to run the
action and return its index as `ran`. An out-of-range index returns
`INVALID_PARAMS`. Actions that would discard edits return `UNSAVED`.

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
checked. A screen not in `NAMES` is `INVALID_PARAMS`. Both read the kit
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

## Methods

`h2 studio methods` is the full list with shapes and an example per
method. A shape is a JSON object whose keys are the params (a trailing
`?` marks an optional one) and whose values name their type; a call with
a param the shape does not name, or without a required one, is refused
before anything runs (`INVALID_PARAMS`). Indices are 0-based throughout
(placements, playlists 0..15, entries).

| Namespace | Methods |
|---|---|
| `session` | `status`, `methods`, `log` |
| `host` | `memory {report?}`, `memory.history {since_ms?}`, `memory.heap {top?}`, `memory.gpu`, `memory.diff {from_ms}`, `memory.postmortem {pid?}`: the editor process's watchdog and flight recorder, answered on the connection thread as the game answers them ([Finding a leak after the fact](../runtime/agent-api.md#finding-a-leak-after-the-fact)) |
| `source` | `open {path}`, `maps` |
| `workspace` | `status`, `extract {disc?, maps?, verify?, root?}`, `prune {dry_run?}` ([The tag workspace](#the-tag-workspace)); `import {map \| path, name}` (planned) |
| `doc` | `list`, `open {map \| path \| package \| variant \| playlists}` (`variant` is `{builtin}`, `{saved}`, `{new}` or `{script}`), `save`, `save_as {path}`, `diff`, `undo`, `redo`, `close` (`undo`, `redo` and `close` also take a tag tab's id) |
| `shell` | `tabs`, `open {tag?, map?, discard?}`, `focus {doc}`, `close {doc, discard?}`: the document tabs ([Document tabs](#document-tabs)) |
| `tag` | `list {group, name, query, path, limit, homes}`, `browse {map}`, `resources {tag}`, `export {tag, out, dependencies, resources}`, `import {path, tag, skip_missing}`, `read {tag, depth, elements, hex}`, `fields {tag, path?}`, `tree {root?, depth?}`, `refs {tag, limit?}`, `kinds {group?}`, `follow {tag, ref}`, `set {tag, path, value}`, `block.add`, `block.remove`, `block.duplicate` |
| `sound` | `read {tag, range?, permutation?, bins?}`, `play {tag, range?, permutation?, from_ms?}`, `stop`, `measure {tag \| clip \| path, range?, permutation?, class?, limit?}` ([Content checks](#content-checks)) |
| `scenario` | `blocks`, `list {block}`, `get`, `set {block, index, field, value}`, `add {block, like, position}`, `move {block, index, position, ypr}`, `delete`, `duplicate`, `select` |
| `volume` | `list {kind?}` (short form `volumes [kind]`), `select {index?, focus?}`, `set {index, origin?, center?, forward?, up?, size?}`, `add {kill?, center?, origin?, forward?, up?, size?}`, `remove {kind?, index}` |
| `collision` | `surfaces {class?, bsp?, offset?, limit?}`, `at {origin, dir}` or `at {point, within?}` (with `select?`), `select {index?, focus?}`, `objects {block?, index?}` |
| `markers` | `list {kind?, layer?}`, `render {kind?, team?, area?, view?, size?, out?}`, `area.set {id, area}`, `area.convert {id}`: the game type markers of the open map, their areas, and the marker shapes drawn on a grid ([Game type markers](#game-type-markers)) |
| `nav` | `status`, `back`, `forward`, `go` (the inspector's history); `pathfinding {bsp?, offset?, limit?}`, `bots {wait?, polys?, links?, spots?, offset?, limit?}`, `pick {origin, dir}` or `pick {point, within?}` (with `layer?`, `shown?`, `select?`) or `pick {kind, index, focus?}` (the Navigation layers) |
| `playtest` | `static {wait?}`: the open map's static checks and each built-in game type's readiness ([playtest.md](playtest.md#static-checks)); `run {preset?, game_types?, seeds?, minutes?, bots?}`, `status {run?}`, `result {run?, game_type?, measure?}`, `cancel {run?}`: headless bot matches on the open map and their measures ([playtest.md](playtest.md#what-a-run-is)) |
| `variant` | `list`, `get` (with `script`), `set {setting \| option, value}`, `rename`, `copy`, `export`, `import`, `delete` |
| `playlist` | `list` (with `unused` per playlist and the `templates`), `get {playlist}`, `set {playlist, title, description, template, clear}` (`template`: make the slot a playlist of that kind, or give a playlist another kind; `clear: true`: make it unused; [live-matchmaking.md §6.10](../live/live-matchmaking.md)), `entry {playlist, op, entry, to, weight}`, `entry_variant` |
| `ui` | `palette {query, run?}`, `screens`, `pick {screen, state \| retail}`, `press {input}`, `widgets`, `render {out}`, `select_folder {folder \| root}`, `sheet {folder?, root?, states?, columns?, out?}`; the shown screen's edits, of our screens or a retail one: `select {id?}`, `move {id, dx, dy}`, `set_rect {id, rect}`, `set_text {id, text}`, `revert {id?}`, `undo`, `redo`, `edits`, `save`, `animate {phase, frames?, out?}` ([ui-builder.md](../game/ui-builder.md#editing-in-the-editor)); `check {screen?, state?, aspect?}` ([Content checks](#content-checks)) |
| `preview` | `target {doc?, target?}`: a document's preview switch and what Preview in game asks of the game ([The preview switch](#the-preview-switch)) |
| `pair` | `list`, `status`, `connect {socket?}`, `disconnect {leave?}`, `auto {enabled?}`, `play`, `stop`, `control {action, ticks?}`, `edit {enabled}`, `preview {what, target?, ...}`, `audio {seconds?, every_ms?, call?}`, `result {id?}` ([Previewing content in the paired game](#previewing-content-in-the-paired-game)) |
| `mirror` | `status`, `objects {kind?, near?, limit?, at_tick?}`, `pick {from? \| ray?}`, `aim_select {enabled?}`, `camera {mode?, target?}`, `player {to, facing?}`, `pending`, `apply {what, at?, peers?}` |
| `view` | `camera {yaw, pitch, distance, target, focus, all, fly_speed}`, `snapshot {out, size, camera}`, `stats`, `layers {set?, preset?, xray?}` |
| `script` | `list {filter?}`, `source {name?}`, `follow {name?, link}`, `uses {kind, name}`, `builtins {filter?, unnamed?}`, `doc {path}` ([rich-editors.md](rich-editors.md#scripts)) |
| `strings` | `list {tag?, language?, query?, limit?, offset?}`: every `unic` string of the open map by name and language ([rich-editors.md](rich-editors.md#strings)) |
| `rules` | `open {path? \| starter? \| source?, discard?}`, `get {text?}`, `set {source}`, `save {path?, reload?}`, `format`, `reload {mode?}`: the game type script editor ([editor-guide.md](editor-guide.md#game-type-scripts)) |
| `hud` | `list {map \| package}`, `read {hud, state}`, `set {hud, widget, field, value, merge}`, `move {hud, widget, x, y, mode, merge}`, `render {hud, state, out}`, `check {hud, state}`, `save_as {path}` |
| `geometry` | `list {model?}`, `open {target}`, `close {target?}`, `read {target?}`, `check {target?}` |
| `check` | `run {what?, target?}`: every content check in one list ([Content checks](#content-checks)) |
| `image` | `generate {prompt, references, bitmap?, image?, size?, alpha?, out?, from?}`, `result {id?}`, `apply {id}`, `accept {id, save?}`, `discard {id}`: a picture made for a bitmap image from a prompt and the references the user picks ([Generating images](#generating-images)) |
| `mesh` | `mode {mode}`, `select {ids, op \| all \| none \| invert}`, `transform {translate \| matrix, pivot?, merge?}`, `ops`, `apply {ops}`; each with `target?` |
| `model` | `list {group?}`, `read {tag, variant?}`, `render {tag, view?, variant?, character?, biped?, hands?, size?, camera?, out?}`, `check {tag, variant?}`, each with `map?` and `pogo?` ([Models and customisation](#models-and-customisation)); `colors`, `emblem` (planned) |
| `anim` | `list {tag}`, `pose {tag, animation, frame?, overlay?, model?, size?, out?}` ([Model poses](#model-poses)) |
| `custom` (planned) | `list`, `set` |

`view.camera` returns `fly_speed` in world units per second. Its optional
`fly_speed` parameter accepts values from 0.5 through 1000.

Notes on some:

- **Scripts.** Every `script.*` call reads the open map's script tables
  with the source's `default.xbe`; without a map it is `NO_DOCUMENT`,
  and without the executable it is `REFUSED` with the tool's sentence
  and the paths it looked at. `script.source` shows the definition in
  the Inspector and gives each link's byte span in `source`, its kind
  (`script`, `global`, `object`, `volume`, `tag`), its `target` now
  (`{script}`, `{global}`, `{block, index}`, `{volume}`, `{tag}`) or
  `null` and `why`. `script.follow` does what a click on the link does
  and is `REFUSED` with the same `why` for a link that goes nowhere.
  `script.uses {kind: "recording"}` is `REFUSED` until recordings are
  read. `script.builtins` gives each builtin's `name` and `from` (`map`,
  `doc` or `null`), its `label` (the name or `#builtin:<index>`) and its
  call form. `script.doc {path}` names builtins from the user's script
  documentation listing, read from that path now and not stored
  (`null` drops it); its `doc` says how many entries read, agreed with
  the map's names, disagreed and named. `script.external` writes the
  working copy VS Code and Zed edit (`<data>/scripts/<map>/<map>.hsc`,
  with `h2hs.json`, the context `h2 hs lsp` reads, beside it; a copy
  holding edits is kept) and opens it in `editor` (`code`, `zed`, or
  `auto`, the default) at the definition `name` (default: the one
  shown). Its answer has the `path`, the `context` file, the 1-based
  `line`, the `commands` that open it (`{program, args, line}`) and
  `working`. `launch: false` starts nothing; launching with no such
  editor is `REFUSED`, and an unknown `name` is `INVALID_PARAMS`.
  `script.working` reads the copy now and answers which definitions
  differ from the map's (`changed`, `added`, `removed`), its `problems`
  against the context (`line`, `col` from 1, `severity`, `message`,
  `code`), and `stale` when the map's scripts changed while it held
  edits; `{open: false}` when none is open. It needs no map. Nothing is
  written into the map ([editor-guide.md, "Scripts in VS Code and
  Zed"](editor-guide.md#scripts-in-vs-code-and-zed)).
- **Variants of a script.** `doc.open {variant: {script: <starter>}}` (a
  `rules.get` starter's name, or `open` for the script in the script
  window) opens a new, unsaved variant of it: the record
  `variants::portable::record_for` builds. `variant.get`'s `script` is
  `null` for a record naming none, `{prefix, found: false}` when the
  editor lacks the script, else `{name, about, scoring_carrier, options,
  quick}`: each option has `index`, `name`, `label`, `description`,
  `kind` (`bool`, `int`, `enum`), `value`, `text`, `default` and `quick`,
  and each `quick` row `label`, `option` and `text`. `variant.set
  {option, value}` sets one option by its name in the script (a bool as
  0 or 1, an enum's value as its position from the option's low end); a
  run of edits to one option is one `doc.undo` step. An unknown option
  or a value out of range is `INVALID_PARAMS`, a built-in `REFUSED`.
- **Game type scripts.** `rules.*` drive the one script the Game type
  script window shows (`app/rules_edit.rs`). Each answers its state:
  `title`, `path`, `dirty`, `problems` (each `line`, `col` from 1,
  `severity`, `message`, `code`; errors first), `check` (the
  [workbench](../game/rules-engine.md#workbench) report: `ok`, `hash`,
  `errors`, and `report` with the worst tick against the budget, the
  handlers, slots, IR size, capabilities, and the `settings` the
  script's block names and the `quick` rows it marks), `mode`, `reload_on_save`,
  `reload` and the `starters` `rules.open {starter}` takes (a built-in's
  or example's name, any case). `rules.get {text: true}` adds the text.
  `rules.open` is `UNSAVED` over unsaved edits without `discard`, and
  picks the window's tab. `rules.set` reaches the text widget even while
  someone is typing in it, and leaves the keyboard where it is.
  `rules.save` with no `path` on a script that has no file is `REFUSED`.
  `rules.reload` compiles and verifies the text here, queues the blob for
  the game link and answers `{queued: hash}`; the game's answer arrives as
  `rules.get`'s `reload`: `{state: sending}`, then `{state: done, mode:
  in_place | restarted, hash, tick, reasons}` or `{state: refused, why}`
  (not paired, a game without `rules.reload`, or the game's refusal;
  [Rules scripts](../runtime/agent-api.md#rules-scripts)).
  `rules.reload {path}` is an outside editor's save: when `path` is the
  open script's file it is read first (`UNSAVED` when Studio has other
  unsaved edits; the window then asks which to keep); another file is
  opened and watched, `UNSAVED` over unsaved edits without `discard`.
  `rules.external {editor?, line?, launch?}` saves the script to its
  file (or a new one under `<data>/scripts/`), watches it both ways and
  opens it in `editor` (`code`, `zed`, `auto`) at `line` (default: the
  cursor's); it answers `path`, `line`, `col`, `editor`, `commands` and
  `launched`, as `script.external` does. `rules.run {scenario, source?,
  path?, label?, trace?}` runs a script over a
  [scenario](../game/rules-engine.md#workbench) on the fake game, as `h2
  rules run` does: `scenario` is a file's path or the scenario object,
  and the script is `source`, else the file at `path`, else the open
  one. It answers `ok` (no script error), `result` (how the run ended),
  `text` (the printout), `ticks`, `calls` and `label`; a script that
  does not build is `REFUSED`. The VS Code and Zed commands call these
  ([rules-editors.md, "Commands that talk to Studio"](rules-editors.md#commands-that-talk-to-studio)).
- **Tags.** A `tag` param is an id (`"0xe1230045"` or a number), a full
  name, `name.group`, or a group alone when the map has one tag of it
  (`"scnr"`, `"matg"`).
  `tag.read`, `tag.set` and `tag.block.*` are the Tags tool's
  (`api/tag_fields.rs` over `app::tag_api`, the registry
  `tags::registry`): every field by its label, paths from the root
  (`parts[1].speed`, `+0x14` for an unnamed field), values as `tag.read`
  shows them (`"group:name"` for references), each edit one undo step on
  the `tags` document and a patch the map save writes. A `tag` may also be
  `group:name` as the Tags tool writes it. `tag.read {"raw": true}` gives
  the schema's structural view instead. The scenario tag's edits stay with
  the Scenario tool: `tag.set` paths `netgame_flags[0].position` and
  `tag.block.*` on `netgame_flags` run as the matching `scenario.*` calls.
- **Field rows.** `tag.fields {tag, path?, unnamed?}` returns `{fields:
  [...]}` for the root or a block element such as `parts[1]`. Offsets
  such as `+0x8[1]` also select a block element. The rows are the field
  panel's (`fields::rows`, [tag-editing.md](tag-editing.md)), in its
  order: the named rows, then the Unnamed fields group's (padding last);
  `unnamed: false` leaves those out. Each row has `name`, `offset` in
  bytes, `kind`, `size` in bytes, `value`, `description` (the
  definition's `doc` text, or null), `evidence` (its meaning text),
  `section`, and:
  - `label` (the name in words), `named`, `type` (the type in words:
    `Whole number`, `Decimal number`, `Angle`, `Colour`, `Range`,
    `Choice`, `Switches`, `Name`, `Tag reference`, `List`, `Bytes`,
    `Internal value`, `Unnamed value`, `Padding`), `unit` (or null) and
    `view` (the definition's `view=` word, such as `rgb` or `function`,
    else null; [rich-editors.md](rich-editors.md)); a `function` view's
    row also carries `function: {type, ranged, range, length}`, its
    curve's shape (null when the data is not a function blob, and the
    row says why);
  - `editable`, and `read_only`, the reason as a sentence when not;
  - `edited` (the value differs from the loaded file) and `file_value`
    (the file's value when edited, else null);
  - `effect: {id, text, why}` (an id of the effect table in
    [live-mirror.md](live-mirror.md), "How an edit applies"), always
    given; `chip`, what the row shows at its right edge, null with no
    game connected (when a change takes effect, or for a changed field
    its state: `Waiting`, `Not sent`, `Waiting for reload`, `Sent · Not
    known`); `sending`, a changed field's state with a game connected
    (`sent`, `waiting`, `waiting_for_reload`, `not_sent`, else null);
    `live: {kind, text}`, a changed field's `mismatch` or `refused` mark
    with a game connected;
  - `path`, the `tag.set` path from the root by field names
    (`parts[1].value`, also for an offset path); `summary`, what the row
    shows on its line: `2 items` or `16 bytes` for a list or data, the
    value's name for a choice (`Value 7 (not named)` when none names
    it), `2 of 9 on: Loud, Quiet` for switches, and the kind and last
    path segment for a tag reference (`No tag` when it is empty);
  - `choices: [{value, name}]` for a choice, `bits: [{bit, name}]` for
    switches, `groups` for a tag reference, `count` and `capacity` for a
    list, and `show_as` (`decimal`, `whole_number` or `hex`) for an
    unnamed value.

  An offset path in `tag.set` addresses the typed field that starts
  there. Its row gives the width accepted for an integer or enum value.
  A block's value is its element count. Scalar values follow
  `tag.read`. The rows use the open or browsed tag document and include
  its pending edits (`api/tag_fields.rs`).
- **Curves.** `function.read {tag, path}` reads the function blob a
  `view=function` data field points at
  ([rich-editors.md](rich-editors.md), "Curves"; `api/curves.rs`):
  the type word (`identity`, `constant`, `transition`, `periodic`,
  `linear`, `linear_key`, `multi_linear_key`, `spline`, `multi_spline`,
  `exponent`, `spline_2`, `unknown_<byte>`), whether it is ranged, the
  two sub-type bytes (set 0's and set 1's wave or shape), the output
  range, the blob's length, and `sets`: the two sets' typed values, the
  words the engine's evaluator reads named as the curve editor's
  handles (`shape`, `from`, `to` for a transition; `wave`,
  `frequency`, `phase`, `minimum`, `maximum` for a periodic; `slope`,
  `intercept`; `base` and three `keys` of `value`, `position`,
  `inverse_span`; `points` and `coefficients`; `minimum`, `maximum`,
  `power`; a constant's, type 6's and type 8's words as `words`; `{}`
  for identity), null for a type with no layout. `editable` is false
  and `read_only` says why for a type the game evaluates as 0 or a
  tag that may not be written. `function.set {tag, path, type?,
  ranged?, range?, sets?}` edits the curve as one undo step through the
  Tags tool's own command: `type` changes the type (any of `identity`,
  `constant`, `transition`, `periodic`, `linear`, `linear_key`,
  `spline`, `spline_2`, `exponent`), rebuilding the blob at the new
  type's `blob_len` with defaults (`Function::with_kind`: a transition
  from 0 to 1, a periodic cosine of one cycle, a line of slope 1, a
  ramp, a cubic that is `t`, an exponent that is `x`, a constant of 0,
  the flags, output range and unread header bytes kept) and writing it
  as a resize of the data, so a longer blob grows the tag only when
  growth is on and is refused by name when it is off; `sets` in the
  same call apply to the new type. `ranged` turns the second
  set on or off, `range` sets the output range, and each set object's
  keys present are written (its `shape` or `wave` writes the header
  byte, as the dialog's picker rows do); the answer is the new
  `function.read`. A change to a type the game evaluates as 0 (`multi_linear_key`,
  `multi_spline`, an unknown byte) is refused, and so is anything the
  Curves section refuses.
  `function.sample {tag, path, inputs, range_input?}` returns
  `{outputs}` from `Function::evaluate` itself (the ported engine
  evaluator) at each input, `range_input` blending the two sets of a
  ranged function (default 0, set 0); types the engine does not
  evaluate give 0. A path that is not a function curve field is
  `REFUSED`.
- **The field panel.** `tag.view {tag?, path?, field?, search?,
  changed?, unnamed?, show_as?}` drives the Tags tool's panel and
  returns what it shows. `tag` selects a tag in the Tags tool. `path`,
  a list element (`parts[1]`, or by offset `+0x8[1]`), shows that
  element as the whole panel, as its **Open alone** does; empty shows
  the root. `field`, a `tag.set` path, focuses that field through the
  navigation (as `nav.go`), which opens its lists and its Unnamed
  fields group; on its own it is shown from the root, and it clears the
  filters. `search` sets the filter row's **Search fields** (empty
  clears it) and `changed` sets **Changed only**; with either on, the
  hit list replaces the rows, as `tag.find` describes. `unnamed` sets
  **Show unnamed fields**. `show_as: {path, as}` sets how an unnamed
  value is shown (`decimal`, `whole_number`, `hex`, or null for the
  guess from its bits), kept for that field of every tag of the group
  and saved with the layout. With no params it only reads. The result
  is `{tag, tabs, tab, path, breadcrumbs, field, search, changed,
  unnamed, rows, unnamed_hidden, header}`: `tabs` is `[{id, label}]`,
  **Fields** first then the tag views of its group
  (`fields::rich::TAG_VIEWS`), and `tab` the one shown; the `tab` param
  shows one (`fields`, or a view of the group), refused by name when the
  group has none; `path` is the element shown (by field
  names, empty for the root), `breadcrumbs` its crumbs as drawn
  (`weap: objects\test\blaster`, then `Parts 1 scope` per list step),
  `rows` the element's `tag.fields` rows the panel shows — with a
  search or **Changed only** on, the hit list's rows, each a
  `tag.fields` row of the field it found — `unnamed_hidden` the
  unnamed rows in a folded group (`unnamed: true` adds them to `rows`;
  0 with a filter on), `field` the focused path, `search` the panel's
  search text (empty when none) and `changed` its **Changed only**, and
  `header` the header's lines as drawn, without icons: the kind and
  name, the path with the unsaved count, any browsed, BSP, shared-map
  and read-only notes, how a change reaches the game (`Changes are
  saved with the map.` with no game), and the search budget's sentence
  last when the walk stopped. The datum and address are not in it.
- **Field search.** `tag.find {tag, query?, path?, changed?, unnamed?,
  limit?}` returns the fields the panel's filter row finds
  (`fields::find`): `{hits: [{path, breadcrumbs, label, type, value,
  matched}], truncated, why}`. `query`'s words (case-insensitive) match
  the label, the name, the description, the type words, the choice and
  switch names, and the referenced tag's row text; every word must
  match somewhere, and `matched` names the first candidate that alone
  holds them all (the label for an empty query and in **Changed
  only**). The walk covers every element of every list from `path` (a
  list element to search within; the root by default), to 12 lists
  below it and 5,000 list items, in the panel's order; reaching the
  budget sets `truncated` with the sentence in `why`. `changed` keeps
  only the fields that differ from the loaded file, lists included
  (with `query`, both must hold). `unnamed` leaves the unnamed rows
  out; `limit` cuts the hits and sets `truncated`, with `why` only for
  the budget. A `path` that names no element is refused with
  `INVALID_PARAMS`.
- **List elements.** `tag.elements {tag, path, filter?, offset?, limit?}`
  returns a list's elements as the panel's rows show them (`1  scope`):
  `{count, capacity, elements: [{index, title, path}], truncated}`. The
  title is the value of the block's `title=` field, else of its first
  named name or tag reference, else null (the row shows the index
  alone; `fields::row::element_title`). `filter` keeps the elements whose
  row words have every word of it, as the list's filter box does (a list
  of more than 12 elements has one); `offset` and `limit` page the hits
  and `truncated` says more follow. `path` that is not a list is refused
  with `INVALID_PARAMS`.
- **Pick lists.** `tag.choices {tag, path, query?, kind?, limit?}`
  returns what the field panel's pick dialog lists for the field at
  `path` (`fields::picker`). A choice gives `{values: [{value, name}]}`,
  its named values. Switches give `{bits: [{bit, name, label, on}]}`,
  every bit of the word, `name` null for a bit nothing names. A tag
  reference gives `{kinds: [{group, name}], tags: [{id, group, name}],
  truncated}`: the kinds of the map's tags it takes, and every such tag
  with no cap. `kind`, a group code, narrows the tags as the dialog's
  Kind row does; `limit` cuts them and sets `truncated`. An unnamed value
  gives `{views: [{id, name}], shown}`, the **Show as** choices and the
  one in use. `query` narrows values and tags as the dialog's search box
  does: every word in the name, or in `group:name` for a tag. Any other
  field is refused with `INVALID_PARAMS`.
- **Bytes and reset.** `tag.bytes {tag, path?}` returns the bytes of an
  element (the root by default, or a block element such as `parts[1]`)
  or of one field: `{address, size, hex, file_hex, spans: [{offset, size,
  name, named}]}`. `hex` is the bytes now and `file_hex` the loaded
  file's, null when the file has none. The spans are the element's
  fields at offsets from `address` and tile it, or the one field.
  `tag.reset {tag, path}` puts a field back to its value in the loaded
  file, as **Reset to file value** does: one undo step, and none when it
  already has that value. It returns `{value}`. Lists and data are
  refused with `INVALID_PARAMS`, and the scenario's fields and a browsed
  map's are refused with `REFUSED`.
- **Scenario tree.** `tag.tree {"root":"scenario","depth":2}` reads the
  open scenario and returns `{root, unreachable, incomplete, truncated}`.
  The root's children are Structure (BSPs with their lightmaps), Skies,
  Objects (grouped by palette), Globals and Other references.
  Each row has `label`, `tag` (ID), `group`, `name`, `placed` (the
  count for a palette entry), `via`, `cycle`, `edited`, `cache` and
  `children`. Section labels can include a count. `root` defaults to
  `"scenario"`. `depth` limits references below palette entries
  (default 3, maximum 8). `unreachable` uses the full walk from the
  scenario and globals, including BSP-region links and unsaved
  scenario reference edits. `incomplete` marks a partial reachability
  walk and leaves `unreachable` empty. `truncated` indicates that an
  object chain hit its display walk budget.
- **Kinds.** `tag.kinds {group?}` reads the kind registry
  ([live-editors.md](live-editors.md), "The registry";
  `app::kinds::KINDS`). It returns `{kinds, default, groups, target}`:
  `kinds` has one row per kind with `label`, `groups`, `editor`
  (`field_panel`, `scenario`, `modeling`, `textures`, `audition`,
  `hud`, `ui_screens`, `scripts`, `animation`), `preview` (`none`, `viewport`,
  `model_render`, `thumbnails`, `waveform`, `tool`, `reports`),
  `subject` (`none`, `level`, `placement`, `swatch`, `loop`, `play`),
  `classes` (`field`, `raw`, `shape`, `host`) and `missing`; `default`
  is the row a group without one gets (the field panel, no preview,
  `field` and `shape`); `groups` lists every group the registry or the
  open map has, each with `group`, `kind`, `default`, `editor`,
  `preview`, `subject` and `classes`; `target` is where the game view's
  changes go now (`off`, `own`, `attached`, `attached_edit`). With
  `group` (up to four characters, as `tag.list` writes it) the reply is
  that group's line alone; a group no row names gets the default. It
  needs no open map.
- **References.** `tag.refs {tag, limit?}` lists direct links in the
  open map, including in-memory tag edits and current scenario-owned
  reference fields. Each result has `to` (the target ID),
  `via` (the field path or scanned tag offset),
  and `found` (`"field"` or `"scan"`), alongside `path`, `id`, `group`,
  `name` and `missing` when the target is absent. The aligned scan covers
  a group without a layout and supplements the scenario's structural
  schema. Tags whose payload lives in a separate BSP region use the
  source schema. `tag.follow` accepts a result's offset path
  (`"0x98[0].0x0"`), `via`, or its position in the list. The list has
  a default limit of 500.
- **Tag chains.** `tag.chain {"block":"weapons","index":0}` follows a
  placement's palette or item reference in the open map.
  `tag.chain {"tag":"weap","depth":2}` starts at a tag in the document
  shown by the Tags tool. Both read current tag edits. `depth` counts
  references below the root (default 3, maximum 8). The response is
  `{root, truncated}`, with a null root for a placement without a tag.
  Each node has `tag` (ID), `group`, `name`, `via` (the field path),
  `label` (a named shortcut, or null), `where` (`here` or `shared`),
  `cache` (the shared cache name, or null), `variant` (model variant
  name or ID, if known), `cycle`, `uses` (the placements and tags that
  use it, as `tag.uses` counts them) and `children`. A missing target
  keeps its ID and path with null tag metadata. Cycles stop the current
  branch.
  `truncated` means a traversal limit beyond the requested depth cuts
  the result short. Live handles belong to the [click-to-edit design](click-to-edit.md).
- **Where a tag is used.** `tag.uses {tag, highlight?}` answers from the
  reverse reference index of the open map's tags, cached in the document
  (a reference edit rebuilds it): `{tags, placements, objects}`. `tags`
  are the tags that refer to it, transitively, so the object tags whose
  chains run through it are named, in the map's tag order, each with
  `via`, the field path one step closer to it (the scenario's own
  references are not a tag's; they are the placements'). `placements`
  are the placements whose palette entry reaches it, in block-name order
  then index. `objects`, the live objects whose definition reaches it,
  is empty until a game is paired (the click-to-edit design, C5).
  `highlight: true` outlines the placements in the viewport (a square on
  the ground each) until the next highlight or selection;
  `highlight: false` clears them. `tag.chain`'s nodes carry the count as
  `uses: {placements, tags}`.
- **Inspector navigation.** `scenario.select {"tag":"weap"}` makes
  a tag the inspector's selection and clears the placement selection.
  `nav.go {"tag":"hlmt","path":"render_model"}` focuses a tag and,
  optionally, one field by its registry path. It keeps the viewport
  selection. `nav.go {"block":"weapons","index":0}` selects a placement.
  `nav.go {"panel":"pending"}` opens the Pending tab. `nav.go {"tag":
  "weap", "element": "parts[1]"}` shows a list element alone in the Tags
  tool, as its **Open alone** does, and Back returns to it. These are also the
  target forms returned for placement and panel issues. `nav.status`,
  `nav.back` and `nav.forward` take no params. All `nav.*` methods
  return `{selected, volume, tag, path, element, breadcrumbs, can_back,
  can_forward}`; `element` is the list element shown alone, by field
  names, or null.
  `volume` is the selected trigger volume's index, or null.
  `selected` is a placement `{block,index}`, a tag `{tag}` or null.
  `tag` and `path` identify the focused tag and field, or are null.
  Breadcrumbs begin with the placement when selected, followed by tags
  as `{tag,group,name}`. Back and forward restore the tag and field.
  At either end of history the call returns the current state.
- **Other maps and tag bundles.** `tag.browse {"map": "shared"}` shows
  another map of the source in the Tags tool, read-only (`shared`,
  `single_player_shared`, `mainmenu`, any level); `tag.browse {}` goes
  back to the open map. `tag.list`, `tag.read`, `tag.set` (refused),
  `tag.resources` and `tag.export` then work on the browsed map;
  `tag.tree`, `tag.refs` and `tag.read {"raw": true}` use the open map.
  `tag.list` searches like the Tags tool (`query`: words each in
  `group:name`; `path`: a folder) and marks each tag with `elsewhere`
  (the shared cache that has the data of a tag with none here) and, with
  `homes`, its raw pointers counted by file. `tag.resources` lists the
  data references and raw pointers, and a bitmap's images or a sound's
  permutations. `tag.export` writes a tag bundle (`.h2tag`,
  [editor.md](editor.md#tag-bundles)) to `out`, else under
  `<data>/tag-bundles/`, refused in a Git checkout; `tag.import`
  replaces the open map's tags with a bundle's as one undo step of the
  `tags` document. It does not add tags: one the map lacks refuses the
  import unless `skip_missing`.
- **Sounds.** `sound.read` lists a `snd!` tag of the map the Tags tool
  shows: its header, every pitch range with its permutations
  (`encoded_bytes`, `chunks`), and for the permutation asked for (range 0,
  permutation 0 by default) its decoded `channels`, `rate`, `frames`,
  `duration_ms` and `bins` waveform columns of `[low, high]` 16-bit
  peaks, and its `mouth` curve. A permutation that cannot be decoded
  gives `refusal` in place of those (a codec stream without the game's
  executable says `This sound needs the game's executable to decode.`)
  or `error` (a bad length, a missing resource file); `mouth` is given
  either way, because the curve needs no executable. It also gives
  `playback`: whether something plays, which, `position_ms` and the
  `output`. `sound.play` plays a permutation from `from_ms` through the
  host's default audio output, opened on the first call, and returns at
  once; a refusal is `REFUSED`, no device or an unreadable resource
  `FAILED`. `sound.stop` stops it. `sound.mouth` reads one permutation's
  mouth curve from the sound's extra-info resource in the map's `ugh!`:
  `{ticks, curve}` with one aperture value, 0 to 1, per 30 Hz tick, or
  `curve: null` for an unvoiced permutation (the sound has no extra
  info, its descriptor index is negative, or the mouth entry is
  zero-length). The window's Sound window draws the same curve under
  the waveform and calls the same `App::sound_*` methods. The
  executable comes from the source: the disc's `default.xbe`, or the one
  beside a loose map or folder (`H2_XBE` names another).
- **Placements.** `scenario.set` takes a number, `[x, y, z]`, or for a tag
  reference `{"id": "0x..."}` (or `null` for none). The scenario checks it
  first: read-only fields, integer ranges, finite floats, palette indices,
  tag groups. `scenario.add` and `.duplicate` are refused where the block
  is full and the map editor can't grow it safely (the same
  `GROWTH_REFUSAL` the window shows; `data` has the count and capacity);
  `.delete` where something refers to entries by index. A new entry goes
  to `position`, else the camera focus, in one undo step.
- **Pairing.** The editor pairs with a running game through the game
  link in the bar. [Editor guide](editor-guide.md#play-from-the-editor)
  `pair.list` looks at the games now: the sockets in the agents directory
  where `h2 play` listens by default (`agent_proto::discover`), plus the
  default and `$H2_AGENT` sockets and Play's own. Each entry has its
  `socket`, `pid`, `map`, `phase`, `paused`, `tick`, whether its map is the
  open one (`matches`: `same`, `different`, `unknown`) and whether the
  editor is `paired` with it; a socket nothing answers on is removed from
  the agents directory. It works headless. `pair.status`, `pair.connect`
  and `pair.disconnect` act on the editor's pairing (`api/pair.rs`, through
  `play::pair::Bridge` on the app). The window runs it each frame and
  `h2-editor --serve` between requests (`Api::serve_once`); an `Api` with
  neither answers REFUSED. `pair.connect` answers before the editor has
  paired: poll `pair.status` for `state: "connected"`. `pair.connect` with no `socket`
  takes the one game running, and is refused with none or several.
  `pair.auto {enabled}` reads or sets auto-pair, on by default.
  The `link` object in `pair.status` contains `state`, the pill's
  `text` and its `why` tooltip (string or null). Its states are
  `no_game`, `found`, `connecting`, `reconnecting`, `viewing`, `editing`, `blocked`,
  `other_map` and `lost`. The top-level `state` continues to report the
  socket connection. When a game omits an optional method, `link.unavailable`
  lists the affected features and methods. `link.issues` is an array of
  `{kind,text,target}` rows. The kinds are `waiting`, `reload`,
  `refused` and `mismatch`. Pass a row's `target` directly to `nav.go`
  to inspect the field, placement, tag or Pending tab. A refused player
  move has a target that opens Pending and stays active for up to four seconds.
  A successful move or a change of scope clears the issue.
  A clear link reports
  an empty array. The pill shows the largest count. During reconnect, `link.elapsed_ms` counts from the drop.
- **Playtest.** `pair.playtest {}` queues F5 and reports the step for the
  window's published state: `launch`, `pair`, `resume`, `reload` or
  `ask`. A reload or launch using the saved player place includes
  `place: {pos, yaw, pitch}` (angles in degrees). An unavailable reload
  reports the manual-load instruction in `why`. An `ask` includes its
  integer `prompt` ID. Echo that ID with `confirm` to answer the same
  prompt. Closing it, changing its operation or editing the document
  expires the ID. A bare `confirm` answers the prompt open at the time
  of the call. For `ask: "send"` choose `"send"` or `"cancel"`. For
  `ask: "load"` choose `"load"`, `"open"` (open the game's map here)
  or `"cancel"`. An open or cancel reply carries the choice in `answer`.
  After automatic pairing, poll `pair.status.ask` and `pair.status.prompt`
  for the follow-up prompt. A stale offer or mismatched map identity
  returns `REFUSED` with the reason.
  `pair.settings` reads or sets `pause_while_editing` (default false)
  and `spawn` (`auto`, `level`, `resume`, `camera` or `selection`).
- **Game controls.** `pair.play` queues the same action as F5. `pair.stop` stops
  the editor's own game. `pair.control` accepts `pause`, `resume` or
  `step` as `action`. Step requires a paused game and accepts `ticks`
  from 1 to 600 (default 1). `pair.edit {enabled}` sets the Edit switch
  on the pairing. A game that is already running pairs in View mode.
  Play starts its own game in Edit mode. The four calls need the window
  or `--serve`, queue a request on the pairing and return `{"requested":true}`. Read
  `pair.status` for the result in a later frame. A call that needs a
  game, an open map or a pause returns `REFUSED` when that requirement
  is missing. The pill's **Open** action uses the normal map selection
  path (`doc.open {map}` for a map in the source).
- **Live tag editing.** Pairing starts in View. Play starts the editor's own
  copy in Edit. `pair.edit` returns the requested `mode` or `REFUSED` with
  the reason. The editor checks the loaded file checksum, epoch and session
  from the game's `edit.map`. In Edit, tag fields and placement fields travel
  together in one `edit.batch`, each with its expected `was` value. Tags use
  the `at` form with hex bytes. The optional `known_epoch` on `edit.map`
  refreshes the loaded datum table used to check reference targets. Undo and redo use the same check.
  `mirror.pending` returns `pending` rows with `doc,path,class,effect,text,why` and an optional
  `tag` and `label`: `class` is the change's fan-out class (`field`, `raw`, `shape` or
  `host`, `app::live::Class`, [live-editors.md](live-editors.md)): `raw` for `geometry`,
  `bitmaps` and `collision` rows, `shape` for a block layout or a new map string, `host`
  for a `host` row (a Host change the game has no reload for, or one an attached game
  in View waits to take, with `host` naming its source), else `field`;
  `effect` is when the change takes effect (`now`, `next_use`,
  `next_spawn`, `reload` or `not_known`, `play::tag_sync::Effect`) and `text` its sentence,
  the game’s existing `journal` from `edit.changes`, and `leaving`.
  A placement block with a different layout revision from the loaded copy
  goes to a game that offers `world.placement.edit` as one transaction in
  Edit; its row is a `now` row while the transaction is out and leaves on
  the receipt. Otherwise, or when the game refuses or its answer is lost,
  the block appears as one `Reload` row with the reason (`refused`, and
  `unknown` for a lost answer), and its fields stay out of the batch until
  a transaction, undo, redo, or loading a Play copy brings the revisions
  into agreement. An entry the game wrote without making its object is a
  `Reload` row of its own (`path` `block[index]`). Rows of this path carry
  `placement_edit: true`.
  A structure's level collision edit is one row with `doc` `collision`,
  `path` `structure_collision:<tag>:0` and `rebuild` (whether the commit
  changes indices). In Edit it goes as `edit.collision.stage`, then
  `edit.collision.commit` (realtime-edits.md, "The editor"); a refusal
  makes it a `reload` row with the game's reason, and a `MISMATCH` marks
  it `conflict` with `resolvable:false` until the documents change or
  the user sends again.
  `mirror.apply {what:"send"}` enters Edit. On MISMATCH, `take` and
  `overwrite` require the row’s numeric `at`. `take` reads the field at the
  current game boundary before applying the bytes to the document and baseline.
  A failed read or a relocated field leaves the conflict pending. A row with
  `resolvable:false` opens Pending and disables both resolutions. Take and
  Overwrite require the tag's field path to resolve to the refused address. On rejoin, `adopt` takes
  the game's current bytes as the baseline and preserves the document.
  `revert` uses the game's reported restored and left-alone bytes as the
  baseline for the next send. A System Link host
  confirms the peer count using `confirm` with `peers`, including when
  another map is loaded. Confirmation applies to the paired host, map epoch
  and peer count. Field writes still require the open map. `reload` writes
  a play copy. In Edit, `respawn` applies acknowledged creation-time
  placement edits through `world.object.respawn`. Moving, deleting and
  undoing placements use the same document actions as the window. The
  game link calls the advertised object methods with the current host
  confirmation. Object rows in `mirror.pending` explain changes that
  need a reload or cannot reach a live object.
  `pair.disconnect {leave:"keep"|"restore"}` makes the leave choice over the
  API. During restore, `mirror.pending.leaving` stays true while the editor
  waits for an outstanding batch, reverts the journal, and disconnects on
  acknowledgement. Omitting `leave` opens the prompt in the window. Both `edit.set` and
  `edit.revert` enforce the same game-side session check as `edit.batch`.
- **Live mirror.** The paired editor reads the game's `objects`,
  `players` and `camera` stream. `mirror.status.stream.refused.camera`
  says why a game has no camera layer. `mirror.status` reports the stream state, layers,
  frame rate, presentation delay, dropped and skipped counts, and the
  store's latest, earliest retained and shown ticks. `mirror.objects`
  returns `{tick, objects, truncated}` for the viewport sample.
  `at_tick` selects an exact game sample in the interpolation window
  and answers `REFUSED` when it has expired. Each object has its salted
  `id`, tag group `kind`, definition `tag`, `pos`, basis, scale,
  velocity, flags, parent, vitality, placement and node count. Positions
  of attached objects are relative to their parents. `near` takes
  `{point: [x,y,z], radius}` in world space. Bind-pose objects have no
  streamed nodes. Pairing and presentation are described in [live-mirror.md](live-mirror.md).
- **Live selection.** `mirror.pick {"from":"aim"}` uses the local player's
  aim and skips that player's unit and held weapon. A viewport click in
  Player's eyes skips the viewed player's unit and held weapon. Supply
  `{"ray":{"origin":[x,y,z],"dir":[dx,dy,dz]}}` for an unrestricted world ray.
  The first live object in front of the level returns
  `{object, placement, tag, point, distance}`. Handles are hex strings,
  `placement` is a linked `{block,index}` or null, and a miss returns null.
  The link uses the object's kind and placement index in the loaded block
  layout. If a structural edit changes that layout, the object stays
  unlinked until undo or a reload restores matching slots. For a preview, the
  current map’s respawn receipt links its full salted ID to its document
  placement, including after a swap-removal changes the row index.
  Picking over the socket leaves selection alone. Clicking in the viewport
  selects the linked placement or the live object and its available definition.
  `mirror.aim_select {enabled?}` returns `{enabled,candidate}`. The
  candidate has the same shape as a pick. Holding the displayed aim on one
  object for 0.6 seconds selects it. A stream cut restarts the dwell timer.
  In View and Edit the paired window renews its selection and candidate
  marks once a second on the game's display. Unpairing clears the link
  plate and marks. Pairing again to the same endpoint waits for the old
  connection's clear before sending either lease. A dropped connection
  lets the three-second leases expire. `pair.status.link.unavailable` reports
  missing presentation methods and the host's display refusal. While the
  game waits for a genuine frame, a `drawn: false` reply keeps the marks
  pending without a warning. Individual refused marks still report their
  indices. A failed clear retries at the renewal interval. The game plate
  shows the largest link issue count, or `Aim select` when aim selection
  is on and the count is zero.
- **Shared camera.** `mirror.camera {mode?,target?,view?}` returns
  `{mode,target}`, with `view` in game mode.
  `free` releases the editor viewport, `follow` keeps the eye's orbit offset
  as the local player moves, `eyes` looks along the player's aim from
  eye height, and `game` takes a game view's camera from the `camera`
  layer exactly (eye, basis with roll, vertical field of view, near and
  far; the editor viewport's aspect). `view` names one as
  `<kind>:<player>:<view>` (`game:0:0`, `first_person:0:0`); without it the
  first `game` view is taken. A stream without the layer, a view the game
  did not draw or a `target` in game mode is refused. Leaving game mode
  keeps the eye and restores the editor's field of view. Every mode but
  `game` draws each game view's frustum. An explicit `player.<index>` keeps the camera on that local
  player across samples. While its unit is missing, the viewport holds its
  pose and resumes following when the unit returns. A map change or unpair
  releases the camera. The free camera reports null. The viewport toolbar
  has the same modes. `mirror.player {to,facing?}` queues a move in Edit
  and returns `{"requested":true}`. Give a world position, `"camera"` for
  the ground under the editor camera's focus, or `"cursor"` for the level under the
  viewport pointer. T uses the cursor. Shift+T or `facing:true` also sends
  the camera's yaw and pitch. A missing destination, current player sample,
  permission or game method returns `REFUSED`, as does a pending reload or
  a playtest transition. The control worker checks the latest exact sample
  for the expected epoch and salted unit immediately before the game call.
- **Mirror controls.** `mirror.control {op, ticks?}` pauses, steps or
  resumes the paired game as `pair.control` does (`op` is `pause`,
  `resume` or `step`; `ticks` 1 to 600) and returns
  `{requested, tick}`, `tick` being the store's latest game tick before
  the request. Each step sends one frame per tick, so after a step of `n`
  ticks `mirror.status.store.tick` is `tick + n`.
- **Viewport layers.** `view.layers {}` returns `{layers, xray}`. Each layer
  has a stable `name` for calls, a display `label`, a `group`, the visibility
  switch `on`, a `count` and its legend `color` (RGB, each channel 0 to 1).
  Counts include hidden entries. For example,
  `view.layers {"preset":"everything","set":{"fog":false},"xray":true}`
  shows every layer except fog and draws occluded overlays at quarter
  strength. Presets are `art`, `gameplay`, `collision`, `navigation` and
  `everything`.
  Individual switches apply after the preset. An invalid name or value
  rejects the entire request. The window and `view.snapshot` share these
  settings.
  The Gameplay rows are `spawn_points`, `flags` and `netgame_items`. Their
  counts cover starting locations, netgame flags and netgame equipment,
  respectively. Each uses its matching placement block switch as well.
  Turn off `markers` to hide annotations, including the flag-type and
  equipment respawn labels. The Art preset keeps equipment models visible.
  The Volumes rows are `trigger_volumes`, `kill_volumes` and
  `bsp_switches`. Every trigger volume is in the first; the other two
  count the distinct volumes a kill trigger or a BSP switch names. The
  Gameplay preset shows all three and the Collision preset shows
  `kill_volumes`. The Navigation rows are `pathfinding`, `bot_nav` and
  `bot_spots`, counting sectors, bot polygons and spots; the bot rows
  count 0 until the bots' data has loaded (`nav.bots`).
- **Trigger volumes.** `volume.list {}` returns `{volumes, kill_triggers,
  bsp_switches, selected, error}`. Each volume has its `index` in the
  scenario's trigger volume block, `name` (the string ID's text, or null),
  `label`, `kinds` (what it is drawn as, most specific first: `kill`,
  `bsp_switch`, `trigger`), the box as `origin` (a corner), `forward`,
  `up` and full `size` along forward, left and up, its `center`,
  `parent` (an object name index, or null) and `world`, and `named_by`:
  `[{kind: "kill", index}]` and `[{kind: "bsp_switch", index, from, to}]`.
  A volume with a parent object has `world: false` and a null `center`:
  its frame is relative to an object only a running game places, so the
  viewport does not draw or pick it. `{"kind": "kill"}` keeps the
  volumes with that kind. `error` says why the volume blocks did not read;
  the placements still open. `volume.select {"index": 1, "focus": true}`
  selects a volume, clears the placements and frames the box;
  `volume.select {}` clears it.
- **Editing volumes.** `volume.set {index, ...}` gives a trigger volume a
  new box as one undo step and returns its `volume` row: `forward`, `up`
  and `size` over its own (forward and up are made unit and square),
  then the corner `origin`, or the `center` (its own centre when neither
  is given, so a resize or turn keeps it). `volume.add {}` adds a 2-unit
  cube at the camera focus (or the box the same fields give) and selects
  it; `{"kill": true}` also adds a kill trigger naming it, in the same
  undo step, and returns `{index, kill_trigger, volume}`. Adding past a
  block's source size is refused (`REFUSED`, with `{kind, count,
  capacity}`) while growth is off, as `scenario.add` is.
  `volume.remove {index}` removes a trigger volume (the last one takes its
  slot, and the kill triggers and BSP switches that named it follow);
  `{"kind": "kill", "index": 0}` removes a kill trigger. A volume a kill
  trigger or BSP switch names is refused with a message naming them and
  `{named_by}`; so is a volume the map came with when it has compiled
  scripts, which name volumes by index. BSP switches are read only. The
  three blocks are the editor's own list (`scenario_edit::volumes::VolumeBlock`),
  not `map::scenario_layout` blocks, so `scenario.*` does not list them.
- **Level collision.** `collision.surfaces` lists every surface of each
  structure BSP's collision BSP (`scenario_edit::collision_view`) with
  its plane (`[nx, ny, nz, d]`, the flip bit applied), raw `flags`
  (`breakable` is bit 3; the other bits are not named), material, ring
  and `class`: `collision_only` when no drawn triangle of the structure
  or its instances lies on its plane (within 0.01 units, the normal
  within about five degrees either way) over it, else `drawn`, with the
  share `covered`. `{"class": "collision_only"}` lists the invisible
  walls; `offset` and `limit` (200) page the list, `matching` counts it,
  and `errors` names a BSP whose collision did not read.
  `collision.at {origin, dir}` returns the first surface a ray meets and
  `{point}` the one a point lies on (within `within`, 0.05 units), each
  whatever layers are shown; `select` selects it. `collision.select
  {index, focus}` selects a surface and frames it. Instanced geometry's
  collision is in the same list, placed in the world, with the
  placement's index as `instance` (`null` for the structure's own).
  `collision.objects` lists each placed object tag's model, `coll` and
  `phmo` (`models`), and every shape a placement places (`shapes`: a
  `coll` BSP or a `phmo` sphere, pill, box or polyhedron, with the `hlmt`
  node it is posed by, its centre and radius in world space); `block` and
  `index` keep one placement's. Read only: the barrier edits change
  collision. The four collision layers (Level collision, Collision only,
  Instance collision, Object collision) start hidden; the Collision and
  Everything presets show them.
- **Navigation.** (`scenario_edit::nav_view`; docs/editor/editor.md,
  "Navigation".) `nav.pathfinding` reads each structure BSP's own AI
  pathfinding data: per BSP whether it has any (`present`), its sector,
  link, 2D node, vertex and ref counts, `orphan_links` and the sectors
  left out because their ring does not close (`broken`); then the
  drawable `sectors` (`index`, `bsp`, `sector`, `flags`, `hint`, `floor`,
  the ring's `links`, `ring`, `center`), filtered by `bsp` and paged
  with `offset` and `limit` (200). A map with none (retail multiplayer
  maps) answers empty with a `note` saying why. `nav.bots` reports the
  bots' nav mesh and superbounce spots (`studio::app::bot_nav`): it
  starts the load if nothing has, and `wait` blocks until it is done
  (a mesh built in memory on a large map takes a while). `state` is
  `no_map`, `idle`, `loading`, `ready` or `failed` (with `error`);
  `origins` names each BSP's cache file, or `built`; `counts` has the
  polygons, those out of the map (`out_of_map`), links, wall triangles
  and spots; `links_by_kind`,
  `spots_by_kind` (bounce, strip, crouch) and `spots_by_state` (proven,
  failed, dead, learned, training, likely) count them; `notes` says what
  was built or scanned instead of read, and which file did not read.
  `polys`, `links` and `spots` list them, each paged by `offset` and
  `limit`; a polygon row's `out_of_map` is true for one only a glitch
  reaches ([bots.md](../game/bots.md) §3.2, "In the map"). `nav.pick {origin, dir}` or `{point, within}` (0.3 units)
  returns what a ray meets or a point names: a bot spot or link first,
  else the nearer bot polygon or pathfinding sector, whatever layers are
  shown unless `shown`; `layer` keeps one layer's. `select` selects it;
  `{kind, index, focus}` selects one directly (`sector`, `poly`, `link`
  or `spot`, indices as the lists give them). The three layers
  (`pathfinding`, `bot_nav`, `bot_spots`) start hidden; the Navigation
  and Everything presets show them.
- **Playtest's static checks.** (`studio::app::playtest_static`;
  [playtest.md](playtest.md#static-checks).) `playtest.static` checks the
  open map as edited, saved or not, with no match. It starts the bots'
  mesh load as `nav.bots` does, and `wait` blocks until it is done; `nav`
  is that load's state. `problems` lists each finding as `{kind, text,
  target}`: `no_mesh` (no mesh to check against, and why), `off_mesh` (a
  starting location, netgame item or netgame flag over no polygon, from
  0.1 units under its floor to 1 over it), `unreachable` (a starting
  location the mesh's links do not lead from to another), `island`
  (polygons no starting location reaches, grouped by their links, 1
  square unit or more), `one_way` (a link from the reached mesh into
  polygons with no way back, one per trap) and `in_sight` (two starting
  locations not of one team with a clear line between their eyes, 0.6
  units up and at most 40 apart, through the level collision). A
  `target` is `{block, index}` for `nav.go`, `{kind: "poly", index}` for
  `nav.pick`, or `null`. `ok` is true when there is no problem. `ready`
  has one row per built-in game type (`rules::builtin::ALL`): its
  `game_type`, `engine` and `ok`, and `missing`: the start gate's
  problems with the map's census for teams 0 and 1, two starting
  locations on the mesh, and for Capture the Flag a `ctf_flag_spawn`
  flag per team, for Assault an `assault_bomb_spawn` flag per team (or a
  neutral one) and an `assault_bomb_return` flag per team, and for both a
  starting location each team can use (its own or one with no team).
  Without a map it is `NO_DOCUMENT`.
- **Playtest's runs.** (`studio::app::playtest`;
  [playtest.md](playtest.md#what-a-run-is).) `playtest.run` plays the
  open map through a sim runner: it checks the map as `playtest.static`
  does, builds the project, makes the bots' mesh for the build, writes
  `match.json` and `batch.json` under `<cache>/playtest/<run>/` and
  starts the batch, then answers at once with `{run, build, matches,
  parallel, cached}`. `preset` is `probe` (the default: seeds 1 to 4 of
  the first ready game type, 2 game minutes), `standard` (seeds 1 to 8
  of every ready game type, 5 minutes) or `soak` (seeds 1 to 32 of every
  ready game type, each played twice with the sides swapped, to the
  score limit). `seeds` (1 to 256) replaces the count, `minutes` (1 to
  120) the length, `bots` (2 to 16, default 4) the seats, and
  `game_types` names the ones to play (any name `h2 sim` takes); one
  that is not ready is refused. A run with the same build, match, seeds,
  bots' version and image as a finished run with no failed match answers
  with that run and `cached: true`, and starts nothing. A newer run
  cancels one still going. With no sim runner connected (none is wired
  to the window yet) it is `REFUSED`; with no game type ready it is
  `FAILED` and says what each lacks; a bad param is `INVALID_PARAMS`.
  `playtest.status` gives `{run, state, preset, build, done, total,
  speedup, stale, coarse, failed}`: `state` is `none` (no run yet),
  `running`, `done`, `failed` (every match failed) or `cancelled`;
  `speedup` is game seconds played per wall second; `stale` is true once
  a newer run's build is another build; `coarse` is true under 8 seeds;
  `failed` lists `{game_type, seed, swapped, why}`. `playtest.result`
  answers over the matches that ended so far, for one `game_type` (the
  first played by default): the run's `build`, `map`, `match_file`,
  `seeds` and `game_types`, `traces` and `seconds` counted, `partial`
  while matches are still to come, and `measures`: each of `heat`,
  `spawns`, `flow`, `sight`, `items` and `stuck` as `{ok: true}` or
  `{ok: false, refused: {measure, what, have, need}}` when its events are
  too few. With `measure` it answers that one in full: `ok` and `value`
  (its bins) or `refused`. `playtest.cancel` stops the running run, or
  the one named, and answers `{cancelled}`; what came back stays. A call
  names a run by its id (`r1`), or the newest.
- **Renders.** `ui.render` draws on the CPU (no GPU needed); `view.snapshot`
  renders the open map offscreen through the viewport's own renderer
  (wgpu, no window) and answers `NO_GPU` on a machine without an adapter.
- **Render stats.** `view.stats` (short form `render-stats`) says what the
  viewport draws for the open map, with no GPU: `lightmap_pages`, each
  BSP's `lighting` (`lightmapped`, `no lightmap`, `empty lightmap group`,
  `lightmap of another BSP`, `lightmap does not decode`), the world's
  `triangles` by blend with `solid_triangles` and `lightmapped_triangles`,
  every material (`shader`, `template`, `blend`, `blend_from_passes`,
  `emissive`, `base`, `details`, and `mask`: the coverage mask a
  see-through layer multiplies in, or null), the `sky` drawn and the `atmosphere`
  read from it (`sun` direction, colour and power; `fog` colour, density,
  start and opaque distance; `fallback_rgb`). `{"scene": false}` while
  the viewport shows the grey world (no art built). `native` is the
  level on the native renderer (`scenario_edit::native_view`): the
  `lighting` mode, each BSP's `bsp`, `draws`, `batches`, `materials`,
  `lightmap_pages`, `untextured` (draws left out for a missing picture),
  `failed_sections` and `lightmap_error`, its `base_look`
  (`translated` when the base look draws as a translated pass; else the
  `fallback` cause, such as a placement with no model rows or a program
  that does not compile, with `world.wgsl` drawing it and `layers_off`
  true: no world layer draws over it), its world `layers` (null
  when not read from the map: `unresolved`, the world materials that
  did not resolve by cause with how many, and `by_layer`, for each of
  layers 1, 3, 5, 7, 8 and 20 the `layer`, the materials whose passes
  `compiled`, those `drawn` and their `parts`, every other material's
  cause with how many, `refused`, as `cause: detail`: `program` for the
  load's refusals, such as a view constant the camera does not give or
  a layer-3 stage bitmap that is not 2D, then the plan's causes,
  `animated`, `fog`, and for layer 1 `layer 1: stencil mode not checked
  against the game`; and `parts_refused`, layer 3's parts of a drawn
  material that its pass does not take, by cause with how many, which
  draw the base look; they draw in the `game` mode only), the `errors` of BSPs that
  did not load, and the placed `objects`: how many object tags have a
  native model (`models`), how many placements draw one (`placed`), and
  each tag left to the scene's model with why (`errors`, `tag` and
  `error`). What each field means is in [editor.md](editor.md),
  "What the viewport shows".
- **Lighting.** `view.lighting {}` returns `{mode, modes}`;
  `view.lighting {"mode":"flat"}` sets it for the window and
  `view.snapshot`: `game` (base map times lightmap), `flat` (no
  lightmap), `lightmap` (the lightmap on white), `collision` (flat at a
  third). Another mode is `INVALID_PARAMS`. To check a
  map: `h2 studio open mainmenu`, `h2 studio render-stats`, then
  `h2 studio snapshot` and look at the PNG.

### The preview switch

`preview.target {doc?, target?}` reads or sets where a document's preview
shows (docs/editor/studio-redesign.md, "Preview and the paired game";
`app/preview_switch.rs`, `api/preview.rs`). `doc` is a `shell.tabs` id
(the front tab without it). `target` is `studio` (the native view or the
kind's own renderer, the default), `game` (a game Studio started) or
`attached` (a game someone else started). A game target needs a paired
game of that kind, else it is `REFUSED` with the reason. The answer is
`{doc, target, targets: [{id, label, available, why?}], game, why?}`.
`game` is the `pair.preview` params the document's kind takes, or null
with `why`:

| Document | `game` |
|---|---|
| a `weap` tag | `{what: "weapon", target: <tag name>}` |
| a `vehi`, `bipd`, `scen` or `eqip` tag | `{what: "object", target: "<group>:<tag name>"}` |
| a `wgit` or `wgtz` tag, or the screen in the UI Screens tool | `{what: "ui", target: <screen name>}` |
| a `nhdt` tag, or the HUD tool's document | `{what: "hud"}` |
| a `snd!` tag | `{what: "sound", target: <tag name>}` |
| any other tag (`lsnd` and `ugh!` too), the map, a variant or a playlist file | null, with why |

The **Preview** menu holds the three targets and **Preview in game**
(Cmd/Ctrl+Enter). The click is `Action::PreviewInGame`: it runs
`pair.preview` with the front document's `game` params, so the job is the
one `pair.result` reads, and a refusal (a paused game, no pairing, a kind
with no preview) goes to the log with the message the API gives. The
target menu is `Action::SetPreviewTarget`, the command `preview.target`
runs. Through the API, call `pair.preview` with `game`.

```sh
h2 studio preview.target '{"target":"attached"}'
```

The tests are `api/preview/tests.rs`' `switch` module, on the invented
game.

### Previewing content in the paired game

`pair.preview` puts content into the paired game and reads it back, so a
weapon, an object, a menu screen or the HUD is checked in the running game
rather than found in play (`crates/apps/studio/src/api/preview.rs`). It
answers at once with `{id, state}`. `pair.result {id?}` reads the job
(the latest one without `id`). One job runs at a time and the last eight
are kept.

| `what` | `target` | The game calls, in order |
|---|---|---|
| `weapon` | a `weap` tag's name or datum (required) | `sim.give` to `player` (default 0; `keep: true` adds it as a spare), `session.wait`, `world.object` for the held weapon, `pogo.state`, `render.scene`, the capture |
| `object` | an object tag's name or datum (required; `vehi:<name>` names the group) | `world.object` for the player, `sim.spawn` at `pos`, else `distance` (default 3) world units in front of the player and facing them, `session.wait`, `world.query` near it, `render.scene`, the capture |
| `ui` | the screen expected (optional) | `ui.audit`, `ui.stack` and `ui.screen` where listed, the capture |
| `hud` | a `<name>.state` method to read with it (optional: `superbounce`, `pogo`) | `cheat.list`, `cheat.set hide_hud false` when it was on, `session.wait`, the state, `render.scene`, the capture, then `hide_hud` back on |
| `sound` | a `snd!` tag's name or datum (required) | `audio.play` to `player`, `audio.voices` every 100 ms for `seconds` (0.1 to 30, default 2), `audio.loops` |

A `sound` preview has no picture. Beside the game's readings (`applied`,
the answer of `audio.play`, and `audio`, as `pair.audio` gives it) the
result has `editor`: Studio's own `sound.measure` of the tag (levels and
verdict against the open map's sounds), or `{why}` when the tag is not in
the map the Tags tool shows. A game that does not list `audio.play` fails
the job. The sound starts at the player's unit through the game's own sound
start ([`audio.play`](../runtime/agent-api.md#audioplay)).

The other params are `ticks` to let run before the reads (0 to 600;
default 30, and 0 for `ui`), `view` for the picture, `out` to copy the
PNG to, and `edits` (`send`, the default, or `skip`). `view` is `render`
(the default: `render.capture`, falling back to `host.screenshot`) or
`screen` (`host.screenshot`, the window's frame as the player sees it).

Pending edits reach the game first, through the pairing's own path. With
edits waiting and the editor in View, the job queues `mirror.apply
send`'s request. It is `sending` until the window has sent every waiting
row (up to 15 s, else `failed`). A conflict, a refused write or an edit
that needs a reload stays in Pending. The result lists those rows under
`edits.left`, with a warning. A preview of a paused game is `REFUSED`,
except for `ui`.

The game calls run on a thread and a socket connection of their own, so
the window keeps drawing. A method the game does not list is skipped.
A missing method the preview needs (`sim.give`, `sim.spawn`, `ui.audit`
or `ui.stack`) fails the job, and so does a game refusal, with the game's
message in `why`.

`pair.result` gives `state` (`sending`, `running`, `done`, `failed`),
`why`, `elapsed_ms`, `steps` (the game calls made so far) and `edits
{pending, sent, left}`. Once the job is done it also gives:

- `applied`: the game's answer to `sim.give` or `sim.spawn`.
- `reads.weapon`: `object`, `tag`, `name`, `held` (the player's held
  weapon's tag is the given one after the wait) and `held_tag`.
- `reads.pogo`, where `pogo.state` is listed: `stick` (the weapon is one
  of the kit's three), `active`, the `rider` holding it and the chase
  `camera` (`on`, `drawn`, `room`, `share`).
- `reads.camera`, where `render.scene` is listed: the last sealed scene's
  `views` (`key`, `instances`, `cut`) and `first_person {views, drawn}`.
  `drawn` is true when a `first_person:` view drew an instance.
- `reads.object`: the spawned object as `world.query` lists it, or null.
- `reads.ui`: `shown` (the names and ids of `ui.audit`'s active surface
  and menu, and `ui.screen`'s class and screen), `matches`, and the raw
  `audit`, `stack` and `screen`.
- `reads.hud {hidden_before, shown}` and `reads.state`.
- `capture`: `path`, `method`, `tick`, `present`, `width`, `height`,
  `mean_luma` (Rec. 709, 0 to 1), `dark_share` (pixels below 4% luma) and
  `black` (a `dark_share` of 98% or more).
- `warnings`: a black frame, a weapon not held, no first-person view
  drawn (except while the Pogo chase camera draws), no rider for a Pogo
  stick, a chase camera asked for and not drawn, a spawned object not
  found, another screen shown, `ui.audit` issues, edits left in Pending.

`pair.preview` does not open a menu screen. The game API has no call that
opens a screen by name: the guest opens screens from its own UI code
(`game::ui_kit::dialog_tag::open_over`), and a host call into it would
need a guest call queue like `sim.give`'s. Drive the menu to the screen
with the game's `ui.press`, then preview `ui`.

`pair.audio {seconds?, every_ms?, call?}` makes `call` (`{method,
params?}`, a game method that makes the sound, such as `pogo.test`),
samples the game's `audio.voices` every `every_ms` (20 to 1000, default
100) for `seconds` (0.1 to 30, default 3), then reads `audio.loops`.
`pair.result` gives `audio`:

- `samples`, `voices` (distinct voice ids) and `most_at_once`.
- `peak_gain` and `peak_db`: the largest `applied_gain`, the level a
  voice's direct path leaves the mixer with, before bin volumes and pan.
- `loudest`: the eight loudest voices, with `id`, `kind`, `peak_gain`,
  `peak_db`, `volume_mb`, `distance_units` and `samples`.
- `loops`: the looping sounds playing, with `tag_name`, `tag_datum` and
  `segment`.

These are mixer gains, not the samples' own level. The game has no output
meter (`H2_AUDIO_STATS` reports playout health, not level), so a sample
that is too loud at full gain shows here only as a voice at 0 dB. A
sound's own level is read in the editor from its tag.

```sh
h2 studio pair.preview '{"what":"weapon","target":"objects\\weapons\\multiplayer\\pogo\\unsc\\pogo_unsc","out":"/tmp/pogo.png"}'
h2 studio pair.result '{"id":1}'
h2 studio pair.audio '{"seconds":3,"call":{"method":"pogo.test","params":{"trick":"backflip"}}}'
h2 studio pair.result
```

### Generating images

`image.generate` makes a picture for an image of a bitmap from a prompt
and reference pictures, and the `image.*` calls after it put the
picture into the image through the Textures tool's own edits
(`crates/apps/studio/src/api/image_gen.rs`, `src/app/image_gen.rs`,
`src/imagegen.rs`). The window's Generate button does the same
([editor-guide.md](editor-guide.md#generating-an-image)). A generation
takes a minute or more, so it runs as a job: `image.generate` answers at
once with `{id, state: "running", generator, slot}` (`slot` is
`{bitmap, image}` or null) and `image.result {id?}`
reads the job (the newest without `id`). One job runs at a time and the
last sixteen are kept.

`image.generate`'s params:

- `prompt`: what the picture is.
- `references`: a list, empty for none. Each is a PNG path, or
  `{bitmap, image?}`, an image of the open map's bitmap, exported as a
  PNG into the job's directory for the generator. The references are
  the user's choice: an agent passes the ones the user named (for a menu
  icon, the other icons of the same bitmap), and never picks disc images
  of its own accord.
- `bitmap` and `image` (default 0): the slot, named as `bitmap.*` names
  a bitmap. The picture is fitted to the slot. `image: "new"` (which
  needs `bitmap`) first adds an image to the bitmap (`bitmap.image.add`),
  shaped like image `like` (default 0) or made at `size`, and makes it
  the slot; the answer's `slot` names it. Without a slot, `size
  [w, h]` fits it to that size as 32-bit RGBA; with neither, it is kept
  as it came. A slot and a size together are `INVALID_PARAMS` (a `new`
  slot is the one case that takes both).
- `alpha`: `auto` (the default: the picture's own alpha, except that an
  opaque picture going into an `A8` or `AY8` image takes its
  brightness), `image`, `luma` or `opaque`.
- `out`: a path to copy the fitted PNG to.
- `from`: the job this one regenerates, with the same or a tweaked
  prompt. The result and the provenance name it.

The job gives the source the prompt with the slot's needs added
(`imagegen::compose_prompt`): its size, a transparent background when the
format keeps alpha, and how the references are meant (match their style,
palette, line weight and framing, not their subject). It keeps the
source's picture as `generated.png`, then fits it (`imagegen::fit`). The
picture is resampled with the Textures tool's filter: stretched when the
aspects are within a tenth, else centred on transparent pixels. Then
each pixel is rounded to what the slot's format stores and its decoder
gives back. The uncompressed formats round as `assets::encode` stores
them and `gfx::convert` reads them. The DXT formats encode each 4 by 4
block with `assets::encode::encode_block` and decode it with `gfx::dxt`.
The fitted picture, `fitted.png`, is what the game shows. A slot of a
format that is not a picture (`P8`, bump, float) is `REFUSED`, and so is
a cube map or a volume. A slot with a `bitmap.reshape` asked for is
fitted to the reshape's format.

`image.result` gives `state` (`running`, `done` or `failed`),
`generator`, `elapsed_ms`, `prompt`, `references` (each with its `path`
and `origin`, `file` or `map` with its `bitmap` and `image`), `slot`,
`size`, `dir` (the job's directory, in the cache's `studio/imagegen/`),
`from`, and `applied`, `accepted` (the files written, or null) and
`discarded`. A done job adds `raw {path, width, height}`, `fitted {path,
width, height, format, alpha}`, `revised_prompt` and `out`. A failed one
adds `why`, the source's message, and `error`:

| `error` | Meaning |
|---|---|
| `missing` | the generator is not installed (`codex` not on `PATH`); `why` says how to install it and sign in |
| `unauthenticated` | Codex is not signed in; `why` says to run `codex login` and sign in with ChatGPT |
| `quota` | the ChatGPT account's allowance is used up; `why` has Codex's message, with when it renews |
| `no_image` | the run finished without a picture |
| `failed` | anything else, with the generator's message |

Then:

- `image.apply {id}` writes the fitted picture into its slot as one
  `BitmapAction::Import`, one undo step, after keeping the slot's pixels
  as `original.png` in the job's directory. The picture then shows
  wherever the image is used: the Textures canvas, `bitmap.pixels`, the
  UI Screens tool's pictures and `ui.render` (the disc's picture of the
  bitmap, `UiScreens::show_image`), and a paired game through the
  Textures tool's live sync ([realtime-edits.md](realtime-edits.md)).
  `pair.preview {what: "ui"}` captures the menu with it. A job without a
  slot, or one that failed, is `REFUSED`.
- `image.accept {id, save?}` keeps it, applying it first if it is not in
  its slot. The picture stays as the bitmap's edit and saves with the
  map (`doc.save {doc: "bitmaps"}`), like any texture edit. It writes `provenance.json` in
  the job's directory and logs a line naming the bitmap, the generator,
  the references and the prompt. With `save` (a directory) it copies
  `<bitmap>-<image>.png` (the generator's picture),
  `<bitmap>-<image>.fitted.png` and `<bitmap>-<image>.provenance.json`
  there. A job made to a `size` has no slot: accepting it keeps the art
  and its provenance (`image-<id>.png`, `image-<id>.fitted.png`,
  `image-<id>.provenance.json` under `save`) for an image made later,
  and changes no bitmap.
- `image.discard {id}` takes it out: the import's undo step while it is
  still the newest step of that bitmap, else the original pixels
  imported back.

The provenance names the prompt, the prompt sent, the generator's
rewrite of it, every reference with its origin, the generator (`codex-cli
0.170.0 (ChatGPT image generation)`), the slot, the alpha, the job it
regenerates, and whether it was accepted. Generated art is original
AI-made art and may go in the repository with its provenance; every PNG
there is stored in LFS.

**The source.** `imagegen::ImageSource` makes the picture.
`H2_IMAGEGEN` picks it: `codex` (the default) or `fake`. An OpenAI API key
source, with the key in the Keychain under the service `openai`, is
another implementation of the trait; it is not built.

- **Codex** (`imagegen::codex::CodexCli`) uses ChatGPT's image generation
  on the user's ChatGPT sign-in to the Codex CLI, with no API key. It
  first runs `codex --version` and `codex login status`. Then it runs
  `codex exec --image <ref>... --json --skip-git-repo-check --sandbox
  workspace-write --cd <job dir> --output-last-message <job
  dir>/codex-last-message.txt` once, with the prompt on stdin (`--image`
  takes many values, so a prompt argument after it would be read as an
  image). The prompt asks Codex to make one image with its image
  generation tool and copy it to `out.png` in the job's directory. The
  picture is `out.png`, else a `saved_path` in the run's events, else
  the newest PNG in `$CODEX_HOME/generated_images/<thread id>/` (Codex
  saves each picture there, the thread id from the `thread.started`
  event), else a PNG path in the last message. The events are kept as
  `codex-events.jsonl` in the job's directory. A run may take up to ten
  minutes. Studio never reads Codex's credentials.
- **The fake** (`imagegen::fake::FakeSource`) draws a disc on a
  transparent ground, its colour from the prompt. The tests use it, and
  `H2_IMAGEGEN=fake` tries the flow with no generator.

Into an image the map has (here `mp_games2`'s last icon), then into no
slot, at the icons' size, for an image made later, then into an image
the bitmap did not have, added first:

```sh
h2 studio image.generate '{"prompt":"a radar dish","references":[{"bitmap":"mp_games2","image":0},{"bitmap":"mp_games2","image":3}],"bitmap":"mp_games2","image":10}'
h2 studio image.result '{"id":1}'
h2 studio image.apply '{"id":1}'
h2 studio pair.preview '{"what":"ui"}'
h2 studio image.accept '{"id":1,"save":"art/ui-icons"}'
h2 studio doc.save '{"doc":"bitmaps"}'
h2 studio image.generate '{"prompt":"a radar dish","references":[{"bitmap":"mp_games2","image":0}],"size":[40,40]}'
h2 studio image.accept '{"id":2,"save":"art/ui-icons"}'
h2 studio image.generate '{"prompt":"a radar dish","references":[{"bitmap":"mp_games2","image":0},{"bitmap":"mp_games2","image":3}],"bitmap":"mp_games2","image":"new"}'
h2 studio image.apply '{"id":3}'
h2 studio doc.save '{"doc":"bitmaps"}'
```

### The tag workspace

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
reads every stored file back, and `root` sets the tree's root — refused
inside a Git checkout, for the tree is game data and never sits in a
repository. It answers at once with the maps it will read; the progress
is in `workspace.status` and the window's status line, and work
continues while the tree fills. An unchanged disc is a no-op, and a
later start of the editor runs the plan against the recorded disc in the
background. `workspace.prune {dry_run}` removes raw resources no tag
file or manifest names, generated layers no project asks for and build
caches unused for a week — with `dry_run`, what would go; the outputs of
deleted projects are the projects' and are not pruned. The tree itself
is never pruned in part: it is deleted whole and extracted again.
`workspace.import` (planned) stores a map as an import layer (W9).

The window's first start — no tree, no source open other than a disc
image — is one step: choose the disc (`H2_DISC` and `disc/halo2.iso`
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

### The browser

`asset.*` is the browser's data (docs/editor/studio-redesign.md, "The
browser"; `app/assets.rs`). An asset is named by an id: `tag:<group>:<path>`
(a tag of the tree, or one a package adds), `package:<id>` or
`item:<package>:<n>` (the n-th item of a package's manifest).

`asset.list {root?, package?, kind?, map?, query?, limit?}` answers
`{rows, total, limit, truncated, maps, kinds, problems}`. With `root`
`tags` (the default) a row is one tag path of the tags tree: a tag every
map holds alike is one row with `variants: []`; where maps hold
different versions the row's `variants` lists each (`variant`, null for
the main file, and its `maps`). A tag a package adds is in the same list
(`new: true`, `origin: "package"`, badge `new in <package>`); a tag an
edit line of a package on touches has badge `edited by <package>` and
the packages in `packages`. `kind` is one of `kinds` (Maps, Objects,
Models, Animations, Bitmaps, Shaders, Effects, Sounds, Screens, HUDs,
Strings, Other) or a tag group; `map` keeps what that map's manifest
holds, with the tags a package adds for it; every word of `query` must
match the path, group, kind or a package id; `package` keeps what that
package adds or edits. `root` `packages` lists the set (baked, then
`<data>/packages/`) with on or off and why; with `package` it lists that
package's items. `root` `live` answers `offline: true`: the server's
offer is not held. A missing tree is refused with the way to make it
(`workspace.extract`).

`asset.thumb {asset, out?}` writes a PNG and answers its path: for a
package's bitmap item with a PNG source. A tag's own picture (a bitmap,
a model through `model.render`, a waveform, a screen) is not drawn yet
and answers `not-yet`.

`asset.copy {asset, package, to?}` is Copy into a package: it writes
`tags/<path>.<group>.yaml` holding only `like: '<group>:<path>'`, adds a
`tag` item to the package's `package.toml` and rewrites its
`package.lock`. The package is one under `<data>/packages/` (a baked one
is refused); the new path is `to`, or the source's with `_copy`; a path
the tree or the package already has is refused.

```sh
h2 studio asset.list '{"kind":"Objects","query":"rifle"}'
h2 studio asset.list '{"root":"packages","package":"pogo"}'
h2 studio asset.copy '{"asset":"tag:weap:objects\\weapons\\rifle","package":"my-pack"}'
```

The tests (`api/tests/assets_tests.rs`) run over an invented tree of two
maps and one package.

### Packages

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
| `package.check {id?}` | `{ok, count, findings: [{kind, message, item?, file?, line?}]}`: `lock` (missing or stale), `files` (a cache map, a `.h2tag` bundle, disc files, model weights or a dataset folder), `item` (no builder, a missing or oversize source, a source its builder refuses), `set` (a requirement missing or off), `conflict` (a tag path another package adds) |
| `package.build {id?, map?, out?}` | for each map of the open source the package has an item for (or `map`), `{map, scope, touched, took_ms, tags, added, changed, overrides, bytes, problems}`; with `out`, each built map is written to `<out>/<map>.map` |
| `package.play {id, map}` | the build for `map`, then `game.load` on the paired game |

Each write puts the new manifest through the set before it keeps it: a
manifest that does not read, a source that is not in the folder, a
`requires` cycle or a set `content::package::Set` refuses leaves the old
manifest in place and answers `REFUSED` with the reason. `package.enable`
and `package.order` refuse by name: `core` and `ui` cannot be switched off
(`SetError::AlwaysOn`), and an order that puts a package before one it
`requires` or follows names both. A `set.toml` written by hand that turns
`core` off is named in `package.list`'s `problems` and stops every write to
the set file. The edit calls that default `id` to the edit target answer
`NO_TARGET` when none is set. `package.build` needs a source (`NO_DOCUMENT`)
and refuses a package that is off. `package.play` needs a pairing and
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

### Problems

`problems.list {source?}` is the Problems panel's list
(docs/editor/studio-redesign.md, "The shell"; `app/problems.rs`,
`api/problems.rs`), read again on every call. A row is
`{source, area, code, severity, message, target, path?, asset?, field?,
tool?}`, worst first. The sources:

| `source` | Rows from |
|---|---|
| `check` | `check.run`'s `models` and `sounds` checks |
| `ui` | `ui.check` |
| `hud` | `hud.check` and the HUD kit's check |
| `package` | the manifest checks `h2 package check` runs over the packages under `<data>/packages/`: an item with no builder, a missing or oversize source, a source its builder refuses, two items that add one tag path |
| `build` | what the game's package set refuses to load, as the boot log says it: an unreadable package, a refused id, a `requires` that is missing, a set that cannot be ordered |

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

```sh
h2 studio problems.list '{"source":"package"}'
```

The tests are `app/problems_tests.rs` (findings of two invented sources
list together, a row opens its document and field), `api/tests/problems_tests.rs`
and `headless::tests::the_problems_panel_lists_rows_and_a_double_click_opens_them`.

### Sounds into a package

`import.sound {file, package, tag, class?}` (`api/import.rs`,
`app/sound_import.rs`) adds a WAV to a local package as a `sound` item that
writes the `snd!` `tag`. The WAV (16-bit PCM, mono or stereo, any rate) is
copied unchanged to `sounds/<last segment of tag>.wav`, the item is added to
`package.toml` and `package.lock` is rewritten. `class` is a sound class by
name (`weapon_fire`, `ui`) or number; the default is `ui` (51), and any
other is written to `sounds/<name>.sound.yaml`, a `data` item beside the
WAV. The answer is what the build will write: `rate` (22,050, 32,000 or
44,100 Hz, the nearest not below the clip's), `frames` at that rate,
`duration_ms` and the `bytes` of Xbox ADPCM, with `from` the clip's own
rate, channels and frames. A package that is baked, a tag the package adds
already or a file that is not a 16-bit PCM WAV is refused; a class that is
not one is `invalid params`.

```sh
h2 studio import.sound '{"file":"/tmp/boing.wav","package":"my-pack","tag":"sounds\\boing"}'
```

The build is `workspace::build` (`h2 package build`); the tests
(`api/tests/import_tests.rs`, `workspace/src/import_tests.rs`) run over
invented tones and an invented map.

### Errors

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

## Content checks

An agent building content (the Pogo Stick's sounds, a screen of ours, a
HUD kit element) measures and checks it here before handing it back:
`sound.measure`, `ui.check`, `hud.check`, and `check.run` for all of
them at once.

**`sound.measure`** (`api/sound_measure.rs`) takes one of `tag` (a
`snd!` of the map the Tags tool shows, decoded by the audition as
`sound.read` decodes it), `clip` (one of our WAV clips as the game
embeds them: `pogo/compress`, `pogo/spring`, `pogo/settle` from
`boot::pogo_sound::CLIPS`, `announcer/<medal>` from
`boot::medal_announcer::CLIPS`) or `path` (a WAV file, such as one
`scripts/pogo-sounds.py` just wrote). WAVs are read by the game's
`parse_wav`: 16-bit PCM, one or two channels; a Git LFS pointer is
`FAILED` and names `git lfs pull`. A tag is measured in every
permutation of `range` (default 0), or the one `permutation` names;
the loudest is the one compared. It answers:

- `levels`: `peak_dbfs` (the largest sample, full scale 32768),
  `rms_dbfs` (every sample of every channel), `lufs` (integrated
  loudness), `max_momentary_lufs` (the loudest 400 ms block),
  `duration_ms`, `clipped` (samples at 32767 or -32768), `frames`,
  `rate`, `channels`. Levels are to one decimal place; silence is -120.
- `reference`: the open map's `snd!` tags of the same class (the
  header's `+2` byte, unnamed: [sound.md](../formats/sound.md)), or of
  every class for a clip or file, or of `class`. Up to `limit` (default
  32, at most 512) of them, spread evenly over the map's tag order, are
  decoded (range 0, permutation 0) and measured. It gives the class,
  how many there are (`considered`), `decoded`, `skipped` (a codec
  stream without the game's executable), the `lufs` and `peak_dbfs`
  spreads (`p10`, `p50`, `p90`, `max`, nearest rank) and the three
  loudest. With no map open it gives `why`.
- `verdict`: `loud` when the loudness is over the reference's p90 by
  more than 1 dB, `quiet` when under its p10 by more than 6 dB, else
  `ok`; `no_reference` with nothing decoded. `over_p90_db` and
  `peak_over_p90_db` are the margins, and `text` says it ("louder than
  the p90 of the map's sounds by 7.2 dB").

The loudness is ITU-R BS.1770-4's (`crates/platform/audio/src/meter.rs`,
shared with the game's `audio.meter`): each
channel K-weighted (the high shelf and high pass, their coefficients
derived for the clip's rate from the 48 kHz design, as libebur128 does),
mean square over 400 ms blocks stepped 100 ms, blocks under -70 LUFS
dropped, then blocks more than 10 LU under the remaining mean,
`-0.691 + 10 log10` of the mean of the rest. Each channel weighs 1, so
a mono 997 Hz sine reads its RMS level and a stereo one 3 dB more. A
clip shorter than 400 ms is one block. Clips and tags are compared
sample for sample: the game's gain, distance and mixing are not
applied.

**`ui.check`** (`api/ui_check.rs`) checks our screens (`screen`, and
`state`, narrow it) in every state with the unsaved edits
(`UiScreens::check_state`: the edited docs in scope, the disc's art
and fonts when a disc is open, else the synthetic source). The checks
are the kit's (`boot::ui_kit::findings::of_state`;
[ui-builder.md](../game/ui-builder.md) "The checks"): text overflowing
its rect (`fit`), overlapping texts, widgets outside the title-safe
area (`unsafe`), contrast under 2.5, missing glyphs and unregistered
string ids, art drawn past its rect or off screen, and the design
system's rules. Each finding has `code` `ui.<check>.<kind>`
(`ui.layout.unsafe`, `ui.layout.fit`, `ui.layout.text_overlap`,
`ui.content.missing_glyph`), `severity` (`error`; `warning` for the
design system's `consistency`; `note` for a layout note), `message`,
`target` (`screen/state`), `path` (the screen's file under
`content/ui/screens/`), and the `screen`, `state`, `check`, `kind` and
widget `ids`. A state that does not draw is `ui.draw`. `checked` lists
each state with its count and the checks the source could not run
(`skipped`). `aspect` is `4:3`, `16:9` or `both` (the default); the
host fits a menu uniformly into a centred 4:3 area at any window
aspect ([high-resolution.md](../graphics/high-resolution.md), "HUD and
UI"), so the layout checked is the 640 by 480 one and `aspects` says
which fit each aspect asked is.

**`hud.check`** gives each finding's `code` beside its text:
`off_screen`, `outside_frame`, `art_overlap`, `text_overlap` or
`text_spills` (`hud_edit::checks`).

**`check.run {what?, target?}`** (`api/checks.rs`) runs every check of
`CHECKS` (or one area's: `models`, `sounds`, `ui`, `hud`, `markers`) and answers
`{findings, counts, ran, skipped}`. A finding is `{area, code,
severity, message, target, path?}`, worst first; `counts` has `error`,
`warning`, `note`, `total` and `by_area`; `ran` names each check with
its count; `skipped` each check that could not run and why, and each
area with no check registered. `target` narrows every check to one
thing; a check it does not fit is skipped. The checks:

| Area | Check | What it checks |
|---|---|---|
| `models` | `model.check` | every biped, vehicle and weapon of the scenario tool's map (or the source's only map), or the tag `target` names, read without the Pogo kit: `model.<code>` for each `model.check` finding (`model.no_first_person`, ...), the tag as `target` and the `tag.set` path in the message; `model.tag_memory` once (`api/models.rs`, `check`) |
| `sounds` | `sound.measure` | each of our clips (or the clip id, id prefix such as `pogo`, or `snd!` tag `target` names) against the open map's sounds: `sound.loud` (a warning, an error past 6 dB over the p90), `sound.clipping`, `sound.unreadable`, `sound.no_reference` (a note) |
| `ui` | `ui.check` | every state of our screens, or `target` (`screen` or `screen/state`) |
| `hud` | `hud.check` | the HUD the HUD tool shows, at its state (`hud.<code>`); skipped with no map open there |
| `hud` | `hud.kit` | every kit HUD (or `target`) in every state and screen mode over the kit's synthetic map, as `h2 hud sheet --synthetic` checks them (`hud_kit::check::all`): `hud.kit.<check>` errors, and a note for each check the synthetic map cannot run |
| `markers` | `markers.check` | the open map's markers (`api/markers_check.rs`): `markers.area.invalid` (error), `markers.hill.corners_and_area` (error), `markers.area.no_floor` and `markers.area.overlap` (warnings, the second naming the pair); no map is no findings |

A check is a row of `CHECKS`: `ContentCheck { area, name, run }`,
`run` being `fn(&mut App, Option<&str>) -> Result<Vec<Finding>,
String>`.

## Announcer

`voice.say`, `voice.list` and `voice.commit` (`api/voice.rs`, over
`app/announcer.rs`) make announcer lines from text with a Fish Audio voice.
The design, the key and the steps are in [announcer.md](announcer.md).

**`voice.say {text, voice?, model?, speed?, out?, clip?, play?,
target_lufs?, fresh?}`** says `text` (at most 500 characters) in `voice`
(a Fish Audio model id; default `86126c4c4dad4911979556b4569802cd`), trims
the silence, measures the line against the open map's retail announcer
lines and brings it to their median loudness (`target_lufs` instead), saves
44.1 kHz mono 16-bit PCM under `<data root>/announcer`, and plays it
(`play` defaults to true). The same text, voice, `model` and `speed` reuse
the earlier file: `cached` is true and the service is not called, so no key
is needed. `fresh` asks again. It answers `file`, `raw` (the voice's own
file in the cache), `levels` and `raw_levels` (as `sound.measure`),
`trimmed_ms`, `gain_db`, `limited` (the -1 dBFS peak ceiling held the gain
back), `target` (`lufs`, `from`: `given`, `retail`, `our clips` or
`fallback`, and `why` when not retail), `verdict` (as `sound.measure`),
`playback`, and with `clip` a `ready` object: the checkout path
`art/announcer/<clip>.wav`, the `CLIPS` entry to add, and whether the clip
exists. With no key in the Keychain the error is `REFUSED`, its message
says to add the key with the exact command, and its `data` is
`{step: "add_key", service, command}`. A bad param is `INVALID_PARAMS`
before the service is asked; the service's errors and a silent line are
`FAILED`.

**`voice.list`** answers the voice source, the default voice, the voices
kept lines used, whether the key item exists (`key.state`: `present`,
`missing` with the command, or `unreadable`; the key is not read), the
cache's directory and size, and the lines most recently said. It asks
nothing of the service.

**`voice.commit {clip, from?, repo?, replace?}`** copies a saved line
(`from`, else the last one made) to `art/announcer/<clip>.wav` of the
checkout (`repo`, else the one above the editor's working directory). It is
the only way a line reaches the checkout. It does not run `git` or edit
`boot::medal_announcer::CLIPS`; `next` says what is left. An existing clip
needs `replace`.

## Voices

`voice.characters`, `voice.dataset.build`, `voice.dataset.status`,
`voice.clone.train` and `voice.clone.say` (`api/voice_clone.rs`, over
`app/voices.rs` and `app/announcer.rs`) clone a game character's voice from
the user's own disc and say new lines with it, all on this machine. The
design, the dataset, the sidecar and the models are in
[voice-cloning.md](voice-cloning.md).

**`voice.characters {map?}`** lists the characters Studio knows: `id`,
`name`, the tag-path `rules`, how many dialogue sounds of the source's maps
(or `map`) they pick (`tags`), their `dataset` summary (or null) and
whether they are `trained`. `speakers` lists the groups the maps' dialogue
tags fall into (`kind: code` for the letters at the end of a tag name,
`folder` for the first folders under `sound\dialog\`) with a count and a
sample tag, to check the rules against. Nothing is decoded.

**`voice.dataset.build {character, paths?, maps?, asr?, min_ms?, max_ms?,
keep_radio?, limit?, max_permutations?}`** builds the character's dataset
under `<data root>/voices/<character>`: 24 kHz mono clips and
`manifest.jsonl`. `paths` (pieces of tag paths) replaces the character's
rules and allows a new id. It answers `kept`, `tags`, `decoded`, `dropped`
by reason (`too_short`, `too_long`, `silent`, `clipped`, `radio`,
`decode`) with a few `dropped_examples`, `maps_read`, `maps_failed`, the
`dataset` summary (`lines`, `usable`, `minutes`, `from_subtitle`,
`from_asr`, `pending_asr`, `no_text`, `lufs`, `peak_dbfs`), and `asr_job`:
the background transcription started for clips without a subtitle (`asr`
defaults to true). An unknown character without `paths` is
`INVALID_PARAMS`; no map read, or a job still running, is `REFUSED`.

**`voice.dataset.status {character?, job?}`** answers the `sidecar`
(`backend`, `state`: `installed`, `installing` or `missing`, `dir`,
`trains`), the latest `job` (or `job`'s) and the last few `jobs` (`id`,
`kind`: `install`, `transcribe` or `train`, `state`: `running`, `done` or
`failed`, `stage`, `done`, `total`, `text`, `log`, `result` or `why`), and
each built character's `dataset`, last `build` and `model` (`trained`,
`checkpoint`).

**`voice.clone.train {character, steps?, min_minutes?, backend?}`** starts
the fine-tune on the character's lines with text as a background job
(installing the model first when needed) and answers `{job, state:
"running", lines, minutes}`. It is `REFUSED` under `min_minutes` of speech
(default 10), without a dataset, or while a job runs, and
`INVALID_PARAMS` for a backend that does not train.

**`voice.clone.say {character, text, reference?, speed?, out?, clip?,
play?, target_lufs?, fresh?, backend?}`** says `text` with the clone,
copying the delivery of `reference` (a dataset clip id, or a source tag
with an optional `#<permutation>`; default a subtitled line near 7 s), and
answers as `voice.say` does plus `engine: "clone"`, `character`,
`backend`, `model` (the backend, and the fine-tune's digest when there is
one) and the `reference` used (null for a cached line). `target.from` is
`character`: the median loudness of the character's own lines. Before the
model is installed it starts the install and is `REFUSED` with
`data.step: "install"`; without a dataset, or with no line that has text,
`data.step` is `dataset`. A model failure is `FAILED` with the sidecar's
message. To hear a committed clip in the game, `voice.commit` it and use
`pair.audio` as for any announcer clip.

## Extension points

`crates/apps/studio/src/api/seams.rs`. A tool implements `api::Provider`:

```rust
pub trait Provider {
    fn methods(&self) -> Vec<MethodDef>;
    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply>;
}
```

and adds one line to `seams::register`. A provider answers ahead of the
built-in methods and the planned stubs (`None` falls through to them); its
`methods()` replace the table's entries of the same name, so a planned
method lists as `ready` once its definition has `planned: None` (until
then it answers `NOT_YET` whatever its params). Params
reach it already checked against its shape. Its edits go through the
tool's own actions and `App::perform`, as the built-in namespaces do, so
the window and the API stay one path.

- **Tags (studio-tags, landed).** `api/tag_fields.rs` is the first
  provider: `tag.read` through `tag_api::read_tag`, `tag.set` through
  `tag_api::set_field`, `tag.block.*` through `tag_api::block`, applied as
  `Action::Tag` with `App::perform`; it returns `None` for the scenario
  tag's edits and for `tag.read {"raw": true}`. `api/tag_io.rs`
  (`TagIo`, after it) answers `tag.list` (replacing the built-in one),
  `tag.browse`, `tag.resources`, `tag.export` and `tag.import` through
  `Action::BrowseTags`, `Action::TagExport` and `Action::TagImport`.
  `api/tags.rs` registers `TagGraph` for `tag.tree`, `tag.refs` and
  `tag.kinds` (over `app/kinds.rs`).
  The tree and `tag.follow` share the reference reader with `tag.refs`.
- **Strings (rich editors N1).** `api/strings.rs` registers `Strings` for
  `strings.list`, a read of `tags::Localization` over the open map's
  `unic` tags. It returns `{languages, tags, total, offset, limit,
  truncated, strings, problems}`. `languages` lists the nine codes (`en`,
  `ja`, `de`, `fr`, `es`, `it`, `ko`, `zh`, `pt`). `tags` has one row per
  `unic` tag, `{tag, name, counts}`, with the number of strings each
  language's span holds. `strings` has one row per string key of a tag,
  `{tag, tag_name, id, name, texts}`: `id` is the packed string ID in hex,
  `name` the identifier it resolves to (`null` when the map's name list
  has none), and `texts` maps a language code to the text in that
  language's span (a language without the key is absent). `tag` picks one
  `unic` tag (an ID or name; another group is refused), `language` shows
  one language's strings only, and `query` keeps the keys whose name, ID
  or shown text contains it, case-insensitively. `limit` (default 200,
  at most 5000) and `offset` page the rows; `truncated` says more follow.
  A span that falls outside its table is named in `problems` and its
  language is left out of that tag. A map with no readable locale tables
  answers with no rows and `unavailable` giving the reason.
- **Scripts (rich editors H1).** `api/scripts.rs` registers `Scripts`
  for `script.*`, over `app/scripts.rs`'s `Loaded`; `script.source`,
  `script.follow` and `script.doc` are `Action::Script` through
  `App::perform`.
- **Curves (rich editors F1).** `api/curves.rs` registers `Curves` for
  `function.read`, `function.set` and `function.sample`, over
  `tags::effects::function` (`Set`, `Function::set_values`,
  `Function::write_set`) and the curve editor's own helpers
  (`fields/curve.rs`); `function.set` is a `TagAction::Write` of the
  same length at the data's start through `App::perform`, or, for a
  type change, a `TagAction::ResizeData` (rich editors F2), one undo
  step, exactly as the dialog's drags and type picker write.
- **Game type scripts.** `api/game_script.rs` registers `GameScript` for
  `rules.*`, over `App::rules` (`app/rules_edit.rs`), the state the
  window draws; `rules.reload` leaves the blob in its outbox and
  `play.rs` sends it on the game link.
- **UI screen edits.** `api/ui_edit.rs` registers `UiEdit` for the
  `ui.*` edit methods over `Action::UiEdit` (`app/ui_screens/edit.rs`),
  the same edits the canvas makes. Our screens' edits change their YAML
  docs and `ui.save` writes the sources named in the checkout’s UI package; a retail screen's
  are tag writes on the open `mainmenu` map, undone with it and saved by
  `doc.save`. Each edit answers with the widget and the checks' findings
  on the edited view; `ui.animate` writes a transition's filmstrip PNG
  (docs/game/ui-builder.md "Editing in the editor").
- **Retail tag edits.** `api/ui_retail.rs` registers `UiRetail` for
  `ui.hide {id}`, `ui.duplicate {id, rect?, grow?}` and `ui.set_string
  {id, string, shows?}`, over the same `Action::UiEdit`
  (`app/ui_screens/edit/retail_tag.rs`). They edit the retail screen
  `ui.pick {retail}` shows, in the open `mainmenu` map's tag document:
  `ui.hide` parks a text or bitmap block off screen, `ui.duplicate`
  appends a copy of a text, button or bitmap block to the end of its
  block and answers its id as `copy`, and `ui.set_string` sets the
  string ID of the title, a text or a button to a string by name, added
  to the map when it has none. `shows` is the stand-in text the picture
  draws for a string the game's code answers at run time; it is the
  tool's and is not saved. `grow: true` turns on Edit > Allow growing
  full blocks first, which a copy into a full block needs. A screen of
  ours, a button to hide, and a widget with no string ID answer
  `REFUSED` with the owner. `ui.edits` lists the blocks added and edited
  (`retail_blocks`) and the stand-ins (`stand_ins`); `ui.revert` puts a
  block back as the loaded map has it and removes a copy
  (docs/game/ui-builder.md "Retail tag edits").
- **Retail edits as data.** `api/ui_export.rs` registers `UiExport` for
  `ui.export_edits {path, screen?}`: the open map's retail screen edits
  (the tag document's writes to each screen tag's panes and header, as
  loaded against as edited) written as retail edit files, one per edited
  pane, into the directory `path` (`boot::ui_kit::retail_edits::diff_pane`,
  `to_yaml`). It answers `{map, written: [{path, screen, pane, edits}],
  skipped}`; `skipped` names each change the format cannot hold. No map
  open is `NO_DOCUMENT`, and a `screen` the map lacks `INVALID_PARAMS`.
  The files are ours, so `path` may be the checkout's
  `content/ui/retail-edits/`. The game applies files named by
  `screen-edits` items in the UI package at boot
  (docs/game/ui-in-maps.md "Retail edits as data").
- **HUDs (studio-hud, landed).** `api/hud.rs`: `hud.list` opens a map of
  the source (or a package) in the HUD tool and lists its `nhdt` tags
  and who names them; `hud.read` shows one and gives its widgets
  (`bitmap:<i>`, `text:<i>`, or a name), every field by its key in
  `hud_edit::layout`'s tables, and where each lands at the HUD state;
  `hud.set` and `hud.move` are `Action::HudSet` and `Action::HudMove`
  (one undo step; `merge` joins a drag), a `mode` first setting the
  state's screen mode; `hud.render` and `hud.check` draw at a `state`
  (mode, shield, body, grenade, frags, plasmas, rounds, heat, battery,
  zoomed, gate, words, inputs; `Action::HudState`) and give the PNG and
  the checks' findings; `hud.save_as` is `Action::HudSaveAs`, never over
  the source or into Git. `doc.*` with `"doc": "hud"` undoes, redoes,
  diffs (patches by widget and field) and saves the same session. String
  IDs are the Tags tool's to edit (`REFUSED`).
- **Modeling (studio-modeling, landed in part).** `api/geometry.rs`:
  `geometry.list` gives the open map's structure clusters and instanced
  definitions (and a render model's sections with `model`) as target ids
  `kind:tag:index` (`cluster:e1000004:0`); `geometry.open` enters edit
  mode on one, after which a `target` param may be left out.
  `geometry.read` gives the mesh (positions by vertex id, faces, the
  selection); `mesh.*` are `Action::Model(ModelAction::Apply)`, one undo
  step each (`mesh.transform` with a `merge` key joins a drag), and
  `mesh.apply` replays a `mesh.ops` log as one step. While a target is in
  edit mode, `doc.undo` and `doc.redo` with `"doc": "map"` are its undo.
  `geometry.check` gives each open target's write class and
  `save_refusal`, why a save would be refused (Allow unverified geometry
  writes off, `allow_unverified`, or an edit that changes a count).
  `geometry.collision.add_box {structure, lo, hi, material?}` adds a
  closed box solid inside `[lo, hi]` to a structure's level collision
  (`structure` its tag id), onto the boxes added before, as
  `Action::Model(ModelAction::AddCollisionBox)`, and answers its counts
  (`surfaces`, `planes`, `leaves`, `file_surfaces`); a box with no volume,
  outside the level's open space or on a tag that is not a structure is
  refused with the reason. `geometry.collision.clear {structure}` drops the
  structure's boxes (`cleared` whether it had any). A paired game gets
  both live as the `collision` row; the save writes the boxes;
  docs/editor/modeling.md, "Interfaces".
- **Textures (studio-modeling slices 5 and 6).** `api/bitmaps.rs`:
  `bitmap.list {name?}` gives the map's bitmap tags and their images;
  `bitmap.read` one bitmap's images (size, format, mips, write class)
  and its sequences (label, first image, count, sprites), with `grown`
  saying a save writes the whole tag again and `movable` null or why the
  bitmap cannot gain an image;
  `bitmap.pixels {bitmap, image?, rect}` the RGBA of a rectangle (at most
  65,536 pixels); `bitmap.paint`, `bitmap.fill` and `bitmap.adjust`
  (invert, levels, hsv) and `bitmap.import` (a PNG, resampled to the
  rectangle) are `Action::Bitmap`, one undo step each; `bitmap.export
  {bitmap, image?, mip?, out}` writes a PNG and is refused inside a Git
  checkout. A bitmap is named by hex tag id, path, `\key` path tail or a
  unique substring. `bitmap.reshape {bitmap, image?, width?, height?,
  format?}` and `bitmap.image.resize {bitmap, image?, size? | width?,
  height?, format?}` ask for a new size and/or format (`dxt1`,
  `argb8888`, ...) for an image, refused at once when a save would
  refuse it;
  `bitmap.image.add {bitmap, from?, size?, format?}` appends an image
  after the bitmap's last, its record copied from image `from` (default
  0; a 2D image) at `size` and `format` or the source's, its pixels
  transparent black until painted, imported or generated into; it
  answers the new index;
  `bitmap.sequence.add {bitmap, first, count?, label?}` appends a
  sequence entry over `count` images from `first`, labelled `label`,
  shaped like the bitmap's first sequence;
  each of these is one undo step;
  `bitmap.spans {bitmap, image?, face?, slice?, rect?}` gives the stored
  bytes the edits change (per LOD: offset, length, hex) without writing
  anything: the spans a save patches in place and a live push sends. Edits
  stay in the editor until a save. `doc.save_as` (and `doc.save`, for a map
  with a save target) with `"doc": "bitmaps"` (alias `textures`) is the
  map's save: it encodes every edited image and writes it with the rest,
  by the write class `bitmap.read` names for the image: `in_place`
  (bytes patched in the map's own raw blocks), `appended` (a LOD in a
  shared cache is copied into the map as a new raw block, or the image has
  a reshape), `new` (an added image: the whole tag is written again at
  the end of the metadata and the tag's index entry pointed at the copy,
  the other tags keeping their bytes), or `none` for P8, P8Bump, V8U8 and
  G8B8, which are read only.
  A save the map cannot take (a new raw block where the metadata does not
  end the file) is `REFUSED` with the reason. `doc.diff` lists the edited
  layers by tile and the pending reshapes; `doc.list` says the doc is
  `dirty` until its edits have been saved. `doc.undo`, `doc.redo` work on
  that doc (a reshape, an added image and an added sequence are one undo
  step each). docs/editor/modeling.md, "Texture editing" and "Interfaces".
- **Models.** `api/models.rs`: `model.list`, `model.read`,
  `model.render` and `model.check`, read-only over a map of their own
  (no actions, no undo); see "Models and customisation" below.
  `model.colors`, `model.emblem` and `custom.*` are planned there.
- **Image generation.** `api/image_gen.rs` registers `ImageGenApi`:
  `image.generate`, `image.result`, `image.apply`, `image.accept` and
  `image.discard` over the jobs on the app (`App::imagegen`, shared with
  the window's Generate window). Apply, accept and discard run the
  commands `Action::Image` runs, which write through the Textures tool's
  `BitmapAction`s ([Generating images](#generating-images)).

## Game type markers

`markers.*` (`api/markers.rs`, a provider) is the agent's view of the
shapes Studio draws for a game type's parts
([gametype-markers.md](../game/gametype-markers.md)): player spawns, the
netgame flags the game reads (flag, bomb, ball, teleporter, territory,
hill), item spawners and trigger volumes, all from the shared
`game_markers` set through `scenario_edit::marker_view`.

- `markers.list {kind?, layer?}`: every marker of the open map, whatever
  the layers show, as `{id, kind, layer, team, position, facing, label,
  number, area, area_from, members, bounds, lines, triangles}`, with
  `counts` by kind. `id` is the placement as `scenario.*` names it
  (`netgame_flags[3]`), or `volume[2]` for a trigger volume (one row a
  kind it draws as: trigger, kill, BSP switch); a hill of three or more
  corner flags is one row, `id` its first corner, `members` all of them,
  `area` the path of their hull and `area_from: "corners"`. `area` is
  written as the document writes it (floats as hex bits) and `area_from:
  "default"` marks a flag with no shaped area. `kind` is a name of
  `markers.render`'s `kinds`; `layer` one of `spawn_points`, `flags`,
  `netgame_items`, `trigger_volumes`, `kill_volumes`, `bsp_switches`. A
  name that is neither is `INVALID_PARAMS`. A flag of a type the game
  does not read has no row. `NO_DOCUMENT` with no map open.
- `markers.render {kind?, team?, area?, view?, size?, out?}`: the shapes
  drawn offscreen through the viewport's renderer on a floor grid, to a
  PNG under the cache directory's `studio/markers/`, never into a Git
  checkout (`REFUSED`). It needs no map and no disc. With no `kind` it is
  the contact sheet of every kind in red, blue and neutral (a kind of one
  fixed colour once); `team` (0 to 7) draws one. `area` (`{radius,
  below?, above?}` or `{path: [[x, y], ...], below?, above?}`, plain
  numbers) replaces the area of a hill, territory or siege site, and a
  refused area gives `Area::validate`'s sentence. `view` is `angled`
  (default), `top` or `front`, +x to the right; `size` defaults to
  `[1280, 720]`. The answer is `{path, width, height, view, covered,
  kinds}`: `covered` counts the pixels that are not the corner's
  background, and each of `kinds` has its `cell`, its `center` in pixels
  and its mesh's `lines`, `triangles` and `bounds`, so a picture is read
  against the list. `NO_GPU` where there is no adapter.
- `markers.area.set {id, area}`: sets the area of the netgame flag `id`
  (`netgame_flags[3]`) from `{radius, below?, above?}` or `{path: [[x,
  y], ...], below?, above?}` in plain numbers, or clears it with `null`
  (a missing `area` is `INVALID_PARAMS`). One undo step. Answers `{id,
  area}` as stored. `Area::validate`'s sentence is `REFUSED` and leaves
  the document as it was.
- `markers.area.convert {id}`: a retail hill's corner flag `id` becomes
  one flag with `Area::of_corners`' path, the other corners removed, in
  one undo step. Answers `{id, area, position}` of the kept flag; a flag
  that is not a hill's corner, or a hull `Area::validate` refuses, is
  `REFUSED`.

`view.layers` shows the markers' layers (`spawn_points`, `flags`,
`netgame_items`, the volume layers) and `view.snapshot` draws them.
`api/tests/markers_tests.rs` runs the list over the invented cache (each
block's kind and team, a hill of corners, the volumes), the refusals, the
sheet with every kind, and a snapshot whose spawn pixels all lie in the
spawns' projected boxes.

## Models and customisation

`model.*` is where a content author (an agent building a weapon, an
object or a player model) sees and checks the content before handing it
back: the objects of a map and their models, the model drawn the ways
the game draws it, and the checks that find what would break in play.
`custom.*` and `model.colors`/`model.emblem` are planned: their stubs
answer `NOT_YET`.

### `model.*`

The provider (`api/models.rs`) reads one map, read-only, and keeps it
between calls:

- With no `map`, the map the scenario tool has open (followed when the
  scenario tool opens another).
- `map: "<name>"`: that map of the source (`source.maps` lists them),
  opened as the HUD tool opens one.
- `pogo: true`: that map with the Pogo Stick's kit added
  (`pogo_kit::inject::add_pogo`, which the `pogo` package's build equals,
  uncached). A map the kit cannot go into is `REFUSED` with
  `data.reason: "pogo_refused"` and the kit's reason. A map that already
  holds the kit (its `pogo_kit_v<n>` string) is read as it is. `pogo`
  left out keeps the open map's choice.

Every method takes `map?` and `pogo?`. `tag` is a `0x...` id,
`group:name`, a tag's name, a lone group with one object tag, or a
unique part of an object tag's name (`pogo_unsc`). A name that fits
several tags is `INVALID_PARAMS` with the candidates.

- `model.list {group?}`: the object tags of `group` (`bipd` by default;
  `weap`, `vehi`, ... or `all`) with each one's `hlmt`, render model,
  variant names, region count and, for a `weap`, how many first-person
  entries it has. With it: `pogo` (the kit's three `weap` tags, and what
  adding the kit cost when Studio added it) and `tag_memory` (`used`,
  `limit`, `free` bytes: the map header's `+0x1c` against
  `map::writer::max_metadata_size` for its cache type).
- `model.read {tag, variant?}`: the object's `hlmt` and its linked
  render, collision, physics and animation tags; the variant drawn (the
  object's `default_variant` unless `variant` names one) and every
  variant's name; the variant's regions with each permutation, its
  probability, whether the render model has it, and its `tag.set` path
  on the `hlmt` (`variants[v].regions[r].permutations[p].probability`);
  the node count; the render model's markers (`mode +0x58`, name and
  node); the object's change-colour count; and for a `weap` its
  `first_person` entries with their `model` and `animations` and paths.
- `model.render {tag, view?, variant?, character?, biped?, hands?,
  size?, camera?, out?}`: the model drawn offscreen through the
  viewport's native object renderer
  (`scenario_edit::native_view::preview`, wgpu headless) to a PNG under
  the cache directory's `studio/model/`, never into a Git checkout
  (`REFUSED`). `NO_GPU` where there is no adapter. `view` is one of:
  - `ground` (the default): the object's world model at its variant,
    orbited to fit (`camera` takes `yaw`, `pitch` in degrees, `distance`,
    `target`).
  - `third_person`: on a player biped (the globals' player
    representation `character`, default 0, or `biped`), the weapon at the
    biped's right-hand marker (`unit right_hand_node`), both in their
    rest pose.
  - `first_person`: the `weap`'s `first_person[character].model` and the
    globals' first-person hands for that character (`hands: false` leaves
    them out), seen from the first-person camera: the eye at the
    first-person models' origin looking along +x, +z up, at the game's
    default 70 degrees across (`camera.fov`), the models in their rest
    pose (no first-person animation).

  A view that cannot be drawn is `REFUSED` with `data.reason` and no
  picture: `no_first_person` (the `weap` has no `first_person` entry,
  `data.path: "first_person"`), `first_person_no_model` (the entry's
  model is null), `first_person_model_fails`, `not_a_weapon`,
  `no_player_biped`, `biped_fails`, `hand_marker_missing`, `model_fails`.
  A material whose shader names no base map the renderer decodes is drawn
  grey (`untextured` in the piece). The answer gives `covered`, how many
  pixels differ from the background, and `notes`; a first-person picture
  with nothing in view says so.
- `model.check {tag, variant?}`: the content's findings, each
  `{code, severity, message, tag, path}`: `severity` is `error`,
  `warning` or `note`, `tag` the tag to edit as `group:name`, and `path`
  its `tag.set` path when one field is the fix (null otherwise). The
  answer also gives `counts`, `tag_memory`, `pogo` and `editable` (false
  when Studio added the kit: the kit's tags are fixed in
  `crates/tools/pogo-kit`, not with `tag.set`). The codes:

  | Code | Severity | Finding |
  |---|---|---|
  | `no_first_person` | error | the `weap` has no `first_person` entry (`path: first_person`): the first-person view has no weapon model |
  | `first_person_no_model` | error | `first_person[i].model` is null |
  | `first_person_model_fails` | error | the first-person `mode` does not load |
  | `first_person_no_animations` | warning | `first_person[i].animations` is null: the model is not posed |
  | `first_person_missing_character` | warning | fewer `first_person` entries than the globals have player characters |
  | `unresolved_link` | error, warning | a reference names a tag the map does not have (the object's own references, the `hlmt`'s, `first_person`, `hud`) |
  | `wrong_group` | error | a first-person model that is not a `mode` |
  | `no_model`, `no_render_model` | error | the object names no `hlmt`, or the `hlmt` no `mode` |
  | `variant_missing` | error | the variant asked for, or the object's `default_variant`, is not in the `hlmt` |
  | `no_sections` | error, warning | a permutation the variant picks has no section in the render model (`path` on the `hlmt`), or a render-model region has none at all |
  | `node_count_mismatch` | error | the `hlmt` and its `mode` disagree on the node count |
  | `missing_marker` | error, warning | a barrel's `optional_barrel_marker_name` the weapon's model lacks; a player unit's `right_hand_node` its model lacks |
  | `untextured` | warning | materials with no decodable base map |
  | `model_fails`, `render_model_unreadable`, `model_invalid`, `unreadable` | error | the model, its geometry or the tag does not read |
  | `tag_memory` | note, warning, error | the map's tag memory used and free (with the kit in it, when added); a warning under 64 KiB free, an error over the limit |

Edits to a model's tags (a variant's permutation pick, a region's
probability, a weapon's first-person entry) are `tag.set` and
`tag.block.add` on the map the Tags tool edits, so they share its undo
and save; each `path` above is that call's. The provider re-reads its
map when the scenario tool opens another; after a save, `model.list`
with the saved map's name reads the edit.

### Model poses

`anim.*` reads a `jmad` tag of the map the Tags tool shows, poses it
on the model that uses it (rich-editors.md, "Model poses", slice M1) and
lists a clip's channels as keys. The Animation window and the API share
the same editable documents in `app::anim`. See the animation editor
in [studio-redesign.md](studio-redesign.md) (AN1, AN3 and AN4). `tag` takes
the same form as `sound.*`. Document IDs last for the Studio session,
and edits live in memory.

- `anim.list {tag}`: the graph's animations, each
  `{index, name, type (base|overlay|replacement|unknown), frames,
  codecs: [static, animated] (the engine's codec names, null where a
  stream is absent), aim_grid, error}` — `aim_grid` marks codec 8, whose
  "frames" are the cells of an aim grid, not points in time; `error`
  names why a damaged clip does not build (inline data is refused by
  name). With them: `nodes`, and `users`, the map's objects whose `hlmt`
  names the graph.
- `anim.pose {tag, animation, frame?, overlay?, model?, size?, out?}`:
  one animation sampled at `frame` (a fractional frame blends the
  neighbours; the default 0; a frame past the end holds the last) and
  posed on the model. `animation` is its name or index. `overlay` is
  `{animation, weight}`: a second animation applied over the base at
  `weight` 0 to 1 with the engine's scaled-add rule
  (anim-sampling.md, "Evaluators"; 0 changes nothing, 1 is the plain
  add). `model` names which user to pose (by tag name or `0x...` id)
  when several use the graph; without it the only one is posed, and
  when none does, the answer still gives the animation's own channels
  with a note saying so. The reply: `nodes`, each
  `{index, name, parent, rotation [x,y,z,w], translation, scale, has
  {rotation, translation, scale}, world}` — the local transform that
  composes (the animation's channel where it has one, the render model
  node's default where it does not, which `has` says), and the posed
  world matrix, column-major `T·R·S` (null without a model). The
  composition is the engine's own: the graph's nodes and the render
  model's are the same list, and the merged locals compose up the model
  node tree (animation.md, "Composing a pose"; a node-count
  disagreement is refused by name). The model is also drawn offscreen
  through the viewport's native object renderer to a PNG (`out`, or the
  cache directory's `studio/anim/`; never into a Git checkout), with
  `width`, `height`, `covered` as `model.render` gives them; without a
  renderer the transforms still answer and a note names why.
- `anim.pose {doc, frame?, base?, overlay?, model?, size?, out?}`:
  the same reply sampled from an editable document using its key
  interpolation. For an overlay document, `base` names or indexes a
  base animation in the same graph. Without `base` the overlay applies
  over rest. A base document accepts the `overlay` parameter described
  above. The Tags tool must show the document's map for posing.
- `anim.read {tag, animation, tolerance?, keys?}`: one decoded
  animation's source reduction. `nodes` has a row per node, `{index, name, parent,
  channels}`, with the channels the clip drives, each `{kind (rotation,
  translation or scale), moves, key_count, keys: [{frame, value}]}`. A
  rotation's value is a quaternion `[x, y, z, w]`, a translation's
  `[x, y, z]`, a scale's `[s]`. The keys are the clip's dense decoded
  frames reduced (`tags::anim_keys::reduce`): between two keys the value
  is linear (a rotation along the shorter arc, re-normalised, as
  `anim.pose` blends), before the first and after the last it holds, and
  every frame bakes back within `tolerance`. The default tolerance is a
  tenth of a degree of turn, a thousandth of a unit of translation and a
  thousandth of scale; `tolerance` is `{rotation_degrees?, translation?,
  scale?}`, none negative. A channel with one key does not change
  (`moves` false): the static stream's channels, and any that sit within
  tolerance of their first frame. `reduction` says what it cost:
  `{channels, dense_values, keys, max_error: {rotation_degrees,
  translation, scale}}`, the error measured at every frame against the
  decoded clip. `keys: false` lists the counts only. Also `animation`
  (the `anim.list` row), `frame_rate` (30), `frames`, `duration`
  (seconds), `tolerance`, `events {frame, sound, effect, decoded}` and
  `notes`. Events are counted, not decoded (`decoded` is false), and a
  note says so when there are any; so is a root movement
  (`frame_info_type` other than none). An aim grid (codec 8) is not a
  time series: its nodes list without channels and a note says so.
  `doc` identifies the same editable clip used by the window, seeded
  with the default tolerance even when this reply uses a custom
  tolerance. It is null when the source clip cannot open as a document,
  including aim grids.
- `anim.read {doc, keys?}` returns `{doc, name, frame_rate, frames,
  duration, revision, nodes}` for the document's current keys. Each
  key includes `{frame, value, interp, tangent}`. A tangent has the
  channel's width and uses stored coordinates per frame (quaternion
  components for rotation).
- `anim.new {tag, name, frames}` creates an empty base clip on the
  graph's skeleton. The name must contain a non-space character.
  `frames` is an integer from 1 to 32767.
- `anim.duplicate {tag, animation, name}` creates a document from a
  decoded clip. `anim.duplicate {doc, name}` copies an open document's
  current keys. Both forms and `anim.new` return `{doc, name, frames,
  nodes, revision}` with a node count and a fresh revision of zero.
- `anim.key {doc, node, channel, frame, value?, interp?, remove?}`
  edits one key. `node` is an index or a unique exact node name.
  `channel` is `rotation`, `translation` or `scale`. A zero-based
  `frame` must fit the clip. Set `value` as above or supply just
  `interp` to change an existing key. `step`, `linear` and `smooth`
  govern the segment to the next key. A new key defaults to linear.
  `remove: true` takes neither `value` nor `interp`. The reply is
  `{doc, revision, changed, key}` with the current key or null.
- `anim.save {doc, package, mode?, class?, weapon_type?, action?}`
  writes an `animation` item into a local package for the Tags tool's
  current map. A document opened from a source clip replaces that clip
  and keeps its action. New and duplicate documents require an
  `action`. Their mode, class and weapon type each default to `any`.
  The reply is `{doc, package, tag, map, source, item}`, with the
  package-relative `.anim.yaml` path and the zero-based manifest item
  index. Saving the same clip again updates its item and lock. The
  package builder checks the graph and writes its records and raw
  stream when the package builds. The game takes the rebuilt graph on
  map load.
- `anim.check {doc, model?}` returns `{doc, nodes, feet_checked,
  feet_below_ground, loop_seam}`. Each `nodes` row has
  `{node, rotation_degrees, translation, scale}` and records the
  largest quantisation error across all baked frames for codecs 1
  and 3. `loop_seam` uses the same row shape for the difference in
  local transforms between the first and last frames. When a model
  and a named foot node are available, `feet_checked` is true and
  `feet_below_ground` lists `{node, frame, z}` for each foot below
  model-space `z = 0`. Overlay diagnostics use full weight over rest.
  `model` and the shown map follow the rules for posing.

### `model.colors` and `model.emblem` (planned)

`model.colors {player?, colors?}` and `model.emblem {player?, emblem?}`
read or set a player's four colour indices and emblem. These are not
tag edits: they are the profile's look bytes (`bots::guest::bot_look`
builds the eight bytes the game checks at `0x153750`: four colour
indices below 18, the model below 4, the emblem foreground below 0x40,
background below 0x20, and flags), so they edit a profile document
(`doc: "profile"`) with its own undo and a save into the user's save
storage under the data root, never the map. The bounds are `0x153750`'s,
refused with `INVALID_PARAMS` and the range. To establish first: where
the profile's look bytes live in a save (the variant store's save layout
covers game variants only), and which shader parameters take the change
colours (so a render matches the game).

### `custom.*`

Retail Halo 2 has no armour choice: a player's model is one of four and
its permutations are picked by the variant's probabilities. Armour
customisation is therefore an h2recomp extension, like the variant
extension block (`variants::ext`):

- `custom.list {biped?}`: for each region of the biped's player variant,
  the permutations a player may pick (from `model.read`), and which are
  locked and why.
- `custom.set {biped, region, permutation}`: pick one for the profile,
  stored in the profile's h2recomp extension (not the map, not the
  retail look bytes), one undo step in the `profile` document.

It needs the game side first: the runtime choosing a player's
permutation from the profile instead of by probability, and the network
carrying the choice. Until that lands the methods stay planned; the
editor side is small once the profile document exists.

### Tests

`model.*` runs its examples in `every_method_in_methods_has_a_test` on
the Pogo kit's invented map with the kit in it (`pogo_kit::fixture`, no
game data). `api/models/tests.rs` checks the kit's weapons listed, a
look with its first-person block taken out found by `model.check` and
refused by a first-person render, the first-person view the kit copies
from the fixture pistol checked (its invented models do not load:
`first_person_model_fails`), a weapon held in its own model checked and
drawn (the picture only where there is a GPU), a render refused in a
Git checkout, and `pogo: true` refused by name on a map the kit cannot
go into. The planned methods need: colour and emblem bounds refused;
profile edits undone and redone as their document; and the window's
actions and the API's make identical profiles.

`anim.*` runs its examples on an invented two-bone map (a body box and
an arm box, their graph and the `bipd` that uses them, built with the
Halo 1 converter's writers in `api/anim/tests.rs`). That suite is the
slice's accepted test: posed at the last frame of the base clip, the
arm's transforms are the expected quarter turn and the body's are the
rest pose — the pose differs from the rest where the clip moves it and
nowhere else. It also checks the list (names, codecs, the user), the
overlay at weights 0 and 1 (0 changes nothing; 1 composes the base's
quarter about z with the overlay's about x), a frame past the end
holding the last, and the refusals: a tag that is not a `jmad`, an
animation that is not there, a model that does not use the graph, and a
graph nothing uses (the channels answer with a note).

`anim.read` runs on its own invented map (`api/anim/read_tests.rs`: the
Pogo kit's map with a two-bone graph in it, built with the Halo 1
converter's writers): a base clip of six frames whose arm turns 0, 45,
90, 90, 45, 0 degrees while it slides, and an overlay. The slice's
accepted test: the clip lists its channels (the body's three as one key
each, the arm's turn as four keys at frames 0, 2, 3 and 5, its slide as
two, its scale as one), and the keys played back by the stated rule sit
within the tolerance of `anim.pose` at every frame, at the default and
at a looser one. It also checks that a wide tolerance collapses a
channel to one key with the measured error, `keys: false`, an overlay's
single channel, and the refusals (a tag that is not a `jmad`, an
animation that is not there, a negative or unknown tolerance).
`tags::anim_keys`' own tests (`anim_keys_tests.rs`) cover the reduction:
keys within tolerance at every frame on a noisy clip, a flipped
quaternion sign, undriven channels, one frame, none. The timeline's
transport (scrub, play, step, loop, speed) is `app::anim::Transport`,
tested in `app/anim.rs`. The same invented graph tests animation
documents, including gizmo/RPC equivalence and single-step drag undo.
`crates/tools/anim-edit/src/tests.rs` covers interpolation, history,
diagnostics and angle display. `ui/anim/edit.rs` tests that plotting
the rotation leaves its stored quaternions unchanged.
`api/tests/anim_save_tests.rs` verifies source clip replacement,
map scoping, lock updates and key round-trips through `anim.save`.

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
