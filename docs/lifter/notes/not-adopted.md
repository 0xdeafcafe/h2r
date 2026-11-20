# Lifter experiments that lost

Evidence for [FLOAT.md](../FLOAT.md), [x87-transcendentals.md](../x87-transcendentals.md) and [BUILD-CACHE.md](../BUILD-CACHE.md). One row per experiment, so it is not built twice.

| What it was | Result | Removed by |
|---|---|---|
| Libm `f64` evaluation of FSIN, FCOS, FSINCOS, FYL2X and F2XM1 (`set_f64_transcendentals`, `trans_f64`, boot switch `H2_X87_FAST_TRANSCENDENTALS`), measured for speed only | Errors up to 2^24.8 ulps for small angles and 2^41 for large ones against the correctly rounded series; never turned on | 361a99744 (switch), 2658859b9 (module) |
| Keying clang object-cache entries on its IR instead of on source plus headers ([notes/build-perf.md](build-perf.md), "Header changes") | Any line shift in `h2abi.h` or `h2gen.h` changes every shard's IR: 0 of 10 sample IRs unchanged for a prototype or comment edit, 4 of 10 after reverting the native-x87 header changes. Saves under half the shards for the rarest kind of change, at the price of a front-end run per miss. Never built | not committed |
