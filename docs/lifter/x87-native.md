# Guarded native x87 lowering

Status: Built. On by default.
Code: `crates/recompiler/emit/c/h2x87_native.h`, `crates/recompiler/emit/src/x87.rs`. Tests: `crates/recompiler/emit/tests/native_x87.rs`.
Switch: `H2_X87_NATIVE` (on), listed in [switches.md](../runtime/switches.md). Measurements: [notes/x87-native.md](notes/x87-native.md).

The generated C runs guarded x87 arithmetic in native binary64. It is
**on by default**; `H2_X87_NATIVE=0` keeps every operation on the
`h2x87_v1_*` helpers. Set the variable for both the image build and its
run: the C flags participate in the object-cache keys and the boot dylib
stamp. Values other than `0` and `1` are rejected.

The separate `x87/fast-trans` Cargo feature (on by default) accelerates guarded integer
log/exp evaluation; see [x87-transcendentals.md](x87-transcendentals.md).

```sh
H2_X87_NATIVE=0 cargo run --release -p cli -- build "$H2_XBE" --out "$TMPDIR/helper-game" --jobs 4
```

Never put the resulting C, dylib, game data or screenshots in Git.

## How it works

The lifter emits `Intrinsic::X87` operations. `emit/src/x87.rs` emits one
`h2x87_v1_*` Rust helper call per operation (plus a separate store commit)
unless the native path applies. `crates/recompiler/x87` uses integer ext80 software
arithmetic, not host `long double`. All eight physical registers remain ten-byte ext80
values in `X87Abi`; TOP is status bits 11–13 and tags are physical.
The emitter caches environment pointers in handler locals.

The native path uses inline C functions in `emit/c/h2x87_native.h` for
loads, register copies, store conversion/commit and guarded arithmetic.
FILD m16/m32 is native too: every such integer is exact in binary64, so
the load builds that double's bits directly (no host FP) and pushes it
like FLD m64; stack overflow and a pending #MF use the helper. Old
Mombasa's hot SSE routine (0x002dc220) alone has 18 `fild m32` sites.
`integer_loads_match_the_authority` checks every bit position, the
extremes and 1,000,000 random values over random TOP/tags/status.
Eligible ADD/SUB/SUBR/MUL/DIV/DIVR and SQRT use transient C `double`
locals; every rejected operation calls the original helper. The helper
ABI, dispatch, interpreter and authoritative soft-float implementation
are the same with the native path on or off.

This implementation retains dynamic TOP and architectural ext80 state,
including bytes in empty registers after pops. It does not persist an
f64 stack across instructions, calls or CFG joins. Unknown entry depth,
FRSTOR, MMX aliasing and values below callee arguments therefore need no
special inter-block convention. Memory-fault exits observe the correct
preceding instruction state; a store converts without mutation, writes
guest memory, and only then commits status/pop. These boundaries would
also need preserving in a later static stack allocation pass.

Software sqrt and division, not stack indexing, were the measured cost
([notes/x87-native.md](notes/x87-native.md)). A static stack allocation pass
does not exist.

## Precision and range proof

Arithmetic requires all of the following, checked at the operation:

- Guest PC=53, RC=nearest and all six exception masks set; no pending #MF.
- Host binary64 arithmetic uses nearest rounding and has exceptions
  masked. FPCR is checked on arm64 and MXCSR on x86-64. A directed host
  mode falls back without changing that mode.
- Every nonzero operand is canonical normal ext80, has its low eleven
  significand bits zero, and has unbiased exponent in **[-400,400]**.
  Canonical signed zero is also accepted. Memory sources are checked in
  their original format; denormals fall back, preserving DE and its
  priority. Integer sources are exactly representable int16/int32.

The result of add/sub/mul/div/sqrt therefore has the same rounded
significand as PC=53 ext80. Multiplication/division results remain well
inside binary64's normal exponent range. Cancellation in addition can
produce zero or a result no smaller than 2^-452. Multiplication's exact
nonzero residual is no smaller than 2^-904. Division's fused residual and
sqrt's squared-result residual also remain normal. Thus gradual-underflow
or flush-to-zero behavior cannot erase a residual or change a result.
No overflow, underflow or invalid computation is possible on this path;
division by zero and sqrt of negative values fall back before computing.

The new result is exactly widened back to ext80 using integer bit
manipulation. A result outside the operand eligibility range is retained
exactly, but its next operation falls back. PC=53 alone never establishes
representability: FLD constants, m80 loads, restored state and earlier
PC=64 results can still carry all 64 significand bits. The low-bit check
rejects them rather than rounding them during entry.

### PE and C1 remain observable

Native rounding is not enough: guest sticky precision and the direction
of rounding must match too. The C implementation reconstructs them from
the signed error relative to the rounded result:

