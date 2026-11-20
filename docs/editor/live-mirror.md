# Live mirror: the running game in the editor, and edits back into it

Status: Design. Nothing here is built; "What exists" lists the built parts it stands on.
Code: none yet. It adds `crates/tools/live-mirror` and `agent_proto::live`, and changes `crates/extensions/agent`, `crates/rewrites/sim-state`, `crates/extensions/hot-tags`, `crates/apps/studio/src/play` and `crates/tools/scenario-edit/src/render.rs`; each slice names its files and tests.
Switch: none. A game with no subscriber pays one relaxed load a tick.

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
| Pairing, auto-pair, the status chip | Built | `studio::play` (`play.rs`, `play/pair.rs`), `pair.*` ([agent-api.md](agent-api.md)) |
| Live objects as markers | Built: `session.status` and `world.query` polled four times a second, at most 300 objects, a marker and a facing line each | `play/link.rs` (`POLL`, `OBJECTS`), `ui/play.rs` |
| Pause, step, resume, go to player, teleport | Built | `play.rs`; `session.claim`, `pause`, `step`, `sim.state.set` |
| Placement field edits sent live | Built: a diff against `sync::Baseline` every 200 ms, each field as one `edit.set` raw path on the game's `scnr`; shows when the object is next made | `play/sync.rs` |
| Tag field writes in the game | Built: `edit.get`, `edit.set`, a journal and `edit.revert`; no editor UI sends them | `crates/extensions/hot-tags`, [hot-iteration.md](../graphics/hot-iteration.md) |
| No writes in co-op or as a client | Built | `agent_proto::sim::SessionKind::writes` |
| Typed object reads | Built: transform, velocity, sphere, vitality, node matrices, unit player, team, seat, weapons and aim | `crates/rewrites/sim-state` (`ObjectView`, `UnitView`), over `crates/hooking/guest-layout` |
| The tick's events | Built: `deleted`, `damage`, `kill`, while `impl.sim.native.objects` is on | `sim_state::events`, `sim.events` |
| The game's cameras, drawn instances, lights | Built for a run that renders with the native renderer | the sealed `FrameScene` (`frame_scene::View`, `Instance`), [framescene.md](../graphics/framescene.md) section 4 |
| Object to placement link, a machine's position | Built | `live_telemetry::memory` (`placement`), `guest_layout::unit::Device::POSITION` |
| Per-tick rows taken inside the tick observer | Built, as JSON to a file | `crates/extensions/agent/src/trace.rs` |
| Models in the viewport | Built in the bind pose, first permutation, instanced per model | `scenario_edit::render::Viewport`, [editor.md](editor.md), "What the viewport shows" |
| Map save, packages, play copy | Built | `scenario_edit::save`, `MapEditor`, [scenario-editor.md](scenario-editor.md) section 5 |

New here: the stream and its wire format, the capture, the editor's
store, interpolation, timeline and recordings, posed models, projectile
arcs, the device, effect and animation reads, tag pokes from the editor
with apply classes, moving and making live objects, the reload path and
the overlays.

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
reader skips a layer it does not know. The types and both codecs are
`agent_proto::live`, with no guest and no editor dependency.

| Layer | Carries | Source |
|---|---|---|
| `objects` | per live object: slot and salt, definition tag, type, header flags, parent, position, forward, up, scale, velocity, body and shield vitality, the placement that made it | `sim_state::ObjectView`; `placement` moves from `live_telemetry::memory` to a `guest-layout` field |
| `players` | per player: index, name (key frames only), team, unit, held weapon, aim vector, local user | `UnitView`, the player reads `world.query` uses |
| `poses` | per posed object: its node matrices, 13 floats a node, world space | `ObjectView::nodes` |
| `anim` | per animated object: the animation graph tag, the animation and frame each channel plays | new `sim_state::AnimationView` over `Object::ANIMATION_MANAGER` |
| `projectiles` | per projectile: the `objects` fields plus owner, flight flags, ticks alive | new `sim_state::ProjectileView` |
| `devices` | per machine and control: position 0 to 1, and power and target once read | `Device::POSITION`, new `sim_state::DeviceView` |
| `camera` | per game view: position, forward, up, field of view, near, far, viewport, the cut flag; the handles of the instances the game drew | `frame_scene::View::camera`, `Instance::key` |
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
   `FrameScene` (`agent::render::set_scene_source`): a sealed scene is an
   immutable `Arc`, so it costs the game thread nothing.
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
holds and greys the status chip. **Latest** (no delay, no blend) is a
toggle, for watching an edit land.

