# Floating point in the lifter

Measured 2026-11-15 with `h2 fpstats` ([crates/analyze/src/fpstats.rs](../crates/analyze/src/fpstats.rs))
over the retail `default.xbe` (SHA-256 `03215919…935d`, see [engine-map.md](engine-map.md)).

Status: measurement plus design. This refines the FP parts of
[LIFTER.md §3 and §8](LIFTER.md). The interface is in
[FLOAT-ABI.md](FLOAT-ABI.md). The design was revised after the design review
to be consistently correctness-first.
- **Built:** `crates/x87` (the x87 ext80 helpers), `crates/simd` (SSE),
  event 14 (#MF) in the runtime and headers, with the MMX and `emms` checks,
  and the lowering of every x87 instruction to the helpers in the IR,
  lifter and emitter (e5c4251).
- **Built, on by default:** a guarded native binary64 path for PC=53
  arithmetic (`H2_X87_NATIVE=0` disables it); see
  [x87-native.md](x87-native.md) for its range proof, status-bit
  reconstruction, fallback modes and validation. Operations outside its
  guards, and every build with it off, use the ext80 helper path described
  below.

**Recommendation, in one paragraph.** Halo 2 is an SSE1 game with an x87
sidecar. Scalar SSE is 53% of its floating-point instructions, packed SSE 20%,
x87 25%, MMX under 2%, and nothing past a Pentium III. Every FP instruction is
lowered for bit-exactness first. SSE goes through `crates/simd`, which is
integer soft-float ([simd.md](simd.md)). x87 goes through `crates/x87`, which
is integer ext80 arithmetic that is exact under every precision and rounding
control. It runs on master's `X87Abi`, keeps the stack as architectural state
(TOP in the status word), and handles stack faults, sticky flags and #MF
delivery like a P6 does. No host floating point and no libm decide any
guest-visible bit. The game pins PC=53 and round-to-nearest with all
exceptions masked, which makes a later `double` fast path possible, but only
under the written value-level proof rule in §2.8, never by default. The one
documented approximation is the x87 transcendentals. They are evaluated at 512
bits and rounded to nearest, which is very likely correctly rounded but not
proven, and they are not the PIII's 66-bit-π results. They sit under a named
oracle tolerance until hardware traces exist.

Evidence labels follow LIFTER.md: **measured** means `h2 fpstats` output on
this XBE; **source** means code that was read (Unicorn/QEMU, iced-x86); **design**
is a proposal. The tool is static. It counts what the analyzer decoded and
claimed, which is not what executes.

## 1. What the game uses

Run it with:

```sh
cargo h2 fpstats [disc] [--root 0x12190]
```

`--root` adds start points for the reachability split. `0x12190` is
`game_main` in [engine-map.md](engine-map.md).

### 1.1 Mix (measured)

1,199,450 decoded instructions. 171,806 of them (14.3%) are floating point or SIMD.

| class | instructions | share | functions using it |
|---|---:|---:|---:|
| SSE scalar (`*ss`, `cvt*ss*`) | 91,403 | 53.2% | 3,398 |
| x87 | 42,377 | 24.7% | 1,930 |
| SSE packed (`*ps`, `cvt*ps*`) | 34,648 | 20.2% | 2,308 |
| MMX | 2,712 | 1.6% | 72 (incl. SSE-MMX ints) |
| SSE state/hints (`ldmxcsr`, `prefetch*`, `sfence`) | 442 | 0.3% | |
| SSE integer ops on MMX regs (`pshufw`, `movntq`, ...) | 224 | 0.1% | |
| SSE2 or later, 3DNow! | **0** | | 0 |

4,477 functions touch FP. 1,248 mix x87 and SSE in one body; one common shape is
x87 for `fsqrt` and compares inside otherwise-SSE vector code. Only 5 mix MMX
and x87. The top 10% of FP functions (448) hold 53.9% of FP instructions.
Some of the largest are in the `0x0035f000–0x00365000` range at around 590
SSE instructions each, and `0x00143600` has 1,704.

By section, MMX is mostly third-party code: Bink (`BINK*` sections, 1,485),
WMA (`WMADEC`, 253) and `D3D` (60), with 914 in `.text`. DirectSound's section has
907 x87 instructions and `D3D` has 1,035, and both are HLE targets. The game's own
`.text` holds 38,568 x87, 91,395 scalar SSE and 34,335 packed SSE instructions.

Top mnemonics: `movss` 47,720; `movaps` 19,832; `mulss` 17,137; `fld` 12,143;
`addss` 9,116; `fstp` 8,703; `subss` 6,949; `comiss` 6,379; `fmul` 5,251;
`shufps` 3,286; `mulps` 3,238; `xorps` 2,811. The full table is in the tool's
output.

**x87 memory operand sizes:** float32 24,848 (96%), float64 987, int32 1,634,
int64 19, int16 2, float80 **29**, and 4 FPU-state (`fnsave`/`frstor`). The x87
unit mostly loads floats, computes, and stores floats.

### 1.2 Control words and rounding (measured)

There are 10 `fldcw` and 9 `fnstcw`. Every `fldcw` sits in one of 9 CRT
functions in `0x320000–0x374000`; none is in game code. To find the modes, the
tool resolves the arguments pushed at call sites of those functions and of
their short wrappers:

- `_control87` at `0x003212d6` has 26 direct callers. 15 pass
  `(_PC_53 | _EM_DENORMAL | 0x1f, mask 0xfffff)`, which means PC=53,
  round-to-nearest, affine off, all exceptions masked: control word `0x027f`.
  10 pass the same value with mask `_MCW_EM` (masks only). One wrapper
  (`_controlfp`, `0x00321308`) passes `(_PC_53, _MCW_PC)`. The first call is in
  `platform_bootstrap` at `0x00012038`, before nearly all game work. The
  others include `0x00053580` and `0x00053f50`/`0x00053f80`, which also set
  MXCSR masks and look like per-thread FP initialization.
- `PC=24` is never requested and `PC=64` is never requested by game code.
  `PC=64` occurs only inside CRT internals: the transcendental dispatchers at
  `0x00327d64`/`0x00327dcb` load `0x133f` (PC=64, near) or a PC=53 variant
  derived from the saved word, and restore afterwards. Two CRT functions that
  are very likely `ceil` and `floor` (`0x00320fc6`, `0x00321099`, via
  `0x00324be9`) load `0x1b3f` (RC=up) and `0x173f` (RC=down), both PC=64.
- Round-toward-zero via `fldcw` never appears. Float-to-int is `_ftol2`
  (`0x00322b0c`, **214 callers**): `fistp` at round-to-nearest, then a
  correction toward zero. There are 256 inline `cvttss2si` and 636 `fistp`
  overall.
- `fnsave`/`frstor` appear twice each; `fninit`, `fldenv`, `fnstenv` and
  `fxsave`/`fxrstor` appear zero times.

**MXCSR:** 23 `ldmxcsr`, 23 `stmxcsr`, 0 `fxrstor`. Every write resolves to
`saved | 0x1f80` (mask all) or `saved & ~0x3f` (clear sticky flags). None sets
FTZ, DAZ or RC, so the game runs SSE with the thread default (expected
`0x1f80`, to be confirmed against Xbox/xemu; see LIFTER.md §3). Denormals are
therefore live, IEEE-gradual, in all SSE code. Nothing writes the PIII-reserved
DAZ bit.

### 1.3 Compare idioms (measured)

| idiom | sites |
|---|---:|
| `fcomip` / `fcomi` / `fucomip` (P6, straight to EFLAGS) | 823 / 2 / 9 |
| `fnstsw ax` then `test ah, 0x41` | 632 |
| `fnstsw ax` then `test ah, 0x05` | 504 |
| `fnstsw ax` then `test ah, 0x01` | 96 |
| `fnstsw ax` then `test ah, 0x44` (unordered/parity) | 71 |
| `fnstsw ax` then `sahf` | 9 |
| `fnstsw ax` then `and ax, 0x20` (PE, sticky precision) | 3, all CRT |
| `fnstsw m16` | 14, all CRT (`_statusfp`/`_clearfp` and friends) |
| other or unrecognized consumer | 11 |
| `comiss` / `ucomiss` | 6,379 / 829 |
| `cmpss` / `cmpps` | 22 / 45 |

The game's own code, compiled for P6, prefers `fcomip`. `fnstsw` is
concentrated in library code (1,340 total, 157 in functions reached by direct
calls from `game_main`). Only CRT code reads x87 exception flags. Nothing reads
MXCSR flags except the read-modify-write sequences above.

### 1.4 Transcendentals and 80-bit (measured)

`fsqrt` 564, `fcos` 273, `fsin` 249, `fpatan` 88, `fyl2x` 55, `fscale` 31,
`frndint` 30, `f2xm1` 25, `fptan` 19, `fsincos`/`fprem`/`fxtract` 1 each.
`fsin`/`fcos`/`fpatan` are mostly inline in game code; `fyl2x`/`f2xm1`/`fscale`
sit in CRT `pow`/`exp`/`log`. Over SSE: `sqrtss` 37, `rsqrtss` 118, `rsqrtps` 14,
`rcpss` 9.

All 29 80-bit memory operands (`fld m80` 25, `fstp m80` 4) are in 19 CRT
functions in `0x322bf5–0x37379a` (constants and spills inside transcendental
helpers). Game code never touches an 80-bit value in memory.

### 1.5 x87 stack depth (measured)

The tool walks each function's CFG from depth 0, following direct branches and
the analyzer's jump tables. It tracks the stack depth relative to entry, the
slots read before being pushed (values passed in), depth at each return and
call, and joins reached at two depths. Calls apply the callee's own effect
(slots consumed, net change). Where that effect is unknown, the walk assumes
the MSVC convention: what is on the stack at the call is arguments, and a
result exists only if the straight-line code after the call reads it.

| result over the 1,930 x87 functions | count |
|---|---:|
| single static depth everywhere, one return depth, nothing unresolved | **1,899 functions, 41,393 x87 insns (97.7%)** |
| inconsistent depth at a join | 15 |
| unresolved indirect jump | 10 |
| several return depths | 6 |
| TOP loaded from memory (`frstor`) | 2 |
| take x87 arguments (entry needs > 0) | 79 |
| return 0 / 1 / 2 values on the stack | 1,668 / 138 / 1 (123 unresolved) |
| max depth 0 / 1–4 / 5–8 / >8 | 60 / 1,725 / 144 / 1 (an inconsistent outlier) |
| call x87 functions with values live below the callee's arguments | 64 |

The functions that take or return stack values are mostly CRT helpers:
`_ftol2` (takes 1, returns 0), a two-argument `_CI*` entry, likely `_CIpow`
(`0x00372d48`, 51 callers; the walk leaves it unresolved, but callers pass 2
values and consume 1), an inverse-trig `_CI*` entry (`0x00322bd8`, 38
callers), plus small game helpers returning a float in ST(0). The unresolved remainder sits in the CRT intrinsic
dispatchers, which share a frame across `call` and use `jmp [ebx]`
tables, and in a handful of game functions downstream of them.

### 1.6 What runs before the first menu frame (measured lower and upper bounds)

No trace exists yet. Two static sets bracket the question:

- **Reached by direct calls/jumps from the entry point and `game_main`:**
  7,353 functions, 50.4% of FP instructions (86,526).
- **Also following code-address immediates** (thread routines, registered
  callbacks): 9,464 functions, 60.3% (103,568).

Virtual calls and data-table callbacks are not followed, so these sets miss
code. They also include gameplay code that a menu frame never runs. The
mainmenu frame does load a scenario/BSP and render a 3D world plus UI
(engine-map.md), so the menu needs the general vector/matrix math paths, not a
toy subset. MMX is the clear outlier at 146 of 2,936 reachable, in Bink/WMA
and HLE'd D3D code.

## 2. Design

### 2.1 Representation of x87 values

**Design: ext80 always, through helpers.**

The x87 state is master's `X87Abi` inside `CpuAbi` (FLOAT-ABI.md §1):

- eight physical registers as guest-format `uint8_t[10]`;
- control, status (holding TOP), tag and opcode;
- FIP/FDP and the selectors.

That is the only representation. Every x87 instruction lowers to a call into
`crates/x87`. That crate does integer-only ext80 arithmetic, exact for every
precision and rounding control, so a lifted function gets the hardware's bits
under any control word, any stack depth and any exception mask.

The cost is a function call per x87 instruction, at about 65 ns or less for
arithmetic (FLOAT-ABI.md §6.2). x87 is 25% of FP instructions and is
concentrated in a few functions, so this is acceptable as a first build. Speed
comes later, from a `double` fast path where the proof rule in §2.8 allows it,
and never at the expense of a bit.

**`long double`: never.** On Apple arm64, `long double` is `double`. On x86-64
hosts it is the host x87, which Clang neither keeps at PC=53 nor treats
consistently. No host floating point is used for x87 values at all.

**Transcendentals.** These are `fsin`, `fcos`, `fptan`, `fpatan`, `fyl2x`,
`fyl2xp1`, `f2xm1` and `fsincos`.

- **Precision and rounding.** They compute at full 64-bit precision whatever
  PC is. The crate evaluates them at 512 bits and rounds once, to nearest
  only. Under any other RC the helper stops with `Unmodelled` rather than
  guess; measured, that never happens.
- **Range.** For |x| ≥ 2^63, `fsin`, `fcos` and `fptan` set C2 and leave the
  operand unchanged, exactly.
- **Not the Pentium III's bits.** The PIII reduces arguments with a 66-bit π,
  and the crate reduces with the exact value. The difference is documented
  and kept under a named oracle tolerance (§2.9).
- **Accuracy evidence and speed** are in FLOAT-ABI.md §6.2.

### 2.2 The x87 stack in the IR

**Design: the stack is architectural state.**

The IR keeps one `X87` state, and ops name registers as `St(i)` relative to
TOP, exactly as the instruction does (FLOAT-ABI.md §3).

- **Stack operations.** The helpers push, pop, maintain the tags, and raise
  stack faults: IE+SF, C1, and the indefinite when masked.
- **No depth analysis needed.** The lifter needs no stack-depth analysis for
  correctness, and so has no dynamic fallback tier. The 15 inconsistent
  joins, 10 unresolved indirect jumps and the `frstor` functions in §1.5 are
  handled like everything else.
- **What the walk is for now.** The `fpstats` stack walk becomes an
  optimisation input. A future fast path can use it to turn a proven
  fixed-depth region into named `double` slots, under §2.8, with
  `fxch`-as-permutation and pushes and pops removed. Only `Proven` walk
  results may feed that.

**MMX aliasing** is owned by `h2gen.h`, not by `crates/x87`.

- An MMX write stores the 64-bit value in bytes 0..7 of the physical register
  and `0xffff` in bytes 8..9.
- MMX entry sets every tag valid and clears TOP in `status`.
- `emms` sets every tag empty.
- Every MMX instruction starts with `h2i_x87_wait`, and `emms` makes the same
  check, because a pending x87 exception is delivered as #MF there, before
  the instruction does anything (FLOAT-ABI.md §2.2).

None of this is needed before the menu.

### 2.3 Precision control

Precision control is honoured exactly at every setting.

- **PC=24, 53 and 64.** The crate rounds the significand to the selected
  width and keeps the full 15-bit exponent range, as the hardware does.
  - The measured steady state is PC=53/RN, set by `_control87(0x9001f,
    0xfffff)` in `platform_bootstrap` and by the per-thread init.
  - PC=64 appears only inside CRT internals (`0x133f`, `0x1b3f`, `0x173f`).
  - Both run through the same helpers, with no mode guard and no stop.
- **`fldcw`** is `h2x87_v1_ldcw`. It writes the normalised word
  `(cw & 0x1f3f) | 0x40` and recomputes ES/B. The CRT translator's `0x023F`
  reads back as `0x027F` (FLOAT-ABI.md §2.3).
- **Which operations PC affects.** It governs `fadd`, `fsub`, `fmul`, `fdiv`
  (including integer and memory sources) and `fsqrt`. It does not affect
  loads, stores, constants, `frndint`, `fscale`, `fxtract`, `fprem` or the
  transcendentals (FLOAT-ABI.md §4.6).

### 2.4 Rounding modes and the CRT

- **Every RC is honoured exactly** by every rounding x87 operation:
  arithmetic, `fist`/`fistp`, `frndint`, `fscale`, stores to m32/m64/m80/BCD,
  and the constants. The transcendentals are the exception, stopping under a
  directed RC (§2.1). Directed rounding is measured only in CRT `floor`/`ceil`
  (`0x1b3f`, `0x173f`).
- **No host FP environment dependence.** x87 helpers are integer code, and SSE
  is integer soft-float (`crates/simd`, §2.7). Generated C still compiles with
  `-ffp-contract=off -fno-fast-math` as a belt-and-braces measure, but no
  guest-visible result depends on host FP state or host libm.
- **`fist`/`fistp`** (637 overall) are `h2x87_v1_st_i16/i32/i64`. They round
  by the guest RC, and give the integer indefinite (`0x8000`, `0x80000000`,
  `0x8000000000000000`) with IE for NaN or out-of-range values. They follow
  the convert, write, commit protocol of FLOAT-ABI.md §4.2. The
  tick-budget code (`0x1467a1`/`0x1469ad`) and script sleep (`0x208954`) run
  `fld m32; fistp m32`, which makes RC observable, so this matters.
- **Rust overrides for CRT FP helpers.** These cut call overhead and the
  dispatcher frames. They are not needed for correctness. They replace:
  - `_ftol2`, `_control87`, `_controlfp`, `_clearfp`, `_statusfp`;
  - `floor`, `ceil`, `_CIpow`, `_CIfmod`;
  - the `_CIsin`/`_CIcos` dispatchers, `exp`/`log`, and
    `asin`/`acos`/`atan2`.

  Each override must produce the guest's integer and register results. It
  follows the x87 stack contract the routine has, and is validated against
  the lifted original running on the helpers (LIFTER.md §8 replay).
  - **`_ftol2`** (`0x322b0c`, 214 callers) has the contract in FLOAT-ABI.md
    §4.5. It returns int64 in EDX:EAX by truncation, gives
    `0x8000000000000000` for NaN or |x| ≥ 2^63, and consumes one stack value.
  - **`fisttp`** (SSE3) is not a P6 instruction. The decoder raises #UD for
    it. `h2x87_v1_st_i32_trunc` is a helper for overrides only.

### 2.5 Exceptions and #MF

The game masks every x87 exception, but the model does not depend on that.

- **Pre-computation exceptions** (IE, stack faults, DE, ZE), when unmasked,
  are modelled exactly. The raising instruction sets only the status word
  (no OE/UE/PE from the computation it skipped), and #MF (event code 14,
  vector 16, `arg` = FSW) is delivered at the next waiting x87 instruction,
  `fwait`, or MMX instruction. `saved_eip` is that instruction, and
  FIP/FOP/FDP describe the raiser. `fnstenv` discards a pending #MF by
  masking everything.
