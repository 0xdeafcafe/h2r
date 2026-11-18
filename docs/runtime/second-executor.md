# A second executor

Status: **runtime building block, tested on synthetic code; used by the
pipelined-render prototype** (`H2_PIPELINE_RENDER=1`,
docs/graphics/pipelined-render.md, "The prototype"). It is the prerequisite for pipelined rendering
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

## 2. Guest memory view

`Guest::cow_snapshot` (and `Guest::cow_refresh`, which reuses a view's
page table) gives a `Guest` with the same mappings and permissions over a
copy-on-write copy of physical RAM. Writes to either side stay on that
side; the view reads the bytes as they were at the snapshot.

**How.** Guest RAM is one 64 MB heap allocation (`vec![0; 64 MB]`). On
macOS a block that size is its own VM region, page-aligned, so
`mach_vm_remap(copy = TRUE)` of it gives a second mapping with copy
semantics: the kernel shares the physical pages, write-protects both
sides and copies a host page (16 KB on Apple silicon) on the first write
to it from either side. Setup is a VM map operation, not a 64 MB copy.
The view's RAM is then a mapping the runtime owns (`cow::Ram`, freed with
`mach_vm_deallocate`); the live Guest's stays the heap block it was, so
nothing on the live path changed. If the block is not page-aligned, or
off macOS, the copy falls back to a plain 64 MB `memcpy`
(`CopyKind::Copied`), correct but slower.

Why not the alternatives: RAM is not file-backed (no memfd/shm), so
`mmap(MAP_PRIVATE)` of a shared object would mean moving the live RAM to
one first, which changes the live path for every build. `vm_copy` would
work too but needs a destination allocated first; remap does both.
Software COW through the fast page table (pipelined-render.md) races a
sim write against a render read of the same page; host COW has no race.

**The page table.** A view has its own fast page table, built from the
same mappings (8 MB, a pass over the mapped pages). `cow_refresh` keeps
it when the source's layout has not changed since the last refresh, so a
per-frame refresh is the RAM remap alone.

**Written pages and lines.** The view's write watch watches all of RAM, so
every store (Rust, or C's inline dirty bits, published at the next memory
guard) marks its physical page. A second COW copy, `base`, keeps the
bytes as they were at the snapshot and is never written. At the join,
`SecondExecutor::written` compares each marked page of the view with
`base` 64 bytes at a time: a `WrittenPage { physical, lines }` per marked
page, `lines` the 64-bit mask of 64-byte lines whose bytes changed. A
page stored to with the bytes it already held is listed with
`lines == 0`. `hand_back` then merges into the live Guest byte by byte
over those lines: a byte the view changed takes the view's value; a byte
it did not keeps whatever the live side has (so a sim write to another
byte of the same line survives); a byte both changed differently is a
conflict, counted, view wins.

## 3. The executor

`runtime::executor::SecondExecutor` is its own `Runtime` (own gate, own
page table, own TSC, own interrupt queue) over a view Guest, with its own
`ExecCtx` and a `Dispatch` copied once from the main runtime:

- generated handlers and resumes: copied (function pointers into the
  same loaded module);
- every override the caller's `allow` list does not name, and every
  kernel import token: a refusal handler, which records what was called
  and ends the run (`ExecutorError::Refused { target, name, ordinal,
  return_pc }`), never runs the main thread's HLE;
- allowed overrides: the main runtime's handler, run on the executor's
  thread.

