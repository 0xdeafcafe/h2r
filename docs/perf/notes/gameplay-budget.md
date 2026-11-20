# Gameplay frame budget

Evidence for the in-level frame, [../sim-budget.md](../sim-budget.md) and [../README.md](../README.md). Entries are dated; re-measure before acting on any row.

Where the wall time of an in-level frame goes, what the worst frames are
made of, and what it would take to get the worst frame under 33 ms (never
below 30 fps). [menu-budget.md](menu-budget.md) is the same for the main
menu. This one is a level.

Measured 2026-11-15 on master 9a39ed7 plus this branch's stats (the stats
add nothing to the guest's schedule). Dylib built from 9a39ed7, Apple
Silicon, release `h2` built with `--features perf`. The machine was
shared: load average 35-67 during the runs, from other lanes' builds and
benches. Expect every ms to be lower on a quiet machine. The shares and
the hitch causes carry over.

The current numbers are in [Re-measured on master 1a5aa8d
(2026-11-15)](#re-measured-on-master-1a5aa8d-2026-11-15) below: 14.8 fps,
no compile hitch with a warm pipeline cache, and a cost that grows by
about 0.12 ms per draw.

## What "gameplay" is here

The smallest level reachable today is The Heretic (`00a_introduction`).
Scripted input (docs/runtime/BOOT.md, milestone 19) goes through the menus and
loads it, and the first level frame is about present 4210. The Heretic is
an in-engine cinematic: the full 3D level renderer runs (BSP, characters,
shadow maps, cinematic lighting), but there is no player control. The
first level with control is the Armory (`01a_tutorial`), and nothing
reaches it yet: the intro has to finish and queue the next map
(docs/runtime/level-preflight.md, risk 1). Simulation, AI and physics load will be
higher in real gameplay. Rendering load may differ either way.

The window is presents 4220-4520: 300 frames, 36 s of wall time on
master defaults.

```
I="START@f310,DOWN@f408,A@f419,A@f524,A@f585,A@f632,A@f700,A@f760,A@f820"
H2_BOOT_GFX=$R/gfx H2_BOOT_GFX_EVERY=4215 H2_BOOT_INPUT=$I \
H2_BOOT_FPS_FROM=4220 H2_BOOT_STOP_AT_PRESENT=4520 \
H2_FRAME_STATS=1 H2_FRAME_STATS_CSV=$R/frames.csv H2_FRAME_STATS_FROM=4220 \
H2_PERF_FROM=4220 H2_PROFILE=$R/prof H2_PROFILE_HZ=250 \
H2_TIMELINE=$R/trace.json H2_TIMELINE_CATS=all \
h2 run disc/halo2.iso --out $OUT --data $R/data --fresh-data --slices 2000000
h2 profile budget $R/trace.json            # and --json for every frame
```

Three views of the same run:

- **Frame stats** (`H2_FRAME_STATS`, added by this work and now on master; docs/runtime/app.md
  "Frame stats") split every frame into phases on the game thread and
  count what happened in it. This is the tool for users: `h2 play
  --frame-stats` shows the same split live.
- **Timeline** (`h2 profile budget`) gives self time per span, per frame.
- **The sampler**, at 250 Hz (a 1.5% pause tax), splits the guest
  `slice` span into guest code, accessors, x87 and dispatch.

These runs used merged passes, then an opt-in (`H2_GFX_MERGE_PASSES=1`).
Merged passes have since become the default for `h2 run` and `h2 play`
(docs/graphics/graphics.md, "Merged passes"), and the variable is gone.

## What a level frame is

Per frame, median over the window:

- **Logical time 33.3 ms, 2 vblanks.** As on the menu, the game runs at
  its 30 Hz cadence in logical time. Logical/wall is 0.28: the guest
  charges 9.4 ms of logical work, and 2 idle advances skip the other
  31.8 ms for free.
- **1.41 M guest accesses** in 25 slices (the menu: 2.56 M). Only the
  game thread runs.
- **306 draws, 26 submits** (merged passes), 1 texture upload, 3.6 GPU
  readbacks, about 3.6 ms of surface writes.
- **No waiting on swap or vblank.** The guest never waits for a vblank in
  wall time: its frame wait is an idle advance of the logical clock. In
  `h2 play` the pacer sleeps only when the guest is ahead of real time.
  At 8 fps it never is (`pace` 0.0 ms).

## Budget (master defaults)

Frame stats over 301 frames: **8.42 fps; frame time p50 112.0, p90
131.5, p99 195.9, max 808.3 ms.** No frame was under 33.3 ms. The fastest
was 98 ms.

| phase | ms/frame | % |
|---|---|---|
| cpu: guest code, runtime, kernel, HLE | 98.4 | 82.8% |
| encode: wgpu CPU work, pipeline compiles, uploads, present copy | 13.6 | 11.4% |
| gpu-wait: readbacks, `D3DSWAP_FINISH` | 4.0 | 3.3% |
| submit: `queue.submit` | 2.6 | 2.2% |
| decode: push-buffer decode | 0.2 | 0.2% |
| pace | 0.0 | 0% |

Ranked by owner. The sampler's whole-thread shares are scaled to the
118.8 ms mean frame. Rows with a `budget` value use `h2 profile budget`
self time instead.

| # | cost | ms/frame | % | owner |
|---|---|---|---|---|
| 1 | Guest-memory accessors: `checked_start` 9.1%, `Memory::lock` 8.1% (all of it under `h2abi_v1_read`/`write`), `read_bytes` 6.4%, `ffi::read` 5.3%, `ffi::write` 3.2%, `write_bytes` 3.1%, `WriteWatch::mark` 1.7%, `h2abi_v1_read` 1.0% | ~46 | 38.6% | runtime (fastmem) |
| 2 | Guest code (generated C, own time; flat, top function 1.3%) | ~18 | 15.4% | codegen-opt |
| 3 | x87 soft-float: `Wf<8>` div, div_u64, add, mul. Two thirds of it under three guest functions (`0x00016b90`, `0x00372d6c`, `0x00327f84`) | ~16 | 13.6% | x87 |
| 4 | `hle call` self (budget): D3D entry wrappers (the write-barrier shadows `0xa0ff4xxx`), mostly the per-draw entry (297 calls a frame) and one with 13 calls a frame at 0.8 ms each | 10.2 | 8.6% | d3d |
| 5 | wgpu CPU per draw (budget): `draw` 3.7, `flush pass` 2.0, `shader prepare` 1.3, `render encode` 0.2, vertex repack | 7.2 | 6.1% | gfx-perf |
| 6 | GPU round trips (budget): `surface readback` 3.1 (3.6 a frame, including depth decode), `GPU wait` 0.8; render-to-texture write-back | 3.9 | 3.3% | d3d, gfx-perf |
| 7 | `surface write` (budget): CPU writes into render targets, re-uploaded | 3.6 | 3.1% | d3d |
| 8 | Dispatch and runner: `handler::execute`, `run_inner`, resolve | ~7.6 | 6.4% | runtime |
| 9 | `queue submit` (budget): 26 a frame, ~100 us each | 2.6 | 2.2% | gfx-perf |
| 10 | Fallback interpreter (`boot::fallback`), unread switch targets | ~3.1 | 2.6% | analyze |
| 11 | `pipeline create` (budget). The mean hides one 665 ms frame, below | 2.2 | 1.9% | gfx |
| 12 | Push-buffer decode, scheduler, kernel, XAPI, DirectSound | <1.5 | <1% | - |

Rows 1-3 and 8 are the guest `slice` span (87.7 ms budget self time).
Together they are 74% of the frame. At 1.45 M accesses a frame that is
about 60 ns per guest access, all in. Between frames the slice time does
not follow the access count (r = 0.04): the guest does the same work
every frame, and the spread comes from the host.

## Worst frames

From the frame-stats report and the per-frame budget (`--json`):

| frame | ms | what happened |
|---|---|---|
| 4329 | **808** | **Pipeline compile hitch.** 3 nv2a programs translated and 3 render pipelines created. `pipeline create` is 665 ms of that, about 220 ms per Metal pipeline (`MTLCompilerScheduler` on the game thread), plus 13 ms of shader prepare. Everything else is a normal frame. |
| 4374 | 252 | Host contention. Same 1.41 M accesses and 25 slices as the median frame, but slice 190 ms (median 85), and the HLE, surface write and readback rows all doubled together. |
| 4375, 4373, 4378, 4377, 4379, 4376 | 168-211 | The same burst, frames 4372-4379. Accesses within 0.3% of the median, every phase up 1.5-2.5x, gpu-wait up to 31 ms. |
| 4323, 4315 | 174-179 | The same pattern alone: slice 145-154 ms at the median workload. |
| 4256 | 115 | One pipeline and one shader compiled, about 5 ms: the Metal compiler's own cache already had it. |
| 4494, 4343 | 140, 120 | The largest file reads in the window, 282 KB and 117 KB. The cost is in the noise. |

Hitch causes, checked one by one:

- **Shader and pipeline compiles: the only real hitch.** 4 compiles in
  300 frames, one frame of 808 ms. It is synchronous on the game thread.
  A new camera angle or material is enough to cause one.
- **Host contention: the whole p99.** Frames with the median workload
  cost 1.5-2.2x the median during bursts of other processes' load. On
  the user's machine this is any background work. Fewer threads and a
  smaller CPU budget make it less likely, not impossible.
- **Texture uploads: not a hitch.** Exactly one upload every frame (a
  texture the game rewrites each frame), 0.27 ms a frame in total.
- **Cache-miss streaming: not seen.** The game reads 7.9 KB a frame on
  average, 2.4 MB over the window, in 49 of 301 frames. Its largest read
  is 282 KB, and that frame is not slow. The tag, texture and geometry
  caches are not thrashing in the intro: no frame shows a burst of reads
  followed by uploads. (The Halo-2-HD patch,
  <https://github.com/grimdoomer/Halo-2-HD>, MIT, enlarges these caches
  with a 128 MB console to cut pop-in. That is the place to look once a
  level streams. The intro does not.)
- **Waiting on swap or vblank: not seen.** `pace` is 0 and logical time
  skips the vblank wait. The guest sees 2 vblanks a frame and 30 Hz
  whatever the wall time. `D3DSWAP_FINISH`'s `GPU wait` is 0.8 ms.
- **Allocations and GC-like bursts: none.** There is no periodic spike.
  The ten worst frames other than 4329 and 4256 have no compile, no extra
  upload, and normal reads and accesses. Metal buffer allocation
  (`AGXBuffer init`) is 0.7% of samples, spread evenly.

## Opt-ins

These three are now on by default; "defaults" below means
the old defaults, which `H2_FASTMEM=0 H2_X87_NATIVE=0 H2_ACCESS_COUNT=0`
reproduces.

Same window, same input, with `H2_FASTMEM=1 H2_X87_NATIVE=1
H2_ACCESS_COUNT=1` set for both build and run, merged passes as before. These numbers are for measurement only: FASTMEM has a known P1
being fixed. The schedule is identical (1.45 M accesses, 26 slices, the
same 4 compiles in the same frames), so the frames compare one for one.

| | defaults | all opt-ins |
|---|---|---|
| fps over the window | 8.42 | 13.31 |
| frame ms p50 / p90 / p99 / max | 112.0 / 131.5 / 195.9 / 808.3 | 71.6 / 78.9 / 81.5 / 721.1 |
| frames over 33.3 ms | 301 of 301 | 301 of 301 |
| cpu phase, ms/frame | 98.4 | 58.1 |
| encode / submit / gpu-wait | 13.6 / 2.6 / 4.0 | 12.5 / 2.1 / 2.2 |
| guest `slice` self time (budget) | 87.7 | 49.0 |
| sampler: accessors and memory lock | ~46 ms (38.6%) | ~3 ms (3.8%) |
| sampler: guest code | ~18 ms | ~24.5 ms (the inline RAM path now runs here) |
| sampler: x87 | ~16 ms | ~15 ms |
| sampler: mean pause (a load gauge) | 58.6 us | 28.0 us |

Load was lower during the opt-in run (load average about 10 at the end,
against 35-67). The gfx rows the opt-ins cannot touch (`draw`, `surface
write`, `flush pass`, `queue submit`) came in 5-17% lower. Allowing for
that, **the opt-ins take about 35-40% off the steady frame**. Almost all
of it is FASTMEM: accessor time drops from ~46 ms to ~3 ms, and ~6 ms of
it comes back as inline code in the guest functions.

**`H2_X87_NATIVE` does nothing here.** 97% of x87 samples in both runs
are transcendentals (`h2x87_v1_unary` -> `x87::trans`), which the native
path leaves on the exact helpers by design (x87-native.md):

| helper | share of all samples (opt-in run) |
|---|---|
| `yl2x` (FYL2X, log2) | 14.3% |
| `x2m1` (F2XM1, 2^x - 1) | 3.7% |
| `patan` | 1.1% |
| `tan`, `sin`, `cos` | 0.6% |

FYL2X plus F2XM1 is how x87 code computes `pow`, `log` and `exp`. On the
opt-in frame they cost ~13.5 ms. Their `Wf<8>` series arithmetic
(div, div_u64, add, mul) is the top of the self-time table.

The compile hitch is unchanged: frame 4329 is 721 ms, 662 ms of it
`encode` (3 pipelines). After it the worst frame is 102 ms (4513), and
the next nine are all 80-82 ms. With less load the host-noise tail
nearly vanishes: p99/p50 is 1.14, against 1.75 in the loaded run.

## Getting the worst frame under 33 ms

The worst frame is the sum of three things: the steady frame (p50), the
host noise on top of it (p99/p50 = 1.75 here), and one-off hitches.
Each needs its own fix. Savings are from the master-defaults run and do
not add up: each assumes the ones above it.

Base: the opt-in run (FASTMEM is the path being finished anyway).
Steady frame p50 71.6 ms, p99 81.5, max 721.

| # | change | owner | worst-frame effect | steady saving (ms/frame) | confidence |
|---|---|---|---|---|---|
| 0 | **Merged passes in `h2 play`** (this branch; now the default): about 300 draws a frame otherwise mean about 300 passes and submits, against 26 | gfx | follows p50 | not measured in the level; on the menu, merging was about 3x on gfx time. Expect -15 to -30 for a user on master's `h2 play` | medium |
| 1 | **Compile pipelines off the game thread, and prewarm them** (done: docs/graphics/graphics.md "Pipeline compiles"; draws wait at encode rather than being skipped). Create nv2a programs and render pipelines on a worker. Until one is ready, skip the draw for a frame or two, or block only on a deadline. Persist the keys seen (program hash plus pipeline key) and compile them at map load. Metal's own cache already makes a repeat compile ~5 ms (frame 4256), so the hitch is the first time a material or angle appears. | gfx, d3d nv2a | **721 -> ~80 ms** (-640): the only hitch over 110 ms in either run | ~2 | high: 665 of 721 ms is `pipeline create` |
| 2 | **FASTMEM on by default** once its P1 is fixed (with `H2_ACCESS_COUNT`) | runtime, emit | follows p50 | **-35 to -40** (119 -> 75 measured, with a load difference) | high: measured |
| 3 | **Fast exact FYL2X/F2XM1** (done: [x87-transcendentals.md](../../lifter/x87-transcendentals.md); both first passes are table-driven now). A table-driven or double-double first pass with a rounding-ambiguity check, keeping the `Wf<8>` series as the exact fallback for the rare hard cases. That is the correctly-rounded-libm pattern, and the guest result stays bit-exact. | x87 | follows p50 | **-10 to -12** (13.5 -> ~2) | medium: 97% of x87 time is these two; the exactness check is the work |
| 4 | **Render thread:** hand recorded draws to a thread that encodes and submits (the `render-thread` lane). The game thread keeps the D3D resolve. | gfx-perf | follows p50 | **-10 to -12** of game-thread time (encode 12.5 + submit 2.1, minus the handoff) | medium: the GPU must keep up in parallel, and readbacks still sync |
| 5 | **Render-to-texture write-back without round trips:** 3.6 readbacks a frame (depth decoded on the CPU), 3.3 ms of surface writes, `D3DSWAP_FINISH`'s full wait | d3d, gfx-perf | removes gpu-wait spikes (up to 31 ms in the loaded run) | -5 to -7 | medium: the RTT-direct work on master is the start |
| 6 | **Guest code and dispatch:** IR passes (flags, dead stores, register promotion), cheaper `handler::execute`/resolve, close the remaining fallback-interpreter targets | codegen-opt, runtime, analyze | follows p50 | -8 to -12 of ~33 (guest 24.5, dispatch 6.1, fallback 2.5) | low: the profile is flat, top guest function 2.2% |
| 7 | **D3D entry wrappers** (`hle call` self 8.6 ms): the per-draw entry, 297 calls a frame. The dirty-page handoff and state resolve per call. | d3d | follows p50 | -3 to -5 | medium |
| 8 | Vertex `repack` (2.5 ms) and per-frame Metal buffer churn | gfx-perf | follows p50 | -2 | medium |

Where that lands, from the opt-in base (item 2 is already in it):
71.6 - 11 - 11 - 6 - 10 - 4 - 2 = **~28 ms p50** with every item done.
That is under 33 ms, but not with room for host noise. The worst frame
is then p50 times the host's noise factor: 1.14 on a quiet machine
(32 ms), 1.75 on the loaded one (49 ms). So:

- Items 1-5 are needed before anything is near 30 fps. Item 1 alone fixes
  the worst frame's order of magnitude.
- **A never-below-30 worst frame needs p50 around 25 ms** on a quiet
  machine. That means item 6 delivering at the top of its range, or the
  guest side moving to near-native code: ~15 ns per access all in, from
  ~35 ns in the opt-in run.
- Once frames are under 33 ms, `h2 play`'s pacer sleeps the slack. That
  slack is what absorbs host noise, and `pace` in the stats shows it.

Not needed on this evidence:

- **Bigger texture and geometry caches** (as a host-side setting, the
  way Halo-2-HD does it on a 128 MB console): no streaming hitch in the
  window. The largest read, 282 KB, did not slow its frame. Recheck in a
  level that streams (Cairo, or the first BSP switch). If a frame's
  `read_bytes` and `uploads` jump together, it becomes a candidate.
- **Triple buffering:** the guest never waits on swap or vblank in wall
  time. Vblanks are on the logical clock, and the shell's present
  handoff is already a 3-slot mailbox that never blocks the game.
  A late frame costs its own lateness, not a whole extra vblank. Revisit
  if a fixed-vblank pacing mode is added.

## Re-measured on master 1a5aa8d (2026-11-15)

Same window (presents 4220-4520), input and command as above, with the
pipeline cache pointed at a fresh scratch file (`H2_PIPELINE_CACHE`).
Everything that was an opt-in above is now the default: merged passes,
FASTMEM, x87-native, the access counter and the guarded FYL2X/F2XM1
fast path. Dylib emitted from cae8a4a. 1a5aa8d changes only d3d, gfx and
shaders, so the generated code is the same, and the dylib's stamp
matched. Release `h2` built at each commit with `--features perf`. The
schedule is identical in every run (56,572 slices, 94,097 preemptions,
1.45 M accesses and 306 draws a frame), so the frames compare one for
one. Load is the 1-minute load average while the window ran (the last
~20 s of each run).

| run | commit | load | fps | p50 / p90 / p99 / max ms | guest CPU ms/frame | encode / submit / gpu-wait |
|---|---|---|---|---|---|---|
| previous: old defaults (above) | 9a39ed7 | 35-67 | 8.42 | 112.0 / 131.5 / 195.9 / 808.3 | 98.4 | 13.6 / 2.6 / 4.0 |
| previous: all opt-ins (above) | 9a39ed7 | ~10 | 13.31 | 71.6 / 78.9 / 81.5 / 721.1 | 58.1 | 12.5 / 2.1 / 2.2 |
| defaults, cold pipeline cache | 1a5aa8d | 9-11 | 14.39 | 64.6 / 80.4 / 85.8 / 469.9 | 44.6 | 11.8 / 7.2 / 5.8 |
| defaults, warm pipeline cache | 1a5aa8d | 3-4 | **14.80** | 64.8 / 78.0 / 82.1 / **83.3** | 44.4 | 10.1 / 7.1 / 5.8 |
| defaults, cold cache | cae8a4a | 3-4 | 14.76 | 63.1 / 77.8 / 86.8 / 464.7 | 43.7 | 11.2 / 6.9 / 5.8 |
| defaults, warm cache | cae8a4a | 7 | 14.48 | 65.6 / 80.4 / 87.6 / 90.6 | 45.1 | 10.5 / 7.4 / 5.9 |
| warm cache + `H2_RENDER_THREAD=1` | cae8a4a | 7-8 | **15.79** | **60.4** / 71.7 / 90.3 / 110.7 | ~47 (63.1 minus ~16 of waits) | moved to `d3d-render` |
| warm cache + `H2_RENDER_THREAD=1` | 1a5aa8d | 15-40 | 10.31 | 84.4 / 141.1 / 229.9 / 271.1 | - | loaded, not comparable |
| warm cache + `H2_RENDER_THREAD=1` | 1a5aa8d | 22-65 | 12.53 | 59.6 / 100.7 / 508.6 / 1008.4 | - | loaded, not comparable |

Guest CPU is frame stats' `cpu` phase (game code, runtime, kernel, HLE).
With the render thread `cpu` also holds the game thread's waits for the
render thread; the sampler puts those at ~16 ms a frame, which is
subtracted here. Both 1a5aa8d render-thread runs met other lanes' builds
(load up to 65), so the cae8a4a run stands for the render thread: its
code did not change between the two commits.

- **+11% over the opt-in run, +76% over the old defaults.** p50 drops
  from 71.6 to 64.8 ms. Guest CPU drops from 58.1 to 44.4 ms a frame,
  almost all of it x87: the guarded transcendental fast path takes x87
  from ~15 ms to 2.6 ms a frame. Some of that comes back on the GPU
  side: `submit` is 7.1 ms against 2.1 and `gpu-wait` 5.8 against 2.2.
  `encode + submit + gpu-wait` is 23 ms against 16.8. Not bisected. The
  likely cause is the pipeline-compile and recording changes that came
  in after 9a39ed7 (see "Per-draw cost" below: the time is now in
  wgpu-core's `command_encoder_finish`, which runs at submit).
- **The warm pipeline cache removes the compile stall.** Cold, frame
  4329 still compiles 2-3 pipelines on the game thread: 465-470 ms,
  416 ms of it `encode`. The compile workers bring that down from 665 ms
  but do not remove it. Warm, no frame waits for a pipeline
  (`pipelines` 0 in all 301 frames) and the worst frame is 83 ms, a
  normal 388-draw frame. The file held 581 entries after one run.
- **No frame is under 33.3 ms.** The fastest is 58 ms with the sync
  backend and 55 ms with the render thread.
- **W-buffer fix (1a5aa8d): no measurable GPU cost here.** 235 of the
  275 programs in the cache are W-buffered and now write `frag_depth`,
  which turns off early depth rejection (and hidden-surface removal on
  Apple GPUs) for those draws. Warm, cae8a4a against 1a5aa8d: `gpu-wait`
  5.89 against 5.77 ms, `surface readback` (which includes waiting for
  the GPU to finish the frame's draws) 5.52 against 5.41 ms, and its
  slope 26.9 against 25.4 us per draw. This scene is bound by CPU and
  round trips, not by fragment work, so the GPU has slack to absorb it.
  Recheck in a GPU-bound open view: there it could show, and it scales
  with overdraw, not draw count.

### The render thread in a level

It worked. The first in-level run of `H2_RENDER_THREAD=1` stopped
nowhere, with no judge mismatch or backend error in three runs. The
schedule matched the synchronous runs exactly, and the frame-4215
screenshot was byte-identical to the synchronous run's, on both commits.

It gained 5 ms of p50 (65.6 to 60.4 ms, 14.48 to 15.79 fps at the same
load). That is less than the ~17 ms of encode, record and submit it
takes off the game thread, because the guest still waits for the render
thread at every read-back. `H2_RENDER_THREAD_STATS` counts 32,479 waits
over the run, 65 s of 259 s. In the window the sampler puts the game
thread's waits at 15.8 ms a frame: `read_surface` 9.6, `read_depth` 4.6,
`read_pixel_count` 1.6. Each one drains everything queued before it,
then a GPU round trip. A further 2.2 ms is the handoff itself (cloning
stream bytes into the queue in `ThreadedBackend::draw`). The tail was
worse, p99 90 and max 111 against 88 and 91, at a load of 8: the render
thread competes for cores with the game thread and the compile workers.

### Fixed cost and per-draw cost

The user's report on master: 18-30 fps on the menu, about 17 fps in
game, worst looking over a large open space (multiplayer). That points
at a cost that grows with what is on screen. The Heretic window has two
kinds of shot, which is enough to fit the slope: 192 frames at 266-280
draws (29 submits, 4 readbacks) and 69 frames at 380-392 draws
(21 submits, 3 readbacks), plus a few in between. A least-squares fit
of each frame's time against its draw count (warm, 1a5aa8d, 299 frames):

| cost | fixed ms | per draw | r | at 306 draws | at 600 draws |
|---|---|---|---|---|---|
| **whole frame (wall)** | **32.0** | **116 us** | 0.92 | 67.5 | ~102 |
| guest `slice` (game code: culling, building the push buffer) | 27.6 | 25.6 us | 0.68 | 35.5 | 43 |
| `queue submit` (wgpu-core `command_encoder_finish` and Metal commit) | -4.6 | 38.3 us | 0.97 | 7.1 | 18 |
| `surface readback` (drain, then wait for the GPU to finish the draws so far) | -2.4 | 25.4 us | 0.98 | 5.4 | 13 |
| `hle call` (D3D entry wrappers, per-draw entry) | 4.0 | 14.6 us | 0.88 | 8.5 | 13 |
| `draw` (record into the pending pass, vertex repack) | -0.5 | 13.1 us | 0.93 | 3.5 | 7.4 |
| `shader prepare` | 0.4 | 2.8 us | 0.92 | 1.2 | 2.1 |
| `surface write`, `flush pass` | ~6 | ~0 | - | 4.8 | ~5 |
| guest accesses | 1.12 M | 1,090 per draw | 0.97 | 1.45 M | 1.77 M |

`queue submit` does not follow the submit count (r = -0.95: the
388-draw shots have fewer, bigger passes). It follows the draws: wgpu
encodes the recorded passes into Metal at `CommandEncoder::finish`, so
its cost is per draw. With the render thread the game thread's slope is
about the same, 108 us per draw (30.3 ms fixed, r = 0.79), because the
read-backs make it wait for the render thread's per-draw work anyway.

So a frame is **~32 ms fixed plus ~0.12 ms per draw**. The fixed part
alone is the whole 33 ms budget. Each extra 100 draws costs ~12 ms,
~40% of it in wgpu and Metal on the CPU, ~20% the GPU itself (as seen
through read-backs), ~20% the game's own work and ~12% the D3D wrappers.
Draws, accesses and passes are the counters that move. Texture uploads
stay at one a frame and readbacks at 3-4, whatever the shot.

### Budget (warm cache, 1a5aa8d, sync backend)

Sampler shares on the game thread, scaled to the 67.5 ms mean frame,
ranked:

| # | cost | ms/frame | % | grows with draws |
|---|---|---|---|---|
| 1 | Guest code (generated C, own time; flat, top function 2.9%) | 14.9 | 22.0% | partly |
| 2 | GPU round trips: `read_surface` 8.8 (of which ~3.5 is flushing and encoding the pending pass, ~4.6 the `poll` wait), `read_depth` 2.1 (CPU depth decode included), pixel counters 1.4 | 12.3 | 18.2% | yes |
| 3 | wgpu and Metal per draw: `record` 3.1, submit and wgpu encode 2.6, shader prepare 1.3, other draw 0.9 | 7.9 | 11.7% | yes |
| 4 | D3D HLE: entry wrappers, state resolve, `WriteTracker::dirty_since` (SipHash), stream copies out of guest memory (`read_vec`) | 6.0 | 8.9% | yes |
| 5 | Dispatch, runner, budget charge (`handler::execute`, `run_inner`, resolve, interrupts) | 5.0 | 7.5% | no |
| 6 | Guest block copies (`h2_fast_copy` and the memmove under it, `rep movs`) | 3.9 | 5.8% | partly |
| 7 | `write_surface`: CPU writes into render targets, re-uploaded | 3.9 | 5.7% | no |
| 8 | SSE soft-float helpers (`h2s_addps`, `round_pack`, `mul1`, ...) | 3.3 | 4.9% | partly |
| 9 | x87 (was ~15) | 2.6 | 3.9% | no |
| 10 | Kernel, XAPI, scheduler, DirectSound | 2.1 | 3.1% | no |
| 11 | Fallback interpreter (unread switch targets) | 2.0 | 3.0% | no |
| 12 | Guest memory accessors (was ~46 before FASTMEM) | 1.5 | 2.2% | no |
| 13 | Boot trace file writes (`h2 run` only, not `h2 play`) | 0.9 | 1.3% | no |
| 14 | Present, snapshots, uploads, clears | 1.4 | 2.0% | no |

Guest-side rows (1, 5, 6, 8-12) are 35 ms. Graphics rows (2-4, 7, 14)
are 31 ms. Unlike the menu, x87 and the memory accessors are no longer
in the top half.

### Next targets

The open-space case is the per-draw slope. At 600 draws the frame is
~100 ms, and 30 fps needs the per-draw cost on the game thread far below
the 116 us measured. The three targets that move it most, in order:

| # | target | owner | steady saving here (306 draws) | at 600 draws | confidence |
|---|---|---|---|---|---|
| 1 | **Take the read-backs off the frame's critical path, then turn the render thread on.** 3-4 `read_surface`/`read_depth` and the pixel counters drain the queue and wait for the GPU every frame. Keep render-to-texture and depth-alias results on the GPU when only the GPU samples them, and write a level back to guest memory only when the guest's CPU really touches it (a write-watch or fault on those pages, the way FASTMEM tracks writes). Decode depth on the GPU. Answer pixel counts from a report that has already landed, where the guest polls rather than blocks. With nothing to wait for, the render thread overlaps all of the wgpu work. | d3d, gfx-perf | **-17 to -20** sync and render thread together (12.3 of round trips and ~8 of per-draw wgpu off the game thread): ~65 -> ~45-48 ms p50 | game-thread slope from ~110 to ~45 us per draw (guest plus HLE only) | medium: the waits are measured; the hard part is proving which write-backs the guest never reads on the CPU |
| 2 | **Cut wgpu's per-draw CPU cost** (38 us per draw in `queue submit`, 13 in `record`, 3 in prepare). Persistent mapped rings instead of a `queue.write_buffer` per flush, which shows as `madvise` and memmove churn. Bind groups and uniforms cached across frames. Fewer pipeline and state switches per merged pass. Find what came in after 9a39ed7 that took `submit` from 2.1 to 7.1 ms. | gfx-perf | -5 to -8 on the sync path. With the render thread, this bounds its throughput rather than the game thread | -15 to -25 (54 -> ~25 us per draw) | medium: the time is in wgpu-core's `command_encoder_finish` and Metal's encoder setup |
| 3 | **Guest per-draw and fixed work** (native SSE, the REP copy and `dirty_since` parts are done: [guest-memory.md](guest-memory.md#guest-cpu-sse-rep-copies-and-d3d-entry-2026-11-15)): native SSE scalar and packed ops instead of the `h2s_*` soft-float helpers (3.3 ms; the x87-native pattern), `h2_fast_copy` as a host memcpy on FASTMEM RAM (3.9 ms), and in the D3D per-draw entry, FxHash or a generation counter instead of SipHash in `dirty_since`, and stream copies without the `read_vec` allocation (6.0 ms). | codegen-opt, runtime, d3d | -7 to -10 | -10 to -14 (the guest and HLE slope is ~40 us per draw) | medium for SSE and copies (the same pattern as x87 and FASTMEM), low for HLE |

From 64.8 ms p50: -18, -6, -8 is **~33 ms**. That is at the 30 fps
line for this scene, with no margin for host noise or a wider view. The
fixed 32 ms still has to come down below ~20 ms before a 600-draw view
fits. That is guest code and dispatch (~20 ms, the flat profile of row 1
and 5), the previous section's item 6.

Not new, still true: the host-contention tail is the p99 (the loaded
render-thread runs reached p99 509 ms at load 65), and with the
render thread the game thread and the render thread compete for cores.

### Depth aliases on the GPU (2026-11-15)

Item 1's depth half. The user's own `h2 play` run on master (1,897
presents, mostly the menu and some level, ~1,100 draws a present)
counted `rtt_writebacks` 0, every write-back cause 0, and `depth_reads`
1,763 with `depth_alias_uploads` 1,763: about one depth-alias read-back
a frame (the fog pass, `0x509368`), and no render-to-texture write-back
at all. So the depth alias went first: `GfxBackend` now decodes the
depth buffer as the alias's format on the GPU
(`Backend::depth_alias_texture`, docs/graphics/d3d.md "Depth aliases on the
GPU"). No read-back, no CPU decode, no upload; with the render thread
it no longer drains the render thread's queue either.
`GfxBackend::set_gpu_depth_aliases(false)` restores the read-back
(tests and benches).

Headless microbench, `cargo test --release -p d3d --test gpu_depth_alias
-- --ignored --nocapture`: a 640x480 frame that clears colour and depth,
draws 200 depth-writing quads, samples the whole depth buffer through a
`LIN_A8R8G8B8` header in one full-screen quad, draws 100 more quads and
swaps. 60 frames after a warm-up, then one read-back drains the queue.
Apple Silicon, load average 9 (other lanes running), two rounds:

| path | round 0 ms/frame | round 1 ms/frame |
|---|---|---|
| sync, read-back (GPU alias off) | 3.44 | 3.31 |
| sync, GPU alias | **1.31** | **1.41** |
| render thread, read-back | 3.37 | 3.53 |
| render thread, GPU alias | **1.21** | **1.28** |

About 2.1 ms a frame here, where the queue ahead of the read-back is 200
small draws. In the game the read-back waits for everything queued
before the fog pass (the measured `read_depth` share was 2.1 ms a frame
in the level and 4.6 ms with the render thread, up to ~20 ms on the
menu), and that whole wait goes. Both paths give byte-identical back
buffers (`gpu_depth_aliases_draw_what_the_readback_draws`).

### Overlapping levels on the GPU (2026-11-15)

The user's 327 s run, mostly in a level, counted `rtt_writebacks` 21,976,
every one `overlap` (about 3 a frame over 7,113 presents), and
`rtt_uploads` 38,459, at 21.75 fps. The alias mirror (docs/graphics/d3d.md,
"Overlapping levels on the GPU") moves those levels through a GPU buffer
instead of guest memory: no read-back, no CPU encode or decode, no upload
from guest bytes. `Device::set_alias_mirror(false)` restores the
write-backs (tests and benches only).

Headless microbench, `cargo test --release -p d3d --test alias_mirror --
--ignored --nocapture`, Apple Silicon, load average 20-38 (other lanes),
two rounds. `bench_heretic_sized_pool`: each frame 150 quads into the back
buffer, then a 320x240 and two 160x120 `LIN_A8R8G8B8` levels over one
1 MiB pool, each sampling the one before, and the back buffer sampling the
last (3 overlap write-backs a frame on the old path, none with the mirror,
no flush either):

| path | round 0 ms/frame | round 1 ms/frame |
|---|---|---|
| sync, write-backs (mirror off) | 15.05 | 10.84 |
| sync, mirror | 10.44 | 9.58 |
| render thread, write-backs | 10.15 | 10.07 |
| render thread, mirror | **5.24** | **3.82** |

With the render thread (the default) the guest no longer waits for the
queue at each level switch: about 5-6 ms a frame here. On the device's
thread less goes: the copies of the level's guest bytes the device still
makes per move and fill (the match it keeps for guest stores) remain.
`bench_overlapping_pool_frames` (three 32x32 levels, no other work):
7.5-7.9 against 3.4-3.9 ms a frame on the device's thread, 4.6-5.7
against 5.1-5.3 through the render thread (too small to matter there).
Not yet measured in the game.

The shadow map's own refill still goes through memory: a depth level that
overlaps bytes the mirror holds newer flushes them and is filled on the
CPU (there is no GPU decode into a depth surface yet). If a level run
shows `mirror_flushes` at one a frame, that is the next step; a whole
depth-and-stencil clear right after binding would let the fill be
skipped.

### The submit regression (bisected 2026-11-15)

`queue submit` went from 2.1 ms a frame (9a39ed7, merged passes opt-in)
to 7.1 ms (1a5aa8d). `crates/d3d/tests/draw_churn.rs` replays 600
textured draws a frame with level-like churn (two textures, new pixel
uniforms every draw, alpha test on every third draw, blending on every
fifth) at every d3d/gfx commit in the range. The render-pass count is
the signal that does not depend on load: 1 a frame at 9a39ed7 and
b9c05ec with merged passes, 200 from 38cd3f7 on. 38cd3f7 made merged
passes the default and gave each draw that can `discard` (every
alpha-tested draw) a render pass of its own, for exactness on Apple GPUs
(docs/graphics/graphics.md, "Merged passes"). Each render pass is two Metal command
buffers and a render encoder in wgpu 30: about 40-75 us of CPU a pass,
in Metal's encoder setup and command-buffer commit and teardown. The
compile workers (6f95526), the render thread and the W-buffer change
leave both the count and the cost alone.

Same machine, master and this branch alternated, load 49-61 (us per
draw, all in):

| scenario | master | branch |
|---|---|---|
| merged, no alpha test (1 render pass) | 7.8-8.3 | 7.7-8.6 |
| merged, level churn (200 render passes) | 41-53 | 39-43 |
| fast mode, level churn (1 render pass) | - | 9.0-10.5 |
| fast mode, churn + an RGB-only draw every 20 (31 render passes) | - | 12.7-13.8 |
| one render pass per draw (600) | 109-122 | 105-118 |

The fast rule split a discarding draw only after a partly masked draw.
It was off by default at first because the merge self-check finds a few
edge pixels off in a level frame (docs/graphics/graphics.md); it became
the default, and was later superseded by the loose rule and removed. The
rule measured as "merged" above is the exact rule
(`GfxBackend::set_merge_masks(false)`).
One run of The Heretic with it on (presents 4220-4520, cold pipeline
cache, `H2_GFX_MERGE_CHECK=4216`, otherwise the command above) met a load
average of 100-160 from other lanes, so it does not compare with the
runs above: 8.56 fps, frame mean 117 ms, median 105; `submit` mean 7.6 ms
and median 4.4 ms a frame (1a5aa8d at load 3-4: mean 7.1), `encode`
20.4, `gpu-wait` 13.2. The schedule matched the earlier runs exactly
(56,572 slices, 94,097 preemptions).

### Per-draw CPU preparation (2026-11-15, `perf-perdraw`)

Synthetic headless Metal measurements on an Apple M1 Max (10 CPU cores),
against pre-LFS master `453e6c4` (rewritten as `7151bc3`).
The existing `draw_churn` integration benchmark is also available through
Cargo's bench runner:

```sh
H2_BENCH_ONLY=city H2_BENCH_FRAMES=50 cargo bench -p d3d --bench draw_churn
```

The city cases use 1,500 draws over an 8x8 grid of synthetic textured quads,
with six draws per material. Materials alternate two textures and change
the texture factor, alpha test (every third material) and blending (every
fifth). The masked case also writes only RGB on every twentieth draw.
Both keep the default fast merge rule. This measures draw submission and
state preparation through `Device<GfxBackend>` and `SparseMemory`; the
geometry and shaders are deliberately small. It does not measure a game
frame or the guest's culling and push-buffer construction.

A 1 kHz samply recording of the **bench binary only** found uniform
serialization at 13.0% inclusive (6.4% vertex, 6.5% pixel), pipeline-cache
lookup at 7.1%, and `madvise` at 0.7% self time. The backend already uploads
one packed buffer per ring per flush, caches texture groups across frames,
uses dynamic uniform offsets, and skips redundant pipeline and bind-group
sets. Two changes address the remaining preparation costs:

- Serialize uniforms directly into reusable provider-owned byte arrays.
  Contiguous float blocks use a safe bulk copy on little-endian hosts;
  big-endian hosts retain explicit little-endian conversion. This removes
  the temporary pixel `Vec<Vec4>`, its growth, per-float output growth, and
  both per-draw byte-vector allocations. Float payload bits and integer
  stipple rows are preserved.
- Use a borrowed `hashbrown` pipeline-cache entry with its fast seeded
  hasher. A hit hashes and probes once and clones no key; the old
  `contains_key` followed by indexing hashed the full key twice with
  SipHash. Keys, equality, pipeline construction and hit/miss accounting
  retain their meaning.

Eight alternating A/B runs per binary, 50 measured frames per case after
two warm-up frames, release/bench profile, no concurrent compilation in
this lane. Values below are medians of the per-run us/draw values; total
includes the final GPU completion wait, record covers device draw/state
calls, and swap covers flush/encode/submit. The host was shared, so the
short alternating runs are more useful than the early standalone samples
(which varied from about 5 to 24 us/draw as load changed).

| case | total before / after | record before / after | swap before / after | total saving |
|---|---|---|---|---|
| city, 1 render pass/frame | **4.8 / 3.8 us** | 3.6 / 2.6 us | 1.15 / 1.15 us | **21%** |
| city + masked, 76 render passes/frame | **8.6 / 7.4 us** | 3.7 / 2.6 us | 4.9 / 4.8 us | **14%** |

Total ranges were 4.6-5.0 -> 3.6-4.4 us/draw for city, and 8.1-10.6 ->
7.1-8.2 for masked city. Both submit once per frame. The saving is about
1.0-1.2 us/draw, or 1.5-1.8 ms for these 1,500-draw synthetic frames.
Rendering commands, draw order, pass boundaries and upload lifetime rules
are unchanged. The earlier 116 us/draw whole-game slope includes many
costs this fixture does not exercise; these numbers do not re-fit it.

Correctness evidence: the captured 64x64 output bytes matched the saved
baseline binary for both city cases at every measured optimization stage.
`d3d/tests/draw_churn.rs` also compares every output byte over three frames
against individual render passes, under both fast and exact merging,
changing texture and vertex data between frames. `shaders/tests/uniform_bytes.rs`
checks every field's serialized layout, including NaN payloads, signed zero,
clip/clamp changes and stipple integer bits while reusing the output arrays.
All resources and expected values are synthetic.

`H2_BENCH_DRAWS` overrides both the legacy 600-draw cases and the new
1,500-draw cases. `H2_BENCH_OUTPUT=<existing-directory>` writes each case's
final raw color-buffer bytes after the timed window for an offline A/B
comparison. The ignored integration entry remains available with
`cargo test --release -p d3d --test draw_churn -- --ignored --nocapture`.

### Render-thread frame stats and the supplied city CSV

The supplied master CSV has 773 frames. All six renderer counters
(`draws`, `submits`, `pipelines`, `shaders`, `uploads`, `readbacks`) and all
three renderer timers (`encode_us`, `submit_us`, `gpu-wait_us`) are zero
in every row. Their hooks wrote to the render thread's thread-local
bucket, while `frame_end` read only the game thread's bucket. The user's
city observation was 11-12 fps, p50 80-86 ms, with 70-91 ms in `cpu` and
5-7 ms decoding; the menu was about 29 fps. The CSV's whole-run medians
are 66.895 ms total, 57.999 ms `cpu`, and 4.244 ms decode.

The old `cpu` includes synchronous render-thread reply waits and the
frame-in-flight throttle. Their share **cannot be recovered from this
CSV**. `read_bytes` is `NtReadFile` traffic (mean 526,980 bytes/frame in
this file), not GPU readback bytes. Neither GPU readback volume nor the
reported roughly 1,100 draws/present can be reconstructed from those
zero renderer columns. No game run was made to fill the gap.

Frame stats now queue a telemetry marker at the device's existing
`count_swap` boundary. Boot's diagnostic and headless wrappers forward
that hook to their inner backend. It joins the render thread's phase/counter deltas
to that exact game-thread frame, including when the worker finishes
before or after `frame_end`. CSV/overlay publication waits for both
samples; the game thread does not wait for telemetry. Rows retain their
original index, end timestamp and wall duration and are published in
order. Worker idle time is excluded. A recording restarted with old work
still queued discards that old work at its marker. At shutdown, any
still-incomplete tail is omitted with an explicit count in the report,
rather than emitted with zero renderer statistics or waited on.

A new `render-wait_us` phase (`rt` in the overlay) measures game-thread
wall time waiting for render-thread replies or the frame-in-flight
limit, taking that time out of `cpu`/decode self time. This is the number
to divide by `total_us` to see how much of a future city frame waits on
the renderer. It includes queue backlog, backend CPU work and GPU waits
needed to obtain a reply; it is not GPU time alone. With a render thread,
`encode_us`, `submit_us` and `gpu-wait_us` describe its work in parallel
with the game thread, so summing every phase can exceed `total_us`.
FPS and frame percentiles still use the original game-thread wall time.

The CPU-only `d3d/tests/threaded_frame_stats.rs` regression blocks a
synthetic worker until after Swap returns, checks all nine formerly-zero
CSV fields, distinct counters on consecutive frames, file-byte counts,
reply-wait attribution, and restart/shutdown behavior. Timeline unit tests
also cover worker samples arriving on either side of the game sample,
ordered publication, unchanged wall-time FPS and exactly-once accounting.

### Validation after the history rewrite

Rebased `perf-perdraw` onto `origin/master` `3bfc473` with
`git rebase --onto origin/master 453e6c4 perf-perdraw`; no merge or push.
With a single APFS-cloned target, `RUSTC_WRAPPER=`, four compiler jobs,
incremental compilation disabled and `nice`:

- `cargo test -p d3d -p gfx -p boot -- --test-threads=2`: **545 passed,
  21 ignored** (including the GPU city byte comparison and queued-stats
  CSV regression).
- `cargo test -p shaders -p timeline -- --test-threads=2`: **94 passed**,
  including the uniform ABI tests and timeline ordering/restart checks.
- `cargo clippy -p d3d -p gfx -p boot --all-targets -- -D warnings` and
  `cargo clippy -p shaders -p timeline --all-targets -- -D warnings`: clean.

The GPU fixture reads the last presented surface after a flip and reports
its synthetic resource edits through `Device::note_write` under tracked
uploads. The stats fixture uses a FINISH swap, which reaches `count_swap`
on a flipping chain. Tests also caught and fixed boot wrappers dropping
the new hook. All proof used tests and synthetic resources; no game image
was built and no game was run.

### Render front-end versus simulation (`render-timing`)

The game thread's guest time (`cpu`) holds both the simulation and the
game's own render front-end: the recompiled code that walks views,
passes, objects and lights and writes the push buffer. Profilers show
guest functions flat, without callers, so they cannot split the two. The
`render-timing` feature does, with read-only observers on six points
(`engine::points`, all opaque):

| Stage | Point | Address |
|---|---|---|
| `frame` | `render_frame_and_present` | `0x00223240` |
| `game_views` | `render_game_views` | `0x002234e0` |
| `views` | `render_views` | `0x0002b5d0` |
| `game_view` | `render_game_view` | `0x0002b790` |
| `scene_passes` | `render_scene_passes` | `0x0002bcd0` |
| `light_passes` | `render_light_passes` (one dynamic light) | `0x00031170` |

Turn it on with the frame stats:

```sh
H2_FRAME_STATS=1 H2_FEATURES=render-timing h2 run ...
```

It is off by default: each hooked function leaves the direct-call path,
and the light pass runs once per light. With `H2_FRAME_STATS` unset it
measures nothing and says so once.

**What the numbers mean.**

- The frame stats gain a `render` phase (`render_us` in the CSV, `ren` in
  the overlay). It is the time between `render_frame_and_present`'s entry
  and its return on the guest thread that called it, minus every other
  phase inside it (`decode`, `render-wait`, `encode`... on the game
  thread) and minus slices of other guest threads, DPCs and the
  scheduler: the kernel tells the stats which guest thread each slice
  belongs to, and the render scope is suspended while it is not the
  owner's. `cpu` no longer includes that time. **Guest time is `cpu +
  render`; the render front-end's share is `render / (cpu + render)`.**
  The `render` column appears only while it is non-zero.
- Swap happens inside `render_frame_and_present`, so a frame boundary
  falls inside the render scope: the part after Swap (little) counts to
  the next frame. Means over many frames are unaffected.
- Once a second the feature logs, over the frames since its last line:

  ```text
  render-timing: 30 frames; ms/frame render 14.20 = frame 0.31 + game_views 0.12 + views 0.40 + game_view 1.05 + scene_passes 9.80 + light_passes 2.52; calls/frame game_views 1.0 views 1.0 game_view 1.0 scene_passes 1.0 light_passes 6.0
  ```

  (illustrative numbers). Each stage is **exclusive**: its own code and
  every guest function it calls, except the stages listed after it. So
  `scene_passes` is all scene passes except the dynamic lights, which are
  `light_passes`; `game_view` is a view's setup and UI composition outside
  its scene passes; `frame` is the fallback screen (loading, menus with
  no game view) and the present. The stages add up exactly to `render`,
  because both read the same clock (a test pins that). `calls/frame`
  counts entries: `game_view` is the number of views, `light_passes` the
  dynamic lights drawn.
- Interrupt service routines delivered inside the owner's slice are still
  charged to it; they are short.

**The `d3d` phase and the scene-pass calls.**

- Every D3D API entry (our `d3d` layer's HLE overrides) on the game thread
  is now its own phase, `d3d` (`d3d_us` in the CSV), minus `decode` and
  `render-wait` inside it. It used to be inside `cpu` or `render`. With
  render-timing on, `render` is guest code only and the overlay shows
  `cpu X ren Y d3d Z`; with it off the overlay's `cpu` still includes
  `d3d`. Guest-thread work is `cpu + render + d3d`.
- The `render-timing:` line now reads `render R d3d D = <stages>`: `R`
  and `D` are the guest code and the D3D entries called from inside the
  render front-end, and each stage figure is both together. Ending
  `foreign N` counts calls a frame to the scene callees from anywhere
  other than the scene passes (see overhead).
- Under it, the 15 most expensive calls in `render_scene_passes`, one
  per line, by ms/frame, then the scene passes' own code:

  ```text
  render-timing:   site 0x0002c06a -> 0x0002c370 3.21 ms (guest 2.90 d3d 0.31) 1.0 calls
  ```

  `site` is the address of the `call` instruction in
  `render_scene_passes`, `->` the function it calls. The time is
  exclusive like the stages: everything from that call until the next
  call of the sequence, except a nested stage (the per-light passes in
  `0x00031c80`'s call are `light_passes`, not that site's). It also holds
  the few instructions of the scene passes between that call and the
  next. The sites add up, with `scene_passes own code`, to the
  `scene_passes` figure. `calls` is per frame: a site runs once per
  view, so 1.0 in one view, 4.0 in four-player split screen.
- The sites come from observers before the 37 callees, which see every
  call to them; only the ones returning into `render_scene_passes`
  count. The others (`foreign`) still pay an observer and leave the
  direct-call path; they are charged to whatever site or stage contains
  them. Before-only observers need no shadow call, so each costs tens of
  nanoseconds: 10,000 foreign calls a frame would be a fraction of a ms.

For the scene list A/B, use the `game_view` stage for the object gather
and setup before `render_scene_passes`. Within `render_scene_passes`, the
relevant sites are `0x0002bd2d` (transparent geometry and effect producers),
`0x0002c03e` (dynamic-light bookkeeping) and `0x0002c19a` (transparent
ordering and draw handlers). Compare those times alongside the list
counters with `impl.render.native=off` against the default. The
`native_scene_lists_object_setup_*` columns count whole setup calls,
including their child traversal. Use the refusal columns to separate
resource I/O, light-probe work and animated poses from the accepted
calls in a capture. See [native-renderer.md](../../graphics/native-renderer.md#scene-list-builders).

**Overhead.** Twelve observer calls per frame plus two per view and per
light, each a mutex and a clock read, a few microseconds a frame; plus
the hooked functions leaving the direct-call path. The split is valid to
that precision; compare `fps` with the feature off to see the cost.

Whether the front-end could run one frame behind the simulation on its
own thread, and the `render-census` feature that measures which pages
the two share, are in [pipelined-render.md](../../graphics/pipelined-render.md).

### What scales with draws on the render thread (2026-11-17)

Old Mombasa, presents 4000-7000, 1x, headless with the render thread,
500 Hz sampler on both threads. Frames bucketed by draws (ms per frame):

| draws | frames | total | cpu | d3d | render-wait | encode | submit | submits |
|---|---|---|---|---|---|---|---|---|
| 250 | 684 | 19.3 | 15.9 | 2.7 | 0.4 | 3.2 | 5.0 | 37 |
| 1000 | 3035 | 57.6 | 42.4 | 10.7 | 2.5 | 15.0 | 22.9 | 46 |
| 2000 | 105 | 52.6 | 33.8 | 10.9 | 5.2 | 18.5 | 22.2 | 65 |

At ~1000 draws the game thread is the critical path (cpu + d3d + decode
+ render-wait = total); the render thread (encode + submit, ~38 ms) runs
beside it with a third of its samples idle. Past ~2000 draws the render
thread's work outgrows the frame, `render-wait` climbs (35-107 ms in the
worst seconds) and the frame rate falls off: the cliff is render-thread
throughput. Its busy samples (38.2k):

- **per render pass, ~26%**: wgpu's `begin_render_pass` into Metal (AGX
  render-context init) 5.6k, tracker setup and teardown (`UsageScope`
  drop, `set_size`, `new_pooled`) 2.4k, `open_pass` and
  `begin_encoding` (a Metal command buffer per pass) 1.8k;
- **per submission and command buffer, ~17%**: `commit` 2.4k,
  `Queue::maintain` 3.1k (dropping staging buffers and trackers);
- **staging buffers, ~11%**: `AGXBuffer initWithDevice` 2.8k and
  `dealloc` 1.5k. 64% of them were the upload rings' 2 MiB
  `write_buffer_with` views, one per ring per submission whatever they
  held; 28% persistent vertex uploads (one per new stream, exact size);
- **per draw, ~25%**: recording (`Renderer::record` 4.1k), shader
  preparation (4.7k, uniform staging included) and wgpu's per-draw
  encode (pipeline, bind groups, vertex buffers, `drawIndexed`: 2.7k).

The `GfxBackend::submit` callers' shares are misleading on their own: a
submit encodes the pending scene pass too, so `depth_level_texture` (6.0k
inclusive) and `mirror_store` (5.0k) mostly carry scene draws. What they
added themselves is submissions: `mirror_store`, `mirror_load` and the
GPU depth alias each submitted twice (the pending pass, then their own
encoder), and with `depth_level_texture` and `clear_pixel_count` they were
half the render thread's `Queue::submit` samples (3.4k of 7.0k).

The 16.5% `semaphore_wait_trap` is the render thread idle in its command
channel's `recv`, not the game thread; the game thread's own waits for
replies are 579 samples (1%).

Changes:

- the upload rings stage on the CPU and flush one exactly sized queue
  write (`gfx/src/ring.rs`). `tests/staging_cost.rs`: a 2 MiB view costs
  about 130 us against about 20 us for an exact one, submit share
  included;
- side work rides the open encoder (`GfxBackend::set_defer_side_passes`,
  off under a render scale);
- frames.csv gains `passes` (render and compute passes gfx begins) and
  `queue_writes` (staging buffers from gfx's queued writes).

Not changed, and next: the render-pass count itself. Each costs ~40-75
us of wgpu and Metal CPU; `passes` in the next capture says how many a
1000-draw frame has and, against `draws` and `submits`, where they come
from (count passes and sums for visibility tests, splits after partly
masked draws, side passes, clears).

### Render passes by cause (2026-11-17, second round)

The branch measured on the same scene: `passes` 195 a frame at ~1000
draws (3035 frames), 440-466 at 1250-1500, 312 at 2000 with 537
`queue_writes`; encode + submit 23.8 ms at 1000 draws against 37.9 on
master. `passes` grows with draws at about 0.2 a draw, while `submits`
stays near 28-48 a frame. Visibility tests submit at every
`EndVisibilityTest` (the report flushed), so they are bounded by the
submits; side passes (mirror, depth aliases) were a few a frame in the
profile (`GuestWords` and `DepthBits` sample counts). What grows with
draws is the merged pass's splits: a draw after a partly masked draw
(Halo's multipass shaders alternate RGB-only, alpha-only and full
writes) starts a render pass of its own, the Apple-GPU workaround in
graphics.md "Merged passes".

The frame stats now split `passes` by cause:

| column | passes |
|---|---|
| `pass_mask` | a draw after one that wrote some colour channels but not all, writing others |
| `pass_nocolor` | the same after a draw that wrote no colour channel |
| `pass_discard` | a draw that can `discard`, after a partly masked draw with the same mask (fast rule), or every one (exact rule) |
| `pass_counted` | each visibility test's count pass and sum, and the pass the draws after it resume in |
| `pass_side` | alias mirror stores and loads, depth aliases and levels, masked clears |

The rest of `passes` is one render pass per pending pass (target
switches, clears that start a pass) and the present.

Changes in this round:

- A counted draw alone in its run that writes nothing (the flare
  tests: colour writes off, no depth or stencil writes) no longer gets
  a render pass after its count pass: it would load and store the
  attachments unchanged. One pass fewer per flare test
  (`gfx::renderer::set_skip_idle_counted` restores it for tests).
- The report of a visibility test goes into the open encoder like the
  other side work, so a test no longer submits on its own (it reaches the
  GPU with the next submission, at the latest the present; Halo reads it
  two frames later). A read, or the render thread's poll for a count the
  guest is spinning on, submits it first. With side passes not deferred (a render scale) it
  submits as before.
- New persistent vertex and index buffers (vertex-cache snapshots) are
  written through one staging ring per submission and buffer copies,
  instead of a queue write (a staging buffer) each: most of the 537
  `queue_writes` at 2000 draws. `Renderer::set_staged_uploads(false)`
  restores the queue writes (tests).
- The loose merge rule (opt-in then, the default since 2026-11-18; not
  exact) drops the merge-rule splits, to measure what `pass_mask` + `pass_nocolor` + `pass_discard`
  cost and, with `H2_GFX_MERGE_CHECK`, which pixels they protect.

The mask splits are left as they are: each one exists because merged
and per-draw rendering differ on Apple GPUs without it, and no test can
show a replacement gives the same pixels.

### Buffer count and per-pass bookkeeping (2026-11-17, third round)

Master on the same scene: 30.8 ms a frame at ~1000 draws (encode 6.3,
submit 11.2, 193 passes), 44 ms at 2000 (encode 10.4, submit 17.4, 297
passes). The render thread's self time had `madvise` 10%, `memset`
5.5%, `BufferUsageScope::clear` 4%, `Vec<Option<Arc<Buffer>>>::resize`
2.7%.

wgpu-core 30 sizes every pass's usage scope to the buffer tracker
index allocator's high-water mark: `Device::new_usage_scope` ->
`UsageScope::new_pooled` -> `BufferUsageScope::set_size(
tracker_indices.buffers.size())`, where `size()` is `next_index`, the
most buffers ever alive at once (freed indices are reused, the mark
never drops). `set_size` resizes the state and `Option<Arc<Buffer>>`
vectors and the ownership bit vector to that length, and `clear` at the
pass's end empties them again, so every pass memsets and walks one
entry per buffer the device has held, used or not. Command-buffer
trackers (`BufferTracker::set_size`) and submission triage do the same
per command buffer and submission.

The persistent vertex cache made that mark large: a buffer per
vertex-cache snapshot and index list (256-byte classes up), up to the
128 MiB budget, plus free buffers up to half of it. A test scene of 600
new streams a frame for ten frames (`persistent_pool.rs`,
`slabs_keep_the_buffer_count_small`) holds 10,800 buffers.

`tests/pass_cost.rs` (ignored timing, `--release`), draw-less passes
with a load and a store, a submit per 8:

| buffers alive | us per pass |
|---|---|
| 0 | 52 |
| 1,000 | 47 |
| 10,000 | 46 |
| 30,000 | 49 |
| 100,000 | 76 |

About 0.3 us per pass per 1,000 buffers once past the noise, before any
draw binds one: at 100,000 buffers and ~300 passes that is ~7 ms of
render thread a frame, at 30,000 about 1 ms. The game's own count was
not measured here (no game runs); `Renderer::persistent_buffers` gives
the cache's share.

Changes:

- Persistent vertex and index regions are carved out of 8 MiB slabs
  (a buddy allocator, 256-byte to 8 MiB blocks; larger lists still get
  a buffer of their own). The test scene holds 5 slabs instead of
  10,800 buffers. Same bytes at the offsets the draws bind (the pool
  tests compare every region with a buffer per id, staged and not,
  through evictions and reuse). `Renderer::set_vertex_slabs(false)`
  restores a buffer per region (tests).
- A draw's 3 KiB constant block (`ShaderInputs::constants`) was
  allocated on the game thread and freed on the render thread, every
  draw; the allocator handed the freed pages back with `madvise`. The
  render thread now returns the blocks once per batch and the device
  overwrites a spare one (`shader::spare_constants`, up to 2,048 kept).

Not done: the `memmove` in `flush_uploads` is `queue.write_buffer`
copying the rings' staged bytes into wgpu's staging buffer. Writing in
place needs mapped ring buffers (`MAP_WRITE` on a vertex or uniform
buffer is `MAPPABLE_PRIMARY_BUFFERS`, and each needs its own map/unmap
cycle across submissions); left for a later round.

### Game-thread D3D work (2026-11-17, fourth round)

After round 3 the game thread is the limit: 23.8 ms CPU a frame at 1000
draws, 29.9 at 2000. In a 29.9k-sample profile of the main thread at
2000 draws (about one sample per microsecond of a frame) `Device` is
about 5.4k. Cut, with the backend call streams unchanged:

| what | samples | change |
|---|---|---|
| `flush_for_texture` per sampled texture | ~250 of 329 | the answer per texture is kept with its header words and an epoch the render-to-texture targets bump when one is added, removed or becomes dirty; while both match, only the mirror flush runs |
| `ThreadedBackend::draw_owned` malloc | 127 | the render thread hands emptied draw boxes back once per batch; the next draws fill them |
| `mirror_supersede` `_bzero` | 85 | every draw into a level zeroed the mirror's per-byte state over the whole level; the span zeroed last stays zero until a take marks bytes newer, so a repeat is skipped |
| `GuestAdapter::read_u32` `memmove` | 79 | a fixed-size read, a load on the guest's fast path |
| `DrawStats::add` | ~60 of 99 | `boot`'s draw diagnostics (on in `h2 play` as well as headless) count into Fx maps instead of SipHash ones |

About 600 samples, ~0.6 ms a frame at 2000 draws (2% of the game
thread). Left: `GfxJudge` (308: the program key the render thread
builds again, then a hash lookup comparing whole keys), `submit_inner`
itself (388), `drain`'s push-buffer decode and `Immediate::apply`
(~210), `read_stream` and `vertex_setup` (~360 self).

Fifth round, same profile:

- `submit_inner`'s own time is mostly `shader_inputs`, inlined: per
  draw it converted all 192 constant registers into a fresh 3 KiB block
  and boxed the 60-word PSDEF and the 57 combiner words (two more small
  allocations, freed on the render thread). `ShaderInputs` now holds
  them in `Arc`s: the constant block is shared until a register is
  written (a generation every constant write bumps, push-buffer methods
  included), and the PSDEF and combiner words are shared while they
  read the same as the draw before. Recorded draws compare equal as
  before; the blocks are only shared.
- `GfxJudge` is about 150 ns a draw (key build ~70, lookup with a whole
  key compare ~55, hash ~15). A memo by state would need the same
  comparisons: an API draw's state words are a fresh `Arc` per draw, so
  there is no cheap generation covering every input `program_key` reads.
  Left as it is.
- `decode_one_into` and `Immediate::apply` are already a header decode
  and one `match`; the 4-byte reads they make went through `memmove`
  until round 4's `read_u32`. Left as they are.
- `DrawStats` is read by the run report printed at exit (`h2 play`
  included) and by `H2_BOOT_GFX_SHADOWS`'s shadow-frame screenshots, so
  it is not diagnostics-only; round 4 made it Fx maps. Left on.

## Reading these numbers on another machine

`h2 play --frame-stats` (or `H2_FRAME_STATS=1`) prints the same split
once a second and the ten worst frames at exit. Compare `cpu` with
`encode + submit + gpu-wait`. If `cpu` dominates, rows 1-3 are the work.
If one frame's `encode` is hundreds of ms with `pipelines` above 0 in the
exit report, it is the compile hitch. If the worst frames have normal
counters and every phase is up, the host was busy.
