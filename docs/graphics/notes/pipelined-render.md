# Pipelined rendering: census, prototype and measurements

Evidence for [second-executor.md](../../runtime/second-executor.md) and [framescene.md](../framescene.md). Research record: nothing here states what the code does today.

| Page | What it holds |
|---|---|
| this page | the plan, what pipelining could buy, the verdicts and the measured result |
| [pipelined-render-census.md](pipelined-render-census.md) | the `render-census` method and its two runs: every page and line the render writes that the next tick reads |
| [pipelined-render-prototype.md](pipelined-render-prototype.md) | the prototype's mechanisms, the faults it hit and its measurements |

The pipelined COW render worker (`impl.render.pipelined_frame`, `H2_PIPELINE_*`, `crates/rewrites/render-bridge/src/pipeline_*.rs`) was deleted on 2026-11-22 and is in git history up to `1afa616a6`. It was a research switch, not a speed-up. The `render-census` feature stays.

The whole-frame design below was the first plan for a render one frame behind the simulation. The worker that shipped was the narrower frame-end split.

Entries are from 2026-11-17 and 2026-11-18. Commit: not recorded; the source page was last revised at `b8b163968`. Method: `render-census` (`crates/rewrites/render-bridge/src/render_census.rs`) and the prototype (`pipeline_render.rs`, deleted) on the owner's disc, headless. Not re-run.

## What it would buy

From docs/perf/gameplay-budget.md, "Render front-end versus simulation" (Old Mombasa, headless, `render-timing`, 2026-11-17), per frame:

| draws | sim (guest, outside render) | render front-end (guest) | our D3D layer on the game thread | serial | pipelined, ideal |
|---|---|---|---|---|---|
| 2000 | 7.8 ms | 18.9 ms | 7.7 ms | 34.4 ms | 26.6 ms |
| 1000 | 6.2 ms | 16.3 ms | 5.2 ms | 27.7 ms | 21.5 ms |

The D3D layer moves with the render front-end, so the render side is 26.6 ms and the sim 7.8 ms. Pipelining hides the smaller side: at best the sim's 6-8 ms a frame (about 22%), less the snapshot and the hand-back. The render side stays the critical path, so per-draw work in docs/perf/gameplay-budget.md keeps its value either way.

## Design

One frame of the main loop (docs/game/high-framerate.md, section 1) is ticks, per-frame work, render and present (`0x00223240`), then sound. Pipelined, frame `k`'s render runs while frame `k+1`'s ticks run.

```
sim thread:    [ticks k ][join k-1 | snapshot k | launch k][ticks k+1 ][join k | ...
render thread:                                    [render k from snapshot k     ]
```

At `render_frame_and_present`'s entry the game thread joins render `k-1` and hands its results back, snapshots guest RAM for render `k`, and launches render `k` on the render thread with a copy of the CPU state.

| Part | Rule |
|---|---|
| Snapshot | Host copy-on-write: `mach_vm_remap(copy = TRUE)` of the 64 MB guest RAM gives the render thread a second `Guest` and fast page table. Setup is tens of microseconds; each sim page written during a render costs one COW fault (16 KB host page on arm64). The alternative, software COW through the fast page table, races a sim write against a render read of the same page. |
| CPU and stack | Own `ExecCtx` and `CpuAbi` from the entry's CPU state. The game thread's stack addresses stay valid because those pages are private copies; they are dropped at hand-back. |
| Hand-back | Render-only pages (`rw`, not `ww`) copy back whole. Pages both write merge three-way against the snapshot. The D3D layer's pages (push buffer, globals) move with the device. |
| What the sim must not see change | Pages the render writes and the next sim reads (`rs`). Serially tick `k+1` reads render `k`'s writes; pipelined it reads render `k-1`'s. Each such page is a behaviour change unless the value is cosmetic or the sim can compute it. That set is the census's `rs_core`. |
| D3D calls | The device serialises on its own lock and is reached only from D3D entries, so it runs on the render thread while the sim makes no D3D calls of its own (`sim_d3d_hits` counts them). |
| Kernel and HLE | The render thread gets a fixed, audited set of HLE calls (time reads from the snapshot, no waits). Anything else ends pipelining for that frame. |
| Logical time | The render's accesses are charged to the game thread's clock at the join, so the schedule stays a function of the inputs. Pipelined and serial runs are not lockstep-compatible: co-op peers must agree on the mode. |
| Frame wait and present | The frame wait (`0x12b2e0`) reads the vblank counter on the game thread's clock, so it runs on the game thread at the launch. Swap goes through the device as before. |
| Latency | The picture is one frame older than the simulation (+33 ms at 30 Hz). Input is polled at the same point. |
| High frame rate | docs/game/high-framerate.md works after the present on host surfaces, so it is unaffected. |
| Snapshots and pause | A running render is live host state that blocks a snapshot (docs/runtime/snapshots.md). Snapshots, checkpoints and the agent pause join first. |

