# Independent x86 instruction oracle

`crates/oracle` executes synthetic bytes in **Unicorn**, independently of the
lifter, IR, generated C, and runtime. It is an instruction-testing reference, not
a claim that QEMU reproduces an Xbox Pentium III exactly. Agreement between our
lifter and itself is not independent evidence; see [LIFTER.md §3 and §8](LIFTER.md).

## Choice and alternatives

| Candidate | Assessment |
| --- | --- |
| Unicorn | Selected. Embeddable QEMU TCG CPU core, x86-32 on macOS arm64, Rust bindings, register/memory APIs and hooks. A fresh engine per input gives isolation without a guest OS. |
| Bochs / Bochs-derived cores | Valuable second reference with different implementation ancestry. Portable C++ CPU and software FP, but integrating a complete CPU environment or maintaining an extracted core is a larger dependency and harness project. |
| Full QEMU / xemu | Useful for exception, system and Xbox traces; considerably more machine setup than a single-instruction runner. Shared QEMU ancestry means agreement with Unicorn is correlated evidence. |
| iced-x86 | Decoder/encoder and instruction metadata only; it does not execute. Used here for boundaries, feature restrictions, and register-use metadata, never arithmetic or FP results. |
| Native 32-bit execution | Not available in an arm64 macOS process. A separate x86 host, bootable hardware harness, or Xbox can provide stronger independent evidence later. Rosetta is not a native 32-bit Pentium III oracle. |

