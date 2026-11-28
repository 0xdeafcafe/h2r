# The editor's agent API: extension points

Status: Built. Code: `crates/apps/studio/src/api`, `crates/apps/studio-proto/src`. Tests: in-source unit tests.
Switch: `H2_STUDIO` (the socket path).

The tools that register providers behind the API: tags, strings, recordings, scripts, curves, colours, shaders, UI edits, HUDs, modeling, textures, AI ground, siege layouts, new maps, game type markers and block-out pieces. The index is [agent-api.md](agent-api.md).

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
  `tag.kinds` (over `app/kinds.rs`), `tag.open` and `tag.subject` (over
  `app/subject.rs` and `play/subject.rs`).
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
- **Recordings (rich editors K1).** `api/recordings.rs` registers
  `Recordings` for `recording.list` and `recording.read`, over
  `hs::recording` and `app/recordings.rs`; both are reads and need a
  map (`NO_DOCUMENT` without one). `recording.list` answers
  `{recordings}`, one row per element of the scenario's recorded
  animations: `name`, `ticks`, `version`, `control_version`,
  `stream_bytes`, `events` (the count, `null` when the stream does not
  read, with `problem` saying why and at which offset) and `played_by`
  (`[{name, item, count}]` of the scripts that name it, `null` when the
  scripts do not read, with `played_by_unavailable` giving the reason).
  An empty block answers `note: "This map has no recordings."`.
  `recording.read {name}` (case-insensitive) answers the stream as
  Halo 2's player reads it: `initial.fields` (the unit control state the
  stream starts from, by field) and `initial.trailer` (the twelve bytes
  copied as they are, hex), then `events`: `tick` (since the start),
  `at` (the header's offset in the stream), `delay`, `kind`, `name`,
  `fields` and `payload` (hex). `limit` (default 1000, at most 20000)
  and `offset` page the events and `truncated` says more follow;
  `walked_ticks` is the stream's own length, `ticks` the element's. A
  stream that does not read, or whose version is not 4, is `REFUSED`
  with the reason; an unknown name is `INVALID_PARAMS`.
  **Playing (rich editors K2).** `recording.play {name, unit?, mode?}`
  asks the paired game to play the recording on `unit` (`player.<n>`,
  default `player.0`; `mode` `play`, `delete` or `hover`) through the
  game's [`recording.play`](../runtime/agent-api-input.md#recordingplay), on a
  connection of its own, and starts the path: the unit's position from
  each streamed tick (`play::recording_path`), which the Scene viewport
  draws. It is `REFUSED` with no pairing, a paused game, a game that
  does not list the method, or the game's own refusal (its reason in
  the message); an unknown name, a unit that is no `player.<n>` and a
  bad mode are `INVALID_PARAMS`. `recording.path {clear?, limit?}` reads
  the path (`recording`, `player`, `count`, the last `limit` `points`,
  `truncated`) and with `clear` drops it.
  **Writing (rich editors K3).** `recording.write {name, initial?,
  trailer?, events?}` edits one stream, all or nothing, as one undo
  step: `initial` sets unit control fields by the names `recording.read`
  gives (integers, or an array of reals), `trailer` the twelve bytes
  (hex), and `events` is a list applied in order of `{op: "set", index,
  delay?, payload?}` (move the event, the later ones keep their ticks,
  and/or replace its payload), `{op: "insert", index, kind, delay?,
  payload?|fields?}` (before event `index`, the end event included) and
  `{op: "remove", index}`; `fields` names a payload's values as
  `recording.read` does (`value`, or `a` and `b`). It answers `{name,
  ticks, stream_bytes, events, grows}`; `grows` says the stream is longer
  than the source's, which saves as an appended copy and needs **Allow
  growing full blocks** (`REFUSED` otherwise, as it is for a stream that
  does not read or is not version 4). An edit that does not fit (an event
  that is not there, a value past its width, the end event removed) is
  `INVALID_PARAMS` with the number of the failing edit. The result is an
  unsaved change of the map (`session.status`). `recording.remove {name}`
  answers `{removed, recordings}` and is `REFUSED` while a script plays
  the recording or a later one by its number, and when the scripts do not
  read. `recording.undo {redo?}` answers `{changed}`.
