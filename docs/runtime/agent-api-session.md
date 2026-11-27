# Agent API: the live mirror and the session

Status: Built. Part of the [agent API](agent-api.md), which holds the wire, the method index and the rules.

## Live mirror

The editor opens a second connection to the same Unix or TCP endpoint.
It calls `live.subscribe` there and reads the JSON reply line before
switching to length-prefixed binary frames. Keep the original JSON
connection for control calls. The stream reads the game at tick boundaries.

| Method | Params | Result |
|---|---|---|
| `live.subscribe` | `{layers: ["objects", "players", "camera"], every?, key_every?}` | `{at, stream, protocol: "h2-live-mirror/1", tick_rate, map: {name, checksum, epoch, activation}, layers, refused}`. The stream id is numeric. An unavailable checksum is null. Each unsupported layer has a reason under `refused`. |
| `live.status` | none | `{map: {name, checksum, epoch, activation}, streams, capture_us: {p50, p99, budget}, shed}`. Stream fields are `stream`, `layers`, `every`, `frames`, `bytes`, `queue`, `dropped` and `skipped`. Frames and bytes count complete writes, including the length prefix. |
| `live.close` | `{stream}` | `{closed: true}` if the stream existed. Disconnecting the binary socket also ends its subscription. |

`every` samples one tick in 1–30 (default 1). `key_every` requests a
full frame at least every specified number of game ticks (default 150).
An empty accepted layer set yields header-only frames. The first frame and frames after a cut or a dropped sample are keys.
Deltas identify records by salted object handle, player index or view key and carry
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
team, unit, held weapon, aim and local user.

`camera` (layer id 7) carries each view of the last sealed frame scene
(`frame_scene::View`; [framescene.md](../graphics/framescene.md)
section 4). A view's key is `kind << 16 | player << 8 | view`, the kind
indexing `game`, `cinematic`, `alternate`, `first_person`, which
`agent_proto::live::View::name` writes as `render.scene` does
(`game:0:0`). Each view has the scene's game tick, position, forward, up,
vertical field of view in radians, near, far, the viewport (top, left,
bottom, right in guest pixels), the cut flag and the salted handles of
the objects it drew, in draw order. The stream worker reads the scene,
not the game thread, so a camera can be a rendered frame behind the tick
it travels with. The game's scene glue registers the source
(`Agent::set_camera_source`, `scene_glue::install_cameras`). Without one,
or when boot reports that this run seals no scene (native rendering or
the frame scene off; `Agent::set_camera_unavailable`), `refused.camera`
gives the reason. Before the first sealed scene the layer is empty. The game thread fills reusable
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
| `session.takeover_info` | none | `{window, pid}`, whether the game has a window and its process id |
| `session.request_takeover` | `{ask?, requester, name, operation, builds_image, source, level, own_data, estimate_seconds, timeout_seconds}` | `{id}` for the consent prompt, or for the running job's question with `"ask": "running"` |
| `session.takeover_status` | `{id}` | `{decision}` for the request |
| `features.list` | none | `{at, plan, features}`: the feature plan text the game printed at start, and each feature as `{id, on, reason?}` (`reason` when the plan turned it off), read from the plan's `feature <id>: on` and `feature <id>: off (<reason>)` lines (`agent_proto::FeatureState`) |

`until` takes `tick`, `ticks` (more ticks than when the wait began),
`activation` (exact match), `map_ticks`, `present`, `phase`, `map` and `paused`; all given must hold,
at least one is required. `wait` settles at tick boundaries (and between
slices for wall-clock timeouts and while loading); a condition already true
answers at once. `timeout_ms` defaults to 30 s, at most 600 s; the failure
is `TIMEOUT` with the stamp.

```sh
h2 agent wait '{"phase":"gameplay","map_ticks":30}' 5000
h2 agent watch phase map          # events until interrupted
```