A run is one guest call: `snapshot(&main)` (it reads the main runtime's
guest time first, which takes the kernel lock, then takes the main
runtime's memory guard, so no store is in flight while RAM is copied),
then `call(cpu, entry, args)`
pushes the arguments and a return to `HARNESS_EXIT` on the executor's
stack and runs the runner to completion on the calling thread (any
thread: `SecondExecutor` is `Send`). No kernel scheduler, no interrupts,
no slice expiry: the context is armed for host requests only, which
counts its accesses (`CallOutcome::accesses`, for the caller to charge to
the game thread's clock at the join) and lets `cancel_handle()` stop it at
the next safe point from another thread.

## 4. Tests and measurements

Unit tests in `crates/runtime` (Rust stores only): `cow` (a copy keeps
the bytes of its moment and each side its own writes, repeated copies),
`guest::view` (a view reads the snapshot with the source's permissions;
refresh keeps the page table until the layout moves; the written-line
set is exactly the changed lines, a same-value store is a page with no
lines, and the byte merge keeps the live side's other bytes and counts
conflicts), `executor` (private writes and hand-back; kernel calls,
registered on the main runtime or not, and disallowed HLE refused by
name; an allowed override runs; cancel).

`crates/emit/tests/second_executor.rs`, on synthetic x86 lifted, emitted
with the inline RAM path (`H2_FASTMEM`) and access accounting, compiled
and run through the real runtime:

- `the_executor_runs_generated_code_on_a_frozen_snapshot_while_main_mutates_live_ram`:
  each round, snapshot; the executor sums a 16 KB array 32 times on its
  own thread while the main context, on the test thread, adds 1 to every
  word of it one to four times. Every round: the executor's sum is the
  frozen array's; its stores never reach live RAM before the hand-back;
  `written()` is exactly the three lines it stored plus its stack line;
  the hand-back merges three lines with no conflict and keeps the main
  side's store to another line of the same page; the main context is
  charged exactly what a lone run charges. 200 rounds by default
  (`H2_EXECUTOR_ROUNDS`); the test asserts the two actually overlapped
  (179 of 200 and 1,896 of 2,000 rounds did, debug build).
- `a_kernel_call_from_generated_code_is_refused`: an indirect call to a
  kernel import token from generated code ends the run with
  `Refused { ordinal: 128, name: "KeQuerySystemTime", return_pc }`.

No loom or TSAN: the two executors share no mutable runtime state by
construction (section 1); the stress test is the evidence.

**Costs** (`cargo test -p runtime --release --lib cow_copy_cost -- --ignored --nocapture`,
Apple silicon, loaded machine): a COW copy of the 64 MB RAM block with
16 MB resident is 0.43-0.60 ms; afterwards the first write to each 16 KB
host page from the live side faults, about 2.8 us a host page (0.7 us
per 4 KB guest page written at a 4 KB stride); dropping the copy 0.06
ms. A snapshot takes two copies (view and base), so about 1 ms a frame,
plus 2.8 us for every host page the sim writes while a render runs.

The live path: the only change on it is `Guest`'s RAM field, now a
`cow::Ram` (pointer and length, `Deref` to `[u8]`) instead of a
`Box<[u8]>`; the runner and C paths are unchanged.
`runner_transfer_benchmark` after the change, minimum of 9 trials, ns per
transfer: 41.1 (16 callees), 41.7 (256), 45.0 (1,024), 53.8 (4,096; median
92.9, the machine was busy), against 39.2, 40.4, 42.2, 45.4 in
docs/perf/guest-memory.md, within the noise of a shared machine.

## What remains before the render front-end can run on it

Items 1-5 and 7 are built in the prototype (pipelined-render.md, "The
prototype"); the executor gained what they needed:
`SecondExecutor::refused_cpu` (the CPU at a refused call, so the main
context can make it and carry the run on through the resume points;
tested on lifted code), `resume` (carry a refused run on on the
executor after someone else made the call: a return to the call's
return address with the result in the CPU, the view and continuations
as they were), `last_accesses`, `revert` (undo the view's change
to a range before the hand-back), `share_watch` with
`Guest::adopt_watch` (item 4), and `ExecCtx::charge_accesses` (item 5).
Items 6 and 8 are open.

1. **The launch point.** An override at `render_frame_and_present`
   (`0x00223240`) that joins render `k-1`, calls `snapshot`, and hands
   `call(cpu, 0x00223240, [])` to a long-lived render host thread with a
   copy of the entry CPU state, then returns to the game as the original
   would (pipelined-render.md, "Design"). Nothing here starts a thread;
   the caller owns it.
2. **The allowlist.** Which HLE the render front-end reaches: D3D entries
   (the device has its own lock, but the sim must make no D3D calls while
   a render runs: `sim_d3d_hits`), and the kernel calls the census or an
   HLE-call count per scope finds (time reads, critical sections). Allowed
   kernel calls need executor-safe handlers (answer time from the
   snapshot's frozen clock, no waits), not the kernel's own, which are
   single-threaded; today every kernel call is refused, which ends the
   run with its name, the signal to fall back to a serial render.
3. **Hand-back policy.** `written()` lists every page; the join needs the
   census's split: drop the stack, copy `rw` pages, merge `ww` pages
   (`hand_back` does the byte merge), and decide the `rs_core` pages.
   `HandBack::conflicts` is the count to watch.
4. **Write-watch subscribers on the live side.** `hand_back` reports the
   merged pages to the live Guest's watch (D3D's resource barrier sees
   them), but the view's own D3D-side subscriptions (if D3D runs on the
   executor) are the view Guest's, not the live one's: the D3D layer's
   dirty tracking has to be told which Guest it is reading.
5. **Logical time.** `CallOutcome::accesses` has to be charged to the
   game thread's clock at the join (the kernel's `take_slice_spent`
   path), at a fixed point, so the schedule stays deterministic
   (pipelined and serial runs are not lockstep-compatible).
6. **Cost on the real working set.** About 1 ms of snapshot a frame and
   2.8 us per 16 KB host page the sim writes during a render (section 4):
   the census's `sim_written` decides whether that eats the 6-8 ms
   pipelining saves. The base copy could be dropped (diff against nothing
   and copy pages whole) if the merge is not needed. A layout change (a
   map or protect) rebuilds the view's page table (8 MB).
7. **Quiescence.** Snapshots, checkpoints and the agent API's pause must
   join a running executor first (docs/runtime/snapshots.md).
8. **Timeline phases.** Phase accounting is per host thread; render-timing
   and frame stats have to fold the render thread's in.
