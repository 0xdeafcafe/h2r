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
The Tags tool (`tag.*`) and the HUD tool (`hud.*`) answer through
providers. `model.*` and `custom.*` are planned namespaces: listed by
`session.methods` as `planned` and answering `NOT_YET` until their tools
register.

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
| Open a source, map, package, Halo 1 map | `OpenSource`, `SelectMap`, `OpenPackageEntry` | menus, Maps list | `source.open`, `doc.open` |
| A placement's field | `EditSelection {field, edit, merge}` (every selected placement), `SetField {block, index, field, value, merge}` (one), `SealEdit` | the inspector dispatches `EditSelection` (a drag merges under one key, sealed when it ends) | `scenario.set`, `tag.set` |
| Any tag's field or block | `Tag(TagAction)` built by `app::tag_api::{set_field, block}` | the Tags tool | `tag.set`, `tag.block.*` |
| Inspect tag fields | read the registry layout | Tags panel and inspector | `tag.fields {tag, path?, unnamed?}` |
| Show unnamed fields, Show as, focus a field | `App::tag_fields` (`fields::Settings`), `Navigate(Go)` | the Tags tool's **Show unnamed fields** and **Show as** | `tag.view` |
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
| Show or hide viewport layers | `SetLayers`, `SetOverlay` | Show popover, Scene legend, View > Halo 1 overlay | `view.layers` |
| Choose what the level is drawn with | `SetLighting` | the toolbar's Lighting menu | `view.lighting` |
| Navigate tag fields | `Navigate` | inspector links and breadcrumbs | `nav.go`, `nav.back`, `nav.forward` |
| Undo, redo | `Undo`, `Redo` (the tool shown), `UndoIn(tool)`, `RedoIn(tool)` | Edit menu, keys | `doc.undo`, `doc.redo` |
| Save | `Save`, `SaveMapAs`, `SaveVariant`, `SaveHopper(As)`, `ExportVariant` | File menu, shortcuts, prompt | `doc.save`, `doc.save_as` |
| Go to anything | `GoTo` for tags and placements, existing map-open and menu actions for the other entries | Edit menu, Cmd/Ctrl+P | `ui.palette {query, run?}` |
| Variant edits | `OpenVariant`, `NewVariant`, `CopyBuiltin`, `SetVariantName`, `SetSetting`, `ImportVariant*`, `DeleteVariant` | the variant tool | `doc.open`, `variant.*` |
| Playlist edits | `EditPlaylist(PlaylistEdit)`, `SetEntryVariant` | the title and description fields, the table's row buttons, weights, Add, the unused slot's "Make a … playlist" buttons and "Remove playlist" apply the same `PlaylistEdit`s | `playlist.set`, `playlist.entry`, `playlist.entry_variant` |
| UI screens | `UiPick`, `UiPress` | the UI Screens tool | `ui.pick`, `ui.press` |
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
whether it can undo and redo.

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
| `doc` | `list`, `open {map \| path \| package \| variant \| playlists}`, `save`, `save_as {path}`, `diff`, `undo`, `redo`, `close` |
| `tag` | `list {group, name, query, path, limit, homes}`, `browse {map}`, `resources {tag}`, `export {tag, out, dependencies, resources}`, `import {path, tag, skip_missing}`, `read {tag, depth, elements, hex}`, `fields {tag, path?}`, `tree {root?, depth?}`, `refs {tag, limit?}`, `follow {tag, ref}`, `set {tag, path, value}`, `block.add`, `block.remove`, `block.duplicate` |
| `sound` | `read {tag, range?, permutation?, bins?}`, `play {tag, range?, permutation?, from_ms?}`, `stop` |
| `scenario` | `blocks`, `list {block}`, `get`, `set {block, index, field, value}`, `add {block, like, position}`, `move {block, index, position, ypr}`, `delete`, `duplicate`, `select` |
| `volume` | `list {kind?}` (short form `volumes [kind]`), `select {index?, focus?}`, `set {index, origin?, center?, forward?, up?, size?}`, `add {kill?, center?, origin?, forward?, up?, size?}`, `remove {kind?, index}` |
| `collision` | `surfaces {class?, bsp?, offset?, limit?}`, `at {origin, dir}` or `at {point, within?}` (with `select?`), `select {index?, focus?}`, `objects {block?, index?}` |
| `nav` | `status`, `back`, `forward`, `go` (the inspector's history); `pathfinding {bsp?, offset?, limit?}`, `bots {wait?, polys?, links?, spots?, offset?, limit?}`, `pick {origin, dir}` or `pick {point, within?}` (with `layer?`, `shown?`, `select?`) or `pick {kind, index, focus?}` (the Navigation layers) |
| `variant` | `list`, `get`, `set {setting, value}`, `rename`, `copy`, `export`, `import`, `delete` |
| `playlist` | `list` (with `unused` per playlist and the `templates`), `get {playlist}`, `set {playlist, title, description, template, clear}` (`template`: make the slot a playlist of that kind, or give a playlist another kind; `clear: true`: make it unused; [live-matchmaking.md §6.10](../live/live-matchmaking.md)), `entry {playlist, op, entry, to, weight}`, `entry_variant` |
| `ui` | `palette {query, run?}`, `screens`, `pick {screen, state \| retail}`, `press {input}`, `widgets`, `render {out}` |
| `pair` | `list`, `status`, `connect {socket?}`, `disconnect {leave?}`, `auto {enabled?}`, `play`, `stop`, `control {action, ticks?}`, `edit {enabled}` |
| `mirror` | `status`, `objects {kind?, near?, limit?, at_tick?}`, `pick {from? \| ray?}`, `aim_select {enabled?}`, `camera {mode?, target?}`, `player {to, facing?}`, `pending`, `apply {what, at?, peers?}` |
| `view` | `camera {yaw, pitch, distance, target, focus, all, fly_speed}`, `snapshot {out, size, camera}`, `stats`, `layers {set?, preset?, xray?}` |
| `script` | `list {filter?}`, `source {name?}`, `follow {name?, link}`, `uses {kind, name}`, `builtins {filter?, unnamed?}`, `doc {path}` ([rich-editors.md](rich-editors.md#scripts)) |
| `rules` | `open {path? \| starter? \| source?, discard?}`, `get {text?}`, `set {source}`, `save {path?, reload?}`, `format`, `reload {mode?}`: the game type script editor ([editor-guide.md](editor-guide.md#game-type-scripts)) |
| `hud` | `list {map \| package}`, `read {hud, state}`, `set {hud, widget, field, value, merge}`, `move {hud, widget, x, y, mode, merge}`, `render {hud, state, out}`, `check {hud, state}`, `save_as {path}` |
| `geometry` | `list {model?}`, `open {target}`, `close {target?}`, `read {target?}`, `check {target?}` |
| `mesh` | `mode {mode}`, `select {ids, op \| all \| none \| invert}`, `transform {translate \| matrix, pivot?, merge?}`, `ops`, `apply {ops}`; each with `target?` |
| `model` (planned) | `list`, `read`, `render`, `colors`, `emblem` |
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
  the map's names, disagreed and named.
- **Game type scripts.** `rules.*` drive the one script the Game type
  script window shows (`app/rules_edit.rs`). Each answers its state:
  `title`, `path`, `dirty`, `problems` (each `line`, `col` from 1,
  `severity`, `message`, `code`; errors first), `check` (the
  [workbench](../game/rules-engine.md#workbench) report: `ok`, `hash`,
  `errors`, and `report` with the worst tick against the budget, the
  handlers, slots, IR size and capabilities), `mode`, `reload_on_save`,
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
    `Internal value`, `Unnamed value`, `Padding`) and `unit` (or null);
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
- **The field panel.** `tag.view {tag?, path?, field?, unnamed?,
  show_as?}` drives the Tags tool's panel and returns what it shows.
  `tag` selects the tag in the Tags tool. `path`, a list element
  (`parts[1]`, or by offset `+0x8[1]`), shows that element as the whole
  panel, as its **Open alone** does; empty shows the root. `field`, a
  `tag.set` path, focuses that field through the navigation (as
  `nav.go`), which opens its lists and its Unnamed fields group; on its
  own it is shown from the root. `unnamed` sets **Show unnamed fields**.
  `show_as: {path, as}` sets how an unnamed value is shown (`decimal`,
  `whole_number`, `hex`, or null for the guess from its bits), kept for
  that field of every tag of the group and saved with the layout. With no
  params it only reads. The result is `{tag, path, breadcrumbs, field,
  search, changed, unnamed, rows, unnamed_hidden, header}`: `path` is
  the element shown (by field names, empty for the root), `breadcrumbs`
  its crumbs as drawn (`weap: objects\test\blaster`, then `Parts 1
  scope` per list step), `rows` the element's `tag.fields` rows the
  panel shows, `unnamed_hidden` the
  unnamed rows in a folded group (`unnamed: true` adds them to `rows`),
  `field` the focused path, and `header` the header's lines as drawn,
  without icons: the kind and name, the path with the unsaved count, any
  browsed, BSP, shared-map and read-only notes, and how a change reaches
  the game (`Changes are saved with the map.` with no game). The datum
  and address are not in it. The panel has no search yet, so `search` is
  null and `changed` false, and those params are refused.
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
  name or ID, if known), `cycle` and `children`. A missing target keeps
  its ID and path with null tag metadata. Cycles stop the current branch.
  `truncated` means a traversal limit beyond the requested depth cuts
  the result short. Live handles belong to the [click-to-edit design](click-to-edit.md).
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
  peaks. A permutation that cannot be decoded gives `refusal` in place of
  those (a codec stream without the game's executable says `This sound
  needs the game's executable to decode.`) or `error` (a bad length, a
  missing resource file). It also gives `playback`: whether something
  plays, which, `position_ms` and the `output`. `sound.play` plays a
  permutation from `from_ms` through the host's default audio output,
  opened on the first call, and returns at once; a refusal is `REFUSED`,
  no device or an unreadable resource `FAILED`. `sound.stop` stops it. The
  executable comes from the source: the disc's `default.xbe`, or the one
  beside a loose map or folder (`H2_XBE` names another). The window's
  Sound window calls the same `App::sound_*` methods.
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
  `mirror.pending` returns `pending` rows with `doc,path,effect,text,why` and an optional
  `tag` and `label`: `effect` is when the change takes effect (`now`, `next_use`,
  `next_spawn`, `reload` or `not_known`, `play::tag_sync::Effect`) and `text` its sentence,
  the game’s existing `journal` from `edit.changes`, and `leaving`.
  A placement block with a different layout revision from the loaded copy
  appears as one `Reload` row. Its fields stay out of the batch until undo,
  redo, or loading a Play copy brings the revisions into agreement.
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
- **Renders.** `ui.render` draws on the CPU (no GPU needed); `view.snapshot`
  renders the open map offscreen through the viewport's own renderer
  (wgpu, no window) and answers `NO_GPU` on a machine without an adapter.
- **Render stats.** `view.stats` (short form `render-stats`) says what the
  viewport draws for the open map, with no GPU: `lightmap_pages`, each
  BSP's `lighting` (`lightmapped`, `no lightmap`, `empty lightmap group`,
  `lightmap of another BSP`, `lightmap does not decode`), the world's
  `triangles` by blend with `solid_triangles` and `lightmapped_triangles`,
  every material (`shader`, `template`, `blend`, `blend_from_passes`,
  `emissive`, `base`, `details`), the `sky` drawn and the `atmosphere`
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
  `api/tags.rs` registers `TagGraph` for `tag.tree` and `tag.refs`.
  The tree and `tag.follow` share the reference reader with `tag.refs`.
- **Scripts (rich editors H1).** `api/scripts.rs` registers `Scripts`
  for `script.*`, over `app/scripts.rs`'s `Loaded`; `script.source`,
  `script.follow` and `script.doc` are `Action::Script` through
  `App::perform`.
- **Game type scripts.** `api/game_script.rs` registers `GameScript` for
  `rules.*`, over `App::rules` (`app/rules_edit.rs`), the state the
  window draws; `rules.reload` leaves the blob in its outbox and
  `play.rs` sends it on the game link.
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
  writes off, `allow_unverified`, or an edit that changes a count);
  docs/editor/modeling.md, "Interfaces".
- **Textures (studio-modeling slice 5).** `api/bitmaps.rs`:
  `bitmap.list {name?}` gives the map's bitmap tags and their images;
  `bitmap.read` one bitmap's images (size, format, mips, write class);
  `bitmap.pixels {bitmap, image?, rect}` the RGBA of a rectangle (at most
  65,536 pixels); `bitmap.paint`, `bitmap.fill` and `bitmap.adjust`
  (invert, levels, hsv) and `bitmap.import` (a PNG, resampled to the
  rectangle) are `Action::Bitmap`, one undo step each; `bitmap.export
  {bitmap, image?, mip?, out}` writes a PNG and is refused inside a Git
  checkout. A bitmap is named by hex tag id, path, `\key` path tail or a
  unique substring. Edits are kept in the editor only: the write class is
  `memory`, or `none` for P8, P8Bump, V8U8 and G8B8, and `doc.save` with
  `"doc": "bitmaps"` (alias `textures`) is `REFUSED`. `doc.undo`, `doc.redo`
  and `doc.diff` work on that doc. docs/editor/modeling.md, "Interfaces".
- **Player models (planned, not started).** See "Planned: player
  models and customisation" below.
- **Customisation (later).** Same section.

## Planned: player models and customisation

Design notes for the `model.*` and `custom.*` providers. Nothing here is
built; the table's stubs answer `NOT_YET`. Each part
says what it reads, what it edits, through which action, and what is
still to establish.

### What there is to build on

- `tags::object` resolves an object to its model: `Context::resolve_object`
  gives the object's `hlmt` and `Context::resolve_model(id, variant)` a
  variant's regions and the permutations each picks (`RegionChoice`,
  `PermutationChoice`), with the render, collision, physics and animation
  tags linked.
- `native-render::model_load` loads a `mode` and picks a variant's
  sections (`pick_sections`): what a preview draws.
- `bots::guest::bot_look` builds the eight player look bytes the game
  checks (`0x153750`; `crates/live/bots/src/phantom/session.rs`): four colour indices (armour primary and secondary,
  emblem primary and secondary, each below 18), the model (below 4), the
  emblem foreground (below 0x40), background (below 0x20) and flags.
- The Tags tool (`app::tag_doc`) writes any tag field with undo and saves
  with the map; the scenario viewport renders offscreen (`view.snapshot`).

### `model.*`

A Models tool (`ToolKind::Models`, its state beside the HUD tool's) over
the map open in the scenario tool, and a provider in `api/models.rs`:

- `model.list {group?}`: bipeds (and with `group`, any object group)
  with their `hlmt`, variant names and region count, from
  `Catalog::from_map` and `resolve_object`. Read-only.
- `model.read {tag, variant?}`: the variant's regions, each with its
  permutations and probabilities, the change colours the object's
  shaders take, and the render model's node count. Read-only.
- `model.render {tag, variant?, colors?, camera?, out?}`: the variant
  drawn offscreen with the scenario viewport's renderer (wgpu headless,
  `NO_GPU` where there is none) at the player colours given, to a PNG
  under `studio/model/`, never into a Git checkout. The preview state
  (variant, colours, camera) is an action (`ModelPreview`), not an edit:
  no undo, no dirty mark.
- `model.colors {player?, colors?}` and `model.emblem {player?, emblem?}`:
  read or set a player's four colour indices and emblem. These are not
  tag edits: they are the profile's look bytes, so they edit a profile
  document (`doc: "profile"`) with its own undo and a save into the
  user's save storage under the data root, never the map. The bounds are
  `0x153750`'s, refused with `INVALID_PARAMS` and the range.

To establish first: where the profile's look bytes live in a save (the
variant store's save layout covers game variants only), and which
shader parameters take the change colours (so a render matches the
game).

Edits to a model's tags (a variant's permutation pick, a region's
probability) are not new methods: they are `tag.set` on the `hlmt`
through the Tags tool, so they share its undo and save. `model.read`
returns the `tag.set` path of each editable value so an agent can go
straight there.

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

