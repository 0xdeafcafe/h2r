# Agent API: profiling and the render pipeline

Status: Built. Part of the [agent API](agent-api.md), which holds the wire, the method index and the rules.

## `profile`: where a frame goes

| Method | Params | Result |
|---|---|---|
| `profile.counters` | none | the frame-stats rolling window (`timeline::phase`), the numbers `H2_FRAME_STATS` prints: `frames`, `fps`, frame ms, `phase_ms`, `counts`. When nothing else collects them the glue starts a quiet 5 s window |
| `profile.capture` | `{ticks \| ms, budget_ms?, spikes?, sample?}` | percentiles, phases, counters and spikes over a window (below) |
| `profile.fallbacks` | `{top?}` | where the fallback interpreter runs |
| `profile.memory` | none | the flat RAM window's counters (below) |
| `profile.memory.pages` | `{top?, census?}` | the pages outside the window by reason and owner, and the slow-access census (below) |
| `profile.faults` | `{top?}` | the fault census: every guest memory fault and whether execution went on from it |
| `profile.sample.start`, `.stop`, `.status` | `{hz?}`, `{inline?, symbols?}`, none | the in-process sampler; `symbols` names the frames |

```sh
h2 agent counters
h2 agent capture 900                 # 900 game ticks, about 30 s
h2 agent capture 10000ms sample      # h2 built --features perf
h2 agent fallbacks 20
h2 agent flatmem
h2 agent slowpages on              # start the slow-access census
h2 agent slowpages 30              # outside pages and the 30 slowest
h2 agent faults
```

- **`profile.capture`** keeps every frame-stats row over the window and
  answers `frames`, `fps`, `frame_ms` and `busy_ms` (frame less pacing) as
  mean, p50, p95, p99, max and total; the same for each phase (`cpu`,
  `decode`, `encode`, `submit`, `gpu-wait`, `pace`, `render-wait`, `d3d`,
  and `render` when the render-timing feature measures it); mean, max and
  total of each counter (`draws`, `passes`, `uploads`, ...); `over_budget`,
  the frames longer than `budget_ms` (default 33.3); and `spikes`, the
  worst of them (20 by default, at most 500), each with its present
  number, game tick and map, its own phases and counters, its three
  biggest phases (pacing left out) and `causes`. Every frame is also
  written as a CSV row (present, end, tick, map, total and busy us, each
  phase in us, each counter) under the cache directory's `captures/`, and
  the reply gives its path. A frame belongs to the last tick that had ended
  when it did. By ticks the window is 1 to 36000 (it gives up after 600 s
  with `complete: false`); by `ms`, 1 to 600000. One capture at a time; it
  is read-only. With frame stats off it answers `UNAVAILABLE`
  (`crates/extensions/agent/src/capture.rs`).
