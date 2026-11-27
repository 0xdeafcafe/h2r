# The editor's agent API: methods

Status: Built. Code: `crates/apps/studio/src/api`, `crates/apps/studio-proto/src`. Tests: in-source unit tests.
Switch: `H2_STUDIO` (the socket path).

The method reference: every namespace, its params and what it answers, for tags, scenarios, sounds, shaders, curves and placements. The index is [agent-api.md](agent-api.md).

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
| `workspace` | `status`, `extract {disc?, maps?, verify?, root?}`, `prune {dry_run?}` ([The tag workspace](agent-api-workspace.md#the-tag-workspace)); `import {map \| path, name}` (planned) |
| `project` | `list`, `read {project, closure?}`, `build {project}`: projects over the tree ([Projects and edit files](agent-api-workspace.md#projects-and-edit-files)) |
| `edits` | `list`, `check {project}`, `set {file, tag, path, value, project?}`: edit files over the tree ([Projects and edit files](agent-api-workspace.md#projects-and-edit-files)) |
| `doc` | `list`, `open {map \| path \| package \| variant \| playlists}` (`variant` is `{builtin}`, `{saved}`, `{new}` or `{script}`), `save`, `save_as {path}`, `diff`, `undo`, `redo`, `close` (`undo`, `redo` and `close` also take a tag tab's id) |
| `shell` | `tabs`, `open {tag?, map?, discard?}`, `focus {doc}`, `close {doc, discard?}`: the document tabs ([Document tabs](#document-tabs)) |
| `tag` | `list {group, name, query, path, limit, homes}`, `browse {map}`, `resources {tag}`, `export {tag, out, dependencies, resources}`, `import {path, tag, skip_missing}`, `read {tag, depth, elements, hex}`, `fields {tag, path?}`, `tree {root?, depth?}`, `refs {tag, limit?}`, `kinds {group?}`, `follow {tag, ref}`, `set {tag, path, value}`, `block.add`, `block.remove`, `block.duplicate` |
| `sound` | `read {tag, range?, permutation?, bins?}`, `play {tag, range?, permutation?, from_ms?}`, `stop`, `measure {tag \| clip \| path, range?, permutation?, class?, limit?}` ([Content checks](agent-api-media.md#content-checks)) |
| `scenario` | `blocks`, `list {block}`, `get`, `set {block, index, field, value}`, `add {block, like, position}` (or `add {block: "label", label, position, facing?, team?, phase?}`: a ground label, [AI ground](agent-api-extensions.md#ai-ground)), `move {block, index, position, ypr}`, `delete`, `duplicate`, `select` |
| `volume` | `list {kind?}` (short form `volumes [kind]`), `select {index?, focus?}`, `set {index, origin?, center?, forward?, up?, size?}`, `add {kill?, center?, origin?, forward?, up?, size?}`, `remove {kind?, index}` |
| `collision` | `surfaces {class?, bsp?, offset?, limit?}`, `at {origin, dir}` or `at {point, within?}` (with `select?`), `select {index?, focus?}`, `objects {block?, index?}` |
| `markers` | `list {kind?, layer?}`, `render {kind?, team?, area?, view?, size?, out?}`, `area.set {id, area}`, `area.convert {id}`: the game type markers of the open map, their areas, and the marker shapes drawn on a grid ([Game type markers](agent-api-extensions.md#game-type-markers)) |
| `nav` | `status`, `back`, `forward`, `go` (the inspector's history); `pathfinding {bsp?, offset?, limit?}`, `bots {wait?, polys?, links?, spots?, offset?, limit?}`, `pick {origin, dir}` or `pick {point, within?}` (with `layer?`, `shown?`, `select?`) or `pick {kind, index, focus?}` (the Navigation layers) |
| `ground` | `status {offset?, limit?}`, `build {wait?, offset?, limit?}`: the open map's AI ground ([AI ground](agent-api-extensions.md#ai-ground)) |
| `map` | `new {name, kind?, replace?}`: File > New map, a starter map built and opened ([New maps](agent-api-extensions.md#new-maps)); short form `new-map <name> [multiplayer\|campaign]` |
| `playtest` | `static {wait?}`: the open map's static checks and each built-in game type's readiness ([playtest.md](playtest.md#static-checks)); `run {preset?, game_types?, seeds?, minutes?, bots?, compare_to?}`, `status {run?}`, `result {run?, game_type?, measure?}`, `compare {a?, b?, game_type?}`, `cancel {run?}`: headless bot matches on the open map and their measures ([playtest.md](playtest.md#what-a-run-is)) |
| `variant` | `list`, `get` (with `script`), `set {setting \| option, value}`, `rename`, `copy`, `export`, `import`, `delete` |
| `playlist` | `list` (with `unused` per playlist and the `templates`), `get {playlist}`, `set {playlist, title, description, template, clear}` (`template`: make the slot a playlist of that kind, or give a playlist another kind; `clear: true`: make it unused; [live-matchmaking.md §6.10](../live/live-matchmaking.md)), `entry {playlist, op, entry, to, weight}`, `entry_variant` |
| `ui` | `palette {query, run?}`, `screens`, `pick {screen, state \| retail}`, `press {input}`, `widgets`, `render {out}`, `select_folder {folder \| root}`, `sheet {folder?, root?, states?, columns?, out?}`; the shown screen's edits, of our screens or a retail one: `select {id?}`, `move {id, dx, dy}`, `set_rect {id, rect}`, `set_text {id, text}`, `revert {id?}`, `undo`, `redo`, `edits`, `save`, `animate {phase, frames?, out?}` ([ui-builder.md](../game/ui-builder.md#editing-in-the-editor)); `check {screen?, state?, aspect?}` ([Content checks](agent-api-media.md#content-checks)) |
| `preview` | `target {doc?, target?}`: a document's preview switch and what Preview in game asks of the game ([The preview switch](agent-api-pairing.md#the-preview-switch)) |
| `pair` | `list`, `status`, `connect {socket?}`, `disconnect {leave?}`, `auto {enabled?}`, `play`, `stop`, `control {action, ticks?}`, `edit {enabled}`, `preview {what, target?, ...}`, `audio {seconds?, every_ms?, call?}`, `result {id?}` ([Previewing content in the paired game](agent-api-pairing.md#previewing-content-in-the-paired-game)) |
| `mirror` | `status`, `objects {kind?, near?, limit?, at_tick?}`, `pick {from? \| ray?}`, `aim_select {enabled?}`, `camera {mode?, target?}`, `player {to, facing?}`, `pending`, `apply {what, at?, peers?}` |
| `view` | `camera {yaw, pitch, distance, target, focus, all, fly_speed}`, `snapshot {out, size, camera}`, `stats`, `layers {set?, preset?, xray?}` |
| `script` | `list {filter?}`, `source {name?}`, `follow {name?, link}`, `uses {kind, name}`, `builtins {filter?, unnamed?}`, `doc {path}` ([rich-editors.md](rich-editors.md#scripts)) |
| `recording` | `list`, `read {name, limit?, offset?}`: the open map's recorded animations and each stream, typed ([rich-editors.md](rich-editors.md#recordings)) |
| `strings` | `list {tag?, language?, query?, limit?, offset?}`: every `unic` string of the open map by name and language ([rich-editors.md](rich-editors.md#strings)) |
| `rules` | `open {path? \| starter? \| source?, discard?}`, `get {text?}`, `set {source}`, `save {path?, reload?}`, `format`, `reload {mode?}`: the game type script editor ([editor-guide.md](editor-guide.md#game-type-scripts)) |
| `hud` | `list {map \| package}`, `read {hud, state}`, `set {hud, widget, field, value, merge}`, `move {hud, widget, x, y, mode, merge}`, `render {hud, state, out}`, `check {hud, state}`, `save_as {path}` |
| `geometry` | `list {model?}`, `open {target}`, `close {target?}`, `read {target?}`, `check {target?}` |
| `check` | `run {what?, target?}`: every content check in one list ([Content checks](agent-api-media.md#content-checks)) |
| `image` | `generate {prompt, references, bitmap?, image?, size?, alpha?, out?, from?}`, `result {id?}`, `apply {id}`, `accept {id, save?}`, `discard {id}`: a picture made for a bitmap image from a prompt and the references the user picks ([Generating images](agent-api-media.md#generating-images)) |
| `mesh` | `mode {mode}`, `select {ids, op \| all \| none \| invert}`, `transform {translate \| matrix, pivot?, merge?}`, `ops`, `apply {ops}`; each with `target?` |
| `model` | `list {group?}`, `read {tag, variant?}`, `render {tag, view?, variant?, character?, biped?, hands?, size?, camera?, out?}`, `check {tag, variant?}`, each with `map?` and `pogo?` ([Models and customisation](agent-api-models.md#models-and-customisation)); `colors`, `emblem` (planned) |
| `anim` | `list {tag}`, `pose {tag, animation, frame?, overlay?, model?, size?, out?}` ([Model poses](agent-api-models.md#model-poses)) |
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
  `script.uses {kind: "recording"}` gives the scripts that name the
  recording (a value of type `cutscene_recording`, by name or index).
  `script.builtins` gives each builtin's `name` and `from` (`map`,
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
  edits; `{open: false}` when none is open. It needs no map. It writes
  nothing into the map ([editor-guide.md, "Scripts in VS Code and
  Zed"](editor-guide.md#scripts-in-vs-code-and-zed)).
  `script.compile {source?, working?, name?, apply?}` compiles source into
  the map's script tables (`hs::compile`): a definition named as one the
  map has replaces it, any other is added, and only those definitions'
  nodes change. `source` is the text; `working: true` takes the working
  copy's changed and added definitions (it is `REFUSED` with no working
  copy or no edits; give one of the two, else `INVALID_PARAMS`); `name`
  requires the source to define it (`INVALID_PARAMS` otherwise). Without
  `apply` nothing changes and the answer says what would. Problems are
  an answer, not an error: `{ok: false, applied: false, errors: [{line,
  column, span: [start, end], code, text}]}` (a compile past the node
  limit has both counts in its `text`). Otherwise `{ok: true, applied,
  definitions: [{name, item}], nodes, freed, reused, grown, new_strings,
  grows, free_nodes, depth: {undo, redo}}`: nodes the definitions use,
  slots freed and reused, slots added, string bytes added, whether a
  table outgrows the map's (`grows`) and the free nodes left. With
  `apply` the compile is one undo step, the Scripts tab and the unsaved
  count follow, and a table that grows is `REFUSED` while **Allow growing
  full blocks** is off. With no executable it is `REFUSED` with the
  tool's sentence, and with no map `NO_DOCUMENT`. Names the map's scripts
  do not already use do not resolve (`name.unresolved`).
  `script.undo {redo?}` takes back (or redoes) the last applied compile:
  `{changed, free_nodes, depth}`.
  `script.eval {expression, type?}` sends an expression to the paired
  game's `hs.eval` ([runtime/agent-api.md](../runtime/agent-api.md#hseval))
  and returns `{ok: true, type, type_index, value, bits}`. Problems in the
  text are an answer, as in `script.compile`: `{ok: false, errors: [{line,
  column, code, text}], error}`, counted in the expression; so is the
  evaluator's refusal, `{ok: false, error}`. `REFUSED` without a paired
  game, with the game paused (the call answers on its next tick) or with a
  game that does not list `hs.eval`. The Scripts tab's expression box sends
  the same call.
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
- **Suggested map variants.** `variant.maps` lists this machine's map
  variants a game variant may suggest (`{map, name, base, variant,
  source}`, by map and name). `variant.export {path, maps: [variant id]}`
  writes a version-2 file whose `maps` are those, in that order, and
  answers how many; an id not on the list is `INVALID_PARAMS`, and more than
  eight, or a record that names a script, is refused by name. Without `maps` the file is
  the version-1 text.
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
  `rules.save` and `doc.save {doc: "rules"}` write a `rules` item in the
  edit target and answer `NO_TARGET` when it is unset. A `path` in
  `rules.save` exports a copy. For a package save, `reload` overrides
  the editor's Reload on save checkbox.
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
    row says why); a colour row (a colour `view=` word, or three floats
    whose name says colour) carries `colour: {form, by, hex, rgba,
    intensity?}`: the stored form (`rgb`, `argb`, `rgb8`, `argb8`,
    `xrgb8`, `hdr_rgb`), `by` (`view` or `name`), the colour as
    `#aarrggbb`, its channels in 0 to 1, and an `hdr_rgb` colour's
    intensity (the hex and channels are at intensity 1);
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

  A colour field's `tag.set` value may also be `"#aarrggbb"`,
  `"#rrggbb"` or `{rgba: [r, g, b, a], intensity?}`, converted to the
  field's form as the colour picker writes it (a form without alpha drops
  it, a packed form without alpha keeps its top byte, an `hdr_rgb` field
  keeps its intensity unless one is given); channels outside 0 to 1, an
  intensity below 0 or on another form are `INVALID_PARAMS`.
  `tag.colours {tag}` lists every colour row of the tag, every element of
  every list walked as `tag.find` walks: `{colours: [{path, label, form,
  by, hex, rgba, intensity?}], truncated}`
  ([rich-editors.md](rich-editors.md), "Colours"; `api/colours.rs`).

  An offset path in `tag.set` addresses the typed field that starts
  there. Its row gives the width accepted for an integer or enum value.
  A block's value is its element count. Scalar values follow
  `tag.read`. The rows use the open or browsed tag document and include
  its pending edits (`api/tag_fields.rs`).
- **Shaders.** `shader.read {tag, time?}` reads a `shad` as the Shader tab shows
  it ([rich-editors.md](rich-editors.md), "Shaders"; `api/shader.rs`):
  the template, the level of detail shown (0) and how many there are, the
  bitmap, colour and vector slots of the compiled postprocess (each with
  its `tag.set` path, value, and `used_by`: the layer, pass,
  implementation and register or stage the template's binding puts it
  in), the animated properties (each with the `path` of its function
  curve, which `function.read`, `function.set` and `function.sample`
  take, the kinds of target it drives, a `refusal` when it drives a
  target the layer plan does not animate, and its `value` at `time` on
  the preview clock or `no_value` with the reason, such as a missing
  object function, an invalid period or a wave requiring an engine table), `time` and `clock_length`,
  the passes per layer, `names_known` (false until the template's parameter names are
  read), `read_only` and the bindings that did not read (`refusals`).
  `shader.set {tag, slot: {kind, index}, value}` writes one slot: the
  `tag.set` at the slot's path with the same value, one undo step,
  refused as `tag.set` refuses; a slot past the shader's is
  `INVALID_PARAMS`. `shader.preview {tag, shape?, object?, size?, time?, out?}`
  draws the shader alone on a sphere (the default), a flat square
  (`quad`) or an object tag's model, unlit, to a PNG outside Git; it
  answers `passes_drawn`, each left-out layer in `refusals` with its pass
  count and reason, `base` when the base look does not draw, and
  `covered`. `time` (0 to 86400 seconds, default 0) is the preview
  clock: the answer repeats it and lists each animated property's value
  there in `animation`. The drawn layer passes use the pixel constants,
  vertex constants and vertex defaults sampled at that time. A bitmap
  frame change or a missing texture-transform input appears in `refusals`.
  With no renderer `png` is null and the counts
  still answer.
  `shader.code {tag, pass?, implementation?}` reads one pass's programs
  as text (`native_view::code`, the Shader tab's Code section): `pass`
  counts the passes across the layers of the first level of detail in
  layer order (default 0), `implementation` picks one of the pass's
  (default 0). The answer gives `passes`, `layer`, `pass_in_layer`,
  `pass_tag`, `implementations`, and for `vertex` (the world
  classification, `classification: "world"`) and `pixel` each
  `{listing, wgsl, warnings}`: the disassembly, the WGSL the translator
  makes of it and the translator's warnings. A state program has its
  listing, a null `wgsl` and the refusal in `warnings`; a program that
  does not decode has a null `listing` too. A pass or implementation the
  shader lacks, or a tag that is not a `shad`, is `REFUSED`. It reads
  with the document's edits and writes nothing.
  `shader.override {tag, pass?, implementation?, text?}` reads or edits the
  WGSL of a pass's program (slice S5; `app::shader_override`): `base` is the
  vertex and pixel programs linked into one module (`vs_main`, `fs_main`;
  default options), `text` the draft (a string sets it, null discards it),
  `check` the draft as the game parses it (`{ok: true}`, or `{ok: false,
  error, line, column}`; null with no draft), `edited` whether it differs
  from `base` and `pushed` the overrides sent from it, each
  `{program, file, state, why}` with `state` one of `queued`, `sent`,
  `loaded`, `refused` and `reverting`. `shader.push {tag, pass?,
  implementation?, program, dir?}` queues the draft as the override of the
  game program `program` (16 hex digits from `render.programs`): a missing
  or unparsable draft is `REFUSED` with its line; the link then writes
  `<program>_studio.wgsl` in `dir` (default the game's default override
  directory on this machine) and calls `render.reload {dir}` on the paired
  game. Not paired, or a game without `render.reload`, ends as `refused` and
  writes no file. `shader.revert {tag, pass?, implementation?, program?}`
  queues deleting the pass's pushed files (or the one `program`) and the
  same reload; a program with no override pushed from here is `REFUSED`.
  An override changes the game's renderer only; nothing is written into a
  map.
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
  `REFUSED`. `function.colours {tag, path, samples?}` reads the blob as a
  shader colour property (the engine's reader at `0x0013bc00`,
  `Function::output_colour` and `colour_stops`): `mode` (`grey`,
  `single`, `two`, `three`, `four`, `white`, from the flags' high
  nibble), `stops` as `#aarrggbb`, and `gradient`, `samples` (2 to 256,
  default 16) rows of `{at, colour}` over the input 0 to 1. The blob
  does not say it is a colour, so the caller picks the field.
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
  `rows` the element's `tag.fields` rows the panel shows - with a
  search or **Changed only** on, the hit list's rows, each a
  `tag.fields` row of the field it found - `unnamed_hidden` the
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
- **Names.** `tag.names {group?}` returns `{groups: [{group, title,
  named, unnamed, described, with_unit, with_effect}]}`, one line per tag
  group (`api/tag_names.rs`): the fields the definitions name, the typed
  or unknown fields they do not (padding is not counted), and how many
  fields have a description, a `unit=` and an `applies=`. A block's
  element is counted once. With `group` (a group code) the one line also
  has `unnamed_fields: [{path, type, size}]`, the largest first, with each
  path from the root (`+0x14`, `parts[].+0x6`). A code that is not a
  group is refused with `INVALID_PARAMS`.
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
- **Subject.** `tag.open {tag}` opens the tag in the editor its kind
  gets (as `shell.open` does) and makes it the subject
  ([live-editors.md](live-editors.md), "The subject"). It returns `tag`,
  `group`, `name`, `kind`, `editor`, `preview`, `front` and `subject`,
  the place the subject has in the scene Studio's play last saw:
  `{rule, show, scene, at, yaw, note}`. `scene` is `level` (an own game
  runs the open level; `at` is the viewport camera's focus), `stage` (the
  own game runs the content stage; `at` is its flat bay's subject pad and
  `yaw` faces the spawn) or `nowhere`; `show` is `object`, `swatch`,
  `loop`, `play` or `nothing`; `note` says what is missing. The own game
  stands an object subject at its next frame and plays a sound subject.
  `tag.subject` reads what the own game holds: `own`, `tag`, `place`,
  `object` (the standing object's id), `owed` (`respawn`, `retrigger`:
  follow-ups waiting for the write's batch), `unread` (fields written
  that no definition says when the game reads), `refusal` (why no object
  stands), `failure` (the last call the game refused), `busy` (the call
  in flight) and `calls` (the spawns, deletes, respawns and retriggers
  made). With no own game `own` is false and the rest is the registry's.
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
  zero-length). With `curve` (one value 0 to 1 per tick, exactly the
  stored count), `from_sound` (`{gain?, smoothing?}`: the curve of the
  sound's own loudness), `revert` or `undo` (the last redraw of the shown
  map; no `tag` needed; the reply's `undone` names it or is null)
  `sound.mouth` redraws the curve in place: one undo step, shown by every
  read at once (`edited` is true while it differs from the file), saved
  with the map as a patch of those bytes in the raw data, the checksum
  aside. `written` is `{ticks, changed, edited}`. It is `REFUSED`, by
  name, for a sound whose data is in a shared cache, a permutation that
  is a split chain, an unvoiced one, a curve of another length and a
  browsed map; `INVALID_PARAMS` for a value outside 0 to 1 or more than
  one of `curve`, `from_sound` and `revert`. The window's Sound window
  draws the same curve under the waveform, draws with a drag on it and
  calls the same `App::sound_*` methods. `sound.face`
  is the face preview: the weight the curve gives `at_ms` into a
  permutation (the byte of that 30 Hz tick over 255, 0 past the end;
  the playhead by default while that permutation plays), or a `weight`
  given in place of the sound, and with `on` (an object, a biped) the
  object posed with its graph's `talk` overlay at that weight over its
  `idle`, answered as `anim.pose` answers and drawn to a PNG outside
  Git. `overlay` names another of the graph's animations in place of
  `talk`; a graph with no `talk` at `combat:any:any` or `any:any:any`
  is `REFUSED` by name. The
  executable comes from the source: the disc's `default.xbe`, or the one
  beside a loose map or folder (`H2_XBE` names another).
- **Placements.** `scenario.set` takes a number, `[x, y, z]`, or for a tag
  reference `{"id": "0x..."}` (or `null` for none). The scenario checks it
  first: read-only fields, integer ranges, finite floats, palette indices,
  tag groups. `scenario.add` and `.duplicate` are refused where the block
