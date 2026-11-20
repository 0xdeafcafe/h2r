# simd

Bit-exact, MXCSR-exact reference semantics for the Pentium III's SSE1 floating point, written as integer soft-float. `c/h2simd.h` is the same code in C for generated code, and the tests check the two against each other.

## Where it sits

`recompiler` domain. No workspace dependencies. Used by `ir`, `sim-core` and `engine`; `emit` dev-depends on it.

## Layout

- `src/lib.rs`: packed and scalar operations over `u128` registers and `u32` scalars.
- `c/h2simd.h`: the C twin for generated code.
- `tests/differential.rs`: Rust against C.
- `tests/hardware.rs`: both against SSE hardware results.

## Tests

`cargo nextest run -p simd`. One test is `#[ignore]`d (run it with `-- --ignored`). No disc needed.

## Docs

- [lifter/simd.md](../../../docs/lifter/simd.md)
- [lifter/sse-oracle.md](../../../docs/lifter/sse-oracle.md)
- [lifter/FLOAT.md](../../../docs/lifter/FLOAT.md)
