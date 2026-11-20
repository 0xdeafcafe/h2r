# SSE1 floating-point reference semantics

Status: Built.
Code: `crates/simd/src/lib.rs` (Rust side), `crates/simd/c/h2simd.h` (C side). Tests: `crates/simd/tests/` (`differential.rs`, `header.rs`, `oracle_corpus.rs`, `hardware.rs`).
Switch: none.

`crates/simd` defines what every SSE1 floating-point instruction the Pentium
III runs does, down to the bit: results, NaN payloads, signed zeros, the
MXCSR sticky flags and whether the instruction raises `#XM`. It has two
sides:

- the Rust side, `crates/simd/src/lib.rs`, which the IR interpreter calls;
- the C side, `crates/simd/c/h2simd.h`, which the generated code calls.

The C side is the Rust side written again line for line. Both are integer
soft-float only. Neither uses the host's floating point or its floating-point
environment, so the results don't depend on the host CPU, the compiler or
fast-math. The C side is header-only C11 (`static inline`) and has no
undefined behaviour. It builds cleanly with
`-std=c11 -Wall -Wextra -Werror -pedantic` (and `-Wconversion`) on arm64 and
x86_64.

## API

Registers are `u128` (lane `i` is bits `32i..32i+32`), scalars `u32` and MMX
registers `u64`. In C these are `h2u128` (`unsigned __int128`), `uint32_t`
and `uint64_t`. C functions take the same arguments with an out-pointer last.
They return 0 on success and 1 for `#XM`, and they write `*out` only on
success.

| Rust | C | Notes |
|---|---|---|
| `addss(mx, a: u128, b: u32) -> Result<u128, Xm>` | `h2s_addss(mx, a, b, out)` | lane 0 = a0 + b; lanes 1-3 from `a` |
| `addps(mx, a: u128, b: u128) -> Result<u128, Xm>` | `h2s_addps(mx, a, b, out)` | per lane |
| `subss/subps`, `mulss/mulps`, `divss/divps`, `minss/minps`, `maxss/maxps` | same | same shapes |
| `sqrtss(mx, a, b: u32)` | `h2s_sqrtss(mx, a, b, out)` | lane 0 = sqrt(b) |
| `sqrtps(mx, b: u128)` | `h2s_sqrtps(mx, b, out)` | no `a` operand; matches the lifter's stub |
| `cmpss(mx, a, b: u32, pred: u8)`, `cmpps(mx, a, b: u128, pred)` | `h2s_cmpss/cmpps(mx, a, b, pred, out)` | all-ones or 0 per lane |
| `comiss(mx, a: u32, b: u32) -> Result<u32, Xm>`, `ucomiss` | `h2s_comiss/ucomiss(mx, a, b, uint32_t *out)` | EFLAGS ZF/PF/CF bits only |
| `cvtsi2ss(mx, a, i: u32) -> Result<u128, Xm>` | `h2s_cvtsi2ss(mx, a, i, out)` | |
| `cvtpi2ps(mx, a, mm: u64) -> Result<u128, Xm>` | `h2s_cvtpi2ps(mx, a, mm, out)` | lanes 2-3 from `a` |
| `cvtss2si(mx, a: u32) -> Result<u32, Xm>`, `cvttss2si` | `h2s_cvtss2si/cvttss2si(mx, a, uint32_t *out)` | |
| `cvtps2pi(mx, a: u64) -> Result<u64, Xm>`, `cvttps2pi` | `h2s_cvtps2pi/cvttps2pi(mx, a, uint64_t *out)` | `a` = lanes 0-1 |
| `rcpss(a, b: u32) -> u128`, `rsqrtss` | `h2s_rcpss/rsqrtss(a, b)` returns `h2u128` | no MXCSR |
| `rcpps(b: u128) -> u128`, `rsqrtps` | `h2s_rcpps/rsqrtps(b)` returns `h2u128` | no MXCSR |
| `rcp_approx(x: u32) -> u32`, `rsqrt_approx` | `h2s_rcp_approx/rsqrt_approx(x)` | the per-lane helpers |

`mx` is `&mut u32` in Rust and `uint32_t *` in C. The functions OR the sticky
flags they detect into it on success and on `#XM` alike. `Err(Xm)` (or a
return value of 1) means the instruction raises `#XM`: its destination
register or EFLAGS is not written. The Rust side also exports the flag
constants (`IE`..`PE`, `FTZ`, `RC_SHIFT`, `MASK_SHIFT`, `DEFAULT_NAN`,
`INT_INDEFINITE`), and the C side defines them as `H2S_*`.