### Tests the providers need

As the HUD provider's: every method runs its example in
`every_method_in_methods_has_a_test` (on an invented map with a biped,
`hlmt` and `mode`, no game data); `model.render` refused in a Git
checkout; colour and emblem bounds refused; profile edits undone and
redone as their document; and the window's actions and the API's make
identical profiles.

## Code

- `crates/apps/studio-proto`: method names, error codes, the socket path,
  the short forms and the client (`h2 studio`, `h2-editor call`). No
  editor dependencies, so the `h2` CLI links it without wgpu.
- `crates/apps/studio/src/api/mod.rs`: `METHODS` (every method's shape and
  example), `Api::call` (shape check, providers, built-ins),
  `Api::pump` (the window's between-frames answer).
- `api/server.rs`: the socket. `api/{docs,scenario,tags,variants,playlists,ui_screens,view}.rs`:
  the namespaces (`volume.*` is in `api/scenario.rs`, `collision.*` in `api/collision.rs`). `api/seams.rs`: providers; `api/tag_fields.rs`,
  `api/tag_io.rs`, `api/sound.rs`, `api/hud.rs` and `api/scripts.rs` the Tags,
  sound, HUD and Scripts tools', `api/geometry.rs` the Modeling tool's.
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
- `view.stats` of an invented scene reports its BSP's lighting, its
  triangles by blend, its materials and its sky's sun and fog
  (`api::view::tests`);
- the window's loop answers a client between frames.

`studio-proto`'s tests cover the short forms and the wire round trip.
