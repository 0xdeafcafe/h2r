# Render thread

On the Xbox the NV2A consumes the push buffer while the CPU goes on with
the next frame. Here every part of rendering ran on the guest's runner
thread, between guest instructions, so guest CPU work and host rendering
added up instead of overlapping. This page says where that work ran,
what a render thread may and may not take off the guest's thread without
changing anything the guest can see, what the opt-in prototype
(`H2_RENDER_THREAD=1`, `d3d::ThreadedBackend`) does, and what it measured.

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
- **Depth aliases** (`0x509368`, the menu's fog pass): `read_depth` of the
  depth buffer, cached by depth generation.
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
2. **Handles** (`create_surface`, `create_texture_target`,
   `upload_texture`, `snapshot_surface`, `surface_texture`,
   `report_pixel_count`): the guest side hands out a proxy id at once; the
   render thread maps it to the inner backend's handle when it reaches
   the command. A proxy texture freed by the device is freed on the
   render thread after the work queued before the drop.
3. **Outcomes** (`draw`, `clear_rects`: `Drawn`; `write_surface`,
   `copy_surface`: `true`; `snapshot_surface`, `surface_texture`,
   `report_pixel_count`: `Some`): answered at once with the answer
   `GfxBackend` gives for every valid call. The device's state depends on
   them: a drawn draw marks an RTT level dirty and bumps the depth
   generation, a skipped one records `Event::DrawSkipped`. If the render
   thread later gets another answer, the speculation was wrong; it is
   latched, and the **next backend call fails** with a named error, so the
   run stops instead of continuing with a device that differs from a
   synchronous run. `ThreadedBackend::misses` counts them.
   `GfxBackend` skips a draw only when the nv2a translator cannot
   translate it (`Nv2aShaders::prepare`), which on the menu does not
   happen (no `draw skipped` events in the menu window).

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
  synchronous path stays the default and is the fallback: with
  `H2_RENDER_THREAD` unset nothing changes.
- **Errors.** A backend error now stops at the next backend call (or the
  next read), not at the call that caused it. Every backend error is a
  stop either way; only the reported entry differs.

## Prototype

- `d3d::ThreadedBackend<B>` (`crates/d3d/src/threaded.rs`): a `Backend`
  that moves `B` to a thread named `d3d-render` and queues each call as
  a closure on a channel. `finish`, `with_inner` and `with_surface` run
  something after the queue and wait (tests, reports).
- `H2_RENDER_THREAD=1` with `H2_BOOT_GFX` (boot's `d3d_glue::install`)
  wraps the headless `GfxBackend` in it. `h2 play` is not wired yet: it
  builds its own `GfxBackend` (`crates/cli/src/play_cmd.rs`) and would
  wrap it the same way; its present hook already only publishes into the
  shared frame slot.
- `menu_cpu` (docs/BOOT.md) now also prints
  `guest_thread_cpu_ms_per_frame`, the runner thread's own CPU time,
  which is the guest's critical path.
- Tests (`crates/d3d/tests/threaded.rs`): one device sequence (an RTT
  level cleared, sampled in place, stored into by the guest, written back
  by a busy check, three swaps) through a backend and through the render
  thread gives the same backend calls, the same device stats and events,
  the same guest memory, and on the GPU the same back-buffer bytes and
  batch counts; a wrong speculation stops the next call; reads wait for
  the work queued before them.

## Measurements

MEASUREMENTS

## Risks and next steps

RISKS
