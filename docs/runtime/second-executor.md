# A second executor

Status: **inventory; the view and the executor are being built.** It is the prerequisite for pipelined rendering
(docs/graphics/pipelined-render.md): run guest code on a second host
thread, at the same time as the game thread, against a copy-on-write
snapshot of guest RAM.

Today one host thread runs every guest thread. A guest thread is a CPU
state and an `ExecCtx`, and the kernel's cooperative scheduler
(`crates/kernel` `threads.rs`) hands the one host thread from one to the
next (docs/runtime/snapshots.md, "How guest threads run"). Nothing in the
runtime assumed a second host thread would run guest code; this page is
the inventory of what that takes, then what was built.

## 1. Inventory

Every piece of state generated code or the runner touches, and what it
is to a second executor:

- **per-executor**: each executor has its own; nothing to do but make
  one;
- **shared read-only**: one copy, both read it, nobody writes it while
  both run;
- **shared mutable**: one copy that both would write; it needs a lock,
  or splitting into one per executor.

Headline: **almost everything the hot path touches is already per
`ExecCtx` or per `Runtime`.** The guest RAM, its gate and its fast page
table belong to a `Runtime` (one `Memory`); the resolve cache, direct-call
slots, slice and events belong to an `ExecCtx`. So a second executor is a
**second `Runtime` over a COW copy of the `Guest`, sharing only the
generated code (function pointers) and a copy of the dispatch tables**,
with its own `ExecCtx`. The shared mutable state that is left is the
kernel (refused), the HLE overrides (refused unless allowed), the
time-stamp counter (split, frozen), and process-wide diagnostics that are
already atomic.

### Runtime (`crates/runtime`), per `Runtime`

| State | Where | What it is to a second executor |
|---|---|---|
| Guest RAM, 64 MB | `Guest::ram` | shared mutable: **split** by a COW copy (section 2) |
| Mappings, reservations, claims, code guards | `Guest` | shared read-only while both run (the copy has its own) |
| Fast page table (1M entries) | `Guest::fast_pages` | per executor: the copy builds its own from the same mappings |
| Mapping hints | `Guest::mapping_hints` | per Guest (atomics) |
| Layout identity | `Guest::layout`, global `NEXT_LAYOUT` | per Guest; the counter is a global atomic, already thread-safe |
| Write watch, dirty bits | `Guest::watch` | per Guest: the copy's own watch is how its writes are collected |
| Access trap (`render-census`) | `Guest::trap` | per Guest; the copy has none |
| Guest mutex + contention counters | `Memory::guest`, `LockContention` | per Runtime |
| C gate, C RAM view, C dirty bits | `MemoryShared` (`h2fastmem.h` `H2MemoryShared`) | per Runtime: two runtimes, two gates; nothing serialises the threads |
| `ready`, `enabled`, `layout` epoch | `Memory` | per Runtime |
| Dispatch tables | `Runtime::dispatch` (`RwLock<Dispatch>`) | generated handlers and resumes: shared read-only (fn pointers), **copied** into the executor's own `Dispatch` once; overrides: shared mutable HLE, refused unless allowed (below) |
| Dispatch generation | `Runtime::dispatch_gen` | per Runtime |
| Stop recovery hook | `Dispatch::stop_recovery` | copied (an `Arc`'d closure; it must be thread-safe, it is `Send + Sync`) |
| Interrupt queue | `Runtime::interrupts` | per Runtime: nothing raises on the executor's |
| Time-stamp counter (`rdtsc`) | `Runtime::tsc` (`Tsc { clock, last }`) | shared mutable in meaning (the Xbox has one counter): **split**; the executor's starts at the main counter's `last` and follows a frozen clock (the guest time at the snapshot) |
| Preempt word registry | `Runtime::words` | per Runtime; `request_preempt` on the executor's runtime cancels its run |

### Runtime, per `ExecCtx` (one per guest thread today)

| State | Where | What it is |
|---|---|---|
| Preempt word (first field) | `ExecCtx::preempt`, `word` | per executor |
| Direct-call slots (second field) | `ExecCtx::direct_ptr`, `direct` (`DirectState`) | per executor; the slot count is a process `OnceLock` read once (read-only) |
| Resolve cache (4096 ways) | `ExecCtx::resolved` | per executor; keyed on its runtime's dispatch generation and layout |
| Slice: accesses spent, budget, expiry | `ExecCtx::slice` | per executor. This is the logical clock's input: generated code charges it (`h2abi_v1_charge`, `h2abi_v1_exit`, the inline count in `FastAccess`) and reads it (`h2abi_v1_budget`); only the kernel scheduler turns it into guest time (`take_slice_spent`). An executor with no scheduler never touches the main thread's budget |
| Inline RAM view handle | `ExecCtx::fast_memory` (`FastAccess`: gate pointer, count, budget, preempt pointer) | per executor, pointing at its own runtime's gate |
| Events, pending fault | `ExecCtx::events` | per executor |
| Continuations (Rust callbacks) | `ExecCtx::continuations` | per executor |
| Shadow call stack | `ExecCtx::frames` | per executor |
| Interrupt mask and token, preempted transfer | `ExecCtx` | per executor |
| Profile | `ExecCtx::profile` | per executor |

### Guest CPU and stack

`CpuAbi` (registers, flags, x87, SSE) is a value the runner borrows; the
executor has its own. The guest stack is guest RAM: in a COW copy the
executor may keep the game thread's stack addresses (its pushes land in
its private pages), or use any other mapped RW range.

