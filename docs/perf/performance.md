# Runtime performance: guest memory access and the runner

The runtime side of the 30 fps work: guest memory accessors, the
inline RAM path (`H2_FASTMEM`, on by default), runner transfers and their counters. The
profiling and benchmark tools are in [perf.md](perf.md); the main-menu
frame budget and the plan across every area are in
[perf-budget.md](perf-budget.md).

The goal is **retail running correctly at 30 presented frames per wall second**,
a **33.33 ms** budget per frame, measured in the main menu with `H2_BOOT_GFX`.
Startup time, scheduler slices and guest virtual time are not frame rate.
Count `(last_present - first_present) / (last_time - first_time)` over a
verified menu interval. Retain sparse frame dumps to confirm the scene.
Use identical generated modules, storage seed, scripted input and capture
cadence on both sides of a runtime change. Record shared-machine contention.

## Instrumentation

`ExecCtx::enable_profiling(true)` enables per-context counters. At run end,
print `ctx.profile().unwrap()`. `take_profile` drains without disabling;
`Profile::merge` aggregates scheduler threads and slice snapshots.

Counters cover dispatched runner transfers, runner safe points, generated
loop safe points, memory accessor calls (including failed and atomic calls),
and x87 helper calls. `memory_accessors` includes successful inline accesses;
subtract `fast_memory_accesses` to get Rust accessor calls. Neither counts
ordinary HLE Guest reads/writes.
Kernel registration times completed, stopped and faulted kernel invocations.
`Scope::Runner` covers the entire synchronous runner call, including handlers.

Compile generated C with `H2_PERF=1` for loop/x87 counters. The compiler flag
is included in object cache keys. Counters stay in handler locals and flush
once on **every** exit, including faults and preemption, through the optional
`h2abi_v1_profile(ctx, loop_polls, x87_calls)` extension symbol. The runtime
exports it; a dylib bridge must forward it. Ordinary builds have no generated
profiling instructions and require no new symbol. ABI v2 layouts are unchanged.
Generated counts are available after a handler exits, not during an unbounded
handler invocation. Inline status checks such as FWAIT are not helper calls.

No hot-path counter reads the clock or uses atomics. Timers are inclusive host
wall durations. Nested Runner/Kernel/D3d/Gfx/Fallback times **overlap** and must
not be summed. GPU completion latency is not measured by a host submission timer.
An unobserved category prints `unobserved`, not a misleading measured zero.

Boot integration (not done; `H2_PERF` counters already reach the runtime
through boot's bridge, and a timeline of a real run comes from `H2_TIMELINE`,
see [perf.md](perf.md)):

- Enable profiling for every guest ExecCtx, including new threads and DPCs.
- After registering D3D entries, use `Dispatch::profile_override(addr,
  Scope::D3d)` once per entry. Tag the original API handler after boot wrappers
  are installed; avoid timing both wrapper and shadow entry in the same category.
- Wrap the actual interpreter entry with `ctx.measure(Scope::Fallback, |ctx| …)`.
- In the boot backend adapter, record each underlying backend call using a
  separate `Profile::record(Scope::Gfx, elapsed)` collector, then merge once.
  This requires no changes to d3d or gfx. D3d minus nested Gfx is an upper bound
  on parsing/state/upload preparation, not purely push-buffer parsing.
- Aggregate contexts plus the external backend collector at run end. Sample
  stacks for generated code, accessor and x87 time; their counters alone do not
  imply time. Keep per-present timestamps separately for the FPS acceptance test.

## Synthetic accessor experiment

`cargo test -p emit --release --test perf generated_runtime_benchmark --
--ignored --nocapture` builds synthetic x86 loops with the normal emitter and
clang, then executes through the **production** runtime's memory accessors and
runner. No retail bytes are used. `H2_PERF_KIND` selects register, read, write,
x87, call or tail; `H2_PERF_ITERS` and `H2_PERF_TRIALS` control repetitions.
`H2_PERF_MAPS` adds disjoint mappings to exercise lookup cost with a populated
guest address space (default zero).
`H2_DEBUG=none` avoids debug-info compilation noise. Exact small fixtures run
in ordinary tests, including call/resume/tail routing and generated counter totals.

The first candidate removes the 16-byte staging buffer in ordinary FFI reads
and writes. Guest already preflights the whole span, so direct caller-buffer
copies preserve failed-read output, transactional writes, mapping/permission
checks, alias tracking, the mutex, and scheduler access charging. No raw RAM
pointer is exported to C. Atomics retain their existing operation-wide path.

ARM64 macOS, clang `-O1`, Rust release, 2 million iterations x 25 trials per
batch, sequential before/after/after/before batches, profiling disabled:

| Work | Before best ns/iteration (two batches) | After best ns/iteration (two batches) |
|---|---:|---:|
| Load + add | 24.096 / 24.289 | 21.524 / 21.838 |
| Store | 21.213 / 21.204 | 19.116 / 19.109 |

Best-trial reductions are about 10.7% and 9.9%. Shared load made medians range
34–125 ns; these lower-envelope microbenchmarks are **not** a menu FPS claim.
A macOS `sample` trace confirms mapping validation, mutex operations and copies
inside the accessor path. Keep logs and samples outside the public repository.

The accessor-only change does not change generated C or clang inputs: the
synthetic read/write objects remained 2824/2656 bytes, with cache reuse after
the Rust change. Instrumentation changes the support header once, invalidating
old objects; enabled counters add C locals and one exit callback. Whole-image
compile effects must be reported separately from these tiny fixture compiles.

