# Guarded transcendental evaluation

The `x87/fast-trans` Cargo feature opts into integer-only 256-bit working
precision for FYL2X, F2XM1 and FPATAN. It is off by default. The final
result still has all 64 ext80 significand bits and the extended exponent
range. This feature is independent of `H2_X87_NATIVE`, which controls
generated C basic arithmetic. No host floating-point operation or libm
function computes a transcendental result.

```sh
cargo build --release -p cli --features x87/fast-trans
cargo test --release -p x87 --features fast-trans
cargo test --release -p emit --features x87/fast-trans,oracle-tests
```

The feature affects the Rust helper implementation installed by the host.
The C ABI, generated instruction sequence, stack convention and exception
policy are unchanged. Directed guest RC still stops as unmodelled. All PC
values continue to produce 64-bit transcendental results, as before.

## Selection from the menu profile

A fresh baseline at `9a39ed7` sampled only presents 480–560 at 200 Hz
with exact GPU passes and `START@f310,DOWN@f408,A@f419`. Of 5,518 on-CPU
main-thread samples, 659 contained x87 and 599 contained a transcendental
frame. Each row below counts the outer operation once per stack,
including its callees; nested series are not added again.

| Operation | Inclusive samples | All on-CPU samples |
| --- | ---: | ---: |
| FYL2X | 352 | 6.38% |
| FSIN/FCOS | 137 | 2.48% |
| F2XM1 | 57 | 1.03% |
| FPATAN | 51 | 0.92% |
| FPTAN | 2 | 0.04% |

These are costs, not instruction frequencies. The selected subset covers
460/599 (76.8%) of the measured transcendental samples. This confirms the
same hot subset identified in the earlier lane's recovered profile.
Sin, cos, sincos, tan and FYL2XP1 retain the existing evaluator; no new
argument-reduction bound is claimed for them.

## Domains and per-function error bounds

Let `E` be the exponent of an operand's leading bit. Every operand must
be canonical normal ext80 with **-32 <= E <= 32**. Zeros, denormals,
pseudo-denormals, unsupported encodings, NaNs and infinities use the
reference. No input significand is narrowed: all 64 bits enter exactly.

The following bounds concern the **unrounded 256-bit approximation**
`v` relative to the mathematical nonzero result `f`, before the final
rounding guard. They are deliberately loose analytical budgets, not the
largest errors seen in a random test.

| Function | Additional domain | Bound on `abs(v-f)/abs(f)` |
| --- | --- | --- |
| FYL2X | x > 0; power-of-two x uses reference | < 2^-208 |
| F2XM1 | -1 < x < 1; x nonzero | < 2^-224 |
| FPATAN | Both inputs nonzero; all four quadrants | < 2^-208 |

For F2XM1, the common exponent guard also requires `abs(x) >= 2^-32`.
FYL2X and FPATAN take both x and y through the common guard.

### Error accounting

Write `u = 2^-255`, a bound on relative truncation in a normalised
256-bit multiplication, division or square root. Constants are truncated
from the existing independently checked 1024-bit tables, also within u.
Scaling by powers of two and loading the 64-bit inputs are exact.
Addition has an extra guard limb: its absolute alignment error is below
`2^(max(Ea,Eb)-319)`, plus a final significand truncation below
`2^(Eresult-255)`. Cancellation therefore needs an explicit check; it
cannot be dismissed by applying a relative-u bound to every subtraction.

* **FYL2X:** normalisation puts m in approximately [1/sqrt(2),sqrt(2)]
  (the branch uses a 64-bit approximation of sqrt(2)). Subtracting 1 from
  this 64-bit dyadic m is exact in 256 bits, including the nearest
  neighbours of 1. Thus a small logarithm does not acquire an absolute-u
  error from that subtraction. `s=(m-1)/(m+1)` has `abs(s)<0.18`.
  Its atanh series has one sign and a term ratio below 0.033, with fewer
  than 128 terms before the 2^-264 relative stopping threshold.
  Bounding each recurrence's accumulated error by 512u and all series
  additions/divisions by another 512u per term leaves less than 2^20 u
  relative error after the ln(2) division. The final addition of the
  integer exponent is either with zero or produces magnitude above 0.49:
  there is no cancellation into a tiny logarithm there. Multiplication
  by y adds one truncation. The stated 2^-208 budget permits 2^47 u,
  comfortably covering these terms and alignment error.
