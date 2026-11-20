# Architecture review, November 2026

Review of 2026-11-20. Frozen: later work is in the design pages and the code.

Written 2026-11-20 against master `bc48d2053`. An independent read of the
whole repository: the recompiler, the runtime, the hook engine, the
renderers and the crate structure. Nothing was built or run for it. The
evidence is the code, the docs, the measurements already recorded in
`docs/perf` and `docs/lifter`, and `git log`. Savings are estimates from
reading code unless a measurement is cited.

An earlier review, [2026-11-16-frame-time.md](2026-11-16-frame-time.md)
(2026-11-16), covered the gameplay frame-time gap. Most of its top five
were acted on. This one is broader and reaches a different conclusion
about where the remaining cost comes from; section 1.9 says where the two
differ.

## Summary

The project is 1.1 million lines of Rust in 88 crates, 152,000 lines of
docs and 3,809 commits (1,173 of them merges), all dated between
2026-11-14 and 2026-11-20. It boots, renders, plays campaign and
multiplayer, and has Live, co-op, bots, an editor and a Halo 1 conversion.
That breadth is real. The structure shows how it was built: many parallel
lanes, each adding a switch, a glue file and a doc section, joined in a
few central files.

Five findings carry most of the review.

1. **The recompiler's cost is in its contract, not its backend.** Every
   guest function is a resumable handler, every memory access is a
   possible precise exit, and the scheduler's clock is the count of memory
   accesses. Each later speed-up (direct calls, register homes, lent
   views, lazy flags, the gate, flat RAM) buys back part of what that
   contract costs. The result is 570 MB of C, 234 MB of machine code for
   about 4 MB of x86, an hour of clang, and 23.8% of the sim thread inside
   the runtime. Changing the backend (LLVM IR, Cranelift only) fixes none
   of that. Simplifying three rules does: flat RAM as the only memory
   model, a block-based clock, and imprecise memory faults.
2. **Fix the recompiler before rewriting the sim for speed.** Guest code
   is 70.8% of the sim thread and the runtime 23.8%
   ([perf/sim-budget.md](../perf/sim-budget.md)). The sim-rewrite plan's
   first three phases estimate 2.7-3.7 ms of a 14.12 ms tick. Codegen and
   runtime work applies to all guest code at once. Port the sim to own it
   and extend it, on a timetable that speed does not force.
3. **The hand-lifted ports are a dead end.** `crates/rewrites/engine/src/sim_native`
   and `sim_physics` transcribe single functions at register level, flags
   included. All 32 are off by default. They are neither readable Rust nor
   faster than the lifter's output, and the lifted function on the IR
   interpreter is already a cheaper oracle.
4. **The native renderer is a tenant of the layer it replaces.** It
   depends on `d3d`, enters through per-draw admissions, and is slower
   than the D3D path in the last recorded A/B (15.4 against 25.6 fps).
   It needs to own the frame from a typed scene snapshot.
5. **`boot` and `feature` are the structural problems.** `boot` is a
   145,000-line composition root that also holds the product UI.
   `feature` is a hook registry that absorbed the renderer's guest-side
   half. Central registries (`boot/src/lib.rs`, `registration.rs`,
   `points.rs`, `cli/src/main.rs`) are where every lane collides.

The ordered list of ten changes is in section 5.

## 1. Recompilation architecture

### 1.1 What is there

`analyze` (8.5k lines) finds functions by recursive descent, then tries
speculative candidates (code references, data pointers, gap starts) in an
overlay and keeps only clean decodes (`crates/recompiler/analyze/src/lib.rs:1-17`).
`lift` (7.9k) lowers each instruction to `ir` (3.7k): one `IrInsn` per
guest instruction, temps that never outlive their instruction, total
operations, explicit undefined flags and precise fault ordering
(`crates/recompiler/ir/src/lib.rs:10-52`). `opt` (1.4k) has two passes, dead flags
and lazy flags. `emit` (15k) writes one C function per owner in about
1,000 shards keyed by `entry >> 12`, and `clif` (2.1k) lowers the same
planned IR to Cranelift objects.

A generated function has the signature
`TransferAbi h2fn(CpuAbi *, ExecCtxOpaque *, uint32_t start)`. It can be
entered at any registered selector, keeps the registers it writes in
locals, and leaves by returning a transfer (call, jump, return, fault,
stop) to the Rust runner (`crates/runtime/runtime/src/runner.rs`). Return
addresses stay on the guest stack and resolve through a resume table.
All guest threads run on one host thread under a cooperative scheduler
whose clock is the number of guest memory accesses
(`crates/recompiler/emit/RUNTIME-CONTRACT.md`, sections 1.5 and 4).

### 1.2 What is good and should not change

- **The IR and its interpreter.** Total semantics, first-class undefined
  flags and a reference interpreter make every later stage checkable. The
  differential tests (interpreter against generated C, against Cranelift,
  against Unicorn fixtures, against hardware SSE under Rosetta) are the
  strongest part of the repository.
- **Discovery does not need to be sound, because the fallback makes it
  complete.** A target the analyzer missed stops the handler and the IR
  interpreter carries on from there (`crates/runtime/recomp/src/fallback.rs:1-20`).
  That is the right design for a binary with LTCG and jump tables. What
  it lacks is a loop back into the build: see 1.8.
