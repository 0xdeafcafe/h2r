# Floating-point census of the retail image

Evidence for [FLOAT.md](../FLOAT.md).

## 2026-11-21

At `b292ee619`, static reading with `h2 disasm 0x339acf 5` and
`h2 disasm 0x339e9f 5` on the owner's retail XBE identifies two
read-modify-write sequences that set FTZ. This corrects the FTZ conclusion
in the November 15 census. Entry histograms can establish which control
words actually reach a collision query. [All claimed status-save sites](../../engine/collision-queries.md#status-instruction-census)

## 2026-11-15

Measured with `h2 fpstats` ([crates/recompiler/analyze/src/fpstats.rs](../../../crates/recompiler/analyze/src/fpstats.rs)) over the retail `default.xbe` (SHA-256 `03215919...935d`), at commit 1ec714ed0 (the commit that added the tool; the tool's output has not been rerun since this note was written). Reproduce with `cargo h2 fpstats [disc] [--root 0x12190]`. The tool is static: it counts what the analyzer decoded and claimed, which is not what executes. `--root` adds start points for the reachability split; `0x12190` is `game_main` in [engine-map.md](../../engine/engine-map.md).

### What the game uses

Run it with:

```sh
cargo h2 fpstats [disc] [--root 0x12190]
```

`--root` adds start points for the reachability split. `0x12190` is
`game_main` in [engine-map.md](../../engine/engine-map.md).

#### Mix (measured)

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

#### Control words and rounding (measured)

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

**MXCSR.** The census counted 23 `ldmxcsr`, 23 `stmxcsr` and zero `fxrstor`.
Its original classification missed two writes that enable FTZ. The other
21 writes mask exceptions or clear sticky flags. [Corrected classification](#2026-11-21)

#### Compare idioms (measured)

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

#### Transcendentals and 80-bit (measured)

`fsqrt` 564, `fcos` 273, `fsin` 249, `fpatan` 88, `fyl2x` 55, `fscale` 31,
`frndint` 30, `f2xm1` 25, `fptan` 19, `fsincos`/`fprem`/`fxtract` 1 each.
`fsin`/`fcos`/`fpatan` are mostly inline in game code; `fyl2x`/`f2xm1`/`fscale`
sit in CRT `pow`/`exp`/`log`. Over SSE: `sqrtss` 37, `rsqrtss` 118, `rsqrtps` 14,
`rcpss` 9.

All 29 80-bit memory operands (`fld m80` 25, `fstp m80` 4) are in 19 CRT
functions in `0x322bf5–0x37379a` (constants and spills inside transcendental
helpers). Game code never touches an 80-bit value in memory.

#### x87 stack depth (measured)

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

#### What runs before the first menu frame (measured lower and upper bounds)

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


### Instruction share by lowering, and menu relevance

The rows are ranked by their share of the 171,806 measured FP instructions.
"Reached" is the set of instructions reached by direct calls from the entry
point and `game_main` ("What runs before the first menu frame"), a lower bound on what runs. The **menu** column
says whether the first mainmenu frame needs the row. There is no trace yet,
so that column is a judgement from "What runs before the first menu frame" and engine-map.md, not a measurement.

| # | lowering | insns | share | reached | menu |
|---:|---|---:|---:|---:|---|
| 1 | SSE scalar moves and arithmetic through `crates/recompiler/simd` (`movss`, `addss`, `subss`, `mulss`, `divss`, `sqrtss`) | 82,019 | 47.7% | 50,600 | **yes** |
| 2 | x87 through the `crates/recompiler/x87` helpers on `X87Abi`: loads, stores (three-phase), arithmetic, `fxch`, constants, `fsqrt` | 37,375 | 21.8% | 16,580 | **yes** |
| 3 | SSE packed moves, shuffles and bit logic (`movaps`, `shufps`, `xorps`, `movlps`/`movhps`, `andps`, `movmskps`, ...) | 27,530 | 16.0% | 10,180 | **yes** |
| 4 | Compares and flag fusion: `comiss`/`ucomiss`, `fcomip` (with `eflags_defined`), `fcomp`+`fnstsw`+`test ah`, `sahf`, `ftst`, `cmpss`/`cmpps` | 10,786 | 6.3% | 5,502 | **yes** |
| 5 | SSE packed arithmetic (`mulps`, `addps`, `subps`) | 6,895 | 4.0% | 1,303 | **yes** (matrix/vector paths) |
| 6 | MMX and SSE-on-MMX integer ops, with the `h2gen.h` aliasing and #MF check | 2,936 | 1.7% | 146 | no: Bink/WMA/HLE'd D3D; stop on use |
| 7 | Conversions with x86 indefinite: `cvtsi2ss`, `cvttss2si` (simd), `fistp` (x87, RC-exact), `cvtpi2ps`/`cvtps2pi` | 2,731 | 1.6% | 1,491 | **yes** (`_ftol2` has 214 callers) |
| 8 | x87 transcendentals through `crates/recompiler/x87` (`fsin`, `fcos`, `fpatan`, `fyl2x`, `f2xm1`, `fscale`, `frndint`, ...) | 773 | 0.4% | 453 | **yes**: camera/orientation math is inline |
| 9 | Hints and state: `prefetch*`, `sfence` no-ops; `fnsave`/`frstor` image/commit | 400 | 0.2% | 99 | prefetch yes (trivial); `fnsave` on use |
| 10 | min/max; `rcp`/`rsqrt` (simd) | 238 | 0.1% | 83 | **yes** (`rsqrtss` in normalization) |
| 11 | Control and mode: `fldcw` (normalising), `fnstcw`, `fwait` with #MF, `ldmxcsr`/`stmxcsr` | 123 | 0.1% | 89 | **yes**: `_control87` runs in `platform_bootstrap` |

Cross-cutting pieces have no instruction count of their own:

| piece | covers | menu |
|---|---|---|
| Event 14 (#MF) in runtime `events.rs`, `h2abi.h`, `h2gen.h` (FLOAT-ABI.md §2.2): built | every x87 row | **yes** (the path must exist, even if masked) |
| FIP/FDP/FOP materialisation at block exits, before env ops, on the #MF path | rows 2, 9, 11 | **yes** |
| Rust overrides for CRT FP helpers (`_ftol2`, `_control87`/`_controlfp`, `floor`/`ceil`, `_CIpow`, `_CIfmod`, `_CI*` dispatchers, `_clearfp`/`_statusfp`) | speed and simpler stacks; not correctness | `_ftol2` first; rest on demand |
| `h2 fpstats` stack walk, `Proven` grade | input to the FLOAT.md §2.8 fast path only | no |
| `double` fast path under FLOAT.md §2.8 | speed in proven-PC53 regions | no |

First fixtures:

- `_control87` at `platform_bootstrap`, including the `0x023F` → `0x027F`
  read-back;
- `_ftol2`, with NaN and ±2^63;
- an `fcomp`/`fnstsw`/`test ah` branch;
- `fcomip` with `eflags_defined`;
- `cvttss2si` overflow;
- `comiss` with a NaN;
- an unmasked-IE stack overflow followed by `fwait`, which raises event 14.


### Fixture counts of the lowering cross-check

The corpus was 37,207 fixtures when last counted: the original 21,152, plus 16,055 covering every SSE and MMX form the image uses. It gave 36,293 matches, 0 differences and 914 skips. These counts were taken before x87 lowering was complete, so the x87 skips in the table should now be matches and the totals may differ. Generate the fixtures with `cargo run -p oracle --features unicorn --bin oracle-fixtures` ([oracle.md](../oracle.md)) and reproduce with `H2_ORACLE_FIXTURES=<files> cargo test -p lift --features oracle-fixtures --test oracle -- --nocapture`, which prints the skip classes.

| skipped | why |
|--------:|-----|
| 330 | 16-bit address-size string ops (`67` prefix, CX/SI/DI): not lowered |
| 259 | BCD and `salc` (`aaa`, `aad`, `aam`, `aas`, `daa`, `das`, `salc`): not lowered |
| 185 | refused by the oracle (`popfd`, `pause`) |
| 66  | x87 arithmetic (`fld1`, `faddp`): skipped before x87 was lowered |
| 37  | `cmpxchg8b`: not lowered |
| 37  | 16-bit addressing (`lea` with `67`): not lowered |
