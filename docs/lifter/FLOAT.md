# Floating point in the lifter

Status: Built. Every x87, SSE and MMX instruction the image uses is lowered; the exact paths are the reference and the native paths are guarded fast paths.
Code: `crates/x87`, `crates/simd`, `crates/lift/src/lower/x87.rs`, `crates/lift/src/lower/sse.rs`, `crates/emit/src/x87.rs`, `crates/emit/c/h2x87_native.h`, `crates/emit/c/h2sse_native.h`. Tests: `crates/lift/tests/oracle_x87/`, `crates/emit/tests/differential.rs`.
Switch: `H2_X87_NATIVE`, `H2_SSE_NATIVE`, listed in [switches.md](../runtime/switches.md).

Halo 2 is an SSE1 game with an x87 sidecar. Every floating-point instruction is lowered for bit-exactness first, and no host floating point or libm decides a guest-visible bit. The interface is [FLOAT-ABI.md](FLOAT-ABI.md); the census of what the retail image uses is [notes/fp-census.md](notes/fp-census.md).

## The paths

- **Exact x87 helpers.** `crates/x87` is integer ext80 arithmetic, exact under every precision and rounding control; the generated C calls `h2x87_v1_*`. This is the authority and the path when `H2_X87_NATIVE=0`. [FLOAT-ABI.md](FLOAT-ABI.md).
- **Guarded native x87.** `h2x87_native.h` runs add, subtract, multiply, divide, square root and integer loads in host binary64 when a value-level proof holds (PC=53, round-to-nearest, exceptions masked, operands exactly representable), and calls the helper otherwise. On by default. [x87-native.md](x87-native.md).
- **Transcendentals.** `fsin`, `fcos`, `fptan`, `fpatan`, `fyl2x`, `fyl2xp1`, `f2xm1` and `fsincos` evaluate at 512 bits and round once; the `fast-trans` feature tries a guarded 128-bit pass first. [x87-transcendentals.md](x87-transcendentals.md).
- **SSE reference.** `crates/simd` is integer soft-float for every SSE1 form, with MXCSR, NaN payloads and `#XM` exact; `h2simd.h` is its C side. [simd.md](simd.md), [sse-oracle.md](sse-oracle.md).
- **Native SSE.** `h2sse_native.h` runs guarded SSE arithmetic on the host's vector unit and keeps MXCSR in guest state. On by default. [sse-native.md](sse-native.md).

## 1. What the game uses

The measured mix, compare idioms, x87 stack depths and the reachability bounds are in [notes/fp-census.md](notes/fp-census.md). The facts the design relies on:

- The game uses SSE1 and x87 only; no SSE2 or later and no 3DNow!. MMX is confined to third-party video and audio code.
- Nearly every x87 function has one static stack depth, but the lifter does not depend on that: the stack is architectural state (section 2.2).
- The retail image has 29 80-bit memory operands, all in CRT transcendental helpers.

### 1.2 Control words and rounding

- Every `fldcw` is in CRT code. Game code reaches it through `_control87`, which the platform bootstrap calls first, with precision 53, round-to-nearest and all exceptions masked: control word `0x027f`. `CpuAbi::xbox_thread()` starts threads at the same word (`crates/runtime/src/cpu.rs`).
- Precision 64 and the directed rounding modes appear only inside CRT `floor`, `ceil` and the transcendental dispatchers.
- Float-to-int is `_ftol2`: `fistp` at round-to-nearest, then a correction toward zero.
- Every MXCSR write sets the exception masks or clears the sticky flags. None sets FTZ, DAZ or a rounding mode, so SSE runs with the thread default `0x1f80` and denormals are live.

## 2. Design

### 2.1 Representation of x87 values

The x87 state is `X87Abi` inside `CpuAbi`: eight physical registers as ten bytes, control, status (holding TOP), tag, opcode, FIP/FDP and selectors. It is the only representation; every instruction lowers to a call into `crates/x87` or to a guarded native path that produces the same bits. `long double` is never used (on Apple arm64 it is `double`). The transcendentals compute at full 64-bit precision under round-to-nearest only and stop with `Unmodelled` under another rounding mode; for |x| of 2^63 or more, `fsin`, `fcos` and `fptan` set C2 and leave the operand. They use the exact value of pi, not the Pentium III's 66-bit one.

### 2.2 The x87 stack in the IR

The IR names registers as `St(i)` relative to TOP, as the instruction does (`crates/ir/src/x87.rs`); the helpers maintain TOP and the tags and raise stack faults. No stack-depth analysis is needed for correctness. MMX aliasing lives in `h2gen.h`: an MMX write stores 64 bits and `0xffff` in the top two bytes, MMX entry marks every tag valid, `emms` marks them empty, and every MMX instruction first checks for a pending #MF (`h2i_x87_wait`).

### 2.3 Precision control

Precision control is exact at every setting: the significand is rounded to 24, 53 or 64 bits with the full 15-bit exponent range kept. It governs add, subtract, multiply, divide and square root, and not loads, stores, constants, `frndint`, `fscale`, `fxtract`, `fprem` or the transcendentals. `fldcw` is `h2x87_v1_ldcw`, which stores `(cw & 0x1f3f) | 0x40`. FLOAT-ABI.md sections 2.3 and 4.6.

### 2.4 Rounding modes and the CRT