- **Priority** follows SDM Vol. 1 §4.9.2: invalid, then ZE, then DE, then
  OE/UE, then PE. A denormal divided by zero is ZE alone.
- **Post-computation exceptions** (OE, UE, PE), when unmasked, stop with
  `Unmodelled` (code 12). The hardware stores a biased or rounded result
  first, which is not modelled. With UE unmasked that includes every tiny
  result, exact or not.
- **Sticky flags** are always computed exactly, so `fnstsw`, `_statusfp` and
  `and ax, 0x20` see true values.

FLOAT-ABI.md §2 has the details, including where event 14 lives in the
runtime, `h2abi.h` and `h2gen.h`.

### 2.6 `fnstsw`/`sahf` flag fusion

**Design: compares return their relation; fusion is a peephole over that.**

`h2x87_v1_cmp` writes C3/C2/C0 (or, for `fcomi*`, returns ZF/PF/CF) and also
returns the bits in its low payload (FLOAT-ABI.md §4.4).

- **`fcomi*` (834 sites).** On `H2X87_OK` the generated code replaces
  ZF/PF/CF with the payload. On `H2X87_SUPPRESS` (an unmasked invalid or
  denormal: the compare did not happen) it keeps them. Either way it clears
  OF/SF/AF and marks all six flags defined in `eflags_defined`.