## Mapping lookup experiment

The second candidate adds eight relaxed atomic mapping-index hints to Guest.
Each hit rechecks the live mapping's virtual range; callers still read current
permissions and physical backing. A stale index after insert, split, protect
or unmap either identifies the correct live mapping or misses. The hints do
not expose RAM, cache permission decisions or bypass write tracking. Atomic
hints preserve safe concurrent reads through `&Guest`.

With 1000 extra mappings and the staging-buffer fix already applied, sequential
before/after/after/before batches (2 million iterations, 11 trials) gave:

| Work | Before best ns/iteration (two batches) | After best ns/iteration (two batches) |
|---|---:|---:|
| Load + add | 37.843 / 67.829 | 20.333 / 19.858 |
| Store | 55.040 / 31.783 | 20.904 / 20.988 |

The best observed reductions are 47.5% and 34.2%. The wide between-batch
spread reflects shared-machine contention; these remain synthetic results,
not verified menu improvements. With only a few mappings, the write fixture
was about 1 ns slower (19.264 to 20.338 ns best); direct-call and tail-transfer
results were too noisy to establish a change. Retain this small-map tradeoff
when interpreting the populated-address-space benefit.

The runtime-only change leaves generated
C and clang cache keys unchanged.

## Inline RAM path (on by default)

`H2_FASTMEM` (on unless set to `0`) compiles ordinary single-page RAM accesses into C bounds,
permission and alignment checks plus a copy. A lazily allocated 8 MiB page
table tracks mapping changes. An atomic gate excludes Rust memory guards;
physical dirty bits preserve write watches, and batched context accounting
preserves the exact access-budget preemption boundary. Cross-page accesses,
MMIO, failed checks and atomics retain the Rust helper. See
`crates/emit/RUNTIME-CONTRACT.md` for the extension and bridge requirements.

On the same machine, the synthetic read loop's best trial fell from
19.858 ns after the mapping hint to 8.348 ns (20 trials, median 16.382 ns).
The `perf-tooling` e5d6cc7 overlay then measured the mixed-memory loop with
20 samples per batch, three warmups, alternating fast/before/fast/before.
The before version includes both earlier runtime fixes:

| Batch | Before median ns (MAD) | Inline median ns (MAD) |
|---|---:|---:|
| 1 | 278.30 (89.09) | 46.79 (3.08) |
| 2 | 261.90 (135.20) | 57.55 (12.54) |

Contention was substantial. `h2 bench compare` classified the first memory
comparison as improved and the second as within its noise bound. Minimum
trials were about 106 ns before and 42–45 ns inline. The call-loop medians
were 393.20 → 125.08 and 535.26 → 150.42 ns, but only the second comparison
cleared the tool's noise bound. These are synthetic results, not menu FPS.

A five-second, 500 Hz guest-aware sample of the inline memory loop collected
1422 samples without misses: 99.93% were attributed to generated code, with
no Rust `h2abi_v1_read` or `Guest::checked_start` samples. An earlier
profile of the same loop attributed 28% and 24% to those functions. Inline checks and
copies now reside in generated code: their cost has moved, not vanished, and
must not be attributed to IR optimization work.

## Whole-menu profile

A scratch overlay combined perf code through `c3f422a`, master `4330dfb`,
boot's menu driver and perf-tooling `e5d6cc7`. Input was
`START@f310,DOWN@f408,A@f419`, with fresh data and a PNG every 40 presents.
Frame 480 was visually verified as the main menu. Both modes used the same
module compiled with `H2_FASTMEM=1 H2_PERF=1`; the control disabled fast
memory on each execution context. This is not an unmodified-master control.

| Mode | Presents | FPS | Median ms (MAD) |
|---|---:|---:|---:|
| Checked access | 480–560 | 1.133 | 735.03 (209.39) |
| Inline RAM | 560–640 | 1.993 | 505.62 (110.18) |
| Inline RAM | 640–720 | 2.208 | 411.50 (106.27) |
| Checked access | 720–800 | 1.379 | 716.13 (213.62) |

Other game processes overlapped this run. Camera progression and
changing contention prevent treating the ratios as causal accessor speedups.
The second control also retains the initialized memory gate and execute-page
cache. The master-plus-boot reference from docs/runtime/BOOT.md at the time was
**1.91 FPS, 484 ms median frame**. A fresh-data, same-frame pair would
reduce these confounders.

A separate diagnostic interval, presents 800–880, took **366.97 ms/frame**.
The 500 Hz sampler collected 11,423 samples without misses, with a 35 us mean
sampling pause. Samples include waiting and runnable time: the sampler's
`on_cpu` label is not a CPU-utilization measurement. Scoped timers and
identified stack categories give the following approximate owner budget.
They are different measurements and must not be summed as an exact ledger.

| Owner / work | Approximate ms/frame | Evidence |
|---|---:|---|
| runtime/emit memory | 14.1 | 3.9% of disjoint samples, including identifiable inline checks |
| runtime/emit runner and safe points | 10.8 | 2.9% of samples |
| x87 helpers | 21.2 | 5.8% of samples, mainly wide arithmetic rather than call boundaries |
| codegen-opt generated operations | 21.0 | 5.7% of samples after separating identifiable inline-memory PCs |
| gfx-perf backend work | 169.3 | Timed backend calls; about 48% of samples |
| D3D parsing/state/resource preparation | 6.8–11.7 | Drain excluding backend calls; all D3D minus backend is an upper bound |
| kernel/VFS | about 100 | 27% of samples; inclusive kernel timer 97.6 ms |
| boot tracing | 18.1 | 4.9% of samples |
| fallback interpreter | 0.027 | 720 entries, 2.137 ms total |

