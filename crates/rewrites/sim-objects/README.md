# sim-objects

The object update in Rust: typed views over the object pool, the definition table and the object kernels. Guest memory stays the only object table; this crate reads and writes it through `guest_layout`'s layouts.

## Where it sits

`rewrites` domain, guest-free. Depends on `guest-layout` and `sim-core`. Used by `sim-state`.

## Layout

- `src/view/`, `src/view.rs`: typed views over the object pool.
- `src/defs.rs`: the definition table.
- `src/synth.rs`: synthetic object pools and definitions (`synth`).

## Features

- `synth`: synthetic object pools and definition tags in guest form, for tests here and in readers built on this crate. No game data.

## Tests

`cargo nextest run -p sim-objects`. No disc needed.

## Docs

- [engine/sim-rewrite-objects.md](../../../docs/engine/sim-rewrite-objects.md)
- [engine/object-update.md](../../../docs/engine/object-update.md)
- [engine/guest-layout.md](../../../docs/engine/guest-layout.md)