## Semantics

The Intel SDM (Volume 1, sections 4.8-4.9 and 10-11, plus the instruction
pages) is the authority. Where it is vague, the choice is stated below along
with the evidence for it.

### Exceptions: detection, priority and delivery

Each element (lane) is computed on its own and produces its masked-response
result plus the exceptions it detected. Within one element the SDM's
priority applies (SDM Volume 1, 4.9.2):

1. An SNaN operand: IE.
2. A QNaN operand. This is not an exception, but it decides the result and
   suppresses everything below it. A QNaN divided by zero gives the QNaN, not
   ZE.
3. Any other invalid operation (inf - inf, 0 * inf, 0/0, inf/inf, sqrt of a
   negative number, a NaN under a signalling compare or `comiss`, an
   out-of-range conversion), or divide by zero. Both suppress DE: `denormal /
   0` raises only ZE, and `sqrt(-denormal)` raises only IE.
4. A denormal operand (DE). Execution continues, so a lower-priority exception
   can still be raised. Infinity combined with a denormal raises DE (`inf +
   denormal`, `inf * denormal`, `denormal / inf`).
5. Overflow (OE) and underflow (UE), each with inexact.
6. Inexact (PE).

IE, DE and ZE are pre-computation exceptions. OE, UE and PE are
post-computation exceptions. An element with IE or ZE has a fixed result (the
default NaN or a signed infinity) and no post-computation exceptions.

Delivery covers the whole instruction. `deliver` / `h2s__deliver` takes the
OR of every element's flags:

- If any pre-computation exception is unmasked, in any element, only the
  pre-computation flags of every element are ORed into MXCSR and the
  instruction raises `#XM`. Post-computation flags are not recorded, because
  the hardware never gets that far.
- Otherwise all flags are ORed into MXCSR. If any of them is unmasked, the
  instruction raises `#XM`.
- `#XM` leaves the destination untouched.

