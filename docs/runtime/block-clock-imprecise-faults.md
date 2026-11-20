# Block clock and imprecise memory faults

Status: Built. The block clock and imprecise memory faults are opt-in at build time; the census runs in every build.
Code: `crates/runtime/runtime/src/fault_census.rs` (`FaultCensus`), `crates/recompiler/emit/src/clock.rs` (the costs), `crates/recompiler/emit/c/h2gen.h` ("Block clock"), `crates/recompiler/opt/src/flags.rs` (`Observers`). Tests: `crates/runtime/runtime/src/fault_census.rs` (unit), `crates/runtime/runtime/tests/generated.rs`, `crates/recompiler/emit/tests/block_clock.rs`, `crates/recompiler/opt/tests/flags.rs`.
Switch: `H2_ACCESS_COUNT=block` (the block clock; the access clock by default) and `H2_PASSES=all,imprecise-faults` (imprecise memory faults; precise by default), listed in [switches.md](switches.md).

Item 3 of the [architecture review](../reviews/2026-11-20-architecture.md)
(sections 1.4 and 5) changes two rules of the recompiler contract
together: the scheduler's clock stops being the guest memory access count,
and a guest memory fault stops being a point where the handler leaves with
exact state. Both change where guest threads interleave, so lockstep
traces, films and sim-diff baselines are re-recorded once, for both. Each
is selected at build time and declared by a module feature bit, so an
image built before them keeps the old clock and precise faults, and a
runtime that does not know a bit refuses the image. The rule that must
hold: a fault exit may leave state inexact only where nothing goes on
from that state, and the fault census is how that is checked.

## How it works

### The fault census

`runtime::fault_census` counts, for the life of the process, every guest
memory fault and what became of the state it left
([`Runtime::fault_census`](../../crates/runtime/runtime/src/services.rs)):

1. A fault is noted where its event is recorded
   (`ExecCtx::emit_memory_fault`): from an accessor generated code called
   (`ffi.rs`, `status`), source `generated`, or from the runtime or a Rust
   handler (`ExecCtx::fault`), source `runtime`. Each PC is a site, with its
   count and its first fault's address, width, access and reason.
2. When a run returns a memory FAULT to its caller (`Runner::run_from`,
   `Runner::scoped`), the fault becomes the context's open fault and counts
   once as `ended`, however many nested runners hand it up.
3. When the runner next dispatches a transfer on that context
   (`ExecCtx::census_continue`, at the safe point), guest execution goes on
   from the faulting state: `continued`, and `continued_generated` when the
   fault came from generated code. `ExecCtx::discard_fault` and
   `ExecCtx::restart` drop an open fault without counting it; the second
   executor's `call` and `enter` discard, since they start from the host's
   own state.

