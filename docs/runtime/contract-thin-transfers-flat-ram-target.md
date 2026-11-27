# Contract change 1: the target design

Status: Design. Section 4: the flat memory model and the thin transfers. Part of [Contract change 1: thin transfers and flat RAM](contract-thin-transfers-flat-ram.md).

## 4. Target design

### 4.1 Memory

**One window, stable for the life of the `Runtime`.** `Memory`
(`ffi/memory.rs:52`) owns a guarded 4 GiB reservation, not `Guest`
(`crates/runtime/runtime/src/window.rs`). Replacing or restoring the
`Guest` re-aliases RAM into the same window. `base`
(`Runtime::flat_base`, the host address of guest address 0) never
changes, so generated code may keep it in a register for a whole handler
and across calls. Which pages the window holds is kept whatever RAM's
backing; RAM itself is aliased in only when its backing allows, which is
while flat RAM is selected.

**An access is one host instruction.** `H2_LD` and `H2_ST` become an
`asm goto` statement: one load or store at `base + (uint64_t)addr`, with
the site's `slow_` label as the indirect target, and a row in a fixup
table giving the instruction's address and the label's. Per target:

| Host | Load, 4 bytes | Notes |
|---|---|---|
| arm64 | `ldr w, [xbase, waddr, uxtw]` | 16 bytes is one `ldr q`; 10 bytes is `ldr x` then `ldrh`, two rows, one label |
| x86-64 | `movl (%rbase,%raddr), %e` with `addr` zero-extended | 16 bytes `movdqu`; 10 bytes `movq` then `movw` |

The address wraps at 32 bits by construction. A span that runs off the
end of the window lands in the guard and takes the fixup. An access that
crosses a 4 KiB boundary is now native when both pages are in the window.

The statement declares that it reads or writes memory, so the compiler
keeps guest order between accesses through different addresses. The
fence goes.

**The fixup table.** Each row is two 32-bit self-relative words: the
access instruction and its landing label. Rows are emitted from the same
`asm` string into a named section (`__DATA_CONST,__h2_fix` on Mach-O,
`h2_fix` on ELF, a C identifier so the linker defines `__start_h2_fix`
and `__stop_h2_fix`, and `.h2_fix$m` between `$a` and `$z` markers on
COFF). The module exports the section bounds. At
registration the runtime checks the rows are sorted by address and fails
with a named error if not; it never sorts or skips.

**The fault handler.** A new crate, `crates/recompiler/hostfault`, with no
dependency but `libc` (for `sigaction` and the saved machine context),
owns the process-wide handler: `SIGBUS` and `SIGSEGV` on
macOS, `SIGSEGV` on Linux, a vectored handler on Windows. It keeps a
fixed array of registered regions. A region is an address range and a
callback that runs in the handler. For a fault inside a window the
callback finds the faulting PC in the registered fixup tables by binary
search, sets the saved PC to the landing label and returns. A PC with no
row, or an address in no region, is forwarded to the previous handler
unchanged. The signal-chaining code in `protection.rs:102-181` moves here.

**The landing label is today's slow path.** It calls
`h2abi_v1_read_counted` or `h2abi_v1_write_counted` with the instruction's
PC, address and width, and leaves through `H2_FAULT_EVENT` on a non-zero
status. So everything the helper decides today it still decides: fault
order, whole-span validation, MMIO diagnostics, the access trap's log,
the event's backtrace. `H2_ACC_ONE()` stays where it is, before the
access. This is why imprecise faults are not needed for this change.

When imprecise faults are decided later, `H2_LD` becomes a plain
`memcpy` and the table goes. That is a one-macro change.

**What the window holds.** Host page protection is the union of the
permissions of the guest pages in that host page. A guest page is in the
window when it is mapped readable and its physical address is congruent
with its virtual address modulo the host page size. Everything else is a
guard: unmapped addresses, the NV2A range, pages an access trap has
armed, and non-congruent pages. A guarded page is **slow, not wrong**:
every access faults, takes the fixup and goes through the helper.

