# Guarded FYL2X and F2XM1

`x87/fast-trans` opts into a 128-bit integer first pass for **FYL2X and
F2XM1 only**. It is off by default and independent of `H2_X87_NATIVE`.
Other transcendentals use the original helpers. An accepted result has
exactly the same ext80 bytes and flags as the 512-bit helper; ambiguous
rounding and operands outside the guards use that helper.

The gameplay profile in `frame-budget` identifies FYL2X and F2XM1 as
14.3% and 3.7% of all samples. It does not contain an operand census.
The domains below cover the full binary32/binary64 value ranges and the
fractional reduction used by pow/exp, rather than assuming a narrow
observed distribution. Validation uses synthetic sweeps and an independent
MPFR oracle. No game image build or frame capture is needed for this proof.

## Argument ranges

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
The 1,024 public fixtures are entirely synthetic and require no installed
MPFR to execute: 1,024 ext80 checks plus 2,048 PC-rounding checks.

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

## Focused measurements

A paired release benchmark uses 31 batches of the same 256 synthetic
inputs, alternating the first implementation each batch after five
warmup pairs. Inputs have multiplier exponents [-3,3] and fractional
argument exponents [-3,-1]. Times include guards and fallback.

| Function | Reference median (MAD), microseconds | Guarded median (MAD), microseconds | Ratio |
| --- | ---: | ---: | ---: |
| FYL2X | 22.846 (0.060) | 1.969 (0.021) | 11.60x |
| F2XM1 | 16.396 (0.136) | 1.472 (0.012) | 11.14x |

These measure helpers on the shared Apple Silicon host. They do not
measure a gameplay frame saving or establish the distribution of operands
in a level. The requested validation is entirely synthetic; the partial
operand census was stopped when the no-game-run rule arrived and supplies
no validation data.

Final checks: 79 passing tests (4 ignored) in `x87` with the feature off,
79 (4 ignored) with it on, two emitter differential tests with it on,
and two more with both the fast feature and native C arithmetic.
Strict clippy for `x87`/`emit` and formatting for `x87` pass. The MPFR
fixture generator reproduces the committed synthetic corpus byte-for-byte.