When an exception is unmasked, `#XM` is the only observable outcome, so the
masked-response result is never visible. Unmasked OE and UE trap before the
masked response (overflow to infinity or the largest finite number,
denormalisation, FTZ) is applied, following Intel's
[floating-point reference flowchart](https://cdrdv2-public.intel.com/786447/floating-point-reference-sheet-for-intel-architecture.pdf).
So PE goes with them only when rounding the exact result to 24 bits with an
unbounded exponent is inexact:

- `0x7F000000 * 2.0` (exactly 2^128) with OE unmasked gives `#XM` with OE
  alone. `0x7F7FFFFF * 1.5` gives OE | PE.
- `2^-126 * 2^-24` (exactly 2^-150) with UE unmasked gives `#XM` with UE
  alone, with FTZ on or off, even though denormalising it would be inexact.
- In a packed op, PE from a different lane's genuine rounding is still
  recorded.

Rosetta ignores the exception masks, so no hardware we can reach checks this
(see below). `src/tests.rs` pins it with hand-derived cases.

### Rounding, tininess and FTZ

All four RC modes are supported (MXCSR bits 13-14: nearest even, down, up,
toward zero). The arithmetic computes the exact result, or keeps enough guard
and sticky bits to round it as if it were exact, and then rounds once.

- **Overflow** is detected after rounding. In nearest mode the result is
  infinity. Toward zero it is the largest finite number. Down and up give
  infinity on the side they round toward and the largest finite number on the
  other side. Flags: OE | PE.
- **Tininess** is detected after rounding, as in SoftFloat's 8086 mode. A
  result is tiny if, rounded to 24 bits with an unbounded exponent, it is below
  2^-126. Hardware confirms this: `0x00800001 * 0x3F7FFFFE` (2^-126 * (1 -
  2^-46)) gives `0x00800000` with PE only in nearest mode, and `0x007FFFFF`
  with UE | PE toward zero.
- **UE**: when UE is masked it is set only if the result is tiny and inexact
  after denormalisation. When UE is unmasked it is set for any tiny result,
  exact or not, and the instruction traps (PE as described above).
- **FTZ** (bit 15) applies only while UE is masked. A tiny result, even an
  exact one, becomes a zero with the result's sign, and UE | PE are set.
  (Hardware: `2^-126 * 0.5` under FTZ gives +0 with UE | PE. Without FTZ it
  gives `0x00400000` with no flags.) While UE is unmasked, FTZ does nothing.
  Halo turns FTZ on at 0x339ada.
- FTZ never touches results that aren't computed: `min`, `max`, compares, and
  the zero-plus-zero fast path. FTZ is not DAZ, so denormal inputs are still
  used as they are and still raise DE.
- **DAZ** (bit 6) is ignored. The Pentium III doesn't implement it and
  `ldmxcsr` rejects it with `#GP`, so it can never be set.

### NaNs and special values

- The default NaN (QNaN indefinite) is `0xFFC00000`. It is the result of every
  masked invalid operation.
- With one or two NaN operands, an arithmetic op returns the first source
  operand (`a`, the destination register) if it is a NaN, and otherwise the
  second. Either way the result is quieted (bit 22 set), with sign and payload
  kept. IE is raised if either operand is an SNaN.
- An exact zero sum of operands with opposite signs, such as `x + (-x)`, is
  +0, or -0 when rounding down. `(-0) + (-0)` is -0.
- `sqrt(-0)` is -0, `sqrt(+inf)` is +inf, and the square root of anything
  else that is negative is the default NaN with IE.

### min and max

`minss`/`maxss` return `a` only when it is strictly less (or greater) than
`b`. Otherwise they return `b`. So they return `b` when both are zeros of any
sign, and when either operand is a NaN. They return `b` as it is: an SNaN in
`b` is not quieted. A NaN operand of either kind raises IE (`min` and `max`
signal on QNaNs). DE is raised for a denormal operand unless there is a NaN.

### Compares

`cmpss`/`cmpps` use bits 0-2 of imm8. Bits 3-7 are ignored, as on any SSE1 CPU:

| pred | name | true when | signals on QNaN |
|---|---|---|---|
| 0 | EQ | ordered and equal | no |
| 1 | LT | ordered and a < b | yes |
| 2 | LE | ordered and a <= b | yes |
| 3 | UNORD | either is a NaN | no |
| 4 | NEQ | unordered or not equal | no |
| 5 | NLT | unordered or not a < b | yes |
| 6 | NLE | unordered or not a <= b | yes |
| 7 | ORD | neither is a NaN | no |

An SNaN raises IE under every predicate. A QNaN raises IE only under the
signalling ones. -0 equals +0. DE is raised for a denormal operand only when
neither operand is a NaN, under every predicate. Rosetta disagrees for UNORD
and ORD (see below).

`comiss` raises IE on any NaN. `ucomiss` raises it only on an SNaN. Both raise
DE for a denormal operand when neither is a NaN. They return the EFLAGS bits
ZF (0x40), PF (0x04) and CF (0x01): unordered 0x45, less 0x01, equal 0x40,
greater 0. The caller clears OF, SF and AF, and leaves EFLAGS untouched on
`#XM`.

### Conversions

- `cvtsi2ss` and `cvtpi2ps` round under RC and raise only PE.
- `cvtss2si` and `cvtps2pi` round under RC. `cvttss2si` and `cvttps2pi`
  truncate. A NaN, an infinity or a result outside the int32 range gives the
  integer indefinite `0x80000000` with IE and no PE. Otherwise the conversion
  raises PE if it is inexact. Exactly -2^31 converts without IE.
- Conversions never raise DE. A denormal converts to 0, or to ±1 when it
  rounds away from zero, with PE.
- For `cvt[t]ps2pi` the flags of both lanes are delivered together, under the
  rules above.

### rcp and rsqrt

These raise no exceptions, set no flags, and ignore RC and FTZ.

- `rcp`: ±0 and denormals give ±inf. ±inf gives ±0. NaNs are quieted.
  Results below the normal range are flushed to a signed zero, which happens
  for every |x| >= 2^126.
- `rsqrt`: +0 and positive denormals give +inf. -0 and negative denormals
  give -inf. +inf gives +0. Every other negative input, -inf included, gives
  the default NaN. NaNs are quieted.

**Precision and the table.** Intel promises only a relative error of at most
1.5 * 2^-12, so the bits depend on the implementation. The model is a rule:

- **rcp**: the top 11 fraction bits of the input pick an interval [1 + i/2048,
  1 + (i+1)/2048). The result's significand is 1/midpoint rounded to nearest
  at 12 fraction bits: `n = round(2^25 / (4097 + 2i))`, giving `0.n` with
  `n/8192` in (0.5, 1). The exponent is the input's mirrored.
- **rsqrt**: the parity of the exponent and the top 10 fraction bits pick an
  interval of [1, 2) or [2, 4). The result's significand is 1/sqrt(midpoint)
  rounded to nearest at 12 fraction bits, computed exactly with an integer
  square root.

Evidence:

- The rule reproduces the RCPPS/RSQRTPS table measured on Intel hardware
  (Core i7-12700H and i9-11900H, which agree on all 2^32 inputs, from
  [camoufox PR #35](https://github.com/JWriter20/camoufox/pull/35)) for all
  12,288 entries.
- Both sides reproduce the published FNV-1a hashes of the native
  instructions over all 2^32 inputs, flush thresholds and special values
  included (`tests/approx.rs`, `every_input_matches_intel`, about 5 minutes
  in release).
- The 86Box Pentium III work
  ([skiretic/86box-cpu-experimental#279](https://github.com/skiretic/86box-cpu-experimental/issues/279))
  reports that a rule-built "exact-rational" table matched a real Katmai
  Pentium III (CPUID 0672) over 1024 of 1024 test classes. That fits Intel
  keeping the same table from the Pentium III onward. We haven't seen that
  rule or table ourselves, and nobody has compared a physical Xbox CPU (a
  Coppermine-derived Pentium III) directly.

Rosetta's tables differ, for example `rcp(1.0)`: Rosetta gives `0x3F7FF800`,
Intel `0x3F7FF000`. Rosetta's output is never used as the reference for these
two instructions.

## Testing

`cargo test -p simd` runs everything below on any host, and nothing in it
needs Rosetta:

| Test | What it checks |
|---|---|
| `src/tests.rs` | hand-checked cases for every rule above |
| `tests/differential.rs` | Rust vs C, with C built under UBSan: 159,894 systematic edge records (every op on every pair of 63 edge values, all predicates, RC/FTZ modes, unmasked exceptions) plus 2,000,000 random records with any MXCSR (`H2SIMD_DIFF_N`, `H2SIMD_DIFF_SEED`) |
| `tests/hardware.rs` | both sides vs `tests/fixtures/sse-rosetta.bin`: 5,772 records of real x86_64 SSE results recorded once under Rosetta (masked MXCSR, all RC/FTZ, pre-set sticky flags, every op except rcp/rsqrt) |
| `tests/approx.rs` | rcp/rsqrt vs the measured Intel table hash, plus the ignored exhaustive 2^32 check |
| `tests/mutation.rs` | a C side built with `-DH2SIMD_TEST_MUTATION` (round up done as round down) has to be caught by the differential and by the hardware fixture, and only in round-up records |
| `tests/oracle_corpus.rs` | both sides vs the sse-oracle JSONL corpus at `H2SIMD_ORACLE_CORPUS`; it skips when that is unset |

Opt-in checks:

- `H2SIMD_ROSETTA=1 cargo test --release -p simd --test hardware`: a fresh
  random set through the host's real SSE unit. It runs natively on x86_64 and
  through `arch -x86_64` on Apple silicon, and skips if neither works.
  `H2SIMD_HW_N` and `H2SIMD_HW_SEED` set the size and seed. Adding
  `H2SIMD_RECORD_FIXTURE=1` re-records the fixture.
- `CC=<a wrapper running clang -arch x86_64>`: runs the differential with the
  C side built for x86_64.

Results recorded while writing this crate:

- Rust vs C: 30,000,000 random records (release) and 3,000,000 with the C side
  built for x86_64, with no differences.
- Rust vs live SSE under Rosetta: 3 x 2,000,000 random records plus the edge
  set, with no differences except the known compare deviation.
- The sse-oracle corpus (5,123,616 fixtures, 943,824 of them the
  instructions here, all four RC modes, FTZ on and off): every one matches on
  both sides. Of its 134,832 rcp/rsqrt fixtures, 53,256 differ, as expected
  given Rosetta's tables.

### Known hardware deviations

Rosetta 2 stands in for the hardware and is not a Pentium III. Where it
disagrees with the SDM, the SDM wins:

1. **Exception masks.** Rosetta never raises `#XM` and reads the masks back as
   all set, so unmasked behaviour rests on the SDM and the Rust/C
   differential, which tests unmasked MXCSRs heavily.
2. **DE on UNORD/ORD compares.** For `cmpss`/`cmpps` with predicates 3 and 7,
   Rosetta sets DE when one operand is a NaN and the other a denormal. For the
   other six predicates, and for every other instruction (arithmetic,
   min/max, `comiss`), it lets the NaN suppress DE. We use the one consistent
   rule (a NaN suppresses DE), which is how we read Bochs's softfloat compare
   too, though we haven't checked that against its source. The fixture keeps
   Rosetta's raw output, and the tests accept exactly this difference.
3. **rcp/rsqrt tables**: see above.
4. MXCSR loading (reserved bits, DAZ, `ldmxcsr 0`) is outside this crate. The
   lifter's `ldmxcsr` raises `#GP` for bits 16-31 and for DAZ.
