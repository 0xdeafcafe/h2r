# Lift Xbox code toward readable Rust

Research date: 2026-11-14. Status: design proposal, written before the build. The lifter, IR and C emitter now exist (`crates/lift`, `crates/ir`, `crates/emit`); where this text and the code differ, the code wins. No builds, tests, or timings were performed for this text.

**Build a Rust static x86 lifter that initially emits disposable C11 through a backend-independent semantic IR.** Target the original Xbox's 32-bit Pentium III instruction model, SSE1+x87, and 64 MB unified RAM. Link with the Rust runtime on macOS arm64 and x86-64. Generated functions carry explicit CPU state and yield control transfers to a Rust runner; every function entry can resolve to a per-address Rust replacement. This preserves guest stacks, custom conventions, and eventual guest exception handling while readable, moddable Rust replaces scaffolding. D3D8 API HLE on wgpu/Metal is settled, with tvOS or Xbox Developer Mode possible later. This proposal refines the implementation sketches in [ARCHITECTURE.md](../ARCHITECTURE.md), preserving its goals and the disc-derived-output policy in [README.md](../../README.md).

Evidence labels matter: **Facts** below describe cited projects or inspected repository source; **local evidence** comes from the supplied binary-analysis notes and subsequent disassembly review, not a fresh execution here. **Recommendations** are proposed contracts. **Unknowns** need measurements or independent traces. The planning workload is approximately 3.4 MB of code and 18,000 discovered functions; discovery counts do not establish correct boundaries or executable coverage.

## Precedents support separation, not automatic compatibility

