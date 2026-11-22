# Lifter experiments that lost

Evidence for [FLOAT.md](../FLOAT.md), [x87-transcendentals.md](../x87-transcendentals.md) and [BUILD-CACHE.md](../BUILD-CACHE.md). One row per experiment, so it is not built twice.

| What it was | Result | Removed by |
|---|---|---|
| Libm `f64` evaluation of FSIN, FCOS, FSINCOS, FYL2X and F2XM1 (`set_f64_transcendentals`, `trans_f64`, boot switch `H2_X87_FAST_TRANSCENDENTALS`), measured for speed only | Errors up to 2^24.8 ulps for small angles and 2^41 for large ones against the correctly rounded series; never turned on | 361a99744 (switch), 2658859b9 (module) |
| Keying clang object-cache entries on its IR instead of on source plus headers ([notes/build-perf.md](build-perf.md), "Header changes") | Any line shift in `h2abi.h` or `h2gen.h` changes every shard's IR: 0 of 10 sample IRs unchanged for a prototype or comment edit, 4 of 10 after reverting the native-x87 header changes. Saves under half the shards for the rarest kind of change, at the price of a front-end run per miss. Never built | not committed |
| Dropping unread floating-point exception state per image (`analyze::fpfacts` census, `_nf` native SSE/x87 forms, module features 256/512, switch `H2_FP_DEAD_FLAGS`; branch `worktree-agent-a15fc8473033ce655`, commit 8e7ea35b9) | Tests pass, but deciding for the whole image keeps both groups on retail: `h2 fpstats` on 2026-11-22 found 12 live MXCSR readers and 363 x87 PE/C1 observers (300 reads, 9 loads that may unmask, 54 unclaimed bytes that decode as readers), so the C would not change while the three header edits recompile every shard. Next attempt: decide per function or per call-graph region instead of per image | not merged; branch parked |
