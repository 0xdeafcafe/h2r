# Pipelined rendering

Status: **a first prototype is built, off by default**
(`H2_PIPELINE_RENDER=1`, "The prototype" below), on the census that
decided it (`render-census`) and the second executor
(docs/runtime/second-executor.md). The first census run and its verdict
are in "Census results (2026-11-18)". Not yet run in the game.

The question: can the game's own render front-end
(`render_frame_and_present` `0x00223240` and everything under it,
docs/engine/feature-engine.md section 7) run on a second host thread one
frame behind the simulation, against a copy-on-write snapshot of guest
RAM, so a frame costs `max(sim, render)` instead of `sim + render`?

## What it would buy

From docs/perf/frame-budget.md, "Render front-end versus simulation"
(Old Mombasa, headless, `render-timing`, 2026-11-17), per frame:

| draws | sim (guest, outside render) | render front-end (guest) | our D3D layer on the game thread | serial | pipelined, ideal |
|---|---|---|---|---|---|
| 2000 | 7.8 ms | 18.9 ms | 7.7 ms | 34.4 ms | 26.6 ms |
| 1000 | 6.2 ms | 16.3 ms | 5.2 ms | 27.7 ms | 21.5 ms |

The D3D layer moves with the render front-end (it is called from it), so
the render side is 26.6 ms and the sim 7.8 ms. Pipelining hides the
**smaller** side: at best it saves the sim's 6-8 ms a frame (about 22%),
less whatever the snapshot and the hand-back cost. The render side stays
the critical path, so the per-draw work in frame-budget.md keeps its
value either way; pipelining stacks on top of it.

## Design

One frame of the main loop (docs/game/high-framerate.md, section 1) is
ticks, then per-frame work, then render and present (`0x00223240`), then
sound. Pipelined, frame `k`'s render runs while frame `k+1`'s ticks run:

```
sim thread:    [ticks k ][join k-1 | snapshot k | launch k][ticks k+1 ][join k | ...
render thread:                                    [render k from snapshot k     ]
```

At `render_frame_and_present`'s entry the sim thread (the game thread)
does not run the original. It:

1. **Joins** render `k-1`: waits for it, then hands its results back
   (step 4).
2. **Snapshots** guest RAM for render `k`.
3. **Launches** render `k` on the render thread with a copy of the CPU
   state at the entry, and returns to the game as `0x00223240` would.

**The snapshot.** Two ways, in the order to try them:

- *Host copy-on-write.* Guest RAM is one 64 MB allocation
  (`runtime::Guest`). `mach_vm_remap(copy = TRUE)` (or `vm_copy`) of it
  gives the render thread a second mapping that the kernel copies page by
  page as the sim writes the original. Setup is a page-table walk (tens
  of microseconds for 64 MB); each sim page written while a render runs
  costs one COW fault, on arm64 macOS per 16 KB host page (the census
  counts 4 KB guest pages: divide `sim_written` by up to 4). The render
  thread needs its own `Guest` over that mapping and its own fast page
  table (`Guest::prepare_fast_pages` builds it from the same mappings),
  so in practice a second `runtime::Runtime` sharing the dispatch and the
  generated module, with its own `MemoryShared` gate. The two threads
  never share a gate, so nothing serialises them.
- *Software copy-on-write through the fast page table.* The access trap
  `render-census` uses (`runtime::trap`) already sends the first write to
  each page through the slow path; that path could copy the page aside
  first. It is cheaper per page than a kernel fault, but the render
  thread would read the live RAM for pages the sim has not touched, and
  a sim write racing a render read of the same page needs a per-page
  handshake. Host COW has no such race; use it unless its fault cost is
  measured to matter.

**The render thread's CPU and stack.** The render front-end runs as guest
code on its own `ExecCtx` and `CpuAbi`, starting from the CPU state at
the entry. It can keep the game thread's stack *addresses*: in its
snapshot those pages are private copies, so its pushes never reach the
sim's stack. Stack pages are dropped at the hand-back (the census's
`stack` region; `rs_core` leaves them out for this reason).

**Hand-back (render-written state).** Render `k`'s writes land in its
private snapshot. What the next render (or the sim) needs has to reach
live RAM at join `k`:

- pages only the render writes (`rw`, not `ww`): copy back whole;
- pages both write (`ww`): three-way merge against snapshot `k`'s
  original bytes, render bytes winning where only the render changed
  them; the census's line masks bound how much of each page that is;
- the stack: dropped;
- the push buffer and the D3D globals (`push_buffer`, `d3d_globals`,
  `rw_d3d_only`): the D3D layer's own; they move with the device (below)
  and are copied back like the rest.

The cost is the copy of `rw` pages (a few hundred 4 KB pages is well
under a millisecond).

**What the sim must not see change.** Pages the render writes and the
next sim reads (`rs`) are the problem. Serially, ticks `k+1` read what
render `k` wrote. Pipelined, they read what render `k-1` wrote: the hand
back of render `k` has not happened yet. Every such page is a behaviour
change unless what the sim reads there is cosmetic or can move. That set,
less the stack and our D3D layer's own pages, is the census's `rs_core`;
its line masks (`dep_lines`) say which bytes, its `render_writer` and
`sim_reader` say which code.

**D3D calls from the render thread.** The D3D device (`d3d::Device`, the
HLE entries) already serialises on its own lock and is only reached from
D3D entries, so it can run on the render thread as long as the sim makes
no D3D calls of its own while a render runs; the census counts those
(`sim_d3d_hits`). The backend's render thread
(docs/graphics/render-thread.md) is unchanged: the device's thread just
becomes the pipelined render thread instead of the game thread. Device
reads that wait on the GPU (class 1 there) then wait on the render
thread, not the game.

