# In-place callouts from Rust bodies

Status: Design. Not built.
Code: this spec adds `ExecCtx::call_inline` to `crates/runtime/runtime/src/exec.rs` and uses it in `crates/rewrites/engine/src/sim_objects/mod.rs` (`drive`). Tests: `crates/runtime/runtime/tests/generated.rs`, `crates/rewrites/sim-bridge/src/sim_objects.rs`.
Switch: `impl.sim.native.inline-callouts` (proposed, default off; on with `impl.sim.native.objects` once its parity run is clean).

A Rust body that replaces a guest function often has to call guest code
that is still lifted. The object bodies do it by returning a `CALL`
transfer to the runner and being entered again at the call's return site
([sim-rewrite-objects.md](../engine/sim-rewrite-objects.md#local-dispatcher)).
`call_inline` makes the same call on the host stack, as a generated
function calls another, and falls back to the transfer whenever the
runner would have had something to do. It is lever 9 of
[sim-10x.md](sim-10x.md) and the primitive the sim tier's call-outs use
([sim-10x-tier.md](sim-10x-tier.md)). The rule that must hold: with the
switch on or off, the guest sees the same transfers in the same order
with the same counts, so the two arms are bit-identical in memory and in
the logical clock.

## How it works

### What a callout costs without it

A body entered through the host-call thunk runs in place
(`runtime::direct`, `h2rt_thunk`). When it returns
`TransferAbi::call(target, site)` the thunk's caller sees something other
than a plain return and hands it up, and so does every generated frame
above it. Then, per callout:

1. the runner routes the `CALL`: safe point, resolve, execute check;
2. the callee runs as an outermost handler and acquires its own memory
   view (`h2abi_v1_fast_acquire`);
3. its `RETURN` to `site` goes through the runner's resume lookup to the
   body's closure (`sim_bridge::sim_objects::install_table`);
4. the closure takes a memory guard (`ExecCtx::memory`), checks the float
   environment (`sim_objects::supported`), and `drive` finds the body by
   binary search (`Table::at`) and re-enters its `match` on `pc`;
5. every ancestor that lost its host frame is later resumed the same way.

The lifted function makes the same call with one table load, one frame
record and a C call.

### `call_inline`

```rust
pub enum Inline {
    /// The callee returned to `site`; the guest state is what a `RETURN`
    /// to `site` leaves. Continue in Rust.
    Returned,
    /// Give this transfer to the runner. The body returns it unchanged.
    Leave(TransferAbi),
}

impl ExecCtx {
    /// Call guest `target` in place. The caller has pushed `site`, holds
    /// no memory guard, and is registered to resume at `site`.
    pub fn call_inline(&mut self, cpu: &mut CpuAbi, target: u32, site: u32) -> Inline;
}
```

In order, mirroring a generated direct call (`H2_CALL_SLOT_`,
[contract §2.3](../runtime/contract-thin-transfers-flat-ram.md#23-a-call-between-generated-functions)):

1. **Quiet test.** The context has a direct-call block, its depth is
   under the cap, the leave word is zero and the transfer budget is not
   exhausted. If not: `Leave(TransferAbi::call(target, site))`, which is
   exactly what the body returns without this primitive.
2. **Resolve.** `live[index(target)]` through the module's `pcmap`; a
   target outside the text takes the thunk. An element of 0 (an interior
   entry, unlifted code) is `Leave(call)`.
3. **Account.** Take one transfer for the `CALL`, write the frame record
   (`kind = CALL`, `target`, `resume = site`) at `frames[depth]` and
   increment `depth`, as the generated caller does. The shadow stack and
   every event's backtrace then show the callee under the body.
4. **Invoke** the element with `eip = target`. The direct-call floor is
   the depth before step 3, so anything nested replays and drops only its
   own frames.
5. **Judge the result**, with the test `h2_direct_back` makes: a plain
   `RETURN` whose target is `site`, the leave word still zero, the budget
   not exhausted. Then take one transfer for the `RETURN`, drop the
   frame, and answer `Returned`.
6. **Anything else** is `Leave(result)`: a `STOP` from a blocking kernel
   call, a `CALL` or `JUMP` the callee handed up, a return to another
   address, a preemption, a fault. The frame stays for the runner's
   replay, as on any hand-up. The body returns the transfer; when the
   guest later returns to `site`, the runner enters the body through its
   resume registration, which is the path every callout takes today.

`call_inline` never runs a safe point. An interrupt, an expired slice or
a parked context is a non-zero leave word, and a non-zero word is a
`Leave`. So no snapshot, thread switch or interrupt delivery ever happens
with the body's Rust frame live below it, and bodies stay stateless
across guest calls in every case that reaches the runner.

### `drive`

`sim_objects::drive` takes the context instead of a `Guest`:

```
loop:
  guard = ctx.memory()                 // held across fused bodies
  next = body.run(cx(cpu, guard), pc)
  match next:
    Call{target, site}: push site
        if fused(target): pc = target; continue          // as built
        drop(guard)
        match ctx.call_inline(cpu, target, site):
            Returned if supported(cpu) && table.resumes(site): pc = site; continue
            Returned: return resume_original(site)       // float environment changed under us
            Leave(t): return t
    Ret / Tail: as built
```

The body's obligations are the callout contract it already keeps (retail
frame, all eight registers, stores before the call): at a `Leave` the
guest state is what the runner path would have seen. A guarded body
(another feature hooks its point, `Table::guard`) still goes through the
runtime, so hooks run around it.

The float-environment check after `Returned` exists because a callee may
change MXCSR or the x87 control word; the runner path makes the same
check when it re-enters the closure at `site`.

### Why both arms are bit-identical

- **Transfers.** Step 3 and step 5 take the two transfers the runner
  takes for the `CALL` and the `RETURN`. `TransferLimit` fires on the
  same transfer.
- **Accesses.** The same guest code runs on the same memory; the callee's
  count settles when its handler exits, in place or under the runner.
- **Safe points.** Without the primitive the runner has a safe point
  before the callee and before the resume. With it, both are skipped only
  when the leave word is zero and nothing is queued, which is when those
  safe points do nothing.
- **Block clock.** Under `Runtime::block_clock` the runner charges
  `HLE_COST` each time it invokes a Rust handler, so a body re-entered at
  a site is charged again. `drive` charges `HLE_COST` at each `Returned`
  to keep the clock equal. Under the access clock there is nothing to
  charge.

So `h2 sim diff` between the arms prints nothing with arena hashes on,
and `H2_LOCKSTEP` whole-RAM hashes match.

## Interfaces

| Caller | Use |
|---|---|
| `engine::sim_objects::drive` | every true callout of an object body |
| `runtime::tier` (`h2t_call_out`) | a tier function's call to non-tier code |
| `engine::sim_tick` (lane R1 of sim-10x.md) | the tick driver's call of each phase |
| Other overrides | may use it where they return `CALL` and are registered to resume at the site; `ExecCtx::call_guest` stays the way to call guest code with a Rust continuation |

`H2_ENTRY_STATS` counts a handler entered by `call_inline` as `inline`,
beside `thunk` and the runner entries, so a census shows callouts moving
from the runner column.

## Rules and limits

- The caller holds no `MemoryGuard` across the call. Under the paged
  model the callee needs the gate; under flat RAM the guard is a mutex
  the callee's own HLE calls would deadlock on.
- `site` must be a resume the installer registered for the body
  (`implement(..).resumes(sites)`). `call_inline` asserts it in debug
  builds; a `Leave` to an unregistered site would strand the guest.
- The depth cap bounds host stack use: body, callee, a thunk under it and
  a body under that all count.
- It adds no yield points and removes none.
- It does not make a body's own code faster. A body that is slow between
  callouts stays slow; that is the tier's job or a kernel rewrite's.

## Tests

In `crates/runtime/runtime/tests/generated.rs`, on the synthetic module
the direct-call tests use:

- `inline_call_returns_to_the_site`: a Rust override calls a generated
  leaf with `call_inline`; zero runner iterations after the first; the
  transfer count equals the same program with the switch off.
- `inline_call_leaves_when_the_word_is_set`: with a preemption requested
  before the call, the result is `Leave(CALL)` and the run's transfers
  and final CPU equal the off arm's.
- `inline_callee_that_blocks_resumes_at_the_site`: the callee makes a
  blocking kernel call; the override's resume registration runs it at
  `site` after the wake.
- `inline_callee_handing_up_keeps_frames`: a callee that hands up a
  `CALL` leaves the frame record for the runner's replay; the event
  backtrace equals the off arm's.
- `transfer_limit_fires_on_the_same_transfer`, both arms, the limit set
  inside the callee and at its return.
- `inline_under_the_block_clock_charges_like_the_runner`.

In `crates/rewrites/sim-bridge/src/sim_objects.rs`:
`a_body_resumes_at_its_sites_on_the_real_guest_stack` runs in both modes
with the same calls, registers and stack; a new case disables the body
during a callout (`set_enabled`) and checks the original finishes the
frame from `site`.

`crates/rewrites/engine/tests/sim_objects_differential.rs` does not
change: it compares bodies at their transfers and does not go through the
runtime.

Run proof, by the coordinator: `h2 sim diff` of
`impl.sim.native.objects` with the switch on against off, arena hashes
on, Lockout and Waterworks, 9,000 ticks; then `H2_ENTRY_STATS` on both
arms (runner entries per tick must fall by about the `inline` count) and
three timed medians each (sim-10x.md, M2).

## Evidence

| Fact | Label | Source |
|---|---|---|
| A thunk-run override's non-return result is handed up by every generated caller | Code | `crates/runtime/runtime/src/direct.rs` module doc; `H2_DIRECT_HANDUP` in `crates/recompiler/emit/c/h2gen.h` |
| `drive` returns `TransferAbi::call` for a target with no body | Code | `crates/rewrites/engine/src/sim_objects/mod.rs` |
| The body's closure takes a guard and checks the environment at every entry and site | Code | `crates/rewrites/sim-bridge/src/sim_objects.rs` (`install_table`) |
| A nested generated handler can be invoked from an override | Code | `runtime::handler::OriginalHandler::invoke`; test `override_calling_original_nests` |
| Callouts per tick with the object bodies on, and the saving | S | sim-10x.md, M2 |

## Not verified

- How many true callouts a tick makes with `impl.sim.native.objects` on.
  If M2 counts under 100 a tick, this lane is not worth building for the
  objects and waits for the tier.
- That no body relies on the runner's safe point between its callouts to
  let another guest thread run (none should: the lifted code has no safe
  point there when its direct call is quiet).
- The cost of dropping and retaking the memory guard per callout under
  the paged model; under flat RAM it is one uncontended mutex.

## Related

- [sim-10x.md](sim-10x.md): lever 9, lane K1, measurement M2.
- [sim-10x-tier.md](sim-10x-tier.md): the tier's call-outs.
- [contract-thin-transfers-flat-ram.md](../runtime/contract-thin-transfers-flat-ram.md): the direct call and the thunk this mirrors.
- [object-update.md](../engine/object-update.md): the bodies and their sites.