N64Recomp translates instructions into C functions, uses labels/switches for internal control flow, and runtime lookup for indirect calls. XenonRecomp emits C++, exposes generated originals behind replaceable functions, and requires a separate runtime. Unleashed supplies address lookup, thread contexts, and typed hook adapters. These demonstrate a useful generated-code/runtime boundary; their MIPS/PowerPC assumptions do not establish x86 correctness. ([N64Recomp](https://github.com/N64Recomp/N64Recomp/blob/main/README.md#how-it-works), [XenonRecomp](https://github.com/hedge-dev/XenonRecomp/blob/main/README.md#implementation-details), [Unleashed adapters](https://github.com/hedge-dev/UnleashedRecomp/blob/main/UnleashedRecomp/kernel/function.h))

ReXGlue is a verified public, experimental Xbox 360 toolkit: it emits batched C++23, preserves originals for overrides, and supplies a Xenia-derived runtime with function dispatch. Its documented implementation is a precedent, not an original-Xbox solution. ([ReXGlue SDK](https://github.com/rexglue/rexglue-sdk/blob/main/README.md), [generated structure](https://github.com/rexglue/rexglue-sdk/wiki/Generated-Code-Structure), [overrides](https://github.com/rexglue/rexglue-sdk/wiki/Function-Overrides))

McSema separates control-flow recovery from Remill instruction lifting; Remill explicitly models state and memory. rev.ng uses QEMU-based lifting/helpers and LLVM analysis, and documents recompilable C plus an editable model. SR's DOS and SRW's Win32 game ports use executable-specific annotations and assembly/`llasm`, with `llasm` translated to LLVM IR: describing these as x86-to-C ports would be inaccurate. Their lesson is to retain corrections as inputs, separate from regenerated output. ([McSema](https://github.com/lifting-bits/mcsema), [Remill design](https://github.com/lifting-bits/remill/blob/master/docs/DESIGN.md), [rev.ng helpers](https://docs.rev.ng/developer-manual/qemu-helpers/), [rev.ng overview](https://docs.rev.ng/what-is-revng/), [SR](https://github.com/M-HT/SR/blob/master/SR/README.md), [SRW](https://github.com/M-HT/SR/blob/master/SRW/README.md), [llasm](https://github.com/M-HT/SR/blob/master/llasm/README.md))

Cxbx supplies reverse-engineered Xbox kernel and D3D interception contracts; xemu supplies CPU/device models and potential traces. Neither proves this title's HLE coverage. Their useful evidence is discussed at the relevant boundaries below.

## 1. Generate C11 first; keep semantics independent

**Recommendation:** decode with iced-x86, recover a validated CFG, lower into Rust-owned semantic blocks, then emit C11 and native objects. The IR records instruction PC, widths, register slices, defined/preserved/undefined flags, memory effects, and fault/control outcomes. It must not encode C syntax or inferred host signatures.

| Backend | Build and debugging tradeoff | Decision |
|---|---|---|
| C11 → Clang | Inspectable source and ordinary objects/debuggers; frontend parsing and C semantic constraints remain. | Initial backend with strict helpers. |
| LLVM IR | Avoids C parsing; explicit integer/vector operations and constrained FP. LLVM optimization, code generation, debug metadata, and version coupling remain. | Later experiment using identical IR/ABI. |
| Cranelift AOT | Rust integration, x86-64/AArch64 and Mach-O objects; different optimization/debugging tradeoffs. Dynamic FP rounding/status/exceptions still require helpers. | Benchmark candidate, not a promised speedup. |

C-through-Clang already uses LLVM: this compares emission/frontend strategies, not wholly different optimizers. LLVM overflow annotations can introduce poison; CLIF's documented FP contract omits guest exception flags and dynamic rounding. Neither backend removes semantic obligations. ([LLVM semantics](https://llvm.org/docs/LangRef.html), [Cranelift IR](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/docs/ir.md), [object backend](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/object/src/backend.rs))

Use unsigned fixed-width arithmetic, explicit truncation/sign extension and partial-register merges, masked/special-cased shifts, checked divide faults/overflow, and checked FP conversions. Helpers implement guest behavior before host operations could invoke undefined behavior. Use byte access/`memcpy` for representations. Compile FP helpers with strict FP and contraction disabled; prohibit fast-math throughout. These settings support, but do not implement, guest FP semantics. ([Clang FP controls](https://clang.llvm.org/docs/UsersManual.html#controlling-floating-point-behavior), [aliasing](https://clang.llvm.org/docs/UsersManual.html#strict-aliasing))

Start with stable 16 KiB entry-VA buckets (`entry_va >> 14`), isolating oversized functions in their own shards. Use content hashes to preserve unchanged files and cache using image, analysis, semantic-helper, ABI, and compiler-option hashes. Benchmark and tune bucket size later. Avoid both one enormous translation unit and one compiler process per function. XenonRecomp already preserves unchanged output and batches functions; this supports incremental generation, not a numeric compile estimate. ([emitter](https://github.com/hedge-dev/XenonRecomp/blob/main/XenonRecomp/recompiler.cpp#L2330))

Use `-O0 -g` for debugging and initially `-O1 -g` for release, both without LTO; compare `-O2` through measurement. Retain guest-address symbols and synthetic source mappings on both architectures. Source C benefits from compiler-generated debug information; direct LLVM IR and CLIF require explicit debug metadata/mapping work. An override-only edit should rebuild handwritten Rust and relink without recompiling generated shards. Smaller shards improve incremental isolation at the cost of compiler startup/object overhead.

Never hand-edit or commit generated files; fixes belong in the lifter, metadata, or Rust overrides. C/LLVM/Cranelift compile costs have not been measured here, and no speed advantage is promised. Debug information and optimization can each dominate costs regardless of frontend. Direct LLVM emission saves C parsing but does not eliminate optimization or target code generation. Cranelift's potential compilation advantage needs an equivalent-helper benchmark, not a comparison that silently omits guest semantics.

## 2. Preserve guest control flow with explicit transfers

**Recommendation:** one host function per guest function owner, with labels/gotos for its CFG and an entry selector for validated resume PCs. Jump tables evaluate the actual guest table load and computed target, then switch on validated addresses. Fold index-to-label only when the table is proven immutable. Unbounded analyzer heuristics must not truncate runtime target possibilities; unknown computed targets take the explicit unsupported-target path. Local indirect jumps resolve exact validated labels; cross-owner jumps yield transfers. Address-taken interior entries receive explicit adapters. Never infer ownership from an interval ending at the next discovered function.

The current [analyzer](../../crates/analyze/src/lib.rs) exposes starts, labels, switch kinds, and speculative discovery evidence, but not a complete public block/edge/table-target model. Export owners, instruction boundaries, actual switch targets/bounds, call continuations, and evidence/conflicts before emission. Speculative coverage is not validation. Include executable library sections beyond `.text`; unidentified SDK helpers remain generated until replacement is justified.

Its `NORETURN_KERNEL` ordinals 49, 95, 96, and 258 identify HalReturnToFirmware, KeBugCheck, KeBugCheckEx, and PsTerminateSystemThread. Model these as explicit process/thread termination outcomes, ending guest flow. The public `Function.returns` bool also includes inferred nonreturning behavior: validate that evidence rather than blindly dropping paths when `returns=false`.

Every guest `call` evaluates its target, pushes its actual guest continuation into guest memory, commits CPU state, and yields `Call(target)`. The runner applies Dispatch and invokes the selected handler. Tail transfers yield `Jump(target)` without pushing. `ret` reads the actual guest return address, updates ESP by four plus its immediate cleanup, and yields `Return(target)`. Guest values return through EAX/EDX/x87 or other documented locations; the host return describes control only.

The runner maintains an exact **generated-only resume map** from validated continuation PCs to owner/resume selectors. Returns to those PCs resume generated code without restarting a whole-function override. Changed returns to recognized function entries use entry Dispatch; unsupported targets stop for offline discovery/regeneration. Calls, indirect calls, tails, callbacks, and external entry adapters all apply the entry override policy. Do not optimize direct calls into generated-symbol calls that bypass it.

Identify entry versus resume explicitly in transfer metadata. If one address serves both roles, its generated continuation contract and external entry contract remain distinct; a numeric PC alone must not accidentally reapply an entry wrapper. A return to an unexpected validated label needs an explicit resume adapter, not a nearest-address fallback or runtime JIT.

Interior aliases need their own replacement contracts/overrides, or must retain generated interior adapters when the main entry is replaced. A typed whole-function wrapper cannot accept arbitrary mid-function state. Known local branches remain local; externally entered aliases and exceptional resumes use their explicit adapters.

This runner costs transfer/lookup/spill work but gives bounded host-stack use and guest-controlled returns. Native recursion is simpler, yet incorrectly couples guest unwinding and changed returns to the host stack; C tail-call elimination is not a correctness guarantee. Rust overrides that call guest code use explicit continuation state as well. Unknown targets trap with PC, target, and provenance; an explicit harness exit token is the only synthetic successful exit.

## 3. Make CPU state authoritative before optimizing it

**Recommendation:** canonical CPU state includes eight GPRs, EIP, FS base and required segment state, EFLAGS values plus validity, eight x87 extended values and environment, eight bit-preserving XMM registers, and MXCSR. Preserve remaining EFLAGS bits, including IF/TF where relevant, rather than reducing state to today's arithmetic booleans. Privileged/unsupported changes must have an explicit policy. Include x87 FIP/FDP/opcode for environment instructions such as FNSTENV if encountered, TOP/tags/control/status, and MMX alias behavior if used.

Initially load register locals on body entry and spill full state at every transfer and observable helper/fault/reentry/trace boundary. Faulting instructions must report the architecturally correct pre/post-state; REP progress needs explicit semantics. This is more traffic than selective spilling, but makes LTCG conventions irrelevant to correctness. Later use per-register/per-flag liveness conservatively across unknown calls. Compute flags eagerly first; block-local lazy flags can follow, materialized before PUSHFD/LAHF, helpers, exceptions, and transfers. Undefined flags receive deterministic stored values plus invalidity masks, never compiler undef/poison.

Use a software ext80 x87 reference with instruction-specific precision, rounding, exception, stack, and storage semantics. SoftFloat supplies extF80 arithmetic and precision controls, not a complete x87 CPU or all transcendentals. Keep its mutable modes isolated per execution context. Optional validated f64 fast paths need guards/evidence: PC53 retains the extended exponent range, PC64 retains extra precision, and loads do not simply round under arithmetic PC. Apple arm64 `long double` is only double precision. ([SoftFloat](https://www.jhauser.us/arithmetic/SoftFloat-3/doc/SoftFloat.html), [Intel x87 precision](https://cdrdv2-public.intel.com/843827/253665-sdm-vol-1-dec-24.pdf), [Apple ABI](https://developer.apple.com/documentation/xcode/writing-arm64-code-for-apple-platforms))

Distinguish architectural x87 initialization (`0x037f`, status zero, tags `0xffff`) from provisional Xbox thread-start policy: Cxbx initializes control `0x027f` and MXCSR `0x1f80`. Validate the latter against Xbox/xemu traces; current zero-derived defaults are unsuitable. ([Cxbx thread context](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlKi.cpp))

Initially map SSE1 to per-lane f32 semantic helpers and explicit bit operations, preserving scalar upper lanes as specified. Implement MXCSR independently of host controls: rounding, sticky status, masks, signed zeros/NaNs, and FTZ. PIII treats DAZ bit 6 as reserved; it is not an SSE1 feature. Validate loads against the guest mask and fault reserved settings. FTZ applies to masked underflow, not arbitrary input denormals. Native SSE/NEON arithmetic is a guarded optimization (on by default, `H2_SSE_NATIVE=0` disables it; see [sse-native.md](sse-native.md)) that keeps MXCSR in the guest state rather than the host's; SSE2 availability on the host grants no guest instructions. RCP/RSQRT approximations require a PIII oracle if used. ([Intel MXCSR rules](https://www.intel.com/content/dam/support/us/en/documents/processors/pentium4/sb/25366521.pdf))

## 4. Separate physical backing from virtual mappings

**Recommendation:** retain one 64 MB physical backing plus explicit virtual mappings, permissions, and backing-page identities. Guest addresses remain `u32`; host pointers remain outside guest memory. Cxbx separates user, physical-map, system, tiled/write-combined, and uncached regions; xemu distinguishes NV2A MMIO from shared-RAM apertures. These do not justify a universal high-address mask. ([Cxbx ranges](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/common/AddressRanges.h), [physical mappings](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/memory-manager/PhysicalMemory.cpp), [xemu NV2A](https://github.com/xemu-project/xemu/blob/master/hw/xbox/nv2a/nv2a.c))

Both target hosts are little-endian. Begin with checked little-endian helpers for every width, including 64/80/128-bit operands. Compute 32-bit effective-address wrap separately from whole-access validity: validate the entire byte span, overflow, mapping boundaries, permissions, and instruction alignment before touching backing memory. Unaligned ordinary accesses and alignment-sensitive SIMD instructions differ. Cross-page accesses require a checked slow path with correct fault/write behavior.

A private single-thread mode can use base-plus-offset and `memcpy` after full RAM/mapping checks. Shared mode uses synchronized helpers. Sparse mappings cost more than blind indexing but preserve aliases, holes, and faults. A reserved host arena is a later implementation option, not the memory contract. Classify MMIO before aliases; NV2A accesses in its `0xFD000000–0xFDFFFFFF` aperture produce diagnostic unsupported-device events rather than RAM access or device emulation. ([XboxDev memory map](https://xboxdevwiki.net/Memory))

## 5. Give threads private state and synchronized shared services

**Recommendation:** each Thread owns a Cpu and ExecCtx; Runtime owns shared memory, Dispatch, kernel objects, and HLE services. ExecCtx holds thread identity, trace policy, and shared Runtime access, without a second aliasing CPU reference. Eventually use one host thread per guest thread, initially with an execution gate serializing guest execution for determinism.

FS addresses guest KPCR/TIB state, not the host's TLS or a Windows TEB. Cxbx identifies FS+0 as the exception chain, FS+0x24 as IRQL, and FS+0x28 as current KTHREAD; KTHREAD+0x28 then addresses TlsData. The original single CPU has one KPCR view. Model that view across scheduling, with guest stacks, TLS template/zero-fill, and kernel structures. Subsequent threads enter through their guest SystemRoutine. ([Cxbx types](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/common/types.h), [FS setup](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/support/EmuFS.cpp), [nxdk startup](https://github.com/XboxDev/nxdk/blob/master/lib/winapi/thread.c))

Following nxdk's startup contract, allocate `align_up(raw_size + zero_fill, 16) + 4` bytes, initialize the leading guest TLS pointer to the following payload, copy the XBE raw template, and zero its tail. The aligned payload size and leading pointer are guest layout requirements, not Windows TEB/TLS-slot semantics. ([nxdk startup](https://github.com/XboxDev/nxdk/blob/master/lib/winapi/thread.c))

For initial concurrent execution, synchronize **all ordinary shared-memory accesses**, including HLE reads/writes, behind one shared backing-memory mutex and runtime guards. No byte slices escape a guard; generated C receives only accessors in shared mode. Complete LOCK-prefixed RMW and memory XCHG occur under one operation-wide lock, including alias identity and flags/results. Release memory/Dispatch/service locks before callbacks, waits, or reentry.

This conservative sequentially consistent baseline is slower and restricts schedules; it avoids C/Rust data races and concurrent `&mut Guest` aliases. Volatile is insufficient. Later atomics/page locks require a documented x86-ordering model on arm64, including ordinary accesses and fences, not just atomic kernel helpers. Match waits, wakeups, suspended starts, and scheduling effects through replay before enabling parallel performance paths.

## 6. Represent faults and SEH as guest transfers

**Recommendation:** initially stop on unsupported instructions, kernel services, exceptions, and `int3`, with structured diagnostics. Allow guest FS:0 chain construction, but do not pretend SEH succeeds. Record faulting instruction PC, exception kind/address, and canonical state. For `int3`, separately record the instruction PC and architectural saved next EIP; they are not interchangeable.

Eventually explicit Fault/Unwind/Resume outcomes walk guest registration/scope tables, invoke filters/finally funclets with the correct containing frame, and resume only validated PCs. Extend offline splitting/resume metadata as needed. nxdk's SEH implementation demonstrates FS:0 manipulation and frame-sensitive filter/finally behavior; Cxbx's unimplemented Rtl exception routines cannot establish completeness. ([nxdk SEH](https://github.com/XboxDev/nxdk/blob/master/lib/xboxrt/vcruntime/excpt.cpp), [Cxbx Rtl](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlRtl.cpp))

The runner makes this possible without host stack unwinding. The cost is explicit exception metadata and resumable state machines. Never unwind Rust panics/C++ exceptions or longjmp across Rust/C frames to implement guest SEH. Deferred x87 exceptions and restart/partial-progress rules need separate validation; unsupported cases remain visible stops.

## 7. Keep the native ABI uniform and guest contracts explicit

**Recommendation:** expose a versioned C ABI: `CpuAbi` and `TransferAbi` use `#[repr(C)]`, fixed-width fields, explicit padding, and integer tags/flags. A proposed generated-handler signature is:

```rust
unsafe extern "C" fn(
    cpu: *mut CpuAbi,
    exec: *mut ExecCtxOpaque,
    start_pc: u32,
) -> TransferAbi;
```

Opaque ExecCtx accessors expose memory/services; Rust references, HashMaps, enums, and host-layout booleans do not cross this boundary. Specify ext80 storage independently of native `long double`. Verify layouts and struct-return ABI on both targets when implementing. Safe Rust adapters can retain idiomatic interfaces behind a small audited FFI layer.

TransferAbi should carry fixed-width kind, target/resume PC, and diagnostic/event identifiers; richer fault records live in ExecCtx-owned storage. Define Call, Jump, Return, Fault, Stop, and eventual Unwind/Resume tags. The ABI is a transport contract, not a native representation of guest stdcall or LTCG. Version mismatches fail at registration/loading rather than corrupting CPU state.

Fix [Dispatch](../../crates/runtime/src/dispatch.rs)'s current `fn(&mut Cpu, &mut Guest)` shape: resolve using shared Runtime access, copy/pin a handler, release the table lock, then invoke it with separate mutable CPU/context access. Handler invocation cannot borrow Dispatch exclusively or retain a lock across nested guest calls. Keep a separate generated-original resolver for differential replay and call-through.

Patch function import slots with validated guest dispatch tokens/addresses, never host pointers. DATA exports instead point to guest-backed data with synchronized updates. Preserve indirect load semantics: a thunk shortcut must observe runtime slot rewrites rather than hard-code its original ordinal. Kernel ABI metadata is individual: most stdcall assumptions do not cover fastcall Interlocked routines. ([nxdk declarations](https://github.com/XboxDev/nxdk/blob/master/lib/xboxkrnl/xboxkrnl.h), [DATA exports](https://github.com/XboxDev/nxdk/blob/master/lib/xboxkrnl/xboxkrnl.exe.def))

For each override record image hash/address, argument/result locations, stack cleanup, clobbers/preserved state, memory effects, entry aliases, and continuation behavior. For example, this hypothetical two-argument stdcall override illustrates guest argument/return handling, not a recovered Halo function:

```rust
fn add_u32(cpu: &mut Cpu, ctx: &mut ExecCtx) -> Result<Transfer, GuestFault> {
    let a = ctx.read_u32(cpu.esp.wrapping_add(4))?;
    let b = ctx.read_u32(cpu.esp.wrapping_add(8))?;
    cpu.eax = a.wrapping_add(b);
    ctx.ret(cpu, 8) // read guest return; ESP += 4 + 8; Return(target)
}
```

Helpers here are proposed fallible APIs; `ctx.ret` also returns `Result<Transfer, GuestFault>`, and the wrapper preserves all other contracted state. LTCG needs address-specific register mappings instead: Cxbx has D3D variants taking arguments in EAX/EDI/EBX, but that is not proof of Halo's mapping. ([LTCG wrappers](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/hle/D3D8/Direct3D9/Direct3D9.cpp))

D3D HLE must cover surviving entries and inlined state/submission sites through explicit region hooks or larger semantic wrappers. Reconcile guest-written render state at draw boundaries; preserve guest descriptors/pointer identity, resource contents, shader/combiner translation, and completion effects. Cxbx's state mapping and push-buffer code expose these obligations. Unknown MMIO paths stop until a higher-level replacement exists; they do not expand scope into NV2A emulation. This trades broad mechanical coverage for verified semantic interception. ([render states](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/hle/D3D8/Direct3D9/RenderStates.cpp), [push buffers](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/hle/D3D8/XbPushBuffer.cpp))

## 8. Replay complete pre-state in isolated clones

**Recommendation:** capture CPU, mappings, the full 64 MB pre-state, runtime allocation/handle state, and ordered external events. Full copies first, COW later: recording only bytes read by the generated oracle is insufficient because a replacement can read different bytes. Run generated-original and override in two isolated clones from identical pre-state. Bypass only the override under test, with a pinned policy/version for other replacements.

Record both read/write sets; compare the union of dirty bytes, control outcome/ESP, defined/live registers and flags, x87 values/environment, XMM/MXCSR, allocations/handles, and ordered service events. Mask architecturally undefined state explicitly; do not broadly tolerate FP differences. Any tolerance is function-specific and justified. Reads outside captured mappings/state, unexpected external events, or unreplayed nondeterminism invalidate replay rather than obtaining arbitrary host results. Overrides may read different memory within the complete captured state.

At opaque boundaries, treat all defined architectural state as live unless a documented contract proves otherwise. Comparing generated builds on arm64 and x86-64 helps detect backend/host differences, but both can share the same incorrect lifter helper. Keep that evidence separate from independent CPU validation and from replacement equivalence.

Replay time, RNG, file/input, kernel, graphics/audio, and thread events at service boundaries. Begin with single-thread leaves, then stopped-world captures and explicit schedules. Full snapshots cost memory/copy time but make replay independent of oracle access patterns. Logic rewrites can share established HLE event replay; HLE itself needs independent Xbox/xemu trace validation. Generated D3D code reaching MMIO is not an executable native hardware oracle.

Generated-versus-override agreement tests replacements, not CPU semantics shared by both. Validate instruction helpers separately against independent native x86 cases and Xbox/xemu traces, especially PIII-specific FP behavior. Remill's native instruction testing is a useful precedent; emulator agreement still needs hardware checks where disputed. ([Remill testing](https://github.com/lifting-bits/remill/blob/master/docs/ADD_AN_INSTRUCTION.md))

## 9. Stop at the first kernel boundary

**Local evidence:** the analyzed XBE SHA256 is `03215919bb7163259257d361f4c7bf802a7ab12aa85e2689436369b5c427935d`. Entry is `0x2d0aee`; its call at `0x2d0b42` reaches generated CreateThread at `0x2d21c6`. That wrapper calls through slot `0x4115c4` at PC `0x2d21ff`. Ordinal 255 is independently named `PsCreateSystemThreadEx`, exported as stdcall `@40` by nxdk. ([export definition](https://github.com/XboxDev/nxdk/blob/master/lib/xboxkrnl/xboxkrnl.exe.def))

Implement only the reached instructions and CFG initially. Load headers at base `0x10000`, sections at their virtual addresses, and zero virtual tails with validated ranges. Headers matter: entry reads header size `0x2000` at `0x10108` and certificate pointer `0x10184` at `0x10118`, potentially clamping certificate size. This is not heap initialization. [The XBE parser](../../crates/xbe/src/lib.rs) exposes header fields and `tls_addr`, but still needs a parsed TLS directory/startup contract.

Provision a valid mapped guest stack using stack-commit metadata (`0x40000`) and explicit harness termination. FS/KPCR can remain a minimal skeleton: no FS read occurs before this boundary, so the smallest slice does not require a full thread runtime. TLS directory `0x411a30` has empty raw bounds, index address `0x480100`, no callbacks, and `0x90` zero-fill. Execute startup arithmetic rather than pre-seeding expected stores: the path should write `0x94` to global `0x55f138` and `0xffffffdb` to the TLS index. Full entry-thread TLS setup remains trace-dependent.

The observed sizing is aligned payload plus a leading four-byte slot; signed division of `0x94` by minus four produces index minus 37. Thus this small fixture checks signed arithmetic and actual stores as well as instruction decoding. The generated CreateThread wrapper must run, rather than being replaced before the measured kernel boundary.

At the boundary, verified useful arguments include ThreadHandle=`EBP+0xc` in CreateThread's frame, KernelStackSize=`0x40000`, TlsDataSize=`0x94`, StartRoutine=`0x2d0a7a`, and SystemRoutine=`0x2d212e`; extension size, ThreadId pointer, context, suspended, and debugger arguments are zero. Preserve all ten arguments for inspection.

**Acceptance ends before kernel handling:** guest return `0x2d2205` has been pushed, the slot resolves to ordinal 255, and the runner emits a stop/event with canonical CPU/stack state. Register stop handlers for every function import; require no kernel implementation, created-thread execution, graphics, or audio. This narrow milestone exercises loading, arithmetic, stack, direct calls, indirect thunk loading, and call-transfer/entry routing. No `ret` or resume executes yet; return/resume behavior requires separate fixtures.

An optional subsequent milestone implements correct stdcall forty-byte argument cleanup, valid handle/output writes, and thread-object/startup behavior. Returning fake zero success would conceal missing contracts and is not acceptance.

## Runtime changes and evidence still needed

| Existing source | Proposed change |
|---|---|
| [cpu.rs](../../crates/runtime/src/cpu.rs): bool flags, f64 x87, zero defaults | Versioned CpuAbi, EIP/FS/full flag policy, ext80/environment, explicit initialization. |
| [guest.rs](../../crates/runtime/src/guest.rs): checked flat backing | Virtual mappings, all-width/fault/atomic helpers, synchronized HLE access, snapshots. |
| [dispatch.rs](../../crates/runtime/src/dispatch.rs): override-first maps | Context-aware handler ABI, explicit runner, exact entry/resume maps, original resolver. |
| [analyze](../../crates/analyze/src/lib.rs), [xbe](../../crates/xbe/src/lib.rs) | Validated CFG/alias/table metadata; loader/TLS contracts and image identity. |

Unresolved evidence: complete reachable opcode/CFG and interior-entry inventory; code mutation; actual mappings/tiling aliases; entry-thread FS/TLS/SEH setup; x87 control/status dependencies and deferred exceptions; PIII approximation/denormal behavior; synchronization schedules; D3D inline interception coverage, shader/resource formats, and completion semantics. Pin upstream revisions for implementation; cited branch/wiki links are moving references. No source establishes a measured backend winner or complete Halo 2 compatibility.

Future acceptance should progress from integer/flag/fault and FP instruction fixtures, to the entry-boundary snapshot on both Macs, to changed returns/tail chains/interior aliases, guest SEH, atomics/alias coherence, and isolated override replay. Validate D3D semantic events against Xbox/xemu traces before image comparisons. Include reserved DAZ, FP precision/range/NaNs, SIMD alignment, thunk rewrites, HLE callbacks without held locks, and unknown-target failures.

Future benchmarks should separate discovery, lowering, emission, compilation, linking, and shader work. Compare C11/LLVM/Cranelift using identical semantics and fixtures, recording compiler/SDK versions, hardware, optimization/debug options, shard sizes, parallelism, clean/incremental wall time, peak RSS, object/debug size, and runtime transfer/helper cost. Measure a single regenerated function and an override-only edit as well as the full image. These results should guide optimizations without weakening the reference contracts.
