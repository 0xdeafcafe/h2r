# Live mirror: the running game in the editor, and edits back into it

Status: Partly built. Live object streaming, the game's cameras (the
`camera` layer, frustums, Follow, Through its eyes and Game camera), pause
and step, a windowless paired editor, conditional tag edits, live object
edits, live placement adds and removes (`world.placement.edit`) and
reload from a play copy are built. The richer live view, timeline
and substantial structural edits are designs. Real-game parity and cost
checks are pending.
Code: `crates/extensions/agent-proto/src/live` (wire format), `crates/extensions/agent/src/live` (capture and stream), `crates/extensions/game/src/scene_glue.rs` (the camera source), `crates/apps/live-mirror` (editor store), `crates/apps/studio/src/play` and `crates/tools/scenario-edit` (pairing and viewport).
Switch: none. With no subscriber the tick observer skips capture and no stream worker runs.

A paired editor shows the game as it runs: players, bipeds, vehicles,
weapons, projectiles and their arcs, machines, effects, each object posed
as the game poses it. In edit mode, a change made in the editor reaches
the running game at once where that is safe, and is marked "needs a map
reload" where it is not. The game sends a stream of world state over the
agent socket; the editor never reads the game's memory and the game never
waits on the editor. The rule that must hold: the stream only reads, and
every write goes through a named agent method that refuses a session where
one machine writing alone would split it from the others.

Clicking an object to edit the tags behind it, the merged scenario and
tag editor, and Halo 1 tags are in [click-to-edit.md](click-to-edit.md).

## What exists

