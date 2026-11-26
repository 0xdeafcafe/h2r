# Agent API: driving the running game without pixels

Status: Built. Every method below is registered by the code; the ones no run has exercised are named under "Not verified".
Code: `crates/extensions/agent-proto` (wire types, blocking client), `crates/extensions/agent` (server, tick-boundary executor, most methods), `crates/extensions/game/src/agent_glue.rs` (host-answered methods), `crates/apps/cli/src/agent_cmd.rs` (`h2 agent`). Tests: `crates/extensions/agent/src/exec/tests.rs`, `crates/extensions/agent/src/sim/tests.rs`, `crates/extensions/agent/src/trace/tests.rs`, `crates/apps/boot/tests/feature_plan.rs`.
Switch: `agent` feature (`H2_FEATURES=-agent` removes the tick hooks), `H2_AGENT` (the socket), listed in [switches.md](switches.md).

An agent, a script or a test reads and drives a running Halo 2 through one
JSON-RPC interface: level contents, frame cost, menus, and putting the game
in a state and acting in it. It replaces screenshots, pasted logs and
launch-time environment variables. The client is `h2 agent`; the editor
uses the same endpoint.

Reads of game state run on the game's thread at a boundary, and the `at`
stamp names that moment. Host reads such as `host.memory` use their own
sample clock and carry the game stamp at reply time.

## Method index

`session.methods` lists what the running process registers; a host method
appears only where the host adds it. Access (`agent::Access`): **read**
answers any connection; **act** answers any connection while none holds
control, else only the controller (`NOT_CONTROLLER`); **claim** needs this
connection to hold `session.claim`.

