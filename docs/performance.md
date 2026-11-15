# Runtime performance lane

The acceptance target is **at least 1 presented frame per wall second while
sitting in the main menu**, using the boot branch with `H2_BOOT_GFX`.
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
and x87 helper calls. This counts ABI accesses, not all HLE Guest reads/writes.
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

## Ownership

This lane owns runtime/emit accessors, runner transfers, safe points and helper
call overhead. `codegen-opt` owns IR optimizations, `gfx-perf` owns wgpu work,
`perf-tooling` owns guest-aware sampling, Perfetto and `h2 bench`, and
`build-perf` owns compile-time improvements. Gfx/d3d source is untouched here.
