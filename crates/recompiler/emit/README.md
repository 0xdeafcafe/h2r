# emit

The C11 back end: planned semantic IR in, disposable C out, written against the runtime's versioned ABI. It decides each owner's shape, writes one handler per owner and shards the result by entry address so unchanged files are not rewritten.

## Where it sits

`recompiler` domain. Depends on `ir`, `opt`, `lift`, `analyze`, `x87` and `xbe`, plus `hostfault`. Used by `clif`, `engine` (differential tests), `perf`, `recomp`, `boot` and `cli`.

## Layout

- `src/lib.rs`: `plan` (owner shapes, runs `opt`) and `write_module` (shards, registration object).
- `src/c.rs`, `src/c/`: the C writer, one handler per owner.
- `src/compile.rs`, `src/compile/`: generated C to objects and static libraries with clang, cached by content hash.
- `src/clock.rs`, `src/backedge.rs`: block-clock accounting and polled loop headers.
- `src/x87.rs`: x87 output.
- `src/sim.rs`: the sim tier, a second compile of the same IR under a looser contract.
- `src/host.rs`: a minimal host for generated code, for tests and `--run-entry`.
- `src/par.rs`, `src/output.rs`: parallel emit and publishing files without truncating an existing inode.

## Features

- `oracle-tests`: pulls in `oracle` (with its `unicorn` backend) for the oracle-checked tests.

## Tests

`cargo nextest run -p emit`. Most `tests/` compile generated C and need a C compiler; the ones that read the retail executable take `H2_XBE`. Build-shape switches (`H2_OPT`, `H2_FLAT_RAM`, `H2_PGO`) are in [switches](../../../docs/runtime/switches.md).

## Docs

- [lifter/LIFTER.md](../../../docs/lifter/LIFTER.md)
- [lifter/BUILD-CACHE.md](../../../docs/lifter/BUILD-CACHE.md)
- [lifter/FLOAT-ABI.md](../../../docs/lifter/FLOAT-ABI.md)
- [lifter/cranelift-tier.md](../../../docs/lifter/cranelift-tier.md)