Every rounding x87 operation honours every rounding mode (the transcendentals excepted, section 2.1). `fist` and `fistp` round by the guest mode and give the integer indefinite with IE for NaN or out-of-range values. `fisttp` is SSE3 and raises #UD. Rust overrides of CRT helpers (`_ftol2`, `_control87`, `floor`, `ceil` and others) are a speed choice, not a correctness need; `_ftol2`'s contract is FLOAT-ABI.md section 4.5.

### 2.5 Exceptions and #MF

The game masks every x87 exception, but the model does not depend on that.

- Pre-computation exceptions (IE, stack faults, DE, ZE), when unmasked, set only the status word; #MF (event 14, vector 16, `arg` = status word) is delivered at the next waiting x87 instruction, `fwait` or MMX instruction, with `saved_eip` at that instruction and FIP/FOP/FDP describing the raiser. Priority follows SDM Vol. 1 section 4.9.2.
- Post-computation exceptions (OE, UE, PE), when unmasked, stop with `Unmodelled` (event 12).
- Sticky flags are always exact, so `fnstsw`, `_statusfp` and `and ax, 0x20` see true values.

Details: FLOAT-ABI.md section 2; the event table is `crates/emit/RUNTIME-CONTRACT.md` section 1.3.

### 2.6 `fnstsw` and `sahf` flag use

`h2x87_v1_cmp` writes C3/C2/C0 into the status word on both OK and SUPPRESS, so `fnstsw ax; test ah, M; jcc` and `fnstsw ax; sahf; jcc` need no fusion: `fnstsw` is a field read and the following integer instructions work on AX (`crates/lift/src/lower/x87.rs`). `fcomi`, `fcomip` and `fucomip` write ZF/PF/CF from the returned bits, clear OF/SF/AF and mark all six flags defined (FLOAT-ABI.md section 4.4). `fcom` and `fucom` differ in raising IE on a quiet NaN, and the helper models that.

### 2.7 SSE

SSE is defined by `crates/simd` ([simd.md](simd.md)): results, NaN payloads, signed zeros, MXCSR sticky flags and `#XM` are exact, with no host floating point. `rcp` and `rsqrt` have no Pentium III table here, so they are held to the architectural error bound.

### 2.8 Proof rule for the native `double` path

The native path ([x87-native.md](x87-native.md)) keeps the architectural ext80 stack and applies a value-level proof per operation: operands exactly representable as binary64, PC=53, round-to-nearest, all exceptions masked, no pending #MF, and a guard on the result for overflow, tiny results and NaNs, with a helper recompute on the rare path. Constants like `fldpi`, every transcendental, `fld m80` and anything under another precision or rounding mode use the helpers.

### 2.9 How the oracle checks FP

`crates/oracle` runs synthetic fixtures in Unicorn (QEMU 5.0.1 core) and `crates/sse-oracle` runs SSE against hardware ([oracle.md](oracle.md), [sse-oracle.md](sse-oracle.md)). Unicorn is evidence for x87 add, subtract, multiply, divide and square root, which use SoftFloat extended precision, for `minps`/`maxps` and for the conversion indefinite. It is not evidence for the transcendentals (it uses host libm), x87 sticky exception flags other than ZE, C1 and the undefined condition codes, control-word bit 6, MXCSR sticky flags or `rcp`/`rsqrt` bits.

The comparison policy (`crates/lift/tests/oracle_x87/`):

1. Bit-exact: every ext80 and XMM byte, the control word except bit 6, C0-C3 where the SDM defines them, TOP, the tags, EFLAGS from compares, memory and integer results, and event 14 with its `arg`.
2. Masked by mnemonic, never a blanket ignore: x87 sticky bits and MXCSR bits 0-5 where Unicorn does not model them, C1 and the undefined condition codes, FOP, FCS and FDS (and FIP/FDP after control instructions), and control-word bit 6. `crates/x87` is the authority for masked bits.
3. Tolerance by name: the transcendentals against `crates/x87`'s 512-bit result (cross-checked at 768 bits), with C2 and out-of-range behaviour exact; cases near multiples of pi where the Pentium III's 66-bit reduction differs are labelled `pi66`.

Fixture families cover signed zeros, denormals, infinities, quiet and signalling NaNs with distinct payloads, round-to-even ties, the three precisions, conversion extremes, every compare-and-branch combination with unordered operands, and unmasked-exception ordering.

## Evidence

- **Code**: the paths above; the exact x87 and SSE paths are the reference.
- **Test**: `crates/lift/tests/oracle_x87/` (lowering against Unicorn), `crates/emit/tests/differential.rs` (interpreter against generated C).
- **S**: PC=53 and mask-all being the thread-start state on hardware; verified by an Xbox or xemu trace.

## Not verified

- The thread-start x87 control word and MXCSR on Xbox (`CpuAbi::xbox_thread` is a provisional policy; CRT startup runs before the game forces `0x027f`).
- What executes before the first menu frame; the census has static bounds only.
- Behaviour that needs hardware traces: the Pentium III `rcp`/`rsqrt` tables, transcendental bit patterns, the `rcp` tiny-flush window, C1, the undefined condition codes and control-word bit 6.
- Whether any game code depends on NaN payload bits (the design preserves them).

## Related

[FLOAT-ABI.md](FLOAT-ABI.md), [x87-native.md](x87-native.md), [x87-transcendentals.md](x87-transcendentals.md), [simd.md](simd.md), [sse-native.md](sse-native.md), [oracle.md](oracle.md), [notes/fp-census.md](notes/fp-census.md).
