# Gameplay frame budget

Where the wall time of an in-level frame goes, what the worst frames are
made of, and what it would take to get the worst frame under 33 ms (never
below 30 fps). [perf-budget.md](perf-budget.md) is the same for the main
menu. This one is a level.

Measured 2026-11-15 on master 9a39ed7 plus this branch's stats (the stats
add nothing to the guest's schedule). Dylib built from 9a39ed7, Apple
Silicon, release `h2` built with `--features perf`. The machine was
shared: load average 35-67 during the runs, from other lanes' builds and
benches. Expect every ms to be lower on a quiet machine. The shares and
the hitch causes carry over.

## What "gameplay" is here

The smallest level reachable today is The Heretic (`00a_introduction`).
Scripted input (docs/BOOT.md, milestone 19) goes through the menus and
loads it, and the first level frame is about present 4210. The Heretic is
an in-engine cinematic: the full 3D level renderer runs (BSP, characters,
shadow maps, cinematic lighting), but there is no player control. The
first level with control is the Armory (`01a_tutorial`), and nothing
reaches it yet: the intro has to finish and queue the next map
(level-preflight.md, risk 1). Simulation, AI and physics load will be
higher in real gameplay. Rendering load may differ either way.

The window is presents 4220-4520: 300 frames, 36 s of wall time on
master defaults.

```
I="START@f310,DOWN@f408,A@f419,A@f524,A@f585,A@f632,A@f700,A@f760,A@f820"
H2_BOOT_GFX=$R/gfx H2_BOOT_GFX_EVERY=4215 H2_BOOT_INPUT=$I \
H2_BOOT_FPS_FROM=4220 H2_BOOT_STOP_AT_PRESENT=4520 H2_GFX_MERGE_PASSES=1 \
H2_FRAME_STATS=1 H2_FRAME_STATS_CSV=$R/frames.csv H2_FRAME_STATS_FROM=4220 \
H2_PERF_FROM=4220 H2_PROFILE=$R/prof H2_PROFILE_HZ=250 \
H2_TIMELINE=$R/trace.json H2_TIMELINE_CATS=all \
h2 run disc/halo2.iso --out $OUT --data $R/data --fresh-data --slices 2000000
h2 profile budget $R/trace.json            # and --json for every frame
```

Three views of the same run:

- **Frame stats** (`H2_FRAME_STATS`, new on this branch; docs/app.md
  "Frame stats") split every frame into phases on the game thread and
  count what happened in it. This is the tool for users: `h2 play
  --frame-stats` shows the same split live.
- **Timeline** (`h2 profile budget`) gives self time per span, per frame.
- **The sampler**, at 250 Hz (a 1.5% pause tax), splits the guest
  `slice` span into guest code, accessors, x87 and dispatch.

`H2_GFX_MERGE_PASSES=1` is on in every run. Without it the backend
encodes and submits a pass per draw, which BOOT.md measures at about 3x
slower on the menu. On master, `h2 play` never merged: the variable was
only read for `H2_BOOT_GFX`'s headless backend. So the user's `h2 play`
numbers include a submit per draw. This branch makes `h2 play` honour
`H2_GFX_MERGE_PASSES=1` too (opt-in, because a few edge pixels differ).
Merged passes have since become the default for both, matching one pass
per draw on the menu (graphics.md, "Merged passes"), so
`H2_GFX_MERGE_PASSES=1` no longer changes anything.

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

Same window, same input, with `H2_FASTMEM=1 H2_X87_NATIVE=1
H2_ACCESS_COUNT=1` set for both build and run, plus `H2_GFX_MERGE_PASSES=1`
as before. These numbers are for measurement only: FASTMEM has a known P1
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
| 0 | **Merged passes in `h2 play`** (`H2_GFX_MERGE_PASSES=1`, this branch): about 300 draws a frame otherwise mean about 300 passes and submits, against 26 | gfx | follows p50 | not measured in the level; on the menu, merging was about 3x on gfx time. Expect -15 to -30 for a user on master's `h2 play` | medium |
| 1 | **Compile pipelines off the game thread, and prewarm them.** Create nv2a programs and render pipelines on a worker. Until one is ready, skip the draw for a frame or two, or block only on a deadline. Persist the keys seen (program hash plus pipeline key) and compile them at map load. Metal's own cache already makes a repeat compile ~5 ms (frame 4256), so the hitch is the first time a material or angle appears. | gfx, d3d nv2a | **721 -> ~80 ms** (-640): the only hitch over 110 ms in either run | ~2 | high: 665 of 721 ms is `pipeline create` |
| 2 | **FASTMEM on by default** once its P1 is fixed (with `H2_ACCESS_COUNT`) | runtime, emit | follows p50 | **-35 to -40** (119 -> 75 measured, with a load difference) | high: measured |
| 3 | **Fast exact FYL2X/F2XM1.** A table-driven or double-double first pass with a rounding-ambiguity check, keeping the `Wf<8>` series as the exact fallback for the rare hard cases. That is the correctly-rounded-libm pattern, and the guest result stays bit-exact. | x87 | follows p50 | **-10 to -12** (13.5 -> ~2) | medium: 97% of x87 time is these two; the exactness check is the work |
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

## Reading these numbers on another machine

`h2 play --frame-stats` (or `H2_FRAME_STATS=1`) prints the same split
once a second and the ten worst frames at exit. Compare `cpu` with
`encode + submit + gpu-wait`. If `cpu` dominates, rows 1-3 are the work.
If one frame's `encode` is hundreds of ms with `pipelines` above 0 in the
exit report, it is the compile hitch. If the worst frames have normal
counters and every phase is up, the host was busy.