- **`fnstsw ax`** is a field read of `status`. TOP is in it already, and the
  sticky flags are exact.
- **Peephole fusion.** It recognises `fnstsw ax; test ah, M; jcc` and
  `fnstsw ax; sahf; jcc` and branches on the returned payload instead of
  re-reading AX, while still materialising AX unless it is dead. The
  payload's low 16 bits are the C3/C2/C0 `fnstsw` would read on both
  `H2X87_OK` and `H2X87_SUPPRESS` (the latter keeps the old ones), so the
  fused branch is right either way; only `H2X87_MF` must be checked first
  (FLOAT-ABI.md §4.4).
- **Bit positions.** In AH, C0 is bit 0, C2 bit 2 and C3 bit 6. For `sahf`,
  CF = C0, PF = C2, ZF = C3, AF = status bit 12 (TOP bit 1), and SF = B
  (status bit 15). The measured masks map as follows:

| sequence (`a` = ST(0), `b` = operand) | taken when | C for the branch |
|---|---|---|
| `test ah,0x41; jz` (632 sites, either jcc) | C3=C0=0: gt | `a > b` |
| `test ah,0x41; jnz` | lt, eq or unordered | `!(a > b)` |
| `test ah,0x05; jp` (504) | even parity of C2,C0: gt, eq or unordered | `!(a < b)` |
| `test ah,0x05; jnp` | lt | `a < b` |
| `test ah,0x01; jz` / `jnz` (96) | C0=0: gt or eq / C0=1: lt or unordered | `a >= b` / `!(a >= b)` |
| `test ah,0x44; jnp` / `jp` (71) | eq / gt, lt or unordered | `a == b` / `a != b` |
| `sahf; jcc` (9) | CF=C0, PF=C2, ZF=C3 | as for `comiss` |