* **F2XM1:** `z=x*ln(2)` has `abs(z)<0.7`. The expm1 series takes fewer
  than 128 terms; the sum of absolute terms divided by the magnitude of
  the signed result is below 3. Per-term recurrence and addition errors,
  and the sensitivity to the ln(2) multiplication, total below 2^20 u.
  Its omitted tail is below twice the first omitted term. The 2^-224
  budget permits 2^31 u. Computing expm1 directly avoids subtracting 1
  from a rounded exponential for small x.
* **FPATAN:** form min(abs(x),abs(y))/max(abs(x),abs(y)), in (0,1].
  Each positive half-angle step uses a denominator in [2,1+sqrt(2)]
  and has relative sensitivity at most 1 to its input. At most ten
  steps reach t < 2^-10. Their positive additions, square roots and
  divisions introduce less than 128u relative error before the atan
  series. Its alternating terms have ratio below 2^-20; fewer than
  128 terms and a factor below 2 for cancellation put the series error
  below 2^20 u. Doubling the angle is exact. A quadrant correction
  using pi/2 or pi has a result of magnitude at least pi/4, so its
  cancellation factor is below 4. The 2^-208 budget again permits
  2^47 u, including pi's truncation and alignment error.

The series stopping threshold is eight bits below the working
significand. For atanh and expm1 the remainder is less than twice the
first omitted term; for atan it is bounded by that term. This is far
below all three budgets. The guarded domains keep outputs normal and
far from ext80 underflow/overflow, so exponent exceptions cannot arise.

## Acceptance guard and observable error

For a nonzero approximation with leading exponent T, evaluate endpoints
`v - 2^(T-160)` and `v + 2^(T-160)`. Each radius is more than 2^46 times
the largest budget above; endpoint truncation is below 2^(T-254), so
both endpoints still enclose the true result. Round both with the
existing ext80 packer and accept only if **both the value and all flags
agree, with PE set**.

Checking flags is essential. Equal rounded values alone would allow an
interval to straddle that representable value and give the wrong C1 or
PE. An accepted interval crosses neither a nearest-rounding midpoint nor
the representable result itself. Consequently the accepted result has
the mathematical nearest 64-bit value and the same rounding direction;
its final rounding error is at most half an ext80 ulp. Its difference
from the reference is **zero bits** on the differential and oracle
corpus. No tolerance or status mask is added to any test.

Exact results and intervals near a rounding boundary use the 512-bit
reference. The total interface's existing
`TRANSCENDENTAL_IS_CORRECTLY_ROUNDED=false` remains: no exhaustive
worst-case proof is claimed for that fallback.

## Reproducible validation

The unit suite checks 4,000 cases per selected function against both
512-bit and 768-bit evaluations, plus the remaining atan quadrants. It
also checks 2,000 unrounded approximations per function against 768 bits
with the budgets above. Dedicated guard tests put intervals on exact
values and half-ulp midpoints, on both signs, and exercise rejected
encodings and exponent boundaries. The ABI matrix compares every state
field, empty-register bytes and return status across all eight TOPs,
PC/RC modes, pending exceptions and masked/unmasked responses.

`tests/trans_oracle.rs` reads 384 public **synthetic** fixtures produced
independently with MPFR. The generator computes directed 1024-bit lower
and upper bounds and requires both to agree on the rounded value and
rounding flags. It fails instead of emitting an ambiguous result. Normal
tests need no MPFR installation. To regenerate:

```sh
cc -Wall -Wextra -Werror scripts/x87-trans-oracle.c $(pkg-config --cflags --libs mpfr gmp) -o "$TMPDIR/x87-trans-oracle"
"$TMPDIR/x87-trans-oracle" > crates/x87/tests/data/trans_oracle.txt
```

