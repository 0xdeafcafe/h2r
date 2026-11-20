# sse-oracle

Bit-preserving SSE1 and MMX queries to an isolated x86-64 hardware runner. It supplies the hardware results that `simd` is checked against.

## Where it sits

`recompiler` domain. Depends on `analyze` and `xbe`. Nothing depends on it; `emit` dev-depends on it.

## Layout

- `src/corpus.rs`, `src/fixtures.rs`: edge-pair and random inputs across the MXCSR modes, and fixture files.
- `src/fidelity.rs`: the SDM rules as executable checks.
- `src/bin/sse-oracle.rs`: the runner binary.

## Tests

`cargo nextest run -p sse-oracle`. `tests/hardware.rs` needs an x86-64 host; the corpus comes from the retail executable (`H2_XBE`) when it is rebuilt.

## Docs

- [lifter/sse-oracle.md](../../../docs/lifter/sse-oracle.md)
- [lifter/simd.md](../../../docs/lifter/simd.md)
