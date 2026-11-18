# h2-editor's agent API

An agent (or a script, or a test) edits what the editor edits: maps and
their tags, scenario placements, game variants, playlists and UI screens,
through JSON methods on a local socket. The style is the game's agent API
([agent-api.md](../runtime/agent-api.md), `h2 agent`): JSON-RPC 2.0, one
object per line, short forms on the command line, scripts on stdin, and a
`session.methods` call that lists every method with its params and result.

Status: first slice built (`crates/studio/src/api`, `crates/studio-proto`).
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
  `--hopper`, ...), so a run can begin with a map open. It runs until
  killed.

The socket is `--socket`, else `$H2_STUDIO`, else `studio.sock` in the
cache directory (`~/Library/Caches/h2recomp/` on macOS, beside the game's
`agent.sock`). It is a Unix socket only, mode 0600; the editor never
listens on TCP, and the client refuses a `tcp://` path.

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
(`crates/studio/src/app.rs`). The window turns clicks into actions and
`App::dispatch`es them (an action that would drop unsaved edits waits for
the prompt). The API turns calls into the same actions and
`App::perform`s them: the action runs through the same `App::run`, and
the caller gets the log lines it wrote or why it failed. An action that
would drop unsaved edits is refused (`UNSAVED`) unless the call passes
`"discard": true`; the API never holds one for a prompt.

What goes through actions now, from both sides:

| Edit | Action | Window | API |
|---|---|---|---|
| Open a source, map, package, Halo 1 map | `OpenSource`, `SelectMap`, `OpenPackageEntry` | menus, Maps list | `source.open`, `doc.open` |
| A placement's field | `EditSelection {field, edit, merge}` (every selected placement), `SetField {block, index, field, value, merge}` (one), `SealEdit` | the inspector dispatches `EditSelection` (a drag merges under one key, sealed when it ends) | `scenario.set`, `tag.set` |
| Any tag's field or block | `Tag(TagAction)` built by `app::tag_api::{set_field, block}` | the Tags tool | `tag.set`, `tag.block.*` |
| Move and turn a placement | `MovePlacement` | (the gizmo still drives `Session` gestures) | `scenario.move` |
| Add, duplicate, delete | `Add`, `DuplicateEntry`, `DeleteEntry` | outliner, toolbar, keys | `scenario.add`, `.duplicate`, `.delete`, `tag.block.*` |
| Select, frame | `Select`, `FocusSelected`, `FrameAll` | outliner, viewport, keys | `scenario.select`, `view.camera` |
| Undo, redo | `Undo`, `Redo` (the tool shown), `UndoIn(tool)`, `RedoIn(tool)` | Edit menu, keys | `doc.undo`, `doc.redo` |
| Save | `SaveMapAs`, `SaveVariant`, `SaveHopper(As)`, `ExportVariant` | File menu, prompt | `doc.save`, `doc.save_as` |
| Variant edits | `OpenVariant`, `NewVariant`, `CopyBuiltin`, `SetVariantName`, `SetSetting`, `ImportVariant*`, `DeleteVariant` | the variant tool | `doc.open`, `variant.*` |
| Playlist edits | `EditPlaylist(PlaylistEdit)`, `SetEntryVariant` | the table's row buttons, weights and Add apply the same `PlaylistEdit`s | `playlist.set`, `playlist.entry`, `playlist.entry_variant` |
| UI screens | `UiPick`, `UiPress` | the UI Screens tool | `ui.pick`, `ui.press` |

Not yet through actions: the gizmo's frame-by-frame gesture
(`Session::gesture_set`, merged into one undo step by the session), and
the playlist table's map pickers and its title and description text boxes,
which edit the working copy in place inside the frame's
`HopperSession::edit_with` (one undo step per typing run, as before).
`PlaylistEdit::Title` and `::Description` are the API's path to the same
fields.

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
map: its widget edits, with their own undo stack and save). Without one, a `doc.*` call means the tool shown.
`doc.list` says, for each, whether it is open, its name, dirty, and
whether it can undo and redo.

Saving never writes where it wasn't told: `doc.save_as` takes a path;
`doc.save` writes a variant to its save storage under the data root, a
playlist file to its own path, and a map only to the file it was last
saved as. A map is never saved over its source, and never into a Git
checkout (`REFUSED`): what is patched from the user's disc stays out of
repositories. Renders (`ui.render`, `view.snapshot`) default to
`studio/` in the cache directory and refuse a path in a Git checkout too.

