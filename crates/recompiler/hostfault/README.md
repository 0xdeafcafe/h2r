# hostfault

The process-wide host fault handler. One handler per process is installed
on first use (`SIGBUS` and `SIGSEGV` on Apple systems, `SIGSEGV` on Linux).
It holds two things in fixed arrays, so the handler never allocates or
locks: regions (an address range plus a callback that runs inside the
handler) and fixup tables that send a faulting access to its slow path.

## Where it sits

Domain `recompiler`; no workspace dependencies. `emit` generates the fixup
tables it reads and `runtime` registers regions. It sits in `recompiler`
because `emit` uses it and `runtime`, which also does, may use `recompiler`.

## Layout

- `src/lib.rs`: handler install and region registration.
- `src/table.rs`: `Table`, `Format`, `register_table`, `lookup`.
- `src/unix.rs`: the signal handler.

## Tests

`cargo nextest run -p hostfault`. `tests/chain.rs` exercises handler
chaining.

## Docs

- [docs/runtime/contract-thin-transfers-flat-ram.md](../../../docs/runtime/contract-thin-transfers-flat-ram.md),
  section 4.1.