| Method | Access | Section |
|---|---|---|
| `session.status`, `methods`, `claim`, `release`, `wait`, `subscribe` | read | [`session`](#session-what-is-running-and-its-clock) |
| `session.takeover_info`, `takeover_status` | read | [Lab consent](#lab-consent) |
| `session.request_takeover` | act | [Lab consent](#lab-consent) |
| `live.subscribe`, `live.status`, `live.close` | read | [Live mirror](#live-mirror) |
| `session.pause`, `resume`, `step` | claim | [Control](#control) |
| `host.pause`, `host.step` | act, without the controller check | [Window pause and frame step](#window-pause-and-frame-step) |
| `host.screenshot` | act, without the controller check | [Window screenshots](#window-screenshots) |
| `host.quit` | act, without the controller check | [Close the window](#close-the-window) |
| `host.window.resize` | act, without the controller check | [Resize the window](#resize-the-window) |
| `world.count`, `world.query`, `world.object` | read | [`world`](#world-what-is-in-the-level) |
| `world.object.set`, `world.object.delete`, `world.object.respawn` | act | [Object edits](#object-edits) |
| `world.object.animate` | act | [`world.object.animate`](#worldobjectanimate) |
| `world.placement.edit` | act | [Placement edits](#placement-edits) |
| `world.ai` | read | [`world.ai`](#worldai) |
| `world.breakable` | read | [`world.breakable`](#worldbreakable-broken-glass) |
| `net.replication` | read | [`net.replication`](#netreplication) |
| `actors.net` | read | [`actors.net`](#actorsnet) |
| `world.raw` | read | [`world.raw`](#worldraw-an-objects-bytes-before-and-after) |
| `world.poke` | claim | [`world.poke`](#worldpoke-replace-bytes-in-one-object-part) |
| `havok.bodies`, `havok.agents`, `havok.contacts`, `havok.digest` | read | [`havok`](#havok-the-physics-world) |
| `havok.pair.set` | claim | [`havok.pair.set`](#havokpairset) |
| `superbounce.state` | read | [`superbounce`](#superbounce-the-superbounce-game-type) |
| `superbounce.set` | act | [`superbounce`](#superbounce-the-superbounce-game-type) |
| `superbounce.reload`, `superbounce.tune` | act | [`superbounce`](#superbounce-the-superbounce-game-type) |
| `superbounce.medal_test` | act | [`superbounce`](#superbounce-the-superbounce-game-type) |
| `superbounce.assist` | read | [`superbounce`](#superbounce-the-superbounce-game-type) |
| `superbounce.assist_test` | act | [`superbounce`](#superbounce-the-superbounce-game-type) |
| `superbounce.verify`, `superbounce.verify_cancel` | act | [`superbounce`](#superbounce-the-superbounce-game-type) |
| `superbounce.verify_status` | read | [`superbounce`](#superbounce-the-superbounce-game-type) |
| `superbounce.view` | read | [`superbounce`](#superbounce-the-superbounce-game-type) |
| `superbounce.view_test` | act | [`superbounce`](#superbounce-the-superbounce-game-type) |
| `superbounce.ghosts` | read | [`superbounce`](#superbounce-the-superbounce-game-type) |
| `superbounce.ghost_show` | act | [`superbounce`](#superbounce-the-superbounce-game-type) |
| `pogo.state` | read | [`pogo`](#pogo-superbounces-pogo-stick) |
| `pogo.test` | act | [`pogo`](#pogo-superbounces-pogo-stick) |
| `profile.counters`, `profile.capture`, `profile.fallbacks`, `profile.memory`, `profile.faults` | read | [`profile`](#profile-where-a-frame-goes) |
| `profile.sample.start`, `profile.sample.stop` | act | [`profile`](#profile-where-a-frame-goes) |
| `profile.sample.status` | read | [`profile`](#profile-where-a-frame-goes) |
| `render.frame`, `render.stats` | read | [`render`](#render-the-frame-and-the-pipeline) |
| `render.marks` | read | [`render.marks`](#rendermarks) |
| `render.markers` | read | [`render.markers`](#rendermarkers) |
| `render.frame_layout` | read | [Display raster and texture coordinates](#renderframe_layout) |
| `render.capture_clip`, `render.capture_clip.status` | act, read | [Headless display clips](#rendercapture_clip) |
| `render.picture.open`, `render.picture.close` | read | [Picture feed](#picture-feed-renderpicture) |
| `render.coop_look` | read | [`render`](#render-the-frame-and-the-pipeline) |
| `render.compare`, `render.capture`, `render.scene`, `render.view.current`, `sim.hashes` | read | [FrameScene checks](#framescene-checks-compare-capture-scene-parity-hashes) |
| `render.spawn_view`, `render.view` | act | [FrameScene checks](#framescene-checks-compare-capture-scene-parity-hashes) |
| `render.parity` | act | [FrameScene checks](#framescene-checks-compare-capture-scene-parity-hashes) |
| `render.residual` | read | [Residual D3D work](#renderresidual-residual-d3d-work) |
| `render.particles.refusals` | read | [`render.particles.refusals`](#renderparticlesrefusals) |
| `render.overrides`, `render.programs` | read | [`edit` and hot `render`](#edit-and-hot-render-live-tag-and-shader-changes) |
| `render.reload`, `render.dump`, `render.param`, `render.toggle` | act | [`edit` and hot `render`](#edit-and-hot-render-live-tag-and-shader-changes) |
| `edit.map`, `edit.tags`, `edit.get`, `edit.changes` | read | [`edit` and hot `render`](#edit-and-hot-render-live-tag-and-shader-changes) |
| `edit.set`, `edit.batch`, `edit.revert` | act | [`edit` and hot `render`](#edit-and-hot-render-live-tag-and-shader-changes) |
| `edit.arena` | read | [Block moves](#block-moves) |
| `edit.collision.stage`, `edit.collision.commit` | act | [Collision edits](#collision-edits) |
| `load.timeline` | read | [Load timeline](#load-timeline) |
| `game.menu` | act | [Returning to the main menu](#returning-to-the-main-menu) |
| `game.load`, `game.levels` | act, read | [Loading a level](#loading-a-level) |
| `game.code`, `game.code.restart` | read, act | [Game code](#game-code-full-speed-or-the-slower-start) |
| `features.list` | read | [`session`](#session-what-is-running-and-its-clock) |
| `live.join.open`, `live.join.status` | act, read | [Live session links](#live-session-links) |
| `live.matchmaking.trace` | read | [Matchmaking transitions](#matchmaking-transitions) |
| `host.metrics`, `host.memory`, `host.audit`, `host.perf` | read | [`host`](#host-the-process-and-its-lobby) |
| `host.memory.history`, `heap`, `gpu`, `diff`, `postmortem` | read | [Finding a leak after the fact](#finding-a-leak-after-the-fact) |
| `host.lobby.open`, `close`, `start` | act | [`host.lobby`](#hostlobby) |
| `host.lobby.state` | read | [`host.lobby`](#hostlobby) |
| `ui.screen`, `ui.campaign`, `ui.audit`, `ui.stack`, `ui.lobby.players`, `ui.lobby.tabs`, `ui.browser.filters`, `ui.attract` | read | [`ui`](#ui-what-the-menus-show) |
| `ui.press` | act | [Menu inspection and navigation](#menu-inspection-and-navigation) |
| `audio.voices`, `audio.listeners`, `audio.loops` | read | [`audio`](#audio-voice-routing) |
| `audio.meter` | read | [`audio.meter`](#audiometer) |
| `audio.play` | act | [`audio.play`](#audioplay) |
| `recording.play` | act | [`recording.play`](#recordingplay) |
| `hs.threads`, `hs.globals`, `hs.player_actions` | read | [HaloScript state](#haloscript-state) |
| `hs.parity` | read | [HaloScript parity](#hsparity) |
| `hs.eval` | read | [`hs.eval`](#hseval) |
| `input.pad`, `release`, `record`, `replay`, `stop` | act | [`input`](#input-act-like-a-player) |
| `input.state`, `input.host` | read | [`input`](#input-act-like-a-player) |
| `input.route`, `input.route.stop` | act | [Routes](#routes) |
| `input.route.status` | read | [Routes](#routes) |
| `state.save`, `load`, `list`, `drop`, `fork` | act (`list` read) | [`state`](#state-machine-snapshots) |
| `bots.stats` | read | [Bot combat](#bot-combat) |
| `bots.bounce_to` | act | [Bounce routes](#bounce-routes) |
| `bots.bounce_status`, `superbounce.spots` | read | [Bounce routes](#bounce-routes) |
| `player.goto`, `player.goto_cancel` | act | [`player.goto`](#playergoto-drive-the-player-somewhere) |
| `player.goto_status` | read | [`player.goto`](#playergoto-drive-the-player-somewhere) |
| `player.mark`, `player.recall` | act | [Saved places](#saved-places-playermark-and-playerrecall) |
| `sim.*` | read or act | [`sim`](#sim-simulated-players-and-direct-state) |
| `forge.status`, `forge.things`, `forge.doc`, `forge.log`, `forge.propose`, `forge.input`, `forge.look`, `forge.look.set`, `forge.status.monitors`, `forge.ground`, `forge.ground.build`, `forge.nav` | read or act | [Local Forge session](#local-forge-session) |
| `forge.capacity`, `forge.probe` | read, act | [Forge probes](#forge-probes) |
| `firefight.state`, `firefight.kit` | read | [Firefight](#firefight) |
| `firefight.command` | act | [Firefight](#firefight) |
| `rules.state`, `rules.zones`, `rules.reload` | read, act | [Rules scripts](#rules-scripts) |
| `variants.quick`, `variants.quick.set` | read, act | [Game variants](#game-variants) |
| `match.status`, `match.report`, `match.stop`, `sim.guest_census` | read, read, act, read | [Sim matches](#sim-matches) |
| `debug.views`, `debug.view` | read, act | [World debug views](#world-debug-views) |
| `settings.list`, `settings.set`, `perf.overlay`, `cheat.list`, `cheat.set` | read or act | [Menu methods](#menu-methods-settings-perf-overlay-cheats) |
| `editor.link`, `editor.focus` | read, act | [Paired editor](#editor-a-paired-editor) |
| `editor.marks` | read | [Paired editor](#editor-a-paired-editor) |
| `theater.*` | read or act | [Theater transport](#theater-transport) |
| `collision.stats` | read | [`collision`](#collision-the-native-collision-world) |
| `collision.trace` | read | [`collision`](#collision-the-native-collision-world) |
| `collision.sources`, `collision.sources.clear` | read, claim | [`collision`](#collision-the-native-collision-world) |
| `anim.graph_walk`, `anim.manager` | read | [`anim`](#anim-the-animation-graph-walk) |
| `sim.arms` | read | [`sim.arms`](#simarms-the-native-sim-gate-read) |
| `input.list`, `input.log.read` | read | [Input logs](#input-act-like-a-player) |
| `trace.*`, `flight.*` | read | [Tick traces and the flight recorder](#tick-traces-and-the-flight-recorder) |
| `shadow.enable`, `shadow.disable`, `shadow.status` | act, act, read | [Shadow checks](#shadow-checks) |
| `map.reload` | act | [Test maps](#test-maps) |
| `maps.load`, `maps.test.list`, `create`, `load`, `remove` | act, read, act, act, act | [Test maps](#test-maps) |
| `packages.list` | read | [Content packages](#content-packages) |
| `store.status` | read | [The tag store](#the-tag-store) |

A new method gets a row here and a section for its domain. The names
live in `crates/extensions/agent-proto/src` (`method::ALL`, `hot::method::ALL`,
`state::method::ALL`, `sim`, `forge`, `firefight`, `rules`, `trace`, `theater`, `goto`, `host_pause`, `host_shot`, `game_load`, `picture`) and where the host adds
them (`Agent::add_method`, `Agent::add_game_method`, `Agent::add_connection_read`).

## Connecting

```sh
H2_OPT=2 h2 serve disc/halo2.iso --out <the image you built>   # never builds
h2 agent status
h2 agent count
h2 agent query '{"kind":["actor"],"near":{"player":0,"radius":30},"fields":["pos","team"]}'
h2 agent watch phase map
printf 'claim\npause\nstep 30\ncount\nresume\n' | h2 agent -
```

- **The game that serves.** `h2 serve` is `h2 run` with the agent socket
  on, no window, optional GPU rendering and optional real-time pacing
  (`H2_BOOT_REALTIME=1`). `h2 play` with `H2_AGENT` set serves the same
  API with a window (`h2 lab run --serve`, `h2 lab bench`). Agents attach
  to a running game and never build one. `h2 serve --system-link
  --host-light` always paces to real time ([dedicated-host.md](../live/dedicated-host.md)).
- **The socket.** `--socket PATH`, then `$H2_AGENT`, then `agent.sock` in
  the user's cache directory (`agent_proto::default_socket`; see
  [storage.md](storage.md#host-directories)). Local clients use an AF_UNIX
  stream (mode 0600); `tcp://HOST:PORT` is unauthenticated TCP, including
  `h2 agent --xbox HOST` on port 4720 ([xbox.md](xbox.md)). A stale socket
  file is replaced at startup.
- **Another Mac.** `h2 lab agent --host H <run id> <call>` runs `h2 agent
  <call>` against that run's socket; `h2 lab bench` waits, captures and
  reads `render` and `counters` in one command ([lab.md](lab.md#driving-a-run-the-agent-api)).
- **The CLI.** `h2 agent <method> [JSON]` calls any method; `h2 agent
  --help` lists the short forms. `h2 agent -` reads a script on stdin, one
  call per line, on one connection. `h2 agent run <script>` adds `expect`
  lines and one JSON verdict ([Scripted scenarios](#scripted-scenarios-with-a-verdict)).
  `h2 agent poke ...` takes a claim. A result prints as JSON; an error goes
  to stderr and exits 1.
- **Settings of a headless run.** With no settings store, menus stay
  retail. `--settings DIR` on `serve` and `run` builds the store as
  `h2 play` does (`boot::setting_defs::load`, `crates/apps/cli/src/boot_cmd.rs`);
  menu changes are written back to `DIR/settings.toml`, so pass a copy.
- **Cost.** The agent is one more `GAME_TICK` observer, which costs
  dispatch transfers as `lockstep`'s does: compare runs that all have it or
  all lack it. With `H2_FEATURES=-agent` requests are answered only between
  slices, and pause and step are refused.

Details: see the [detail note](notes/2026-11-26-agent-api-detail.md), "Connecting".

### Finding a game

`h2 play` with `H2_AGENT` unset opens `agents/<pid>.sock` under the cache
directory (`agent_proto::discover`, `Host::discoverable_agent`), so the
[lab](lab.md) can find it; `H2_AGENT=off` suppresses it ([switches.md](switches.md)).
`h2 serve`, `h2 run` and the Xbox app open a socket only where `H2_AGENT` or
`--socket` says. `h2 agent games` lists each game's socket, pid, map and
phase (`discover::scan` removes dead sockets); a call with no socket named
uses the one game in the directory. The editor pairs with these games
([editor-guide.md](../editor/editor-guide.md), "Play from the editor").

## Wire

JSON-RPC 2.0, one JSON object per line each way (`crates/extensions/agent-proto/src/lib.rs`).
A request with an `id` gets one response. Valid JSON that is not a request
gets `-32600`; unknown params fields are refused; batches are not
supported. Method names are `domain.verb`.

**The stamp.** Every result carries `at` (`agent_proto::At`):

```json
{"at": {"tick": 48211, "ticks": 52000, "activation": 3, "map_ticks": 1900, "present": 96400,
        "map": "<map>", "mode": "campaign", "bsp": 1,
        "phase": "gameplay", "paused": false, "boundary": "tick"}}
```

| Field | Meaning |
|---|---|
| `tick` | the game's own count; absent when no game state is readable |
| `ticks` | the agent's count since the game started; never resets |
| `map_ticks` | ticks since the map loaded (first tick reports 1) |
| `activation` | identifies the full game start the editor observes; zero before adoption |
| `present` | frames presented |

Activation rules: see the [detail note](notes/2026-11-26-agent-api-detail.md), "Wire".

- **Phase.** `loading` (no tick for 500 ms: `agent::TICK_STALE`), `menu`
  (mode `main_menu`), `gameplay` (campaign or multiplayer), `cinematic`
  (between `cinematic_start` and `cinematic_stop`). A level's first tick
  precedes its opening cutscene, so ask for
  `{"phase":"gameplay","map_ticks":30}` to mean "playing".
- **Boundary.** `tick`: read right after a tick returned, one consistent
  state. `slice`: read between scheduler slices with no tick for 500 ms,
  where guest memory may be mid-update. The agent observes `GAME_TICK`
  last (`crates/apps/boot/tests/feature_plan.rs`).

**Errors** (`agent_proto::code`):

| Code | Name | Meaning |
|---|---|---|
| 1 | `NOT_CONTROLLER` | a control or act method while another connection holds control |
| 2 | `TIMEOUT` | `session.wait` or a deferred answer ran out of time |
| 3 | `GONE` | the object id names an object no longer in the world, or its slot now holds another |
| 4 | `UNAVAILABLE` | the game cannot answer now: nothing loaded, a read faulted, frame stats off, a feature absent |
| 5 | `SHUTDOWN` | the server is stopping |
| 6 | `INTERRUPTED` | a `session.step` ended early |
| 7 | `BUSY` | `state.save` or `state.load` never found a quiescent boundary; `data.blockers` says what stayed live |
| 11 | `REFUSED` (`agent_proto::sim::REFUSED`) | the running session, placement or map cannot support the requested edit |


**Connections.** A client that half-closes (`nc -N`) still gets its
answers. Closing releases control. Each connection queues at most 4096
lines (`OUT_QUEUE` in `crates/extensions/agent/src/server.rs`); a client that
lets it fill is dropped and its control released.

**Host methods.** A host adds methods with `Agent::add_method(name,
Access::Read | Act | Claimed, f)` (answered between slices too, where the
method does not read guest memory) or `Agent::add_game_method` (answered on
the game's thread with guest memory at hand).

## Live mirror

The editor opens a second connection, calls `live.subscribe`, reads the JSON
reply line, then reads length-prefixed binary frames (layout and blocking
decoder in `agent_proto::live`). Control calls stay on the first connection.

| Method | Params | Result |
|---|---|---|
| `live.subscribe` | `{layers: ["objects", "players", "camera"], every?, key_every?}` | `{at, stream, protocol: "h2-live-mirror/1", tick_rate, map: {name, checksum, epoch, activation}, layers, refused}`. The stream id is numeric. An unavailable checksum is null. Each unsupported layer has a reason under `refused`. |
| `live.status` | none | `{map: {name, checksum, epoch, activation}, streams, capture_us: {p50, p99, budget}, shed}`. Stream fields are `stream`, `layers`, `every`, `frames`, `bytes`, `queue`, `dropped` and `skipped`. Frames and bytes count complete writes, including the length prefix. |
| `live.close` | `{stream}` | `{closed: true}` if the stream existed. Disconnecting the binary socket also ends its subscription. |

`every` samples one tick in 1–30 (default 1). `key_every` requests a
full frame at least every specified number of game ticks (default 150).
An empty accepted layer set yields header-only frames. The first frame and frames after a cut or a dropped sample are keys.

A frame starts with a little-endian u32 payload length (cap 8 MiB). Layers
are `objects`, `players` and `camera` (id 7, the views of the last sealed
frame scene, `frame_scene::View`, source set by `Agent::set_camera_source`).
A game in a remote Live match refuses a subscription; the embedded service
of `--live-local` supports it. `scripts/check-live-mirror.py` compares
hashes with and without a subscriber. Frame format, camera layer and capture
budget: see the [detail note](notes/2026-11-26-agent-api-detail.md), "Live mirror".

## `session`: what is running and its clock

| Method | Params | Result |
|---|---|---|
| `session.status` | none | `at`, `protocol`, `fps` (frame-stats window, when collected), `controller` (this connection holds control), `claimed` (any does) |
| `session.methods` | none | every method this process answers |
| `session.claim`, `session.release` | none | take or give up control; closing the connection releases it |
| `session.wait` | `{until, timeout_ticks?, timeout_ms?}` | `{at, ticks}` once every field of `until` holds |
| `session.subscribe` | `{events: ["phase","map","bsp","pause"]}` | then `event` notifications: `phase {from,to}`, `map {from,to}`, `bsp {from,to}`, `paused`, `resumed {why: request, timeout or released}` |
| `session.takeover_info` | none | `{window, pid}`, whether the game has a window and its process id |
| `session.request_takeover` | `{ask?, requester, name, operation, builds_image, source, level, own_data, estimate_seconds, timeout_seconds}` | `{id}` for the consent prompt, or for the running job's question with `"ask": "running"` |
| `session.takeover_status` | `{id}` | `{decision}` for the request |
| `features.list` | none | `{at, plan, features}`: the feature plan text the game printed at start, and each feature as `{id, on, reason?}` (`reason` when the plan turned it off), read from the plan's `feature <id>: on` and `feature <id>: off (<reason>)` lines (`agent_proto::FeatureState`) |


`until` takes `tick`, `ticks` (more ticks than when the wait began),
`activation` (exact), `map_ticks`, `present`, `phase`, `map` and `paused`;
all given must hold, at least one is required. A condition already true
answers at once. `timeout_ms` defaults to 30 s, at most 600 s; the failure is
`TIMEOUT` with the stamp.

```sh
h2 agent wait '{"phase":"gameplay","map_ticks":30}' 5000
h2 agent watch phase map          # events until interrupted
```

`session.wait` replaces every "sleep and screenshot" loop. Live changes go
through [`render.toggle`](#edit-and-hot-render-live-tag-and-shader-changes)
and [`cheat.set`](#menu-methods-settings-perf-overlay-cheats).

### Lab consent

The lab uses `session.takeover_info` to tell a windowed game from a headless
one. Windowed player runs register `session.request_takeover` and
`session.takeover_status`. The request needs act access, so another
controller can block it.

| Param | Meaning |
|---|---|
| `name`, `requester` | the job and who asked; required, with `source` |
| `operation` | `"build"`, `"run"` or `"sim"` |
| `builds_image` | the job builds the image first |
| `source` | `{"kind": "branch", "name", "commit"}`, `{"kind": "rev", "rev", "commit"}` or `{"kind": "current", "commit"}` |
| `level`, `own_data` | the run's level or null; plays on the player's own data |
| `estimate_seconds`, `timeout_seconds` | estimate (0 unknown); wall-clock limit from acceptance, 1 to 3600 |
| `alongside` | default true; false hides **Start alongside** (benchmarks) |
| `ask` | `"start"` (default) or `"running"` |

A missing requester, name or source is `INVALID_PARAMS`; a second request
while a prompt is queued or open is `BUSY`. The status result is `pending`,
`allowed` (**Close and start**; the lab then calls
[`host.quit`](#close-the-window)), `alongside`, `declined` (**Not now**
or Back), `timed_out` or `unavailable`. An unknown or superseded id is
`GONE`. With `"ask": "running"` the dialog (`lab.running`) asks about a job
already running ([lab.md](lab.md#when-the-player-opens-a-game)) and answers
`finish_this`, `keep_running` or `stop_and_pause`. Dialog classes and
rows: see the [detail note](notes/2026-11-26-agent-api-detail.md), "Lab consent".

### Control

`session.pause`, `session.resume` and `session.step` need `session.claim`.
A pause blocks the game thread inside the tick observer, still answering
requests, and ends after `max_ms` (default 60 s, at most 600 s), on release
or disconnect, or when the game exits. `step {ticks}` (1 to 1800) answers
once paused again; a resume, new pause or release answers it with
`INTERRUPTED`. With the tick observer off, both answer `UNAVAILABLE`. In
`h2 play` a pause also stops the window's event loop, so use
[`host.pause`](#window-pause-and-frame-step) there. Input calls need no
claim while nobody holds control.

```sh
printf 'claim\npause\nstep 1\nobject player.0\nrelease\n' | h2 agent -
```

### Window pause and frame step

`host.pause` is the window's own pause (Cmd+P) over the API; `host.step`
runs game ticks under it. They are for `h2 play` and other windowed games;
the window keeps drawing and audio pauses as for Cmd+P.

| Method | Params | Result |
|---|---|---|
| `host.pause` | `{on: bool}`, or `{}` to read | `{paused, held, tick}` once the window has applied it |
| `host.step` | `{ticks?}`: 1 to 1800, default 1 | `{paused, held, tick, stepped}` once the ticks ran and the game is held again |

`held` says the game thread stopped at its pause wait; `stepped` can exceed
`ticks` after a catch-up frame. Refusals:

| Case | Error |
|---|---|
| no window (headless) | `UNAVAILABLE`, naming `session.pause` |
| `host.step` while not paused | `INVALID_REQUEST` |
| a step already running | `BUSY` |
| `ticks` out of range | `INVALID_PARAMS` |
| resume or window close during a step | `INTERRUPTED` |
| ticks not run within 0.2 s each plus 5 s (at most 2 min) | `TIMEOUT` with `data.stepped` |

Both are answered on the connection's own thread
(`Agent::add_connection_read`), so they skip the controller check. Code:
`crates/apps/app/src/shell.rs`, `crates/apps/app/src/lifecycle.rs`,
`crates/extensions/agent/src/sim/pause.rs`, `agent_proto::host_pause`. Every
other method waits while the game is held. Pause handshake and in-process
debug menu behaviour: see the [detail note](notes/2026-11-26-agent-api-detail.md), "Window pause and frame step".

```sh
h2 agent host.pause '{"on":true}'
h2 agent host.step '{"ticks":10}'
h2 agent host.pause '{"on":false}'
```

### Window screenshots

`host.screenshot {path?}` writes the window's newest published frame (no
debug menu or overlays) as a PNG and answers `{path, width, height}`. With no
`path` it names a file in the window's screenshots directory. It is
answered on the connection's own thread (`crates/apps/app/src/shell.rs`,
`agent_proto::host_shot`), so it works while paused. A headless game, a
window with no frame yet, a failed write or a missing screenshots directory
is `UNAVAILABLE` with the reason (headless names `render.capture`).

```sh
h2 agent host.screenshot '{}'
h2 agent host.screenshot '{"path":"/tmp/ledge.png"}'
```

### Close the window

`host.quit {}` closes the window as the close button and Cmd+Q do, and
answers `{closing: true}` at once; the caller watches the process exit. A
repeat is harmless. The lab calls it on **Close and start** ([consent
request](#lab-consent), [lab.md](lab.md#player-consent-before-a-job)). A
campaign keeps what its last checkpoint saved. It is answered on the
connection's own thread (`crates/apps/app/src/shell.rs`,
`agent_proto::host_quit`), so a paused game takes it. A headless game is
`UNAVAILABLE`.

```sh
h2 agent host.quit '{}'
```

### Resize the window

`host.window.resize` sets the window's inner size in physical pixels, or
toggles borderless fullscreen as Cmd+F does, for aspect and field-of-view
checks.

| Method | Params | Result |
|---|---|---|
| `host.window.resize` | `{width, height}` (each 64 to 16384), `{fullscreen: bool}`, or both with `fullscreen: false` | `{width, height, fullscreen, scale}` once the window has that shape |

A size leaves fullscreen first; a size with `fullscreen: true` is refused.
`scale` is physical pixels per point. The call waits up to 5 s; a shape not
taken in time answers `TIMEOUT` with the window's state as `data`. A
headless game is `UNAVAILABLE`. Missing or partial params, an out-of-range
side or unknown fields are `INVALID_PARAMS`. It runs on the connection's own
thread (`crates/apps/app/src/shell.rs`, `agent_proto::host_window`).

```sh
h2 agent host.window.resize '{"width":2560,"height":1080}'
h2 agent host.window.resize '{"fullscreen":true}'
h2 agent host.window.resize '{"fullscreen":false}'
```

## `world`: what is in the level

Objects come from `live-telemetry::memory` and `live_telemetry::things`,
read at the tick boundary. Ids are `player.<index>` or `<kind>.<datum>` (the
datum handle as 8 hex digits, salt included), so a reused slot answers
`GONE`.

| Method | Params | Result |
|---|---|---|
| `world.count` | `{group_by: "kind" \| "team"}` | `{at, total, groups}`: the cheap first call |
| `world.query` | `{kind?, near?, tag?, fields?, limit?, offset?}` | `{at, total, offset, objects, more}` |
| `world.object` | `{id}` | `{at, object}`: every field the reader has for one object |


- **Kinds** (`agent_proto::Kind`): `player`, `actor` (AI biped), `vehicle`,
  `machine`, `body` (a dead biped), `scenery`, `weapon` and `equipment`
  (lying in the world), `control`, `crate`, `creature`. The last six come
  from the object header table's type byte (`Kind::of_object_type`).
  Projectiles, effects, particles, garbage, light fixtures and sound
  scenery are not listed.
- **Query.** `near` is `{point | player, radius}`, sorts nearest first and
  adds `distance`. `fields` trims each object (`id` and `kind` stay). `tag`
  keeps one definition: a datum (8 hex digits) or a name substring (any
  case, `/` for `\`). Pages default to 50, at most 500.
- **Tag names.** An object with a `tag` also has `tag_name`, the path from the
  loaded map file's names (as `edit.tags` reads them), found by
  `boot::map_override::map_file`; absent when unreadable.
- **Fields.** `pos`, `velocity`, `team`, `tag`, `tag_name`, `vehicle`,
  `weapon`, `name`, `local_user` and `unit` (players), `mounted_on` (a turret
  is a vehicle with it set; its `pos` and `forward` are relative),
  `occupied`, `placement`, `position` (a machine's device position, 0 to 1)
  and `physics`.
- **Facing.** `forward` is a unit's aiming vector (unit `+0x168`, which
  turns with the camera); `body_forward` is a biped's own forward (`+0x70`).
  `yaw` (degrees about +z from +x) and `pitch` are `forward` as angles.
- **`physics`** (units): crouch, airborne and hitbox-offset state, unit and
  biped flags, `havok_pos`, `havok_velocity`, `support` (the ground the
  biped update keeps, layout in `live-telemetry`'s `layout.rs`) and, for
  bipeds, `standing_fits` (the superbounce capsule test,
  `crates/extensions/game/src/superbounce_capsule.rs`; present while that
  mode is on). Offsets, flag bits and support fields: see the [detail note](notes/2026-11-26-agent-api-detail.md),
  "`world`: what is in the level". For a movement trace, poll
  `object player.0` on one `h2 agent -` connection or use [a
  trace](#tick-traces-and-the-flight-recorder).

```sh
h2 agent count team
h2 agent query '{"kind":["scenery"],"tag":"<part of a tag name>","near":{"player":0,"radius":500},"fields":["pos","tag_name"]}'
h2 agent object player.0
```

## `world.breakable`: broken glass

`world.breakable` reads one structure BSP's breakable-surface state at the
tick boundary. The bots' break step reads the same state
(`bots::guest::breakable`; [bots.md](../game/bots.md#breaking-the-glass)).

| Params | Result |
|---|---|
| `{bsp?}` (default 0) | `{bsp, enabled, broken, mask}` |

- `enabled` is the first byte of the state at `[0x4ed280]`.
- `mask` is the BSP's 32 bytes from `[0x4ed280] + 1 + 32 * bsp`, in hex; a
  set bit is a whole surface ([collision.md](../formats/collision.md#what-the-engine-does-with-a-bsp)).
- `broken` lists the indices whose bits are clear, ascending.
- `UNAVAILABLE` while the state's pointer is null or the read faults; a
  stray parameter is `INVALID_PARAMS`.

```sh
h2 agent world.breakable '{}'
```

## `world.ai`

`h2 agent world.ai` takes no params and returns `{at, globals, actors}` at one
boundary (`crates/extensions/agent/src/ai.rs`). `globals` is `{on,
initialised}`, the two bytes at `[0x4f55d0]` that `ai_update` (`0x1c83c0`)
needs set, or null. Each actor has its salted AI `datum`, `unit` ID (for
`world.object`), character tag datum, team, squad index, behaviour ID and
`behavior_name`, combat status, `sector`, `location_cached` and
`focus_object`; a missing unit, squad or behaviour is null. Sources and
addresses: see the [detail note](notes/2026-11-26-agent-api-detail.md), "`world.ai`".

## `net.replication`

`h2 agent net.replication` takes no params and returns what the distributed
simulation replicates, at one boundary (`crates/extensions/agent/src/net.rs`).
It measures the probes of [ai-networking.md](../game/ai-networking.md); the
types are in that page's [notes](../game/notes/ai-networking.md).

| Field | Meaning |
|---|---|
| `world_mode`, `mode` | the simulation world's mode (`[0x4cf77c]+8`) and its name: `none`, `local`, `sync-server`, `sync-client`, `dist-server`, `dist-client`; null before the simulation exists |
| `distributed` | true in modes 4 and 5 |
| `objects` | per object tag group (`bipd`, `vehi`, `proj`, ...; `other` for a type outside the table): `{objects, with_entity}`, the live objects and how many have a replication entity (object body `+0xd4` is not -1) |
| `entities` | the sum of `with_entity` |
| `creature_type` | the registered creature probe slot, 17, or null when the definition is absent |
| `creatures` | live bindings `[{entity, type, object, updates}]`; `object` is the local salted handle as in `sim.objects`; `updates` counts movement operations over the binding's lifetime |
| `events` | `{total, by_type: [{type, name, count}]}`: replication events queued since the process started, by event type; an unknown type is counted under `other` with a null `type` |
| `traffic` | `{sent, sent_bytes, received, received_bytes, largest}`: the title's datagrams since the process started (`live_hle::traffic`) |

Counts accumulate over the process lifetime; divide the difference of two
calls by the ticks between their `at` stamps for a rate. Event counts come
from the `agent-net` feature (`SIM_EVENT_GENERATE`, `0xb5a70`);
`H2_FEATURES=-agent-net` leaves them at zero. Example: see the [detail note](notes/2026-11-26-agent-api-detail.md),
"`net.replication`".

## `actors.net`

`h2 agent actors.net` takes no params and returns the actor channel of
[ai-networking.md](../game/ai-networking.md) as this machine ran it on the
last tick (`crates/extensions/agent/src/actors.rs`, published by
`game::actor_net::report`).

| Field | Meaning |
|---|---|
| `role` | `host` on a distributed server, `client` on a distributed client, `off` elsewhere or with `-actor-net` |
| `tick` | the simulation tick of the report; null when `role` is `off` |
| `peers` | on the host, each Live tunnel peer: `{peer, bytes_per_second, packets, bytes, keys, digests, records, dropped}`; `bytes_per_second` covers the last 30 host ticks, the rest count since connect; empty on a client and on a System Link host |
| `client` | on a client, `{actors, puppets, waiting, lines_waiting, mode_state_bytes, digest_misses, key_requested, diverged, host_log_head}`; null elsewhere |

On a client, `puppets` lists actors whose body arrived through retail
replication as `{entity, character, squad, team, posture, unit}`;
`diverged` is the channel closed for the match (`actor_channel_diverged`);
`host_log_head` is the last digest's host log head (64 hex digits, null
before the first). To check both ends of a Live Firefight agree, compare
the host's `firefight.state` `log.hash` with the client's
`client.host_log_head` within 30 ticks. Field details and example: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "`actors.net`".

## `world.raw`: an object's bytes, before and after

A game method, read only, no claim (`agent::raw::install`; reader
`crates/live/live-telemetry/src/raw.rs`). It looks for state a forced crouch
leaves behind ([superbounce.md](../game/superbounce.md)).

`world.raw {id, parts?, len?}` returns `id`, `datum` and `parts`, each
`{part, base, hex}` or `{part, missing}` (an unset pointer, a fault). Each part
is found through the one before it ([havok-runtime.md](../engine/havok-runtime.md)):

| Part | Where | Bytes |
|---|---|---|
| `object` | the object's body (`player.N`: its unit) | the header's size (`+0x06`), else `len` (default `0x400`), at most `0x2000` |
| `physics_component` | body `+0xb4` into the component array | `0xa0` |
| `entity` | the component's first rigid body (`+0x70`, `+0x40`) | `0x200` |
| `motion` | entity `+0x3c` | `0x140` |
| `shape` | entity `+0x0c` | `0x80` |

`h2 agent raw-diff a.json b.json` needs no game: per part it prints the
differing 32-bit words with their offsets (and as floats). A part on one side
only, or whose base moved, is listed, not compared
(`crates/apps/cli/src/agent_raw.rs`).

```sh
h2 agent raw player.0 > before.json             # world.raw {"id":"player.0"}
h2 agent raw player.0 > after.json
h2 agent raw-diff before.json after.json
h2 agent raw player.0 entity,shape              # some parts
h2 agent world.raw '{"id":"player.0","parts":["object"],"len":2048}'
```

## `world.poke`: replace bytes in one object part

`world.poke {id, part?, offset, hex}` (access: claim) writes 1 to 64 bytes at a
tick boundary.

| Param | Meaning |
|---|---|
| `id`, `part` | as `world.raw`; `part` defaults to `object` |
| `offset` | nonnegative byte offset from the part's base |
| `hex` | two hex digits per byte; the range must fit the part `world.raw` reads by default |

The connection must hold `session.claim`. A session that refuses host writes
gives `INVALID_REQUEST` with `data.session` (`sim.session` reports it); a
request served between slices is `UNAVAILABLE`. The result has `id`, `datum`,
`part`, `base`, `offset`, `at`, `before` and `after` (lowercase hex).

```sh
h2 agent poke <id> <part> <offset> <hex>    # takes a claim; offset decimal or 0x
```

## `havok`: the physics world

Game methods (`agent::havok::install`), read only, answered at a tick
boundary. Layout and evidence: [havok-runtime.md](../engine/havok-runtime.md).

| Method | Params | Result |
|---|---|---|
| `havok.bodies` | `{near?, radius?, limit?}` | rigid bodies: `entity` (the key contacts use), `state` (`active`, `inactive`, `fixed`), `object`, `datum`, `pos`, `velocity`, `angular_velocity`, `inv_mass`, `mass`, `friction`, `restitution`, `shape` (`addr`, `vtable`, `kind`: `capsule` `a` `b` `radius` `height`; `sphere` `radius`; `transform` `translation` `child`; else `other`). `near` (an object id) keeps bodies within `radius` (default 5), nearest first, with `distance`; `limit` defaults to 64 |
| `havok.agents` | `{object}` | collision pairs and cached state ([below](#havokagents)) |
| `havok.contacts` | `{object?}` | contact constraints: `a`, `b` (`entity`, `object`, `datum`, `fixed` for level and static bodies) and `points`: `pos`, `normal` (from `b` toward `a`), `distance` (negative is penetration), `impulse` (the solver's last step), `friction`, `restitution`. With `object`, only its contacts, with it as `a` |
| `havok.digest` | `{}` | the ordered heap walk (sim-rewrite-havok.md §9.2): `digest` (16 hex digits, FNV-1a over every live byte and address), `bodies`, `constraints`, `pairs`, `heap_used`. The arena hash does not cover the Havok heap, so this proves a physics rewrite; the sim report carries it as `havok.world` |

```sh
h2 agent havok.agents '{"object":"player.0"}'
h2 agent havok.contacts '{"object":"player.0"}'
h2 agent havok.bodies '{"near":"player.0","radius":3}'
```

Superbounce polling recipe: see the [detail note](notes/2026-11-26-agent-api-detail.md), "`havok`: the physics world".

### `havok.agents`

`object` is required. The reply has `object`, `at` and `agents`: each pair with
an allocated agent on any rigid body of that object (empty for a live object
with none). A malformed selector is `INVALID_PARAMS`; an absent object or a
player between lives is `GONE`.

| Field | Meaning |
|---|---|
| `entry`, `island`, `collidables: [a,b]` | guest addresses, native pair order |
| `object_side`, `entity` | `a` or `b`; the object's participating rigid body |
| `other` | the partner entity with `datum`, object ID, fixed-island flag; omitted without an entity |
| `pair_word` | the last four bytes of the pair record |
| `cache` | `base`, `vtable`, `manager`, `kind`, `hex` (eight lowercase hex digits for addresses; `hex` is the raw bytes from `base`) |
| `tree` | `bv_tree` and `welded_bv_tree`: `min`, `max`, and per child `entry`, `key`, `word_04`, `cache`; a type-1 MOPP key adds `surface`; a filtered child is `kind: "empty"` |
| `convex` | `convex` and `swept_convex`: `vertices_a`, `vertices_b`, `contact_ids`, `separating_axis`, `separating_distance`; `swept_sides` (bit 0 A, bit 1 B) |
| other | `kind: "other"` with inline bytes |

Bounds: 32,768 scanned pairs, 512 children per tree, 4 KiB per allocation, 256
KiB of raw cache per request; a failed read or an exceeded bound is
`UNAVAILABLE`. Reading notes: see the [detail note](notes/2026-11-26-agent-api-detail.md), "`havok.agents`".

### `havok.pair.set`

`havok.pair.set {object, entry, word}` (access: claim) overwrites one pair's
word (pair `+0x0c`) at a tick boundary.

| Param | Meaning |
|---|---|
| `object` | the object |
| `entry` | the pair's `entry` from `havok.agents`; without it, the object's one level pair |
| `word` | eight hex digits, as `pair_word` |

Another `entry`, no single level pair, an absent object or a player between
lives is `GONE`. The reply has `object`, `entry`, `before`, `after`. It needs
the claim and host-write session of `world.poke`
(`agent::havok::install_writes`; `set_pair_word` in
`crates/live/live-telemetry/src/havok_agents.rs`). It tests whether the word a
forced crouch leaves is what makes a superbounce
([superbounce.md](../game/superbounce.md)).

```sh
printf '%s\n' claim 'havok.pair.set {"object":"player.0","word":"fffffffd"}' | h2 agent -
```

## `superbounce`: the superbounce game type

`superbounce.state` (access: read; any param is `INVALID_PARAMS`) answers from
the state the `superbounce` feature publishes each tick
(`crates/extensions/game/src/superbounce_mode.rs`; wire types
`crates/extensions/agent-proto/src/superbounce.rs`; registered in
`crates/extensions/game/src/agent_glue.rs`). The mode:
[superbounce.md](../game/superbounce.md#the-superbounce-game-type).

| Field | Meaning |
|---|---|
| `active` | the mode is on (script requires `hud.superbounce`, `superbounce.set` has not forced it off); all else is empty or `idle` while off |
| `primed` | the local player's level pair word is -3 |
| `word`, `word_signed` | the pair word, hex and signed; absent without a level pair |
| `position` | local player position, world units |
| `nearest` | nearest bounce spot: `distance` to its edge, `launch`, `fall`, `edge` |
| `scan` | `status` (`idle`, `scanning`, `ready`, `failed`), `map`, `error`, `bounce` and `crouch` counts |
| `airborne` | biped off the ground (flags bit 0); absent without a player |
| `vz` | vertical velocity, units/s, up positive |
| `yaw` | aim yaw, radians from +X toward +Y |
| `velocity` | velocity, units/s; absent without a player |
| `options` | the script's options: `targets` (`common`, `likely`, `all`) and bools `velocity`, `alignment`, `miss_info`, `medals`, `second_bounce`, `approach_guide`, `charge_segments` ([options](../game/superbounce.md#the-huds-options)) |
| `targets` | seam targets ([superbounce.md](../game/superbounce.md#targets)): `mode`, `count`, `tag`, `marked`, `second` (seams reachable after a bounce, each with `at`, `time`, `steer`, `reach`, `speed`, `need`, `cost`, `caught`; [second bounce](../game/superbounce.md#the-second-bounce)) |
| `charge` | `phase` (`near`, `under`, `held`, `primed`, `lost`), `fill`, `still_ticks`; absent while hidden ([charge](../game/superbounce.md#charge)) |
| `verb` | prompt verb: `let_go_of_crouch`, `walk_out_to_stand`, `keep_moving`, `charge_again` |
| `lost_reason` | `stood_still`, `crouched` or `unknown`, while `lost` or `charge_again`; absent otherwise |
| `nav_target` | `at`, `kind` (`crouch`, `strip`), `distance`, `fall` ([floor marks](../game/superbounce.md#floor-marks)) |
| `takeoff` | while primed on the ground, where to leave from ([The takeoff](../game/superbounce.md#the-takeoff)); absent when none |
| `guide` | the [approach guide](../game/superbounce.md#the-approach-guide) on the seam being gone for; absent while not shown or not primed. Fields: `seam`, `call` (`off_line`, `lined_up`, `too_shallow`, `too_high`, `off_edge`, `no_drift`, `crouching`, `will_bounce`), `jump`, `impact`, `row`, `ticks`, `speed`, `caught`, `deep`, `depth`, `past`, `across`, `chance`, `chance_capped`, `jump_in`, `cue`, `takeoff`, `strength`; each is absent in the cases the note lists |
| `landing` | airborne toward a strip: `at`, `verdict` (`on_slot`, `fixable`, `miss`), `speed`, `need`, `miss`, `locked` ([lock-on](../game/superbounce.md#lock-on)) |
| `last_landing` | the last landing filed: `result` (`bounce`, `not_primed`, `missed`, `too_slow`, `no_bounce`, `through`), `tick`, `at` ([landings](../game/superbounce.md#landings)) |
| `off_spot` | the last 16 bounces no marked strip explains, oldest first: `tick`, `at`, `edge`, `speed_in`, `launch`; absent when none |
| `standing_fits` | the standing capsule fits at the origin; absent before the scan is ready |
| `medals` | every player's medals ([medals](../game/superbounce.md#medals)): `players` (`player`, `chain`, `live`, `counts`) and `recent`, the last 8 awards (`player`, `tick`, `chain`, `medals`, `strips`). Ids: `superbounce`, `double-bounce`, `triple-bounce`, `quad-bounce`, `chain-bounce`, `bouncetacular`, `bounced`, `monster-bounce` |

Full `targets`, `guide` and `medals` rows: see the [detail note](notes/2026-11-26-agent-api-detail.md), "`superbounce`: the superbounce game type".

```sh
printf '%s\n' 'superbounce.state' | h2 agent -
```

`superbounce.set {"on": false|null}` (access: act) forces the mode off in any
game (`false`) or follows the game's script again (`null` or `{}`). `true` is
refused with `invalid_params`. The answer is `{"mode": "off"|"game"}`; the
on/off check runs every 30 ticks.

```sh
h2 agent superbounce.set '{"on":false}'
```

`superbounce.medal_test {"chain": N}` or `{"bounced": true}` (access: act) files
a bounce at chain `N` (1 and up), or a Bounced, for the first local player with
a unit, by `superbounce_chain::bounce_medals_with`; `"monster": true` with
`chain` files a Monster Bounce. The award reaches the HUD and
`superbounce.state`'s `medals`. The answer is `{"player", "chain", "medals",
"line"}`. It refuses with the mode off, no local player, `chain` 0, both or
neither of `chain` and `bounced`, or `monster` with `bounced`.

```sh
h2 agent superbounce.medal_test '{"chain":6}'
h2 agent superbounce.medal_test '{"bounced":true}'
```

`superbounce.assist` (access: read; no params) says what the hold-X fall assist
did at its last pad read
([superbounce.md](../game/superbounce.md#hold-x-the-fall-assist);
`crates/extensions/game/src/superbounce_assist.rs`).

| Field | Meaning |
|---|---|
| `active` | the mode is on |
| `enabled` | the player's `superbounce.fall_assist` setting is on (opt in) |
| `test_hold` | `superbounce.assist_test` is holding the button |
| `engaged` | the assist steered at its last read |
| `reason` | engaged: `kept`, `marked`, `nearest`. Held, not engaged: `not_falling`, `no_strip` (none within 6 units), `refused`, `no_body`. Else `not_held`, `off`, `inactive` |
| `target` | the chosen strip's stand point |
| `point` | the point of the strip being landed on |
| `aim` | the point aimed at (X, Y) |
| `landing` | where the player lands if the stick is let go (X, Y) |
| `fall_time` | seconds to touchdown at the strip's height |
| `stick` | stick sent to the guest, `[x, y]`, -1 to 1 |
| `from`, `velocity` | origin and velocity at the read; absent while the button is up |
| `candidates` | strips judged while held, at most 12: chosen, others in reach, then refused, nearest first |
| `chose` | the last engagement's choice: `target`, `point`, `reason`, `from`, `velocity`, `candidates`; kept until the next |

`target` to `stick` are absent while not engaged. A candidate:

| Field | Meaning |
|---|---|
| `stand` | the strip's stand point |
| `verdict` | `chosen`; `passed`; or refused: `above`, `long` (fall over 3 s), `far` (over 3 units sideways), `out_of_reach`, `covered`, `walled` |
| `distance` | sideways, to the strip's edge |
| `speed`, `need` | touchdown speed the fall gives, and the speed needed, units/s |
| `point`, `fall_time`, `miss` | in reach only: the landing point, seconds, and the miss of the coasting landing |

`superbounce.assist_test {"hold": true|false}` (access: act) holds the assist's
button until set `false`; the answer is `{"hold": bool}`. Turn the setting on
with `settings.set`.

```sh
h2 agent settings.set '{"key":"superbounce.fall_assist","value":true}'
h2 agent superbounce.assist_test '{"hold":true}'
h2 agent superbounce.assist
```

`superbounce.verify {kind, ...}` (access: act) starts a batch of trials of the
loaded map's spots, run tick by tick on one seat by an agent ticker
(`Agent::add_ticker`), and answers at once with the plan
([superbounce.md](../game/superbounce.md#verifying-spots);
`crates/extensions/game/src/superbounce_verify.rs`; wire types
`agent_proto::superbounce::Verify*`). One batch runs at a time. It needs the
mode on and its scan ready, except over explicit `points`; the `player.<n>`
seat needs the `superbounce-assist` feature's pad tap, and every kind but
`route` needs the `agent-calls` teleport (`UNAVAILABLE` otherwise). Every param
but `kind` is optional:

| Param | Meaning |
|---|---|
| `kind` | `crouch`, `drop` (a fall onto a strip with the word set) or `route` (a bot's `bots.bounce_to` route) |
| `spots` | scan spots of that kind by index, best score first; default the first `max_spots` (12) that have not failed |
| `points` | explicit spots `{stand, out?, edge?, drift?, need?}` ([the target](../game/superbounce.md#the-target-the-collision-edge)); never saved |
| `seat` | `player.<n>` (default `player.0`) or a brain-driven bot by gamertag, handle or `player.<n>`; `route` needs a bot |
| `turns`, `distances`, `offsets` | crouch: approach turns in degrees (default `[0,25,-25]`), start distances outside the lip (`[0.3,0.6]`), X/Y offsets (`[[0,0]]`) |
| `across`, `along`, `heights` | drop: offsets across the target line (`[-0.1,-0.05,0,0.05,0.1]`), along it (`[0]`), heights over the strip (`[1.5,2.5,3.5]`) |
| `prime` | drop: `set` (-3, default), `none` (0, the control) or `keep` |
| `drift_stick` | drop: stick along the drift, 0 to 1 (default 0) |
| `settle_ticks`, `hold_ticks`, `out_ticks`, `fall_ticks`, `route_ticks` | defaults 20, 15, 30, 120, 3600 |
| `to`, `mode`, `repeat` | route: target (default 6 up from each stand), `help` or `fun` (default), routes a spot (3) |
| `trace` | keep the per-tick trace of this trial index |
| `save` | write the map's verified file at the end (default true; never for `points` or `route`) |
| `dry_run` | plan and list, run nothing |

The answer is `{job, kind, seat, map, spots, trials, estimated_ticks,
dry_run}`; spots carry `index`, `kind`, `stand`, `status`, `score`, trials
`spot`, `start`, `yaw`, `distance`, `offset`, `height`.

`superbounce.verify_status {wait_ticks?, brief?}` (access: read) reports on the
newest batch: `state` (`none`, `running`, `done`, `failed`, `cancelled`), `job`,
`kind`, `seat`, `map`, `done`, `total`, `current`, `spots`, `results` (per spot
`status` `proven`, `failed` or `untried`), `trials` (not with `brief`),
`trace`, `lines`, `saved`, `error`. `wait_ticks` (at most 290) waits for the
end. Per-kind trial fields: see the [detail note](notes/2026-11-26-agent-api-detail.md), "`superbounce`: the superbounce game type".
`superbounce.verify_cancel` (access: act; no params) stops the batch, releases
the seat and answers with the report; nothing is written.

```sh
h2 agent superbounce.verify '{"kind":"crouch","dry_run":true}'
h2 agent superbounce.verify '{"kind":"crouch","spots":[0,1,2],"trace":0}'
h2 agent superbounce.verify_status '{"wait_ticks":290,"brief":true}'
```

`superbounce.view` (access: read; no params) says what the bounce view shows,
as of the last tick ([superbounce.md](../game/superbounce.md#the-bounce-view);
`crates/extensions/game/src/superbounce_view.rs`; wire types
`crates/extensions/agent-proto/src/superbounce_view.rs`):

| Field | Meaning |
|---|---|
| `active` | the mode is on; all else empty while off |
| `mode` | `none`, `pulse` or `hold` |
| `button` | the flashlight action at the last tick |
| `test_hold` | `superbounce.view_test` is holding |
| `pulse_age`, `pulse_front` | seconds since the pulse left, ring distance in world units; absent without a pulse |
| `grow` | the tracker's growth into the map, 0 to 1 |
| `spots`, `marks`, `hidden`, `tested` | spots known, marked now, behind a wall, with a line-of-sight answer |
| `tracker` | `enabled`, `range`, and up to two `blips` (`kind`, `proven`, `x`, `y`, `level`, `rim`, `nav`) |
| `map` | while held or shrinking: `floors`, `edges`, `pairs`, `blips` over 50 m |

`superbounce.view_test {"mode": "pulse"|"hold"|"none"}` (access: act) drives
the view with no pad; the answer is `{mode, test_hold}`. Refused while the mode
is off.

```sh
h2 agent superbounce.view_test '{"mode":"hold"}'
h2 agent superbounce.view
```

`superbounce.ghosts` (access: read; no params) lists the loaded map's ghost
runs, kept in `<map>.ghosts` in the bots directory
([superbounce.md](../game/superbounce.md#ghost-runs);
`crates/extensions/agent-proto/src/superbounce.rs`; game side
`crates/extensions/game/src/superbounce_ghost.rs`).

| Field | Meaning |
|---|---|
| `active` | the mode is on |
| `map`, `file` | the map, and the ghosts file's path |
| `error` | why the file could not be read; it is not written over then |
| `ghosts` | each run: `index`, `at`, `landing`, `source` (`bot`, `verify`, `player`), `ticks`, `launch`, `crouch_primed`, `presses` (`frame`, `button`) |
| `looked_at` | the run whose spot the first local player looks at |
| `prompt` | the prompt line's ghost verb |
| `playing` | `index`, `seconds`, `tick`, `at`, `done`; absent when none plays |
| `recorded` | runs kept since the game started |

`superbounce.ghost_show {spot?, stop?}` (access: act) plays run `spot` once, or
the run looked at; `{"stop": true}` stops it. It answers `playing`, `index`,
`ticks`. Refused while the mode is off, with no map, for an index with no run,
or (to play) without the `superbounce-ghost` feature (off by default).

```sh
h2 agent superbounce.ghosts
h2 agent superbounce.ghost_show '{"spot":0}'
h2 agent superbounce.ghost_show '{"stop":true}'
```

`superbounce.reload {map?}` (access: act) re-reads a map's `<map>.spots`,
`.learned`, `.verified` and `.ghosts` from the bots directory (the loaded map
without `map`) and republishes them as a map load does, with no restart, at a
tick boundary (`crates/extensions/game/src/superbounce_reload.rs`,
`bots::spots::reload_in`). The answer has `map`, `dir`, `spots_file` (`read`,
`absent` or `unread: <why>`), `before` and `after` (scan counts), `verified_entries`,
`learned_entries`, `ghosts_before`, `ghosts_after`, `mode_updated`. Refused
with no map and no `map`, or no bots directory. Behaviour detail: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "`superbounce`: the superbounce game type".

`superbounce.tune {bounce_speed?, catch?, launch_gain?}` (access: act) sets
live overrides of the model's constants, in memory only:

| Key | Constant | Range |
|---|---|---|
| `bounce_speed` | `superbounce::speed::BOUNCE_SPEED` | above 0 |
| `catch` | `superbounce::CATCH` | 0 up to but not including 1 |
| `launch_gain` | `speed::EXCESS_GAIN` | 0 or more |

A key left out stays; `null` clears its override; every key is checked before
any applies, and an unknown key is refused. The answer is `bounce_speed`,
`catch`, `launch_gain`, each `{value, source}` with `source` `default` or
`override`.

```sh
h2 agent superbounce.reload '{"map":"zanzibar"}'
h2 agent superbounce.tune '{"bounce_speed":5.0,"catch":0.15}'
h2 agent superbounce.tune '{"bounce_speed":null,"catch":null,"launch_gain":null}'
```

## `pogo`: Superbounce's Pogo Stick

`pogo.state` (access: read; no params) says what the Pogo Stick is doing, as of
the last tick ([pogo-stick.md](../game/pogo-stick.md);
`crates/extensions/game/src/pogo_agent.rs`; wire types
`crates/extensions/agent-proto/src/pogo.rs`):

| Field | Meaning |
|---|---|
| `active` | the superbounce mode is on; `riders` is empty while off |
| `option`, `style` | the script's `pogo` and `pogo_style` options (`unsc`, `covenant`, `forerunner`) |
| `map` | the loaded map's name; absent without one |
| `weapons` | the three sticks' weapon tags, in the order `unsc`, `covenant`, `forerunner`; empty when the map lacks the kit |
| `riders` | players holding a stick, local first: `player`, `local`, `unit`, `weapon`, `style`, `phase` (`carried`, `mount`, `ride`, `compress`, `spring`, `dismount`), `mounted`, `compress`, `flare`, `tuck`, `trick` (`id`, `name`, `progress`), `posed` |
| `camera` | the chase camera: `on`, `drawn`, `room`, `share`, `fallback_frames` |
| `tricks` | the ids `pogo.test` takes |

`pogo.test {"trick": "<id>"}` (access: act) shows a trick from its start on the
first local player's stick. It is presentation only: no bounce is filed. The
answer is `{player, trick, name}`. Refused with the mode off, an unknown id, or
no local player holding a stick.

```sh
h2 agent pogo.state
h2 agent pogo.test '{"trick":"backflip"}'
```

## `profile`: where a frame goes

| Method | Params | Result |
|---|---|---|
| `profile.counters` | none | the frame-stats rolling window (`timeline::phase`), as `H2_FRAME_STATS` prints: `frames`, `fps`, frame ms, `phase_ms`, `counts`. When nothing else collects them the glue starts a quiet 5 s window |
| `profile.capture` | `{ticks \| ms, budget_ms?, spikes?, sample?}` | percentiles, phases, counters and spikes over a window (below) |
| `profile.fallbacks` | `{top?}` | where the fallback interpreter runs |
| `profile.memory` | none | the flat RAM window's counters (below) |
| `profile.faults` | `{top?}` | the fault census: every guest memory fault and whether execution went on |
| `profile.sample.start`, `.stop`, `.status` | `{hz?}`, `{inline?}`, none | the in-process sampler |

```sh
h2 agent counters
h2 agent capture 900                 # 900 game ticks, about 30 s
h2 agent capture 10000ms sample      # h2 built --features perf
h2 agent fallbacks 20
h2 agent flatmem
h2 agent faults
```

- **`profile.capture`** keeps every frame-stats row over the window and
  answers `frames`, `fps`, `frame_ms`, `busy_ms` (frame less pacing) as mean,
  p50, p95, p99, max, total; the same per phase (`cpu`, `decode`, `encode`,
  `submit`, `gpu-wait`, `pace`, `render-wait`, `d3d`, `render`); mean, max,
  total per counter; `over_budget` (frames over `budget_ms`, default 33.3);
  and `spikes` (default 20, at most 500) each with its present number, tick,
  map, phases, counters, three biggest phases and `causes`. Every frame is
  also a CSV row under the cache directory's `captures/`; the reply gives the
  path. By ticks the window is 1 to 36000 (gives up after 600 s with
  `complete: false`); by `ms`, 1 to 600000. One capture at a time, read only.
  With frame stats off it answers `UNAVAILABLE`
  (`crates/extensions/agent/src/capture.rs`).
- **Causes.** `causes` counts every over-budget frame by cause (`causes` in
  `capture.rs`): `shader_compile`, `pipeline_compile`, `texture_uploads`,
  `gpu_readback`, `file_io`, `gpu`, `render_thread`, `pushbuf_decode`, else
  `guest_cpu`. Thresholds: see the [detail note](notes/2026-11-26-agent-api-detail.md), "`profile`: where a frame goes".
- **Native renderer counters.** `profile.counters` and `profile.capture` use
  the field names of `H2_FRAME_STATS_CSV`, with native columns per
  `native_<stage>_` prefix. Definitions:
  [native-renderer.md](../graphics/native-renderer.md#frame-counters). The
  `setup_*`, `d3d_*` and `map_slot_*` counters are in the note; the `d3d`
  ones are always on ([retire-d3d.md](../graphics/retire-d3d.md);
  [`render.residual`](#renderresidual-residual-d3d-work) breaks them down).
- **Fallbacks.** `profile.fallbacks {top}` (20 by default, 1 to 500) returns
  the interpreter's totals and entry addresses ranked by instructions, each
  with hits, x87 instructions and the enclosing function. Kept in
  `boot::fallback::Stats::by_site`, at most 4096 addresses (`sites_full`).
  Slim host modules add `cold_entries` and `cold_sites`.
- **Flat RAM window.** `profile.memory` sums the counters of every runtime's
  flat RAM window (`runtime::window::process_stats`;
  [contract-thin-transfers-flat-ram.md](contract-thin-transfers-flat-ram.md),
  section 4.1): `windows`, `aliased`, `fixups`, `pages_outside`,
  `congruence_skipped`. A slow page shows as `fixups` rising or
  `pages_outside` above zero.
- **Faults.** `profile.faults {top}` (20 by default, 1 to 500) is the fault
  census (`runtime::fault_census`,
  [block-clock-imprecise-faults.md](block-clock-imprecise-faults.md)):
  `generated`, `runtime`, `ended`, `continued`, `continued_generated`,
  `precise_exits_needed`, and the `top` PCs (`source`, `count`, `continued`,
  `addr`, `len`, `access`, `reason`), at most 4096 (`sites_full`). It also
  names `clock`, `imprecise_faults` and `continued_imprecise` (should be 0).
- **Sampling.** `sample: true` also runs the in-process sampler
  (`profile.sample.*`, `boot::sampling`); `sampling` is what
  `profile.sample.stop` answers (`profile.h2prof` under `H2_SAMPLE_DIR`).
  Only an h2 built with `--features perf` on a host with a sampler has it;
  otherwise `sampling.unavailable` says why ([xbox.md](xbox.md), "Profiling on
  the console").

## `render`: the frame and the pipeline

`render.frame` and `render.stats` are host methods (types in
`crates/extensions/agent-proto/src/render.rs`, logic in
`crates/extensions/agent/src/render.rs`).

```sh
h2 agent frame                     # the next whole frame: passes, totals
h2 agent frame draws               # ... and every draw, with its depth state
h2 agent frame constants -38..-35,58,59   # ... and those vertex constants on each
h2 agent frame snap 0,3            # ... and passes 0 and 3 read back as PNGs
h2 agent render                    # render.stats
```

### `render.frame`

`render.frame {"arm": true, "draws", "max_draws", "constants", "snap"}`
arms the recorder; the next present starts the frame and the one after
ends it, then it disarms. `render.frame {}` reads the state and the last
frame (`frame.seq` is the arming's number). `h2 agent frame` does both and
waits up to 10 s; a paused or loading game presents nothing. Unarmed, the
draw diagnostics cost one relaxed load per draw or clear. With a native
frame owner the recorder follows the worker's presentation choice; arming a
snapshot does not change it. Detail: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "`render.frame`".

- **The frame.** `totals` (passes, draws by outcome, vertices, clears,
  copies, reads, distinct targets); `passes` (runs of work on one colour
  surface and depth, each with target, clears, draws, vertices, outcomes,
  primitives, D3D entries, texture formats, pixel-program, blended and
  depth-writing draw counts); `ops` (copies and readbacks, with the draw
  count before them).
- **A draw** (with `draws`; at most `max_draws`, 2000 by default;
  `truncated` says when there were more). Its pass, D3D entry, primitive,
  vertex and index counts, outcome, texture formats, pixel program, blend
  and depth-write flags; `vs`: `fvf 0x...` or `prog @start n=slots #hash`
  (low 32 bits of the words' FNV-1a hash, stable across frames); and
  `depth`: `mode`, `w`, `func`, `write`, `viewport`, `scale`, `clip`,
  `full`, `clamp` (field meanings in the note).
- **Constants.** `constants` (with `draws`; at most 32 registers, D3D
  numbering -96..=95, hardware register = number + 96) records vertex
  shader constants on each listed draw:
  `"constants": {"-38": [x, y, z, w], ...}`. The CLI takes ranges
  (`constants -38..-35,58`), implies `draws`, and `max N` sets `max_draws`.
- **Snapshots.** `snap` (at most 4 pass numbers): when such a pass ends,
  its colour target is read back as BGRA and written as a PNG under the
  cache directory's `frames/`. The path or error is in `snaps`. A snapshot
  incurs a GPU round trip.

A private capture has its own recorder. When the private scene renderer is
installed, arm it before requesting a lighting capture, then read the
result:

```sh
h2 agent render.frame '{"source":"private","arm":true,"draws":true,"snap":[0]}'
h2 agent render.capture '{"lighting":"retail"}'
h2 agent render.frame '{"source":"private"}'
```

The next completed private draw supplies the report. Omitting `source` or
giving `"display"` uses the display recorder. `"private"` returns
`UNAVAILABLE` when the host has no private scene renderer.

### `render.frame_layout`

`render.frame_layout {"arm":true,"max_draws":2000,"snap":[0,3]}` arms the
same one-frame recorder as `render.frame` and includes draw rows
automatically. `max_draws`, `constants` and `snap` are optional with the same
limits. `render.frame_layout {}` reads the state and the last frame; both
methods share the sequence and the result.

`frame.draws[].layout` gives the guest target size and the host colour and
depth allocation sizes, the viewport and `clip` in guest pixels,
`display_clip`, the UI transform and `window_xy`. Each `sampled` entry gives
stage, texture, source surface, sizes, address modes and `uv`. Snapshots read
the host colour allocation at its actual size. Fields, the backdrop and
modal-blur workflow: see the [detail note](notes/2026-11-26-agent-api-detail.md),
"`render.frame_layout`".

The host records the layout in `crates/extensions/game/src/d3d_diag.rs` and
`crates/extensions/game/src/d3d_diag/layout.rs`. The display allocations and
readbacks come from `crates/platform/d3d/src/scaled.rs` and
`crates/platform/d3d/src/gfx_backend.rs`.

### `render.stats`

No params. `display_rate` is the frame owner's display-rate loop
([framescene.md](../graphics/framescene.md), section 7). `counters.line` is
the frame-stats window's summary; `counters` is `null` where that window is
not running. `counters` also reports per-frame means for
`shadow_walk_layers`, each `native_scene_lists_*` stage and the overlay's
`native_ui_overlay_*` counters.

| Counter group | Names |
|---|---|
| Overlay items | `items`, `quads`, `text_runs`, `fills`, `glyphs`, `blurs`, `model_views`, `batches` |
| Overlay refusals | `refused_material`, `refused_clip`, `refused_glyph`, `refused_texture`, `refused_model_view`, `refused_blur` ([retire-d3d.md](../graphics/retire-d3d.md#what-replaces-each-subsystem)) |
| HUD | `hud_tag_quads`, `hud_text_quads`, `hud_tag_fallback`, `hud_text_fallback`, `hud_tag_recorded`, `hud_tag_record_failed` ([the HUD on the overlay](../graphics/native-post-hud.md#the-hud-on-the-overlay)) |
| Menu shaders, screens | `menu_shader_quads`, `menu_shader_fallback`, `fade_quads`, `bar_quads`, `loading_quads`, `screen_fallback` ([the fade, the bars and the loading screen](../graphics/native-post-hud.md#the-fade-the-bars-and-the-loading-screen)) |
| Menus | `menu_claimed`, `menu_skipped`, `menu_passes`, `menu_mismatch`, `menu_refused_*` ([native-post-hud.md](../graphics/native-post-hud.md#the-menus-on-the-overlay)) |
| Modal blur | `modal_blurs`, `modal_blur_replayed`, `modal_blur_refused_*` ([the blur under a dialog](../graphics/native-post-hud.md#the-blur-under-a-dialog)) |
| Object shadows | the list, row gather and caster each count calls to their own entry ([native-light-gather.md](../graphics/native-light-gather.md), L7) |

The `_native` and `_fallback` fields and every overlay counter include
zeroes; another refusal reason appears when its window count is positive.
What each counter counts: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "`render.stats`".

`ui` is the layout the menus and the HUD are drawn with
([high-resolution.md](../graphics/high-resolution.md#the-games-640x480-assumptions)),
as the game's UI scopes last set it; absent before the first one drew.

| Field | Meaning |
|---|---|
| `output` | `[width, height]`, the window the UI is drawn at; the guest's 640x480 canvas before the first menu scope |
| `scale` | the menu fit's `[sx, sy]`, guest window units: guest `p` draws at `p * scale + offset` |
| `offset` | the fit's translation, guest window units |
| `anchor` | HUD corner groups: `pinned` to the window's edges at their physical scale, or `centered` on the safe area's edges |
| `atlas_scale` | the whole multiple of a glyph's own size the text atlas rasterises at (0 before the first text run) |
| `atlas_page` | the side of a glyph page in texels |

The fit is `render_bridge::ui_layout`'s one rule: the safe area is the
largest centred 4:3 box, and a live resize moves it on the next frame. A
check resizes the window (`host.window.resize`), reads `ui`, and expects
`scale` to follow the new window while `anchor` and `atlas_scale` stay.

```sh
h2 agent render.stats | jq .ui
```

`display_rate` is present while a paced frame owner runs it: a window or GPU
serve with the render thread on, `frame-scene.sink` not off, and no locked
mailbox (`H2_BOOT_STOP_AT_PRESENT`, an unpaced display or a headless run
lock it). Counts run since the loop was installed.

| Field | Meaning |
|---|---|
| `rate_hz` | the rate `graphics.fps` asks for; 0 at 30 |
| `shown_hz` | the rate after the headroom cap |
| `capped` | display frames do not fit, so the rate is held below `rate_hz` |
| `swaps` | owned presents the loop saw |
| `blended` | display frames drawn with blended cameras, at a Swap or between |
| `swap_blends` | of `blended`, the ones shown in place of a Swap's scene |
| `between_newest` | frames between Swaps drawn as the newest scene (alpha 1, a cut or a gap) |
| `failed` | display frames that failed; the window kept the frame before |
| `resets` | pairs dropped by a Swap without the graph |
| `avg_cost_us` | mean render-thread cost of a display frame |
| `moved_draws` | carried draws drawn on a blended camera |
| `moved_stages` | typed stages drawn on a blended camera |
| `held_stages` | typed stages in a game view drawn at their tick's camera |
| `marks_moved` | views on display frames whose world marks were moved onto the camera the frame drew them with |
| `marks_held` | views on display frames whose world marks kept their image's camera |

At `--fps 60` and above with the frame owner on, `blended` rises with play;
at 30 it stays 0 and `swaps` still counts. The loop is in
`crates/rewrites/frame-scene/src/display.rs` and
`crates/rewrites/native-render/src/frame_graph/display_rate.rs`.

```sh
h2 agent render.stats | jq .display_rate.blended
```

`display_frame` gives the identity, lighting and backend counter deltas when
the worker's latest presentation used the native graph; an original
presentation clears it. `render.scene` includes it when the map epoch, frame
and tick match its sealed scene. `private_frame` reports the counts and
identity of the most recent completed private target draw; it is absent
before the first capture and after a renderer reset, and `render.scene`
includes it when map epoch, frame, tick and summary hash match.

A completed native E3 draw includes an `e3` object in its `display_frame` or
`private_frame`, and the armed `render.frame` record includes it for the
draw. Its `passes` array has `e3.tiles`, `e3.sun_tiles`, `e3.sun`, `e3.marks`
and `e3.lights`, each with `name`, `draws` and `gpu_ns` (null for an empty
or unmeasured stage). The per-frame counters are `lights_lit`,
`lights_shadowed`, `lights_over_cap`, `lights_beyond_guest`, `sun_tiles`,
`sun_tiles_redrawn`, `sun_tiles_offscreen`, `atlas_halved`, `tiles_refused`,
`cones_shadowed`, `cones_flagged_off`, `flashlight_shadowed`,
`shadows_over_cap`, `cones_refused`, `cones_drawn` and `cones_kept`. They
count the E3 renderer's selections and atlas work
([e3-lighting.md](../graphics/e3-lighting.md)); the guest's light selections
are in `render.scene.views`. Retail graph draws omit `e3`. Counter meanings:
see the [detail note](notes/2026-11-26-agent-api-detail.md), "`render.stats`".

`H2_BOOT_GFX_BETWEEN=N` writes the first N display frames between scenes as
PNGs, with the guest frame before and after each ([switches.md](switches.md)).

### `render.capture_clip`

`render.capture_clip {"tick":9000,"ticks":180,"fps":120}` arms a clip on the
game thread. `tick` is 1 to 240 ticks ahead, `ticks` is 1 to 300, and `fps`
is 30, 60 or 120. The reply has `id`, `path`, `frames` and `written`. The GPU
hooks write `frame-00000.png` onward under `path`, a clip folder under the
headless graphics output directory.

`render.capture_clip.status {"id":1}` returns the same fields for the latest
clip; when `written` equals `frames`, all PNG files are closed. A lost guest
tick or missing display slot fails the request.

Clip capture needs headless GPU rendering. The [graphics size switch](switches.md)
sets the output dimensions. The writer takes one frame per tick, so `fps`
above 30 is refused.

The state is in `crates/extensions/agent/src/render/clip.rs`. The GPU writer
is in `crates/extensions/game/src/d3d_glue/clip_png.rs`.

### Picture feed: `render.picture`

A client shows the game's presented frames live: Studio's Game panel
([game-view.md](../editor/game-view.md)) and `h2 agent picture`. Frames come
from the handoff `host.screenshot` reads (the window's newest published
frame), not a sealed scene; for a still at a tick use `render.capture`.

| Method | Access | Params | Result |
|---|---|---|---|
| `render.picture.open` | read | `{width?, fps?, transport?}` | `{id, transport, path?, size, format, fps, at}`; then a `picture` event per frame (ring) or binary frames (stream) |
| `render.picture.close` | read | `{id}` | `{closed}`: whether the picture was open |

`transport` is `ring` (the default) or `stream`. `width` is the widest frame
wanted, 16 to 2560; a frame keeps its aspect and is never scaled up. `fps` is
1 to 60. A ring defaults to 1280 wide at 60, a stream to 640 wide at 15.
`size` is the largest frame, `[width, width]`. A game with no frame source
(no window: `h2 serve`, the Xbox app) answers `UNAVAILABLE`.

**Ring.** For a client on the same machine. `path` is a 0600 file under the
cache directory's `pictures/` with a 256-byte header and three RGBA8 slots
(`agent_proto::picture::Ring`; reader `Ring::read_newest`). The opening
connection gets one event per frame, sent only to it and dropped when its
queue is full (the frame is in the ring either way). Slot protocol and event
example: see the [detail note](notes/2026-11-26-agent-api-detail.md),
"Picture feed: `render.picture`".

**Stream.** For a game reached over `tcp://`. Open it on a connection of its
own: after the reply the connection switches to length-prefixed binary
frames, as `live.subscribe`'s does, and takes no more requests. A connection
that holds control, has subscribed or has requests in flight is refused with
`BUSY`; an in-process connection gets `UNAVAILABLE`. Each frame is a
little-endian u32 payload length (at most 16 MiB), a u32 header length, the
`picture` event's fields as JSON, and a JPEG (quality 75, no alpha)
(`agent_proto::picture::read_stream_frame`). At most three frames wait for
the socket.

**Cost and the idle skip.** The `agent-picture` worker thread reads the newest
presented frame back once per due frame of the fastest picture and scales it
on the CPU. A ring unacknowledged for a second (`IDLE_AFTER`) and a stream
with a full queue are skipped; with nothing due, nothing is read back.

**Lifetime.** `render.picture.close`, the opening connection closing, and the
game exiting each close the picture: a ring's file is removed, a stream's
connection ends.

```sh
h2 agent picture                           # a window showing the game, from a ring
h2 agent picture --width 640 --fps 30      # smaller, slower
h2 agent --socket tcp://10.0.3.29:4720 picture   # a remote game: the stream
h2 agent picture --frames 60               # no window: sizes, rate and ticks of 60 frames
```

The methods are answered on the executor (`crates/extensions/agent/src/exec/picture.rs`);
the pictures, the worker, the scaling and the JPEG are in
`crates/extensions/agent/src/render/picture.rs`; the wire types, the ring and
the stream codec in `crates/extensions/agent-proto/src/picture.rs`. The frame
source attaches at `agent_proto::picture::hub` (`crates/apps/app/src/picture.rs`).
The client is `crates/apps/cli/src/agent_picture.rs`.

### `render.marks`

`render.marks` (no params) reads where the window drew the world marks
([Drawing world-space lines over the frame](#drawing-world-space-lines-over-the-frame))
on its last 32 presents. The first call arms the window's probe and returns
`armed: false` with no presents; later calls return `armed: true` and
`presents`, oldest first. Each present has:

- `seq`, counting the window's draws since the probe was armed;
- `age_ms`, how long ago the window drew it;
- `frame`: `guest` (a genuine image), `synthetic` (an in-between frame at 60
  or 120 fps), `no_world_view`, `raw` or `loading`;
- `fraction`: a synthetic frame's blend fraction, 0 the frame before and 1
  the newest; null otherwise or without marks;
- `shifted`: the synthetic frame drew some view with another camera than its
  marks' image, and the marks were moved onto it
  ([high-framerate.md](../game/high-framerate.md), "World marks on made frames");
- `anchors`: the first 16 mark groups' anchors in window pixels through the
  first view's camera, null where off screen or behind it.

A headless host has no window and returns no presents. Code:
`crates/platform/gfx/src/world_lines/probe.rs`, `crates/apps/app/src/shell.rs`,
`crates/extensions/game/src/world_lines.rs`. The marks check: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "`render.marks`".

```sh
h2 agent render.marks
h2 agent render.marks | jq '.presents[] | [.frame, .fraction, .shifted, .anchors[0]]'
```

### `render.markers`

`render.markers` reads the Forge game type markers the window's last capture
drew ([gametype-markers.md](../game/gametype-markers.md), "The game").
Params are `{}` or `{"kind": "spawn"}`, a kind name of
`game_markers::Kind::name` (`spawn`, `flag_stand`, `hill`, `ff_drop`, ...) or
`wall`; a bad name or any other field is refused with `invalid params`. The
answer is

```json
{"shown": 2, "hidden_by_budget": 0,
 "markers": [{"thing": "starting_locations:0", "kind": "spawn", "team": 0,
              "anchor": [1.0, 2.0, 0.85], "lines": 38, "faded": false}]}
```

| Field | Meaning |
|---|---|
| `shown`, `hidden_by_budget` | markers drawn, and in range ones the 256 a view left out (nearest first); counts cover every kind, `kind` filters only `markers` |
| `thing` | `<block>:<index>`, `added:<op>`, `problem:<label>`, or for walls `zone:<hex>` |
| `team` | 0 to 7, or null |
| `anchor` | where the label sits, in world units |
| `lines` | the marker's line count |
| `faded` | a marker of another game type, drawn at a quarter |

Walls a running rules script shows (`show_area`) are rows of `kind` `wall`;
they count in `shown` and are not held to the 256. With no Forge session, no
wall and no present since, `shown` is 0 and `markers` is empty; a headless
host draws none.

The method is the host's (`crates/extensions/game/src/forge_glue/markers_world.rs`,
`add_method`; the name is `agent_proto::method::RENDER_MARKERS`).

```sh
h2 agent render.markers
h2 agent render.markers '{"kind":"hill"}'
```

### `render.particles.refusals`

`render.particles.refusals` says why the native particle path left draws to
D3D. It keeps, per named geometry check, a count and the shape of the first
draw that failed it. It takes no params; any field is refused with
`invalid params`. The answer has `refused` and `samples` keyed by check, each
with `label`, `producer`, `count` and `shape` (example: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "`render.particles.refusals`").

- The checks, in the order they run: `indexed`, `pretransformed`, `layouts`,
  `streams`, `stride`, `attributes`, `attribute_slot`, `primitive`,
  `vertex_count`, `data_len`, `data_type`, `inputs`, `corner_vertices`,
  `corner_compare`, `corner_differs`, `unpublished`, `data_differs`,
  `indices`, `raw_bytes`, `raw_attributes` and `raw_vertex_count`.
- `count` totals the run, not one present. `producer` is `Particle`,
  `Dynamic` or `Immediate`. Captured-particle fallback draws and refusals of
  other kinds (target, state, shader) are not counted.

The method is the host's (`crates/extensions/game/src/native_particles_admission.rs`,
`add_refusals_method`; the name is `agent_proto::method::RENDER_PARTICLES_REFUSALS`).

```sh
h2 agent render.particles.refusals
```

### `render.coop_look`

A read-only host method registered with the Live co-op prediction feature
(`crates/extensions/game/src/coop_look.rs`). With no parameters it returns
the most recent 256 rendered local views, oldest first; `{"after":123}`
returns samples newer than that cursor. `latest` is the most recent sample
number (initially zero); a gap means the reader fell behind. A sample shows
up a tick after its frame.

```sh
h2 agent --socket ~/Library/Caches/h2recomp/coop-bravo.sock render.coop_look
```

Each entry in `frames` has a monotonic `sample`, the guest render `frame`,
the local `user` (0 to 3), and a `status` such as `active`, `waiting`
(awaiting a baseline), `host`, `setting off`, `cinematic`, `dead`,
`director`, `vehicle`, or `no local control`.

| Field | Meaning |
|---|---|
| `sent` | Latest outgoing retail look, also recorded with the setting off for A/B timing |
| `confirmed` | Local player's yaw and pitch from the last applied update carrying that player's action |
| `predicted` | Current absolute target from the still-pending sends, or confirmed aim once they have drained |
| `camera`, `drawn` | First render camera before and after the adjustment; all aims are `{yaw,pitch}` in radians, or null when unavailable |
| `correction_rad` | Largest reconciliation error for this draw in radians (Euclidean length of shortest yaw difference and pitch difference); a pending turn's lead is measured by comparing predicted and confirmed |
| `update`, `last_send`, `matched_send`, `matched_update` | Last applied update, last sent sequence, and the sequence/update pair most recently matched by quantized aim; null before the event |
| `pending`, `dropped` | Unmatched sends, and the number superseded or expired since this continuous camera began; identical quantized actions have indistinguishable echoes |

### FrameScene checks: compare, capture, scene, parity, hashes

The checks the native renderer's slices gate on
([framescene.md](../graphics/framescene.md), sections 9 and 12). Host
methods; types in `crates/extensions/agent-proto/src/render.rs` and
`crates/extensions/agent-proto/src/sim/hashes.rs`, logic in
`crates/extensions/agent/src/compare.rs`, `crates/extensions/agent/src/render.rs`
and `crates/extensions/agent/src/sim/hashes.rs`, host glue in boot's
`agent_glue`, `agent_frames`, `d3d_diag`, `d3d_glue` and `scene_glue`. Tested
in `crates/extensions/agent` over synthetic images, the frame recorder's rig
and the sim report's tally.

```sh
h2 agent frame compare off/frame-03000.png on/frame-03000.png --diff d.png  # no game
h2 agent render.spawn_view '{"index": 0}'
h2 agent render.capture '{}'
h2 agent render.spawn_view '{"reset": true}'
h2 agent render.view '{"eye": [12.5, -3, 2.2], "yaw": 1.57, "pitch": -0.2, "fov": 70}'
h2 agent render.capture '{}'
h2 agent render.view '{"reset": true}'
h2 agent render.capture '{"tick": 9000}'
h2 agent render.capture '{"pass": 3}'          # the next tick's frame, pass 3
h2 agent render.capture '{"lighting":"retail"}' # latest sealed scene, private draw
h2 agent render.capture '{"lighting":"e3","tick":42}' # use the first reply's tick
h2 agent render.capture '{"lighting":"relight-off","tick":42}' # installed lamp, same scene
h2 agent render.scene '{"dump": true}'
h2 agent sim.hashes '{"from_tick": 3000}'
h2 agent render.parity '{"arm": true}'
```

| Method | Access | Params | Result |
|---|---|---|---|
| `render.compare` | read | `{a, b, diff?}`: two PNG paths, and where to write a difference image | `{size: [w, h], differing, max_channel, mean, first: [x, y] \| null, within_bar, diff?}` |
| `render.spawn_view` | act | `{index, yaw_offset?}` or `{reset: true}` | `{index, count, position: [x,y,z], yaw}` or `{reset: true}` |
| `render.view` | act | `{eye: [x,y,z], yaw, pitch, fov?}` or `{reset: true}` | `{eye, yaw, pitch, fov}` or `{reset: true}` |
| `render.view.current` | read | `{seat?}` | `{eye: [x,y,z], yaw, pitch, fov, private?}` |
| `render.capture` | read | `{tick?, pass?, lighting?, game_id?}` | `{path, tick, present, pass?, lighting?, scene_hash, source?}` |
| `render.scene` | read | `{dump?}` | `{sealed, map_epoch, frame, tick, hash, cut, views, counters, entries?, carried?, carried_census?, carried_activation?, carried_checkpoint?, visibility_first_mismatch?, light_visibility_first_mismatch?, first_person_visibility_first_mismatch?, view_record_first_mismatch?, dump?, display_frame?, private_frame?}` |
| `render.parity` | act | `{arm: true}` | `render.compare`'s fields plus `tick` |
| `sim.hashes` | read | `{from_tick?, max?}` | `{rows: [{tick, gameplay, arena?}], dropped, resets, truncated?}` |

- **`render.compare`.** Compares the colour channels of two PNGs (decoded to
  8-bit RGBA; alpha is ignored). `differing` counts pixels with any channel
  off by more than 2; `max_channel` is the largest one-channel difference;
  `mean` the mean absolute difference; `first` the first differing pixel in
  row order, `null` when none. `within_bar` is the parity bar:
  `max_channel <= 8` and `differing` at most 0.01% of the pixels. `diff`
  writes a difference image (directories made). Errors are `INVALID_PARAMS`
  with `data.error`: `size_mismatch` (`data.a`, `data.b`, each `[w, h]`),
  `read` or `write` (`data.path`). `h2 agent frame compare A B [--diff D]`
  runs the same function with no socket and exits 1 outside the bar.
- **`render.spawn_view`.** Selects a player start of the loaded scenario by
  zero-based index; the eye is 0.64 world units above it, looking along its
  facing, turned by `yaw_offset` radians. The reply gives the start count and
  the pose used. It answers when the world renderer accepts the private view;
  a following `render.capture {}` takes the next tick's frame. The camera
  stays until reset or the next game activation, and a new selection
  interrupts a pending request. An invalid index gives `INVALID_PARAMS`, a
  missing or unreadable start `UNAVAILABLE`, and a session that draws no
  world frames times out. `crates/extensions/game/src/agent_spawn_view.rs`
  shares Theater's playerless view in `spectator_view.rs`.
- **`render.view`.** Places the same private camera at an explicit eye in
  world units. `yaw` is the heading about +z in radians, from +x towards +y;
  `pitch` is up from level, within +-1.55; `fov` is the horizontal lens in
  degrees, 1 to 150 (without it the drawn view's lens is kept and the reply
  has `fov: null`). The reply echoes the pose with `yaw` wrapped into 0 to
  2pi. `render.view` and `render.spawn_view` select one camera: either
  replaces the other, and a pending request answers `INTERRUPTED` when it is
  replaced, reset or released by the next game activation. The camera is
  drawn only while its scenario is loaded. Missing, non-finite or
  out-of-range params, or `reset` with a pose, give `INVALID_PARAMS`; no
  loaded scenario gives `UNAVAILABLE`. Types: `ViewParams` and its `check` in
  `crates/extensions/agent-proto/src/render.rs`.
- **`render.view.current`.** Reads the pose of the view the game draws for
  local player `seat` (0 to 3, default 0) at the tick boundary, in
  `render.view`'s conventions, plus the usual `at` stamp. Feeding its `eye`,
  `yaw`, `pitch` and `fov` to `render.view` draws the same view in a test
  boot. While a `render.view` or `render.spawn_view` camera is selected and
  loaded, the reply is that camera's pose with `private: true`. No view
  record for the seat (no world, a cinematic, a seat without a screen) gives
  `UNAVAILABLE`; a `seat` outside 0 to 3 or unknown params give
  `INVALID_PARAMS`. A Theater camera is not reported. Types:
  `CurrentViewParams` and `CurrentView` in `agent-proto/src/render.rs`, the
  method in `crates/extensions/game/src/agent_spawn_view.rs`
  (`spectator_view::drawn_view`, `CurrentView::looking`). Derivation and the
  player-body caveat: see the [detail note](notes/2026-11-26-agent-api-detail.md),
  "`render.view.current`".
- **`render.capture`.** Takes a PNG of the frame for game tick `tick` (default
  the next, at most 240 ahead). Four modes:

  | Mode | Params | What it gives |
  |---|---|---|
  | Presented frame | none of the below | the presented frame at the render scale, as `H2_BOOT_GFX` writes `frame-N.png`, at `frames/capture-<pid>-<seq>-tick-<tick>.png` under the cache directory |
  | Pass | `pass` | the frame recorder (`render.frame`) is armed at the end of the previous tick and reads that pass back at 1x; it replaces a not-yet-done `render.frame` arming, and one made while it waits answers `INTERRUPTED` |
  | Private lighting | `lighting`: `retail`, `e3`, `relight-on` or `relight-off` | the native renderer draws a sealed scene into a private target; default scene is the latest sealed, an explicit `tick` selects that scene |
  | Live gallery | `game_id` (after selecting a spawn view) | waits for a whole frame drawing that view at or after `tick`; adds `source: {game_id, map_id, map, camera, digest}` (`camera` has `position`, `forward`, `up`; `digest` is the SHA-256 of the PNG); the Live CLI adds the capture permit |

  On the D3D path a tick's frame is the first guest present after it; `tick`
  in the answer is the last tick that ended before that present (later than
  asked when two ticks ran in one frame) and `present` numbers it as
  `render.frame` does. The presented-frame and `pass` modes need a GPU run
  with `H2_BOOT_GFX` (`H2_BOOT_GFX_EVERY=never` writes nothing else);
  without it, or under `H2_BOOT_GFX_LIGHTING_AB`, the answer is
  `UNAVAILABLE`. `scene_hash` is the sealed scene's summary hash at that
  present (`null` with no source). The answer waits for the frame (up to the
  300-tick limit for deferred answers); a paused game holds it, and a new
  `render.capture` replaces a waiting one, which answers `INTERRUPTED`.
  Unasked, the cost is one relaxed load per tick and per present. Capture
  does not wait for unresident scene content (framescene.md 9.2).

  Private lighting: the most recent private capture retains its scene for
  another request with the same tick; a renderer reset releases it. A missing
  private renderer or an unreplayable scene gives `UNAVAILABLE`, as does a
  failed original Swap (`reference present failed`). `lighting` with `pass`,
  or `game_id` with `pass` or `lighting`, gives `INVALID_PARAMS`. The relight
  modes need a run started with either reconstruction selected and a matching
  installed bake; with no selected range binding a page whose baked states
  differ the answer is `UNAVAILABLE` (`no native world range in this scene
  uses the installed lightmap`). `lighting` in a successful reply echoes the
  requested state. Pass both paths to `render.compare` to compare modes while
  play continues. See the
  [three-render comparison](../graphics/relighting-review.md). Tests:
  `private_captures_draw_the_same_pinned_scene_with_each_requested_mode` and
  `retained_stage_uses_the_current_private_attachments_on_every_draw`.
  Resolution, retention and tick details: see the
  [detail note](notes/2026-11-26-agent-api-detail.md), "`render.capture`".
- **`render.scene`.** Reads the last sealed `FrameScene` from the source the
  frame step registers (`agent::render::set_scene_source`, in boot's
  `scene_glue` over `render_bridge::scene_step`). Map activation or snapshot
  load clears the scene and any private capture pin; `sealed` stays false
  until a frame on the new timeline seals. With no source registered the
  answer is `UNAVAILABLE`, "no scene has been built". `{"dump":true}` writes
  the whole scene as JSON to `scenes/scene-<frame>-<tick>.json` under the
  cache directory and `dump` names it. Per-field detail, the full counter
  lists and the dump layout: see the
  [detail note](notes/2026-11-26-agent-api-detail.md), "`render.scene`".

  | Field | Holds |
  |---|---|
  | `sealed`, `map_epoch`, `frame`, `tick`, `hash`, `cut` | whether a scene is sealed, its map epoch, frame, tick, summary hash and cut reasons |
  | `views` | per view (`key` is `kind:player:view`: `game:0:0`, `first_person:0:0`): cut, clusters, instances, parts, rows, lights and their layers, shadow maps, layer items and remainders, `part_flags`, decorators, the late passes `fog` (layer 16), `water` (layer 6) and `post`, `camera_use`, and for a game view `visibility_clusters`, `structure_clusters`, `visibility_objects` and `visibility_source` (`rust` or `guest`, [native-visibility.md](../graphics/native-visibility.md)) |
  | `counters` | session totals by family (below) |
  | `visibility_first_mismatch` | first game view whose Rust visibility differed from the guest's: key, frame, tick, first difference, counts each way |
  | `light_visibility_first_mismatch` | the same for the first light |
  | `first_person_visibility_first_mismatch` | first differing first-person slot count, handle, record count or record |
  | `view_record_first_mismatch` | first game view whose Rust-built records differed, and the first field (`position`, `forward`, `up`, `fov`, `viewport`, `near`, `far`, `projection`, `start cluster`, `mode`) |
  | `entries` | entry census, only with `frame-scene.census` in `H2_FEATURES` (`render_bridge::entry_census`): `by_entry` keyed `layer.<site>.<layer>` (site `scene`, `alternate`, `first_person`, `light`, `object_shadow`, `other`), `light_passes`, `object_shadow`, `screen_effects`; each `{calls, d3d_draws, seed_steps, effects_seed_steps, seed_unmatched, animation_gets, particle_updates}`; plus `unbalanced`, `draws_counted` |
  | `carried`, `carried_census`, `carried_activation`, `carried_checkpoint` | with the [scene sink](../graphics/framescene.md#slice-6-the-renderer-owns-game-view-frames) enabled: guest D3D ops by frame slot (`opaque`, `after_opaque`, `after_lights`, `transparents`, `after_post`, `overlay`, `outside`), side targets and readbacks of the last frame; the census across frames; the load activation record; the census frozen at original Swap 3000. All absent until the sink records a frame |
  | `display_frame`, `private_frame` | the display worker's and private renderer's frame observations |

  Counter families in `counters` (names under the prefix in the table):

  | Prefix | Counts |
  |---|---|
  | `scene_visibility_*`, `scene_light_visibility_*`, `scene_geometry_lists_*`, `scene_view_record_*` | the Rust-versus-guest visibility, light, geometry-list and view-record differentials (`checked`, `equal`, `differ`, `taken`, `ns`, `declined_<reason>`, lens counters) |
  | `scenes_sealed`, `scenes_unsealed`, `scene_views`, `scene_no_camera`, `scene_refused_writes`, `scene_layers*` | scene and layer totals |
  | `scene_decorator_*` | decorator draws, placements, grid producer and stage ([native-decorators.md](../graphics/native-decorators.md)) |
  | `scene_light_*`, `scene_host_lights_*`, `scene_e3_*` | light extraction, groups, entries left to the guest by cause, host and E3 lights |
  | `scene_shadow_*`, `scene_dropped_shadow_layers` | shadow groups, filters and chains ([native-shadow-layers.md](../graphics/native-shadow-layers.md)) |
  | `scene_fog_*`, `scene_water_*`, `scene_post_*`, `post_effect_refused_<cause>` | late layers and post passes (framescene.md, slice 5) |
  | `scene_stages_<stage>`, `scene_dropped_<stage>` | stages queued to the render thread and dropped (`world`, `objects`, `fog`, `water`) |
  | `scene_world_stipple_*` | fade records and how the display detail drew them ([occlusion-culling.md](../graphics/occlusion-culling.md), "Projected size and level of detail") |
  | `layer_plan_*`, `native_{fog,water}_walk_*`, `sky_plan_*`, `scene_submitted_*` | native layer plans and refusals by cause, sorted range admission, sky and flare draws (`render_bridge::native_layers`; off with `frame-scene.layers=off` in `H2_FEATURES`) |
  | `scene_carried_*`, `scene_side_target_frames`, `scene_readback*` | the sink's carried ops and guest readbacks |
  | `scene_window_*` | per-device interval counters (`start`, `through`, `owner_enabled`, `gaps`, `games`, `owned`, `ordinary`, `failed`, `stage_drops`, `unresident`, palette and scale histograms) |
  | `scene_owned_*` | the frame owner's presentations, declines, candidates, barriers, mismatches, refusals, scaled grids ([framescene.md](../graphics/framescene.md), slice 8, "Fractional grids") |
  | `scene_private_*` | the private renderer's publication queue |
  | `scene_residency_*`, `scene_unresident`, `scene_palette_*` | object residency and palette gauges |

  The world stipple and window counters are session totals: the delta between
  two `render.scene` replies is a frame's.
- **`render.parity`.** `{"arm": true}` requests one game frame from the sealed
  scene's private renderer and the D3D display and returns the sealed scene's
  `tick` with `render.compare`'s comparison. The reference is the ordinary
  display surface before gamma, at its display scale, and needs Retail
  display lighting: an E3 display returns `UNAVAILABLE`. The comparison covers
  the carried guest commands; pixel parity of a typed substitution needs a
  separate reference run exercising that stage. The request completes at a
  tick boundary after both paths finish; a new arming interrupts one waiting.
  Unavailable inputs or a missing reference or private renderer give
  `UNAVAILABLE`; no game frame within the deferred request limit times out;
  `{"arm": false}` gives `INVALID_PARAMS`. The sink retains the guest draws
  while armed. Reference and failed-Swap details: see the
  [detail note](notes/2026-11-26-agent-api-detail.md), "`render.parity`".
- **`sim.hashes`.** The running game's post-tick digests, in the rows sim mode
  writes as `state_hashes_v1` (sim.md §6): `tick`, `gameplay` (16 hex digits)
  and, under `impl.sim.arena-hash`, `arena`. Boot's `agent-frames` tick
  observer computes them with sim mode's digest function and
  `impl.sim.metrics-projection` switch on any run with an agent socket. It
  keeps the last 108,000 rows (`dropped` counts the oldest lost); a tick
  before the last, or more than 30 after it, restarts the series (`resets`).
  `from_tick` and `max` pick rows; `truncated` says more matched. Compare two
  launches row by row:

  ```sh
  diff <(jq -c '.rows[]' off-hashes.json) <(jq -c '.rows[]' on-hashes.json) | head
  ```

### `render.residual`: residual D3D work

What the guest front-end still asks `d3d` to do, by caller and by
producer, over a window of presents: the census every slice of
[retire-d3d.md](../graphics/retire-d3d.md) gates on. Types in
`crates/extensions/agent-proto/src/render.rs` (`ResidualParams`,
`ResidualReply`), logic in `crates/extensions/agent/src/render/residual.rs`,
the window and the producer registry in
`crates/rewrites/render-bridge/src/residual_census.rs`, the adapter's
watch in `crates/platform/d3d/src/residual.rs` and
`crates/platform/d3d/src/runtime_glue.rs`, glue in boot's `scene_glue`
and `d3d_glue`. Registered with the frame methods; answers
`UNAVAILABLE` until the GPU renderer's scene glue has wired the census.

```sh
h2 agent residual 600                          # arm, wait, print with per_frame
h2 agent render.residual '{"arm": {"frames": 900}}'
h2 agent render.residual '{"top": 500}'        # the last finished window
```

| Method | Access | Params | Result |
|---|---|---|---|
| `render.residual` | read | `{arm?: {frames}, top?}`: `frames` 1 to 3600, `top` 1 to 500 (50) | `{state, window?, totals?, producers?, callers?, unattributed?, sites_full?, unbalanced?}` |

- **Arming.** `{"arm": {"frames": N}}` starts a window at the next guest
  `Swap` and ends it after N more; a new arming replaces one in progress.
  `{}` reads. `state` is `idle`, `armed` or `done`; the other fields are the
  last finished window's. `h2 agent residual N [top K]` arms, polls until a
  new window is done, and adds `per_frame`: each total over the window's
  presents.
- **`window`.** `{frames, game_frames, presents: [first, last], ticks:
  [first, last], map, timing}`. `presents` are `Swap` ordinals since process
  start; `ticks` the last tick ended at each end (`null` before the first).
  `game_frames` goes through the `game_view` producer, so it is zero without
  `frame-scene.census`. `timing` is true on a `render-timing` run.
- **`totals`.** Counters over the window:

  | Field | Counts |
  |---|---|
  | `draws` | guest draws `d3d` resolved, inline ones included |
  | `claimed` | of those, a native replacement committed |
  | `residual` | reached the backend (growth of `d3d::device::backend_draws`) |
  | `entries`, `push_words`, `state_words` | D3D entry calls, decoded push words, deferred-state words applied |
  | `clears`, `binds`, `copies`, `uploads` | clears, target/texture/palette binds, frame-buffer copies, texture uploads |
  | `locks`, `queries`, `presents`, `d3d_ms` | `BlockOnResource`/`BlockOnTime`/`IsBusy`, visibility tests and results, presents, time in the adapter |
  | `native_rect_clears`, `clear_refused_rect_count`, `native_guest_binds`, `guest_bind_refused_destroy` | native rectangle plans, longer lists on the linear path, title binds without destruction, final releases (also on producer rows; helper rows `scene_clear`, `target_select`) |
- **`callers`**, at most `top`, by `residual`, then calls:
  `{function, sites, entries, draws, inline_draws, residual, d3d_ms}`. A
  caller is the analysed guest function holding the entry calls' return
  addresses (read at `[esp]`), named as `profile.fallbacks` names one
  (`recomp::fallback::function_at`), or the bare return address before the
  analysis is built. At most 4096 return addresses are kept; later ones count
  in `totals` only and `sites_full` is true.
- **`producers`**, at most `top`, by `residual`, then `entries`:
  `{producer, status, slot, calls, draws, claimed, residual, entries,
  push_words, state_words, clears, binds, native_rect_clears, clear_refused_rect_count,
  native_guest_binds, guest_bind_refused_destroy, copies, uploads, d3d_ms,
  guest_ms}`. Present only with `frame-scene.census` in `H2_FEATURES`, which
  places entry and exit observers on each producer in
  `residual_census::REGISTRY` and on the layer draw (`layer.<site>.<layer>`).
  Work goes to the innermost open producer, so rows sum to `totals`; inside a
  render frame with none open it is `unknown`, outside it `outside_render`.
  `status` is `native`, `d3d` or `partial` (`residual_census::PARTIAL_LAYERS`
  lists the partial layers); `slot` is the carried slot; `guest_ms` is the
  producer's exclusive time less its D3D time, `null` without
  `render-timing`. `unbalanced` counts producer exits with nothing to end and
  producers dropped unended. Producer list, status rules and layer lists:
  see the [detail note](notes/2026-11-26-agent-api-detail.md), "`render.residual`".
- **`unattributed`.** `{draws, entries}`: with producers, the `unknown`
  row's; without, the inline draws, which no producer owns.
- **Cost.** Unarmed, the adapter does one relaxed load per entry call. Armed,
  it reads `[esp]`, copies the device's tally three times and times the call.
  The producer observers exist only with `frame-scene.census` and drain only
  while armed. The always-on frame counters `d3d_entries`,
  `d3d_residual_draws` and `d3d_push_words` are in
  [`profile`](#profile-where-a-frame-goes).
- Tested in `crates/rewrites/render-bridge` (the window, caller rows,
  producer charging, claimed and residual totals, `unknown`), in
  `crates/extensions/agent` (ranking, grouping, params) and in `d3d`'s
  draw rig (`tests/residual_tally.rs`).

## Load timeline

`load.timeline {"last": N}` is a core method (8 by default, 1 to 32 kept;
`crates/extensions/agent/src/loads.rs`).

```sh
h2 agent loads                     # the last 8 loads, BSP switches and stalls
h2 agent wait '{"phase":"gameplay","map_ticks":30}' && h2 agent loads 1
```

- **What a load is.** Read at each tick: the map changed (`map`), the BSP
  changed (`bsp`), or no tick came for 500 ms (`stall`, where the `loading`
  phase starts). The run's first tick ends a `boot` load. Each load has a
  `stall` segment and a `settle` segment (the next 30 ticks); a load cut short
  by the next says `settled: false`.
- **What each segment counted.** The difference of the frame-stats running
  totals (`timeline::phase::totals`): frames, ms per phase and each counter
  that moved (`read_bytes`, `uploads`, `shaders`, `pipelines`, `readbacks`,
  ...). Frames count by present. With frame stats off, `no_frame_stats` says
  the counts are missing.
- **Now.** While no tick has come for 500 ms, `current` is the stall so far.
- Seeks and cache hits are not counted (the kernel's file I/O counts bytes
  and host time: `read_bytes` and `file_io_us`).

## Game code: full speed or the slower start

`game.code` (read, no params) says which game code this start of `h2 play`
runs ([cranelift-tier.md](../lifter/cranelift-tier.md)): clang's
full-speed module, or the Cranelift fast tier (`--tier fast`, or
`bridge` while clang builds the full-speed one in the background). Host method,
`crates/extensions/game/src/game_code.rs`; `h2 play` supplies it, other
hosts don't have it.

| Field | Holds |
|---|---|
| `tier`, `full_speed` | `fast` or the full-speed tier; whether it is full speed |
| `module`, `stamp` | the module path; the module stamp's first line |
| `background` | `{state, log, elapsed_s, typical_s, step}` |
| `restart` | `{ok, why}`: whether `game.code.restart` would run |

`background.state` is `building`, `ready`, `failed` (from the log: its
`clang (...): N compiled` summary line, or an `Error:` line), or `none` when
this start began no background build. `elapsed_s` is the log's age;
`typical_s` the estimate the banner counts down from; `step` the log's last
line.

`game.code.restart` (act, no params) starts `h2 play` again in this process:
the same executable and arguments by `exec`, with the pid-named agent socket
freed first, so the new start runs clang's module. Unsaved progress is lost.
It is refused (`UNAVAILABLE`, with `restart.why`'s text) unless this start
runs the fast tier, the background build is ready and clang's module exists.

## Returning to the main menu

`game.menu` (act, no params) leaves the running campaign or multiplayer
game for the main menu as the title's own departure does: at the next
tick the agent's guest calls retire the network sessions
(`LEAVE_SESSIONS`) and call `request_mainmenu(0)`
([level-boot.md](level-boot.md#returning-to-the-main-menu)). Host method,
`crates/extensions/game/src/game_menu.rs`; it needs the `agent-calls`
feature.

The answer comes once the calls are made: `{"from": <mode>, "queued":
true, "calls": 2}`, where `from` is the game's options mode (1 campaign,
2 multiplayer). At the menu, or with no game running, it is refused
(`UNAVAILABLE`, "already at the main menu"). Wait for the menu with
`session.wait`, then measure it:

```sh
h2 agent game.menu
h2 agent wait '{"map":"mainmenu"}' 120000 && h2 agent loads 1
```

## Loading a level

`game.load {scenario, difficulty?, variant?}` (act) queues any campaign
mission or multiplayer map on the disc as the next game, from the main
menu or a running game. Host method, `crates/extensions/game/src/game_load.rs`;
it needs the `agent-calls` feature. The options are the level boot's
([level-boot.md](level-boot.md)), made by the agent's guest calls at the next
tick (`Call::QueueGame`), as `maps.test.load` queues a test map.

| Param | Values |
|---|---|
| `scenario` | a map file's stem (`03a_oldmombasa`, `lockout`) or a scenario path, any case, or a level of a campaign installed under the data root (`a30`); resolved as `h2 play --level` does (`level_boot::resolve_in`) |
| `difficulty` | campaign only: `easy`, `normal` (default), `heroic`, `legendary` |
| `variant` | multiplayer only, a game type: `slayer` (default), `oddball`, `juggernaut`, `king`, `ctf`, `assault`, `territories`; a local game on that game type's default variant, built by `VARIANT_DEFAULTS` ([community-gametypes.md](../game/community-gametypes.md)) |

The answer comes after the next tick: `{name, scenario, kind, difficulty?,
variant?, check, queued}`, `kind` being `campaign` or `multiplayer`,
`check` the game's options check and `queued` whether the options went to
the next transition. Wait for the level with `session.wait` on its `map`
and `phase: "gameplay"`.

Refusals: an unknown level is `INVALID_PARAMS` listing the disc's
levels; so are a `difficulty` for a multiplayer map, a `variant` for a
campaign mission and an unknown difficulty or game type. A co-op or
networked session (simulation modes 2 to 5) is `UNAVAILABLE`: leave it first
with `game.menu`. Without the `agent-calls` feature, `UNAVAILABLE`.

`game.levels` (read, no params) lists what `scenario` takes:
`{campaign: [{name, scenario}], multiplayer: [{name, scenario}],
difficulties, variants}`, the disc's maps sorted by name (read once and
kept). Installed campaigns' levels are not listed but load by name.

```sh
h2 agent game.levels
h2 agent game.load '{"scenario":"03a_oldmombasa","difficulty":"heroic"}'
h2 agent wait '{"map":"03a_oldmombasa","phase":"gameplay","map_ticks":30}' 120000
h2 agent game.load '{"scenario":"lockout","variant":"ctf"}'
```

## Live session links

`live.join.open {"url":"..."}` submits a website link to the same inbox as
the macOS app and `h2 play --join`. It returns `{"id":1}` on acceptance.
Malformed links return `INVALID_PARAMS`. Reopening the same pending link
returns its existing ID. A different link during a join returns `BUSY`.
The player confirms an agent-delivered join, including at the main menu.
Offline, dedicated host, simulation and film runs return `UNAVAILABLE`, as
does a player run with join links disabled.

`live.join.status` takes no parameters. Its `join` field is null until the
first request. It then contains `id`, `server`, `session` and `phase`.
`queued` means the game has yet to claim the request. `signing_in` waits
for the account picker, `resolving` checks admission, `confirm` includes
the host's gamertag when known, and `joining` waits for the title to connect.
The last outcome stays available as `joined`, `cancelled` or `failed`
with a human-readable `reason` on failure. Use `ui.press` to operate the
in-game confirmation. [Live session links](../live/live-custom-games.md#join-in-halo-2)

```sh
h2 agent live.join.open '{"url":"h2recomp://join?server=https%3A%2F%2Flive.forbes.red&session=8000000000000001"}'
h2 agent live.join.status
```

### Matchmaking transitions

`live.matchmaking.trace` takes no parameters. `transitions` holds up to 128
changes in capture order; `hopper_failures` holds the last block-read failure
per descriptor (hopper index, guest caller, message). An empty rotation names
the unavailable map IDs. Call it after a match starts or returns to the party.

Each row has a `sequence`, an `event` (`frame`, `matchmaking_update` or
`release_key`, which adds the guest return address as `caller`) and a `state`:
guest lifecycle (`6` matchmaking progress, `1` party lobby), the update's
`result` (`3` while active), and the arbitration state and result.

`result` `2` is a started match. Any other value but `3` sends the party back
to its lobby, and the game logs `live: matchmaking ended with result R:
<reason> (arbitration state S, result A)` once, from
`game::agent_glue::matchmaking::result_reason`:

| `result` | set by | meaning |
|---|---|---|
| `1` | `0x72330` | the matchmaking setup failed |
| `4` | `0x72260` | the party leader cancelled (squad `+0x49fc`) |
| `5` | `0x707d0` | a party console failed to load the map |
| `6` | `0x72540` | the party changed or left the hopper |
| `7` | `0x66050`, `0x718b1` | the party or the target has no hopper or roster for it |
| `0xe` | `0x717b6` | the target session left its lobby states |
| `0x11` | `0x72260` | this console's arbitration registration failed (peer status above 2) |
| `0x13` | `0x70765` | the update's `+0x10` flag is clear |

`squad` and `target` are null when the session is absent; each carries
session id, state, class, hopper index, host and local peer indices, player
and roster counts, arbitration and countdown, `roster_ids_present`, and peers
(map, hopper, arbitration status, phantom). Unavailable readings are null. The
history holds no exchange keys, names or player IDs. Sampling points
(`RENDER_FRAME`, `0x706f0`, `0x5fb60`, `0x71730`): see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Matchmaking transitions".
[Clankermatch lobby](../game/bots.md#84-search-and-lobby-built-the-empty-search)

```sh
h2 agent live.matchmaking.trace
```

## `host`: the process and its lobby

### `host.perf`

The newest host perf line (`boot::host_perf`): `{line, age_ms}`;
`UNAVAILABLE` until one exists (`H2_HOST_PERF=1`; one every 100 frames or
5 s). `h2 agent hostperf`.

### `host.metrics`

`h2 agent hostmetrics`; no parameters. It reports the latest complete window
of 30 intervals between returns from `GAME_TICK` (`0x00137fe0`); until 31 tick
returns it answers `UNAVAILABLE`
([`crates/extensions/game/src/host_metrics.rs`](../../crates/extensions/game/src/host_metrics.rs)).

| Field | Meaning |
|---|---|
| `ticks` | Tick intervals in the sample (30) |
| `game_cpu_ms_per_tick` | Game-thread CPU per tick interval, including work between ticks |
| `process_cpu_ms_per_tick` | CPU on all process threads per tick interval |
| `wall_ms_per_tick` | Wall time per tick interval, including pacing sleeps, pauses and loads |
| `wall_ms_per_tick_p99` | Light host: nearest-rank p99 of multiplayer tick intervals in the last 30 s; `null` when empty |
| `game_cpu_ms_per_tick_p99` | Light host: p99 of per-tick game-thread CPU deltas over the same window; clears when gameplay ends; `null` when empty or no thread clock |
| `late_ticks` | Intervals in that window longer than 36 ms; both interval fields are `null` outside the light profile |
| `cpu_percent` | Whole-process CPU over the wall interval; 100 is one core |
| `cpu_seconds` | User and system CPU seconds used by this process |
| `package_idle_wakeups`, `interrupt_wakeups` | Process-lifetime macOS `proc_pid_rusage` counts, sampled together |
| `wakeups_at_ns` | Monotonic sample time in ns, or `null`; divide count change by it for wakeups per second |
| `rss_bytes` | Resident bytes at interval end (macOS, Linux) |
| `pss_bytes`, `peak_pss_bytes` | Current and highest sampled Linux proportional resident bytes |
| `private_bytes`, `shared_bytes` | Private and shared resident mappings (Linux smaps) |
| `private_dirty_bytes`, `shared_clean_bytes` | Linux smaps Private_Dirty and Shared_Clean |
| `age_ms` | Time since the 30-tick CPU sample ended |
| `host_light` | Whether the process uses the light host profile |

An unavailable platform counter is `null`. The CPU fields exclude sleeping;
the wall field shows the pace the host sustained.

### `host.memory`

`h2 agent hostmemory` reads the process watchdog; no parameters, no file.
`h2 agent hostmemory report` sends `{"report":true}`, collects a diagnostic
report on the connection thread, and returns its path as the last item in
`reports`; a write failure is `UNAVAILABLE`.

| Field | Meaning |
|---|---|
| `footprint_bytes`, `peak_bytes` | Latest and highest sampled physical footprint |
| `soft`, `hard` | Process limits in bytes; zero disables one |
| `growth_bytes_per_min` | Estimated footprint change per minute |
| `samples` | Last 60 samples, oldest first, `[ms_since_watchdog_start, bytes]` |
| `probes` | Named subsystem counts (pending requests, socket reply buffers, queued pad steps) |
| `reports` | Recent report paths on the game host |

### Explain memory growth

The map-load and cache probes appear in `host.memory`, its reports and the
flight recorder. The footprint also includes driver allocations, in-flight GPU
work and heap pages the allocator keeps.

| Probe | Ownership measured |
|---|---|
| `native.map_retired_payloads` | Detached native payloads queued for or being destroyed by the retirement worker |
| `native.decoded_image_bytes` | Capacity of shared native RGBA, mip and BC payloads, counted once per allocation |
| `native.light_model_entries` | Models retained for standalone light and shadow passes |
| `native.world_cpu_geometry_bytes`, `native.object_geometry_buffer_bytes` | Retained world vertex/index vector capacity and live object buffer size |
| `native.world_buffer_cache_bytes`, `native.world_section_payload_bytes`, `native.transparent_packed_bytes` | Indexed draw caches and packed transparent geometry |
| `native.world_section_windows`, `native.transparent_packed_bindings` | Live indexed windows and packed binding entries |
| `gfx.pipeline_cache_entries`, `gfx.shader_cache_entries`, `gfx.texture_group_cache_entries` | Entries across live renderers or binding sets |
| `gfx.pipeline_draw_jobs`, `gfx.pipeline_prewarm_jobs` | Queued compile slots, including cancelled slots awaiting removal |
| `d3d.nv2a_program_entries`, `d3d.nv2a_archive_pending_keys`, `d3d.nv2a_archive_pipeline_records` | Translated programs, unregistered archive keys, archive pipeline records |
| `gfx.upload_ring_staging_bytes`, `gfx.upload_ring_buffer_bytes` | CPU capacity and GPU buffer size of live upload rings |
| `gfx.upload_idle_cpu_bytes`, `gfx.upload_idle_gpu_bytes` | The idle subset of those totals |
| `assets.rewritten_map_cache_bytes`, `assets.rewritten_shared_map_cache_bytes` | Rewritten level and shared resource payloads in the VFS cache |

Compare menu, level activation, settled play and menu return in the memory
history; `host.memory.heap` breaks out live Rust allocations by scope. See the
[detail note](notes/2026-11-26-agent-api-detail.md), "Explain memory growth".

### Finding a leak after the fact

Every watched process (`h2 play`, `run`, `serve`, `h2 lab host`, `h2-editor`)
keeps evidence that outlives it
(`crates/platform/mem-watch/src/recorder.rs`):

| Evidence | What it is |
|---|---|
| Flight recorder | One JSON line every 10 s to `mem-<pid>.ndjson` in `H2_MEM_REPORT_DIR`, else `logs/mem/` under the data directory; rows carry `t_ms`, `unix_ms`, `footprint_bytes`, `soft`, `probes`, `tags`, `stacks`, `gpu`, `level`, `tick`, `agent_connections`, `agent_calls`; hourly or 8 MiB segments, seven per process; a clean exit appends `{"closed_cleanly":true}` |
| Reports | `mem-<pid>-<nanos>-<serial>.json` beside the recorder on a soft-limit, sustained-growth or hard-limit trigger; the hard limit's report is written before exit code 86; `--fresh-data` follow/watch runs use `<cache>/data/mem` (`crates/apps/cli/src/follow.rs`) |
| Recovery | A starting recorder moves unfooted or over-soft streams to `reports/mem-<pid>-<nanos>/`, deletes clean ones, and shares a 512 MiB budget per directory |
| Heap tags | `mem_watch::TaggedAllocator` tags each allocation by thread scope (`mem_watch::scope("assets.bitmap")`); untagged counts as `unscoped`; counts are requested bytes |
| Sampled stacks | `H2_MEM_SAMPLE` (a mean interval such as `1M`) records Poisson-sampled allocation stacks; unset captures none |
| GPU census | wgpu `generate_report` counts, HAL byte counters (Vulkan, D3D12), `metal.current_allocated_bytes`, and the `gfx.*` and `native.*` probes |

Details of each: see the [detail note](notes/2026-11-26-agent-api-detail.md),
"Finding a leak after the fact".

All five methods run on the connection thread, one at a time per process; a
second concurrent call is `UNAVAILABLE`.

| Method | Params | Result |
|---|---|---|
| `host.memory.history` | `{"since_ms"?}` | `{"records":[row],"next_since_ms"}`: up to 24 rows or 512 KiB from `since_ms`; call again with `next_since_ms` until it is `null` |
| `host.memory.heap` | `{"top"?: 1..128, default 20}` | `{"sample_interval_bytes","dropped_samples","tags":{name:{"live_bytes","live_allocations"}},"stacks":[{"live_bytes","live_allocations","frames":[{"ip","symbol"}]}]}`; `UNAVAILABLE` unless `H2_MEM_SAMPLE` was set at start |
| `host.memory.gpu` | `{}` | the census, `{name: value}`; names ending `_bytes` are bytes, the rest counts |
| `host.memory.diff` | `{"from_ms"}` | `{"from_ms","to_ms","footprint_delta","growers":[change]}` from the row at or before `from_ms` to the newest; `UNAVAILABLE` outside the retained history |
| `host.memory.postmortem` | `{"pid"?}` | the newest dead process's evidence in this process's report directory: `{"pid","file","diff","reports","report"}` |

A `change` is `{"category","name","unit","before","after","delta"}`.
`category` is `tag`, `stack`, `gpu` or `probe`; `unit` is `bytes` or `count`.
Growers rank by `delta` within each unit, bytes first. Categories overlap, so
never add them together. A postmortem prefers the newest dead flight recorder,
then watchdog and lab reports (`mem-<pid>-*.json`, `lab-host.json`).

Offline, with no game running:

```sh
h2 mem postmortem                 # newest dead process in logs/mem/
h2 mem postmortem --pid 4242
h2 mem postmortem ~/saved/mem-4242.ndjson
h2 agent postmortem --pid 4242    # the same, through the agent CLI
```

`h2 lab mem <id>` fetches a lab job's `mem/` directory and prints the largest
growers ([lab.md](lab.md#memory-limits-and-reports)). The hunt: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Finding a leak after the
fact".

### `host.audit`

Set `H2_HOST_AUDIT=/path/to/audit.json` before boot to register it
(`crates/extensions/game/src/host_audit.rs`). `h2 agent hostaudit` takes no
parameters, writes a snapshot to that path and returns the path, ticks,
observed and registered function counts, tag range count and inclusive CPU
timing. It also flushes any enabled map read recording; an orderly exit writes
the final snapshot, so call it before killing a dedicated process. The
collector changes dispatch costs, so size with `host.metrics`. Report fields
and corpus: [sim.md](sim.md#host-audit-corpus).

### `host.lobby`

A System Link light host started with `H2_HOST_LOBBY=1` registers these four
game methods (`crates/extensions/game/src/host_lobby.rs`, types
`crates/extensions/agent-proto/src/host_lobby.rs`). Each result also carries `at`.

| Method | Parameters | Result |
|---|---|---|
| `host.lobby.open` | `{"map":"<short name>","variant":"<name>","max_players":8,"live":false}` | System Link: an empty object. Live: the advertised `session_id` |
| `host.lobby.state` | `{}` | `{"phase":"lobby","players":[{"slot":3,"peer":1,"name":"<name>"}],"ready":true}` |
| `host.lobby.start` | `{}` | an empty object when retail accepts the start request; observe `state` for load and play |
| `host.lobby.close` | `{}` | an empty object once the network lobby has closed and the host is back at the menu |

| Param | Rule |
|---|---|
| `map` | Short name, scenario path or English display name |
| `variant` | Optional, default Slayer: disc variant, signed saved variant present at boot, stock game type or built-in Live playlist key, case-insensitively; a name matching two rulesets is `INVALID_PARAMS` |
| `max_players` | Integer 1 to 16 |
| `live` | Required: System Link or a Live custom game (needs a service identity that can publish sessions) |
| `synchronous` | With `"live":false` and no `variant`: a variant-free System Link probe |

Open waits for retail to publish the lobby parameters. Repeating the same open
in pregame returns the existing lobby; close it before changing parameters.
The phases are `menu`, `lobby`, `loading`, `playing` and `postgame`. Slots are
retail membership slots and can have gaps; split-screen players on a remote
console share a peer index. An empty joinable lobby reports `"players":[]` and
`"ready":false`. Readiness needs a remote player, party leadership, the
requested map and limit, loaded maps on every peer, and retail's
ready-to-start result. Any connection may read state; mutations respect
`session.claim`, and a concurrent mutation is `BUSY`.
[Playerless lobby and the two-console check](../live/dedicated-host.md#playerless-lobby).

## `ui`: what the menus show

`ui.screen` needs `H2_UI_WATCH=1`. Other UI methods read guest widgets or the
game's UI state without it. All are read-only except `ui.press`.

- **`ui.screen`** (`h2 agent ui`): the menu screen player one's input goes to,
  read each UI update, including during a campaign pause (`game::ui_watch`):
  `{screen, window, slot, class?, live_menu_row, mode, dialog, age_ms,
  layout_findings, content_findings}`, or `UNAVAILABLE` before the first read.
  `class` names a Rust list screen whose numeric ID identifies its retail
  skin. The main menu keeps Campaign, Xbox Live, Split Screen and System Link
  at retail rows 0 to 3, with Theater after System Link. `dialog` is
  `{id, name, window, slot}` for the dialog up, or `null`; `name` comes from
  `boot::ui_watch::dialog_name` (`dialog_live_signin`, `confirmation_dialog`,
  `error_dialog_ok`, `party_leader_picker`, `clanker_player_options`,
  `kit.picker`, `kit.picker.game`, ...) and is `null` for an unnamed id.
  Dialogs read as `"screen": null`. `layout_findings` lists text and rows
  outside the title-safe area or their container: `screen`, `widget`, `rect`
  ([top, left, bottom, right]), `text`, and `issue.kind` (`unsafe`,
  `outside_panel`, `outside_container`). `content_findings` lists broken
  strings, dead controls and missing content: `screen` and `issue: {kind, id,
  detail}`, plus `widget` and `text` for widgets, or `source: "page_model"`
  and `screen_instance` for Rust page models. The read activates nothing; the
  [UI builder](../game/ui-builder.md#content-findings) checks each state.
  Pause Settings walkthrough: [settings-menu](../game/settings-menu.md#to-check-in-game);
  more: see the [detail note](notes/2026-11-26-agent-api-detail.md), "`ui`: what the menus show".
- **`ui.stack`** (`h2 agent ui.stack`, no parameters): a tick-boundary read of
  the occupied channels, their active, pending and outgoing screen instances,
  and the game-shell channel's saved screen IDs (`history`, nearest parent
  first). Each screen has `id`, `instance` (a guest address valid for its
  lifetime) and `parent`. Map picker stages have `mode` `folders`, `maps` or
  `pack`, with the open one in `folder`; `focused_row` is zero-based.
  `variant_category` and `class` name a custom variant group and a kit class,
  else null. `input` uses the same target as `ui.audit`; `back` gives the
  instance that closes and its parent, with action `close`, `reopen` or `wait`,
  or null when Back is custom. No UI watch needed.
- **`ui.campaign`** (`h2 agent ui.campaign`, no parameters): a tick-boundary
  read of the campaign menu without UI watch
  (`crates/extensions/game/src/campaign_menu.rs`). With no installed Halo 1
  campaign it returns `{"installed":false}`. Otherwise it reports `page`
  (`picker`, `options`, `levels`, `difficulty`, `dialog` or `other`), `game`,
  `rows`, `focused_row` and `focused`; unknown values are null. On the picker
  `card` has `description` and `picture_datum`; on Campaign Options the action
  keys are `resume`, `new_game`, `select_level` and `co_op`. `pick` records the
  latest press (`campaign`, and `request` with `mask`, `window`, `slot`,
  `loader`) and `list` gives guest addresses (`screen`, `handler`,
  `row_array`, `children`, `row_count`, `press_record`); both are null on
  other pages. `h2 agent press a` on the main menu's Campaign row opens the
  picker (native dialog request `0x230888`, solo branch `0x2b16ad`).
- **`ui.browser.filters`** (`h2 agent ui.browser.filters`, no parameters):
  reads the active Custom Game Browser preferences, even when the browser is
  closed: `{"filters":{...}}` with the field names of the
  [Live session query](../live/live-api.md#browser-filters). Unset fields are
  omitted; the defaults give `{"filters":{}}`.
- **`ui.lobby_options`** (`h2 agent ui.lobby_options`, no parameters):
  `{"lines":[...]}`, the custom game panel's visible Quick Options lines in
  display order, up to six, read at a game boundary
  (`crates/extensions/game/src/lobby_options.rs`). `UNAVAILABLE` when the pane
  is unavailable.
- **`host.peer_rtts`** (`h2 agent host.peer_rtts '{}'`): the simulation
  membership's reliable-channel RTT at a game boundary. `players` has the same
  slot, peer, name, bars and nullable RTT as `ui.lobby.players`; local players,
  phantoms and unsampled routes are null. `UNAVAILABLE` without a simulation
  membership.
- **`ui.lobby.players`** (`h2 agent ui.lobby.players`, no parameters): the
  pregame player list at a game boundary, without UI watch
  (`crates/extensions/game/src/lobby_quality.rs`). Each entry has `slot`,
  `peer`, `name`, `bars` (1 to 6) and `rtt_ms` (null with no sample).
  Split-screen players share a console's peer and bars; local players and
  phantoms have six bars and null RTT; unsampled remote peers have one. Slot
  order; empty outside pregame. Example:
  `{"players":[{"slot":1,"peer":1,"name":"Clanker","bars":6,"rtt_ms":null}]}`.
- **`ui.lobby.tabs`** (`h2 agent ui.lobby.tabs`, no parameters): the pregame
  lobby's modes ([party-lobby.md](../game/party-lobby.md)), read at a game
  boundary (`crates/extensions/game/src/lobby_tabs/header.rs`):
  `{place, leader, selected, header, tabs, clanker, notice}`.

  | Field | Meaning |
  |---|---|
  | `place` | `split_screen`, `system_link` or `xbox_live` |
  | `leader` | Whether this console leads the party |
  | `selected`, `header` | The current mode and its header text (`CUSTOM GAMES`, `FORGE`, ...) |
  | `tabs` | `{tab, label, state, reason}`; `tab` is `multiplayer`, `custom_games`, `forge` or `co_op`; `state` is `selected`, `on` or `off`; `reason` is the status sentence of a mode that cannot run, else null |
  | `clanker` | In MULTIPLAYER, whether the playlist is a Clanker playlist; null in other modes |
  | `notice` | The status-line reason shown for four seconds after picking an off mode, else null |

  `UNAVAILABLE` when the pregame lobby is not on top or unreadable.

Screens moved to Rust (Video, skulls, account, co-op, sign-in;
[feature-engine.md](../engine/feature-engine.md)) share one list-screen
implementation (`crates/extensions/game/src/list_class.rs`).

### Menu inspection and navigation

`ui.audit {}` inspects the instantiated widgets at one game boundary. The
result has `screen` (the underlying menu), `dialogs` (the active dialog on each
occupied channel), `active` (the surface that accepts player one's input),
`target` (its opaque token) and `focused_row`; ambiguous input leaves them
null. Each surface has `id`, `window`, `slot`, `pane` and kit class `name`.
`panes[].labels` and `panes[].lists[].rows` give text read after retail's
localized lookup; `navigation` marks verified inspection actions; `item_count`
includes off-screen items. `issues` names affected surfaces, panes and rows,
and `transition` reports the compositor (`stuck_blur`, `stuck_transition`,
`stuck_fade`, `requests_backdrop`). Source:
[`ui_audit.rs`](../../crates/extensions/game/src/ui_audit.rs),
[`ui_transition.rs`](../../crates/extensions/game/src/ui_transition.rs),
[User-interface tags](../formats/ui.md). Field detail: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Menu inspection and navigation".

`ui.press` accepts `{target, action, row?, phase?}`; copy the token from the
latest audit.

| Action | Effect |
|---|---|
| `up`, `down`, `left`, `right` | Move focus |
| `choose` (A), `back` (B), `friends` (Y) | Press the button |
| `open_live_menu` | A on the Live players screen in mode 3 |
| `change_mode` | X in the pregame lobby: the GAME MODE picker, for the party leader; `GONE` on any other screen ([party-lobby.md](../game/party-lobby.md#changing-mode)) |

A changed owner, row or phase invalidates the press, and the XAPI tap checks
the target again when the game reads the pad. Success is `{delivered:true,
target, action, row, player, at}`; an unread gesture expires. Audit again after
the transition settles. `session.methods` lists `ui.press` when the process
has an XAPI input tap.

### Crawling the menus

On a running agent-enabled game with a signed-in profile and a recent Clanker
at the main menu, set `H2_AGENT` to its socket path:

```sh
h2 agent --socket "$H2_AGENT" ui-crawl --depth 6 --budget-ms 120000 --require-clanker
```

Omit `--socket` for the normal endpoint. The crawl explores known navigation
actions and backs out before trying siblings; it skips game starts, quit,
unverified actions and setting or account changes. `--from-current` starts at
the open surface; depth zero audits only it; defaults are depth 6 and 2
minutes. `--require-clanker` fails the run on missing seed history or an
unreached card. The JSON verdict lists visited paths, findings, skipped
actions, Clanker cards and navigation errors; findings or a failed traversal
exit nonzero. Save the report outside the repository.
[CLI implementation](../../crates/apps/cli/src/agent_ui_crawl.rs)

### `ui.attract`

`h2 agent ui.attract` takes no parameters; boot registers it when the Bink HLE
selects playback outside a film recording or replay. It reads the menu's UI
clock, idle time, music gate and movie flags at a game boundary (`timer`).
`state` says whether the menu is idle, fading, playing the attract movie or
keeping its music; `asset` reports preparation or unavailability;
`blocked_reason` explains kept music. `last_attempt` gives UI time, idle time,
present and an `outcome`; `returned_ui_ms` is set when the movie closes.
[Attract timer](../audio/bink-hle.md#attract-timer)

## `audio`: voice routing

`h2 agent audio.listeners` takes no parameters. At a game boundary it reads the
active world-sound listeners. Each `listeners` row has the local `user` index,
salted `player_datum` (null for an unmapped slot), `phantom` (null without a
resolved player) and world-space `position` `[x, y, z]`. `UNAVAILABLE` if the
listener state is uninitialized or unreadable.

`h2 agent audio.voices` takes no parameters and is registered with DirectSound
(`crates/extensions/game/src/dsound_glue.rs`). The result has `distance_reverb`,
`reverb_active` and `voices`. Each voice has a mixer `id`, `kind`,
`volume_mb` and `distance_units` (null if unpositioned); `sends_to` lists the
playing mix-in ids its bins reach; a submix's `input_bin` is a host bin
(`0x100` and up; the guest sees `0x1f`;
[dsound.md](../audio/dsound.md#positioned-sound)).

| Field | Meaning |
|---|---|
| `spatial` | Null for a 2D voice; else `position`, `relative`, `min_distance`, `max_distance`, `rolloff`, `distance_gain`, `cone_gain`, `gain`, `pan`, `front`, `pitch` |
| `applied_gain` | `direct_gain` times `spatial.gain`: the direct path's level before bin volumes and pan |
| `direct_gain`, `room_gain` | Linear low-frequency gains including voice volume and headroom |
| `direct_hf_db`, `room_hf_db` | Extra loss at `reference_hz` |
| `bins` | Ordered `{bin, volume_mb, gain}` routing entries; null for plain stereo |

Halo's submixes have `FLT_MAX` distances and rolloff 0; the game's distance
attenuation is the gain on the stream's bin into the submix. More: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "`audio`: voice routing".

### `audio.loops`

`h2 agent audio.loops` takes no parameters. With DirectSound installed it
reports the guest's active looping sounds and the host's playout health.

| Field | Meaning |
|---|---|
| `loops` | Rows: `loop_datum`, `tag_datum`, `tag_name`, `track`, `segment` (`in`, `loop`, `out`, `alt`, `alt_in`, `alt_transition_out`, `alt_out`, `unknown`), `sound_instance`, `sound_datum`, `pitch_range`, `permutation`, `chunk`, `play_packet`, `play_position_frames` (null with no queued packet) |
| `transitions` | Last 64 start, stop, segment and permutation changes; null `from` is a start, null `to` a stop; stamp has `tick`, `present`, `logical_100ns`, `elapsed_us` |
| `records_transitions` | Whether the observer runs |
| `menu` | In the main menu: `ui_ms`, `idle_ms`, music gate, requested and applied music state, attract timeout, stop threshold, movie active |
| `menu_changes` | Last 64 gate, music, movie or timer-reset changes |
| `output` | Cumulative sink underruns and silent frames, playout starves, dropped frames, mixer stream starves, with last-change times on the `elapsed_us` clock; optional WMA counters; playout fill and target |

History restarts with a new map or a rewound guest clock. Compare `present`
numbers in `transitions` and `menu_changes` with the output counters to tell a
selection change from an idle restart or a device gap. See
[the attract timer](../audio/bink-hle.md#attract-timer) and the
[detail note](notes/2026-11-26-agent-api-detail.md), "`audio.loops`".

### `audio.meter`

The level of the host mixer's output bus, before the device, so a run with no
audio device meters the same samples
([audio.md](../audio/audio.md#metering-the-mixer)). A game method in
`crates/extensions/game/src/mixer_meter.rs`; types in
`crates/extensions/agent-proto/src/audio.rs`.

```sh
h2 agent audio.meter '{"window_ms": 1000}'
h2 agent audio.meter '{"play": "pogo/spring"}'
```

| Params | Result |
|---|---|
| `{window_ms?}`: 50 to 5000, default 1000 | `{window_ms, frames, rate, channels, peak_dbfs, rms_dbfs, lufs, max_momentary_lufs, clipped}` |
| `{play}`: a host clip | the same, with `play: {clip, duration_ms}` |

- **A window.** Meters the latest `window_ms`; capture starts with the first
  request and keeps 10 s.
- **`play`.** Plays `pogo/compress`, `pogo/spring`, `pogo/settle` or
  `announcer/<medal id>` and answers when it has played, metering to its end
  plus 20 ms.
- **Levels.** Peak and RMS in dBFS, integrated and loudest-400 ms LUFS
  (ITU-R BS.1770-4), to 0.1 dB; silence reads -120; `clipped` counts samples at
  full scale.
- **Errors.** `UNAVAILABLE` on a clock-only run, a Git LFS pointer clip or a
  clip over 5 s; `INVALID_PARAMS` for bad params and unknown clips. A paused
  game does not answer until it runs.

### `audio.play`

Starts a sound tag at a player's unit so it can be heard and read back
(`audio.voices`, `audio.meter`). A game method of the sim provider
(`crates/extensions/agent/src/sim/mod.rs`, `Sim::audio_play`; types in
`crates/extensions/agent-proto/src/sim/mod.rs`).

```sh
h2 agent audio.play '{"sound": "sound\\pogo\\spring"}'
```

| Params | Result |
|---|---|
| `{sound, player?}`: a `snd!` tag by datum or name (as `edit.tags` lists them); `player.<n>` or `sim.<n>`, default `player.0` | `{player, sound: {tag, group, name}, instance}` |

- **The call.** `SOUND_START_AT_MARKER` (`0x1891d0`) with the unit's head
  marker, at the start of the next tick like `sim.give`
  ([`sim`](#sim-simulated-players-and-direct-state)). Presentation only, so a
  co-op session may ask.
- **Errors.** Not a `snd!` or not found: `INVALID_PARAMS`; player with no unit:
  `GONE`; no instance started: `UNAVAILABLE`. A paused game answers when it runs.

### `world.object.animate`

Plays a named animation of an object's own graph. A game method of the sim
provider (`crates/extensions/agent/src/sim/mod.rs`, `Sim::object_animate`; the
write in `crates/extensions/agent/src/probe/animation.rs`, `start`; types in
`crates/extensions/agent-proto/src/sim/mod.rs`).

```sh
h2 agent world.object.animate '{"id": "scenery.12340000", "animation": "idle"}'
```

| Params | Result |
|---|---|
| `{id, animation}`: an object id as `world.object` spells it (not a player); the animation's name in the graph the object's manager holds | `{object, graph, animation, index, frames, frame, session}` |

- **The call.** Points the object's first animation channel at the graph's own
  `jmad` entry (frame 0, rate 1) at the start of the next tick, like
  `sim.give` ([`sim`](#sim-simulated-players-and-direct-state)); inherited
  animations are not found.
- **Host only.** A co-op (synchronous) session refuses it, as `sim.spawn`.
- **Errors.** Bad or player id, unknown string ID, or no such animation:
  `INVALID_PARAMS`; a stale id: `GONE`; no animation manager or graph:
  `UNAVAILABLE`.
- **Not yet confirmed in a running game.** Use it for scenery, machines,
  crates, creatures and vehicles, not bipeds; check with `frame` and a later
  `anim`/`world.object` read. Channel layout: see the
  [detail note](notes/2026-11-26-agent-api-detail.md), "`world.object.animate`".

### `recording.play`

Plays one of the loaded scenario's recorded animations on a player's unit, as
the script call `recording_play` does. The editor's Recordings section calls it
([rich-editors.md](../editor/rich-editors.md), "Recordings"). A game method of
the sim provider (`crates/extensions/agent/src/sim/mod.rs`,
`Sim::recording_play`, the block read in `sim/recording.rs`; types in
`crates/extensions/agent-proto/src/sim/mod.rs`).

```sh
h2 agent recording.play '{"recording": "arrive", "mode": "hover"}'
```

| Params | Result |
|---|---|
| `{recording, player?, mode?}`: a name in the scenario's recorded animations (without case); `player.<n>` or `sim.<n>`, default `player.0`; `play` (default), `delete` or `hover` | `{player, recording, index, unit}` |

- **The call.** The recording player (`0x1fb360`) at the start of the next tick
  like `sim.give` ([`sim`](#sim-simulated-players-and-direct-state)); flags 0
  for `play`, 8 for `delete`, `0x10` for `hover`. The unit moves, so a co-op
  session gets `INVALID_REQUEST`.
- **Errors.** Unknown name or mode: `INVALID_PARAMS` (the first lists the
  names); player with no unit: `GONE`; nothing started: `UNAVAILABLE`. Disc
  multiplayer maps have no recordings. A paused game answers when it runs.

## HaloScript state

Three game methods with an empty parameter object
(`crates/extensions/agent/src/hs_state.rs`) that sample guest state at a tick
boundary. Each answers `UNAVAILABLE` if its backing state is unreadable or
uninitialized.

- `hs.threads` returns `threads`, the live slots: `slot`, numeric `category`,
  signed `wake` tick, and for scenario script threads `script` and `kind`
  (null for transient threads).
- `hs.globals` returns `globals` in scenario declaration order: `name`,
  `type`, `type_index`, `value.bits`, and `boolean`, `real` or signed `number`
  for scalars.
- `hs.player_actions` returns three unsigned masks from local player control:
  `latched`, `capture` and `until_release`; bit 0 is the action/reload button.
  It takes no player index.

### `hs.parity`

`h2 agent hs.parity` takes no parameters. It returns the switch name
(`impl.sim.native.hs`) and a `checks` object keyed by VM operation, each with
`matched`, `mismatched` and `declined` counts; `first` holds the first
mismatch description or `null`. The counters cover the process lifetime and
can be read before a map loads. [HaloScript VM](../engine/hs_vm.md)

### `hs.eval`

`hs.eval {expression, type?}` compiles one HaloScript expression against
the loaded map's scripts and answers its value
(`crates/extensions/agent/src/hs_eval.rs`). It runs at the tick boundary
the request is served at, over a snapshot, on `hs::vm`, the native evaluator
([hs_vm.md](../engine/hs_vm.md)). Nothing is written back to the game, so a
`set` of a global changes the snapshot only. Builtins that need the game's
objects, AI or sound are refused by name, and so is an expression that
sleeps. Design: see the [detail note](notes/2026-11-26-agent-api-detail.md),
"`hs.eval`".

- **`type`** names the result's type (`real`, `short`, `boolean`, ...).
  Without it the expression's own is used where it is fixed: a call of a
  builtin or script that returns a plain value, or a bare global's name.
  A `begin`, an `if` or a literal needs it.
- **Result.** `{type, type_index, value, bits}`: `value` is a boolean, a
  number, the string's text, or the raw bits of a handle or index, and
  `null` for void.
- **Errors.** Problems in the text are `INVALID_PARAMS` with the compiler's
  sentence and `data.problems: [{line, column, code, text}]`. An unknown
  `type` is `INVALID_PARAMS`. The evaluator's refusal (an engine builtin,
  a sleeping expression, a thread that faults) and unreadable or
  uninitialized scripts and globals are `UNAVAILABLE`.

```sh
h2 agent hs.eval '{"expression":"my_global"}'
h2 agent hs.eval '{"expression":"(begin (set counter 3) counter)","type":"short"}'
```

## `input`: act like a player

Input goes through the normal pad path, so the game cannot tell an agent
from a person. The agent's tap is an `xapi::tap::InputTap`
(`crates/extensions/agent/src/tap.rs`, the layer `H2_INPUT_RECORD` and
`H2_INPUT_REPLAY` use) that sees each `XInputGetState` result before the
guest gets it. Every host that runs the game through boot's XAPI has it:
`h2 serve`, `h2 play` and the Xbox app. A replay runs first, agent steps go
on top, and a recording runs last. Without XAPI input there is no tap and
the input methods are not listed. Tap order: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "`input`: act like a
player".

```sh
h2 agent press a                                # A for 150 ms
h2 agent pad '{"player":1,"at_tick":900,"steps":[{"buttons":["rt"],"ticks":30}]}'
h2 agent pad '{"steps":[{"left":[0,1],"ticks":90},{"buttons":["a"],"reads":2}]}'
h2 agent padstate                               # every port, open ports, mode
h2 agent record courtyard                       # ...play or pad...
h2 agent stop                                   # writes inputs/courtyard.input
h2 agent replay courtyard
printf 'claim\npress start\nwait {"ticks":30}\ncapture 300\n' | h2 agent -
```

- **`input.pad {player, at_tick, steps, append}`.** Each step is
  `{buttons, left, right, lt, rt, ms, ticks, reads}`. `player` is the
  controller port (0 to 3, default 0); each port has a queue of steps. A
  step holds for `ms` of wall time, `ticks` of game time, or both, and until
  the game has read the pad `reads` times (default once); with neither `ms`
  nor `ticks` it holds until replaced. `at_tick` (a tick, or `"now"`, the
  default) holds the first step back until the game's tick reaches it.
  `append` adds to the queue, otherwise it replaces it. A step merges over
  the read (buttons are or'd, a stick or trigger the step moves wins).
  Buttons are `a b x y black white lt rt` (analog, fully pressed),
  `up down left right start back ls rs`; sticks `[x, y]` from -1 to 1 (up is
  +y); triggers 0 to 1. Queues outlive the connection and are emptied when
  the map changes. The reply says `open: false` when the game has not opened
  that controller: the steps are kept, but nothing reads them.
- **`input.release {player}`** empties one port's queue, or all of them.
- **`input.state`** gives `pad` (player one), `pads` (every port: `active`,
  `queued`, `done`, what is held), `open` ports, `tick`, and the recording
  or replay. It needs no claim while nobody holds control.
- **`input.record {name, replace}`** logs every read (status, handle, the
  22 bytes, the game tick) to `inputs/<name>.input` under the
  [agent output root](switches.md#tools), in the `xapi::tap` text format.
  The log is saved at `input.stop` or when the game exits. Names use 1 to 64
  letters, digits, `.`, `_` or `-` and may not start with a dot. An existing
  log is kept unless `replace` is true. A [lab job](lab.md#runs) stores
  these logs in its run directory.
- **`input.list`** returns `{names}`, the sorted names of saved input logs
  without their `.input` suffix.
- **`input.log.read {name, offset?, limit?}`** returns `{text, next}` from
  a saved log. Offset defaults to 0; pass `next` as the next offset and stop
  at `null`. The limit is in bytes (default 16384, maximum 65536). Both file
  methods are read-only and need no claim.
- **`input.replay {name}`** answers reads from a log read for read.
  `input.state` and `input.stop` give its counts: `reads` in the log,
  `replayed`, `matched` (same handle as recorded), `handle_mismatches` (the
  runs have parted) and `past_end` (left live). One recording or replay at a
  time. A replay reproduces a run only from the same starting state: the
  same boot, or the same [snapshot](#state-machine-snapshots). Recording and
  playback exclude map reloads, and a pending reload prevents starting
  either.

#### Routes

`input.route {player?, legs, timeout_ticks?}` starts a sequence on a local
controller port (0 to 3, default 0). The timeout covers the whole route and
defaults to 1800 simulation ticks. The route uses the Clankers' path search
and movement and feeds ordinary pad input through the same tap as
`input.pad`. It uses the default stick and button layout and requires a
living local player on foot. It loads the active BSP's bot nav mesh from the
game's `bots` data directory; if the mesh is missing or stale, run
`h2 bots nav <map>`. See [Clankers' navigation](../game/bots.md#32-navigation).

A route controls its port's full pad report while active. Starting a route
on a port with a route or queued pad steps returns `BUSY`, as does
`input.pad` on a port running a route.

| Leg | What the controller does |
|---|---|
| `{"move_to":[x,y,z],"crouch":false,"tolerance":0.1,"speed":1}` | Follow the bot nav mesh with the left stick and turn with the right stick. Stop within `tolerance` of the goal (default 0.1 map units). `speed` limits stick magnitude from 0 to 1 (default 1). `crouch: false` keeps the crouch button released. |
| `{"face":yaw_deg}` or `{"face":[x,y]}` | Turn in place to a world yaw in degrees from +x toward +y, or toward a world point. |
| `{"jump":{"from":[x,y,z],"land":[x,y,z]}}` | Use the bot jump arc to plan a run-up, press A at takeoff and steer toward the landing. The leg succeeds within 0.15 map units of `land` and reports the observed points and speeds. |
| `{"crouch_into":[x,y,z],"hold_ticks":N,"exit_face":yaw_deg}` | Hold the left-stick click and creep toward the point until movement stalls. Release the click, keeping pressure into the ceiling for `hold_ticks`, then turn toward `exit_face` with the stick forward until the player stands. |
| `{"wait":ticks}` | Leave the pad idle for the given number of game ticks. |

`input.route.status {player?}` returns `player`, `state` (`idle`,
`running`, `completed`, `failed` or `stopped`), elapsed `ticks`, the
current zero-based `leg` (or null) and `legs`. Each leg reports
`{ok, ticks, reason}`; `ok` is null while the leg is pending or running.
After a failure or stop, legs never started get `ok: false` and
`reason: "not_run"`. A jump leg includes its available `takeoff`, `apex` and
`landing` samples in `jump`, each with `position`, `velocity` and `speed` in
map units per second.

`input.route.stop {player?}` stops the route and releases its port;
`input.release` does the same on the ports it releases. The final status
stays until a new route starts on that port. The `route` short form takes
the array of legs; use the full method to pick the port or set the timeout.

```sh
h2 agent route '[{"move_to":[1,2,3],"crouch":false},{"wait":30}]'
h2 agent route-status
h2 agent input.route.stop
```

Worked route: see the [detail note](notes/2026-11-26-agent-api-detail.md),
"Routes" (Ascension superbounce setup), and
[Superbounce](../game/superbounce.md).

#### `input.host`: the host controllers

`input.host` (`h2 agent hostpads`, read-only, no params) shows the
controllers the host's input layer has, before the game sees anything. It
answers from the last input sync (`syncs` counts them) and is added wherever
the game has XAPI input (`crates/extensions/game/src/debug_menu_glue.rs`,
`crates/extensions/agent/src/hostpads.rs`).

| Field | Content |
|---|---|
| `devices` | each controller as its backend reads it (`input::HostBackend::devices`): `backend` (`game-controller`), `id`, `name`, `profile` (`dualsense`, `dualshock4`, `extended`, `micro`), `category`, and the state |
| `pads` | each pad the input manager polled: `backend`, `key`, `name`, `physical`, `port` (0 to 3, or null) and the state |
| state (in both) | `touchpad` (null, else `click`, `primary`, `secondary` as `[x, y]` or null), `buttons`, `left_stick`, `right_stick`, `triggers` |
| `gate` | null with no debug menu, else `open`, `syncs`, and per pad `key`, `touchpad`, `click`, `chord`, `chord_pending`, `opened_by_press`, `latched` ([debug-menu.md](debug-menu.md)) |
| `notes` | where a touchpad goes missing, in words |

Button names, the keyboard merge and the `notes` cases: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "`input.host`".

```sh
H2_AGENT=/tmp/h2-play.sock h2 play           # then, holding the touchpad down:
h2 agent --socket /tmp/h2-play.sock hostpads
```

#### `input.lightbar`: PlayStation controller colours

The lightbar methods are available in player runs with XAPI input and work
without a controller connected (`crates/extensions/agent/src/lightbar.rs`).
`input.lightbar` is read-only with no params. It returns `enabled` and four
`ports`, each with a zero-based `port`, the last sampled `game_rgb`, the
temporary `override_rgb` and the desired `rgb`. Each colour is three
integers from 0 to 255, or null. The desired colour is the override if set,
otherwise the game colour; with the Lightbar setting Off it is null.

`input.lightbar.set {port, rgb}` sets an override for one port and returns
the same state. A null `rgb` clears it. The override lasts until cleared or
the run ends, and follows the usual control ownership rules for actions.

```sh
h2 agent input.lightbar
h2 agent input.lightbar.set '{"port":0,"rgb":[32,128,224]}'
h2 agent input.lightbar.set '{"port":0,"rgb":null}'
```

See [controller output](../audio/playstation-audio.md#player-colour-lightbars)
for the guest palettes and macOS lightbar backend.

## `state`: machine snapshots

`state.save`, `load`, `list`, `drop` and `fork` (types in
`crates/extensions/agent-proto/src/state.rs`, the executor's part in
`crates/extensions/agent/src/exec/snap.rs`, the machine in
`crates/extensions/game/src/snapshot_glue.rs`). A snapshot is the whole
machine at one tick boundary, named. Contents, load and file locations:
[snapshots.md](snapshots.md). Snapshots stay in the cache directory, one
directory per build, and never enter the repository. The game answers
`UNAVAILABLE` when it has no snapshot support installed.

| Method | Params | Does |
|---|---|---|
| `state.save` | `{name, replace?, cow?, max_ticks?}` | guest RAM, the kernel and its threads, D3D, audio, XAPI with the input cursor, every feature's section, and a RAM hash, at the first quiescent slice boundary after a tick. `cow: true` also writes a sparse raw RAM image `<name>.ram` beside it |
| `state.load` | `{name, cow?, max_ticks?}` | back to exactly that tick; the RAM hash (`lockstep::hash_guest`) is checked against the saved one, naming the MiBs that differ. `cow: true` maps that RAM image private copy-on-write; it is refused, naming the file, when the save wrote no image |
| `state.list`, `state.drop {name}` | | this build's snapshots, answered at once |
| `state.fork` | `{from, name, replace?}` | load `from` and save it again as `name` |

Names are `a-z 0-9 . _ -`, 1 to 64, not starting with a dot. A save or load
waits for the next tick, then retries at each tick while something is live,
for `max_ticks` (default 90, at most 1800), then answers `BUSY` with the
blockers in `data.blockers`. One sent while the game is paused runs at the
end of the held tick's slice. Calls other than `state.list` need control
when another connection holds it. A snapshot is refused while `agent-calls`
writes are queued, and while an editor-bound DVD file or an unlinked
placement preview exists (this also blocks snapshot-based rewind and film
keyframes).

```sh
h2 agent state save NAME [replace]
h2 agent state load NAME
h2 agent state list
h2 agent state drop NAME
h2 agent state fork FROM NAME [replace]
```

## `edit` and hot `render`: live tag and shader changes

Host methods answered where the game's host registers them
(`crates/extensions/agent/src/hot.rs`, names in
`crates/extensions/agent-proto/src/hot.rs`). The design, the override
directory and the rules are in [hot-iteration.md](../graphics/hot-iteration.md).
The override directory is `$H2_RENDER_OVERRIDES`, else `render-overrides` in
the cache directory.

| Method | Access | Params | Does |
|---|---|---|---|
| `edit.map` | read | `{known_epoch?}` | `{at,name,file,checksum,scenario,epoch,session,bsp}` for the loaded map. File and checksum can be null. `bsp` is the loaded structure BSP's index (`-1` before one loads, null when unreadable). Session includes `writes` and the refusal reason in `why`. |
| `edit.tags` | read | `{group?, name?, limit?}` | the loaded map's tags: `tag`, `group`, `name`, `address`, `size`, `defined` |
| `edit.get` | read | `{tag, group?, path?, epoch?}` | a field by path (`postprocess[0].colors[1].color`), or an element's fields; `tag` is a datum or a name |
| `edit.read` | read | `{epoch,at,len}` | `len` bytes (1 to 524288) at numeric address `at` inside one loaded tag. Returns `{at,holds_bytes}` as lowercase hex. |
| `edit.batch` | act | `{writes,label?,epoch?,confirm_peers?}` | `{id,applied,changes}` at one boundary. One journal entry with its ID, optional label and changed `fields`. |
| `edit.set` | act | `{tag, group?, path, value, epoch?, confirm_peers?}` | writes the field now, journalled; returns the change |
| `edit.changes` | read | none | the current epoch's journal, with `was_bytes`, `new_bytes` and current `holds` as hex |
| `edit.revert` | act | `{id?,epoch?,confirm_peers?}` | restores a change or batch, or the whole journal in reverse order. `reverted[]` gives `id,at,tag,path,value,holds_bytes`. `dropped[]` gives `id,at,tag,path,why,holds_bytes` for each field left alone. |
| `render.overrides` | read | none | the override directory, what is loaded and how each fared |
| `render.reload` | act | `{dir?}` | reads the directory again (`dir` moves it) |
| `render.programs` | read | `{vertex?, limit?}` | the translated programs, by id |
| `render.dump` | act | `{program?, to?}` | writes translated WGSL, the start of an override |
| `render.param` | act | none, `{name}`, `{name, value}` | lists, reads, sets (`value: null` resets) a parameter |
| `render.toggle` | act | none, `{name, on}` | lists, sets (`on: null` resets) a toggle |

All three edit writes check the session at the game boundary. Co-op,
clients, matchmade or other-hosted Live games, film playback and unknown
sessions refuse writes (a multiplayer host with an unknown peer count
reports `writes:false`). A System Link or Live custom game host with remote
peers requires `confirm_peers` equal to the current total peer count. An
epoch pins a write or field read to the map `edit.map` reported; a client
passes `known_epoch:null` to get the tag datums in `loaded`. A BSP switch
revalidates the journal; a map load or state restore clears it. In a
reverse revert, a field the game changed protects its whole byte range and
older overlapping fields appear in `dropped`.

`edit.map.file` follows the source path recorded by the active cache slot
(map identity, DVD slots and editor copies): see the
[detail note](notes/2026-11-26-agent-api-detail.md), "`edit` and hot
`render`". After the first successful file binding, external Live sign-in is
refused until restart.

Each batch write takes `{tag,group?,path,value,was?}` with typed values, or
`{at,value,was?}` with hex bytes. An address is a u32 or a `0x` string. The
whole range must fit one loaded tag, or one block moved into the edit arena
([Block moves](#block-moves)). Overlapping writes are refused. A write the
game already holds succeeds. If it holds neither value, error 8
(`MISMATCH`) returns `data.fields` with `at`, `tag`, `path`, `was`, `holds`
and `holds_bytes`, and the batch and journal stay untouched.

```sh
h2 agent edit tags weap
h2 agent edit get <tag name> <path>
h2 agent render toggle <name> off
```

Tag names come from the map file the game loaded ([`world`](#world-what-is-in-the-level)).

### Raw edits

Section vertices and indices, bitmap pixels and unloaded structure BSPs
live in the map files' raw blocks. These methods write them live through the
raw overlay that the guest's page-ins and the native loaders read. Design:
[realtime-edits.md](../editor/realtime-edits.md). Code: `crates/extensions/agent/src/hot.rs`,
`crates/extensions/hot-tags/src/raw.rs`,
`crates/runtime/vfs/src/map_override/overlay.rs`,
`crates/extensions/game/src/hot_glue.rs`.

| Method | Access | Params | Result |
|---|---|---|---|
| `edit.batch` | act | adds `raw: [{owner, offset, bytes, was?}]` and `evict: [owner]`; `writes` may be left out when either is given | adds `raw: [{owner, offset, resident, refresh}]` and `evicted: [owner]` |
| `edit.raw.append` | act | `{file?, bytes, align?, epoch?, confirm_peers?}` | `{id, pointer, len}` |
| `edit.stage.put` | act | `{stage?, bytes}` | `{stage, len}` |
| `edit.overlay` | read | none | `{entries, bytes, appended, appended_bytes, metadata, metadata_bytes, files, owners}` |

`owner` is one of:

| Owner | Names |
|---|---|
| `{section: {tag, kind, index}}` | `kind` is `cluster`, `instance_def` or `mode` |
| `{pixels: {tag, image, lod}}` | `lod` 0 to 2 |
| `{sound: {tag, range, permutation, chunk}}` | one sample chunk of a `snd!` permutation |
| `{bsp: {index}}` | the scenario's structure BSP `index`; refused while it is the loaded BSP (write it with `at`) |

`tag` is a datum or a name. `offset` counts from the start of the owner's
raw block, LOD chain, chunk or BSP region, and the bytes must fit inside.
`bytes` is base64, or `{stage, at?, len?}` naming bytes `edit.stage.put`
holds. `was` is base64 bytes or `{hash}` (FNV-1a 64, a number or hex
string). `edit.raw.append` reserves an extent past the end of `file`
(`local`, `mainmenu`, `shared` or `single_player_shared`; `local` by
default), aligned to `align` (a power of two, default 16); `pointer` is the
raw pointer to write into a block info, file selector in its top two bits.

A batch checks every field and raw `was` before its first write. A mismatch
returns error 8 (`MISMATCH`) with `data.fields` and `data.raw` (`owner`,
`offset`, `holds_hash`, `resident_holds_hash`, `why`), and nothing is
written. `resident` says whether a resident copy was written; `refresh` names
the native refresh queued (`section`, `model` or `image`, null for a sound
chunk or `bsp` owner). An eviction clears the section's loaded flag so the
guest pages it in again; pixels cannot be evicted. Order of writes, the
checks and the eviction details: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Raw edits".

`edit.changes` lists raw acts under `raw`; `edit.revert` undoes them newest
first and lists them in `reverted[]` with `raw`, `evict` or `append`. A stage
belongs to the connection that made it, is used up by the request that
applies it, and at most 256 MiB is staged at once. The overlay, raw journal
and stages live for one map. Raw writes refuse with `UNAVAILABLE` when the
loaded map's cache slot is unknown or no VFS is mounted. Session rules are
those of the other edit writes.

```sh
h2 agent edit.batch '{"epoch":E,"raw":[{"owner":{"section":{"tag":"<sbsp>","kind":"cluster","index":0}},"offset":4096,"bytes":"AACAPw=="}]}'
h2 agent edit.overlay
h2 agent edit.revert '{"id":N}'
```

### Block moves

A block that must hold more elements than the map gave it moves into the
**edit arena**: guest memory the host maps in the HLE reservation as the
arena grows. Design: [realtime-edits.md](../editor/realtime-edits.md), "Four
write primitives". Code: `crates/extensions/hot-tags/src/arena.rs`,
`crates/extensions/hot-tags/src/batch.rs`,
`crates/extensions/agent/src/hot.rs`,
`crates/extensions/game/src/hot_glue.rs`.

| Method | Access | Params | Result |
|---|---|---|---|
| `edit.batch` | act | adds `moves: [{root, stride, count, capacity?, bytes, was}]`; `writes` may be left out when it is given | adds `moves: [{root, tag, path, count, address, capacity}]` |
| `edit.arena` | read | none | `{base, len, mapped, used, largest_free, physical_free, physical_floor, moves: [{tag, name, root, address, capacity, stride, len}]}` |

`root` is `{tag, group?, path}` (a path ending at a block field or an
8-byte raw field such as `+0x20:u32[2]`) or `{at}`, the pair's address.
`stride` is the bytes of one element and must equal a defined block's
element size. `bytes` holds the `count` new elements (base64 or `{stage,
at?, len?}`), `capacity` (default `count`) the room. `was` is `{count,
address}`, the pair before the move.

A root that holds another pair than `was` answers error 8 (`MISMATCH`) with
`data.moves: [{root, tag, path, was, holds}]`. A write over a move's root in
the same batch is refused. An arena without room answers `REFUSED` (11) with
`data.why`, `data.root`, `data.need` and `data.largest_free`, and nothing is
written. Paths and addresses resolve against the blocks as they are before
the batch, so an edit inside a moved block goes in its `bytes` or a later
batch. A moved block belongs to its tag: `{at}` writes and `edit.read`
inside it are admitted. `edit.changes` lists moves under `moves`;
`edit.revert` puts the old pair back and frees the extent.

`edit.arena` reports the window (`base`, `len`, 4080 KiB), the bytes mapped
and held, the largest extent a move could have, the free physical memory and
the floor left free for the game (`physical_floor`, 2 MiB).
`world.placement.edit` grows placement blocks into the same arena
([Placement edits](#placement-edits)). The arena lives for one map (a BSP
switch keeps it). While it holds an extent, machine snapshots, rewind and
film keyframes are refused.

```sh
h2 agent edit.arena
h2 agent edit.batch '{"epoch":E,"moves":[{"root":{"tag":"<shad>","path":"postprocess[0].colors"},"stride":S,"count":3,"capacity":8,"bytes":"<base64>","was":{"count":2,"address":"0x..."}}]}'
```

### Collision edits

The lifted game reads a collision BSP in place through its eight
`(count, address)` pairs, so a live collision edit stages the new blocks in
the edit arena and repoints the pairs at one tick boundary. Design:
[realtime-edits.md](../editor/realtime-edits.md), "Collision". Code:
`crates/extensions/agent/src/hot/collision.rs`.

| Method | Access | Params | Result |
|---|---|---|---|
| `edit.collision.stage` | act | `{target, blocks: {name: {count, bytes}}, leaves?: {count, bytes}, mopp?, epoch, confirm_peers?}` | `{stage, bytes, blocks: {name: {count, address}}, arena: {used, largest_free}, activation}` |
| `edit.collision.commit` | act | `{stage, mode: "append" \| "rebuild", activate?, was: {name: {count, address}}, epoch, confirm_peers?}` | `{at, stage, mode, pairs: {name: {count, address}}, activated, freed: [address]}` |

`target` is `{structure: tag}` (`sbsp+0x14`), `{instance_def: {tag, index}}`
(the definition's BSP at `+0x70`) or `{coll: {tag, region, permutation,
bsp?}}` (`bsp` defaults to 0). `blocks` holds all eight blocks by the field
names of `tags::collision::Bsp` (`nodes`, `planes`, `leaves`, `refs`,
`nodes_2d`, `surfaces`, `edges`, `vertices`), laid out as
`Bsp::encode_blocks` lays them (`collision_build::coll::stage_blocks`). For
a structure, `leaves` is the per-leaf block (`sbsp+0x2c`) and `mopp` the
`sbsp+0x1e0` block; either left out keeps the structure's own. Bytes are
base64 or `{stage, at?, len?}`. A stage of `pathfinding` is refused.

The stage runs `Bsp::validate` and writes each nonempty block into its own
arena extent; nothing points at them yet. A full arena answers `REFUSED`
(11) with `data.block`, `data.need` and `data.largest_free`. A second stage
of the same target replaces the first. A stage belongs to its connection and
lives until a commit uses it or the map changes.

The commit names in `was` every pair it repoints: the eight block names,
`leaf_clusters` and `mopp`, each `{count, address}`. A pair that holds
another answers error 8 (`MISMATCH`) with `data.pairs: [{name, root, was,
holds}]` and nothing is written. All pairs are written at the same
boundary.

| Mode | Rule |
|---|---|
| `append` | for added solids: every existing element keeps its index, so a block with fewer elements than the one it replaces is refused |
| `rebuild` | changes indices; always activates; refused on a structure with pathfinding |

With `activate: true`, and always for `rebuild`, the commit asks the game to
activate the loaded structure BSP again (`Call::ReactivateStructure`,
`crates/extensions/agent/src/sim/calls.rs`: `STRUCTURE_BSP_DEACTIVATE`
`0x137da0`, then `STRUCTURE_BSP_ACTIVATE` `0x137d40`) and answers after it
has run. A game with no activation refuses these commits (`REFUSED`); a
failed activation puts the old pairs back, frees the stage and answers
`REFUSED`. Old extents are freed after the next activation, listed in
`freed`; a `coll` BSP's are kept until the map changes (respawn bodies with
`world.object.respawn`). The journal does not hold a commit: `edit.revert`
leaves it, and the editor undoes it with the inverse stage and commit.
Sessions are those of the other edit writes. What the commit rebuilds
(physics, `debug.view` geometry, bot line-of-sight walls): see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Collision edits".

```sh
h2 agent edit.collision.stage '{"epoch":E,"target":{"structure":"<sbsp>"},"blocks":{"nodes":{"count":N,"bytes":"<base64>"},...},"leaves":{"count":L,"bytes":"<base64>"},"mopp":"<base64>"}'
h2 agent edit.collision.commit '{"epoch":E,"stage":1,"mode":"append","activate":true,"was":{"nodes":{"count":N0,"address":"0x..."},...}}'
```

## Menu methods: settings, perf overlay, cheats

The in-game debug menu is a client of this API ([debug-menu.md](debug-menu.md));
its methods are in `crates/extensions/debug-menu/src/sources/mod.rs`
(`method`).

| Method | Access | Params | Does |
|---|---|---|---|
| `settings.list` | read | `{prefix?}` | settings (`settings.toml`) and how each applies (`crates/extensions/agent/src/menu.rs`) |
| `settings.set` | act | `{key, value}` | saves the value and applies it; `UNAVAILABLE` when something else set it for this run |
| `perf.overlay` | act | `{}` reads, `{on}` sets | the perf overlay (the watermark) |
| `cheat.list` | read | `{}` | `{refused, pending, cheats}`: each cheat's `name`, `label`, `about`, `kind` (`bool`, or `choice` with `choices`), `value`, `refused` (why the session refuses it) and `pending` (why it does nothing yet); the top-level `pending` says the same with no game running (`crates/extensions/game/src/cheats.rs`) |
| `cheat.set` | act | `{name, value}` | sets one, for example `{"name":"hide_hud","value":true}`; answers with `cheat.list`'s answer. Cheats: `god`, `infinite_ammo`, `bottomless_clip`, `game_speed` (0.1, 0.25, 0.5, 1, 1.5, 2, 4), `fly`, `noclip`, `hide_hud`, `ai_off` ([debug-menu.md](debug-menu.md#cheats)) |

### World debug views

| Method | Access | Params | Result |
|---|---|---|---|
| `debug.views` | read | none or `{}` | `{views, range, trace_ticks}` |
| `debug.view` | act | one of `{name, on}`, `{range}` or `{trace_ticks}` | the updated listing |

Each row has `name`, `label`, `on` and an optional `why`. The names are
`geometry_edges`, `trigger_volumes`, `kill_volumes`, `invisible_barriers`,
`collision_geometry`, `hitboxes` and `traces`. For example,
`h2 agent debug.view '{"name":"hitboxes","on":true}'` enables the same
switch as the window's Debug views section. Range accepts whole world units
from 1 to 512 (default 48). Trace lifetime accepts 1 to 300 guest ticks
(default 30).

A host without a window lists the views with a reason, and enabling a
refused view returns `UNAVAILABLE`. The source and capture code live in
`crates/extensions/game/src/debug_views.rs` and
`crates/extensions/agent/src/debug_views/mod.rs`; the wire types are in
`crates/extensions/agent-proto/src/debug.rs`. Capture and trace behaviour:
see the [detail note](notes/2026-11-26-agent-api-detail.md), "World debug
views", [Debug views](debug-menu.md#debug-views), and
[Drawing world-space lines over the frame](#drawing-world-space-lines-over-the-frame).
The collision query's layout is in
[Collision rewrite, §5.4](../engine/sim-rewrite-collision.md).

## `editor`: a paired editor

`h2 play` shows a plate in the top-right corner while an editor holds a
link lease: "Editor · viewing", "Editor · editing live" or "Editor · edits
blocked", with optional `text` on the second line. The plate expires on the
wall clock, even while the game is paused
(`crates/apps/cli/src/play_cmd.rs`, `crates/apps/app/src/shell.rs`). While
the debug menu is open the lines show inside it
([debug-menu.md](debug-menu.md#layout)).

| Method | Access | Params | Result |
|---|---|---|---|
| `editor.link` | read | `{editor, mode, text?, ttl_ms?}` or `{clear: true}` | `{shown, why?, expires_ms}` |
| `editor.focus` | act | none | `{focused, why?}` |
| `editor.marks` | read | `{marks, ttl_ms?}` | `{held, drawn, why?, refused}` |

`editor` is a string. `mode` is `view`, `edit` or `blocked`. Each call
replaces the lease, with a `ttl_ms` of 500 to 10000 (default 3000). `text`
takes up to 64 characters. Invalid arguments return `INVALID_PARAMS`.
`expires_ms` is the time remaining, or zero when the plate is cleared or
refused; clearing returns `shown: false`. The link is available in every
session kind (`crates/extensions/agent-proto/src/editor.rs`,
`crates/extensions/agent/src/editor.rs`).

`editor.focus` returns `focused: true` when it queues a request that
restores a minimized window and focuses it (`crates/apps/app/src/shell.rs`).
On `h2 serve` both methods answer `shown: false` or `focused: false` with
`why: "this host has no window"` (`crates/apps/boot/src/lib.rs`).

`editor.marks` replaces the mark set for 500 to 10000 ms (default 3000); an
empty set clears it. Each of up to 256 entries has exactly one geometry key.

| Entry key | Value |
|---|---|
| `object` | `"<eight hex digits>"` |
| `box` | `{origin, forward, up, size}`; size is the full lengths along forward, left and up |
| `point` | `[x, y, z]` |

An entry takes `color: [r, g, b]` (components 0 to 1), optional `style`
(`selected` or `candidate`), optional `label` and optional reserved `xray`
(no effect). An invalid entry leaves the previous set intact. `held` counts
accepted entries; stale object handles and object entries without the agent
tick observer are refused by index in `refused: [{index, why}]`. A live
stream epoch change clears the set
(`crates/extensions/agent/src/editor/scene.rs`,
`crates/extensions/agent/src/exec.rs`).

`drawn: true` reports a current genuine projection; `drawn: false` gives a
reason (headless host, native rendering or scene capture off, a loading
picture, a raw submission, an unusable world camera, or waiting for a
render). The window projects marks over the finished frame, including on
in-between frames ([high-framerate.md](../game/high-framerate.md), "World
marks on made frames"). Capture and drawing code
(`crates/rewrites/render-bridge/src/scene_step.rs`,
`crates/platform/d3d/src/present_camera.rs`,
`crates/apps/app/src/frame.rs`, `crates/apps/app/src/overlay/marks.rs`) and
the pause behaviour: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "`editor`: a paired
editor". A shell pause or paused Theater playback makes editor methods wait;
an agent pause keeps servicing them.

### Content previews from the editor

A paired editor's `pair.preview` and `pair.audio` are clients of this API on
a second connection to the paired game's socket ([editor agent
API](../editor/agent-api.md#previewing-content-in-the-paired-game);
`crates/apps/studio/src/api/preview/run.rs`). They add no game method. A
preview calls `session.methods`, then the methods it needs (`sim.give`,
`sim.spawn`, `cheat.set hide_hud`, `session.wait`, `world.object`,
`world.query`, `pogo.state`, `render.scene`, `ui.audit`, `ui.stack`,
`ui.screen`, `render.capture`, `host.screenshot`, `audio.voices`,
`audio.loops`), each where the game lists it. A game that leaves one out
answers a preview without that read. `sim.give` and `sim.spawn` are refused
in a co-op session and on a multiplayer client.

### Drawing world-space lines over the frame

A windowed host registers a `WorldLineSource` with
`Host::add_world_line_source` before boot. Each source supplies an immutable
sample of world-space edges; each group has an anchor, colour, dash flag,
optional label and a style. A sample lives as long as its image or carries its
own lease. Only the editor sample feeds `editor.marks` status. The superbounce
game type and its bounce view are two more sources, with image-lived samples
(`crates/extensions/game/src/superbounce_mode.rs`,
`crates/extensions/game/src/superbounce_view.rs`,
`crates/extensions/game/src/run_opts.rs`,
`crates/extensions/game/src/world_lines.rs`,
`crates/platform/gfx/src/world_lines.rs`).

A group's `LineStyle` (`crates/platform/gfx/src/world_lines.rs`) sets how it
is drawn; the default is a thin opaque line.

| Field | Meaning |
|---|---|
| `width` | Core width in pixels at 1080p, scaled by output height |
| `alpha` | Opacity |
| `glow` | Optional wider low-opacity halo |
| `pulse` | Optional period and depth, read from present time; brightness breathes between full and `1 - depth` |
| `fade` | Optional distance fade: full to its start, none from its end, in world units from the camera |
| `chevrons` | Optional arrowheads lying flat along each edge, pointing along a world direction; they flow when speed is not zero |
| `stroke` | On by default; off leaves only chevrons, ribbon and label |
| `ribbon` | Optional translucent floor strip of a world width along each edge, clipped at the near plane |
| `label_face` | `Debug` (8x8 font, default) or `Hud` (the disc's HUD font with button glyphs, once `gfx::world_lines::set_hud_font` has published one; 8x8 until then) |

A sample may also give marks in the image's HUD space (`ImageMarks::hud`,
`gfx::world_lines::HudMark`): paths, filled convex polygons and text in guest
pixels of the 640 by 480 image, drawn after the world lines and scaled with
the image. The superbounce bounce view draws its tracker blips and map this
way (`crates/extensions/game/src/superbounce_view.rs`).

All edges, ribbons, arrowheads and labels go through one pipeline and one draw
per frame (`crates/apps/app/src/overlay/marks.rs`). A sample is captured once
per guest frame, so motion belongs in the style (pulse, chevron speed), which
runs at every present.

In-between frames draw the samples of the newest genuine image with each
view's camera moved onto the frame's
([high-framerate.md](../game/high-framerate.md), "World marks on made
frames"). There is no depth test, so hidden edges draw at full strength
(`crates/platform/d3d/src/present_camera.rs`,
`crates/apps/app/src/overlay/marks.rs`, `crates/apps/app/src/shell.rs`). An
inactive producer costs an atomic poll
(`crates/extensions/game/src/world_lines.rs`,
`crates/rewrites/render-bridge/src/scene_step.rs`). Detail: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Drawing world-space
lines over the frame".

## Bot combat

`h2 agent bots.stats` reads the host's brain-driven phantoms at a tick
boundary. It takes no parameters. The result is `at` and a `bots` array; each
row has `handle`, `name`, `player` (null until bound to a game player),
`skill`, `profile`, `combat` and `accuracy`.

`maintenance` measures the roster work on render frames with configured seats
or pending removals; subtract two readings to get a window's mean cost.

| Counter | Meaning |
|---|---|
| `frames`, `total_ns`, `max_ns` | Calls, summed wall time, longest call |
| `unchanged_lobby_frames` | Frames where the seated custom lobby's records matched the previous sample |
| `write_frames` | Frames that wrote the session membership (join, removal, repair); stays near zero in a settled lobby |
| `props_writes` | Phantom peer properties rewritten to follow this console's |
| `identity_writes` | Phantom names or looks written back into player records |
| `handicap_writes` | Phantom handicaps put back to none ([bots.md](../game/bots.md), "A phantom's player configuration") |
| `team_moves` | Team rebalance moves |
| `props_diff_chunks` | Mask of 8-byte chunks of the `0xc8`-byte peer properties that differed; bit `i` covers bytes `8i` to `8i + 7` |
| `facing_releases` | Built actions after which the host cleared a phantom biped's facing-set flag ([bots.md](../game/bots.md), "The facing release"); counted on the action gather |

A growing `Δwrite_frames` means the game undoes a phantom write every frame;
the log says `bots: phantoms: the session's membership was rewritten on 30
frames in a row (...)` (`crates/live/bots/src/phantom.rs`, `churn_line`).

`profile` holds `summary`, `archetype`, `favorite_weapon`, the name's
`try_hard` score and the `weights` (each 0 to 1): `radar_use`, `noob_combo`,
`dual_wield`, `aggression`, `grenades`, `melee`, `weapon_preference`,
`range_preference`, `strafe`, `jump`, `reaction_jitter`, `retreat`.

| Field in `combat` | Meaning |
|---|---|
| `tick`, `observation_tick` | Latest think tick and the world sample it used (can be older) |
| `goal`, `target`, `range` | Current activity; visible target's player index and distance in world units (null outside a fight) |
| `weapon`, `weapon_role` | Held weapon's map datum and handling class; null before an inventory read |
| `last_decisions` | Up to eight goal changes and combat decisions, oldest first, each `tick` and `action` |
| `melee_presses`, `switch_presses`, `grenade_requests` | Inputs chosen by the brain, once per press |
| `weapon_switches`, `grenades_used` | Observed weapon exchanges and grenade decreases within a life |
| `last_reaction_ticks`, `total_reaction_ticks`, `reactions` | Ticks from first fight observation to first attack decision; mean is total over count |
| `think_ms`, `total_think_ms`, `think_ticks` | Wall time of the latest brain decision and controller update, total, sample count |
| `goal_decisions`, `standing_decisions` | Decisions per goal, and those whose left stick (under 0.2) stood still |
| `look_asked`, `look_stalled` | Decisions after a right-stick turn ask (over 0.3), and those where aim had not moved; a share near one means the look is not reaching the unit ([bots.md](../game/bots.md), "The facing release") |

`accuracy` groups registered shots and credited enemy hits by weapon and
decision range at fire time: close under 3 world units, mid 3 to under 12,
long 12 or more, or unknown. Each row has `damage_code`, `weapon`, `range`,
`shots`, `hits` and `accuracy`; an empty list means no registered shots.
Detail: see the [detail note](notes/2026-11-26-agent-api-detail.md), "Bot
combat".

## Bounce routes

A brain-driven bot is sent somewhere by a superbounce
([bots.md](../game/bots.md#superbounce-routes), docs/game/superbounce.md);
no other method sends a bot to a point.

`bots.bounce_to {bot, to, mode?}` asks a bot for a route to `to` (`[x, y, z]`,
world units). The reply is `{bot, handle, mode, to}`; the route is planned at
the bot's next sample. Put the bot at its start first with `sim.state.set`.

| Param | Meaning |
|---|---|
| `bot` | A `handle` from `bots.stats`, a gamertag (any case) or `player.<n>`; must be a brain seat, else `INVALID_PARAMS` |
| `to` | Target point |
| `mode` | `help` (default: bounce only where walking cannot get there, or much sooner) or `fun` (highest launch within reach, then on to `to`) |

`bots.bounce_status {bot}` reads the route back.

| Field | Meaning |
|---|---|
| `state` | `none`, `queued`, `running`, `done`, `failed` |
| `to`, `mode` | As asked |
| `why` | `helps`, `fun`, or `plain` when no bounce helped |
| `steps` | Any of `walk_crouched`, `prime`, `walk`, `bounce`, `drop`, `fly`, `nav` |
| `step`, `phase` | Index on, and the step's own phase (prime: `start`, `in`, `release`, `out`; drop: `line`, `run`, `air`) |
| `tries` | The step's try, from 1 |
| `primed`, `primed_read` | Level pair word is at -3; whether read from the game or assumed |
| `landing` | Last touchdown: `tick`, `at`, `speed_in`, `vz_out`, `miss`, `result` (`bounce`, `not_primed`, `missed`, `too_slow`, `no_bounce`) |
| `log` | Last 64 step and phase changes, `{tick, step, what, at}` |
| `error` | For a failed route: `timeout`, `no_map`, `no_path`, `goal_off_mesh`, `tail_stuck`, `path_lost_<check>_<leg>` ([bots.md, "Superbounce routes"](../game/bots.md#superbounce-routes)), `prime_failed`, `pair_lost`, `no_bounce`, `no_drop`, `stalled`, `state`, `enemy`, `no_route`, `practice_off`, `died`, `map_changed` |
| `practice` | While the superbounce mode is on: `state` (`idle`, `route`, `waiting`), `site`, `attempts`, `bounces`, `spots` ([bots.md](../game/bots.md#practising-bounces)) |
| `practice.train` | In a sim training run ([bounce-search.md](../game/bounce-search.md)): `site`, `stage`, `axis`, `slot`, `tries` |
| `knobs` | The learned file's keys the route's site is flown with; `null` for the route's own |

While a route runs the bot fights and picks up nothing. Types:
`crates/extensions/agent-proto/src/bot_bounce.rs` (`LearnedKnobs`,
`PracticeTrain`), filled in `crates/extensions/game/src/sim_bots.rs`. Stages,
axes and knob keys: see the [detail note](notes/2026-11-26-agent-api-detail.md),
"Bounce routes".

`superbounce.spots {map?, kind?}` lists the spots the bots know for a map
(default the loaded one): `map`, `scanned` and `spots`, measured first, then
scan best first.

| Spot field | Meaning |
|---|---|
| `kind` | `bounce` or `crouch`; also the request filter |
| `source` | `measured` or `scan` |
| `edge` | Two points: a bounce spot's collision edge, a busy point's vertex twice |
| `stand`, `fall`, `launch`, `drift`, `score` | Place and measures |
| `need`, `fall` | Measured only; 0 for a scanned spot, which uses the shared threshold ([superbounce.md, "Speed"](../game/superbounce.md#speed)) |
| `status` | `proven`, `failed`, else `likely` |
| `out`, `lip` | Crouch spot's way out from `stand` (X, Y); its overhead edges at `stand`, up to 8 |
| `shape` | `slot`, `crease`, `vee` or `busy` |
| `learned` | For a bounce spot with a learned line: `state` (`open`, `learned`, `partial`, `dead`, `blocked`), `knobs`, `tries`, `hits`, `n`, `ok`, `score`, `best`, and `why` (`SpotLearned`) for `dead` or `blocked` |

A `dead` site is listed, but the bots skip it unless it is proven.

## `player.goto`: drive the player somewhere

`player.goto` drives `player.0` to a point or a spot with the bots' route
brain, in one call ([bots.md](../game/bots.md#driving-a-player);
`crates/extensions/game/src/player_goto.rs`, autopilot `bots::goto`). It plans
over the nav mesh, replans when the player strays or stalls, and holds the
first controller's pad through the pad tap. It works with no bots; the nav
mesh is built by the nav worker when stale (`h2 bots nav <map>` builds by
hand).

| Param | Meaning |
|---|---|
| `to` | Point `[x, y, z]` or `{"spot": n, "kind": "bounce" \| "crouch"}` (default `bounce`), n the index among scanned spots of that kind, best first (as `superbounce.verify` uses; mode on, scan ready) |
| `within` | World units, default 0.3, range 0.05 to 10; a spot ends at its approach |
| `face` | Yaw in radians from +X toward +Y, applied on arrival by teleport |
| `timeout_ticks` | Default 3600 |
| `then` | `stop` (default), `crouch_recipe` (prime, then walk to approach) or `bounce` (a bounce spot's whole route) |

The reply is `{job, kind, goal, then, map, nav}` (`nav` is `ready` or
`loading`). A new call replaces a running one. It is refused while a
`superbounce.verify` batch drives `player.0`, and the batch while a drive
runs; without the pad tap (`superbounce-assist` off) it is `UNAVAILABLE`.

`player.goto_status {wait_ticks?}` reads it back, waiting up to 290 ticks for
the end.

| Field | Meaning |
|---|---|
| `state` | `none`, `loading`, `running`, `facing`, `done`, `failed`, `cancelled` |
| `job`, `kind`, `goal`, `then` | As started |
| `steps`, `step`, `phase`, `primed` | As in `bots.bounce_status` (steps add none beyond that list) |
| `position`, `distance` | Player position; distance left along the walk, else straight to the goal |
| `path` | Player, then waypoints ahead, at most 32 |
| `replans`, `replanned` | Count, and reason (`strayed`, `stuck`, `path_lost_<check>_<leg>`, `stalled`, `pair_lost`) |
| `ticks` | Ticks driven |
| `error` | The route's reasons as in `bots.bounce_status`, plus `stuck`, `timeout`, `no_route`, `no_launch_floor`, a missing nav mesh, a changed map |

`player.goto_cancel` stops it and lets go of the pad. The player strafes
along the path and does not turn while it walks.

### Saved places: `player.mark` and `player.recall`

`player.mark {slot?, player?}` keeps a player's position and aim under a
slot; `player.recall {slot?, player?}` puts a player back through
[`sim.state.set`'s teleport](#the-games-own-functions-teleport-give-spawn).
Code: `crates/extensions/agent/src/sim/mod.rs`; wire types
`agent_proto::sim::{MarkParams, Mark}`.

| Param | Meaning |
|---|---|
| `slot` | Name of 1 to 32 characters, or a number taken as its decimal name; default `quick`; at most 32 slots, marking an existing slot replaces it |
| `player` | `player.<index>` or `sim.<n>`; default the player on local user 0 (recall with none found: the mark's own player) |

`player.mark` answers at once with `{slot, player, pos, forward?, yaw?,
pitch?, map, slots}`, read at the tick boundary. `player.recall` answers after
the next tick with the teleport result (`{player, wrote, teleport, session}`)
plus `slot` and `mark`.

| Refusal | Cause |
|---|---|
| `GONE` | No position (between lives), no local player, or empty slot on recall |
| `INVALID_PARAMS` | Bad slot name, a 33rd slot, or `player` not a player |
| `INVALID_REQUEST` | Mark made on another map; or a recall in a co-op or client session, as `sim.state.set` |

A recall needs the `agent-calls` feature. Marks live as long as the process.

## `sim`: simulated players and direct state

`sim.*` lets an agent test co-op, multiplayer and Live flows from one running
game: non-human players, direct state writes, and assertions. It never starts
another game instance. They are game methods (`Agent::add_game_method`,
`crates/extensions/agent/src/sim/`), answered on the game's thread at a tick
boundary. Wire types: `agent_proto::sim`.

| Method | Access | Params and result |
|---|---|---|
| `sim.player.add` | act | `{kind: "phantom", identity: {xuid, gamertag}, source: script\|recording\|brain, team?, steps?, recording?}` adds a phantom peer, returns its `sim.<n>` id. `kind: "local"` is `INVALID_PARAMS` |
| `sim.player.remove` | act | `{id}` or `{all: true}` |
| `sim.player.list` | read | `players` (simulated players and their `player.<i>`) and `clankers` (brain-driven bots and brain state) |
| `sim.player.pad` | act | `{id, steps, append?}` sets or appends pad steps for a script-source phantom, in `input.pad`'s step format |
| `sim.state.set` | act | `{player, pos?, yaw?, pitch?, facing? (or forward?), team?, health?, shield?, grenades?, ammo?}`: host writes; `pos` and facing go through the game's teleport, answered after the next tick |
| `sim.give` | act | `{player, weapon, ammo?, keep?}`: makes a weapon from a `weap` tag (datum or name) and gives it; answered after the next tick |
| `sim.spawn` | act | `{tag, pos, yaw?}`: makes an object from its tag at `pos`; answered after the next tick |
| `sim.view` | read | `{roots?}`: the game as one JSON document |
| `sim.expect` | read | `{checks}`: expectations judged against `sim.view` at one tick |
| `sim.session` | read | The session kind and whether writes are allowed |
| `sim.objects` | read | `{kinds?, max?}`: every live object in slot order, `{objects, truncated}` |
| `sim.events` | read | `{kinds?}`: the last tick's events in order (`deleted`, `damage`, `kill`, `actor_killed`), `{events}` |

**Brains.** For a brain-driven player, `sim.player.add`, `sim.player.list` and
the `sim` root of `sim.view` include `brain` when the backend provides it.
`clankers` entries have `name`, `kind`, `brain`, and once joined `player`;
script and recording sources have no brain state. `brain` holds `skill`,
`profile` ([Bot combat](#bot-combat)), `personality` (the nine 0 to 1 traits
in [the Clankers trait table](../game/bots.md#personality-and-temperament)
and `favorite_weapon`: `spray`, `precise`, `scatter`, `sniper`, `blast`,
`blade`) and `mood` (`confidence`, `tilt`, `fear`, `streak`, optional
`revenge_target` `player.<i>`).

For a joined phantom, `clankers[].aim` appears once the host has gathered
input for its unit: `tick`, `stick`, `trigger`, `arrived_stick`,
`arrived_trigger`, `action_deg`, `fire_primary`, `assist_strength`,
`unit_forward`, `assist_player`. Field meanings: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "`sim`: simulated players
and direct state". Verify damage with `h2 agent sim view player`.

**`sim.objects`** reads the objects pool through `sim-state`
(`crates/rewrites/sim-state`,
[sim-rewrite-objects.md](../engine/sim-rewrite-objects.md) section 8).
`kinds` filters by tag group (`bipd`, `vehi`, `weap`, `eqip`, `garb`, `proj`,
`scen`, `mach`, `ctrl`, `lifi`, `ssce`, `bloc`, `crea`); `max` defaults to and
is capped at 2048.

| Object field | Meaning |
|---|---|
| `id`, `kind`, `definition` | Salted handle (`0x` and 8 hex digits), tag group (`null` outside the type table), definition |
| `flags` | `active`, `awake`, `post`, `new`, `deleting`, `defer`, `attach`, `child` |
| `parent`, `position`, `forward`, `up`, `scale` | Parent-relative under a parent |
| `velocity`, `sphere {center, radius}`, `vitality {body, shield, depleted}`, `nodes` | Units a second; node matrix count |
| `unit {player, actor, team, seat, weapons, held}` | Bipeds and vehicles only |

Wire types: `agent_proto::sim::objects`.

**`sim.events`** reads the simulation's event buffer through `sim-state`
(`sim_state::events`, [sim-rewrite-objects.md](../engine/sim-rewrite-objects.md)
section 8): the tick that just ran, in order; the buffer empties at each tick
start. An unknown `kinds` name is `INVALID_PARAMS`. Datums are `0x` and 8 hex
digits, or `null`.

| `type` | Fields |
|---|---|
| `deleted` | `id`, `kind`, `definition` |
| `damage` | `tick`, `victim`, `victim_kind`, `victim_player`, `owner_player`, `owner_object`, `owner_team`, `code` (`damage_type` low 6 bits, `modifier` high 2), `effect`, `flags`, `depleting`, `body`, `shield`, `direction`, `origin`, `position` |
| `kill` | `tick`, `attacker`, `victim`, `code`, `damage_type`, `modifier`, `victim_unit`, `attacker_unit`, `position`, `attacker_position` |
| `actor_killed` | `unit`, `character`, `killer`, `weapon` (packed damage code) |

Events are recorded only while `impl.sim.native.objects` is on
([object-update.md](../engine/object-update.md), "Slice 8 body"). Wire types:
`agent_proto::sim::events`.

**`sim.view`'s roots.**

| Root | Content |
|---|---|
| `at` | The stamp |
| `session` | Session view |
| `ui` | `ui.screen`'s answer; `null` without `H2_UI_WATCH=1` |
| `player` | By index: the `world` object plus `lives`, `score`, and with a unit `health`, `shield`, `grenades {frag, plasma}`, `ammo {loaded, reserve}` |
| `object` | Every `world` object by id; only when asked for |
| `count` | By kind, `team` per team, `total` |
| `sim` | Simulated players by `<n>`, and `count` |
| `input` | `input.state` |

Pointers read straight off it: `/player/1/team`, `/object/<id>/occupied`,
`/count/team/1`, `/sim/1/player`. `lives` is the record's lives remaining
(`+0x1ac`, `i16`; `null` while unlimited, -1); `score` is the player's stat
row (`+0x308 + 0x1c * index`, `+0x00`), `null` without a game engine. Both
are there between lives (`crates/extensions/agent/src/sim/state.rs`;
[firefight.md](../game/firefight.md#guest-boundary)).

**Expectations** are `h2 agent run`'s: `"<pointer> <op> [JSON]"` or
`{"pointer", "op", "want"}`; ops `==` `!=` `<` `<=` `>` `>=` `contains`
`exists` `missing` and `near {"to": number or array, "within": d}`. One
implementation, `agent_proto::expect`, serves `h2 agent run` (client side) and
`sim.expect` (in the game). The answer is `{"pass", "failed", "expects":
[{expect, pass, want, got}], "at"}`.

### `h2 agent sim` and scripts

There is one script format, `h2 agent run`'s. Any line may be a `sim` verb;
`h2 agent sim run <script>` also removes the simulated players the script
added (`"cleanup"` in the verdict). Run it against a game this console hosts,
with a bot identity issued by the local Live server. Example script: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "`h2 agent sim` and
scripts".

Verbs (`crates/apps/cli/src/agent_sim.rs`):

| Verb | Form |
|---|---|
| `add` | `add phantom [source] xuid=X gamertag=G [key=JSON...]` |
| `remove` | `remove <sim.N\|all>` |
| `list` | `list` |
| `pad` | `pad <sim.N> <steps> [append]` |
| `set` | `set <player.N\|sim.N> key=JSON...` |
| `give` | `give <player.N\|sim.N> <weap tag> [loaded=N] [reserve=N] [keep]` |
| `spawn` | `spawn <tag> pos=[x,y,z] [yaw=DEG]` |
| `view` | `view [root...]` |
| `expect` | `expect <expectation>` |
| `until` | `until <ticks> <expectation>`: `sim.expect` every 5 ticks; timeout fails with the last verdict, and stops an `h2 agent run` script |
| other | `session`, `lobby ...`, `run` |

**Lobby steps** (`agent_proto::sim::menu`) press player one's pad
(`input.pad`) and check each step on `ui.screen` with a timeout, on the client.

| Step | Does |
|---|---|
| `lobby mode <campaign\|xbox_live\|split_screen\|system_link>` | From the main menu, down to the row and A; expects the campaign or players' screen in that mode |
| `lobby host live_coop` | From the players' screen in Xbox Live mode (or the Live menu): A, down to Co-op Campaign (row 4), A; expects the Live menu to go |
| `lobby invite [ROW]` | Y in a lobby, then A on friend row ROW |
| `lobby start` | A on Start Game (row 0); expects the screen to go |
| `lobby press <b[+b...]> [JSON expect]` | Any presses, then wait for `{"screen", "main_menu", "mode", "not"}` |

Screen ids and rows are those `h2 coop test --drive` uses
(`agent_proto::sim::menu`, re-exported by `coop_drive`). They assume a menu
opens on row 0; a wrong row fails the step and names the screen seen.

### Determinism

`sim.session` reads the simulation mode (`coop_probe::read_sim`,
[live-coop.md](../live/live-coop.md) 1.1) and squad session
(`coop_probe::read_session`) through `boot::agent_glue::session_view`:

| Mode | Kind | `sim.state.set` |
|---|---|---|
| 0 | `none` (menus) | allowed (no players to write) |
| 1 | `local` (campaign, split screen) | allowed |
| 2, 3 | `coop_host`, `coop_guest` (synchronous) | refused: a write on one machine desyncs the others and trips the sim hash |
| 4 | `mp_host` (distributed server world) | allowed: host-authoritative |
| 5 | `mp_client` (distributed client world) | refused: the host owns the state |
| unreadable | `unknown` | refused |

A refusal is `INVALID_REQUEST` with the session in `data`. In co-op, drive
players with pads (`sim.player.pad`, `input.pad`). Writes happen at a tick
boundary only (`UNAVAILABLE` between slices), and every value is checked
before the first write, so a refused set writes nothing. An `h2 lockstep`
comparison run is local (mode 1) and not detected: a write there makes the
runs differ by design.

### What `sim.state.set` writes

Teleport and facing go through the game ([below](#the-games-own-functions-teleport-give-spawn)).
The other fields are host writes at the addresses Live telemetry reads
(`live_telemetry::layout`, `engine::view`):

| Field | Offset |
|---|---|
| Player record team | `+0xc0` |
| Unit team | `+0x138` |
| Body, shield vitality | `+0xec`, `+0xf0` ([health.md](../halo1/health.md); `engine::view::objects::Body`) |
| Grenade counts | `+0x23e` |
| Held weapon first magazine | `+0x22c` loaded, `+0x22a` reserve |

Spawning a player between lives needs the game's respawn and is not provided.

### The game's own functions: teleport, give, spawn

A host write of a position cannot move a player, because the biped's physics
puts the body back. So a move goes through the game
(`crates/extensions/agent/src/sim/calls.rs`). A game method answers in the
tick observer, where nothing can call the guest, so it queues a call and
defers its answer (`GameCtx::defer`, polled at each tick boundary, `TIMEOUT`
after 30 ticks, held while paused). The `agent-calls` feature (a `before` on
`game_tick`, first; `H2_FEATURES=-agent-calls` turns it off) makes the queued
calls at the start of the next tick, then runs the tick; the answer comes
after it. A snapshot is refused while calls are queued.

| Call | Guest functions |
|---|---|
| Teleport (`sim.state.set` with `pos`, `yaw`/`pitch` or `facing`) | `object_teleport` body `0x29ffb0`; `PLAYER_PLACE_NEAR` `0x14e970` (EBX player, ESI point, anchor -1); vehicle exit `0xe68c0` action `0x1e`; place `0x14e200`; facing vectors `+0x150`, `+0x15c`, `+0x180`; view `0x1874b0`; `object_set_position` `0xb75a0` |
| `sim.give` | `placement_data_new` `0xb7930`, `object_new` `0xb7b40`, `unit_add_weapon` `0xcd0c0` (stdcall `(unit, weapon, mode)`), delete `0xb8540`, empty slots `0xccff0`, weapon choice `0xcdeb0`; models `0xcce78`, `0x1e05c8` |
| `sim.spawn` | Netgame equipment spawn `0x15ab94..0x15abe6`: `placement_data_new` with no owner, position at `+0x1c`, forward and up at `+0x28`/`+0x34`, then `object_new` |

- **Teleport.** `yaw` is degrees about +z from +x, `pitch` degrees up (-89 to
  89), as `world.query` reports; or `facing: [x,y,z]` (also `forward`). A
  seated unit's facing is left and the answer says so. The answer has `wrote`
  (`pos`, `facing`) and `teleport` (`pos`, `body_forward`, `faced`, `unit`).
- **`sim.give`.** Default mode 2 empties the weapon slots and readies the new
  weapon; `keep: true` is mode 1 (added, held only if the unit's choice picks
  it). `ammo` writes the first magazine. The tag is a datum or a name with `/`
  or `\` (`h2 agent edit tags weap` lists them); a bare name looks among `weap`
  tags only; `group:name` or `name.group` gives the group. The answer has
  `weapon` (`tag`, `group`, `name`), `object`, `mode`, `held`.
- **`sim.spawn`.** Takes any group `object_new` knows (`bipd vehi weap eqip
  garb proj scen mach ctrl lifi ssce bloc crea`); a name in more than one
  group needs `vehi:<tag name>`. A distributed host with the
  [creature probe](../game/ai-networking.md#creature-slot-probe) enrolls a new
  creature. The answer has `object`, `pos`, `tag`.

All three are host writes: refused in a co-op session or on a multiplayer
client, and only at a tick boundary, as `sim.state.set`. Detail: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "The game's own
functions: teleport, give, spawn".

### Object edits

The first authorised object edit or `map.reload` adopts the game for editing
and installs lifecycle observers; starting the agent, listing methods and
pairing leave the ordinary hooks in place. The host needs the agent tick and
guest-call features. Film recording and playback, or a boot configured to
record or replay input, exclude these edits.

These methods share the tick and session checks of `sim.state.set` and the
peer-count confirmation of `edit.batch` (`confirm_peers` on a System Link host
with other machines). An unknown peer count, a Live matchmade game or a Live
game another console hosts is refused before queueing; a Live custom game
this console hosts is a host like a System Link one. Send `epoch` from
`edit.map` to pin a request; a stale `epoch` returns `INVALID_REQUEST` ("the
map epoch changed") at dispatch and again before the engine call. A timed-out
request loses its place in the queue. Object ids use the salted spelling from
`world.object`; a removed object or reused slot returns `GONE`. Code:
`crates/extensions/agent/src/sim/edits.rs`,
`crates/extensions/agent/src/sim/calls.rs`.

| Method | Params | Result after the tick |
|---|---|---|
| `world.object.set` | `{id, pos?, forward?, up?, epoch?, confirm_peers?}` | `{at, object}` with the moved object's id |
| `world.object.delete` | `{id, epoch?, confirm_peers?}` | `{at, deleted}` with the deleted id |
| `world.object.respawn` | `{id, epoch?, confirm_peers?}`, `{placement: {block, index}, epoch?, confirm_peers?}` or `{definition, epoch?, confirm_peers?}` | `{at, old, object, linked}` (old is null for an empty placement); for a definition, `{at, definition, respawned, skipped, engine_made}` |

- **Respawn by definition.** `definition` is a tag datum handle in hex. Every
  placement of the loaded scenario that names it and a live object occupies is
  remade by "Linked creation" below
  (`crates/extensions/agent/src/sim/calls/placement/definition.rs`),
  replacement first. `respawned` lists `{placement: {block, index}, old,
  object, linked}`; `skipped` lists `{placement, why}` for placements the
  path cannot remake; `engine_made` lists live objects no placement made.
- **Set** calls `OBJECT_SET_POSITION` (`0xb75a0`) with position in EAX, forward
  in ECX, up in EDX. A position-only move leaves orientation null; a partial
  turn must give perpendicular unit axes.
- **Delete** calls `OBJECT_DELETE` (`0xb8540`) with the datum.
- **Respawn** reads the entry's palette index, definition, position and
  orientation, creates the replacement with the `sim.spawn` sequence, then
  deletes the old occupant; a creation refusal leaves the old object. It
  accepts unnamed records with zero placement flags, default scale and zero
  type-specific fields, else `REFUSED` before deletion. The preview has
  `linked: false`. Held or attached occupants, multiple occupants, unidentified
  previews and unsupported blocks are `REFUSED`; a held item's salted id is
  `GONE`. Supported blocks: `scenery`, `vehicles`, `weapons`, `equipment`,
  `machines`, `controls`, `crates`, `creatures` (offsets from
  `tags::object::OBJECT_TYPES`, `crates/tools/scenario-edit/src/layout.rs`;
  reads in `crates/extensions/agent/src/sim/calls/placement.rs`).

Register conventions, preview bookkeeping and checkpoint behaviour: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Object edits".
[Placement index evidence](../live/live-telemetry.md).

### Placement edits

`world.placement.edit` is one transaction on one scenario placement block, at
one tick boundary. The editor computes it from its document diff (design:
[realtime-edits.md](../editor/realtime-edits.md), "Placements"). It takes the
session, epoch, identity and `confirm_peers` checks of
[Object edits](#object-edits). Code:
`crates/extensions/agent/src/sim/calls/placement/edit.rs`; wire types in
`crates/extensions/agent-proto/src/sim/edits.rs`.

| Param | Meaning |
|---|---|
| `block` | A `placement::BLOCKS` name (`scenery`, `vehicles`, `weapons`, `equipment`, `machines`, `controls`, `crates`, `creatures`), or `sound_scenery`, `starting_locations`, `netgame_flags`, `netgame_equipment`, `starting_equipment` |
| `was_count`, `count` | The root count the edit started from and the count after it |
| `records` | `[{index, bytes, was?}]`: the record at `index` of the new numbering, one stride in hex. `was` is the slot's current bytes in hex, or `{hash}` (FNV-1a 64, 16 hex digits) |
| `names` | `[{name, placement}]`: point scenario object name at `placement` (-1: none); object blocks only |
| `delete` | `[index]` in the old numbering: delete the live objects of these entries |
| `relink` | `[{from, to}]`: give old entry `from`'s live objects placement index `to` |
| `create` | `[index]` in the new numbering: make an object for the entry; object blocks only |

The result is `{at, count, capacity, moved?, deleted, relinked, created}`:
`deleted` and `relinked` are object ids, `created` is `[{index, object?,
linked, created, why?}]`, and `moved` is `{address, capacity}` when the block
grew into the edit arena.

| Rule | Behaviour |
|---|---|
| Order | The host checks everything first (root count equals `was_count`, each `was` matches, names exist and match the block type, nothing to delete or relink is held or attached). A disagreeing count or stale record is `MISMATCH` (`data.count`, or `data.records: [{index}]`) with nothing changed. Then it deletes, relinks, writes records, name repoints and count, and creates |
| Room | A block holds as many records as it loaded with, or its arena extent's capacity ([Block moves](#block-moves)). A `count` past the room moves the block into the arena with twice the room. A full arena, or a game without one, is `REFUSED` with `data.need`, `data.capacity`, and from a full arena `data.need_bytes`, `data.largest_free`. A `count` past the per-type limit (`map::scenario_layout`) is `REFUSED` with `data.limit` |
| Linked creation | Objects are made by the `world.object.respawn` sequence with the placement index at `+0x10` of the placement data; `linked` is read back. Records beyond the honoured shape (unnamed, zero flags, default scale, zero type fields) are written and come back `created: false` with the reason |
| Engine-walked blocks | Starting locations, netgame equipment, netgame flags and starting equipment take records and a count only; `create`, `delete`, `relink`, `names` are invalid |
| Count changes with other people | Refused on a System Link host (or a Live custom game this console hosts) with other people in it; `data` carries `people`, `peers`, `phantoms`. Phantom bots do not count (`Bots::joined_phantoms`). A count-preserving edit is not refused for peers. Matchmaking, a client and an unreadable Live session are refused earlier by `agent_glue::live_edit_refusal` (`Agent::set_live_edit_probe`); without the probe every Live game is refused |
| Not reverted by `edit.revert` | The editor sends the inverse transaction |

A respawn preview of a deleted entry is deleted with it, and one of a relinked
entry follows it. Detail: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Placement edits".

### Phantom backend

`boot::sim_bots::SimBots` implements `agent::sim::bots::Bots` over the
process-wide phantom registry, placing a player through
`bots::phantom::add(identity, team, source)` with a `bots::pad_source::PadSource`.
The bots feature must be enabled; a light host also needs `--host-bots`. A
script source uses the `input::backend::remote::Remote` step queue as
`input.pad` does, sampled once per game tick; a recording supplies the input
log's first pad; a brain supplies its own. `sim.player.remove` removes players
made through `sim.player.add`. Phantoms join multiplayer games this console
hosts; in a level-boot `h2 serve` session, drive the existing player with
`input.pad`, `sim.state.set` or `sim.give`.

### Live

Sim never talks to a Live server and never makes accounts or sessions. A
phantom needs `identity: {xuid, gamertag}` from the local embedded server
(`h2 play --live-local`, `live_server::start_embedded`), which accepts
`bot: true` only for xuids it issued. Never point a sim run at the public Live
server.

## Local Forge session

The local Forge feature registers these methods in
`crates/extensions/agent/src/forge/local.rs`. The controller and the agent
edit the same document. The methods appear in `session.methods` when the
feature is enabled.

| Method and access | Parameters | Result |
|---|---|---|
| `forge.status` (read) | `{}` or omitted | The local session, role, log head, budget, monitor camera, aimed and held IDs, screen and focus, pending work, last notice. Fields: tool settings `rotation_snap`, `movement_snap`, `axes`; `overlays` (`markers`, `monitors`, `gizmo`, `nav`); links `tool`, `aimed_link`, `link_from`, `teleporters`, `added_teleporters`, `links`, `pending_rebuild`; `palette`, `palette_refused`; `parked`; `groups`, `selection`; `labels`, `prefabs`; things `aimed_thing`, `held_thing`, `game`, `show`; `variant` `{name, title, id, tier, dirty}`; `ground` and `nav` in brief; `fine`, `ui_up`, `dirty`, `screen`, `area_edit`. An inactive session returns `{session:"inactive", reason}` |
| `forge.ground` (read) | `{}` or omitted | The AI ground (docs/game/forge-map-variants.md, "AI ground"): `{state, building, available, loaded, recipe, counts, markers, problems, report}`. `state` is `not_built`, `current` or `stale`; `recipe` is `{kit_version, home, probe}` or null; `counts` maps each label (`ff_drop`, `ff_base`, `ff_cover`, `siege_site`, `siege_goal`, `siege_line`, `siege_barrier`) to its count; `markers` lists flags as `{id, label, number, position, facing, team}`; `problems` is `{label, position, reason}`; `report` is null before a build, else `{zones, squads, problems, receipt}` |
| `forge.ground.build` (act) | `{if_stale?}` or omitted | **Build AI ground**, or with `if_stale: true` **Test waves** (build only when stale or never built). Answers when the build ends with `forge.ground`'s reply plus `built`. Runs off the game thread and edits wait. Refused as `forge_refused` with no builder, in a shared session, while another edit or file job waits, or with the build's own sentence on failure; a failed build changes nothing |
| `forge.nav` (act) | `{generate?}` or omitted | **Generate nav** ([forge-nav.md](../game/forge-nav.md)): `{state, available, generating, counts, problems, overlay}`. `counts` is null before a generation, else `{sectors, links, out_of_map, solids}` with `links` keyed by `walk`, `drop`, `jump`, `crouch_jump`, `teleporter`, `lift`, `elevator`. `problems` is `{kind, what, position, reason}` with `kind` `start_off_mesh`, `teleporter_off_mesh` or `solid_left_out`; `overlay` is `{kind, a, b}` lines. `generate: true` (a local multiplayer tick) generates first. Refused as `forge_refused` with no generator, in a shared session, while another job waits, or with the generator's sentence; a failure changes nothing |
| `forge.things` (read) | `{kind?, changed?}` or omitted | `{things, left_out}`. `kind` is `object`, `item_spawner`, `starting_point`, `goal` or `teleporter`; `changed` filters by whether an entry op changes the thing. Each thing is `{id, block, kind, own, changed, position, rotation, fields}`; `id` is `{base:{block, index, print}}` or `{added: op}`; netgame flags add `area` (or `null`). `left_out` lists unread blocks as `"block: reason"` |
| `forge.doc` (read) | `{}` or omitted | `{document}`, the folded Forge document including confirmed edits |
| `forge.log` (read) | `{since?}` | `{entries}` in sequence order, starting strictly after `since` (default zero) |
| `forge.input` (act) | `{action}` or `{movement?, look?, roll?, rotating?}` | `{queued:true}` when the current local session accepts one monitor input |
| `forge.propose` (act) | `{op, was?}` | `{entry}` on acceptance, or a refusal: its sentence as the message and `{code, message}` as the error's `data`. `op` may be an entry op (`{"entry":{"set":{target, write}}}`, `remove`, `reset` or `{"entry":{"area":{target, area}}}`) naming a thing by its `forge.things` `id` |
| `forge.look` (read) | `{seat?}` | `{session, look, drawn_as, saved, catalogue, locks}`, plus `notice`, `catalogue_note`, `problems` when present. `drawn_as` is `{as: "model" \| "marker" \| "hidden", reason?}`; `catalogue` rows are `{key, name, source, sha256, variants, colours, light, nodes, triangles, available, refusal?}`. A `seat` other than 0 is refused |
| `forge.look.set` (act) | `{reset?, save?, model?, variant?, primary?, secondary?, tint?, light?, light_colour?}` | `{look, changed, saved, drawn_as}`. `reset: true` takes no field; `variant: null` clears the variant; `save: true` writes the profile (`not_saved` says why not). Errors: `monitor_look_invalid` (`field`, `reason`), `monitor_look_locked` (`field`, `reason`), `monitor_look_no_session`; the look stays as it was |
| `forge.status.monitors` (read) | `{}` or omitted | `{monitors: [{seat, name, monitor, pose, look, drawn_as, peers_see, seen_ticks_ago}]}`. `pose` is `{position, yaw, pitch}`; `drawn_as` is always `hidden` in the seat's own view |

Field-by-field detail for `forge.status`, `forge.ground`, `forge.nav`,
`forge.things` and `forge.look`: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Local Forge session".

The input queue accepts `toggle`, `accept`, `select`, `back`, `delete`,
`palette`, `properties`, `tools`, `settings`, `overlays`, `undo`, `redo` and
`link`. In player mode only `toggle` enters the monitor. While a screen is
up, `forge.input` is refused ("A screen is up; drive it with ui.press.") and
`menu` is refused ("The Forge menu is the game's GAME MENU; open it with
Start."). An analog request is one 1/30-second sample: `movement` is
[right, forward, up], `look` is [yaw, pitch], each axis in [-1, 1]; `roll`
turns a preview with `rotating:true`. Per-action behaviour: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Local Forge session".

Choose **Edit area** in a goal's Properties before sending area input. While
a draft is active, `forge.status.area_edit` gives
`{target, shape, below, above, radius?, points?}` in world units, else
`null`.

| Input in the draft | Radius | Path |
|---|---|---|
| `accept` | Save | Drop a point, or close at the first point |
| `properties` | Open the typed dimension rows | Remove the last point |
| `area_shape` | Switch to an empty path | Switch to an equal-floor radius |
| `area_close` | | Close and save the path |
| `area_grab`, `area_release` | | Press or release A for a drag |
| `back` | Cancel | Cancel |

In a radius draft the forward axis grows or shrinks the radius. Read
`forge.status.notice` for geometry refusals and `forge.log` for the area op
written on save.

A proposal uses the [Forge document's](../game/forge-design.md#the-document)
portable op and optional prior-value digest; the local host supplies author
and sequence. Moves, deletes and restores run at the next tick. A live
object that no longer resolves is refused as `forge_object_gone`, and a
block with no `world.object` kind as `forge_object_kind`. A refusal leaves
the log head in place.

An entry op edits one of the map's own things
([forge-map-variants.md](../game/forge-map-variants.md), "Applying an
edit in a session"): read its `id` from `forge.things`. A refused input or
proposal carries its name in `data.code` (`agent_proto::forge::refusal`):

| Code | Refused because |
|---|---|
| `forge_print` | the entry's print is not the one the op carries: the map's entry is not the one the edit was made on |
| `forge_was` | the target's value is not the one `was` names |
| `forge_left_out` | the entry's block did not read when the session opened (`forge.things` `left_out`) |
| `forge_object_gone`, `forge_object_kind`, `forge_object_held` | the game call's: the live object no longer resolves, its block has no live objects, or the own entry's object is held or attached |
| `forge_no_kit` | an add or a drop of a starting location, netgame flag or netgame equipment row, on a map whose next row is not in the kit's spare form: the map has no kit (or its spare rows are used) |
| `forge_refused` | any other: the message says why |

A kit session adds those rows with one `Add` base step and removes them with
one `Drop` step (`agent::forge::base`). A batch is checked in order; a
refusal changes nothing.

An area op names a netgame flag, own or added, and sets its
[document area](../game/gametype-markers.md#in-the-document). Send
`"area": null` to clear it. Radius, offsets and heights use the document's
eight-digit lowercase hexadecimal float spelling. `forge_refused` carries
the area validator's sentence. The `was` precondition applies to field sets.

A teleporter link or unlink (or its undo) writes each changed flag's
identifier into the loaded scenario at the same tick, after the batch's
objects, checking the flag index and type (7 or 8) first. A device link
reaches the game only through a rebuild and counts in `pending_rebuild`.

## Shared Forge session

In a synchronous network game whose co-op link carries the Forge lanes, the
local Forge methods answer for the shared session
(`crates/extensions/agent/src/forge/net.rs`; the design is
[forge-design.md](../game/forge-design.md), "F5 in detail").

- `forge.status` has `session: "shared"`, the seat's `role`, and `net`:
  `host`, `joined`, `seat`, `name`, `members` (`seat`, `name`, `role`),
  `holds` (`seat`, `target`, `carried`), `head`, `n` (the last stream
  item applied), `through` (the host's watermark), `in_flight`
  (proposals and holds not yet settled) and `ended` (why the session
  ended, else null).
- `forge.propose` sends the op to the host and answers when the stream
  carries the entry or the host refuses it. It waits up to 240 ticks, not
  30. A refusal's message begins with its name and a colon: `forge_not_joined`,
  `forge_role`, `forge_held`, `forge_not_holding`, `forge_stale`,
  `forge_ahead`, `forge_lane`, `forge_was`, `forge_print`, `forge_refused`,
  `forge_map` (the game's palette, budget or bounds), `forge_seat`,
  `forge_name`, `forge_head_differs` and `forge_clock`. A proposal waiting
  when the session ends is refused `forge_not_joined`.
- `forge.log`, `forge.doc` and `forge.things` read what this machine has
  applied, so two machines at the same `through` return the same entries.

A session that has ended keeps answering `forge.status` with `ended` set
and refuses every proposal.

## Forge probes

The running game's agent registers these methods through
`crates/extensions/agent/src/forge.rs`. The wire types are in
`crates/extensions/agent-proto/src/forge.rs`. Results include the normal
`at` stamp. See the [measurement procedure](../game/notes/forge-probes.md).

| Method and access | Parameters | Result |
|---|---|---|
| `forge.capacity` (read) | `{}` or omitted | `tag_arena {low, high, headroom_bytes}`, `object_pool {used, limit}`, and `blocks` keyed by scenario block name |
| `forge.probe` (act) | `{what:"replication", group, tag?, pos?}` | `{what, session, tag:{tag, group, name}, object, pos}` after the next tick |
| `forge.probe` (act) | `{what:"sync", from_tick?, max?}` | `{what, session, scenario_type, variant_engine, desync, hashes}` |
| `forge.probe` (act) | `{what:"ai"}` | `{what, enabled, active, actors}` |
| `forge.probe` (act) | `{what:"kit"}` | `{what, blocks, objects_by_group, actors}` |

**Capacity and kit.** `headroom_bytes` is the gap between the two physical
arena cursors. Each block reports `count`, `limit` (or `null`) and
`kit_left`, the contiguous marked tail of the block; a block with no known
marker reports `null`. The kit probe adds an object census by group and the
live actor count.

**Replication.** The host must be in a ticking `mp_host` session. `group` is
one that [`sim.spawn`](#the-games-own-functions-teleport-give-spawn) accepts;
`tag` is a datum or name in it (default: the first valid loaded tag); `pos`
defaults to one world unit above the first eligible starting location. The
probe queues one `sim.spawn` call and returns the host's salted `object`
handle, as `sim.objects` gives it.

**Synchronous protocol.** `session` is read as `sim.session` reads it;
`scenario_type` is the tag's kind (multiplayer is 1); `variant_engine` is
the engine in the active game options (none is 0); `desync` is the desync
latch. The probe returns the current map's
[`sim.hashes`](#framescene-checks-compare-capture-scene-parity-hashes) rows
up to the sampled tick. `from_tick` is inclusive; `max` defaults to 4096 and
accepts 1 to 4096. The rows are empty in pregame or with no hash samples.

**AI.** `enabled` is the flag the engine's `ai` builtin sets; `active` is the
adjoining activity flag. A missing AI global gives `null` for both; a missing
actor pool gives zero actors. Capacity, AI and kit reads require a gameplay
tick. Marker values and replication detail: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Forge probes".

## Firefight

The [Firefight](../game/firefight.md) runtime of the loaded map, read and
driven at a tick boundary. `crates/extensions/agent/src/firefight.rs`
checks the params and calls the game's runtime
(`game::firefight::feature::AgentSource`, over
`crates/extensions/game/src/firefight/runtime.rs`); the wire types are in
`crates/extensions/agent-proto/src/firefight.rs`. The methods are always
registered; with the `firefight` feature off
([switches.md](switches.md)) `firefight.state` says so and
`firefight.command` refuses.

| Method and access | Parameters | Result |
|---|---|---|
| `firefight.state` (read) | `{}` or omitted | `{running, refused?, map?, role?, team?, director?, log?, errors}` |
| `firefight.command` (act) | `{command?, director?: "hold" \| "run"}`, at least one | `{appended?: {seq, tick}, held, log}` |
| `firefight.kit` (read) | `{}` or omitted | `{kit, slots, team?, zones: [{index, point, starts}], classes, bases, characters, hunt_orders}` |

**State.** `running` is false outside a multiplayer game, on a map without
the kit, and with the feature off; `refused` names which. `role` is `host`,
`replica` or `client`. `team` is the actors' team. `director` (host only) has
`phase` (`warmup`, `spawning`, `fighting`, `cleanup`, `intermission`,
`done`), `phase_tick`, `round`, `wave`, `reserve`, `teams`, `pools`,
`scores`, `live_actors`, `cap`, `players`, `skulls` and `held`. `log` is the
head (`seq`, `tick`, `hash`) and `applied`. `errors` holds the latest 16
refused entries, each with `seq`, `tick` and `error`.

**Command.** `command` is one director command in the host log's shape
(externally tagged, snake_case): `place_squad {slot, class, count, zone,
weapon_tier}`, `target {slot, team}`, `hunt {slot}`, `erase_squad {slot}`,
`round {index}`, `wave {index}`, `skull {id, on}`, `lives {player?, pool}`,
`team_lives {team, pool}`, `score {player, points, reason: kill | assist |
player_kill}`, `respawn {player}` and `finish {outcome: won | lost |
{team_won: {team}} | draw}`. It is appended at the tick after the last one
the runtime stepped, with the author `agent`. `director: "hold"` stops the
director stepping; `"run"` restarts it; a director change applies before the
command. Refusals:

| Error | When |
|---|---|
| `UNAVAILABLE` | the mode does not run |
| `REFUSED` (11) | on a client or a replica (only the host writes the log) |
| `INVALID_PARAMS` | a command of another shape, a class or zone the kit lacks, a `target` team with no base, a `team_lives` team above 1, or params with neither field |

An engine refusal comes later, in `firefight.state`'s `errors`.

**Kit.** Read from the loaded scenario on any machine: `slots` (sixteen
squad indices, `null` where unreserved), `zones`, `classes`, `characters`,
`bases` (per player team; base 0 is home) and `hunt_orders` (`ff_hunt_00`,
`ff_hunt_01`; `null` where none). A map without the kit answers
`{"kit": false, ...}` with empty lists.

```sh
h2 agent firefight.kit
h2 agent firefight.command '{"director":"hold"}'
h2 agent firefight.command '{"command":{"erase_squad":{"slot":0}}}'
h2 agent firefight.state
```

## Rules scripts

The [rules script](../game/rules-engine.md#the-host-runtime) of the game
running now, read at a tick boundary. `crates/extensions/agent/src/rules.rs`
checks the params and calls the game's runtime
(`game::rules::feature::AgentSource`, over
`crates/extensions/game/src/rules/runtime.rs`); the wire types are in
`crates/extensions/agent-proto/src/rules.rs`. The methods are registered in
builds with the `rules` cargo feature; with the `rules` switch off
([switches.md](switches.md)) it says so.

| Method and access | Parameters | Result |
|---|---|---|
| `rules.state` (read) | `{}` or omitted | `{running, refused?, script?, version?, hash?, source?, carrier?, scoring?, options?, authority?, stopped?, budget?, pending, errors, last_reload?, blocked?, labelled?}` |
| `rules.zones` (read) | `{}` or omitted | `{zones: [{id, label, number?, shape: "area" \| "corners" \| "default", area, at, reports}]}` |
| `rules.reload` (act) | `{ir_b64 \| source, mode?: auto \| in_place \| restart}` | `{mode: in_place \| restarted, hash, reasons, tick}` |

`running` is false with the switch off, outside a multiplayer game, for a
record that names no script, on a machine that is not the engine's
authority, after the game's end, and for a refused script; `refused` says
which. The other fields:

| Field | Meaning |
|---|---|
| `hash` | the blob's SHA-256 in hex, or the record's 16-digit prefix when no blob was found |
| `script`, `version` | the script header's |
| `source` | `builtin` or `live` |
| `carrier` | the active variant record's physical index (6 on a script-index record) |
| `scoring` | who scores: `script` (the VM), or `carrier` for a script with a `uses` line ([rules-variants.md](../game/rules-variants.md#a-script-that-keeps-its-carriers-rules)) |
| `options` | the record's option bits |
| `authority` | what `GAME_ENGINE_AUTHORITATIVE` answered at the first tick boundary |
| `stopped` | the script error that stopped the VM |
| `budget` | `tick`, `ops` against `limit` (20,000), and the verifier's `worst_tick` |
| `pending` | calls waiting for the next tick boundary |
| `errors` | the latest 16 refused calls: `tick`, `call`, `error` |
| `last_reload` | the last `rules.reload`: `mode` (`in_place`, `restarted`, `refused`), `hash`, `tick`, `reasons` |
| `blocked` | the start gate's problems with the map ([rules-start-gate.md](../game/rules-start-gate.md)), in check order; in the lobby, the lobby check's (`rules::gate`) |
| `labelled` | the map's labelled things: `label`, `class` (`object`, `flag`, `volume`, `spawn`), `number`, `team`, `id`, `declared` |

To check a map against a script, choose both in the lobby as its leader (or
start a game) and read `rules.state`; there is no separate census method.
Block and refusal wording: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Rules scripts".

`rules.zones` lists the zones the host built for the running script, in `id`
order: the flag's `id`, `label`, `number`, `shape` (the flag's own `area`, a
hill's `corners`, or the `default` cylinder), `area` (`{radius, below,
above}` or `{path: [[x, y], ...], below, above}`), `at` (centre in world
units) and `reports`
([gametype-markers.md](../game/gametype-markers.md), "In a script";
`Runtime::zones` in `crates/extensions/game/src/rules/runtime.rs`).

`rules.reload` puts a new script into the running game
([rules-engine.md](../game/rules-engine.md#reloading-a-running-script);
`crates/extensions/game/src/rules/reload.rs`). Give exactly one of `ir_b64`
(the IR in base64) or `source`.

| `mode` | Behaviour |
|---|---|
| `auto` (default) | in place between ticks if the state shape is kept (every variable keeps its value), else restart; `reasons` says why |
| `in_place` | refuses rather than restart |
| `restart` | always restarts the game on the same map and variant; answers up to 30 ticks later; needs the `agent-calls` feature |

It is refused (`REFUSED`, with `data.why`) for a Theater film, a matchmade
Live game, a guest, outside a multiplayer game, and for a blob that does not
verify or that this host cannot run; a Live custom game reloads in place only.

```sh
h2 agent rules.state
h2 agent rules.zones
h2 agent rules.reload '{"source":"rules \"Mine\" 1\ntype slayer\nuses slayer\n..."}'
h2 agent rules.reload '{"ir_b64":"...","mode":"in_place"}'
```

## Game variants

The lobby variant's Quick Options as its rules script marks them
([rules-variants.md](../game/rules-variants.md#quick-options-in-the-lobby)),
read at a tick boundary, and one row changed through the lobby variant
setter. `crates/extensions/agent/src/variants.rs` checks the params and
calls the game's rows (`game::variant_quick::AgentSource`, over
`crates/extensions/game/src/variant_quick.rs` and `variant_quick/page.rs`).

| Method and access | Parameters | Result |
|---|---|---|
| `variants.quick` (read) | `{}` or omitted | `{variant, engine, script?: {name, hash}, shown, rows: [{slot, kind: setting \| option, name, label, value, text, line, help?, values: [{value, text}]}]}` |
| `variants.quick.set` (act) | `{slot, value}` | `variants.quick`'s result, read after the change |

The variant is the lobby's; with none it is `UNAVAILABLE`. `script` is the
script the record names, when this machine has it. `shown` is true when the
`variant-quick` switch ([switches.md](switches.md)) is on and the script was
found. `rows` are the script's `quick` marks in order, at most five: `kind`
is a record setting (`rules::settings::TABLE`) or a script option, `line` is
the panel's "Label: value", and `values` are what the row may take. The
panel's lines are in [`ui.lobby_options`](#ui-what-the-menus-show).

`variants.quick.set` sets row `slot` to one of its `values`, checks the
record with `record::refused`, queues it, and answers after the next UI
update calls `LOBBY_SET_VARIANT`.

| Error | When |
|---|---|
| `UNAVAILABLE` | no script on this machine, or the setter refuses ("the game refused the variant") |
| `INVALID_PARAMS` | a slot past the script's marks, a value the row does not offer, or a record `record::refused` refuses (with the reason) |
| `TIMEOUT` | the update does not run within the deferred limit |

It changes the lobby's variant, not a save
([rules-variants.md](../game/rules-variants.md#quick-options-in-the-lobby)).

```sh
h2 agent variants.quick
h2 agent variants.quick.set '{"slot":0,"value":10}'
```

## Sim matches

Host methods while sim mode is on (`H2_SIM=<match file>`, `h2 sim run
<match> --socket PATH`; `boot::sim_glue::Sim::add_methods`, [sim.md](sim.md)).
Sim mode is a headless match with bots and no rendering, ending in a report;
these let a client watch it, read its numbers and end it. The `sim.*` methods
above are different: simulated players added to an ordinary run.
`sim.guest_census` is the one `sim.*` method of a sim match.

| Method | Access | What |
|---|---|---|
| `match.status` | read | `match`, `map`, `game_type`, `bots`, `phase` (`waiting`, `binding`, `playing`, `over`), `lobby`, `bound_seats`, `ticks`, `max_ticks`, `game_seconds`, `wall_s`, `match_s`, `speedup`, `frames` (`skipped`, `rendered_world`, `other`, `stub_faults`), `end` (`game_over`, `tick_limit`, `wall_limit`, `stopped`; `null` while it plays), `drops` (a [seam drops](sim.md#seam-drops) match's `planned`, `done`, `control`, `error`; `null` without `[drops]`), `bounces` (a [bounce search](bounce-search.md)'s trials, `sites`, `batches`, `phase`, `batch`, `variant`, forks `captures`, `restores`, `waves`, `busy`, `last_busy`, and `error`; `null` without `[bounces]`) |
| `match.report` | read | the report as it would be written now (`h2-sim-report/1`, sim.md section 6); `result.end` is `aborted` until the match ends |
| `match.stop` | act | end the match: `end` becomes `stopped`, the report is written. `{stopping, end, already_over}` |
| `sim.guest_census` | read | no parameters (any other is `INVALID_PARAMS`). `{now, match}`: `now` is the guest census at this boundary, `{local_users, local_visibility_empty, arrays: {effect, particle_system, particles, decals, lights}}`; `match` is the report's `guest_census` so far (`ticks`, `local_users`, `local_visibility`, `arrays` with `samples`, `mean`, `peak`). Each is `null` where unread. Addresses: sim.md section 6 (`game::sim_census`) |

`match.status.lobby.step` advances through `Create`, `SquadCreated`,
`SessionStarted`, `Start` and `Loading`; `wait` gives the current setup
blocker or `null`. `session_type` and `squad_state` come from the same setup
tick and keep the last setup reading during play. Use `phase` and
`bound_seats` for gameplay progress.

```sh
h2 sim run docs/runtime/sim-examples/aim-net-2v2.toml --socket /tmp/sim.sock &
H2_AGENT=/tmp/sim.sock h2 agent match.status
H2_AGENT=/tmp/sim.sock h2 agent match.report
H2_AGENT=/tmp/sim.sock h2 agent sim.guest_census
H2_AGENT=/tmp/sim.sock h2 agent match.stop
```

`session.pause` and `session.step N` step a match.

## Theater transport

The game agent lists these host methods in `session.methods`
(`crates/extensions/agent-proto/src/theater.rs`,
`crates/extensions/game/src/theater/controls.rs`). From a signed-in local
multiplayer lobby, `python3 scripts/theater-e2e.py` runs a game, opens its
Recent film, exercises both cameras and the transport, and closes the player.

A game that records its own games plays a film in itself: `theater.watch`
queues the film's game options as the next game, and the transport methods
answer from that game until `theater.end` (unavailable until it has
activated). One film at a time; the session's recording is parked while it
plays; a co-op or networked session refuses `theater.watch`. Details: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Theater transport".

In any other CLI-hosted game, `theater.play` with a library ID starts an
isolated playback process, and later transport requests go to its player
(retry after a temporary unavailable response). The process also accepts calls
on its own `--socket` (`H2_AGENT`, else the cache's `theater.sock`).

| Method | Parameters | Result |
|---|---|---|
| `theater.watch` | `{"id":"film-id"}` or `{"path":"/films/a.h2film"}`; optional `tick`, `seat`, `camera` | on a game that records its games: `{id, length_ticks, queued}`. The film must be playable (`theater.compat`) and have a game start. `tick` plays unrendered to that tick. Refused while a film is watched |
| `theater.end` | `{}` | on a game that watches a film: queues the main menu and leaves the film. Refused with no film |
| `theater.exit` | `{}` on the library agent | Closes the active playback window. Wait for `theater.state` to return `active:false` on the original game. |
| `theater.list` | `{}` on the library agent | Local films, newest first, as `{id, map, variant, started_ms, length_ticks, saved}`. Pass the ID to `theater.play`. |
| `theater.open` | `{"url":"h2recomp://theater/<film>?server=<encoded-origin>"}` | Opens the film detail page in the signed-in game and returns its `ui.screen`. Watch starts playback. Optional `clip`, `t`, `seat` and `camera` select the initial view. |
| `theater.share` | `{"id":"film-id","visibility":"link"}`, optional `clip` (a clip ID or two tick bounds) | Returns `{film,url}` after publication. Visibility is `private`, `link` or `public`. A participant shares the existing host film by link. |
| `theater.play` | `{"id":"film-id","range":[300,900]}` to open, `{}` to resume; omit `range` for the whole film | `{id, starting:true}` on launch, otherwise transport state |
| `theater.state` | `{}` | transport state, or `{id:null,active:false}` with no player |
| `theater.pause` | `{}` | holds playback and reports state. During a seek it holds at the destination |
| `theater.compat` | `{"id":"film-id"}` | `{state, reasons, film: {xbe, contract, hooks, abi}, installed: [{stamp, state}], monitor_looks?: [{seat, map, reason}]}` |
| `theater.speed` | `{"speed":0.5}` | sets host pacing from 0.25 to 4 times realtime |
| `theater.seek` | `{"tick":900}`, optionally `"clip_marks":[300,null]` and `"source":"boot"` | a tick ahead runs forward (`seek.source` is `forward`); a tick behind, or `"source":"boot"`, queues the film's options again and replays from the game's first update (`boot`). Camera and timeline carry across |
| `theater.seek` | `{"cancel":true}` | returns to the tick the active seek left and holds there. `tick` is ignored. With no active seek, returns state |
| `theater.step` | `{"ticks":1}` | positive ticks while paused and outside a seek; runs them, holds, returns state. Read state again for the completed tick |
| `theater.camera` | `{"mode":"orbit","seat":3,"look":[0.2,-0.1],"zoom":-1,"fov":85}` | selects the view and returns transport state. Absolute film seat 0 to 15 |
| `theater.hud` | `{"game":false,"bar":true}` | changes game HUD and Theater bar visibility. Omitted fields keep their values |
| `theater.start_point.capture` | `{"path":"/tmp/a.sp","key":"text","ids":"0a0b"}`; `ids` is optional hex, at most 64 bytes | game agent, at a tick boundary: writes the arena and extension records as a tagged start point; returns `{path, tick, arena_hash, arena_bytes, ext_bytes, stored_bytes}`. Refused with no arena or tick |
| `theater.start_point.load` | `{"path":"/tmp/a.sp","key":"text","film_hash":"<16 hex>"}`; `film_hash` optional | game agent: checks tag, digest, size and hash before touching the game, then returns `{tick, arena_hash, loaded_hash, matches, error?}`. A file that loads but hashes differently reports `matches:false` and the loaded state stays; a refused file changes nothing |
| `theater.status` | `{}` | `{default_recording, recording, in_game, films, last_discard, notes, failure, watch_failure}` on any game or playback agent: the recording state, the film file being written (`null` outside a game), sealed films (oldest first), why the last game went without a film, and the failures. `watch_failure` is why the last watched film stopped early (a start or digest divergence) |
| `theater.measurements` | `{}` | timings and page counts from the instrumented recorder or playback child |
| `theater.verify` | `{"from":1,"to":900}`, both optional | plays the film from its game start in an isolated, unpaced player; returns `{ticks, digests_checked, first_divergence}` |
| `theater.screenshot` | `{"scale":2,"path":"/path/view.png"}`, both optional | `{path, width, height, tick}` for a clean PNG |
| `theater.clip.mark` | `{"in":300}`, `{"out":900}`, or `{"clear":true}` | `{in, out}` with null for each unset mark |
| `theater.clip.export` | `{"from":300,"to":900,"fps":60,"format":"mp4"}`; optional `path`; 30 or 60 fps, `png` or `mp4` | `{job}` once the offline player starts |
| `theater.clip.status` | `{"job":"<job>"}` | `{frames_done, frames_total, path, error, finished}` |

The start point methods are in `crates/extensions/game/src/theater/start_point_agent.rs`
(envelope `start_point.rs`, state `start_point_state.rs`) and serve the
experiments of slice 15 in [theater-design.md](../game/theater-design.md).
Use them on a local game only; a load writes a running match's arena.

Other rules, one line each:

- `theater.verify` bounds are inclusive; a clean run returns
  `first_divergence: null`, a mismatch its tick and reason.
- `theater.screenshot` defaults to scale 1; scales 2 to 4 render larger; the
  default output is a new PNG under the library's `screenshots` directory.
- `theater.clip.export` takes a half-open range (`to` excluded) from boot,
  640x480, `(to - from) * fps / 30` frames, under the library's `exports`
  directory. PNG makes a directory of `frame-000000.png`; MP4 needs `ffmpeg`
  on `PATH` (else the `h2 movies` reason, exit 3) and mixes in 48 kHz stereo.
  Use a fresh path. `finished` is true once published or failed; `error` is
  null on success; the job ID survives seeks.
- `theater.compat` accepts a local film ID or `live:<game-id>`. States are
  `playable` and `unavailable`; `contract` and `hooks` are null with no
  digest. It checks the XBE, the simulation ABI and the game start.
- The rendered player starts at the clip's first tick and loops; its overlay
  drives transport, camera and HUD over the player's socket.

Export, screenshot, compat and start point detail: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Theater transport".

Camera modes: rendered local playback starts in `default`; host playback
starts in `free`. `orbit`, `follow` and `first-person` take a player name or
film seat; a recorded Forge monitor seat can be named too. If a subject has no
recorded viewport, state reports `camera: "follow"` and `camera_fallback:
"no-recorded-viewport"`. `theater.state` and `theater.compat` list
`monitor_looks` (`{seat, map, reason: "monitor_look_source"}`), which never
change `state`.

| Camera field | Meaning |
|---|---|
| `look` | yaw and pitch in radians, free and orbit modes |
| `movement` | strafe, forward, rise in world units, free mode |
| `zoom` | orbit radius in world units |
| `fov` | horizontal field of view at 4:3, 20 to 140 degrees; omitted keeps the lens |

Headless playback (`--render-every 0`) accepts only `default`; playerless
playback has no game HUD.

Transport state carries `id`, `tick`, `length_ticks`, `paused`, `speed`,
`speed_reached`, `seeking`, `compatibility`, `camera`, optional
`camera_player` and `camera_seat`, `hud`, `bar`, `camera_pose` (`position`,
`yaw`, `pitch`, `fov`, `radius`), `kills` and the `at` stamp. Tick zero is
boot. `speed_reached` is ticks per wall second over the film's tick rate,
capped at `speed`; null before a half-second sample or during a seek or step.
Fast-forward checks the film's gameplay digests ([theater.md](../game/theater.md)).

The desktop playback socket also exposes these host page methods, which work
while the guest is paused.

| Method | Parameters | Result |
|---|---|---|
| `ui.screen` | `{}` | `screen` names `theater.playback` or `theater.pause-menu`, with `focus` and the UI-builder `page`. `dialog` names the open picker, or is null. `native` is false |
| `ui.input` | `{"input":"start"}` | Applies one page input and returns the screen. Inputs: `a`, `b`, `x`, `y`, `start`, `back`, `up`, `down`, `left`, `right` |

The `seek` object has `target`, `from_tick`, `source` (`forward` or `boot`)
and `remaining_ticks`; it is `null` in linear playback and stays at the target
with zero ticks left. A replacement target keeps the original `from_tick`.

`theater.measurements` reads on the recorder's or playback child's own socket
when the Theater measurement switch is set ([switches.md](switches.md#tools)).

| Field | Meaning |
|---|---|
| `child_to_first_slice_ms` | child entry to its first guest scheduler slice |
| `boot_to_match_start_ms`, `match_start_tick` | boot to the first recorded match tick; null if never passed |
| `fast_forward_ms`, `fast_forward_ticks` | first slice after boot to the seek target; null until reached |
| `page_windows` | `{seconds, from_tick, to_tick, pages}` distinct guest pages written per 5 s and 30 s window |
| `page_windows_omitted` | entries older than the 256 retained |

```sh
h2 agent theater.state '{}' --socket "$film_run/play.sock"
h2 agent theater.seek '{"tick":30,"source":"boot"}' --socket "$film_run/play.sock"
```

## `anim`: the animation graph walk

Under `impl.sim.native.anim` the animation graph's node walk `1dd1c0` runs as
an arm: the guest answers each call and the Rust result is compared with it
([skeleton-walk.md](../engine/skeleton-walk.md#the-graph-walk-1dd1c0)).
Code: `crates/extensions/agent/src/anim.rs`, wire types in
`crates/extensions/agent-proto/src/anim.rs`.

| Method | Access | Params and result |
|---|---|---|
| `anim.graph_walk` | read | No parameters. Returns `switch`, `addr` (`001dd1c0`), `arm` and `body`. `arm`: `checked`, `matched`, `mismatched`, `dropped`, `declined` by reason (`float`, `fault`, `count`, `queue`, `alias`, `parent`) and `first` (first mismatch: `what`, `plan`, `guest`, `entry`), or absent. `body`: `native`, `nodes`, `declined`, zero while the arm is installed |
| `anim.manager` | read | No parameters. Returns `switch` and `entries`, keyed by the eight-digit guest address of each observed function: `matched`, `differed`, `declined`, `interrupted`, and `first` (`pc`, `what`, `plan`, `guest`) or `null` |

Counters are zero with the switch off; sample before and after a scenario and
subtract. Parity is `mismatched` 0 with `declined` small against `checked`.
See [animation-manager.md](../engine/animation-manager.md).

## `sim.arms`: the native-sim gate read

`sim.arms` reads every native-sim shadow and authority arm in one call, the
instrument that decides whether a sim phase may be on by default
([sim-rewrite.md](../engine/sim-rewrite.md#3-determinism-and-exactness)). It
is answered at a tick boundary from process counters. Code:
`crates/extensions/agent/src/sim_arms.rs`, wire types in
`crates/extensions/agent-proto/src/sim_arms.rs`.

| Method | Access | Params and result |
|---|---|---|
| `sim.arms` | read | No parameters. Returns `subsystems`: `physics`, `objects`, `anim`, `collision`, `seats`, `effects`, in that order. |

Each subsystem has `name`, `switch` (`impl.sim.native.<name>`), `on`, `arms`
and `clean` (`checked > 0 && mismatched == 0`, summed). Each arm has `name`,
`checked`, `matched`, `mismatched`, `answered` and `declined`. A source
without a counter reports 0; a subsystem with its switch off reads zeros, so
`clean` is false. Source counters and the zero mapping: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "`sim.arms`".

Run the scenario, read `sim.arms`, and flip a phase on only when its
subsystem is `clean` with `checked` large enough.

## `collision`: the native collision world

The collision switch counts admitted native answers, declined queries,
floating-point environments and comparisons with lifted returns, read at a
tick boundary. See the
[entry addresses and admission rules](../engine/collision-queries.md).

| Method | Access | Params and result |
|---|---|---|
| `collision.stats` | read | No parameters. Returns `switch`, `entries`, `decodes`, `decode_failures`, `flushes`, `bsps`, `mopps`, `bytes`, `array_bytes`, `budget_bytes`, `ring_fallbacks`, `cluster_budget_reads`, `precision` (x87 precision calls at C and D), `vector` (C), `context` (D), `instance`, `flood`, `visit`, `query` (E, G, H, J) and `ray` (I). |
| `collision.trace` | read | `{start, delta, flags}`: `start` and `delta` are three finite binary32 world coordinates, `flags` an unsigned 32-bit query mask. |
| `collision.sources` | read | No parameters. Checks the source fingerprints of each resident native BSP and MOPP against guest RAM. |
| `collision.sources.clear` | claim | No parameters. Returns the same check and drops all resident entries at that tick boundary. |

`collision.trace` evaluates entry J on the active structure over the segment
`start` to `start + delta`. It returns the tick stamp, `hit_type` (0 for a
miss), `fraction`, `position`, optional `material` and `plane`, first and last
`locations` (`{leaf, cluster, bsp}`) and the `cluster_marks`,
`instance_marks` and `object_marks` in traversal order. Guest memory is
untouched. A path outside the native kernel returns `UNAVAILABLE` with the
bail name in `error.data.reason`.

Each `entries` row has `name`, eight-digit guest `addr`, `native` answers,
`bails` by reason, and `mxcsr` and `x87_control` frequency maps (lowercase
hex keys, eight and four digits). `budget_bytes` caps `bytes` at 8 MiB per
loaded map. Sample before and after a scenario and subtract.

| Group | Fields |
|---|---|
| `vector`, `context` | `matched`, `mismatched`, `unverified` by bail reason, `first_mismatch` (C: AL or first differing byte; D: AL, R and the 40-byte context) |
| `instance`, `flood`, `visit`, `query` | same counters; a declined prediction is `unverified`; J compares AL, flags, result and mark tables |
| `ray` | `matched`, `mismatched`, `first_mismatch`, `unverified` by bail reason, `fraction_moved` |

The bail audit (under 0.1% of calls bailing) reads `bails` against `native`.
A reason that appears on one architecture only is a determinism bug. Field
semantics, byte accounting and examples: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "`collision`".

### Checking a cold collision world

`collision.sources` returns `sources` sorted by `kind` and guest `address`
with `cached` and `live` 64-bit hex fingerprints, `bail` when a live read
fails (`live: null`), and separate `mismatched` and `unreadable` counts. A
debugger memory trap makes it unavailable. `collision.sources.clear` is the
cache-cold form for the parity arm; start each arm from the same state with
`impl.sim.native.collision` and `impl.sim.arena-hash` on, one claimed
connection per arm. The 1,800-tick capture script and comparison steps: see
the [detail note](notes/2026-11-26-agent-api-detail.md), "Checking a cold
collision world".

## Tick traces and the flight recorder

`trace.*` and `flight.*` take one row per tick inside the game, in the tick
observer, before queued requests are served (polling `world.object` drops
ticks). Code: `crates/extensions/agent/src/trace.rs` (tests `trace/tests.rs`),
wire types and `show` in `crates/extensions/agent-proto/src/trace.rs`, CLI in
`crates/apps/cli/src/agent_flight.rs`, readback in
`crates/extensions/agent/src/outputs.rs`.

Files go under `traces/`, `flights/` and `inputs/` in the
[agent output root](switches.md#tools) (the cache directory by default; for a
[lab job](lab.md#runs), `runs/<id>`, included by `h2 lab fetch`).

```sh
h2 agent trace start player.0 physics,pos          # rows to traces/trace-<ms>.jsonl
h2 agent trace stop                                # rows, bytes, first/last tick
h2 agent flight start player.0 physics,pos,velocity 20
h2 agent flight dump crouch-bug.jsonl              # flights/crouch-bug.jsonl
h2 agent trace.list                                # saved trace and flight file names
h2 agent flight show crouch-bug.jsonl --remote --field physics.crouch
```

- **Rows.** One JSON line per tick: `{tick, ticks, map, objects: {id:
  Object}, gone, error}`. `objects` holds each asked-for id trimmed to
  `fields` (`world.query` names; `id` and `kind` kept); absent ids are in
  `gone`. A paused game writes no rows; each `session.step` tick does.
- **`trace.start {objects, fields?, path?}`.** 1 to 64 ids, streamed to a bare
  file name in `traces/` (default `trace-<ms>.jsonl`, replaced if present).
  `trace.stop` and `trace.status` give `rows`, `bytes`, `first_tick`,
  `last_tick`, `path`, `error`. One trace at a time.
- **`flight.start {objects, fields?, seconds?}`.** Keeps the last `seconds`
  (default 30, 1 to 600) x 30 rows, at most 64 MiB. Each row also has `input`
  (the `XInputGetState` results since the last tick, at most 64; absent
  without XAPI input). `flight.dump {path?}` writes to `flights/` (default
  `flight-<ms>.jsonl`) and keeps recording; `flight.stop` drops the ring;
  `flight.status` gives `capacity`, `rows`, `bytes`, `ticks`, tick range.
- **`trace.list`** returns `{traces, flights}`, sorted file names.
- **`trace.read {path, kind?, from_tick?, to_tick?, offset?, limit?}`**
  returns `{rows, next}`. `kind` is `trace` or `flight` (default: `traces/`
  then `flights/`); tick bounds inclusive; `limit` default 64, maximum 256.
  Page with the returned byte cursor until `next` is `null`.
- **Scrub.** `h2 agent flight show <file>` reads a local file, or the game
  socket with `--remote` or a missing file; `--kind`, `--field`, `--from`,
  `--to` select. To re-simulate a stretch, start from a
  [snapshot](#state-machine-snapshots).
- **Access.** No claim; answered between slices as well as at tick boundaries.

Details of the per-row fields: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Tick traces and the flight recorder".

## Shadow checks

Design: [shadow-exec.md](../engine/shadow-exec.md). Host methods
(`agent::shadow`) over `feature::shadow`'s registry. A native sim port is
checked against the lifted original on its own copy of guest memory. Only
installed ports are checked; `shadow.enable` is the only switch for checks.
Select ports by switch name (`sim.native_unit_pose`), object point name
(`OBJECT_FUNCTION_VALUE`), group (`sim.native`, `sim.physics`,
`sim.native.objects`) or `all`.

| Method | Access | What |
|---|---|---|
| `shadow.enable` | act | `{ports, every?, step_limit?}`: checks on, counts reset. `every` (default 1) checks one call in that many; `step_limit` (default 5,000,000) caps the original's instructions per check. The first call builds the lifted original (a few seconds). → `{enabled, not_installed, every, step_limit}`. Unknown names: invalid params; no image: unavailable |
| `shadow.disable` | act | `{ports?}`: checks off (none: all). → `{disabled}`. Counts stay |
| `shadow.status` | read | `{on, ready, every, step_limit, calls, checked, matched, mismatched, skipped, ports, mismatches, mismatches_dropped}`. Each port: `name`, `group`, `point`, `function`, `installed`, `on`, `calls`, `checked`, `matched`, `mismatched`, `skipped`, `skip_reasons`. Each mismatch (64 kept): `port`, `group`, `point`, `function`, `pc`, `tick`, `call`, `kind` (`transfer`, `registers`, `memory`, `returned`, `nondeterministic`), `summary`, `transfer`, `registers`, `memory`, `memory_bytes`, `steps`, `entry` |

```sh
H2_FEATURES=impl.sim.native_unit_pose h2 serve ...
h2 agent shadow.enable '{"ports":["sim.native_unit_pose"]}'
h2 agent session.wait '{"until":{"ticks":300}}'
h2 agent shadow.status
```

`scripts/shadow-gate.sh` is the scripted disc gate over these methods: a
headless sim match, checks on for N ticks, a failure on any mismatch
([shadow-exec.md](../engine/shadow-exec.md#the-disc-gate)).

## Scripted scenarios with a verdict

```sh
cat > grenade.txt <<'X'
wait {"phase":"gameplay","map_ticks":30}
expect /at/phase == "gameplay"
claim
press rt 100
capture 90
expect /over_budget <= 2
release
X
h2 agent run grenade.txt          # one JSON verdict; exit 1 unless it passed
```

`h2 agent run <script|->` (`crates/apps/cli/src/agent_run.rs`) is the `h2
agent -` script on one connection, plus `expect <pointer> <op> [JSON]` on the
last answer (JSON pointer; ops `==`, `!=`, `<`, `<=`, `>`, `>=`, `contains`,
`exists`, `missing`, `near {"to", "within"}`) and `keep` (put the last answer
in the verdict). Expectations are `agent_proto::expect`, which `sim.expect`
judges in the game too. The verdict is `{"pass", "failed", "ms", "calls":
[{line, call, ms, ok, error}], "expects": [{line, expect, want, pass, got}],
"kept", "stopped"}`. A failed expect is recorded and the script goes on; a
failed call stops it. Only kept answers are printed.

## Test maps

Purpose-built test maps from recipes (a spawn grid, a weapon range, the
superbounce patch), built from the user's disc while the game runs and queued
as the next game. Design and recipes: [test-maps.md](../game/test-maps.md).
Host methods in `crates/extensions/game/src/test_maps.rs` (the `test-maps`
feature of `boot`, recipes in `crates/apps/boot/src/test_maps_recipes.rs`);
wire types `agent_proto::test_maps`.

| Method | Access | Params and result |
|---|---|---|
| `maps.load` | act | `{map_id}` → `{name, map {id, path}, check, queued, expect}`. Queues a local Slayer game on the installed multiplayer package with this full hash. An unknown id reinstalls the offers and reloads the VFS packages before refusing ([the tag workspace](../editor/tag-workspace.md)). Wait for gameplay on the returned path's basename. |
| `maps.test.list` | read | `{name?}` → `{recipes, maps}`: recipes (`name`, `title`, `base`, `fidelity`, `purpose`, `note`) and test maps (`name`, `base`, `state` `building`/`ready`/`failed`, `dir`, `error`, `map_id`, `listed {id, path}`, `built` with `anchor`, `placed`, `counts`, `notes`, `expect`) |
| `maps.test.create` | act | `{recipe: <built-in name> \| <recipe object>, base?, replace?}` → `{name, state: "building", dir}` at once; the build runs on its own thread. `base` builds on another disc map (`<recipe>-<base>`) |
| `maps.test.load` | act | `{name}` → after the next tick, `{name, map {id, path}, check, queued, expect}`: a local Slayer game queued through the `agent-calls` feature (`Call::QueueGame`). Wait with `session.wait` or `h2 agent maps load` |
| `maps.test.remove` | act | `{name}` → `{name, removed}` |

`map.reload {}` queues the active campaign or multiplayer map again and
returns `{queued, map, activation}` (the map name, the activation reserved for
this request and the executor's tick stamp). It follows the
[object edit session rules and hooks](#object-edits); film and input recording
or playback refuse it. `Sim::reload_map`
(`crates/extensions/agent/src/sim/edits.rs`) copies the active options block
into `Call::QueueGame`'s scratch, keeping scenario path, players, variant and
difficulty; `OPTIONS_QUEUE` takes the scratch address in EAX (see
[level-boot.md](level-boot.md#addresses)). Pass the returned `activation` and
`map` to `session.wait` with `phase: "gameplay"` and `map_ticks: 1`, for
example `{"until":{"activation":4,"map":"lockout","phase":"gameplay","map_ticks":1}}`.
A checkpoint revert cannot publish a queued reload's activation.

`map.reload {"file":"/absolute/path/<map>.map"}` loads a play copy for the
active DVD map. The filename and the cache-header map name must match the
active name (ASCII case ignored). A worker prepares a private generation and
checks cache type, slot capacity, tag index and retail payload checksum; at
the tick boundary the agent rechecks the session and source, reserves the DVD
file and retires the old cache keys before the guest's options queue. The VFS
publishes the copy when the guest accepts the transition, else the host
restores the keys. The result adds `file` and the numeric `checksum`; after
the activation wait, check that `live.status.map.checksum` or
`edit.map.checksum` matches. Both forms accept `confirm_peers` on a System
Link host (total includes the host; each peer needs its own file). A
writable game with the agent enabled can accept a play copy without an
override directory. Preparation and rollback steps: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Test maps".

Forge's **Rebuild map** and the reload after **Build AI ground** run the same
reload (`agent::sim::MapReloader`, calling `Sim::start_reload`), with the same
refusals ([forge-design.md](../game/forge-design.md), "Rebuild map").

A light host, a server-map mount, a scenario loaded from a content path, or a
host without the editor map mount returns `REFUSED` with `data.why`. `--film`,
film playback and external Live sign-in also exclude file binding. A session's
[default recording](../game/film-format.md#recording-every-game) allows it: the
first authorized editor write discards the game in play's film. After a
binding succeeds, restart before signing in to external Live. Machine
snapshots and snapshot-based rewind are unavailable while a play copy is
bound. See [Map overrides](storage.md#map-overrides) and
[Changed maps](cache-slots.md#changed-maps).

```sh
h2 agent maps                          # recipes and test maps
h2 agent maps create superbounce       # waits for the build
h2 agent maps load superbounce         # waits until the new map is in gameplay
```

`h2 agent maps` (`crates/apps/cli/src/agent_maps.rs`) reads a recipe file
(`.toml` or `.json`) on the client and sends it as the recipe object; its
verbs work in `h2 agent -` and `h2 agent run` scripts.

### Test maps on disk

`h2 testmap <source.map> <script.json | -> --out <new.map> [--force]`
(`crates/apps/h2-tools/src/testmap_cmd.rs`) is a host-only tool that runs a
JSON edit script (`scenario_edit::script`,
`crates/tools/scenario-edit/src/script.rs`) over a map's placement blocks
through the editor's `Session` and saves a new map; the source is never
written. Operations: `clear`, `keep`, `add` (`like` or palette first item,
then `set`), `set`, `remove`; `allow_growth` lets a block grow. Blocks:
`scenery`, `vehicles`, `equipment`, `weapons`, `machines`, `sound_scenery`,
`crates`, `starting_locations`, `netgame_flags`, `netgame_equipment`,
`starting_equipment`. AI and non-palette tag references are not covered. The
user or coordinator loads the result (`H2_MAP_DIR`); agents do not.

## Content packages

The game's content packages: ours baked into `h2` from `content/<id>/`, then
the user's under `<data>/packages/<id>/`, ordered by
`<data>/packages/set.toml` and the game's features
([studio-redesign.md](../editor/studio-redesign.md), "How the game consumes
packages"). Registered by `crates/apps/boot/src/packages.rs`; set and rows are
`workspace::GameSet`, maps `workspace::PackageRewrite`'s record.

| Method | Access | Params and result |
|---|---|---|
| `packages.list` | read | `{}` → `{packages, problems, content_hash, local_only, maps}` |

| Field | Contents |
|---|---|
| `packages` | every package read, on in load order then off by id: `id`, `name`, `package_id`, `origin` (`baked`/`local`), `dir`, `on`, `why` when off, `always_on` (`core`, `ui`), `signed`, `features`, `items`, `scopes` (`sim`, `client`, `live`, `per-feature`) |
| `problems` | boot's lines: unreadable package, a local package with a baked id, an unused `set.toml`, a cycle |
| `content_hash` | SHA-256 of the packages on with a `sim` item; `null` if the set did not order |
| `local_only` | why a Live or System Link session would be refused (an unsigned `sim` package), or `null` |
| `maps` | by map name: `scope`, `packages`, `held`, `refused` (`package`, `why`), `key`, `hits`, `builds`, `added`, `changed`, `error` |

Reasons for `why` and `refused`: see the
[detail note](notes/2026-11-26-agent-api-detail.md), "Content packages".

## The tag store

The tag store at `<data>/tags` and the maps it gives back byte for byte from
skeletons and extent tables ([asset-store.md](../engine/asset-store.md)). With
`store-maps` on, the game reads the disc's maps from it, never the disc.
Registered by `crates/apps/boot/src/store_status.rs`; maps are
`vfs::store_map::status`.

| Method | Access | Params and result |
|---|---|---|
| `store.status` | read | `{}` → `{root, exists, served, maps, stale, broken, images}` |

| Field | Contents |
|---|---|
| `root`, `exists` | the store's directory; whether it is a tag repository |
| `served` | whether this boot serves the disc's maps from it (`store-maps`) |
| `maps` | `map`, `length`, `sha256`, `skeleton_bytes`, `extents`, `extent_bytes` |
| `stale` | maps `extract.toml` records with no skeleton or table (`h2 tags extract --update`) |
| `broken` | maps whose extent table does not read, with `why` |
| `images` | level images under `<data>/store/images`: `level_id`, `length`, `sha256`, `disk_bytes`, `extents`, `extent_bytes` |

## Rules and limits

- Agents attach to a running game; a client never builds, relaunches or
  changes `H2_OPT`. A client that needs new code writes a test instead.
- Answers are small by default: counts first, details by id, paging
  everywhere.
- World reads happen between guest ticks inside a hook on the game's own
  thread, so a reply is one consistent moment. Reads from Rust are
  uncharged and invisible to the guest.
- Hooks on shared per-tick entry points need an explicit order, or the
  feature plan refuses to build; the `agent` feature declares its order
  against `lockstep`, co-op and telemetry.
- Text resolved from the user's disc (tag names, strings) is returned to
  the client and never written into the repository; tests use made-up
  names.
- Snapshots, captures, input logs, traces and flights live in the cache
  directory (`agent_proto::cache_dir`), never in the repository.
- Methods a table names but this build does not register (an `input.*`
  method without XAPI input, `audio.voices` without DirectSound,
  `host.lobby.*` without the light host) are absent from `session.methods`.

## Evidence

Statements are **Code** (read from this repository, the path given) unless
labelled. Guest addresses are **E** (read from the retail XBE with `h2
disasm`) and are research points, not data. Tests that cover the page:
`crates/extensions/agent/src/exec/tests.rs`, `sim/tests.rs`, `trace/tests.rs`,
`havok.rs`, `raw.rs`, `crates/extensions/agent-proto` (wire types),
`crates/apps/boot/tests/feature_plan.rs` (observer order),
`crates/extensions/agent/src/render/picture/tests.rs` and `exec/picture.rs`.
## Not verified

Nothing here was checked by running the game unless a section says so; the
tests use fake readers, synthetic memory, pads and frames. Unchecked against a
live game: `ui.audit`, `ui.press` and the menu crawl; `input.route`; `state.*`;
`sim.give`, `sim.spawn` and teleport; lobby steps `invite` and `start`;
`render.*`, `load.timeline`, `world.raw`, `havok.*`, `sim.hashes` and the
`trace`/`flight` recorder; `shadow.*` and `scripts/shadow-gate.sh` against the
disc; `maps.test.*` ([test-maps.md](../game/test-maps.md#not-verified)).
Not provided: effect, projectile and particle counts; per-subsystem guest
time; owner attribution of frame cost; kernel I/O seeks and cache hits in the
load timeline; a widget-level HUD; setting a feature switch through the API;
loading a named level other than a test map.

## Behaviour probes

**Built.** A socket-enabled serve registers these four game methods under
`agent-probes` (wire types `crates/extensions/agent-proto/src/probe.rs`,
code `crates/extensions/agent/src/probe/`). The three acting methods need a
writable local or host session and a ticking game; use one controller
connection for a multi-call experiment. Objects are full eight-digit hex
datums, as returned by `sim.give`, `sim.spawn` or a player's `world.query` row.

| Method | Request and result |
|---|---|
| `probe.weapon` | `{"player":0,"object":"e1230042","ticks":60,"steps":[{"rt":1,"ticks":55},{"ticks":5}]}`; give the weapon with `sim.give` first. Returns an initial sample and a row per tick: loaded/reserve counts, magazine state, heat, and `shots` (barrel events with projectile datum, definition, cone, `bloom`, direction, deflection). `"buttons":["x"]` reloads |
| `probe.animation` | `{"unit":"e1230043","path":["combat","rifle","ar","fire_1"],"overlay":false}` (mode, class, weapon type, action). Returns graph datum and name, selected string IDs and names, inherited graph index, animation index, clip name and frame count. Does not change the playing channel |
| `probe.hit` | `{"projectile":"proj:weapons/assault rifle/bullet","target":"e1230043","from":[3,0,1],"to":[2,0,1],"ticks":12}`; optional `node`, `shield`. Returns the first collision (kind, position, object datum, region, node, material index), vitality `before` and `after` as [body, shield], and `damage` reports. A miss through the window is an error; the world is changed |
| `probe.round` | `{}` → round-start tick, per-player first applied control and first movement/fire ticks, observation error. Capture starts at map activation. Join indices to `sim.player.list.clankers` for bots |

Weapon and hit captures share one slot, window 1 to 290 ticks. A changed
weapon, missing object, unreadable fields or discontinuous trace fails the
request. Machine snapshots preserve round history and refuse an in-progress
capture. Hook addresses: barrel fire `0x104150`, launch `0x1052c5`,
projectile collision `0xfaa60`, animation selectors `0x1db170` and
`0x1dbc80` (entry resolution `0x1daea0`), round observer callers of
`0xc6de0`. None of the probes requires rendering. The
[Halo 1 parity suite](../halo1/parity.md) drives them against converted Blood
Gulch and a30.

## Related

- [feature-engine.md](../engine/feature-engine.md): switches, hooks, native UI screens
- [high-framerate.md](../game/high-framerate.md): `lockstep` and `xapi::tap`
- [profiling.md](../perf/profiling.md), [gameplay-budget.md](../perf/notes/gameplay-budget.md)
- [level-boot.md](level-boot.md), [checkpoints.md](checkpoints.md), [snapshots.md](snapshots.md)
- [lab.md](lab.md): `h2 lab`, runs on a second Mac over SSH with replays and frame stats
- [hot-iteration.md](../graphics/hot-iteration.md): `edit.*` and hot `render.*`
- [debug-menu.md](debug-menu.md): the in-game debug menu, an in-process client of this API
- [sim.md](sim.md): sim mode and the match report
- [docs/live](../live/): the Live service, the other reader of `live-telemetry`
