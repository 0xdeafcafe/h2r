# Guarded transcendentals: measurements

Evidence for [x87-transcendentals.md](../x87-transcendentals.md).

## 2026-11-17

Measured on the shared Apple Silicon host at the commit that last changed the design page (d0ef387e7), release build. The benchmark command is in the entry. These measure helpers, not a gameplay frame saving.

A paired release benchmark uses 31 batches of the same 256 synthetic
inputs, alternating the first implementation each batch after five
warmup pairs. FYL2X/F2XM1 inputs have multiplier exponents [-3,3] and
fractional argument exponents [-3,-1]. Trig angles have E in [-4,3]
(|a| < 16), and atan2 operands have E in [-3,3]. Times include guards
and fallback, in ns per operation:

| Function | Reference median (MAD) | Guarded median (MAD) | Ratio |
| --- | ---: | ---: | ---: |
| FYL2X | 22,014 (68) | 1,913 (12) | 11.5x |
| F2XM1 | 15,305 (23) | 1,370 (6) | 11.2x |
| FSIN | 16,571 (22) | 815 (2) | 20.3x |
| FCOS | 16,553 (22) | 817 (2) | 20.3x |
| FPTAN | 20,310 (16) | 1,964 (12) | 10.3x |
| FSINCOS | 16,963 (228) | 1,648 (25) | 10.3x |
| FPATAN | 92,960 (734) | 1,098 (25) | 84.6x |

These measure helpers on the shared Apple Silicon host. They do not
measure a gameplay frame saving or establish the distribution of operands
in a level. The requested validation is entirely synthetic; no game build
or run supplies validation data.

Run the benchmark with
`cargo test -p x87 --release --lib benchmark_hot -- --ignored --nocapture --test-threads=1`.
The sweeps print their acceptance counts with `-- --nocapture ACCEPT`.
Unoptimised test builds run an eighth of each new sweep. The rates above
are from release.

Final checks: the `x87` library tests pass with the
feature on and off, plus the ABI, spec and oracle integration tests.
Strict clippy for `x87` passes with the feature on and off, and so does
formatting of the touched files. The MPFR fixture generator reproduces the committed synthetic
corpus byte-for-byte, and its first 1,024 lines are unchanged.
