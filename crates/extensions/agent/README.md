# agent

The agent API server: a Unix socket (a TCP port for the Xbox app) in the game process, answered at tick boundaries. It is how agents and tests read and drive the running game.

## Where it sits

`extensions`. Uses `agent-proto`, `hooks`, `engine`, `guest-layout`, `runtime`, `sim-state`, `sim-bridge`, `render-bridge`, `live-telemetry`, `hot-tags`, `hot-render`, `debug-menu` and `snapshot`. Used by `boot`, `cli`, `studio` and `game`.

## Layout

- `src/server.rs`: the listener and the request queue.
- `src/lib.rs`: `Agent` (the executor, `on_tick`, `between_slices`, `add_method`) and `AgentFeature`, the read-only frame observer.
- `src/exec.rs`, `src/exec/`: method dispatch.
- `src/hot.rs`, `src/render.rs`, `src/sim/`, `src/havok.rs`, `src/trace.rs`, `src/shadow.rs`, `src/menu.rs`, `src/raw.rs`: the method groups.
- `src/hostpads.rs`, `src/tap.rs`: pad and input taps.

## Features

- `default` = `render-overrides`.
- `render-overrides`: shader and render overrides through `hot-render`.

## Tests

`cargo nextest run -p agent`. `tests/socket.rs`, `tests/tcp.rs`, `tests/sim.rs`, `tests/render.rs` and `tests/loads.rs` run without a disc.

## Docs

- [agent-api.md](../../../docs/runtime/agent-api.md)
- [hot-iteration.md](../../../docs/graphics/hot-iteration.md)
- [framescene.md](../../../docs/graphics/framescene.md)
- [dedicated-host.md](../../../docs/live/dedicated-host.md)