The versions are pinned to `unicorn-engine = 2.1.5` and the corresponding
`unicorn-engine-sys` in Cargo.lock (the bundled core identifies as Unicorn 2.1.4).
Only the x86 core is built. Upstream's 2.1.4 release specifically strengthened
instruction-PC synchronization, which matters for faults.
Sources: [Unicorn release](https://github.com/unicorn-engine/unicorn/releases/tag/2.1.4),
[Rust crate](https://crates.io/crates/unicorn-engine/2.1.5),
[Bochs](https://github.com/bochs-emu/Bochs),
[Bochs software-FP changes](https://github.com/bochs-emu/Bochs/releases/tag/REL_3_0_FINAL),
[iced-x86](https://github.com/icedland/iced),
[xemu](https://github.com/xemu-project/xemu).

The optional Unicorn Rust dependency declares GPL-2.0; the upstream core has
GPL/LGPL components. Keep this as an optional testing dependency when considering
distribution and attribution. Our own source retains the workspace license.

## API and execution contract

```rust
use oracle::{run, CpuState, Stop};

let result = run(
    &[0x01, 0xd8], // ADD EAX, EBX
    CpuState { eax: u32::MAX, ebx: 1, ..CpuState::default() },
    &[],
);
assert_eq!(result.stop, Stop::Completed); // requires feature `unicorn`
assert_eq!(result.state.eax, 0);
```

`run(bytes, state, mem) -> Outcome` installs the bytes at `state.eip` and runs
until EIP reaches the exclusive end, execution leaves the range, a fault occurs,
or 4096 emulator steps have run. REP iterations consume the same step budget;
they appear as one instruction record. A same-PC branch still consumes steps.
Every invocation creates a new emulator and memory image. No host kernel calls,
game bytes, lifter helpers, or shared guest state are involved.

`CpuState` owns the eight GPRs, EIP and EFLAGS; eight logical x87 ST registers as
little-endian 10-byte ext80 arrays; the full x87 control/status/tag/environment;
eight little-endian 16-byte XMM arrays; and MXCSR. Values never pass through
Rust `f64` or host `long double`. Default x87 control is architectural `0x037f`,
tags are empty (`0xffff`), and MXCSR is `0x1f80`; these are not a claim about
Xbox thread initialization. Set `control: 0x027f` explicitly when testing that
provisional thread policy. TOP resides in status; the tag word uses physical
register order and ST arrays use logical order.

The harness selects `PENTIUM3`, 32-bit protected mode with flat zero-based
segments, paging off, CR0.PE/NE set and CR4.OSFXSR/OSXMMEXCPT enabled. Segment,
descriptor, privileged, I/O and exception-delivery machinery are outside its
contract. The decoder's feature allowlist also excludes SSE2 and later ISA;
unsupported cases produce `Unsupported`, not a fabricated architectural #UD.
VM86 pre-state and reserved MXCSR bits, including PIII-reserved DAZ, are rejected.
This does not emulate Xbox CPUID, FS/KPCR, page tables, SEH, interrupts or MMIO.

Each nonempty `(address, bytes)` memory seed maps the containing 4 KiB pages as
RW, non-executable, initially zero. Multiple nonoverlapping seeds may share a
page. The rest of that page is **captured zero-filled pre-state**, not an
uncaptured memory read. Code pages are RX; a seed sharing a code page, overlapping
seeds, and 4 GiB range wraps are invalid input. Data permissions are deliberately
simple because the requested API carries no permissions. Do not use this API
as a replay adapter without first preserving the real mapping/permission model.
Truncated encodings are invalid input, so zero padding cannot silently complete
an instruction. Code writes fault. Branches within the supplied range work;
branches leaving it stop explicitly.

Outcomes include final CPU state, per-instruction flag masks, per-bit GPR
validity, read accesses, and coalesced final bytes for the union of writes.
Successful stores of unchanged values count as writes. A final page comparison
also captures partial memory effects on faults. Reads include attempted accesses
that Unicorn's read hook reports, not instruction fetches or a guaranteed ordered
bus trace. Instructions record whether they completed. On a fault, do not apply
the incomplete instruction's normal post-instruction undefined flag mask.

`DivideError`, `InvalidOpcode`, synthetic `PageFault { address, access,
protection }`, and other exception vectors remain structured. The page fault
describes the harness's linear mappings; it has no architectural paging error
code/CR2/page-table walk. Backend failures and resource stops remain distinct
from guest exceptions. REP progress and fault EIP come from the emulator, not
locally simulated instruction semantics.

## Defined flags

The explicit common-integer rules in `flags.rs` were checked against Intel SDM
Volume 2, **325383-078US, December 2022**, each instruction's Operation and Flags
Affected sections. The [download used](https://cdrdv2-public.intel.com/671110/325383-sdm-vol-2abcd.pdf)
identifies that edition on its title page; Intel's [current manual index](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html)
may point to a newer edition. Masks use architectural EFLAGS bit positions.

| Instructions | Undefined arithmetic flags after successful execution |
| --- | --- |
| ADD/ADC/SUB/SBB/CMP/NEG | None |
| INC/DEC | None; CF preserved |
| AND/OR/XOR/TEST | AF; CF/OF cleared |
| MUL/IMUL, all operand forms | SF/ZF/AF/PF; compare CF/OF |
| DIV/IDIV | CF/PF/AF/ZF/SF/OF |
| BT/BTC/BTR/BTS | OF/SF/AF/PF; ZF preserved |
| BSF/BSR | OF/SF/AF/PF/CF; ZF defined |
| SHL/SHR/SAR | Nonzero count: AF; OF also undefined except count 1. SHL/SHR CF undefined for masked count >= operand width. SAR CF remains defined. |
| SHLD/SHRD | Nonzero count: AF; OF undefined except count 1. Count greater than width: all arithmetic flags and destination undefined. |
| ROL/ROR | Only OF undefined for masked count > 1; nonzero multiples of operand width still update CF. |
| RCL/RCR | Effective zero count preserves flags; otherwise OF undefined unless masked count is 1. |
| SETcc/CMOVcc, MOV/partial writes, strings copying/loading/storing | No flags modified |
| CMPS/SCAS | Arithmetic flags defined; zero-count REP preserves flags |

Counts are masked with 31 before width-specific treatment, including counts 32,
33, 64 and 255. Byte/word carry rotates additionally reduce modulo 9/17. Per
instruction masks account for immediate or actual pre-state CL counts.

Other instructions use iced's independent flag metadata. It is coverage metadata,
not an independent executor. x87 condition-code validity and NaN payload validity
are not yet modeled: scope comparison of FP state to instruction-specific defined
fields and check the limitations below. The harness does not broadly mask FP
differences or emulator bugs as architectural undefined state.

Final validity preserves prior undefined flags until an instruction defines
them again. BSF/BSR with zero source also mark the destination GPR bits invalid.
The runner stops with `UndefinedInput` before a later instruction reads an
undefined GPR/flag, rather than recording an arbitrary emulator choice as a
required result. This is conservative; it can stop even when downstream logic
would cancel that dependency. For faulting instructions, pre-existing validity
is retained. A differential adapter must compare `(actual ^ expected) & mask`,
including all preserved architectural bits and GPR bits, not merely flags an
instruction writes. Stopped, unsupported, and undefined-input runs are not
successful execution fixtures.

Oversized double shifts with a memory destination are refused as `Unsupported`:
this first schema has no per-byte memory validity mask for their undefined
results. GPR destination validity is modeled for BSF/BSR and oversized double
shifts; it is not an exhaustive validity model for every legacy x86 instruction.

## Known limitations and inaccuracies

* **x87 transcendental precision is not PIII precision.** The pinned core's
  `helper_f2xm1`, `helper_fptan` and other transcendental helpers convert ext80 to
  host double and use libm. Extra significand/exponent information and
  instruction-specific rounding/status behavior can be lost. Basic ext80
  transport and an exact 1+1 case are tested; this does not validate the full FPU.
  [Pinned FPU helpers](https://github.com/unicorn-engine/unicorn/blob/2.1.4/qemu/target/i386/fpu_helper.c).
* **FCOMI/FUCOMI flag handling needs quarantine.** The same source computes
  ZF/PF/CF while preserving old SF/AF/OF; the SDM specifies clearing those three
  flags. Do not change lifter semantics to match that implementation. Keep such
  mismatches as emulator defects pending independent traces. x87 stack faults,
  C1, deferred/unmasked exceptions, FIP/FDP/opcode updates and unusual ext80/tag
  combinations need separate instruction-specific validation. Unicorn rebuilds
  nonempty tag classes from values instead of preserving arbitrary inconsistent
  tag words. [Pinned register interface](https://github.com/unicorn-engine/unicorn/blob/2.1.4/qemu/target/i386/unicorn.c).
* **PIII SSE details remain unproven.** RCP/RSQRT approximation bits, NaN choices,
  denormals/FTZ, sticky exceptions, unmasked SIMD faults and alignment cases need
  hardware checks. The core's reciprocal helpers compute floating-point
  reciprocals rather than reproducing measured PIII approximation tables.
  Rejecting DAZ in pre-state does not prove the engine faults correctly if guest
  LDMXCSR sets it. [Pinned SIMD helpers](https://github.com/unicorn-engine/unicorn/blob/2.1.4/qemu/target/i386/ops_sse.h).
* **Undefined flags are not evidence.** QEMU's lazy condition-code helpers choose
  concrete values for some architecturally undefined flags, and have their own
  implementation/version bugs. The masks remove only SDM-undefined bits; a
  disagreement on a defined bit is a dispute, not permission to expand the mask.
  [Pinned condition-code helpers](https://github.com/unicorn-engine/unicorn/blob/2.1.4/qemu/target/i386/cc_helper.c).
* **Fault delivery is intentionally shallow.** Vector/address and stopped CPU
  state are observations, not validated SEH/IDT semantics. Complex faulting
  instructions and cross-page partial writes need hardware comparison. The REP
  MOVSB page-boundary restart case is tested; this does not settle every string
  comparison/exception restoration case.
* **macOS sandbox startup issue, observed here.** Denying
  `sysctlbyname("hw.cachelinesize")` makes the pinned engine fall back to reading
  `CTR_EL0`, which traps with host SIGILL on this Apple CPU. Unsandboxed execution
  passes. This is a host startup failure, not guest #UD. Do not generate fixtures
  from that failure. [Pinned cache probe](https://github.com/unicorn-engine/unicorn/blob/2.1.4/qemu/util/cacheinfo.c).

No xemu, Xbox, native PIII or Bochs execution was performed for this delivery.

## Fixtures and checks

The default catalog contains **484 encodings**, including 8/16/32-bit ALU,
ADC/SBB, unary arithmetic, shifts/rotates with CL and immediate edge counts,
SHLD/SHRD, MUL/IMUL/DIV/IDIV success/overflow/zero-divisor cases, BT*, BSF/BSR,
all 16 SETcc/CMOVcc conditions, REP/REPE/REPNE string operations in both
directions, address-size overrides, partial-register writes, memory stores,
short sequences and a few exact FP smoke cases. Each encoding gets 25 edge
inputs and 16 seeded randomized inputs by default: **19,844 fixtures**. This is
an initial corpus, not exhaustive operand or opcode coverage.

The full JSONL corpus is written under `$TMPDIR`; the generator can consume a
custom JSON encoding list, for example
`[{"name":"adc-eax-ebx","bytes":[17,216]}]`, with `--encodings PATH`.
Schema version 1 records encoding, case, seed, complete CPU pre-state, memory
seeds, backend version, post-state, validity, writes and stop. Fixed xorshift64*
and a decimal seed make inputs reproducible. Reproducibility requires the same
catalog/order, random count, dependency versions, and backend behavior.

`crates/oracle/fixtures/smoke.json` holds **19** hand-picked generated fixtures.
They are synthetic and suitable for source control; no game data is included.
Default-feature tests deserialize them and check selected results by hand.
Feature-enabled tests replay the corpus, comparing only defined GPR/flag bits.
Other hand-checked SDM tests exercise carry/overflow, multiplication halves,
signed division truncation, #DE/#UD, shifts, scans, conditional/partial writes,
unchanged stores, REP faults, ext80 1+1 and scalar SSE upper-lane preservation.

Ordinary workspace testing has **no Unicorn/CMake requirement**. Execution
returns `BackendUnavailable` when the feature is disabled; there is no silent
fallback executor. The opt-in build needs a C/C++ toolchain, CMake, a make/ninja
build tool, and libclang for bindgen. On macOS, Xcode command-line tools plus
Homebrew CMake suffice in this environment. No external Unicorn installation is
needed; the bundled x86 core is built from the pinned crate.

```sh
cargo test --workspace
```

```sh
CARGO_TARGET_DIR="$TMPDIR/oracle-target" cargo test -p oracle --features unicorn
```

```sh
CARGO_TARGET_DIR="$TMPDIR/oracle-target" cargo run -p oracle --features unicorn --bin oracle-fixtures -- --out "$TMPDIR/oracle-fixtures.jsonl" --small-out crates/oracle/fixtures/smoke.json
```

## Settling disputes later

Minimize a disagreement to synthetic instruction bytes plus full CPU and mapped
memory pre-state. Record the defined-bit comparison, input/output ext80 and SIMD
bytes, exception PC/vector/address, tool versions, host/CPU identity and seed.
Do not decide which side is right from lifter-versus-generated-C agreement.

Run the identical minimized case on actual Xbox/PIII hardware with an assembly
harness that saves GPRs/EFLAGS and FXSAVE/FNSTENV state immediately, using a
fault handler that records pre-delivery state. Preserve x87 precision, rounding,
tags, MXCSR, mapped pages and restart behavior. Modern native x86 is useful for
stable integer behavior, but cannot settle PIII approximation/DAZ/model details.
Retain these hardware observations as a separately attributed fixture source.

Use instrumented xemu CPU snapshots/traces around the same instruction for
Xbox environment and exception evidence, pinning its revision/configuration and
capturing complete pre-state. Because xemu and Unicorn share QEMU ancestry,
agreement strengthens reproduction evidence but does not independently settle a
shared CPU bug. Bochs adds a distinct software implementation; real PIII/Xbox
observations and the applicable SDM/errata settle CPU-specific disputes. Any
accepted emulator workaround should be narrowly attributed and reproducible,
never a broader undefined mask or a global floating-point tolerance.
