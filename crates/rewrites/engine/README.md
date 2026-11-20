# engine

Hand-written Rust replacements for Halo 2 engine functions, installed behind the runtime's override table one guest function at a time. Each type is a view over guest memory with the original layout.

## Where it sits

`rewrites` domain. Depends on `guest-layout`, `sim-core`, `sim-anim`, `sim-collision`, `simd`, `x87` and, with `runtime-glue`, `runtime`. Used by `render-bridge`, `sim-bridge`, `hooks`, `game`, `agent`, `bots`, `h1campaign` and `boot`.

## Layout

- `src/lib.rs`: the ported functions (tag lookup, datum arrays, locale, hs, squads, screen effects, lights).
- `src/sim_native/`, `src/sim_objects/`, `src/sim_collision/`, `src/sim_physics/`, `src/sim_anim.rs`: the adapters that gather, call the sim crates and scatter.
- `src/render_loop/`, `src/render.rs`: the render-loop ports.
- `src/entry.rs`, `src/abi.rs`, `src/point_shapes.rs`, `src/runtime_glue.rs`: entry points and hook shapes, re-exported from `guest-layout`.

## Features

- `runtime-glue`: the adapters over the runtime's guest memory (`dep:runtime`, `dep:mopp`).
- `differential`: differential tests against the lifted original (implies `runtime-glue`).

## Tests

`cargo nextest run -p engine`. The `differential` feature turns on seeded differential tests against the lifted original; they read the retail executable from `H2_XBE` (some also `H2_DISC`) and take `H2_DIFF_SEEDS` and `H2_DIFF_SCALE`.

## Docs

- [engine/README.md](../../../docs/engine/README.md)
- [engine/feature-engine.md](../../../docs/engine/feature-engine.md)
- [engine/guest-layout.md](../../../docs/engine/guest-layout.md)
- [engine/sim-rewrite.md](../../../docs/engine/sim-rewrite.md)
