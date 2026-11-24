# sim-collision

The collision world decodes BSPs, MOPP programs and portal graphs for segment, point, box, ray and object queries. The engine adapters select exact native exits and compare the other candidate results against lifted returns.

## Where it sits

`rewrites` domain, guest-free. Depends on `sim-core` and `mopp` (the opcode helpers, and `mopp::boxvm` as the box query's test oracle). Used by `engine` (`engine::sim_collision` is the adapter).

## Layout

- `src/bsp/`: the decoded BSP and its tree walks.
- `src/mopp/`: MOPP programs decoded into flat trees, the integer box query (entry F), and the float ray walker (entry I).
- `src/model.rs` and `src/instance.rs`: object model rays and placement transforms.
- `src/batch.rs`: ordered visit batches and box packets.
- `src/store.rs`: shared backing for immutable decoded columns.
- `src/world.rs`: decoded BSPs, map invalidation, memory accounting and entry counters.
- `src/flood.rs`: the structure's portal graph and the sphere cluster flood (entry G).
- `src/objects.rs`: the object spatial visit over the live reference lists (entry H).
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