Derive each branch condition from the bit definitions, not from this table.
Exercise every mask/jcc pair, including unordered, in oracle fixtures.
Unrecognised consumers (11 sites) use the materialised AX.

- **`ftst`** (6 sites) is an `fcom` against +0.0 (FLOAT-ABI.md §3).
- **`fcom` and `fucom`** differ in raising IE on a QNaN. The helper models
  that exactly.

### 2.7 SSE

SSE is defined by `crates/simd` and [simd.md](simd.md). It is integer
soft-float and uses no host floating point, with the C side in `h2simd.h`. It
covers results, NaN payloads, signed zeros, MXCSR sticky flags and `#XM`.

The earlier plan in this section lowered SSE to host C `float` with NaN
fix-ups. That plan is superseded and must not be revived without the same
kind of value-level proof as §2.8.

Measured facts that `crates/simd` relies on:

- Nothing sets FTZ, DAZ or RC. Every MXCSR write is `saved | 0x1f80` or
  `saved & ~0x3f` (§1.2).
- `rcp`/`rsqrt` need hardware tables or a tolerance (simd.md).

### 2.8 Proof rule for a future `double` fast path

The optional native path in [x87-native.md](x87-native.md) applies this
rule per operation with narrower range guards and exact PE/C1 recovery.
It retains the architectural ext80 stack. A function, or a region of one,
may later keep
x87 values in host `double` locals only if, for every value, the lifter can
prove that the `double` computation gives the same bits as the ext80 helpers.
The rule is:

