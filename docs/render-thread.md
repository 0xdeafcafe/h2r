# Render thread

On the Xbox the NV2A consumes the push buffer while the CPU goes on with
the next frame. Here every part of rendering ran on the guest's runner
thread, between guest instructions, so guest CPU work and host rendering
added up instead of overlapping. This page says where that work ran,
what a render thread may and may not take off the guest's thread without
changing anything the guest can see, what the render thread
(`d3d::ThreadedBackend`, on by default since 2026-11-15,
`H2_RENDER_THREAD=0` turns it off) does, and what it measured.

## Where the work runs today

`h2 run` and `h2 play` run every guest thread cooperatively on one host
thread, the runner (`kernel::Machine::run`; in `h2 play` the thread
named `guest`, docs/app.md). D3D is a set of HLE overrides
(`d3d::runtime_glue`), so all of the following runs on that thread,
inside an override, holding the guest memory lock and then the device
lock:

| work | where | when |
|---|---|---|
| push-buffer decode and inline state | `Device::sync_push_buffer`, `drain_until_callback` | at the start of every D3D entry, `MakeRequestedSpace`, `KickOff` |
| state translation, draw resolve (streams and indices copied out of guest memory) | `Device` draws (`device.rs`, `device/raster.rs`) | at each draw the drain or an API call reaches |
| texture decode | `Device` texture cache | the first draw that samples a changed texture |
| shader preparation: nv2a translation (cached by key), uniforms, bind groups | `Nv2aShaders::prepare` | every draw through `GfxBackend::draw` |
| recording into the pending pass (rings, pipelines) | `GfxBackend::record`, `gfx::Renderer::record` | every draw |
| encoding and submission | `GfxBackend::flush`, `submit` | target switch, surface operation, 1024 encoded passes, `Swap` |
| uploads, surface writes, snapshots, copies | `GfxBackend` | as the device asks |
| the present hook (PNG or the app's frame copy) | `GfxBackend::present_with_gamma` | `Swap` |
| `D3DSWAP_FINISH` GPU wait (`device.poll(wait_indefinitely)`) | same | Halo's one `Swap(0)` a frame |

The window thread in `h2 play` only shows the frame the hook published.
No other thread does rendering work.

Where the guest waits on the GPU (all synchronous today, all on the
runner):

- **Fences** (`SetFence`, `BlockOnTime`, the reference counters at
  `+0x2C`/`[+0x30]`): the device drains first, so every fence has
  completed when it is handed out (docs/d3d.md, "GPU model").
- **`BlockOnResource` and the busy check `0x3fb710`**: drain, then
  lazily write back the render-to-texture levels the resource overlaps:
  `read_surface`/`read_depth`, a GPU round trip.
- **`KickOff`**: drains and rewrites the NV2A register words
  (`NV_PFIFO_CACHE1_DMA_PUT/GET` equal, `NV_PGRAPH_STATUS` idle).
- **Push-buffer callbacks** (`0x24970`): drain, write back, call.
- **Pixel counters**: `read_pixel_count` waits for its report.
  `GetVisibilityTestResult` now polls instead (`try_pixel_count`) and
  answers `D3DERR_TESTINCOMPLETE` for a recent test whose count is not
  back (docs/d3d.md, "Visibility tests").
- **Depth aliases** (`0x509368`, the menu's fog pass): decoded from the
  depth buffer on the GPU (`GfxBackend`, 2026-11-15); before that
  `read_depth` of the depth buffer, cached by depth generation.
- **`Swap`**: present, then the full GPU wait for `D3DSWAP_FINISH`.
- **Vblank and the frame wait** (`0x12b2e0`) wait on the logical clock,
  never on the GPU or wall time (docs/d3d.md, "Vertical blank").

## What can move

The guest observes the GPU only through values: guest memory the device
writes (write-backs, register words, fence words, `Lock`), the answers of
D3D entries, and pixel counts. Everything the device computes from guest
memory has to stay where guest memory is current, on the runner, in
program order. Everything after that is host work whose only output is
pixels on host surfaces.

The seam is the `d3d::Backend` trait. A `ResolvedDraw` already holds no
guest pointers (docs/d3d.md, "Draws"): the device has copied the streams
and indices and translated the state. So:

- **Stays on the runner:** push-buffer decode, the pushed-state snapshot,
  state translation, draw resolve, texture decode, the device's whole
  model (RTT dirty bits, the lazy write-back merge, depth generations,
  fences, register words). Its order is the guest's program order.
- **Moves to the render thread:** every `Backend` call, run in the order
  issued on one queue: shader preparation, recording, encoding,
  submission, uploads, surface writes, snapshots, copies, present and the
  present hook, and the `D3DSWAP_FINISH` GPU wait.

Decoding the push buffer itself on another thread, the way the NV2A
fetches it, is not worth its risk here: the drain reads the state arrays
and resource memory at the moment of the draw (game code changes them
right after), it is 2% of the frame (docs/perf-budget.md, row 11), and it
would need real asynchronous fences, which Halo would then wait on.

## Keeping guest-visible semantics exact

Each `Backend` method falls in one of three classes
(`crates/d3d/src/threaded.rs`):

1. **Reads** (`read_surface`, `read_depth`, `read_pixel_count`): queued,
   and the runner waits for the answer. Everything queued before has run,
   so the bytes are what a synchronous backend returns. These were full
   GPU round trips already, so the wait adds the queue's backlog, no more.
   `try_pixel_count` is the exception: it answers from the counts already
   landed and never waits. A count is the same either way; only whether
   it is back yet depends on timing, and the device turns that into
   `D3DERR_TESTINCOMPLETE` only where the hardware could (docs/d3d.md,
   "Visibility tests"; `H2_D3D_VISIBILITY_POLL=0` waits instead).
2. **Handles** (`create_surface`, `create_texture_target`,
   `upload_texture`, `snapshot_surface`, `surface_texture`,
   `report_pixel_count`): the guest side hands out a proxy id at once; the
   render thread maps it to the inner backend's handle when it reaches
   the command. A proxy texture freed by the device is freed on the
   render thread after the work queued before the drop.
3. **Draw outcomes** are decided on the guest's side. The device's state
   depends on them: a drawn draw marks an RTT level dirty and bumps the
   depth generation, a skipped one records `Event::DrawSkipped`.
   `Backend::judge` gives a `DrawJudge`, made before the backend moves to
   the render thread, that answers what `draw` will return from the draw
   and whether each bound texture has a sampleable view (known from the
   texture's host format when its proxy is made). For `GfxBackend` it
   runs the same routing function `draw` does (`route`: built-in shader,
   provider, or skipped), and for the provider `Nv2aJudge`, which builds
   the same program key (`program_key`) and runs the same WGSL
   translation and validation (`translate`), cached by key the way
   `prepare` caches compiles. So an untranslatable draw is skipped with
   the same reason as with the synchronous backend, and the run goes on.
   The draw still goes to the render thread, because `GfxBackend`
   submits a pending pass for another target before it skips.
4. **Other outcomes** (`clear_rects`: `Drawn`; `write_surface`,
   `copy_surface`: `true`; `snapshot_surface`, `surface_texture`,
   `report_pixel_count`, `depth_alias_texture`: `Some`) are answered at
   once with the answer `GfxBackend` gives for every valid call.
5. **Checks.** If the render thread later gets an answer other than the
   one returned (a judge that disagrees, or one of the above), it is
   latched and the **next backend call fails** with a named error. The run
   then stops instead of continuing with a device that differs from a
   synchronous run. `ThreadedBackend::misses` counts them.
6. **Device-internal reads.** A texture over the depth buffer
   (`0x509368`, `0x50936C`: the menu's fog/distortion pass) used to be
   made by the device: `read_depth`, decode, `upload_texture`. Those
   bytes never reach guest memory, but the read made the guest wait for
   the whole queue once a frame. `Backend::depth_alias_texture` (default:
   the device does it as before) lets the render thread read back,
   decode and upload at that point in the queue, once per depth
   generation as the device did. Whether a description decodes depends
   only on the description, so the device checks it once per description
   on zero bytes and skips exactly where it skipped before. Since
   2026-11-15 `GfxBackend` makes the texture itself, on the GPU
   (`gfx::guest_words::GuestWords::depth_alias`, docs/d3d.md "Depth
   aliases on the GPU"), so neither thread reads the depth buffer back
   for the formats that pass covers; `H2_D3D_GPU_DEPTH_ALIAS=0` restores
   the read-back.

Against the requirements:

- **Fences and reference counters.** Unchanged. The device model still
  says every fence has completed when handed out, the register window
  still says the FIFO has fetched everything, and both remain true *as
  far as the guest can tell*: the only ways to look at GPU results are
  the reads in class 1, which wait.
- **RTT write-backs** (lazy since rtt-direct). The device decides when a
  level is written back exactly as before; the write-back's
  `read_surface` waits for the draws queued before it. In-place sampling
  of a level binds the surface's own view on the render thread, after the
  draws into it, as on the single queue before.
- **Guest reads of GPU-written memory, pixel counters.** Class 1.
- **Vblank and present pacing.** Vblanks, the frame wait and the swap
  count are the device's and the kernel's, on logical time, unchanged.
  The render thread finishes presents in order. `Swap` waits on wall time
  when the guest is more than two presents ahead
  (`MAX_FRAMES_IN_FLIGHT`), which bounds memory and latency. That wait,
  like every class-1 wait, is on host wall time and never charges logical
  time, so it changes no guest-visible value. `H2_BOOT_REALTIME` already
  makes logical time follow wall time; the render thread only lets the
  runner reach a vblank sooner.
- **Determinism.** The render thread never reads or writes guest memory,
  the logical clock or the scheduler. The guest sees the same values in
  the same order whatever the render thread's timing, so slices,
  preemption and logical time are reproducible as before. The
  synchronous path is the fallback: `H2_RENDER_THREAD=0` gives it.
- **Errors.** A backend error now stops at the next backend call (or the
  next read), not at the call that caused it. Every backend error is a
  stop either way; only the reported entry differs.

## Prototype

- `d3d::ThreadedBackend<B>` (`crates/d3d/src/threaded.rs`): a `Backend`
  that moves `B` to a thread named `d3d-render` and queues each call.
  `finish`, `with_inner` and `with_surface` run something after the
  queue and wait (tests, reports).
- **Batches** (2026-11-16). Calls collect in one pending list and cross
  the channel together: one send and one wake per batch, not per call.
  A batch goes at every call that waits (before the wait), at every
  present (before the frame-in-flight wait), every `threaded::BATCH`
  (64) calls and on drop. Texture-proxy drops go into the same list, so
  they keep their place. The render thread runs a batch in order, so its
  calls and their order are unchanged; they only start later.
  The device hands each draw over with `Backend::draw_owned`, so the
  render thread takes the `ResolvedDraw` (streams, indices, state, shader
  inputs) without a copy and frees it there. The draw's stream buffers
  come back through a bounded pool (256 draws, 16 MiB) for the device to
  refill. Wrappers forward it (`DiagBackend`, `Headless`); one that does
  not falls back to `draw`, which copies.
  `cargo bench -p d3d --bench pushbuf` times the guest's side through
  `ThreadedBackend<NullBackend>`: 4.0-4.5 us a draw before (one boxed
  closure, channel send and wake per call, the draw cloned), 2.5-2.7
  after, against 2.1-2.3 for the device alone. In a 250 Hz profile of
  the main menu before this, the per-call wake alone
  (`semaphore_signal_trap` under the channel send) was 1.3% of the game
  thread, and the draw clone showed up in its copies and mallocs.
- On unless `H2_RENDER_THREAD=0` (only `0` turns it off;
  `d3d_glue::render_thread_from`), it wraps
  the `GfxBackend` in it: the headless one with `H2_BOOT_GFX`, and the
  window's in `h2 play` (`boot::Host::render_thread`). Its present hook
  only publishes into the shared frame slot, so it runs on the render
  thread unchanged. The persistent pipeline cache and its compile
  workers belong to the backend and move with it.
  `H2_RENDER_THREAD_STATS=1` prints the waits when it is dropped.
  Frame stats also forward worker counters/timers through a queued marker
  at the device's existing Swap boundary, preserving frame numbers without
  making Swap wait. `render-wait_us` measures game-thread reply and throttle
  waits separately from `cpu`; renderer phases can overlap that game-thread
  wall time. See [frame-budget.md](frame-budget.md).
- `menu_cpu` (docs/BOOT.md) now also prints
  `guest_thread_cpu_ms_per_frame`, the runner thread's own CPU time,
  which is the guest's critical path.
- Tests (`crates/d3d/tests/threaded.rs`, `depth_textures.rs`): one
  device sequence through a backend and through the render thread. The
  sequence clears an RTT level, samples it in place, has the guest store
  into it, writes it back with `BlockOnResource`, and then runs
  `EndVisibilityTest`, `SetFence`, `BlockOnTime`, the busy check,
  `GetVisibilityTestResult`, `Swap` and `KickOff`, over three frames.
  Both paths give:
  - the same observed values: entry results, the level's bytes after
    each wait, and 8 KiB of device words (GPU time, completion word, swap
    fences, register window);
  - the same backend calls, device stats, events and guest memory;
  - on the GPU, the same back-buffer bytes and batch counts.

  They match too with the persistent pipeline cache, which builds the
  same pipelines behind the render thread. An untranslatable draw
  (untransformed fixed-function vertices) between two drawn ones is
  skipped with the same event, the same stats and the same pixels.
  They match too with the render thread lagging by seeded random
  amounts (`ThreadedBackend::with_jitter`, three seeds), which is the
  determinism check: its timing changes nothing the guest sees.
  Depth-alias textures made on the render thread give the same
  read-backs, uploads, draws, skips and stats. A wrong speculation stops
  the next call, and reads wait for the work queued before them.
  A call sequence several batches long (draws handed over and copied,
  uploads and texture drops, clears, reads, pixel counts that every draw
  adds to, presents) gives the synchronous backend's calls in its order
  and the same answers, with and without jitter
  (`batched_commands_run_in_order_and_reads_see_everything_before_them`).

## Measurements

Measured on master 9a39ed7, before the guest-side judge, the merged-pass
default and pipeline-async; not re-run since (this lane runs no game).
`menu_cpu` (docs/BOOT.md), exact passes, presents 480-560, fresh data,
the same dylib (master's generator), one run at a time in the bench gate
on a shared 10-core M-series machine. Load averages are at each run's
start. `guest CPU` is the runner thread's own CPU per frame; `reads` and
`render busy` come from the render thread's counters (busy is wall time
in backend work, GPU waits included).

| build | render thread | load | fps | ms/frame | process CPU ms | guest CPU ms | reads/frame (wait ms) | render busy ms |
|---|---|---|---:|---:|---:|---:|---|---:|
| master 9a39ed7 | - | 55 | 4.75 | 211 | 226 | - | - | - |
| branch | off | 7 | 4.81 | 208 | 222 | 167.8 | - | - |
| branch | off | 86 | 4.66 | 215 | 229 | 171.8 | - | - |
| first cut | on | 9 | 4.71 | 212 | 248 | 125.9 | 1.00 (82.8) | 112.9 |
| final | on | 100 | 6.30 | 159 | 252 | 130.5 | 0 (throttle 8.9) | 159.4 |
| final | on | 23 | **7.65** | **131** | 240 | **125.9** | 0 | 104.1 |

- **Guest-thread CPU** falls from about 170 to about 126 ms a frame
  (-27%): that is the backend's CPU work (shader preparation, recording,
  encoding, submission, uploads, the present hook).
- **The guest also stops waiting.** With the synchronous backend a
  frame took about 210 ms of wall time against 170 ms of guest CPU: about
  40 ms of GPU waits (`D3DSWAP_FINISH`, read-backs). With the final cut,
  wall time per frame (131 ms) is the guest thread's CPU time (126 ms).
- **The first cut gained nothing**, which is why the depth-alias change
  is part of the design. Its one wait a frame was the depth read-back for
  the fog pass. It cost 83 ms, because the guest waited for the whole
  frame's queue before it. Moving that read to the render thread
  removed the last per-frame wait.
- **Fps** goes from 4.75 (master) to 7.65 (+61%) at similar load. Under
  heavy load (100) it was still 6.30, with the frame-in-flight limit
  holding the guest 9 ms a frame: there the render thread was the
  slower side.
- **Frames are byte-identical.** Every 20th frame from 1 to 560 (29
  PNGs) of master and of both render-thread cuts compared equal with
  `cmp`.

Estimate against the 33.3 ms budget: the render thread takes the
backend's whole cost off the critical path. The frame then costs
max(guest, render) instead of guest + render. Both are now about 100-130
ms, so 30 fps needs both sides cut about 4x. The guest side is the
runtime, generated code, x87 and kernel work in docs/perf-budget.md. The
render side is gfx-perf's encode and submit, now off the guest's thread
but still a ceiling of about 10 fps at 100 ms.

### The Heretic, master ac9fb31 plus the boot forwarding fix (2026-11-15)

`h2 run`, presents 4220-4520, warm pipeline cache, frame stats over the
window, one run at a time in the bench gate on the shared machine. Load
is the 1-minute load average at the end of each run (the window is its
last ~25 s).

| run | load | fps | p50 / p90 / p99 / max ms |
|---|---|---|---|
| sync, `H2_D3D_LAZY_DEPTH=0` | 31 | 12.88 | 76.2 / 86.9 / 123.0 / 165.1 |
| sync, defaults | 88 | 10.58 | 83.3 / 137.5 / 213.7 / 270.7 (loaded) |
| render thread, `H2_D3D_LAZY_DEPTH=0` | 18 | 14.78 | 59.1 / 81.4 / 178.6 / 265.0 |
| render thread, defaults | 9 | **17.07** | **57.0** / 67.4 / 75.6 / 77.0 |

Every run has the same schedule (56,572 slices) and the same frame-4215
PNG (`md5` equal across all five runs); the render thread latched no
error or wrong speculation. The render thread answered 27,300 pixel-count
reads from reports already back (`landed_counts`) and still waited
4,700-5,200 times (42-56 s over the run), for the read-backs below.
menu-fps measured the menu flyover at 15.48 fps against 11.84 with it
off, 29 PNGs byte-identical (docs/perf-budget.md). So the render thread
is now the default.

The Heretic's remaining read-backs (`H2_D3D_WRITEBACK_TRACE=1`): 4 a
frame, every one `Overlap`. Two 160x120 colour levels, a 320x240 one and
the 512x512 `LIN_D24S8` shadow map share guest memory, so binding or
sampling one writes the others back first. None is a guest wait or read.
Lazy depth therefore saves nothing here (the shadow map's read-back moves
from its binding change to the overlap). Moving pixels between
overlapping levels on the GPU, with per-byte tracking of which level
wrote last, was the next fix: the alias mirror (2026-11-15, docs/d3d.md
"Overlapping levels on the GPU", `H2_D3D_ALIAS_MIRROR=0` turns it off).

## Risks and next steps

- **The judge has to stay in step with `GfxBackend::draw` and
  `Nv2aShaders::prepare`.** Both share `route`, `program_key` and
  `translate`, so a new skip reason added there is judged too. A new
  failure added anywhere else would be a mismatch, which stops the run
  with a named error rather than diverging. Translation runs twice per
  new program (judge and compile), and the key is built on both threads
  for every draw. That is CPU on the guest's thread that the measurements
  above predate.
- **Copies on the guest thread.** Draws move (`draw_owned`, above), but
  `upload_texture`, `write_surface` and `mirror_write` still copy their
  bytes, because the trait passes references.
- **Errors arrive late.** A backend error stops the guest at the next
  backend call, not at the entry that caused it.
- **Waits remaining.** RTT write-backs the guest asks for
  (`BlockOnResource`, the busy check, `BlockOnTime`, push-buffer
  callbacks) still wait for the queue. Depth levels (shadow maps) no
  longer read back at every binding change: they are lazy like colour
  levels and sampled through a texture the render thread makes on the
  GPU (`Backend::depth_level_texture`, docs/d3d.md "Lazy depth levels").
  Pixel-count reads of a report already back are answered on the guest's
  side without the queue: after each present and each batch the render
  thread moves the counts the GPU has written (`Backend::try_pixel_count`,
  in report order up to the first not back) into a shared map
  (`Waits::landed_counts` counts them). `GetVisibilityTestResult` polls
  that map (`try_pixel_count`) and, for a test ended at most two `Swap`s
  before whose count has not landed, answers `D3DERR_TESTINCOMPLETE`
  without waiting (`Waits::pending_counts`), as the hardware may; the
  miss hands the pending batch over so the count lands. Halo keeps last
  frame's value for that flare. An older test, or every test with
  `H2_D3D_VISIBILITY_POLL=0`, still waits for the queue. In the Old
  Mombasa profile at `183781e` that wait was 8.3% of the game thread.
- **Memory and latency.** At most two presents in flight. Under load the
  limit is what holds the guest (6.30 fps run).
- **`h2 play`** is wired but has not been run with it (no game runs on
  this lane); only the flag is unit-tested.
- **Measurement noise.** These are single runs on a shared machine with
  load swinging between 7 and 100. Repeat on a quiet machine before
  quoting fps.
