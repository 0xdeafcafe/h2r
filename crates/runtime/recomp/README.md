# recomp

Build and load. The XBE is lifted, emitted and compiled into one host
module; this crate holds the build stamp, loads the module with the boot
bridge, and runs the interpreter for what the module leaves out (x87 and
SSE paths, fallbacks). `boot` re-exports it, so `boot::build`,
`boot::tier` and `boot::load_module` resolve here.

## Where it sits

Domain `runtime`; it depends on `analyze`, `lift`, `ir`, `emit`, `x87`,
`lifted`, `runtime`, `xbe` and `xiso`, and on `clif` behind a feature.
`boot` and `game` use it.

## Layout

- `src/tier.rs`: the tiers and the clang build.
- `src/shim.rs`: the boot bridge the module is loaded with.
- `src/fallback.rs`, `src/x87.rs`, `src/sse.rs`: the interpreter for what
  the module leaves out.
- `src/host_coverage.rs`, `src/host_report.rs`: what the module covers.

## Features

- `desktop-runtime`: the Cranelift fast tier and the clang build
  (`clif`). `tests/fast_tier.rs` requires it.

## Tests

`cargo nextest run -p recomp`. The fast tier test needs
`--features desktop-runtime`. Tests that build need `H2_OPT` to match the
image; the build reads `H2_BOOT_CACHE`, `H2_TIER` and `H2_OPT`
([docs/runtime/switches.md](../../../docs/runtime/switches.md)).

## Docs

- [docs/runtime/BOOT.md](../../../docs/runtime/BOOT.md)
- [docs/lifter/cranelift-tier.md](../../../docs/lifter/cranelift-tier.md)
- [docs/lifter/FLOAT.md](../../../docs/lifter/FLOAT.md)