- **Scripts (rich editors H1).** `api/scripts.rs` registers `Scripts`
  for `script.*`, over `app/scripts.rs`'s `Loaded`; `script.source`,
  `script.follow` and `script.doc` are `Action::Script` through
  `App::perform`.
- **Curves (rich editors F1).** `api/curves.rs` registers `Curves` for
  `function.read`, `function.set`, `function.sample` and
  `function.colours`, over
  `tags::effects::function` (`Set`, `Function::set_values`,
  `Function::write_set`) and the curve editor's own helpers
  (`fields/curve.rs`); `function.set` is a `TagAction::Write` of the
  same length at the data's start through `App::perform`, or, for a
  type change, a `TagAction::ResizeData` (rich editors F2), one undo
  step, exactly as the dialog's drags and type picker write.
- **Colours (rich editors C1).** `api/colours.rs` registers `Colours`
  for `tag.colours`, over the field panel's rows (`fields::find`) and
  the colour picker's helpers (`fields/colour.rs`); a colour's `tag.set`
  is `app::tag_api::set_field` with `fields::colour::from_json`, a
  `TagAction::Write` of the field's own length, as the dialog writes.
- **Shaders (rich editors S1, S2).** `api/shader.rs` registers `Shader` for
  `shader.read`, `shader.set`, `shader.preview`, `shader.code`,
  `shader.override`, `shader.push` and `shader.revert` (S5; the link sends
  the queued push in `play::send_shader_overrides`), over
  `app/shader.rs` (the Shader tab's read, on the shown map with the
  document's edits), `native_view::preview::material_at` (the preview clock),
  `native_view::animate` (the properties' values at a time) and
  `native_view::code::pass_code` (S4); `shader.set` is `tag.set` at the
  slot's path (`SlotKind::path`) through `tag_fields::perform`, one undo
  step.
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
  definitions, every `coll` BSP (`coll_bsp:…`) and, with `model`, a render
  model's sections as target ids `kind:tag:index` (`cluster:e1000004:0`); `geometry.open` enters edit
  mode on one, after which a `target` param may be left out.
  `geometry.read` gives the mesh (positions by vertex id, faces, the
  selection); `mesh.*` are `Action::Model(ModelAction::Apply)`, one undo
  step each (`mesh.transform` with a `merge` key joins a drag), and
  `mesh.apply` replays a `mesh.ops` log as one step. `mesh.sculpt
  {brush, centre | points, radius, strength?, falloff?, height?, axis?,
  detail?}` is one brush stroke (`raise`, `lower`, `smooth`, `flatten`,
  `cut`) as one step and one `sculpt` op: it subdivides where the brush
  needs vertices, rewrites the target's collision with it (a `coll`
  BSP; a cluster's level collision, portals and pathfinding), and
  answers `class`, `moved`, `verts` and `faces`; a stroke the kernel or
  the rebuild refuses is `REFUSED` with the reason, and nothing is kept
  (docs/editor/modeling.md, "Sculpting"). While a target is in
  edit mode, `doc.undo` and `doc.redo` with `"doc": "map"` are its undo.
  `geometry.check` gives each open target's write class and
  `save_refusal`, why a save would be refused (Allow unverified geometry
  writes off, `allow_unverified`, or an edit that changes a count).
  `geometry.lower_lods {target?, on?}` is **Use for lower LODs** on an
  open model section (`Action::Model(ModelAction::UseForLowerLods)`, one
  undo step; `on` defaults to true) and answers `changed`; any other
  target is refused. `model.collision_from_render` runs Modeling's
  **From render** action and records a replayable `replace_polygons` op.
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
  65,536 pixels); `bitmap.paint`, `bitmap.fill`, `bitmap.adjust`
  (invert, levels, hsv), `bitmap.channel_copy`, `bitmap.turn` and
  `bitmap.import` (a PNG, resampled to the rectangle) are
  `Action::Bitmap`, one undo step each; `bitmap.export
  {bitmap, image?, mip?, out}` writes a PNG and is refused inside a Git
  checkout. `bitmap.paint {bitmap, image?, points, color, size, ...}`
  stamps at every point and at most `spacing` apart between them, so two
  points are a line; with `source` ([x, y]) it is a clone stroke, each
  stamp the pixels at the offset from `source` to the first point, read
  from the layer `from` (`{image?, face?, slice?}`; the layer painted
  without it) as it was when the stroke began, and `color` is not
  needed. `bitmap.channel_copy {bitmap, image?, from, to, rect?}` writes
  a channel (`r`, `g`, `b`, `a`, `luma`) into the channels `to` (letters
  of rgba); `bitmap.turn {bitmap, image?, by, rect?}` is
  `flip_horizontal`, `flip_vertical`, `quarter`, `half` or
  `three_quarters` of the rectangle, in place (a quarter turn needs a
  square). An edit that changes no pixel is no undo step. A bitmap is
  named by hex tag id, path, `\key` path tail or a
  unique substring. `bitmap.reshape {bitmap, image?, width?, height?,
  format?}` and `bitmap.image.resize {bitmap, image?, size? | width?,
  height?, format?}` ask for a new size and/or format (`dxt1`,
  `argb8888`, ...) for an image, or `auto` (the format it has when that
  holds the painted alpha, else `dxt1` or `dxt5`, or a linear format at a
  size DXT refuses), refused at once when a save would refuse it;
  `bitmap.image.add {bitmap, from?, size?, format?}` appends an image
  after the bitmap's last, its record copied from image `from` (default
  0; a 2D image) at `size` and `format` or the source's, its pixels
  transparent black until painted, imported or generated into; it
  answers the new index;
  `bitmap.sequence.add {bitmap, first, count?, label?}` appends a
  sequence entry over `count` images from `first`, labelled `label`,
  shaped like the bitmap's first sequence;
  `bitmap.sequence.set {bitmap, sequence, label?, first?, count?}`
  changes a sequence (its own or an added one), keeping its sprites;
  `bitmap.sprites {bitmap, sequence?}` lists each sequence's sprites
  (`image`, `left`, `right`, `top`, `bottom` as fractions of the image,
  `registration` `[x, y]`); `bitmap.sprite.add {bitmap, sequence, image?,
  bounds?, registration?}` appends one (a copy of the last, or over all of
  the first image), `bitmap.sprite.set {bitmap, sequence, sprite, ...}`
  changes one and `bitmap.sprite.remove {bitmap, sequence, sprite}` drops
  one; an inverted or non-finite bound, an image the bitmap lacks or a
  sprite that is not there is refused and changes nothing;
  each of these is one undo step, and any edit to a sequence makes the
  bitmap `grown` (a save moves the tag);
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
  that doc (a reshape, an added image, an added sequence and a sequence or sprite
  edit are one undo step each). docs/editor/modeling.md, "Texture editing" and "Interfaces".
