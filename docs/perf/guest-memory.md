# Runtime performance: guest memory access and the runner

Status: Built. Four switches (`H2_ENTRY_GATE`, `H2_DIRTY_INLINE`, `H2_DIRECT_GATE`, `H2_DIRECT_COUNT`) are built and off by default; see "Rules and limits".
Code: `crates/emit/c/h2fastmem.h` and `h2gen.h` (the generated side), `crates/emit/src/c.rs` (register homes), `crates/runtime/src/ffi/memory.rs`, `exec.rs`, `direct.rs`, `perf.rs` (the runtime side). Tests: `crates/emit/tests/{direct,exits,homes,fast_copy,perf}.rs`.
Switch: `H2_FASTMEM` (on), `H2_DIRECT_CALLS` (on), `H2_VIEW_CHARGE` (on), `H2_ACCESS_COUNT` (on), `H2_PERF` (off); listed in [switches.md](../runtime/switches.md).

Generated code reaches guest RAM, calls other generated handlers and
leaves to the runtime through the paths below. Each exists to keep a
guest access, a guest call and a handler exit cheap while the runtime
still sees exactly the state the checked Rust path would leave: the same
registers, memory, dirty pages, access count and slice boundary. The rule
that holds for all of them: with the switch off, the result is identical
(the tests below run each path against the reference path). The ABI
between a module and the runtime is in
[RUNTIME-CONTRACT.md](../../crates/emit/RUNTIME-CONTRACT.md); the profiling
tools are in [profiling.md](profiling.md). The measurements for every path
are in [notes/guest-memory.md](notes/guest-memory.md).

## How it works

### The inline RAM path

With `H2_FASTMEM`, `h2abi_v1_fast_acquire(ctx)` hands a handler a view of
RAM: a page table, the RAM pointer and the watch flag (`H2MemoryData` in
`crates/emit/c/h2fastmem.h`), behind an atomic gate (`H2MemoryShared.gate`).
A Rust memory guard excludes the gate. The runtime side is
`crates/runtime/src/ffi/memory.rs` (`fast_shared`).

A handler keeps the gate across consecutive accesses (`H2Gate`). With the
gate held, an access that lies in one page, has the page's permission bit
and a physical page inside RAM is one load or store of constant width at the
host address (`H2_HELD_TRY` in `h2fastmem.h`; `H2_LD` and `H2_ST` in `h2gen.h`). Anything else (gate not
held, a page-crossing span, no permission, MMIO, an atomic) is one call to
`h2_access_cold`, which is out of line, cold and `preserve_most`: it takes
the gate and retries, or releases it and runs the Rust helper. The hold is
dropped before anything that takes the context, on every exit, and after
`H2_GATE_TRIPS` (32) loop headers or REP trips. A Rust guard that finds the
gate held marks it wanted; the handler gives way at its next loop header.
Stores set physical dirty bits (`dirty_lo`, `dirty_hi`) so write watches
see them, and the hold remembers the last page it marked.

`REP MOVS` that the emitter recognises, flat and without segment override,
copies in page-bounded `memmove` chunks under the same gate
(`h2_fast_movs` in `crates/emit/c/h2fastmem.h`; `crates/emit/tests/fast_copy.rs`). Both spans
stay inside their current RAM page; overlap in the propagating direction
falls back to element-by-element execution, and each element still charges
two accesses.

### Access accounting and the exit call

With `H2_ACCESS_COUNT`, a handler counts its accesses and the runtime charges
them to logical guest time (`H2_ACCOUNTING` in `h2gen.h`). With
`H2_VIEW_CHARGE` the handler spends the count inline in the RAM view
(`h2_view_spend`, `h2_view_left`) instead of calling `h2abi_v1_charge`,
`h2abi_v1_budget` and `h2abi_v1_exit`; the runtime settles the view after
every native handler (`call_native`) and before every helper, so the slice
ends at the same access.

The shared epilogue calls `h2abi_v1_exit(ctx, n)` once, after the register
spill (`crates/runtime/src/ffi.rs`): the runtime settles the view's count,
then spends the handler's `n`. Without `H2_FASTMEM` the charge alone stays.
The boot bridge table has an `exit` slot installed by `h2boot_install_v3`;
a module built before it is installed through `h2boot_install_v2`
(`crates/boot/src/lib.rs`). `H2_EXIT_CALL=0` and `H2_DIRECT_VIEW=0` are C
defines the equivalence tests set to build the older exit and entries; they
are not part of `cflags_for`.

