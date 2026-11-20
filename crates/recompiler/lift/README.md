# lift

The lifter: decoded x86 instructions lowered into the semantic IR, one instruction to one `IrInsn`, then whole functions from the analyzer's control-flow graphs.

## Where it sits

`recompiler` domain. Depends on `ir`, `analyze` and `xbe` (dev: `opt`). Used by `emit`, `lifted`, `recomp`, `engine`, `native-render`, `perf` and `cli`.

## Layout

- `src/lower/`: per-instruction lowering (`lower()`).
- `src/builder.rs`: the `Lifter` that walks CFGs and fills each transfer's site.
- `src/crt.rs`: lifter-side replacements for CRT floating-point helpers.
- `src/harness.rs`: runs an image on the IR interpreter from its entry point to the first kernel call.

## Features

- `oracle-fixtures`: cross-checks lowering against oracle fixture files.

## Tests

`cargo nextest run -p lift`. `tests/entry.rs` reads the retail executable from `H2_XBE`. `tests/oracle.rs` and `tests/oracle_x87/` run only with the `oracle-fixtures` feature and read `H2_ORACLE_FIXTURES`.

## Docs

- [lifter/LIFTER.md](../../../docs/lifter/LIFTER.md)
- [lifter/oracle.md](../../../docs/lifter/oracle.md)
- [lifter/FLOAT.md](../../../docs/lifter/FLOAT.md)
