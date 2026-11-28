# The editor's agent API: models and customisation

Status: Built. Code: `crates/apps/studio/src/api`, `crates/apps/studio-proto/src`. Tests: in-source unit tests.
Switch: `H2_STUDIO` (the socket path).

`model.*`, model poses, and the planned `model.colors`, `model.emblem` and `custom.*`. The index is [agent-api.md](agent-api.md).

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
  (`content_kit::inject::add_pogo`, which the `pogo` package's build equals,
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
- `model.collision_from_render {render, target?, materials?}`: replace
  a collision BSP with polygons made from the current mesh of a render
  section. Open both with `geometry.open` and pass their target ids
  (source `mode_section`, `cluster` or `instance_def`; destination
  `coll_bsp`, defaults to the active target). Choose meshes in the same
  coordinate space. The BSP keeps its existing node binding. Each entry
  in `materials` is the collision material index for the render material
  at that position. An omitted or empty array assigns every face
  collision material 0. The call answers `{target, verts, faces}` and
  creates one map undo step. An unopened target is `NO_DOCUMENT`.
  Unusable geometry, missing material mappings and compiler failures are
  `REFUSED` and leave the mesh unchanged. The edit saves with the map
  when Allow unverified geometry writes is on.
- `model.physics.shapes {tag}` reads a local `phmo` tag from the open
  map with unsaved edits applied. The result has `spheres`, `pills`,
  `boxes` and `polyhedra` arrays in tag block order. Entries give the
  editable fields for each shape. Polyhedra also report their vertices,
  plane equations and AABB centre and half extents.
- `model.physics.set {tag, kind, index, field, value, vertex?}` edits one
  shape in place. Use `sphere`, `pill`, `box` or `polyhedron` for
  `kind` and the zero-based array index. Fields are `radius` for
  primitives, `half_extents` for boxes, `bottom` and `top` for pills,
  `translation` and `rotation` for spheres and boxes, and `vertex` for
  polyhedra. Rotation takes three right-handed orthonormal rows. For a
  vertex, pass its zero-based `vertex` index and a position as `value`.
  The editor returns `REFUSED` for a shape under a MOPP or a vertex move
  that breaks the convex faces. A change saves with the map. Use
  `doc.undo` and `doc.redo` with `"doc": "tags"` to undo or redo it in
  one step. The result's `warning` flags stale rigid-body bounds and
  mass properties on a change and is null if the value matches.
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
  `crates/tools/content-kit`, not with `tag.set`). The codes:

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
on the model that uses it (slice M1 of rich-editors.md) and
lists a clip's channels as keys. The Animation window and the API share
the same editable documents in `app::anim`. See the animation editor
in [studio-redesign.md](studio-redesign.md) (AN1, AN3 and AN4). `tag` takes
the same form as `sound.*`. Document IDs last for the Studio session,
and edits live in memory.

- `anim.list {tag}`: the graph's animations, each
  `{index, name, type (base|overlay|replacement|unknown), frames,
  codecs: [static, animated] (the engine's codec names, null where a
  stream is absent), aim_grid, error}` - `aim_grid` marks codec 8, whose
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
  {rotation, translation, scale}, world}` - the local transform that
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
- `anim.save {doc, package?, mode?, class?, weapon_type?, action?}`
  writes an `animation` item for the Tags tool's current map. It uses
  the edit target by default and answers `NO_TARGET` when no package
  is specified and the target is unset. A document opened from a source clip replaces that clip
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

- `anim.retarget {from, to, map?, name?, from_model?, to_model?,
  preview?}` copies open document `from` onto the skeleton of the jmad
  tag `to` in the shown map as a new document. Nodes pair by unique name;
  `map` is `[{source, target}]` with a `null` target to drop a node, and
  wins over names. `from_model` and `to_model` name the model whose rest
  skeleton applies when several use a graph. The reply is `{doc, name,
  nodes, unmatched_source, unmatched_target, dropped_channels, notes}`;
  `nodes` has `{source, source_name, target, target_name}` for every
  source node. With `preview: true`, `doc` is null and nothing opens. A
  node outside either skeleton or a node mapped twice is `REFUSED`; a
  malformed `map` is `INVALID_PARAMS`.
- `anim.import {file, to?, clip?, map?, name?, to_model?, preview?}`
  reads a local `.gltf` or `.glb` and places one animation on the graph
  `to` (default: the selected jmad) by the same rules and reply. `clip` is
  an animation name or index and is required when the file has several;
  the `INVALID_PARAMS` error then lists them in `data.clips`. An
  unreadable or unsupported file is `REFUSED`; skipped channels are in
  `notes`.

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
the Pogo kit's invented map with the kit in it (`content_kit::fixture`, no
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
rest pose - the pose differs from the rest where the clip moves it and
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
`api/anim/transfer_tests.rs` covers `anim.retarget` and `anim.import`
on the invented graph and an invented glTF file.
