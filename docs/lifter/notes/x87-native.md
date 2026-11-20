# Guarded native x87: measurements

Evidence for [x87-native.md](../x87-native.md).

## 2026-11-15

Measured at base commit 32d8f8fe3 and the rebased tree that made the native path default (2f7490458), on macOS arm64 with generated C at `-O1`. The commands are in each entry. Wall times came from a shared machine and vary materially; the synthetic loop results are not a claimed menu frame-rate gain.

### Benchmark and profile

`tests/c/x87_bench.c` times fld m32 / op / fstp m32 sequences with the
authority stubbed out (reaching it aborts). arm64, `-O1`, ns per
sequence; "strict FP" is the previous header built from the same file:

| Sequence | Strict FP | Inlined |
| --- | ---: | ---: |
| fld + fadd m32 + fstp | 41.5 | 34.2 |
| fld + fmul m32 + fstp | 39.4 | 32.4 |
| fld + fdiv m32 + fstp | 42.2 | 35.6 |
| fld + fsqrt + fstp | 44.3 | 36.6 |
| fld + fstp | 21.3 | 18.7 |

Most of what remains is the loop-carried read-modify-write of status,
tag and register bytes in `X87Abi`, which only static stack allocation
would remove. `mrs fpcr` itself costs about one cycle.

```sh
clang -std=c11 -O1 -fno-fast-math -ffp-contract=off -DH2_X87_NATIVE=1 -Icrates/emit/c -Icrates/x87/include -Icrates/runtime/include crates/recompiler/emit/tests/c/x87_bench.c -o "$TMPDIR/x87-bench" && "$TMPDIR/x87-bench"
```

`hot-loop/x87` is a synthetic workload assembled at runtime, never copied
from the game. Each trip loads a binary32, multiplies, adds, square-roots,
divides and stores a binary32 through the generated C and real runtime.
The perf bridge installs the same reference x87 exports used by emitter
tests. Each result records wall ns/trip and, on Unix, mean thread CPU
`cpu_ns_per_trip` in the JSON extras. CPU time excludes descheduling and
background worker threads; it is not whole-process or GPU time.

Use separate work directories and run comparisons serially:

```sh
H2_X87_NATIVE=0 cargo run --release -p cli --features perf -- bench --only hot-loop/x87 --label helpers --samples 31 --warmup 5 --out "$TMPDIR/x87-bench" --work "$TMPDIR/x87-base"
cargo run --release -p cli --features perf -- bench --only hot-loop/x87 --label native --samples 31 --warmup 5 --out "$TMPDIR/x87-bench" --work "$TMPDIR/x87-fast"
cargo run --release -p cli --features perf -- bench compare "$TMPDIR/x87-bench"
cargo run --release -p cli --features perf -- profile hot-loop/x87 --seconds 5 --cpu --by-category --out "$TMPDIR/x87-profile"
```

On macOS arm64 at base `32d8f8f`, the baseline's 5-second profile charged
55.5% of self samples to software sqrt and 7.7% to software division.
Neither appeared in the native profile; checked memory access and locking
became the main costs. This workload deliberately stresses sqrt; it does
not measure the game's instruction mix or establish the reported 5% x87
share of a real menu frame. A whole-frame gain cannot be inferred by
applying the synthetic ratio to that share.

The final 31-sample comparison (100,000 trips per sample, 5 warmups,
release Rust, generated C at the default `-O1`) recorded:

| Measurement | Helpers | Native flag | Change |
| --- | ---: | ---: | ---: |
| Wall median, ns/trip | 934.01 | 167.58 | -82.1% |
| Wall MAD, ns/trip | 331.94 | 31.12 | |
| Mean thread CPU, ns/trip | 576.18 | 138.68 | -75.9% (4.15x) |
| Wall minimum, ns/trip | 565.56 | 135.58 | |

The machine was shared with other builds; wall-time variation remains
material. These are synthetic loop results, not a claimed menu FPS gain.

The x86-64 header is also cross-compiled with strict warnings. Execution
and timing evidence here is from arm64; x86-64 native performance, including
its library FMA implementation, is unmeasured. The test oracle remains the
pinned Unicorn core, not Xbox/PIII hardware.

### Real menu baseline and remaining cost

A bounded exact-rendering run uses the normal boot driver, fresh data and
`START@f310,DOWN@f408,A@f419`. A private wrapper around `menu_cpu` samples
only presents 480–560 at 200 Hz, finishing symbolisation while the dylib
is still loaded. The shared `32d8f8f` image is APFS-cloned; all 1,001
generated C files were checked against the native image after replacing
only the native helper-name prefix. The private baseline clone's stamp is
updated for that verified semantic equivalence; the shared image and its
stamp are never edited. The flagged image is built normally through the
emit gate. Both runs use the same host binary, exact GPU passes and the
same input script, serially under the game gate.

The baseline captured 4,682 on-CPU samples in its 80-frame window. 551
(11.77%) have an `x87::` frame, of which 510 (10.89% of all samples) have
an `x87::trans::` frame. Log/exp and atan-series evaluation are larger
than sin/cos; basic x87 is at most the other 0.88% in this capture.
These are inclusive stack counts, not sums of overlapping parent frames.
The stated approximate 5% frame share is not reproduced by this run.
The exact-rendering baseline measured 276.70 process CPU ms/frame and
1.816 wall fps over presents 480–560 on the shared host. Sampling and all
process threads contribute to CPU time; CPU scheduling and GPU contention
also affect FPS. This is not a 30-fps result.

Native binary64 cannot reproduce the helpers' full 64-bit transcendental
significands, and Unicorn's double/libm transcendentals are not an
independent accuracy authority. Those operations remain unchanged. Static
stack allocation alone would not remove that dominant cost either. The
native flag therefore addresses the proven basic-arithmetic subset;
further menu gains require work on exact extended-precision transcendental
evaluation or separately justified whole-region equivalence.

### Final menu comparison

Both exact-rendering runs reached present 560. The main-menu PNG at
present 480 is byte-for-byte identical at 640×480: **zero changed pixels**.
The initial frame also matches. Images and raw profiles are kept outside
the repository; none is committed.

| Presents 480–560 | Helpers/shared baseline | Native flag |
| --- | ---: | ---: |
| Process CPU ms/frame | 276.70 | 218.77 |
| Wall fps | 1.816 | 3.659 |
| On-CPU samples | 4,682 | 2,268 |
| Samples with an x87 frame | 11.77% | 11.99% |

The native run happened after the full-image compile ended; the baseline
ran concurrently with that compile. Thermal state, CPU placement and GPU
contention were not controlled. The large whole-frame difference cannot
be credited to this lowering, especially since most measured x87 time
remains in unchanged transcendental helpers. The controlled acceptance
result is matching pixels and full architectural-state comparisons; the
synthetic CPU benchmark establishes the arithmetic-path improvement.
It does not by itself establish 30 fps.

Final checks on the rebased tree: 153 passing tests (5 ignored) for
`emit`, `perf`, `x87` with the oracle feature, then 83 passing tests
(2 ignored) for `emit`, `perf` with the native flag. The native C tests
exercise over 1.2 million synthetic operations; the independent oracle
adds 600 exact-result cases. Strict clippy passes. The five focused tests
also pass with generated C at `-O2`; the x86-64 native header compiles with
`-Wall -Wextra -Werror`, without claiming x86-64 execution evidence.