**1. dbl-exact values: exactly representable as a double and produced exactly.**
- `fld m32`, and `fld m64` (an SNaN load is quieted, with IE);
- `fild m16`, `fild m32`, and `fild m64` iff \|v\| ≤ 2^53;
- `fldz`, `fld1`;
- `fabs`, `fchs`, `fxch` and `fld st(i)` on dbl-exact values.

**2. The same bits as C `double` at PC=53 and RC=nearest, on dbl-exact
operands.** This covers `fadd`, `fsub`, `fmul`, `fdiv` and `fsqrt`, including
the integer-source forms. Three cases are excepted:
- results above `DBL_MAX` (the x87 keeps a finite extended value);
- `fmul` and `fdiv` results below `DBL_MIN` (the x87 keeps a normal extended
  value where the double denormalises);
- NaN results (the x87 indefinite is `0xFFF8…` once narrowed, and the larger
  significand wins).

Guard these with one compare on the result and an ext80 recompute on the rare
path. `fadd`/`fsub` of doubles cannot produce a tiny inexact result, so they
need only the overflow and NaN guards.

**3. Also exact on dbl-exact values** (no guard beyond the special-value
cases above):
- `fst m64` and `fst m32` (double rounding through 53 bits is innocuous for
  binary32);
- all compares;
- `fist`/`frndint` under RN;
- `fscale` while the result stays in double range;
- `fprem`.

