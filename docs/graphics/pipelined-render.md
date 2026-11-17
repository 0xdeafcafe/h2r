# Pipelined rendering

Status: **design and measurement only.** Nothing here is implemented
except the census that decides it (`render-census`, below).

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
`pc`, or `d3d`/`host`) and, for the top `rs` pages made sticky (every
access logged), the 64-byte lines the render wrote, the sim read, and
their overlap (`dep_lines`). Regions: `stack` (256 KB below the render
entry's `esp`), `push_buffer` and `d3d_scratch` (from the boot),
`d3d_globals` (the D3D library's device and state block in the XBE,
`0x00403000..0x00409000`), `datum:<name>` (datum arrays found in the
game-state block by their `d@t@` signature), `game_state` (the
`0x3fe000`-byte block at `[0x004e6080]`), `world_pool` (`0x80061000..
0x83145000` and its `0xf...` alias: tag cache, BSPs, carved buffers),
`xbe_image` (globals), `contiguous`, `write_combined`, `other`.

**Limits.** Raw RAM readers that bypass the accessors
(`Guest::physical_ram`: the lockstep hash, snapshots) are not seen; none
is guest code. Atomics
(`runtime::atomic`) count as the host. A hit from another guest thread
inside the render scope consumes that page's arming for the scope, so
the render's own first access to it is missed (rare: count shown).
Without frame stats every host hit is `host` and nothing is
`other_thread`. The run is slower while the census is on (two passes
over the mappings and a few thousand slow accesses a frame); its numbers
are page sets, not times.

**Output.** Every `H2_RENDER_CENSUS_EVERY` frames (default 300) and at
game end: `render-census: window …` and `run …` summary lines (per-frame
averages), `region …` counts, the top `rs`, `ww` and `sr` pages
(`H2_RENDER_CENSUS_TOP`, default 20) with region and first writer and
reader, and `lines …` for the sticky pages (`H2_RENDER_CENSUS_LINES`,
default 16). With `H2_RENDER_CENSUS_OUT=<path>` the full per-page table
is rewritten there as TSV at each report.

**Running it.** It needs a new `h2` binary only (no generated-code or
image change):

```sh
H2_FEATURES=render-census H2_FRAME_STATS=1 H2_RENDER_CENSUS_OUT=$TMPDIR/census.tsv h2 run …
```

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

## Related

- docs/perf/frame-budget.md, "Render front-end versus simulation": the
  times this starts from.
- docs/graphics/render-thread.md: the backend's render thread, which
  stays as it is.
- docs/graphics/render-loop.md, docs/engine/feature-engine.md section 7:
  the front-end's structure and the native route.
- docs/runtime/snapshots.md: quiescence.