- **Models.** `api/models.rs`: `model.list`, `model.read`,
  `model.render` and `model.check`, read-only over a map of their own
  (no actions, no undo); see "Models and customisation" below.
  `model.colors`, `model.emblem` and `custom.*` are planned there.
  `api/geometry/physics.rs` serves the open map’s `model.physics.*`.
- **Image generation.** `api/image_gen.rs` registers `ImageGenApi`:
  `image.generate`, `image.result`, `image.apply`, `image.accept` and
  `image.discard` over the jobs on the app (`App::imagegen`, shared with
  the window's Generate window). Apply, accept and discard run the
  commands `Action::Image` runs, which write through the Textures tool's
  `BitmapAction`s ([Generating images](agent-api-media.md#generating-images)).

## AI ground

`ground.*` (`api/ground.rs`, a provider over `app/ground.rs`) is the agent's
view of the AI ground of the open map
([game-types-without-carriers.md](../game/game-types-without-carriers.md),
"In Studio"): the labels placed on it, **Build AI ground** and what the
build made. Nothing builds at open or while placing.

- **Labels.** `scenario.add {block: "label", label, position?, facing?,
  team?, phase?}` places a ground label (`ff_drop`, `ff_base`, `ff_cover`,
  `siege_site`, `siege_goal` or `siege_line`) as one netgame flag of a
  type the game reads no marker for, with the label attached, in one undo
  step; `facing` is radians, `team` 0 (red) or 1 (blue; a base and a siege
  line read it), `phase` a siege line's. It answers `{index}`, the flag's.
  The Scene tab's **AI and objectives** group and its **Place new with the
  mouse** place by the same action (`Action::PlaceLabelAt`), so a label
  placed in the window and one placed here are one document. The label
  belongs to its flag, as an area does, so a swap-removal moves it with the
  flag and an undo brings it back.
- `ground.status {offset?, limit?}` answers `{state, stale, job, error,
  labels, built, shown, matching, sectors}`. `state` is `no_map`, `never`
  (not built for this map), `building`, `current`, `stale` or `failed`;
  `stale` says why: `labels` (one was placed, moved or removed),
  `collision` (a placed solid object or a Modeling write changed),
  `recipe` or `kit_version`. A move of a spawn, a rename or a script edit
  leaves the ground current. `labels` counts the placed ones by label and
  in `total`. `built` is `{zones, squads, sectors, links, problems,
  costs, headroom: {tag_growth, available, fits}, bases}` for the last
  build, and `problems` are the labels the build could not honour
  (`{label, index, position, reason}`). `sectors` lists the built ground's
  pathfinding sectors as `nav.pathfinding` does (`matching` is how many),
  empty until a build.
- `ground.build {wait?, offset?, limit?}` builds over the open map as
  edited (`firefight_kit::ground::build`) on its own thread and answers
  `{job, status}` at once, or when it is done with `wait`. A build
  already running is `REFUSED`; a failed one is in `status.error` and in
  Problems. The built ground's pathfinding then draws in the Navigation
  layers and `nav.pathfinding` reads it in place of the map's own.

- `ground.save {name?}` writes the labels placed on the open map to the edit
  target (`NO_TARGET` with none) as a `map` item and its ground file
  (`maps/<name>/map.toml` and `maps/<name>/ground.yaml`;
  `workspace::GroundFile`) and answers `{package, name, item, file, labels,
  changed}`. The map is `onto` over the open map and served as `name`
  (default `<map>-ai`: letters, digits, `_` and `-`, not the open map's name,
  else `REFUSED`); the target's `edits` items for the open map take the new
  name as well, so the new map is the open one as edited. Saving the same
  labels again changes nothing (`changed` false) and keeps the one item.
  Nothing is built and the built ground is not saved: the game builds the
  ground over the map the item makes when it prepares its maps
  (`boot::map_ground`). `NO_DOCUMENT` with no map open.

The built ground lives in the open session; the labels are written by
`ground.save`.

The tests are `app/ground_tests.rs` (the window's placing and
`scenario.add` make one document, a build on the kit's invented court
lists its sectors, the stale rule, a failed build in Problems, the ground
file and `ground.save`) and
`scenario-edit/src/ground_tests.rs`.

## Siege layouts

`siege.layout {attack?, per_side?, barriers?, out?}` (`api/siege.rs`)
plans a Siege layout over the open map as edited, with the recipe
`h2 siege layout` runs (`forge_doc::siege_layout`;
[rules-engine.md](../game/rules-engine.md), "Siege layouts"): the attack
line from the attackers' CTF flag stand (`attack` `red`, the default, or
`blue`) to the defenders', the territories between them as the sites, two
a phase, the defenders' stand as the `siege_goal`, and each phase's
starting locations labelled `siege_attack` and `siege_defend` (`per_side`
a side, default 6). `barriers` are strings `<block>:<index>=<phase>`
(`crates:22=2`). It answers `{sites, goal, attack, defend, barriers,
phases, document, written}`, `document` the labels-only layout, and with
`out` writes it there. The open map is not changed. A bad `attack` or
barrier is `INVALID_PARAMS`, a map without both stands or a start for a
side is `REFUSED`, no map `NO_DOCUMENT`. Tests: `api/tests/siege_tests.rs`.

## New maps

`map.new {name, kind?, replace?}` (`api/new_map.rs`) is File > New map
([editor.md](editor.md), "New maps"). It builds a starter map over the open
Halo 2 source and writes it as a derived package, waits for the build, then
opens the map.

| Param | Value |
|---|---|
| `name` | the title; the map name is its lowercase form, `_` for spaces (`Big Yard` is `big_yard`) |
| `kind` | `multiplayer` (default) or `campaign` |
| `replace` | overwrite a package of that name |

It answers `{name, kind, dir, map_id, scenario, opened}`. Pass `map_id`
to the game's `maps.load`.

| Code | When |
|---|---|
| `INVALID_PARAMS` | `kind` is not one of the two |
| `REFUSED` | no source is open, the source lacks the base map, the name is not a map name, a build is running, the package exists without `replace`, the data directory is in a Git checkout |
| `FAILED` | the build failed; the message names the step and the tag |

Boot check for a multiplayer map, on the game's agent API:

```sh
h2 studio map.new '{"name":"Courtyard"}'      # answers map_id
h2 agent maps.load '{"map_id":"<map_id>"}'    # a Slayer game on it
```

Tests: `starter-map/src/tests.rs` (the template, the coverage of every
mode, each piece's collision, the placements on an invented map, the
campaign AI) and `studio-proto/src/tests.rs` (the short form).

## Game type markers

`markers.*` (`api/markers.rs`, a provider) is the agent's view of the
shapes Studio draws for a game type's parts
([gametype-markers.md](../game/gametype-markers.md)): player spawns, the
netgame flags the game reads (flag, bomb, ball, teleporter, territory,
hill), item spawners and trigger volumes, all from the shared
`game_markers` set through `scenario_edit::marker_view`.

- `markers.list {kind?, layer?}`: every marker of the open map, whatever
  the layers show, as `{id, kind, layer, team, icon, use, position, facing,
  label, number, area, area_from, members, bounds, lines, triangles}`,
  with `counts` by kind. `icon` names the drawing on the marker's plate
  (`respawn`, `crown`, ...; `null` for a volume) and `use` is a spawn's
  `respawn` or `start` (`null` for any other kind); `bounds`, `lines`
  and `triangles` are of the edit form with its plate facing the
  marker's front. `id` is the placement as `scenario.*` names it
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
- `markers.render {kind?, team?, form?, use?, area?, view?, size?, out?}`:
  the shapes drawn offscreen through the viewport's renderer on a floor
  grid, to a PNG under the cache directory's `studio/markers/`, never into
  a Git checkout (`REFUSED`). It needs no map and no disc. With no `kind`
  it is the contact sheet of every kind in red, blue and neutral (a kind
  of one fixed colour once); `team` (0 to 7) draws one. `form` is `edit`
  or `play` and draws that form alone; with none, the sheet has a block
  of rows a form, edit then play, and a kind with no play form (a spawn,
  an item spawner, a drop, a base, cover, a line) is an empty cell. The
  play form of a flag return is its capture plate. `use` (`respawn` or
  `start`) keeps one use of a spawn and is `INVALID_PARAMS` when no kind
  drawn is a spawn. Every plate turns about the upright to face the
  camera and grows past 8 world units of distance
  (`marker_view::mesh_turned`). `area` (`{radius, below?, above?}`,
  `{box: {half: [x, y]}, below?, above?}`, `{sphere: radius}` or
  `{path: [[x, y], ...], below?, above?}`, plain numbers; a sphere takes
  no `below` or `above`) replaces the area of a flag return, bomb goal,
  hill, territory, siege site or siege goal, and a refused area gives
  `Area::validate`'s sentence. A spawn is on the sheet twice, a respawn
  point and a starting location (`SpawnUse`). `view` is `angled`
  (default), `top` or `front`, +x to the right; `size` defaults to
  `[1280, 720]`. The answer is `{path, width, height, view, forms,
  covered, kinds}`: `covered` counts the pixels that are not the corner's
  background, and each of `kinds` has its `form`, `icon`, `cell`, its
  `center` in pixels and its mesh's `lines`, `triangles` and `bounds`
  (`null` for an empty cell), so a picture is read against the list.
  `NO_GPU` where there is no adapter.
- `markers.area.set {id, area}`: sets the area of the netgame flag `id`
  (`netgame_flags[3]`) from `{radius, below?, above?}`, `{box: {half: [x,
  y]}, below?, above?}` (half lengths along the flag's own axes),
  `{sphere: radius}` (no `below` or `above`) or `{path: [[x, y], ...],
  below?, above?}` in plain numbers, or clears it with `null`
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
Studio's viewport draws the same handles on the selected flag's area as
the game's Edit area does (gametype-markers.md, "Studio"): a box has one
on each corner and one on each side, on the flag's axes; a sphere one on
its equator and one on its pole. A corner drag writes one area op with a
box, merged into one undo step.
`api/tests/markers_tests.rs` runs the list over the invented cache (each
block's kind, team, icon and use, a hill of corners, the volumes), the
refusals, the sheet with every kind in both forms, a box and a sphere
through `markers.area.set`, and a snapshot whose spawn pixels all lie in the
spawns' projected boxes.

## Block-out pieces

`blockout.*` is the content render and check of the Forge block-out
shapes ([forge-blockout.md](../game/forge-blockout.md)); it is in
`api/blockout.rs`. Both methods need no map and no disc, and every
material is `Dev`. A definition is a canonical `name` (with its digest,
checked against a Dev definition's, or without it as
`<shape>_<x>x<y>x<z>_<param>`) or `shape`, `size` (`[x, y, z]` in
units) and `param` (steps, sides, voussoirs or seed); both refuse a
definition `PieceDef::validate` refuses with `INVALID_PARAMS`, the
sentence led by the param it is about (`shape`, `size`, `param`,
`name`).

- `blockout.render {name?, shape?, size?, param?, view?, pixels?, plates?,
  fit?, out?}`: the definition generated, built into the kit's render
  mesh in the dev view (`content_kit::blockout::mesh::build`) and drawn offscreen
  through the viewport's renderer to a PNG under the cache directory's
  `studio/blockout/` (never into a Git checkout: `REFUSED`). Each face is
  in its role's family colour (`dev_texture::Family`), its shell edges are
  dark lines, and each plate is drawn as the kit cuts it: a quad a run of
  texels of the glyph's cell in `dev_texture::font::font`. The ground
  is a floor with a grid every 64 units and a heavier line every 256,
  from the world's origin. `view` is `angled` (default: three-quarter,
  from the front and the +x side, from above), `top`, `front` or `side`.
  `pixels` is `[w, h]` (a piece 960 by 720, the sheet 3200 by 2000).
  `plates` false draws each plate's cells as the face's flat colour, so
  two renders differ only where a plate projects. The answer is `{path,
  width, height, view, name, shape, size, param, covered, piece_pixels,
  vertices, triangles, plates, grid_units}`: `covered` counts the pixels
  that are not the corner's colour, `piece_pixels` those that differ
  from the same picture with no piece, and each of `plates` has its
  `face`, `role`, `width` and `height` in units, the cell size `h`, its
  three rows of `text`, whether the viewer `visible` faces it, and its
  `center` and `bounds` in pixels. With no `name` or `shape`, the
  contact sheet: every structural piece of `forge_shapes::starter()` (a
  structural shape at each size the starter has), then three seeds of
  each natural shape, each family from the start of its own row. Each
  cell is its own picture at one camera angle, with the label under it:
  the canonical name without its digest, then the family and the digest.
  `fit` is `sheet` (default: one floor and one distance for every cell,
  so a size reads against the grid at the same scale in each) or `shape`
  (one scale a shape, so a small block is not a speck). The sheet's
  answer has the same fields summed, and `cells`: each cell's `name`,
  `family`, `shape`, `size`, `param`, `cell` (column, row), `rect` and
  `center` in pixels, `covered`, `piece_pixels`, `vertices`, `triangles`
  and `plates`. A sheet that leaves a cell under 96 by 132 pixels is
  `INVALID_PARAMS`. `NO_GPU` where there is no adapter.
- `blockout.describe {name?, shape?, size?, param?}`: the definition as
  text: `name`, `tag_path`, `digest`, `family` (`structural` or
  `natural`), `label` (the name a plate prints, in capitals with spaces),
  the shell's `faces`, `hulls`, `hull_points`, `volume` and `bounds` (world
  units), the dev mesh's `vertices` and `triangles` and its `parts` (a
  paint each: `tile:<role>` and `font`), and the `plates`: for each
  rectangular face the kit cuts one in, the face's `width` and `height`
  in units, its `role`, the cell `h` and the three rows of `text` that
  `dev_texture::label::layout` gives, with `metres` its second and third
  rows. `unplated` counts the faces with none (too small, not a rectangle,
  or a rock). The kit's `Cost` is not in it: it needs a map to lay the tags
  out in.

`api/tests/blockout_tests.rs` runs describe against `label::layout` and
the kit's mesh, the refusals, the sheet's items (every shape of every
family), and, where there is a GPU, a block whose two renders differ only
where its plates project and a sheet with every definition drawn.
