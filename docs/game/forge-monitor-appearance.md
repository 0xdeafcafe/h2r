# Forge monitor appearance

Status: Partly built (ML1 to ML8). The look, its profile keys, its refusals, the census, the offline agent methods, the presentation body (ML4: the host model list, a seat's draw, the model read from its campaign map in the mounted game, and the frame graph's bodies stage that draws it in each game view, `native_render::bodies`; the draw is the model's own base maps and the tint, with no change-colour areas and the model's default variant), the Appearance page's body (ML4b: its place, `forge_glue::appearance_body`, and a picture of the monitor in the right share of the page's model widget, `monitor_model::gpu::appearance_view`), the host light (ML5: in each game view's light list and the E3 light choice, last, and drawn by the native light passes over the structure), the session channel (ML7: the hello's declaration, the host's table, `look` and `pose` on the session's lanes, the refusals, the join reset, interpolation and staleness), film and Theater records (ML8: the record subtypes, the recorder and its checkpoint, the playback seats, the camera subjects and the `monitor_look_source` reason; the game writes the records, Theater's reader feeds them to the playback, the camera rig, the bar, `theater.state`, `theater.compat` and the film's detail page read it, and the playback's draws go to the same bodies stage), the kit's `Scene3d` preview widget and the look page's rows (ML6's `forge::look::page`: the seven rows, their values, locks, picks and focus previews) exist; none of the drawn parts has been seen in a running game. The look's page as a Forge screen (ML6: the screen kind, `Pick::on_focus`, the Forge menu row, the `Scene3d` draw and `forge.look.preview`), the lobby row (ML9, waits for F14) and a real body (ML10, waits for F10) do not.
Code: `crates/extensions/forge/src/look.rs` and `look/` (the look, `census`, `saved`), `crates/apps/cli/src/forge_looks.rs` (the disc reader behind `h2 forge looks`), `crates/extensions/forge/src/look/monitors.rs` (the seat's look and the agent methods' rules), `look/body.rs`, `look/appearance.rs` and `look/page.rs` (the drawn body in the world, on the Appearance page and the page's rows), `crates/rewrites/native-render/src/bodies.rs` and `frame_graph/bodies.rs` (the bodies stage), `crates/extensions/game/src/monitor_model.rs` and `monitor_model/gpu.rs` (the models read from the disc, the published set and the page's picture), `render_bridge::native_hud::Host::model_view`, `crates/rewrites/frame-scene/src/light.rs` (`Light::host_light`), `crates/rewrites/render-bridge/src/scene_step.rs` (`set_host_lights`), `crates/rewrites/native-render/src/light_choice.rs`, `crates/extensions/game/src/forge_hud.rs` (`Points`, the navpoints over other monitors), `crates/extensions/game/src/forge_controls/render.rs` (the parked biped and the monitor's view), `crates/extensions/forge/src/camera.rs` (the pose), `crates/extensions/game/src/ui_kit/view.rs` (`Kind::Scene3d`). Tests: `look/tests.rs`, `forge_looks.rs`, `forge_hud_tests.rs` and `crates/extensions/agent/src/forge/tests.rs`; `look/appearance.rs` (ML4b); `bodies.rs`, `monitor_model.rs` and `monitor_model/gpu.rs` (ML4, ML4b); `look/page.rs` (ML6's rows); the tag rule in `look/tests.rs`; `forge_glue.rs`, `frame-scene/src/builder.rs` and `light_choice.rs` (ML5); each later slice names its own.
Switch: `forge`, listed in [switches.md](../runtime/switches.md). No switch of its own.

What a player looks like while they fly as a monitor in a Forge session:
the model, its colours, its tint and its light, how the player picks
them, how the other machines in the session draw the same thing, and how
a film shows it. It sits on the two monitor bodies of
[forge-features.md](forge-features.md), "Monitor mode and bodies", and on
the session of [forge-design.md](forge-design.md). The rule that must
hold: **a look is presentation**. It is never read by the tick, never
written to guest memory and never part of the sim hash, so a machine
that draws it differently, or not at all, runs the same game. The one
exception is a real body (slice F10), whose look is a log entry and is
described apart.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).