**4. ext80 only: the result is tainted.** Everything downstream of a tainted
value goes through the helpers:
- `fldpi`, `fldl2e`, `fldl2t`, `fldlg2`, `fldln2` (73 sites, 64-bit constants);
- every transcendental;
- `fld m80` and `fstp m80`;
- anything under PC=64 or PC=24, or under a non-nearest RC.

**Also required:**
- The control word must be proven to be PC=53/RN with every exception masked
  over the region. The sticky flags the helpers would have set must be either
  dead or recomputed at the region's exit.
- At region boundaries, live values are written back to `X87Abi` exactly
  (double to ext80 is exact), with TOP and the tags set accordingly.

### 2.9 How the oracle checks FP

**Facts about the current oracle** (source: `unicorn-engine-sys 2.1.5`, QEMU
`5.0.1` core, `qemu/target/i386/fpu_helper.c` and `ops_sse.h`):

| behaviour | Unicorn 2.1.5 | usable as evidence? |
|---|---|---|
| x87 add/sub/mul/div/sqrt | SoftFloat `floatx80`; PC and RC from the control word (`cpu_set_fpuc`) | yes, bit-exact |
| `fsin`, `fcos`, `fptan`, `fpatan`, `f2xm1`, `fyl2x` | converted to host `double`, host libm (`sin`, `atan2`, `pow`, `log`), converted back | **no** |
| x87 status exception flags | only ZE (divide by zero) is set; PE/UE/OE and most IE are not | **no** |
| C1 and undefined C0/C2/C3 | not verified against the SDM | **no** until verified |
| control word bit 6 | stored raw (P6 forces 1) | **no** |
| MXCSR sticky flags | not updated from `sse_status` | **no** |
| `rcpps`/`rsqrtps`/`rcpss`/`rsqrtss` | exact `1/x`, `1/sqrt(x)` | **no** (not PIII) |
| `minps`/`maxps` | `lt ? a : b`, matches SSE | yes |
| `cvtt*2si` indefinite | `0x80000000` via `WRAP_FLOATCONV` | yes |
| NaN propagation | SoftFloat's target-specific `pickNaN` | verify against SDM Table 4-7 fixtures first |

