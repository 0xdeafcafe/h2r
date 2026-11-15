# Runtime performance lane

The goal is **retail running correctly at 30 presented frames per wall second**,
a **33.33 ms** budget per frame. The first milestone is **1 FPS while sitting
in the main menu**, using the boot branch with `H2_BOOT_GFX`.
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

Boot integration (boot is not part of this branch):

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

## Inline RAM experiment (opt-in)

`H2_FASTMEM=1` compiles ordinary single-page RAM accesses into C bounds,
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
no Rust `h2abi_v1_read` or `Guest::checked_start` samples. The tooling lane's
original profile attributed 28% and 24% to those functions. Inline checks and
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

Other lanes' game processes overlapped this run. Camera progression and
changing contention prevent treating the ratios as causal accessor speedups.
The second control also retains the initialized memory gate and execute-page
cache. The external master-plus-boot reference supplied by the boot lane is
**1.91 FPS, 484 ms median frame**. A fresh-data, same-frame pair is queued
through the shared game gate to reduce these confounders.

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
that gap. Memory checks and runner dispatch remain this lane's targets;
codegen-opt owns the generated-operation budget, gfx-perf owns the largest
backend budget, and kernel/VFS persistence requires its owning lane.
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

All further game/menu runs use the shared `game` gate, and full-image
compiles use the `emit` gate, with `CARGO_BUILD_JOBS=4`, empty `RUSTC_WRAPPER`
and `H2_OBJ_CACHE=$TMPDIR/shared-objcache`. This lane needs its own generated
image because the emitter support code changes. Runtime-only follow-ups
reuse that image rather than triggering another full emit.

## Ownership

This lane owns runtime/emit accessors, runner transfers, safe points and helper
call overhead. `codegen-opt` owns IR optimizations, `gfx-perf` owns wgpu work,
`perf-tooling` owns guest-aware sampling, Perfetto and `h2 bench`, and
`build-perf` owns compile-time improvements. Gfx/d3d source is untouched here.

Reports must separate runtime/emit, IR passes (`codegen-opt`), wgpu
(`gfx-perf`), D3D parsing/state preparation, and kernel costs. Report the
measured menu FPS, interval and load conditions, then the time each owner
must remove to fit the 33.33 ms budget. A proportional extrapolation from
samples is an estimate, not proof of an achievable FPS; preserve guest
correctness, graphics work, input and scheduling throughout.
