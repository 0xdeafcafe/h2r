# Floating-point IR ops and runtime ABI

2026-11-15. Status: `crates/x87` implements §4 and §6. Event 14 (#MF) is in
the runtime, `h2abi.h` and `h2gen.h`, and MMX instructions and `emms` check
for it (§2.2). The IR, lifter and emitter (`crates/ir`, `crates/lift`,
`crates/emit`) lower every x87 instruction to these helpers (e5c4251). Revised after
the design review (APPROVE WITH CHANGES): the helpers run on master's
`X87Abi`, #MF has an event code, and stores and environment operations are
split into phases. Revised again after the re-review (COMMIT AFTER FIXES):
exception priority, unmasked exact underflow, FNSTENV discarding a pending
#MF, an explicit status for suppressed compares, and FSCALE normalisation.

This is the interface the lifter and emitter build against for x87 floating
point. It follows the correctness-first decision in [FLOAT.md](FLOAT.md). The
IR models the full architectural x87 state. Every x87 operation goes through a
runtime helper backed by the exact ext80 implementation in `crates/x87`. A
`double` fast path may come later, but only under the written proof rule in
FLOAT.md §2.8. SSE is not covered here: see §5.

The C ABI is versioned `h2x87_v1_*`, declared in
[`crates/x87/include/h2x87.h`](../crates/x87/include/h2x87.h). The Rust crate is
`x87::`, with the safe layer in `x87::abi` and the `extern "C"` wrappers in
`x87::abi::ffi`. Every C entry point is `extern "C"` and takes no Rust
references across the boundary. A null pointer or an out-of-range selector is a
generator bug: the helper panics, and because an `extern "C"` function cannot
unwind, the process aborts.

## 1. State

The helpers operate on the runtime's `X87Abi`, which `CpuAbi` v1 embeds by
value at offset 56 (h2abi.h and `crates/runtime/src/cpu.rs` are the
authority):

```c
typedef struct X87Abi {
    uint8_t  st[8][10];                    // physical registers R0..R7, guest byte order
    uint16_t control, status, tag, opcode; // FCW, FSW (TOP in bits 11..13), FTW (2 bits/reg), FOP
    uint32_t instruction_pointer, data_pointer;     // FIP, FDP
    uint16_t instruction_selector, data_selector;   // FCS, FDS
} X87Abi;  // 100 bytes, align 4
```

- `st[p]` is physical register `p` in the guest's 10-byte memory format:
  little-endian 64-bit significand, then little-endian sign and exponent.
  `ST(i)` is `st[(TOP + i) & 7]`.
- **TOP exists only in `status` bits 11..13.** There is no unpacked copy and
  no sync step. `FNSTSW` is a plain field read.
- `tag` holds the full two-bit Intel encoding: 00 valid, 01 zero, 10 special,
  11 empty. The helpers keep it exact.
- XMM and MXCSR live in `CpuAbi`, outside `X87Abi`, and belong to
  [simd.md](simd.md).
- MMX aliasing belongs to `h2gen.h` (lifter-land), not to this crate. An MMX
  write stores bytes 0..7 of `st[n]` and `0xffff` in bytes 8..9, and
  `h2i_mmx_enter` sets every tag valid and clears TOP in `status`. `EMMS` sets
  `tag = 0xffff`. MMX instructions and `EMMS` are waiting instructions, so
  both check for a pending #MF first (§2.2).

The crate's `#[repr(C)] x87::abi::X87Abi` is the same struct. A test checks
the size, the alignment and every offset, and `h2x87.h` repeats the checks as
`_Static_assert`s.

## 2. Status, faults and #MF

```c
#define H2X87_OK          0x00000000u
#define H2X87_MF          0x01000000u
#define H2X87_UNMODELLED  0x02000000u
#define H2X87_SUPPRESS    0x03000000u   // stores and h2x87_v1_cmp only
#define H2X87_STATUS_MASK 0xff000000u
```

Every helper returns `uint32_t`. The top byte is a status and the low 24 bits
are a payload, which only `h2x87_v1_cmp` uses (§4.4).

### 2.1 What the hardware does, and what is modelled

On a P6, an x87 exception that the control word does not mask is not reported
by the instruction that raises it. That instruction sets the exception flag, ES
and B, and #MF (vector 16) is delivered at the **next waiting** x87
instruction, `FWAIT`, or MMX instruction. The exceptions come in two kinds:

- **Pre-computation: IE (including stack faults, IE+SF), DE and ZE.** The
  instruction writes no result, does not change TOP, and leaves its
  destination unwritten. **Modelled exactly.** The helper returns `H2X87_OK`
  (stores and compares: `H2X87_SUPPRESS`, §4.2, §4.4) having changed only
  `status`: the pre-computation flags it detected, ES, B, and C1 for a stack
  fault (1 for overflow, 0 for underflow). The computation did not happen,
  so no OE/UE/PE it would have raised is recorded: `fadd m32` of a binary32
  denormal to 1.0 with DE unmasked sets DE alone, not DE+PE.
- **Post-computation: OE, UE and PE.** The hardware stores a result (an
  exponent-wrapped value for OE/UE, or the rounded value for PE) and then sets
  ES. **Not modelled.** If the instruction's unmasked exceptions are only
  OE/UE/PE, the helper changes nothing and returns `H2X87_UNMODELLED`, and the
  generated code stops with `H2SHIM_UNMODELLED` (code 12). If a
  pre-computation exception is unmasked as well, the pre-computation rule
  wins, as on hardware.

**Priority.** An instruction reports the highest-priority exception it
detects and discards the lower ones, except that a masked DE lets execution
continue to the lower ones (SDM Vol. 1 §4.9.2):

1. IE from an SNaN or unsupported operand (and stack faults, before any
   operand is examined);
2. a QNaN operand (not an exception, but it decides the result);
3. any other IE, or ZE;
4. DE;
5. OE or UE, possibly with PE;
6. PE.

So a denormal divided by zero is ZE alone, whatever DM says: with only DE
unmasked it is a masked ZE, giving ±∞ and nothing pending. The same holds for
a memory operand that is denormal in its own format, and for `fyl2x`/
`fyl2xp1` with x giving log2(0). The crate's operations return flags already
reduced by this order; the ABI then applies the masks.

**Underflow with UE unmasked.** Masked, UE is raised only for a result that is
tiny *and* inexact; an exact denormal result raises nothing. Unmasked, UE is
raised whenever the result is tiny, exact or not: with UM clear "the
underflow exception is reported when the result is tiny, regardless of
inexactness" (SDM Vol. 1 §4.9.1.5; §8.5.5 gives the x87 responses). For
stores, FST/FSTP's description adds that storing a denormal raises no #D;
it "is simply signaled as a numeric underflow exception (#U) condition",
which is the same rule seen from the store side. Tininess is detected after
rounding with an unbounded exponent, at the PC width for arithmetic and the
format's width for stores.
The crate reports it as `Flags::TINY`, which never reaches the status word;
the ABI treats it as UE when UM is clear. What hardware does next differs by
destination (§8.5.5): a register destination gets the result scaled by
2^24576 and then faults, and a memory destination is left unwritten with the
stack unchanged. Neither is modelled: both return `H2X87_UNMODELLED` with the
state unchanged. With `CW = 0x036f`, `2^-16382 * 0.5` and an `fst m32` of
exactly 2^-149 both stop.

Measured, the game masks every exception (FLOAT.md §1.2), so neither path
should execute. Both must exist rather than be assumed away.

With an exception masked, the masked response is applied and committed
normally. A stack overflow or underflow produces the real indefinite
(`se = 0xffff`, `sig = 0xc000_0000_0000_0000`) and sets IE+SF and C1.

### 2.2 Delivering #MF: event code 14

Every waiting helper starts by checking ES. If ES is set, the helper does
nothing and returns `H2X87_MF`. Generated code then calls

```c
h2shim_v1_event(ctx, pc, /*saved_eip*/ pc, H2SHIM_FP_ERROR, /*arg*/ x87.status);
```

- `pc` and `saved_eip` are both **the waiting instruction**: #MF is a fault on
  that instruction.
- FIP, FOP and FDP must describe **the raising instruction**, not the waiting
  one (§2.4).
- `h2x87_v1_wait` is the bare check, used for `FWAIT` and before
  FLDENV/FRSTOR read guest memory. MMX instructions and `EMMS` make the same
  check in `h2gen.h` (below).

The non-waiting forms never check ES: `FNSTSW`, `FNSTCW`, `FNCLEX`, `FNINIT`,
`FNSTENV` and `FNSAVE`. An `FWAIT`-prefixed `FSTSW` and friends decode as
`FWAIT` followed by the non-waiting form.

The only path is the `pending()` check at the next instruction. No helper
raises #MF for itself.

**Where event 14 lives.** It is token-identical in every header, so they can
be included together:

1. **`crates/runtime/include/h2abi.h`** and **`crates/emit/c/h2gen.h`**:
   `#define H2SHIM_FP_ERROR 14u`. `h2x87.h` takes it from `h2abi.h` and
   `_Static_assert`s its value.
2. **`crates/runtime/src/events.rs`**: `ShimEvent::FP_ERROR = 14` decodes to
   `GuestException::FpError { status: u16 }`, a fault (not a trap), with the
   status word as `arg`. An `arg` above 0xFFFF cannot be a status word and
   decodes as `UnknownShimEvent`, so a broken generator cannot continue
   silently.
3. **MMX and `EMMS`** (`crates/ir`, `crates/lift`, `h2gen.h`). A pending
   exception stops an MMX instruction before it reads, writes or faults. The
   lifter therefore puts `Intrinsic::X87Wait` (`h2i_x87_wait`) first in every
   instruction that ends with `Intrinsic::MmxEnter`. `h2i_mmx_enter` stays
   last and does not check, because by then the instruction's writes are
   done. `Intrinsic::Emms` (`h2i_emms`) makes the check itself before
   emptying the tags. Both raise `Fault::FpError`, which the emitter turns
   into event 14 with `arg` = the status word (`H2_INTRINSIC_ARG`). The IR
   interpreter does the same, and the emitter's differential test covers
   both paths. `X87Wait` is also the lowering for `fwait` once the x87
   lowering lands.

Delivering vector 16 into the guest (the Xbox kernel turns it into a
`STATUS_FLOAT_*` code from `status & ~control`) is the runtime's job, as for
#XM. The oracle compares event 14 like any other guest exception: `pc`,
`saved_eip`, and `arg`.

### 2.3 Control word normalisation

`FLDCW`, `FLDENV` and `FRSTOR` store `(cw & 0x1f3f) | 0x40`. Reserved bits
13..15 and bit 7 read back 0, and bit 6 reads back 1, as on a P6 (Bochs forces
bit 6 too). The CRT translator at `0x3211fe` never sets bit 6, so the game's
`fldcw` writes `0x023F`, and this rule reads it back as `0x027F`.

Unicorn/QEMU stores the raw word, so the oracle masks control-word bit 6 and
tags that check `needs-hardware`.

After `FLDCW`, `FLDENV` and `FRSTOR`, ES and B are recomputed: they are set
iff some exception flag in `status` bits 0..5 is unmasked. They therefore
become set when an unmasked word meets a pending flag, and clear when
everything is masked.

### 2.4 FIP, FDP, FOP, FCS, FDS

The helpers never write these fields, except that `FNINIT` zeroes them and
`FLDENV`/`FRSTOR` load them. The emitter materialises them:

- **At block exits and before `FNSTENV`/`FNSAVE`**, it stores the values of
  the last non-control x87 instruction executed in the block:
  - FIP: its guest address.
  - FOP: the low 11 bits of its opcode.
  - FDP: its memory operand's address, or unchanged for a register form.
  - FCS and FDS: the guest selectors.
- **On the `H2X87_MF` path**, before raising event 14, it stores the same
  values for the most recent non-control x87 instruction before the faulting
  one. That instruction is the raiser: any waiting instruction in between
  would have faulted first. Inside a block the raiser is known statically.
  FDP is dynamic, so the emitter keeps the last memory operand address in a
  local. Across blocks the values were already materialised at the exit.

They are observable only through `fnsave` (2 CRT sites), `fnstenv` (0 sites),
and #MF. The oracle masks them per mnemonic, except for those three.

## 3. x87 IR ops

The IR keeps the x87 stack as architectural state: one `X87` value in the CPU
state, not per-slot SSA values. Operations name registers the way the
instruction does, `St(i)` relative to TOP. The lifter does not need a depth
analysis for correctness (§7).

| IR op | operands | lowers to |
|---|---|---|
| `X87Load { from: F32 \| F64 \| F80 \| I16 \| I32 \| I64 \| Bcd, addr }` | guest address | guest read, then `h2x87_v1_ld_*` |
| `X87LoadConst { which: One \| Z \| Pi \| L2E \| L2T \| Lg2 \| Ln2 }` | — | `h2x87_v1_ld_const` |
| `X87LoadReg { src: St }` | — | `h2x87_v1_ld_st` |
| `X87Store { to: F32 \| F64 \| F80 \| I16 \| I32 \| I64 \| Bcd, addr, src: St, pop }` | | `h2x87_v1_st_*`, then guest write, then `h2x87_v1_store_commit` (§4.2) |
| `X87StoreReg { dst: St, pop }` | | `h2x87_v1_st_st` |
| `X87Arith { op: Add \| Sub \| SubR \| Mul \| Div \| DivR, dst: St, src: Src, pop }` | `Src = St(i) \| Mem{F32,F64,I16,I32}` | guest read, then `h2x87_v1_arith` |
| `X87Unary { op: Sqrt \| Abs \| Chs \| RndInt \| Sin \| Cos \| Tan \| Patan \| Yl2x \| Yl2xp1 \| X2m1 \| Scale \| Xtract \| SinCos \| Prem \| Prem1 }` | — | `h2x87_v1_unary` |
| `X87Cmp { kind: Com \| Ucom \| Icom, src: Src, pop: 0\|1\|2, to_flags }` | | `h2x87_v1_cmp` |
| `X87Xam` | — | `h2x87_v1_xam` |
| `X87Xch { other: St }` | — | `h2x87_v1_xch` |
| `X87Free { reg: St, pop }` | — | `h2x87_v1_free` |
| `X87IncStp` / `X87DecStp` | — | `h2x87_v1_adj_top(+1 / -1)` |
| `X87Cmov { cc, src: St }` | — | `h2x87_v1_cmov` (cc evaluated by the caller) |
| `X87LoadCw { value: u16 }` / `X87StoreCw` | | `h2x87_v1_ldcw` / field read |
| `X87StoreSw { to: Ax \| Mem }` | | field read (TOP is already in it) |
| `X87ClEx` / `X87Init` | — | `h2x87_v1_clex` / `h2x87_v1_init` |
| `X87SaveEnv { op: StEnv \| Save, addr }` | | `h2x87_v1_save_image`, guest write, `h2x87_v1_save_commit` |
| `X87LoadEnv { op: LdEnv \| Rstor, addr }` | | `h2x87_v1_wait`, guest read, `h2x87_v1_load_env` |
| `X87Wait` | — | `h2x87_v1_wait` |
| `X87Ftol2` (CRT override only) | — | `h2x87_v1_ftol2` |

Decoder mappings with no op of their own:

- **`ftst`** (6 sites) is `X87Cmp { kind: Com, src: Mem F32 with the constant
  +0.0 (src = 0), pop: 0, to_flags: false }`. This is exact, because comparing
  with +0.0 from binary32 is the same operation, and +0.0 is not denormal, so
  it cannot set DE.
- **`fneni`, `fndisi` and `fsetpm`** are no-ops on a P6 (8087/287
  leftovers) and lower to nothing. **`fnop`** does nothing either, but it is a
  waiting instruction, so it lowers to `X87Wait`.
- **`fisttp` is not a P6 instruction** (SSE3). The decoder must raise #UD for
  it. `h2x87_v1_st_i32_trunc` exists only for CRT overrides.
- **`fcomi`/`fucomi`/`fcomip`/`fucomip`** are `X87Cmp { to_flags: true }`. On
  `H2X87_OK` the generated code replaces ZF/PF/CF with the returned bits; on
  `H2X87_SUPPRESS` it keeps them. Either way it clears OF/SF/AF and marks all
  six defined in `eflags_defined` (§4.4).

The IR keeps the comparison and the flag transfer as separate ops, so the
`fnstsw`/`test ah`/`jcc` fusion (FLOAT.md §2.6) is a peephole over the IR
rather than a decoder special case. Fusion must leave `status` correct
whenever it is observable.

## 4. x87 runtime ABI

The full declarations are in `h2x87.h`. In outline:

```c
uint32_t h2x87_v1_ld_f32  (X87Abi *s, uint32_t bits);
uint32_t h2x87_v1_ld_f64  (X87Abi *s, uint64_t bits);
uint32_t h2x87_v1_ld_f80  (X87Abi *s, const uint8_t m[10]);
uint32_t h2x87_v1_ld_i16  (X87Abi *s, int16_t v);
uint32_t h2x87_v1_ld_i32  (X87Abi *s, int32_t v);
uint32_t h2x87_v1_ld_i64  (X87Abi *s, int64_t v);
uint32_t h2x87_v1_ld_bcd  (X87Abi *s, const uint8_t m[10]);
uint32_t h2x87_v1_ld_const(X87Abi *s, uint32_t which);   // X87LoadConst order
uint32_t h2x87_v1_ld_st   (X87Abi *s, uint32_t i);

uint32_t h2x87_v1_st_f32(X87Abi *s, uint32_t i, uint32_t *value, uint16_t *sw);
/* ... st_f64, st_f80, st_i16, st_i32, st_i64, st_bcd, st_i32_trunc ... */
uint32_t h2x87_v1_store_commit(X87Abi *s, uint16_t sw, uint32_t pop);
uint32_t h2x87_v1_st_st(X87Abi *s, uint32_t dst, uint32_t pop);
uint32_t h2x87_v1_ftol2(X87Abi *s, uint64_t *out);

uint32_t h2x87_v1_arith(X87Abi *s, uint32_t op, uint32_t dst_i, uint32_t src_kind, uint64_t src);
uint32_t h2x87_v1_unary(X87Abi *s, uint32_t op);
uint32_t h2x87_v1_cmp  (X87Abi *s, uint32_t kind, uint32_t src_kind, uint64_t src,
                        uint32_t pop, uint32_t to_flags);
uint32_t h2x87_v1_xam  (X87Abi *s);

uint32_t h2x87_v1_xch    (X87Abi *s, uint32_t i);
uint32_t h2x87_v1_free   (X87Abi *s, uint32_t i, uint32_t pop);
uint32_t h2x87_v1_adj_top(X87Abi *s, int32_t delta);
uint32_t h2x87_v1_cmov   (X87Abi *s, uint32_t i, uint32_t take);

uint32_t h2x87_v1_ldcw (X87Abi *s, uint16_t cw);
uint32_t h2x87_v1_wait (X87Abi *s);
uint32_t h2x87_v1_clex (X87Abi *s);
uint32_t h2x87_v1_init (X87Abi *s);

uint32_t h2x87_v1_save_image (const X87Abi *s, uint32_t op, uint8_t img[108]);
uint32_t h2x87_v1_save_commit(X87Abi *s, uint32_t op);
uint32_t h2x87_v1_load_env   (X87Abi *s, uint32_t op, const uint8_t img[108]);
```

Selector values:

| selector | values |
|---|---|
| `op` for `arith` | 0 add, 1 sub, 2 subr, 3 mul, 4 div, 5 divr; OR `0x100` to pop |
| `src_kind` | 0 ST(i) (`src` = i), 1 f32 bits, 2 f64 bits, 3 i16, 4 i32 |
| `op` for `unary` | `X87Unary` order, 0..15 |
| `kind` for `cmp` | 0 fcom (signals on QNaN), 1 fucom, 2 ficom |
| `op` for env | 0 fnstenv, 1 fldenv, 2 fnsave, 3 frstor |

### 4.1 Memory operands are read by generated code

Helpers never touch guest memory. Generated code reads loads and memory
sources itself, so a guest page fault happens before the helper runs and
leaves the x87 state untouched, and then passes the bits by value. For a
waiting instruction with a memory operand, a pending #MF must win over the
page fault. The emitter therefore calls `h2x87_v1_wait` before the guest read
whenever ES may be set. Conservatively that is always; with the masks proven
set, it can be dropped (§8 Q3).

### 4.2 Stores: convert, write, commit

A store is three steps, so a faulting guest write can never leave a popped
stack or an updated status word behind:

1. `h2x87_v1_st_*` converts `ST(i)`, or the format's indefinite on stack
   underflow. It writes the bytes to `*value` and the status word that would
   result to `*sw`. **It does not modify `X87Abi`.**
2. Generated code acts on the status the helper returned:

   | status | write `*value`? | then |
   |---|---|---|
   | `OK` | yes, to the guest | if the write succeeded: `h2x87_v1_store_commit(s, sw, pop)` |
   | `SUPPRESS` | no (unmasked pre-computation fault) | `h2x87_v1_store_commit(s, sw, 0)` |
   | `MF` | no | raise event 14 |
   | `UNMODELLED` | no | stop with code 12 |

3. `store_commit` installs `sw` (keeping the current TOP) and pops if asked.

### 4.3 Environment operations: image, write, commit

- **`FNSTENV`/`FNSAVE`:**
  1. Materialise FIP/FDP/FOP/FCS/FDS (§2.4).
  2. `h2x87_v1_save_image` builds the image into a local buffer, without
     changing state. The image is 28 bytes for fnstenv or 108 for fnsave, in
     the 32-bit protected-mode layout:
     - control word at 0, status at 4 and tag at 8, each with its upper half
       set to `0xffff`;
     - FIP at 12, FCS at 16, FOP (11 bits) at 18, FDP at 20, FDS at 24;
     - for fnsave only, `ST(0)..ST(7)` from offset 28.
  3. Generated code writes the bytes to the guest.
  4. `h2x87_v1_save_commit` masks all exceptions (fnstenv) or runs `FNINIT`
     (fnsave). A faulting guest write skips this step. For fnstenv the image
     keeps ES and B, but the live state recomputes them (§2.3): with every
     exception masked they clear, so a pending #MF is discarded, as Intel
     documents for FNSTENV. The flags themselves stay, and an `FLDENV` of the
     saved image brings the fault back.
- **`FLDENV`/`FRSTOR`:**
  1. `h2x87_v1_wait`.
  2. Generated code reads the image.
  3. `h2x87_v1_load_env` loads everything, normalises the control word
     (§2.3), and recomputes ES/B. It recomputes each register's tag from its
     contents unless the loaded tag is 11 (empty).

### 4.4 Compares

`h2x87_v1_cmp` compares `ST(0)` with the source and pops `pop` times. The
relation goes into the return payload:

| relation | `to_flags = 0`: C3 C2 C0 (also written to `status`) | `to_flags = 1`: ZF PF CF |
|---|---|---|
| ST(0) > src | 0 0 0 | 0 0 0 |
| ST(0) < src | 0 0 1 (`0x0100`) | 0 0 1 (`0x01`) |
| equal | 1 0 0 (`0x4000`) | 1 0 0 (`0x40`) |
| unordered | 1 1 1 (`0x4500`) | 1 1 1 (`0x45`) |

C1 is always cleared. With `to_flags = 1`, C0/C2/C3 in `status` are left
alone.

**Unmasked faults.** An unmasked IE (an SNaN or unsupported operand, a QNaN
for the `fcom` forms, a stack underflow) or an unmasked DE means the compare
does not happen. Intel's tables say C3/C2/C0 and ZF/PF/CF are "not set" in
that case. The helper returns `H2X87_SUPPRESS`: the status word has the flag,
ES, B and, for a stack fault, C1 = 0; C3/C2/C0, TOP and the tags are
unchanged; nothing is popped. A suppressed compare must not look like a
successful "greater" (payload 0), so the status is explicit and the payload
is defined:

| return status | `to_flags = 0` payload | `to_flags = 1` payload | the lifter must |
|---|---|---|---|
| `H2X87_OK` | new C3/C2/C0 (= `status` now) | ZF/PF/CF | `to_flags`: replace ZF/PF/CF, clear OF/SF/AF, mark six defined |
| `H2X87_SUPPRESS` | unchanged C3/C2/C0 (= `status` now) | 0 | `to_flags`: keep ZF/PF/CF, clear OF/SF/AF, mark six defined; carry on (the next waiting instruction raises #MF) |
| `H2X87_MF` | 0 | 0 | raise event 14; change nothing |

`H2X87_UNMODELLED` cannot come from a compare (no post-computation
exceptions), but generated code handles it like any helper: stop with code 12.

So for the status-word forms the low 16 bits are always the C3/C2/C0 that
`fnstsw` would read after the instruction, on both `OK` and `SUPPRESS`, and a
fused branch may test `ret & mask` without checking the status first (it still
must check for `MF`). For FCOMI the lifter must branch on the status before
merging.

Returning the bits makes the `fnstsw ax; test ah, M; jcc` fusion a compare on
the return value. For `sahf` fusion, `AH` = `status >> 8`:

| AH bit | flag | source |
|---|---|---|
| 0 | CF | C0 |
| 2 | PF | C2 |
| 4 | AF | TOP bit 1 (status bit 12) |
| 6 | ZF | C3 |
| 7 | SF | B (status bit 15) |

### 4.5 `_ftol2`

The `_ftol2` CRT override (`0x322b0c`, 214 callers) calls
`h2x87_v1_ftol2(s, &out)` and puts `out` in EDX:EAX.

- It truncates toward zero whatever RC is.
- NaN, ±∞, an unsupported encoding, or |x| ≥ 2^63 gives
  `0x8000_0000_0000_0000` and sets IE. EAX is 0 in that case.
- An inexact truncation sets PE.
- It consumes one stack value (pop).
- An empty ST(0) is a stack fault: IE+SF, with the same indefinite.

This matches the retail routine's integer result for every input. That
routine runs `fistp m64` at RN and then corrects toward zero. Its sticky flags
can differ from the override's, for example PE from the correction
arithmetic. Only CRT code reads sticky flags, so the oracle masks them for this
override.

### 4.6 Rules every helper obeys

- **Precision control applies only to `arith` and to `Sqrt`.** Loads, stores,
  `Abs`, `Chs`, `RndInt`, `Xtract`, `Scale`, `Prem`, `Prem1`, the
  transcendentals and the constants compute at the full 64-bit significand.
  `FLD m32`/`m64` is an exact widening. The exponent range stays 15-bit at
  every PC.
- **Rounding control applies to every operation that rounds**: arithmetic,
  stores, `RndInt`, `Scale`, and the constants (`FLDPI` and friends round
  their infinitely precise value by RC). Transcendentals are the exception:
  the crate rounds them to nearest only. Under any other RC, a transcendental
  `unary` op changes nothing and returns `H2X87_UNMODELLED` instead of
  guessing. Measured, no transcendental runs under a directed RC (FLOAT.md
  §1.2).
- An out-of-range or NaN integer store produces the width's integer
  indefinite and sets IE. With IE masked the store still happens.
- C1 is set iff the delivered result was rounded up in magnitude. It is
  cleared by exact results, and set or cleared by stack faults as the SDM
  says.
- The helpers never read or write host FP state, never use host floating
  point, and never call libm.

## 5. SSE

Out of scope for this document. SSE1 semantics, MXCSR and `#XM` are defined
by `crates/simd` and [docs/simd.md](simd.md). That design is integer
soft-float, with C side `h2simd.h`, and uses no host floating point. The
previous §5 here assumed FTZ was live. That was wrong: nothing in the XBE sets
FTZ, DAZ or RC. Every MXCSR write is `saved | 0x1f80` or `saved & ~0x3f` (at
`0x535ab`, `0x535e2` and `0x53f69`).

## 6. `crates/x87`

The crate has no dependencies beyond `core` (`#![no_std]`), edition 2024, and
uses integer arithmetic only, with no host floats and no libm.
`#![forbid(unsafe_code)]` holds without the default `abi` feature. With it,
only `abi::ffi` may use `unsafe`, for pointer handling. The crate is the
arithmetic authority for the helpers, for any future fast path's proofs, and
for the oracle's reference values. Where the crate and Unicorn disagree, the
crate wins until hardware traces say otherwise (§6.3).

```rust
pub struct F80 { pub sig: u64, pub se: u16 }

pub enum Rc { Near, Down, Up, Chop }
pub enum Pc { Single, Double, Extended }   // 24, 53, 64 significand bits
pub struct Mode { pub rc: Rc, pub pc: Pc } // Mode::from_cw, Mode::EXTENDED
pub struct Flags(pub u16);   // IE DE ZE OE UE PE SF at status-word positions, plus
                             // Flags::C1 (0x0200): "rounded up in magnitude",
                             // Flags::TINY (0x1000): "result is tiny" (§2.1)
pub enum Class { Zero, Normal, Denormal, Inf, QNaN, SNaN, Unsupported }
pub enum Const { One, Z, Pi, L2E, L2T, Lg2, Ln2 }
pub enum Cmp { Less, Equal, Greater, Unordered }

impl F80 {
    pub const ZERO, ONE, INDEFINITE, INFINITY, NEG_INFINITY: F80;
    pub const PI, L2E, L2T, LG2, LN2: F80;     // round-to-nearest values
    pub fn from_bytes([u8; 10]) -> F80;  pub fn to_bytes(self) -> [u8; 10];
    pub fn classify(self) -> Class;
    pub fn from_f32_bits(u32) -> F80;  pub fn from_f64_bits(u64) -> F80;   // exact
    pub fn from_i64(i64) -> F80;                                           // exact
    pub fn to_f32_bits(self, Mode) -> (u32, Flags);
    pub fn to_f64_bits(self, Mode) -> (u64, Flags);
    pub fn to_i16 / to_i32 / to_i64(self, Mode, trunc: bool) -> (iN, Flags);
    pub fn from_bcd([u8; 10]) -> F80;  pub fn to_bcd(self, Rc) -> ([u8; 10], Flags);
    pub fn constant(Const, Rc) -> F80;   // FLDPI etc. under RC
}

pub fn add/sub/mul/div(a: F80, b: F80, m: Mode) -> (F80, Flags);
pub fn sqrt(a: F80, m: Mode) -> (F80, Flags);
pub fn compare(a: F80, b: F80, quiet: bool) -> (Cmp, Flags);
pub fn rndint(a: F80, rc: Rc) -> (F80, Flags);
pub fn scale(a: F80, b: F80, rc: Rc) -> (F80, Flags);
pub fn xtract(a: F80) -> (F80, F80, Flags);           // (exponent, significand)
pub fn prem(a: F80, b: F80) -> Partial;               // FPREM
pub fn prem1(a: F80, b: F80) -> Partial;              // FPREM1
pub fn sin/cos/tan(a: F80) -> Transcendental;
pub fn sincos(a: F80) -> (Transcendental, F80);
pub fn patan/yl2x/yl2xp1(y: F80, x: F80) -> (F80, Flags);
pub fn x2m1(a: F80) -> (F80, Flags);

pub struct Partial { pub value: F80, pub incomplete: bool, pub q: u8, pub flags: Flags }
pub struct Transcendental { pub value: F80, pub out_of_range: bool, pub flags: Flags }
pub const TRANSCENDENTAL_IS_CORRECTLY_ROUNDED: bool = false;
```

The binary operations take the destination first. NaN ties go to the
destination, so the ABI's reversed forms (`subr`, `divr`) call the internal
`add_impl`/`div_impl` with the tie on the correct side.

### 6.1 Semantics

| operation | behaviour |
|---|---|
| `add`/`sub`/`mul`/`div`/`sqrt` | **Exact.** The infinitely precise result is rounded once, to `pc` significand bits, under `rc`. The exponent range stays 15-bit at every `pc`. Denormal operands set DE, except where a higher-priority IE or ZE is raised (finite / 0 is ZE alone). Tiny results denormalise gradually; tininess is detected after rounding and always reported as `TINY`, and UE is set only when the result is tiny and inexact. |
| `prem` | **Exact.** `a - b*trunc(a/b)`, with the sign of `a`. `prem(5.5, 2) = 1.5`. An exponent difference of 64 or more gives `incomplete`, a partial reduction by `2^(32 + (D mod 32))`, and `q = 0`. Otherwise `q` holds quotient bits 0..2. `b = 0` or `a = ±∞` sets IE and gives the indefinite. |
| `prem1` | **Exact.** IEEE remainder (round-to-nearest-even quotient). `prem1(5.5, 2) = -0.5`. Same iteration and C0/C1/C3 rules. |
| `rndint`, `scale`, `xtract` | **Exact**, by RC where they round. `scale` clamps `trunc(b)` to ±2^20, which overflows or underflows exactly like the unclamped value. `scale(a, ±0)` returns `a` repacked: a pseudo-denormal becomes its normal encoding (exponent 1) with DE, since arithmetic never produces one (SDM Vol. 1 §8.2.2), and the ABI tags it valid. `xtract(±0)` sets ZE and gives `(-∞, ±0)`. `prem`/`prem1` repack every finite remainder the same way, including the shortcuts that return `a` (quotient 0, or an infinite divisor), so a denormal remainder reports `TINY` and a pseudo-denormal comes back normalised. |
| conversions | **Exact.** One rounding by `rc`. Narrowing sets `TINY` for a result denormal in the target format, UE+PE for a tiny inexact one, and OE+PE on overflow. An integer out of range or NaN gives the integer indefinite and IE. BCD out of range gives the BCD indefinite. |
| constants | **Exact.** The RN value or its neighbour by RC. π, log2(e), log10(2) and ln 2 round up at RN, and log2(10) rounds down (so `FLDL2T` under RC=up is one ulp above the RN value). |
| NaN rules | SDM Table 4-7 x87 rules. An SNaN operand sets IE and is quieted. SNaN against QNaN gives the QNaN. Otherwise the larger significand wins, ties to the destination. An invalid operation with no NaN input gives `INDEFINITE`. |
| `sin`/`cos`/`tan`/`sincos` | **Approximate (§6.2).** `out_of_range` when \|a\| ≥ 2^63: the operand is unchanged and C2 set, with no flag. ±∞ sets IE and gives the indefinite. ±0 gives ±0 (sin, tan) or +1 (cos). |
| `patan` | **Approximate.** Every zero and infinity combination follows the SDM table exactly, including `atan2(+0, -0) = +π` and `atan2(-∞, -∞) = -3π/4`. |
| `yl2x`, `yl2xp1`, `x2m1` | **Approximate.** `yl2x`: x < 0 sets IE; x = ±0 sets ZE (y ≠ 0); `x = 1, y = ±∞` sets IE. `yl2xp1` and `x2m1` are total and documented beyond the SDM's ranges (\|x\| < 1−√2/2, \|a\| ≤ 1). `x2m1` of an integer is exact. |

### 6.2 Transcendental accuracy

`TRANSCENDENTAL_IS_CORRECTLY_ROUNDED` is **false**. The flag now agrees with
this document: the previous text said `true` while the crate had `false`.

- **Method.** Each function evaluates its series in a 512-bit truncating
  software float, `Wf<8>`. The constants come from 1024-bit tables, which are
  checked against independent formulas (Machin, Størmer, a Blowfish π digit
  run, and two ln 2 series). The function then rounds once to the 64-bit
  significand at RN. The series stop below 2^-520 relative, and the sign of
  the omitted tail feeds the rounding, so PE and C1 are right whenever the
  rounding is.
  The optional `x87/fast-trans` feature first attempts guarded 128-bit
  evaluation for FYL2X and F2XM1. Its domains, per-function error
  bounds and value/PE/C1 acceptance test are documented in
  [x87-transcendentals.md](x87-transcendentals.md). Other functions and
  rejected intervals retain the 512-bit reference.
- **Evidence.** A test evaluates every function at 512 and at 768 bits and
  checks that both round to the same ext80 result:
  - 1,350 random arguments agree in the default test run;
  - 180,000 agree in the ignored extended run, in release.

  No disagreement has been seen. This is strong evidence of correct rounding
  but not a proof, because a case harder than 2^-520 could exist. Hence
  `false`.
- **Not the Pentium III.** For `fsin`, `fcos` and `fptan`, the PIII reduces
  arguments with a 66-bit approximation of π. Near multiples of π, and for
  large arguments, its results differ from the true function. The crate
  reduces with the exact value (1024 bits of 2/π), which gives the
  mathematically correct answer. **Decision:** this is documented, not
  modelled. Modelling it needs the PIII's exact reduction algorithm, which is
  not public. The oracle keeps transcendentals under the named tolerance in
  FLOAT.md §2.9 until hardware traces exist.
- **Rounding control.** RN only. Under other RCs the helper stops (§4.6).
- **Speed** (release, per call, measured on the dev Mac):

  | function | time per call |
  |---|---:|
  | sin | 18.5 µs |
  | tan | 100 µs |
  | patan | 148 µs |
  | yl2x | 109 µs |
  | x2m1 | 127 µs |
  | div | 65 ns |

  There are 522 inline `fsin`/`fcos` sites. Guarded lower working precision
  is now available for the measured hot log/exp subset described
  above. Trigonometric argument reduction still uses the reference;
  no speed or error bound for a lower-precision trig path is claimed.

### 6.3 The oracle and Unicorn

Unicorn 2.1.5 is a QEMU 5.0.1 core. Its x87 transcendentals use host libm,
and its exception flags are mostly not set (FLOAT.md §2.9). Until each case is
verified against hardware, the oracle masks the following per mnemonic and
treats the crate as the authority:

- C1;
- the C0/C2/C3 bits the SDM leaves undefined (for example after `fld`, `fst`,
  arithmetic, and the transcendentals except C2);
- control-word bit 6 (§2.3).

### 6.4 Tests

All the tests below pass.

- **`tests/spec.rs`**: 31 tests, plus 1 ignored, in 8 groups. Each value is
  hex with a comment giving its source.
  1. Special values against the SDM tables, including unsupported encodings.
  2. PC=24/53/64 double rounding, including `1 + 2^-63 − 1 = 2^-63` at PC=53.
  3. `prem` against `prem1`: (5.5, 2) and (−5.5, 2), iteration, and the
     quotient bits.
  4. The trig range around 2^63, ±∞ and ±0.
  5. All four RCs on ties, negatives, and `rndint`.
  6. Integer conversion at ±2^31 and ±2^63, `0x8000_0000`, and NaN.
  7. Round trips: 1,000,000 binary32/binary64 cross-checks; the ignored test
     is the exhaustive 2^32 binary32 round trip, which passes in 464 s in
     release.
  8. Byte round trips, `FLDPI`'s full significand, and the indefinite
     encoding.
- **`tests/abi.rs`**: 33 tests:
  - the `X87Abi` layout, TOP in `status`, push and pop wraparound, and tags;
  - stack overflow and underflow, with IE+SF+C1 and the indefinite;
  - an unmasked stack fault and an unmasked ZE that commit nothing and give
    #MF at the next instruction, plus the non-waiting forms;
  - unmasked PE/OE giving `UNMODELLED`;
  - the two-phase store, with no change before commit and `SUPPRESS`;
  - control-word normalisation (`0x023F` reads back as `0x027F`), and ES/B
    recomputed by `FLDCW` and `FLDENV`;
  - `FLDENV` tag recomputation, the env and save image layout, and the commit
    step;
  - `_ftol2`;
  - compare payloads, including `ftst`;
  - `fxam`, the unary ops, `fxch` and `fcmov`;
  - the FFI symbols;
  - a bad selector panicking;
  - directed RC stopping the transcendentals;
  - the re-review regressions: unmasked UE on an exact tiny product and an
    exact tiny `fst m32`; ZE outranking DE (register and memory operands); no
    PE recorded under a suppressing DE (arithmetic, `fsqrt`, `fpatan`);
    FNSTENV discarding a pending #MF; suppressed compares (`fcom`, `fcomi`,
    stack underflow); FSCALE by zero of a pseudo-denormal; `save_image`
    through a const pointer to an immutable static; FPREM/FPREM1 shortcuts
    stopping on a tiny exact remainder and normalising a pseudo-denormal.
- **Library unit tests**: 6, plus 2 ignored: the constant-table generator and
  the 180,000-case precision run.

## 7. Where the measurement tool fits now

`h2 fpstats`'s stack walk is no longer a correctness precondition. It stays
as measurement, and feeds two optional later steps:

- choosing which functions are worth a `double` fast path, which needs the
  value-level proof in FLOAT.md §2.8 and not just a depth proof;
- reporting the shape of the work: which helpers dominate, where the stack is
  live across calls, and which CRT helpers to override by hand.

Only `Proven` walk results may feed an optimisation. `Convention` and
`Heuristic` results are for reporting only.

## 8. Resolved questions

1. **Split `h2x87_v1_arith` per source kind?** Yes, eventually, as thin
   per-kind entry points over the same core. `fld`/`fmul`/`fadd` with an `m32`
   source dominate. v1 keeps the single entry point. Adding
   `h2x87_v1_arith_f32` and friends later is ABI-compatible.
2. **Eager or lazy status-word sync?** Moot. TOP lives in `status`, and the
   helpers keep `status` exact, so there is nothing to sync.
3. **Emit `fwait`?** Yes. It is `h2x87_v1_wait` and it can deliver #MF (§2.2).
   It may be dropped only where the masks are proven set and no flag can be
   pending, which is an optimisation needing a proof.
4. **Register storage size?** Moot. The registers are master's
   `uint8_t st[8][10]` (80 bytes).
