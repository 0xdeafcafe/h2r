# sim-collision

The collision world holds decoded BSPs for native point-to-leaf and location queries, and decoded MOPP programs for the box query. It also supplies the segment kernels used in differential tests. The engine adapter commits accepted results to guest memory.

## Where it sits

`rewrites` domain, guest-free. Depends on `sim-core` and `mopp` (the opcode helpers, and `mopp::boxvm` as the box query's test oracle). Used by `engine` (`engine::sim_collision` is the adapter).

## Layout

- `src/bsp/`: the decoded BSP and its tree walks.
- `src/mopp/`: MOPP programs decoded into flat trees, and the integer box query (entry F), single and packet.
- `src/world.rs`: decoded BSPs, map invalidation, memory accounting and entry counters.
- `src/structure.rs`: the signed leaf-to-cluster table for the active structure.
- `src/mem.rs`: the `GuestRead` boundary.
- `src/synth.rs`: synthetic BSPs in guest form and random MOPP programs (`synth`).

## Features

- `synth`: synthetic collision BSPs laid out in guest form and random MOPP programs, for tests here and in `engine`. No game data.

## Tests

`cargo nextest run -p sim-collision`. No disc needed; the differential test is `engine`'s `sim_collision_differential`.

## Docs

- [engine/sim-rewrite-collision.md](../../../docs/engine/sim-rewrite-collision.md)
- [engine/collision-queries.md](../../../docs/engine/collision-queries.md)
- [engine/sim-rewrite.md](../../../docs/engine/sim-rewrite.md)