- **C as the optimising tier.** See 1.6.
- **One host thread for guest threads.** The title was written for one
  CPU, and lockstep, films and snapshots depend on a reproducible
  interleaving.

### 1.3 The contract is the cost

Three rules in the contract set the size and speed of everything
downstream.

**Rule 1: every access checks a page table under a gate.** An inline
access is `H2_HELD_TRY` (`crates/recompiler/emit/c/h2fastmem.h:404-417`): a held-gate
test, a page-crossing test, a page-table load, a permission test, a
physical-range test, a watch test on stores, a charge and an address
computation. The gate is a lock shared with Rust memory guards; handlers
take it, drop it before every call that takes the context, and count loop
trips to bound how long they hold it (RUNTIME-CONTRACT.md, lines 33-110).
Seven guards for one load is why each access site is tens of instructions
with a cold call and a fault exit behind it.

**Rule 2: every access is a point where the handler may stop with exact
state.** `opt`'s rule is that state is observable at "faults (any load,
store, `FaultIf` or intrinsic may fault)" (`crates/recompiler/opt/README.md`). So a
flag or register write cannot be removed across a memory access, and the
lazy-flag machinery exists to make those exits exact without paying on
the hot path. That machinery is 43% of the generated C by bytes: 23% lazy
flags computed in place, 11% settled in place, 6% `h2lz_` switches, 3%
bookkeeping ([lifter/build-perf.md](../lifter/notes/build-perf.md), "What the C
is"). `lazy-flags` alone grows the C from 417 MB to 539 MB
(`crates/recompiler/opt/README.md`, "Measurements").

**Rule 3: the scheduler's clock is the access count.** Each handler keeps
a local count, charges it at exits, reads a budget on entry and polls at
loop headers (RUNTIME-CONTRACT.md, lines 348-400). The clock is tied to
how a backend performs accesses and calls, so the Cranelift tier cannot
be in a lockstep session with the clang tier
([lifter/cranelift-tier.md](../lifter/cranelift-tier.md), section 5), and a
native port that replaces guest code moves every later slice boundary.

On top of these sits the transfer model. A direct call between two
generated functions is allowed only if the preempt word is zero, the
interrupt queue is empty, the budget is non-zero, the depth is under the
cap, a per-context slot holds this target with the current dispatch
generation, and the target's page has the execute bit, read under the
gate (RUNTIME-CONTRACT.md, lines 606-622). A callee's return continues
inline only if five more conditions hold. Anything else goes to the
runner, whose loop validates the transfer and the CPU state, takes the
preempt reasons, checks interrupts, unwinds the diagnostic frame list,
looks for a continuation token, resolves the target, checks the execute
permission and updates up to four sets of counters, per transfer
(`crates/runtime/runtime/src/runner.rs:58-281`).

The codegen configuration shows the same history. `h2gen.h` and
`h2fastmem.h` hold 71 `#if` blocks and 13 independent feature macros
(`H2_ACCOUNTING`, `H2_VIEW_CHARGE`, `H2_VIEW_BUDGET`, `H2_ENTRY_GATE`,
`H2_DIRTY_INLINE`, `H2_EXIT_CALL`, `H2_DIRECT_CALLS`, `H2_DIRECT_VIEW`,
`H2_DIRECT_GATE`, `H2_DIRECT_COUNT`, `H2_INDIRECT_CALLS`, `H2_FLAT_RAM`,
`H2_RAM_PAGE_CACHE`); `emit` reads 57 distinct `H2_*` names. Most
combinations are never tested together.

### 1.4 Three rule changes

**Flat RAM as the only memory model.** It exists, opt-in, since
`bd6b600f8` and `5dc8d8ca7` (2026-11-20): a guarded 4 GiB window per
guest with the 64 MB of RAM aliased into it, four windows on 16 KiB
hosts, and first-write faults in place of software dirty bits
(RUNTIME-CONTRACT.md, lines 128-200). An access becomes one host load or
store at `base + address`. Finish it, make it the default, then delete
what it replaces: `H2_HELD_TRY`, the page cache, the entry gate, inline
dirty marking, the span grouping, `fastmem-4k` and `crates/recompiler/fastmem-proto`.
Two parts need care.

- The flat path still takes the gate (`H2_SPAN` in
  `crates/recompiler/emit/c/h2gen.h:750-752`). The gate should leave generated code
  altogether. Mappings change only inside kernel HLE, which runs on the
  game thread when no handler is running, so generated code needs no lock
  to read the translation. Host threads that read guest memory while the
  game thread runs (the render thread, the second executor) need a
  handoff or a snapshot, not a lock every handler takes.
- MMIO and unmapped addresses become host faults caught by the guard
  pages. That is acceptable only with the next change.

**Imprecise memory faults.** The Xbox does not demand-page committed game
memory, and RUNTIME-CONTRACT.md already says the runtime "does not
restart on its own" after a fault. If no shipped code path recovers from
an access violation through SEH, a faulting access is a crash report, and
the report needs the guest PC and a best-effort register dump, not exact
flags. Confirm it with a census first: count FAULT events and SEH
continue-execution dispositions over a campaign run and a multiplayer
match through the agent API. If the count is zero, drop loads and stores
from `opt`'s list of observation points. Flags then die across accesses
by ordinary dead-flag elimination, most `LazyFlags` groups disappear, and
the lazy-flag settle code goes with them. Expected effect: roughly 35-40%
less C, and a matching cut in what clang optimises away after parsing it.
Keep precise state at the places a handler really stops: calls, returns,
loop polls, `FaultIf` for divide errors, stops and atomics.

**A block clock.** Replace the access count with a cost the planner
assigns in the IR: a constant per loop header and per function entry
(for example the number of guest instructions on the path, or simply
one). The handler decrements a budget at the poll sites it already has.
HLE entries and native ports charge a declared constant. This removes
`H2_ACC_ONE`, the flush before calls, the charge at exits and the budget
read on entry; it makes both backends interleave identically; and it
stops a port from moving slice boundaries for the rest of the tick.

The second and third changes alter where threads interleave, so lockstep
traces, films and sim-diff baselines must be regenerated. Do both in one
lane so that happens once.

### 1.5 Removing the runtime's 23.8%

The sim profile attributes 6.25% to `handler::execute`, 4.54% to the run
loop, 1.36% to guest reads, 1.53% to exit and charge, 1.31% to resume
and resolve, 0.73% to context creation and 0.52% to fast acquisition
([perf/sim-budget.md](../perf/sim-budget.md), "Runtime and other host
work"). Static calls are nearly all direct already, so the runner's load
comes from four sources: Rust handlers (HLE and hooks), returns that miss
their inline continuation, direct calls refused at run time, and indirect
calls. `H2_ENTRY_STATS` (`crates/runtime/runtime/src/entry_stats.rs`) can split
those; record that split in the doc before choosing an order. The
changes, cheapest first:

1. **A dense entry table in place of the slot cache.** The emitter knows
   every entry. Give each an index and let a static call load
   `entries[index]` and call it. An override swaps the pointer. That
   removes the slot hash, the generation compare, the collision misses
   and the warm-up through the runner. Indirect calls map the target
   through a table indexed by `pc - text_base`.
2. **Hooks that stay on the fast path.** Today a slot is filled only for
   `Handler::Native`, so any hook on a hot function sends all its callers
   through the runner, and one `edit_dispatch` invalidates every slot
   (`crates/runtime/runtime/src/direct.rs:12-17`). The code already avoids hooks
   for that reason: the datum and tag leaves stay off because "each
   override takes them off the direct-call path"
   (`crates/hooking/hooks/src/leaves.rs:30-38`). Give exact Rust overrides a
   C-ABI thunk with the handler signature so they can sit in the entry
   table like any generated function.
3. **Direct HLE entries.** A D3D, DirectSound or kernel call is a
   transfer out to the runner and a return back in. With the same thunk
   shape an HLE entry is one call from generated code, and the stdcall
   return is handled by the thunk.
4. **One word for "leave now".** Fold the preempt word, the interrupt
   count and the budget into one word that is zero when a direct call is
   allowed. Drop the execute-bit read per call: `.text` permissions do
   not change after load, so check them when the module registers.
5. **A lean runner loop.** Move ABI validation, the diagnostic frame
   list, entry stats, both censuses and the profile counters out of the
   per-transfer path into a diagnostic runner chosen once per run.
6. **After hooks without allocation.** `install.rs:366-384` clones three
   `Arc`s, boxes a continuation and locks `rt.memory()` on every call of
   a function with an `after` hook.

With flat RAM and the block clock removing acquire, exit and charge, a
reasonable target is under 6% of the sim thread in the runtime.

### 1.6 C against the alternatives

- **Direct LLVM IR.** Clang's front end is 11% of compile time
  (build-perf.md, "Where clang's time goes"). Emitting IR saves at most
  that, and costs a pinned LLVM, a second text format to debug, and the
  portability that lets the same C build for tvOS, Windows and Linux.
  Not worth it.
- **Cranelift only.** Cranelift compiles the image in 37.5 s
  (cranelift-tier.md, section 7) but does not inline the x87 and SSE
  helpers and optimises less. The game is not yet at its frame budget, so
  it cannot be the only tier today. It matters for a different reason:
  the user supplies the disc, so every user builds the image on their own
  machine. An hour of clang, and a C toolchain as a prerequisite, is a
  product problem. The fast tier is the answer to that, and the three
  rule changes above are what make it cheap: with a one-instruction load,
  a table call and a block clock there is very little backend-specific
  fast path left to port twice. Without them, every codegen improvement
  has to be written in `h2gen.h` macros and again in `crates/recompiler/clif`.
- **Whole-program or per-function.** Per-function owners in address
  shards are right for this ABI. Whole-program optimisation has nothing
  to work with while calls go through a table.
- **Stackful guest threads.** Running each guest thread on its own host
  stack would make guest calls and returns ordinary host calls and remove
  selectors, resumes and hand-ups. I considered it and do not recommend
  it. Snapshots save and restore guest threads at safe points
  (`d6ed91358`), and that works because all execution state is in
  `CpuAbi` and guest RAM, never in host frames. Resumable handlers are
  the price of snapshots, rollback and films. Keep the model and make its
  fast path thin.

### 1.7 Build time

The measured facts (build-perf.md): clang is nearly all of a cold build;
per-function time grows as lines^1.4; 663 of 18,647 handlers are 2,000
lines or longer and take about half of all clang time; `-O0` costs 13% of
`-O2` and `-O1` costs 84%. The recorded cold build was 11 minutes at
`-O1` on a quiet-ish machine; the hour reported for `-O2` includes other
lanes' load.

In order of payoff:

1. **Emit less.** Flat RAM and imprecise faults shrink every access site
   and remove most flag code. Compile time follows machine-code size, and
   the register allocator and coalescer are 38% of a median shard.
2. **Tier by default.** `H2_OPT_COLD` and `H2_HOT` exist and are off
   (`crates/recompiler/emit/src/compile/tiers.rs`). A hot list is guest addresses
   from a profile, which the repository's rules allow. Ship one, compile
   the hot sources at `-O2` and the rest at `-O1` or `-Os`, and measure
   fps once. If nine tenths of clang time is in cold sources, `-O0` for
   them cuts the build to about a fifth.
3. **Split the giant handlers.** A handler can be cut at any block
   boundary where state is canonical, because that is what a selector
   already is. Cut owners above a line threshold into parts joined by
   tail transfers, with cold blocks grouped in the part that compiles
   cheaply. This attacks the lines^1.4 term directly.
4. **Finish the fast tier and remove its clang dependency.** The registry
   and helper library are still C compiled by clang (cranelift-tier.md,
   section 2). Emit the registry as data from Rust and build the helpers
   into the runtime, so a first run needs no C compiler.

A reasonable target is a first run in under a minute and an optimised
image in 10-15 minutes on the same machine.

### 1.8 Discovery

Two gaps.

- **No feedback from runs.** When the fallback interpreter handles a
  missed target, nothing records it for the next build. Write each
  fallback entry PC to a per-user file and have `analyze` read it as
  certain seeds. The contract already anticipates "a list of PCs from a
  run's fault log". The earlier review found two jump tables costing
  3.3% of a frame this way.
- **Speculative function starts.** A gap-scan or code-reference candidate
  that is really the middle of a function turns local branches into
  transfers. Report how many executed `JUMP/ENTRY` transfers come from
  branch sides and fall-throughs (the census already separates them), and
  merge owners where the count is high.

### 1.9 Floating point

The design is right: exact software ext80 as the authority
(`crates/recompiler/x87`), native fast paths proved equal to it, hardware-checked
SSE reference semantics. Two structural steps remain.

- **x87 stack promotion.** Values live as ten bytes in `cpu->x87.st[]`
  and each native operation unpacks, checks tag and control words,
  computes and repacks. Compiler-generated x87 code has a statically
  known stack depth at each instruction. Track TOP in the planner
  (`H2_X87_TOP` is the opt-in start) and keep stack slots in locals as
  host doubles with a guard exit to the soft path, materialising ext80
  only at exits.
- **Decide which exception flags are observable.** `analyze/src/fpstats.rs`
  already censuses floating-point use. If no reachable code reads the
  MXCSR sticky bits or the x87 precision flag other than to save and
  restore them, stop computing them, set the host rounding and
  denormal modes from the guest's at thread entry, and let an SSE
  operation be one host instruction. Keep the software tables for
  `rcpps` and `rsqrtps`, which differ between hosts.

Where this review differs from the earlier one: that review ranked five
per-event costs and proposed making each cheaper inside the existing
contract, and those gains were taken. What remains is the contract
itself. It also suggested cheap stack accesses as a later step; flat RAM
subsumes that.

## 2. Codebase structure

### 2.1 Shape

| | lines of Rust |
|---|---:|
| `boot` | 144,722 |
| `cli` | 78,215 |
| `d3d` | 68,259 |
| `live-server` | 64,967 |
| `bots` | 44,157 |
| `tags` | 42,351 |
| eleven `h1*` crates | about 123,000 |
| `engine` + `feature` | 72,404 |
| recompiler core (`analyze`, `lift`, `ir`, `opt`, `emit`, `clif`, `x87`, `simd`) | about 50,000 |
| `runtime` | 17,955 |

The recompiler core is 5% of the code and has a clean dependency chain:
`xbe` to `analyze` to `lift` to `ir`, then `opt`, `emit` and `clif`;
`runtime` depends only on `timeline`, `x87` and `xbe`. The problems are
above it.

### 2.2 `boot`

`boot` has 278 files, 149 `mod` lines, fan-out to 51 crates and 224
`static` items. `lib.rs` is 2,906 lines and the second most-changed file
in the repository (242 commits); `run_configured` at
`crates/apps/boot/src/lib.rs:1067` is one function of about 1,460 lines. It is
big because it is the only crate allowed to see everything, so every
cross-crate join lands there, and product features were then written in
it directly. By content: about 40,000 lines of guest UI and screens
(`ui_kit/`, `list_class.rs`, `mods_screen`, `map_select`, variants,
skulls), 17,000 of renderer wiring (27 `native_*_glue` files), 10,000 of
Live, co-op, host and theater glue, 6,000 of bot glue, 17,000 of tests.

Split it along those lines: `ui-kit`, `game-ui`, `render-bridge` (merged
with `feature/src/native_*`, see section 4), `net-glue`, `bot-glue`, and
a thin `boot` that builds, loads and runs. `studio` depends on all of
`boot` for one type, `boot::ui_kit::input::Input`
(`crates/apps/studio/src/app.rs:203`); the `ui-kit` split removes that edge.

### 2.3 `feature`, `engine` and `sim`

`feature` is 35,843 lines, of which the hook machinery is about 2,500
(`lib.rs`, `plan.rs`, `install.rs`, `entry.rs`, `switches.rs`). The rest
is the native renderer's guest half: `pipeline_render.rs` (3,165),
`render_census.rs`, `render_timing.rs` and 18 `native_*` modules.
`feature` depends on `native-render`, which depends on `d3d`. The hook
registry therefore sits above the renderer and the D3D layer, and
anything that wants a hook pulls both in.

`crates/live/sim` is not the simulation. It is the headless match harness
(`crates/live/sim/src/lib.rs:1-9`). The native sim code is in
`feature/src/sim_*.rs` and `engine`. Rename it `sim-bench` before the
sim rewrite starts using the obvious name.

### 2.4 `cli`

`cli` is 78,215 lines in 117 files. About 20,000 are the Live web art
pipeline (`live_models.rs`, `live_geometry.rs`, `live_art.rs` and
relatives), 4,400 are a branch-following supervisor and a tunnel
(`follow.rs`, `tunnel.rs`), and about 8,000 are test drivers and verdict
logic for bots, co-op and sim. Move the art pipeline to a library crate
and the drivers next to what they drive. `main.rs` (1,441 lines, 111
commits) is another merge hot spot.

### 2.5 Central registries and merge cost

31% of commits are merges. The most-changed files are `Cargo.lock` (270),
`boot/src/lib.rs` (242), `docs/graphics/native-renderer.md` (169),
`cli/src/h1_import.rs` (145), `engine/src/points.rs` (123),
`d3d/src/device.rs` (121, and 6,657 lines), `cli/src/main.rs` (111),
`boot/src/registration.rs` (67). Apart from the two real hot files, these
are lists that every lane appends to. There are 14 commits whose subject
is a compile slip or merge fix (`bcc4455be` is one).

Make registration local: each module exposes one `register(&mut Registry)`
and its crate lists its modules, so a lane adds a file and one line in
its own crate. Split `points.rs` by subsystem. Docs change more than any
crate (2,949 file changes against 2,372 for `boot`), which is the same
problem in prose: see 2.8.

### 2.6 Switches and environment variables

There are 466 distinct `"H2_*"` string literals in `crates/` (183 in
`boot`, 108 in `cli`, 57 in `emit`, 35 in `d3d`), 40 distinct `impl.*`
switch names (35 of them `impl.render.*`), and about 35 `sim.native_*`
keys built from tables. Switches are a `BTreeMap<String, bool>` parsed
from `H2_FEATURES` (`crates/hooking/hooks/src/switches.rs`) with defaults
scattered across call sites. The planned parent `impl.sim.native` does
not exist in code; the keys are `impl.sim.native_x`, whose parent is
`impl.sim`.

This is the largest hidden cost in the repository. Nobody can say which
combinations work, and every off-by-default switch is code that the
default build does not exercise. Three rules:

- One typed switch table with name, default, owner and removal condition.
  A switch not in the table does not parse.
- A switch that has been off for more than one integration cycle is
  deleted with its code, or turned on.
- Environment variables are for paths and diagnostics. Behaviour goes
  through the switch table or a CLI flag.

### 2.7 Tests

There are 9,797 `#[test]` functions and 504 `#[ignore]`. The differential
suites are good: `crates/recompiler/emit/tests/differential.rs`,
`crates/recompiler/clif/tests/differential.rs`, `crates/recompiler/lift/tests/oracle.rs`, 14
`*_differential.rs` files in `crates/rewrites/engine/tests`. Three weaknesses.

- **CI runs none of them.** `.github/workflows/ci.yml` runs clippy and
  `cargo test --workspace` with no disc, so the oracle fixtures, the
  engine differentials and every disc test skip. The synthetic suites
  (emit, clif, opt) do run; the ones that check ports do not. A local
  gate that the coordinator runs with the disc should be a script in the
  repository, not a convention. The shared test ledger is not referenced
  by anything in `scripts/` or the `justfile`.
- **The recompiler core is thinly tested for its importance.** `emit` has
  152 tests, `lift` 119, `analyze` 102, against 1,279 in `boot`. The
  oracle corpus counts in ARCHITECTURE.md predate x87 lowering and say so.
- **Each crate has its own harness.** `engine` and `native-render` both
  take optional `analyze`/`ir`/`lift`/`xbe` dependencies to rebuild the
  same "run the lifted original" fixture. Make it one dev crate.

The record-and-replay harness that ARCHITECTURE.md describes ("Calls to
a function can be recorded ... Each override is replayed against those
recordings") does not exist. The engine differentials use seeded random
states, not states the game reaches. Section 3.4 proposes the
replacement.

### 2.8 Delete, merge, rename

- **Delete:** `crates/recompiler/fastmem-proto` (2,563 lines, no dependents, its own
  table row says prototype); `crates/tools/viewer` (7,258 lines, fan-in zero,
  superseded by `studio`) once its shadow lab is moved or dropped; the
  stage 7a render-loop ports that the docs say save nothing; the
  hand-lifted sim ports as each is superseded (section 3).
- **Merge:** the eleven `h1*` crates into three or four (reader,
  converters, campaign); `oracle`'s WMA oracle out of the x86 oracle;
  `boot/native_*_glue` with `feature/native_*`.
- **Rename:** `sim` to `sim-bench`; `apple-host` to something that covers
  the UWP host that depends on it.
- **Docs:** ARCHITECTURE.md's crate table lacks 24 of the 88 crates
  (`agent`, `bots`, `clif`, `native-render`, `snapshot` and others) and
  describes `engine` as leaf overrides. `native-renderer.md` is over
  3,600 lines and contradicts itself on the default (line 3 says every
  stage has defaulted on; the status at line 3605 says the switches stay
  off). Split each area's doc into a short current-design page and a
  dated lab notebook, and generate the crate table from `Cargo.toml`
  descriptions.

## 3. The hook engine and the path to Rust-owned subsystems

### 3.1 The engine as built

Points are named addresses in `crates/hooking/guest-layout/src/points.rs`: 331 of them,
each with a signature kind, a required or optional class and a doc
reference. A hook chain per address runs observers, `before` hooks (which
may return early), the body or its replacement, then `after` hooks
through a shadow address (`crates/hooking/hooks/src/install.rs:256-417`).
Mid-function `Site` hooks need the emitter to have emitted a selector at
that PC.

Four weaknesses, in order of consequence.

- **Hooks cost the fast path** (1.5, item 2). This shapes behaviour
  already: hot leaves are left unported because porting them makes their
  callers slower.
- **Addresses are not in one table.** Beyond the 331 points there are
  about 6,300 other five-digit-or-longer hex literals in `boot`,
  `feature` and `engine` sources: local constants for globals, field
  offsets and resume PCs (`crates/rewrites/engine/src/sim_physics/object_update.rs:6-19`
  is typical).
- **No point is verified against the image.** `ImageAddrs` has `Base` and
  a `Remap` meant for tests and a future title-update map
  (`crates/hooking/hooks/src/plan.rs:35-56`). The only check against a real
  image is one ignored test over about 20 render points. A point whose
  address is wrong fails at run time, or silently if the install degrades
  to "body runs alone".
- **Site hooks couple features to the generated image.** Adding one means
  a rebuild, and a missing selector skips the port.

### 3.2 What has been ported

- Leaf overrides (datum, tags, locale): only the locale group is on.
- Render-loop ports: off.
- `sim_native` (16 entries) and `sim_physics` (16): all off. All landed
  in 12 commits dated 2026-11-20.
- `hs` has a native HaloScript VM that is not wired in.
- State is always guest RAM. Nothing is Rust-owned.

The sim ports are written against `CpuAbi`: they assign `eax`, push and
pop the guest stack, compute EFLAGS and re-enter at mid-function resume
PCs. They are the lifter's output written by hand in Rust.
[engine/sim-rewrite.md](../engine/sim-rewrite.md) already demotes them to
"reference and test oracles", but the lifted original running on the IR
interpreter is a cheaper oracle and needs no maintenance. Stop writing
them, and delete each when its subsystem lands.

Guest access is by raw offset almost everywhere. A typed layer exists
(`engine::view` and its `record!` macro, about ten records), but there
are about 3,500 raw `u32()`/`set_*()` calls in `engine`, 3,200 in `boot`,
1,550 in `feature`, against roughly 100 uses of a typed view in total.
Views read through `&dyn GuestMemory`, so each field is a virtual call.

### 3.3 The sim-rewrite plan

Its core is sound: guest-layout memory stays the persistent truth, Rust
owns the work plus static, derived, transient and extension state,
derived state is dropped on revert and restore, each subsystem has one
switch that is deleted on exit, and the first boundary (animation
sampling) is one below which nothing calls back into guest code.

Where I disagree or would be careful:

- **Its speed case is weak, and it should not be the reason.** Phases 1
  to 3 sum to 2.7-3.7 ms of 14.12 ms. The halving it targets depends on
  Havok. It also counts the runtime's 23.8% as if rewriting guest code
  removed it; that share is a runtime defect and section 1.5 removes it
  for all guest code. The reason to port the sim is to own it.
- **The attribution cannot order the phases.** 37.5% of guest samples
  are "Unreached" and 14.3% "Multiple" in the static census. Use an
  inclusive measurement per phase (the host-audit hooks) before choosing
  what follows animation.
- **Bit-exactness against retail limits the structural win.** While the
  bar is exact, a port must keep retail's operation order, so the gain
  is decode caching and layout, not algorithms. That is fine for a
  pilot; it means the readable, restructured sim arrives only when the
  bar relaxes to self-determinism, subsystem by subsystem, with the
  user's sign-off each time.
- **Object update keeps per-type callbacks in guest code.** A gather,
  guest callback, scatter loop where the callback mutates sibling
  objects defeats the working set. Port the callbacks' closure first or
  leave objects until then.
- **Havok.** It is a statically linked library with no source. A
  replacement is a new physics engine and permanently changes feel
  (superbounces included) and every recorded film. Do not start it
  without a product decision that this is wanted.
- **The extension arena** adds a second format to saves, films, hashes
  and snapshots. Introduce it only with the first feature that needs it.

### 3.4 Recommended path

**Unit of port:** the call-closed subsystem boundary. Not single
functions, and never a register-level transcription.

**Foundations first.** These are small and everything else depends on
them.

1. **A typed guest layout crate.** One definition per guest structure,
   generic over the memory type so a field read compiles to a load. Move
   field offsets and globals into it, and add a lint (a test that greps)
   against raw guest offsets outside it. `bots`, `live-telemetry`,
   `agent` and the native renderer's extraction use it too, so a layout
   fix lands once.
2. **Shadow execution in place of record-and-replay.** The runtime
   already has copy-on-write guest memory (`crates/runtime/runtime/src/cow.rs`)
   and a second executor (`docs/runtime/second-executor.md`). For a
   ported entry, at each call: fork guest memory, run the lifted original
   on the fork, run the port on the live memory, and compare return value
   and written pages. Report the first divergence with the entry state
   saved as a fixture. Expose it as an agent API method so a sim run can
   turn it on for one entry. This checks ports against states the game
   reaches, needs no recording format, and makes "default on" a decision
   backed by a count of matching calls.
3. **Per-point verification.** Each point carries a check the build can
   run against the user's image (the function's discovered size and call
   shape from `analyze`, not bytes), so a wrong address fails at install
   with the point's name, and a title update needs only a remap.
4. **A port scaffolder.** The lifter has typed IR for every function.
   Emit a first-draft Rust body from it in terms of the layout crate
   (loads and stores as field accesses where the offset is known). A port
   then starts as a mechanical translation that already passes shadow
   execution and is refactored under that test, which is how
   decompilation projects work.

**Order:** animation sampling as the pilot for the whole ladder; then
wire in the HaloScript VM, which is written, integer-exact and relevant
to campaign and Halo 1; then collision queries; decide on objects after
an inclusive census. Ownership moves fully to Rust only where every
reader is already Rust (particles and effects once the native renderer
owns them, the sound simulation).

**Gate:** arena-hash parity over the three sim cases plus one campaign
level, a cold-cache arm and a cross-architecture arm, and a shadow run
with zero divergences. A port turns on in the lane that lands it and its
switch is deleted.

## 4. Rendering

### 4.1 The D3D path

The boundary is the D3D8 API: 88 overrides (`crates/platform/d3d/src/runtime_glue.rs`,
adapters in `abi.rs`). It leaks because the title is LTCG and inlines
D3D: game code writes the render-state and texture-state arrays and the
push buffer directly, so the device treats the guest arrays as
authoritative, re-reads them at every draw and decodes a real push
buffer at every entry (`pushbuf.rs`, `regs.rs`, `device/fragments.rs`).
It is an API replacement plus part of an NV2A method decoder. Shader
translation (`crates/formats/shaders`, `d3d/src/nv2a.rs`) and the persisted
pipeline keys (`pipeline_store.rs`) are sound.

The crate is 37,000 lines of source and 29,000 of tests. `device.rs` is
6,657 lines. The `Backend` trait has 55 methods (`backend.rs:1075`) and
six implementations stacked as decorators
(`ThreadedBackend<InterpBackend<ScaledBackend<GfxBackend>>>`, composed in
`crates/extensions/game/src/d3d_glue.rs`), so every new capability is written six
times.

### 4.2 The native renderer

`native-render` (28,500 lines) loads tags once and reads the camera, node
matrices and lists from guest RAM each frame. It makes the game skip a
replaced subtree by editing mask bits in guest memory and restoring them
(`skip.rs`). Its passes enter the D3D device through `ExternalPass`,
`ExternalPlan` and `DrawAdmission`, record into private encoders that are
spliced into the D3D submission order, share the guest's colour and
depth surfaces, and fall back to the original draw on any refusal.

Three structural problems.

- **The dependency points the wrong way.** `native-render` depends on
  `d3d` for the admission types. The new renderer is subordinate to the
  layer it is meant to retire.
- **It is slower.** The last recorded A/B on 03a has identical frames
  and D3D at 25.6 fps against native at 15.4, with round 4 "pending
  measurement" (`docs/graphics/native-renderer.md:3561-3575`). The cause
  given is synchronous admissions that make the game thread wait on the
  render thread, and per-pass load and store (73 shadow passes a frame).
  Both follow from being a per-draw tenant. The doc disagrees with itself
  on whether this path is the default; if it is, a slower path shipped.
- **About 35,000 lines of glue surround a 28,000-line renderer:** 15,400
  in `boot/src/native_*_glue.rs` and `d3d_glue.rs`, about 15,500 in
  `feature/src/native_*`, with guest addresses in the renderer itself
  (`screen_look`, `object_lighting`, `hud`, `objects`, `lights`, `glow`,
  `skip`) and about 200 guest reads through `dyn GuestRead`.

There are also three or four world renderers. Seven crates create wgpu
render pipelines; the editor viewport (`scenario-edit/src/render.rs`),
`viewer` and its shadow lab do not use `native-render`. Geometry decode
is shared through `tags::geometry`, which is the part that was done
right.

### 4.3 Target structure

- **A scene extraction layer** is the only code that knows guest offsets
  (built on the layout crate of 3.4). Once per view it produces an owned,
  typed `FrameScene`: camera, visible clusters, object instances with
  palettes, lights, transparents, effect quads, post parameters.
- **`native-render` becomes `FrameScene` to wgpu**, with no `d3d`
  dependency and no guest reads. It then runs entirely on the render
  thread against the snapshot, which removes the synchronous admission,
  and it serves the editor and viewer.
- **The native renderer owns its targets and pass graph.** What stays on
  D3D (UI, HUD, loading screens, Bink) composites as a layer. Those are
  cheap and can stay on D3D indefinitely.
- **`FrameScene` supersedes the pipelined-frame prototype.** A typed
  snapshot handed to the render thread achieves what running the guest's
  render front end on a second executor against a COW copy was for, with
  far less machinery.

Stop: the stage 7a orchestrator ports (`views`, `game_view`,
`scene_passes`, `layer_draw`, `layer_entries`, `states`, `states_skip`),
which are documented as saving nothing; per-stage `_walk`, `_split` and
`_skip_build` switches (collapse to `impl.render.native`); new `Backend`
methods and decorators; and frame interpolation in `d3d/src/interp`
(5,500 lines) if the high-framerate decision that replaced it stands.

## 5. The ten changes, in order

Each is one lane unless noted. Payoffs are estimates.

1. **Thin transfers: entry table, hook thunks, direct HLE entries, lean
   runner** (1.5). Record the `H2_ENTRY_STATS` split first. Payoff: most
   of the runtime's 23.8% of the sim thread, the D3D entry cost on the
   game thread, and hooks on hot functions become free, which unblocks
   every port. Two lanes: emitter and runtime.
2. **Flat RAM as the default and only model** (1.4). Remove the gate from
   generated code and delete the page-table inline path and its
   switches. Payoff: an access becomes one instruction; smaller image,
   faster guest code, shorter build; about ten codegen switches gone.
3. **Block clock and imprecise memory faults, together** (1.4), after a
   fault census. One re-baseline of lockstep, films and sim-diff.
   Payoff: roughly 35-40% less C, both tiers lockstep-compatible, ports
   stop moving slice boundaries.
4. **Build: tiering on by default, giant-handler splitting, fast tier
   without clang** (1.7). Payoff: first run under a minute; optimised
   image in 10-15 minutes.
5. **Typed guest layout crate, switch table, per-point verification**
   (2.6, 3.4). Payoff: one place for every offset; wrong addresses fail
   by name at install; the switch count becomes visible and bounded.
6. **Shadow-execution differential harness through the agent API** (3.4),
   with one shared "lifted original" test crate and a scripted disc gate.
   Payoff: ports are checked on states the game reaches; replaces the
   harness ARCHITECTURE.md promises and the per-crate copies.
7. **`FrameScene` and a native renderer that owns the frame** (4.3).
   First settle the default (D3D until native wins the A/B). Then
   extraction for camera, world and objects; one opaque pass on the
   render thread; batched shadows. Payoff: recovers the 10-20 ms
   render-wait and makes native faster than D3D instead of slower. Three
   lanes in sequence.
8. **Break up `boot`, `feature` and `cli`; local registration** (2.2-2.5).
   Payoff: `boot` under 40,000 lines, `feature` down to the hook engine,
   and the four merge hot spots gone, which matters more than any other
   change for running many lanes at once.
9. **Cull and delete** (2.6, 2.8): switches off for more than a cycle,
   `fastmem-proto`, `viewer`, stage 7a ports, superseded hand-lifted
   ports; rename `sim`; regenerate the ARCHITECTURE.md crate table; split
   design pages from lab notebooks.
10. **Sim pilot: animation sampling through the scaffolder, then wire in
    the HaloScript VM** (3.4). Payoff: the first subsystem owned in Rust
    with the full verification ladder, and a template for the rest.

Items 1 to 4 are the recompiler and can run in parallel with 5 and 6.
Items 7 and 10 depend on 5; 10 also depends on 1 and 6.

### Dead ends: stop these

- **Register-level hand ports** in `sim_native` and `sim_physics`.
- **More variants of the software memory path:** the page cache, the
  entry gate, 4 KiB translation, `fastmem-proto`. Flat RAM replaces all
  of them.
- **Per-draw native admission inside the D3D device** as the way to
  retire D3D.
- **The stage 7a orchestrator ports** and the pipelined-frame COW worker
  in its current form.
- **A Havok rewrite** at "deterministic but not exact", until there is a
  product decision that retail physics may change.
- **Feature parity for the Cranelift tier under the current contract.**
  Simplify the contract first; then parity is small.
- **Direct LLVM IR emission and IR-keyed object caching.** The second was
  already measured and rejected (build-perf.md); the first saves at most
  the 11% front end.
- **New experiment switches and `H2_*` behaviour variables.**
- **Docs as a changelog.** A 3,600-line design page that changes 169
  times cannot be trusted as a statement of the current design.

## What this review does not establish

- Nothing was built, run or measured. Every saving is an estimate.
- The split of runner transfers by cause is unknown until
  `H2_ENTRY_STATS` is recorded for a sim run and a gameplay run.
- Whether any shipped code path recovers from an access violation
  through SEH. Imprecise faults (1.4) depend on a census showing none.
- Whether the native renderer is the default today; the docs disagree.
- Live, co-op, bots, theater, the editor and the Halo 1 conversion were
  read for structure only, not reviewed for correctness.
