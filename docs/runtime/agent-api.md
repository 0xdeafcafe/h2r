# Agent API: driving the running game without pixels

Status: Built. Every method below is registered by the code; the ones no run has exercised are named under "Not verified".
Code: `crates/extensions/agent-proto` (wire types, blocking client), `crates/extensions/agent` (server, tick-boundary executor, most methods), `crates/extensions/game/src/agent_glue.rs` (the methods the host answers), `crates/apps/cli/src/agent_cmd.rs` (`h2 agent`). Tests: `crates/extensions/agent/src/exec/tests.rs`, `crates/extensions/agent/src/sim/tests.rs`, `crates/extensions/agent/src/trace/tests.rs`, `crates/apps/boot/tests/feature_plan.rs`.
Switch: `agent` feature (`H2_FEATURES=-agent` removes the tick hooks), `H2_AGENT` (the socket), listed in [switches.md](switches.md).

An agent, a script or a test reads and drives a running Halo 2 through one
JSON-RPC interface: what is in the level, where frame time goes, what the
menus show, how to put the game in a state and act in it. It replaces
screenshots, pasted logs and environment variables read at launch. The
command-line client is `h2 agent`. The editor connects to the same endpoint.

Reads of game state run on the game's thread at a boundary. The `at`
stamp identifies that moment. Independent host reads such as `host.memory`
use their own sample clock and carry the game stamp at reply time.

## Method index

`session.methods` lists what the running process registers; a host method
appears only where the host adds it. Access (`agent::Access`): **read**
answers any connection; **act** answers any connection while none holds
control, else only the controller (`NOT_CONTROLLER`); **claim** needs this
connection to hold `session.claim`.

