# Architecture review: the frame-time gap in gameplay

Review of 2026-11-16. Frozen: later work is in the design pages and the code.

Written 2026-11-16 against master 183781e. Old Mombasa gameplay runs at
about 18 fps (55 ms a frame); the budget is 33.3 ms, and "never below
30" needs a few ms of margin under that. This review looks for
structural changes worth tens of percent, not tuning. Nothing here was
built or run: the evidence is the three sampled profiles, the code, and
the generated C and dylib of the image the Old Mombasa profile ran
(`-O2`, FASTMEM, x87-native, SSE-native, direct calls, accounting).

The short version: the frame is not lost in one place. It is lost in
four per-event costs that are each several times what they need to be:
a guest memory access, a guest call, a floating-point operation and a
draw. Each has a specific structural cause, listed below. Fixing the
five items in the ranking takes the game thread from about 48 ms (after
the two in-flight fixes) to about 28-31 ms. That reaches the line, with
little margin. The margin has to come from the second-stage work in
"After the top five".

## Evidence and method

- `oldmombasa-183781e` (26,092 samples, 100 Hz, 323 s, the main target),
  `heretic-e9be08f` (2,367 samples) and `menu-pre-merge` (7,099
  samples). All three sample the `main` thread only: it runs every guest
  thread (`kernel::Machine::run`), all D3D HLE and the audio pump call.
  The `d3d-render` thread is not in any of them.
- The folded stacks were re-partitioned with short scripts into the
  disjoint ledger below, by the first non-guest frame under the deepest
  `guest 0x...` frame, or by the handler for stacks with no guest frame.
  This differs from the profiler's own categories, which charge
  `h2_held_read` to guest and split the rest by crate.
- 1% of the Old Mombasa frame is 0.55 ms. Sampling error on a 1% line
  is about +/-0.1% absolute there; the Heretic profile is ten times
  smaller, so read only its large rows.
- The profile came from `h2 run` (`boot::run` is on the stack), so it
  contains the kernel-call trace writer, which `h2 play` does not have.

## Where the frame goes

Share of all samples on the game thread. "ms" is the Old Mombasa share
of a 55 ms frame.

| cost | Old Mombasa | ms | Heretic | menu |
|---|---:|---:|---:|---:|
| Guest code, own time | 26.8% | 14.7 | 25.0% | 25.4% |
| D3D entries: resolve, state, textures, handoff | 13.6% | 7.5 | 32.6% | 19.7% |
| Memory accessors called from guest code (`h2_held_read`/`write`, slow helpers) | 11.8% | 6.5 | 12.2% | 11.9% |
| D3D visibility-result wait (in-flight fix) | 8.3% | 4.6 | 0.0% | 0.0% |
| Runner and dispatch (`run_inner`, `resolve_cached`, `handler::execute`, `check_execute`, `interrupt::deliver`, the boot trampoline) | 7.7% | 4.2 | 7.4% | 13.4% |
| D3D vertex stream copies (in-flight fix) | 6.2% | 3.4 | 1.1% | 0.3% |
| SSE helpers (`h2sn_*`, `h2s_*`, `h2i_sse_*`) | 6.1% | 3.4 | 5.2% | 4.1% |
| Per-handler glue (`fast_acquire`, `fast_release`, `charge`, `budget`, `h2_direct_enter`) | 3.9% | 2.2 | 4.1% | 3.5% |
| x87 transcendentals (`yl2x`, `x2m1`, sin/cos on `Wf<2>`) | 3.8% | 2.1 | 2.8% | 3.6% |
| Fallback: unknown-switch stop plus the interpreter | 3.3% | 1.8 | 3.8% | 0.1% |
| Boot kernel-call trace file writes (`h2 run` only) | 2.8% | 1.5 | 1.8% | 7.5% |
| x87, other helpers | 1.9% | 1.0 | 0.9% | 1.9% |
| D3D write-barrier shadows | 1.8% | 1.0 | 1.8% | 2.7% |
| Kernel, DirectSound, XAPI | 1.7% | 0.9 | 1.0% | 5.7% |

