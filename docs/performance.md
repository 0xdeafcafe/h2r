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

Boot integration (not done yet; `H2_PERF` counters already reach the runtime
through boot's bridge):

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
cache. The master-plus-boot reference from docs/BOOT.md at the time was
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

`H2_SSE_NATIVE` enables guarded native binary32 arithmetic with observable
MXCSR preserved; [sse-native.md](sse-native.md) gives the proof and escape
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
RUSTC_WRAPPER= cargo test -p emit --test fast_copy guest_microbenchmarks -- --ignored --nocapture
RUSTC_WRAPPER= cargo test --release -p d3d --lib dirty_query_benchmark -- --ignored --nocapture
RUSTC_WRAPPER= cargo bench -p d3d --bench pushbuf
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
RUSTC_WRAPPER= H2_ACCESS_COUNT=1 cargo test -p emit --release --test perf generated_runtime_benchmark -- --ignored --nocapture
RUSTC_WRAPPER= cargo test -p emit --release --test perf image_gate_holds -- --ignored --nocapture
```

Not done here, in the order the code suggests: the runner's transfer path
(`call` is about 180 ns for a call and return, two `HashMap` resolves with
SipHash and a shadow-stack update per transfer; an address-indexed table
and direct calls between generated handlers are the next step, about 5 ms),
then cross-block dead stores and register promotion, which a flat profile
at `-O1` does not point at.
