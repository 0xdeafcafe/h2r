# Render thread: measurements

Evidence for [render-thread.md](../render-thread.md). Nothing here states
what the code does today; the design page does. Entries are newest first.
Variables that these runs set or name (`H2_D3D_LAZY_DEPTH`,
`H2_D3D_DRAW_SHARING`, `H2_D3D_VISIBILITY_POLL`) were removed; the
experiments that lost are in [not-adopted.md](not-adopted.md).

## 2026-11-17: the draw path on the guest's thread

Commit: not recorded; the 500 Hz profile was of a PGO image. Method: the
game thread's samples on Old Mombasa at about 2000 draws a frame. Not
re-run.

- The D3D overrides were 21% of the game thread's samples,
  `DrawIndexedVertices` alone 11%.
- What is pure host work on that thread was small and spread out: the
  shader judge (`Nv2aJudge`: `program_key`, the `ProgramKey` hash and
  compare) 1.1%, `TranslateMemo` 0.4%, `draw_state` (a 1176-byte state
  copy and an `Arc` per draw) 1.0%, the vertex layout clone 0.3%, the
  queue push 0.8%.
- A cut that followed: the judge builds the PSDEF from the words, not
  through a byte array and a collected `Vec`; draw sharing (a draw whose
  state arrays read the same as the pushed snapshot shares it, and a
  vertex program the game switches back to gets its earlier `Microcode`,
  64 kept).
- Reproduce the guest-thread microseconds per draw at 2000 draws, sharing
  off and on, behind the render thread with the judge and on the device
  alone: `cargo bench -p d3d --bench draw_main`. The equivalence test is
  `crates/platform/d3d/tests/draw_sharing.rs`.

## 2026-11-16: batches

Method: `cargo bench -p d3d --bench pushbuf`, which times the guest's side
through `ThreadedBackend<NullBackend>`.

- 4.0-4.5 us a draw with one boxed closure, channel send and wake per call
  and the draw cloned; 2.5-2.7 us with batches and `draw_owned`; 2.1-2.3
  us for the device alone.
- A 250 Hz profile of the main menu before batching: the per-call wake
  alone (`semaphore_signal_trap` under the channel send) was 1.3% of the
  game thread, and the draw clone showed up in its copies and mallocs.

## 2026-11-15: The Heretic

Commit: master `ac9fb31` plus the boot forwarding fix. Method: `h2 run`,
presents 4220-4520, warm pipeline cache, frame stats over the window, one
run at a time in the bench gate on a shared machine. Load is the
1-minute load average at the end of each run.

| run | load | fps | p50 / p90 / p99 / max ms |
|---|---|---|---|
| sync, lazy depth off | 31 | 12.88 | 76.2 / 86.9 / 123.0 / 165.1 |
| sync, defaults | 88 | 10.58 | 83.3 / 137.5 / 213.7 / 270.7 (loaded) |
| render thread, lazy depth off | 18 | 14.78 | 59.1 / 81.4 / 178.6 / 265.0 |
| render thread, defaults | 9 | 17.07 | 57.0 / 67.4 / 75.6 / 77.0 |

- Every run has the same schedule (56,572 slices) and the same frame-4215
  PNG (`md5` equal across all five runs); the render thread latched no
  error or wrong speculation.
- The render thread answered 27,300 pixel-count reads from reports
  already back (`landed_counts`) and still waited 4,700-5,200 times
  (42-56 s over the run), for the read-backs below.
- The menu flyover measured 15.48 fps against 11.84 with the render
  thread off, 29 PNGs byte-identical
  ([menu-budget.md](../../perf/notes/menu-budget.md)).
- The read-backs left (`H2_D3D_WRITEBACK_TRACE=1`): 4 a frame, every one
  `Overlap`. Two 160x120 colour levels, a 320x240 one and the 512x512
  `LIN_D24S8` shadow map share guest memory, so binding or sampling one
  writes the others back first. None is a guest wait or read. The fix was
  the alias mirror (`Device::set_alias_mirror`).
- Old Mombasa at `183781e`: the visibility wait was 8.3% of the game
  thread, which `try_pixel_count` removed.

## 2026-11-15: the first cuts

Commit: master `9a39ed7`, before the guest-side judge, the merged-pass
default and async pipeline builds. Method: `menu_cpu`
(`crates/apps/boot/examples/menu_cpu.rs`), exact passes, presents 480-560,
fresh data, the same dylib, one run at a time in the bench gate on a
shared 10-core M-series machine. Load averages are at each run's start.
`guest CPU` is the runner thread's own CPU per frame. Single runs, load
between 7 and 100: repeat on a quiet machine before quoting fps.

| build | render thread | load | fps | ms/frame | process CPU ms | guest CPU ms | reads/frame (wait ms) | render busy ms |
|---|---|---|---:|---:|---:|---:|---|---:|
| master 9a39ed7 | - | 55 | 4.75 | 211 | 226 | - | - | - |
| branch | off | 7 | 4.81 | 208 | 222 | 167.8 | - | - |
| branch | off | 86 | 4.66 | 215 | 229 | 171.8 | - | - |
| first cut | on | 9 | 4.71 | 212 | 248 | 125.9 | 1.00 (82.8) | 112.9 |
| final | on | 100 | 6.30 | 159 | 252 | 130.5 | 0 (throttle 8.9) | 159.4 |
| final | on | 23 | 7.65 | 131 | 240 | 125.9 | 0 | 104.1 |

- Guest-thread CPU falls from about 170 to about 126 ms a frame (-27%):
  the backend's CPU work (shader preparation, recording, encoding,
  submission, uploads, the present hook).
- With the synchronous backend a frame took about 210 ms of wall time
  against 170 ms of guest CPU: about 40 ms of GPU waits (`D3DSWAP_FINISH`,
  read-backs). With the final cut, wall time per frame (131 ms) is the
  guest thread's CPU time (126 ms).
- The first cut gained nothing: its one wait a frame was the depth
  read-back for the fog pass, 83 ms, because the guest waited for the
  whole frame's queue. Making that texture on the render thread removed
  the last per-frame wait.
- Fps went from 4.75 to 7.65 (+61%) at similar load. Under load 100 it was
  6.30, with the frame-in-flight limit holding the guest 9 ms a frame: the
  render thread was the slower side.
- Every 20th frame from 1 to 560 (29 PNGs) of master and of both
  render-thread cuts compared equal with `cmp`.
- Estimate against the 33.3 ms budget: the frame costs max(guest, render)
  instead of guest + render. Both were about 100-130 ms, so 30 fps needs
  both sides cut about 4x.
