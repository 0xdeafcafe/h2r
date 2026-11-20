# Contract change 1: thin transfers and flat RAM

Written 2026-11-20. Slices T1, F1, T2 and T3 are built (section 6), and
F2 except its first bullet: deleting the opt-in flat RAM waits for M0,
which has not been taken. The F2 counters are on the agent API, not yet in
the sim report. None has been measured yet. This is the spec for
decision 7 of [sim-rewrite.md](../engine/sim-rewrite.md) section 8: change
the recompiler contract, starting with two items of
[2026-11-20-architecture.md](../reviews/2026-11-20-architecture.md):
cheaper guest/host transfers, and flat RAM as the only memory model. The
block clock, imprecise faults and the other review items are not in scope
and nothing here depends on them.

**Code references** are `file:line` at `integ-1120` `2440ee1e6`, which is
master's code plus the flag cleanup and the codegen-mem merge `bbb2f45b8`.
Master `21ca66709` does not have that merge yet: `H2_EMIT_FLAT_RAM`,
`H2_FLAT_STATS` and the alias retirement are on `integ-1120` only. Symbol
names are given with every reference; trust the name if a line has moved.

Labels: **Code** is read from the source, **Measured** is a number already
recorded in a doc, **Estimate** is reasoning that a run must confirm.

## 1. Summary

- **The opt-in flat RAM is not a memory model.** It is a second probe in
  front of the paged path. A flat load still loads the page-table entry
  under the gate; a flat store under a write watch needs a second
  translation; and every site carries both paths. It cannot become "the
  only model" as built, and the code explains the 2x slowdown without a
  run (section 3.1).
- **Target for memory:** one host instruction per access at
  `base + address`, no gate and no page table in generated code. Exactness
  is kept by a fixup table: a host fault on an access jumps to that site's
  existing slow path, so faults, MMIO diagnostics and the access trap
  behave as today. Dirty tracking becomes two byte stores per guest store
  (card marking) and never a host fault.
- **Target for transfers:** a call between generated functions is one
  test of one word, one table load and a call. A call to an HLE function,
  a hook or a kernel import runs in place through a thunk and no longer
  unwinds the chain of callers. The runner's loop loses its diagnostics
  on the normal path.
- **Nothing a film can see changes.** The access count, where it is
  flushed, where a thread can yield and how transfers are counted stay
  bit-identical (section 5). No float code is touched. The tick stays
  30 Hz.
- **No new switches.** `H2_FLAT_RAM` is reused as the one transitional
  selector and is deleted in the last slice with twelve other flags.
- Eleven slices, in two tracks that share no macro: T (transfers) and F
  (flat RAM). Opus starts with **T1**, which is runtime-only and needs no
  image rebuild. Section 8 has the order against the `sim-tier` lane.

## 2. The current mechanism

### 2.1 A memory access

**Code.** A load is `H2_LD_` (`crates/recompiler/emit/c/h2gen.h:658`), a store
`H2_ST_` (`:699`). Both call `H2_SITE_TRY` (`crates/recompiler/emit/c/h2fastmem.h:509`),
which without flat RAM is `H2_HELD_TRY` (`:404`):

1. is the gate held by this handler (`h2g_.held`);
2. is the width one of six and does the span stay in one 4 KiB page;
3. load the page-table entry `pages[addr >> 12]`;
4. test the permission bit;
5. test the physical address is inside RAM;
6. for a store, test the watch flag, compare with the page last marked
   and maybe call `h2_mark_dirty` (`:377`);
7. compute `ram + physical + offset`.

Anything else is one call to `h2_access_cold` (`:552`), which takes the
gate and retries, or drops it and calls the Rust helper. The gate is a
word shared with every Rust memory guard (`Memory::lock`,
`crates/runtime/runtime/src/ffi/memory.rs:78`). A handler drops it before every
call that takes the context, on every way out, and after 32 loop headers
(`h2_gate_tick`, `h2fastmem.h:315`; RUNTIME-CONTRACT.md lines 54-80).

Every Rust guard pays too. Taking one drains the C dirty bitmap
(`ffi/memory.rs:125-134`); dropping one re-derives the RAM pointer, the
page-table pointer and the watch flag and republishes them
(`MemoryGuard::drop`, `:218-244`).

The count for the scheduler is separate: `H2_ACC_ONE()` before every
attempted access (`h2gen.h:294`, `:666`, `:707`), flushed at exits, before
direct calls and before atomics (RUNTIME-CONTRACT.md lines 365-416).

### 2.2 The opt-in flat RAM

