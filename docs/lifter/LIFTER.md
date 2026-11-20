# Lift Xbox code toward readable Rust

Status: Built. The pipeline described here runs; the Cranelift tier is partly built ([cranelift-tier.md](cranelift-tier.md)).
Code: `crates/analyze`, `crates/lift`, `crates/ir`, `crates/opt`, `crates/emit`, `crates/clif`, `crates/runtime`. Tests: `crates/lift/tests/`, `crates/emit/tests/`.
Switch: the build switches (`H2_OPT`, `H2_PASSES`, `H2_FASTMEM`, `H2_X87_NATIVE`, `H2_SSE_NATIVE` and others) are listed in [switches.md](../runtime/switches.md).

The lifter turns the retail XBE's 32-bit x86 code (Pentium III model, SSE1 and x87) into C11 through a backend-independent semantic IR. Clang builds the C into a host library that the Rust runtime loads. Every generated function carries explicit CPU state and returns control transfers to a Rust runner, so every function entry can resolve to a Rust replacement. Nothing derived from the disc is committed; generated files stay in the caller's output directory.

## How it works

1. `analyze::analyze` finds functions by recursive descent from the entry point and every direct call target, then adds speculative candidates (code references, data pointers, gap starts) that decode cleanly. It sizes switch tables from their guards (`crates/analyze/src/lib.rs`, `cfg.rs`). The discovery is not trusted to be complete: a target it missed stops the handler and the IR interpreter carries on (`crates/boot/src/fallback.rs`).
2. `lift::Lifter` walks each function's CFG and `lift::lower` turns one decoded instruction into one `ir::IrInsn`. An instruction the lifter does not implement becomes `ir::Stmt::Unsupported` and is counted in `LiftStats`.
3. `opt::optimize` runs the named passes (`dead-flags`, `lazy-flags`; see `crates/opt/README.md`) under one rule: state is observable only where a handler can stop.
4. `emit::plan` decides each owner's start selectors, resumes and loop safe points; `emit::c::function` writes one C handler per owner; `emit::write_module` groups handlers into 4 KiB entry-address shards (`emit::SHARD_SHIFT`), isolates owners over `emit::OVERSIZED`, and rewrites only files whose content changed. `emit::compile` builds the shards with clang and caches objects by content hash ([BUILD-CACHE.md](BUILD-CACHE.md)).
5. `crates/clif` lowers the same planned owners to Cranelift objects for the fast first build (`boot::tier`, [cranelift-tier.md](cranelift-tier.md)).

The CLI stages are `h2 analyze`, `h2 lift`, `h2 emit` and `h2 build` (`crates/cli/src/main.rs`).

## 1. Generate C11 first; keep semantics independent

The IR records instruction address and length, widths, register slices, defined and undefined flags, memory effects and control outcomes. It contains no C syntax and no host signatures (`crates/ir/src/lib.rs`). A reference interpreter (`ir::interp`) runs it, so every later stage can be checked against it.

The emitter uses unsigned fixed-width arithmetic, explicit truncation and sign extension, masked shifts, checked divide faults, and helpers for guest floating point. `emit::compile::CFLAGS` sets `-std=c11 -fno-strict-aliasing -fno-fast-math -ffp-contract=off`. `H2_OPT` and `H2_DEBUG` choose the optimisation and debug level (default `-O1 -gline-tables-only`, `emit::compile`). Generated files are never edited or committed; a fix belongs in the lifter, the metadata or a Rust override.

Shard names are a pure function of the entry address, so adding or removing a function never moves another into a different shard.

## 2. Preserve guest control flow with explicit transfers

One host function per guest function owner (`h2fn_<entry>`), with labels and gotos for its CFG and a start selector for each validated resume address. Its signature is `TransferAbi h2fn(CpuAbi *, ExecCtxOpaque *, uint32_t start)` (`crates/emit/src/c.rs`).

- A guest `call` evaluates its target, pushes the real guest return address into guest memory, commits CPU state and returns `H2_CALL`. `ret` reads the guest return address, adjusts ESP and returns `H2_RETURN`. Tail transfers return `H2_JUMP` (`crates/runtime/include/h2abi.h`).
- A branch, jump, fall-through or switch case that lands on another function's entry is a tail transfer (`ir::Dest::tail`), routed through dispatch, so an override of the inner function is honoured even when the outer function runs into it.
- Code shared without an entry is duplicated into each owner. Every address the runtime can enter from outside is registered once, against its canonical owner (`ir::canonical_owner`).
- A call continuation that is itself another function's entry is registered as a resume of that function's handler.
- The runner keeps an exact resume map from continuation addresses to owner and selector. A return to one resumes generated code without applying entry overrides; a changed return to a function entry goes through entry dispatch (`crates/runtime/src/runner.rs`, `dispatch.rs`).
- `analyze::NORETURN_KERNEL` lists the kernel ordinals that end a flow (49, 95, 96, 258).
- Unknown targets stop with the address and the reason (`H2SHIM_UNKNOWN_TARGET`, `H2SHIM_BAD_ENTRY`); only a return to `HARNESS_EXIT` ends a harness run successfully.

Rust overrides that call guest code use continuation closures keyed by a guest return token (`crates/runtime/src/continuation.rs`; `crates/runtime/README.md`).

## 3. Make CPU state authoritative before optimising it