**Policy (design):**

1. **Bit-exact by default.** This covers:
   - every ext80 register byte and every XMM byte;
   - the control word except bit 6;
   - C0–C3 where the SDM defines them, TOP and the tags;
   - EFLAGS from compares, memory bytes, and integer results;
   - event 14 and its `arg`.
2. **Masked, by name.** Each mask comes from a table keyed by mnemonic, never
   a blanket FP ignore. **`crates/x87` is the authority for masked bits; Unicorn
   is not evidence for them.**
   - x87 sticky exception bits and MXCSR bits 0–5, in the instruction
     families where Unicorn does not model them.
   - C1 and the undefined C0/C2/C3, until hardware-verified.
   - FIP/FDP/FOP/FCS/FDS, except for `fnsave`, `fnstenv` and #MF.
   - Control-word bit 6, tagged `needs-hardware`.
3. **Tolerance, by name, with a reference.**
   - **x87 transcendentals.** The reference is `crates/x87`'s 512-bit result,
     cross-checked at 768 bits when the fixture is generated. Accept a
     difference of at most 1 ext80 ulp against PIII traces once those exist.
     C2 and the out-of-range behaviour must match exactly. Unicorn's answer
     is neither expected nor counted. The PIII's 66-bit-π reduction
     (FLOAT-ABI.md §6.2) is the known source of larger differences near
     multiples of π; label such cases `pi66` and report their count.
   - **`rcp*`/`rsqrt*`.** Per simd.md.
4. **Fixture sets per family:**
   - ±0, smallest/largest denormal, ±FLT_MIN/MAX and the DBL equivalents, ±∞;
   - QNaN/SNaN with distinct payloads in each operand position;
   - round-to-even ties, and PC=24/53/64 double-rounding cases;
   - values around ±2^31 and ±2^63 for conversions;
   - every `fnstsw`/`test`/`jcc` combination with unordered operands;
   - unmasked-exception cases for #MF ordering.
5. **Independent evidence for what Unicorn can't answer.** These need PIII or
   Xbox hardware traces, or xemu traces for sticky flags, noting xemu's QEMU
   ancestry:
   - transcendental bit patterns;
   - `rcp`/`rsqrt` bit patterns and the `rcp` tiny-flush window;
   - x87 exception flags, C1, and CW bit 6.

   Until then, fixtures for those families are tagged `needs-hardware`.
6. **Function replay** (LIFTER.md §8) compares any future fast path against
   the helper-only lowering of the same lifted function, and compares the
   arm64 and x86-64 builds.

## 3. What to build first

The rows are ranked by their share of the 171,806 measured FP instructions.
"Reached" is the set of instructions reached by direct calls from the entry
point and `game_main` (§1.6), a lower bound on what runs. The **menu** column
says whether the first mainmenu frame needs the row. There is no trace yet,
so that column is a judgement from §1.6 and engine-map.md, not a measurement.