Over these 80 frames, 135,021,296 of 135,022,668 generated memory accesses
completed inline (99.999%). There were about 133,406 runner transfers,
1.69 million memory accesses and 97,087 x87 helper calls per frame.
The interpreter is not a material cost in this interval.

Backend draw handling takes 116.4 ms/frame, including nested queue submission
at 55.3 ms/frame; surface readback takes another 40.2 ms/frame. Do not add
nested submission time to draw time. The capture shows about 840 draws,
843 submissions and seven readbacks per frame. Gfx-perf owns submission and
upload batching; D3D/gfx should examine readbacks while preserving guest
coherence. Kernel/VFS samples predominantly follow `Vfs::write` through
`HostBackend::write_at`, `store_basic` and directory synchronization.

**30 FPS requires 33.33 ms/frame.** This diagnostic window needs about
333.6 ms/frame removed (90.9%); the unprofiled inline windows need roughly
13.6–15.1 times their observed throughput. Runtime/emit alone cannot close
that gap. Memory checks and runner dispatch remain the runtime's targets;
codegen-opt owns the generated-operation budget, gfx-perf owns the largest
backend budget, and kernel/VFS persistence belongs to the VFS.
Most x87 cost is arithmetic and needs work beyond helper-call overhead.
Disable boot debug tracing for production acceptance measurements.

## Generated-image build cost

The runtime-only copy and mapping-hint changes leave generated C and cache
keys unchanged. The inline support header invalidates C objects once.
The synthetic read object grew from 2824 to 3240 bytes (about 15%); its tiny
clang compile rose from approximately 0.06 to 0.09 seconds.

The full image contains 18,691 lifted functions, 418.1 MB of emitted C and
1001 objects. Its first build segment used one clang job under heavy
contention for 698.11 s wall / 270.96 s user. The resumed four-job segment
reused 112 objects, compiled 889 in 712.9 s and linked in 0.7 s. Including
analysis and emission, that segment took 753.40 s wall / 1220.53 s user.
This mixed, contended run does not establish a compile-time delta against
the historical roughly eight-minute parallel build.

Across 354 unchanged generated-C shards, object bytes grew from 67,633,024
to 108,415,832 (**60.3%**), and executable text from 40,250,060 to 55,107,324
(**36.9%**). The after side enables both profiling and inline memory; the
comparison does not isolate either flag. This footprint tradeoff belongs
in build-perf and codegen-opt planning.

## Ownership

This work covers runtime/emit accessors, runner transfers, safe points and helper
call overhead. `codegen-opt` owns IR optimizations, `gfx-perf` owns wgpu work,
`perf-tooling` owns guest-aware sampling, Perfetto and `h2 bench`, and
`build-perf` owns compile-time improvements. Gfx/d3d source is untouched here.

Reports must separate runtime/emit, IR passes (`codegen-opt`), wgpu
(`gfx-perf`), D3D parsing/state preparation, and kernel costs. Report the
measured menu FPS, interval and load conditions, then the time each owner
must remove to fit the 33.33 ms budget. A proportional extrapolation from
samples is an estimate, not proof of an achievable FPS; preserve guest
correctness, graphics work, input and scheduling throughout.

## Guest CPU: SSE, REP copies and D3D entry (2026-11-15)

`H2_SSE_NATIVE` (on by default; `0` disables it) enables guarded native binary32 arithmetic with observable
MXCSR preserved; [sse-native.md](../lifter/sse-native.md) gives the proof and escape
hatch. The runtime ABI and Rust integer authority are unchanged.

The per-access `h2_fast_copy` already used host `memcpy`; switching its
runtime width to a fixed-width switch did not establish a speedup and was
not retained. The new optimization batches flat REP MOVS iterations into
page-bounded host `memmove` calls under the existing memory gate. The emitter
recognizes the complete semantic IR shape, including DF and ESI/EDI updates.
FS-relative or altered bodies retain ordinary iteration.

Both source and destination must stay inside their current fully mapped RAM
page with the required permissions. Crossings finish a page chunk then
recheck; MMIO, partial mappings and failed checks use the original accessors.
Physical spans determine overlap, including virtual aliases. Propagating
overlap retains element-by-element execution; the opposite direction and
identical spans permit memmove. Dirty pages are marked under the same gate.
Each completed element still charges two accesses. With access accounting,
chunks stop at the iteration that reaches the budget, and pending preemption
allows only one iteration before the usual REP poll. Native chunks are
bounded by 4 KiB; asynchronous requests arriving within a chunk are observed
at its end. The helper does not turn REP into an unbounded critical section.

D3D's `WriteTracker` now uses a sparse two-level page-generation array:
8 KiB of roots plus 8 KiB per populated 4 MiB virtual region, with no hashing
in `dirty_since`. Watch, reset and generation semantics are unchanged.
API-bound and UP vertex stream copies reuse a device-local pool after the
backend returns. It holds at most 16 payloads and 16 MiB in total. Each draw
still reads current guest bytes; backend-owned clones remain independent.
The outer stream vector is reused too. A larger draw can allocate when
capacity grows; index and unrelated draw-state allocations are unchanged.
No gfx, submission, readback or render-thread code is changed.

Synthetic benchmarks (clang `-O1`, Apple Silicon, based on `33a73cc`):