**Code.** `H2_EMIT_FLAT_RAM=1` at build defines `H2_FLAT_RAM` in the C
(`crates/recompiler/emit/src/compile.rs:138`, `flat_image_flag` `:188`).
`H2_FLAT_RAM=1` at run makes each `Guest` build an `Arena`
(`crates/runtime/runtime/src/flat.rs:12`, `:89`; `Guest::flat_view`,
`crates/runtime/runtime/src/guest.rs:1065`):

- A guarded 4 GiB window with guest RAM aliased in at guest virtual
  addresses. On a 16 KiB host a 4 KiB guest page can sit at four offsets
  inside its host page, so there are four windows, one per offset
  (`flat.rs:97-106`, `map_ranges` `:184`). Bits 3 to 5 of each page-table
  entry say which window holds the page (`flat.rs:18-28`).
- While a write watch is active, stores go to a second alias set, the
  mirror: all of physical RAM, four planes on a 16 KiB host, indexed by
  physical address (`crates/runtime/runtime/src/flat/protection.rs:183`). Watched
  clean pages are read-only there. The first store takes `SIGBUS`
  (`SIGSEGV` on Linux), the handler opens the page with `mprotect` and
  records it (`State::fault`, `:60`). A page that faults in two watch
  epochs is promoted to the software dirty bit for good
  (`Mirror::publish`, `:247`; `hot`, `:191`).
- The access is `h2_flat_span` (`h2fastmem.h:464`): gate held, capability
  bit, **load the page-table entry**, permission test, size test; for a
  store under a watch a physical-range test, the mirror pointer, the
  promoted-page bitmap and a plane computation; otherwise the window bit
  or the window index. If it returns null the site runs the whole paged
  path after it (`H2_SITE_TRY`, `:509-515`).
- The emitter groups accesses off one register into a span checked once
  (`crates/recompiler/emit/src/c/span.rs`, `c.rs:549`; `H2_SPAN`, `h2gen.h:729`).
- Every store is bracketed by `H2_ALIAS_FENCE` (`h2gen.h:646`, `:710`).
- Every Rust guard also calls `publish_flat` when taken
  (`ffi/memory.rs:124`) and `flat_view`, which calls `Arena::rearm`, when
  dropped (`:226`, `guest.rs:1077`).
- A copy-on-write restore from a file and a second-executor refresh turn
  flat RAM off for that `Guest` (`Guest::restore_cow`,
  `crates/runtime/runtime/src/guest/state.rs:166-170`; `Guest::cow_refresh`,
  `guest/view.rs:95-101`). An installed access trap suspends it
  (`guest.rs:1067`).

### 2.3 A call between generated functions

**Code** (after T3). `H2_CALL_SLOT_` (`crates/recompiler/emit/c/h2gen.h`),
from `H2_DCALL` and `H2_ICALL` (`crates/recompiler/emit/src/c.rs`,
`Emitter::control`). In order:

1. load the context's `H2Direct` block; test it, its `live` pointer and
   depth against the cap; load `live[i]` (`h2_direct_static`, `i` the
   callee's entry index, a constant), or for an indirect call test the
   target against `[text_lo, text_hi)` and load `pcmap[pc - text_lo]`
   first, or take the host-call thunk outside it (`h2_direct_indirect`);
2. drop the gate and flush the access count (`H2_DCALL_GATE_`);
3. test the transfer budget and the preempt word, `IRQ_QUEUED` included
   (`h2_direct_quiet`, one load);
4. write a 16-byte frame record at `frames[depth]` and take a transfer;
5. spill dirty registers, set `eip`, lend the RAM view, call (the
   linked symbol when `live[i]` is it, else what `live[i]` holds);
6. on return (`h2_direct_back`): five tests on the returned `TransferAbi`
   (20 bytes, returned through memory), the quiet test again, the budget
   again;
7. reload the homed registers, re-read the access budget, go to the
   continuation.

`live` is the runtime's entry table (section 4.2), filled without any
runner dispatch: a call to an entry the runner never saw is direct.
Hooks and HLE functions at overridden entries have the host-call thunk in
their element, and kernel imports and other targets outside the module's
text get it from `h2_direct_indirect`: they run in place (section 4.2).
Interior entries and code that is not lifted executable code have a 0
element and go to the runner.

### 2.4 The runner

**Code.** `Runner::run_inner` (`runner.rs:58-280`) does this for every
transfer it routes: pending-fault check; `transfer.valid_abi()` and
`cpu.valid_abi()`; a kind and route match; the profile counter;
`take_preempt`; `interrupt::deliver`; `preempt_decide`; the diagnostic
frame list (`frames.unwind`, then `call` or `enter`); the scope test; the
transfer limit; `take_continuation`; the token test; `resolve_cached`
(`crates/runtime/runtime/src/exec.rs:170`); the kernel-token test;
`check_execute_cached` (`exec.rs:218`); the entry-stats and census
branches; `direct_begin` (which checks the entry table again when the
guest's lifted code changed); the HLE census; then
`execute_dispatched` (`crates/runtime/runtime/src/handler.rs:34`), which checks
coverage, calls `ffi::call_native` (`crates/runtime/runtime/src/ffi.rs:27`),
checks the pending fault and validates both ABIs again.

`call_native` clears the lent view before the call and settles the view's
access count after it. A generated handler entered this way then calls
`h2abi_v1_fast_acquire` (`ffi/memory.rs:336`) itself.

### 2.5 Why the runner is entered so often

**Measured** ([sim-attribution.md](../perf/sim-attribution.md), "Which
call patterns cause the runner's share"): the runner and scheduler are
8.85% of the sim thread, direct-call glue 3.1%, ABI 2.6%. Of runner
entries, 19% are indirect calls to functions no direct caller warmed,
13% resumes in functions that make indirect calls, 3% hooked points, and
65% "resume after a deeper unwind, a preemption or a refused direct
call".

**Code.** The 65% row is the first three multiplied. When any call in a
chain cannot be direct, the handler leaves through its epilogue. Its
direct caller sees something other than a plain return and hands it up
(`H2_DIRECT_HANDUP`, `h2gen.h:1185`), and so does every ancestor. Each
ancestor has lost its host frame, so each later return goes through the
runner and re-enters its handler at a resume selector. One HLE call at
depth *d* costs *d* + 2 runner iterations. Sound, DirectSound, input
sync, kernel file calls and the bots hook are all such calls, every tick.

## 3. Why the opt-in flat RAM is slower, and why it can diverge

The last measurement (README.md:112, **Measured**): Waterworks 2v2, 3,600
ticks, 10.11 ms/tick off and 21.67 on, state diverging at tick 6. That was
before the codegen-mem merge. It has not been re-measured.

### 3.1 Slower

All **Code**; the ranking is an **Estimate**.

| # | Cause | Where |
|---|---|---|
| P1 | A flat access does not remove the paged checks, it precedes them. A flat load is: gate held, capability bit, page-table load, permission, size, window bit. That is the same count as `H2_HELD_TRY` and the same page-table cache miss. Each site carries both paths plus the cold call, so the image grows. | `h2fastmem.h:464-494`, `:509-515` |
| P2 | The D3D write barrier watches all 64 MiB, so a watch is active for the whole run and every store takes the mirror branch: physical-range test, mirror pointer, plane, and a second address formed from the physical page. | `crates/apps/boot/src/d3d_glue.rs:1683-1688`; `h2fastmem.h:471-486` |
| P3 | Once any page is promoted, the mirror pointer published to C is null, and every store also loads a bit from the promoted-page bitmap. A store to a promoted page fails the flat probe and then runs the full paged store with dirty marking. Promoted pages are by construction the ones written most. | `flat.rs:240-244`; `h2fastmem.h:476-481`; `protection.rs:258-265` |
| P4 | Each first store to a watched clean page is a signal and an `mprotect`. Pages are re-protected whenever the watch epoch moves: every `take_dirty`, every mute or unmute, every subscription change. | `protection.rs:60-92`, `:275-319`; `crates/runtime/runtime/src/watch.rs:131`, `:151`, `:312` |
| P5 | Every Rust guard got heavier: `publish_flat` on take, `flat_view` (with `rearm`) on drop. HLE-heavy ticks take thousands. | `ffi/memory.rs:124`, `:226-241` |
| P6 | On a 16 KiB host any mapping or protection change replaces whole host ranges in all four windows with guard mappings and re-aliases them. The kernel's page-table entries for those ranges are gone, so later accesses take soft faults. | `Arena::refresh`, `flat.rs:124-158` |
| P7 | Three alias sets (window, mirror, canonical) each need their own kernel page-table entries. First touch of each host page through each alias is a soft fault. | `flat.rs:117-119`; `protection.rs:199-204` |
| P8 | The fence around every store is an optimisation barrier. | `h2gen.h:646`, `:710`, `:743` |

P4 alone can account for a doubling: 11.5 ms a tick is roughly 1,000 to
2,000 signal round trips. `H2_FLAT_STATS=1` prints the fault count and
settles it (M0, section 7).

### 3.2 Diverging

The access count is the same on both paths (`H2_ACC_ONE` is before the
probe; `H2_SPAN_LD` and `H2_SPAN_ST` count once per member, `h2gen.h:735`,
`:742`), so the schedule is the same. A divergence therefore means
generated code and Rust saw different bytes. Candidates, most likely
first:

| # | Hypothesis | Evidence | How to tell |
|---|---|---|---|
| D1 | Stale aliases after a copy-on-write restore. `Ram::become_private_of` replaces the canonical mapping; the window and mirror kept aliasing the old backing. Generated code then read and wrote RAM that Rust no longer saw. | Fixed by `bbb2f45b8` (`state.rs:163-170`). The only caller is the snapshot loader's `load_cow` (`crates/apps/boot/src/snapshot_glue.rs:673-677`); whether the measured run went through it is Q2. | After the fix the hashes match. Note what the fix does: it **turns flat RAM off** for that guest. A run that starts through `load_cow` is then not a flat run at all, and `H2_FLAT_STATS` prints zeros. |
| D2 | The source side of `Ram::cow_copy`. The live RAM is a private anonymous mapping with shared aliases; `mach_vm_remap(copy = TRUE)` of it makes the source copy-on-write. If the canonical mapping and its aliases then resolve their writes to different objects, they split. | `crates/runtime/runtime/src/cow.rs:109`, `mach::remap_copy` `:506`, `mach::alias` `:477`. No test covers a store through an alias after a copy of the source (`ffi/memory/flat_tests.rs` has three tests, none does this). | New test in F2: copy, write through the window, read through canonical, and the reverse. On Linux the copy is a full `memcpy` and cannot split. |
| D3 | A page mapped in a shifted window on a 16 KiB host whose host page holds a second guest page with a different physical neighbour. | `map_ranges`, `flat.rs:195-229`: "a split group can have candidates in different physical host pages". | First differing arena block from `h2 sim diff` maps to a page whose entry has bits 4-5 set. Does not exist on a 4 KiB host: run D-check on Linux x86-64. |
| D4 | Read-after-write across aliases reordered by the compiler. | The fence exists for this; span members share one pointer. Unlikely. | Divergence disappears at `H2_OPT=0`. |
| D5 | The run is not deterministic at all. | Not checked for this measurement. | Two paged runs with the same seed must match first. M0 does this before anything else. |

D2 and D3 both come from the design having three alias sets and four
windows. The target design has one window and no mirror.

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

**The fault handler.** A new crate, `crates/runtime/hostfault`, with no
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
  itself parked, waits for the guard to drop and carries on.
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

## 5. What must not change, and why it does not

| Invariant | Argument | Proof |
|---|---|---|
| I1. Guest results | No IR, lifter, x87 or SSE code is touched. A load or store moves the same bytes. | Existing differential suites, unchanged. |
| I2. The access count per handler activation, and where it is flushed | `H2_ACC_ONE`, `H2_ACC_FLUSH`, `h2_view_spend` and the poll comparison are not edited. Spans never changed the count. | `crates/recompiler/emit/tests/accounting.rs` and `preempt.rs`, unchanged and green. |
| I3. Where a thread can yield | The leave word is non-zero exactly when the old two-word test failed. Calls and returns test it at the same points. `PARK` never ends a slice. | New test `leave_word_matches_old_quiet`; the hash gate. |
| I4. The transfer count | Direct calls, tail jumps, inline returns and thunk calls each take one, as the runner does. A park takes none. | New test `thunk_takes_the_runners_transfers`. |
| I5. Guest-visible layout | Only physical addresses of non-contiguous allocations move. | Film goldens replay; the hash gate on a recorded film. |
| I6. Tick rate and `dt` | Not touched. | Review. |

**The hash gate**, used by every slice that changes the image or the
runner: the Waterworks match of the 2026-11-20 profile, 9,000 ticks, with
`H2_FEATURES=impl.sim.arena-hash`, must give a report whose
`state_hashes_v1` equals the baseline's at every tick
(`h2 sim diff`, [sim.md](sim.md) "Post-tick state hashes"), on macOS
arm64 and on Linux x86-64, and the two hosts must equal each other. Plus
one co-op campaign film replayed to its end on both.

## 6. Slices

Each slice is one branch off master and one merge. T and F slices touch
different macros and files and can alternate. Slices that change
`h2gen.h` or `h2fastmem.h` invalidate every object: batch them as section
7 says. The lane writes code and tests; the coordinator runs them.

### T1. Lean runner and the leave word (runtime only)

- `runner.rs`: `run_inner::<SCOPED, DIAG>`; move the per-transfer body
  into one `step` function that T3 will reuse.
- `preempt.rs`, `interrupt.rs`: `IRQ_QUEUED`, kept in step with the queue
  once `Runtime::enable_leave_word` is called (T2's entry table calls it;
  see 4.2 on loop polls).
- Tests: `leave_word_matches_old_quiet` (property test over queue, mask
  and raise sequences: word non-zero iff old test fails);
  `lean_and_diag_runners_agree` (same transfers, events and final CPU on
  the runtime's existing runner fixtures).
- Exit: runtime tests green; hash gate on the existing image;
  `Runner::run_from` self samples per tick down (M1).

### T2. Entry table, indirect map, no per-call execute read

- `crates/recompiler/emit/src/lib.rs` (`entry_rows`, `function_indices`,
  `registry`): dense indices, `pcmap`, module feature bit 8 on every
  module. A module with bit 8 masks `IRQ_QUEUED` out of `H2_POLLED` and
  `h2_fast_movs` and compares the whole word in `h2_direct_quiet`; the
  runtime turns `Runtime::enable_leave_word` on when dispatch first has
  such a module, before the table can hold anything.
- `h2gen.h`: `H2_CALL_SLOT_` and `H2_JUMP_SLOT_` on `live`
  (`h2_direct_static`, `h2_direct_indirect`); `H2Direct` loses the
  generation and interrupt-count pointers and the 196 KiB slot table
  (`H2DirectSlot`, `h2_direct_slot`, `h2_direct_lookup`, `h2_direct_exec`
  are gone), `frames_len` merges with `depth`. `c.rs` passes indices.
- `direct.rs`: `Live` per `Runtime`, kept by `edit_dispatch` and checked
  against `Guest::code_id` at `direct_begin`; `note_direct` and the slot
  table are gone. `H2_ENTRY_STATS` splits runner calls and jumps by what
  `live` held: `live`, `null`, `none`.
- Tests: `override_stops_direct_calls_at_once`,
  `indirect_call_to_cold_entry_is_direct`,
  `entry_outside_code_guard_goes_to_runner`,
  `protecting_lifted_code_empties_the_entry_table`,
  `a_loop_ignores_a_queued_interrupt_for_another_thread` and the rest of
  the direct-call suite (`crates/recompiler/emit/tests/direct.rs`, now on
  guarded code); the registry's numbering and map
  (`registry_numbers_rows_and_maps_each_entry_byte`); registration
  (`an_entry_table_maps_each_row_and_nothing_else`,
  `direct_calls_need_an_entry_table`, `one_entry_table_per_dispatch`,
  `an_entry_table_turns_the_leave_word_on`,
  `crates/runtime/runtime/tests/generated.rs`); `Live` itself
  (`runtime::direct` unit tests).
- Exit: hash gate; runner entries per tick from `H2_ENTRY_STATS=1` down by
  the indirect-entry rows (M2); ms/tick not worse.

### T3. Host calls in place

- `h2rt_thunk` in `crates/runtime/runtime/src/direct.rs`, built on T1's `step`.
  `live[i]` holds it for overridden entries; `H2_ICALL` and `H2_IJUMP`
  call it for targets outside the module's text, and calls through a
  kernel import slot are emitted as `H2_ICALL` (`c.rs`,
  `Emitter::control`). `H2Direct` carries it (module feature bit 32;
  `DirectCallsWithoutHostCalls` refuses a direct-call module without it).
  `H2_ENTRY_STATS` counts generated handlers the thunk invokes as
  `thunk`, beside the runner entries.
- Tests: `thunk_takes_the_runners_transfers`;
  `hle_return_keeps_callers_inline` (a three-deep chain whose leaf is a
  Rust handler, a call through a register and one through a kernel import
  slot to a Rust handler outside the text: zero runner iterations after
  the first);
  `blocking_hle_reaches_the_runner`; `hle_raising_slice_yields_at_same_point`;
  `override_calling_original_nests` (all in
  `crates/recompiler/emit/tests/direct.rs`); `direct_calls_need_host_calls`
  (`crates/runtime/runtime/tests/generated.rs`); `Live` and the shadow
  stack's mark and rewind (`runtime::direct`, `runtime::frames` unit
  tests).
- Exit: hash gate including the co-op film; runner entries per tick under
  a quarter of the baseline; runner and scheduler under 3% of the thread
  (M2).

### T4. Transfer in a register (only if measured)

Take it only if T3's profile shows more than 1% of the thread in
`TransferAbi` traffic. Native handlers return a packed `uint64_t`;
`call_native` and the thunk convert. ABI v3. Exit: hash gate; the 1% is
gone.

### F1. `hostfault` and the fixup spike

- New `crates/runtime/hostfault`: handler install and chaining, region registry,
  fixup-table registration and lookup.
- A C fixture in `crates/recompiler/emit/tests/c/` with one `asm goto` load and store
  per width, built for every target `Arch` knows
  (`crates/recompiler/emit/src/compile.rs:202`). `H2_FIX_ABS=1` (a define of the
  fixture only) emits 64-bit absolute rows instead, and `hostfault`
  takes either (`Format::Relative32`, `Format::Absolute64`);
  `fixture_builds_for_every_target` prints which one each target took.
- Tests: `fixup_lands_on_the_slow_label` per width;
  `unowned_fault_reaches_the_previous_handler`; `table_must_be_sorted`.
- Exit: the fixture builds for macOS arm64 and x86-64, Linux x86-64,
  tvOS, iOS and Windows, and runs on the first three. If a target cannot
  express self-relative rows across sections, the slice says which
  encoding it used there. **This slice decides whether the design
  stands; do it before F2.**

### F2. One stable window, congruent allocation, cards, parking

Runtime only. Old images keep running on the paged path.

- Delete the opt-in flat RAM: `flat.rs` windows 1-3, `flat/protection.rs`,
  `H2_FLAT_WRITE_WATCH`, `H2_FLAT_STATS`, and in C `H2_FLAT_RAM`'s
  `h2_flat_span`, `h2_begin_span`, `H2_SPAN*`, `H2FlatMemoryData`; delete
  `crates/recompiler/emit/src/c/span.rs` and `H2_EMIT_FLAT_RAM`,
  `H2_FLAT_STATS_OWNER`. M0 must have been taken first.
- `Memory` owns the window; `Guest` map, unmap, protect, restore and trap
  arming update it. Congruent `fresh_physical` and `find_free_physical`.
- Cards and `fold_cards`; `PARK` and the `Memory` state word.
- Counters, always on: fixups taken, pages outside the window, bytes
  skipped for congruence. Add them to the sim report and to the agent
  API's `profile` method, and document them in
  [agent-api.md](agent-api.md). (An API expansion under AGENTS.md: later
  lanes need the fixup count to see a slow page.)
- Tests: `alias_tracks_canonical_after_cow_copy` (both directions, D2);
  `window_survives_guest_replacement`; `non_congruent_page_is_guarded`;
  `cards_fold_to_exact_pages`; `fold_before_mute_and_drain`;
  `park_does_not_end_a_slice`; `guard_on_helper_thread_does_not_deadlock`.
- Exit: runtime and boot tests green; hash gate on the existing image;
  skipped bytes under 1 MiB and pages outside the window at zero on
  Waterworks and one campaign level (M3).

### F3. Emit the flat access

- `h2gen.h`, `h2fastmem.h`: `H2_LD_`, `H2_ST_`, `h2_fast_movs` as section
  4.1; `H2Mem`; module feature bit 16. Selected by the existing
  `H2_FLAT_RAM=1` at build; the runtime takes the model from the module's
  feature bit, so there is no run-time variable.
- `crates/recompiler/emit/src/host.rs`: the test host's memory is a window of its
  own with its pages mapped in place.
- Tests: every existing inline-against-helper test in
  `crates/recompiler/emit/tests/perf.rs` rerun as window against null window;
  `page_crossing_access_is_native`; `access_past_4gib_takes_the_fixup`;
  `armed_trap_page_logs_every_access`; `mmio_access_faults_as_before`.
- Exit: hash gate; fixups per tick zero after the first second; ms/tick
  at or under the paged image's; image `__text` smaller (M4).

### F4. Watch consumers on cards

- `d3d_glue.rs` barrier and `native_world_glue.rs` drain through
  `fold_cards`. Remove the C dirty bitmap from the guard path.
- Tests: `barrier_sees_every_store_between_draws`; the existing
  `crates/apps/boot/tests/write_barrier.rs`.
- Exit: gameplay frame time on the lab level not worse than paged with
  rendering on (M5); no signal in a steady-state run.

### F5. Views and restores

- `cow_refresh`, `restore_cow` and `cow_snapshot` re-alias into the
  window. Q1 decides the Linux and Windows restore.
- Tests: `view_refresh_realiases`; `restore_cow_keeps_window_coherent`;
  the pipelined-render hash parity tests.
- Exit: pipelined frame time not worse (M5); resident memory per
  dedicated host reported for Q1.

### F6. Default on macOS and Linux x86-64

- `H2_FLAT_RAM` defaults to on for those targets.
- Exit: every gate above on the default build; the parity suite; a
  campaign level and a multiplayer match played by the user.

### F7. Remaining targets

- `hostfault` and the window on tvOS and iOS (same Mach calls; confirm
  the address-space entitlement), Windows (`VirtualAlloc2` placeholders,
  `MapViewOfFile3`, a vectored handler).
- Exit: boot and a level on each device, by the user.

### F8. Delete the paged model

- Remove `H2_HELD_TRY`, `H2_PAGED_TRY`, `H2Gate`, `h2_access_cold`'s gate
  half, the gate word and `GATE_*`, `H2MemoryData`, `fast_acquire` and
  `fast_release`, view lending, `crates/recompiler/fastmem-proto`, the `fastmem-4k`
  branch's doc, and the flags `H2_FASTMEM`, `H2_FLAT_RAM`,
  `H2_RAM_PAGE_CACHE`, `H2_ENTRY_GATE`, `H2_DIRTY_INLINE`,
  `H2_DIRECT_GATE`, `H2_DIRECT_COUNT`, `H2_DIRECT_VIEW`, `H2_VIEW_CHARGE`,
  `H2_EXIT_CALL`, `H2_COLD_STATS`, `H2_DIRECT_CALLS`. Rewrite
  RUNTIME-CONTRACT.md sections 1.1 and 4.
- `crates/recompiler/clif` keeps calling the checked helpers; note it in
  [cranelift-tier.md](../lifter/cranelift-tier.md) as its next step.
- Exit: workspace tests; hash gate; a cold image build timed.

## 7. Measurements for the coordinator

All timing on the quiet laptop through `h2 lab` ([lab.md](lab.md)), one
optimised CLI with `--features perf`, `H2_OPT=2` at build **and** run.
`arm` is the function in [sim-attribution.md](../perf/sim-attribution.md)
"A. Waterworks, both rosters"; `$match` is that section's
`waterworks.toml`. Use the local roster unless stated.

**Image builds.** Four full builds in total, no more: (1) the existing
paged image, which is also the M0 flat image if built with
`H2_EMIT_FLAT_RAM=1`; (2) T2 and T3 together; (3) F3 on top; (4) F8.
Correctness gates may use `H2_OPT=1` images.

**M0. Re-measure the opt-in flat RAM (before F2 deletes it).** On
`integ-1120` or any master that has `bbb2f45b8`:

```bash
export H2_EMIT_FLAT_RAM=1          # one image for both arms
h2 build "$H2_DISC" --out "$H2_IMAGE"   # BOOT.md, "h2 build"
arm local paged-a  H2_FLAT_RAM=0 H2_FEATURES=impl.sim.arena-hash
arm local paged-b  H2_FLAT_RAM=0 H2_FEATURES=impl.sim.arena-hash
arm local flat     H2_FLAT_RAM=1 H2_FLAT_STATS=1 H2_FEATURES=impl.sim.arena-hash
h2 sim diff "$out/ww-local-paged-a.json" "$out/ww-local-paged-b.json"   # D5: must match
h2 sim diff "$out/ww-local-paged-a.json" "$out/ww-local-flat.json"
grep 'flat RAM:' "$out/ww-local-flat.log"
for pass in 1 2 3; do arm local t-paged$pass H2_FLAT_RAM=0; arm local t-flat$pass H2_FLAT_RAM=1; done
```

Record in this doc: the two medians; the `faults`, `re-protects` and
`software-pages` line; the first differing tick and component if any;
whether the match booted through `restore_cow` (if `faults=0` and
`pages-armed=0`, flat RAM was off and the timing is P1 alone). Repeat the
`flat` arm on Linux x86-64 for D3. M0 informs section 3; it gates
nothing.

**M1. Baseline for transfers.** On master's paged image:

```bash
arm local dispatch H2_DISPATCH_STATS=1
arm local entries  H2_ENTRY_STATS=1
arm local profile  H2_PERF_FROM=level H2_PROFILE_HZ=500 H2_PROFILE_CPU=1 H2_PROFILE="$out/prof-base"
arm local hash     H2_FEATURES=impl.sim.arena-hash      # the hash gate's baseline
```

Record `dispatch_per_tick`, the entry census by cause, and the profile's
`Runner::run_from`, `handler::execute`, resolve and direct-glue samples.
Repeat on Linux x86-64 for the hash baseline. Record one co-op campaign
film on master for the film gate.

**M2. After T1, after T2+T3.** The same four arms on the slice's branch.
Then `h2 sim diff` against M1's `hash` report, on both hosts. Three timed
passes each side, medians.

**M3. After F2.** The `hash` arm; read the three new counters from the
report.

**M4. After F3.** The four arms with the flat image, three timed passes
against the T3 image, and:

```bash
size -m "$H2_IMAGE/libh2game.dylib" | grep -E '__text|__h2_fix'
```

**M5. Frames.** `h2 lab bench --host laptop --branch <slice> --level
03a_oldmombasa --scale 4` as in lab.md "`--serve` and `bench`", against
the same command on master, serial and pipelined.

## 8. Order against the `sim-tier` lane

The sim tier compiles the tick's closure under a looser contract and
keeps this image as its oracle and as the tier for everything else:
render front-end, UI, sound, HLE glue. So this work is needed whatever
the experiment's gate says, and its gates always run with the sim tier
off.

**Files.** This lane owns `crates/recompiler/emit/c/h2gen.h`, `h2fastmem.h`,
`crates/recompiler/emit/src/c.rs`, `c/span.rs`, `compile.rs`' flag list,
`crates/runtime/runtime/src/{flat*,direct,runner,ffi*,cow,watch,preempt}.rs`,
`guest.rs`' fast-page and allocation code, `crates/runtime/hostfault` and
RUNTIME-CONTRACT.md. The sim tier adds new files only (its own emit
module, header and runtime module) and one registration call. A change it
needs in a file above goes through a slice here.

**What the sim tier takes from here, and when.**

| It needs | Until then | From |
|---|---|---|
| A base pointer for unchecked `base + offset` | the existing `Guest::flat_view` window with `H2_FLAT_RAM=1`, read under a guard at its thunk; restricted to snapshots that did not come through `restore_cow` | F2: `Runtime::flat_base()`, fixed for the runtime's life |
| A host-fault hook for its rollback | none; a wild access crashes the experiment | F1: a `hostfault` region with its own callback |
| A cheap tier boundary | `register_override`, through the runner | T2 and T3: its boundary thunk sits in `live[i]` and is called in place |
| Dirty pages for its tick-start copy | its own `memcpy` of the arena | F2: cards, if it wants them |

**Order.**

1. Now, in parallel with the experiment: M0 and M1 (coordinator), T1 and
   F1 (this lane). Neither touches a file the sim tier reads.
2. F2 next, because it deletes the window the experiment is borrowing.
   Land it when the sim tier has moved to `flat_base()`; the two lanes
   agree the day. If the experiment is mid-measurement, F2 waits and T2
   goes first.
3. T2 and T3, one image build. The sim tier rebases its boundary onto
   `live`.
4. F3 onward. By then the experiment's one-week gate is known. If it
   passed, measure F3 and later on a rendering run as well as the sim
   run, since the tick is no longer where this image spends its time.

**Shared machine time.** Both lanes need the hour-long image build. The
coordinator builds once per step above and both lanes measure on it.

## 9. Risks

- **R1. The fixup rows cannot be emitted on some target.** F1 exists to
  find out in a day. The fallback inside the design is 64-bit absolute
  rows on that target, at a load-time cost.
- **R2. Host-page permissions on 16 KiB hosts.** A guest that reads an
  unmapped page beside a mapped one continues on arm64 and faults on
  x86-64. That would split a cross-architecture film. It requires a wild
  access that retail would have crashed on. Check: over the M1 runs and
  the co-op film, the event log must hold no `MemoryFault`; and the
  helper path stays exact, so the null window on arm64 is the audit.
- **R3. A slow page nobody notices.** A non-congruent or trap-armed page
  costs microseconds per access. The fixup counter is always on, in the
  sim report and the agent API, and F3's gate is zero per tick.
- **R4. Debuggers.** lldb stops on every `EXC_BAD_ACCESS`. With faults
  only on wild accesses this is rare; [fastmem-proto.md](../perf/notes/fastmem-proto.md)
  "lldb" has the `process handle` line.
- **R5. `asm` accesses hide loads from the optimiser.** Clang can no
  longer fold a load into an x86 arithmetic instruction or remove a
  repeated load. It could not before either (a cold call sat between
  them). M4's code size and time are the check.
- **R6. Physical RAM lost to congruence.** F2's gate is under 1 MiB. If a
  level exceeds it, the allocator reuses the skipped pages for
  allocations of matching residue before bumping.
- **R7. Old snapshots.** Their non-congruent pages run slow on arm64.
  The fixup counter shows it; re-saving fixes it. No silent fallback.

## 10. Open questions

- **Q1. Shared clean pages on Linux and Windows hosts.** A dedicated host
  restores from a private file mapping so guests share clean pages. Two
  mappings cannot share private copy-on-write pages on those systems, so
  under one window the restore must copy the image into the guest's own
  backing: up to 64 MiB resident per host instead of only the written
  pages. macOS keeps sharing. F5 reports the resident-memory difference.
  The user decides whether that is acceptable for host density or whether
  Linux hosts need a different restore.
- **Q2. Does `h2 sim run` always boot through `restore_cow`?** If so,
  every flat number so far measured the paged path with a probe in front.
  M0 answers it.
- **Q3. Does anything depend on the watch being exact to the page?**
  Cards report the page after each written page too. The D3D caches
  re-validate, so it is safe; M5 shows whether it costs.
- **Q4. iOS and tvOS address space.** A 4 GiB reservation per `Memory`
  may need the extended-virtual-addressing entitlement. F7 confirms on a
  device.
- **Q5. Films recorded before F2.** They carry no physical addresses, and
  I5 argues they replay. If a golden film diverges at F2, the congruent
  allocator is the cause and the film format needs its layout recorded.