Pause, step and resume are the Game panel's, unchanged. A step sends its
frame, so stepping one tick moves the viewport one tick. In `h2 play` a
pause stops the game window's event loop (agent-api.md, "Control").

### Cameras

| Mode | Does |
|---|---|
| Free | the editor's fly, orbit and pan camera (`camera.rs`), as now |
| Follow | orbits a chosen player or object at a kept offset; optionally turns with its aim |
| Through its eyes | at the unit's head node looking along its aim; the editor's own render, not the game's picture |
| Game camera | takes a game view's camera from the `camera` layer exactly, field of view included |

In every mode but Game camera, each game view's frustum is drawn as a
wire pyramid from the `camera` layer, one per split-screen player, and
objects the game drew that frame can be tinted (the `drawn` overlay). A
run with no sealed scene (a headless `h2 serve` with no GPU view) has no
`camera` layer: `live.subscribe` lists it under `refused` with the
reason, and the editor says the game has no camera. It does not invent
one from a player's aim.

## Live edit

**View** and **Edit** are a switch in the Game panel and the toolbar.
Pairing starts in View: nothing is written. Edit is the user's choice. A
game the editor started with Play starts in Edit, since it is the
editor's own copy of the map.

### How an edit applies

Each edit has an apply class, shown as a badge on the field and in the
pending list.

| Class | Edits | Reaches the game by | Shows |
|---|---|---|---|
| **Poke, each frame** | values the draw path reads every draw: shader colours, vectors and bitmaps, effect and light colours where the draw reads the tag | `edit.batch` | next frame |
| **Poke, next use** | values read when used: damage, rates of fire, machine speeds and delays, a projectile's drag | `edit.batch` | the next shot, hit or cycle |
| **Poke, next creation** | values copied into an object when it is made: a projectile's initial speed, an object's vitality, placement fields | `edit.batch`, then optionally a respawn | objects made after it; **Respawn now** remakes the ones that exist |
| **Poke, unknown** | any field with no recorded class | `edit.batch` | written at once; the badge says when it shows is not known |
| **Object** | moving, turning or deleting a thing that exists; previewing a new placement | `world.object.set`, `.delete`, `.respawn`, `sim.spawn` | next tick |
| **Reload** | a block's count changing (placements added or removed, block elements), new tags, new strings, grown blocks, structure BSP and lightmap edits, bitmap pixels of another size | saving the play copy and loading the map again | after the reload |

The class of a field is data, not a guess made in the UI: `tags::defs`
gains an `applies` attribute (`frame`, `use`, `creation`) with its
evidence, as names have theirs, and the table in
[hot-iteration.md](../graphics/hot-iteration.md), "What shows when", is
its first content. A field with none is `unknown`.

**Placements.** Moving a placement writes its `scnr` fields as today.
In Edit, the object made from it (matched by the `objects` layer's
placement link) is also moved with `world.object.set`, so the thing
follows the gizmo. Adding a placement cannot grow the game's block, so
the editor spawns a preview object with `sim.spawn` from the entry's tag
and transform and marks the entry "in the game as a preview until the map
reloads". Deleting one deletes its object and marks the entry "still in
the game's scenario until the map reloads", since a respawn timer may
make it again.

**Reload.** Pending reload edits collect under one **Reload map** button.
For a game the editor started it is Play again: the play copy is written
and the game relaunched (`play/launch.rs`). For a game it only paired
with, the editor writes the play copy and calls `map.reload {file}`; a
game that cannot (see Interfaces) leaves the button saying to save the
map and load it in the game.

### Undo and sync