| Work | Before | After | Measurement |
| --- | ---: | ---: | --- |
| Four packed add/mul/sub/div operations | 142.34–145.17 ns | 47.95–48.37 ns | thread CPU per trip |
| 1 KiB REP MOVSD, 256 elements | 3621.37–3623.41 ns | 21.06–21.40 ns | thread CPU per copy |
| Four-page clean `dirty_since` | 31.34 ns | 3.22 ns | wall per query, release |
| D3D, 1,200-draw synthetic frame | 2.750–2.824 ms | 2.545–2.566 ms | wall median per frame |

The D3D frame benchmark used 100 frames per batch, alternating
before/after/after/before with unchanged 126,000 draws, 48 uploads and
251,952 cache hits including warmup. Load averages were 4.53 / 29.85 / 50.91
throughout those batches. The earlier contended baseline was 3.054 ms
median; it is excluded from this paired comparison.

The C benchmark alternates before/after/after/before and preserves matching
checksums, MXCSR and 102,400,000 accesses in both copy modes. It compares the
per-element path against batching, not memcpy's intrinsic throughput. Run:

```sh
cargo test -p emit --test fast_copy guest_microbenchmarks -- --ignored --nocapture
cargo test --release -p d3d --lib dirty_query_benchmark -- --ignored --nocapture
cargo bench -p d3d --bench pushbuf
```

These are synthetic costs, not a measured 7–10 ms improvement to the 67.5 ms
Heretic frame. No game image was built and no game measurement was taken;
unit and oracle tests establish exactness without one. Whole-frame savings
remain to be measured after integration on presents 4220–4520 with a warm
pipeline cache and recorded load. Build-target disk headroom limited this
lane to focused tests and microbenchmarks.

## Inline RAM: one gate hold per run of accesses (2026-11-15)

After FASTMEM, the "generated guest code" row of the level frame (14.9 ms,
[frame-budget.md](frame-budget.md)) is mostly the inline RAM path itself.
Each access took the memory gate with a compare-and-swap and released it
with an ordered store, and that pair is most of an access's 6-7 ns: the
`read` loop below spends 7.1 ns a trip on one load, against 0.8 ns for the
same loop on registers. At 1.45 M accesses a frame that is about 9-10 ms.

A handler now keeps the gate across consecutive accesses, with the page
table and RAM pointers it read when it took it. It drops the gate before
anything that takes the context, on every way out, and after 32 loop
headers or REP trips. A Rust memory guard that finds the gate held marks it
wanted; the handler gives way at its next loop header and cannot retake the
gate ahead of that guard. `crates/emit/RUNTIME-CONTRACT.md` has the rules.
An access under a held gate is a page-table load, a permission check and
the copy. Access accounting, the slice boundary, dirty bits and fault order
are unchanged: `held_gate_matches_the_helper_path_at_every_stop_and_fault`
runs a synthetic handler (runs of accesses, loops, a REP, x87, calls, a
page-crossing read, a store to a read-only page) against the same module on
the helper path, for seven slice lengths, with and without accounting.

Synthetic loops through the real runtime (`crates/emit/tests/perf.rs`,
accounting on as in the image, clang `-O1`, Apple Silicon, best of 15, load
average 41-70):

| loop | accesses a trip | before | after |
|---|---:|---:|---:|
| `read` (one load) | 1 | 7.06 ns | 3.14 ns |
| `write` (one store) | 1 | 7.09 ns | 2.50 ns |
| `frame` (two pushes, two loads, a store, two pops) | 7 | 50.55 ns | 11.15 ns |

So an access that shares a hold costs about 1.5 ns instead of 7.1, and a
one-access loop about 2 ns (it pays the header check, and the gate once in
32 trips).

Over the lifted image (`image_gate_holds`, all passes): 707,377 accesses in
196,764 blocks. Taking a hold to end with its block, which undercounts
(holds last across blocks until a call, an exit or a loop header), 509,970
accesses (72.1%) follow another under the same hold: 3.58 accesses a hold.

Estimate for the level frame, not measured in the game: 1.45 M accesses at
about 7 ns is 10 ms today. With 72% of them at 1.5 ns and the rest
unchanged it is 4.4 ms; with holds lasting across blocks and loop trips, as
they do, nearer 3 ms. That is **5.5 to 7 ms a frame** off the guest-code
row, weighted by static counts and synthetic costs. The dynamic mix is
unknown until the game is run, on presents 4220-4520 with a warm pipeline
cache. No game image was built for this.

The city flyover (render thread on, 2026-11-15 frame stats) spends 70-75 ms
of an 80-86 ms frame in the game thread's `cpu` phase. Its access count was
not recorded. If its mix is The Heretic's (the gate is about 10 of 44 ms of
`cpu` there), this change is worth 9 to 12 ms of the 75; the saving is
about 5 ns for every access the frame makes, so a run with frame stats'
access count settles it.

```sh
H2_ACCESS_COUNT=1 cargo test -p emit --release --test perf generated_runtime_benchmark -- --ignored --nocapture
cargo test -p emit --release --test perf image_gate_holds -- --ignored --nocapture
```

Not done here, in the order the code suggests: the runner's transfer path
(`call` is about 180 ns for a call and return, two `HashMap` resolves with
SipHash and a shadow-stack update per transfer; an address-indexed table
and direct calls between generated handlers are the next step, about 5 ms;
both have since landed, see "Direct calls" below),
then cross-block dead stores and register promotion, which a flat profile
at `-O1` does not point at.

## Direct calls between generated handlers (2026-11-15)

