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
| Move and turn a placement | `MovePlacement` (one step); `GestureSet {position, ypr}`, `GestureScale`, `EndGesture` (a drag: one step, a multi-selection riding along) | the viewport gizmo dispatches the gesture actions every frame of a drag | `scenario.move` |
| Add, duplicate, delete | `Add`, `DuplicateEntry`, `DeleteEntry` | outliner, toolbar, keys | `scenario.add`, `.duplicate`, `.delete`, `tag.block.*` |
| Select, frame | `Select`, `FocusSelected`, `FrameAll` | outliner, viewport, keys | `scenario.select`, `view.camera` |
| Navigate tag fields | `Navigate` | inspector links and breadcrumbs | `nav.go`, `nav.back`, `nav.forward` |
| Undo, redo | `Undo`, `Redo` (the tool shown), `UndoIn(tool)`, `RedoIn(tool)` | Edit menu, keys | `doc.undo`, `doc.redo` |
| Save | `SaveMapAs`, `SaveVariant`, `SaveHopper(As)`, `ExportVariant` | File menu, prompt | `doc.save`, `doc.save_as` |
| Variant edits | `OpenVariant`, `NewVariant`, `CopyBuiltin`, `SetVariantName`, `SetSetting`, `ImportVariant*`, `DeleteVariant` | the variant tool | `doc.open`, `variant.*` |
| Playlist edits | `EditPlaylist(PlaylistEdit)`, `SetEntryVariant` | the title and description fields, the table's row buttons, weights, Add, the unused slot's "Make a … playlist" buttons and "Remove playlist" apply the same `PlaylistEdit`s | `playlist.set`, `playlist.entry`, `playlist.entry_variant` |
| UI screens | `UiPick`, `UiPress` | the UI Screens tool | `ui.pick`, `ui.press` |

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
| `tag` | `list {group, name, query, path, limit, homes}`, `browse {map}`, `resources {tag}`, `export {tag, out, dependencies, resources}`, `import {path, tag, skip_missing}`, `read {tag, depth, elements, hex}`, `refs {tag}`, `follow {tag, ref}`, `set {tag, path, value}`, `block.add`, `block.remove`, `block.duplicate` |
| `scenario` | `blocks`, `list {block}`, `get`, `set {block, index, field, value}`, `add {block, like, position}`, `move {block, index, position, ypr}`, `delete`, `duplicate`, `select` |
| `variant` | `list`, `get`, `set {setting, value}`, `rename`, `copy`, `export`, `import`, `delete` |
| `playlist` | `list` (with `unused` per playlist and the `templates`), `get {playlist}`, `set {playlist, title, description, template, clear}` (`template`: make the slot a playlist of that kind, or give a playlist another kind; `clear: true`: make it unused; [live-matchmaking.md §6.10](../live/live-matchmaking.md)), `entry {playlist, op, entry, to, weight}`, `entry_variant` |
| `ui` | `screens`, `pick {screen, state \| retail}`, `press {input}`, `widgets`, `render {out}` |
| `pair` | `list`, `status`, `connect {socket}`, `disconnect`, `auto {enabled}` |
| `view` | `camera {yaw, pitch, distance, target, focus, all}`, `snapshot {out, size, camera}`, `stats` |
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
  the result short. Live handles and usage counts belong to later slices
  of the [click-to-edit design](click-to-edit.md).
- **Inspector navigation.** `scenario.select {"tag":"weap"}` makes
  a tag the inspector's selection and clears the placement selection.
  `nav.go {"tag":"hlmt","path":"render_model"}` focuses a tag and,
  optionally, one field by its registry path. It keeps the viewport
  selection. Both resolve tags in the open map. `nav.status`,
  `nav.back` and `nav.forward` take no params. All `nav.*` methods
  return `{selected, tag, path, breadcrumbs, can_back, can_forward}`.
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
  `tag.read {"raw": true}` and `tag.refs` still read the open map.
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
- **Placements.** `scenario.set` takes a number, `[x, y, z]`, or for a tag
  reference `{"id": "0x..."}` (or `null` for none). The scenario checks it
  first: read-only fields, integer ranges, finite floats, palette indices,
  tag groups. `scenario.add` and `.duplicate` are refused where the block
  is full and the map editor can't grow it safely (the same
  `GROWTH_REFUSAL` the window shows; `data` has the count and capacity);
  `.delete` where something refers to entries by index. A new entry goes
  to `position`, else the camera focus, in one undo step.
- **Pairing.** The editor pairs with a running game (the Game panel's
  Pair; [editor-guide.md](editor-guide.md), "Play from the editor").
  `pair.list` looks at the games now: the sockets in the agents directory
  where `h2 play` listens by default (`agent_proto::discover`), plus the
  default and `$H2_AGENT` sockets and Play's own. Each entry has its
  `socket`, `pid`, `map`, `phase`, `paused`, `tick`, whether its map is the
  open one (`matches`: `same`, `different`, `unknown`) and whether the
  editor is `paired` with it; a socket nothing answers on is removed from
  the agents directory. It works headless. `pair.status`, `pair.connect`
  and `pair.disconnect` act on the window's pairing (`api/pair.rs`, through
  `play::pair::Bridge` on the app), so they answer REFUSED with no window,
  and `pair.connect` answers before the window has paired: poll
  `pair.status` for `state: "connected"`. `pair.connect` with no `socket`
  takes the one game running, and is refused with none or several.
  `pair.auto {enabled}` reads or sets auto-pair, on by default.
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
  the viewport shows the grey world (no art built). What each field means
  is in [editor.md](editor.md), "What the viewport shows". To check a
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
  the namespaces. `api/seams.rs`: providers; `api/tag_fields.rs`,
  `api/tag_io.rs` and `api/hud.rs` the Tags and HUD tools'.
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
- `view.stats` of an invented scene reports its BSP's lighting, its
  triangles by blend, its materials and its sky's sun and fog
  (`api::view::tests`);
- the window's loop answers a client between frames.

`studio-proto`'s tests cover the short forms and the wire round trip.