`session.wait` replaces every "sleep and screenshot" loop. Switches do not
change through `features.list`; live changes go through [`render.toggle`](agent-api-edit.md#edit-and-hot-render-live-tag-and-shader-changes)
and [`cheat.set`](agent-api-edit.md#menu-methods-settings-perf-overlay-cheats).

### Lab consent

The lab uses `session.takeover_info` to distinguish a windowed game
from a headless agent. Windowed player runs register
`session.request_takeover` and `session.takeover_status`. Windowed
replays and lockstep runs report the window without offering a prompt.
The request
uses the usual agent action access, so an unrelated connection holding
control can block it. `name` is the job's name. `operation` is
`"build"`, `"run"` or `"sim"`, and `builds_image` says the job builds the
image first. `source` is `{"kind": "branch", "name", "commit"}`,
`{"kind": "rev", "rev", "commit"}` or `{"kind": "current", "commit"}`
(the installed image; `commit` may be null). `level` is the run's level
or null, and `own_data` says the run plays on the player's own data.
`estimate_seconds` is an estimate, with zero for an unknown duration.
`timeout_seconds` is wall-clock time from acceptance, from 1 to 3600.
The request is refused with `INVALID_PARAMS` without a requester, a name
or a source. The game shortens the requester, name, level and source for
display. A second request returns `BUSY`
while a prompt is queued or open.

`alongside` (a boolean, true when left out) says whether the dialog offers
**Start alongside**. The lab sends `false` for a job whose numbers need a
quiet machine (a benchmark).

The status result is `pending`, `allowed`, `alongside`, `declined`,
`timed_out` or `unavailable` (the dialog failed to open). **Not now** and
Back both decline. **Close and start** answers `allowed`: consent for that
request, after which the lab closes the game with
[`host.quit`](#close-the-window) and waits for it to exit. **Start
alongside** answers `alongside`: consent to start the job at once with the
game left open, so the lab doesn't call `host.quit`. The dialog's rows are
**Not now**, **Start alongside** and **Close and start** (screen
`lab.takeover.alongside`, class `lab-takeover-alongside-dialog`), or only
**Not now** and **Close and start** (`lab.takeover`) when `alongside` is
false; a game never answers `alongside` to a request that withheld it. A request expires even if the UI is still loading. The game
dismisses the expired dialog at the next UI update. The status method is
read-only and answers `GONE` for an unknown or superseded id.

`ask` is `"start"` when left out: the consent prompt above. With
`"ask": "running"` the request asks about a job already running ([When the
player opens a game](lab.md#when-the-player-opens-a-game)), in its own
dialog (`lab.running`, class `lab-running-dialog`). Its rows are **Finish
this one**, **Keep running** and **Stop and pause**, answering
`finish_this`, `keep_running` and `stop_and_pause`; Back also answers
`finish_this`, and an unanswered question `timed_out`.

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
it is meant for `h2 serve`; a windowed game pauses with
[`host.pause`](#window-pause-and-frame-step). Input calls need no claim while nobody holds
control.

```sh
printf 'claim\npause\nstep 1\nobject player.0\nrelease\n' | h2 agent -
```

### Window pause and frame step

`host.pause {on}` is the window's own pause, the one Cmd+P toggles, asked
for over the API; `host.pause {}` reads the state, changing nothing.
`host.step {ticks?}` runs game ticks under the pause. They are
meant for `h2 play` and other windowed games, where `session.pause` also
stops the window's event loop. The window stays live: it keeps drawing
its last frame, the debug menu and overlays keep working, and the audio
pauses as for Cmd+P.

| Method | Params | Result |
|---|---|---|
| `host.pause` | `{on: bool}`, or `{}` to read | `{paused, held, tick}` once the window has applied it |
| `host.step` | `{ticks?}`: 1 to 1800, default 1 | `{paused, held, tick, stepped}` once the ticks ran and the game is held again |

`paused` is the window's pause (Cmd+P's, whoever set it); `held` says
the game thread has stopped at its pause wait; `tick` is the game's tick
at the last game tick. A pause waits up to 2 s for the window and up to
2 s more for the hold, so `held` can be false during a long load. Cmd+P
and `host.pause` change the same pause, so either undoes the other.
`stepped` counts the game ticks the step ran. A frame that runs several
ticks before the game stops again (catch-up after a slow frame) gives a
`stepped` above `ticks`.

Refusals: no window (a headless game) is `UNAVAILABLE`, naming
`session.pause`; `host.step` while not paused is `INVALID_REQUEST`, with
a step already running `BUSY`, and `ticks` out of range `INVALID_PARAMS`.
A resume (Cmd+P or `host.pause {"on": false}`) or a closed window during
a step answers it with `INTERRUPTED`. Ticks that do not run within 0.2 s
each plus 5 s (at most 2 minutes) answer `TIMEOUT` with `data.stepped`;
the step then ends. Focus pause (`pause_on_focus_loss`) is not the
window's pause: a game paused only by losing focus refuses a step.

Both are answered on the connection's own thread
(`Agent::add_connection_read`), since a held game thread answers
nothing, so they skip the controller check that **act** methods have.
The in-process debug menu sends from the window's own thread, which
cannot wait for itself: there `host.pause` answers at once with the pause
asked for and `held` false, and `host.step` answers at once without
`stepped` while the step runs on (a second step is `BUSY` until it ends). The
window shell (`crates/apps/app/src/shell.rs`) applies each request as
Cmd+P does. The game thread's pause wait (`crates/apps/app/src/lifecycle.rs`)
lets the game run while a step has ticks left and reports when it is held.
An agent ticker counts the game ticks (`crates/extensions/agent/src/sim/pause.rs`).
They meet in `agent_proto::host_pause`. The game stops at the next
slice boundary after the step's last tick.

Every other method is answered on the game thread, so while the game is
held it waits. A request sent while held is answered at the next tick a
step runs; `session.pause` and `session.step` are the way to read the
world between ticks without running any.

```sh
h2 agent host.pause '{"on":true}'
h2 agent host.step '{"ticks":10}'
h2 agent host.pause '{"on":false}'
```

### Window screenshots

`host.screenshot {path?}` writes the window's newest published frame - the
one it is showing, without the debug menu or the overlays - as a PNG, and
answers `{path, width, height}` with what it wrote. With no `path` it
names a new file in the window's screenshots directory (`h2 play`'s is
`screenshots/` under the data root).

Like `host.pause` it is answered on the connection's own thread, with the
window shell's capture (`crates/apps/app/src/shell.rs`, over
`agent_proto::host_shot`): the frame source's newest frame read back and
encoded on the asking thread, so the debug menu's own call works while
the game is paused, and a call from `h2 agent` never waits on the window's
event loop. A headless game has no window: `UNAVAILABLE`, naming
`render.capture` for a frame there. A window still starting, one with no
frame yet, a write that fails, and a default-path shot where the window
has no screenshots directory are `UNAVAILABLE` with the reason.

```sh
h2 agent host.screenshot '{}'
h2 agent host.screenshot '{"path":"/tmp/ledge.png"}'
```

### Close the window

`host.quit {}` closes the game window as its close button and Cmd+Q do:
the window shell stops the guest and leaves its event loop, and the
process exits once the guest has stopped. It answers `{closing: true}` at
once, before the window has closed; the caller watches the process to
see it go. It takes no params (`{}` or none), and a repeat is harmless.
The lab calls it when the player answers **Close and start** to a
[consent request](#lab-consent) ([Player consent before a
job](lab.md#player-consent-before-a-job)).

The game has no save step of its own to run first: a campaign keeps what
its last checkpoint saved, as with the close button. Like `host.pause` it
is answered on the connection's own thread, through the window shell's
hub (`crates/apps/app/src/shell.rs`, over `agent_proto::host_quit`), so a
paused game takes it too. A headless game has no window: `UNAVAILABLE`.

```sh
h2 agent host.quit '{}'
```

### Resize the window

`host.window.resize {width, height}` sets the game window's inner size,
in physical pixels (the size the game renders at), and
`host.window.resize {fullscreen}` takes the window in or out of
borderless fullscreen on its display, as Cmd+F does. An aspect or
field-of-view check drives a live window through the shapes it covers
with it. A size leaves fullscreen first; `{width, height, fullscreen:
false}` says the same thing, and a size with `fullscreen: true` is
refused, since a fullscreen window takes its display's size.

| Method | Params | Result |
|---|---|---|
| `host.window.resize` | `{width, height}` (each 64 to 16384), `{fullscreen: bool}`, or both with `fullscreen: false` | `{width, height, fullscreen, scale}` once the window has that shape |

`scale` is physical pixels per point (2 on a Retina display). The call
waits up to 5 s for the window to take the shape: a fullscreen change
animates on macOS, and a size asked for while leaving fullscreen is asked
again once the window is out. A shape the window does not take in time
(larger than the display, or a fullscreen size other than the one asked)
answers `TIMEOUT`, with the window's state as `data`.

Like `host.pause` it is answered on the connection's own thread, through
the window shell's hub (`crates/apps/app/src/shell.rs`, over
`agent_proto::host_window`): the shell applies the request on its event
loop and reports the window's shape after it and on every resize. A
headless game has no window: `UNAVAILABLE` (its render size comes from
its command line). Missing or partial params (a width with no height),
a side out of range and unknown fields are `INVALID_PARAMS`.

```sh
h2 agent host.window.resize '{"width":2560,"height":1080}'
h2 agent host.window.resize '{"fullscreen":true}'
h2 agent host.window.resize '{"fullscreen":false}'
```
