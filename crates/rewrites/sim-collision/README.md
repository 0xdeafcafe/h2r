# sim-collision

The native collision world: each collision BSP the guest names is decoded once into flat arrays, and a query is one Rust call over them. Nothing here writes guest memory.

## Where it sits

`rewrites` domain, guest-free. Depends on `sim-core`. Used by `engine` (`engine::sim_collision` is the adapter).

## Layout

- `src/bsp/`: the decoded BSP and its tree walks.
- `src/world.rs`: the collision world and its queries.
- `src/mem.rs`: the `GuestRead` boundary.
- `src/synth.rs`: synthetic BSPs in guest form (`synth`).

## Features

- `synth`: synthetic collision BSPs laid out in guest form, for tests here and in `engine`. No game data.

## Tests

`cargo nextest run -p sim-collision`. No disc needed; the differential test is `engine`'s `sim_collision_differential`.

## Docs

- [engine/sim-rewrite-collision.md](../../../docs/engine/sim-rewrite-collision.md)
- [engine/collision-queries.md](../../../docs/engine/collision-queries.md)
- [engine/sim-rewrite.md](../../../docs/engine/sim-rewrite.md)
