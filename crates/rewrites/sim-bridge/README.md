# sim-bridge

The guest side of the native simulation: adapters from hook entries to the sim crates, the installs under `impl.sim.native`, the tick driver and the shadow checks that compare each port with the guest.

## Where it sits

`rewrites` domain. Depends on `hooks`, `runtime`, `engine`, `lifted` and `sim-ext`. Used by `game`, `agent` and `boot`.

## Layout

- `src/lib.rs`: the install entry point.
- `src/sim_native.rs`, `src/sim_objects.rs`, `src/sim_seats.rs`, `src/sim_physics.rs`, `src/sim_sound.rs`: the installs per subsystem. (The seats run as the shadow's guest-authoritative arm first, then answer themselves.)
- `src/sim_frame.rs`: the tick driver.
- `src/shadow.rs`: shadow checks against the guest.
- `src/sim_ext.rs`: the `SimExt` records moved with checkpoint save, revert and full restore, and its snapshot section.
- `src/sim_status_poll.rs`: a smaller port.

## Tests

`cargo nextest run -p sim-bridge`.

## Docs

- [engine/sim-rewrite.md](../../../docs/engine/sim-rewrite.md)
- [engine/sim-rewrite-objects.md](../../../docs/engine/sim-rewrite-objects.md)
- [engine/sim-rewrite-collision.md](../../../docs/engine/sim-rewrite-collision.md)
- [engine/object-update.md](../../../docs/engine/object-update.md)