The editor's document is the truth, and the game is brought to it.
`sync::Baseline` extends from placement blocks to every tag field the
editor has sent: for each (tag, path), the bytes the game is taken to
hold. After any change, undo or redo, the diff against the baseline is
the set of writes to send, so undo sends the old bytes back with no
second undo log. Each write carries `was`, the bytes the baseline
expects; the game refuses a batch whose `was` differs
(`MISMATCH`, naming the field and what it holds), which replaces "trust
that the game runs the map as shown". Object edits keep an inverse beside
the undo step: a move's old transform, a preview's object to delete.

On pairing, `edit.map` gives the loaded map's name, file and checksum.
Same checksum as the file the editor's document came from: the baseline
is that file, and unsaved edits show as pending until the user picks
Edit, which sends them. Another checksum: View only, with the reason. A
new map epoch (a reload, `state.load`, a revert) resets the baseline to
the file the game loaded and the pending list is built again.

Saving is the existing path and is not changed by pairing: **Save map
as** and **Save as package** write the document
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
| Multiplayer client | yes: what this machine holds | no: the host overwrites it |
| A Live session (not the local embedded server of `--live-local`) | no stream while a game is in progress: it would show one player every position | no |
| Session unknown | yes | no |

The game enforces this, not only the editor: `edit.set`, `edit.batch`,
`world.object.*` and `map.reload` refuse by `SessionKind::writes`, as
`sim.state.set` does, and refuse on a Live session whatever its kind.
`live.subscribe` refuses on a Live session in a game. Nothing is ever
written without the Edit switch on, and the switch never turns itself on
for a game the editor did not start. For a changed map in a networked
game, every machine loads the same saved map, as now
([editor-guide.md](editor-guide.md)).

## The editor's UI

- **Layers** (a panel beside the outliner): a tree of toggles, each with
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
- **Status chip**: as now, plus the stream's frame rate, delay, dropped
  and skipped counts, and the game's notice when it shed a layer.
- **Pending** (in the Game panel): every edit not in the game, by class,
  with why (View mode, another map, the session, needs a reload), and
  the buttons that clear them: Send, Respawn now, Reload map.

## Interfaces

### The game's agent API

New methods, named in `agent_proto::live` and `agent_proto::hot`, listed
by `session.methods`, documented in
[agent-api.md](../runtime/agent-api.md) when built.

| Method | Access | Params | Result |
|---|---|---|---|
| `live.subscribe` | read | `{layers: [name], every?, key_every?, poses?: {every?, near?: {point, radius}}}` | `{at, stream, protocol: "h2-live-mirror/1", tick_rate, map: {name, checksum, epoch}, layers: [name], refused: {name: reason}}`, then binary frames on this connection |
| `live.update` | read | `{stream, layers?, every?, poses?}` | the stream's settings; the next frame is a key frame |
| `live.key` | read | `{stream}` | `{at, seq}`: the next frame is a key frame, taken now when paused |
| `live.status` | read | none | `{streams: [{stream, layers, every, frames, bytes, dropped, skipped, queue}], capture_us: {p50, p99, budget}, shed: [name]}` |
| `live.close` | read | `{stream}` | `{closed}`; closing the stream's connection does the same |
| `edit.map` | read | none | `{at, name, file, checksum, scenario, epoch, session}` |
| `edit.batch` | act | `{writes: [{tag, group?, path, value, was?}], label?}` | `{id, applied, changes}`: all written at one tick boundary as one journal entry, or none, with `MISMATCH` and `data.fields` (`path`, `was`, `holds`) |
| `world.object.set` | act | `{id, pos?, forward?, up?, velocity?}` | `{at, object}` after the next tick; through the engine's own move, so the object is re-clustered |
| `world.object.delete` | act | `{id}` | `{at, deleted}`; through `OBJECT_DELETE` |
| `world.object.respawn` | act | `{id}` or `{placement: {block, index}}` | `{at, old, object, linked}`: the object deleted and made again from the game's scenario bytes; `linked` says whether the new object is tied to the placement |
| `map.reload` | act | `{file?}` | `{queued, map}`: the running map queued again, from `file` when given |

