# pathfind

Halo 2 AI pathfinding data. It decodes retail structure-BSP pathfinding,
generates it from a BSP's collision geometry, compares generated against
retail, and emits a `map::build` block tree.

## Where it sits

Domain `formats`; it depends on `map` and `tags`. `h1campaign` and `bots`
use it.

## Layout

- `src/generate.rs`: `generate` and its `Options`.
- `src/geom.rs`: the geometry it works on.
- `src/compare.rs`: generated against retail.
- `src/build.rs`: emit blocks for `map::build`.

## Tests

`cargo nextest run -p pathfind`. `tests/generate.rs` and the examples read
the owner's disc: the tests are `#[ignore]`d and need `H2_DISC`
(`cargo test -p pathfind -- --ignored`).

## Docs

- [docs/formats/pathfinding.md](../../../docs/formats/pathfinding.md)
- [docs/formats/collision.md](../../../docs/formats/collision.md)