## How it works

### What is drawn today

- Entering the monitor makes the player's chosen Forge body their unit
  and parks their biped, undamageable and left out of the render list.
  The view is the player's own, drawn from the body (**Code**,
  `forge_glue/editing/body.rs`, `forge_controls/render.rs`: `park`;
  [forge-design-slices.md](forge-design-slices.md), "The monitor body").
- The monitor is a `forge::camera::Camera`: a position, a yaw and a
  pitch. It is published to the input bridge and to `forge.status`
  (**Code**, `forge_controls.rs`: `publish`, `eye`).
- The only things drawn at a world position are the HUD kit's: the
  Objects tool's gizmo, three world segments, and the navpoints over the
  thing held or aimed at and over other monitors (**Code**,
  `forge_glue/gizmo.rs`: `Axes::from_frame`, `forge_hud.rs`: `Points`).
- The swap runs on the local machine. Other machines and the film do not
  carry it yet.

A look dresses the transplanted body.

### The look

One value per player, the same shape for every use (the picker, the
profile, the network, the film):

```
MonitorLook
  version: 1
  model: key            "marker", or a census key (below)
  variant: name?        one of the model's named variants; absent = its default
  primary: 0..17        palette index
  secondary: 0..17      palette index
  tint: 0..3            none, light, medium, strong: how far the lit colour is mixed towards primary
  light: 0..2           off, dim, bright
  light_colour: 0..17   palette index
```

- `forge::look` (in `crates/extensions/forge`) owns the type, its
  bounds, its canonical bytes (canonical JSON through
  `content::canonicalize`, as the document, with the variant left out
  when absent) and `validate`. It has no guest, GPU or disc dependency.
- A model key and a variant name are 1 to 32 and 1 to 64 characters of
  `a-z`, `0-9` and `_`. The marker has no variant, tint or light, so a
  marker look with any of them is `monitor_look_invalid`; choosing the
  marker from a disc model clears them.
- A look is changed by a `Patch` of named fields (`look::apply`): the
  bounds, then the catalogue, then the locks. A field the model locks is
  refused as `monitor_look_locked` when the patch would change it.
- Palette indices are the retail player palette's: 18 colours
  (**Code**, `live_telemetry::layout::PALETTE_COLOURS`). Their RGB is
  read from the loaded map's globals at run time, the block
  `pad_lightbar.rs` reads (`MATG_PLAYER_COLOURS`, **Code**). No colour
  value is in this repository.
- A look names its model by key, never by bytes. Nothing in it can hold
  a file.

### Which model

There are three sources, in the order they are built.

| Source | What it is | Needs | Seen by a machine without it |
|---|---|---|---|
| **Marker** | the HUD kit's navpoint over the monitor, with the distance as its number | nothing: no disc tag, no renderer feature | cannot happen |
| **Disc model** | the render model of a flying biped on a campaign map of the user's own disc, read by the host process and drawn by the native renderer at the monitor's pose | that campaign map on this machine; the native renderer | the choice is refused (below) |
| **Real body** (F10) | the biped itself, transplanted into the map; the player's unit | F10, tag headroom, a Rebuild | the document is refused at the gate (forge-design.md, "Saving and sharing on Live") |

A disc model is **not** a transplant. The tag stays in its own map file;
the host process reads it from there, as the Live art extractor reads
campaign bipeds (**Code**, `crates/apps/cli/src/live_models.rs`,
`build_ai_bipeds`) and as Theater's independent view draws posed models
without the guest (**Code**, `theater/host_view.rs`, `GpuModels`,
`native_render::instances::ObjectInput`). So it costs no tag headroom,
needs no Rebuild and works on a map with no kit.

**The census.** When a session opens, `forge::look::census` lists the
models this machine can draw:

1. Walk the campaign maps the disc mounts. For each `bipd` tag, take
   its species key by `species_of` (the tag's directory name). Every
   campaign biped is a candidate, flying or not, and the player
   bipeds are included. A tag whose directory is not a key, and the key
   `marker`, are listed as skipped with the reason.
2. Where several tags share a key, pick one by `choose_bipeds`' rule
   (the tag named like its directory, else the one on the most maps,
   else the shortest name). `species_of` and `choose_bipeds` are
   `forge::look::census`'s; `live_models` calls them, so the Live art
   extractor and the census agree on a species.
3. Resolve it: `Context::resolve_object` gives the `hlmt`, its render
   model and its variants ([objects.md](../formats/objects.md),
   "Resolver").
4. Record the entry: key, display name (the tag's base name, read at run
   time), source map and that map file's SHA-256, the variant names,
   whether any material has a change-colour map, whether the object
   attaches a light, and the node and triangle counts.

The rules, the cache and the report are `forge::look::census`, over a
`Disc` trait (campaign maps with their hashes, the biped tags of a map,
one tag described). `crates/apps/cli/src/forge_looks.rs` implements it
over the user's disc: `Context::resolve_object` for the variants and the
attached light, `live_models::read_source` for the nodes, triangles and
change-colour maps. A tag or variant the census cannot list is a
`skipped` row with its reason, never dropped.

The census is cached under `<data root>/cache` by the set of source map
hashes (`forge-looks-<key>.json`), and the latest one is also written as
`forge-looks.json`, the file the game reads as its catalogue. A game
with no `forge-looks.json` has an empty catalogue and says why (the
`catalogue_note` of `forge.look`). An entry is identified across
machines by `(key, source map SHA-256)`, never by a tag index.

**Variants.** A model with more than one named variant gets a
**Variant** row. A variant's sections come from
`native_render::model_load::pick_sections`. Where a variant region
lists several permutations the look takes the one with the highest
probability, the first on a tie (**Design**; the game draws by
`0x0016dad0`, which is not reproduced, objects.md). Damage states are
never chosen.

**The marker.** Another seat's monitor, whatever its look, has a navpoint
of the monitor HUD over it (`forge.nav.monitor.1` and `.2`: the two
nearest), the game's arrow and marker with the distance in metres as its
number ([forge-firefight-ui.md](forge-firefight-ui.md), "Markers in the
world"). The game draws a navpoint in one colour, so the seat's colour is
not on it; the colour is on the body. A monitor never has one over its own
body, and no other seat exists until the session channel (ML7).

**Pose.** The body sits at the camera's position, turned to its yaw and
pitch, in the model's rest pose, with a slow vertical bob computed from
the render clock. A monitor never draws its own body in its own view:
the camera is inside it.

**Where it is drawn.** `forge_glue::monitor_lights`, the call the frame
step makes once a game view, also publishes the frame's bodies
(`monitor_model::gpu::publish`, `native_render::bodies::publish`): each
live seat's draw and the film's playback draws, each model once. A census
entry's model is read once, in the background, from the entry's source
map in the mounted game (`monitor_model::gpu::asset`: the biped tag the
entry's key and name name, `census::tag_for`; its render model and each
opaque material's base map, as the Skulls page reads its skull). Until
it has loaded, or if it cannot be read (said once in the log), the seat
draws the marker. The frame graph puts one stage after the last op of
each game view, in the markers' place and before them
(`frame_graph::bodies`): the stage uploads the models to a `GpuModels`
per device, puts each node at its rest frame under the seat's pose
(`bodies::nodes`), and draws them on the scene's colour and depth against
the guest's W-buffer, inside the view's viewport. The seat's own body is
not in the draws. A machine declares that it draws bodies
(`forge_glue::bodies_drawn`) only with the native renderer on.

### Colours, tint and light

| Part | Marker | Disc model |
|---|---|---|
| Primary, secondary | the ring and the name | the model's change colours, where the census found a change-colour map; otherwise the rows are locked with "No colour areas" |
| Tint | none | the lit colour mixed towards primary by a fixed alpha a step (`ObjectInput::highlight`, the editor's selection mix, **Code**) |
| Light | none | a host light at the body's origin: the palette colour, one of two fixed radii, no shadow |

- The bodies stage draws a model's own base maps with the tint. It does
  not yet apply change colours, so the primary and secondary reach the
  draw (`look::body::Draw`) and the light, and the model's colour areas
  keep their base-map colours.
- Change colours are drawn as the game tints them: per channel, one
  where the change-colour map is empty and the colour where it is full,
  red primary and green secondary ([live-art.md](../live/live-art.md),
  "Vehicle colours"). Only primary and secondary are used.
- The light is the renderer's, not the game's. `frame_scene::Light::host_light`
  makes it: no light datum and no `ligh` tag (`handle` and `definition`
  are `NO_DATUM`, `host` the seat), a sphere at the body's origin of the
  look's radius, both colours the palette colour, no guest steps, flag
  bit 2 clear so no shadow map is made for it. `forge_glue::monitor_lights`
  turns each drawn body (not a marker) into one, with the colour read from
  the loaded map; a colour the map does not hold gives no light. The
  frame step adds them to each game view after the game's candidates
  (`render_bridge::scene_step::set_host_lights`, set by `frame_owner`).
  The E3 light choice ranks them after every game light, so they are
  skipped first when a view is over its light budget
  ([e3-lighting.md](../graphics/e3-lighting.md), "Lights: which, and how
  many"). It changes nothing in the sim.
- The passes draw it without a guest record. Once the main view's light
  pass is over (the hook where the frame step adds the lights),
  `render_bridge::native_lights::host::constants` makes its constants:
  the guest's own `0x0004a1e0` port and flag code, run on a sphere
  volume, a frame block and a definition held at addresses past guest
  RAM (`Call::light` of `u32::MAX`, a light without a record), so colour
  B is zero, no gel and no shadow. `native_lights_glue::host` then draws
  its layers 12 and 13 as one external pass at the view's target: the
  structure and placement parts whose bounds meet the sphere, in the
  world's order, each with the implementation the engine's chooser gives
  the flags (layer 12 the sphere implementation, layer 13 only for a
  colour B above the threshold). Object (model) parts are not lit.
  `scene_host_lights_drawn`, `scene_host_lights_kept` and
  `scene_host_lights_unread` count the outcomes.
- In a team session the host may set **Monitor colours: team**. Primary
  is then the team's colour from the map's multiplayer globals (the
  block `pad_lightbar.rs` reads, **Code**) and the Primary row is
  locked with that reason.

### Choosing it

The entry is a row, **Monitor look**, in two places: the Forge menu
(Back while a monitor: `forge.menu`, on its Options tab,
[forge-firefight-ui.md](forge-firefight-ui.md), "The screens") and the
Forge lobby (F14). Both open the same page, `forge.look`, a Forge screen
of the game's on the large dialog (`forge_screens`), as the object menu
and Link are ([ui-builder.md](ui-builder.md), "Screen kinds", "The
value picker"). Not built: the row joins the Forge menu with the page
(`forge_screens::menu`).

```text
 +----------------------------------------------------------+
 | MONITOR LOOK                                             |
 | Model             Sentinel                               |
 | Variant           Default                                |
 | Primary colour    <colour>              (more rows scroll)|
 | +------------------------------------------------------+ |
 | |                      preview                         | |
 | +------------------------------------------------------+ |
 | (A) Change   (X) Reset   (B) Done                        |
 +----------------------------------------------------------+
```

- **Rows.** The list shows three rows at a time and scrolls through the seven, over the preview strip: the large panel holds no more. Each row shows its value in the second column; a colour's
  name is the game's own string for that palette index, read at run
  time. A opens the
  kit's value picker over the page (`ui_kit::picker::offer_list`) with
  the row's values and one line of description. No row steps sideways
  and none draws arrows.
- **Locked rows.** A row that cannot be changed shows its value and its
  reason as the description: no colour areas, team colours, a model
  with one variant. A model the census lists but a peer cannot draw
  stays in the Model picker, greyed, with the refusal's sentence.
- **Preview.** A new `View` widget, `Scene3d {id, rect}`: the look drawn
  by the native renderer into an offscreen target (the model, its
  colours, tint and light, a slow turn, a neutral backdrop) and placed
  in the rect by the window. `h2 ui preview forge.look` draws the
  rect with the model's name in it, since the offline preview has no
  GPU scene; the kit's checks treat the widget as art.
- **Preview follows focus.** While a picker is open the preview shows
  the focused value, not the held one. `Pick` gains an `on_focus`
  callback for it. B restores the held value; A sets it.
- **Applying.** A in a picker sends the changed look. Offline it is set
  at once. In a network session it is a proposal to the host, and the
  row keeps the old value until the host answers; a refusal shows its
  sentence in the dialog's description line and in the HUD strip.
- **The game's Appearance page.** When the player's look is the Forge
  monitor, the page's own model preview draws the monitor beside the
  Spartan (ML4b, decision 8). `forge::look::appearance::beside` places
  it: the saved look of the profile on local slot 0, resolved against
  the catalogue (`saved::resolve`), on the widget camera's right of the
  biped, clear of its sphere by `GAP`, facing the camera, level, with
  the look's colours, tint and light. No saved look, the marker, a model
  or variant the catalogue lacks, or a camera with no right axis adds
  nothing, so the page is retail's. `forge_glue::appearance_body` gives the
  draw. The page's model widget is the overlay's: at its item call the
  native path reads its view and calls `Host::model_view` (`native_hud::ui`)
  with the widget's bounds and its camera and objects, and
  `monitor_model::gpu::appearance_view` queues a picture of the monitor
  (the Skulls page's showcase pass, `native_render::showcase_gpu`) in the
  right share of those bounds (`monitor_model::BESIDE_SHARE`), the Spartan
  keeping the rest. The picture turns slowly, tinted by the look's
  highlight; it is not placed in the widget's own 3D scene, so it does not
  follow the widget's camera. Every model widget's view is offered; the
  look and the loaded model decide whether anything is drawn.
- **Reset** (X) is the default look: the marker, the seat's colour, no
  tint, no light.

### Where it is saved

Per profile, in `settings.toml`, through the store's per-profile values
(`settings::Store::profile_get` and `profile_set`, **Code**;
[settings-menu.md](settings-menu.md), "Settings as a schema"):

```toml
[profiles."<name>".forge]
monitor_model = "sentinel"
monitor_source = "<source map SHA-256>"
monitor_variant = ""
monitor_primary = 3
monitor_secondary = 0
monitor_tint = 1
monitor_light = 1
monitor_light_colour = 3
```

- The five numeric keys (`monitor_primary`, `monitor_secondary`,
  `monitor_tint`, `monitor_light`, `monitor_light_colour`) are
  `settings::Def`s with `Scope::View`: they change presentation only.
  Tint and light are choices (`none light medium strong`, `off dim
  bright`) and are written by name. They are in `forge::look::saved::DEFS`
  and listed in `setting_defs::ALL`, with no menu row, environment
  variable or flag.
- The three names (`monitor_model`, `monitor_source`, `monitor_variant`)
  are read and written as text through `Store::profile_text` and
  `profile_text_set`, because a `Def` holds only a bool or a whole
  number. An empty text is "none".
- A missing key is its default: the colours are the seat's colour, tint
  and light are none and off.
- The file follows the account through cloud saves with the other
  per-profile settings ([live-cloud-saves.md](../live/live-cloud-saves.md)).
- A saved look whose model this machine's census lacks, or has from
  another source map, is not replaced. It is kept in the file, the
  session starts with the marker in the saved colours, and the HUD strip
  and the page say why once ("Sentinel needs a campaign map this install
  does not have"). A variant the model no longer has is said the same
  way. An out-of-range value is reported by key, as any setting is, and
  a bad name is reported by key; neither is rewritten.

### Other players

A look and a pose reach the other machines on the Forge session's own
channel, beside the edit log (forge-design.md, "The edit log and
multiplayer"). They are not log entries: the log is the document's
history, and a look edits no map.

```
look:   {seat, look, base?}             peer -> host, reliable ordered
look:   {seat, look}                    host -> every peer, reliable ordered
pose:   {seat, position, yaw, pitch, monitor: bool, held: OpId?}
                                        peer -> host -> peers, unreliable, at most 15 a second
```

- **The host owns the table** of seat to look. A peer proposes; the host
  validates and sends the accepted look to everyone, the proposer
  included. A machine draws only looks the host sent.
- **What each machine can draw** is declared in the session hello: the
  look version, whether the native renderer is on, and the SHA-256 of
  each census source map it holds. The host keeps these per machine.
- **A choice everyone cannot draw is refused**, to its author, with a
  named reason and the machine it failed on. The seat keeps its
  previous look.

| Refusal | When |
|---|---|
| `monitor_look_invalid` `{field, reason}` | a field out of bounds, an unknown key, a model or variant the catalogue lacks |
| `monitor_look_source_missing` `{machine, map}` | a machine in the session does not hold the model's source map at that hash |
| `monitor_look_renderer` `{machine}` | a machine in the session runs without the native renderer |
| `monitor_look_locked` `{field, reason}` | the look changes a field the model or the session locks: the host's team colours lock primary, a model with no colour areas locks both colours, one variant locks the variant, the marker locks variant, tint and light |
| `monitor_look_version` | at join: the joiner's look version is not the host's; the join is refused before the map loads |
| `monitor_look_no_session` | a set outside a Forge session through the agent API (the menu writes the profile instead) |

- **A joiner who cannot draw a look in use** does not join into a
  session that silently differs. The host resets each affected seat to
  the marker with its colours kept, sends that look to everyone, and
  tells the affected player in the HUD strip: "Monitor look reset:
  \<gamertag\> does not have \<map\>". The saved profile value is left
  alone, so the look returns in the next session.
- **Poses** are interpolated between samples on the render clock and
  dropped when stale by two seconds; the body then fades out, and the
  seat shows as "not seen" in `forge.status`. `monitor: false` (the
  player is back in their biped) hides the body at once.
- **Both network modes** carry the channel unchanged. Synchronous (F5)
  and host-replicated (F15) differ in how the sim is kept equal; the
  look is in neither.
- **Retail-compatible sessions are untouched.** A Forge session is not
  one: every machine in it runs Forge. A match on a published map has no
  monitors, sends no look or pose message and gives every player their
  own biped (forge-design.md, "Rules and limits").

### A real body

The monitor is a transplanted unit (`forge_doc::bodies`). Its look is
part of the game:

- The model is the kit's body and the unit swap. The body is chosen by
  the `forge.body` setting: Monitor, Sentinel or Prophet in chair. A body
  the kit left out is shown greyed with its reason (`forge::body::rows`).
- Its colours are the unit's change colours in guest memory, which the
  object update evaluates (`OBJECT_CHANGE_COLORS`, **Code**,
  `crates/hooking/guest-layout/src/points.rs`). Setting them is a Forge
  `body` op in the log, applied by every machine at the entry's tick, so
  the sim hash stays equal.
- Its light is whatever the biped's own tags attach. The look's `light`
  and `tint` rows are locked for a real body.
- The presentation body is not drawn for a seat that has a real one.

### Theater

A film of a Forge session already records each log entry under a Forge
tag (forge-design.md, "Films and determinism"). That tag's payload
starts with a subtype byte: 0 is the session's welcome and 1 a log
item (`forge::net::film`); the look and the pose are 2 and 3
(`forge::look::film`, `LOOK` and `POSE`).

- The recorder (`look::film::Recorder`, fed by `forge_glue::net`) writes
  a look record, JSON of `{seat, look, source}`, when the table changes
  on the machine that records, and a pose record, 27 bytes of seat,
  monitor flag, film tick and position, yaw and pitch, for every monitor
  seat at most 15 a second (every second tick), its own included. A seat
  stepping back into its biped writes its pose record at once. `source`
  names the map the model was read from and its SHA-256, as the
  recording machine's catalogue has them.
- Each seek checkpoint repeats the table and each seat's last pose
  (`Recorder::checkpoint`, written beside tag 3's baseline in
  `theater/unified.rs`). A playback fed the checkpoint and the records
  after it holds the table and every seat's pose that one fed the whole
  film holds, at every tick from the checkpoint on.
- Playback reads the records into the same `Seats` the live session
  draws from (`look::film::Playback`), and draws with `Bodies::draw`.
  The film player reads the stats container beside the film file a game
  tick at a time (`theater::film_play::Looks`) and hands each tick's
  Forge entry records to `theater::forge_film::deliver` as the game tick
  ends. Only the look and pose subtypes go to the playback; the welcome
  and the stream items are the edit log, which the film file's extension
  records replay (`theater::extension`; `net::film::Replay::feed` routes
  all four for a consumer that wants the peer). A start-route seek clears
  the playback and the container is read again from the game's first
  update; the film's end clears it. A film whose container is not on the
  machine (a host's film read from Live) plays with no monitor bodies.
  The container's tick is the render clock, 30 a second. The records are outside the gameplay digest, so a film
  verifies the same with or without them, and a record that is not
  readable is a named film error. The film reader accepts the subtypes
  (`live_proto::film::stream::FORGE_LOOK`, `FORGE_POSE`); any other is
  refused.
- `Playback::subject(seat, tick, view)` gives the camera on a monitor
  seat its subject: the body's pose for follow and orbit, the monitor's
  own camera pose with no body drawn for first person
  (`Playback::draws` hides that seat). A marker seat has a pose to
  follow, though no body. `theater.camera` takes a monitor seat in follow,
  orbit and first person by its seat number, even when the film's roster
  does not name it (`theater/camera.rs`, `monitor_subject`): the rig's
  offset and orbit work as on a body, a first-person camera looks from the
  monitor's pose over the whole view, and a seat that is back in its biped
  is the guest's player again. `forge_glue::playback_bodies` gives the
  drawn bodies at the film tick and `forge_glue::monitor_lights` adds their
  lights.
- A playback machine that cannot draw a recorded look says so and draws
  the marker in that look's colours: `Playback::unseen` lists each
  seat and the map it needs, `theater.compat` carries them as
  `monitor_looks` entries with the reason `monitor_look_source`
  (`agent_proto::theater::MonitorLookSource`,
  `theater::compatibility::note_monitor_looks`). `Installation::compatibility`
  reads the film's look records once per file (`forge_film::unseen_in`,
  kept until the file changes), so the film's detail page names the count
  (`FilmMeta::monitor_note`) and `theater.compat` lists them. `theater.state`
  lists the looks read so far that this machine cannot draw
  (`monitor_looks`); the bar says each seat's sentence once, for six
  seconds of film, when the seat first appears (`Track::note_monitor_look`).
  They are not reasons the film is unavailable: playback is not refused,
  because the simulation is whole. `forge_glue::can_draw` is this
  machine's declaration, from the running Forge driver's catalogue; with
  no driver the catalogue is empty and a model look is drawn as the marker.
- A real body needs nothing here: it is a unit in the film's own state.

## Pages

| Page | What |
|---|---|
| [Forge monitor appearance: interfaces, slices and evidence](forge-monitor-appearance-contract.md) | Interfaces, rules, slices, decisions, evidence and open items. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## Interfaces

See [Forge monitor appearance: interfaces, slices and evidence](forge-monitor-appearance-contract.md#interfaces).

## Rules and limits

See [Forge monitor appearance: interfaces, slices and evidence](forge-monitor-appearance-contract.md#rules-and-limits).

## Slices

See [Forge monitor appearance: interfaces, slices and evidence](forge-monitor-appearance-contract.md#slices).

## Decisions

See [Forge monitor appearance: interfaces, slices and evidence](forge-monitor-appearance-contract.md#decisions).

## Evidence

See [Forge monitor appearance: interfaces, slices and evidence](forge-monitor-appearance-contract.md#evidence).

## Not verified

See [Forge monitor appearance: interfaces, slices and evidence](forge-monitor-appearance-contract.md#not-verified).

## Related

See [Forge monitor appearance: interfaces, slices and evidence](forge-monitor-appearance-contract.md#related).
