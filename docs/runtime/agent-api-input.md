# Agent API: audio, HaloScript, input and snapshots

Status: Built. Part of the [agent API](agent-api.md), which holds the wire, the method index and the rules.

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
unpositioned voice). `sends_to` lists the ids of the playing mix-ins the
voice's bins reach: a Halo stream in a submix names its emitter there. A
submix's `input_bin` is a host bin (`0x100` and up), which only its own
streams carry. The guest sees bin `0x1f` in their place
([dsound.md](../audio/dsound.md#positioned-sound)).

`spatial` is null for a 2D voice. For a positioned one it gives the
DirectSound 3D terms the mixer applies against the listener:
`position` as the game set it, `relative` to the listener, `min_distance`
and `max_distance` (null when not finite), `rolloff` (listener times
source), `distance_gain` and `cone_gain` (linear), their product `gain`,
`pan` (left and right gains), `front` (the front crosstalk share) and
`pitch` (the Doppler ratio). `applied_gain` is `direct_gain` times that
`gain`: the level the voice's direct path leaves with, before bin volumes
and pan. Halo's submixes have `FLT_MAX` distances and rolloff 0, so their
`distance_gain` is 1. The game's distance attenuation is the gain on the
stream's bin into the submix, so one emitter's level at a distance is that
bin `gain` times the submix's `applied_gain`.

`direct_gain` and `room_gain` are linear low-frequency gains including
voice volume and headroom. `direct_hf_db` and `room_hf_db` give the
additional loss at `reference_hz`. Each ordered entry in `bins` has its
raw `bin`, `volume_mb` and its linear `gain` including that pair's volume
and path gain. These are routing settings before spatial panning, master
gain and the reverb return. A disabled environment or unconnected bus can
have a configured send gain and deliver silence. Plain stereo routing has
`bins: null`.

### `audio.loops`

`h2 agent audio.loops` takes no parameters. It reports the sound table's
looping sounds (`Sounds::loops`) and the host's playout health.
Each row in `loops` gives the salted `loop_datum`, `tag_datum`, resolved
`tag_name` when the map names the tag, zero-based `track`, `segment`,
`sound_instance` and `sound_datum`. `segment` is `in`, `loop`, `out`, `alt`,
`alt_in`, `alt_transition_out`, `alt_out` or `unknown`. Overlapping instances
of a track get separate rows. `pitch_range`, `permutation` and `chunk` come
from the sound's selection in the table. The table can select the next
segment while PCM from its predecessor is still queued.

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

### `audio.meter`

The level of the host mixer's output bus: the stereo samples
DirectSound's output renders, before the device, so a run with no audio
device meters the same samples
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

- **A window.** Meters the latest `window_ms` of output. The capture
  starts with the first request and keeps 10 seconds, so the first call
  answers once that much has been rendered.
- **`play`.** Starts a host clip in the mixer as the game plays it:
  `pogo/compress`, `pogo/spring`, `pogo/settle` or `announcer/<medal id>`
  (an unknown name's error lists them). It answers when the clip has
  played, metering from the call to its end plus 20 ms, with anything
  else the game played meanwhile. `window_ms` is that span. The clip is
  removed from the mixer when the method answers or times out.
- **Levels.** Peak and RMS in dBFS, integrated loudness and the loudest
  400 ms block in LUFS (ITU-R BS.1770-4, the meter Studio's
  `sound.measure` uses), each to 0.1 dB; silence reads -120. `clipped`
  counts samples at or past full scale.
- **Errors.** A clock-only run (a light host or sim) renders no samples:
  `UNAVAILABLE`. So is a clip still a Git LFS pointer, or one over
  5 seconds. Bad params and unknown clips are `INVALID_PARAMS`. Audio
  time is logical, so a paused game does not answer until it runs.

### `audio.play`

Starts a sound tag at a player's unit, so a sound can be heard and read
back (`audio.voices`, `audio.meter`) without the game having a reason to
play it. A game method of the sim provider
(`crates/extensions/agent/src/sim/mod.rs`, `Sim::audio_play`; types in
`crates/extensions/agent-proto/src/sim/mod.rs`).

```sh
h2 agent audio.play '{"sound": "sound\\pogo\\spring"}'
```

| Params | Result |
|---|---|
| `{sound, player?}`: a `snd!` tag by datum or name (as `edit.tags` lists them); `player.<n>` or `sim.<n>`, default `player.0` | `{player, sound: {tag, group, name}, instance}` |

- **The call.** The game's own `sound_start_at_marker`
  (`SOUND_START_AT_MARKER`, `0x1891d0`) with the unit's head marker, the
  call a dialogue line starts with, made at the start of the next tick
  like `sim.give` ([`sim`](agent-api-sim.md#sim-simulated-players-and-direct-state)).
  Presentation only: nothing is filed, sent or recorded, so a co-op
  session may ask. `instance` is the sound instance the game made.
- **Errors.** A tag that is not a `snd!` or is not found is
  `INVALID_PARAMS`; a player with no unit is `GONE`; a sound the game
  starts no instance for (a tag with no permutation, or a full sound
  table) is `UNAVAILABLE` with the game's reason. Audio time is logical,
  so a paused game answers when it runs.

### `world.object.animate`

Plays a named animation of an object's own graph, so an animation can be
seen on the object without a gameplay reason to start it. A game method
of the sim provider (`crates/extensions/agent/src/sim/mod.rs`,
`Sim::object_animate`; the write in `crates/extensions/agent/src/probe/animation.rs`,
`start`; types in `crates/extensions/agent-proto/src/sim/mod.rs`).

```sh
h2 agent world.object.animate '{"id": "scenery.12340000", "animation": "idle"}'
```

| Params | Result |
|---|---|
| `{id, animation}`: an object id as `world.object` spells it (not a player); the animation's name in the graph the object's manager holds | `{object, graph, animation, index, frames, frame, session}` |

- **The call.** The name becomes a string ID of the loaded map; the
  graph's animation block (`jmad` `+0x2c`) is searched for it, and the
  object's first channel (the manager's own `+0x0`) is pointed at the
  entry: the animation word at `+4` (graph -1, the channel's own, and the
  index), the frame at `+0x1c` set to 0 and the rate at `+0x18` to 1.
  It is made at the start of the next tick, like `sim.give`
  ([`sim`](agent-api-sim.md#sim-simulated-players-and-direct-state)). The animation is
  the graph's own: an inherited one is not found.
- **Host only.** An object's pose is simulation state, so a co-op
  (synchronous) session refuses it as `sim.spawn` is refused.
- **Errors.** A bad or player id, or a name the map has no string ID for,
  is `INVALID_PARAMS`; an id that no longer names the object is `GONE`; a
  graph with no such animation is `INVALID_PARAMS`; an object with no
  animation manager or no loaded graph is `UNAVAILABLE`.
- **Not yet confirmed in a running game.** The channel's fields are the
  ones the engine's channel set and frame setter read and write
  (`live-telemetry` `layout`), but a biped's own animation state restarts
  its channel as its names change, so the method is for scenery, machines,
  crates, creatures and vehicles; the answer's `frame` and a later
  `anim`/`world.object` read are how a run checks it plays.

### `recording.play`

Plays one of the loaded scenario's recorded animations on a player's
unit, as the script call `recording_play` does: the unit's control
follows the recording's stream. The editor's Recordings section calls it
([rich-editors.md](../editor/rich-editors.md), "Recordings"). A game
method of the sim provider (`crates/extensions/agent/src/sim/mod.rs`,
`Sim::recording_play`, the block read in `sim/recording.rs`; types in
`crates/extensions/agent-proto/src/sim/mod.rs`).

```sh
h2 agent recording.play '{"recording": "arrive", "mode": "hover"}'
```

| Params | Result |
|---|---|
| `{recording, player?, mode?}`: a name in the scenario's recorded animations (without case); `player.<n>` or `sim.<n>`, default `player.0`; `play` (default), `delete` or `hover` | `{player, recording, index, unit}` |

- **The call.** The recording player (`0x1fb360`; EAX the unit, CX the
  recording's index, the play flags as its one stack argument, `ret 4`,
  AL 0 when it starts nothing), made at the start of the next tick like
  `sim.give` ([`sim`](agent-api-sim.md#sim-simulated-players-and-direct-state)). The
  flags are the ones the three evaluators push (`0x2a2d1a`, `0x2a2d7a`,
  `0x2a2dda`): 0 for `play`, 8 for `delete`
  (`recording_play_and_delete`), `0x10` for `hover`
  (`recording_play_and_hover`). The unit moves, so a co-op session may
  not ask (`INVALID_REQUEST`, as for `sim.give`).
- **Errors.** A name the scenario lacks, or a mode that is none, is
  `INVALID_PARAMS` (the first lists the names the scenario has); a
  player with no unit is `GONE`; a recording the player starts nothing
  for (the unit already plays one, or no free slot) is `UNAVAILABLE`.
  A disc multiplayer map has no recordings. A paused game answers when
  it runs.

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

### `hs.eval`

`hs.eval {expression, type?}` compiles one HaloScript expression against
the loaded map's scripts and answers its value (`crates/extensions/agent/src/hs_eval.rs`).
It runs at the tick boundary the request is served at, over a snapshot:
the scenario's tables (`hs::Scenario::read_memory`), the builtin table
from the running image (`hs::Catalog::from_memory`, named by the map's own
calls) and the live cell of each scenario global. The text is wrapped in
a static script of a copy of the tables (`hs::compile`) and run on
`hs::vm`, the native evaluator ([hs_vm.md](../engine/hs_vm.md)); nothing
is written back to the game, so a `set` of a global changes the snapshot
only. Control flow, arithmetic, comparison and reads of the map's globals
run. A builtin that needs the game's objects, AI or sound is refused by
name until the guest-backed engine exists, and an expression that sleeps
is refused: the answer comes at one boundary.

- **`type`** names the result's type (`real`, `short`, `boolean`, ...).
  Without it the expression's own is used where it is fixed: a call of a
  builtin or script that returns a plain value, or a bare global's name.
  A `begin`, an `if` or a literal needs it.
- **Result.** `{type, type_index, value, bits}`: `value` is a boolean, a
  number, the string's text, or the raw bits of a handle or index, and
  `null` for void.
- **Errors.** Problems in the text are `INVALID_PARAMS` with the compiler's
  sentence and `data.problems: [{line, column, code, text}]`, counted in the
  expression (a `(` it never closes is named at its own place). An unknown
  `type` is `INVALID_PARAMS`. The evaluator's refusal (an engine builtin,
  a sleeping expression, a thread that faults) and unreadable or
  uninitialized scripts and globals are `UNAVAILABLE`.

```sh
h2 agent hs.eval '{"expression":"my_global"}'
h2 agent hs.eval '{"expression":"(begin (set counter 3) counter)","type":"short"}'
```

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
  22 bytes, the game tick) to `inputs/<name>.input` under the
  [agent output root](switches.md#tools). This is the `xapi::tap` text format
  that `H2_INPUT_RECORD` writes. The log is saved at `input.stop` or when
  the game exits. Names use 1 to 64 letters, digits, `.`, `_` or `-`
  and may not start with a dot. An existing log is kept unless `replace`
  is true. A [lab job](lab.md#runs) stores these logs in its run directory.
- **`input.list`** returns `{names}`, the sorted names of saved input logs
  without their `.input` suffix.
- **`input.log.read {name, offset?, limit?}`** returns `{text, next}` from
  a saved log. Use the same name as for `input.record`. The first offset
  defaults to 0. Pass `next` as the offset for the next page and stop at
  `null`. The limit is in bytes (default 16384, maximum 65536).
  Concatenate the text pages to reconstruct the log. Both file methods
  are read-only and require no claim.
- **`input.replay {name}`** answers reads from a log read for read (keyed
  by read index, as `H2_INPUT_REPLAY`). `input.state` and `input.stop` give
  its counts: `reads` in the log, `replayed`, `matched` (same handle as
  recorded), `handle_mismatches` (the runs have parted) and `past_end`
  (left live). One recording or replay at a time. A replay reproduces a
  run only from the same starting state: the same boot, or the same
  [snapshot](#state-machine-snapshots). Recording and playback exclude
  map reloads. A pending reload also prevents starting a recording or replay.
  After the transition completes a new input log can start in the loaded map.

#### Routes

`input.route {player?, legs, timeout_ticks?}` starts a sequence on a local
controller port (0 to 3, default 0). The timeout covers the whole route
and defaults to 1800 simulation ticks. The route uses the Clankers' path
search and movement on the game thread and feeds ordinary pad input
through the same tap as `input.pad`. It uses the default stick and button
layout and requires a living local player on foot. The route loads the
active BSP's bot nav mesh from the game's `bots` data directory, even
with no Clankers in the game. If the mesh is missing or stale, use
`h2 bots nav <map>`. See [Clankers' navigation](../game/bots.md#32-navigation).

Each route controls its port's full pad report while active. Starting a
route on a port that already has a route or queued pad steps returns
`BUSY`. `input.pad` also returns `BUSY` while the port runs a route.

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
`{ok, ticks, reason}`. `ok` is null while the leg is pending or running.
After a failure or stop, legs the route never started get `ok: false`
and `reason: "not_run"`. A jump leg includes its available `takeoff`,
`apex` and `landing` samples in `jump`, each with `position`, `velocity`
and `speed` in map units per second. The landing position comes from
the contact tick, or the last descending tick on an immediate rebound.
The landing velocity is the last observed incoming velocity.

Use `input.route.stop {player?}` to stop the route and release its port.
`input.release` also stops routes on the ports it releases. The final
status stays available until a new route starts on that port. The `route`
short form takes the array of legs. Use the full method to pick the
port or set the timeout.

```sh
h2 agent route '[{"move_to":[1,2,3],"crouch":false},{"wait":30}]'
h2 agent route-status
h2 agent input.route.stop
```

**Ascension superbounce setup.** Begin at approximately `(−10.85, 1.16)`
next to the low lip on Ascension. Face −120.7° for the crouch entry. The
route releases the button while pushing into the lip, turns to −66° to
exit, then walks to the upper ledge and jumps toward the floor seam.

```sh
h2 agent route '[
  {"face":-120.7},
  {"crouch_into":[-10.91,0.72,-1.16],"hold_ticks":15,"exit_face":-66},
  {"move_to":[-8.0,2.2,-0.7],"crouch":false},
  {"jump":{"from":[-8.42,2.51,-0.76],"land":[-10.39,4.00,-3.19]}}
]'
```

Compare the recorded jump with the reference takeoff heading `(-0.8, 0.6)`
at about 1.6 horizontal units/s and the landing's incoming vertical
velocity of about −5.4 units/s. See [Superbounce](../game/superbounce.md) for the traces and
collision geometry.

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

#### `input.lightbar`: PlayStation controller colours

The lightbar methods are available in player runs with XAPI input. They
work without a controller connected. `input.lightbar` is read-only and
takes no params. It returns `enabled` and four `ports`, each with a
zero-based `port`, the last sampled `game_rgb`, the temporary
`override_rgb`, and the desired `rgb`. Each colour is an array of three
integers from 0 to 255, or null. The desired colour uses the override if
set, otherwise the game colour. When the Lightbar setting is Off, desired
`rgb` is null and the game gives up control of the light.

`input.lightbar.set {port, rgb}` sets an override for one port and returns
the same state. A null `rgb` clears the override. The override lasts until
cleared or the run ends. Setting it follows the usual control
ownership rules for actions. The desired colour applies at input sync
when that port has a supported controller. It describes the host request
independently of what the hardware displays.

```sh
h2 agent input.lightbar
h2 agent input.lightbar.set '{"port":0,"rgb":[32,128,224]}'
h2 agent input.lightbar.set '{"port":0,"rgb":null}'
```

See [controller output](../audio/playstation-audio.md#player-colour-lightbars)
for the guest palettes and macOS lightbar backend. The methods live in
`crates/extensions/agent/src/lightbar.rs`.

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
An editor-bound DVD file or an unlinked placement preview prevents
machine snapshots, snapshot-based rewind and film keyframes. The host
keeps neither the play copy nor preview ownership in a snapshot.

```sh
h2 agent state save NAME [replace]
h2 agent state load NAME
h2 agent state list
h2 agent state drop NAME
h2 agent state fork FROM NAME [replace]
```
