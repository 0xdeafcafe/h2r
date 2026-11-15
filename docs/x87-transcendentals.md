# Guarded transcendentals

`x87/fast-trans` adds a 128-bit integer first pass for **FYL2X, F2XM1,
FSIN, FCOS, FSINCOS, FPTAN and FPATAN**. It is a default feature of the
`x87` crate (build with `--no-default-features --features abi` to drop
it) and independent of `H2_X87_NATIVE`.
FYL2XP1 uses the original helper. An accepted result has
exactly the same ext80 bytes and flags as the 512-bit helper; ambiguous
rounding and operands outside the guards use that helper.

The gameplay profile in `frame-budget` identifies FYL2X and F2XM1 as
14.3% and 3.7% of all samples. A later main-menu profile put 12.8% of the
game thread in the remaining trig helpers (FCOS 258 samples, FSIN 244,
FPATAN 200, FPTAN 15); see [Trig and atan2](#trig-and-atan2). It does not contain an operand census.
The domains below cover the full binary32/binary64 value ranges and the
fractional reduction used by pow/exp, rather than assuming a narrow
observed distribution. Validation uses synthetic sweeps and an independent
MPFR oracle. No game image build or frame capture is needed for this proof.

## Argument ranges (FYL2X, F2XM1)

E is the exponent of an operand's leading bit. All admitted operands are
canonical normal ext80, with their **entire 64-bit significands** retained.
A binary32/binary64 subnormal widened to ext80 is normal and can enter;
an actual ext80 denormal or pseudo-denormal cannot.

| Function | Guarded domain |
| --- | --- |
| FYL2X `y*log2(x)` | Any positive normal ext80 x; normal y with E in [-1074,1023] |
| F2XM1 `2^a-1` | Normal a with E in [-1138,-1], hence -1 < a < 1 |

The y interval includes every finite, nonzero binary32/binary64 value,
including subnormals widened exactly. x covers every normal ext80 exponent,
not just binary64. FYL2X reduces x exactly to m near [1/sqrt(2),sqrt(2)],
so its series always sees `s=(m-1)/(m+1)` with `abs(s)<0.18`, regardless
of x's exponent.

For these x values, the smallest nonzero magnitude of log2(x) exceeds
2^-64 (the closest representable x below one is 1-2^-64); its magnitude
is below 2^14. Thus a nonzero FYL2X result has leading exponent in
[-1138,1037], well inside the normal ext80 range. In a pow/exp reduction,
subtracting an integer from a larger 64-bit or PC=53 value cannot produce
a new fraction below the input's grid spacing. For magnitude at least
1/2 that spacing is at least 2^-64; for smaller values the fraction is
the value itself. The F2XM1 guard therefore includes these fractional
outputs as well as ordinary binary32/binary64 fractions. Its smallest
output has exponent at least -1139, also normal ext80.

Zeros, NaNs, infinities, unsupported encodings, actual ext80 denormals,
out-of-domain y/a, and power-of-two x retain the reference. Exact
F2XM1 cases at zero and +/-1 also retain the reference. No game-wide
claim is made about the frequency of future ext80-only operands;
falling outside a guard always preserves correctness.

## Error bound against the exact helper

Both passes use the existing `Wf<N>` implementation and constant tables.
Let p be the working significand width and `u=2^(1-p)`. A multiplication,
division or constant truncation has relative error below u.
Input loading and power-of-two scaling are exact. Addition's extra guard
limb bounds alignment loss by `2^(max(Ea,Eb)-p-63)`, plus final truncation
below `2^(Eresult-p+1)`. Cancellation must be accounted for explicitly.

The following accounting applies to p=128 and p=512:

* **FYL2X:** m is an exact 64-bit dyadic, so both m-1 and m+1 fit exactly
  in p bits, including x's nearest neighbours of one. Computing s adds
  one division error. The atanh series has terms of one sign and ratio
  below 0.033. It stops in fewer than 128 terms even at p=512: the
  relative term bound is below `24^-128 < 2^-576`, below its 2^-520
  stopping threshold. Propagating square/product/division errors through
  at most 128 recurrences gives less than `2^11*u` relative error per
  term (including higher-order terms). Sum conditioning is one. Series
  additions, division by ln(2), integer-exponent addition and multiplication
  by y leave the total below `2^20*u`. The integer addition is either
  with zero or has result magnitude above 0.49 and cancellation factor
  below three; there is no unaccounted tiny difference there.
* **F2XM1:** z=a*ln(2) has abs(z)<0.7 and relative input error below 3u.
  Evaluate expm1 directly. Its absolute-term sum divided by the result
  magnitude is below three, and its sensitivity to z is below two.
  There are fewer than 128 terms at either precision: `128! > 2^642`
  suffices to bound the relative term below 2^-641. Recurrence errors
  are below `2^11*u` per term, and accumulated additions, argument error
  and the tail keep the total below `2^20*u`. No subtraction from a
  rounded exp(z) occurs, even for tiny a.

In each series the omitted tail is less than twice the first omitted
term, itself below 2^(-p-8) of the sum. All intermediate exponents use
software integers, so host underflow/FTZ and host rounding mode do not
enter this accounting. The guards keep the final results normal ext80.
Conservatively relax `2^20*u` to `2^31*u` to obtain these budgets:

| Function | 128-bit approximation vs mathematical result | 512-bit approximation vs mathematical result |
| --- | --- | --- |
| FYL2X | Relative error < 2^-96 | Relative error < 2^-480 |
| F2XM1 | Relative error < 2^-96 | Relative error < 2^-480 |

The triangle inequality therefore bounds the first-pass value's relative
difference from the **unrounded reference helper** by less than 2^-95.
This statement does not assume that the reference is globally correctly
rounded. Its existing `TRANSCENDENTAL_IS_CORRECTLY_ROUNDED=false` remains.

## Guard and precision control

For first-pass value v with leading exponent T, form
`v-2^(T-80)` and `v+2^(T-80)`. This radius exceeds the error budget against
the reference by over 2^13, even after endpoint truncation (below
2^(T-125)). Thus the interval contains the unrounded reference result.
Round both endpoints with the existing ext80 packer. Accept only if
**both full values and flags agree, with PE set**.

Equal endpoint values put the interval inside one nearest-rounding cell.
Equal PE/C1 additionally put it on one side of the representable value;
this rejects an interval that would have equal values but inconsistent
rounding direction or an exact-result exception. The reference's omitted
tail cannot alter rounding inside that open cell. Accepted results
therefore differ from the reference by **zero bits**, including PE/C1.
No exception masks, status masks or tolerance are relaxed.

Transcendental helpers round to 64 bits independently of PC; changing the
helper output to 53 bits merely because PC=53 would change the contract.
This implementation retains the 64-bit output. Consequently rounding that
same ext80 result to the game's PC=53 gives exactly the reference's PC=53
result. The same implication holds for PC=24 and PC=64. The extended
exponent range is preserved even when the result exceeds binary64's
exponent range. Directed guest RC retains the original unmodelled response.

## Oracle and sweeps

`scripts/x87-trans-oracle.c` uses independent MPFR 4.2+ directed 1024-bit
bounds. Both bounds must agree on the 64-bit result and rounding flags;
otherwise fixture generation fails. It then independently rounds that
ext80 result to PC=53 and PC=24, including halfway cases made from exact
log identities. Tests check those results and their rounding flags.
The 2,048 public fixtures are entirely synthetic and require no installed
MPFR to execute: 2,048 ext80 checks plus 4,096 PC-rounding checks. The
first 1,024 cover FYL2X/F2XM1; the rest add 256 each of sin, cos and tan
(MPFR reduces by the exact pi, as the reference does) and atan2. The trig
lines include binary64/binary32 roundings of k\*pi/2 and k\*pi/4 (|k| up
to 2000), reductions up to 2^63, and the tiny guard down to 2^-480;
below that, 1024-bit intervals cannot resolve `x^3/6` and the unit tests
check against the reference instead. The FSIN/FCOS lines also check
FSINCOS's two values.

Unit tests additionally cover:

* Every normal ext80 x exponent for FYL2X, with multiplier exponent edges.
* Every admitted multiplier exponent, both signs and neighbours of x=1.
* Every admitted F2XM1 exponent, both signs and four significand patterns.
* 4,000 random cases per function against both 512 and 768 bits.
* 2,000 unrounded approximations per function against the error budget.
* Rounding-cell boundaries, exact cases, rejected encodings and range exits.
* Full ABI state and return status across all TOPs, PC/RC modes and
  masked/unmasked exceptions, including empty-register bytes.

The emitter's synthetic FYL2X/F2XM1/store sequence also compares generated
C with the interpreter, including memory faults. Only `x87` and `emit`
are checked. A game run cannot strengthen these per-operation equalities,
and no frame-480 comparison is part of this validation.

```sh
cargo test --release -p x87
cargo test --release -p x87 --features fast-trans
cargo test --release -p emit --test differential --features x87/fast-trans
H2_X87_NATIVE=1 cargo test --release -p emit --test differential --features x87/fast-trans
cargo clippy -p x87 -p emit --all-targets --features x87/fast-trans -- -D warnings
```

Regenerate fixtures, if MPFR is installed:

```sh
cc -Wall -Wextra -Werror scripts/x87-trans-oracle.c $(pkg-config --cflags --libs mpfr gmp) -o "$TMPDIR/x87-trans-oracle"
"$TMPDIR/x87-trans-oracle" > crates/x87/tests/data/trans_oracle.txt
```

## Trig and atan2

### Domains

| Function | Guarded domain |
| --- | --- |
| FSIN, FCOS, FSINCOS, FPTAN | Normal a with E in [-16000,62], subject to the reduction guard |
| FPATAN `atan2(y,x)` | Normal y and x, each with E in [-4096,4096] |

Zeros, NaNs, infinities, unsupported encodings and ext80 denormals keep
the reference, as does |a| >= 2^63 (C2 set, operand unchanged). The trig
domain is every finite exponent below the C2 limit except the bottom 382
binades, where a result "just below x" could be tiny. Every binary32 and
binary64 angle with |a| < 2^63, including subnormals widened exactly, is
inside it. The FPATAN domain includes every binary32/binary64 pair; its
quotient exponent stays above -8194, so a nonzero result is normal.

### Reduction (the reference's semantics)

The reference reduces by the **exact** pi: `|a| = r + k*pi/2` with
`|r| <= pi/4`, computed from a 512-bit `2/pi`. Its own accuracy note
(trans.rs) bounds the unrounded result within about 2^-280 relative of
the mathematical sin/cos/tan of the ext80 operand. Real x87 hardware uses
a 66-bit pi, so for large |a| neither path reproduces hardware; the fast
path reproduces the reference.

The fast pass forms `y = |a|*2/pi` with a 192-bit `2/pi` (`Wf<3>`), in a
192-bit product. Both truncations give `|y|` an absolute error below
`2^(Ey-189)`, with Ey the leading exponent of y. Splitting y into the integer k and the fraction f is
exact, as is moving f from [1/2,1) to [-1/2,0) with k+1. The guard
requires f nonzero with `Ef >= Ey - 80`, so f has relative error below
2^-109. f is then truncated to 128 bits (2^-127) and multiplied by a
128-bit pi/2 (two more 2^-127 terms): r's relative error is below
**2^-108**. For |a| < 2^13, `Ey <= 12` and f is rejected only when
|f| < 2^-68, that is, a within about 2^-67 of a multiple of pi/2. The closest binary64 multiple of pi/2 there
is far outside that, so all game angles pass; even the ext80 values nearest
k\*pi/4 pass (below). At |a| near 2^62 the
guard rejects f below 2^-18, a 2^-17 fraction of operands. When f is
near 1/2, the fast pass and reference may choose k and k+1 differently.
Both are valid reductions of the same angle, and the series below
tolerate `|r|` slightly above pi/4.

### Error budget

With p=128 and `u=2^-127`:

* **sin r, cos r:** for `|r| <= pi/4`, the term ratios are below 0.11
  (sin) and 0.31 (cos). The series stop after fewer than 20 terms at their
  2^-136 threshold, and the tail is below twice the first omitted term.
  Each term carries less than `2^7*u` of recurrence error. The alternating
  sums' conditioning `sinh r/sin r` and `cosh r/cos r` is below 1.9.
  Sensitivities `r*cot r <= 1` and `r*tan r < 0.8` turn r's 2^-108 into
  below 2^-108 relative. Total: **below 2^-106**. Quadrant swaps and sign
  changes are exact.
* **tan:** sin/cos or -cos/sin adds one division: **below 2^-105**.
* **atan2:** let `a <= b` be the operand magnitudes (swapped exactly as the
  reference does when |y| > |x|). An integer quotient of the significands
  picks `j = round(64*a/b)`, so `c=j/64` has `|a/b - c| <= 1/128`. Then
  `atan(a/b) = atan(c) + atan(u)` with `u = (a - c*b)/(b + c*a)`.
  `c*b` and `c*a` are exact 71-bit products. Numerator and denominator are
  exact too: the operands lie within 2^9 of each other, so each sum spans
  under 128 bits. That leaves one division (u) and `|u| <= 1/128`. The
  series ratio is below 2^-14, so it stops within 10 terms with
  error below `2^7*u`. `atan(c)` is a 65-entry table of floor(atan(j/64)*2^128)
  (absolute error below 2^-128). It is checked against the 768-bit helper
  and, independently, a Python integer evaluation. For j >= 1 the sum
  exceeds atan(1/128) > 2^-7.01, so its relative error is below 2^-112.
  For j = 0, `u = a/b` and the error is below 2^-119. `pi/2 - v` (v <= pi/4)
  and `pi - v` (v <= pi/2) each at most double the relative error and add
  one pi truncation. Total: **below 2^-105**.

Against the reference's own 2^-280, the fast value is within 2^-104 of
the unrounded reference. The certify radius 2^(T-80) used by FYL2X/F2XM1
exceeds that by 2^24, so the acceptance rule and its proof carry over
unchanged: equal endpoint values and equal PE/C1, with PE set. The
2,000-case unrounded test checks 2^-100 against 768 bits.

### Exact-neighbour rules

The certify interval necessarily rejects a result within 2^-81 of a
representable value. Three such cases are common and have a known
direction, so they are rounded by the same packer (`Wf::to_f80` with a
tail) as the reference's own series tails:

* `2^-16000 <= |a| < 2^-32`: `sin a` lies strictly below a in magnitude,
  by less than `|a|^3/6`, and `tan a` strictly above, by less than
  `|a|^3/3 * 1.01`. Both gaps are below 2^-65 relative, inside a half-ulp
  even at a power of two. `cos a` lies within 2^-65 below one (FCOS uses
  this for every normal |a| < 2^-32, as its result is one). They are a with
  tail -1, a with tail +1, and one with tail -1.
* After reduction, `|r| < 2^-33`, so the true |r| is below 2^-32 and
  nonzero (no nonzero ext80 value is a multiple of pi/2). Then cos r lies
  in (1 - 2^-65, 1), and the cosine branch (FSIN at odd k, FCOS at even
  k) is ±1 with tail -1. This covers `sin(float(pi/2))`,
  `cos(double(pi))` and the like, which are the values a game uses.

FSINCOS accepts only when both results are accepted. It merges flags with
the reference's own `sincos_flags` (both results' exceptions, the
cosine's C1). FPATAN has no such rule: an exactly representable ratio
below 2^-33 (for example y = 2^-4096, x = 1) falls back.

### Acceptance (synthetic, release sweeps)

| Sweep | sin | cos | tan | sincos |
| --- | ---: | ---: | ---: | ---: |
| 6,000 random a, E in [-32,12] | 5,999 | 6,000 | 6,000 | 5,999 |
| 6,000 random a, E in [-32,62] | 6,000 | 5,999 | 6,000 | 5,999 |
| 6,000 random a, E in [-16382,62] | 5,850 | 6,000 | 5,850 | 5,850 |
| 41,000 ext80 nearest k\*pi/4 ±2 ulps, k <= 4096 and four huge k | 40,998 | 41,000 | 41,000 | 40,998 |
| 32,008 binary64/binary32 k\*pi/2 \* {1, 1/2, 1/4, 1/3}, \|k\| <= 2000 | 32,000 | 32,000 | 32,000 | 32,000 |

The 150 misses in the third row are the exponents below -16000. The eight
in the last row are the zero operands, which the reference already
handles as specials.

| FPATAN sweep (random signs, all quadrants) | Accepted |
| --- | ---: |
| 6,000 pairs, E in [-20,20] | 5,998 |
| 3,000 pairs, E in [-1074,1023] | 3,000 |
| 3,000 pairs, E in [-4200,4200] | 2,844 (the misses are outside the guard) |

Every accepted result in every sweep equals both the 512- and 768-bit
helpers in bytes and flags. Rejected results and the public dispatch
equal the 512-bit helper. Further tests cover every exponent from
-16382 to 62 (stride 23 in the middle) with four significands, C2 at
2^63 and above, NaN/inf/zero/denormal/pseudo-denormal/unnormal operands,
the atan2 sign/quadrant table (all four sign pairs, |y| = |x|,
|y| on both sides of |x|), each atan table cell's neighbourhood, and the
FPATAN exponent guard edges. The ABI test runs FSIN/FCOS/FPTAN/FSINCOS/
FPATAN through the full state comparison across all TOPs and control
words.

## Focused measurements

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

Final checks: 22 passing library tests (4 ignored) in `x87` with the
feature on and off, plus the ABI, spec and oracle integration tests.
Strict clippy for `x87` passes with the feature on and off, and so does
formatting of the touched files. The MPFR fixture generator reproduces the committed synthetic
corpus byte-for-byte, and its first 1,024 lines are unchanged.
