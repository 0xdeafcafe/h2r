# The sim tier in the image

Status: Design. The tier compiler exists and runs five roots in a test dylib; nothing here is built.
Code: `crates/recompiler/emit/src/sim.rs` (`Closure`, `Tier`, `write`), `crates/recompiler/emit/c/h2tier.h`; this spec adds `crates/runtime/runtime/src/tier.rs` and a feature in `crates/rewrites/sim-bridge`. Tests: `crates/recompiler/emit/tests/sim_tier.rs`, `crates/rewrites/engine/tests/sim_tier_differential.rs`.
Switch: `H2_SIM_TIER` at build and `impl.sim.tier` at run (both proposed, default off).

The sim tier compiles the same lifted IR as the image under a looser
contract: registers, flags and x87 values are C locals, guest memory is
`base + address` with no check, guest calls are C calls, and floats are
host binary32 and binary64 with every operation checked
([sim-tier-experiment.md](../engine/sim-tier-experiment.md)). This spec
puts tier code into the production image as **roots**: guest function
entries whose closure runs on the tier when a run asks for it, entered in
place from reference code and from Rust. It is lever 7 of
[sim-10x.md](sim-10x.md). The rule that must hold: a root either produces
exactly the memory, registers and logical clock the reference image would
have produced, or it leaves no trace and the reference image runs.

It starts only after measurement M6 of sim-10x.md gives the tier a factor
of 2 or more on its five experiment roots.

## How it works

### A root's life