**Congruent physical allocation.** The three shifted windows go.
`Guest::fresh_physical` and `Guest::find_free_physical_for` take the
virtual address and return a physical address with
`physical % 16384 == virtual % 16384` (`window::CONGRUENCE`). The rule is
16 KiB on every host, so a snapshot's layout does not depend on where it
was made. Contiguous memory at `0x80000000 + physical` is congruent
already, and keeps `find_free_physical`. The kernel's virtual allocations
place each page with `find_free_physical_for`
(`crates/runtime/kernel/src/memory.rs`, `physical_pages`). The cost is up
to 12 KiB of skipped physical RAM per allocation whose residue differs.
When no congruent space is left, a reused gap of any residue is taken:
slow through the window, not wrong, and counted.

**Permissions at host-page grain.** On a 16 KiB host a read-only guest
page next to a writable one is writable through the window, and an
unmapped page next to a mapped one reads whatever its physical neighbour
holds. On a 4 KiB host the window is exact. A correct guest never
notices. The helper path stays exact everywhere. See risk R2.

**Dirty tracking is card marking.** After its store instruction a store
writes two bytes, in plain C:

```c
h2_cards_[a_ >> 12] = 0;          /* 1 MiB, one byte per guest page */
h2_cards_[(a_ >> 22) - 1024] = 0; /* 1,024 bytes below it, one per 4 MiB */
```

Zero means written, so arm64 needs no constant. There is no test, no
physical address, no fault, and the grain is 4 KiB on every host. A
`Guest::fold_cards` walks the 1,024 region bytes, then the 1,024 cards of
each written region, translates each written page to its physical page
and calls `WriteWatch::note` for it and for the page after it (a store
may cross). It runs before anything reads or changes watch state
(`take_dirty`, `has_dirty`, mute, subscribe, unsubscribe), before a
mapping change, and before a view is handed back. With no active watch it
is not called, and activating a watch clears the cards. The mirror, the
signal-driven watch, the promoted pages and `H2MemoryData.dirty` all go.

**No gate in generated code.** Topology only changes under a Rust guard.
What replaces the gate:

- `Memory` has a state word: idle, generated code running (with the
  running context's preempt word), or in Rust on that context's behalf.
  `call_native` sets "running" around the call. A Rust helper called from
  generated code, on the same thread, sets "in Rust" before it blocks on
  the `Guest` mutex and restores "running" when its guard drops.
- A guard taken on another thread while the state is "running" sets a new
  preempt bit, `PARK` (8), on the running context's word and waits for
  the state to become parked or "in Rust". Generated code reaches a safe
  point within one loop trip or one call. The runner sees `PARK`, marks
  itself parked, waits for the guard to drop and carries on. On the host,
  contenders sleep on a condition variable signalled by state transitions.
  A runner finding a pending guard waits on the guest mutex.
- A park is not a preemption. It does not call `preempt_decide`, does not
  end the slice and does not take a transfer from the budget. The loop
  exit and re-entry it causes are an ordinary `JUMP`/`RESUME`, with no
  guest-visible effect.
- The runtime uses the state word only once `Runtime::enable_parking` is
  called, which the flat model does. A paged image keeps the gate, and no
  guard sets `PARK` (`crates/runtime/runtime/src/ffi/memory.rs`, module
  docs).

A guard no longer drains anything or republishes pointers.

**The per-context block.** The context's third word (after the preempt
word and `H2Direct`) points to:

```c
typedef struct H2Mem {
    uint8_t *base;             /* window; fixed for the Runtime */
    uint8_t *cards;            /* cards[va >> 12]; regions at cards - 1024 */
    uint64_t accesses, budget; /* as H2FastAccess today */
    uint32_t *preempt;
} H2Mem;
```

A handler loads `base` and `cards` once at entry.
`h2abi_v1_fast_acquire`, `h2abi_v1_fast_release` and the lent view go.
`call_native` sets the budget before the call and settles the count after
it, with the same arithmetic as `fast_acquire` and `sync_accesses`
(`ffi/memory.rs:316`, `:336`) so a slice ends on the same access.

**The null window.** `ExecCtx::enable_fast_memory(false)`
(`exec.rs:162`) points that context's `base` at a 4 GiB reservation with
nothing mapped. Every access then takes the fixup and the helper. That is
the comparison mode the equivalence tests use today, with no codegen
switch and the same image.