| Method | Access | Section |
|---|---|---|
| `session.status`, `methods`, `claim`, `release`, `wait`, `subscribe` | read | [`session`](agent-api-session.md#session-what-is-running-and-its-clock) |
| `session.takeover_info`, `takeover_status` | read | [Lab consent](agent-api-session.md#lab-consent) |
| `session.request_takeover` | act | [Lab consent](agent-api-session.md#lab-consent) |
| `live.subscribe`, `live.status`, `live.close` | read | [Live mirror](agent-api-session.md#live-mirror) |
| `session.pause`, `resume`, `step` | claim | [Control](agent-api-session.md#control) |
| `host.pause`, `host.step` | act, without the controller check | [Window pause and frame step](agent-api-session.md#window-pause-and-frame-step) |
| `host.screenshot` | act, without the controller check | [Window screenshots](agent-api-session.md#window-screenshots) |
| `host.quit` | act, without the controller check | [Close the window](agent-api-session.md#close-the-window) |
| `host.window.resize` | act, without the controller check | [Resize the window](agent-api-session.md#resize-the-window) |
| `world.count`, `world.query`, `world.object` | read | [`world`](agent-api-world.md#world-what-is-in-the-level) |
| `world.object.set`, `world.object.delete`, `world.object.respawn` | act | [Object edits](agent-api-sim.md#object-edits) |
| `world.object.animate` | act | [`world.object.animate`](agent-api-input.md#worldobjectanimate) |
| `world.placement.edit` | act | [Placement edits](agent-api-sim.md#placement-edits) |
| `world.ai` | read | [`world.ai`](agent-api-world.md#worldai) |
| `world.pools` | read | [`world.pools`](agent-api-world.md#worldpools) |
| `world.breakable` | read | [`world.breakable`](agent-api-world.md#worldbreakable-broken-glass) |
| `net.replication` | read | [`net.replication`](agent-api-world.md#netreplication) |
| `actors.net` | read | [`actors.net`](agent-api-world.md#actorsnet) |
| `world.raw` | read | [`world.raw`](agent-api-world.md#worldraw-an-objects-bytes-before-and-after) |
| `world.poke` | claim | [`world.poke`](agent-api-world.md#worldpoke-replace-bytes-in-one-object-part) |
| `havok.bodies`, `havok.agents`, `havok.contacts`, `havok.digest`, `havok.islands` | read | [`havok`](agent-api-world.md#havok-the-physics-world) |
| `havok.pair.set` | claim | [`havok.pair.set`](agent-api-world.md#havokpairset) |
| `superbounce.state` | read | [`superbounce`](agent-api-world.md#superbounce-the-superbounce-game-type) |
| `superbounce.set` | act | [`superbounce`](agent-api-world.md#superbounce-the-superbounce-game-type) |
| `superbounce.reload`, `superbounce.tune` | act | [`superbounce`](agent-api-world.md#superbounce-the-superbounce-game-type) |
| `superbounce.medal_test` | act | [`superbounce`](agent-api-world.md#superbounce-the-superbounce-game-type) |
| `superbounce.assist` | read | [`superbounce`](agent-api-world.md#superbounce-the-superbounce-game-type) |
| `superbounce.assist_test` | act | [`superbounce`](agent-api-world.md#superbounce-the-superbounce-game-type) |
| `superbounce.verify`, `superbounce.verify_cancel` | act | [`superbounce`](agent-api-world.md#superbounce-the-superbounce-game-type) |
| `superbounce.verify_status` | read | [`superbounce`](agent-api-world.md#superbounce-the-superbounce-game-type) |
| `superbounce.view` | read | [`superbounce`](agent-api-world.md#superbounce-the-superbounce-game-type) |
| `superbounce.view_test` | act | [`superbounce`](agent-api-world.md#superbounce-the-superbounce-game-type) |
| `superbounce.ghosts` | read | [`superbounce`](agent-api-world.md#superbounce-the-superbounce-game-type) |
| `superbounce.ghost_show` | act | [`superbounce`](agent-api-world.md#superbounce-the-superbounce-game-type) |
| `pogo.state` | read | [`pogo`](agent-api-world.md#pogo-superbounces-pogo-stick) |
| `pogo.test` | act | [`pogo`](agent-api-world.md#pogo-superbounces-pogo-stick) |
| `profile.counters`, `profile.capture`, `profile.fallbacks`, `profile.memory`, `profile.faults` | read | [`profile`](agent-api-render.md#profile-where-a-frame-goes) |
| `profile.sample.start`, `profile.sample.stop` | act | [`profile`](agent-api-render.md#profile-where-a-frame-goes) |
| `profile.sample.status` | read | [`profile`](agent-api-render.md#profile-where-a-frame-goes) |
| `render.frame`, `render.stats` | read | [`render`](agent-api-render.md#render-the-frame-and-the-pipeline) |
| `render.marks` | read | [`render.marks`](agent-api-render.md#rendermarks) |
| `render.markers` | read | [`render.markers`](agent-api-render.md#rendermarkers) |
| `render.frame_layout` | read | [Display raster and texture coordinates](agent-api-render.md#renderframe_layout) |
| `render.capture_clip`, `render.capture_clip.status` | act, read | [Headless display clips](agent-api-render.md#rendercapture_clip) |
| `render.picture.open`, `render.picture.close` | read | [Picture feed](agent-api-render.md#picture-feed-renderpicture) |
| `render.coop_look` | read | [`render`](agent-api-render.md#render-the-frame-and-the-pipeline) |
| `render.compare`, `render.capture`, `render.scene`, `render.view.current`, `sim.hashes` | read | [FrameScene checks](agent-api-render-checks.md#framescene-checks-compare-capture-scene-parity-hashes) |
| `render.spawn_view`, `render.view` | act | [FrameScene checks](agent-api-render-checks.md#framescene-checks-compare-capture-scene-parity-hashes) |
| `render.parity` | act | [FrameScene checks](agent-api-render-checks.md#framescene-checks-compare-capture-scene-parity-hashes) |
| `render.particles.refusals` | read | [`render.particles.refusals`](agent-api-render.md#renderparticlesrefusals) |
| `render.overrides`, `render.programs` | read | [`edit` and hot `render`](agent-api-edit.md#edit-and-hot-render-live-tag-and-shader-changes) |
| `render.reload`, `render.dump`, `render.param`, `render.toggle` | act | [`edit` and hot `render`](agent-api-edit.md#edit-and-hot-render-live-tag-and-shader-changes) |
| `edit.map`, `edit.tags`, `edit.get`, `edit.changes` | read | [`edit` and hot `render`](agent-api-edit.md#edit-and-hot-render-live-tag-and-shader-changes) |
| `edit.set`, `edit.batch`, `edit.revert` | act | [`edit` and hot `render`](agent-api-edit.md#edit-and-hot-render-live-tag-and-shader-changes) |
| `edit.arena` | read | [Block moves](agent-api-edit.md#block-moves) |
| `edit.collision.stage`, `edit.collision.commit` | act | [Collision edits](agent-api-edit.md#collision-edits) |
| `load.timeline` | read | [Load timeline](agent-api-host.md#load-timeline) |
| `game.menu` | act | [Returning to the main menu](agent-api-host.md#returning-to-the-main-menu) |
| `game.load`, `game.levels` | act, read | [Loading a level](agent-api-host.md#loading-a-level) |
| `game.code`, `game.code.restart` | read, act | [Game code](agent-api-host.md#game-code-full-speed-or-the-slower-start) |
| `features.list` | read | [`session`](agent-api-session.md#session-what-is-running-and-its-clock) |
| `live.join.open`, `live.join.status` | act, read | [Live session links](agent-api-host.md#live-session-links) |
| `live.matchmaking.trace` | read | [Matchmaking transitions](agent-api-host.md#matchmaking-transitions) |
| `host.metrics`, `host.memory`, `host.audit`, `host.perf` | read | [`host`](agent-api-host.md#host-the-process-and-its-lobby) |
| `host.memory.history`, `heap`, `gpu`, `diff`, `postmortem` | read | [Finding a leak after the fact](agent-api-host.md#finding-a-leak-after-the-fact) |
| `host.lobby.open`, `close`, `start` | act | [`host.lobby`](agent-api-host.md#hostlobby) |
| `host.lobby.state` | read | [`host.lobby`](agent-api-host.md#hostlobby) |
| `ui.screen`, `ui.campaign`, `ui.audit`, `ui.stack`, `ui.lobby.players`, `ui.lobby.tabs`, `ui.browser.filters`, `ui.attract` | read | [`ui`](agent-api-host.md#ui-what-the-menus-show) |
| `ui.press` | act | [Menu inspection and navigation](agent-api-host.md#menu-inspection-and-navigation) |
| `audio.voices`, `audio.listeners`, `audio.loops` | read | [`audio`](agent-api-input.md#audio-voice-routing) |
| `audio.meter` | read | [`audio.meter`](agent-api-input.md#audiometer) |
| `audio.play` | act | [`audio.play`](agent-api-input.md#audioplay) |
| `recording.play` | act | [`recording.play`](agent-api-input.md#recordingplay) |
| `hs.threads`, `hs.globals`, `hs.player_actions` | read | [HaloScript state](agent-api-input.md#haloscript-state) |
| `hs.eval` | read | [`hs.eval`](agent-api-input.md#hseval) |
| `input.pad`, `release`, `record`, `replay`, `stop` | act | [`input`](agent-api-input.md#input-act-like-a-player) |
| `input.state`, `input.host` | read | [`input`](agent-api-input.md#input-act-like-a-player) |
| `input.route`, `input.route.stop` | act | [Routes](agent-api-input.md#routes) |
| `input.route.status` | read | [Routes](agent-api-input.md#routes) |
| `state.save`, `load`, `list`, `drop`, `fork` | act (`list` read) | [`state`](agent-api-input.md#state-machine-snapshots) |
| `bots.stats` | read | [Bot combat](agent-api-sim.md#bot-combat) |
| `bots.bounce_to` | act | [Bounce routes](agent-api-sim.md#bounce-routes) |
| `bots.bounce_status`, `superbounce.spots` | read | [Bounce routes](agent-api-sim.md#bounce-routes) |
| `player.goto`, `player.goto_cancel` | act | [`player.goto`](agent-api-sim.md#playergoto-drive-the-player-somewhere) |
| `player.goto_status` | read | [`player.goto`](agent-api-sim.md#playergoto-drive-the-player-somewhere) |
| `player.bot` | act | [`player.bot`](agent-api-sim.md#playerbot-a-clanker-plays-the-players-seat) |
| `player.mark`, `player.recall` | act | [Saved places](agent-api-sim.md#saved-places-playermark-and-playerrecall) |
| `sim.*` | read or act | [`sim`](agent-api-sim.md#sim-simulated-players-and-direct-state) |
| `forge.status`, `forge.things`, `forge.doc`, `forge.log`, `forge.propose`, `forge.input`, `forge.look`, `forge.look.set`, `forge.status.monitors`, `forge.ground`, `forge.ground.build`, `forge.nav`, `forge.waves`, `forge.waves.test`, `forge.pieces` | read or act | [Local Forge session](agent-api-modes.md#local-forge-session) |
| `forge.capacity`, `forge.probe` | read, act | [Forge probes](agent-api-modes.md#forge-probes) |
| `firefight.state`, `firefight.kit` | read | [Firefight](agent-api-modes.md#firefight) |
| `firefight.command` | act | [Firefight](agent-api-modes.md#firefight) |
| `rules.state`, `rules.zones`, `rules.reload` | read, act | [Rules scripts](agent-api-modes.md#rules-scripts) |
| `variants.quick`, `variants.quick.set` | read, act | [Game variants](agent-api-modes.md#game-variants) |
| `match.status`, `match.report`, `match.stop`, `sim.guest_census` | read, read, act, read | [Sim matches](agent-api-modes.md#sim-matches) |
| `debug.views`, `debug.view` | read, act | [World debug views](agent-api-edit.md#world-debug-views) |
| `settings.list`, `settings.set`, `perf.overlay`, `cheat.list`, `cheat.set` | read or act | [Menu methods](agent-api-edit.md#menu-methods-settings-perf-overlay-cheats) |
| `editor.link`, `editor.focus` | read, act | [Paired editor](agent-api-edit.md#editor-a-paired-editor) |
| `editor.marks` | read | [Paired editor](agent-api-edit.md#editor-a-paired-editor) |
| `theater.*` | read or act | [Theater transport](agent-api-modes.md#theater-transport) |
| `collision.stats` | read | [`collision`](agent-api-checks.md#collision-the-native-collision-world) |
| `collision.trace` | read | [`collision`](agent-api-checks.md#collision-the-native-collision-world) |
| `collision.sources`, `collision.sources.clear` | read, claim | [`collision`](agent-api-checks.md#collision-the-native-collision-world) |
| `anim.graph_walk`, `anim.manager` | read | [`anim`](agent-api-checks.md#anim-the-animation-graph-walk) |
| `sim.arms` | read | [`sim.arms`](agent-api-checks.md#simarms-the-native-sim-gate-read) |
| `input.list`, `input.log.read` | read | [Input logs](agent-api-input.md#input-act-like-a-player) |
| `trace.*`, `flight.*` | read | [Tick traces and the flight recorder](agent-api-checks.md#tick-traces-and-the-flight-recorder) |
| `shadow.enable`, `shadow.disable`, `shadow.status` | act, act, read | [Shadow checks](agent-api-checks.md#shadow-checks) |
| `map.reload` | act | [Test maps](agent-api-checks.md#test-maps) |
| `maps.load`, `maps.test.list`, `create`, `load`, `remove` | act, read, act, act, act | [Test maps](agent-api-checks.md#test-maps) |
| `packages.list` | read | [Content packages](agent-api-checks.md#content-packages) |
| `store.status` | read | [The tag store](agent-api-checks.md#the-tag-store) |

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
  (`H2_BOOT_REALTIME=1`); it is the shared instance agents use. `h2 play`
  with `H2_AGENT` set serves the same API with a window, which is what
  `h2 lab run --serve` and `h2 lab bench` launch. Frame cost measured on
  `h2 serve` without a GPU view backend is not what a player sees. Agents
  attach to a running game and never build one. `h2 serve --system-link
  --host-light` always paces to real time ([dedicated-host.md](../live/dedicated-host.md)).
- **The socket.** `--socket PATH` selects the endpoint, followed by
  `$H2_AGENT`, then `agent.sock` in the user's cache directory
  (`agent_proto::default_socket`; the directory per host is in
  [storage.md](storage.md#host-directories)).
  Local clients use an AF_UNIX stream on Unix and on Windows 10 version 1803
  or later. Unix sockets have mode 0600. Windows socket files inherit the
  directory's ACL, so custom socket paths belong in the user's private directory.
  A live listener retains its path. A stale socket file can be replaced at
  startup. On Unix, `h2 serve` removes its socket on Ctrl-C or SIGTERM.
  `tcp://HOST:PORT` selects unauthenticated TCP, including the Xbox app's
  `h2 agent --xbox HOST` endpoint on port 4720 ([xbox.md](xbox.md)).
- **Another Mac.** `h2 lab agent --host H <run id> <call>` runs `h2 agent
  <call>` against that run's socket; `h2 lab bench` does a wait, capture,
  `render` and `counters` in one command ([lab.md](lab.md#driving-a-run-the-agent-api)).
- **The CLI.** `h2 agent <method> [JSON]` calls any method; short forms
  exist for common ones (`h2 agent --help` lists them). `h2 agent -` reads
  a script on stdin, one call per line, on one connection. `h2 agent run
  <script>` is the same with `expect` lines and one JSON verdict
  ([Scripted scenarios](agent-api-checks.md#scripted-scenarios-with-a-verdict)). A call
  `h2 agent poke ...` takes a claim for its connection. A result prints as
  JSON; an error prints to stderr and exits 1.
- **Settings of a headless run.** A headless run has no settings store
  unless given one, so menus built on settings stay retail and the log
  says so. `--settings DIR` on `serve` and `run` builds the store as
  `h2 play` does (`boot::setting_defs::load`, `crates/apps/cli/src/boot_cmd.rs`):
  `DIR/settings.toml`, then the `H2_*` variables, then the defaults. A
  setting changed in a menu is written back to that file, so pass a copy.
- **Cost.** The agent is one more observer on `GAME_TICK`, which costs
  dispatch transfers as `lockstep`'s does: compare runs that all have it,
  or all lack it. With `H2_FEATURES=-agent` requests are answered only
  between slices, and pause and step are refused.

### Finding a game

`h2 play` with `H2_AGENT` unset opens its own socket,
`agents/<pid>.sock` under the cache directory (`agent_proto::discover`,
selected by `Host::discoverable_agent`). On Unix its directory has mode 0700,
so only the user can connect. On Windows it inherits the profile directory's
ACL. A windowed macOS game with an enabled agent also listens at this
per-process socket when it has a custom endpoint, so the [lab](lab.md) can
find it. Both sockets serve the same agent. A game started any other way
(`h2 serve`, `h2 run`, the Xbox app) opens a socket only where `H2_AGENT` or
`--socket` says. `H2_AGENT=off` keeps `h2 play` from opening one
([switches.md](switches.md)). A client lists the directory
and asks each socket `session.status` (`discover::scan`); a file nothing
answers on, left by a game that was killed, is removed. `h2 agent games`
prints each game's socket, pid, map and phase, and `h2 agent <call>` with
no socket named, no `$H2_AGENT` and nothing at the default socket uses
the one game in the directory. The editor pairs with these games
([editor-guide.md](../editor/editor-guide.md), "Play from the editor").

## Wire

JSON-RPC 2.0, one JSON object per line each way (`crates/extensions/agent-proto/src/lib.rs`).
A request with an `id` gets one response with that `id`. Valid JSON that is
not a request gets `-32600`; params fields a method does not know are
refused; batches are not supported. Method names are `domain.verb`.

**The stamp.** Every result carries `at` (`agent_proto::At`):

```json
{"at": {"tick": 48211, "ticks": 52000, "activation": 3, "map_ticks": 1900, "present": 96400,
        "map": "<map>", "mode": "campaign", "bsp": 1,
        "phase": "gameplay", "paused": false, "boundary": "tick"}}
```

`tick` is the game's own count (absent when no game state is readable),
`ticks` the agent's count since the game started (never resets),
`map_ticks` ticks since the map loaded, `present` frames presented.
After adoption, `activation` identifies the full game start the editor
observes. A reload receipt reserves its own value. Only the successful
start for that transition publishes it. A checkpoint revert, a restore
or another start cannot satisfy the wait. `map_ticks` counts from the
start and its first tick reports one. A machine snapshot saves and
restores that count. Before adoption, `activation` is zero and
`map_ticks` counts from the map name change.

- **Phase.** `loading` (no tick for 500 ms: `agent::TICK_STALE`), `menu`
  (mode `main_menu`, always: its background runs `cinematic_start` too),
  `gameplay` (campaign or multiplayer), `cinematic` (between HaloScript's
  `cinematic_start` and `cinematic_stop`, observed by the agent feature;
  cleared when the map changes). A level's first tick comes before its
  scripts start the opening cutscene, so ask for
  `{"phase":"gameplay","map_ticks":30}` to mean "playing". A fade leaves the phase unchanged.
  `ui.audit` reports menu transition state. During a load the mode reads as none.
- **Boundary.** `tick`: read right after a tick returned, one consistent
  simulation state. `slice`: read between scheduler slices with no tick for
  500 ms, where guest memory may be mid-update. While the game ticks,
  answers between slices keep the last tick's state and say `tick`. The
  agent feature observes `GAME_TICK` last, after telemetry and lockstep
  (`crates/apps/boot/tests/feature_plan.rs`).

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

**Connections.** A client that half-closes (`nc -N`) still gets answers to
what it sent. Closing releases control. Each connection queues at most
4096 lines for its client (`OUT_QUEUE` in `crates/extensions/agent/src/server.rs`); a
client that lets it fill is dropped and its control released, so the game
thread never waits on a client.

**Host methods.** A host adds methods with `Agent::add_method(name,
Access::Read | Act | Claimed, f)` (answered between slices too, where the
method does not read guest memory) or `Agent::add_game_method` (answered
on the game's thread with guest memory at hand, at a tick boundary while
the game ticks).

## Live mirror

In [agent-api-session.md](agent-api-session.md#live-mirror).

## `session`: what is running and its clock

In [agent-api-session.md](agent-api-session.md#session-what-is-running-and-its-clock).

### Lab consent

In [agent-api-session.md](agent-api-session.md#lab-consent).

### Control

In [agent-api-session.md](agent-api-session.md#control).

### Window pause and frame step

In [agent-api-session.md](agent-api-session.md#window-pause-and-frame-step).

### Window screenshots

In [agent-api-session.md](agent-api-session.md#window-screenshots).

### Close the window

In [agent-api-session.md](agent-api-session.md#close-the-window).

### Resize the window

In [agent-api-session.md](agent-api-session.md#resize-the-window).

## `world`: what is in the level

In [agent-api-world.md](agent-api-world.md#world-what-is-in-the-level).

## `world.breakable`: broken glass

In [agent-api-world.md](agent-api-world.md#worldbreakable-broken-glass).

## `world.pools`

In [agent-api-world.md](agent-api-world.md#worldpools).

## `world.ai`

In [agent-api-world.md](agent-api-world.md#worldai).

## `net.replication`

In [agent-api-world.md](agent-api-world.md#netreplication).

## `actors.net`

In [agent-api-world.md](agent-api-world.md#actorsnet).

## `world.raw`: an object's bytes, before and after

In [agent-api-world.md](agent-api-world.md#worldraw-an-objects-bytes-before-and-after).

## `world.poke`: replace bytes in one object part

In [agent-api-world.md](agent-api-world.md#worldpoke-replace-bytes-in-one-object-part).

## `havok`: the physics world

In [agent-api-world.md](agent-api-world.md#havok-the-physics-world).

### `havok.agents`

In [agent-api-world.md](agent-api-world.md#havokagents).

### `havok.pair.set`

In [agent-api-world.md](agent-api-world.md#havokpairset).

## `superbounce`: the superbounce game type

In [agent-api-world.md](agent-api-world.md#superbounce-the-superbounce-game-type).

## `pogo`: Superbounce's Pogo Stick

In [agent-api-world.md](agent-api-world.md#pogo-superbounces-pogo-stick).

## `profile`: where a frame goes

In [agent-api-render.md](agent-api-render.md#profile-where-a-frame-goes).

## `render`: the frame and the pipeline

In [agent-api-render.md](agent-api-render.md#render-the-frame-and-the-pipeline).

### `render.frame`

In [agent-api-render.md](agent-api-render.md#renderframe).

### `render.stats`

In [agent-api-render.md](agent-api-render.md#renderstats).

### `render.capture_clip`

In [agent-api-render.md](agent-api-render.md#rendercapture_clip).

### Picture feed: `render.picture`

In [agent-api-render.md](agent-api-render.md#picture-feed-renderpicture).

### `render.marks`

In [agent-api-render.md](agent-api-render.md#rendermarks).

### `render.markers`

In [agent-api-render.md](agent-api-render.md#rendermarkers).

### `render.particles.refusals`

In [agent-api-render.md](agent-api-render.md#renderparticlesrefusals).

### `render.coop_look`

In [agent-api-render.md](agent-api-render.md#rendercoop_look).

### FrameScene checks: compare, capture, scene, parity, hashes

In [agent-api-render.md](agent-api-render-checks.md#framescene-checks-compare-capture-scene-parity-hashes).

## Load timeline

In [agent-api-host.md](agent-api-host.md#load-timeline).

## Game code: full speed or the slower start

In [agent-api-host.md](agent-api-host.md#game-code-full-speed-or-the-slower-start).

## Returning to the main menu

In [agent-api-host.md](agent-api-host.md#returning-to-the-main-menu).

## Loading a level

In [agent-api-host.md](agent-api-host.md#loading-a-level).

## Live session links

In [agent-api-host.md](agent-api-host.md#live-session-links).

## `host`: the process and its lobby

In [agent-api-host.md](agent-api-host.md#host-the-process-and-its-lobby).

### `host.perf`

In [agent-api-host.md](agent-api-host.md#hostperf).

### `host.metrics`

In [agent-api-host.md](agent-api-host.md#hostmetrics).

### `host.memory`

In [agent-api-host.md](agent-api-host.md#hostmemory).

### Finding a leak after the fact

In [agent-api-host.md](agent-api-host.md#finding-a-leak-after-the-fact).

### `host.audit`

In [agent-api-host.md](agent-api-host.md#hostaudit).

### `host.lobby`

In [agent-api-host.md](agent-api-host.md#hostlobby).

## `ui`: what the menus show

In [agent-api-host.md](agent-api-host.md#ui-what-the-menus-show).

### `ui.attract`

In [agent-api-host.md](agent-api-host.md#uiattract).

## `audio`: voice routing

In [agent-api-input.md](agent-api-input.md#audio-voice-routing).

### `audio.loops`

In [agent-api-input.md](agent-api-input.md#audioloops).

### `audio.meter`

In [agent-api-input.md](agent-api-input.md#audiometer).

### `audio.play`

In [agent-api-input.md](agent-api-input.md#audioplay).

### `world.object.animate`

In [agent-api-input.md](agent-api-input.md#worldobjectanimate).

### `recording.play`

In [agent-api-input.md](agent-api-input.md#recordingplay).

## HaloScript state

In [agent-api-input.md](agent-api-input.md#haloscript-state).

### `hs.eval`

In [agent-api-input.md](agent-api-input.md#hseval).

## `input`: act like a player

In [agent-api-input.md](agent-api-input.md#input-act-like-a-player).

## `state`: machine snapshots

In [agent-api-input.md](agent-api-input.md#state-machine-snapshots).

## `edit` and hot `render`: live tag and shader changes

In [agent-api-edit.md](agent-api-edit.md#edit-and-hot-render-live-tag-and-shader-changes).

### Raw edits

In [agent-api-edit.md](agent-api-edit.md#raw-edits).

### Block moves

In [agent-api-edit.md](agent-api-edit.md#block-moves).

### Collision edits

In [agent-api-edit.md](agent-api-edit.md#collision-edits).

## Menu methods: settings, perf overlay, cheats

In [agent-api-edit.md](agent-api-edit.md#menu-methods-settings-perf-overlay-cheats).

## `editor`: a paired editor

In [agent-api-edit.md](agent-api-edit.md#editor-a-paired-editor).

## Bot combat

In [agent-api-sim.md](agent-api-sim.md#bot-combat).

## Bounce routes

In [agent-api-sim.md](agent-api-sim.md#bounce-routes).

## `player.goto`: drive the player somewhere

In [agent-api-sim.md](agent-api-sim.md#playergoto-drive-the-player-somewhere).

## `sim`: simulated players and direct state

In [agent-api-sim.md](agent-api-sim.md#sim-simulated-players-and-direct-state).

### Determinism

In [agent-api-sim.md](agent-api-sim.md#determinism).

### Object edits

In [agent-api-sim.md](agent-api-sim.md#object-edits).

### Placement edits

In [agent-api-sim.md](agent-api-sim.md#placement-edits).

### Live

In [agent-api-sim.md](agent-api-sim.md#live).

## Local Forge session

In [agent-api-modes.md](agent-api-modes.md#local-forge-session).

## Shared Forge session

In [agent-api-modes.md](agent-api-modes.md#shared-forge-session).

## Forge probes

In [agent-api-modes.md](agent-api-modes.md#forge-probes).

## Firefight

In [agent-api-modes.md](agent-api-modes.md#firefight).

## Rules scripts

In [agent-api-modes.md](agent-api-modes.md#rules-scripts).

## Game variants

In [agent-api-modes.md](agent-api-modes.md#game-variants).

## Sim matches

In [agent-api-modes.md](agent-api-modes.md#sim-matches).

## Theater transport

In [agent-api-modes.md](agent-api-modes.md#theater-transport).

## `anim`: the animation graph walk

In [agent-api-checks.md](agent-api-checks.md#anim-the-animation-graph-walk).

## `sim.arms`: the native-sim gate read

In [agent-api-checks.md](agent-api-checks.md#simarms-the-native-sim-gate-read).

## `collision`: the native collision world

In [agent-api-checks.md](agent-api-checks.md#collision-the-native-collision-world).

## Tick traces and the flight recorder

In [agent-api-checks.md](agent-api-checks.md#tick-traces-and-the-flight-recorder).

## Shadow checks

In [agent-api-checks.md](agent-api-checks.md#shadow-checks).

## Scripted scenarios with a verdict

In [agent-api-checks.md](agent-api-checks.md#scripted-scenarios-with-a-verdict).

## Test maps

In [agent-api-checks.md](agent-api-checks.md#test-maps).

## Content packages

In [agent-api-checks.md](agent-api-checks.md#content-packages).

## The tag store

In [agent-api-checks.md](agent-api-checks.md#the-tag-store).

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
- Methods a method table names but this build does not register (an
  `input.*` method without XAPI input, `audio.voices` without
  DirectSound, `host.lobby.*` without the light host) are absent from
  `session.methods`.

## Evidence

Statements are **Code** (read from this repository, the path given) unless
labelled. Guest addresses are **E** (read from the retail XBE with `h2
disasm`) and are research points, not data. Tests that cover the page:
`crates/extensions/agent/src/exec/tests.rs` (wire, control, wait, deferred answers),
`crates/extensions/agent/src/sim/tests.rs` (`sim.*`, with the callees played by the
tests), `crates/extensions/agent/src/trace/tests.rs`, `crates/extensions/agent/src/havok.rs` and
`crates/extensions/agent/src/raw.rs` (synthetic guest memory), `crates/extensions/agent-proto`
(wire types), `crates/apps/boot/tests/feature_plan.rs` (the observer order),
`crates/extensions/agent/src/render/picture/tests.rs` and `exec/picture.rs`
(`render.picture` over a synthetic frame source).

## Not verified

- Nothing here was checked by running the game unless a section says so:
  the tests above use fake readers, synthetic guest memory, synthetic pads,
  synthetic frames and a fake sampler.
- `ui.audit`, `ui.press` and the menu crawl against a live game. Their regression
  cases use synthetic memory, input and agent replies.
- `input.route` and the Ascension recipe against a running game.
- `state.*` against the real machine: the tests run over fakes.
- `sim.give`, `sim.spawn` and teleport against a running game: the callees
  are played by the tests.
- Lobby steps `invite` and `start` have not been pressed through by `sim`
  in a run.
- `render.frame`, `load.timeline`, `world.raw`, `havok.*` and the
  `trace`/`flight` recorder against a running game.
- `shadow.*` and `scripts/shadow-gate.sh` against the disc: the tests run
  over synthetic code and memory.
- `render.compare`, `render.capture`, `render.scene`, `render.parity`
  and `sim.hashes` against a running game.
- `render.picture` against a running game, and its readback's cost to the
  game's frame time: the tests use a synthetic frame source.
- `maps.test.*` against a running game: the build, the slot and the
  queued load ([test-maps.md](../game/test-maps.md#not-verified)).
- Not provided: effect, projectile and particle counts; per-subsystem
  guest time; owner attribution of frame cost; kernel I/O seeks and cache
  hits in the load timeline; a widget-level HUD; setting a feature switch through the API; loading a named level
  other than a test map (`maps.test.load`).

## Behaviour probes

**Built.** A socket-enabled serve registers these four game methods under
`agent-probes`. Their names and wire types are in `agent-proto`'s `probe` module (`crates/extensions/agent-proto/src/probe.rs`); the implementation is `crates/extensions/agent/src/probe/`. Discover them with `session.methods`. The three acting
methods require a writable local/host session and a ticking game. Use a
single controller connection for a multi-call experiment. Object arguments
are full eight-digit hex datums, as returned by `sim.give`, `sim.spawn` or
a player's `world.query` row.

| Method | Request and result |
|---|---|
| `probe.weapon` | Give the weapon with `sim.give` and wait until it is held. Send `{"player":0,"object":"e1230042","ticks":60,"steps":[{"rt":1,"ticks":55},{"ticks":5}]}`. The initial sample and one row per subsequent tick contain first-magazine loaded/reserve counts, magazine state and heat. Each tick's `shots` lists barrel events and the projectiles launched by each. Each projectile has its datum, definition datum, applied cone in radians, barrel error fraction (`bloom`), direction vector and measured deflection in radians. Two trigger steps separated by a neutral step exercise two presses. A step with `"buttons":["x"]` exercises a reload. |
| `probe.animation` | Send `{"unit":"e1230043","path":["combat","rifle","ar","fire_1"],"overlay":false}`. The four strings are mode, class, weapon type and action. Set `overlay` for an overlay lookup. Returns the unit's graph datum/name, requested path, selected mode/class/type string IDs and names, inherited graph index, animation index, selected clip name and frame count. The native selector runs against the unit's current animation manager. This samples the answer to a state request without changing the unit's playing channel. |
| `probe.hit` | Send `{"projectile":"proj:weapons/assault rifle/bullet","target":"e1230043","from":[3,0,1],"to":[2,0,1],"ticks":12}`. Coordinates are world-space, or target node-local when `node` is supplied. The projectile starts at `from` and travels in the direction of `to` with the projectile definition's velocity and range. Optional `shield` sets the target's shield fraction at launch. Returns the first collision of this projectile, including kind, position, object datum, region, node and global material index. Object hits include vitality `before` and `after` as [body, shield]. `damage` contains the game's damage reports for the specified victim during that projectile's impact. A wall or a different victim is a reportable hit. A miss through the entire window is an error. Damage and shield preparation affect the world. |
| `probe.round` | Send `{}`. Returns the engine's round-start tick, per-player first applied control and first movement/fire ticks, and any observation error. Capture starts at map activation, so callers can ask after the countdown. Join player indices to `sim.player.list.clankers` to identify bots. |

Weapon and hit captures share one slot. Their window is 1–290 ticks. A
changed weapon, missing object, unreadable fields or discontinuous weapon
trace fails the request. Machine snapshots preserve round history and refuse
an in-progress capture. Take a snapshot before a destructive experiment and
load it afterwards.

The simulation hooks measure the barrel fire call at `0x104150`, the launch
call at `0x1052c5`, projectile collision at `0xfaa60` and its nested damage
report. The animation selectors are `0x1db170` and `0x1dbc80`, with entry
resolution at `0x1daea0`. The round observer uses the two non-neutral
per-player callers of `0xc6de0`. None of the probes requires rendering.

The [Halo 1 parity suite](../halo1/parity.md) drives these methods against
converted Blood Gulch and a30.

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
