# Main-menu frame budget

Where the wall time of one main-menu frame goes on master plus the boot
runner, and a ranked plan for getting from under 2 fps to 30 fps.
Measured 2026-11-15 on master 4620ba0 + `boot` (6eaa2cd), with a dylib
built from 4620ba0, Apple Silicon (AGX G13), release build. The tools are
in [perf.md](perf.md): the timeline, `h2 profile budget` and the sampler.
The runtime's accessor work and its own menu profile are in
[performance.md](performance.md).
The in-level budget and its worst frames are in
[frame-budget.md](frame-budget.md).

Since then master has landed plan items 1 and 2 (draws batched per
target into one submission, upload rings and cached texture bind groups,
592686a) and item 4 (deferred VFS write metadata, 6eb79b6). Those target
rows 2, 3, 4 and 6 of the budget. Items 3 (the inline RAM path,
`H2_FASTMEM`) and 5 (native x87, `H2_X87_NATIVE`) have landed too and are
on by default. The flyover section below is the latest menu measurement
(about 11-16 fps on a quiet machine, with the render thread); the budget
and plan after it are the original 4620ba0 numbers. Re-measure before
acting on any row.

## The city flyover at menu start (re-measured)

Measured on master 064a955 and ac9fb31. `h2 run` with `H2_BOOT_GFX` (no
PNGs, merged passes, fastmem, native x87), input
`START@f310,DOWN@f408,A@f419`, fresh data, frame stats per 100 presents
(`H2_FRAME_STATS_CSV`), runs one at a time in the bench gate. The load
average is given for each run.

**Why the menu starts slow and speeds up later.** It is the scene, not a
warm-up. Presents 200-500 (the title, then the main menu fading in) show
the 3D city flyover at 1400-1700 draws a frame. From about present 500
the view has 850-1100 draws, and fps rises with it. Nothing grows from
frame to frame.

Quiet machine (load 3-6), 064a955, warm pipeline cache, fps per 100 presents:

| presents | draws/frame | render thread off | `H2_RENDER_THREAD=1` |
|---|---:|---:|---:|
| 101-200 | 755 | 11.8 | 18.3 |
| 201-300 | 1521 | 7.5 | 12.5 |
| 301-400 | 1704 | 7.3 | 11.4 |
| 401-500 | 1435 | 9.1 | 12.6 |
| 501-600 | 1132 | 12.1 | 15.9 |
| 601-700 | 1072 | 13.3 | 16.7 |

A repeat pair gave 11.31 and 15.38 fps over presents 440-700, with all 29
sampled PNGs byte-identical. On ac9fb31 (load 14-67) the pair gave 5.70
and 10.69 fps over the same window: the same order, with more noise.

The ranked costs on the flyover with the render thread off, from a 500 Hz
sample plus the timeline of presents 250-330 (064a955; load 120-170, so
read the shares, not the ms):

| # | cost | share | notes |
|---|---|---:|---|
| 1 | guest `slice`: generated code, accessors, x87 helpers, dispatch | 51% | the remaining floor: 55-80 ms a frame on a quiet machine, with the render thread on or off |
| 2 | the fog pass's depth-alias texture (`Device::depth_texture`, then `read_depth`, then `poll(wait)`): one full GPU sync a frame | ~13% | counted as `decode` in frame stats, not `gpu-wait`, because it happens inside the push-buffer drain. The render thread does it on its side (`reads: 0`) |
| 3 | `queue submit` (18-28 a frame), mostly Metal's `commit` ending render encoders | 12% | on the render thread when it is on |
| 4 | `hle call` self (the D3D entry wrappers) | 9% | |
| 5 | shader prepare, draw recording, pass encode | ~9% | on the render thread when it is on |

Ruled out on this evidence:

- **frag_depth (1a5aa8d) and other GPU fill.** cae8a4a and 1a5aa8d give
  the same menu fps within the noise (12.8-14.9 against 10.7-13.6 at
  presents 601-900, load 23-41). The `D3DSWAP_FINISH` wait stays under 1
  ms a frame, so the GPU keeps up at 640x480. GPU time shows only through
  row 2's sync.
