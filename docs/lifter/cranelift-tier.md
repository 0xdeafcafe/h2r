# A Cranelift fast tier for the first build

Status: design, 2026-11-18, with the first stages built (`crates/clif`).
Where this text and the code differ, the code wins.

The cold `h2 build` spends about eleven minutes in clang on 1,001 shards of
generated C ([build-perf.md](build-perf.md)); everything before clang takes
under two seconds when cached. This note adds a second backend that lowers
the same planned IR (`emit::Owner`) straight to machine code with
Cranelift, so a first build is ready in seconds. Clang's optimised build
still happens, in the background, and replaces the fast one when it is
done.

## 1. What the fast tier is

- **Input**: exactly what the C emitter gets: `emit::plan` output, one
  `Owner` per function, after the same `opt` passes, with the same
  selectors, resumes, interior entries and loop headers. Planning stays in
  one place, so both tiers register the same addresses.
- **Output**: a host dylib with the same exports as clang's module:
  `h2gen_module`, `h2gen_module_abi_version`, the boot bridge's install
  functions and one `h2fn_<entry>` per owner with the C signature
  `TransferAbi h2fn(CpuAbi *, ExecCtxOpaque *, uint32_t start)`. Boot,
  the runtime's dispatch and the test host load it with no change.
- **Not in it**: no new runtime ABI, no new kind of handler. A Cranelift
  handler is an `H2Handler` like any other.

## 2. Where the code lives: objects linked into a dylib

Three options were weighed.

| | In-process JIT at game start | JIT, cached as a blob | Objects per shard, linked into a dylib |
|---|---|---|---|
| Start-up cost | compile all 18,647 functions every launch | custom loader, relocation, W^X, cache keys | `dlopen`, as today |
| New runtime code | JIT module lifetime, symbol resolution | a loader and its format | none |
| Debuggers, profilers, `perf` crate's symbol map | JIT maps needed | JIT maps needed | ordinary symbols |
| Windows/tvOS later | no JIT on tvOS (no W+X pages) | same | the same objects and link step |
| Cache | none | new | the existing per-object cache |

**Choice: `cranelift-object`, one object per shard, linked by the same
`host_dylib` step clang's objects use.** The decisive points are tvOS (no
runtime code generation allowed), zero change to boot and the runtime,
and that every existing loader, test host, profiler and stamp check keeps
working. Start-up cost is the same as today's `dlopen`. The cost is the
link (2.3 s today for 1,001 objects, less for fewer, larger shards).

The module is three kinds of object:

1. **Shards** from Cranelift: the handlers, sharded like the C (entry >>
   12), each with its own `h2gen_abi_version_<stem>` probe, also written
   in Cranelift.
2. **The registry**, `h2clif_registry.c`: the entry and resume tables,
   generated as C exactly as emit's `registry` does, with features 0 (no
   accounting, no direct calls; see section 5). It is plain data, so clang
   compiles it at `-O0` in well under a second.
3. **The helper library**, `h2clif_helpers.c`: a few hundred lines that
   include the same `h2gen.h` and expose the SSE intrinsics (`h2i_sse_*`,
   which take `h2u128` by value and are `static inline`) as out-of-line
   functions with pointer arguments. It is not derived from the disc and is
   the same for every image, so it compiles once and stays in the object
   cache.