**Kernel and other HLE from the render front-end.** The render code may
call the kernel (time, critical sections, events). The kernel's machine
is single-threaded and cooperative (docs/runtime/snapshots.md, "How guest
threads run"). The render thread can only be allowed a fixed, audited
set of HLE calls (time reads answered from the frame's snapshot, no
waits); anything else ends pipelining for that frame (join, then run the
rest serially). The census's `rw_host_only` and host reads in the render
scope are a first indicator; an HLE-call count per scope is the next
measurement if they are not small.

**Logical time.** Generated code charges the logical clock one
nanosecond per guest memory access (docs/game/high-framerate.md, "In our
runtime"), and the render front-end is three quarters of the game
thread's accesses. The render thread's access count must be charged to
the game thread's clock at a fixed point (the join), so the schedule
stays a function of the inputs. The schedule then differs from a serial
run's (the charge is one frame late), which is fine for play but means
**pipelined and serial runs are not lockstep-compatible**: co-op peers
(docs/live) must agree on the mode, and lockstep/oracle tests compare
like with like.

**The frame wait and present.** The frame wait (`0x12b2e0`, vblank
pacing) and the Swap happen inside `render_frame_and_present`. The
frame wait reads the vblank counter, which the kernel's periodic vblank
source advances on the game thread's clock; it has to stay on the game
thread (at the join), and the render thread's Swap goes through the
device as today.

**Latency.** The picture shown is one frame older than the simulation:
+33 ms at 30 Hz. Input is still polled once per frame at the same
point; only its display is a frame later.

**Interpolation and high frame rate.** The high-frame-rate design
(docs/game/high-framerate.md) works after the present hook on host
surfaces and never touches guest memory, so it is unaffected; it gets
frames one frame later. A guest-native 60 Hz (the interval word) would
double the render side and is rejected there already.

**Snapshots and quiescence.** A running render thread is live host state
(an `ExecCtx` mid-frame) of exactly the kind docs/runtime/snapshots.md
says blocks a snapshot. Snapshots, checkpoints and the agent API's pause
must join first; after the join the machine is serial again and the
existing rule applies.

## The census (`render-census`)

Built: `crates/feature/src/render_census.rs`, on the runtime's access
trap `crates/runtime/src/trap.rs`. Off by default.

**How it sees accesses.** Every inline guest access, in generated C
(`h2fastmem.h`: `H2_HELD_TRY`, `h2_fast_copy`, `h2_fast_movs`) and in
Rust (`Guest::read_bytes`/`write_bytes`), first checks the page's read or
write bit in the fast page table. Arming a page clears those bits, so the
next access of that kind goes to the checked slow path, which logs it:
the guest `pc` (the ABI helpers pass it), or 0 for the host, and the
timeline phase (D3D, render, or another guest thread's slice). A guest
hit re-opens that kind on that page, so each page costs at most one slow
read and one slow write per scope; the host's first hit is logged and
the page stays armed, so guest code that writes after our D3D layer is
still seen. No `mprotect`, no fault handler, no change to generated code
or the image: the fast page table already is a software MMU every access
consults. The cost is re-arming every mapped page twice a frame (a pass
over the mappings) and one slow access per touched page and kind.

Write tracking alone (the write watch's dirty bits) could not do this: it
has no reads, no `pc`, and no way to tell our D3D layer's stores from
the game's.

**What it records.** At each `render_frame_and_present` entry and return
it takes the log and re-arms every page. The render scope gives the
render's read and write sets, the sim interval between two renders the
sim's. Per frame it pairs them:

| line field | pages that are... |
|---|---|
| `rw` | written in the render scope; split `guest` (any guest code wrote), `d3d_only`, `host_only` |
| `render_read` | read in the render scope |
| `sim_written`, `sim_read` | written / read in the sim interval |
| `rs` | render-written, then read by the next sim interval |
| `rs_core` | `rs`, less the stack and pages only our D3D layer wrote |
| `ww`, `ww_core` | render-written, then written by the next sim interval |
| `sr` | sim-written, then read by the next render: what the snapshot freezes |
| `other_thread_hits` | hits from another guest thread's slice inside the render scope (left out) |
| `sim_d3d_hits` | D3D-layer hits in sim intervals: D3D calls the sim makes |

Per page it keeps the first writer and reader of each relation (guest
`pc`, or `d3d`/`host`) and, for the pages made sticky, the 64-byte lines
the render wrote, the sim read, and their overlap (`dep_lines`). Regions: `stack` (256 KB below the render
entry's `esp`), `push_buffer` and `d3d_scratch` (from the boot),
`d3d_globals` (the D3D library's device and state block in the XBE,
`0x00403000..0x00409000`), `datum:<name>` (datum arrays found in the
game-state block by their `d@t@` signature), `game_state` (the
`0x3fe000`-byte block at `[0x004e6080]`), `world_pool` (`0x80061000..
0x83145000` and its `0xf...` alias: tag cache, BSPs, carved buffers),
`xbe_image` (globals), `contiguous`, `write_combined`, `other`.

**Census v2 (2026-11-18).** The first run's three caveats ("Census
results" below) are fixed:

- *Sticky pages per region group.* At each report, the pages most often
  in `rs_core` are made sticky in each group: `xbe` (`xbe_image`),
  `game_state`, `datum` (every `datum:*`), `world_pool` and `other`
  (contiguous, heap, the rest). The stack and our D3D layer's own memory
  are never sticky. `H2_RENDER_CENSUS_LINES` is the budget per group
  (default 64, `0` none, `all` every `rs_core` page). The trap keeps a
  sticky page's line masks itself, per guest thread (`runtime::LineRec`),
  instead of logging each access, so the budget costs slow accesses but no
  log space. The `coverage group=…` lines give each group's `rs_core`
  pages a frame and how many of them had line data; `rs_core_lined` in
  the summary is the total.
- *Order.* A sim read counts toward `sr_lines`/`dep_lines` only if the
  same thread had not written that line earlier in the interval.
  `sr_lines_any`/`dep_lines_any` keep the old, unordered meaning, for
  comparison.
- *Threads.* Every hit carries the running guest thread (from the frame
  stats). The game thread is the one running the render entry. In both
  scopes, other threads' hits leave the page sets and are counted per
  thread (`other_thread scope=… thread=…`). Another thread reading a page
  the render wrote, in the next interval, is `rs_other` (per page and
  per frame), and its lines `dep_lines_other`. Without frame stats there
  are no thread ids, and the census falls back to the phase rule.
- *HLE calls.* The runner counts each Rust-handler call (kernel export
  or override) by target and guest thread, with the calls that ended in
  a STOP (blocked, yielded or preempted). The census splits them by scope
  and names them (`kernel:<export>`, or the override's registered name):
  `hle scope=render name=… calls/frame game=… other=… stops …`, the top
  `H2_RENDER_CENSUS_TOP` per scope plus every wait (`Wait`, `Delay`,
  `Yield`, `Sleep` in the name).
- *Host writes by subsystem.* A host hit carries the Rust handler it ran
  in. `host_writes scope=render by=<name>` counts render-scope pages a
  frame by first host writer, and the TSV's `host_writer` column names
  it per page. `<source>:no-handler` is host code outside any handler
  (the boot loop between slices, say).

The TSV keeps the first 20 columns (with `sr_lines` and `dep_lines` now
ordered) and adds `rs_core`, `rs_other`, `host_writer`, `sr_lines_any`,
`dep_lines_any`, `other_read_lines`, `dep_lines_other` and
`sticky_frames`. The `lines …` report lines list every page with a
dependency line of any kind.

**Limits.** Raw RAM readers that bypass the accessors
(`Guest::physical_ram`: the lockstep hash, snapshots) are not seen; none
is guest code. Atomics
(`runtime::atomic`) count as the host. A hit from another guest thread
consumes that page's arming for the scope, so the owner's own first
access to it is missed at page level (rare: count shown). Sticky pages
are exact per thread.
Without frame stats every host hit is `host` and nothing is
`other_thread`. The run is slower while the census is on (two passes
over the mappings and a few thousand slow accesses a frame); its numbers
are page sets, not times.

**Output.** Every `H2_RENDER_CENSUS_EVERY` frames (default 300) and at
game end: `render-census: window …` and `run …` summary lines (per-frame
averages), `region …` counts, the top `rs`, `ww` and `sr` pages
(`H2_RENDER_CENSUS_TOP`, default 20) with region and first writer and
reader, and the v2 lines above. With `H2_RENDER_CENSUS_OUT=<path>` the full per-page table
is rewritten there as TSV at each report.

**Running it.** It needs a new `h2` binary only (no generated-code or
image change):

```sh
H2_FEATURES=render-census H2_FRAME_STATS=1 H2_RENDER_CENSUS_LINES=64 H2_RENDER_CENSUS_OUT=$TMPDIR/census.tsv h2 run …
```

`H2_FRAME_STATS=1` is required for v2's thread split and handler names
per thread. If the run is too slow with 64 sticky pages per group, lower
the budget; the coverage lines show what is lost.

Add `render-timing` to `H2_FEATURES` to get both in one run; the census
then leaves the render scope to it.

## Reading the result

Per frame, from the `run` line, after a minute in a busy level:

**Feasible** (build it): `rs_core` is a handful of pages, and for each
of them `dep_lines` is a few lines with one or two known writers, and
each is either cosmetic for the sim (camera or listener copies, a
frame counter only the renderer reads back) or a value the sim can
compute itself or receive at the join one frame late without changing a
tick (prove with the simulation hash, `lockstep`). `ww_core` is small and
its lines don't overlap (`rw_lines & sr_lines` per page), `sim_d3d_hits`
is zero in gameplay, `other_thread_hits` small. `sim_written` (the COW
fault count, over up to 4 per 16 KB host page) times the measured fault
cost is well under the sim's 6-8 ms.

**Hard** (each item is a port): `rs_core` is tens of pages in
`game_state`/`datum:*` (objects, players, effects) with real line
overlap: the render front-end writes simulation-visible state (render
or visibility flags on objects, cached node matrices, LOD state the
simulation reads back). Each writer `pc` is a site to move to the sim
side or double-buffer, checked by the simulation hash. Or the sim calls
D3D (`sim_d3d_hits` steady), so the device needs ordering between the
threads.

**Infeasible** (for this approach): `rs_core` is hundreds of pages, or
spread across most datum arrays with dense `dep_lines`, so the renderer
and the simulation share state frame to frame; or the render front-end
waits on other guest threads inside its scope (`other_thread_hits` high,
kernel waits), so it cannot run apart from the kernel's single-threaded
scheduler. Then the remaining route is the native renderer (feature
engine stages 7b-7d), which owns its state by construction.

`sr` (what the snapshot freezes) may be large; that is expected and only
enters through the COW fault cost.

## Census results (2026-11-18)

One run: Old Mombasa, headless, 4200 frames, `render-census` with frame
stats, default sticky set (16 pages, re-picked every 300 frames). The
TSV and log stay outside the repository. The writer and reader names
below come from the generated C of master's image (the global addresses
each instruction loads or stores), the Ghidra function table, and a
direct-call graph from the main loop's callees (docs/game/high-framerate.md,
section 1). "Sim" in this section means the interval between two renders,
not just the ticks (see the second caveat).

### Numbers

Per frame (the `run` line):

| | pages |
|---|---|
| `rw` render-written | 226.2 (guest 162.7, `d3d_only` 62.0, `host_only` 1.6) |
| `render_read` | 796.0 |
| `sim_written` / `sim_read` | 167.6 / 670.8 |
| `rs` / `rs_core` | 54.1 / 46.5 |
| `ww` / `ww_core` | 42.3 / 35.2 |
| `sr` | 88.7 |
| `other_thread_hits` | 48.7 (204,512 over the run) |
| `sim_d3d_hits` | 404 over the run (about 0.1 a frame) |

`rs` pages outside the stack, by region, per frame: `xbe_image` 21.3,
`game_state` 9.1, `world_pool` 6.8, `other` 3.8, `datum:lights` 2.0,
the particle and effect datum arrays 2.6 together, `contiguous` 1.0.

At line granularity, 71 64-byte lines on 15 pages are both written by
the render and read by the next sim interval (`dep_lines`): 69 of them on
13 `xbe_image` pages (the XBE's data and bss), one on a heap page
(`0x7fce6000`), one on a contiguous page (`0x8000b000`).

### Three caveats on reading the line data

1. **Only 17 pages have line data, all but two in the XBE image.** The
   sticky set is the 16 pages with the highest `rs` count. About 20
   pages are `rs` in every frame, so the ties are broken by the lowest
   address, and the XBE pages win every time. The `game_state`,
   `datum:*` and `world_pool` pages were never sticky. Their zero
   `dep_lines` means "not measured", not "no line overlap". That is about
   25 of the 46.5 `rs_core` pages a frame. The region that decides
   between hard and infeasible is the one the run did not look at.
2. **The sim interval holds every guest thread, not only the game
   thread.** `ScopeSet::from_hits` drops other threads' hits only in the
   render scope. A "sim read" can be the async I/O worker reading a
   request the render queued. The `0x004e0000` pool lines below are that
   case.
3. **`dep_lines` ignores order.** A line the sim writes first and only
   then reads counts as a dependency. Scratch state that both sides
   overwrite before use is reported as if the sim consumed the render's
   value. The table separates these where the code shows it.

### Every dependency line

Class: (a) cosmetic or dead for the sim; (b) the sim can take it a frame
late, or the value is reproducible on the sim side; (c) shared state that
has to move to one side or be buffered; (d) not identified.

| page | lines | what (writer → sim reader) | class | remedy |
|---|---|---|---|---|
| `0x004de000` | 16: line 11, 19-24, 40-41, 53-60 | **Object visit marks.** `0x004de2fc` is a counter and `0x004de2f8` a flag; `0x004de300` is a `u32` stamp per object index (2048 × 4 bytes, up to `0x004e0300`). The render's `0x001320f0` (under `render_views` → `0x0002b990` → `0x0003f2c0`, an object gather) bumps the counter (`0x001321b0`), sets the flag, stamps the objects it visits and clears the flag. The tick's object code does the same (`0x000bb050` from objects-before-physics `0x000b6f10`; `0x000bb430` from player-control preparation `0x001870a0`): it skips an object whose stamp equals the current counter. | (a) with a condition | The stamps only matter for equality with the current counter, and the counter only grows, so a stale render stamp never matches a later sim counter value. Make the counter and stamp array **render-private**: leave them out of the hand-back. Live then only holds sim stamps, which are all at or below the sim's counter. If the render's stamps were merged back, the sim (which never saw the render's increments) could reuse their values and skip objects. That would be a tick change. |
| `0x004e7000` | 10: 16-20, 23, 48-49, 51, 53 | Two more mark schemes of the same shape. Counter `0x004e7414`, flag `0x004e7411`, stamps from `0x004e7418`: the render's `0x00132d60` (called by `0x001320f0`) uses it, as do the tick's `0x000bb050`, `0x0016a440` and `0x0014a6d0`. Counter `0x004e7c1c`, flag `0x004e7c18`, stamps from `0x004e7c20`: the render's `0x001320f0` (`0x00132186`), the observer's `0x001697c0` and the tick's `0x0016a440`. | (a) with the same condition | Same as above. One check is still open: `0x00244c10` reads `0x004e7c1c`. Confirm that it compares the counter and does not use it as a number. |
| `0x004e0000` | 1: line 12 | `0x004e0300` is the object header table pointer (written at init, read everywhere). The render writes a fourth mark pair in the same line: flag `0x004e0304`, counter `0x004e0308` (`0x001321bc`/`0x001321c2`). | (a); the line is shared | Exclude the mark pair from the hand-back. The header pointer is never written after init. |
| `0x004e0000` | 20: 13-21, 25-26, 28-36 | **Async I/O request pool and queue** (`async_io_initialize` `0x00120a90`): mutex `0x004e0358`, semaphore `0x004e035c` (150), worker thread `0x004e0360` (entry `0x00120c30`), queue words `0x004e0354`/`0x004e0364`, and 150 requests of `0x40` bytes from `0x004e0368`, linked through `+0x3c` with the free-list head at `0x004e28e8`. Render-scope cache reads allocate a request (`0x001208b0`: wait on the mutex, pop, release, clear; census writer `0x001208d4`) and queue it (`0x001209c0`). The worker (`0x00120c40`, a separate guest thread) reads them during the next sim interval and returns them to the free list. | (c) | The requests have to reach the live pool and the live worker, and they come from a thread that may not take kernel objects. Override the request path on the render thread: log each request and submit it on the game thread at the join, so a stream request lands one frame later. Better, override it at the cache request functions so that the texture and geometry cache bookkeeping stays on one side (next row). |
| `0x004e6000` | 2: 17-18 | **Texture and geometry cache globals** (`0x0012c0d0`/`0x0012d8b0` initialize them): `0x004e6440..0x004e64bf`. Both sides write them: the render through `0x0012c2e0`, `0x0012c600`, `0x0012ce00`, `0x0012de70`, `0x0012e480`, `0x0012e5e0` and `render_game_views` `0x002234e0`; the tick through `0x0016f200`; the observer and the tick through `0x0012ce00` (for example observer `0x0016f280` → `0x0003f450` → `0x0003bcb0` → `0x0012ce00`). | (c), structural | The render front-end drives streaming: it requests pages, ages entries and moves data into `world_pool` (the render writers of the `world_pool` `rs` pages are the same functions: `0x0012c889`, `0x0012e425`, `0x0012d65d`). A 3-way merge of an allocator both threads mutate is not sound. Make the caches single-owner: the game thread owns them; the render thread reads residency from its snapshot and queues requests, which are replayed at the join. This is the largest item, and the one that decides between hard and infeasible. |
| `0x004e6000` | 1: 14 | **Frame timing.** The frame wait (`0x0012b2e0`, inside `render_frame_and_present`) writes `0x004e63a8..0x004e63bc`: the vblank count at the wait. Frame timing (`0x0012b0e0`) reads it to turn vblanks into `dt`, which becomes the tick budget. Bink's stop path (`0x00156560`, reached from the render through `0x0002c8a0`) writes the flags at `0x004e6388`/`0x004e6389`, which `0x0012b0e0` also reads. | (c) | Tick-visible: `dt` sets the tick count. Run the frame wait on the game thread at the launch, as the design already requires, so its writes go to live memory at the serial point. Turn pipelining off while a Bink movie plays. |
| `0x00485000` | 2: 42-43 | **Present counter and vblank.** `rasterizer_present` `0x00013f10` increments the 64-bit `0x00485aa0`/`0x00485aa4` (copied to `0x00467008`). The tick reads it: `0x0017ef10` (game tick → `0x00176e40` → … → `0x0017ee20`) stores it into a record at `+0x28`. In the same lines: the vblank counter `0x00485ab0` (the host's vblank source writes it, the frame wait spins on it, `0x0012b0e0` reads it), and the render frame-begin fields `0x00485ad4..0x00485ae0` and `0x00485af0..0x00485af4` (`0x00012a50`, `0x00013450`, `0x000151e0`). | (b), exact if done on the sim side | Only the render adds 1 per present. Increment the counter on the game thread at the launch and leave the render's write out of the hand-back; the tick then sees the serial value. The vblank counter is the host's and stays on the game thread. Merge the frame-begin fields byte by byte (only the render writes them). |
| `0x00485000` | 1: 24 | `0x00485600..0x0048563f`: a block the render writes through a pointer (the page's first writer is the frame-begin `0x00012a78`) and many render functions read. Director `0x00039e50` (from `0x00137ed0`) and particle code `0x00248df0` (director → `0x00176f60` → … → `0x00173ba0`) also read it: by the readers, the current view or camera parameters. | (b) | Director and particle code run per frame, not in a tick. A one-frame-old view is what they would get from the previous render anyway, but this has to be confirmed (the director writes camera state the next render reads). |
| `0x004ba000` | 2: 0-1 | `render_views` state (docs/graphics/render-loop.md): the frame context `0x004ba030`, the render frame counter `0x004ba034` (incremented per `render_views`), the arguments `0x004ba048`/`0x004ba04c`, and `0x004ba050` (`render_game_view`). Particle update `0x00248620`/`0x00248750` (director → `0x00176f60` → `0x00174990` → `0x00174a30`) reads the frame counter. In line 1, the director's own double at `0x004ba040` (written by `0x00137ed0`) shares the line with the render's words. | (b); line 1 is (a) | Particles are cosmetic, so a counter one frame late is acceptable. To keep it exact, increment it on the sim side at the launch, as with the present counter. Line 1 is false sharing: merge by byte. |
| `0x00467000` | 2: 3-4 | Rasterizer and pass state. `0x004670e4` (`0x0002c490`, per view), `0x00467130..0x00467138` (each scene and light pass, from `0x0002bcd0`, `0x00031170`, `0x0003fdf0`, …) and `0x004670cc..0x004670e0` (loading screen `0x000289e0`) are read only by render code. `0x004670fc`/`0x00467100` are written by `0x000398d0` (render, under the scene passes) and read by `0x0018ae80` (director → `0x0018c060` → `0x0018bb80`). `0x00467104..0x00467124` is the director's `0x00039e50` reading and writing its own state. | (a), except `0x004670fc` (d) | Merge by byte. Find out what `0x000398d0` publishes to `0x0018ae80` (the next row has the same pair). |
| `0x00509000` | 4: 14-17 | Rasterizer globals (device pointer, render state). `0x005093e8..0x005093f4` are written by the light-probe interpolation `0x00022850` (time-cycled lightmap coefficients, docs/formats/lighting.md) and read by `0x000241c0`/`0x00024490`. `0x00509420` is written by `0x000398d0` and read by `0x0018ae80`, the same pair as `0x004670fc`. The tick's readers in these lines (`0x0003ddd0`, `0x001794e0`, `0x0017b5d0` at `0x00509434`; `0x000439d0` at `0x00509444`) read words the render does not write. | (a) for most; `0x00509420` (d) | Merge by byte. Light-probe values the sim reads are object lighting, which is cosmetic (to confirm). |
| `0x00479000` | 2: 33-34 | **Game-time bracket.** `render_views` calls `0x00146de0` on entry when `0x0047989c` is non-zero, and `0x00146b80` on exit (render-loop.md). They update `0x0047989c`, `0x0047988c`, `0x004798a0` and call the cache's `0x0012d520`. The game-time code that Havok's step reaches (`0x001c3d30` → `0x00146bf0` → `0x00146ac0`/`0x00146fd0`) reads and writes the same words. | (d), likely (c) | This lives next to `game_time_initialize` `0x00146550` and the tick budget `0x00146660`, and is read inside the physics step. Read `0x00146a20..0x00147320` before building anything. |
| `0x00510000` | 1: 49 | The system-pointer table `0x00510c40..0x00510c7f` (`0x00510c54` is the game-time record). The render writes the byte `0x00510c48` (`0x00146b80`/`0x00146de0`, the bracket above) and `0x00510c40`. The sim's many readers read the pointers. | (d), tied to the previous row | Same investigation. The pointers themselves are written only at init. |
| `0x7fce6000` | 1 (2096 frames) | A heap line written by `0x001470f8` in the render scope and read by `0x00146ff4` (from Havok's step): the same game-time module. | (d) | Same investigation. |
| `0x004e6000` | 1: 2 | `0x004e6098`, a one-shot byte flag. Game-state code sets it (`0x00123d10`); the render clears it per view (`0x002231a4`, in `0x00223080`); the geometry cache reads it (`0x0012dd14`). `0x004e6080` (the game-state block pointer) shares the line. | (b) | Apply the render's clear to live memory at the launch (an eager hand-back of a known word), so the sim sees what it would see serially. |
| `0x004e6000` | 1: 37 | `0x004e6948` (the engine-globals pointer, read everywhere) shares the line with words only the render writes, `0x004e6968..0x004e697c` (`0x00200462`, `0x0022bbca`, `0x0024e9c7`, …). | (a) | False sharing. Merge by byte. |
| `0x0054d000` | 1: 22 | UI and progress state around `0x0054d580..0x0054d5bc` (`0x001476c7`, `0x001479c3`, `0x001483c3`). `render_views` sets `0x0054d598` to -1 at frame begin; `0x001483c3` writes `0x0054d59c`, and the UI side reads it. | (a), to confirm | UI only. |
| `0x00484000` | 1: 55 (29 frames) | `0x00484dc0`, written by the pass-end `0x000445d0`. | (a) | Rare. Merge by byte. |
| `0x004e9000` | 1: 9 (1 frame) | Bink state `0x004e9258..0x004e926c`. | (a) | Pipelining is off during Bink. |
| `0x8000b000` | 1: 0 (1800 frames) | The first line of a contiguous allocation. The host writes it in the render scope (our D3D or kernel layer), and a generic copy helper `0x003314b0` reads it. | (d) | Probably device state: it moves with the device. Tag host writes by subsystem to confirm. |

Tally of the 71 lines: 27 are visit marks (dead for the sim if kept out
of the hand-back); 20 are the async I/O pool; 2 are the cache globals; 3
are frame timing and the present counter; 1 is view state (line 24 of
`0x00485000`); 13 are false sharing, render-only words, rare lines or a
one-word fix (`0x004e6098`, the render frame counter); 5 are not
identified, 4 of them around the game-time bracket. Several of the classes are stateful
protocols between the two sides (marks, cache, I/O, timing), not values
read once.

**Lines both sides write** (`ww_core` 35.2 pages a frame). On the line
pages, the overlap of `rw_lines` and `sr_lines` is the table above: the
mark counters and stamps (written by both, resolved by keeping the
render's copy private), the I/O pool (the worker writes completions),
`0x004ba000` line 1, and the `0x00467000`/`0x00509000` rasterizer lines
(different words). Outside the sticky pages the census has no lines. The
`ww` pages in `game_state` (`0x82e04000..0x82e1f000`, first render
writer `0x0003dfb1` inside `0x0003ddd0`, a function the tick also reaches
through `0x001794e0`), `datum:lights` (`0x000c132e`) and the particle
arrays (`0x001747aa`, `0x00174837`) are the next candidates for real
aliasing.

### Other threads inside the render scope

`other_thread_hits` is 48.7 a frame: first touches of pages by another
guest thread's slices while a render scope is open. The census does not
record which thread. Two mechanisms put another thread inside the scope:

- **Preemption.** The kernel preempts on a logical access count
  (docs/runtime/snapshots.md, "How guest threads run"), so the async I/O
  worker, the audio threads and the network threads get slices whenever
  the game thread's budget runs out. The render front-end is three
  quarters of the game thread's accesses, so most preemptions land in it.
  On its own, this is harmless for pipelining: on a second host thread
  those slices would run beside the render, not inside it.
- **Synchronisation the render starts.** Every async I/O request made in
  the render scope does `WaitForSingleObject` on the pool mutex
  (`0x001208b0` → `0x002d63fa`, with an infinite timeout) and then
  `ReleaseMutex` (`0x002d62c4`). The enqueue (`0x001209c0`) signals the
  worker. The worker takes the same mutex when it frees a request
  (`0x00120c40`), so **the render front-end can block on the I/O worker
  through a kernel mutex**. The Bink stop path (`0x00156560`, from the
  render through `0x0002c8a0` → `0x00156040`) reaches thread-priority and
  wait exports. The direct-call graph from `0x00223240` also reaches
  more XAPI and kernel entries (file reads through the cache's
  `0x0021ec00` → `0x001915f0`, critical sections). Static reachability
  over-approximates, so the actual set needs the per-scope HLE call count
  the design already names.

There is no evidence that the render waits on a result from another
thread (an I/O completion, an audio fence) inside its scope. It does take
kernel objects that another thread holds, so on a second host thread it
cannot call the kernel. Those calls have to be overridden (the async I/O
row above) or forbidden. The infeasible case in "Reading the result"
(the render waits on other threads) is not shown. Nor is it ruled out
until the HLE count exists.

### Verdict: hard; feasibility not yet decided

This was the verdict before census v2. "Census v2 results (2026-11-18)"
below replaces it and the build plan.

By the criteria above:

- Not **feasible** as defined. `rs_core` is 46.5 pages, not a handful.
  On the pages that were measured, the dependencies are not one-shot
  cosmetic values: they are three shared protocols (visit marks, the
  texture and geometry caches with the async I/O queue behind them, and
  frame timing) and an unidentified game-time bracket called from
  `render_views`. `sim_d3d_hits` is small, but not zero.
- **Hard**, with a list: each item above is a port with a named writer.
  The marks, the present counter and the frame counter have cheap, exact
  remedies, and the frame wait is already part of the design. The caches
  and the async I/O queue are a real ownership change in engine
  subsystems the tick also uses.
- **Not shown infeasible, and not cleared either.** About 25 `rs_core`
  pages a frame in `game_state`, `world_pool` and the datum arrays have
  no line data (caveat 1). If those turn out to be dense, real overlap
  (the render writing object or light state the tick reads), the
  verdict becomes infeasible, and the native renderer (feature engine
  stages 7b-7d) is the route.

Recommendation: run census v2 (step 1 below) before writing any
pipelining code. It is a feature-crate change and one run; everything
after it is weeks of work.

### Build plan, in order

1. **Census v2** (`crates/feature/src/render_census.rs` only, no image
   change):
   - pick sticky pages per region (for example the top 8 of each of
     `game_state`, `datum:*`, `world_pool`, `other`, after the XBE pages
     already measured), or take an explicit list from
     `H2_RENDER_CENSUS_STICKY`;
   - mark a sim read of a line as a dependency only if the sim had not
     written that line earlier in the interval (caveat 3);
   - split the sim interval's hits into the game thread and other
     threads (caveat 2);
   - in the render scope, count HLE entries by export, and waits by the
     object waited on;
   - tag host writes by subsystem (D3D, kernel, audio).

   Go on to step 2 only if the `game_state`/`datum`/`world_pool` lines
   are few and identifiable.
2. **Runtime prerequisites for guest code on a second host thread.**
   Each of these is needed before any game-side remedy can be tested:
   - **CPU and stack.** A second `ExecCtx`/`CpuAbi`, with its own direct-call
     shadow stack and continuation table. It keeps the game thread's stack
     addresses, since its snapshot pages are private.
   - **Memory.** A second `Guest` over a `mach_vm_remap(copy)` mapping of
     the 64 MB guest RAM, with its own fast page table
     (`Guest::prepare_fast_pages`) and its own `MemoryShared` gate. Today
     the generated code assumes one guest thread holds the gate at a time
     (`H2_GATE_DROP`); the two runtimes must never share one.
   - **Caches.** Per-thread copies of the 4096-entry resolve cache, the
     remembered execute check and the direct-call target caches. The
     alternative is to make them read-only after warm-up and audit every
     write. Either way, check that none of them is process-global.
   - **No kernel on the render thread.** No preemption: the
     `H2_LOOP_POLL`/`H2_REP_POLL` safe points must not hand the thread to
     `kernel::Machine`. No interrupt or DPC delivery, and no kernel
     objects. An allowlist of HLE entries: the D3D device entries, and
     time reads answered from a value captured at the launch. Anything
     else stops the render thread at that call and hands the call to the
     game thread at the next join (an RPC through a slot). That stays
     correct, but it serialises that frame.
   - **D3D device.** It is driven from the render thread under its
     existing lock. The sim's own D3D calls (`sim_d3d_hits`; identify the
     about 0.1 a frame) are queued in order behind the render's. Swap
     goes through the device as today. The frame wait (`0x0012b2e0`) is
     taken out of the render thread and runs on the game thread at the
     launch.
   - **Logical clock.** The render thread counts its accesses into its
     own counter. The join charges them to the game thread's clock at a
     fixed point, so the schedule stays a function of the inputs.
     Pipelined and serial runs are different modes for lockstep and
     co-op.
   - **Snapshots and agent pause.** These join first; a test pins that
     a snapshot never sees a live render `ExecCtx`.
3. **Game-side remedies**, cheapest first. Each is tested by comparing a
   pipelined run against a serial run with a tick-state hash that leaves
   out the excluded words. The raw simulation hash differs by design: the
   mark counters take different values.
   1. Mark counters and stamp arrays (`0x004de2f8..0x004e0300`,
      `0x004e0304..0x004e030b`, `0x004e7411..`, `0x004e7c18..` and their
      stamp ranges) stay render-private.
   2. The present counter `0x00485aa0` and the render frame counter
      `0x004ba034` are incremented on the game thread at the launch.
      `0x004e6098` is cleared at the launch.
   3. The frame wait runs on the game thread; pipelining is off while
      Bink plays.
   4. The game-time bracket (`0x00146de0`/`0x00146b80`, `0x0047988c..`,
      `0x00510c48`, the `0x7fce6000` line) is identified and moved to the
      game thread at the launch, if its effect is only the bracket.
   5. Texture and geometry caches with async I/O: the render thread's
      requests (at the cache request functions under `0x0012c..`/`0x0012d..`,
      or at `0x001208b0`/`0x001209c0`) go to a host queue that the game
      thread replays at the join. Cache bookkeeping is written only on the
      game thread; the render reads residency from its snapshot. If the
      render's per-frame cache writes cannot be split this way, stop
      here.
   6. Whatever census v2 finds in `game_state`/`datum`/`world_pool`.
4. **Hand-back.** Copy render-only pages back whole. Merge `ww` pages
   byte by byte against the snapshot, leaving out the exclusions from
   step 3. Drop the stack.
5. **Measure.** COW fault count and cost against the 6-8 ms the sim
   costs; the time a frame saves; the join wait.

## Census v2 results (2026-11-18)

One run: Old Mombasa, headless, 3300 frames, census v2 with
`H2_RENDER_CENSUS_LINES=64`. The log and TSV stay outside the repository.
The writers and readers were identified the same way as in the first run,
plus field offsets taken from the generated C at each logged pc.

### Numbers

Per frame, averaged over the run:

- **Coverage.** Line data covers 81-93% of `rs_core` pages in each group
  (`xbe`, `game_state`, `datum`, `world_pool`, `other`). Caveat 1 of the
  first run no longer applies.
- **Pages.** `rs_core` 44.1, and 39.5 of those have line data;
  `sim_written` 147.9; `rs_other` 0.1.
- **Ordered dependency lines.** `dep_lines` is 74.1 when a sim read counts
  only if the sim had not written the line first. Counting every read, it
  is 150.4, so half the first run's lines were the sim reading its own
  writes (caveat 3).
- **Other threads.** Thread 3 makes 2-4 hits a frame in each scope, and
  hits with no thread id (host-side touches with no guest thread running)
  make 8.4 in the render scope. `rs_other` is 0.1 a page a frame, so the
  48.7 of the first run were preemption slices, not dependencies (caveat 2).
- **`sim_d3d_hits`.** 502 over the whole run.

### The new dependency sites

The XBE lines were classified in the first run's table. They are the
marks, the async I/O pool, the cache globals, the frame timing, the
present counter and the game-time bracket. These are the sites the
per-region sticky pages added, biggest first. Offsets are fields within
the record the instruction addresses.

| writer (render) → sim readers | lines, line-frames | what | class | remedy |
|---|---|---|---|---|
| `0x0003dfb1` (in `0x0003ddd0`) → `0x000bc50d` (`0x000bc470`, object update), `0x001a588b`, `0x001511f0` | 25 / 29 / 13 lines over about 3000 frames | **Object render-state handle.** `0x0003ddd0` stores a cached render-state handle at object `+0xcc` and stamps it with the render frame counter `0x004ba034`. The sim readers load `+0x68`, `+0x0c` and `+0xaa`, never `+0xcc`; object records are not 64-byte aligned, so these are other fields, often of a neighbouring object, in the same line. | (a), false sharing | Render-owned field. Merge `game_state` pages byte by byte; the render's `+0xcc` write never meets a sim write. |
| `0x000c132e` (in `0x000c12d0`, from `0x000c15a0` ← `0x0002ba10` under `render_views`) → `0x000c3238` (`0x000c3220`), `0x000c0ca7` (`0x000c0c80`, light update from the tick through `0x000c1400`) | 12 / 7 lines, every frame | **Lights.** The render writes per-light render fields at `+0xf8..+0x108` of the 0x110-byte light record (`0x000c12d0` is the only function that touches them). The tick's light update reads `+0x02` and `+0x4c`. With a 0x110 stride, `+0x100` of one light shares a line with `+0x02`/`+0x4c` of the next. | (a), false sharing | Byte-level merge of `datum:lights`. The fields are render-only. |
| `0x001747aa`, `0x00174837` (in `0x00174420`, from `0x00174220`/`0x00176cb0` ← the object and effect draw paths `0x0003fdf0`/`0x00040e90`) → `0x00179128` (`0x00179020` ← `0x001792b0` ← `0x00176e40`, the effect update in the tick), `0x0017531c` (`0x00175270`), `0x00248756` (`0x00248750`; both under `0x00174a30`) | 35 lines over 1820 frames, 14, and 64 over 1579 | **Particle systems.** The render pass updates particle systems as it draws them: `0x00174420` clears a flag bit at `+0x0d` and rewrites `+0x10`, `+0x30` and `+0x44` in `datum:particle_system`, `particle_emitter`, `particle_location` and `effect location`. The sim's effect and particle updates read and write the same fields (`+0x0c`, `+0x10`, `+0x30`, `+0x44`). This is real two-writer state, not false sharing. | (c), cosmetic | Make the four particle datums render-owned. The sim's particle-side updates (`0x00174a30` and `0x00179020`'s particle work) run on the render thread at the launch, against the sim's committed state, and effect creation and deletion reach them through a queue. Particles do not feed gameplay, so the order change is visible only as a one-frame shift in particle motion. Until that exists, a pipelined frame has to skip these datums in the merge and accept conflicts, which is not correct. |
| `0x0012387a` (in `0x001237e0`, `animation_resource_get`) → `0x0012377e` (`0x001236f0`, `animation_resource_request`, from the animation code in the tick) | 25 lines over 2835 frames | **Animation cache.** The render's get updates the cache entry's use stamp (`+0x14` + 8 × index) in the cache records at `0x7ff60000`; the tick's request reads the header at `+0x64` in the same lines. When the data is not resident, the get loads it and waits (`0x001237a0`, a `SwitchToThread` loop). | (c) | Same as the texture and geometry caches (step 3.5 of the first plan): cache bookkeeping is written only on the game thread, the render's stamp updates and requests are queued and replayed at the join. |
| host, `other` (3 lines); `0x002df76e` → host | small | **Per-thread kernel scratch.** The pages at the top of the thread stacks (`0xa1005000`, `0xa1048000`, `0xa1049000`) and `0xa1001000`, which the host's kernel code writes for the calling thread. | (a) | The render thread gets its own (the second `ExecCtx` already needs its own stack and TLS). |

Smaller lines (each under 10 lines a frame): the cached object
render-state records on the heap at `0x7fe4....` (`0x0016b840`,
`0x0016bc00`), read by the geometry cache (`0x0012d1aa`, `0x0013d4a8`)
when heap blocks are reused, are (a) with the same byte-level merge; the
world_pool lines from the texture cache writers (`0x0012c889`,
`0x0012e425`, `0x0012d65d`) are part of the cache ownership change (c).

### Waits inside the render scope

In the render scope, by kernel export:

- **`NtYieldExecution`: 8.9 calls a frame, all on the game thread, 2677
  of them (0.8 a frame) stopping the thread.** Two kinds of code yield:
  - **The frame wait.** `0x0012b2e0` runs inside
    `render_frame_and_present` and is overridden by the d3d crate's clocked
    frame wait (`FrameWait::HALO2_CLOCKED`, `crates/d3d/src/vblank.rs`). It
    calls `NtYieldExecution` through `call_guest` until the vblank counter
    reaches its target, so the other threads run and the logical clock
    moves to the next vblank. That is logical-clock pacing, not a spin on
    the GPU or the push buffer, and it leaves the render thread anyway
    (the design runs the frame wait on the game thread at the launch).
  - **Guest `SwitchToThread` loops.** XAPI's `SwitchToThread`
    (`0x002d2084`) calls the same export. The ones reachable from the
    render front-end (`0x00223240`) are the async I/O pool when it is
    full (`0x001208b0`), the texture cache (`0x0012c990`, `0x0012de70`),
    the cache drain at the end of drawing (`0x0012d400`, which calls
    `CDevice_KickOff` and yields while reads are pending, within a time
    budget from `0x00223bb0`), the animation cache load (`0x001237a0`),
    and a few more under `0x00120d80`, `0x001224c0`, `0x00140f00..0x00141350`,
    `0x001a0af0` and `0x00218850`. Each waits for the async I/O worker to
    finish a read.

  The census counts calls by export, not by caller, so it cannot split
  the 8.9 between the two. The stops say most yields return at once.
- **No GPU spin.** `D3DDevice_IsBusy` is called 253 times a frame and never
  stops: the host device answers at once, and no guest loop around it
  yields. No D3D entry in the render scope stops the thread.
- **`NtWaitForSingleObjectEx`: 1.9-4.4 calls a frame on the game thread,
  0-1 stops in the run.** These are the async I/O pool mutex
  (`0x001208b0` → `0x002d63fa`) followed by `NtReleaseMutant`. The mutex is
  never contended in practice, but it is a kernel object the I/O worker
  also takes. The other threads' waits in the render scope (9.1 a frame,
  about 200 stops) are the I/O worker and audio threads on their own
  semaphores during preemption slices.

So the render waits on another thread in two places: the cache drain
`0x0012d400` and the animation load `0x001237a0`, both waiting for the
async I/O worker to finish a read the render asked for. This is the
"render waits on other threads" case from "Reading the result", but it
is narrow. In steady play the data is resident, and the drain has a time
budget.

### Verdict: hard, feasible with ports

By the criteria in "Reading the result":

- **Not feasible as it stands.** `rs_core` is 44 pages and 74 ordered
  lines a frame, not a handful.
- **Not infeasible either.** With 81-93% of pages measured, no line shows
  the render writing gameplay state the tick reads. Every dependency is
  one of:
  - false sharing, which a byte-level merge removes: object `+0xcc`,
    light render fields, the heap render states;
  - render-private bookkeeping: the visit marks and the per-thread kernel
    scratch;
  - a value the game thread can produce at the launch: the present
    counter, the render frame counter and the frame wait;
  - one of three subsystems the render owns or shares by design: the
    texture, geometry and animation caches with the async I/O queue
    behind them, and the particle systems.
- **Hard.** The subsystems are real ports. The caches need a request
  queue and a single writer. That covers the drain and the animation load
  that wait for the I/O worker: the render thread has to get the data
  from the game thread or skip the draw that frame. The particle systems
  need the sim's particle updates moved to the render thread. The
  runtime prerequisites (step 2 of the first plan) are unchanged and are
  the largest single item.

The game-time bracket (`0x00146de0`/`0x00146b80`) from the first run is
still not identified. It is the one open item that could still be real
sim state.

### Build plan, revised

1. **Done:** census v2.
2. **Optional census tweak, before any code:** attribute wait-class HLE
   calls to the caller, using the guest return address, or the override
   name for the frame wait. This splits the 8.9 yields a frame between the
   frame wait and the cache waits. It also says how often the drain and
   the animation load wait at all, which decides whether "skip the draw"
   is acceptable. A second run would also name the game-time bracket's
   readers.
3. **Runtime prerequisites** (step 2 of the first plan, unchanged): a
   second `ExecCtx` and `Guest` over a COW remap, per-thread caches, no
   kernel on the render thread with an HLE allowlist (D3D entries and
   captured time), the frame wait on the game thread, the logical clock
   charged at the join, and snapshots joining first. The second executor
   work (docs/runtime/second-executor.md) covers part of the first item.
4. **Hand-back with a byte-level merge** (step 4 of the first plan, moved
   earlier). Copy render-only pages, merge `ww` pages byte by byte, drop
   the stack. This retires every false-sharing line (object `+0xcc`,
   lights, heap render states) without any game-side change, so test it
   first. Compare a pipelined run against a serial run with a tick-state
   hash.
5. **Cheap game-side remedies:** marks render-private, the present and
   frame counters on the game thread at the launch, pipelining off during
   Bink.
6. **Cache ownership:** the texture, geometry and animation caches and the
   async I/O pool get a single writer on the game thread. The render's
   requests and use stamps are queued and replayed at the join. When the
   render needs data that is not resident, it draws without it that frame
   instead of waiting (drain `0x0012d400`, animation load `0x001237a0`).
   This is the biggest port, and the first one with a visible cost: a
   frame without its texture or animation pose for one frame after a
   load.
7. **Particle ownership:** the particle system, emitter, location and
   effect-location datums become render-owned. The sim's particle updates
   run on the render thread, and effect creation and deletion are queued
   to it.
8. **The game-time bracket**, once identified: move it to the game thread
   at the launch, or stop if it is real sim state.
9. **Measure** (step 5 of the first plan).

Steps 4 and 5 make a pipelined run possible with caches and particles
still serial: the render could run ahead on frames with no cache misses
and no particle updates. That does not happen in real play, so the
speedup only comes after steps 6 and 7. The native renderer (feature
engine stages 7b-7d) remains the alternative if steps 6 and 7 cost more
than they save.

## The prototype (`H2_PIPELINE_RENDER=1`)

Build plan steps 3 (the cheap remedies), 4 (hand-back) and 5 (measure),
as `crates/feature/src/pipeline_render.rs`, wired in the boot by
`d3d_glue::pipeline_render`. Off unless `H2_PIPELINE_RENDER=1`; off under
lockstep (`H2_LOCKSTEP`, `H2_INPUT_RECORD`, `H2_INPUT_REPLAY`) and with
`render-census`. No image change: it needs a new `h2` binary only.

**The frame.** An implementation at `render_frame_and_present`
(`0x00223240`, which is: world path `0x002234e0` or fallback `0x002232a0`,
then the frame wait `0x0012b2e0`, then `rasterizer_present(0, 0)` when
both resource-cache countdowns `0x004e6470`/`0x004e64a0` are `<= 0`; a
plain `ret`). On the game thread, at each entry:

1. **Join** the render launched at the previous entry and hand it back
   (below). Charge its guest accesses to the game thread's slice
   (`ExecCtx::charge_accesses`), so the logical clock sees them at this
   fixed point.
2. **Decide.** Serial (the original, on the game thread) when the world
   path would not be taken (the three tests at `0x00223240` and
   `0x00068290` read in Rust: menus, loading screens, Bink), when a
   countdown runs (loading), or after a quiesce request.
3. Otherwise: the **frame wait** here (`call_guest` to `0x0012b2e0`, the
   D3D layer's clocked wait), then **snapshot** (`SecondExecutor::snapshot`,
   two COW copies), then the **launch remedies** on live RAM: the 64-bit
   present counter `0x00485aa0` + 1 (the predicted present), the one-shot
   byte `0x004e6098` cleared. Then **launch** `0x00223240` (the generated
   original) on the render host thread with the entry's registers and its
   own 256 KiB guest stack from the feature arena, and return to the game.
   The stack is as large as the game thread's own (the main thread's size
   in the XBE header), because the front-end may use as much as it does
   serially. The first build gave it 128 KiB, and the first world frame
   crashed: a function under the render probes (`_chkstk`, `0x00320560`)
   a frame of over 128 KiB, and the probe read the unmapped page under the
   render stack (`MemoryFault ... Read, Unmapped` about 128 KiB below
   `esp`). A fallback runs the rest of the render on the game thread but
   still on this stack, so the same limit holds there.

On the render thread the executor's dispatch has the generated original
at `0x00223240` and a no-op at `0x0012b2e0`; it may run the D3D layer's
overrides (the entries, the write barrier's and CreateDevice's shadow
pages, the port I/O stubs). Everything else (kernel imports, XAPI, other
features' hooks, plan chains) is refused, which ends the render there.

**Hand-back** (join of a completed render): the render stack pages are
dropped; the visit marks (`0x004de2f8..0x004e0300`, `0x004e0304..`,
`0x004e7411`, `0x004e7418..0x004e7c18`, `0x004e7c18`, `0x004e7c20..0x004e8000`)
stay render-private; their counters (`0x004de2fc`, `0x004e0308`,
`0x004e7414`, `0x004e7c1c`) get the render's delta (see "The dark
first-person model" below); the present counter gets the render's delta less the
prediction (a difference is a `mispredict`); the render-frame counter
`0x004ba034` gets the render's delta (a frame late for particles, as the
census row allows); `0x004e6098` and the host-owned vblank words
(`0x00485ab0..0x00485ac0`, `0x0055e6b8..0x0055e6c0`) never come back.
Every other byte the render changed is merged byte by byte against the
snapshot (`Guest::hand_back`: a render-only page comes back whole, a page
both wrote keeps the sim's bytes the render did not change, and an aligned
word both changed keeps the sim's value whole and is a counted conflict,
because in serial order the tick runs after the render; in the
render-owned ranges, the D3D device and its push buffer, the render's word
wins instead). The stamp array
from `0x004e7c20` is assumed to end at the page (the census saw up to
`0x004e7d7f`); its real end is still to read.

**D3D.** The D3D handlers read guest memory through `ctx.memory()`, so on
the render thread they read the snapshot and write the view by
construction. The write barrier drains its watch by id; the executor
takes the live Guest's `boot:d3d` subscription over at each snapshot
(`Guest::adopt_watch`: same id, the sim's undrained pages moved to the
view), so the device hears of the sim's writes and the render's own. The
sim's D3D calls on the game thread (the `0x3f…` entries, wrapped at the
first frame) join the render first; one from another guest thread is not
ordered (`d3d_other_thread`).

**Serviced calls.** The async I/O pool's lock (`NtWaitForSingleObjectEx`
on its mutex, `NtReleaseMutant`, `NtReleaseSemaphore` to wake the
worker; `SERVICED`) does not fall back. At the join the game thread
makes the call itself, on its own context (the kernel sees the thread
the render runs for serially), with the render's stack copied to live
RAM from `esp`'s page up and the pool's bytes (`Addrs::live_bytes`,
`0x004e0354..0x004e28ec`) brought in step: the render's changes go to
live RAM, live RAM's bytes come back into the view. The stack comes back
too (the semaphore's previous count is a local), and the render carries
on on its own thread (`SecondExecutor::resume`, a return to the call's
return address) against its snapshot. The join waits for it and settles
again: it completes, or makes another serviced call, or falls back. The
pool is left out of the final merge. While the render holds the pool
lock no guest thread runs, because the game thread is waiting in the
join, so the render's pool changes and the worker's never interleave.
This is what the async I/O row of "Every dependency line" needs, without
the request-queue port. The frame costs its render time on the game
thread from the first serviced call on.

Why it matters: the first build made these calls fallbacks, about 40% of
gameplay frames in Old Mombasa. A fallback runs the rest of the render
against live RAM, which is a tick ahead of the frame the render started
on. At present 2803 the frame's object list (built against the snapshot)
named an object the tick had since deleted, and the object render setup
(`OBJECT_RENDER_SETUP` `0x00133790`) read the body's sphere radius
(`+0x3c`) through the null body pointer: `MemoryFault ... addr: 3c`, on
the render stack, on the game thread.

**Flushes.** A serviced call can wake another guest thread that runs
before the call returns: the semaphore release wakes the I/O worker
(`0x00120c40`), which takes the queue head `0x004e28ec` and calls the
request's handler (`[request+0x34]`) on its fields. Those fields are
copied from the requester's stack (`[request+8..+0x30]`) and point at
the requester's memory: the file name, a handle out-pointer at
`+0x14`, the completion byte at `[request+0x38]` that the worker sets
to 1. So before each serviced call everything the render wrote so far
goes to live RAM (`SecondExecutor::flush`, against the bytes each page
had at its last flush; the marks, the counters, the host-owned words and
the live ranges are left alone). Before the final hand-back,
`SecondExecutor::unflush` puts every byte unchanged since its flush back
to the snapshot's value in the view, so the merge leaves what live RAM
did with it since (the worker's completion byte) alone. The first build
synced only `0x004e0354..0x004e28ec`, one word short of the queue head:
the worker read a null head and crashed at `0x00120c79`
(`call [esi+0x34]`, `addr: 34`) on the first pipelined frame.

**Fallbacks.** A fallback is still unsound in that way, so any refused
call other than the serviced ones should be rare. A refused call does
not rerun the frame: draws already
made would be made twice and the device's own state could not be rolled
back. Instead everything the render wrote is handed back (its stack too;
the mark counters take the larger value, the marks themselves are
merged), the present counter loses its prediction, and the game thread
makes the refused call with the render's CPU at that call
(`SecondExecutor::refused_cpu`) and runs the rest of the render through
the generated code's resume points; the render's last `ret` lands in a
continuation that restores the game thread and carries on (the next
launch, or the sim's D3D call). The frame wait inside it is skipped (it
ran at the launch). A refusal beneath a Rust continuation on the render
thread (a D3D push callback, a destroy delivery) cannot move to the game
thread's context: it is counted as `unresumable:<call>` and the run stops
with that name. Faults stop the run as they would serially.

A fallback after serviced calls had two more faults, fixed with the
crash at present 1825 (a render-thread fault in object code at
`0x00103823` after two `MmSetAddressProtect` fallbacks in the level's
load burst). The render stack went back by the byte merge against the
snapshot, but a serviced call had already copied it to live RAM, so a
return address equal to the snapshot's kept live RAM's stale bytes and
the rest of the render returned to the wrong place. The stack now goes
to live RAM whole at a fallback. And pages flushed earlier and not
stored to since were missing from the fallback's hand-back, so what the
flushes leave out (the visit marks) never came back. They are now
handed back with the rest.

**Protection changes.** `MmSetAddressProtect` is serviced too: the
game thread makes it, and each page of its range gets live RAM's
permissions in the view, so the render's next access sees what it would
serially.

**Guards.** The decision adds three serial reasons, counted in the
`serial [...]` list:

- `warmup`: the first `Addrs::warmup` world frames (150) after a frame
  that was not one (a level load, a menu, a loading screen). The load
  burst of cache requests is over before the first launch.
- `io_busy`: the async I/O queue head (`0x004e28ec`) is not empty.
- `after_fallback`: the `Addrs::after_fallback` frames (30) after a
  fallback.

**Snapshots and pause.** The feature's snapshot policy reports a render
in flight and asks for the next frame to run serially, so a snapshot at
the next tick boundary after it passes.

**Output.** Once a second, from the game thread:

```text
pipeline: frames pipelined 58 completed 41 serial [loading 2] fallbacks 17 [kernel:<export> 12, <override> 5] serviced 40 [kernel:NtReleaseMutant 20, kernel:NtWaitForSingleObjectEx 20] d3d_joins 0 d3d_other_thread 0 join_wait 11.20ms hand_back 0.40ms snapshot 1.10ms render 25.70ms pages 231.0 conflicts 3 mispredicts 0
```

(an illustration of the shape, not a measurement). `completed` frames
ran entirely on the render thread; `fallbacks` by the call that stopped
them (`kernel:` for kernel imports, else the override's name);
`serviced` the calls the game thread made for the render; times are
per pipelined frame: the game thread's wait at the join, the hand-back,
the snapshot, and the render thread's run; `pages` handed back per frame.

The line is printed at a decision, at most once a second, so a run that
stops less than a second after the warm-up ends shows only serial lines
although frames were pipelined. A line at each switch says which kind
ran last: `pipeline: pipelining from here (0 pipelined, 150 serial so
far)` and `pipeline: serial from here (...)`. A kernel call in the stop
dump whose return address is a continuation token (`0xb...`) and whose
arguments point into the render stack (in the feature arena) is a
serviced call, so it is also a sign of pipelined frames.

**Watching words** (`H2_PIPELINE_WATCH=0x485a80,0x4e034c`). The first
sight of each listed guest word prints `pipeline: watch <addr> = <value>
(its page mapped|unmapped) first seen after <step>`, and every change of
the value, or of whether the page it points at is mapped, prints
`pipeline: watch <addr> <old> -> <new> after <step>`; at most 200 lines.
With `H2_PIPELINE_RENDER` unset the watch still runs, at each
`render_frame_and_present` entry (`the game thread ran (pipelining off)`),
and nothing else of the feature is installed (no render stack in the
arena): the run to compare with. A step says who changed it: `the game
thread ran (at a frame / before the hand-back / before a service)` is the
game or another guest thread; `a serviced kernel call` is the kernel
call; `the frame wait and the launch` covers the launch's counter writes;
`the hand-back`, `the hand-back before a fallback` and `a service's stack
copy, push and flush` are merges of the feature's. It is the switch that
settles whether a bad global came from a merge.

From the first check on, the watched words' pages also carry a write trap
on live memory (`Guest::set_trap_focus`, `crates/runtime/src/trap.rs`):
every write over a watched word through the checked path prints at once
`trap: focus <word> = <value> written by pc <pc> (<addr>+<len>, thread
<guest thread>, hle <handler>, host thread <name>)` (400 lines at most),
so a write in the middle of a frame that is followed by a crash before
the next check is still named. `pc 0` is the host (HLE, a Rust port);
`hle` is then the kernel export or override running. Not seen: raw
physical writers (the feature's merges, which the step lines cover). The
trapped pages stay on the slow path for the rest of the run.

Run 7 (2026-11-18) had `0x00485a80 = 0x804d9f58` at every check while the
game thread faulted at `0x1958f` with `edx = 0xfbdc19a8`, which `mov
edx,[0x485A80]` at `0x19585` loaded in the same serial frame, the first
after 6 pipelined ones (no other branch reaches `0x19585..0x19599`; the
jump table at `0x19b68` enters the case at `0x19581`). So the word was
written in that frame before the read and the crash came before the next
check: the focus trap names that write.

**The view stack leak (fixed).** The trap named `pc 0x2bc19` on the game
thread: the `rep movsd` that pushes a view, copying a 0x298-byte record to
`0x004850d0 + 0x298 * depth` after `inc [0x0046701c]` (popped at
`0x13c44`; pushed also at `0x2c736`, `0x2c83d`, `0x2cace`). At depth 3
the record covers `0x00485a80` and the globals after it. The depth had
leaked one: a pipelined render pushed a view, made a serviced call (whose
flush gave live RAM the pushed depth), popped the view after it, and its
hand-back, comparing the view with the snapshot, saw the depth at its
snapshot value and left live RAM at the pushed one. Every byte a render
changed and put back after a flush did the same. The join now flushes one
last time against what the earlier flushes gave live RAM before the
hand-back, so live RAM ends at the render's final bytes; the render stack
is left out of that flush (a fallback copies it whole from the view).

**Render-safe hooks of other features.** The HUD's text reaches the string
lookup `0x001a0180` on the render thread, where `community:strings` (and
`community:descriptions` at `0x0019fd00`) were refused: after a cinematic
every pipelined frame fell back there. Both act only on the context they
run on (registers, stack, the caller's buffer, the original), so they run
there now.

**Render-safe hooks.** After that, `plan:LOCALE_FIND[engine-leaves]` was
the steady-state fallback: one or two a second after the cinematic, each
followed by `after_fallback` serial frames. Rather than allowing one hook
per run, an override can now be marked context-only
(`Dispatch::mark_context_only`): it acts only on the context it runs on
(its registers, guest stack and the memory that context sees, plus host
state it only reads) and returns without a Rust continuation. The
executor's fork keeps every marked override, whatever `allow` says, and a
new override at the same address clears the mark. Plan hooks say it on
declaration (`HookRef::context_only`). The plan marks a chain when every
hook in it says so and nothing runs after the body (`Chain::render_thread`):
an `after` or `observe_after` hook means the body is called through the
shadow with a Rust continuation, which a fallback cannot resume.

Marked now:

- `engine-leaves`: every adapter, including `LOCALE_FIND` and
  `LOCALE_EXISTS`. They are functions of the registers and the guest
  memory they are given. `LOCALE_FIND` also asks the plan's string
  sources, which answer with addresses from state set at boot (or, for
  co-op's lobby line, state the game thread sets and the render only
  reads).
- The `locale` bridge's `before` on `LOCALE_FIND`, when engine-leaves is
  off.
- `skulls-mp`'s Blind hooks on `HUD_SHOWN` and `FIRST_PERSON_RENDER`. They
  read the skull set fixed at the game's start and answer through
  registers and stack.
- `community:strings` and `community:descriptions`. These are direct
  overrides that mark themselves; the old address-and-name list is gone.

Everything else is in `boot::render_thread::REFUSED`, with its reason:

- `render-timing`, `render-loop` and `render-census` are off by default.
  Timing writes the game thread's frame table and needs after
  continuations. The render-loop ports park in Rust continuations around
  their guest calls. Pipelining is off while the census is on.
- On `RENDER_FRAME`, the pipeline's own implementation and map-slots'
  observer run on the game thread.
- Game-thread points have after parts: health, precision, level-boot,
  aim-assist, skulls (tick, stock), skulls-mp (start, Famine), the custom
  skulls, touch glue and the Controls row text. Skulls' `OPTIONS_QUEUE`
  writes the settings store.

A boot test builds the plan from every feature boot adds, with every hook
switch on. It fails on a hook that is neither context-only nor listed, and
on a listed entry that nothing places any more. Direct overrides outside
the plan (layouts, d3d, kernel wrappers) are not covered: the render thread
keeps only the D3D entries, their shadows, the port I/O stubs and marked
overrides, so any other override it reaches is still a fallback.

**The object render-state cache (dark first-person weapon).** With 0
fallbacks, pipelined frames drew the first-person rifle and hands, and
another object, dark: unlit, against lit brown serially. The world and the
HUD were right. The cache is `0x0003ddd0` (object handle in, record handle
out). Records are 0x100 bytes in the data array at `[0x00509434]`: the
owner's handle at +4, the frame it was lit at +8 (`0x0003dfe0` relights
when that is not the frame counter), the last use at +0xc (for eviction
when the array is full), the lights at +0xa8..+0xb8 (`0x0003dd10`) and the
lighting at +0x44.. and +0xc0... Object body +0xcc names the record. A
lookup whose record is still the object's only reads. Otherwise the
function takes a free record (`0x0016b840`) or evicts the least recently
used, initializes it and lists its lights.

Both sides call it: the render for every object it draws (`0x0003db00`
from the object draw `0x0004b5d0`), and the tick's effect updates for an
effect's object, to read its lighting (`0x001794e0`, `0x0017b5d0`; they
read +0x28/+0x2c). Census v2 saw the sim's `0x0016b840` in these records
and filed it as false sharing. It is two allocators in one array. While a
render is in flight, both take the same free record from the same
snapshot. The hand-back keeps the render's bytes where they differ from
the snapshot and the sim's everywhere else, so the record carries the
render's owner with a mix of both objects' lights and lighting. The owner
check then passes every frame, and the object is drawn with that record
until it is evicted.

The fix is an ownership rule like the sim D3D calls': on the game thread,
a call to the cache that would allocate joins the render first. The test
runs on live RAM, before the call: the body's +0xcc is -1, or its record's
owner is another object. A lookup that finds its record only reads and
does not join. `Addrs::shared_caches` names the cache, and
`cache_joins` in the `pipeline:` line counts the joins. Steady-state effects
whose objects are already drawn find their records, so the joins should be
rare. A high count would mean the sim allocates every tick (for example,
objects that are never drawn and are evicted each frame).

The rerun had `cache_joins 0` and the rifle still dark, so this cache is
not the cause.

**Bisecting switches.** These are read at boot. All default to the
behaviour described above.

- `H2_PIPELINE_PREDICT_FRAMES=0`: the launch does not credit the frame
  counter. The sim's tick reads it one short, as before that change, and
  the join adds the render's views.
- `H2_PIPELINE_SEED_REPLAY=0`: the seeds merge as bytes, with the render's
  value winning, as before that change.
- `H2_PIPELINE_REFUSE=<hex>,...`: the render thread refuses these guest
  functions. A render that reaches one falls back there and finishes on
  the game thread. A refusal makes the rest of that frame serial, so it
  tells you something only when the function is reached late in the frame.
- `H2_PIPELINE_CONFLICTS=1`: tallies each hand-back's conflicts by word
  (bytes both sides changed, to different values). Every 300 hand-backs
  it prints the 32 most frequent, with their physical and virtual
  addresses and the last live, render and snapshot bytes.

The bisect at afde6702 ruled out the prediction, the seed replay and the
lightmap sample: `H2_PIPELINE_REFUSE=0xd1850` gave 0 fallbacks, so no
relight sample runs on the render thread in steady state. The `pipeline:`
lines of that run report about 21,000 conflicts a second, roughly 350
bytes a frame, against single digits in the first prototype runs. Two-writer state on
that scale is the next lead for the unlit dynamic objects.

**Conflicts follow serial order.** The conflict log of 2026-11-18 showed
where those bytes are:

- `0x00504410..0x005044d0` is the animation sampler's scratch: frame
  indices clamped to the animation's frame count (`+0x14`), blend fields at
  `0x00504464..0x005044b0`, the codec selector at `0x00504450` read by the
  dispatch at `0x00279860`, temporary vectors at `0x00504410` and
  `0x00504430`, and a pointer at `0x005044c0`. The writers are the animation
  evaluation `0x002798a0`, the codec readers in `0x0027a6e0..0x0028d000`
  (thousands of references to the scratch), `0x001c73a0` and `0x001e12b0`.
  The tick runs them for the animation update and the render runs them for
  the first-person model. It looks like per-call scratch, and if it is, the
  final value is all that matters. That has not been checked.
- The heap words in `0x82e055xx..0x82e14cxx` (contiguous memory, also
  mapped at `0xf2e...`) repeat at a 0x34-byte stride, the size of a
  `real_matrix4x3`. They are probably node matrices that both sides write.
  Their writers are not traced yet.
- `0x004dc2xx..0x004dc3xx` are not identified yet.

The hand-back used to merge byte by byte, with the render's byte winning.
A word both sides wrote then came back mixed: the render's bytes where it
changed them and the sim's elsewhere. A float or pointer made from two
writers is a value neither side wrote, and the next render and tick read it
(a node matrix that is half one pose and half another, for example). In
serial order render k runs before tick k + 1, so the sim's write is the
later one. `Guest::hand_back` now keeps the sim's whole word wherever both
sides changed it, and takes the render's bytes only in words the sim left
alone. That is the serial run's result for every word, whichever side
owns it. The rig test
`a_word_both_sides_wrote_keeps_the_sims_write_whole_and_is_reported`
fails at its serial comparison under the old byte merge.

The flushes the serviced calls make (`Guest::merge_page`) still let the
view win per byte. A flush hands the render's bytes over in the middle of
the frame, before the sim's later writes, so the next hand-back's word
rule applies to anything the sim writes after it.

**Render-owned state.** With the sim's word winning everywhere, a run
stopped at present 3917, after 1713 pipelined frames, inside the render:
`D3DDevice_SetRenderState_Dxt1NoiseEnable` read a push-buffer put pointer
(device `+0`, `0x00404fe0`) behind what the host had already consumed. The
previous hand-back had found the put pointer changed on both sides and kept
live RAM's older value. Between a render's launch and its join only the
render may change the D3D device, because the sim's D3D calls join first.
So a live change there is not a later serial write, and the render's word
has to stand. `Addrs::render_owned` lists the device (`0x00404fe0`, 0x928
words) and the device pointer `0x00407488`. Boot adds the push buffer and
the device's surface scratch (`PipelineRender::render_owned`, from
`PUSH_BASE` to the end of the scratch page). In those ranges a word both
sides changed takes the render's whole word (`Guest::hand_back_owned`).
It still counts as a conflict and is also counted as render-owned (the
`(render-owned N)` after `conflicts` on the `pipeline:` line). The rig test
`a_render_owned_word_the_sim_also_changed_keeps_the_renders_value` checks
both rules in one run.

Which live-side write changed the put pointer is still open. All the
device's direct references are in the D3D library (`0x003fxxxx`), but game
code pushes inline through the device pointer `[0x00407488]` (for example
in `0x00116de0..0x00117000` and `0x0019f880..0x0019fc20`). If any of that
runs in the tick while a render is in flight, the sim writes into the same
push buffer from the same put pointer. Its commands are then lost, as they
were under the byte merge. A steady render-owned count in the `pipeline:`
line would show it, and those pushes would need a join like the D3D calls.

The prediction leaves the render's own count unchanged. The launch
snapshots before it credits live RAM, so the view counts from the serial
value, and the "lit at" stamps the relight check `0x0003dfe0` compares
with are the serial run's. The rig test
`the_frame_counter_and_seed_steps_the_sim_sees_are_the_serial_runs` checks
the count every render reaches. It fails, one high, when the snapshot is
taken after the credit.

**The dark first-person model.** A capture of frame 6000 in both modes
(every draw with its shader inputs) showed the cause. The serial frame has
977 draws and the pipelined frame 670. The serial frame's 60 draws with a
32x32 `Al8` volume texture and a 64x64 cube (the dynamic lights' diffuse
and specular layers) are all missing from the pipelined frame. Among them
is a second pass over every first-person mesh: the hands, the rifle and its
parts. The other first-person draws match, so the pipelined render drew no
dynamic light at all.

The visible-light pass (`0x00132190`, under the scene passes) increments
the light visit counter `0x004e0308` (`0x001321bc`). A light becomes a
candidate in `[0x00547f98]` only when its record's stamp at `+0x0c` is not
already equal to the counter (`0x00132a5e`, `0x001325e5`), and visiting it
stores the counter there (`0x000c3110`). New lights start at the counter
less one (`0x000c0328`, `0x000c07f6`). The counter was render-private, so
live RAM kept the value from before the render, since only that pass increments it. The
stamps, though, are in the light array in the heap (`[0x004e030c]`, 0x110
bytes a record), outside the render-private ranges, so they were handed
back. Each render started from the same counter, incremented it to the
value the last render had stamped, and found every light already visited.
The join now adds each counter's delta to live RAM, as the serial run
counts, so the next render counts past every stamp handed back. At a
fallback live is also never behind the view.

The rig test
`a_stamp_handed_back_never_matches_the_next_renders_visit_count` models it:
a counter only the render increments, and a stamp outside the
render-private ranges. Under the old join the pipelined rig drew the light
on 3 of 13 renders, only the serial and fallback ones. Its counter and
stamp also make every serial comparison in the rig check this.

**What the sim reads (determinism against serial).**

- The frame counter `0x004ba034` is read by the sim (the particles at
  `0x0017469d`, `0x00117eda`, `0x002486a3`). It was handed back at the join,
  after the next tick had read it one short. The launch now credits the
  render's views (the last completed frame's count, as for the present
  counter) and the join corrects by the difference (a mispredict counts).
- The random seeds at `[0x004e7408]` (+0 the game's, +4 the effects'): both
  the render (`0x00027be0`, object updates it reaches) and the sim (the
  particles, effects) step them (`x * 0x19660d + 0x3c6ef35f`). The byte
  merge put the render's value over live RAM, dropping the steps the sim
  took meanwhile, so it drew them again. The join now counts the render's
  steps from the snapshot value and applies as many to live RAM's seed
  (`Addrs::random`); flushes leave the seeds out. The total steps match the
  serial run; the sim drawing before the render's draws instead of after is
  inherent to running them side by side.
- Inherent: the logical clock moves by executed work, and the render's
  accesses are charged to the game thread at the join, one frame later
  than serially; I/O requests the render queues are taken by the worker at
  the serviced call, so completions land at other points in the sim's
  ticks (streaming and LOD then differ). The vblank count to a given
  present matched across serial and pipelined runs (10533 to present 7000).
- Visit marks are render-private by design (the sim never sees the
  render's counters).

**Tests.** `crates/feature/tests/pipeline_render.rs`: a synthetic render
front-end (a Rust function registered as generated code with resume
points) and sim, run serially and pipelined over 120 rounds
(`H2_PIPELINE_ROUNDS`; 2,000 pass): the same final memory but the marks,
fallback frames finished on the game thread with the kernel called once
each and one frame wait a frame, D3D reads from the launch snapshot while
the sim has moved on, sim D3D calls after the render's, a snapshot
waiting one frame, a refusal under a continuation stopping by name, a
render probing a frame as deep as a game-sized thread stack (pipelined
and in a fallback; it faults with a 128 KiB render stack), and menu
frames with the feature on running serially with nothing launched and
the same memory as off. With objects created and freed every tick: a
fallback reads the next tick's freed object and faults at `0x3c`, the
crash above; the same call serviced keeps the render on its own frame,
the memory equal to the serial run's, the kernel called once a call on
the game thread, its answer reaching the render through a live range and
through a stack local; a serviced call under a Rust continuation
carries on too. Switched on with every frame serial (a warm-up that never
ends), every kind of frame leaves guest memory (data, marks, the game
thread's stack, the protected page) equal to the switched-off run's after
each frame; a watched word reports each change once.
`crates/emit/tests/second_executor.rs`: lifted x86 refused at a kernel
call carries on on the main context from the refused CPU (twenty rounds).
`crates/d3d/tests/registration.rs`: a real D3D entry on the executor (the
recording backend) reads the snapshot and writes the view.

**Not done** (fallbacks cover them; the `pipeline:` lines say which
first): executor-safe kernel time reads; the async I/O requests made
without the game thread (serviced calls make the frame wait for the
join; the port queues them, build plan 3.5) and the
texture and geometry caches (build plan 3.5); the game-time bracket
(3.4); census v2's `game_state`/`datum`/`world_pool` lines; render-loop
ports on the render thread (they are refused at their entry); the render
thread's frame stats; the COW fault cost on the real working set.

**I/O without stalls.** The render's cache requests go through the async
I/O pool: `0x001208b0` waits on the pool mutex `[0x004e0358]`, pops a
request off the free list `0x004e28e8` and releases the mutex. `0x001209c0`
waits on the queue mutex `[0x004e035c]`, queues the request, releases the
mutex and releases the worker's semaphore `[0x004e0360]`. It passes a
previous-count local that nothing reads. Each of those kernel calls used to
stop the render (a serviced call). The render then waited for the game
thread to reach its join at the end of the tick, and the game thread then
waited for the rest of the render. In loading-heavy stretches that put
every frame in series. Headless runs under load measured 72 to 82 ms a
frame against about 30 ms stock.

Now the render does not stop for them. `Addrs::io_handles` names the three
handle words. On the render thread a wait on one of those handles with no
timeout, a mutex release with no previous count, and a semaphore release
(previous count given as 0) return success at once and are logged. The
render carries on against its snapshot. At the join (`settle`), the game
thread first pushes what the render wrote to live RAM, as before a service,
so the worker sees the request. It then makes the logged calls for real,
in the render's order (`replay`, `replay_next`), and pulls the live ranges
back into the view. A real result other than the one the render was given
is counted (`io_mismatches`).

Two rules keep this deterministic and in serial order:

- A game-thread wait on a pool handle while a render is in flight joins the
  render first (`io_joins`). The render's requests reach the pool before
  the tick's, as in the serial run, where render k runs before tick k + 1,
  whatever either thread's timing. Before this change the serviced path
  queued the render's request after the tick's. The rig shows the worker
  completing the sim's `0x205` before render 4's `0x104`, where the serial
  run completes `0x104` first.
- A frame pipelines only when every request is on the free list
  (`Addrs::io_free`: the head `0x004e28e8`, linked at +0x3c, 150 requests
  from `0x004e0368`). Otherwise it runs serially (`io_busy`). The worker's
  own pool calls happen while the game thread waits, and they are not
  ordered against the render's, so the worker has to be idle at the
  launch. It wakes only through a semaphore release, either the render's
  (replayed at the join) or the sim's (after the join).

`H2_PIPELINE_DEFER_IO=0` restores the serviced path for an A/B. The
`pipeline:` line now ends with
`io_stalls N (T ms) io_deferred D io_joins J io_mismatches M io_other_thread O`.

- `io_stalls` counts frames in which one side waited for the other because
  of the pool: a pool call the render stopped at, or a game-thread pool
  wait that joined.
- `T` is the total time of those waits in the print window: the render
  stopped until the game thread made the call, the game thread waiting for
  the rest of the render after it, and the joins.

Rig tests:

- `the_renders_pool_requests_overlap_the_sim_and_complete_in_the_serial_order`
- `a_render_queueing_io_every_frame_never_stalls`
- `with_deferral_off_each_pool_call_stops_the_render_and_counts_a_stall`
- `a_request_off_the_free_list_makes_the_frame_serial`

## Related

- docs/perf/frame-budget.md, "Render front-end versus simulation": the
  times this starts from.
- docs/graphics/render-thread.md: the backend's render thread, which
  stays as it is.
- docs/graphics/render-loop.md, docs/engine/feature-engine.md section 7:
  the front-end's structure and the native route.
- docs/runtime/snapshots.md: quiescence.