- **Causes.** The reply's `causes` counts every frame over budget, listed
  or not, by cause (`crates/extensions/agent/src/capture.rs`, `causes`):
  `shader_compile` and `pipeline_compile` (any on the critical path),
  `texture_uploads` and `gpu_readback` (more than twice the window's
  median, and at least one), `file_io` (the guest's file calls took at
  least 2 ms of host time in the frame, or at least a tenth of its busy
  time: the `file_io_us` counter; bytes read alone never count), `gpu`,
  `render_thread` and `pushbuf_decode` (that phase is at least 40% of the
  frame's busy time), else `guest_cpu` when the guest's own time is the
  biggest phase. These are what the counters show, not a proof.
- **Native renderer counters.** `profile.counters` and `profile.capture`
  use the field names of `H2_FRAME_STATS_CSV`. Each response and capture
  CSV includes native columns with activity in its window; the streaming
  CSV writes a fixed header at startup. Each `native_<stage>_` prefix has
  examined and suppressed guest items, refusals by reason, prepare and
  encode nanoseconds, GPU buffers and bind groups created, upload bytes and
  pipeline misses; per-view columns count reference and display ranges
  attempted and committed and invariant failures. For example
  `native_world_v1_display_recorded` can be positive while
  `native_world_v1_display_ranges` is zero when the display copy fails
  validation. Frame captures also include `native_lights_worker_round_trips`
  and `native_lights_reference_passes` / `native_lights_display_passes`.
  Definitions: [native-renderer.md](../graphics/native-renderer.md#frame-counters).
- During map preparation, `setup_<stage>_worker_ns` sums elapsed time on workers
  between presents for world, lights, fog, water, decorators, objects, layers,
  pipelines, nav and visibility. Concurrent jobs can contribute more than one
  frame's duration. `setup_over_33ms_frames` counts presents more than 33⅓ ms
  apart while measured work overlaps that interval. Use the capture's phase
  breakdown to distinguish a blocked frame from work running alongside it.
  `setup_dispatch_ns` times foreground handoffs for BSP activation and
  publication, object and light models, first-person models, resident decorator
  geometry snapshots and bot map changes.
  `setup_dispatch_stalled_frames` counts over-budget presents whose handoffs
  alone took more than 33⅓ ms.
  `d3d_entries` counts guest D3D entry calls that ran, `d3d_residual_draws`
  the guest draws `d3d` handed to its backend (what no native replacement
  claimed) and `d3d_push_words` the push-buffer words it decoded.
  `native_inline_full` and `native_inline_fast` count the native inline
  draws (particles) resolved in full and on a same-state run's fast path,
  and `queued_runs` and `queued_run_draws` the queued native admissions the
  render thread prepared and the draws in them
  ([native-particles.md](../graphics/native-particles.md)). They are
  always on: an integer add into the calling thread's frame, or one relaxed
  load when frame stats are off
  ([retire-d3d.md](../graphics/retire-d3d.md), slice R0;
  [`render.residual`](agent-api-render-checks.md#renderresidual-residual-d3d-work) breaks them down).
  For inactive map slots, `map_slot_vfs_wait_ns` times the render-frame hook
  acquiring the shared VFS and `map_slot_check_ns` times its checks for
  changed source files after acquisition. A slot check runs at most twice a
  second and requires an inactive candidate.
  `load.timeline` also sums these counters for the stall and settling periods.
- **Fallbacks.** `profile.fallbacks {top}` (20 by default, 1 to 500)
  returns the fallback interpreter's totals and its entry addresses ranked
  by instructions interpreted, each with hits, x87 instructions and the
  function it is in (the nearest analysed start at or below it). Kept per
  address in `boot::fallback::Stats::by_site`, at most 4096 addresses
  (`sites_full` says when later ones count in the totals only). Slim host
  modules also report `cold_entries` (interpreter invocations for omitted
  owners) and `cold_sites` (distinct owner/selector pairs); the boot log
  names each pair on first use.
- **Flat RAM window.** `profile.memory` answers the counters of every
  runtime's flat RAM window in the process, summed
  (`runtime::window::process_stats`, answered by boot's agent glue;
  [contract-thin-transfers-flat-ram.md](contract-thin-transfers-flat-ram.md),
  section 4.1). They are always on. `windows` is how many there are and
  `aliased` how many hold RAM (guest RAM is aliasable once a flat-RAM
  module is registered, or from the start with `H2_FLAT_RAM=1` at run
  time). `fixups` counts faulting accesses sent to their slow path; the
  null window a context uses with fast memory off is not counted. `pages_outside` counts mapped guest pages the window does not
  hold: non-congruent with their backing, sharing a host page with one,
  byte-granular or armed by the access trap. Each costs a fault and the
  helper per access once flat accesses are emitted. `congruence_skipped`
  is the physical RAM, in bytes, the congruent allocator jumped over to
  give a fresh allocation the residue of its address. A slow page shows as
  `fixups` rising or `pages_outside` above zero.
- **Which pages, and why.** `profile.memory.pages {top, census}` (`top` 1
  to 500, 20 by default) names them. `outside` is every page
  `pages_outside` counts (`Guest::outside_runs`, `runtime/src/guest/pages.rs`):
  `pages`, `runs`, `by_reason` and the `top` largest runs, each `{va,
  pages, reason, owner, mapping: {va, len, physical}}`. `reason` is
  `non_congruent`, `shares_host_page` (whole and congruent, but its host
  page also holds a page that is not, or one with another physical host
  page), `byte_granular`, `unaligned` or `trapped`; `owner` is the HLE
  claim over the run, else the physical reservation under it, else null.
  `census` is the slow-access census (`runtime::slow_pages`): the guest
  pages generated code reached the checked helpers (`h2abi_v1_read*`,
  `h2abi_v1_write*`) for, in both the paged and the flat model. `census:
  true` starts it from zero, `false` stops it and keeps the counts; off,
  it costs a helper one load. It answers `armed`, `reads`, `writes`,
  `pages` (distinct) and the `top` pages by calls, each `{va, reads,
  writes, crossings, gate_busy, no_view, pc, fast, trapped, outside, owner,
  mapping}`: `gate_busy` counts calls made while another thread held the
  inline-memory gate, `no_view` calls from a context that lent no RAM view,
  `crossings` spans into the next page, `pc` is the last caller, and `fast`
  is the page's fast table entry (`{physical, read, write}`) that the paged
  inline path checks. A page whose `fast` entry allows the access and
  whose calls are mostly `gate_busy` is slow for the gate, not its mapping.
- **Faults.** `profile.faults {top}` (20 by default, 1 to 500) is the
  fault census (`runtime::fault_census`,
  [block-clock-imprecise-faults.md](block-clock-imprecise-faults.md)):
  `generated` (memory faults raised by an accessor generated code called)
  and `runtime` (raised by the runtime or a Rust handler); `ended` (faults
  a run returned to its caller with); `continued` (times guest execution
  went on, on the same context, from the state a fault left) and
  `continued_generated` (of those, generated faults);
  `precise_exits_needed`, true when `continued_generated` is not 0; and
  the `top` fault PCs by count, each with `source`, `count`, `continued`
  and the first fault's `addr`, `len`, `access` and `reason`. Kept per PC,
  at most 4096 (`sites_full`). It counts from the process's start. It also
  names the image's choices: `clock` (`accesses` or `blocks`),
  `imprecise_faults` (module feature 128), and `continued_imprecise`, the
  continuations from a fault whose EFLAGS were inexact (should be 0).
- **Sampling.** `sample: true` also runs the in-process sampler
  (`profile.sample.*`, `boot::sampling`) over the window, and `sampling` is
  what `profile.sample.stop` answers: the module-relative dump
  (`profile.h2prof` under `H2_SAMPLE_DIR`, which `h2 profile remote`
  resolves) and its sample count. With `symbols: true` the stop also
  answers `symbols` (`module:offset` to `name (file:line)`, from `atos` on
  macOS with `dladdr` as the fallback) and `top` (`self` and `inclusive`, the
  30 heaviest names with sample counts, a name counted once per stack), so
  one call says where the frame goes. Only an h2 built with `--features perf`
  on a host with a sampler has it; otherwise, or while a
  `profile.sample.start` run holds it, the capture still runs and
  `sampling.unavailable` says why ([xbox.md](xbox.md), "Profiling on the
  console").

## `render`: the frame and the pipeline

`render.frame` and `render.stats` are host methods (types in
`crates/extensions/agent-proto/src/render.rs`, logic in `crates/extensions/agent/src/render.rs`).

```sh
h2 agent frame                     # the next whole frame: passes, totals
h2 agent frame draws               # ... and every draw, with its depth state
h2 agent frame constants -38..-35,58,59   # ... and those vertex constants on each
h2 agent frame snap 0,3            # ... and passes 0 and 3 read back as PNGs
h2 agent render                    # render.stats
```

### `render.frame`

`render.frame {"arm": true, "draws", "max_draws", "constants", "snap"}`
arms the recorder; the next present starts the frame and the one after
ends it, then it disarms. `render.frame {}` reads the state and the last
frame (`frame.seq` is the arming's number). `h2 agent frame` does both and
waits up to 10 s; a paused or loading game presents nothing. Unarmed, the
draw diagnostics use one relaxed load per draw or clear in the D3D
wrapper. On a device with a native frame owner, the recorder waits for
the worker's presentation decision. An original presentation supplies
the D3D report. A native presentation supplies the executed graph's
attachment runs, slots, typed stages, snapshots and `display_frame`
counters. The worker keeps a requested original pass read at its position
among the guest commands. If the native graph takes the frame, its own
pass observer supplies the snapshot. Arming a snapshot preserves the
worker's choice of renderer.

- **The frame.** `totals` (passes, draws by outcome, vertices, clears,
  copies, reads, distinct targets); `passes`: runs of work on one colour
  surface and depth (a return to an earlier target is a new pass), each
  with its target (`640x480 X8R8G8B8`), clears, draws, vertices, outcomes,
  primitives, D3D entries, texture formats, and counts of pixel-program,
  blended and depth-writing draws; `ops`: copies and readbacks with the
  draw count before them. Surfaces are the backend's handles as text.
- **A draw** (with `draws`; at most `max_draws`, 2000 by default;
  `truncated` says when there were more). Its pass, D3D entry, primitive,
  vertex and index counts, outcome, texture formats, whether it has a
  pixel program, blends or writes depth; `vs`, the vertex side: `fvf 0x...`
  or `prog @start n=slots #hash clip=<source>` for a vertex program (the low
  32 bits of the words' FNV-1a hash, the same in every frame, so one
  program's draws can be picked out; `clip` is where its clip position
  comes from: `rows(b)` the matrix `c[b..b+4]`, `fixed` a pass-through,
  `unknown` anything else; ` lens=x<f>` follows when the display lens
  widened the draw, so on a wide display a program draw without `lens` is
  drawn at the retail 4:3 width); and `depth`: `mode` (`D3DRS_ZENABLE`: `off`, `z`,
  `w`), `w` (W-buffered as drawn: `SET_CONTROL0`'s z-perspective bit once
  pushed, else `mode`), `func` and `write` (`D3DRS_ZFUNC`,
  `D3DRS_ZWRITEENABLE`), `viewport` (`[MinZ, MaxZ]`), `scale`
  (`SET_VIEWPORT_SCALE` z and `SET_VIEWPORT_OFFSET` z), `clip`
  (`[SET_CLIP_MIN, SET_CLIP_MAX]`), `full` (the depth target's full scale;
  0 with no depth target) and `clamp` (`D3DRS_DEPTHCLIPCONTROL` clamps
  rather than culls; shown only when on). A W-buffered draw of a vertex
  program stores its w clipped (or clamped) to `clip`, over `full`; a
  z-buffered one its z over `full`.
- **Constants.** `constants` (with `draws`; at most 32 registers, D3D
  numbering -96..=95, hardware register = number + 96) records those
  vertex shader constants on each listed draw:
  `"constants": {"-38": [x, y, z, w], ...}`. The CLI takes ranges
  (`constants -38..-35,58`), implies `draws`, and `max N` sets `max_draws`.
- **Snapshots.** `snap` (at most 4 pass numbers): when such a pass ends,
  its colour target is read back as BGRA and written as a PNG under the
  cache directory's `frames/`. Original passes use
  `frame-<pid>-<seq>-pass-<n>.png`. A retry after a failed Swap or
  renderer replacement adds `-retry-<attempt>` after the sequence. Native passes use a `display-` or
  `private-` prefix. Read the path or error in `snaps`. An original pass
  with no known colour size reports that it cannot be read. A completed
  report retains its files. Replacing a pending capture releases its
  provisional snapshots. A snapshot incurs a GPU round trip.

A private capture has its own recorder. When the private scene renderer
is installed, arm the recorder before requesting a lighting capture,
then read its result:

```sh
h2 agent render.frame '{"source":"private","arm":true,"draws":true,"snap":[0]}'
h2 agent render.capture '{"lighting":"retail"}'
h2 agent render.frame '{"source":"private"}'
```

The next completed private draw supplies the report. Each row covers
draws and clears on the same attachments, with their view/slot pairs
in `slots` and typed stage names in `stages`. A copy ends a row even
when the following draw uses the same attachments. The `draws` list and
vertex totals describe carried guest draws, and `snaps` reads the chosen
passes before the following graph operations. `private_frame` identifies
the source scene, present and lighting mode. Its `passes` counts logical
attachment runs. The backend's `render_passes`, `backend_draws` and
`submits` include work caused by requested snapshots. The private
recorder measures wall time from graph entry to final readback.
Omitting `source` or specifying `"display"` reads or arms the display
recorder independently. A request for `"private"` returns `UNAVAILABLE`
when the host has no private scene renderer.

### `render.frame_layout`

`render.frame_layout {"arm":true,"max_draws":2000,"snap":[0,3]}`
arms the same one-frame recorder as `render.frame` and includes draw rows
automatically. `max_draws`, `constants` and `snap` are optional and have the
same limits as `render.frame`. Call `render.frame_layout {}` for the recorder
state and the last frame. Both methods share the sequence and the result.

`frame.draws[].layout` gives the guest target size and the selected host
colour and depth allocation sizes. The viewport and `clip` use guest
pixels. `display_clip` applies the draw's aspect and scissor transform
and rounds the result to the host allocation's pixel grid. The row gives
the draw's UI transform, vertex-position classification and horizontal
projection correction when present. `window_xy` is the min and max vertex
position before the display fit and subpixel adjustment for supported
small, unindexed draws: the reading the device's UI fit uses to tell a
full-frame pass from a widget (`crates/platform/d3d/src/screen_rect.rs`).

Each `sampled` entry gives the stage, texture header and texture ID,
source surface when known, guest and host sizes, and U and V address
modes. The sampled surface names match the handles in `frame.passes` and
`frame.ops`. For small draws with supported vertex inputs, `uv` gives
the min and max normalised coordinates at the vertices for direct 2D
projective lookups. It includes division by the projective coordinate and
the guest texel-size conversion for linear textures. Dependent lookups,
such as fog distortion, expose their bindings and address modes. Their
coordinates also depend on earlier fragment samples.

Pass snapshots read the selected host colour allocation at its actual
size. The capture records D3D draws and accepted native reservations
that expose a resolved draw packet, including the native menu fans.
Higher-level native stages can appear only in the images. Attachment
sizes describe backing allocations. A draw can use a temporary
attachment, such as the guest-size pair used for mixed-grid draws.
Capturing can synchronise the render worker and flush the GPU for the
requested snapshots.

For backdrop or modal-blur issues, arm a layout frame on each affected
screen, then look up the passes that write the sampled targets. Use
`snap` on those pass indices in a second capture to find the first image
with the artifact. Compare a wide main-menu frame and a 4:3 Choose Profile
frame. The source and destination sizes, vertex positions, normalised UVs
and address modes show where a copy magnifies a smaller raster or samples
beyond the image. The snapshot before that copy shows whether the edge
artifact arrives in its input.

The host records the layout in `crates/extensions/game/src/d3d_diag.rs` and
`crates/extensions/game/src/d3d_diag/layout.rs`. The display allocations and
readbacks come from `crates/platform/d3d/src/scaled.rs` and
`crates/platform/d3d/src/gfx_backend.rs`.

### `render.stats`

No params. `display_rate` is the frame owner's display-rate loop, the
display frames drawn between sealed scenes at 60 and 120 fps
([framescene.md](../graphics/framescene.md), section 7). `counters.line` is the frame-stats window's
summary. `counters` also reports the per-frame means for `shadow_walk_layers`,
each `native_scene_lists_*` stage and the overlay stage's
`native_ui_overlay_*` counters. The `_native` and `_fallback` fields and
every overlay counter include zeroes. Another refusal reason appears when
its window count is positive. The overlay's counters are its items by kind
(`items`, `quads`, `text_runs`, `fills`, `glyphs`, `blurs`, `model_views`),
the draws recorded (`batches`) and the items dropped, by reason
(`refused_material`, `refused_clip`, `refused_glyph`, `refused_texture`,
`refused_blur`, `refused_movie`;
[retire-d3d.md](../graphics/retire-d3d.md#what-replaces-each-subsystem)), and
the movie frames it drew (`movie_frames`). `native_ui_movie_*` counts the
movie's native draws: `placed` and `refused_{record,frame,picture,host,pass}`
([retire-d3d-display.md](../graphics/retire-d3d-display.md#movies)).
The HUD counters measure reservations of bitmap quads (`hud_tag_quads`)
and glyph quads (`hud_text_quads`), with `hud_tag_fallback` and
`hud_text_fallback` for declined submissions. `hud_tag_recorded` counts
display-copy recordings for both kinds and `hud_tag_record_failed` counts
failed attempts per target copy, for every draw the overlay captures.
See [the HUD on the overlay](../graphics/native-post-hud.md#the-hud-on-the-overlay).
Menu shader bitmaps' quads count as `menu_shader_quads`, with
`menu_shader_fallback` for those the UI fan path kept. The fade's, the
bars' and the loading screen's draws count as `fade_quads`, `bar_quads`
and `loading_quads`, with `screen_fallback` for those the UI fan path kept
([the fade, the bars and the loading screen](../graphics/native-post-hud.md#the-fade-the-bars-and-the-loading-screen)).
The menus' (`menu_*`) are the widget draws claimed at their sorted list
(`menu_claimed`), those left out (`menu_skipped`), the passes placed for
runs with items (`menu_passes`), lists whose left-out draws did not match
(`menu_mismatch`), lists the game draws whole (`menu_refused_frame`,
`_mismatched`), drawn bitmap widgets the game keeps (a state the overlay
does not draw as the game does, `menu_refused_bitmap_state`),
other drawn widgets the game keeps by the draw not read
(`menu_refused_text`, `_model`, `_draw`, `_unreadable`,
`_unlisted`), and lists or runs whose pass was not placed
(`menu_refused_pass`;
[native-post-hud.md](../graphics/native-post-hud.md#the-menus-on-the-overlay)).
A list's scroll arrows and a button's image and label belong to the
widget's claim. A failed glyph-page fill or bitmap decode leaves the
whole run on the game renderer and counts under `menu_refused_pass`.
An active model widget can contribute a camera and object handles to the
scene overlay while its geometry draws in the nested model view.
The blur under a dialog counts as `modal_blurs` where the overlay's blur
replaced its quads, `modal_blur_replayed` where the game's quads were
given back, `modal_blur_refused_pass` where the host did not arm it, and
`modal_blur_refused_arguments`, `_steps`, `_combiner` and `_targets` for
calls not read
([the blur under a dialog](../graphics/native-post-hud.md#the-blur-under-a-dialog)).
The object-shadow list, row gather and caster each count calls to their own
entry ([native-light-gather.md](../graphics/native-light-gather.md), L7).
`counters` is `null` where the frame-stats window is not running.

`ui` is the layout the menus and the HUD are drawn with
([high-resolution.md](../graphics/high-resolution.md#the-games-640x480-assumptions)),
as the game's UI scopes last set it; absent before the first one drew. Its
fields:

| Field | Meaning |
|---|---|
| `output` | `[width, height]`, the window the UI is drawn at; the guest's 640x480 canvas before the first menu scope |
| `scale` | the menu fit's `[sx, sy]`, guest window units: guest `p` draws at `p * scale + offset` |
| `offset` | the fit's translation, guest window units |
| `anchor` | how the HUD's corner groups anchor: `pinned` to the window's edges at their physical scale, or `centered` on the safe area's edges |
| `atlas_scale` | the whole multiple of a glyph's own size the text atlas rasterises at (0 before the first text run) |
| `atlas_page` | the side of a glyph page in texels |

The fit is `render_bridge::ui_layout`'s one rule: the safe area is the
largest centred 4:3 box (`min(w/640, h/480)` display pixels per guest
pixel), and a live resize moves it on the next frame. A check resizes the
window (`host.window.resize`), reads `render.stats`'s `ui`, and expects
`scale` to hold the new window's uniform fit while `anchor` and
`atlas_scale` stay.

```sh
h2 agent render.stats | jq .ui
```

`display_rate` is present while a paced frame owner runs it: a window
or GPU serve with the render thread on, `frame-scene.sink` not off, and no
locked mailbox (`H2_BOOT_STOP_AT_PRESENT`, an unpaced display or a
headless run lock it). Its fields, counted since the loop was installed:

| Field | Meaning |
|---|---|
| `rate_hz` | the rate `graphics.fps` asks for; 0 at 30 |
| `shown_hz` | the rate after the headroom cap |
| `capped` | display frames do not fit, so the rate is held below `rate_hz` |
| `swaps` | owned presents the loop saw |
| `blended` | display frames drawn with blended cameras, at a Swap or between |
| `swap_blends` | of `blended`, the ones shown in place of a Swap's scene |
| `between_newest` | frames between Swaps drawn as the newest scene (alpha 1, a cut or a gap) |
| `failed` | display frames that failed; the window kept the frame before |
| `resets` | pairs dropped by a Swap without the graph |
| `avg_cost_us` | mean render-thread cost of a display frame |
| `marks_moved` | views on display frames whose world marks were moved onto the camera the frame drew them with |
| `marks_held` | views on display frames whose world marks kept their image's camera (no drawn view at their viewport) |

At `--fps 60` and above with the frame owner on, `blended` rises with
play; at 30 it stays 0 and `swaps` still counts. The loop is in
`crates/rewrites/frame-scene/src/display.rs` and
`crates/rewrites/native-render/src/frame_graph/display_rate.rs`.

```sh
h2 agent render.stats | jq .display_rate.blended
```

`display_frame` gives the identity, lighting and backend counter deltas
when the worker's latest presentation used the native graph. An
original presentation clears it. The frame recorder can stay unarmed
for these counters. `render.scene` includes them when the map epoch,
frame and tick match its sealed scene.

`private_frame` reports the counts and identity of the most recent
completed private target draw. It is absent before the first capture and
after a renderer reset. `render.scene` includes the same field when its
sealed scene matches the private capture's map epoch, frame, tick and
summary hash.

A completed native E3 draw includes an `e3` object in its
`display_frame` or `private_frame`. The armed `render.frame` record
includes it for the corresponding draw. Its `passes` array contains
`e3.tiles`, `e3.sun_tiles`, `e3.sun`, `e3.marks` and `e3.lights`. Each row has
`name`, submitted `draws` and `gpu_ns` (null for an empty or unmeasured
stage). Snapshot indices refer to the attachment runs in the frame
record. The per-frame counters are `lights_lit`, `lights_shadowed`,
`lights_over_cap`, `lights_beyond_guest`, `sun_tiles`, `sun_tiles_redrawn`, `sun_tiles_offscreen`, `atlas_halved`,
`tiles_refused`, `cones_shadowed`, `cones_flagged_off`, `flashlight_shadowed`,
`shadows_over_cap`, `cones_refused`, `cones_drawn` and `cones_kept`. They
count the E3 renderer's selections and atlas work.
The graph draws the guest's shadow maps as tiles
([e3-lighting.md](../graphics/e3-lighting.md), "Shadows from local
lights"): `e3.tiles` counts the caster draws into them, `e3.marks` the
receivers compared with the Sharp edge, `lights_shadowed` the tiles (one
per shadowed light of the guest's) and `tiles_refused` the shadow maps
left at the guest's size. The display on the guest's live attachments
draws no tiles. `lights_lit` and `lights_over_cap` are the renderer's
own choice of lights from the scene's candidates (e3-lighting.md,
"Lights: which, and how many"): the lights it chose, and the candidates
its cap of 16 left out. `lights_beyond_guest` counts the chosen game
lights the guest did not draw, and `e3.lights` the ranges of the stages
that draw them (layers 12 and 13 over the structure each light meets),
which only an E3 draw records. The five cone counters are the renderer's
choice of shadowed lights from those it lit (e3-lighting.md, "Built: the
shadowed cones"): the cones it shadows itself, the shadowed lights whose
content flag was off, the views whose flashlight holds a slot, the lit
cones its four slots left out, and the lit lights whose frustum makes no
projection. A chosen light the renderer shadows reads its cone's term:
`cones_drawn` counts the cone tiles drawn for those lights (their caster
draws join `e3.tiles`, their receivers `e3.marks`) and `cones_kept` the
lights left undrawn because their cone could not be. `sun_tiles_offscreen` counts offscreen casters for which the graph
submits a receiver draw, summed across views. A cached tile can have
receiver draws with no redraw. The
guest's light selections are in `render.scene.views`.
Retail graph draws omit `e3`.

`H2_BOOT_GFX_BETWEEN=N` writes the first N display frames between
scenes as PNGs, with the guest frame before and after each
([switches.md](switches.md)).

### `render.capture_clip`

`render.capture_clip {"tick":9000,"ticks":180,"fps":120}` arms a clip on
the game thread. `tick` is 1 to 240 ticks ahead, `ticks` is 1 to 300, and
`fps` is 30, 60 or 120. The reply contains `id`, `path`, `frames` and
`written`. The GPU hooks write `frame-00000.png` onward under `path`, a
clip folder under the headless graphics output directory.

`render.capture_clip.status {"id":1}` returns the same fields for the
latest clip. When `written` equals `frames`, all PNG files are closed.
A lost guest tick or missing display slot fails the request.

Clip capture needs headless GPU rendering. The [graphics size switch](switches.md)
sets the clip's output dimensions. The writer takes one frame per tick,
so `fps` above 30 is refused: display frames between scenes are not
written to clip slots. PNG readback and encoding may slow capture.

The state is in `crates/extensions/agent/src/render/clip.rs`. The GPU
writer is in `crates/extensions/game/src/d3d_glue/clip_png.rs`.

### Picture feed: `render.picture`

A client shows the game's presented frames live: Studio's Game panel
([game-view.md](../editor/game-view.md)) and `h2 agent picture`. Frames
come from the same handoff `host.screenshot` reads (the window's newest
published frame), not a sealed scene; for a still at a tick use
`render.capture`.

| Method | Access | Params | Result |
|---|---|---|---|
| `render.picture.open` | read | `{width?, fps?, transport?}` | `{id, transport, path?, size, format, fps, at}`; then a `picture` event per frame (ring) or binary frames (stream) |
| `render.picture.close` | read | `{id}` | `{closed}`: whether the picture was open |

`transport` is `ring` (the default) or `stream`. `width` is the widest
frame wanted, 16 to 2560; a frame keeps its aspect, fits a `width` x
`width` box and is never scaled up. `fps` is 1 to 60. A ring defaults to
1280 wide at 60, a stream to 640 wide at 15. `size` is the largest frame,
`[width, width]`; each frame's own size is in its slot and its event. A
game with no frame source (no window: `h2 serve`, the Xbox app) answers
`UNAVAILABLE`.

**Ring.** For a client on the same machine. `path` is a file under the
cache directory's `pictures/`, mode 0600, named for the process and the
picture. It holds a 256-byte header (the newest frame number, the
reader's acknowledgement, a closed flag, each slot's sequence number,
frame number, tick, present, time and size) and three RGBA8 slots
(`agent_proto::picture::Ring` has the layout, the writer and the reader).
Frame `n` goes into slot `n % 3`, written while its sequence number is
odd. A reader copies the newest slot and keeps the copy only if the
sequence was even and unchanged across it (`Ring::read_newest`), then
writes the frame number back as its acknowledgement. The opening
connection gets, after the reply, one event per frame:

```json
{"jsonrpc":"2.0","method":"event","params":{"kind":"picture","id":1,"frame":42,
 "tick":48211,"present":96400,"time_ms":700.4,"width":1280,"height":720}}
```

Events go only to that connection, whatever it subscribed to, and are
dropped (not queued) when its queue is full. The frame is in the ring
either way.

**Stream.** For a game reached over `tcp://`, which cannot share memory.
Open it on a connection of its own: after the reply line the connection
switches to length-prefixed binary frames, as `live.subscribe`'s does,
and takes no more requests; close it from another connection or by
disconnecting. A connection that holds control, has subscribed to
events or has requests in flight is refused with `BUSY`, and an
in-process connection with `UNAVAILABLE`. Each frame is a little-endian
u32 payload length (at most 16 MiB), then a u32 header length, the
`picture` event's fields as JSON, and a JPEG (quality 75, no alpha)
(`agent_proto::picture::read_stream_frame`). At most three frames wait
for the socket; a frame that finds the queue full is not read back.

**Cost and the idle skip.** A worker thread (`agent-picture`), running
while any picture is open, reads the newest presented frame back once per
due frame of the fastest picture, scales it on the CPU by averaging, and
writes it to every picture that is due. A ring whose newest frame stays
unacknowledged for a second (`IDLE_AFTER`) and a stream whose queue is
full are skipped; with nothing due, nothing is read back. The readback
runs on the worker, not the game thread, on the GPU queue the game
presents on. Its cost at 60 Hz is not measured.

**Lifetime.** `render.picture.close`, the opening connection closing or
being dropped, and the game exiting each close the picture: a ring's
closed flag is set and its file removed, a stream's connection ends.

```sh
h2 agent picture                           # a window showing the game, from a ring
h2 agent picture --width 640 --fps 30      # smaller, slower
h2 agent --socket tcp://10.0.3.29:4720 picture   # a remote game: the stream
h2 agent picture --frames 60               # no window: sizes, rate and ticks of 60 frames
```

The methods are answered on the executor (`crates/extensions/agent/src/exec/picture.rs`);
the pictures, the worker, the scaling and the JPEG are in
`crates/extensions/agent/src/render/picture.rs`; the wire types, the ring
and the stream codec in `crates/extensions/agent-proto/src/picture.rs`.
The window shell and a headless app host attach the frame source at
`agent_proto::picture::hub` (`crates/apps/app/src/picture.rs`). The client
is `crates/apps/cli/src/agent_picture.rs`.

### `render.marks`

`render.marks` (no params) reads where the window drew the world marks
([Drawing world-space lines over the frame](agent-api-edit.md#drawing-world-space-lines-over-the-frame))
on its last 32 presents. The first call arms the window's probe and
returns `armed: false` with no presents. Unarmed, the window pays one
atomic load a present. Each later call returns `armed: true` and
`presents`, oldest first. Each present has:

- `seq`, counting the window's draws since the probe was armed;
- `age_ms`, how long ago the window drew it;
- `frame`: `guest` (a genuine image), `synthetic` (an in-between frame
  at 60 or 120 fps), `no_world_view`, `raw` or `loading`;
- `fraction`: a synthetic frame's blend fraction, 0 the frame before and
  1 the newest; null otherwise or without marks;
- `shifted`: the synthetic frame drew some view with another camera than
  its marks' image, and the marks were moved onto it
  ([high-framerate.md](../game/high-framerate.md), "World marks on made frames");
- `anchors`: the first 16 mark groups' anchors in window pixels through
  the first view's camera, null where off screen or behind it; empty
  without marks.

A headless host has no window and returns no presents. To check that
marks stay on the world at 60 fps, turn at a steady rate with a mark in
view and read twice: an anchor's `x` steps by a similar amount on every
present, genuine and synthetic alike
(`crates/platform/gfx/src/world_lines/probe.rs`,
`crates/apps/app/src/shell.rs`, `crates/extensions/game/src/world_lines.rs`).

```sh
h2 agent render.marks
h2 agent render.marks | jq '.presents[] | [.frame, .fraction, .shifted, .anchors[0]]'
```

### `render.markers`

`render.markers` reads the Forge game type markers the window's last
capture drew ([gametype-markers.md](../game/gametype-markers.md), "The
game"). Params are `{}`, `{"kind": "spawn"}`, `{"form": "edit"}` or
both: `kind` is a kind name of
`game_markers::Kind::name` (`spawn`, `flag_stand`, `hill`, `ff_drop`, ...) or `wall`,
and `form` is `edit` or `play`. A name that is not one is refused with
`invalid params` and the list of names, and so is any other field. The
answer is

```json
{"shown": 2, "hidden_by_budget": 0,
 "markers": [{"thing": "starting_locations:0", "kind": "spawn", "team": 0,
              "anchor": [1.0, 2.0, 0.85], "lines": 38, "faded": false,
              "form": "edit", "icon": "respawn", "plate_scale": 1.0}]}
```

A row is `agent_proto::MarkerRow`.

- The walls a running rules script shows in play (`show_area`) are rows
  too: `kind` is `wall`, `thing` is `zone:<hex of the zone object>` and
  `team` the colour of its wall; they count in `shown`, are not held to
  the 256 and draw with no Forge session.
- `shown` counts the markers the capture drew and `hidden_by_budget` the
  ones in range that the 256 a view left out, the nearest first. A marker
  past its distance fade is in neither. The counts cover every kind; `kind`
  filters only `markers`, which lists the nearest first.
- `thing` names what the marker stands for: `<block>:<index>` for an entry
  of the map, `added:<op>` for one the document added, `problem:<label>`
  for a ground label the last build could not honour.
- `team` is 0 to 7, or null. `anchor` is where the label sits, in world
  units; `render.marks` gives window pixels for the groups the window
  visited. `lines` is the marker's line count, its dimmed edit-only
  parts' included. `faded` is a marker of another game type, drawn at a
  quarter.
- `form` is the form it drew ([gametype-marker-forms.md](../game/gametype-marker-forms.md),
  "When a form shows"): `edit` for every Forge marker with the monitor
  on, its play form's parts at full strength and the rest at 0.6 of it;
  a wall is `play`. `icon` is the icon on its plate
  (`game_markers::Icon::name`), null for a wall and a volume.
  `plate_scale` is how much its plate, turned to the eye, was scaled
  with the eye's distance: 1 out to 8 world units, then the distance
  over 8; null when no plate drew. `kind` and `form` filter only
  `markers`.
- In a match (no Forge session, or the monitor off) every netgame flag the
  map's document shows is a row in its play form: `form` `play`, `thing`
  `netgame_flags:<index>` and the flag's marker kind (a capture plate is a
  `flag_return` row). A retail map has none.
- With no Forge session (or the monitor off), no flag shown, no wall shown
  and no present since, `shown` is 0 and `markers` is empty. A headless host has no window and draws none.

The method is the host's (`crates/extensions/game/src/forge_glue/markers_world.rs`,
`add_method`; the name is `agent_proto::method::RENDER_MARKERS`).

```sh
h2 agent render.markers
h2 agent render.markers '{"kind":"hill"}'
```

### `render.particles.refusals`

`render.particles.refusals` says why the native particle path left draws to
D3D. Every geometry refusal has a named check, which the present's refusal
log also carries (`Particle/Geometry/primitive=428`). The answer keeps, per
check, a count and the shape of the first draw that failed it. It takes no
params; any field is refused with `invalid params`.

```json
{"refused": 428,
 "samples": {"primitive": {
   "label": "Geometry/primitive", "producer": "Particle", "count": 428,
   "shape": {"entry": "D3DDevice_DrawVertices", "primitive": "TriangleList",
             "vertex_count": 6, "indexed": false, "pretransformed": false,
             "layouts": 1, "streams": 1, "layout_strides": [8],
             "stream_strides": [8], "stream_bytes": [48],
             "attribute_count": 1,
             "attributes": [{"stream": 0, "register": 0, "data_type": "Short4", "offset": 0}],
             "provided_inputs": 1}}}}
```

- The checks, in the order they run: `indexed`, `pretransformed`,
  `layouts` (not one stream layout), `streams` (not one stream),
  `stride` (stream index, layout against stream, or the producer's
  stride), `attributes` (not exactly one for a particle),
  `attribute_slot` (offset or register), `primitive`, `vertex_count`,
  `data_len`, `data_type`, `inputs` (the shader's provided registers),
  `corner_differs` (the stream
  differs from the published corner values), `unpublished`,
  `data_differs` (a dynamic call's bytes), `indices`, and the ribbon
  bytes' `raw_bytes`, `raw_attributes` and `raw_vertex_count`.
- `count` totals the run, not one present; the per-window counters are in
  the log. `producer` is the producer of the first draw (`Particle`,
  `Dynamic`, `Immediate`).
- A draw the captured-particle fallback takes is not a refusal and is not
  counted. Refusals of other kinds (target, state, shader) are only in the
  log.

The method is the host's (`crates/extensions/game/src/native_particles_admission.rs`,
`add_refusals_method`; the name is `agent_proto::method::RENDER_PARTICLES_REFUSALS`).

```sh
h2 agent render.particles.refusals
```

### `render.coop_look`

A read-only host method registered with the Live co-op prediction
feature (`crates/extensions/game/src/coop_look.rs`). With no parameters it returns
the most recent 256 rendered local views, oldest first; `{"after":123}`
returns samples newer than that cursor. `latest` is the most recent
sample number, initially zero; a gap in sample numbers means the reader
fell behind. One frame can contain several local users. The render hook
posts each draw and the game thread logs it at its next co-op hook, so a
sample shows up a tick after its frame.

```sh
h2 agent --socket ~/Library/Caches/h2recomp/coop-bravo.sock render.coop_look
```

Each entry in `frames` has a monotonic `sample`, the guest render `frame`,
the local `user` (0 to 3), and a `status` such as `active`, `waiting`
(awaiting a baseline), `host`, `setting off`, `cinematic`, `dead`,
`director`, `vehicle`, or `no local control`. These are guest draws;
additional display-rate replays reuse the guest frame.

| Field | Meaning |
|---|---|
| `sent` | Latest outgoing retail look, also recorded with the setting off for A/B timing |
| `confirmed` | Local player's yaw and pitch from the last applied update carrying that player's action |
| `predicted` | Current absolute target from the still-pending sends, or confirmed aim once they have drained |
| `camera`, `drawn` | First render camera before and after the adjustment; all aims are `{yaw,pitch}` in radians, or null when unavailable |
| `correction_rad` | Largest reconciliation error for this draw in radians (Euclidean length of shortest yaw difference and pitch difference); a pending turn's lead is measured by comparing predicted and confirmed |
| `update`, `last_send`, `matched_send`, `matched_update` | Last applied update, last sent sequence, and the sequence/update pair most recently matched by quantized aim; null before the event |
| `pending`, `dropped` | Unmatched sends, and the number superseded or expired since this continuous camera began; identical quantized actions have indistinguishable echoes |

### FrameScene checks: compare, capture, scene, parity, hashes

In [agent-api-render-checks.md](agent-api-render-checks.md#framescene-checks-compare-capture-scene-parity-hashes).

### `render.residual`: residual D3D work

In [agent-api-render-checks.md](agent-api-render-checks.md#renderresidual-residual-d3d-work).