### Direct calls

With `H2_DIRECT_CALLS` (module feature 2), a guest `call` to a function entry
of the same module, whose continuation is one of the caller's own selectors,
calls the callee's `h2fn_` as a C function; a tail jump to one does the same.
A `RETURN` to exactly the continuation, with ESP above the return slot,
continues inline. Everything else goes to the runner.

The direct path runs only where the runner would have done nothing else:
the preempt word and interrupt queue are empty, `max_transfers` has a
transfer left, the nesting is under the context's cap (`DEFAULT_DEPTH` 32,
at most `MAX_DEPTH` 64, `crates/runtime/src/direct.rs`), the context's slot
says the runner resolved the target to this handler at the current dispatch
generation (any `edit_dispatch` stops direct calls until the runner resolves
again), and the page is executable in the inline view. The runtime counters
keep `transfers` as every guest transfer and `direct_transfers` as the part
made in C.

A direct callee runs on its caller's view. The caller lends it through the
last word of the direct-call block (`H2Direct.view`); the callee takes and
clears it and skips `h2abi_v1_fast_acquire`. Module feature 4
(`FEATURE_DIRECT_VIEW`, `crates/runtime/src/generated.rs`) marks a module that
lends; a runtime whose block ends before `view` refuses it. The runtime
clears `view` before it invokes any native handler itself, so a handler
entered through the runner always acquires.

The contract's rules are in `crates/emit/RUNTIME-CONTRACT.md` section 4,
"Direct calls".

### Register homes

A handler keeps in C locals only the registers it can write (a partial write
such as `AL` counts; EFLAGS with its mask when any flag is read or written, a
REP runs or an atomic runs; ESP always). It reads every other register from
`CpuAbi` where it is used and never loads, spills or reloads it
(`emit::c::homes`, `crates/emit/src/c.rs`). A direct call or tail jump spills
only the homed locals that may differ from `CpuAbi`
(`emit::c::dirty_at_exits`); the shared epilogue stores every homed local.
What the runtime sees at every exit is unchanged
(RUNTIME-CONTRACT.md section 4, "Register homes").

### Runner transfers

Every transfer that leaves generated code goes through the runner
(`crates/runtime/src/runner.rs`). The pieces that make one cheap:

- The per-context resolve cache has `RESOLVE_WAYS` (4,096) entries indexed by
  a multiplicative hash of the target (`crates/runtime/src/exec.rs`).
- The execute check is remembered per cache entry against the guest's layout
  identity (`Guest::layout_id`, `ExecCtx::check_execute_cached`). A layout
  identity is taken afresh at every mapping or permission change and never
  reused, so an unchanged value means the answer cannot have changed; a
  fault is never remembered.
- The interrupt check tests the queue's pending count inline first
  (`crates/runtime/src/interrupt.rs`).
- `timeline::now_ns` reads the clock directly on Apple platforms
  (`crates/timeline/src/lib.rs`), which frame stats call at every phase
  switch.

### The D3D write barrier

Every D3D entry is wrapped by `boot::d3d_glue::barrier_handler`
(`crates/boot/src/d3d_glue.rs`): it drains the watch, then invokes the
entry's own handler in place, so a D3D call costs one runner transfer. The
guest sees the same registers, memory and return; the dead stack word under
the caller's `ESP` keeps the caller's return address. Nothing is drained on
return, so stores made while an entry calls guest code reach the device at
the next entry. Test: `crates/boot/tests/write_barrier.rs`.

### Counters

`ExecCtx::enable_profiling(true)` turns on per-context counters
(`crates/runtime/src/perf.rs`); `take_profile` drains them and
`Profile::merge` aggregates threads and slices. They cover runner
transfers, runner and generated-loop safe points, memory accessor calls
(`memory_accessors` includes successful inline accesses; subtract
`fast_memory_accesses` for Rust accessor calls) and x87 helper calls.
`H2_PERF=1` at build time adds the generated loop and x87 counters; they
flush on every exit through the optional `h2abi_v1_profile(ctx, loop_polls,
x87_calls)` symbol (`crates/runtime/src/ffi.rs`), which a dylib bridge must
forward. `H2_COLD_STATS=1` counts cold accesses by cause. No hot-path counter
reads the clock. Timers are inclusive host durations: nested Runner, Kernel,
D3d, Gfx and Fallback times overlap and are not summed.