Every guest `call` used to leave generated code with `CALL`/`ENTRY` and
come back through the runner twice: once for the call and once for the
callee's `ret`, each paying the safe point, shadow-stack bookkeeping, the
resolve cache, `check_execute` and `handler::execute`. With
`H2_DIRECT_CALLS` (on unless `H2_DIRECT_CALLS=0`; a C define, so it is in
the object cache key and the dylib stamp like `H2_X87_NATIVE`) a call to a
function entry of the same module whose continuation is one of the caller's
own selectors calls the callee's `h2fn_` as a C function, and a tail jump to
one does the same and returns its transfer. A `RETURN` to exactly the
continuation, with ESP above the return slot, continues inline from the
continuation's label. Everything else is handed to the runner as it was.

The direct path runs only where the runner would have done nothing else:
the preempt word and the interrupt queue are empty, `max_transfers` has a
transfer left (each direct call, jump and inline return takes one), the
nesting is under the context's cap (32 by default, at most 64), the
context's slot says the runner resolved the target to this very handler at
the current dispatch generation (any `edit_dispatch` stops all direct calls
until the runner resolves again), and the page is executable in the inline
RAM view. The same checks repeat for the return. The contract has the
details (`crates/emit/RUNTIME-CONTRACT.md` §4, "Direct calls").

Counters: `transfers` still counts every guest transfer; the new
`direct_transfers` is the part made in C, which passes no runner safe point,
so `runner_safe_points` drops by that much. `generated_batches` counts
handler exits, one fewer per call that returned inline.

Synthetic benchmark (`generated_runtime_benchmark`, release, median of 7,
5M iterations, this machine):

| kind | before | `H2_DIRECT_CALLS=0` | direct |
|---|---:|---:|---:|
| call (call + ret) | 52.3 ns | 52.6 ns | 35.3 ns |
| tail (call, tail jmp, ret) | 73.8 ns | 75.1 ns | 45.2 ns |

What was left of a direct call was mostly the callee's own prologue and
epilogue: `h2abi_v1_fast_acquire`/`release` (two Rust calls), the register
spill and reload, and two gate round trips for the execute checks. The
next section removes the Rust calls.

Proof: `crates/emit/tests/direct.rs` builds one synthetic module with the
flag off and on and runs it on the real runtime three ways (built off, built
on with `ExecCtx::enable_direct_calls(false)`, on), requiring identical
transfer, CPU, memory, events, backtrace, preempted transfer and transfer
count for nested and stdcall calls, recursion past the depth cap, a faulting
callee, a tail-jumping callee, a rewritten return address, a return with ESP
at its slot, an override registered and disabled between runs, every
transfer limit, a slice that runs out at every access, a host preempt
request, and pending interrupts.

```sh
cargo test -p emit --test direct
H2_PERF_KIND=call cargo test --release -p emit --test perf generated_runtime_benchmark -- --ignored --nocapture
```

## One exit call, and a callee on its caller's view (2026-11-17)

Two changes to the handler's way in and out, both with `H2_FASTMEM`.
Neither has a benchmark recorded here; the tests below prove them equal to
the old calls.

- **One exit call.** The shared epilogue used to call `h2abi_v1_charge`
  (with accounting, when the count was not zero) before the spill and
  `h2abi_v1_fast_release` after it. It now calls `h2abi_v1_exit(ctx, n)`
  once, after the spill: the runtime settles the inline view's count and
  then spends the handler's `n`. Without `H2_FASTMEM` the charge alone
  stays. The ABI version is unchanged and the two older callbacks stay
  exported. The boot bridge table has a last slot, `exit`, installed by
  `h2boot_install_v3`; boot installs a module built before it through
  `h2boot_install_v2`, so older images still load.
- **A direct callee runs on its caller's view.** A direct call lends the
  caller's inline RAM view through the last word of the direct-call block
  (`H2Direct.view`). The callee's entry takes and clears it and skips
  `h2abi_v1_fast_acquire`; its exit calls `h2abi_v1_exit` only when it has
  a count of its own to charge, and a hand-up from it releases nothing.
  The caller charged its count just before the call, every Rust helper
  settles the view's count before it looks, and the budget is what acquire
  would have set again, so the slice runs out at the same access. The
  runtime clears `view` before it invokes any native handler itself, so a
  handler entered through the runner always acquires. Module feature 4
  (`H2GEN_FEATURE_DIRECT_VIEW`) marks a module that lends: a runtime whose
  block ends before `view` refuses it, and older modules ignore the word.

`H2_EXIT_CALL=0` and `H2_DIRECT_VIEW=0` build the old exit and the old
entries and exits. They are C defines the equivalence tests set, not
`cflags()` switches. `crates/emit/RUNTIME-CONTRACT.md` §4 has the rules.

Proof: `crates/emit/tests/direct.rs` compares lent and unlent views, and
old-exit builds through the v2 bridge table, in every scenario above
(faults, hand-ups, transfer limits, a slice ending at every access, depth
caps, overrides, interrupts), on charged and inline accesses, slice spent,
budget and preempt word. `a_direct_callee_borrows_its_callers_view` counts
the calls: in the nested fixture 7 acquires and 7 releases become 1
acquire and 7 exits, and calls past a depth cap acquire as through the
runner. `crates/emit/tests/exits.rs` checks the preprocessed epilogue, lend,
borrow and hand-up for every accounting and fast-memory combination, the
runtime has `exit_settles_as_charge_then_release`, and
`crates/boot/tests/bridge.rs` loads an old-style bridge through v2.

## Inline guest loads and stores (2026-11-16)

