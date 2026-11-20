# h1-contract

The names both Halo 1 halves share: what the converters write into a
converted map and what the game's Halo 1 hooks read back from guest
memory. It holds only names, offsets, markers, slot tables and small
layout types, so the in-game half needs no converter crate.

## Where it sits

Domain `formats`; it depends on `map`. `h1convert`, `h1campaign` and
`h1-assets` (in `crates/tools`) write what it names, and `boot` and `game`
read it. The converters re-export each item from the module it was written
beside.

## Layout

- `src/campaign.rs`, `src/cinematics.rs`: campaign and cinematic contracts.
- `src/hud.rs`, `src/hud_builtins.rs`: HUD layouts and built-in names.
- `src/script_fields.rs`, `src/unit_builtins.rs`: script slots and
  function names.
- `src/shader.rs`: shader markers.

## Tests

The crate has no tests of its own; the converters and hooks that use it do.
(`cargo check -p h1-contract`).

## Docs

- [docs/dev/crate-structure.md](../../../docs/dev/crate-structure.md)
- [docs/halo1/text.md](../../../docs/halo1/text.md)
- [docs/halo1/script-builtins.md](../../../docs/halo1/script-builtins.md)