### Process-wide (Rust)

| State | Where | What it is |
|---|---|---|
| `H2_ENTRY_STATS` counters | `entry_stats` `COUNTS` (atomics), `SITES` (mutex) | shared mutable, already thread-safe; diagnostics, off by default |
| Timeline host thread id, phase accounting | `timeline` thread-locals `HOST_TID`, `phase::LOCAL` | per host thread: the executor's thread gets its own track and its own phase totals (render-timing would have to add them in) |
| Timeline event buffer | `timeline` global state behind a mutex | shared mutable, already locked |
| D3D texture-descriptor memo | `d3d::texdesc` thread-locals | per host thread, a cache |

### Generated C (`crates/emit/c`, the emitted module)

| State | Where | What it is |
|---|---|---|
| Callback table | `bridge.c` `static H2Bridge bridge`, `bridge_x87.c` `h2x87_bridge` | process-wide, written once at module load: shared read-only |
| Gate, page table, RAM pointer, dirty bits | `H2MemoryShared` (through `FastAccess::memory`) | per Runtime (above) |
| Gate hold (`H2Gate`: held, dirtied page) | handler locals | per call, on the host stack |
| Access count and budget | `H2FastAccess` | per ExecCtx |
| Direct-call slots | `ExecCtx` second field | per ExecCtx |
| x87/SSE native paths | `h2x87_native.h`, `h2sse_native.h` | stateless; they read the host FPCR/MXCSR, which is per host thread and defaults to round-to-nearest with traps off: a fresh thread passes the check; nothing sets it |
| PGO counters (`-fprofile-instr-generate` builds only) | LLVM profile runtime | shared mutable and racy (non-atomic increments): counts from two threads may lose increments; harmless for a profile, never in a play build |
| `h2pgo_path` | `pgo.rs` shim | written once at load |

No `_Thread_local`/`__thread` exists in the runtime C or the emitted
module.

### Outside the runtime

| State | What it is to a second executor |
|---|---|
| Kernel (`crates/kernel`): scheduler, objects, clock, memory manager, files | shared mutable and single-threaded by design (one lock, cooperative). **Refused**: every kernel import token in the executor's dispatch is a refusal that ends the call with `ExecutorError::Refused` (section 3) |
| Kernel logical clock | advanced from `take_slice_spent` on the game thread; the executor's accesses are returned to the caller (`CallOutcome::accesses`) to charge at the join |
| HLE overrides (D3D, XAPI, feature hooks) | shared mutable, each with its own state and locking story. **Refused** unless the caller allows that override address; an allowed one runs on the executor's thread with the executor's `ExecCtx` and memory |
| D3D device | its own lock; reachable only through allowed D3D overrides |