| Part | State | Where |
|---|---|---|
| Pairing, auto-pair, game link pill | Built | `studio::play` (`play.rs`, `play/pair.rs`), `pair.*` ([agent-api.md](agent-api.md)) |
| Live objects and players | Built: a binary stream on a second connection, interpolated bind-pose models and player overlays | `agent_proto::live`, `live_mirror::Store`, `studio::play` |
| Pause, step, resume, go to player, teleport | Built: each step sends its frame | `play.rs`; `session.claim`, `pause`, `step`, `sim.state.set`; `mirror.control` |
| The game's cameras in the editor | Built: the `camera` layer from the last sealed `FrameScene`, a frustum per game view, Follow, Through its eyes and Game camera | `scene_glue::install_cameras`, `play/presence/camera.rs`, `viewport/mirror.rs`, `geometry::Exact` |
| A paired editor with no window | Built: `h2-editor --serve` runs the game link between requests | `Api::serve_once` (`crates/apps/studio/src/api/mod.rs`) |
| Placement field edits sent live | Built: a diff against `sync::Baseline` every 200 ms, one conditional `edit.batch` with the tag document’s fields and raw placement paths on the game’s `scnr`; shows when the object is next made | `play/sync.rs` |
| Geometry and painted pixels sent live | Built: a diff of the Modeling tool's patches and the Textures tool's encoded tiles against `raw_sync::RawBaseline`, sent as `edit.batch` `raw` entries with the bounds as `writes`; a grown section as `edit.raw.append`, then the batch with `evict`; shows on the next frame ([realtime-edits.md](realtime-edits.md)) | `play/raw_sync.rs` |
| Move, delete and remake live objects | In Edit the streamed placement object follows a move or delete. Respawn reads the loaded scenario entry and returns an unlinked preview. | `crates/apps/studio/src/play/objects.rs`, [Object edits](../runtime/agent-api-sim.md#object-edits) |
| Add and remove placements live | In Edit an added, removed, undone or redone entry is one `world.placement.edit` on its block; the receipt advances the loaded layout | `crates/apps/studio/src/play/objects/drive/placements.rs`, `play/sync.rs` (`Baseline::transaction`), [Placement edits](../runtime/agent-api-sim.md#placement-edits) |
| Reload the running map | The editor writes a play copy and the host queues the active campaign or multiplayer options. The editor checks the returned activation and checksum. | `crates/extensions/agent/src/sim/edits.rs`, [Test maps](../runtime/agent-api-checks.md#test-maps) |
| Tag field writes in the game | Built: View/Edit, field effects, the Pending tab, `edit.map`, conditional `edit.batch`, journal adoption and `edit.revert` | `crates/extensions/hot-tags`, [hot-iteration.md](../graphics/hot-iteration.md) |
| No writes in co-op or as a client | Built | `agent_proto::sim::SessionKind::writes` |
| Typed object reads | Built: transform, velocity, sphere, vitality, node matrices, unit player, team, seat, weapons and aim | `crates/rewrites/sim-state` (`ObjectView`, `UnitView`), over `crates/hooking/guest-layout` |
| The tick's events | Built: `deleted` while finalize runs in Rust (by default), `damage` and `kill` always | `sim_state::events`, `sim.events` |
| The game's cameras, drawn instances, lights | Built for a run that renders with the native renderer; the cameras and drawn handles stream as the `camera` layer | the sealed `FrameScene` (`frame_scene::View`, `Instance`), [framescene.md](../graphics/framescene.md) section 4 |
| Object to placement link, a machine's position | Built | `live_telemetry::memory` (`placement`), `guest_layout::unit::Device::POSITION` |
| Per-tick rows taken inside the tick observer | Built, as JSON to a file | `crates/extensions/agent/src/trace.rs` |
| Models in the viewport | Built in the bind pose, first permutation, instanced per model | `scenario_edit::render::Viewport`, [editor.md](editor.md), "What the viewport shows" |
| Map save, packages, play copy | Built | `scenario_edit::save`, `MapEditor`, [scenario-editor.md](scenario-editor.md) section 5 |

The design adds the timeline and recordings, skinned models, projectile
arcs, device and effect reads, and a broader set of live object edits.

## Live view

### Transport

A stream is a second connection to the game's agent endpoint. The editor
keeps its JSON link (`play/link.rs`) for calls and opens another
connection for `live.subscribe`. After that call's JSON result line, the
connection carries only binary frames from the game, so a Unix socket and
`tcp://` (the Xbox app) work alike and nothing is base64. Options change
through `live.update` on the JSON link, naming the stream.

`session.subscribe`'s JSON events stay for phase and map changes. They
are not used for world state: a tick of 600 objects as JSON is several
hundred kilobytes and parses on both sides. The Live telemetry codec
([live-telemetry.md](../live/live-telemetry.md)) is not reused either: it
is built for a small sampled record per player over a network, and the
mirror wants every object at full precision over a local socket.

A frame is `u32` length, then a header (`kind`: key, delta or notice;
`seq`; the game tick; the map epoch; flags `paused`, `cut`), then
sections. A section is a layer id, a byte length and a payload, so a
reader skips a layer it does not know. Layer ids follow the table below
from 1 (`objects`); `camera` is 7. The types and both codecs are
`agent_proto::live`, with no guest and no editor dependency.

| Layer | Carries | Source |
|---|---|---|
| `objects` | per live object: slot and salt, definition tag, type, header flags, parent, position, forward, up, scale, velocity, body and shield vitality, the placement that made it | `sim_state::ObjectView`; `placement` comes from the existing `guest_layout::Object::PLACEMENT_INDEX` field |
| `players` | per player: index, name (key frames only), team, unit, held weapon, aim vector, local user | `UnitView`, the player reads `world.query` uses |
| `poses` | per posed object: its node matrices, 13 floats a node, world space | `ObjectView::nodes` |
| `anim` | per animated object: the animation graph tag, the animation and frame each channel plays | new `sim_state::AnimationView` over `Object::ANIMATION_MANAGER` |
| `projectiles` | per projectile: the `objects` fields plus owner, flight flags, ticks alive | new `sim_state::ProjectileView` |
| `devices` | per machine and control: position 0 to 1, and power and target once read | `Device::POSITION`, new `sim_state::DeviceView` |
| `camera` | per view, keyed by kind, player and view index (`game:0:0`): the sealed scene's tick, position, forward, up, vertical field of view, near, far, viewport, the cut flag; the handles of the instances the view drew | `frame_scene::View::camera`, `Instance::key`, through `scene_glue::install_cameras` |
| `events` | the tick's events: deleted, damage, kill, projectile ended, effect started | `sim_state::events`; `live_telemetry::impacts`; effects are new |
| `effects` | particle points: position, size, colour, per emitter | the native particle stage's captured geometry ([native-particles.md](../graphics/native-particles.md)); new |

A key frame holds every record of every subscribed layer. A delta holds,
per record, a field mask and the fields whose bytes changed since the
last frame sent on that stream, plus the slots that went away. A key
frame is sent on subscribe, on `live.key`, after a cut (map or BSP
change, `state.load`, a checkpoint revert: the `FrameScene::cut` reasons),
after any frame the stream dropped, and every `key_every` ticks (default
150), so a reader that joins a recording part way waits at most five
seconds.

### What runs on the game thread

Three places do the work, in this order.

1. **Capture, on the game thread.** In the agent's `GAME_TICK` observer,
   where `trace.rs` takes its rows, a subscribed game fills a recycled
   `WorldCapture` through the `sim-state` views: plain structs of arrays
   for the object fields, and one copy of each posed object's node
   matrices. It reads guest memory once, at the tick boundary, so a
   capture is one consistent state. It does not compare, encode, allocate
   in the steady state or touch a socket. Reads from Rust are uncharged
   and invisible to the guest (agent-api.md, "Rules and limits"), so the
   sim hash does not move.
2. **Encode, on the stream worker.** The capture goes to a worker thread
   through a bounded channel of two buffers. The worker diffs it against
   the last capture it encoded, writes the frame and hands the buffer
   back. The `camera` layer is read here from the last sealed
   `FrameScene`, through the source the game's scene glue gives the agent
   (`Agent::set_camera_source`): a sealed scene is an immutable `Arc`, so
   it costs the game thread nothing. The worker sorts the views by key and
   keeps the first of any repeated key. A camera can be a rendered frame
   behind the tick it travels with; its `scene_tick` says which.
3. **Send, on the connection's writer thread.** Each stream has a queue
   of eight frames. When it is full the worker drops the frame, counts it
   and makes the next one a key frame.

When the worker still holds both buffers at a tick, the game thread skips
that tick's capture and counts it (`skipped`). A slow editor therefore
costs frames in the editor and nothing in the game.

`every` (1 to 30, default 1) sends one tick in that many; `poses` has its
own `every` and an interest sphere (`near {point, radius}`, which the
editor sets from its camera), since poses are most of the bytes. While
the game is paused no tick runs and no frame is sent, except one after
each `session.step` tick and one per `live.key`, which a paused game
answers from inside the pause as it answers other requests.

### Cost

Sizes are from the record layouts; the object counts and the time are
supposed (**S**) until slice M1 measures them and writes
`docs/editor/notes/live-mirror.md`. All numbers on this page move there
with their commit when measured.

| Layer | Key record | Delta, a tick | A busy 16-player game, every tick |
|---|---|---|---|
| `objects` | about 80 bytes an object | about 50 bytes a moving object, nothing for one at rest | 600 objects, 60 moving: 48 KB a key, 3 KB a tick, 90 KB/s |
| `players` | 40 bytes and the name | 16 bytes | under 10 KB/s |
| `poses` | 52 bytes a node | the same, for objects whose nodes changed | 16 bipeds of about 50 nodes: 42 KB a tick, 1.25 MB/s |
| `projectiles` | 60 bytes | 40 bytes | 50 in flight: 60 KB/s |
| `devices`, `anim`, `camera`, `events` | tens of bytes a record | the same | under 20 KB/s together |

About 1.5 MB/s with everything on, which a local socket carries without
notice. Over `tcp://` the editor asks for `poses` every third tick and
only near its camera.

On the game thread a capture is about 25 typed reads an object and one
copy of the node bytes: for the game above, some 15,000 reads and a
50 KB copy, supposed under 0.1 ms. The budget (**Design**) is 0.15 ms at
the 99th percentile, under half a percent of a 33.3 ms tick. The capture
times itself and `live.status` reports it. When the last 150 ticks'
99th percentile is over the budget, the stream doubles the `poses`
interval, then drops `anim` and `effects`, and says so in a notice frame;
the editor shows the notice. The time is measured on the quiet machine
through `h2 lab` ([lab.md](../runtime/lab.md)), comparing
`host.metrics`' `game_cpu_ms_per_tick` with and without a subscriber.

### Players, objects and poses

The editor's store (`live_mirror::Store`) applies frames to a table of
objects keyed by the salted handle. A live object is drawn with the model
of its definition tag, through the path placements already use (object to
`hlmt` to `mode`, `tags::object`), instanced per model. A placement whose
object is in the stream is drawn at the object's transform, and its
placement marker stays where the scenario puts it, with a line between
the two when they differ.

Players carry their name, team colour, an aim line along the unit's
aiming vector (`UnitView::aim`, the vector the weapon's targeting ray
follows) and bars for shield and body. Vehicles list their seats. A held
weapon is drawn on its unit through its parent link, at the node pose
once `poses` is on.

With `poses`, the viewport skins each model with the node matrices the
game computed that tick, so the editor shows the same pose with no
animation code of its own: blends, aim overlays, ragdolls and vehicle
suspension come through as they are. This needs a skinned pipeline in
`scenario_edit::render::Viewport`, which draws bind poses today. The
`anim` layer adds what the object plays (the graph tag, the animation and
its frame) as a label and for the inspector. It is not used to pose:
resampling in the editor would differ from the game wherever the engine
blends. The fields of the animation manager are not read yet (**S**); the
slice reads them from the unit animation update first
([object-update.md](../engine/object-update.md), "Slice 6c bodies").

### Projectiles and their arcs

A projectile is drawn three ways:

- **The trail**, solid: where it has been, from the store's ring. This is
  the game's own path.
- **The predicted arc**, dashed: `live_mirror::arc::predict` steps the
  position and velocity forward a tick at a time with the `proj` tag's
  gravity and drag, and ends at the first hit of the level
  (`Session::surface_hit`'s ray against the BSP). It is our integrator,
  not the engine's projectile step, which also homes and bounces
  (`PROJECTILE_STEP`, `crates/rewrites/engine/src/sim_objects/projectile_step.rs`).
  The editor shows the arc's error against the trail once the projectile
  has flown it, so a wrong arc is visible as wrong.
- **The impact**: the `events` layer's projectile-ended record, as a mark
  that fades.

**Aim projection** (a toggle) draws, for each player, the arc the held
weapon's projectile would fly if fired now: from the aim vector and the
tag's initial speed. A weapon whose projectile tag has no named speed
field shows a straight ray and says the arc is not predicted.

### Machines, devices and effects

A machine or control is drawn at its streamed transform, with its
position (0 to 1) as a bar and number, and its moving parts posed by its
node matrices like any other object. Power, target position and timing
come once the device update's fields are read into `guest-layout`
(`DEVICE_UPDATE`, **S**); until then the layer carries position only and
the inspector says which fields are not read.

Effects have two parts. An effect starting is an event: its tag, position
and the object it is on, from an observer on the effect creation path
(`EFFECT_PART_CREATE` is the nearest named point; the right entry is to
be found, **S**). It is drawn as a labelled mark. Particles are points:
position, size and colour for each particle of each emitter, taken where
the native particle stage already reads the guest's completed geometry,
and drawn by the editor as its own camera-facing quads, since the game's
quads face the game's camera. The guest keeps the particle simulation;
the editor does not re-simulate. Particles exist only for emitters the
game drew, so off-screen effects show their start mark alone.

### Interpolation and time

The editor draws at the display's rate and shows the state at "the newest
frame, less a delay" (default two sample intervals). Between two samples
it blends position linearly, the basis and each node's rotation by
normalized blend, scale and translation linearly. It does not blend
across a key frame after a cut, an object's first sample, or a move
larger than four times what its velocity explains: those snap. When the
next frame is late it extrapolates by velocity for one interval, then
holds. The Game panel reports when it is waiting for live frames.
**Latest** (no delay, no blend) is a toggle for watching an edit land.

Pause, step and resume are in the bar and Game menu. A step sends its
frame, so stepping one tick moves the viewport one tick. In `h2 play` a
pause stops the game window's event loop (agent-api.md, "Control").

### Cameras

| Mode | Does |
|---|---|
| Free | the editor's fly, orbit and pan camera (`camera.rs`) |
| Follow | orbits a local player at the offset the eye had on entry; orbit and zoom change the offset |
| Through its eyes | at the unit's position plus eye height, looking along its aim; the editor's own render, not the game's picture |
| Game camera | takes a game view's camera from the `camera` layer exactly: eye, forward, up (roll included), vertical field of view, near and far |

Game camera sets the editor camera's exact pose (`geometry::Exact`), so
the viewport's view matrix is the scene's. The projection uses the game's
vertical field of view at the editor viewport's aspect. It takes the
first `game` view, or the one `mirror.camera` names, and holds its last
pose while that view is not drawn. Leaving it keeps the eye where the
game's was and gives back the editor's field of view.

In every mode but Game camera, each `game` view's frustum is drawn as a
wire pyramid from the `camera` layer, one per split-screen player, out to
two world units or the far plane (`viewport/mirror.rs`, `frustum`). The
`drawn` handles are streamed; the overlay that tints the objects a view
drew is not drawn yet. A run with no sealed scene source (no native
renderer, or the frame scene off) has no `camera` layer: `live.subscribe`
lists it under `refused` with the reason, and Game camera is refused with
"the game has no camera". The editor does not invent a camera from a
player's aim.

## Live edit

The **Edit** switch in the bar and Game panel changes between View and Edit.
An attached game starts in View. F5 can send one confirmed batch or load a
play copy in View. Later changes stay in Pending until the next F5 or the
user enables Edit. A game the editor launches starts in Edit.

### How an edit applies

Each edit has an effect, when the change takes effect in the game
(`play::tag_sync::Effect`, docs/editor/tag-editing.md, "When it takes
effect"): an id for the API, a chip on the field and a sentence that
heads its group in the Pending tab.

| Effect (chip) | Sentence | Edits | Reaches the game by |
|---|---|---|---|
| `now` (**Now**) | Applies now. | values the draw path reads every draw: shader colours, vectors and bitmaps, effect and light colours where the draw reads the tag; moved and grown render geometry (clusters, instanced definitions, `mode` sections) and painted pixels of the same size and format | `edit.batch`; shows on the next frame. A grown section's row says the level rebuilds for a moment |
| `next_use` (**Next use**) | Applies the next time the game uses it. | values read when used: damage, rates of fire, machine speeds and delays, a projectile's drag | `edit.batch`; shows on the next shot, hit or cycle |
| `next_spawn` (**Next spawn**) | Applies when the object next spawns. | values copied into an object when it is made: a projectile's initial speed, an object's vitality, placement fields | `edit.batch`, then optionally a respawn. **Respawn now** remakes supported, unattached occupants of unnamed palette entries with default creation fields. Other placements take the edit on their next creation or a reload. A `next_spawn` row on an object tag, or on an `hlmt` or `phmo` that object tags reach, also sends `world.object.respawn {definition}` for each such object tag |
| `not_known` (**Not known**) | Not known; reload to be sure. | any field with no recorded `applies` | `edit.batch`; written at once, when the game reads it is not known |
| `reload` (**After reload**) | Applies after reload. | placement slots added, duplicated or removed in a game without `world.placement.edit` or that refuses the transaction, sound scenery slots, changes to tag block counts, new tags, new strings, grown blocks, structure BSP and lightmap fields edited in the Tags tool, a geometry edit the save refuses or whose tag the game has not loaded, a grown section a play copy already holds, a bitmap resized or reformatted | saving the play copy and loading the map again |

Moving or turning a thing that exists reaches the game by
`world.object.set` on the next tick. Adding and removing placements
reach it by `world.placement.edit`, one transaction per block, on the
next tick. A game without that method gets the older path: deletes by
`world.object.delete` and previews of new placements by
`world.object.respawn`. Object call rows in Pending have `doc` `object`.

**Geometry and pixels.** In Edit, a Modeling edit and a Textures stroke
go as raw entries (`play/raw_sync.rs`, [realtime-edits.md](realtime-edits.md),
"The editor"): one Pending row per edited section (`doc` `geometry`,
`path` the target id) and per painted image (`doc` `bitmaps`), each
**Now** until the game acknowledges it. Every entry carries the bytes the
editor takes the game to hold as `was`; when the game holds others the
row is marked as a conflict and waits until the edit changes or **Send**
is pressed again. Undo sends the old bytes. View mode sends nothing.

The effect of a field is data, not a guess made in the UI: `tags::defs`
has an `applies` attribute (`frame`, `use`, `creation`) with its
evidence, as names have theirs, and the table in
[hot-iteration.md](../graphics/hot-iteration.md), "What shows when", is
its first content. A field with none is `not_known`.

**Placements.** In Edit, dragging a placement writes its scenario fields
and moves its linked object with `world.object.set`. The editor sends at
most one move per object per push interval during a drag. The released
frame follows its scenario field acknowledgement.

Adding or removing a placement, and undoing or redoing either, changes the
block's layout revision. When the game offers `world.placement.edit`
(design: [realtime-edits.md](realtime-edits.md), "Placements"; the
method: [Placement edits](../runtime/agent-api-sim.md#placement-edits)), the
editor sends that block as one transaction instead of putting it behind
reload (`play/objects/drive/placements.rs`). `sync::Baseline` holds what
the game's block holds: its records, the document placement in each slot,
and the bytes a removal left past the count. `Baseline::transaction`
compares that with the document: placements gone from the document are
`delete`d, survivors in another slot are `relink`ed, new ones are
`create`d, every slot whose bytes differ is a record with its old bytes as
`was` where known, and object names follow. Spawn points, netgame flags
and netgame equipment take records and the count only. On the receipt,
`Baseline::placed` advances the loaded revision to the document's, so
later field edits of that block are ordinary writes to the new slots and
the object rows follow the new numbering. Undo is the inverse transaction,
computed the same way; no second undo log exists.

The editor sends one transaction at a time, never beside a field batch or
an object call. While it is out, the block's Pending row reads "Applies
now". A refusal (other people in the game, no room in the block, a
mismatch with the game's records, the session) puts the block under
**Reload map** with the game's reason, and that exact transaction is not
sent again until the document or the peer confirmation changes it; going
back to the game's layout clears it. A created entry the game answers
with `created: false` (a named record, placement flags, a scale,
type-specific fields) shows under **Reload map** with its reason. A
transaction whose answer is lost (the link closes, or the call times out)
puts the block under **Reload map** until the map loads again: the game
may hold it, so the editor never sends it twice. Sound scenery has no live
object method and stays behind reload.

Without `world.placement.edit`, deleting the placement deletes the live
object, and undoing the delete requests a respawn at the original loaded
slot. **Respawn now** applies
creation-time fields that differ from the last actual creation. A replacement
uses the salted object ID. The host creates it before deleting the old
occupant, so an allocation refusal keeps the old object. An observed empty
slot can be explicitly selected by placement.
Every object request carries the map epoch the editor paired on
(`play/objects/drive/wire.rs`), so the host refuses an edit that arrives
after the map changed.

In a game without `world.placement.edit`, an added placement requests a
preview through `world.object.respawn`.
The host reads creation fields from its loaded scenario. An index beyond
that block needs a reload on this host. On success, the editor tracks the
returned salted object ID, moves it to the document frame, and uses the
receipt to mark and pick the preview even without a streamed placement link.
Undoing the add deletes that preview. Pending gives the reason when a
session, missing method or game refusal prevents an object edit. If the
connection closes or times out before a spawn or delete is acknowledged,
Pending recommends reloading the map. The editor keeps that outcome visible
through undo and reconnect so it cannot spawn a second preview by retrying.

**Reload.** The editor compares each placement block's layout revision with the
revision in the loaded file or Play copy, advanced by each acknowledged
placement transaction. A remove followed by an add can leave the count
unchanged and still move a placement to a different slot. While the
revisions differ and no transaction can carry the block (View, a game
without the method, a refusal), all fields in that block wait for reload.
Undoing the structural edits restores the earlier revision. Loading a Play
copy sets the baseline to the layout saved in that copy.

Pending reload edits collect under one **Reload map** button.
Like F5, it sends the game the path and checks the queued reply against the
play copy. Before resuming field writes or restoring the player, the editor
checks the saved checksum against `edit.map`. It waits for the requested
`activation` in the public game tick stamp. A checkpoint revert cannot
satisfy that request. When the receipt or the public stamp omits `activation`,
it waits for an epoch different from the one before the request. It keeps the destination through
a reconnect and retries the identity check for up to 30 seconds. A lost
reply does not cause a second reload. If the host refuses to bind the file,
the editor relaunches its own game or asks the user to load the saved map.
[Play and pair](editor.md#play-and-pair)

### Undo and sync

The editor's document is the truth, and the game is brought to it.
`sync::Baseline` extends from placement blocks to every tag field the
editor has sent: for each (tag, path), the bytes the game is taken to
hold. After any change, undo or redo, the diff against the baseline is
the set of writes to send, so undo sends the old bytes back with no
second undo log. Each write carries `was`, the bytes the baseline
expects; the game refuses a batch whose `was` differs
(`MISMATCH`, naming the field and what it holds), which replaces "trust
that the game runs the map as shown". After undo or redo, `Baseline::transaction` brings a block whose layout
changed, and `play::objects::diff` compares the placement document
with the acknowledged object edits and requests the corresponding move,
delete or respawn.

**Take the game's value** reads the conflicted field at a game boundary in
the paired epoch. A successful read updates the document and its baseline.
A conditional overwrite sends the value seen in the conflict as `was`.
Both choices require the tag path to resolve to the original address,
including when the read returns. If a refusal identifies one field, its
siblings can proceed. Otherwise the refused batch stays together until
a member changes or the user explicitly retries it.

On pairing, `edit.map` gives the loaded map's name, file and checksum.
Map identity uses the same case-insensitive level name as the game link.
Same checksum as the file the editor's document came from: the baseline
is that file, and unsaved edits show as pending until the user picks
Edit, which sends them. Another checksum: View only, with the reason. A
new epoch rebuilds Pending from the loaded file or Play copy and reads the
game journal. A BSP switch retains entries for tag allocations that survive.
Reverting a journal in the same epoch updates the baseline with the restored
bytes and the bytes the game reports for fields it leaves alone. Choosing
**Restore** on disconnect stops new writes, waits for the outstanding batch,
then reverts before closing the link. The journal contains field bytes. Reloading
the map restores its original population of live objects and removes previews.

Saving is the existing path and is not changed by pairing: **Save**
and **Export map...** write the document
(`scenario_edit::save::save_as`, `MapEditor`), never the game's memory
read back. `edit.revert` on unpair is offered ("Put the game back as it
loaded"), not done by default.

### Networked games

| Session | Stream | Edit |
|---|---|---|
| Local: campaign, split screen, a level booted by Play | yes | yes |
| Theater playback | yes; the scrubber and the film's transport are separate | no: a film replays inputs, and a write changes what they produce |
| Co-op host or guest (synchronous) | yes: reads do not touch the sim hash | no. The switch is disabled with the reason; edits stay in the document as pending |
| System Link host, with other machines in the game | yes | only after a prompt that names how many machines will not have the change; tag pokes are local to the host, so clients predict with the old values. Object edits replicate as the host's state does |
| System Link host, peer count unknown | yes | no until the game can verify the peer count |
| Multiplayer client | yes: what this machine holds | no: the host overwrites it |
| A Live custom game this console hosts | no stream while a game is in progress: it would show one player every position | as a System Link host: the same prompt when other machines are in; a placement count change only when the others are bots |
| A Live matchmade game, or a Live game another console hosts (not the local embedded server of `--live-local`) | no stream while a game is in progress | no |
| Session unknown | yes | no |

The host confirmation in Pending applies to the current pairing, game map
epoch and peer count. A file reload applies to the active map. Loading
a different map requires switching maps or relaunching. Field writes also
require a matching checksum.

The game enforces this, not only the editor: `edit.set`, `edit.batch`,
`world.object.*` and `map.reload` refuse by `SessionKind::writes`, as
`sim.state.set` does, and refuse on a Live matchmade game or a Live game
this console does not host (`agent_glue::live_edit_refusal`).
`live.subscribe` refuses on every Live session in a game. Continuous writes require Edit. A confirmed F5 in View
authorizes that playtest and leaves later edits in Pending. For a changed
map in a networked game, every machine loads the same saved map
([editor-guide.md](editor-guide.md)).

## The editor's UI

- **Layers** (in the viewport's Show popover): a list of toggles, each with
  its count from the stream. World: structure, sky, placements. Live:
  players, bipeds, vehicles, weapons and equipment, projectiles,
  machines and controls, effects, scenery and crates. Turning a live
  layer off also drops it from the subscription (`live.update`).
- **Overlays**: names and teams; aim lines; projectile trails, predicted
  arcs, aim projection; game camera frustums and the `drawn` tint;
  velocity vectors; bounding spheres; skeletons; animation labels;
  device positions; hitboxes (the `coll` and `phmo` shapes of the tag,
  posed by the node matrices, and `havok.bodies` for what Havok holds);
  AI (`world.ai`: actors, their state and targets); nav (the level's
  pathfinding data, `crates/formats/pathfind`); spawn influence (the
  weights the game gives each spawn point for a player: not read yet,
  **S**, and the overlay is absent until it is).
- **Timeline**: a bar under the viewport over the store's ring, the last
  N seconds (default 60, bounded by 256 MiB of frames; the oldest go
  first). Dragging shows the world as it was, in the viewport and the
  inspector, with marks for kills, deaths and map events. **Live** jumps
  back to now. Scrubbing moves only the editor's view: the game keeps
  running, and the editor does not send edits while the view is in the
  past.
- **Recording**: Record keeps frames past the ring; **Save recording**
  writes the ring or the recording as a `.h2mirror` file (a header with
  the map's name and checksum, the protocol and tick rate, then the
  frames as sent, starting at a key frame). **Open recording** plays one
  with no game, with the same viewport, layers and timeline. A recording
  holds positions and tag names from the user's game, so it is user
  output: refused inside a Git checkout, as map saves are. It is not a
  film: an `.h2film` replays the game itself in Theater
  ([film-format.md](../game/film-format.md)); a recording is what the
  mirror saw and cannot be played by the game.
- **Session** in the Game panel reports the stream's frame rate, delay,
  dropped and skipped counts. It also shows the game's notice when it
  sheds a layer.
- **Pending** (in the Game panel): every edit not in the game, under its
  effect's sentence, each row `tag · field · reason` with no address,
  with why (View mode, another map, the session, needs a reload), and
  the buttons that clear them: Send, Respawn now, Reload map.

## Interfaces

### The game's agent API

New methods, named in `agent_proto::live` and `agent_proto::hot`, listed
by `session.methods`, documented in
[agent-api.md](../runtime/agent-api.md) when built.

| Method | Access | Params | Result |
|---|---|---|---|
| `live.subscribe` | read | `{layers: [name], every?, key_every?, poses?: {every?, near?: {point, radius}}}` | `{at, stream, protocol: "h2-live-mirror/1", tick_rate, map: {name, checksum, epoch, activation}, layers: [name], refused: {name: reason}}`, then binary frames on this connection |
| `live.update` | read | `{stream, layers?, every?, poses?}` | the stream's settings; the next frame is a key frame |
| `live.key` | read | `{stream}` | `{at, seq}`: the next frame is a key frame, taken now when paused |
| `live.status` | read | none | `{map: {name, checksum, epoch, activation}, streams: [{stream, layers, every, frames, bytes, dropped, skipped, queue}], capture_us: {p50, p99, budget}, shed: [name]}` |
| `live.close` | read | `{stream}` | `{closed}`; closing the stream's connection does the same |
| `edit.map` | read | `{known_epoch?}` | `{at, name, file, checksum, scenario, epoch, session, bsp, loaded?}` |
| `edit.batch` | act | `{writes, label?, epoch?, confirm_peers?}`. A write is `{tag, group?, path, value, was?}` or `{at, value, was?}` with hex bytes | `{id, applied, changes}`: all written at one tick boundary as one journal entry, or none, with `MISMATCH` and `data.fields` (`path`, `was`, `holds`) |
| `world.object.set` | act | `{id, pos?, forward?, up?, epoch?, confirm_peers?}` | `{at, object}` after the next tick, through the engine's object move |
| `world.object.delete` | act | `{id, epoch?, confirm_peers?}` | `{at, deleted}` through `OBJECT_DELETE` |
| `world.object.respawn` | act | `{id, epoch?, confirm_peers?}`, `{placement: {block, index}, epoch?, confirm_peers?}` or `{definition, epoch?, confirm_peers?}` | `{at, old, object, linked}` with the replacement's placement-link status; a definition answers `{respawned, skipped, engine_made}` (agent-api.md, "Object edits") |
| `map.reload` | act | `{file?, confirm_peers?}` | `{queued, map, activation, file?, checksum?}`. Wait for gameplay on the requested map and activation. A file reload also returns the path and expected header checksum |

`sim.session` gains `live` (the session is on a Live service) and
`peers` (the total number of machines, including this host). On System
Link, `confirm_peers` echoes that total. A file reload binds this host's
copy only. Each peer needs its own matching file. The filename must match
the active map. Switching to another map requires a map switch or relaunch.
`session.subscribe` gains the
event `epoch {from, to, why}`.

The in-game check that a placed object changes cluster after a move is
pending. The game API documents the object ids, placement blocks and peer
confirmation for these writes. [Object edits](../runtime/agent-api-sim.md#object-edits)

### The editor's socket API

Namespace `mirror` in `studio_proto`, answered by a provider
(`api/mirror.rs`, [agent-api.md](agent-api.md), "Extension points"). It
needs the pairing, which `play::Play` runs over the `App`'s
`play::pair::Bridge`. The window updates its `Play` each frame;
`h2-editor --serve` keeps one beside the `App` and updates it between
requests (`Api::serve_once`), so `pair.*` and `mirror.*` work with no
window.

| Method | Params | Result |
|---|---|---|
| `mirror.status` | none | the stream (`state`, `protocol`, `layers`, `fps`, `delay_ms`, `dropped`, `skipped`, `notice`), the store (`tick`, `first_tick`, `objects`, `bytes`), `mode` (`view`, `edit`) and why Edit is off |
| `mirror.layers` | `{set?: {name: bool}, overlays?: {name: bool}}` | every layer and overlay, on or off, with its count |
| `mirror.objects` | `{kind?, near?, limit?, at_tick?}` | the store's objects as the viewport draws them: `id`, `kind`, `tag`, `pos`, `forward`, `placement`, `nodes`, at the shown tick |
| `mirror.camera` | `{mode: free \| follow \| eyes \| game, target?, view?}` | `{mode, target}`, and `view` in game mode |
| `mirror.control` | `{op: pause \| step \| resume, ticks?}` | `{requested, tick}`: queued, with the store's latest tick before it |
| `mirror.seek` | `{tick}` or `{live: true}` | the shown tick and the ring's range |
| `mirror.record` | `{op: start \| stop \| save, path?}` | `{recording, frames, bytes, path?}`; `save` is refused in a Git checkout |
| `mirror.open` | `{path}` | a recording opened with no game: its map, tick range and layers |
| `mirror.edit` | `{enabled?, confirm_peers?}` | the mode; `REFUSED` with the session's reason |
| `mirror.pending` | none | each edit not in the game: `doc`, `tag`, `path`, `effect` (an id of the effect table), `text` (its sentence), `why`, and for a tag field `label` (`tag · field`) |
| `mirror.apply` | `{what: send \| respawn \| reload \| revert}` | what was sent, respawned or queued |

`view.snapshot` draws the live objects and overlays that are on, so a
check can look at the picture.

## Slices

Each slice ships something the user sees. Lanes write code and tests;
the coordinator runs them. "Fake game" means the agent crate's server
with an invented game behind it, as `play::tests` use; no game data.

| Slice | Ships | Acceptance |
|---|---|---|
| **M1** See players and objects move (implemented) | `agent_proto::live` (frames, `objects` and `players`); the capture, worker and `live.subscribe`, `live.status`, `live.close`; `live_mirror::Store`, the editor's stream thread and interpolation; live objects drawn with their models in the bind pose, players with name, team and aim line; the 4 Hz poll's markers removed | codec round trip and delta tests in `agent-proto`; capture of a synthetic pool equals `sim.objects` of the same pool; a full queue drops and re-keys and never blocks the capture; fake game: a moved object moves in `mirror.objects` within two frames; with the real game, `mirror.objects` at a tick equals `sim.objects` at that tick, `sim.hashes` are equal with and without a subscriber on one input replay, and the note records bytes a second and `capture_us` |
| **M2** Cameras and controls (implemented) | the `camera` layer from the sealed scene; frustums; Follow, Through its eyes, Game camera; `mirror.camera`, `mirror.control`; a frame per step; `mirror.*` headless through `h2-editor --serve` | fake game with a scene source: the Game camera's view matrix equals the scene's; no source lists `camera` under `refused`; a step of one tick advances `mirror.status.tick` by one (`play/presence/camera/game_camera_tests.rs`); a windowless serve pairs and answers `mirror.objects` (`api/tests.rs`, `windowless_serve`). With the real game: Game camera matches the game's picture |
| **M3** Timeline and recordings | the ring, the scrubber, `mirror.seek`, `mirror.record`, `mirror.open`, the `.h2mirror` file | a recording saved and opened shows the same `mirror.objects` at each tick; the ring stays under its bound and drops whole key intervals; saving into a Git checkout is refused |
| **M4** Poses and animation | the `poses` layer with its interest sphere; a skinned pipeline in `scenario_edit::render`; `sim_state::AnimationView` and the `anim` layer; the shed rule | synthetic model: node matrices in, skinned vertices out match a reference; fake game: a pose change shows in `view.snapshot`; over budget, `poses` is shed and a notice sent |
| **M5** Projectiles and arcs | `ProjectileView`, the `projectiles` layer, trails, `arc::predict`, aim projection, impact marks | `arc::predict` against a closed form for gravity alone; a recorded synthetic flight stays within the stated error of its arc; a projectile-ended event leaves a mark |
| **M6** Machines and devices | `DeviceView`, the `devices` layer, position bars, the device fields read into `guest-layout` | fake game: a door's position follows the stream; fields not read are named in the inspector |
| **M7** Edit tags live | View and Edit; `edit.map`, `edit.batch` with `was`; `applies` in `tags::defs`; the baseline over tag fields; class badges; the pending list; the session rules in the game | fake game: a Tags edit is one `edit.batch`, undo sends the old bytes, a `MISMATCH` is reported and nothing written, co-op and client and Live sessions refuse in the game and the editor, View mode sends nothing |
| **M8** Edit objects live | `world.object.set`, `.delete`, `.respawn`; a moved placement moves its object; previews for added entries; Respawn now | the engine entries proven by differential tests as other ports are; fake game: a gizmo drag sends `scnr` writes and one `world.object.set` a frame at most, undo restores both |
| **M9** Reload | `map.reload`; Reload map; the baseline reset on `epoch` | fake game: adding an entry puts it under Reload, the reload clears it; a game without `map.reload` shows the save-and-load text |
| **M10** Effects | effect-start events; particle points; the `effects` layer | synthetic emitter geometry in, points out; an effect start draws its mark |
| **M11** Overlays | hitboxes, AI, nav, spawn influence | each overlay's data read by a method `session.methods` lists; spawn influence absent until its read exists |

M1 to M3 need no new reads of the game beyond `sim-state`. M4 to M6 and
M10 each start by reading the retail code for the fields they add, into
`guest-layout`, with the evidence in the engine docs.

## Rules and limits

- The stream is read-only and the capture is the only part on the game
  thread. A subscriber never makes the game wait: frames are dropped,
  captures skipped, layers shed.
- The editor never reads guest memory, and no layer is added by scraping:
  a new field is a `guest-layout` field with evidence, read through a
  `sim-state` view.
- A layer the game cannot supply is listed under `refused` with its
  reason and shown as absent. No layer is approximated from another.
- A predicted arc is ours and is drawn as a prediction; the trail is the
  game's.
- Scrubbing changes what the editor shows, never the game. Returning the
  game to an earlier moment is `state.save` and `state.load`
  ([snapshots.md](../runtime/snapshots.md)), which the mirror does not
  drive.
- A bound play copy or an unlinked placement preview prevents machine
  snapshots, snapshot rewind and film keyframes. A campaign checkpoint
  can restore a deleted preview, so its placement history lasts until
  the scenario changes. A film named with `--film` and film playback
  exclude object edits and map reloads. The first edit in a session's
  [default recording](../game/film-format.md#recording-every-game)
  discards its tape and the game in play instead; films of games already
  ended stay. After a file reload succeeds, restart the game before
  signing in to external Live.
- Continuous writes require Edit. F5 in View can authorize a single
  playtest. `SessionKind::writes` gates host edits. An external Live game
  refuses streaming; it refuses writes unless it is a custom game this
  console hosts.
- Recordings, play copies and saved maps are user output and stay out of
  Git checkouts.

## Evidence

- **Code**: the built parts are as "What exists" cites them.
- **Code**: the agent drops a client whose queue fills, so the game
  thread never waits (`OUT_QUEUE`, `crates/extensions/agent/src/server.rs`);
  the stream keeps that rule with its own queue.
- **Code**: a sealed `FrameScene` is immutable and shared
  (framescene.md 4.5), and holds each view's camera and the drawn
  instances' object handles.
- **Code**: the guest keeps particle simulation and the native stage
  reads completed geometry (native-particles.md).
- **Code**: `MapEditor` cannot add entries to the game's loaded blocks,
  and the game reads a placement when it makes the object (editor.md,
  "Play and pair").
- **Design**: the wire format, the two-connection transport, the budget,
  the shed order, View and Edit, the session table.
- **S**: every number under "Cost"; that a capture stays inside the
  budget on the Xbox; the fields of the animation manager, the device
  update and the projectile's flight flags; the entry that creates an
  effect; the engine function that moves an object and re-clusters it;
  that `sim.session` can tell a Live session and count
  peers. Each is settled by the slice that needs it.

## Not verified

- The capture's time and the stream's bytes in a real 16-player game.
- That interpolating node matrices looks right on fast animations at
  `every: 2`.
- Which tag fields the engine reads each frame, on use or at creation,
  beyond the shader rows hot-iteration.md has.
- That a spawned preview object behaves as the placed one would.
- Edits on a System Link host with clients in the game.
- The stream from the Xbox app over TCP.
- That Game camera lines the editor's render up with the game's picture
  on a real run, and how far the sealed scene's camera lags the tick.

## Related

- [click-to-edit.md](click-to-edit.md): click an object to edit its tags,
  the merged editor, Halo 1 tags.
- [editor.md](editor.md), "Play and pair"; [agent-api.md](agent-api.md).
- [agent-api.md](../runtime/agent-api.md): the game's API.
- [hot-iteration.md](../graphics/hot-iteration.md): `edit.*` and what
  shows when.
- [framescene.md](../graphics/framescene.md): the frame as data.
- [sim-rewrite-objects.md](../engine/sim-rewrite-objects.md) section 8,
  [guest-layout.md](../engine/guest-layout.md): the typed reads.