**Bulk copies.** `h2_fast_movs` (`h2fastmem.h:226`) keeps its chunking
and its count. Before the `memmove` it probes the first byte of the
source (a load) and of the destination (a load and a store of the same
byte) with the same `asm goto` form; a fixup rejects the chunk and the
ordinary iteration runs. It marks the destination's card.

**Second-executor views and restores.** A view is a `Memory` with its own
window. `cow_refresh` re-aliases the new copy into it. A copy-on-write
restore from a file re-aliases too; on macOS a shared remap of a private
file mapping keeps the clean pages shared. On Linux and Windows two
mappings cannot share private copy-on-write pages: see open question Q1.
Which blocks a window may alias is `Ram::can_share` (`cow.rs`); a Guest
asks for aliasing when its runtime runs a flat module
(`Guest::require_aliasable_ram`), so a paged runtime's views and restores
keep the window's bookkeeping only.

### 4.2 Transfers

**The leave word.** The preempt word becomes the one test. It gains a
sticky bit, `IRQ_QUEUED` (16), that the runtime keeps set exactly while
the interrupt queue is non-empty (set where `interrupt.rs:104`, `:118`
and `:142` queue; cleared when the queue empties; not cleared by
`take_preempt`). Then "word is zero" means what "preempt word is zero and
interrupt count is zero" means today, and `h2_direct_quiet` is one load.
The transfer budget stays a separate decrement and test, because
`TransferLimit` must fire on the same transfer.

Loop headers and `h2_fast_movs` also read the word, with `word != 0`.
With `IRQ_QUEUED` set they would leave on every trip while an interrupt
for another thread (or a masked one) waits, which changes the transfer
count. So those polls mask it out (`word & ~IRQ_QUEUED`, one more
instruction), and the runtime keeps the bit only once a module that
does so asks (`Runtime::enable_leave_word`). A module with the entry
table (feature bit 8) masks its polls, and the runtime turns the bit on
when dispatch first has one.

**The entry table** (built in T2). The emitter numbers every `H2GenEntry`
row: row `r` has entry index `r + 1`, and 0 is no entry
(`entry_rows`, `crates/recompiler/emit/src/lib.rs`). A module has at most
65,535 rows; the image has about 18,900. Module feature bit 8,
`H2GEN_FEATURE_ENTRY_TABLE`, says the module has the table, and every
module `h2gen.h` builds has it, with direct calls or without. Registration
checks the module's `pcmap` against its rows, keeps the rows in `Dispatch`
(one such module per dispatch, `RegistrationError::SecondEntryTable`), and
refuses a module with direct calls but no table
(`DirectCallsWithoutEntryTable`): an image built before T2 must be
rebuilt.

Each `Runtime` owns `live` (`runtime::direct::Live`), an array of
handler pointers with one element per index. The context's `H2Direct`
block points to it (its first word, beside the module's `pcmap` and text
range), so a context with direct calls disabled reaches neither.
`live[i]` is:

- the row's generated handler, when the row is a function entry, no
  enabled override takes its address, and the address is lifted code
  (`Guest::guard_code`) with the execute bit;
- the host-call thunk for an entry an enabled override takes;
- 0 for an interior entry or anything else, which leaves for the runner
  as before.

`Runtime::edit_dispatch` brings the overridden elements in step before it
returns, under the dispatch write lock: it walks the enabled overrides,
points their elements at the thunk and restores those no longer
overridden. There is
no generation compare, no hash, no collision and no warm-up. A static
call is, in effect:

```c
if (d_ && d_->live && d_->depth < d_->cap && (f_ = live_[IDX])
    && *h2_pre_ == 0 && d_->budget) { /* spill, frame, call f_ */ }
```

It calls the linked symbol, a direct branch, when the element is it, and
the element (the thunk) through a pointer otherwise.

