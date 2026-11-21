# Agent API: driving the running game without pixels

Status: Built. Every method below is registered by the code; the ones no run has exercised are named under "Not verified".
Code: `crates/extensions/agent-proto` (wire types, blocking client), `crates/extensions/agent` (server, tick-boundary executor, most methods), `crates/extensions/game/src/agent_glue.rs` (the methods the host answers), `crates/apps/cli/src/agent_cmd.rs` (`h2 agent`). Tests: `crates/extensions/agent/src/exec/tests.rs`, `crates/extensions/agent/src/sim/tests.rs`, `crates/extensions/agent/src/trace/tests.rs`, `crates/apps/boot/tests/feature_plan.rs`.
Switch: `agent` feature (`H2_FEATURES=-agent` removes the tick hooks), `H2_AGENT` (the socket), listed in [switches.md](switches.md).

An agent, a script or a test reads and drives a running Halo 2 through one
JSON-RPC interface: what is in the level, where frame time goes, what the
menus show, how to put the game in a state and act in it. It replaces
screenshots, pasted logs and environment variables read at launch. The
command-line client is `h2 agent`. The editor connects to the same endpoint.

The rule that must hold: every answer is read at one tick boundary on the
game's thread, and says which moment it describes (the `at` stamp).

## Method index

`session.methods` lists what the running process registers; a host method
appears only where the host adds it. Access (`agent::Access`): **read**
answers any connection; **act** answers any connection while none holds
control, else only the controller (`NOT_CONTROLLER`); **claim** needs this
connection to hold `session.claim`.