`sim.session` gains `live` (the session is on a Live service) and
`peers` (other machines in the game). `session.subscribe` gains the
event `epoch {from, to, why}`.

`world.object.set` is the method editor.md and hot-iteration.md name as
missing. It is added, with `delete` and `respawn`, because moving a live
object will be needed by every later tool and test that places things;
`world.poke` on a position would skip the engine's re-clustering.

### The editor's socket API

New namespace `mirror` in `studio_proto`, answered by a provider
(`api/mirror.rs`, [agent-api.md](agent-api.md), "Extension points"). It
needs the pairing, so it moves `play::Play` from the window to the `App`
and lets `h2-editor --serve` pair; `pair.connect` then works headless.

| Method | Params | Result |
|---|---|---|
| `mirror.status` | none | the stream (`state`, `protocol`, `layers`, `fps`, `delay_ms`, `dropped`, `skipped`, `notice`), the store (`tick`, `first_tick`, `objects`, `bytes`), `mode` (`view`, `edit`) and why Edit is off |
| `mirror.layers` | `{set?: {name: bool}, overlays?: {name: bool}}` | every layer and overlay, on or off, with its count |
| `mirror.objects` | `{kind?, near?, limit?, at_tick?}` | the store's objects as the viewport draws them: `id`, `kind`, `tag`, `pos`, `forward`, `placement`, `nodes`, at the shown tick |
| `mirror.camera` | `{mode: free \| follow \| eyes \| game, target?, view?}` | the camera mode and what it follows |
| `mirror.control` | `{op: pause \| step \| resume, ticks?}` | the game's stamp after it |
| `mirror.seek` | `{tick}` or `{live: true}` | the shown tick and the ring's range |
| `mirror.record` | `{op: start \| stop \| save, path?}` | `{recording, frames, bytes, path?}`; `save` is refused in a Git checkout |
| `mirror.open` | `{path}` | a recording opened with no game: its map, tick range and layers |
| `mirror.edit` | `{enabled?, confirm_peers?}` | the mode; `REFUSED` with the session's reason |
| `mirror.pending` | none | each edit not in the game: `doc`, `tag`, `path`, `class`, `why` |
| `mirror.apply` | `{what: send \| respawn \| reload \| revert}` | what was sent, respawned or queued |

`view.snapshot` draws the live objects and overlays that are on, so a
check can look at the picture.

## Slices

Each slice ships something the user sees. Lanes write code and tests;
the coordinator runs them. "Fake game" means the agent crate's server
with an invented game behind it, as `play::tests` use; no game data.

| Slice | Ships | Acceptance |
|---|---|---|
| **M1** See players and objects move | `agent_proto::live` (frames, `objects` and `players`); the capture, worker and `live.subscribe`, `live.status`, `live.close`; `live_mirror::Store`, the editor's stream thread and interpolation; live objects drawn with their models in the bind pose, players with name, team and aim line; the 4 Hz poll's markers removed | codec round trip and delta tests in `agent-proto`; capture of a synthetic pool equals `sim.objects` of the same pool; a full queue drops and re-keys and never blocks the capture; fake game: a moved object moves in `mirror.objects` within two frames; with the real game, `mirror.objects` at a tick equals `sim.objects` at that tick, `sim.hashes` are equal with and without a subscriber on one input replay, and the note records bytes a second and `capture_us` |
| **M2** Cameras and controls | the `camera` layer from the sealed scene; frustums; Follow, Through its eyes, Game camera; `mirror.camera`, `mirror.control`; a frame per step; `Play` on the `App`, `mirror.*` headless | fake game with a scene source: the Game camera's view matrix equals the scene's; no source lists `camera` under `refused`; a step of one tick advances `mirror.status.tick` by one |
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
- No write without Edit on; no write in a session `SessionKind::writes`
  refuses; no stream and no write on a Live session in a game.
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
  that `map.reload` can queue the running map again from another file
  (`maps.test.load` queues a game through `Call::QueueGame`, which is
  the route to try); that `sim.session` can tell a Live session and count
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