`CpuAbi` (`crates/runtime/src/cpu.rs`) holds eight GPRs, EIP, the FS base, EFLAGS with a defined mask, the x87 registers and environment (`X87Abi`), eight XMM registers as bytes and MXCSR. Ext80 and XMM values are bytes, so the layout does not depend on host floating point. Undefined flags get deterministic stored values plus the defined mask, never compiler undef or poison.

A handler loads the registers it uses into locals and spills full state at every transfer, observable helper, fault and stop. Flag computation is lazy inside a handler (`lazy-flags`) and materialised at those same points.

`CpuAbi::xbox_thread()` sets the provisional Xbox thread-start x87 control word (`0x027f`) and the default MXCSR (`0x1f80`); `Default` differs only in the x87 control word. Floating-point semantics: exact software x87 in `crates/x87` ([FLOAT.md](FLOAT.md), [FLOAT-ABI.md](FLOAT-ABI.md)); SSE reference semantics in `crates/simd` ([simd.md](simd.md)); guarded native fast paths for both ([x87-native.md](x87-native.md), [sse-native.md](sse-native.md)).

## 4. Separate physical backing from virtual mappings

The runtime keeps one 64 MB physical backing and sorted virtual mappings with permissions (`crates/runtime/src/guest.rs`). Guest addresses are `u32`; host pointers never appear in guest memory. A checked access validates the whole span (overflow, mapping, permission) before touching bytes; NV2A addresses produce an unsupported-device fault. Generated code reaches memory through `h2abi_v1_read` and `h2abi_v1_write`, or through the inline RAM view under a shared gate (`H2_FASTMEM`). The protocol is in `crates/emit/RUNTIME-CONTRACT.md` section 1.1; measurements are in [../perf/guest-memory.md](../perf/guest-memory.md).

## 5. Give threads private state and synchronised shared services

Each guest thread owns a `CpuAbi` and an `ExecCtx`; the `Runtime` owns shared memory, dispatch, kernel objects and HLE services. All guest threads run on one host thread under a cooperative scheduler (`crates/kernel/src/threads.rs`); the scheduler's clock is the count of guest memory accesses, charged by handlers at their exits (`RUNTIME-CONTRACT.md` sections 1.5 and 4). Every shared-memory access, including HLE reads and writes, goes through a memory guard. Atomic read-modify-write instructions hold it for the whole operation (`RUNTIME-CONTRACT.md` section 1.2).

FS addresses guest KPCR and TIB state, not the host's thread-local storage.

## 6. Represent faults and SEH as guest transfers

A fault or stop is a transfer with an event id from `h2shim_v1_event` or `h2shim_v1_unsupported` (`RUNTIME-CONTRACT.md` sections 1.3 and 1.4). `cpu->eip` is the faulting instruction; `saved_eip` is what the guest would see (the next instruction for `int3`). Unsupported instructions, unmodelled `popfd` changes and unresolved targets stop with a diagnostic. Guest structured exception handling is not unwound by generated code; Rust panics and C++ exceptions never cross a C frame.

## 7. Keep the native ABI uniform and guest contracts explicit

The ABI is `crates/runtime/include/h2abi.h` (version 2). `CpuAbi`, `X87Abi` and `TransferAbi` are `#[repr(C)]` with fixed-width fields and asserted sizes and offsets. Generated modules export an ABI probe the runtime compares at registration. What a module imports, exports and how the runtime drives it is in `crates/emit/RUNTIME-CONTRACT.md`; the runtime side is `crates/runtime/README.md`.

## 8. Check generated code against independent semantics

The IR interpreter, the C backend and the Cranelift backend are compared against each other and against independent oracles: `crates/oracle` for integer and flag fixtures ([oracle.md](oracle.md)) and `crates/sse-oracle` for SSE ([sse-oracle.md](sse-oracle.md)). The lifter's own tests are in `crates/lift/tests/`; dispatch coverage on the retail image is in [dispatch-coverage.md](dispatch-coverage.md).

## 9. Stop at the first kernel boundary

The smallest end-to-end check runs the retail entry point to its first kernel call. `lift::harness::run_to_kernel` interprets from the entry to the call through the import slot for ordinal 255 (PsCreateSystemThreadEx) and checks the stack and arguments there (`crates/lift/tests/entry.rs`, `entry_runs_to_ps_create_system_thread_ex`); `crates/emit/tests/entry.rs` does the same through generated C, and `crates/lift/tests/semantics.rs` checks the TLS-index arithmetic on the way. They need the owner's XBE (`H2_XBE` or `disc/default.xbe`) and pass with a notice when it is absent.

## Evidence

- **Code**: the files named above.
- **Test**: `entry_runs_to_ps_create_system_thread_ex` (`crates/lift/tests/entry.rs`), `V` when the XBE is present.
- **Design**: the choice of C11 through clang as the optimising tier.

## Not verified

- Guest SEH dispatch through generated code.
- Behaviour of the Cranelift tier in a lockstep session with the clang tier ([cranelift-tier.md](cranelift-tier.md), section 5).

## Related

[ARCHITECTURE.md](../ARCHITECTURE.md), [BUILD-CACHE.md](BUILD-CACHE.md), [notes/precedents.md](notes/precedents.md), [notes/build-perf.md](notes/build-perf.md).
