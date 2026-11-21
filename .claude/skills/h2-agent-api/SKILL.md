---
name: h2-agent-api
description: Use h2 agent or the h2-editor socket API to check the running game, drive the game, inspect menus or inputs, measure frame rate, capture a screenshot, or inspect the editor. Use it to attach to an existing window or headless run.
---

# Inspect and drive an existing game

Attach to the game the user or coordinator has running. Use the API for
state, input, measurements and images. Keep screen scraping, manual
process memory reads, OS-level synthetic input and one-off debug hooks
out of this workflow. See [AGENTS.md](../../../AGENTS.md).

Use the endpoint for the run you intend to check. The same game API is
available in an agent-enabled window and in an existing headless
`h2 serve`. For a run on another Mac, use the lab's agent relay.
See the [lab skill](../h2-lab/SKILL.md).

## Find the socket

```sh
h2 agent games
h2 agent status
h2 agent session.methods
```

`games` shows the discovered games' socket, PID, map and phase. A normal
`h2 play` advertises at `~/Library/Caches/h2recomp/agents/<pid>.sock`
when `H2_AGENT` is unset. `h2 serve` defaults to
`~/Library/Caches/h2recomp/agent.sock`. For a custom socket, get the path
from whoever started the game.

Without `--socket`, the client tries `$H2_AGENT`, then the default
socket. With neither configured nor present, it can pick a sole
discovered game. If more than one game is running, pass the intended path
explicitly. Replace the example PID with the one shown by `games`.

```sh
agent_socket=~/Library/Caches/h2recomp/agents/12345.sock
h2 agent --socket "$agent_socket" session.status
h2 agent --socket "$agent_socket" session.methods
```

The rest of the examples use the default connection selection. Add
`--socket "$agent_socket"` when selecting a specific process.
`session.methods` lists the methods registered in that process. Check it
before using methods tied to a renderer, input tap or host feature.
`h2 agent --help` lists the CLI shortcuts. Full method calls take one
quoted JSON argument and print JSON. RPC errors exit nonzero.

## Wait for game state

Read `session.status` for the current phase, map, tick and present in
`at`, the responding protocol version and the controller claim. Its
`fps` comes from the frame-stats window when one is available.

```sh
h2 agent session.status
h2 agent --timeout 65 session.wait \
  '{"until":{"phase":"gameplay","map_ticks":30},"timeout_ms":60000}'
h2 agent wait '{"ticks":30}' 5000
h2 agent watch phase map
```

Use `session.wait` for the phase or tick you need before reading state or
acting. All fields under `until` must hold. `tick` is a target tick,
`ticks` counts forward from when the wait begins, and `map_ticks` counts
from map activation. Add `map` when a transition to a specific map
matters. Use `present` to wait on rendered frames. Use `timeout_ms` to
bound waits through loads, and keep the CLI's `--timeout` in seconds
above the RPC deadline. If it times out, check the reported stamp and
current status. `watch` streams transitions until interrupted.

## Inspect and send input

```sh
h2 agent input.state
h2 agent ui.stack
h2 agent ui.screen
```

Use `input.state` to verify the open controller ports, queued steps and
what each port holds. Use `ui.stack` for the active menu channels and
Back target. `ui.screen` needs a run with `H2_UI_WATCH=1` and supplies
active screen, dialog and layout findings.

When the run is available for driving, submit a bounded pad step, wait
and check the result. Input goes through the game's controller path.
Choose the port from `input.state`. A queued step survives the CLI
connection, so clear your own queued input if you end the check early.

```sh
h2 agent input.pad '{"player":0,"steps":[{"left":[0,1],"ticks":15}]}'
h2 agent wait '{"ticks":16}' 5000
h2 agent input.state
h2 agent input.release '{"player":0}'
```

Pause and step require a claim on the same connection. Use a single
stdin script on a headless session you can control. The claim ends when
the connection closes, which also releases the pause.

```sh
printf 'claim\npause\nstep 1\nstatus\nresume\nrelease\n' | h2 agent -
```

## Measure or capture a frame

```sh
h2 agent host.metrics
h2 agent render.stats
h2 agent profile.counters
h2 agent --timeout 90 capture 600
```