`H2_LD`/`H2_ST` called `h2_held_read`/`h2_held_write`, generic over the
width. Clang inlined them only in small handlers; in a handler with many
access sites (every image shard) it kept one out-of-line copy per shard,
so each access was a call, about 30 instructions, a `memcpy` with a
run-time width and a 16-byte staging buffer on each side
(`docs/perf/architecture-review.md`, finding 1).

Now the check is a macro (`H2_HELD_TRY`, `h2fastmem.h`) that is always
inline: with the gate held, a span inside one page, the permission bit
and a physical page inside RAM, the access is one load or store of the
constant width at the host address. Anything else is a single call to
`h2_access_cold`, which is out of line, cold and `preserve_most` (the site
spills nothing around it): it takes the gate and retries, or drops it and
runs the ordinary helper exactly as before. The hold keeps one pointer to
the view instead of four fields, and remembers the physical page it last
marked dirty, so repeated stores to a watched page skip the bitmap update.
The tests jump to one label with every local declared in that label's
scope: at the image's default `-O1`, a jump out of a scope with locals
goes through a clang cleanup switch that `-O1` does not fold, which made
an earlier version of this change 40% larger.

The comparison asked for, a tiny non-inlined leaf per width, was built
and measured too: smaller (`wide` 24.0 KB of `__text` at `-O1`), but at
164-171 ns for `wide` it is only 1.5x faster than before, against 2.6x
for the inline form, so it was dropped.

`generated_runtime_benchmark`, release, median of 7, 5M iterations,
accounting on (as the image builds), this machine under load from other
lanes. `wide` is new: one loop with 112 access sites of widths 1, 2 and 4,
the shape in which clang declined to inline the old accessors (the other
kinds were already inlined, so they show only the shape change).

| kind | `-O1` before | `-O1` after | `-O2` before | `-O2` after |
|---|---:|---:|---:|---:|
| read | 3.22 ns | 3.06 ns | 2.52 ns | 1.88 ns |
| write | 2.48 ns | 2.09 ns | 2.60 ns | 2.21 ns |
| frame | 11.1-11.4 ns | 9.6 ns | 8.73 ns | 6.92 ns |
| wide (112 accesses) | 250 ns | 97 ns | 243 ns | 72 ns |
| wide, `H2_ACCESS_COUNT=0` | 306 ns | 215 ns | | |

`wide` per access: 2.23 ns before, 0.87 ns after at `-O1`. Without
accounting each inline access still updates `f->accesses` in memory and
tests the budget, which is why that row gains less; the image builds with
accounting.

`__text` of the benchmark's shard object (same flags as `h2 emit`):

| module | `-O1` before | `-O1` after | `-O2` before | `-O2` after |
|---|---:|---:|---:|---:|
| wide | 28,524 B | 26,888 B (-5.7%) | 28,560 B | 22,376 B (-21.7%) |
| frame | 4,052 B | 4,232 B (+4.4%) | 3,800 B | 3,304 B (-13.1%) |

Proof: `inline_accesses_match_the_helper_path_at_every_width_and_edge`
(every width 1, 2, 4, 8, 10, 16; offsets 0-15 and around each page end;
crossing into RAM and into nothing; unmapped, read-only and MMIO sources
and targets; a write-watched page stored to twice; a slice of 1 and of a
million; accounting on and off) requires the same transfer, registers,
memory, dirty pages, events and access count with the inline path on and
off. `a_new_hold_marks_the_page_the_last_hold_dirtied` drains the watch
from a host thread mid-loop and requires the next inline store to the same
page to be marked. Both were checked by breaking the inline path (no dirty
mark, an off-by-one crossing test, the read bit for stores, a hold that
keeps the last-dirtied page): each break fails one of them.
`held_gate_matches_the_helper_path_at_every_stop_and_fault` and the rest of
`cargo test -p emit` pass unchanged.

```sh
H2_ACCESS_COUNT=1 H2_PERF_KIND=wide cargo test -p emit --release --test perf generated_runtime_benchmark -- --ignored --nocapture
cargo test -p emit --test perf -- inline_accesses_match a_new_hold
```

## Register homes (2026-11-16)

Every handler used to load all eight GPRs, EFLAGS and the defined mask into
locals on entry and store all ten back at every exit, and a direct call
stored all ten before invoking the callee and reloaded them after. A value
held across the handler's out-of-line calls (the memory helpers, the direct
callee) also occupies a callee-saved host register or a stack slot for the
whole handler, whether the guest code touches the register or not.

Now (`emit::c::homes`, on unless `H2_REG_HOMES=0`):

- A handler keeps in locals only the registers it can write (a partial
  write such as `AL` counts; EFLAGS with its mask when any flag is read or
  written, a REP runs or an atomic runs; ESP always). It reads every other
  register from `CpuAbi` where it is used and never loads, spills or
  reloads it. Nothing in the handler assigns it, so `CpuAbi` is current at
  every point, including after a direct callee or atomic changes it there.
- A direct call or tail jump spills only the homed locals that may differ
  from `CpuAbi` (`emit::c::dirty_at_exits`: a forward union over the local
  CFG of what was assigned since entry, the last inline return or the last
  atomic). The shared epilogue still stores every homed local.

What the runtime sees at every exit is unchanged
(`crates/emit/RUNTIME-CONTRACT.md` §4, "Register homes"). A handler that
never touches EFLAGS still sets its bit 1 on the way out.

Static effect, measured before the inline loads and stores below landed,
on a sample of the retail image (every 37th shard plus the
one holding `0x2dc220`: 27 shards, 507 handlers), compiled with the image's
default switches and no debug info, homes off against on
(`cargo test -p emit --release --test homes retail_sample -- --ignored --nocapture`):