Plus the bridge (`boot::BRIDGE`, or the test host's), as now.

## 3. Lowering

### 3.1 Function shape

One Cranelift function per owner, signature `(sret i64, i64 cpu, i64 ctx,
i32 start)` in the target's C convention. Cranelift passes a
`StructReturn` parameter in x8 on AArch64 and in the first integer
register on x86-64 System V, which is how C returns the 28-byte
`TransferAbi`, so the symbol is callable as an `H2Handler`.

- **Entry**: load the eight GPRs, EFLAGS and its defined mask from
  `CpuAbi` into Cranelift variables, then switch on `start` over the
  owner's selectors (`cranelift_frontend::Switch`); any other value stops
  with `H2SHIM_BAD_ENTRY`, as the C does.
- **Blocks**: one Cranelift block per IR block, split again at any
  selector inside a block (REP restart points). A loop header gets a poll
  block in front of it, and a selector for a header enters past the poll,
  like the C's `P_` labels.
- **Exit**: every way out jumps to one shared exit block with the
  transfer's kind, route, target, resume, event and EIP as block
  parameters. It spills the variables to `CpuAbi` (EFLAGS with bit 1 set),
  writes `cpu->eip`, stores the `TransferAbi` through the sret pointer and
  returns. One spill per handler, as `H2_EPILOGUE` has.

All eight registers are kept in variables (the shape the C had before
register homes): the loads of registers a handler never writes are dead after
Cranelift's SSA construction except at the spill, which writes back what
was read. Nothing else writes those `CpuAbi` fields while a handler runs
except the atomic accessors, around which the variables are spilled and
reloaded.

### 3.2 Values

| IR width | Cranelift type |
|---|---|
| W1 | `i8`, 0 or 1 |
| W8, W16, W32, W64 | `i8`, `i16`, `i32`, `i64` |
| W80, W128 | `i128` (W80 in the low 80 bits) |

Temps are instruction-local and single-assignment, so they map to
Cranelift values directly (a map per instruction, cleared at each one).
`Repeat` bodies redefine their temps per trip, which is the same thing
inside a Cranelift loop body block.

### 3.3 Integer operations

IR operations are total; Cranelift's are not, so each partial one is
guarded:

- `Shl`/`Shr`: `select(count >= width, 0, op)`, since Cranelift masks the
  count. `Sar`: the count is clamped to `width - 1` first.
- `Rol`/`Ror`: Cranelift's rotates already take the count modulo the
  width.
- `Udiv`/`Urem`/`Sdiv`/`Srem`: the divisor is replaced by 1 when zero (and
  for signed, the `MIN / -1` case by a negate), then the IR's defined
  results are selected: quotient 0 and remainder = dividend for a zero
  divisor; `MIN / -1` = `MIN`, `MIN % -1` = 0. Cranelift's division traps
  are never reachable. 128-bit division has no Cranelift lowering on
  AArch64; the lifter never produces it, and the backend refuses the
  function (it is then left to the slow path, section 6).
- `Parity`: `popcnt(low byte)` and the inverted low bit. `Ctz`/`Clz`:
  Cranelift's own, which give the width for zero, as the IR does.
- Compares are `icmp`, whose `i8` 0/1 result is a W1 value. Casts are
  `uextend`, `sextend`, `ireduce`, with W1 sign extension as a negate and
  narrowing to W1 or W80 as a mask.

### 3.4 Flags

The C keeps EFLAGS in one local and does a read-modify-write per flag.
Cranelift does better with **one variable per modelled flag and one per
defined bit** (seven of each, `i8`), plus the other EFLAGS bits and their
defined bits in two `i32` variables. A flag that is overwritten before it
is read is then a dead value and Cranelift removes the computation, which
is what the C's lazy flags (`emit::lazy`) work for. So `Stmt::LazyFlags`
is lowered **eagerly**: its formula's flags are computed in place and
written to the variables. That is exactly the IR's definition of the
statement; the dead ones fold away.

`ReadEflags` assembles the word from the variables, `WriteEflags` splits
its value under the mask, and the exit assembles EFLAGS and the defined
mask once. `FlagValue::Undefined` stores 0 and clears the defined bit, as
the IR requires.

### 3.5 Memory

The first version calls the ABI's checked accessors for every guest load
and store: `h2abi_v1_read(ctx, pc, addr, width, 1, buf)` with `buf` a
stack slot, and `h2abi_v1_write` likewise. A non-zero status is the event
id; the handler leaves through the exit with `FAULT`. `Segment::Fs` adds
`cpu->fs_base`. This is the C's path with `H2_FASTMEM=0` and no
accounting, which the runtime supports and charges per call.

The inline RAM path of `h2fastmem.h` (the gate, the page table, the dirty
bits) is the next step for speed. It is a fixed instruction sequence that
can be built in Cranelift once, as a helper the lowering inlines; until
then the fast tier is slower per access, which is acceptable for a tier
that lives only until clang finishes.

### 3.6 Control flow and transfers

- Local branches, jumps, fall-through and switch cases are Cranelift
  jumps. A tail destination leaves with `JUMP`/`ENTRY`.
- `Switch` compares the loaded target against the validated cases
  (`Switch` again) and stops with `UNKNOWN_TARGET` otherwise.
- Calls leave with `CALL`/`ENTRY` and the continuation as resume PC;
  returns with `RETURN`/`RETURN_TARGET`; indirect jumps with
  `JUMP`/`ENTRY`. The runner dispatches all of them, so a Cranelift
  handler and a clang one can call each other freely: the only interface
  is the transfer, the entry table and the resume table.
- **Direct calls** (`H2_DCALL`) are not in the first version: every call
  goes back to the runner. The module declares no
  `H2GEN_FEATURE_DIRECT_CALLS`, which the runtime already handles. They
  can be added later by porting `h2_direct_enter`/`h2_direct_back`; the
  slot table layout is in `h2gen.h`.
- **Loop safe points**: each loop header polls the context's preempt word
  (the first pointer in the context) with one load and leaves with
  `JUMP`/`RESUME` to the header when it is set, as `H2_LOOP_POLL` does.

### 3.7 Faults, stops and REP

Guest exceptions and stops call `h2shim_v1_event` (traps report the next
instruction as the saved EIP), unsupported instructions call
`h2shim_v1_unsupported` with the mnemonic and detail as constant strings
in the object. `Repeat` is a three-block loop (test ECX, body, decrement
and the REPE/REPNE test); the flag variables are saved before it and
restored on every exit from inside the body, matching the IR's contract
that a faulting `repe cmpsb` reports its starting flags. The C's bulk
`rep movs` copy is a fast-memory feature and is not used.

### 3.8 Atomics

As in the C: the pure prefix, then spill every variable, one call to
`h2abi_v1_atomic_rmw`, `xchg` or `cmpxchg` with the old value written to a
stack slot, leave with `FAULT` on failure, reload the variables, and merge
the old value into its register.

### 3.9 x87 and SSE

Both go through calls first, correct before fast:

- **x87**: one call per `X87Op` to the `h2x87_v1_*` helpers the bridge
  already exports (`crates/x87`, the authority the native C path is
  checked against). Status handling follows the C macros: `MF` raises #MF
  with the status word, `UNMODELLED` stops with code 12, `SUPPRESS` skips
  the store or compare. FIP/FOP/FDP and the selectors are written to
  `X87Abi` directly at each `Mark`, so `X87Abi` is current at every helper
  call and every exit, which is what the C's locals-and-spill achieves.
- **SSE and the small intrinsics**: `MXCSR`, control and status word
  reads, `MmxEnter`, `Emms`, `X87Wait`, XMM and MMX register access are a
  few loads and stores and are inlined. The 28 `SseOp`s call the helper
  library's out-of-line wrappers of the same `h2i_sse_*` functions clang's
  tier inlines, with the same `H2_SSE_NATIVE` guard.
- `ReadTsc` calls `h2shim_v1_rdtsc`.

Native Cranelift FP for the guarded common cases (as `h2x87_native.h` and
`h2sse_native.h` do in C) is a later optimisation and must be proved equal
to the authority the same way.

## 4. Tiering

**The game starts on the fast tier; clang's build runs in the background;
the next start uses clang's module.** No hot swap.

- `h2 build --tier fast` writes `<out>/fast/libh2game.dylib` and its own
  stamp (`crates/boot/src/tier.rs`): it starts `tier fast` and names the
  image, the generator's sources plus crates/clif and Cargo.lock, the IR
  passes, Cranelift's optimisation level and the module ABI. Neither
  tier's stamp passes for the other's. Plain `h2 build` still builds
  clang's module in `<out>/libh2game.dylib`, as before.
- `h2 run` and `h2 play` (`--tier auto`, the default, or `H2_TIER`) run
  clang's module when it is current; else the fast tier's, building it
  first if need be, and start clang's build as a separate process:
  `h2 build <disc> --out <out> --tier clang --background`, at nice 19
  (clang inherits it), logging to `<out>/clang-build.log`, and leaving at
  once when another background build holds `<out>/clang-build.lock`. It
  builds in place: `boot::build` removes the old stamp first and writes the
  new one last, so a half-built module is never current. The next start
  finds clang's module current and uses it. `--tier fast` runs the fast
  tier only; `--tier clang` is the old foreground build. `h2 serve` never
  builds and runs clang's module unless told `--tier`.
- **Why not hot-swap**: a running guest has handlers on the host stack
  (direct calls), resumes recorded as `(owner handler, start)`, and the
  runtime's direct slots keyed by handler pointer and dispatch generation.
  Swapping at a safe point is feasible in principle, since every resume
  and entry is by guest address and a dispatch generation bump invalidates
  the slots, but it needs every guest thread parked outside generated
  code, and the old library kept loaded until then. The payoff is one
  session; the risk is a whole class of use-after-unload bugs. A later
  stage can add it behind a flag once the fast tier is trusted.

## 5. Determinism and lockstep co-op

What must be identical, bit for bit, between the two tiers on the same
input:

1. All architectural state at every transfer: GPRs, EFLAGS and the defined
   mask (undefined flags stored as 0), EIP, all eight 80-bit x87 registers,
   control/status/tag words and the instruction/data pointers, XMM, MXCSR.
2. Guest memory, and the order and content of every event (fault, stop,
   unsupported).
3. **Where the guest can be interrupted.** The scheduler preempts on the
   access budget and at safe points (loop headers, handler exits, the
   runner). Lockstep needs the guest's threads to interleave the same way
   on every machine.

Points 1 and 2 hold by construction: integer semantics are the IR's
(checked against the interpreter), and all FP arithmetic is the same
compiled helper code the clang tier uses or its proved-equal native
twins. No host FP instruction is emitted by Cranelift in this version.

Point 3 does **not** hold across tiers. The fast tier charges every access
through the runtime and makes no direct calls; clang's module counts
accesses itself and calls handlers directly, which moves the runner's safe
points. Two machines on different tiers can therefore preempt at
different instructions. So:

- **Lockstep sessions require the same module on every peer.** The
  module's stamp (which names the tier) is part of the session handshake,
  and a peer on a different stamp is refused with a message naming both.
  In practice, co-op waits for the clang build; a host on the fast tier
  can host only peers also on the fast tier of the same build.
- Single-player and non-lockstep sessions may use either tier.

The co-op crates are outside this lane; the handshake check is a separate
change for whoever owns them.

## 6. What the fast tier refuses

A function the backend cannot lower (an unsupported construct, a
Cranelift verifier error) fails the fast build loudly during development.
For production the plan is to emit a stop handler for it (`STOP` with
`H2SHIM_UNKNOWN_TARGET`), which the boot fallback already recovers from,
and report the count; it must be zero on the retail image.

## 7. Proof

- `crates/clif/tests/differential.rs` runs every synthetic fixture of
  `crates/emit/tests/differential.rs` (shared source) through the IR
  interpreter, the clang module and the Cranelift module from the same
  random states, and requires identical ends, CPU state and memory.
- The x87 and SSE fixtures of the same harness, then `native_x87.rs` and
  `native_sse.rs`, cover the FP paths.
- Timing: `crates/clif/tests/image.rs` (ignored; `H2_XBE`) compiles the
  retail XBE, compile-only: 18,647 owners in 1,000 shards, 37.5 s wall on
  4 threads of a heavily loaded machine (78 s of CPU), 206 MB dylib.
- `crates/boot/tests/fast_tier.rs`: a fast-tier module links with boot's
  bridges and exports what `load_module` looks up; `tier.rs`'s tests
  cover the stamps, the choice of module and the background command.

## 8. Stages

1. (a) `crates/clif`: integer, control flow, memory, transfers, flags,
   atomics, REP; then x87 and SSE through helper calls.
2. (b) the differential harness above, against the interpreter and C.
3. (c) whole-image fast build, timed; target 30 s cold.
4. (d) boot loads either tier, the background clang build, the switch at
   the next start.
5. Later: inline fast memory, direct calls, native FP, hot swap.
