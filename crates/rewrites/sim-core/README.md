# sim-core

Shared ground for the native simulation: bit-level math types, the exact SSE-order `f32x4` lanes, the float rules every kernel follows and the epoch that invalidates derived state. It has no `runtime` dependency.

## Where it sits

`rewrites` domain, guest-free. Depends on `simd`. Used by `sim-anim`, `sim-collision`, `sim-objects` and `engine`.

## Layout

- `src/math.rs`: bit-level math types.
- `src/lanes.rs`, `src/lanes/`: exact SSE-order lanes, with host lanes where they match.
- `src/det.rs`, `src/det/`: bounded SSE arithmetic, checked host arithmetic and x87 precision chains.
- `src/epoch.rs`: the derived-state invalidation counter.

## Features

- `override`: `lanes::with` forces host lanes on or off on one thread, for tests that compare both paths.

## Tests

`cargo nextest run -p sim-core`. No disc needed.

## Docs

- [engine/sim-rewrite.md](../../../docs/engine/sim-rewrite.md)
- [engine/collision-queries.md](../../../docs/engine/collision-queries.md)
- [engine/sim-rewrite-objects.md](../../../docs/engine/sim-rewrite-objects.md)
- [perf/sim-budget.md](../../../docs/perf/sim-budget.md)