## Interfaces

- The module imports `h2abi_v1_fast_acquire`, `h2abi_v1_exit`,
  `h2abi_v1_read` and `h2abi_v1_write` and the other symbols in
  RUNTIME-CONTRACT.md section 1; the boot bridge installs them
  (`crates/boot/c/boot_bridge.c`).
- Every compiler switch is a `-D` define chosen in
  `emit::compile::cflags_for` (`crates/emit/src/compile.rs`). They are in the
  object cache key and the boot stamp: set them identically for `h2 build`
  and `h2 run`.

## Rules and limits

- Off-by-default switches, built and tested, each awaiting an in-game
  profile: `H2_ENTRY_GATE` (takes the gate as a handler starts),
  `H2_DIRTY_INLINE` (tests the dirty bit at the store site),
  `H2_DIRECT_GATE` and `H2_DIRECT_COUNT` (lend the gate or the count across
  direct calls), `H2_RAM_PAGE_CACHE` and `H2_FLAT_RAM`. They are listed in
  [switches.md](../runtime/switches.md), "Waiting for a measurement".
- `H2_FASTMEM=0`, `H2_DIRECT_CALLS=0` and `H2_ACCESS_COUNT=0` build the
  reference paths for bisecting a divergence.
- A direct callee that is entered through the runner always acquires its own
  view.
- Counters exist only in builds with `H2_PERF=1`; ordinary builds have no
  generated profiling instructions.

## Evidence

- **Test.** `held_gate_matches_the_helper_path_at_every_stop_and_fault`
  (`crates/emit/tests/perf.rs`) runs a synthetic handler against the helper
  path at seven slice lengths with and without accounting, for each switch
  set. `inline_accesses_match_the_helper_path_at_every_width_and_edge` and
  `a_new_hold_marks_the_page_the_last_hold_dirtied` cover the inline path.
- **Test.** `crates/emit/tests/direct.rs` builds one module with direct calls
  off and on and requires identical transfer, CPU, memory, events,
  backtrace and counts across nested and stdcall calls, recursion past the
  cap, faults, rewritten return addresses, every transfer limit, a slice
  ending at every access, and pending interrupts;
  `a_direct_callee_borrows_its_callers_view` counts the acquires and exits.
- **Test.** `crates/emit/tests/exits.rs` checks the epilogue, lend, borrow
  and hand-up for every accounting and fast-memory combination;
  `exit_settles_as_charge_then_release` is in `crates/runtime/src/ffi/memory.rs`;
  `crates/boot/tests/bridge.rs` loads an old-style bridge through v2.
- **Test.** `crates/emit/tests/homes.rs` and the differential suite
  (`crates/emit/tests/differential.rs`) cover register homes.
- **Test.** `exec::tests::a_remembered_execute_check_never_outlives_a_layout_change`
  and `the_resolve_cache_answers_each_target_with_its_own_handler` cover the
  runner caches; `timeline`'s `now_ns_is_monotonic_and_keeps_time_with_instant`
  the clock.
- **Run.** The numbers, dates and commits for each path are in
  [notes/guest-memory.md](notes/guest-memory.md). Experiments that lost are in
  [notes/not-adopted.md](notes/not-adopted.md).

## Not verified

- The whole-frame effect of each path in a level on a quiet machine: the
  notes record synthetic and contended-machine results only.
- The four off-by-default switches in the game.

## Related

- [profiling.md](profiling.md): `h2 profile`, `h2 bench`, the timeline.
- [sim-budget.md](sim-budget.md), [sim-attribution.md](sim-attribution.md): where a sim's time goes.
- [notes/gameplay-budget.md](notes/gameplay-budget.md), [notes/menu-budget.md](notes/menu-budget.md): frame budgets.
- [../lifter/sse-native.md](../lifter/sse-native.md), [../lifter/x87-native.md](../lifter/x87-native.md): native floating point.