`doc.diff` is what a save would write against the document as opened.
For a map, each patched run of bytes with its address, the block, entry
and fields it holds (a removal's count word shows as `count`), and the
blocks that grew; for playlists, which playlists changed and how; for a
variant, its settings off the defaults.

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
| `source` | `open {path}`, `maps` |
| `doc` | `list`, `open {map \| path \| package \| variant \| playlists}`, `save`, `save_as {path}`, `diff`, `undo`, `redo`, `close` |
| `tag` | `list {group, name}`, `read {tag, depth, elements, hex}`, `refs {tag}`, `follow {tag, ref}`, `set {tag, path, value}`, `block.add`, `block.remove`, `block.duplicate` |
| `scenario` | `blocks`, `list {block}`, `get`, `set {block, index, field, value}`, `add {block, like, position}`, `move {block, index, position, ypr}`, `delete`, `duplicate`, `select` |
| `variant` | `list`, `get`, `set {setting, value}`, `rename`, `copy`, `export`, `import`, `delete` |
| `playlist` | `list`, `get {playlist}`, `set {playlist, title, description}`, `entry {playlist, op, entry, to, weight}`, `entry_variant` |
| `ui` | `screens`, `pick {screen, state \| retail}`, `press {input}`, `widgets`, `render {out}` |
| `view` | `camera {yaw, pitch, distance, target, focus, all}`, `snapshot {out, size, camera}` |
| `hud` | `list {map \| package}`, `read {hud, state}`, `set {hud, widget, field, value, merge}`, `move {hud, widget, x, y, mode, merge}`, `render {hud, state, out}`, `check {hud, state}`, `save_as {path}` |
| `model` (planned) | `list`, `read`, `render`, `colors`, `emblem` |
| `custom` (planned) | `list`, `set` |

Notes on some:

- **Tags.** A `tag` param is an id (`"0xe1230045"` or a number), a full
  name, `name.group`, or a group alone when the map has one tag of it
  (`"scnr"`, `"matg"`). `tag.read` and `tag.refs` read the map as opened
  from its source, with the layout schema (`tags::schema`): blocks with
  their elements (`depth` levels, `elements` per block), tag references,
  datums and string IDs, labelled by offset. `tag.follow` takes a
  `tag.refs` path (`"0x98[0].0x0"`) or its position in that list.
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
- **Placements.** `scenario.set` takes a number, `[x, y, z]`, or for a tag
  reference `{"id": "0x..."}` (or `null` for none). The scenario checks it
  first: read-only fields, integer ranges, finite floats, palette indices,
  tag groups. `scenario.add` and `.duplicate` are refused where the block
  is full and the map editor can't grow it safely (the same
  `GROWTH_REFUSAL` the window shows; `data` has the count and capacity);
  `.delete` where something refers to entries by index. A new entry goes
  to `position`, else the camera focus, in one undo step.
- **Renders.** `ui.render` draws on the CPU (no GPU needed); `view.snapshot`
  renders the open map offscreen through the viewport's own renderer
  (wgpu, no window) and answers `NO_GPU` on a machine without an adapter.

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

`crates/studio/src/api/seams.rs`. A tool implements `api::Provider`:

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
  tag's edits and for `tag.read {"raw": true}`.
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
- **Player models (planned, not started).** `model.list` (bipeds, `hlmt`
  model variants), `model.read` (regions, permutations, change colours),
  `model.render` (a variant with colours from a camera, to a PNG),
  `model.colors` and `model.emblem` (a player's colours and emblem).
- **Customisation (later).** `custom.list` (armour permutations per
  region and what unlocks them), `custom.set` (pick one).

## Code

- `crates/studio-proto`: method names, error codes, the socket path,
  the short forms and the client (`h2 studio`, `h2-editor call`). No
  editor dependencies, so the `h2` CLI links it without wgpu.
- `crates/studio/src/api/mod.rs`: `METHODS` (every method's shape and
  example), `Api::call` (shape check, providers, built-ins),
  `Api::pump` (the window's between-frames answer).
- `api/server.rs`: the socket. `api/{docs,scenario,tags,variants,playlists,ui_screens,view}.rs`:
  the namespaces. `api/seams.rs`: providers; `api/tag_fields.rs` and
  `api/hud.rs` the Tags and HUD tools'.
- `app/commands.rs`: the actions added for the API and the window
  (`SetField`, `EditSelection`, `MovePlacement`, `EditPlaylist`,
  `UndoIn`), `PlaylistEdit`, and `App::perform`.

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
- the window's path and the API's make identical documents;
- growth refused with its reason, unsaved edits never dropped without
  `discard`, outputs refused in a Git checkout or over the source;
- tags listed, read (raw and through the registry), referenced and
  followed; `tag.set` on a tag through the Tags tool, diffed, undone and
  redone as the `tags` document; a tag too short for its layout refused;
- the HUD tool through the API on `hud_edit`'s invented map: read, check,
  move (and in another mode), set, diff, undo and redo as the `hud`
  document, render (refused in a Git checkout), save as and re-open; and
  the window's actions and the API's make identical patches;
- a registered provider answers a planned method;
- the window's loop answers a client between frames.

`studio-proto`'s tests cover the short forms and the wire round trip.
