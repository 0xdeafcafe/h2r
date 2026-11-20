# hooks

The feature registry. A feature declares the named points it attaches to; the registry turns every declaration into one ordered chain per guest address, or a list of conflicts naming both parties, at plan time. It also holds the switches, a guest arena and the frame and game bridges.

## Where it sits

`hooking`. Uses `runtime`, `engine`, `timeline` and `render-bridge`. Used by `boot`, `agent`, `game`, `h1-runtime`, `lockstep`, `live-telemetry`, `render-bridge` and `sim-bridge`.

## Layout

- `src/lib.rs`: `Builder` and `Registry`.
- `src/plan.rs`: `Plan` and its conflicts (pure, unit-tested).
- `src/install.rs`: placing one override per address.
- `src/entry.rs`, `src/leaves.rs`: hook entries and leaf functions.
- `src/switches.rs`, `src/switches/`: the `impl.*` switches.
- `src/arena.rs`: the guest arena.
- `src/bridge.rs`: the frame and game bridges.

## Tests

`cargo nextest run -p hooks`. `tests/plan.rs` and `tests/install.rs` use synthetic guests.

## Docs

- [feature-engine.md](../../../docs/engine/feature-engine.md)
- [guest-layout.md](../../../docs/engine/guest-layout.md)
- [switches.md](../../../docs/runtime/switches.md)