The guest-side rows are nearly the same in all three scenes: own code
25-27%, accessors 12%, glue 3.5-4%, SSE 4-6%, x87 transcendentals 3-4%.
They are properties of the generated code and the runtime contract, not
of a scene, so fixing them moves every frame.

With the two in-flight fixes treated as done (the visibility wait, and
about 5% of stream copies), the game thread is at about 47.7 ms. The gap
to 33.3 ms is 14.4 ms. To hold 30 fps with margin, plan for 28 ms: about
20 ms to remove.

### The questions, with numbers

**How much of "runtime" is dispatch, memory and x87?** The profiler's
runtime category is 27.0% of Old Mombasa. By the ledger: runner and
dispatch 7.7%, per-handler glue 3.9%, x87 5.6% (3.8% of it
transcendentals), and the Rust accessor paths (`read_bytes`,
`checked_start`, `write_bytes`) about 5%, almost all of it called from
D3D HLE, not from generated code. Generated code's own accesses are in
`h2_held_read`/`h2_held_write` (11.8%), which the profiler files under
guest.

**Is guest own time integer work?** No. 15.3% of samples carry a guest
instruction address; classifying each by what its generated block does:
9.5% are instructions whose only real work is a memory access, 2.0%
touch SSE state, 1.5% x87, and 0.6% are pure integer or flag work. The
other 11.4% have no instruction address: handler prologues and
epilogues, register spills and reloads around calls, and the direct-call
checks. So own time is mostly the inline half of memory accesses and
call overhead again.

**SSE helper time that could be NEON:** 6.1% (3.4 ms). 4.0% is the
guarded native path itself (`h2sn_lane` 1.9%, `h2sn_arith` 1.3%,
`h2sn_host_mode` 0.8%), not the soft-float fallback.

**What is left in fallback:** two functions, `0x336f0` and `0x33980`,
both stopping at a switch whose table the analyzer read short. See
change 1.

## Structural findings

### 1. A guest load or store is a function call that calls memcpy

This is the state at the review; `h2_held_read`/`h2_held_write` have since
been replaced by the always-inline `H2_HELD_TRY` (ranked change 2 below).

`H2_LD`/`H2_ST` (`crates/recompiler/emit/c/h2gen.h`) call `h2_held_read`/
`h2_held_write` (`h2fastmem.h`). They are `static inline`, but clang
does not inline them: the dylib has exactly one out-of-line copy per
shard (1,000 of each). Disassembly of one copy shows the width test, the
page-crossing test, the gate check, a page-table load, a permission
test, a physical-range test, and then `bl memcpy` with a run-time width.
The caller stages the value through a 16-byte stack buffer on each side.
`platform_memmove` under these two helpers is 4.8% of the frame by
itself; the helpers' own instructions are another 6.8%.

So an access today costs a call, about 30 instructions, a libc call and
two stack round trips. The page table is not the problem: a software
table lookup can be five instructions.

### 2. Every handler entry and exit makes Rust calls, and so does a call

`H2_LOCALS` calls `h2abi_v1_fast_acquire(ctx)`; the epilogue calls
`h2abi_v1_fast_release(ctx)` and, with accounting, `h2abi_v1_charge`;
handlers with loops call `h2abi_v1_budget`. A direct call
(`H2_DCALL`) first flushes the access count (`charge` again), then
`h2_direct_enter` and `h2_direct_back` each take and release the memory
gate with a compare-and-swap just to read one execute bit. That glue is
3.9% as leaf time, before the register spill and reload it forces.

The runner path costs more per transfer and is still taken often: 7.7%.
Each runner transfer pays the safe point, `frames.unwind`,
`resolve_cached` (a 256-entry direct-mapped cache; a hit clones the
handler), `check_execute`, `handler::execute`'s validation on both
sides, and then `boot::fallback::trampoline`, which looks the selector
up in a SipHash `HashMap` before calling the real handler. The static
call sites are nearly all direct (in a sample of 32 shards: 1,817
`H2_DCALL` against 20 runner calls), so the runner's load comes from
elsewhere: HLE calls (thousands of D3D entries a frame, each a transfer
in and a return out), returns that do not match an inline continuation,
and direct calls refused at run time. A direct call is refused unless
the runner has already resolved that target into one of 256 per-context
slots (direct-mapped on `pc ^ pc >> 8`; 4096 since feddd0402), so two hot
callees that collide evict each other and both keep going through the
runner. The image has 18,691 functions.