## Reading a census result

The criteria the census was read against, per frame from the `run` line after a minute in a busy level.

| Verdict | `rs_core` and the lines | Consequence |
|---|---|---|
| Feasible | a handful of pages, each with a few `dep_lines` and one or two known writers, cosmetic or computable by the sim one frame late (prove with the simulation hash); `ww_core` small with no overlapping lines; `sim_d3d_hits` zero in gameplay; the COW fault cost well under the sim's 6-8 ms | build it |
| Hard | tens of pages in `game_state` or `datum:*` with real line overlap: each writer `pc` is a port to the sim side or a double buffer; or the sim calls D3D steadily | each item is a port |
| Infeasible | hundreds of pages across most datum arrays with dense `dep_lines`, or the render waits on other guest threads inside its scope | the native renderer (feature engine stages 7b-7d) owns its state by construction |

## Results

| Date | Run | Verdict |
|---|---|---|
| 2026-11-18 | Census v1: Old Mombasa, 4,200 frames, 16 sticky pages. `rs_core` 46.5 pages a frame; 71 dependency lines on 15 pages, 69 on XBE pages. About 25 of the 46.5 pages (`game_state`, `datum:*`, `world_pool`) had no line data | Hard, not shown infeasible. Run census v2 before writing code |
| 2026-11-18 | Census v2: Old Mombasa, 3,300 frames, 64 sticky pages a group, 81-93% line coverage. `rs_core` 44.1 pages; 74.1 ordered dependency lines a frame | Hard, feasible with ports: false sharing, render-private marks, values the game thread can produce at the launch, and three shared subsystems (texture, geometry and animation caches with the async I/O queue; particle systems) |
| 2026-11-18 | Whole-frame prototype, tick joining first (03a, `H2_OPT=2` with PGO, 38,107 pipelined frames) | 33-34 fps, no better than stock. Overlap 1.3 ms a frame; hand-back alone costs as much. Off by default; removed on 2026-11-22 |

Details: [census](pipelined-render-census.md), [prototype](pipelined-render-prototype.md).

## Build plan after census v2

| Step | Work |
|---|---|
| 1 | Runtime prerequisites for guest code on a second host thread: a second `ExecCtx` and `Guest` over a COW remap, per-thread resolve and direct-call caches, no kernel on the render thread (an HLE allowlist: D3D entries and captured time), the frame wait on the game thread, the logical clock charged at the join, snapshots joining first. docs/runtime/second-executor.md covers part of the first item |
| 2 | Hand-back with a byte-level merge: retires every false-sharing line without a game-side change. Test it first against a tick-state hash that leaves out the excluded words |
| 3 | Cheap game-side remedies: marks render-private, the present and frame counters incremented on the game thread at the launch, pipelining off during Bink |
| 4 | Cache ownership: the texture, geometry and animation caches and the async I/O pool get a single writer on the game thread; the render's requests and use stamps queue and replay at the join. When data is not resident the render draws without it that frame (drain `0x0012d400`, animation load `0x001237a0`). The largest port and the first with a visible cost |
| 5 | Particle ownership: the particle system, emitter, location and effect-location datums become render-owned; the sim's particle updates run on the render thread and effect creation and deletion queue to it |
| 6 | The game-time bracket (`0x00146de0`/`0x00146b80`, `0x0047988c..`, `0x00510c48`, the `0x7fce6000` line), once identified: move it to the game thread at the launch, or stop if it is real sim state |
| 7 | Measure the COW fault count and cost against the sim's 6-8 ms, the time a frame saves and the join wait |

Steps 2 and 3 make a pipelined run possible with caches and particles still serial, which does not happen in real play, so a speed-up needs steps 4 and 5. The native renderer is the alternative if they cost more than they save. An optional census tweak before any code attributes wait-class HLE calls to their caller, which says how often the drain and the animation load wait at all.

## The prototype

Build plan steps 2, 3 and 7 as `crates/rewrites/render-bridge/src/pipeline_render.rs`, wired in the boot by `d3d_glue::pipeline_render` (deleted). Its mechanisms and measurements are in [pipelined-render-prototype.md](pipelined-render-prototype.md). The D3D entry run by a second executor that reads the snapshot and writes the view is tested at `crates/platform/d3d/tests/registration.rs`.