| # | lowering | insns | share | reached | menu |
|---:|---|---:|---:|---:|---|
| 1 | SSE scalar moves and arithmetic through `crates/simd` (`movss`, `addss`, `subss`, `mulss`, `divss`, `sqrtss`) | 82,019 | 47.7% | 50,600 | **yes** |
| 2 | x87 through the `crates/x87` helpers on `X87Abi`: loads, stores (three-phase), arithmetic, `fxch`, constants, `fsqrt` | 37,375 | 21.8% | 16,580 | **yes** |
| 3 | SSE packed moves, shuffles and bit logic (`movaps`, `shufps`, `xorps`, `movlps`/`movhps`, `andps`, `movmskps`, ...) | 27,530 | 16.0% | 10,180 | **yes** |
| 4 | Compares and flag fusion: `comiss`/`ucomiss`, `fcomip` (with `eflags_defined`), `fcomp`+`fnstsw`+`test ah`, `sahf`, `ftst`, `cmpss`/`cmpps` | 10,786 | 6.3% | 5,502 | **yes** |
| 5 | SSE packed arithmetic (`mulps`, `addps`, `subps`) | 6,895 | 4.0% | 1,303 | **yes** (matrix/vector paths) |
| 6 | MMX and SSE-on-MMX integer ops, with the `h2gen.h` aliasing and #MF check | 2,936 | 1.7% | 146 | no: Bink/WMA/HLE'd D3D; stop on use |
| 7 | Conversions with x86 indefinite: `cvtsi2ss`, `cvttss2si` (simd), `fistp` (x87, RC-exact), `cvtpi2ps`/`cvtps2pi` | 2,731 | 1.6% | 1,491 | **yes** (`_ftol2` has 214 callers) |
| 8 | x87 transcendentals through `crates/x87` (`fsin`, `fcos`, `fpatan`, `fyl2x`, `f2xm1`, `fscale`, `frndint`, ...) | 773 | 0.4% | 453 | **yes**: camera/orientation math is inline |
| 9 | Hints and state: `prefetch*`, `sfence` no-ops; `fnsave`/`frstor` image/commit | 400 | 0.2% | 99 | prefetch yes (trivial); `fnsave` on use |
| 10 | min/max; `rcp`/`rsqrt` (simd) | 238 | 0.1% | 83 | **yes** (`rsqrtss` in normalization) |
| 11 | Control and mode: `fldcw` (normalising), `fnstcw`, `fwait` with #MF, `ldmxcsr`/`stmxcsr` | 123 | 0.1% | 89 | **yes**: `_control87` runs in `platform_bootstrap` |

Cross-cutting pieces have no instruction count of their own:

| piece | covers | menu |
|---|---|---|
| Event 14 (#MF) in runtime `events.rs`, `h2abi.h`, `h2gen.h` (FLOAT-ABI.md §2.2): built | every x87 row | **yes** (the path must exist, even if masked) |
| FIP/FDP/FOP materialisation at block exits, before env ops, on the #MF path | rows 2, 9, 11 | **yes** |
| Rust overrides for CRT FP helpers (`_ftol2`, `_control87`/`_controlfp`, `floor`/`ceil`, `_CIpow`, `_CIfmod`, `_CI*` dispatchers, `_clearfp`/`_statusfp`) | speed and simpler stacks; not correctness | `_ftol2` first; rest on demand |
| `h2 fpstats` stack walk, `Proven` grade | input to the §2.8 fast path only | no |
| `double` fast path under §2.8 | speed in proven-PC53 regions | no |

First fixtures:

- `_control87` at `platform_bootstrap`, including the `0x023F` → `0x027F`
  read-back;
- `_ftol2`, with NaN and ±2^63;
- an `fcomp`/`fnstsw`/`test ah` branch;
- `fcomip` with `eflags_defined`;
- `cvttss2si` overflow;
- `comiss` with a NaN;
- an unmasked-IE stack overflow followed by `fwait`, which raises event 14.

## Unknowns

- **The thread-start x87 control word and MXCSR on Xbox.** The kernel starts
  at `0x027F` (`CpuAbi::xbox_thread`), and the game forces `0x027F` early,
  but CRT startup code runs before that.
- **What actually executes before the first menu frame.** Replace the static
  bounds in §1.6 with a trace once boot reaches it.
- **Behaviour that needs hardware traces:**
  - the PIII `rcp`/`rsqrt` tables;
  - transcendental bit patterns (the 66-bit-π reduction);
  - the `rcp` tiny-flush window;
  - C1, the undefined condition codes, and control-word bit 6 read-back.
- **Transcendental speed.** The opt-in guarded log/exp/atan path and its
  measured costs are documented in [x87-transcendentals.md](x87-transcendentals.md).
  Lower-precision trigonometric argument reduction remains open.
- **NaN payload bits.** Whether any game code depends on them. The design
  preserves them regardless.