### 3. Floating point pays for exact flags one scalar lane at a time

`h2sn_arith` (`h2sse_native.h`) handles a packed op as a loop over four
lanes on an unsigned 128-bit integer: per lane an exponent-range test, a
conversion, the operation in `float`, the same operation again in
`double` to decide the precision flag, and shifts to reassemble. It
reads the host FPCR (`mrs`) on every instruction. It is correct and it
is native, but it is roughly 40 instructions per lane for what is one
NEON instruction plus a flag decision.

x87 values live as ten bytes in `cpu->x87.st[]`; every native op
(`h2x87n_*`) unpacks ext80 to `double`, checks the tag and control
words, computes, and repacks with tag and status updates. Most of these
are out of line too (one copy per shard of `h2x87n_arith`,
`h2x87n_ld_f32`, `h2x87n_st_f32`, `h2x87n_store_commit`).

The transcendentals are the largest single x87 cost: 3.8%, almost all
FYL2X and F2XM1 from three callers (`0x16b90`, `0x372d6c`, `0x327f84`,
the `pow` path). This is already the 128-bit first pass
(`Wf<2>`), not the 512-bit helper. Nearly half of it is division:
`Wf<2>::div`, `div_u64` and the `u128` division builtin are 1.8%.

### 4. A draw copies everything it touches out of guest memory

`Device::draw_indexed_vertices` reads the index list into a `Vec`,
scans it, rebuilds it as a `Vec<u32>`, and copies the whole vertex
window `lo..=hi` of every stream out of guest memory (`copy_stream`
zero-fills the buffer first, then overwrites it). `submit_inner` then
re-resolves all four texture stages (header reads, `TextureDesc`,
`dirty_since`), builds the shader inputs and program key, translates
state and boxes the state words, for every draw, whether or not anything
changed since the previous draw. Guest reads go through `dyn
GuestMemory` to `Guest::read_bytes`, which validates the mapping and
copies (4.2% of the frame is `read_bytes` under D3D). Large per-draw
vectors grow and shrink enough to show as `realloc` and `madvise`
(1.9%). The render thread then uploads the same vertex bytes again.

Level geometry does not change from frame to frame. The write tracker
that would prove that (`d3d::dirty::WriteTracker`) already exists and is
used for textures.

### 5. Two things in the profile should not be there at all

- **Fallback, 3.3%.** `0x336f0` and `0x33980` reach a jump-table case
  the generated switch lacks. The handler records an event
  (`boot::shim::event`, which copies a large record: 0.9%), stops, and
  the trampoline interprets from there. The interpreter clones each IR
  instruction it steps (`Vec<ir::Stmt>::clone` plus its drop is 1.0%).
  `boot/src/fallback.rs` documents the cause: the analyzer stops reading
  a table at its first zero entry.
- **The kernel-call trace, 2.8% (7.5% on the menu).** `boot::trace`
  formats and `write`s one unbuffered line per kernel call into
  `kernel-trace.tsv`. This is `h2 run` only, but it is in every number
  measured with `h2 run`.

### 6. The image is 234 MB of code

`__text` is 234 MB for about 4 MB of x86: roughly 55 times. Each memory
access site expands to a call sequence with a slow-path call and a
fault exit; each call site to the direct-call protocol. That size costs
instruction-cache and iTLB misses that no sampled function shows. It
also decides how an inline access must be shaped (below), and it may be
why the earlier inline-translation experiment (`docs/perf/fastmem-4k.md`) was
slower than phase 1 even though it removed work.

## Ranked changes

Savings are for Old Mombasa on the game thread, against the 55 ms frame.
They overlap a little (items 2 and 3 both shrink "own code"), so the sum
is discounted at the end. Days are for one person, tests included,
excluding image rebuild time.

