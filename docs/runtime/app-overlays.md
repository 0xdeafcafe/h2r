# App: frame stats and the watermark

Status: Built. The `--frame-stats` overlay and the watermark. Part of [App: the host shell](app.md).

### Frame stats: `--frame-stats`

`h2 play --frame-stats`, or `H2_FRAME_STATS=1` for `h2 play` and `h2 run`,
times every frame on the game thread and records the renderer's phases too
(`timeline::phase`, docs/perf/gameplay-budget.md):

| phase | what |
|---|---|
| `cpu` | game code, runtime accessors, kernel, scheduler, HLE calls |
| `dec` / `decode` | push-buffer decode and device state |
| `enc` / `encode` | wgpu on the CPU: draw recording, shader prepare, pass encode, pipeline compiles, texture uploads, the present copy |
| `sub` / `submit` | `queue.submit` |
| `gpu` / `gpu-wait` | blocking GPU round trips: readbacks, `D3DSWAP_FINISH` |
| `pace` | the tick's sleep to wall time; slack, not cost |
| `rt` / `render-wait` | game-thread wait for render-thread replies or the frame-in-flight limit, removed from `cpu`/decode |

Over the last 5 s it shows fps and the p50, p99 and max frame time, and
the mean of each phase: in the window's top-left corner (the banner, so
not with `--follow`, whose banner keeps the corner), and once a second on
stderr. At exit the run prints the whole run's percentiles and its ten
worst frames, each with its phases and what happened in it: draws,
submits, pipelines the encoding thread waited for (docs/graphics/graphics.md "Pipeline compiles"), nv2a shader compiles, texture uploads,
readbacks and bytes read from files. `H2_FRAME_STATS_CSV=<file>` writes
one row per frame; `H2_FRAME_STATS_FROM=N` starts the exit report at
frame N. F6 hides the line until the next refresh.

With the render thread, a queued marker forwards its counters and timings
into the same numbered frame. A row appears after both threads reach its
boundary; the game thread keeps running. Renderer phases overlap the
game thread, so their sum can exceed the frame's wall time. `render-wait_us`
is the game thread's own wait, including queue backlog, CPU encoding and
GPU work needed for a reply. The CSV's `read_bytes` is file I/O, not GPU
readback bytes. An incomplete render-thread tail at shutdown is omitted
and counted in the exit report. See [gameplay-budget.md](../perf/notes/gameplay-budget.md).

Cost: one clock read per phase switch, about six per draw. At 300 draws
that is under 0.1 ms of a 75 ms level frame. Add one short lock per frame. Off, each hook is a relaxed
load and a branch.

### Watermark

`h2 play` draws a development-build watermark in two small panels over
the game image, laid out the way pre-release builds of console shooters
often were (our own wording, not a copy of any real one): a build label
centred near the bottom, and the live run's numbers right-aligned in the
lower right corner. Each keeps to its third of the image width, so they
never overlap.

```
                                                            28.4 fps  ·  35.2 ms
                                                        cpu 24  ·  gfx 5  ·  wait 6
                                                                      draws 1104
                    h2recomp development build                  map 01a_tutorial
               a1987da master  ·  2026-11-15 14:02 UTC   metal, render thread on
```

Both panels sit on the same bottom margin; the live panel grows upwards.

- The label is baked at compile time by `crates/apps/app/build.rs`: the short
  commit, the branch (`detached` off a branch, `unknown` without git) and
  the time the build script last ran (it reruns when HEAD moves or
  `crates/apps/app/src` changes). `H2_BUILD_SHA`, `H2_BUILD_BRANCH` and
  `SOURCE_DATE_EPOCH` override them.
- The live panel comes from the frame stats' rolling window
  (`timeline::phase::window_summary`): fps, the mean frame time, then in
  ms per frame the game thread's `cpu`, `gfx` (decode + encode + submit +
  `gpu-wait`, on whichever thread does them; with the render thread on it
  overlaps `cpu`) and `wait` (`render-wait`: the game thread waiting for
  the render thread, 0 with it off), and draws per frame (the render
  thread's included, joined to their frame by `timeline::phase::WorkerFrame`). The map is the level map the game
  opened last (`kernel::level::last_map`: `maps\<name>.map`, the shared
  maps left out). Then the wgpu backend, whether the render thread is on,
  the raster of the frame being shown, and the drawable size, both in
  physical pixels. With `--frame-stats` the window is its 5 s one; otherwise `h2 play`
  starts frame stats itself with a quiet 2 s window
  (`timeline::phase::Config::live`: no log, no CSV, no exit report). An
  embedder without frame stats gets fps and frame time from the shell's
  present count.
- Drawing is host-side only, at present time, over the scaled game image
  (inside any letterbox or pillarbox), never in guest memory. Timing
  numbers refresh every 250 ms and the dimensions update with the
  presented frame and drawable. Panel text is rasterised on the CPU (`font8x8`, the banner's public-domain letters, with a soft
  one-pixel shadow) into one texture a panel. Every present draws each as
  one alpha-blended quad. Glyphs are 8x8 font pixels, 2x from a target
  1600 pixels tall (Retina), and lines too long for their panel are cut
  with `..`.
- On by default. `--no-watermark` or `H2_WATERMARK=0` turns it off; F3
  toggles it while playing (F3 then never reaches the guest).
- `H2_WATERMARK=full` adds the game's readout above the live numbers, in
  the same panel, grouped as the player (`pos x y z`, `vel x y z (speed/s)`
  in world units a second, `yaw` and `pitch` in degrees), the level (`bsp`
  index of the map, and the `cluster` the player stands in), the world
  (`bipeds` as players plus AI actors, `veh`, `obj`, then `ai`, `decals`,
  `particles`) and the picture (`draws native N d3d K`: the last native
  frame's backend draws against the guest draws the scene sink carried on
  D3D, then `kept <layer>_<kind>_<cause> N`, the `scene_kept_*` counter
  with the most draws). `-` is a value the game has not answered. Without
  `full` nothing is read and the panel is as above.
- The readout is the game's, not the window's:
  `game::debug_readout` asks the agent API in-process every 100 ms while
  the level is on (`sim.view` roots `at`, `player` and `count`,
  `world.pools`, `collision.trace` at the player's position for the
  cluster, and `render.scene`) and leaves the lines in the watermark's
  shared list (`Watermark::readout_handles`), refreshed with the timing
  numbers. It reads no guest memory of its own; the API has no BSP name,
  so the index and the map's name stand for it, and `world.pools` is the
  API method added for the decal and particle counts
  ([agent-api.md](agent-api.md#worldpools)). The cluster is the player's
  position, not the camera's, and reads `-` until the native collision
  world answers.
- The superbounce game type's HUD is not the window's: the game's own
  HUD draws it, through the HUD kit
  ([hud-kit.md](../game/hud-kit.md),
  [superbounce.md](../game/superbounce.md#the-superbounce-game-type)).
- In code: `app::Watermark` (shared on/off, note, map callback) on
  `AppConfig::watermark`; `app::watermark` has the formatting, layout and
  rasteriser, and `WatermarkRenderer` the pass. The banner and the
  watermark share `overlay::Sprite`.