**No execute-bit read per call.** `Guest::map`, `unmap` and
`protect_range` already refuse to take Execute from a guarded code range
(`touches_code`, `check_code`), and a restore requires equal guards
(`state.rs`). The whole-mapping `Guest::protect` is not guarded, and the
kernel uses it to make a section writable for a moment. So the guest has
a code identity, `Guest::code_id`, that moves when a guard is added, when
a whole-mapping protect touches lifted code, and when a snapshot layout or
a view's source layout is adopted; a memory guard publishes it as it ends,
as it does the layout. The runner, about to invoke a handler
(`ExecCtx::direct_begin`), compares it with the identity `live` was
checked under (two loads) and, when it moved, reads every row's execute
bit under one guard and rewrites the table (`Runtime::check_live`). The
return's check goes too: a continuation is in the caller's own function.

**Indirect calls.** The module exports `pcmap`, a read-only array of
16-bit entry indices, one per byte of `[text_lo, text_hi)`, zero for
none, written as designated initialisers of the entries only. About
7 MiB for the image, in the module's read-only data. Registration reads
all of it once to check it is non-zero exactly at the rows. An indirect
call is a bounds test, `pcmap[pc - text_lo]`, then `live` at that index.
A target outside the range (a kernel import token, a continuation token)
calls the thunk. A call through a kernel import slot is an indirect call
of this kind (`H2_ICALL`), so a kernel import runs in place too.

**The host-call thunk.** `h2rt_thunk` (`crates/runtime/runtime/src/direct.rs`)
is an `extern "C"` function in the runtime with the handler signature,
which the `H2Direct` block carries (module feature bit 32,
`H2GEN_FEATURE_HOST_CALLS`; a direct-call module without it is refused,
`DirectCallsWithoutHostCalls`). It is one iteration of the runner for one
transfer, without the safe point: the caller has just seen the leave word
zero, so the safe point would have done nothing. It reads the transfer's
kind and continuation from the caller's top frame record, does the
continuation-token and kernel-token tests, `resolve_cached`, the execute
check, `execute_dispatched` and stop recovery, and returns the handler's
transfer. `Runner::run_inner` and the thunk share that code as one
function (`runner::step`, with `THUNK` set).

The generated caller treats the thunk like any callee. It flushes its
count, takes a transfer, calls, and continues inline only on a plain
return to its continuation with the leave word still zero; anything else
it hands up. An HLE function that blocks, calls back into the guest with
a continuation or raises a preemption therefore reaches the runner
exactly as it does now. An HLE function that just returns no longer
costs its callers their host frames.

**Frames.** `frames_len` merges with `depth`: increment at a call, back at
an inline return, left alone on a hand-up; the runner replays
`frames[0..depth]` and zeroes it. The record keeps its 16 bytes
(`kind, target, resume, slot`): the replay needs the kind and the target
to give the shadow stack, and so every event's backtrace, what the runner
would have recorded.

The thunk replays the frames not yet in the shadow stack as it starts, so
Rust code under it sees what the runner would have shown it. As it leaves
it makes the test its callers will make (`h2_direct_back`, from the top
frame down): if one of them continues inline it rewinds the shadow stack
and leaves the frames in C for a later hand-up to replay; if the result
is handed up past them all, the shadow stack is already what the runner
would have, and `depth` drops to where the replay started.
While it runs, the context's direct-call floor is its caller's depth: a
runner or generated handler nested under it (a callback, an override
running its original) records frames above the caller's and replays and
drops only those. The depth cap therefore bounds the whole C nesting,
thunks included. The thunk settles what the caller lent with the call
(the view's count, a lent count, a lent gate hold) and lends nothing back.
The transfers it and anything under it take are counted, as direct
transfers, by the runner around the caller.

**A lean runner.** `run_inner` gets a second const parameter, `DIAG`.
The lean instance has no entry stats, census, profile counters, coverage,
HLE census, and checks `cpu.valid_abi()` only on the run's first
transfer. `Runner` picks the instance once per run
from the switches that exist. Debug builds always take `DIAG`.

**Not in this change.** Returning the transfer in a register instead of
20 bytes through memory is slice T4, taken only if T2's profile still
shows it. After-hooks that allocate (`crates/hooking/hooks/src/install.rs`)
are the hook engine's, not the contract's.
