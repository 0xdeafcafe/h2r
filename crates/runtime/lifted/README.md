# lifted

The lifted original: the image's own code, lifted and run on the IR
interpreter. It is the oracle in the two places a Rust port is checked
against the original: differential tests, and runtime shadow checks.

## Where it sits

Domain `runtime`; it depends on `analyze`, `ir`, `lift`, `runtime` and
`xbe`. `engine` uses it for differential tests, and `recomp`, `agent` and
`sim-bridge` use it for shadow execution.

## Layout

- `src/image.rs`: `Image` loads the user's XBE once per test binary (pinned
  by hash, skipped without it).
- `src/code.rs`: `Original`, `Code`, `Assembled`: lift a function and step it.
- `src/cpu.rs`: the interpreter's CPU state.
- `src/shadow/`: the runtime shadow-check driver.

## Tests

`cargo nextest run -p lifted`. Tests that need the retail binary read
`H2_XBE` (your `default.xbe`) and skip without it.

## Docs

- [docs/engine/shadow-exec.md](../../../docs/engine/shadow-exec.md)