The generated-C differential suite contains a finite
FYL2X/F2XM1/FPATAN/store sequence with random mapped/faulting addresses.
Run it with the feature off and on. Unicorn remains useful for instruction
transport and exact identities, but its libm transcendentals are not used
as a full-precision accuracy oracle; MPFR supplies that independent check.

```sh
cargo test --release -p x87 -p emit --features emit/oracle-tests
cargo test --release -p x87 -p emit --features x87/fast-trans,emit/oracle-tests
cargo clippy -p x87 -p emit --all-targets --features x87/fast-trans,emit/oracle-tests -- -D warnings
cargo test --release -p x87 benchmark_hot_transcendentals -- --ignored --nocapture --test-threads=1
```

The timing experiment uses 31 batches of 256 deterministic synthetic
inputs per function and reports wall-time median and minimum ns/call,
including the guards and fallback. It is a per-function benchmark,
not a measurement of the game's instruction frequencies or FPS.

## Per-function timing

The paired benchmark on macOS arm64, based on `9a39ed7`, used 31 measured
pairs after five warmup pairs. Each pair uses the same 256 operands and
alternates which implementation runs first. All times include domain
and rounding guards and any rejected-case fallback.

| Function | Reference median (MAD), µs | Guarded median (MAD), µs | Median ratio |
| --- | ---: | ---: | ---: |
| FYL2X | 23.877 (0.084) | 6.136 (0.046) | 3.89x |
| F2XM1 | 17.137 (0.514) | 4.495 (0.069) | 3.81x |
| FPATAN | 302.789 (192.882) | 87.777 (58.238) | 3.45x |

The host was shared with other builds. FPATAN's scheduling noise is
large; its reference/guarded minima were 90.027/28.190 µs. These wall
measurements establish a per-function improvement on this host, not
whole-frame performance or Xbox hardware timing. x86-64 execution and
performance have not been measured.

The complete validation matrix passed: 123 tests (7 ignored) with the
feature off, 123 (7 ignored) with it on, 47 emitter tests (3 ignored)
with both native C arithmetic and fast transcendentals, and 42 tests
(4 ignored) without the ABI feature. Strict clippy and x87 formatting
checks pass. Oracle fixtures reproduce byte-for-byte from the MPFR
generator.

## Frame-480 acceptance and current menu profile

Both hosts run the same normally built image from this tree: the helper
ABI and generated C are identical, while one host enables `fast-trans`.
Their source/compiler stamps match each other and the image. Each run
uses fresh independent data, exact GPU passes (`H2_GFX_MERGE_PASSES=0`),
the input script above, and captures the main menu at present 480.
Both runs reached present 560. The 640x480 PNGs at **frame 480 are
byte-for-byte identical: zero changed pixels**. The initial frames match
too. Screenshots, their hashes, raw stacks, logs and the driver source
remain outside Git.

| Presents 480–560 | Reference | Fast feature |
| --- | ---: | ---: |
| Main-thread on-CPU samples | 5,518 | 5,181 |
| Samples containing any x87 frame | 659 (11.94%) | 333 (6.43%) |
| Samples containing a transcendental | 599 (10.86%) | 288 (5.56%) |
| FYL2X inclusive samples | 352 | 103 |
| F2XM1 inclusive samples | 57 | 19 |
| FPATAN inclusive samples | 51 | 18 |
| Sin/cos inclusive samples | 137 | 145 |
| Process CPU ms/frame, all threads | 309.354 | 293.116 |
| Wall fps | 2.058 | 1.869 |

The selected subset falls from 460 to 140 inclusive samples; the fast
profile contains 256-bit evaluator frames for all three functions.
Sin/cos becomes the largest remaining transcendental cost. Samples are
inclusive stack counts, not invocation counts or sums of overlapping
parent/child frames.

These are one serial pair on a shared machine. Compilation and other GPU
work overlapped the runs, CPU includes all process threads and profiling,
and the fast run's wall FPS was lower despite lower measured process CPU.
No causal whole-frame speedup or 30-fps result is claimed. The acceptance
result is matching pixels and architectural state; the paired synthetic
benchmark measures the per-function improvement.