| # | change | saving | cost | risk |
|---|---|---|---|---|
| 1 | Remove what should not run: the two unread jump tables, the unbuffered trace, the trampoline's hash lookup | 3.3 ms (6%); 1.8-2 ms in `h2 play` | 2-3 days | low |
| 2 | Memory access as a real inline load or store | 5-6 ms (9-11%) | 5-7 days | low to medium |
| 3 | Thin transfers: no Rust calls or gate round trips per handler, direct calls that never miss, a flat dispatch table | 3.5-4.5 ms (6-8%) | 6-9 days | medium |
| 4 | Floating point: packed NEON for SSE with a vector guard; exact memo and division-free series for FYL2X/F2XM1 | 3.5-4 ms (6-7%) | 6-9 days | medium |
| 5 | D3D draw path: persistent vertex and index buffers keyed by write generation, borrowed guest slices, resolve only what changed | 3 ms (5-6%) on the game thread, more on the render thread | 8-12 days | medium |

Sum 18-21 ms; allowing for overlap, 16-19 ms. From 47.7 ms that is
28-31 ms.

### 1. Remove what should not run

Status: landed after this review. `analyze` reads the two tables (and a
third at `0x22dcd9`) past their runs of nulls (11bfe00d), `boot::trace`
is buffered (566f6715), and generated handlers are registered as
themselves, with the trampoline's STOP logic as the runner's stop-recovery
hook (4ce2258a; the trampoline's A/B switch has since been removed). Not re-measured here.

- Read the two jump tables fully in `analyze` (skip zero entries the
  way `boot::fallback::unread_targets` already does at run time) so
  `0x336f0` and `0x33980` have every case and never stop. The stopgap in
  `fallback.rs` names the owner and the rule.
- Buffer `boot::trace` (a `BufWriter` flushed at exit and on stop), or
  leave it off unless asked for.
- Register real handlers instead of one trampoline for all of them, or
  at least index `TABLES.handlers` by a flat table. The stop-recovery
  check only needs to run when a handler returns `STOP`.

Retail behaviour: the interpreter and the generated code implement the
same IR, so replacing one by the other at these two sites is the change
the lockstep harness exists to check. The slice schedule shifts only if
the interpreter and generated code charge accesses differently for the
same instructions; check that first, since goldens depend on it.

Measure first: the fallback `STATS` (fallbacks and interpreted steps per
frame, and the `first` list, to confirm it is only these two sites);
then one run with the trace off.

### 2. Memory access as a real inline load or store

Status (2026-11-16): the width-specialised inline access with a cold,
register-preserving slow call has landed (`docs/perf/guest-memory.md`, "Inline
guest loads and stores"); the host-address tables, dirty tracking through
the write table and slice-long gate ownership below are not done.

Keep the model (4 KiB software page table over 64 MB of RAM, slow helper
for everything unusual). Change the fast path's shape:

- **Width-specialised accessors with no memcpy and no staging buffer.**
  `h2_ld8/16/32/64` return the value in a register; stores take it in
  one. The page-crossing test stays, as one compare.
- **A table entry that is already a host address.** Two tables indexed
  by `addr >> 12`, one for reads and one for writes, each holding
  `host_page_base - guest_page_base` or zero. A zero entry means "take
  the helper": unmapped, no permission, MMIO, partial page. Then a load
  is shift, table load, test, add, load. The permission and
  physical-range tests fold into how the table is built.
- **Dirty tracking through the write table**, as `fastmem-4k.md`'s mode
  3 tried: a watched page that has not been dirtied since the last drain
  has no write entry, so its first store takes the helper, which marks
  it and installs the entry. Later stores are plain. This removes the
  `watching` test and the bitmap update from every store, and keeps
  exact 4 KiB dirty semantics.
- **Keep the slow path out of line and cold**, one call per site at
  most, so the per-site code gets smaller than today's, not larger.
  Given finding 6, build both shapes and compare: a forced-inline fast
  path, and a tiny non-inlined leaf per width (a call, eight
  instructions, no libc). The leaf form alone removes the memmove and
  most of the helper body.

Accounting, fault order, whole-span failure and the access-budget stop
are unchanged: the count is still one per attempted access, and every
unusual case still reaches the same helper.
`held_gate_matches_the_helper_path_at_every_stop_and_fault` is the
equivalence test to extend.

**The gate.** All guest threads run on one host thread. The only other
takers of the memory lock are host threads such as the DirectSound pump
and output threads (`dsound::register::host_call` takes
`runtime.memory()`). So the gate does not need to be taken per run of
accesses and dropped before every call. The runner thread can own it
for a slice, and a waiting Rust guard can raise the preempt word so the
handler gives way at its next poll, as `H2_GATE_WANTED` does today
within 32 loop trips. That removes the `held` test from every access
and the two gate round trips from every direct call. Risk: an audio
thread waits up to one poll interval longer; bound it and measure
underruns.

**Should guest memory become direct host loads (base + address, no
table)?** Not now. It needs a 4 GiB reservation with guest pages mapped
at their guest addresses, and the guest maps at 4 KiB while the host
pages are 16 KiB: `fastmem-4k.md` measured only 61% of accesses eligible
under a 16 KiB contiguity rule against 99.999% for the 4 KiB table. It
would also move write-watch to Mach exceptions (`fastmem-proto`).
After the table form above, what is left to win is one load and one
test per access, perhaps 1-2% of the frame, for the largest and
riskiest change on this list. Revisit only if accesses are still a top
row afterwards.

**Can the per-access charge go?** The count cannot: it is the
scheduler's clock (slices and logical time are in accesses), and the
lockstep harness and the goldens depend on it. Its cost can. The
increments are already a merged local (`h2acc`). What costs 3.7% is
settling it through Rust calls (`charge` 1.1%, `budget` 0.5%, and
`sync_accesses` inside `fast_acquire`/`fast_release` 2.1%). That is
change 3.

Measure first: `generated_runtime_benchmark` (`read`, `write`, `frame`
kinds) for both shapes against today's; `__text` size of a full image
for each; a static census of how many accesses are `esp`/`ebp`-relative
with a constant offset (see "After the top five"); then accesses per
frame from frame stats on Old Mombasa, to turn ns per access into ms.

### 3. Thin transfers

Status (2026-11-17): the epilogue now makes one call, `h2abi_v1_exit`, in
place of `charge` and `fast_release`, and a direct callee runs on its
caller's inline view, so it skips `fast_acquire` and `fast_release`
(`docs/perf/guest-memory.md`, "One exit call, and a callee on its caller's
view"). Reading the budget and count from context fields, the execute
permission by generation, direct calls that need no warming and the flat
dispatch table below are not done.

- **Context header fields instead of calls.** Put the fast-memory view,
  the access budget and the spent count where C can reach them from
  `ctx` (it already reads the preempt word and the `H2Direct` block that
  way). `fast_acquire`, `fast_release`, `charge` and `budget` become a
  few inline loads and a compare; the runner reconciles slice state at
  its safe points, where it already looks.
- **Execute permission by generation, not by gate.** Bump the dispatch
  generation (or a second counter beside it) when a lifted page loses
  Execute; `Guest` already refuses that for guarded lifted code
  (`MapError::LiftedCode`). Then `h2_direct_enter`/`back` need no gate.
- **Direct calls that do not need warming.** Replace the 256 learnt
  slots with a per-function flag the runtime owns: "the registered
  handler for this entry is the generated one". It is written at load
  and at `edit_dispatch`, and read as one byte by the caller. Overrides
  are installed at boot and rarely change, so in steady state every
  static call is direct, with no collisions.
- **A flat dispatch table for the runner**, indexed by `pc` minus the
  image base, for the transfers that remain (indirect calls, HLE
  returns). It replaces the resolve cache, the handler clone and the
  trampoline lookup.
- **HLE calls without two full runner transfers**: a Rust override
  called from the runner needs the safe point, but not `resolve` +
  `check_execute` + ABI validation on both sides of a host function the
  runtime registered itself.

Every transfer still counts against `max_transfers`, safe points still
occur exactly where the runner would have had work to do (preempt word
set, interrupt pending), and the diagnostic frame chain is replayed as
today. `crates/recompiler/emit/tests/direct.rs` already proves identical state
with direct calls off and on across limits, faults and preemption; it
is the harness for this too. This is an ABI revision (module feature
bit and dylib stamp), so old images must be refused, not misread.

Measure first: per frame, `transfers`, `direct_transfers`,
`runner_safe_points` and `generated_batches` from the existing profile
counters, plus a temporary count of why `h2_direct_enter` said no (slot
miss, depth cap, not quiet, execute check). If slot misses dominate, the
flag alone is most of the win. Also count HLE calls per frame by
handler.

### 4. Floating point

Status (2026-11-17): the table-driven first pass for FYL2X and F2XM1 has
landed (`docs/lifter/x87-transcendentals.md`, "Table-driven first pass"),
which is the division-free-series idea below in another form: F2XM1 went
from about 1030 to about 525 ns per call. The memo, the NEON SSE helpers
and the double-slot x87 stack are not done.

**SSE.** Keep the guard design and the proof in `docs/lifter/sse-native.md`;
change the unit of work from a lane to an instruction.

- Hold XMM values as NEON vectors in the helpers, not as a 128-bit
  integer taken apart by shifts.
- Vectorise the guard: one compare pair on the exponent fields of both
  operands answers "all four lanes eligible".
- Decide the precision flag with vector arithmetic (the same exact
  tests, two `float64x2` halves), or skip the decision when MXCSR's PE
  bit is already set: it is sticky, and after the first inexact result
  the answer cannot change until the guest clears it.
- Read FPCR once per handler entry, not per instruction. Nothing inside
  a handler changes the host rounding mode.

The reference helpers stay as the fallback for every rejected case, so
results and flags are the same bits by construction. `crates/recompiler/sse-oracle`
is the check.

**Transcendentals.**

- An exact memo: a small direct-mapped cache from operand bits (and the
  control word) to result bytes and status flags, in front of FYL2X and
  F2XM1. The functions are pure, so a hit is exact by definition. Whether
  it pays depends on how often `pow` repeats operands; that is the first
  thing to measure.
- Remove division from the series: multiply by precomputed reciprocals
  of the small odd integers under the same error budget
  (`docs/lifter/x87-transcendentals.md`), keeping the one real division. The
  interval guard that accepts or rejects a first-pass result does not
  change, so an accepted result is still bit-identical to the 512-bit
  helper's.

**Ordinary x87** (1.9% in helpers, more inline): the larger change is
to stop round-tripping through ext80 on every operation. Within a
function the x87 stack depth is almost always static, so the emitter
can keep stack slots as `double` locals with a "holds an exact double"
flag, and write the ten-byte form and tags only at exits, calls and
operations the guard rejects. That is second-stage work (it changes the
IR and the fault-state rules in `FLOAT-ABI.md`), listed here because it
is where x87-heavy functions such as `0x2dc220` (4.3% of the frame
inclusive, the hottest function) spend their own time.

Measure first: how often `h2sn_arith` accepts (a counter per op class;
if rejection is common in gameplay the soft-float path matters more than
this estimate assumes); the hit rate of a memo of, say, 1,024 entries
from an operand log of one gameplay minute; and how often PE is already
set when an SSE op runs.

### 5. D3D draw path

Status (2026-11-16): persistent vertex and index buffers keyed by write
generation have landed (`docs/graphics/d3d.md`, "Vertex cache"); borrowed guest
slices, resolving only what changed and pooled state words are not done.

After the in-flight fixes the game thread still spends about 15% in D3D
entries and write-barrier shadows (8.5 ms). The structural part:

- **Persistent GPU vertex and index buffers.** Key a buffer by guest
  range, stride and the write tracker's generation for those pages. A
  draw whose source has not been written since the buffer was filled
  sends an id and an offset; nothing is copied on either thread.
  Buffers the guest rewrites every frame (dynamic geometry) fall back to
  today's copy after one or two invalidations. The tracker already gives
  exact 4 KiB dirty pages, and `write_back_overlapping` and the texture
  cache already depend on it being complete.
- **Borrowed guest slices for HLE.** `GuestMemory::read` copies after
  validating; add a call that validates the same way and returns a
  borrowed slice when the range is contiguous in RAM (resource memory
  from the contiguous allocator is). Index scans, header reads and
  texture decode then read in place.
- **Resolve only what changed.** Give the pushed state, each texture
  stage and the shader selection a generation; `submit_inner` reuses the
  previous draw's translated state, texture resolution and program key
  when the generations match. Consecutive draws usually differ in one
  of those, not all.
- **No per-draw heap traffic**: indices as a reused buffer of the
  original 16-bit values plus a base (no rebuild as `u32`), state words
  in a pooled slot instead of a `Box`.

**Should push-buffer decode move to the render thread?** No.
`drain_until_callback` is 2.8% inclusive, and most of that is the
immediate draws it issues, which the items above make cheaper where they
are. Moving decode means the render thread reads guest memory at a later
time than the guest wrote it, which needs either snapshots of everything
a draw can reach or real asynchronous fences that Halo would then wait
on (`docs/graphics/render-thread.md` reaches the same conclusion). The split is
right; the per-draw work on the guest's side of it is too heavy.

Guest-visible behaviour: unchanged if the tracker is complete, since a
cached buffer holds exactly the bytes a copy would have read. That "if"
is the risk. Host writes to guest memory must mark pages too
(`mirror`, write-backs, kernel file reads into resource memory). A
debug mode that copies anyway and compares bytes against the cached
buffer on every draw makes a missed write a named error instead of a
wrong picture.

Measure first: per frame, draws, bytes copied by `copy_stream`, and the
fraction of those bytes whose pages are clean since the previous frame
(`dirty_since`); and the render thread's busy time per frame (frame
stats' `encode`, `submit`, `gpu-wait`, or `H2_RENDER_THREAD_STATS`) for
the same window. If the render thread is near 30 ms in Old Mombasa, it
is the next bound the moment the game thread gets under it, and this
change matters more for what it removes there (uploads) than for the
3 ms here.

## After the top five

What is left at about 28-31 ms is mostly "guest code, own time", still
around 20% of the original frame, plus the residue of each item. Three
structural options, in the order the evidence supports:

1. **Cheap stack accesses.** x86-32 code keeps arguments, locals and
   spills on the stack, so a large share of all accesses are
   `esp`/`ebp`-relative with a constant offset. Thread stacks are
   allocated by our kernel, so they can be made contiguous in RAM, and
   a handler can validate "this stack range is mapped, writable RAM"
   once and then address it as one base pointer. The count per access
   stays. Take the static census in change 2 first: if stack accesses
   are over half, this is worth more than anything else left.
2. **x87 stack promotion** (change 4, last part).
3. **Code size.** Measure iTLB and L1i misses on the game thread
   (`xctrace` counters) before and after change 2. If they are high,
   sharing fault exits per function and moving cold paths out of line
   pays everywhere at once.

**More game threads in parallel?** No. The Xbox has one CPU, and the
title's threads were written for that interleaving; the scheduler here
reproduces it deterministically from access counts, and the lockstep
harness depends on it. Running guest threads on several host threads
would trade that for races the original never had. (The profile cannot
say how the guest's time splits between its threads: they all run on
`main`.) What can leave the game thread is host work whose output the guest cannot see: the
DirectSound mix (`dsound_glue::advance`, 0.9%) and the trace. That is
about 1 ms in total, not worth a design change by itself.

## What this review does not establish

- **The render thread's load in gameplay.** No profile samples it. The
  frame is `max(game, render)`. Sample both threads in the same window
  before committing to the order of changes 4 and 5.
- **Event counts.** The ms estimates divide sampled time by plausible
  per-event costs; accesses, transfers, direct-call refusals, draws and
  bytes copied per frame were not available for Old Mombasa. Each
  change lists the counter that turns its estimate into a number.
- **Quiet-machine numbers.** The load during the Old Mombasa profile is
  not recorded. Shares carry over; ms may not.
- **`h2 play` against `h2 run`.** The 18 fps figure and the profile may
  not come from the same front end. The trace row only exists in
  `h2 run`.
- **Savings are estimates from reading code**, not measurements. The
  two cheapest checks are the synthetic access benchmark for change 2
  and the transfer counters for change 3; both run without building the
  game.