| | `__text` | loads (`ldr`/`ldur`/`ldp`) | stores (`str`/`stur`/`stp`) |
|---|---:|---:|---:|
| `-O1` | 7,994,536 → 7,943,192 B (−0.64%) | 371,986 → 363,709 (−2.2%) | 300,089 → 290,151 (−3.3%) |
| `-O2` | 6,086,616 → 6,010,240 B (−1.25%) | 329,644 → 318,681 (−3.3%) | 190,146 → 184,841 (−2.8%) |

Homes alone (before the per-call spill masks) gave −0.51% / −0.75% of
`__text`. About 25 instructions a handler at `-O1`, most of them in the
prologue, the epilogue and around direct calls, which run on every
invocation; the bulk of each handler is memory-access code this does not
touch. In the whole image 28% of handlers' register loads and 31% of their
exit stores go away (by the GPRs and flags each handler writes; 18,691
handlers). The synthetic `call` benchmark did not separate the two on a
loaded machine (minimums 37-39 ns against 39 ns, medians within noise).

Proof: the differential suite (`tests/differential.rs`, the interpreter on
unoptimised IR against the C, now with `homes-*` fixtures for partial
writes, faults after partial writes, flags across blocks, no flags at all
and indirect jumps), the direct-call suite (`tests/direct.rs`: a callee
writing registers its caller only reads or never touches, and a register
changed round a loop reaching the next direct call, in all three modes),
`tests/homes.rs` (what each statement keeps homed, the spill masks, and
EFLAGS bit 1), and the retail entry path through generated C
(`tests/entry.rs` with `H2_XBE`).

## Runner transfers and the frame-stats clock (2026-11-17)

Old Mombasa headless (PGO image, 2,000 draws) still spent about 17% of the
game thread's on-CPU samples in the machinery around guest code, nearly all
of it per runner transfer (~29k a frame): the runner loop itself, the
resolve cache and its misses into the dispatch maps (`Dispatch::resolve`,
`resolve_resume`, `resolve_with_generation`: 784 of 47,271 samples), the
execute check (`check_execute`, two gate round trips and a page-table read:
245), the interrupt check (`interrupt::deliver`, a call per transfer: 238),
then the handler's entry (`h2abi_v1_fast_acquire`, the first access's gate
take in `h2_access_cold`) and exit. Separately, with frame stats on, every
D3D entry's phase switch read the clock through `Instant::elapsed`
(`clock_gettime` → `mach_absolute_time`): 704 samples, 1.5%.

What changed (runtime and timeline only; generated code is untouched, so
no image rebuild is needed for it):

- The per-context resolve cache has 4,096 entries (was 256), indexed by a
  multiplicative hash of the target. Misses go out of line.
- The execute check is remembered per cache entry against the guest's
  *layout identity*: `Guest` takes a fresh, process-unique value at every
  mapping or permission change (`refresh_fast_range`, `apply_unmap`, a new
  `Guest`), and each memory guard publishes it as it ends. While it is
  unchanged the check is one load; a fault is never remembered.
- The interrupt check tests the queue's pending count inline first (the
  other conditions are side-effect free, so the order changes nothing); the
  continuation lookup on RETURN tests for a token address inline;
  `fast_shared` (every handler entry) is an inline load with the one-time
  set-up out of line.
- `timeline::now_ns` reads `mach_absolute_time` directly on Apple
  platforms, scaled by a fixed-point factor (6 ns a read against 25 ns).

Proof: `exec::tests::a_remembered_execute_check_never_outlives_a_layout_change`
(protect, unmap, map, another thread's protect and a replaced Guest, each
seen at the next transfer, with the C RAM view on and off; it fails if the
memo ignores the layout), `the_resolve_cache_answers_each_target_with_its_own_handler`
(three times as many targets as entries), `timeline`'s
`now_ns_is_monotonic_and_keeps_time_with_instant`, and the runtime and emit
suites.

Benchmarks: `runner_transfer_benchmark` (`crates/emit/tests/perf.rs`: eight
indirect calls a loop trip into one of N callees that only `ret`, so 16
runner transfers a trip) and `timeline`'s `now_ns_cost`:

```sh
cargo test -p emit --release --test perf runner_transfer_benchmark -- --ignored --nocapture
cargo test -p timeline --release now_ns_cost -- --ignored --nocapture
```

Minimum of 9 trials, ns per runner transfer, quiet runs on a shared
machine (later A/B rounds were swamped by other builds):

| callees | before | after |
|---:|---:|---:|
| 16 | 39.4 | 39.2 |
| 256 | 42.2 | 40.4 |
| 1,024 | 48.9 | 42.2 |
| 4,096 | 48.8 | 45.4 |

With the targets inside the old cache nothing moves within noise; past it
the misses are gone, and what is left at 1,024 and more is the callees' own
code and data falling out of the CPU caches. `now_ns`: 8.8 ns a read
against 25.1 ns for `Instant::elapsed`. Expected in the game, from the
profile's own counts: most of the 784 miss samples, the 245 of the
execute check, most of the 238 of the interrupt check, and two thirds of
the 704 of the clock while frame stats are on, roughly 3% of the game
thread's CPU.

Left, by size: the handler's first access taking the gate in
`h2_access_cold` at every runner entry (1,462 + `h2_gate_try` 307), which
only fewer runner entries or a gate taken in the prologue would remove,
both generated-code changes; the x87 transcendentals (`fyl2x` 536, `fcos`
228, `f2xm1` 208, `fsin` 182: the correctly rounded `Wf<2>` series, which
`H2_X87_NATIVE` does not cover, so f64 would change results); and the D3D
write barrier's `take_dirty` and guard (about 700).

