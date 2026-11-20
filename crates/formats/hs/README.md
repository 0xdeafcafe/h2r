# hs

Compiled HaloScript for the retail Xbox build: decoding, checking and
casting scenario scripts, and a VM that runs them. Tables and scripts come
from the owner's disc and XBE at run time; none are build inputs.

## Where it sits

Domain `formats`; it depends on `map` and `xbe`. `boot` and `game` run
scripts through it, `h1campaign` uses the Halo 1 slot and conversation
modules, and `cli` inspects scripts.

## Layout

- `src/source.rs`, `src/expression.rs`, `src/scenario.rs`, `src/wire.rs`:
  the script text, expression nodes and scenario script data.
- `src/catalog.rs`, `src/check.rs`, `src/cast.rs`: the function catalog,
  the checker and `castable`.
- `src/vm/`: the interpreter (`engine`, `eval`, `thread`, `value`).
- `src/h1_ai_slots.rs`, `src/h1_conversation.rs`: Halo 1 mappings.

## Tests

`cargo nextest run -p hs`. `tests/casts_disc.rs` and `tests/vm_disc.rs`
are `#[ignore]`d: they need `H2_DISC` (your disc image), and the VM test
also reads `H2_XBE`, `H2_HS_MAP` and `H2_HS_TICKS`
(`cargo test -p hs -- --ignored`).

## Docs

- [docs/formats/haloscript.md](../../../docs/formats/haloscript.md)
- [docs/engine/hs_vm.md](../../../docs/engine/hs_vm.md)