`host.metrics` reports process CPU, RSS and tick cost after its first
complete tick window. Read its sample age when looking at a load or
pause. `render.stats` reports render pipeline counters and interpolated
frame holds where those paths are active. `profile.counters` reads a
rolling window. `capture 600` is short for a 600-game-tick
`profile.capture`. It returns frame-time percentiles, busy time without
pacing, slow frames and a CSV path. Use a gameplay wait before measuring
and report the window and render configuration with the numbers.

For frame or pass layout, use `frame` (the CLI arms and reads
`render.frame`). For where each draw lands on the display, with raster and
texture coordinates (stretching, pillarbox and edge stripes), use
`render.frame_layout {"arm":true,"max_draws":2000}`
([agent-api.md](../../../docs/runtime/agent-api.md#renderframe_layout)).
`render.scene` inspects the last sealed scene when a source is wired.
Check `session.methods` for these views.

```sh
h2 agent frame
h2 agent frame draws max 40
h2 agent render.scene '{}'
```

Request a game PNG with `render.capture` and use the path in the reply.
The presented-frame capture needs a compatible GPU run with
`H2_BOOT_GFX`. For a pass image, use `frame snap` with an index
returned by `frame`. Replace the example index with that pass.

```sh
h2 agent render.capture '{}'
h2 agent frame snap 0
```

Capture replies include a tick or present number you can cite to tie the
image to the check. For readback requirements, clip capture and result
fields, see the [render API](../../../docs/runtime/agent-api.md#render-the-frame-and-the-pipeline).

## Use the debug menu's controls and live edits

When registered, `features.list`, `settings.list`, `render.toggle` and
`render.param` expose the values behind the debug menu. Start with reads.
`cheat.list` includes reasons a cheat is refused. When checking HUD
visibility on an eligible run with `hide_hud` initially off, set it and
restore it after the check.

```sh
h2 agent features.list
h2 agent settings.list '{"prefix":"graphics."}'
h2 agent render toggle
h2 agent cheat.list '{}'
h2 agent cheat.set '{"name":"hide_hud","value":true}'
# Check the HUD here.
h2 agent cheat.set '{"name":"hide_hud","value":false}'
```

For a live tag edit, get a datum from `edit tags`, read the field and
use the journal entry to revert your change. Replace the example datum
and change id with the ones returned by this run. On shared sessions,
revert by id so other edits stay live.

```sh
h2 agent edit tags shad
tag=0xe1230045
field='postprocess[0].colors[0].color'
h2 agent edit get "$tag" "$field"
h2 agent edit set "$tag" "$field" 0xffff00ff
h2 agent edit changes
h2 agent edit revert 3
```

See [hot iteration](../../../docs/graphics/hot-iteration.md) for field
paths, shader overrides and which edits take effect on the next frame.

## Attach to the editor

Use the editor's own socket with `h2 studio` or `h2-editor call`. It
defaults to `~/Library/Caches/h2recomp/studio.sock` and can be selected
with `H2_STUDIO` or `--socket`. Check `session.methods` before reading
or editing the document. Entries marked `planned` answer `NOT_YET`.
See the [editor API](../../../docs/editor/agent-api.md) for method
schemas and how to inspect pending edits and save them.

```sh
h2 studio session.status
h2 studio session.methods
```

## Add a recurring check to the API

If a needed view, action or check is missing and will recur, extend the
API before using it. For a genuine one-off, record why the API could not
cover it and why an extension would have no later use. Record the
decision in the report or commit message in either case.

1. Define the method name and serializable params and result alongside
   its method group in [agent-proto](../../../crates/extensions/agent-proto/src).
2. Register a handler in the owning feature with `Agent::add_method` for
   host state or `Agent::add_game_method` for game state at a tick
   boundary. Set its access to `Read`, `Act` or `Claimed` and register
   it only when the feature can answer it. Model the handler on
   [the agent executor](../../../crates/extensions/agent/src/exec.rs).
3. Test dispatch, the result and relevant validation or access errors
   with synthetic inputs. Add socket coverage where the client boundary
   matters. See [the agent tests](../../../crates/extensions/agent/tests).
4. Document the method, params, result and prerequisites in the
   [runtime agent API](../../../docs/runtime/agent-api.md). Use
   `session.methods` and the new call on a run with the feature enabled
   when the task permits runtime checks.