`live[i]` for a root's entry holds the host-call thunk, as for any
override ([contract §4.2](../runtime/contract-thin-transfers-flat-ram.md#42-transfers)),
and the override is `tier::enter`. A call of the root from generated code
therefore runs in place.

1. **Enter.** `tier::enter(cpu, ctx, root)` requires the flat window
   (`Runtime::flat_base()`), the float environment the boundary checks
   (`H2T_RARE_ENTRY`) and a zero leave word. It copies `CpuAbi` (the
   entry checkpoint), opens a **segment** and calls the root's boundary
   function `h2tier_<entry>(CpuAbi *, base, tx)`.
2. **Run.** Tier functions run to the root's return, to a call-out, or to
   a rare event.
3. **Commit.** At the root's return the segment is validated and
   committed, and `enter` returns the plain `RETURN` the reference
   function would have made. The generated caller continues inline.
4. **Abort.** A rare event or a failed validation rolls the segment back
   to its checkpoint and **leaves**: `enter` returns the transfer that
   resumes the reference image at the checkpoint. For the entry
   checkpoint that is the root's original handler
   (`Runtime::resolve_original`), invoked in place.

### Segments, the journal and the clock

A segment is the tier's unit of all-or-nothing work. It starts at a
checkpoint (the root's entry, or the return site of a call-out) and ends
at the next call-out or the root's return.

- **Journal.** Every tier store first tests a per-page byte,
  `tx->seen[address >> 12]`. Zero means the page is not yet in the
  segment: the store calls `h2t_touch`, which copies the 4 KiB page to
  the segment's undo buffer, appends the page number to a list and sets
  the byte. Stack stores hit the same one or two pages, so the test is a
  predicted branch. A store that crosses a page tests both. Rollback
  copies the saved pages back and clears the listed bytes; commit clears
  them and marks the flat window's write cards for the listed pages
  (`Guest::fold_cards` consumers see one mark per page, not per store).
- **Clock.** The tier counts guest accesses exactly as the reference
  image does (`H2_ACC_ONE`: one per attempted access, one per element of
  a string operation), in a local added to `tx->accesses` at each exit
  and before each call. The count is charged to the context at commit
  (`ExecCtx::charge_accesses`). The logical clock is therefore the
  reference clock, and a run with the tier on has the reference schedule.
- **Validation.** A segment commits only if the reference image would
  have run the same span without leaving:
  - `tx->accesses` is less than the slice budget the context had at the
    segment's start (a slice is 65,536 accesses,
    `runtime::preempt::DEFAULT_SLICE`; the reference would have polled an
    expired slice at its next loop header or call);
  - the transfers the span makes (one per call and return between tier
    functions, counted like the accesses) fit the transfer budget.

  Otherwise the segment aborts and the reference runs it, which yields at
  the retail point. The abort rate is a reported counter; roots are sized
  so it stays under 2% of entries (see "Roots").
- **The leave word at commit.** A word that became non-zero during the
  segment (an interrupt raised from another host thread) is seen by the
  caller's own test after the plain return, as after any callee.

### Call-outs

A tier function calls three kinds of target.

| Target | How |
|---|---|
| A tier function in the same module, not a barrier | a C call (as built) |
| A barrier, an indirect target outside the module, a function the tier refused, an overridden entry | a **call-out** |
| An indirect target inside the module | the generated switch `h2t_dispatch`, then one of the rows above |

A call-out site is known when the module is generated, or is an indirect
call. At such a site the emitter:

1. stores **every** architectural register, flag, XMM register and x87
   register the function holds in locals into `H2TState` (not only what
   the callee's signature reads: a reference callee's signature is
   unknown), and pushes the guest return address;
2. calls `h2t_call_out(s, base, tx, target, site)`. It validates and
   commits the open segment, moves `H2TState` to `CpuAbi`, and calls the
   target in place with `ExecCtx::call_inline`
   ([sim-10x-callouts.md](sim-10x-callouts.md));
3. on `Returned`: copies `CpuAbi` back to `H2TState`, records it and
   `site` as the new checkpoint, opens a new segment, and the function
   reloads all its locals and continues;
4. on `Leave(transfer)`: returns `H2T_LEAVE`. Every tier frame above
   returns it too, and `enter` returns the transfer to the runner. The
   guest stack holds retail return addresses and `CpuAbi` holds the
   callee's state, so the reference image finishes the root: each return
   lands on a registered resume site.

A function that can reach a call-out is marked **may-call-out** by a
fixpoint over the closure. A tier call to a may-call-out function also
spills the caller's full state first (step 1), because a reference
function reached below it may read any register. A call to a function
that cannot reach a call-out passes only its signature, as built.

A rare event after a call-out rolls back to the checkpoint that call-out
recorded, loads `CpuAbi` from it and leaves with `RESUME` at `site`: the
reference image resumes the innermost function at that return site, which
is a resume selector in every module. Frames the tier entered after the
checkpoint are abandoned with their rolled-back stores.

So an activation ends in exactly one of three ways: a plain return with
everything committed; a leave at the entry checkpoint (nothing happened);
a leave at a call-out return site (everything before it committed,
nothing after it happened).

### Host faults

Tier text is registered as a `hostfault` region
(`crates/recompiler/hostfault`, `register_region`). `enter` saves a
`__builtin_setjmp` buffer in the thread's tier frame. The region's
callback moves the saved PC to a landing that `__builtin_longjmp`s to it
with `H2T_RARE_FAULT` (`Fault::resume_at`); `enter` then aborts the open
segment like any rare event. A wild access is therefore exact: the
reference image repeats it and reports the guest fault.

### Hooks and overrides inside a closure

- **Barriers.** Every named point in `crates/hooking/guest-layout/src/points.rs`
  inside a closure is a barrier: tier callers reach it through
  `h2tier_live[k]`, one pointer per barrier, which holds the tier
  function while dispatch has no enabled override or chain at that entry
  and the call-out stub otherwise. `Runtime::edit_dispatch` keeps the
  pointers in step, as it does `live`.
- **Anything else.** An override or observer installed at a non-barrier
  address inside a closure disables every root whose closure contains it:
  their `live` elements go back to the reference handlers, and the log
  names the roots and the address. No hook is ever skipped.

### Build

`H2_SIM_TIER=1 h2 build` reads the root manifest
(`crates/recompiler/emit/tier-roots.toml`: root address, name, stage),
builds one `Closure` per stage with `Closure::build`, writes the tier
sources with `emit::sim::write` in shards of about 200 functions, and
compiles them with the image's flags plus `-ffp-contract=off`. The
objects go through `H2_OBJ_CACHE` like every other shard. The module
exports a table of `(entry, boundary function, signature hash)` and the
barrier array, and sets a module feature bit; a runtime that is asked for
`impl.sim.tier` and finds no table, or no flat window, fails the run with
a named error.

The reference image is unchanged by the tier build except for that table,
so one image serves both arms of every A/B.

### Roots

A root pays the thunk, the `CpuAbi` copy, the boundary's register moves
and a `setjmp`: about 100 ns (**S**). A root is worth having when its
closure's inclusive reference time per call is 2 µs or more, and short
enough that it rarely spans a slice end (a slice is roughly 0.5 ms of
reference work on Lockout).

| Stage | Roots | Needs | Share of guest time (**S**) |
|---|---|---|---:|
| 1 | Closures with no call-out: the five experiment roots' callers at the right grain (the node-matrix loop `bd090` with `1421f0` and `142a60`; the BSP segment and collision query `1697c0`; the float MOPP ray `301800` and the box query `300150`; the Havok solver `31c100` and the convex and weld agents `2ee610`, `2fa2f0`), the animation stream `2798a0` with its evaluators and codecs, shared math reached from them | T1–T5 | 30% |
| 2 | Roots with call-outs: the per-object pre and post update (`bc470`, `bc820`), the type trees under them, the light walk `c1400`, the sound service `125b40`, the effect and particle updates (`176e40`, `247140`), the Havok step `1c3d30` with Halo's shape callbacks as tier functions | T6, T7 | 60% |
| 3 | The tick's phases, called from the Rust tick driver (lane R1 of sim-10x.md): one boundary per phase instead of one per object | T8, R1 | 85% |

The picker (`h2 analyze --tier-roots`, lane T5) proposes roots from a
profile and the closure census: it lists each candidate's closure size,
refused functions, call-out sites and estimated time per call. The
manifest is edited by hand from its output.

### Refusals

The tier refuses functions with instruction forms it has no exact
lowering for (`fldcw`, 80-bit loads and stores, x87 transcendentals, MMX,
non-static x87 depth). A refused function is a call-out target, so
refusals cost speed, not correctness. Measurement M7 ranks them by sample
weight; lane T6 adds lowerings in that order. The known first candidates
(**S**) are the CRT float-to-integer helper `_ftol2` (`322b0c`, a control
word round trip around `fistp`), which the tier lowers to an exact
truncation with a range check, and the stack probe `320560`.

## Interfaces

| Interface | Shape |
|---|---|
| Boundary function | `uint32_t h2tier_<entry>(CpuAbi *, uint8_t *base, H2TTx *tx)`; returns `H2T_OK`, `H2T_LEAVE` or an `H2T_RARE_*` code |
| `H2TTx` | `seen` (one byte per 4 KiB page of the 4 GiB window), the touched-page list, the undo buffer, `accesses`, `transfers`, the checkpoint (`H2TState`, pc), `rare_pc` |
| `h2t_touch(tx, base, page)` | runtime function: save the page, list it, set its byte |
| `h2t_call_out(s, base, tx, target, site)` | runtime function: commit, call in place, checkpoint; returns `H2T_OK` or `H2T_LEAVE` |
| `runtime::tier::enter` | the override body for a root |
| `runtime::tier::Stats` | per root: entries, commits, leaves by cause (each `H2T_RARE_*` code, slice, transfer budget, call-out left), pages saved, call-outs |
| Agent API | `tier.stats` (read): the table above for the running game; documented in [agent-api.md](../runtime/agent-api.md) by lane T4 |
| Sim report | a `tier` block: totals of the same counters per tick |

The memory-guard protocol does not change: a root runs as generated code
does, between the context's enter and leave of the running state
(`ffi/memory.rs`), so a Rust guard on another thread waits for the root to
end.

## Rules and limits

- **Exact or absent.** No tier result is kept unless the segment
  validated. There is no tolerance and no "close enough".
- **The tier never runs a safe point.** Anything that needs one (an
  expired slice, a blocking callee, a preemption) is a leave.
- **No state of its own.** `H2TTx` is empty between activations; a
  snapshot taken at a runner safe point sees none of it.
- **Flat RAM only.** The tier reads and writes through the window. A run
  without one refuses `impl.sim.tier` by name.
- **MXCSR sticky flags are not kept**, as in the experiment: the
  comparison excludes them and nothing in the tick reads them. Lane T1
  re-checks that claim over the stage's closure (a `stmxcsr` or `fnstsw`
  reader in the closure makes its function refused).
- **One host thread.** The undo buffer and `seen` belong to the game
  thread's context. A second executor never enters a root.
- **Abort cost.** A leave costs the rollback (a page copy per touched
  page) plus the reference run. A root whose leave rate exceeds 2% is
  split or removed from the manifest.

## Lanes

Each is one branch of about two hours of code. T1 to T4 need the spec
above and nothing else.

| Lane | Scope | Files | Proof |
|---|---|---|---|
| **T1** journal and clock | Tier stores test `seen` and call `h2t_touch`; the access and transfer counts; `H2TTx` in place of `rare_pc`; the MXCSR-reader refusal | `crates/recompiler/emit/src/sim.rs`, `emit/c/h2tier.h` | `emit/tests/sim_tier.rs`: after a forced rare event every byte equals the entry state; the tier's access count equals the interpreter's count of accesses on every fixture; a page-crossing store journals both pages |
| **T2** call-outs | The may-call-out fixpoint; full spill and reload at call-out sites; `H2T_LEAVE` propagation; checkpoints; barriers through `h2tier_live` | the same two files | `sim_tier.rs` with a stub `h2t_call_out`: the state handed out equals the interpreter's at the call; a leave from depth three returns `H2T_LEAVE` with nothing after the checkpoint written; a rare event after a call-out restores the checkpoint |
| **T3** runtime | `runtime::tier`: `enter`, segments, validation, commit to cards and clock, rollback, the fault region, `h2t_touch`, `h2t_call_out` over `call_inline`, `Stats`; registration of the module's table; `edit_dispatch` keeping barriers and disabling roots | new `crates/runtime/runtime/src/tier.rs`, `direct.rs`, `generated.rs`, `dispatch.rs` | runtime tests on a hand-written tier module: commit charges the count; a segment over the slice budget leaves and the reference yields at the same transfer as with the tier off; a fault in tier text leaves; an override inside a closure disables its roots |
| **T4** switch and stats | `impl.sim.tier` feature: turns roots on by stage (`impl.sim.tier.stage1`...), `tier.stats`, the report block, switches.md and agent-api.md | `crates/rewrites/sim-bridge`, `crates/extensions/game`, `crates/live/sim-bench/src/metrics.rs` | feature-plan tests; the stats method's schema test |
| **T5** build and manifest | `H2_SIM_TIER` in `h2 build`: shards, object cache keys, the export table and feature bit; `tier-roots.toml` with stage 1; `h2 analyze --tier-roots` | `crates/recompiler/emit/src/compile.rs`, `lib.rs`, `crates/apps/cli` emit and analyze | emit tests on a synthetic module: table rows match roots; a root with no accepted closure is a build error naming the refusal |
| **T6** lowerings | The refusals M7 ranks first, one lowering each with its fixture | `emit/src/sim.rs`, `h2tier.h` | `sim_tier.rs` fixtures against the interpreter, 400 random states each |
| **T7** stage 2 roots | Manifest rows and the differential cases for each new root | `tier-roots.toml`, `crates/rewrites/engine/tests/sim_tier_differential.rs` | below |
| **T8** stage 3 roots | Phase roots under the tick driver | `tier-roots.toml`, `crates/rewrites/engine/src/sim_tick.rs` | below |

### The ladder for every stage

1. **Synthetic** (`emit/tests/sim_tier.rs`, no disc): the lane's fixtures
   against the interpreter.
2. **Differential** (`sim_tier_differential.rs`, `H2_XBE`): every root of
   the stage, 48 seeded cases each, tier against interpreter in every
   byte and CPU field but MXCSR; plus the same cases with a rare event
   injected (a NaN operand) to prove rollback and the reference rerun.
3. **Run parity**: `h2 sim diff` of `impl.sim.tier` on against off with
   `impl.sim.arena-hash`, Lockout and Waterworks, 9,000 ticks, four
   phantoms: it must print nothing. Because the clock is the reference
   clock, `H2_LOCKSTEP` whole-RAM hashes must match too; that is the
   stricter gate and the first to run.
4. **Leave census**: `tier.stats` over the same runs. Leaves under 2% of
   entries per root; no `H2T_RARE_CALL` on a stage 1 root.
5. **Cross-architecture**: the same match on x86-64 Linux gives the same
   hash series.
6. **Timing**: three unprofiled medians per arm on the quiet laptop
   (sim-10x.md, `timed`), and one profiled pair to confirm the roots'
   reference functions lost their samples.

## Evidence

| Fact | Label | Source |
|---|---|---|
| Tier functions exchange state through `H2TState` and return a rare code that every caller passes up | Code | `crates/recompiler/emit/c/h2tier.h`, `emit/src/sim.rs` (`write`) |
| A call outside the closure is `H2T_RARE_CALL` and nothing journals stores | Code | `emit/src/sim.rs` (`h2t_dispatch`), `h2tier.h` (`h2t_st*`) |
| Regions with a callback that moves the saved PC exist for this use | Code | `crates/recompiler/hostfault/src/lib.rs` module doc |
| `flat_base()` is fixed for the runtime's life | Code | `crates/runtime/runtime/src/services.rs` |
| Return sites are resume selectors of their owner | Code | `Dispatch::register_resume`; `crates/rewrites/sim-bridge/src/sim_objects.rs` tests |
| Boundary cost, root grain, stage shares | S | M6, M7 and `tier.stats` |

## Not verified

- The tier's factor over the reference on any root (M6).
- The share of tick time in functions the tier accepts (M7).
- How many pages a stage 2 or stage 3 segment touches, and so what a
  commit and a rollback cost.
- How often a segment spans a slice end at each root grain.
- That nothing in the tick's closure reads MXCSR or x87 status flags the
  tier does not keep.
- That every call-out return site in a closure has a resume selector in
  the module (the image omits some loop selectors; lane T5's build check
  lists any missing site and its root is refused).
- Linux hosts: the flat window copies a restored image there
  ([contract](../runtime/contract-thin-transfers-flat-ram.md), Q1), which
  the tier inherits.

## Related

- [sim-10x.md](sim-10x.md): the budget, the ranking and the measurements.
- [sim-10x-callouts.md](sim-10x-callouts.md): `call_inline`, which `h2t_call_out` uses.
- [sim-tier-experiment.md](../engine/sim-tier-experiment.md), [sim-review.md](../engine/sim-review.md), [sim-tier-writes.md](../engine/sim-tier-writes.md): the tier as built.
- [x87-native.md](../lifter/x87-native.md): the binary64 subset the tier's x87 values stay in.