- ADD/SUB: magnitude-ordered FastTwoSum gives `exact - rounded`.
- MUL: explicit fused `fma(a, b, -rounded)` gives the residual.
- DIV: fused `fma(-rounded, b, a)` gives the residual times the divisor;
  account for the divisor's sign.
- SQRT: fused `fma(rounded, rounded, -a)` gives the square error; its sign
  identifies whether the positive root rounded upward in magnitude.

PE is set for nonzero error and remains sticky. C1 is set exactly when
rounding increased the magnitude, and cleared otherwise. Explicit FMA
is used only to recover error; the guest multiply/add operations are
never contracted. Generated code retains `-fno-fast-math` and
`-ffp-contract=off`.

The header does not use `FENV_ACCESS`: LLVM never inlines a strict-FP
function into the non-strict generated handlers, so every guarded
operation was a real call (`h2x87n_arith`, `h2x87n_get`,
`h2x87n_host_mode` and others; about 1.3% of the game thread in the
Old Mombasa profile). Instead the operands pass through an empty
volatile asm (`H2X87N_BARRIER`) after every guard, so no FP operation
can be evaluated before the checks pass; the zero and sign guards
compare bits rather than doubles. Past the guards the host rounds to
nearest with traps masked, the environment the non-strict compiler
assumes. The guarded operations are `always_inline`.
`x87_fast_paths_inline_into_generated_handlers` checks the IR, and
`randomized_native_sweep_matches_the_authority` compares 2,000,000 random
operations (all five source kinds, sqrt, binary32/binary64 stores) at
PC=53 and other control words with the authority's full state; about
1.28M have a native-eligible ST(0). `H2X87_SWEEP` raises the count. Existing sticky flags and other condition codes are
preserved. Host sticky FP flags may change; guest FP state never reads
those flags.

Binary32 stores accept only signed zero or results between the smallest
normal and largest finite binary32 values. A binary32 cast performs the
one required rounding, and its exact difference reconstructs PE/C1.
Other cases retain the soft-float conversion. Binary64 stores of eligible
values are exact. Store conversion does not change the architectural
state before the guest write succeeds.

### What plain f64 would give up

| Guest mode/value | Plain binary64 loss | This implementation |
| --- | --- | --- |
| PC=24 | Must round after each arithmetic instruction to 24 bits while retaining the extended exponent range; a final f32 cast also risks double rounding | Arithmetic falls back |
| PC=53 | Same significand precision, but only an 11-bit exponent instead of ext80's 15-bit exponent | Guard operand range; preserve widened results exactly |
| PC=64/reserved PC | Eleven significand bits and extended exponent range | Arithmetic falls back |
| Directed RC/unmasked exceptions | Guest-specific rounding, suppression and deferred exceptions | Arithmetic falls back |
| NaNs, infinities, ext80-only values | Payload selection, unsupported encodings, precision or exponent information | Arithmetic falls back |
| Transcendentals | Different PIII argument reduction and precision | Original helpers |

The static census in [notes/fp-census.md](notes/fp-census.md) reports PC=53 in game
initialisation and PC=64/directed modes in CRT internals; it did not find
a game request for PC=24. That is not a runtime invariant. PC=24 is
explicitly tested and retains the reference path.

## Validation

`emit/tests/native_x87.rs` builds a C wrapper with the native flag forced
on independently of the test process's environment. It compares every
physical register byte, tag, control/status/environment field and helper
return code against `crates/recompiler/x87`, including both destination directions,
pops, every PC/RC, unmasked exceptions, empty stacks, unusual ext80
encodings, exponent boundaries, special values and a directed host mode.
Transport/store checks include the convert/write/commit boundary.

The opt-in `emit/oracle-tests` feature adds an independent Unicorn check
using synthetic instructions assembled with iced-x86. It covers exact
arithmetic results and stack/tag transport in PC=24/53/64 at all eight
TOP values. As [oracle.md](oracle.md) and the existing x87 oracle adapter
explain, Unicorn is not an authority for x87 sticky flags, C1 or general
PC-dependent rounding. Those fields are **not** falsely certified by
that check: full-bit differential comparisons against the integer
implementation cover them. No tolerance or new oracle mask is introduced.
There are no new Unicorn dependencies in ordinary builds.

The generated-C differential suite also contains an explicit PC=53
load/add/sqrt/div/store journey, with random mapped/faulting store
addresses. Run that suite both with the native flag on (the default) and
off.

```sh
cargo test -p emit -p perf -p x87 --features emit/oracle-tests
H2_X87_NATIVE=0 cargo test -p emit -p perf --features emit/oracle-tests
cargo clippy -p emit -p perf --all-targets --features emit/oracle-tests -- -D warnings
```