Only `continued_generated` faults need a precise exit. The agent API
reports the census as `profile.faults` ([agent-api.md](agent-api.md#profile-where-a-frame-goes)),
with `precise_exits_needed` true when that count is not zero.

### The block clock

With the access clock (`H2_ACCESS_COUNT=1`, module feature 1) a handler
counts its loads and stores (`H2_ACC_ONE`) and the slice ends when the
count reaches the budget. The count depends on how a backend performs
accesses and calls, so two backends cannot share a lockstep session and a
native port moves every later slice boundary.

The block clock (`H2_ACCESS_COUNT=block`, module feature 64) counts a cost
the plan assigns instead (`emit::clock`):

- entering a handler with its function entry as the selector charges the
  entry block's guest instruction count (`H2_CLOCK(n)` in the selector
  switch); a loop whose header is the function entry charges it again on
  a resume there;
- each trip round a loop charges the header block's instruction count,
  after the header's poll (`H2_CLOCK(n)` at the `P_` label), so a trip that
  leaves at the poll charges nothing and the resumed trip charges once;
- an access charges nothing (`H2_ACC_ONE` is empty), and a REP bulk copy
  neither counts its accesses nor is limited by the budget;
- a Rust handler the runner invokes (an HLE override, a native port)
  charges `preempt::HLE_COST` (16) before it runs (`handler.rs`);
- helpers generated code calls that charge for themselves (atomics) charge
  as before: one per instruction on every backend;
- a handler cut into parts (`emit::c::Split`, "Handler parts" in h2gen.h)
  charges as the whole handler: only the function entry's selector charges
  the entry, a part entered at a cut-in (`block | H2_PART_CUT`) charges
  nothing until a header's `P_` label, and each part's exit charges its
  count, so a slice that ends there leaves at the next poll.

The costs depend only on the lifted IR's blocks, which no IR pass changes,
so any backend that polls the same headers charges the same amounts at
the same points. Everything else is accounting's plumbing, unchanged: the
`h2acc` local, the budget read on entry, the flush before calls and the
charge at every exit (h2gen.h, "Access accounting"). The kernel charges
the slice's work to logical time at `ACCESSES_PER_TICK`, in the clock's
units.

### Imprecise memory faults

With the pass spec `imprecise-faults` (`H2_PASSES=all,imprecise-faults`;
`all` alone leaves it off), loads and stores stop observing the flags in
`opt`'s dead-flag analysis (`flags::Observers::IMPRECISE_FAULTS`): a flag
write that is rewritten before anything else observes it dies even with an
access in between, and the formulas that fed it go with it. A faulting
access still leaves with the exact EIP and registers (no pass removes a
register write), and with EFLAGS and its defined mask as they happen to be.
Calls, returns, loop polls, `FaultIf` (divide errors), stops, intrinsics,
`Repeat` and atomics still observe every flag, so their exits stay exact.

`opt::Report::imprecise_faults` says a function was planned this way, and
the emitter then declares `H2GEN_FEATURE_IMPRECISE_FAULTS` (128) for the
module. The runtime accepts the bit; the census counts a continuation from
a fault while the bit is registered as `continued_imprecise` as well, and
logs the first one: that is the case the rule forbids.

It is safe while `continued_generated` stays 0 over the runs that matter,
because then no fault's EFLAGS is ever read. A future SEH dispatch or
fault recovery that goes on from a generated memory fault needs an image
without the bit.

## Interfaces

| Module feature | Bit | Selected by | Runtime |
|---|---|---|---|
| `H2GEN_FEATURE_ACCOUNTING` | 1 | `H2_ACCESS_COUNT` unset or `1` | the access clock, counted by the handler |
| `H2GEN_FEATURE_BLOCK_CLOCK` | 64 | `H2_ACCESS_COUNT=block` | needs bit 1; one clock per dispatch; `HLE_COST` per Rust handler |
| `H2GEN_FEATURE_IMPRECISE_FAULTS` | 128 | `H2_PASSES=…,imprecise-faults` | accepted; the census counts `continued_imprecise` |

Bits 2, 4, 8 and 32 are direct calls, the lent view, the entry table and
host calls; 16 is kept for flat RAM
([contract-thin-transfers-flat-ram.md](contract-thin-transfers-flat-ram.md)).
None of them changes the clock.

- `Runtime::fault_census()`: `snapshot()` gives `FaultCounts`; `reset()`
  starts a new window.
- `Runtime::module_features()`: the registered modules' features, kept in
  step by `Runtime::new` and `edit_dispatch`; `Runtime::block_clock()`
  tests bit 64.
- `Dispatch::module_features()`: the registered modules' features, OR-ed.

## Rules and limits

- An image without bit 128 keeps precise faults. A runtime older than bit
  128 refuses an image with it.
- An image without bit 64 keeps the access clock. A runtime older than bit
  64 refuses a block-clock image (`RegistrationError::UnknownFeatures`).
- Bit 64 without bit 1 is refused
  (`RegistrationError::BlockClockWithoutAccounting`), and so is a module
  whose clock differs from a module already registered in the same
  dispatch (`RegistrationError::MixedClock`).
- The Cranelift tier declares no features and counts nothing. To join a
  lockstep session with the C tier it charges the same costs at the same
  points (`emit::clock`); that is not built.
- Removing what only the access clock needs (`H2_ACC_ONE`, the flush before
  calls, the exit charge, the budget read on entry) waits until the access
  clock is deleted; while both are selectable they share the plumbing.
- The census sees a fault gone on from only through the runner. A host
  that puts other state on a faulted context and runs it again without
  `discard_fault` is counted as continued: an over-count, the safe
  direction. A snapshot load builds fresh contexts.

## Evidence

- **Code.** Nothing goes on from a generated memory fault today: the
  runner returns it to its caller (`runner.rs`); `kernel::Machine` stops
  with it and `retry_kernel_call` refuses faults (`crates/runtime/kernel/src/threads.rs`);
  the pipelined render turns a second-executor fault into a terminal event
  (`crates/hooking/hooks/src/pipeline_render.rs`, `settle`);
  `RtlRaiseException` is a kernel stop (`crates/runtime/kernel/STOPS.md`);
  stop recovery takes STOPs only (`crates/apps/boot/src/fallback.rs`).
- **Test.** `a_fault_a_run_ends_with_is_counted_once_and_not_as_continued`,
  `going_on_from_a_generated_fault_needs_a_precise_exit`,
  `a_discarded_or_restarted_fault_is_not_continued`,
  `a_rust_handlers_fault_is_counted_as_the_runtimes` (fault_census.rs).
- **Test.** `every_slice_ends_where_the_ir_rule_says` and
  `the_block_clock_does_not_move_with_memory_accesses` (block_clock.rs):
  the generated C gives up its slice where a model built from the IR alone
  says, and two loops of the same shape, one with memory operands and one
  without, interleave identically. `a_split_handler_charges_as_the_whole_one`:
  the same stops and totals with every block in its own part.
- **Test.** `imprecise_faults_change_only_a_memory_faults_flags`
  (opt/tests/flags.rs): every opt fixture, from random states with random
  preemption, ends the same way with the same memory and registers, and
  the same flags unless it ends at a memory fault; and more flag writes go
  than with the precise rule. `with_imprecise_faults_other_exits_stay_precise`
  keeps a divide's and an atomic's flags.

## Not verified

- The census over a campaign run and a multiplayer match: `profile.faults`
  with `continued_generated` 0 and `precise_exits_needed` false.
- Guest timing under the block clock: work moves logical time at
  `ACCESSES_PER_TICK` in block units, and `HLE_COST` is a harness choice.
- Lockstep traces, films and sim-diff baselines re-recorded under the block
  clock and imprecise faults.
- How much generated C and run time imprecise faults save on the game's
  code.

## Related

- [contract-thin-transfers-flat-ram.md](contract-thin-transfers-flat-ram.md): the other contract changes; its fixup table keeps faults precise until this page's change.
- `crates/recompiler/emit/RUNTIME-CONTRACT.md`, §1.5 "Optional access accounting" and §2 (module features).
- [agent-api.md](agent-api.md): `profile.faults`.
