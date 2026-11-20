# oracle

An independent x86-32 instruction oracle that makes synthetic differential fixtures. It uses no lifter or runtime code and no generated instruction semantics, so a bug there cannot hide here.

## Where it sits

`recompiler` domain. No workspace dependencies. Used by `emit` (the `oracle-tests` feature).

## Layout

- `src/fixtures.rs`: fixture generation.
- `src/backend.rs`: the execution backends (Unicorn when enabled).
- `src/flags.rs`: flag masks.
- `src/bin/fixtures.rs`: the `oracle-fixtures` binary that writes fixture files.

## Features

- `unicorn`: enables the Unicorn execution backend (`dep:unicorn-engine`).

## Tests

`cargo nextest run -p oracle`. Execution tests need the `unicorn` feature. No disc needed.

## Docs

- [lifter/oracle.md](../../../docs/lifter/oracle.md)
- [lifter/LIFTER.md](../../../docs/lifter/LIFTER.md)