- **Pipeline compiles and the prewarm.** Cold against warm cache, render
  thread off, presents 101-200 on a quiet machine: 11.4 against 11.8 fps.
  A cold cache costs 7-31 frames that wait on a pipeline, all before
  present 400, and later frames match. With `H2_PIPELINE_CACHE=off
  H2_GFX_COMPILE_THREADS=1` (load ~100) 13 frames wait, all before
  present 400.
- **Bink.** No movie is decoded during the menu (none in the profile).

## Method

The scenario is the menu-idle recipe in docs/runtime/BOOT.md. It uses fresh data
and the input script `START@f310,DOWN@f408,A@f419`, and measures presents
440-560: the main menu after its fade-in, with the Default profile
chosen. `H2_BOOT_GFX` with `H2_BOOT_GFX_EVERY=never` keeps the wgpu/Metal
path and writes no PNGs. A scratch overlay (not committed) adds two
things to `boot::run`:

- `H2_PERF_FROM=N` opens `perf::Session::from_env()` at present N
  (`H2_TIMELINE`, `H2_PROFILE`). It finishes at the end of the run and
  prints the window's wall and logical time per present.
- A `boot: input + audio pump` span around the runner's per-slice
  `xapi::service_input` and `dsound_glue::advance`.

`H2_PERF_FROM` has since landed on master (docs/perf/perf.md, "Boot
hook"); the pump span has not.

| run | what | window mean / median ms | load avg |
|---|---|---|---|
| A | timeline (default cats) + sampler at 1 kHz | 1488 / 1523 | 50-120 |
| B | timeline `all` (HLE calls split out), no sampler | 620 / 582 | 20-30 |
| C | nothing recorded | 637 / 636 | 20-30 |
| D | as B, plus the VFS fix below | 497 / 439 | 20-30 |

B and C agree within noise, so the timeline, even with `hle`, costs
nothing measurable. A ran while other builds were running, which made
everything 2.4x slower, and slower unevenly: fsync-heavy file I/O and
Metal suffer most. So the ms below come from B, and A's sampler is used
only for fractions *inside* B's `slice` self time, where the two runs
agree (A: 36.9% of the frame once file I/O is taken out; B: 38.1%). BOOT.md's
quiet-machine number is 484 ms median. Expect every ms here to scale
down by roughly 0.8 on an idle machine. The percentages are the
portable part.

Reproduce (with the hook above added to `boot::run`):

```
menu.sh() { H2_BOOT_GFX=$R/gfx H2_BOOT_GFX_EVERY=never \
  H2_BOOT_INPUT="START@f310,DOWN@f408,A@f419" \
  H2_BOOT_FPS_FROM=440 H2_BOOT_STOP_AT_PRESENT=560 "$@" \
  h2 run disc/halo2.iso --out $TMPDIR/h2boot --data $R/data --fresh-data; }
R=$TMPDIR/b menu.sh env H2_PERF_FROM=440 H2_TIMELINE=$R/trace.json H2_TIMELINE_CATS=all
h2 profile budget $R/trace.json
R=$TMPDIR/a menu.sh env H2_PERF_FROM=440 H2_PROFILE=$R/prof H2_TIMELINE=$R/trace.json
```

## What a menu frame is

Per frame (all runs, identical because the schedule is deterministic):

- **Logical time 33.3 ms, exactly 2 vblanks.** The game runs at its
  30 Hz cadence in logical time.
- **2.56 M guest accesses** charged (`ACCESSES_PER_TICK` = 100, so
  2.6 ms of logical time) in 45 slices of the game thread. No other
  guest thread runs: thread 3 sits in one long wait.
- **4.7 idle advances skip 30.7 ms of logical time.** The frame wait
  yields, and the clock jumps to the next vblank.
- **1262 draws, each its own command encoder and `queue.submit`**, plus
  7 surface readbacks, 7 surface writes, 5 snapshots, 6 texture uploads
  and one `D3DSWAP_FINISH` GPU wait.
- **About 1.7 `NtWriteFile` calls**: Halo 2 filling its cache partition
  in the background (fresh data).
- Contended guest-memory-lock waits: **none**, on any thread. The
  scheduler (`schedule` self time) is 0.1% and DPCs are 0.1%. There is no
  pacer in the benchmark.

**Pacing is not the problem.** Logical/wall time is 0.054: the guest
does 2.6 ms of logical work per 33.3 ms logical frame. The rest of the
logical frame is crossed by free idle advances, not by work and not by
wall-clock waiting. The game does not wait on a logical vblank that runs
slower than real time, and logical time never charges it extra ticks of
simulation (`0x12b0e0` sees 2 vblanks a frame, so 1 tick at 30 Hz). The
other way round: once a frame takes less than 33 ms of wall time, the
runner needs `H2_BOOT_REALTIME` (the kernel pacer), or the menu will run
faster than 30 fps.

## Budget (run B, 620 ms mean)

Self time on the game thread, from `h2 profile budget`. Rows marked *
split B's `slice` self time (238 ms) by run A's sampler fractions.

| # | cost | ms/frame | % | owner |
|---|---|---|---|---|
| 1 | Runtime guest-memory accessors and the write watch\*: `Guest::checked_start` 6.8, `h2abi_v1_read` 4.5, `h2abi_v1_write` 3.2, `read_bytes` 2.7, `WriteWatch::mark` 1.7, `write_bytes` 1.5 (% of A) | ~134 | ~21.6% | runtime |
| 2 | `queue submit`: 1265 a frame, ~100 us each | 124 | 20.0% | gfx-perf |
| 3 | `NtWriteFile` (`hle call` pc `0xa0ff03b0`): VFS stores the metadata record with two fsyncs per write | 84 | 13.6% | vfs (fixed here) |
| 4 | `shader prepare`: nv2a `prepare`, per-draw uniform/bind-group buffers (`AGXBuffer init` 7.1% and `dealloc` 2.9% of A) | 76 | 12.3% | gfx-perf (with d3d nv2a) |
| 5 | Guest code\*: flat, the top function `0x0013b390` is 0.5% own time; inclusive leaders are `0x00016b90`, `0x0013b390`, `0x00224550`, `0x00246cd0`, `0x00372d6c` | ~43 | ~7.0% | codegen-opt |
| 6 | `render encode`: wgpu render pass per draw | 41 | 6.6% | gfx-perf |
| 7 | x87 soft-float\*: `Wf<8>` div/mul/add and the `atanh`/`log2`/`expm1`/`sin`/`cos` series | ~31 | ~5.0% | x87, codegen-opt |
| 8 | GPU round trips: `surface readback` 14.9 (7 a frame, ~2 ms each), `GPU wait` 3.3, `surface write` 3.2 | 21 | 3.4% | d3d (BOOT.md blocker 3), gfx-perf |
| 9 | Other HLE overrides: the d3d write-barrier shadows `0xa0ff4xxx`, D3D entries such as `0x003f5cf0` (1960 calls a frame) | 15 | 2.5% | d3d, boot |
| 10 | Runner and dispatch\*: `run_from`, `resolve`, `resolve_resume`, `handler::execute` | ~13 | ~2.1% | runtime |
| 11 | `pushbuf drain` self: decode and state, without the draws | 12 | 1.9% | pushbuf-perf |
| 12 | Boot fallback interpreter (`boot::fallback::trampoline`)\* | ~7 | ~1.1% | analyze / boot |
| 13 | `draw` self (d3d resolve, splitting), texture upload, snapshots, pipeline creation (0.1 a frame) | 8 | 1.3% | d3d, gfx |
| 14 | Scheduler, DPCs, vblank, input/audio pump, Swap | 1.6 | 0.3% | kernel, boot |

The rows add to about 100%. A reads lower than B (1488 ms) but splits the
same way: gfx 48.9%, runtime 28.0%, kernel (file writes) 10.8%, guest
6.6%, d3d 4.6%, fallback 1.0%.

Uncertainty:

- The starred rows assume A's internal split of `slice` holds at B's
  load. A's sampler pauses the thread about 58 us per sample, a 6% tax
  spread evenly.
- The accessor row includes the write watch and the bounds and
  permission checks. Which of those dominates needs the `hot-loop/memory`
  bench.
- Row 3 is a fresh-data artefact in size (the cache fill), but the cost
  per write is real for any save or cache write.

## Plan to 30 fps (33 ms)

Ranked by expected win per unit of effort. The wins come from B's
620 ms and are not additive to the ms: each assumes the ones before it.

| # | change | owner | expected win | confidence |
|---|---|---|---|---|
| 1 | **Batch draws** (landed, 592686a): one command encoder and one submit per render-target pass or frame, not per draw | gfx-perf | 124 -> ~5 ms (-120) | high: 1265 submits are the cost, and Metal's per-submit overhead is well known |
| 2 | **Stop allocating per draw** (landed, 592686a): a frame ring buffer for uniforms, vertices and indices; cached bind groups keyed by texture views | gfx-perf (d3d nv2a `prepare`) | 76+41 -> ~15 ms (-100) | medium |
| 3 | **Fast accessors:** an inline host-pointer path in generated code for mapped RAM (one bounds check, then a load or store), with the write watch as a bit test on the store side only | runtime | 134 -> ~20 ms (-110) | medium: it depends on how much of `checked_start` the inline path can skip |
| 4 | **VFS write metadata** deferred to flush/close (landed, 6eb79b6) | vfs | -84 during cache fill, about 0 afterwards | high |
| 5 | **x87:** native f64 for ops whose precision control or operands do not need 64-bit mantissas, and native transcendentals where the guest result is rounded to double anyway | x87 / codegen-opt | 31 -> ~8 ms (-23) | low to medium: a fidelity trade-off |
| 6 | **RTT write-back** reported as the device's own write (BOOT.md blocker 3) so render textures are not read back and re-uploaded; drop `D3DSWAP_FINISH`'s full poll | d3d, gfx-perf | 21 -> ~3 ms (-18) | medium |
| 7 | **Guest code** IR passes (flags, dead stores, register promotion across blocks) | codegen-opt | 43 -> ~25 ms (-18) | low: the profile is flat, with no hot spot |
| 8 | Push-buffer decode | pushbuf-perf | 12 -> ~6 ms (-6) | medium, but small |
| 9 | Hot overrides: no single guest function is worth it on this screen (top inclusive 2.3% of A, own 0.5%) | hot-overrides | <10 ms | high that it is small here; levels may differ |

Where that lands: about 620 - 120 - 100 - 110 - 84 - 23 - 18 - 18 - 6
= ~140 ms of what was measured. The leftovers do not shrink by the same
factors: other HLE, dispatch, fallback, and draw resolve, about 45 ms,
plus the residue of each item. Honest reading:

- Items 1-4 (gfx batching, per-draw allocation, the accessor path,
  vfs) take the menu from ~620 ms to roughly 150-250 ms, 4-7 fps.
- 30 fps needs the guest side, accessors plus guest code plus x87, at
  under ~20 ms for 2.56 M accesses, about 8 ns per access all in. That
  is near-native code with inline memory access. It means items 3, 5
  and 7 together, not a tuning pass.
- On the gfx side, 1262 draws at under ~5 us each.
- At that point add the real-time pacer (`H2_BOOT_REALTIME`) to hold
  30 fps rather than run past it.

## Recommendations per area

- **gfx-perf.** The biggest lever (rows 2, 4, 6: 39% of the frame).
  1. One encoder and one submit per pass.
  2. A per-frame upload ring instead of `create_buffer` per draw, since
     `AGXBuffer` init/dealloc is 10% of A on its own.
  3. Bind groups cached by key.

  Watch `queue submit`, `shader prepare` and `render encode` in
  `h2 profile budget`. Readbacks: 7 a frame at ~2 ms each, all
  render-to-texture write-back.
- **runtime.** Rows 1 and 10, 24%. `checked_start` is the single
  biggest host function after Metal's buffer allocator. The menu does
  2.56 M accesses a frame, so every ns per access is 2.6 ms per frame.
  `WriteWatch::mark` is 1.7% on its own: consider skipping it on pages
  no subscriber watches, with a per-page bit before the per-subscriber
  loop.
- **codegen-opt.** Guest code is 7% and flat; x87 helpers are 5%. Native
  f64 lowering where precision allows is likely the better win of the
  two on this screen.
- **pushbuf-perf.** Decode is 1.9% of the frame (12 ms); the draws it
  issues are gfx's. It is worth it later, not first.
- **hot-overrides.** Nothing on the menu justifies an override. Re-profile
  a level.
- **boot.** Add the `H2_PERF_FROM` session hook and the pump span (the
  overlay's ~30 lines) so these runs need no overlay. Its kernel tracer
  (`trace.rs`) writes an unbuffered TSV line per kernel call into
  `--out`, the shared dylib directory. That is cheap at menu call rates,
  but every run sharing one `--out` writes the same file.
- **d3d.** Blocker 3 (RTT write-back seen as a CPU write) costs about 3%
  in readbacks and re-uploads.

## Instrumentation added

Landed with this budget, with tests (docs/perf/perf.md lists every event):

- `timeline`: `sched` and `lock` categories.
- `kernel`:
  - a `schedule` span per `Machine::run` iteration;
  - a `pacer wait` span;
  - the `logical us` counter after every charge and idle advance;
  - `slice` carries `accesses`, and the idle advance carries `ticks`.
- `runtime`: `Runtime::memory()` takes the lock with `try_lock` first.
  A contended wait goes through a cold path that counts it
  (`Runtime::memory_contention()`) and records a `memory lock wait` span.
- `d3d` and `gfx` spans:
  - per-draw work: `draw`, `shader prepare`, `render encode`;
  - pipeline-cache misses: `pipeline create`;
  - texture and surface transfers: `texture upload`, `surface write`,
    `surface snapshot`;
  - GPU round trips: `surface readback`, `pixel count readback`,
    `GPU wait`.
- `perf::budget` and `h2 profile budget`: per-frame self time per span,
  logical time per frame, vblanks, slices, accesses, idle advances and
  lock waits, from any saved trace.

## Deferred VFS write metadata (landed)

`vfs::host::HostBackend::write_at` and `set_len` stored the file's
metadata record (write and change times, the archive bit) durably on
every call: a staged file, `sync_all`, a rename and a directory
`sync_all`. Data writes themselves are not synced. The sampler had 10.7%
of run A in `store_basic`/`sync_directory`; B's `hle call` for
`NtWriteFile` is 84 ms a frame (~50 ms per write). The change keeps that
basic info in memory per path, the same way access times already were,
and stores it at the next flush, close (cleanup) or drop. It is also
stored once it has been pending for 1 s, by the write path or by a
flusher thread, and by `vfs::flush_pending_metadata()` at `h2 run`'s
normal exit. So a crash loses at most ~1 s of timestamp and
archive-bit updates, never data (docs/runtime/storage.md "Write metadata").
`stat` sees the pending info at once. Synthetic bench: 0.09 ms per
`write_at` against 16.3 ms when the record is stored every write.

Result (run D, same load as B, same 1.73 writes a frame):

| | B | D |
|---|---|---|
| `NtWriteFile`, inclusive, ms/frame | 84.5 | 5.4 |
| `hle call` self, ms/frame | 99.6 | 18.2 |
| frame mean / median, ms | 620 / 582 | 497 / 439 |
| fps over presents 440-560 | 1.61 | 2.01 |

The other rows are unchanged within noise (gfx, accessors). Most of the
mean drop is the 81 ms of fsyncs. The rest is run-to-run noise at this
load, which is around +/-30 ms.