| Method | Access | Section |
|---|---|---|
| `session.status`, `methods`, `claim`, `release`, `wait`, `subscribe` | read | [`session`](#session-what-is-running-and-its-clock) |
| `live.subscribe`, `live.status`, `live.close` | read | [Live mirror](#live-mirror) |
| `session.pause`, `resume`, `step` | claim | [Control](#control) |
| `world.count`, `world.query`, `world.object` | read | [`world`](#world-what-is-in-the-level) |
| `world.ai` | read | [`world.ai`](#worldai) |
| `world.raw` | read | [`world.raw`](#worldraw-an-objects-bytes-before-and-after) |
| `world.poke` | claim | [`world.poke`](#worldpoke-replace-bytes-in-one-object-part) |
| `havok.bodies`, `havok.agents`, `havok.contacts` | read | [`havok`](#havok-the-physics-world) |
| `profile.counters`, `profile.capture`, `profile.fallbacks`, `profile.memory`, `profile.faults` | read | [`profile`](#profile-where-a-frame-goes) |
| `profile.sample.start`, `profile.sample.stop` | act | [`profile`](#profile-where-a-frame-goes) |
| `profile.sample.status` | read | [`profile`](#profile-where-a-frame-goes) |
| `render.frame`, `render.stats` | read | [`render`](#render-the-frame-and-the-pipeline) |
| `render.coop_look` | read | [`render`](#render-the-frame-and-the-pipeline) |
| `render.compare`, `render.capture`, `render.scene`, `sim.hashes` | read | [FrameScene checks](#framescene-checks-compare-capture-scene-parity-hashes) |
| `render.spawn_view` | act | [FrameScene checks](#framescene-checks-compare-capture-scene-parity-hashes) |
| `render.parity` | act | [FrameScene checks](#framescene-checks-compare-capture-scene-parity-hashes) |
| `render.overrides`, `render.programs` | read | [`edit` and hot `render`](#edit-and-hot-render-live-tag-and-shader-changes) |
| `render.reload`, `render.dump`, `render.param`, `render.toggle` | act | [`edit` and hot `render`](#edit-and-hot-render-live-tag-and-shader-changes) |
| `edit.tags`, `edit.get`, `edit.changes` | read | [`edit` and hot `render`](#edit-and-hot-render-live-tag-and-shader-changes) |
| `edit.set`, `edit.revert` | act | [`edit` and hot `render`](#edit-and-hot-render-live-tag-and-shader-changes) |
| `load.timeline` | read | [Load timeline](#load-timeline) |
| `features.list` | read | [`session`](#session-what-is-running-and-its-clock) |
| `host.metrics`, `host.audit`, `host.perf` | read | [`host`](#host-the-process-and-its-lobby) |
| `host.lobby.open`, `close`, `start` | act | [`host.lobby`](#hostlobby) |
| `host.lobby.state` | read | [`host.lobby`](#hostlobby) |
| `ui.screen`, `ui.campaign`, `ui.audit`, `ui.stack`, `ui.lobby.players` | read | [`ui`](#ui-what-the-menus-show) |
| `ui.press` | act | [Menu inspection and navigation](#menu-inspection-and-navigation) |
| `audio.voices`, `audio.listeners`, `audio.loops` | read | [`audio`](#audio-voice-routing) |
| `hs.threads`, `hs.globals`, `hs.player_actions` | read | [HaloScript state](#haloscript-state) |
| `input.pad`, `release`, `record`, `replay`, `stop` | act | [`input`](#input-act-like-a-player) |
| `input.state`, `input.host` | read | [`input`](#input-act-like-a-player) |
| `state.save`, `load`, `list`, `drop`, `fork` | act (`list` read) | [`state`](#state-machine-snapshots) |
| `bots.stats` | read | [Bot combat](#bot-combat) |
| `sim.*` | read or act | [`sim`](#sim-simulated-players-and-direct-state) |
| `match.status`, `match.report`, `match.stop` | read, read, act | [Sim matches](#sim-matches) |
| `settings.list`, `settings.set`, `perf.overlay`, `cheat.list`, `cheat.set` | read or act | [Menu methods](#menu-methods-settings-perf-overlay-cheats) |
| `theater.*` | read or act | [Theater transport](#theater-transport) |
| `collision.stats` | read | [`collision`](#collision-the-native-collision-world) |
| `trace.*`, `flight.*` | read | [Tick traces and the flight recorder](#tick-traces-and-the-flight-recorder) |
| `shadow.enable`, `shadow.disable`, `shadow.status` | act, act, read | [Shadow checks](#shadow-checks) |
| `maps.load`, `maps.test.list`, `create`, `load`, `remove` | act, read, act, act, act | [Test maps](#test-maps) |

A new method gets a row here and a section for its domain. The names
live in `crates/extensions/agent-proto/src` (`method::ALL`, `hot::method::ALL`,
`state::method::ALL`, `sim`, `trace`, `theater`) and where the host adds
them (`Agent::add_method`, `Agent::add_game_method`).

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
- **The socket.** `--socket PATH`, else `$H2_AGENT`, else
  `~/Library/Caches/h2recomp/agent.sock` (`$XDG_CACHE_HOME` or `~/.cache`
  elsewhere), not `$TMPDIR`, which differs between agent sessions
  (`agent_proto::default_socket`). A socket file nobody answers on is
  replaced; a live one is an error. `tcp://HOST:PORT` listens or connects
  over TCP instead (`agent_proto::Endpoint`), which is how the Xbox app is
  reached (`h2 agent --xbox HOST`, port 4720; [xbox.md](xbox.md), "Remote
  control"). TCP has no authentication; Unix sockets are Unix only.
  `h2 serve` removes its socket on Ctrl-C or SIGTERM. A Unix socket is
  created with mode 0600.
- **Another Mac.** `h2 lab agent --host H <run id> <call>` runs `h2 agent
  <call>` against that run's socket; `h2 lab bench` does a wait, capture,
  `render` and `counters` in one command ([lab.md](lab.md#driving-a-run-the-agent-api)).
- **The CLI.** `h2 agent <method> [JSON]` calls any method; short forms
  exist for common ones (`h2 agent --help` lists them). `h2 agent -` reads
  a script on stdin, one call per line, on one connection. `h2 agent run
  <script>` is the same with `expect` lines and one JSON verdict
  ([Scripted scenarios](#scripted-scenarios-with-a-verdict)). A call
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
`<cache>/h2recomp/agents/<pid>.sock` (`agent_proto::discover`; the
directory is mode 0700, so only the user can connect; set in `boot`'s
agent source from `Host::discoverable_agent`). A game started any other
way (`h2 serve`, `h2 run`, the Xbox app) opens a socket only where
`H2_AGENT` or `--socket` says. `H2_AGENT=off` keeps `h2 play` from
opening one ([switches.md](switches.md)). A client lists the directory
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
{"at": {"tick": 48211, "ticks": 52000, "map_ticks": 1900, "present": 96400,
        "map": "<map>", "mode": "campaign", "bsp": 1,
        "phase": "gameplay", "paused": false, "boundary": "tick"}}
```

`tick` is the game's own count (absent when no game state is readable),
`ticks` the agent's count since the game started (never resets),
`map_ticks` ticks since the map loaded, `present` frames presented.

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

The editor opens a second connection to the same Unix or TCP endpoint.
It calls `live.subscribe` there and reads the JSON reply line before
switching to length-prefixed binary frames. Keep the original JSON
connection for control calls. The stream reads the game at tick boundaries.

| Method | Params | Result |
|---|---|---|
| `live.subscribe` | `{layers: ["objects", "players"], every?, key_every?}` | `{at, stream, protocol: "h2-live-mirror/1", tick_rate, map: {name, checksum, epoch}, layers, refused}`. The stream id is numeric. An unavailable checksum is null. Each unsupported layer has a reason under `refused`. |
| `live.status` | none | `{map: {name, checksum, epoch}, streams, capture_us: {p50, p99, budget}, shed}`. Stream fields are `stream`, `layers`, `every`, `frames`, `bytes`, `queue`, `dropped` and `skipped`. Frames and bytes count complete writes, including the length prefix. |
| `live.close` | `{stream}` | `{closed: true}` if the stream existed. Disconnecting the binary socket also ends its subscription. |

`every` samples one tick in 1–30 (default 1). `key_every` requests a
full frame at least every specified number of game ticks (default 150).
An empty accepted layer set yields header-only frames. The first frame and frames after a cut or a dropped sample are keys.
Deltas identify records by salted object handle or player index and carry
field masks and removals. A player joining or changing name requires a
key. The length on each layer lets readers skip unknown layers. The
binary layout and blocking decoder are in `agent_proto::live`. A frame
starts with a little-endian u32 payload length, capped at 8 MiB. The
payload starts with a kind byte, three u64s (sequence, game tick, map
epoch) and a flags byte (paused, cut). Each section has a u8 layer id
and a u32 byte length.

Objects carry their definition, type, header flags, parent, transform,
velocity, vitality and scenario placement index. A child's transform is
relative to its parent, as in `sim.objects`. Players carry their name,
team, unit, held weapon, aim and local user. The game thread fills reusable
buffers. A worker diffs and encodes them and each socket has a bounded
writer queue. `skipped` counts missed captures and `dropped` counts
frames lost on the way to the writer. Capture percentiles are in
microseconds over the last 150 successful captures, against a 150 µs budget. They are null before the first capture.

A game in a remote Live match refuses a subscription and closes an
existing one on entry. The embedded service used by `--live-local`
supports the mirror. For exact-tick comparison and a replay with and
without a subscriber, use `scripts/check-live-mirror.py` with a gameplay
snapshot and its matching input log. It prints the hash comparison,
object count, byte rate and capture timings.

## `session`: what is running and its clock

| Method | Params | Result |
|---|---|---|
| `session.status` | none | `at`, `protocol`, `fps` (frame-stats window, when collected), `controller` (this connection holds control), `claimed` (any does) |
| `session.methods` | none | every method this process answers |
| `session.claim`, `session.release` | none | take or give up control; closing the connection releases it |
| `session.wait` | `{until, timeout_ticks?, timeout_ms?}` | `{at, ticks}` once every field of `until` holds |
| `session.subscribe` | `{events: ["phase","map","bsp","pause"]}` | then `event` notifications: `phase {from,to}`, `map {from,to}`, `bsp {from,to}`, `paused`, `resumed {why: request, timeout or released}` |
| `features.list` | none | `{at, plan}`: the feature plan text the game printed at start |

`until` takes `tick`, `ticks` (more ticks than when the wait began),
`map_ticks`, `present`, `phase`, `map` and `paused`; all given must hold,
at least one is required. `wait` settles at tick boundaries (and between
slices for wall-clock timeouts and while loading); a condition already true
answers at once. `timeout_ms` defaults to 30 s, at most 600 s; the failure
is `TIMEOUT` with the stamp.

```sh
h2 agent wait '{"phase":"gameplay","map_ticks":30}' 5000
h2 agent watch phase map          # events until interrupted
```

`session.wait` replaces every "sleep and screenshot" loop. Switches do not
change through `features.list`; live changes go through [`render.toggle`](#edit-and-hot-render-live-tag-and-shader-changes)
and [`cheat.set`](#menu-methods-settings-perf-overlay-cheats).

### Control

`session.pause`, `session.resume` and `session.step` need `session.claim`
(one connection holds it). A pause blocks the game thread inside the tick
observer, still answering requests. It resumes by itself after `max_ms`
(default 60 s, at most 600 s; a second pause re-arms it), when the
controller releases or disconnects, or when the game exits. A pause asked
for while loading takes hold at the next tick; its limit runs meanwhile.
`step {ticks}` (1 to 1800) runs that many ticks and answers once paused
again; a resume, a new pause or a release first answers it with
`INTERRUPTED`. With the tick observer off, pause and step answer
`UNAVAILABLE`. In `h2 play` a pause also stops the window's event loop, so
it is meant for `h2 serve`. Input calls need no claim while nobody holds
control.

```sh
printf 'claim\npause\nstep 1\nobject player.0\nrelease\n' | h2 agent -
```

## `world`: what is in the level

Objects come from `live-telemetry::memory` and `live_telemetry::things`,
read at the tick boundary. Ids are `player.<index>` or `<kind>.<datum>`
with the datum handle as 8 hex digits, salt included, so a slot reused by
another object answers `GONE` and never another object's data.

| Method | Params | Result |
|---|---|---|
| `world.count` | `{group_by: "kind" \| "team"}` | `{at, total, groups}`: the cheap first call |
| `world.query` | `{kind?, near?, tag?, fields?, limit?, offset?}` | `{at, total, offset, objects, more}` |
| `world.object` | `{id}` | `{at, object}`: every field the reader has for one object |

- **Kinds** (`agent_proto::Kind`): `player`, `actor` (AI biped), `vehicle`,
  `machine`, `body` (a dead biped), `scenery`, `weapon` and `equipment`
  (lying in the world; held ones are their unit's `weapon`), `control` (a
  device's switch or panel), `crate`, `creature`. The last six come from a
  second pass over the object header table by the header's type byte
  (`Kind::of_object_type`). Projectiles, effects, particles, garbage,
  light fixtures and sound scenery are not listed.
- **Query.** `near` is `{point | player, radius}` and sorts nearest first,
  adding `distance`. `fields` trims each object (`id` and `kind` stay).
  `tag` keeps one definition tag: a datum (8 hex digits, `0x` optional) or
  text its name contains (any case, `/` for `\`). Pages default to 50, at
  most 500.
- **Tag names.** An object with a `tag` also has `tag_name`, the
  definition's path from the loaded map file's names (as `edit.tags`
  reads them); it is absent when the names cannot be read. A name filter
  then matches nothing. The map file is found as `boot::map_override::map_file`
  finds it: by file name first, then by the cache header's name (`+0x198`)
  among the maps folder's files, so a converted map served under a disc
  map's file name is the one read. With two files of the same header name
  the first in folder order wins. A map installed as content on the data
  partition is not searched.
- **Fields.** `pos`, `velocity`, `team`, `tag`, `tag_name`, `vehicle`,
  `weapon`, `name` and `local_user` and `unit` (players), `mounted_on` (a
  turret is a vehicle with `mounted_on` set; its `pos` and `forward` are
  relative to that vehicle), `occupied`, `placement` (the scenario
  placement that made it), `position` (a machine's device position, 0 to
  1) and `physics`.
- **Facing.** `forward` is where an object faces. For a unit it is the
  aiming vector (unit `+0x168`, the vector the held weapon's targeting ray
  follows, `0x000cf3d0`), which turns with the camera; a biped's body turns
  only as it moves, so its own forward (`+0x70`) is `body_forward`. `yaw`
  (degrees about +z from +x, 0 to 360) and `pitch` (degrees up) are
  `forward` as angles. A unit whose aim is not finite or not unit length
  falls back to the body's forward.
- **`physics`** (units): `crouch`, `crouch_request` (unit flags bit 23,
  which stays set after release until there is room to stand), `mode`,
  `airborne` (biped `+0x348` bit 0, likely) and `airborne_ticks`
  (`+0x399`), `hitbox_offset_ticks` and `hitbox_offset` (the offset a melee
  lunge leaves, `+0x3aa`; [superbounce.md](../game/superbounce.md)),
  `unit_flags`, `biped_flags`, `havok_pos` and `havok_velocity` (the first
  Havok body; [`havok`](#havok-the-physics-world) has every body and
  contact), and for bipeds `support`: what the biped physics update
  (`0x000dd990`) keeps of the ground (layout in `live-telemetry`'s
  `layout.rs`, `BIPED_SUPPORT_*` and `CHARACTER_CONTACT_*`). `support.flags`
  is biped `+0x349` (`copied`: its bit 4, set when this tick's update copied
  a support contact, `0x000ddeda`); `words` are `+0x354..+0x364` (all
  `ffffffff` for nothing); `point`, `normal`, `contact_point` are `+0x368`,
  `+0x374`, `+0x380`; `ground_state` is `+0x3ec`, the vector the ground
  test `0x001f19c0` measures first; `contacts` are the character contacts
  that test walks (physics component `+0x88`, count `+0x8c`, `0x48` each, at
  most 32): `key`, `other`, `point`, `normal`, `value` (the first pass
  wants at least 1e-4), `flags`. The airborne bit is decided from these,
  unlike `havok.contacts`, which lists the solver's contact constraints.
  Poll `object player.0` on one `h2 agent -` connection and keep one reply
  per stamped tick for a movement trace, or use [a trace](#tick-traces-and-the-flight-recorder).

```sh
h2 agent count team
h2 agent query '{"kind":["scenery"],"tag":"<part of a tag name>","near":{"player":0,"radius":500},"fields":["pos","tag_name"]}'
h2 agent object player.0
```

## `world.ai`

`h2 agent world.ai` returns `{at, actors}` at one game boundary and takes
no parameters (`crates/extensions/agent/src/ai.rs`). Each allocated actor has its
salted AI `datum`, its `unit` ID (for `world.object`), character tag
datum, team, scenario squad index, current behavior ID, its retail name
when available (`behavior_name`), and combat status. A missing unit,
squad or behavior is JSON null. The actor datum and unit ID have separate
lifetimes: compare the same unit across ticks to measure movement, weapon
and vehicle changes.

`sector` is the actor's cached sector index in the active BSP when in
range, otherwise null; `location_cached` distinguishes a pending refresh
from a completed lookup. The observer only reads this cache. Retail
`0x0026c180` refreshes it, and `0x00297e15` passes its sector to the mesh
walk at `0x0026c590`, which bounds it against the mesh's sector count. The
behavior ID comes from the top of the four-frame stack used at
`0x001a813d`; its label from the loaded executable's parallel name table
at `0x0046f0c8`. Combat status is the signed value the `ai_combat_status`
builtin reads per actor (`0x00276070`). `focus_object` follows the
actor's selected perception record to the object datum used at
`0x001b368a`; the fight behavior reads the same record at `0x001af0c2`.
Compare it with the player's `unit` in `world.query` to see attention on
the player. A remembered object can stay selected after contact is lost.

## `world.raw`: an object's bytes, before and after

A game method, read only, no claim (`agent::raw::install`; reader
`crates/live/live-telemetry/src/raw.rs`). It exists to look for state a forced
crouch leaves behind ([superbounce.md](../game/superbounce.md)).

`world.raw {id, parts?, len?}` returns `id`, `datum` and `parts`, each
`{part, base, hex}` or `{part, missing}` (why: an unset pointer, a fault).
Each part is found through the one before it ([havok-runtime.md](../engine/havok-runtime.md)):

| Part | Where | Bytes |
|---|---|---|
| `object` | the object's body (`player.N`: its unit) | the header's size (`+0x06`), else `len` (default `0x400`), at most `0x2000` |
| `physics_component` | body `+0xb4` into the component array | `0xa0` |
| `entity` | the component's first rigid body (`+0x70`, `+0x40`) | `0x200` |
| `motion` | entity `+0x3c` | `0x140` |
| `shape` | entity `+0x0c` | `0x80` |

`h2 agent raw-diff a.json b.json` needs no game: per part it prints the
32-bit words that differ, with their offset from the part's base, and
both as floats where they read as ones. A part on one side only, or whose
base moved, is listed, not compared. It reads what `h2 agent raw` prints,
or a `-` script's last line (`crates/apps/cli/src/agent_raw.rs`).

```sh
h2 agent raw player.0 > before.json             # world.raw {"id":"player.0"}
# ... force a crouch, walk out, stand ...
h2 agent raw player.0 > after.json
h2 agent raw-diff before.json after.json
h2 agent raw player.0 entity,shape              # some parts
h2 agent world.raw '{"id":"player.0","parts":["object"],"len":2048}'
```

## `world.poke`: replace bytes in one object part

`world.poke {id, part?, offset, hex}` (access: claim) writes 1 to 64 bytes
at a tick boundary. `id` and `part` use `world.raw`'s names; `part`
defaults to `object`. `offset` is a nonnegative byte offset from the
part's base; `hex` is two hex digits per byte. The whole range must fit
in the part `world.raw` reads with its default lengths.

The connection must hold `session.claim`. `sim.session` reports whether
the session allows host writes; a session refusal is `INVALID_REQUEST`
with `data.session`, and a request served between slices is `UNAVAILABLE`
when the session allows writes. The result has `id`, `datum`, `part`,
`base` (the part's first address), `offset`, `at`, and `before` and
`after` (the bytes at the offset, lowercase hex).

```sh
h2 agent poke <id> <part> <offset> <hex>    # takes a claim for its connection; offset decimal or 0x
```

## `havok`: the physics world

Game methods (`agent::havok::install`), read only, answered at a tick
boundary. Layout and evidence: [havok-runtime.md](../engine/havok-runtime.md).

| Method | Params | Result |
|---|---|---|
| `havok.bodies` | `{near?, radius?, limit?}` | rigid bodies: `entity` (address, the key contacts use), `state` (`active`, `inactive`, `fixed`), `object` and `datum`, `pos`, `velocity`, `angular_velocity`, `inv_mass`, `mass`, `friction`, `restitution`, and `shape` where it reads: `addr`, `vtable`, `kind` (`capsule`: `a`, `b`, `radius`, `height`; `sphere`: `radius`; `transform`: `translation`, `child`; else `other`). `near` (an object id) keeps bodies within `radius` (default 5) of its body, nearest first, with `distance`; `limit` defaults to 64 |
| `havok.agents` | `{object}` | collision pairs and cached state ([below](#havokagents)) |
| `havok.contacts` | `{object?}` | contact constraints: `a`, `b` (`entity`, `object`, `datum`, `fixed` for the level and static bodies) and `points`: `pos`, `normal` (from `b` toward `a`, **S**), `distance` (negative is penetration), `impulse` (what the solver applied on its last step), `friction`, `restitution`. With `object`, only its contacts, with it as `a` |

```sh
h2 agent havok.agents '{"object":"player.0"}'
h2 agent havok.contacts '{"object":"player.0"}'
h2 agent havok.bodies '{"near":"player.0","radius":3}'
```

For a superbounce, poll `havok.contacts {object: player.0}` once a tick on
one `h2 agent -` connection next to `object player.0`: the contact with
`b.fixed` is the level, and `impulse` over the body's `mass` is the
velocity it gave. A type-`0x17` wrapper reports `kind: "other"` in
`havok.bodies`; its current pill is the pointer at `shape+0x30`
([Shapes](../engine/havok-runtime.md#shapes)). A direct biped capsule's
`shape.height` tells standing from crouching.

### `havok.agents`

`object` is required. The reply has `object`, a stamp (`at`) and `agents`.
Each entry is a pair with an allocated agent on any rigid body of that
object. A live object with no such pair returns an empty list. A
malformed selector is `INVALID_PARAMS`; an absent object or a player
between lives is `GONE`.

- `entry`, `island` and `collidables: [a,b]` are guest addresses in native
  pair order. `object_side` is `a` or `b`, and `entity` is the requested
  object's participating rigid body. `other` identifies the partner
  entity, with its `datum`, object ID and fixed-island flag where
  available; it is omitted for a partner without an entity. `pair_word`
  keeps the last four bytes of the pair record.
- `cache` has `base`, `vtable`, `manager`, `kind` and `hex`. Addresses,
  keys and raw words are eight lowercase hex digits. `hex` is two digits
  per guest byte from `base`, including the allocation header and padding.
- A `bv_tree` or `welded_bv_tree` also has `tree`: `min` and `max` (the
  cached query box in the tree's frame) and, per immediate child, `entry`,
  `key`, raw `word_04` and its own `cache`. A type-1 structure MOPP key
  also has `surface`, the numeric collision BSP surface index. A filtered
  child is `kind: "empty"` with its shared header.
- A `convex` or `swept_convex` cache has a `convex` block: `vertices_a`
  and `vertices_b` (active support vertex IDs in that agent's dispatch
  order; for convex against tree, A is the convex body and B the selected
  tree shape), `contact_ids` (the child manifold's IDs in the shared
  manager), `separating_axis` and `separating_distance` (the bound used to
  skip a closest-feature search while positive; non-finite values are
  null). `swept_sides` on the swept agent records A in bit 0 and B in
  bit 1.
- Unrecognised agents are `kind: "other"` with their inline bytes. The
  bounds are 32,768 scanned pairs, 512 children per tree, 4 KiB per
  allocation and 256 KiB of raw cache bytes per request; a failed read or
  an exceeded bound is `UNAVAILABLE`.

To compare ticks around a crouch, match a child by its partner and
complete `key`, and record both agent bases to see replacements.
Allocators can reuse an address between samples, and the raw dump
contains pointers, unused slots and padding. A cached surface with an
empty manifold is still a candidate in the query box.

## `profile`: where a frame goes

| Method | Params | Result |
|---|---|---|
| `profile.counters` | none | the frame-stats rolling window (`timeline::phase`), the numbers `H2_FRAME_STATS` prints: `frames`, `fps`, frame ms, `phase_ms`, `counts`. When nothing else collects them the glue starts a quiet 5 s window |
| `profile.capture` | `{ticks \| ms, budget_ms?, spikes?, sample?}` | percentiles, phases, counters and spikes over a window (below) |
| `profile.fallbacks` | `{top?}` | where the fallback interpreter runs |
| `profile.memory` | none | the flat RAM window's counters (below) |
| `profile.faults` | `{top?}` | the fault census: every guest memory fault and whether execution went on from it |
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
  answers `frames`, `fps`, `frame_ms` and `busy_ms` (frame less pacing) as
  mean, p50, p95, p99, max and total; the same for each phase (`cpu`,
  `decode`, `encode`, `submit`, `gpu-wait`, `pace`, `render-wait`, `d3d`,
  and `render` when the render-timing feature measures it); mean, max and
  total of each counter (`draws`, `passes`, `uploads`, ...); `over_budget`,
  the frames longer than `budget_ms` (default 33.3); and `spikes`, the
  worst of them (20 by default, at most 500), each with its present
  number, game tick and map, its own phases and counters, its three
  biggest phases (pacing left out) and `causes`. Every frame is also
  written as a CSV row (present, end, tick, map, total and busy us, each
  phase in us, each counter) under the cache directory's `captures/`, and
  the reply gives its path. A frame belongs to the last tick that had ended
  when it did. By ticks the window is 1 to 36000 (it gives up after 600 s
  with `complete: false`); by `ms`, 1 to 600000. One capture at a time; it
  is read-only. With frame stats off it answers `UNAVAILABLE`
  (`crates/extensions/agent/src/capture.rs`).
- **Causes.** The reply's `causes` counts every frame over budget, listed
  or not, by cause (`crates/extensions/agent/src/capture.rs`, `causes`):
  `shader_compile` and `pipeline_compile` (any on the critical path),
  `texture_uploads` and `gpu_readback` (more than twice the window's
  median, and at least one), `file_io` (the guest's file calls took at
  least 2 ms of host time in the frame, or at least a tenth of its busy
  time: the `file_io_us` counter; bytes read alone never count), `gpu`,
  `render_thread` and `pushbuf_decode` (that phase is at least 40% of the
  frame's busy time), else `guest_cpu` when the guest's own time is the
  biggest phase. These are what the counters show, not a proof.
- **Native renderer counters.** `profile.counters` and `profile.capture`
  use the field names of `H2_FRAME_STATS_CSV`. Each response and capture
  CSV includes native columns with activity in its window; the streaming
  CSV writes a fixed header at startup. Each `native_<stage>_` prefix has
  examined and suppressed guest items, refusals by reason, prepare and
  encode nanoseconds, GPU buffers and bind groups created, upload bytes and
  pipeline misses; per-view columns count reference and display ranges
  attempted and committed and invariant failures. For example
  `native_world_v1_display_recorded` can be positive while
  `native_world_v1_display_ranges` is zero when the display copy fails
  validation. Frame captures also include `native_lights_worker_round_trips`
  and `native_lights_reference_passes` / `native_lights_display_passes`.
  Definitions: [native-renderer.md](../graphics/native-renderer.md#frame-counters).
- **Fallbacks.** `profile.fallbacks {top}` (20 by default, 1 to 500)
  returns the fallback interpreter's totals and its entry addresses ranked
  by instructions interpreted, each with hits, x87 instructions and the
  function it is in (the nearest analysed start at or below it). Kept per
  address in `boot::fallback::Stats::by_site`, at most 4096 addresses
  (`sites_full` says when later ones count in the totals only). Slim host
  modules also report `cold_entries` (interpreter invocations for omitted
  owners) and `cold_sites` (distinct owner/selector pairs); the boot log
  names each pair on first use.
- **Flat RAM window.** `profile.memory` answers the counters of every
  runtime's flat RAM window in the process, summed
  (`runtime::window::process_stats`, answered by boot's agent glue;
  [contract-thin-transfers-flat-ram.md](contract-thin-transfers-flat-ram.md),
  section 4.1). They are always on. `windows` is how many there are and
  `aliased` how many hold RAM (guest RAM is aliasable once a flat-RAM
  module is registered, or from the start with `H2_FLAT_RAM=1` at run
  time). `fixups` counts faulting accesses sent to their slow path; the
  null window a context uses with fast memory off is not counted. `pages_outside` counts mapped guest pages the window does not
  hold: non-congruent with their backing, sharing a host page with one,
  byte-granular or armed by the access trap. Each costs a fault and the
  helper per access once flat accesses are emitted. `congruence_skipped`
  is the physical RAM, in bytes, the congruent allocator jumped over to
  give a fresh allocation the residue of its address. A slow page shows as
  `fixups` rising or `pages_outside` above zero.
- **Faults.** `profile.faults {top}` (20 by default, 1 to 500) is the
  fault census (`runtime::fault_census`,
  [block-clock-imprecise-faults.md](block-clock-imprecise-faults.md)):
  `generated` (memory faults raised by an accessor generated code called)
  and `runtime` (raised by the runtime or a Rust handler); `ended` (faults
  a run returned to its caller with); `continued` (times guest execution
  went on, on the same context, from the state a fault left) and
  `continued_generated` (of those, generated faults);
  `precise_exits_needed`, true when `continued_generated` is not 0; and
  the `top` fault PCs by count, each with `source`, `count`, `continued`
  and the first fault's `addr`, `len`, `access` and `reason`. Kept per PC,
  at most 4096 (`sites_full`). It counts from the process's start. It also
  names the image's choices: `clock` (`accesses` or `blocks`),
  `imprecise_faults` (module feature 128), and `continued_imprecise`, the
  continuations from a fault whose EFLAGS were inexact (should be 0).
- **Sampling.** `sample: true` also runs the in-process sampler
  (`profile.sample.*`, `boot::sampling`) over the window, and `sampling` is
  what `profile.sample.stop` answers: the module-relative dump
  (`profile.h2prof` under `H2_SAMPLE_DIR`, which `h2 profile remote`
  resolves) and its sample count. Only an h2 built with `--features perf`
  on a host with a sampler has it; otherwise, or while a
  `profile.sample.start` run holds it, the capture still runs and
  `sampling.unavailable` says why ([xbox.md](xbox.md), "Profiling on the
  console").

## `render`: the frame and the pipeline

`render.frame` and `render.stats` are host methods (types in
`crates/extensions/agent-proto/src/render.rs`, logic in `crates/extensions/agent/src/render.rs`).

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
draw diagnostics use one relaxed load per draw or clear in the D3D
wrapper. On a device with a native frame owner, the recorder waits for
the worker's presentation decision. An original presentation supplies
the D3D report. A native presentation supplies the executed graph's
attachment runs, slots, typed stages, snapshots and `display_frame`
counters. A readback required for a guest-pass snapshot can make the
worker present the original commands for that frame.

- **The frame.** `totals` (passes, draws by outcome, vertices, clears,
  copies, reads, distinct targets); `passes`: runs of work on one colour
  surface and depth (a return to an earlier target is a new pass), each
  with its target (`640x480 X8R8G8B8`), clears, draws, vertices, outcomes,
  primitives, D3D entries, texture formats, and counts of pixel-program,
  blended and depth-writing draws; `ops`: copies and readbacks with the
  draw count before them. Surfaces are the backend's handles as text.
- **A draw** (with `draws`; at most `max_draws`, 2000 by default;
  `truncated` says when there were more). Its pass, D3D entry, primitive,
  vertex and index counts, outcome, texture formats, whether it has a
  pixel program, blends or writes depth; `vs`, the vertex side: `fvf 0x...`
  or `prog @start n=slots #hash` for a vertex program (the low 32 bits of
  the words' FNV-1a hash, the same in every frame, so one program's draws
  can be picked out); and `depth`: `mode` (`D3DRS_ZENABLE`: `off`, `z`,
  `w`), `w` (W-buffered as drawn: `SET_CONTROL0`'s z-perspective bit once
  pushed, else `mode`), `func` and `write` (`D3DRS_ZFUNC`,
  `D3DRS_ZWRITEENABLE`), `viewport` (`[MinZ, MaxZ]`), `scale`
  (`SET_VIEWPORT_SCALE` z and `SET_VIEWPORT_OFFSET` z), `clip`
  (`[SET_CLIP_MIN, SET_CLIP_MAX]`), `full` (the depth target's full scale;
  0 with no depth target) and `clamp` (`D3DRS_DEPTHCLIPCONTROL` clamps
  rather than culls; shown only when on). A W-buffered draw of a vertex
  program stores its w clipped (or clamped) to `clip`, over `full`; a
  z-buffered one its z over `full`.
- **Constants.** `constants` (with `draws`; at most 32 registers, D3D
  numbering -96..=95, hardware register = number + 96) records those
  vertex shader constants on each listed draw:
  `"constants": {"-38": [x, y, z, w], ...}`. The CLI takes ranges
  (`constants -38..-35,58`), implies `draws`, and `max N` sets `max_draws`.
- **Snapshots.** `snap` (at most 4 pass numbers): when such a pass ends,
  its colour target is read back as BGRA and written as a PNG under the
  cache directory's `frames/` (`frame-<seq>-pass-<n>.png`); `snaps` gives
  each file or error. A pass with no draw has no known size and says so.
  This is a GPU round trip, only when asked.

A private capture has its own recorder. When the private scene renderer
is installed, arm the recorder before requesting a lighting capture,
then read its result:

```sh
h2 agent render.frame '{"source":"private","arm":true,"draws":true,"snap":[0]}'
h2 agent render.capture '{"lighting":"retail"}'
h2 agent render.frame '{"source":"private"}'
```

The next completed private draw supplies the report. Each row covers
draws and clears on the same attachments, with their view/slot pairs
in `slots` and typed stage names in `stages`. A copy ends a row even
when the following draw uses the same attachments. The `draws` list and
vertex totals describe carried guest draws, and `snaps` reads the chosen
passes before the following graph operations. `private_frame` identifies
the source scene, present and lighting mode. Its `passes` counts logical
attachment runs. The backend's `render_passes`, `backend_draws` and
`submits` include work caused by requested snapshots. The private
recorder measures wall time from graph entry to final readback.
Omitting `source` or specifying `"display"` reads or arms the display
recorder independently. A request for `"private"` returns `UNAVAILABLE`
when the host has no private scene renderer.

### `render.stats`

No params. `interp` is `d3d::interp::InterpStats` with every field
(frames blended and held, replays, warps, and the hold reasons `unsure_*`,
`mismatch_*`, `dropped`), read from its `Debug` text so fields added there
appear with no change here; `pipeline` is the pipelined render's totals
(`pipelined`, `completed`, `serial` reasons, `fallbacks` by call, joins,
snapshot coverage, and times in ms including `overlap_ms` for the worker's
execution before the game-thread join); `counters` is the frame-stats
window's line. Each is `null` where that piece is not running.

`display_frame` gives the identity, lighting and backend counter deltas
when the worker's latest presentation used the native graph. An
original presentation clears it. The frame recorder can stay unarmed
for these counters. `render.scene` includes them when the map epoch,
frame and tick match its sealed scene.

`private_frame` reports the counts and identity of the most recent
completed private target draw. It is absent before the first capture and
after a renderer reset. `render.scene` includes the same field when its
sealed scene matches the private capture's map epoch, frame, tick and
summary hash.

A completed native E3 draw includes an `e3` object in its
`display_frame` or `private_frame`. The armed `render.frame` record
includes it for the corresponding draw. Its `passes` array contains
`e3.tiles`, `e3.sun`, `e3.marks` and `e3.lights`. Each row has
`name`, submitted `draws` and `gpu_ns` (null for an empty or unmeasured
stage). Snapshot indices refer to the attachment runs in the frame
record. The six per-frame counters are `lights_lit`, `lights_shadowed`,
`lights_over_cap`, `sun_tiles`, `sun_tiles_redrawn` and `atlas_halved`.
They count the E3 renderer's selections and atlas work. Slice L3 replays
the guest's own lights in both modes, so its E3 counters and pass draw
counts are zero. The guest's light selections are in `render.scene.views`.
Retail graph draws omit `e3`.

`interp` exists when in-between frames run: in `h2 play`, or in an `h2
serve` that renders on the GPU off-screen (`H2_BOOT_GFX`) with an
interpolation switch on. The in-between frames are paced on the wall
clock at the frame rate and made and classified for real, so the hold
reasons are the game's ([high-framerate.md](../game/high-framerate.md),
"Headless"). `interp.holds` lists the draws a held frame was held for,
each with its guest draw and pass number (as `render.frame` numbers
them), vertex program, transform and where it lands
([high-framerate.md](../game/high-framerate.md), "Held frames, draw by
draw"). Without `H2_BOOT_GFX`, with `H2_RENDER_THREAD=0` or under
`impl.render.pipelined_frame`, `interp` is `null`. `H2_BOOT_GFX_BETWEEN=N`
writes the first N in-between frames as PNGs, with the guest frame before
and after each ([switches.md](switches.md)).

```sh
H2_OPT=2 H2_BOOT_GFX=$TMPDIR/h2-gfx H2_BOOT_GFX_EVERY=never \
  H2_FPS=60 H2_FEATURES=impl.render.reproject \
  h2 serve disc/halo2.iso --out <the image you built>
```

### `render.coop_look`

A read-only host method registered with the Live co-op prediction
feature (`crates/extensions/game/src/coop_look.rs`). With no parameters it returns
the most recent 256 rendered local views, oldest first; `{"after":123}`
returns samples newer than that cursor. `latest` is the most recent
sample number, initially zero; a gap in sample numbers means the reader
fell behind. One frame can contain several local users.

```sh
h2 agent --socket ~/Library/Caches/h2recomp/coop-bravo.sock render.coop_look
```

Each entry in `frames` has a monotonic `sample`, the guest render `frame`,
the local `user` (0 to 3), and a `status` such as `active`, `waiting`
(awaiting a baseline), `host`, `setting off`, `cinematic`, `dead`,
`director`, `vehicle`, or `no local control`. These are guest draws;
additional display-rate replays reuse the guest frame.

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
`crates/extensions/agent/src/compare.rs`, `crates/extensions/agent/src/render.rs` and
`crates/extensions/agent/src/sim/hashes.rs`, host glue in boot's `agent_glue`,
`agent_frames`, `d3d_diag`, `d3d_glue` and `scene_glue`. Tested in
`crates/extensions/agent` over synthetic images, the frame recorder's rig and the
sim report's tally.

```sh
h2 agent frame compare off/frame-03000.png on/frame-03000.png --diff d.png  # no game
h2 agent render.spawn_view '{"index": 0}'
h2 agent render.capture '{}'
h2 agent render.spawn_view '{"reset": true}'
h2 agent render.capture '{"tick": 9000}'
h2 agent render.capture '{"pass": 3}'          # the next tick's frame, pass 3
h2 agent render.capture '{"lighting":"retail"}' # latest sealed scene, private draw
h2 agent render.capture '{"lighting":"e3","tick":42}' # use the first reply's tick
h2 agent render.scene '{"dump": true}'
h2 agent sim.hashes '{"from_tick": 3000}'
h2 agent render.parity '{"arm": true}'
```

| Method | Access | Params | Result |
|---|---|---|---|
| `render.compare` | read | `{a, b, diff?}`: two PNG paths, and where to write a difference image | `{size: [w, h], differing, max_channel, mean, first: [x, y] \| null, within_bar, diff?}` |
| `render.spawn_view` | act | `{index, yaw_offset?}` or `{reset: true}` | `{index, count, position: [x,y,z], yaw}` or `{reset: true}` |
| `render.capture` | read | `{tick?, pass?, lighting?}` | `{path, tick, present, pass?, lighting?, scene_hash}` |
| `render.scene` | read | `{dump?}` | `{sealed, map_epoch, frame, tick, hash, cut, views, counters, entries?, carried?, carried_census?, carried_activation?, dump?}` |
| `render.parity` | act | `{arm: true}` | `render.compare`'s fields plus `tick` |
| `sim.hashes` | read | `{from_tick?, max?}` | `{rows: [{tick, gameplay, arena?}], dropped, resets, truncated?}` |

- **`render.compare`.** Reads both files (any PNG colour type, decoded to
  8-bit RGBA) and compares the colour channels; alpha is not compared
  (frames are written opaque). `differing` counts pixels with any channel
  off by more than 2; `max_channel` is the largest difference of one
  channel; `mean` the mean absolute difference over every channel of
  every pixel; `first` the first differing pixel in row order, `null`
  when none. `within_bar` is the parity bar: `max_channel <= 8` and
  `differing` at most 0.01% of the pixels (one in 10,000). With `diff`,
  the difference image goes there (directories made): differing pixels
  in red, brighter the further off; pixels off by 1 or 2 in dark blue;
  the rest the first image at a quarter brightness. Errors are
  `INVALID_PARAMS` with `data.error` naming the kind: `size_mismatch`
  (with `data.a` and `data.b`, each `[w, h]`), `read` or `write` (with
  `data.path`). The decode runs on the game thread; `h2 agent frame
  compare A B [--diff D]` runs the same function in the CLI with no
  socket and exits 1 outside the bar.
- **`render.spawn_view`.** Selects a player starting location from the
  loaded scenario by its zero-based index. The eye is 0.64 world units
  above the start, looking along its facing. `yaw_offset` turns it in
  radians. The reply includes the number of starts and the eye position
  and heading used for the picture. The method answers when the world
  renderer accepts the private view. A following `render.capture {}`
  takes the next tick's frame. The camera stays selected until reset or
  the next game activation. A new selection interrupts a pending request.
  An invalid index gives `INVALID_PARAMS`, a missing or unreadable start
  gives `UNAVAILABLE`, and a session that draws no world frames times out.
  `crates/extensions/game/src/agent_spawn_view.rs` shares Theater's
  playerless world view in `spectator_view.rs`.
- **`render.capture`.** With `lighting: "retail" | "e3"`, the native
  renderer draws a sealed scene into a private target. The default is
  the latest sealed scene. An explicit `tick` selects that exact scene
  tick, up to 240 ticks ahead of the latest seal. The draw uses the
  resolution shown for that particular present, including a frame the
  backend showed from its 1x reference. The answer includes the tick,
  present, summary hash, lighting and a fresh PNG path. The most recent
  successful private capture retains its scene for another request with
  the same tick. Pass both
  paths to `render.compare` to compare the modes while play continues.
  A renderer reset releases the retained scene. A missing private
  renderer or an unreplayable scene gives `UNAVAILABLE`. Combining
  `lighting` with `pass` gives `INVALID_PARAMS`. The override applies
  only to the private draw.

  Without `lighting`, the frame is taken for game tick `tick` (as `at.tick`
  counts it; default the next one, at most 240 ahead). On the D3D path
  the frame of a tick is the present that follows it: the first guest
  present after the tick has ended. `tick` in the answer is the last tick
  that ended before that present, which is later than the one asked for
  when the game ran two ticks in one frame; `present` numbers it as
  `render.frame` and `H2_BOOT_GFX`'s `frame-N.png` do. Without `pass`,
  the image is the presented frame at the render scale, the same image
  `H2_BOOT_GFX` writes as `frame-N.png`, written as
  `frames/capture-<pid>-<seq>-tick-<tick>.png` under the cache directory. That
  needs a run that renders on the GPU with `H2_BOOT_GFX` (use
  `H2_BOOT_GFX_EVERY=never` to write nothing else); without it the answer
  is `UNAVAILABLE`, and under `H2_BOOT_GFX_LIGHTING_AB` too. With `pass`,
  the frame recorder (`render.frame`) is armed at the end of the tick
  before and reads that pass back at 1x as `render.frame`'s `snap` does
  (the file is the snapshot's); it replaces a `render.frame` arming not
  yet done, and one made while it waits makes it answer `INTERRUPTED`. `scene_hash` is the sealed scene's
  summary hash at that present (from `render.scene`'s source); `null`
  with no source registered. The answer waits for the
  frame (up to the agent's 300-tick limit for deferred answers); a
  paused game holds it. A new `render.capture` replaces one still waiting,
  which answers `INTERRUPTED`. Unasked, the cost is one relaxed load per
  tick and per present. Capture does not wait for unresident scene
  content (framescene.md 9.2).
- **`render.scene`.** The last sealed `FrameScene`, from the source the
  frame step registers (`agent::render::set_scene_source`, in boot's
  `scene_glue` over `render_bridge::scene_step`): whether
  one has been sealed, its map epoch, frame, tick, summary hash and cut
  reasons; per view (`key` is `kind:player:view`, `game:0:0`, the first
  person's `first_person:0:0`) whether it
  cut, its clusters, instances, parts, rows, guest-selected lights
  (`lights`), all light candidates including those selections
  (`lights_candidates`), candidates with both constant sets and any
  required projection (`lights_with_constants`), shadow maps (its
  lights that cast their shadow), its lights by each layer their passes
  draw (`light_layers`, `"10"` to `"14"`), the items of its typed layer
  draws by layer (`layer_items`) and those the guest still draws
  (`layer_remainder`), the
  instances the guest still draws (`remainder`), its parts by how they
  draw beyond their material (`part_flags`: `stencil_mark`, `strip`,
  `subparts`, `aux_depth`;
  framescene.md, slice 3), its model decorator draws (one per
  placement), their instances and the draws the guest still draws
  (`decorators`, `decorator_instances`, `decorator_remainder`;
  native-decorators.md), its late passes when they ran (framescene.md,
  slice 5): `fog` (layer 16) and `water` (layer 6), each `{draws, items,
  materials, parts, drawn, kept, device_words}` (whether the layer's own
  gate held, its items in the list, the materials and parts the native
  stage draws, whether the stage took the layer, the items the guest
  still draws, the device fog words of 7 that read), and `post`,
  `{colour, late, view_kind, screen_effects, screen_effects_gated,
  colour_pass, effects}` (`render_screen_effects`' arguments and gates,
  whether the colour pass ran, the screen effects drawn under it); each
  absent when its draw did not run in the view; `carried`, its ops by
  slot from the scene sink's last frame (empty without
  `frame-scene.sink`, and for a first-person view), and `camera_use`
  counts the retained guest draws for that scene and game view,
  including its overlay. `screen` is pretransformed or passes screen
  positions through, `view` has the view's verified projection block
  and viewport, and `unknown` covers the other draws. The map is
  empty for other view kinds or without a retained matching scene.
  The top-level `counters` include
  (`scenes_sealed`, `scenes_unsealed`, `scene_views`, `scene_no_camera`,
  `scene_refused_writes`, `scene_layers`, `scene_layer_items`,
  `scene_layer_remainder`, `scene_layers_unread` (layer draws whose list
  did not read), `scene_decorator_draws`, `scene_decorator_instances`,
  `scene_decorator_remainder`, and the decorator stage's
  `scene_decorator_views`, `scene_decorator_views_native`,
  `scene_decorator_items_skipped`, `scene_decorator_draws_unready` and
  `scene_decorator_views_declined` (native-decorators.md),
  `scene_light_extract_views`, `scene_light_extract_ns` and
  `scene_light_extract_unread` (candidate extraction attempts, their
  elapsed nanoseconds and attempts that return no candidates because
  the input could not be read), `scene_light_groups`, `scene_light_groups_whole` (the
  gathered light and shadow layers in the scene, and those with every
  sorted entry in it), `scene_light_layers_alone` (light layers drawn
  from the scene with no handler run), `scene_light_objects_alone`
  (their object entries: model parts drawn with no handler),
  `scene_light_entries` and
  `scene_light_entry_remainder_<cause>` (sorted entries left to the
  guest's handlers, by `scene_extract::light_groups::Cause`: `memory`,
  `callback`, `owner`, `section`, `range`, `state`, `subparts`,
  `material`, `pose`; framescene.md, slice 4),
  `scene_fog_views`, `scene_fog_gate_held`, `scene_fog_drawn`,
  `scene_fog_parts` and the same four for `scene_water_` (late layers
  written, of those whose gate held, those the native stage took, and
  the parts it took), `scene_post_views`, `scene_post_colour`,
  `scene_post_effects` and `scene_post_unplaced` (post calls whose words
  did not read, and colour passes or screen effects with no post call of
  their view; slice 5), and from `d3d::scene_stage`
  `scene_stages_<stage>`, the scene stages queued to the render thread,
  and `scene_dropped_<stage>`, those it could not record after the guest
  skipped their items, for `world` and `objects` (framescene.md, slice
  2) and `fog` and `water` (slice 5); with `frame-scene.layers`
  in `H2_FEATURES`, `layer_plan_asked`, `layer_plan_drawn`,
  `layer_plan_items` and the plan's outcomes by layer,
  `layer_plan_<layer>_taken` and `layer_plan_<layer>_refused_<cause>`
  (`render_bridge::native_layers`; framescene.md, slice 4); a later slice adds
  `scene_held_draws`). With the [scene sink](../graphics/framescene.md#slice-6-the-renderer-owns-game-view-frames)
  enabled, the device files
  each guest D3D op by the frame's slot (`opaque`, `after_opaque`,
  `after_lights`, `transparents`, `after_post`, `overlay`, `outside`)
  for both the original renderer and the frame owner. `carried` describes
  the last recorded frame:
  `slots`, each slot's `{draws, binds, clears, copies, uploads,
  samples}` (slots with none left out), `side_targets`, each surface
  other than the scene's presented colour and its depth that a game
  view's ops name, `{aux, width, height, format, depth, ops}` (`ops` by
  slot), and `readbacks` and `readbacks_outside`, the guest's reads of
  what was drawn in and outside a game view by kind (`surface`,
  `depth`, `mirror`, `pixel_count`); and the counters add
  `scene_carried_frames`, `scene_carried_draws`, `scene_carried_ops`,
  `scene_side_target_frames` (frames that named a side target in a
  game view), `scene_readback` and `scene_readback_<kind>` (in a game
  view, since the start). `carried_census` keeps transient uses across
  all recorded frames. `frames` counts sink presents from one and
  `game_frames` counts those with guest D3D ops in a game view.
  `targets` groups game-view side surfaces by `width`, `height`,
  `format`, `depth`, `slot` and `operation`. Operations distinguish
  attachment binds and clears, the source and destination of a copy,
  uploads, snapshots, in-place samples and depth samples. `reads`
  groups guest readbacks by `slot` and `kind`, including the overlay
  and outside slots. Each row gives its `count`, `first_frame` and
  `last_frame`. Compare counts at two boundaries to isolate a window.
  On a direct level boot, `carried_activation` records the loaded map, its
  activation present, sink frame and a snapshot of process counters. A row
  whose `last_frame` exceeds `carried_activation.frame` occurred after loading.
  A threaded GPU sink also reports `scene_window_*` counters from one device.
  `start` is its original Swap at activation and `through` is the last
  completed Swap. The interval excludes `start` and includes `through`.
  `owner_enabled` is 1 when this device selects a native frame owner.
  `gaps` counts missing or out-of-order joins. `games` counts completed
  frames that entered a game view or issued a game-view operation.
  `owned` and `ordinary` count those game frames by actual display path.
  `failed` counts failed presentations across the interval. `stage_drops`
  includes guest-side and worker-side stage drops. For game frames,
  `unresident` counts omitted instances and `unknown_residency` counts
  frames without a complete observation. `palette_views` counts game-view
  entries sampled for native map-palette residency. `palette_pending_frames`
  counts game frames where a view entered with a known incomplete palette.
  `palette_unknown_frames` counts game frames where a view lacked a valid
  palette observation or game work had no recorded view entry. Each frame
  retains observations from every split view. `read_surface`, `read_depth`,
  `read_mirror` and `read_pixel_count` count guest requests in game views.
  `flare_queries` counts original game-view reports installed by the
  owner. `unmatched_flare_frames` counts owned frames where that number
  differed from the original requests on the same Swap. Each read of
  these counters covers a single completed interval, even when the
  worker is drawing a later frame. The guest continues running.
  The per-frame `aux` number appears only under `carried.side_targets`.
  The cumulative census is available on the census-only path. Rendering
  workers omit this extra scan. Both paths count
  `scene_carried_game_frames`, the guest presents with a D3D op inside
  a game view. With an installed frame owner, `scene_owned_presented`
  counts successful native presentations and `scene_owned_declined`
  counts presentations that take the ordinary backend path, including
  frames outside game views. `scene_owned_flare_queries` counts
  game-view pixel reports whose native result the owner installed.
  `scene_owned_candidates` counts sealed game frames paired with their
  guest presents and checked by the display worker. For each rejected
  candidate the worker records the first cause. `scene_owned_barriers`
  counts frames whose original commands had to run before Swap, for example
  for a guest read. `scene_owned_mismatches` counts disagreement between
  the seal and the queued commands or Swap parameters.
  `scene_owned_target_refusals` counts missing physical attachments and
  frames the physical backend cannot reproduce at the active scale or mode.
  The private renderer reports its own publication queue in
  `scene_private_published`, `scene_private_taken`, `scene_private_replaced`
  and `scene_private_waiting`. In paced mode a newer seal can replace an
  unread private scene. In locked mode at most two scenes wait and each
  reaches the reader. These counts describe capture availability.
  With the sink and a render worker, `scene_residency_frames` counts
  presents where the worker observed a draw, a committed scene stage,
  an owned frame or a failure. `scene_unresident` counts distinct
  object handles per present whose dropped work lacked the planned
  GPU model, geometry buffer or material program.
  `scene_residency_unknown_frames` counts frames with an unclassified
  drop, a skipped guest packet or a worker failure. The counters appear
  after the first observation. A post-load zero requires a positive
  frame delta and zero growth in both omission counters. Private
  captures have no effect. Legacy refusals can keep their guest draws.
  Check native ownership across the same game views to measure native
  coverage.
  The raw `scene_readback_pixel_count` includes these requests.
  Compare raw and native counts at the same recording boundaries
  to identify requests that still need the reference.
  Both carried records are absent until the sink records a frame.
  Without the option the sink counters are also absent. With
  `frame-scene.census` in `H2_FEATURES`, `entries` is the entry census
  (`render_bridge::entry_census`; framescene.md, slice 4), totals since
  the start: `by_entry`, keyed by guest entry (`layer.<site>.<layer>`
  with site `scene`, `alternate`, `first_person`, `light`,
  `object_shadow` or `other`, the late passes' fog and water calls
  being `layer.scene.16` and `layer.scene.6`; `light_passes`,
  `object_shadow`, `screen_effects`, and under it `colour_pass` and
  `screen_effect`), each `{calls, d3d_draws, seed_steps,
  effects_seed_steps, seed_unmatched, animation_gets,
  particle_updates}`: the guest draws handed to the D3D backend under
  the entry, the steps of the shared random seeds `+0` and `+4`, calls
  whose seed was not a run of steps, and calls of
  `animation_resource_get` and of the particle systems' per-view update
  under it. Entries nest and each counts what is under it.
  `unbalanced` counts entries whose exit never ran and exits with
  nothing open; `draws_counted` is false when no D3D draw counter is
  registered. Without the option `entries` is absent. With `dump`, the whole scene is written as JSON
  under the cache directory (`scenes/scene-<frame>-<tick>.json`) and
  `dump` names the file. With no source
  registered: `UNAVAILABLE`, "no scene has been built".
- **`render.parity`.** `{"arm": true}` requests one game frame from the
  sealed scene's private renderer and the D3D display. The sink retains
  the original guest draws while armed. A frame that already admitted
  a typed stage before arming gives way to the next complete guest
  frame. The reference readback uses the ordinary display surface,
  before gamma, at its display scale. The reference requires Retail display
  lighting. An E3 display returns `UNAVAILABLE` because its scaled shadow
  maps use a different filter from the sealed renderer's baseline. The
  worker draws that present's sealed inputs on fresh targets at the
  observed scale. Smaller auxiliary levels retain their guest dimensions.
  The reply carries the sealed scene's `tick` and the pixel comparison
  returned by `render.compare`. The comparison covers the carried
  guest commands. Pixel parity of a typed substitution needs a separate
  reference run exercising that stage.
  The request completes at a tick boundary after both render paths
  finish. A new arming interrupts one still waiting. Unavailable
  inputs or a missing reference or private renderer give `UNAVAILABLE`.
  A request that receives no game frame within the deferred request
  limit times out. `{"arm": false}` gives `INVALID_PARAMS`.
- **`sim.hashes`.** The running game's post-tick digests in the rows sim
  mode writes to its report as `state_hashes_v1` (sim.md §6): `tick`,
  `gameplay` (16 hex digits) and, under `impl.sim.arena-hash`, `arena`
  (the game-state arena's hash). Boot's `agent-frames` tick observer
  computes them as sim mode does (the same digest function and the same
  `impl.sim.metrics-projection` switch) on every tick where the players
  can be read, on any run with an agent socket, rendered or not. It keeps
  the last 108,000 rows (an hour of ticks, about 2.6 MB), dropping the
  oldest (`dropped` counts them); a tick before the last, or more than 30
  after it (a new map, a snapshot load), starts the series again
  (`resets`). `from_tick` and `max` pick rows; `truncated` says more
  matched. Compare two launches row by row:

  ```sh
  diff <(jq -c '.rows[]' off-hashes.json) <(jq -c '.rows[]' on-hashes.json) | head
  ```

## Load timeline

`load.timeline {"last": N}` is a core method (8 by default, 1 to 32 kept;
`crates/extensions/agent/src/loads.rs`).

```sh
h2 agent loads                     # the last 8 loads, BSP switches and stalls
h2 agent wait '{"phase":"gameplay","map_ticks":30}' && h2 agent loads 1
```

- **What a load is.** Read at each tick: the map changed (`map`), the BSP
  changed (`bsp`), or no tick came for 500 ms (`stall`, the point the
  `loading` phase starts). The run's first tick ends a `boot` load timed
  from when the agent started. Each load has a `stall` segment (the last
  tick before to the first after) and a `settle` segment (the next 30
  ticks); a load cut short by the next one says `settled: false`.
- **What each segment counted.** The difference of the frame-stats running
  totals (`timeline::phase::totals`) across it: frames, ms per phase
  (`cpu`, `gpu-wait`, `render-wait`, ...) and each counter that moved
  (`read_bytes`, `uploads`, `shaders`, `pipelines`, `readbacks`, ...).
  Frames are counted by present, so a load screen that presents shows its
  frames; one that does not shows the stall's time only. With frame stats
  off, `no_frame_stats` says the counts are missing.
- **Now.** While no tick has come for 500 ms, `current` is the stall so far,
  with what it has counted.
- Seeks and cache hits are not counted (the kernel's file I/O counts bytes
  and the host time of each file call: `read_bytes` and `file_io_us`).

## `host`: the process and its lobby

### `host.perf`

The newest host perf line (`boot::host_perf`): `{line, age_ms}`;
`UNAVAILABLE` until one exists (`H2_HOST_PERF=1`; one every 100 frames or
5 s). `h2 agent hostperf`.

### `host.metrics`

`h2 agent hostmetrics`; no parameters. It reports the latest complete
window of 30 intervals between returns from `GAME_TICK` (`0x00137fe0`).
The first reading is available after 31 tick returns; until then it
answers `UNAVAILABLE` ([`crates/extensions/game/src/host_metrics.rs`](../../crates/extensions/game/src/host_metrics.rs)).

| Field | Meaning |
|---|---|
| `ticks` | Tick intervals in the sample (30) |
| `game_cpu_ms_per_tick` | CPU time on the game thread per tick interval, including work between ticks |
| `process_cpu_ms_per_tick` | CPU time on all process threads per tick interval |
| `wall_ms_per_tick` | Elapsed wall time per tick interval, including pacing sleeps and any pause or load crossed by the window |
| `wall_ms_per_tick_p99` | On a light host, nearest-rank p99 of individual multiplayer tick intervals ending in the last 30 s. The first tick on entering gameplay starts the clock. An empty window gives `null`. |
| `late_ticks` | Number of intervals in that same window longer than 36 ms. Both interval fields are `null` outside the light profile. |
| `cpu_percent` | Whole-process CPU over the wall interval; 100 is one fully used core |
| `cpu_seconds` | User and system CPU seconds consumed by this game process through the request |
| `rss_bytes` | Resident bytes at the end of the interval on macOS and Linux, including resident shared pages |
| `pss_bytes`, `peak_pss_bytes` | Current and highest sampled Linux proportional resident bytes. Each `host.metrics` request samples the process. |
| `private_bytes`, `shared_bytes` | Private and shared resident mappings in Linux smaps, each including clean and dirty pages |
| `private_dirty_bytes`, `shared_clean_bytes` | Linux smaps Private_Dirty and Shared_Clean in bytes, for comparing copy-on-write and file sharing |
| `age_ms` | Time since the 30-tick CPU sample ended |
| `host_light` | Whether the process uses the light host profile |

A platform counter that is unavailable is `null`. Compare active
gameplay windows after loading; a stale sample can survive a gap in ticks.
The CPU fields exclude sleeping; the wall field shows the pace the host
sustained.

### `host.audit`

Set `H2_HOST_AUDIT=/path/to/audit.json` before boot to register this
method (`crates/extensions/game/src/host_audit.rs`). `h2 agent hostaudit` takes no
parameters. It writes a snapshot to the configured path and returns the
path, ticks, observed and registered function counts, tag range count and
inclusive CPU timing. Each snapshot also flushes any enabled map read
recording for the private store build. An orderly exit writes the final
snapshot. Call the method before terminating a dedicated process externally. The collector
changes dispatch and memory-watch costs, so use `host.metrics` for sizing.
Report fields and corpus: [sim.md](sim.md#host-audit-corpus).

### `host.lobby`

A System Link light host started with `H2_HOST_LOBBY=1` registers these
four game methods (`crates/extensions/game/src/host_lobby.rs`, types
`crates/extensions/agent-proto/src/host_lobby.rs`). Each result also carries `at`.

| Method | Parameters | Result |
|---|---|---|
| `host.lobby.open` | `{"map":"<short name>","variant":"<name>","max_players":8,"live":false}` | System Link: an empty object. Live: the advertised `session_id` |
| `host.lobby.state` | `{}` | `{"phase":"lobby","players":[{"slot":3,"peer":1,"name":"<name>"}],"ready":true}` |
| `host.lobby.start` | `{}` | an empty object when retail accepts the start request; observe `state` for load and play |
| `host.lobby.close` | `{}` | an empty object once the network lobby has closed and the host is back at the menu |

The host looks up `map` by short name, scenario path or English display
name. The optional `variant` defaults to Slayer; it accepts a disc
variant, a signed and valid saved variant present at boot, a stock game
type or a built-in Live playlist variant key, case-insensitively. If a
name matches two different saved or disc rulesets, open is
`INVALID_PARAMS`. `max_players` is an integer from 1 to 16. The required
`live` boolean selects System Link or a Live custom game; Live needs a
working service identity that can publish sessions. Open waits for retail
to publish the requested lobby parameters. Repeating the same open in
pregame returns the existing lobby; close it before changing parameters.

The phases are `menu`, `lobby`, `loading`, `playing` and `postgame`.
Player slots are retail membership slots and can have gaps; split-screen
players on a remote console share a peer index. An empty, joinable lobby
reports `"players":[]` and `"ready":false`. Readiness needs a remote
player, party leadership, the requested map and limit, loaded maps on every
peer, and retail's ready-to-start result. Any connection may read state;
mutations respect `session.claim`, and a concurrent mutation is `BUSY`.
[Playerless lobby and the two-console check](../live/dedicated-host.md#playerless-lobby).

## `ui`: what the menus show

`ui.screen` needs `H2_UI_WATCH=1`. The other UI methods read the current guest widgets directly. All of them are read-only except `ui.press`.

- **`ui.screen`** (`h2 agent ui`): the menu screen player one's input goes
  to, read each UI update, including during a campaign pause
  (`game::ui_watch`): `{screen, window, slot, class?, live_menu_row, mode,
  dialog, age_ms}`, or `UNAVAILABLE` before the first read. `class` names
  a Rust list screen whose numeric ID identifies its retail skin.
  The pause Settings lists report `pause-settings` or `mp-pause-settings`.
  Video and Voice report `video` and `voice`. All four use screen 200 in
  a game. Press B or Back with `h2 agent press` (`input.pad`) and check
  that the previous class returns on the same window and slot. See the
  [pause Settings walkthrough](../game/settings-menu.md#to-check-in-game).
  `main_menu_theater` is present and true when Theater occupies the
  row after Xbox Live. Lobby scripts use it to locate Split Screen and
  System Link. `dialog` is the dialog up (the first screen on windows 0 to
  4, any slot), or `null`: `{id, name, window, slot}`, with `name` the dialog's
  recognized name (`dialog_live_signin` for the account picker,
  `confirmation_dialog`, `error_dialog_ok`, ...; `boot::ui_watch::dialog_name`)
  or `null` for an id not named yet. The party leader picker reports
  `dialog.name = party_leader_picker` with id `0xb1`. The Clanker player
  card reports `clanker_player_options`. The retail-style value picker
  used by Theater reports `kit.picker` in the main menu and
  `kit.picker.game` in game. These names identify the active kit class.
  Their numeric ids identify the shared retail base class. Dialogs read as
  `"screen": null`, so this
  is how a driver tells one from another. The `ui: screen none` log line
  names it the same way ([live-coop.md](../live/live-coop.md), 12.6).
- **`ui.stack`** (`h2 agent ui.stack`, no parameters): a tick-boundary
  read of the occupied channels, their active, pending and outgoing
  screen instances, and the game-shell channel's saved screen IDs
  (`history`, nearest parent first). Each screen has `id`, `instance`
  (a guest address valid for that screen's lifetime), and `parent` where
  a return is known. The map picker stages share an ID and have
  distinct `mode` values (`folders`, `maps`, `pack`), with the open
  category or pack named in `folder`. A pack's parent is the Mods list. All stages show the active picker row in `focused_row`
  (zero-based). `input` identifies the screen taking player one's menu
  input. `back` gives the instance that closes and its parent where
  known, with action `close`, `reopen` (recreate a parent), or `wait`
  (consume Back during a replacement). It is null when there is no menu
  input target or when Back depends on a screen's custom handler.
  `ui.stack` is available without UI watch.
- **`ui.campaign`** (`h2 agent ui.campaign`, no parameters): a tick-boundary
  read of the campaign menu from the guest screen and focus, without UI
  watch (`crates/extensions/game/src/campaign_menu.rs`). With no installed Halo 1
  campaign it returns `{"installed":false}`. With one it reports `page`
  (`picker`, `options`, `levels`, `difficulty`, `dialog` or `other`),
  `game` (the selected campaign key on the three game-specific pages),
  `rows`, `focused_row` (zero-based) and `focused`; unknown selection or
  focus is null. On the picker the rows are installed campaign keys.
  With a focused picker row, `card.description` is the UTF-8 text in
  that row's guest string slot, or null before it is available.
  `card.picture_datum` is the selected bitmap datum, or null when the
  artwork area is empty. `card` is null on other pages or without a
  focused row. On Campaign Options the action keys are `resume`, `new_game`,
  `select_level` and `co_op`; other pages report an empty row list. For
  example, a fresh profile on Halo 2 has `"page":"options"`, `"game":"halo2"`
  and three rows. With a saved Halo 2 checkpoint, Resume is the first of
  four rows.
  On the picker and Campaign Options, `pick` identifies the most recent
  press during this visit. It is null before a game is picked. Its
  `campaign` is the picked numeric id, and `request` is null until retail
  receives the options screen request. The request gives the player
  `mask`, `window`, `slot` and `loader` arguments. The active `page`
  shows whether the options screen opened.
  On those two pages `list` reports guest addresses for the `screen`,
  its list `handler`, the `row_array` and the linked `children` in order.
  `row_count` reads the number of active datums. On Halo 2 with a checkpoint
  the four actions scroll through three linked item widgets. The handler's
  `press_record` holds a `press_target` (the list) and a `press_function`
  (the row-chosen callback). An unreadable field is null. On other pages
  `pick` and `list` are null.
  From the main menu's Campaign row, `h2 agent press a` opens the picker for
  fresh and returning profiles. Main menu row zero requests the native
  campaign dialog from `0x230888`; the picker intercepts it before the
  later profile-dependent solo branch at `0x2b16ad`. With UI watch on,
  `lobby mode campaign` also recognizes the installed picker.

- **`ui.lobby_options`** (`h2 agent ui.lobby_options`, no parameters):
  `{"lines":[...]}` with the active custom game panel's visible Quick
  Options lines in display order, up to six. This reads the lobby's text
  widgets at a game boundary (`crates/extensions/game/src/lobby_options.rs`).
  It returns `UNAVAILABLE` when the custom game pane is unavailable.
  Open a pregame custom lobby to read its retail lines and any Clankers,
  Halo 1 Options and Skulls summaries. After changing the settings,
  close the dialog and read the panel again.

- **`host.peer_rtts`** (`h2 agent host.peer_rtts '{}'`) reads the active simulation
  membership's reliable-channel RTT at a game boundary. The result contains
  `players` with the same slot, peer, name, bars and nullable RTT as
  `ui.lobby.players`. Local players, phantoms and unsampled routes have
  null RTT. The method returns `UNAVAILABLE` without a simulation membership.
  Compare a remote client's `rtt_ms` to its matchmaking assignment's regional
  probe sample during the match.

- **`ui.lobby.players`** (`h2 agent ui.lobby.players`, no parameters):
  reads the pregame player list on a game boundary without UI watch
  (`crates/extensions/game/src/lobby_quality.rs`). Each entry has its
  membership `slot`, console `peer`, `name`, `bars` (1 to 6) and `rtt_ms`
  (milliseconds, or null when there is no network sample). Split-screen
  players on a console share its peer and bar count. Local players and
  phantoms have six bars and null RTT. Remote peers with no sample have
  one bar. The list uses the squad's membership in custom games and the
  target membership when the matchmaking lobby has one. Results are in
  slot order, with an empty list outside pregame.
  For example, `{"players":[{"slot":1,"peer":1,"name":"Clanker","bars":6,"rtt_ms":null}]}`.

Screens already moved to Rust (the Video screen, the skulls page, the
account, co-op and sign-in screens; [feature-engine.md](../engine/feature-engine.md))
share one list-screen implementation (`crates/extensions/game/src/list_class.rs`).

### Menu inspection and navigation

`ui.audit {}` inspects the instantiated widgets at one game boundary.
`screen` is the underlying menu. `dialogs` contains the active dialog
on each occupied dialog channel, including dialogs belonging to another
controller. `active` is the surface that accepts player one's input.
`target` is its opaque token, and `focused_row` is the zero-based focused
item. A transition or ambiguous input receiver can leave these fields null. Each surface
has a numeric `id`, `window`, controller `slot`, current `pane` and
the kit class `name` when available. Kit dialogs can share a retail ID.

Inspect `panes[].labels` and `panes[].lists[].rows` for the text at the
widget. The audit reads the UTF-16 buffer *after* retail's localized
string lookup. A row includes its datum index, text and individual labels.
On the retail profile picker and action menu, `navigation` identifies a
verified inspection action in the current list mode.
A missing widget, an empty string and an unreadable text interface have
distinct statuses. The list's `item_count` includes off-screen items.
Rows describe the materialized viewport. Moving focus exposes the other
rows of a scrolling list. The Y menu exposes its materialized tabs and
marks the selected pane. Source and widget layout are in
[`ui_audit.rs`](../../crates/extensions/game/src/ui_audit.rs) and
[User-interface tags](../formats/ui.md).

`issues` identifies affected surfaces, panes and rows. It includes
missing or empty row text, empty labels, unreadable data, empty panes
and undersized lists. `transition` includes the startup fade time
remaining, scene fade mode and weight, and the active, pending and
outgoing layers of the menu compositor. A modal can request a blurred
backdrop for as long as it is open. `stuck_blur` means a layer requesting
that backdrop still carries its input-blocking transition more than
one second past its authored delay and duration. Other overdue animations
produce `stuck_transition`. `stuck_fade` means audits sampled the same
unfinished main-menu startup fade and clock reading at least 150 game ticks
apart. Unknown backdrop accessors give `requests_backdrop: null`.
These diagnostics describe compositor inputs. The final image still
requires a rendering check. [Transition reader](../../crates/extensions/game/src/ui_transition.rs)

`ui.press` accepts `{target, action, row?, phase?}`. Copy the token from
the latest audit. Actions are `up`, `down`, `left`, `right`, `choose` (A),
`back` (B), `friends` (Y), and `open_live_menu` (A on the Live players
screen in mode 3). Move focus, audit again, then send `choose` with the
observed row. A changed owner, row or requested phase invalidates the
press. The XAPI tap checks the target again when the game reads the pad.
A successful result contains `{delivered:true, target, action, row,
player, at}` after the press and release. An unread gesture expires. Read
`ui.audit` after the menu transition settles to check what the press opened.
`session.methods` lists `ui.press` when the process has an XAPI input tap.

### Crawling the menus

On a running agent-enabled game, place a signed-in profile with a recent
Clanker at the main menu. Set `H2_AGENT` to its socket path.

```sh
h2 agent --socket "$H2_AGENT" ui-crawl --depth 6 --budget-ms 120000 --require-clanker
```

Omit `--socket` to use the normal agent endpoint. The crawl claims one
connection, explores known navigation actions, observes each destination
and backs out before trying its siblings. It visits the Y menu and
Recent Players, including entries for players who left the session.
From Settings it can inspect an existing editable profile, open Controls
and back out of the choice dialogs without selecting a value.
It skips game starts, quit, unverified actions and changes to settings or
accounts. `--from-current` starts at the open menu or dialog for a
focused check. Depth zero audits only the current surface. The defaults
are six menu transitions and a two-minute wall-clock budget.

For a parity run, use `--require-clanker` so missing seed history or an
unreached card fails the run. The JSON verdict lists visited paths,
findings, skipped actions, Clanker cards and navigation errors. Findings
or a failed traversal produce a nonzero exit. Save the report outside the
repository because resolved text can contain profile names and disc strings.
[CLI implementation](../../crates/apps/cli/src/agent_ui_crawl.rs)

## `audio`: voice routing

`h2 agent audio.listeners` takes no parameters. At a game boundary it reads
the guest's active world-sound listeners. Each row in `listeners` reports
the local `user` index, its salted `player_datum` (null for an unmapped
slot), whether the mapped player is a `phantom` (null without a resolved
player), and the listener's world-space `position` as `[x, y, z]`.
With one human and a roster of Clankers, only the human's slot
appears. Splitscreen reports each active human slot. The method returns
`UNAVAILABLE` if the game listener state is uninitialized or unreadable.

`h2 agent audio.voices` takes no parameters and is registered with
DirectSound (`crates/extensions/game/src/dsound_glue.rs`). The result has
`distance_reverb` (host enhancement enabled), `reverb_active` (audible
environment configured), and `voices`. Each playing buffer, queued
unpaused stream and playing mix-in has a mixer `id`, `kind`, `volume_mb`
and `distance_units` (listener-relative world units, null for an
unpositioned voice). Match a stream's output `bin` to a mix-in's
`input_bin` to find the positioned emitter.

`direct_gain` and `room_gain` are linear low-frequency gains including
voice volume and headroom. `direct_hf_db` and `room_hf_db` give the
additional loss at `reference_hz`. Each ordered entry in `bins` has its
raw `bin`, `volume_mb` and its linear `gain` including that pair's volume
and path gain. These are routing settings before spatial panning, master
gain and the reverb return. A disabled environment or unconnected bus can
have a configured send gain and deliver silence. Plain stereo routing has
`bins: null`.

### `audio.loops`

`h2 agent audio.loops` takes no parameters. With DirectSound installed it
reports the guest's active looping sounds and the host's playout health.
Each row in `loops` gives the salted `loop_datum`, `tag_datum`, resolved
`tag_name` when the map names the tag, zero-based `track`, `segment`,
`sound_instance` and `sound_datum`. `segment` is `in`, `loop`, `out`, `alt`,
`alt_in`, `alt_transition_out`, `alt_out` or `unknown`. Overlapping instances
of a track get separate rows. `pitch_range`, `permutation` and `chunk` come
from the engine's source state. `play_packet` and `play_position_frames`
give the DirectSound stream's current packet and its source-frame offset.
They are null while no queued stream packet can be resolved. The playback
cursor is at the mixer. The guest can select the next segment while PCM
from its predecessor is still queued.

`transitions` keeps the last 64 changes in starts, stops, segments and
permutation selection, sampled after the sound frame service. A null
`from` denotes a start and a null `to` denotes a stop. The stamp includes
the retail `tick` when available, the guest `present` counter,
`logical_100ns` and `elapsed_us`. The last field uses a monotonic wall clock
from audio-output creation. History starts over with a new map or a
rewound guest clock. `records_transitions` says whether the observer runs.

In the main menu, `menu` gives `ui_ms`, `idle_ms`, the music gate and
requested and applied music state, the attract timeout, the music stop
threshold from the current map, and whether a movie is active.
`menu_changes` keeps the last 64 changes in the gate, music state, movie
state or timer reset. The threshold applies on the screens eligible for
the attract movie. See [the attract timer](../audio/bink-hle.md#attract-timer).

`output` reports cumulative sink underruns and silent frames, playout
starves, dropped frames and mixer stream starves. Their last-change times
use the same `elapsed_us` clock as the transitions. The output thread
samples the sink counters as it services the device. Stream starves are
counted when a dry stream receives data again. Optional WMA counters show
codec restarts, errors and dropped partials. The playout fill and target
give the approximate delay between the mixer and the device.

Sample the main menu before and after an audible change. Compare present
numbers in `transitions` and `menu_changes` with the output counters and
their last-change times to identify the selection change, idle-triggered
restart or gap at the device.

## HaloScript state

Three game methods with an empty parameter object (`crates/extensions/agent/src/hs_state.rs`)
that sample guest state at a tick boundary. Each answers `UNAVAILABLE` if
its backing state is unreadable or uninitialized.

- `hs.threads` returns a `threads` array of live slots. Each row has
  `slot`, numeric thread `category` and signed `wake` tick. Scenario script
  threads also have their definition's `script` name and `kind`; both are
  null for transient evaluation threads.
- `hs.globals` returns a `globals` array in scenario declaration order.
  Each row has `name`, `type`, `type_index` and `value.bits`; scalar types
  also have `boolean`, `real` or signed `number`. Scenario declarations
  start after the interpreter's external-global cells.
- `hs.player_actions` returns three unsigned masks from local player
  control. `latched` records inputs seen since the script's last
  action-test reset; `capture` identifies inputs claimed by script tests;
  `until_release` retains suppression through button-up after a reset. Bit
  0 of each mask denotes the mapped action/reload button. The state is
  shared by local controls, so the method takes no player index.

## `input`: act like a player

Input goes through the normal pad path, so the game cannot tell an agent
from a person. The agent's tap is an `xapi::tap::InputTap` (the layer
`H2_INPUT_RECORD` and `H2_INPUT_REPLAY` use, `crates/extensions/agent/src/tap.rs`
over `input::backend::remote`'s step queue): it sees each
`XInputGetState` result just before the guest gets it. Every host that
runs the game through boot's XAPI has it: `h2 serve` (headless pad),
`h2 play` and the Xbox app. It wraps the tap those variables set: a replay
runs first (it stands in for the pad) and agent steps go on top; a
recording runs last, so it logs agent steps too. Taps are told which port
each handle opened on (`InputTap::opened`), which is how a step for
`player` 1 finds controller port 1. Without XAPI input there is no tap and
the input methods are not listed.

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
  step holds for `ms` of wall time, `ticks` of game time (the stamp's
  `tick`, the same on every run), or both, and until the game has read the
  pad `reads` times (default once), so a short press is never lost to a
  slow frame; with neither `ms` nor `ticks` it holds until replaced.
  `at_tick` (a tick, or `"now"`, the default) holds the first step back
  until the game's tick reaches it. `append` adds to the queue, otherwise
  it replaces it. A step merges over what the read returns
  (`input::backend::fallback::merge`): buttons are or'd, a stick or trigger
  the step moves wins. Buttons are `a b x y black white lt rt` (analog,
  fully pressed), `up down left right start back ls rs`; sticks `[x, y]`
  from -1 to 1 (up is +y); triggers 0 to 1. The packet number moves
  whenever the pad the game gets changes; until the first step, reads pass
  untouched, so a run with the agent on and no steps is byte for byte the
  same input. Queues outlive the connection that filled them (so a
  one-shot `h2 agent press` works) and are emptied when the map changes,
  since the tick starts again. The reply says `open: false` when the game
  has not opened that controller: the steps are kept, but nothing reads
  them.
- **`input.release {player}`** empties one port's queue, or all of them.
- **`input.state`** gives `pad` (player one), `pads` (every port: `active`,
  `queued`, `done`, what is held), `open` ports, `tick`, and the recording
  or replay. It needs no claim while nobody holds control.
- **`input.record {name, replace}`** logs every read (status, handle, the
  22 bytes, the game tick) to the cache directory's `inputs/<name>.input`
  in `xapi::tap`'s text format, the one `H2_INPUT_RECORD` writes; it is
  written at `input.stop`, or when the game exits. Names are letters,
  digits, `.`, `_`, `-`; an existing log is kept unless `replace` is true.
- **`input.replay {name}`** answers reads from a log read for read (keyed
  by read index, as `H2_INPUT_REPLAY`). `input.state` and `input.stop` give
  its counts: `reads` in the log, `replayed`, `matched` (same handle as
  recorded), `handle_mismatches` (the runs have parted) and `past_end`
  (left live). One recording or replay at a time. A replay reproduces a
  run only from the same starting state: the same boot, or the same
  [snapshot](#state-machine-snapshots).

#### `input.host`: the host controllers

`input.host` (`h2 agent hostpads`, read-only, no params) shows the
controllers the host's input layer has, before the game sees anything.
Use it when a pad, a button or the touchpad does not do what it should. It
answers from the last input sync (`syncs` counts them), which runs
whenever the game reads its pads. Boot adds it wherever the game has XAPI
input (`crates/extensions/game/src/debug_menu_glue.rs`, `crates/extensions/agent/src/hostpads.rs`).

- `devices`: each controller as its own backend reads it, before any
  merging (`input::HostBackend::devices`). From Apple GameController:
  `backend` (`game-controller`), `id`, `name` (the vendor name), `profile`
  (the extended profile's class: `dualsense`, `dualshock4`, `extended`, or
  `micro` for an Apple TV remote), `category` (the framework's product
  category), and the state below. Other backends do not describe their
  pads yet; theirs show under `pads`.
- `pads`: each pad the input manager polled, as the debug menu's gate is
  handed it: `backend` (the top-level one, such as `keyboard-fallback`),
  `key` (backend index and id, the gate's key), `name`, `physical`, `port`
  (0 to 3, or null) and the state. With the keyboard on, player one is a
  single pad that merges the first controller with the keyboard.
- The state, in both lists: `touchpad` (null without one, else `click`,
  and `primary` and `secondary` as `[x, y]` from -1 to 1 with +y up, or
  null for a lifted finger), `buttons` (`south east west north
  left_bumper right_bumper back start left_stick right_stick dpad_up
  dpad_down dpad_left dpad_right`; GameController's menu button is
  `start`, Options is `back`), `left_stick`, `right_stick`, `triggers`.
- `gate`: null with no debug menu, else `open`, `syncs`, and per pad `key`,
  `touchpad`, `click`, `chord` (click and Start together), `chord_pending`
  (Start held back waiting for the click), `opened_by_press` (the click
  that opened the menu is still down) and `latched` (buttons hidden until
  let go) ([debug-menu.md](debug-menu.md)).
- `notes`: where a touchpad goes missing, in words: a plain `extended`
  profile, a DualShock 4 profile without touchpad elements, a touchpad
  under `devices` but on no pad the gate sees, no debug menu.

```sh
H2_AGENT=/tmp/h2-play.sock h2 play           # then, holding the touchpad down:
h2 agent --socket /tmp/h2-play.sock hostpads
```

## `state`: machine snapshots

`state.save`, `load`, `list`, `drop` and `fork` (types in
`crates/extensions/agent-proto/src/state.rs`, the executor's part in
`crates/extensions/agent/src/exec/snap.rs`, the machine in
`crates/extensions/game/src/snapshot_glue.rs`). A snapshot is the whole machine at one
tick boundary, named. What is in it, how a load works and where files live:
[snapshots.md](snapshots.md). Snapshots stay in the cache directory, one
directory per build, and never enter the repository: they contain game
data. The game answers `UNAVAILABLE` when it has no snapshot support
installed.

| Method | Params | Does |
|---|---|---|
| `state.save` | `{name, replace?, cow?, max_ticks?}` | guest RAM, the kernel and its threads, D3D, audio, XAPI with the input cursor, every feature's section, and a RAM hash, at the first quiescent slice boundary after a tick. `cow: true` also writes a sparse raw RAM image `<name>.ram` beside it |
| `state.load` | `{name, cow?, max_ticks?}` | back to exactly that tick; the RAM hash (`lockstep::hash_guest`) is checked against the saved one, naming the MiBs that differ. `cow: true` maps that RAM image private copy-on-write in place of decoding RAM in; it is refused, naming the file, when the save wrote no image |
| `state.list`, `state.drop {name}` | | this build's snapshots, answered at once |
| `state.fork` | `{from, name, replace?}` | load `from` and save it again as `name` |

Names are `a-z 0-9 . _ -`, 1 to 64, not starting with a dot. A save or
load waits for the next tick, then retries at each tick while something is
live, for `max_ticks` (default 90, at most 1800), then answers `BUSY` with
the blockers in `data.blockers`. One sent while the game is paused runs at
the end of the held tick's slice and the game holds again at the next tick.
Calls other than `state.list` need control when another connection holds
it. A snapshot is refused while `agent-calls` writes are queued.

```sh
h2 agent state save NAME [replace]
h2 agent state load NAME
h2 agent state list
h2 agent state drop NAME
h2 agent state fork FROM NAME [replace]
```

## `edit` and hot `render`: live tag and shader changes

Host methods answered where the game's host registers them
(`crates/extensions/agent/src/hot.rs`, names in `crates/extensions/agent-proto/src/hot.rs`).
The design, the override directory and the rules are in
[hot-iteration.md](../graphics/hot-iteration.md). The override directory
is `$H2_RENDER_OVERRIDES`, else `render-overrides` in the cache directory.

| Method | Access | Params | Does |
|---|---|---|---|
| `edit.tags` | read | `{group?, name?, limit?}` | the loaded map's tags: `tag`, `group`, `name`, `address`, `size`, `defined` |
| `edit.get` | read | `{tag, group?, path?}` | a field by path (`postprocess[0].colors[1].color`), or an element's fields; `tag` is a datum or a name |
| `edit.set` | act | `{tag, group?, path, value}` | writes the field now, journalled; returns the change |
| `edit.changes` | read | none | the journal: `live` changes and `stale` ones (made on another map) |
| `edit.revert` | act | `{id?}` | puts back one change, or all |
| `render.overrides` | read | none | the override directory, what is loaded and how each fared |
| `render.reload` | act | `{dir?}` | reads the directory again (`dir` moves it) |
| `render.programs` | read | `{vertex?, limit?}` | the translated programs, by id |
| `render.dump` | act | `{program?, to?}` | writes translated WGSL, the start of an override |
| `render.param` | act | none, `{name}`, `{name, value}` | lists, reads, sets (`value: null` resets) a parameter |
| `render.toggle` | act | none, `{name, on}` | lists, sets (`on: null` resets) a toggle |

```sh
h2 agent edit tags weap
h2 agent edit get <tag name> <path>
h2 agent render toggle <name> off
```

Tag names come from the map file the game loaded ([`world`](#world-what-is-in-the-level)).

## Menu methods: settings, perf overlay, cheats

The in-game debug menu is a client of this API ([debug-menu.md](debug-menu.md));
the methods it added beside the agent's own are in
`crates/extensions/debug-menu/src/sources.rs` (`method`).

| Method | Access | Params | Does |
|---|---|---|---|
| `settings.list` | read | `{prefix?}` | settings (`settings.toml`) and how each applies (`crates/extensions/agent/src/menu.rs`) |
| `settings.set` | act | `{key, value}` | saves the value and applies it as the setting applies; `UNAVAILABLE` when something else set it for this run |
| `perf.overlay` | act | `{}` reads, `{on}` sets | the perf overlay (the watermark) |
| `cheat.list` | read | `{}` | the cheats, their values, and why any is refused (`crates/extensions/game/src/cheats.rs`) |
| `cheat.set` | act | `{name, value}` | sets one, for example `{"name":"hide_hud","value":true}` |

## Bot combat

`h2 agent bots.stats` reads the host's brain-driven phantoms at a tick
boundary. It takes no parameters. The result contains `at` and a `bots`
array. Each row has the seat's `handle`, `name`, `player` (null until
bound to a game player), `skill`, `profile`, `combat` and `accuracy`.

`profile` contains the card's `summary`, its `archetype`, `favorite_weapon`, the name's
`try_hard` score and the `weights` active for that difficulty and style.
The full vector contains `radar_use`, `noob_combo`, `dual_wield`,
`aggression`, `grenades`, `melee`, `weapon_preference`, `range_preference`,
`strafe`, `jump`, `reaction_jitter` and `retreat`. Each value ranges
from zero to one. Higher reaction jitter favors the later part of the
difficulty’s reaction window.

| Field in `combat` | Meaning |
|---|---|
| `tick`, `observation_tick` | Most recent think tick and the world sample the decision used. The sample can be older because perception takes time. |
| `goal`, `target`, `range` | Current activity, visible fight target's player index and distance in world units. The target and range are null outside a visible fight. |
| `weapon`, `weapon_role` | Held weapon's map definition datum and handling class, null before an inventory read. |
| `last_decisions` | Up to eight goal changes and combat input decisions, oldest first, each with `tick` and `action`. |
| `melee_presses`, `switch_presses`, `grenade_requests` | Attack or switch inputs chosen by the brain, counted once per press. |
| `weapon_switches`, `grenades_used` | Observed exchanges of the two carried weapon definitions and decreases in grenade inventory within a life. |
| `last_reaction_ticks`, `total_reaction_ticks`, `reactions` | Ticks from the selected target's first fight observation to the first attack decision on that target. The mean is the total divided by the sample count. |
| `think_ms`, `total_think_ms`, `think_ticks` | Wall time for the latest brain decision and controller update, accumulated time and sample count. Guest reads, worker queueing and retail input processing fall outside this interval. |

`accuracy` groups the game's registered shots and credited enemy hits by
weapon and decision range at fire time. Close is under 3 world units,
mid is 3 to under 12 and long is 12 or more. An unknown range means
the shot had no sampled visible target distance. Each row includes
`damage_code`, `weapon`, `range`, `shots`, `hits` and their ratio
`accuracy`. One shot can earn one hit credit. An empty list means the
host has no registered shots to report for that seat.

## `sim`: simulated players and direct state

`sim.*` lets an agent test co-op, multiplayer and Live flows from one
running game: players that are not people, game state written directly,
and assertions that answer pass or fail with what was seen. It never
starts another game instance. They are game methods
(`Agent::add_game_method`, `crates/extensions/agent/src/sim/`): answered on the
game's thread with guest memory at hand, and like `world.*` at a tick
boundary while the game ticks. The wire types are `agent_proto::sim`.

| Method | Access | Params and result |
|---|---|---|
| `sim.player.add` | act | `{kind: "phantom", identity: {xuid, gamertag}, source: script\|recording\|brain, team?, steps?, recording?}` adds a phantom peer and returns its `sim.<n>` id. `kind: "local"` is `INVALID_PARAMS` |
| `sim.player.remove` | act | `{id}` or `{all: true}` |
| `sim.player.list` | read | `players` (simulated players and their game player `player.<i>`) and `clankers` (the host's brain-driven bots and their brain state) |
| `sim.player.pad` | act | `{id, steps, append?}` sets or appends pad steps for a phantom with a script source, in `input.pad`'s step format |
| `sim.state.set` | act | `{player, pos?, yaw?, pitch?, facing? (or forward?), team?, health?, shield?, grenades?, ammo?}`: host writes; `pos` and the facing go through the game's own teleport, answered after the next tick |
| `sim.give` | act | `{player, weapon, ammo?, keep?}`: the game makes a weapon from a `weap` tag (datum or name) and gives it to the player's unit; answered after the next tick |
| `sim.spawn` | act | `{tag, pos, yaw?}`: the game makes an object from its tag at `pos`; answered after the next tick |
| `sim.view` | read | `{roots?}`: the game as one JSON document |
| `sim.expect` | read | `{checks}`: expectations judged against `sim.view`, at one tick |
| `sim.session` | read | the session kind and whether writes are allowed |
| `sim.objects` | read | `{kinds?, max?}`: every live object in slot order through the simulation's typed views, `{objects, truncated}` |
| `sim.events` | read | `{kinds?}`: the last tick's simulation events in order (`deleted`, `damage`, `kill`), `{events}` |

**Brains.** For a brain-driven simulated player, `sim.player.add`,
`sim.player.list` and the `sim` root of `sim.view` include `brain` when
the backend provides it. The list's `clankers` array includes brain-driven
phantom bots added by the game, headless matches and sim; each entry has
`name`, `kind` and `brain`, and once joined `player`. Script and recording
sources have no brain state. The `brain` object has `skill`,
`profile` as described under [Bot combat](#bot-combat),
`personality` (the nine 0 to 1 traits in
[the Clankers trait table](../game/bots.md#personality-and-temperament) and
`favorite_weapon`: `spray`, `precise`, `scatter`, `sniper`, `blast` or
`blade`) and `mood` (`confidence`, `tilt`, `fear`, `streak`, and an
optional `revenge_target` `player.<i>` while a killer is remembered).

For a joined phantom, `h2 agent sim list` includes `clankers[].aim` once
the host has gathered input for its current unit. `tick` is the gather
tick. `stick` (right, up) and `trigger` are the brain's latest report;
`arrived_stick` and `arrived_trigger` are the report delivered by the
uplink for that gather. `action_deg` is the accepted slot record's
absolute [yaw, pitch] in degrees, or null if the gather was rejected.
`fire_primary` is the accepted primary fire bit and `assist_strength` the
two weapon strengths; a rejected gather reports false and zeros.
`unit_forward` is the unit's actual aiming vector at query time.
`assist_player` identifies the player controlling the selected aim-assist
target when one exists. To verify damage, read the target's shield and
health with `h2 agent sim view player`.

**`sim.objects`** reads the objects pool through `sim-state`
(`crates/rewrites/sim-state`, the read side of the object update rewrite,
[sim-rewrite-objects.md](../engine/sim-rewrite-objects.md) section 8), so
it shows what the simulation's own views see, offsets and all. `kinds`
filters by tag group (`bipd`, `vehi`, `weap`, `eqip`, `garb`, `proj`,
`scen`, `mach`, `ctrl`, `lifi`, `ssce`, `bloc`, `crea`); `max` caps the
list (default and cap 2048) and `truncated` says more matched. Each object
has `id` (its salted handle, `0x` and eight hex digits), `kind` (`null`
outside the type table), `definition`, `flags` (the header flags set:
`active`, `awake`, `post`, `new`, `deleting`, `defer`, `attach`, `child`),
`parent`, `position`, `forward`, `up` and `scale` (parent-relative under a
parent), `velocity` (units a second), `sphere {center, radius}`,
`vitality {body, shield, depleted}`, `nodes` (node matrix count) and, for
bipeds and vehicles, `unit {player, actor, team, seat, weapons, held}`.
The wire types are `agent_proto::sim::objects`.

**`sim.events`** reads the simulation's event buffer through `sim-state`
(`sim_state::events`, [sim-rewrite-objects.md](../engine/sim-rewrite-objects.md)
section 8). The game answers at a tick boundary, so the events are the
tick that just ran, in the order they happened. The buffer is emptied when
each tick starts. `kinds` filters by type (`deleted`, `damage`, `kill`;
another name is `INVALID_PARAMS`). Datums are `0x` and eight hex digits,
or `null` for none. Each event has a `type`:

- `deleted`: finalize deleted an object. `id`, `kind` (its tag group) and
  `definition`.
- `damage`: the damage report applied one damage instance. `tick`,
  `victim`, `victim_kind`, `victim_player`, `owner_player`,
  `owner_object`, `owner_team`, `code` with its halves `damage_type` (low
  6 bits) and `modifier` (high 2), `effect` (the damage effect tag),
  `flags`, `depleting` (the body was depleted), `body` and `shield`
  (damage as fractions of full vitality), `direction`, `origin` and
  `position` (the victim's).
- `kill`: the game credited a kill. `tick`, `attacker` and `victim` (player
  datums), `code`, `damage_type`, `modifier`, `victim_unit`,
  `attacker_unit`, `position` and `attacker_position`.

Events are recorded only while `impl.sim.native.objects` is on; with it
off the list is empty. The records and where they are read are in
[object-update.md](../engine/object-update.md), "Slice 8 body". The wire
types are `agent_proto::sim::events`.

**`sim.view`'s roots.** `at` (the stamp), `session`, `ui` (`ui.screen`'s
answer, `null` without `H2_UI_WATCH=1`), `player` (by index: the `world`
object plus `health`, `shield`, `grenades {frag, plasma}` and the held
weapon's `ammo {loaded, reserve}`), `object` (every `world` object by id;
only when asked for), `count` (by kind, `team` counting players per team,
`total`), `sim` (simulated players by `<n>`, and `count`) and `input`
(`input.state`). Pointers read straight off it: `/player/1/team`,
`/object/<id>/occupied`, `/count/team/1`, `/sim/1/player`.

**Expectations** are `h2 agent run`'s: `"<pointer> <op> [JSON]"` or
`{"pointer", "op", "want"}`, ops `==` `!=` `<` `<=` `>` `>=` `contains`
`exists` `missing` and `near {"to": number or array, "within": d}` (for
positions and vitality, read from `f32`s). One implementation,
`agent_proto::expect`, serves both: `h2 agent run` judges the last answer
on the client, `sim.expect` judges `sim.view` in the game. Its answer is
the verdict's fields, `{"pass", "failed", "expects": [{expect, pass, want,
got}], "at"}`.

### `h2 agent sim` and scripts

There is one script format: `h2 agent run`'s. Any line may be a `sim`
verb, and `h2 agent sim run <script>` is `h2 agent run` that also removes
the simulated players the script added (`"cleanup"` in the verdict). Run
this against a game this console hosts; replace `ROSTER_XUID` and
`ROSTER_GAMERTAG` with a bot identity issued by the local Live server.

```sh
cat > bot-team.txt <<'X'
sim session
expect /kind == "mp_host"
sim add phantom script xuid=ROSTER_XUID gamertag=ROSTER_GAMERTAG
wait {"phase":"gameplay","map_ticks":30}
sim until 600 /sim/1/player exists
sim set sim.1 team=1 health=0.5 pos=[12.0,-3.5,2.0]
expect /wrote contains "pos"
sim view sim
expect /sim/1/player exists
sim pad sim.1 [{"left":[0,1],"ticks":60}]
X
h2 agent sim run bot-team.txt
```

The verbs in `crates/apps/cli/src/agent_sim.rs`: `add phantom [source]
xuid=X gamertag=G [key=JSON...]`, `remove <sim.N|all>`, `list`, `pad
<sim.N> <steps> [append]`, `set <player.N|sim.N> key=JSON...`, `give
<player.N|sim.N> <weap tag> [loaded=N] [reserve=N] [keep]`, `spawn <tag>
pos=[x,y,z] [yaw=DEG]`, `view [root...]`, `expect <expectation>`, `until
<ticks> <expectation>` (`sim.expect` every 5 ticks until it passes; a
timeout fails with the last verdict), `session`, `lobby ...` and `run`. In
`h2 agent run`, a failed `sim until` is a failed call, so it stops the
script.

**Lobby steps** (`agent_proto::sim::menu`) press player one's pad
(`input.pad`) and check each step on `ui.screen`, with a timeout, on the
client, since a step spans many ticks:

- `lobby mode <campaign|xbox_live|split_screen|system_link>`: from the main
  menu, down to the row and A; expects the campaign screen or the players'
  screen in that mode.
- `lobby host live_coop`: from the players' screen in Xbox Live mode (or
  the Live menu): A, then down from the restored row to Co-op Campaign
  (row 4), A; expects the Live menu to go.
- `lobby invite [ROW]`: Y in a lobby, then A on friend row ROW.
- `lobby start`: A on Start Game (row 0); expects the screen to go.
- `lobby press <b[+b...]> [JSON expect]`: any presses, then wait for
  `{"screen", "main_menu", "mode", "not"}`.

Their screen ids and rows are the ones `h2 coop test --drive` uses
(`agent_proto::sim::menu`, re-exported by `coop_drive`). They assume a menu
opens on row 0; a wrong row shows as the wrong screen and the step fails
saying what it saw.

### Determinism

`sim.session` reads the simulation mode (`coop_probe::read_sim`,
[live-coop.md](../live/live-coop.md) 1.1) and the squad session
(`coop_probe::read_session`) through boot's probe
(`boot::agent_glue::session_view`):

| Mode | Kind | `sim.state.set` |
|---|---|---|
| 0 | `none` (menus) | allowed (no players to write) |
| 1 | `local` (campaign, split screen) | allowed |
| 2, 3 | `coop_host`, `coop_guest` (synchronous) | refused: a write on one machine desyncs the others and trips the sim hash |
| 4 | `mp_host` (distributed server world) | allowed: host-authoritative |
| 5 | `mp_client` (distributed client world) | refused: the host owns the state and overwrites it |
| unreadable | `unknown` | refused |

A refusal is `INVALID_REQUEST` with the session in `data`. In co-op, drive
players with pads instead (`sim.player.pad`, `input.pad`): pads are
inputs, which the pad schedule carries to every machine. Writes happen at
a tick boundary only (`UNAVAILABLE` between slices), and every value is
checked before the first write, so a refused set writes nothing. A `h2
lockstep` comparison run is local (mode 1) and is not detected: a write
there makes the runs differ by design.

### What `sim.state.set` writes

Teleport and facing go through the game ([below](#the-games-own-functions-teleport-give-spawn)).
The other fields are host writes at the addresses Live telemetry reads
(`live_telemetry::layout`, `engine::view`): the player record's team
(`+0xc0`); the unit's team (`+0x138`), body and shield vitality (`+0xec`,
`+0xf0`, [health.md](../halo1/health.md); `engine::view::objects::Body`),
grenade counts (`+0x23e`) and the held weapon's first magazine (`+0x22c`
loaded, `+0x22a` reserve). Spawning a player between lives is not a
write; it needs the game's respawn and is not provided.

### The game's own functions: teleport, give, spawn

A host write of a position cannot move a player: the biped's physics puts
the body back at the next tick. So a move goes through the game
(`crates/extensions/agent/src/sim/calls.rs`), and its physics, cluster and vehicle
seat hear of it.

A game method answers in the tick observer, after the tick, where nothing
can call into the guest. So it queues a call and defers its answer
(`GameCtx::defer`, the executor's deferred answers: polled at each tick
boundary from the next, `TIMEOUT` after 30 ticks, held while paused). The
`agent-calls` feature (a `before` on `game_tick`, first;
`H2_FEATURES=-agent-calls` turns it off) finds the queue full at the start
of the next tick, makes the calls, then runs the tick itself, as our
skulls do. The answer comes after that tick. A snapshot is refused while
calls are queued.

- **Teleport** (`sim.state.set` with `pos`, `yaw`/`pitch` or `facing`) is
  HaloScript `object_teleport`'s body (`0x29ffb0`) for a player's unit,
  with our position in place of a cutscene flag's. The position goes to
  the co-op placement `PLAYER_PLACE_NEAR` (`0x14e970`; EBX the player, ESI
  the point, anchor -1, as `0x2a01ec` calls it), which takes the unit out
  of a vehicle (`0xe68c0` action `0x1e`) and places it through `0x14e200` a
  little above the point. The facing: the unit's desired facing, aiming and
  looking vectors (`+0x150`, `+0x15c`, `+0x180`, as `0x2a019e..0x2a01d9`
  writes them), a local player's view (`0x1874b0`: yaw and pitch into the
  player controls, `[0x4ed284] + 0x14 + 0x94 * user`, `+0x10`, `+0x14`),
  then `object_set_position` (`0xb75a0`) with no position for the
  orientation. A seated unit's facing is left (its seat's); the answer says
  so. Other fields of the same call are written at once. The answer:
  `wrote` (with `pos`, `facing`) and `teleport` (`pos` and `body_forward`
  where the game put it, `faced`, `unit`). `yaw` is degrees about +z from
  +x and `pitch` degrees up (-89 to 89), as `world.query` reports them; or
  `facing: [x,y,z]` (also taken as `forward`, `world.query`'s name for it),
  so a check can turn the player to face what a query found.
- **`sim.give`** gives a weapon as a starting profile does (`0xcce78`) and
  as an AI's equipment does (`0x1e05c8`): `placement_data_new` (`0xb7930`,
  owner the unit), `object_new` (`0xb7b40`), then `unit_add_weapon`
  (`0xcd0c0`, stdcall `(unit, weapon, mode)`), deleting the weapon
  (`0xb8540`) if the unit refuses it. Default mode 2: the unit's weapon
  slots are emptied first (`0xccff0`) and it readies the new one. `keep:
  true` is mode 1, the starting profile's: added, held only if the unit's
  own weapon choice (`0xcdeb0`) picks it. `ammo` writes the new weapon's
  first magazine once it is made. The tag is a datum or a name with `/` or
  `\` (names come from the map file, as `edit.tags` lists them: `h2 agent
  edit tags weap`). A bare name is looked for among `weap` tags only, since
  a weapon shares its name with its model, render model, collision and
  animations (`hlmt mode coll jmad`); `group:name` or `name.group` gives
  the group outright. The answer: `weapon` (`tag`, `group`, `name`),
  `object`, `mode`, `held`.
- **`sim.spawn`** makes an object as the netgame equipment spawn does
  (`0x15ab94..0x15abe6`): `placement_data_new` with no owner, the position
  at placement `+0x1c`, forward and up at `+0x28`/`+0x34`, then
  `object_new`. Any object group `object_new` knows (`bipd vehi weap eqip
  garb proj scen mach ctrl lifi ssce bloc crea`). A bare name is looked for
  among those groups; if it names tags in more than one, the answer asks for
  the group (`vehi:<tag name>`). The answer: `object`, `pos` (where it was
  made), `tag`.

All three are host writes: refused in a co-op session or on a multiplayer
client, and only at a tick boundary, as `sim.state.set`.

```sh
h2 agent sim set player.0 'pos=[36.7,-80.8,1.7]' yaw=90 pitch=-10
h2 agent query '{"kind":["player"],"fields":["pos","forward","yaw","pitch"]}'
h2 agent sim give player.0 <weap tag name> reserve=108
h2 agent sim spawn <vehi tag name> 'pos=[40,-80,2]' yaw=180
```

### Phantom backend

`boot::sim_bots::SimBots` implements `agent::sim::bots::Bots` over the
process-wide phantom registry. It places a player through
`bots::phantom::add(identity, team, source)` using a
`bots::pad_source::PadSource`. The bots feature must be enabled; a light
host also needs `--host-bots` to accept simulated players. A script source
uses the same `input::backend::remote::Remote` step queue as `input.pad`;
phantom gather samples it once per game tick. A recording supplies
successful reads of the input log's first pad. A brain supplies its own
pad. `sim.player.remove` removes players created through
`sim.player.add`. Phantoms join multiplayer games this console hosts; in a
level-boot `h2 serve` session, drive the existing player with `input.pad`,
`sim.state.set` or `sim.give`.

### Live

Sim never talks to a Live server and never makes accounts or sessions. A
phantom needs `identity: {xuid, gamertag}`, and the only place to get one
is the local embedded server (`h2 play --live-local`,
`live_server::start_embedded`), which issues bot identities and accepts
`bot: true` only for xuids it issued. Never point a sim run at the public
Live server.

## Sim matches

Host methods while sim mode is on (`H2_SIM=<match file>`, `h2 sim run
<match> --socket PATH`; `boot::sim_glue::Sim::add_methods`, [sim.md](sim.md)).
Sim mode is a whole headless match with bots and no rendering, ending in a
report; these let a client watch it, read its numbers so far and end it.
The `sim.*` methods above are a different thing: simulated players added to
an ordinary run.

| Method | Access | What |
|---|---|---|
| `match.status` | read | `match` (the file), `map`, `game_type`, `bots`, `phase` (`waiting` until the match's game starts, `binding` until every distributed seat has a player and unit, `playing`, `over`), `bound_seats` (logical seat to player index for a distributed match), `ticks` (game ticks counted since the start), `max_ticks`, `game_seconds`, `wall_s` (since the boot), `match_s` (since the first tick), `speedup` (game over match wall time), `frames` (`skipped` world frames, `rendered_world` by `render_every`, `other` menu and loading frames, `stub_faults`), `end` (`game_over`, `tick_limit`, `wall_limit`, `stopped`; `null` while it plays) |
| `match.report` | read | the report as it would be written now (`h2-sim-report/1`, sim.md section 6): players, kills so far, a note while the match has not ended (its `result.end` is then `aborted`) |
| `match.stop` | act | end the match: `end` becomes `stopped` (unless it had ended), the boot loop stops at the next slice and the report is written. `{stopping, end, already_over}` |

```sh
h2 sim run docs/runtime/sim-examples/aim-net-2v2.toml --socket /tmp/sim.sock &
H2_AGENT=/tmp/sim.sock h2 agent match.status
H2_AGENT=/tmp/sim.sock h2 agent match.report
H2_AGENT=/tmp/sim.sock h2 agent match.stop
```

`session.pause` and `session.step N` step a match.

## Theater transport

The game agent lists these host methods in `session.methods`
(`crates/extensions/agent-proto/src/theater.rs`, `crates/extensions/game/src/theater/controls.rs`).
In an ordinary CLI-hosted game, `theater.play` with a library ID starts an
isolated playback process; subsequent transport requests through that
game agent go to its player. During startup the caller can retry after a
temporary unavailable response. The playback process also accepts calls
directly on its own `--socket` (`H2_AGENT`, else the cache's `theater.sock`).

| Method | Parameters | Result |
|---|---|---|
| `theater.play` | `{"id":"film-id","range":[300,900]}` to open, `{}` to resume; omit `range` for the whole film | `{id, starting:true}` on launch, otherwise transport state |
| `theater.state` | `{}` | transport state, or `{id:null}` with no player |
| `theater.pause` | `{}` | holds guest execution and reports state |
| `theater.speed` | `{"speed":0.5}` | sets host pacing from 0.25 to 4 times realtime |
| `theater.seek` | `{"tick":900}`, optionally `"clip_marks":[300,null]` | restarts the player and fast-forwards to the target, carrying camera and timeline state; direct socket clients reconnect after the restart |
| `theater.camera` | `{"mode":"orbit","player":"<name>","look":[0.2,-0.1],"zoom":-1}` or `{"mode":"first_person","seat":3}` | selects the view and returns transport state; an absolute film seat is 0 to 15 |

The rendered player starts at the clip's first tick and loops at its end.
Its overlay uses the player's agent socket for transport and `cheat.set
{"name":"hide_hud","value":true}` for the HUD. A paused player accepts HUD
and camera changes and applies them to the next rendered frame.

Rendered host playback starts in `free`; its other modes are `orbit`,
`follow`, `first-person` and `default`. In a playerless version 2 or 3
host film, `default` uses the first available player's eye position.
`orbit`, `follow` and `first-person` accept a player name or an absolute
film seat; with neither, they use the first spawned player. `look` adjusts
yaw and pitch in radians for `free` and `orbit`. In `free`, `movement`
adjusts strafe, forward and rise in world units. In `orbit`, `zoom` changes
the radius in world units. Local playback starts in `default` and shows
the whole retail frame; version 3 also accepts `first-person` with a local
player's name or seat to select a drawn viewport. Headless playback
(`--render-every 0`) accepts only `default`.

Transport state carries `id`, `tick`, `length_ticks`, `paused`, `speed`,
`seeking`, `camera`, optional `camera_player` and `camera_seat`, and
discovered kill marker ticks in `kills`, alongside the usual `at` stamp.
Tick zero is boot. A paused seek holds at its target until play. The HLE
compares the recorded call index and game-thread hash samples during
fast-forward ([theater.md](../game/theater.md)).

```sh
h2 agent theater.state '{}' --socket "$film_run/play.sock"
h2 agent theater.pause '{}' --socket "$film_run/play.sock"
h2 agent theater.seek '{"tick":30}' --socket "$film_run/play.sock"
h2 agent theater.play '{}' --socket "$film_run/play.sock"
```

## `collision`: the native collision world

Counters for the entries `impl.sim.native.collision` takes over. Contract
and bail reasons: [collision-queries.md](../engine/collision-queries.md).
This is a game method (`agent::collision::install`). The counters are
process globals, so it reads no guest memory, and they stay zero while the
switch is off.

| Method | Access | Params and result |
|---|---|---|
| `collision.stats` | read | no params. `switch`; `entries`, one per entry in contract order, each with `name` (`point_to_leaf`, `location`, `vector_test`, `vector_context`), `addr` (8 hex digits), `native` (calls the kernel answered) and `bails` (calls left to the lifted code, by reason: `env`, `float`, `fault`, `data`, `depth`, `alias`); `decodes`, `decode_failures`, `flushes`; `bsps` and `bytes` held now; `precision`, the x87 precision control seen at the vector-test entries (`single`, `reserved`, `double`, `extended`) |

```sh
h2 agent collision.stats
```

The bail audit (under 0.1% of calls bailing) reads `bails` against
`native`. A reason that appears on one architecture only is a determinism
bug.

## Tick traces and the flight recorder

Polling `world.object` from outside (`h2 agent -`) drops ticks when the
game or the client is busy. `trace.*` and `flight.*` take the rows inside
the game, in the tick observer, before queued requests are served, so each
tick gives exactly one row. Code: `crates/extensions/agent/src/trace.rs` (tests in
`trace/tests.rs`), wire types and `show` in `crates/extensions/agent-proto/src/trace.rs`,
CLI in `crates/apps/cli/src/agent_flight.rs`.

```sh
h2 agent trace start player.0 physics,pos          # rows to traces/trace-<ms>.jsonl
h2 agent trace stop                                # rows, bytes, first/last tick
h2 agent flight start player.0 physics,pos,velocity 20
h2 agent flight dump crouch-bug.jsonl              # flights/crouch-bug.jsonl
h2 agent flight show ~/Library/Caches/h2recomp/flights/crouch-bug.jsonl \
    --from 4100 --to 4160 --field physics.crouch --field physics.airborne --field input
```

- **Rows.** One JSON line per tick: `{tick, ticks, map, objects: {id:
  Object}, gone, error}`. `tick` is the stamp's game tick, `ticks` the
  agent's count (never resets, one more each row). `objects` holds each
  asked-for id in the world, trimmed to `fields` (`world.query`'s names;
  `id` and `kind` always kept; all when absent). Ids not in the world that
  tick are in `gone`; a failed world read gives `error` and still a row.
  The world is read once a tick when either recorder runs, and not at all
  otherwise. A paused game writes no rows; each `session.step` tick does.
- **`trace.start {objects, fields?, path?}`.** 1 to 64 ids. Streams rows to
  `path` (a bare file name in the cache directory's `traces/`, since the
  socket may be unauthenticated TCP; default `traces/trace-<ms>.jsonl`; an
  existing file is replaced) through one buffered writer, flushed every 30
  rows, at `trace.stop` and at exit: memory does not grow with length.
  `trace.stop` and `trace.status` give `rows`, `bytes`, `first_tick`,
  `last_tick`, `path` and `error` (a write failure ends the trace there).
  One trace at a time; the last one's counts stay for `trace.status`.
- **`flight.start {objects, fields?, seconds?}`.** Keeps the last `seconds`
  (default 30, 1 to 600) x 30 rows in memory, at most 64 MiB of JSON (the
  oldest go first). Each row also has `input`: every `XInputGetState`
  result the game got since the last tick, taken at the agent's input tap
  after agent steps merge in (`{port, handle, status, packet, buttons,
  left, right, raw}`, `raw` the 18 pad bytes in hex), at most 64 a tick
  (`input_dropped` counts the rest, as during a load). Without XAPI input
  there is no tap and rows have no `input`. `flight.dump {path?}` writes
  the ring (default `flights/flight-<ms>.jsonl`) and keeps recording;
  `flight.stop` drops it; `flight.status` gives `capacity`, `rows`,
  `bytes`, `ticks` seen and the ring's tick range.
- **Scrub.** `h2 agent flight show <file>` reads a trace or flight file
  without a game: whole rows, or with `--field` one line per tick of each
  object's dotted paths (`physics.crouch`) and `input`'s reads, between
  `--from` and `--to` game ticks. It does not re-simulate; replaying a
  stretch starts from a [snapshot](#state-machine-snapshots).
- **Access.** Read-only (no claim), answered between slices too. Files are
  written where asked, as `input.record`'s are.

## Shadow checks

Design: [shadow-exec.md](../engine/shadow-exec.md). Host methods
(`agent::shadow`, registered by boot's agent glue) over
`feature::shadow`'s registry. A native sim port is checked against the
lifted original, each on its own copy of guest memory from the state the
game reached, then runs on the live Guest as usual. Only installed ports
are checked: a port's `impl.` switch puts it in the plan, and
`shadow.enable` is the only switch for the checks.

Ports are named by switch (`sim.native_unit_pose`,
`sim.native_havok_solver`), by group (`sim.native`, `sim.physics`) or
`all`.

| Method | Access | What |
|---|---|---|
| `shadow.enable` | act | `{ports, every?, step_limit?}`: checks on for those ports, their counts and mismatches reset. `every` checks one call in that many (default 1); `step_limit` caps the original's instructions per check (default 5,000,000). The first call builds the lifted original from the image's cached analysis (a few seconds on the game thread). → `{enabled, not_installed, every, step_limit}`. Unknown names: invalid params; no image in the process: unavailable |
| `shadow.disable` | act | `{ports?}`: checks off (no ports: all). → `{disabled}`. Counts and mismatches stay |
| `shadow.status` | read | `{on, ready, every, step_limit, calls, checked, matched, mismatched, skipped, ports, mismatches, mismatches_dropped}`. Each port: `name`, `group`, `point`, `function`, `installed`, `on`, `calls`, `checked`, `matched`, `mismatched`, `skipped`, `skip_reasons`. Each mismatch (at most 64 kept): `port`, `group`, `point`, `function`, `pc` (entry or resume), `tick`, `call` (which call since `enable`), `kind` (`transfer`, `registers`, `memory`, `returned`, `nondeterministic`), `summary`, `transfer` (`port`, `original`), `registers` (`name`, `port`, `original`), `memory` (runs of bytes: `address` folded physical, `virtual_addresses`, `before`, `port`, `original`), `memory_bytes`, `steps`, `entry` (every register at entry, hex) |

```sh
H2_FEATURES=impl.sim.native_unit_pose h2 serve ...
h2 agent shadow.enable '{"ports":["sim.native_unit_pose"]}'
h2 agent session.wait '{"until":{"ticks":300}}'
h2 agent shadow.status
```

`scripts/shadow-gate.sh` is the scripted disc gate over these methods: a
headless sim match with the ports installed, checks on for N ticks, and a
failure on any mismatch ([shadow-exec.md](../engine/shadow-exec.md#the-disc-gate)).

## Scripted scenarios with a verdict

```sh
cat > grenade.txt <<'X'
wait {"phase":"gameplay","map_ticks":30}
expect /at/phase == "gameplay"
claim
press rt 100
capture 90
expect /over_budget <= 2
expect /causes missing
release
X
h2 agent run grenade.txt          # one JSON verdict; exit 1 unless it passed
```

`h2 agent run <script|->` (`crates/apps/cli/src/agent_run.rs`) is the `h2
agent -` script on one connection, plus `expect <pointer> <op> [JSON]` on
the last answer (a JSON pointer; ops `==`, `!=`, `<`, `<=`, `>`, `>=` on
numbers, `contains` for a substring, an array element or an object key,
`exists`, `missing`, and `near {"to", "within"}`) and `keep` (put the last
answer in the verdict). The expectations are `agent_proto::expect`, which
`sim.expect` judges in the game too. The verdict is `{"pass", "failed",
"ms", "calls": [{line, call, ms, ok, error}], "expects": [{line, expect,
want, pass, got}], "kept", "stopped"}`. A failed expect is recorded and
the script goes on; a failed call stops it. Answers are not printed
except kept ones, so a long scenario costs a few lines of context.

## Test maps

Purpose-built test maps from recipes: a spawn grid, a weapon range, the
superbounce patch and more, built from the user's own disc while the game
runs and queued as the next game. Design, recipe format and the built-in
rigs: [test-maps.md](../game/test-maps.md). Host methods registered by
`crates/extensions/game/src/test_maps.rs` (the `test-maps` cargo feature of
`boot`, which supplies the recipes in `crates/apps/boot/src/test_maps_recipes.rs`); wire types `agent_proto::test_maps`.

| Method | Access | Params and result |
|---|---|---|
| `maps.load` | act | `{map_id}` → `{name, map {id, path}, check, queued, expect}`. Queues a local Slayer game on the installed multiplayer package with this full hash. The VFS verifies the list slot's package identity. Wait for gameplay on the basename of the returned path before driving the camera. |
| `maps.test.list` | read | `{name?}` → `{recipes, maps}`: the built-in recipes (`name`, `title`, `base`, `fidelity`, `purpose`, `note`) and each test map (`name`, `base`, `state` `building`/`ready`/`failed`, `dir`, `error`, `map_id`, `listed {id, path}` once it has a multiplayer slot, `built`: the build report with `anchor`, `placed`, `counts`, `notes`, `expect`) |
| `maps.test.create` | act | `{recipe: <built-in name> \| <recipe object>, base?, replace?}` → `{name, state: "building", dir}` at once; the build runs on its own thread, then the offers are installed again. `base` builds on another disc map (`<recipe>-<base>`) |
| `maps.test.load` | act | `{name}` → after the next tick, `{name, map {id, path}, check, queued, expect}`: a local Slayer game on the map is queued through the `agent-calls` feature (`Call::QueueGame`, the level boot's steps). Wait for it with `session.wait` or `h2 agent maps load` |
| `maps.test.remove` | act | `{name}` → `{name, removed}` |

```sh
h2 agent maps                          # recipes and test maps
h2 agent maps create superbounce       # waits for the build
h2 agent maps load superbounce         # waits until the new map is in gameplay
h2 agent sim expect '/player/0/pos near {"to":[-10.5,3.9,-3.15],"within":1.5}'
```

`h2 agent maps` (`crates/apps/cli/src/agent_maps.rs`) reads a recipe file
(`.toml` or `.json`) on the client and sends it as the recipe object; its
verbs work in `h2 agent -` and `h2 agent run` scripts.

### Test maps on disk

`h2 testmap <source.map> <script.json | -> --out <new.map> [--force]`
(`crates/apps/cli/src/testmap_cmd.rs`) is a host-only tool for a repro that
needs one thing in an empty room. It runs a JSON edit script
(`scenario_edit::script`, `crates/tools/scenario-edit/src/script.rs`) over the
source's placement blocks through the editor's own `Session`, so every
edit is checked as the editor checks it, and saves a new map (the source
is never written). It prints what each operation did and the entries left
per block. Operations: `clear` (blocks, or `"all"`; a block the editor
will not empty is reported and left), `keep` (these entries of a block,
renumbered from 0), `add` (a copy of `like`, or a zeroed entry with the
palette's first item, then `set`), `set` (fields by the editor's names:
`position` `[x, y, z]`, `rotation`, `facing`, `team`, palette `type`, ...),
`remove`. `allow_growth` lets a block grow past its source size, as the
editor's switch. Blocks: `scenery`, `vehicles`, `equipment`, `weapons`,
`machines`, `sound_scenery`, `crates`, `starting_locations`,
`netgame_flags`, `netgame_equipment`, `starting_equipment`. AI (squads and
their spawn points) and tag references other than through a palette index
are not covered. The user or coordinator loads the result (`H2_MAP_DIR`);
agents do not.

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
  DirectSound, `host.*` without the light host) are absent from
  `session.methods`.

## Evidence

Statements are **Code** (read from this repository, the path given) unless
labelled. Guest addresses are **E** (read from the retail XBE with `h2
disasm`) and are research points, not data. Tests that cover the page:
`crates/extensions/agent/src/exec/tests.rs` (wire, control, wait, deferred answers),
`crates/extensions/agent/src/sim/tests.rs` (`sim.*`, with the callees played by the
tests), `crates/extensions/agent/src/trace/tests.rs`, `crates/extensions/agent/src/havok.rs` and
`crates/extensions/agent/src/raw.rs` (synthetic guest memory), `crates/extensions/agent-proto`
(wire types), `crates/apps/boot/tests/feature_plan.rs` (the observer order).

## Not verified

- Nothing here was checked by running the game unless a section says so:
  the tests above use fake readers, synthetic guest memory, synthetic pads,
  synthetic frames and a fake sampler.
- `ui.audit`, `ui.press` and the menu crawl against a live game. Their regression
  cases use synthetic memory, input and agent replies.
- `state.*` against the real machine: the tests run over fakes.
- `sim.give`, `sim.spawn` and teleport against a running game: the callees
  are played by the tests.
- Lobby steps `invite` and `start` have not been pressed through by `sim`
  in a run.
- `render.frame`, `load.timeline`, `world.raw`, `havok.*` and the
  `trace`/`flight` recorder against a running game.
- `shadow.*` and `scripts/shadow-gate.sh` against the disc: the tests run
  over synthetic code and memory.
- `render.compare`, `render.capture`, `render.scene`, `render.parity` and
  `sim.hashes` against a running game.
- `maps.test.*` against a running game: the build, the slot and the
  queued load ([test-maps.md](../game/test-maps.md#not-verified)).
- Not provided: effect, projectile and particle counts; per-subsystem
  guest time; owner attribution of frame cost; kernel I/O seeks and cache
  hits in the load timeline; a widget-level HUD; setting a feature switch through the API; loading a named level
  other than a test map (`maps.test.load`).

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
