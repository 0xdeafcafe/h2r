# x87

Exact software x87: the 80-bit extended arithmetic that the recompiler treats as the authority. It is integer-only, so arm64 and x86-64 builds produce identical bits.

## Where it sits

`recompiler` domain. No workspace dependencies. Used by `ir`, `emit`, `recomp`, `runtime` and `engine`.

## Layout

- `src/arith.rs`, `src/round.rs`, `src/convert.rs`: add, sub, mul, div, sqrt, rounding and conversions.
- `src/trans.rs`, `src/trans_fast.rs`, `src/wide.rs`: transcendentals and the guarded 128-bit fast path.
- `src/abi.rs`: the `#[repr(C)]` state and the `extern "C"` `h2x87_v1_*` surface.
- `src/consts.rs`: constants and tables.

## Features

- `abi` (default): the C ABI structs and `h2x87_v1_*` functions.
- `fast-trans` (default): guarded 128-bit evaluation for the gameplay transcendentals, bit-identical to the 512-bit reference.

## Tests

`cargo nextest run -p x87`. No disc needed.

## Docs

- [lifter/FLOAT-ABI.md](../../../docs/lifter/FLOAT-ABI.md)
- [lifter/x87-transcendentals.md](../../../docs/lifter/x87-transcendentals.md)
- [lifter/x87-native.md](../../../docs/lifter/x87-native.md)
- [lifter/FLOAT.md](../../../docs/lifter/FLOAT.md)
