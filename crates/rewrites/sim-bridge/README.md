# sim-bridge

The guest side of the native simulation: adapters from hook entries to the sim crates, the installs under `impl.sim.native`, the tick driver and the shadow checks that compare each port with the guest.

## Where it sits

`rewrites` domain. Depends on `hooks`, `runtime`, `engine` and `lifted`. Used by `game`, `agent` and `boot`.

## Layout

- `src/lib.rs`: the install entry point.
- `src/sim_native.rs`, `src/sim_objects.rs`, `src/sim_physics.rs`: the installs per subsystem.
- `src/sim_frame.rs`: the tick driver.
- `src/shadow.rs`: shadow checks against the guest.
- `src/sim_observer.rs`, `src/sim_static_light.rs`, `src/sim_submix.rs`: smaller ports.

## Tests

`cargo nextest run -p sim-bridge`.

## Docs

- [engine/sim-rewrite.md](../../../docs/engine/sim-rewrite.md)
- [engine/sim-rewrite-objects.md](../../../docs/engine/sim-rewrite-objects.md)
- [engine/sim-rewrite-collision.md](../../../docs/engine/sim-rewrite-collision.md)
- [engine/object-update.md](../../../docs/engine/object-update.md)