## Handler entry and exit: gate at entry, inline charge, dirty bits (2026-11-18)

Three generated-code switches and one x87 switch, all off by default. The
C ones are in `cflags_for`'s list, so they are in the object cache key and
the boot stamp: set them identically for `h2 build` and `h2 run`.

- **`H2_ENTRY_GATE=1`.** A handler with inline loads or stores takes the
  RAM gate as it starts (after acquire and the budget), and again when a
  direct call returns inline, instead of its first access calling
  `h2_access_cold`. A busy gate leaves it not held, and that access is cold
  as before. `cold_accesses_by_cause` (40 direct calls, 3 runner entries):
  entry 3 → 0, lent-entry 39 → 0, after-dcall 39 → 0.
- **`H2_VIEW_CHARGE=1`** (counting modules with a view). The handler's
  charges, its loops' budget and its exit spend the count in the RAM view
  inline (`h2_view_spend`, `h2_view_left`) instead of calling
  `h2abi_v1_charge`, `h2abi_v1_budget` and `h2abi_v1_exit`; the runtime
  settles the view after every native handler (`call_native`) and before
  every helper, as it already did. Only acquire is left at entry. The
  profile's `fast_memory_accesses` then includes those counts.
- **`H2_DIRTY_INLINE=1`** (existing, now proven and measured). A store to a
  page other than the hold's last-marked one tests the dirty bit at the
  site and calls `h2_mark_dirty` only for an unmarked page. It was correct
  as written: `stores_mark_exactly_their_pages_with_and_without_dirty_inline`
  stores round four pages (an alias, three bitmap words), drains, and runs
  again, with and without the switch and accounting; a deliberately wrong
  "already marked" test fails it.
- **`H2_X87_FAST_TRANSCENDENTALS=1`** (boot, run time only, no rebuild of the
  image). FSIN, FCOS, FSINCOS, FYL2X and F2XM1 in native f64 (`libm`)
  instead of the correctly rounded series; see the sweep below.

Proof: `held_gate_matches_the_helper_path_at_every_stop_and_fault` runs with
each switch, both, and everything with `H2_DIRECT_GATE`/`H2_DIRECT_COUNT`
(same end state, accesses and stops as the helper path, at seven slice
lengths, with and without accounting); `exits.rs`
`a_view_charging_handler_settles_without_a_call`; `direct.rs` with
`H2_VIEW_CHARGE=1` (A acquires once, nothing else is called). The emit,
runtime and oracle suites pass with the three C switches off and on.

Benchmarks (`crates/emit/tests/perf.rs`, release, accounting on, the v3
bridge with one exit call as in the image; switch sets run round-robin,
min of 9 trials, best round, load average 30-80):

| benchmark | off | `ENTRY_GATE` | `VIEW_CHARGE` | both |
|---|---:|---:|---:|---:|
| `entry_access_benchmark` runner (ns/call) | 81.7 | 72.3 | 80.0 | 75.9 |
| `entry_access_benchmark` direct (ns/call) | 34.9 | 32.9 | 34.9 | 34.2 |
| `runner_transfer_benchmark` 16 targets (ns/transfer) | 36.8 | | | 33.9 |
| `runner_transfer_benchmark` 4,096 targets | 43.6 | | | 44.4 |

| `dirty_store_benchmark` (ns per two stores) | off | `DIRTY_INLINE` |
|---|---:|---:|
| watch active | 4.37 | 3.59 |
| no watch | 3.21 | 3.22 |

The entry gate saves about 9 ns a runner entry (11%) and 2 ns a direct
call. The inline charge alone is worth about 2 ns a runner entry and
nothing measurable on a direct call; combined with the entry gate it was
consistently 2-4 ns *slower* than the entry gate alone here, which the
profile in the game should settle before it is turned on. The dirty test
saves 0.4 ns a store to a marked page while a watch is active (all of
`h2_mark_dirty`'s 484 samples are such stores) and costs nothing without
one.

```sh
cargo test -p emit --release --test perf entry_access_benchmark -- --ignored --nocapture
cargo test -p emit --release --test perf dirty_store_benchmark -- --ignored --nocapture
cargo test -p x87 --release --test f64_trans -- --nocapture
```

f64 transcendentals against the series (`tests/f64_trans.rs`, 20,000
synthetic operands each, low 11 significand bits random, difference in
units of the 64-bit result's last place; 2^11 = 2048 is one f64 ulp):

| function | operands | median | p99 | max |
|---|---|---:|---:|---:|
| fsin | \|x\| < 8 | 1,352 | 121,071 | 2^24.8 |
| fcos | \|x\| < 8 | 1,619 | 153,357 | 2^24.9 |
| fsin | \|x\| < 10^6 | 1.8e8 | 1.6e10 | 2^39.5 |
| fcos | \|x\| < 10^6 | 1.8e8 | 1.6e10 | 2^41.3 |
| fyl2x | x in 2^±20 | 683 | 4,084 | 2^20.5 |
| f2xm1 | \|x\| < 1 | 835 | 3,315 | 4,859 |

No exception flag other than PE and C1 differs. Most results are within one
f64 ulp, as expected; the tails are the operand's own rounding to 53 bits,
amplified near zeros of sin and cos (and of log2 at x near 1), and for large
angles the operand error alone is up to |x| * 2^-53 of angle. Not turned on.
