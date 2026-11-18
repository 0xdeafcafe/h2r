# Guarded native SSE arithmetic

`H2_SSE_NATIVE` is on by default; `H2_SSE_NATIVE=0` uses the integer
`h2s_*` authority for every operation. Set it for both image build and run.
Only `0` and `1` are accepted (unset/empty uses the default). The effective
flag is part of clang's object key and boot's dylib stamp, alongside
`H2_X87_NATIVE`. Changing it requires rebuilding the generated image.

The emitter includes `c/h2sse_native.h` next to every generated module.
It accelerates ADD/SUB/MUL/DIV and SQRT in scalar and packed binary32 forms.
It keeps architectural XMM values as raw bits and MXCSR in CpuAbi. Scalar
upper lanes retain their exact bits. Other SSE instructions, including the
RCP/RSQRT approximation tables and conversions, use the existing helpers.

## Eligibility and range proof

The native path requires guest round-to-nearest-even with all six exception
masks set, and host nearest rounding with exceptions masked. It reads FPCR
on arm64 or MXCSR on x86-64; it never changes host controls.
`-fno-fast-math` and `-ffp-contract=off` prevent contraction of separate
guest operations. The header does **not** use `FENV_ACCESS`: LLVM never
inlines a strict-FP function into a non-strict caller, so every packed op
used to cost several real calls (`h2sn_arith`, `h2sn_lane`,
`h2sn_host_mode`, about 6% of the game thread). Instead each operand is
passed through an empty volatile asm placed after all guards, so no FP
operation can be evaluated before the checks pass. Past the guard the host
is round-to-nearest with traps masked: exactly the environment a
non-strict compiler assumes. The guarded path is `always_inline`; the
integer authority sits in a `noinline, cold` function per instruction.
On arm64 the packed forms check and compute all four lanes with NEON
(`h2sn_arith4`), using the same per-lane guards and PE reconstruction.

Each operand must be signed zero or a normal binary32 with unbiased exponent
in [-60,60]. Division additionally requires a nonzero divisor; sqrt requires
a nonnegative operand, with -0 accepted. SQRT's unused destination lanes
are never interpreted as host floats. Failure in any packed lane delegates
the entire instruction to the integer helper before changing guest MXCSR
or destination. Host sticky flags may change; guest flags are independent.

Within this range, nonzero add/sub cancellation is at least 2^-83,
products are in [2^-120,2^122), and nonzero quotients are in
(2^-121,2^121). Sqrt results are normal too. Thus no native result can be
a subnormal, overflow or produce IE/DE/ZE/OE/UE. Host flushing cannot
change these results. Guest FTZ is immaterial on this subset; tiny results,
denormal inputs, infinities, NaNs and directed rounding retain the original
semantics through fallback. PIII rejects DAZ at LDMXCSR; direct helper calls
ignore that bit, as the existing reference does.

## Observable precision flags

The float operation rounds once to binary32. Its sticky PE is reconstructed
without relying on host flags:

- Add/sub uses binary64 FastTwoSum. The exact sum is `s + e`; PE is set
  if `e != 0` or the widened binary32 result differs from `s`. The error
  term matters when operand exponents are far apart.
- A product of two binary32 values is exact in binary64 (at most 48 bits).
  Compare it with the widened result.
- Division is exact iff the rounded quotient times the divisor equals the
  dividend, using that exact binary64 product.
- Sqrt is exact iff the rounded root squared equals its input, again an
  exact binary64 product.

All double intermediates and nonzero residuals remain normal. PE is ORed
into the guest's existing flags even if the guest never reads STMXCSR.
Unmasked guest exceptions use the authority, preserving lane-wide exception
priority, sticky flags, #XM and an unchanged destination. NaN selection,
payload quieting and signed zeros outside the guard use the same authority.

## Per-op timing

`benchmark_sse_ops` (ignored; `tests/c/sse_bench.c`) times each entry
point as generated code uses it: operands loaded from a CPU struct, MXCSR
read-modify-written, result stored, generated-C flags at `-O1`. arm64,
ns per op, minimum of 15 runs of 2M ops. "Strict FP" is the previous
header built from the same file:

| Op | Authority | Strict FP | Inlined | vs strict FP |
| --- | ---: | ---: | ---: | ---: |
| addps | 42.0 | 16.5 | 5.0 | 3.3x |
| subps | 41.8 | 16.4 | 5.2 | 3.2x |
| mulps | 37.4 | 15.9 | 3.7 | 4.2x |
| divps | 38.2 | 16.2 | 3.7 | 4.4x |
| sqrtps | 195.6 | 14.0 | 2.8 | 5.0x |
| addss | 12.7 | 6.2 | 2.5 | 2.5x |
| mulss | 10.8 | 5.6 | 2.1 | 2.6x |
| divss | 11.1 | 5.8 | 2.2 | 2.6x |

```sh
cargo test -p emit --test native_sse benchmark_sse_ops -- --ignored --nocapture
```

## Evidence

`emit/tests/native_sse.rs` builds with UBSan at the production C `-O1` and
compares results, MXCSR and exception delivery against Rust soft float:

- 159,894 systematic edge records, plus 1,000,000 random records. Half the
  random batches force the native range and mode, including both exponent
  endpoints; the rest include arbitrary masks, rounding and raw values.
- The disabled C path over the same edge sweep.
- Directed host rounding, host flushing and host trap controls; controls
  actually installed are preserved. Rosetta forces exception masks on, so
  x86-64 under Rosetta cannot establish unmasked host exception behavior.
- 426,968 arithmetic records from the committed SSE oracle, including its
  full edge matrix, random states, sticky flags and all eight RC/FTZ modes.
- `inlined_fast_paths_match_the_authority` (`tests/c/sse_fast_equiv.c`):
  4,000,000 random records at `-O1` with UBSan and again at `-O2`; with
  `H2SSE_FAST_COUNT=20000000`, 15.1M of 40M records took the fast path.
  Accepted results and MXCSR must equal `h2s_*`, the NEON and scalar
  guards must accept exactly the same records, and every form must take
  the fast path for at least a tenth of its records. Lanes mix the native
  range, its exponent edges (66-68, 186-188), signed zeros, denormals,
  infinities, NaNs, raw bits, cancellation and arbitrary MXCSR.
- `fast_paths_inline_into_generated_handlers` checks that the guarded
  path compiles into a non-strict caller with no calls except the cold
  authority.

A separate opt-in live harness assembles synthetic instructions and checks
10,240 new cases against Rosetta SSE, the Rust authority and the native C:
all ten arithmetic forms, four rounding modes, both FTZ settings, rotating
sticky masks, edge values and independently generated normal operands.
That sweep passed without differences.

The suite runs on arm64 and can cross-compile and execute x86-64 through
Rosetta with `H2SSE_TEST_ARCH=x86_64`. It does not treat Rosetta as physical
PIII evidence; see [sse-oracle.md](sse-oracle.md). Approximation instructions
are outside this change. `emit/tests/differential.rs` additionally exercises
native arithmetic, MXCSR stores, memory-fault exits and preserved state
through lifted synthetic functions and the compiled generated C.

```sh
cargo test -p emit --test native_sse --test differential --test fast_copy --test perf
H2SSE_TEST_ARCH=x86_64 cargo test -p emit --test native_sse
cargo test -p emit --test native_sse live_synthetic_oracle_sweep -- --ignored --nocapture
H2_SSE_NATIVE=0 cargo test -p emit --test differential
cargo test -p boot the_stamp_records_the_effective_fast_path_defaults
```

Use a private seeded target in scratch as required by the lane rules.
No game runs or generated retail modules are needed for exactness.
