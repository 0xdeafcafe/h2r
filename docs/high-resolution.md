# High resolution: 4K and arbitrary internal resolution

A design for rendering above the game's 640x480, as an opt-in setting. Nothing
here is built. It was written from the code and docs on master
([d3d.md](d3d.md), [graphics.md](graphics.md), [shaders.md](shaders.md),
[render-thread.md](render-thread.md)) and from public prior art, without
building or running the game. Every number under "Cost" is arithmetic or a
figure quoted from another doc, not a new measurement.

The setting is `render_scale` in the settings design
(`docs/settings-menu.md`, in progress on another branch). This page proposes
`H2_RENDER_SCALE` as the developer flag, and a per-axis form for widescreen.

## Summary

- **The guest never learns the resolution.** The device (`d3d::Device`) keeps
  every size, viewport, scissor, header and return value in guest pixels, as
  today. Scaling lives below the `d3d::Backend` trait.
- **Two backends, one draw stream.** A *reference* backend renders at 1x and
  answers everything the guest can observe. A *view* backend renders the same
  `ResolvedDraw`s at the chosen scale and owns what is presented. Guest-visible
  state is then the 1x run's by construction, not by argument.
- **Scale with the back buffer:** the frame buffers, the depth buffer, the
  textures the game builds over both, and any render target the size of the
  back buffer. **Leave at guest size at first:** the small square targets
  (bloom, blur, reflection and shadow buffers).
- **Integer scale per axis is the exact class.** Fractional scales work for
  geometry but break 1:1 point-sampled copies, so they come later and are
  marked experimental. 4K output is an integer internal size plus the
  presenter's resample.
- **Shadow maps scale last**, because sampled depth levels go through guest
  memory today.
- **MSAA and forced anisotropy are separate, later options** on the view only.

## Terms

- **Guest pixels**: the coordinates the game uses. The back buffer is 640x480
  ([d3d.md](d3d.md), "Guest structures").
- **Host pixels**: texels of the wgpu surface that stands for a guest surface.
- **Scale** `(sx, sy)`: host pixels per guest pixel, per axis. Halo 2's
  widescreen is anamorphic 640x480 shown at 16:9 ([app.md](app.md),
  "Scaling"), so a 16:9 output wants `sx` larger than `sy`.
- **Reference** and **view**: the two backends above.

## What the guest can observe

From [render-thread.md](render-thread.md) ("What can move") and
[d3d.md](d3d.md), the guest sees the GPU only through values:

| Channel | Where | Scaled rendering could change it? |
|---|---|---|
| Render-to-texture write-backs into guest memory | `Backend::read_surface`, `read_depth`; lazily at `BlockOnResource`, the busy check, fences, push-buffer callbacks (`0x24970`), `PersistDisplay` | Yes. These are pixels. |
| Pixel counts | `GetVisibilityTestResult` (`0x3f8630`); the lens-flare pass `0x20bb0` divides the count by an expected count | Yes. A count is a number of samples. |
| Draw outcomes | drawn or skipped: marks levels dirty, bumps the depth generation, records `Event::DrawSkipped` | Only if the view's answer were used. It must not be. |
| D3D return values, headers, device fields, viewport words | the device | No, if the device never sees the scale. |
| Fences, register words, vblank, swap count | the device and the kernel, on logical time | No. |
| Frame-buffer and depth-buffer contents | never in guest memory; a CPU read through their `Data` faults | No. |
| Textures over the depth buffer (`0x509368`, `0x50936C`) | decoded on the host; the bytes never reach guest memory | No. |

So two channels carry pixels to the guest: write-backs and counts. What the
game does with them is not fully traced. The callback at `0x24970` reads
texels and its purpose is not recorded in our docs; the flare ratio at least
feeds rendering. Until that is traced, assume both can reach the simulation.

## Design: reference and view

```text
 Device (guest pixels, unchanged)
    |  Backend trait: draws, clears, surface ops, reads
    v
 ScaledBackend
    |-- reference: GfxBackend at 1x   -> read_surface, read_depth,
    |                                    read_pixel_count, judge, outcomes
    `-- view:      GfxBackend at s    -> present, screenshots
```

Each `Backend` call falls into one class:

1. **Reads and outcomes** (`read_surface`, `read_depth`, `read_pixel_count`,
   `judge`, every `DrawOutcome`): the reference answers. The view is not
   consulted.
2. **Mutations** (`draw`, `clear_rects`, `write_surface`, `copy_surface`,
   `snapshot_surface`, `create_surface`, `create_texture_target`,
   `surface_texture`, `depth_alias_texture`): go to both. Handles are pairs.
3. **`upload_texture`**: decoded once. The two backends should share one wgpu
   device and the uploaded texture, so ordinary textures are not held twice.
4. **`present`**: the view presents. The reference still rotates its swap ring,
   because textures over the display surface read the previous frame.

A view failure (a draw it cannot make, an allocation that fails at 4K) must
not stop or alter the guest. It is counted, and the frame falls back to the
reference image. This differs from the render thread, where a disagreement
stops the run, because there the disagreeing backend is the one the guest
observes.

Why not scale a single backend, as xemu does? Because its readbacks then come
from the scaled image. xemu decimates on download (one host pixel per block)
and replicates on upload. That is deterministic, but the bytes differ from a
1x run wherever an edge crosses a pixel, and sample counts differ by more than
the scale factor's square. A single scaled backend is described under
"Phases" as a later, explicitly inexact mode.

The cost of the second backend is CPU, not GPU, and is under "Cost".

## Which surfaces scale

| Surface | Policy | Why |
|---|---|---|
| Back buffer, display surface, third buffer | Scale | The image. |
| Depth buffer | Scale, always with the frame buffers | Attachments must match. The existing depth-larger-than-colour stand-in (`prepare_target`) scales both sides. |
| Textures over the frame buffers (`0x509354`, `0x509358`; `snapshot_surface`) | Scale: the snapshot is a copy of the scaled buffer | Current-frame feedback and the previous-frame crossfade sample them in screen space. |
| Textures and the colour surface over the depth buffer (`0x509368`, `0x50936C`, `0x509370`) | Scale: built from the view's depth buffer | The menu's fog and distortion pass reads depth per pixel. Needs the GPU depth-to-colour pass (below). |
| A render-to-texture level the size of the back buffer | Scale | Halo-2-HD reports one intermediate target, texaccum, that follows the back-buffer size (see "Prior art"). A full-screen intermediate left at 640x480 would undo the whole feature. Not yet confirmed in our own captures. |
| Fixed-size square targets (the `0x14850` 64x64 pair, the 128x128 and 512x512 targets seen in the first level) | Do not scale in the first phases | Bloom and blur kernels are measured in texels. Scaling the target shrinks the blur on screen. |
| Shadow maps (depth levels bound as `pNewZStencil`, sampled as `LIN_D24S8`) | Do not scale at first; optional later | They are sampled through guest memory today, and the comparison indexes texels. |
| Ordinary textures | Never | They are the game's assets. |

The rule for classifying a render-to-texture level is its guest size against
the back buffer's guest size, checked when the host surface is created. An
allow-list by render-target index (`0x14bc0` selects targets by index) is the
fallback if a level needs an exception.

### Mixed-scale sampling

A draw can sample a scaled surface into an unscaled target. The first
downsample of a bloom chain does this: back buffer in, small target out. At 4x
a single bilinear tap then skips most of the source and the bloom shimmers.

The view keeps, per scaled surface, a lazily made **guest-size resolve**: a
box filter of the scaled image. The rule:

- target scaled, source scaled: sample the scaled surface;
- target unscaled, source scaled: sample the resolve;
- source unscaled: sample it as it is, whatever the target.

This is the only place the view needs a downsample. It never reads back to the
CPU.

### Depth as a texture

Textures over the depth buffer are made today by a CPU readback, a decode and
an upload, once per depth generation ([d3d.md](d3d.md), "Frame buffers as
textures"). At 3840x2160 that is about 33 MB each way per generation, on a
pass the menu runs every frame. The view needs the pass [d3d.md](d3d.md)
already lists as wanted ("Depth as a texture on the GPU"): a full-screen pass
that writes the depth and stencil bits into a colour texture in the alias's
channel order. It is a prerequisite here, and a speed-up at 1x too.

Those textures hold packed bits. They must be point sampled, at any scale.
A stage that samples one with a linear filter is already wrong at 1x and
should be counted, not fixed here.

## Coordinates

### Positions

Translated vertex programs output guest window coordinates, and the fixup maps
them to clip space with the guest target size (`VertexUniforms::surface`,
[shaders.md](shaders.md), "Position fixup"). Nothing in that mapping depends
on the host size. The built-in pre-transformed shader inverts the guest
viewport the same way. So geometry needs no change: `surface` stays the guest
size and the rasteriser does the rest.

Three details:

- **Subpixel truncation.** The fixup truncates x and y to 1/16 of a guest
  pixel, as the NV2A does. At 4x that is a quarter of a host pixel. Keeping it
  preserves the retail triangle edges. Snapping to 1/16 of a host pixel is a
  possible view-only refinement and is not proposed for the first cut.
- **Slope-scaled depth bias.** The W-buffer path computes the polygon offset
  from screen-space derivatives (`max(|dpdx|, |dpdy|)`). Derivatives per host
  pixel are smaller by the scale, so the slope term shrinks. That is the
  correct behaviour for a bias per sample step, but decals tuned at 480p need
  a test.
- **Stipple.** The pattern is indexed by pixel position ([d3d.md](d3d.md),
  `SetStipple`). The view must index it by guest pixel (`floor(position /
  scale)`), or the pattern becomes finer than retail.

### Scissor, clears and viewport

`Draw::scissor`, `ScissorRect` clears and `Draw::viewport` are in pixels of the
target. The view multiplies them by the target's scale. For an integer scale
this is exact. For a fractional scale every edge must be rounded by the same
function (`floor(edge * s + 0.5)`), so the disjoint pieces of a split scissor
stay disjoint and gapless ([d3d.md](d3d.md), `clip_rects`). xemu had this
wrong for inclusive bounds at one point (xemu issue 1054).

### Half-pixel and texel offsets

The game and D3D put half-pixel offsets in the viewport offset constant, and
the pre-transformed path adds its own (`FF_HALF_PIXEL`). These are lengths in
guest pixels. Scaled, a 0.5 guest-pixel offset is `0.5 * s` host pixels. For
geometry this is harmless: the image is the same continuous image, sampled
more finely.

It matters for screen-space copies. Take a pass that maps guest pixel centres
onto guest texel centres with a shift of `d` guest texels. A host pixel centre
then lands on a host texel centre only when `d * s` is an integer:

- **1:1 copies** (`d` an integer) stay exact for any integer scale. Point
  sampling is safe.
- **Deliberate half-texel taps** (`d = 0.5`, the usual four-tap box blur with
  bilinear filtering) land on texel centres at even scales and on corners at
  odd ones. With a linear filter the result is a narrower blur than retail.
  With a point filter, at an odd scale, the tap picks a neighbour and a seam
  appears. xemu issue 1771 is this case.
- **Fractional scales** break the 1:1 case too.

Three rules follow:

1. Texture-size uniforms take the guest size: `tex_scale` for texel-addressed
   (`LIN_`) textures, and the stage dimensions the engine writes into vertex
   constants ([shader-instances.md](shader-instances.md), "Engine inputs";
   those come from guest slots and need nothing).
2. Code that indexes texels must use the host texture's real dimensions for
   the index and the guest size for the coordinate. The shadow comparison
   (`floor(uv * size)`, [shaders.md](shaders.md), "Shadow maps") is the one
   known case and matters only once shadow maps scale.
3. Passes whose source and target are both unscaled are untouched. This is
   the main reason to leave the blur chain at guest size.

## Post-processing

What we know about the retail post-processing chain is thin. Our docs record
the targets it uses, not what each pass does: the two 64x64 targets built at
`0x14850`, 128x128 and 512x512 targets in the first level, the previous-frame
crossfade (`0x002ab270`) and the depth-based fog and distortion pass on the
menu. Halo-2-HD's write-up counts about 25 render targets in four or five
allocations, from 512x512 down to 64x64.

Policy for the first phases: every pass runs on targets at guest size, and
only the final composite onto the back buffer is at the view's scale. The
bloom is then exactly as wide on screen as in retail, upsampled bilinearly.
That is what a blur wants.

Known failure modes to test for, all seen in xemu at more than 1x:

- bloom that ghosts or shrinks (issues 2728, 2747, 1926);
- history or intermediate buffers kept at 1x while the scene scales
  (issue 993, and the limits listed in PR 3065);
- a row or column of garbage at the screen edge (issues 57, 508);
- water and texture seams in Halo: Combat Evolved (issues 865, 602).

No scale-specific Halo 2 issue was found in xemu's tracker.

## The game's 640x480 assumptions

Because the guest keeps its 640x480 back buffer, the game's own arithmetic
stays valid:

- **Viewports.** `0x14bc0` calls `SetViewport` with the selected target's
  extents, and split screen uses smaller ones. The device mirrors the viewport
  words in guest pixels. Unchanged.
- **Scissors.** `SetScissors` (`0x3f8760`) and the device's scissor region
  work in guest pixels. The view scales the result.
- **HUD and UI.** Widget bounds are in a 640x480 frame ([ui.md](ui.md)). They
  keep their proportions. Bitmaps and font glyphs are 480p assets and will
  look soft or blocky when magnified; sharper text is a separate project.
- **Loading screens and movies** draw through the same path and are upscaled.

This is the main difference from Halo-2-HD, which raises the guest's own
resolution and so had to patch each of these (see "Prior art").

Two things remain:

- **UI atlases.** A linearly filtered bitmap drawn 1:1 at 480p is magnified at
  a higher scale, and neighbours in an atlas can bleed in at its edge. xemu
  issue 602 reports texture bleeding from 2x up, which may be this. Needs a
  capture-based test before deciding on a fix.
- **Lines and points.** wgpu lines are one host pixel wide, so they get
  thinner. Point sprites are not implemented yet; when they are expanded to
  quads in guest window units they will scale for free. Whether Halo 2 draws
  lines in retail paths is not known.

## Filtering, anisotropy and MSAA

**Mip selection** needs nothing. The host computes derivatives per host pixel,
so a scaled target picks sharper mips by `log2(s)` and bottoms out at the top
level. The engine's own LOD inputs (`MIPMAPLODBIAS`, `MAXMIPLEVEL`, the
per-draw LOD term) are guest state and stay.

**Anisotropy.** Today the sampler is anisotropic only when the guest asks
(`gfx::translate`: an `Anisotropic` filter with `MAXANISOTROPY` above 1).
A view-only override, off by default, may raise the clamp on a stage when all
of these hold:

- the guest's min and mag filters are linear and it has a mip filter;
- the texture is an ordinary uploaded texture, not a surface, snapshot or
  depth alias;
- the format is not a depth format, a palette index or a `HILO` pair, and the
  stage has no `COLORSIGN`;
- the texture mode is a plain projective fetch, not a dependent read
  (bump-env, dot-product modes).

Everything excluded is data, not an image, and filtering it differently
changes shading. The override changes the pipeline's sampler key, so it must
not create new pipelines per draw.

**MSAA.** Halo's device is not multisampled ([d3d.md](d3d.md), "Swap"), and
the guest's `MULTISAMPLE*` states stay as the game set them. Host-forced MSAA
on the view is possible but is the weaker tool:

- alpha-tested edges (foliage, grates) are not smoothed, and the translated
  programs emulate alpha test with `discard`;
- every snapshot and depth alias needs a resolve first, and depth has no
  meaningful average;
- render-to-texture surfaces have no MSAA in `gfx` today;
- the W-buffer path writes `frag_depth`, which is per pixel, not per sample;
- the sample count is part of the pipeline key, so every pipeline compiles
  again, at about 220 ms per Metal pipeline on a cold cache
  ([frame-budget.md](frame-budget.md)).

Supersampling through the scale handles all of those uniformly. MSAA is a
later option for hosts that cannot afford the fill. It must never reach the
reference, where sample counts are guest-visible.

## Determinism

The claim: **with scaling on, every value the guest reads is the value it
reads with scaling off, on the same host.** The reference is today's backend
with today's inputs. The view receives copies of the same calls and returns
nothing the device uses.

What this does not claim: identical values across different GPUs or drivers.
That is not true at 1x either, since readbacks come from the host rasteriser.

What must hold for the claim, and how each is checked:

| Requirement | Check |
|---|---|
| The device's code path does not depend on the scale | The scale is not visible above `Backend`. A device test with `RecordingBackend` produces the same call list under a wrapped and an unwrapped backend. |
| The view never writes guest memory, the logical clock or the scheduler | It holds no guest handle, like the render thread. |
| The view never causes a guest-visible wait | Its work is not on the class-1 read path. Any wait is on wall time and charges no logical time ([render-thread.md](render-thread.md)). |
| View errors do not surface as backend errors | They are counted, and the frame falls back. A test injects a failing view. |
| Screenshots used as goldens come from the reference | `H2_BOOT_GFX` PNGs and capture snapshots stay 1x unless asked. |

The acceptance test is a digest comparison: the same input timeline at scale
1 and at scale N must give the same guest memory digest, the same D3D return
trace and the same reference frames.

## Cost

Pixels and attachment memory for the scaled set (three frame buffers at
4 bytes a pixel, one depth buffer at 8, one snapshot and one resolve in
flight):

| Internal size | Scale | Pixels | vs 640x480 | Scaled attachments |
|---|---|---:|---:|---:|
| 640x480 | 1 x 1 | 0.31 M | 1 | about 9 MB |
| 1280x960 | 2 x 2 | 1.23 M | 4 | about 34 MB |
| 1920x1440 | 3 x 3 | 2.76 M | 9 | about 77 MB |
| 2560x1920 | 4 x 4 | 4.92 M | 16 | about 138 MB |
| 3840x2400 | 6 x 5 | 9.22 M | 30 | about 258 MB |

A back-buffer-sized render target adds 4 bytes a pixel each. 3840x2400 is the
integer size for a 16:9 4K display: the presenter resamples it to 3840x2160.
A 4:3 4K image (2880x2160) is 4.5x, so it renders at 5 x 5 or 4 x 4 and is
resampled.

- **GPU.** Fragment work on scaled targets grows with the pixel count. Work on
  unscaled targets does not grow. Snapshots are full copies of the back
  buffer: at 3840x2400 each is about 37 MB, and the menu took five a frame
  when [perf-budget.md](perf-budget.md) was measured. That is about 185 MB of
  GPU copies a frame, which is tolerable but is the first thing to measure.
- **CPU.** This is the real cost. The runtime is CPU-bound today: about 5-6
  fps on the menu. The second backend repeats per-draw work (uniforms, bind
  groups, recording, encoding, submission) but not push-buffer decode, draw
  resolve, texture decode or shader translation. When the render thread was
  measured, the backend's CPU share was about 42 ms of a 168 ms menu frame
  ([render-thread.md](render-thread.md), "Measurements"); a second backend
  would have added roughly that again. Merged passes have cut it since. The
  view should run on its own thread, so the cost is cores, not frame time.
- **Readbacks.** None are added. The view never reads back.
- **Counts.** The count pass (a second draw into an `R16Float` target against
  a scratch depth copy) runs only on the reference. At 4K a scratch copy per
  counted draw would be about 70 MB; the split avoids it.

If the second backend's CPU cost is too high, the reference can become lazy:
keep each surface's draws queued and run them only when a read needs that
surface, dropping the queue when the surface is cleared whole or presented.
On the menu the render thread saw no reads in a frame at all. This needs
dependency tracking between surfaces, so it is an optimisation for later, not
part of the first design.

## Test plan

No test here needs a game build or a game run in CI. Tests invent their data.
Captures come from the owner's own runs and stay in `$TMPDIR`; they hold
game-derived data and never go into Git.

**Unit tests (`gfx`).**

- Scaled `SurfaceStore`: scissor, viewport and rectangle clears at 2x, 3x and
  4x, and at a per-axis scale, checked by pixel.
- Split scissor pieces stay disjoint and gapless at fractional scales: an
  additive draw over the pieces touches every host pixel once.
- A point-sampled 1:1 full-screen copy is an exact replication at integer
  scales. The same copy with a half-texel shift shows the documented seam at
  odd scales, so the detector below is proven to work.
- The guest-size resolve of a flat colour is that colour; of a checkerboard at
  the scale's period it is the mean.
- The depth-to-colour pass gives the same bytes as the CPU decode at 1x.
- Stipple indexed by guest pixel.

**Device tests (`d3d`).**

- The call list reaching the reference is identical with and without the
  wrapper (`RecordingBackend`).
- Guest memory after a synthetic frame with a write-back, a pixel count and a
  depth alias is byte-identical at scale 1 and scale 4 (in the style of
  `tests/rtt_direct.rs` and `tests/round7.rs`).
- A view that fails a draw or an allocation leaves the device unlatched and
  the guest bytes unchanged.
- Mixed-scale sampling picks the resolve or the scaled surface by the rule.

**Offline replay of captures.** `H2_BOOT_CAPTURE` already records what a
backend needs to redraw one frame: textures, surfaces, guest writes and each
`ResolvedDraw` with its streams ([BOOT.md](BOOT.md)). No replayer was found on
master, so one is part of this work: read a capture, feed it to a backend at
1x and at N, and compare.

- Box-filter the scaled frame down and diff it against the 1x frame.
  Differences should sit on geometry edges. A difference covering an area
  means a pass is misaligned or a target has the wrong policy.
- Seam detector: rows or columns that differ from both neighbours along the
  whole target, and the first and last row and column.
- Bisect with per-draw snapshots (`H2_BOOT_CAPTURE_SNAP`): the first draw after
  which the downsampled view leaves the reference names the pass.
- Capture set: the main menu (depth fog, blue tint), the first level's intro
  (crossfade, shadow map, 512x512 and 128x128 targets), a frame with the sun
  and its flare in view, and split screen once it runs.

**Live check, by the owner.** One run with an input timeline at scale 1 and
scale N, comparing the guest digest. This is the only step that runs the game,
and it is an acceptance check, not a development loop.

## Phases

| Phase | Work | Depends on |
|---|---|---|
| 0 | Capture replayer and the diff tools. Count, in a real run, which write-backs and counts happen per frame and at which guest call sites. | Nothing |
| 1 | `ScaledBackend` with integer scales, frame buffers and depth only. Snapshots scale. Depth aliases fall back to the reference's 1x texture. | 0 |
| 2 | Guest-size resolves and mixed-scale sampling. Back-buffer-sized render targets scale. GPU depth-to-colour pass; depth aliases scale. | 1 |
| 3 | Per-axis scale for anamorphic widescreen. Presenter resample to the display. | 2 |
| 4 | Optional extras, each its own flag: anisotropy override, shadow-map scale, MSAA, fractional scales, lazy reference. | 3 |
| Later | Single scaled backend with downsampled readbacks, for hosts that cannot run two. Guest-visible values differ from 1x, so it is excluded from deterministic replays and labelled as such. | Phase 0's counts |

Shadow-map scale needs two things first: sampling a depth level in place
instead of through guest memory (the view can do this without touching the
reference), and the comparison indexing host texels. A sharper shadow map
also narrows the 2x2 percentage-closer filter on screen, so shadow edges get
harder. Whether that is wanted is a taste question; xemu issue 1996 shows
shadows are where scaling artefacts get reported.

Rough effort, as guesses: phase 0 one week, phase 1 two, phase 2 two to
three, phase 3 one, each item of phase 4 one to two.

## Prior art

Read for behaviour and cited only. No code or data was copied. xemu is GPL.
Read 2026-11-15. The xemu source facts are from a summary of the files on its
master branch, not a line-by-line reading, and the Halo-2-HD blog was read
through a mirror because the site refused direct fetches.

### xemu

xemu has one integer `surface_scale_factor` for the whole NV2A
(`hw/xbox/nv2a/pgraph/pgraph.h`, with the work in `pgraph/gl/surface.c`,
`draw.c`, `texture.c` and `glsl/vsh.c`, and the Vulkan backend alongside):
<https://github.com/xemu-project/xemu>.

- Colour and zeta surfaces are allocated at the guest size times the factor.
  The guest's antialiasing mode multiplies the size first.
- Viewport, scissor and clear rectangles are multiplied by the factor. Line
  width is set to the factor, and point size is multiplied in the generated
  vertex shader.
- A texture whose address matches a surface is rendered from the scaled
  surface on the GPU. Zeta surfaces take a slower CPU path.
- Download to guest RAM reads the scaled surface and keeps one pixel per
  block. Upload replicates pixels. Changing the scale at run time downloads
  dirty surfaces and rebuilds them all.
- Anisotropy follows the guest's register and does not interact with the
  scale.

Its known problems above 1x, by issue:

- [1771](https://github.com/xemu-project/xemu/issues/1771): a post-process
  quad with a half-pixel offset samples the neighbouring texel.
- [57](https://github.com/xemu-project/xemu/issues/57) and
  [508](https://github.com/xemu-project/xemu/issues/508): artefacts along the
  screen margins. The 1x case was fixed by 4-bit coordinate rounding
  ([PR 2183](https://github.com/xemu-project/xemu/pull/2183)), which our
  fixup already does.
- [PR 3065](https://github.com/xemu-project/xemu/pull/3065) (open): corrects
  the subpixel offset when upscaling. Its author lists what it cannot fix:
  low-resolution intermediate targets, copies through the blit path
  ([417](https://github.com/xemu-project/xemu/issues/417)) and history buffers
  left unscaled ([993](https://github.com/xemu-project/xemu/issues/993)).
- [2728](https://github.com/xemu-project/xemu/issues/2728),
  [2747](https://github.com/xemu-project/xemu/issues/2747) and
  [1926](https://github.com/xemu-project/xemu/issues/1926): bloom.
- [1996](https://github.com/xemu-project/xemu/issues/1996): shadows flicker
  and do not scale.
- [1054](https://github.com/xemu-project/xemu/issues/1054) and
  [3119](https://github.com/xemu-project/xemu/issues/3119): clip rectangles
  and a download assertion at scale.
- [PR 2984](https://github.com/xemu-project/xemu/pull/2984): surfaces read
  back as vertex data must stay at 1x.
- [865](https://github.com/xemu-project/xemu/issues/865) and
  [602](https://github.com/xemu-project/xemu/issues/602): Halo: Combat Evolved
  water and texture seams.

What we take from it: the surface policy (scale colour and depth together,
leave guest sizes alone), and the list of failure classes as a test list.
What we do differently: readbacks come from a 1x reference, not from a
decimated scaled surface, and intermediate targets are classified, not scaled
uniformly.

### Halo-2-HD

<https://github.com/grimdoomer/Halo-2-HD>, with the author's write-up at
<https://icode4.coffee/?p=738>. It patches the retail game to render at
720x480, 720p and 1080i on Xbox hardware. The higher modes need a 128 MB RAM
upgrade.

What it changed, as its README and write-up describe:

- The stock game always renders 640x480; its 480p mode is an upscale and its
  widescreen is anamorphic. The patch raises the real back-buffer size.
- The textures the game builds over the back buffers and over the depth
  buffer are hard-coded to 640x480, and so is the allocation of the texaccum
  target. Each had to be patched. Left alone, they gave repeating lines and
  cut-off water on the menu. A size check had to change before the menu's
  blue filter came back.
- Of about 25 render targets, only texaccum was resized. The write-up does not
  say the bloom, blur and shadow targets were touched, and they appear to
  have stayed at stock size. That last point is our inference.
- The swap chain moved from two buffers to three, with a hook that re-points
  the game's back-buffer textures at each swap.
- Frame buffers moved into tiled memory with matching pitch; a wrong pitch
  gave striping. Caches were enlarged, and the kernel's allocator limit and
  the GPU clock were patched.
- Field of view, split-screen layout and fog options were added.

What it learned:

- **Performance on the Xbox is GPU-bound at 720p**: about 19 fps stock,
  rising to 27-28 with triple buffering, tiled texaccum and the overclock.
  1080i is single digits. Fill is the cost, as it will be for us once the CPU
  side is fixed.
- **Known issues** in its README: the HUD shrinks at higher resolutions;
  loading, sign-in and lobby screens stay 640x480; fog scrolls and bleeds
  between split-screen views.

Those issues come from changing a size the game can see. We do not change
it, so the HUD and menus keep their layout. The useful lessons for us are the
ones about the renderer's structure:

- the back-buffer and depth aliases must track the buffer exactly, or lines
  and cut-off water appear, which matches our "frame buffers as textures"
  path;
- one intermediate target follows the back-buffer size and must scale with
  it;
- the game looks right with every other target at stock size, which supports
  leaving the square targets alone at first.

## Open questions

- Which write-backs and counts happen in gameplay, and does any value reach
  the simulation? Phase 0 counts them; `0x24970` needs tracing.
- Is there a back-buffer-sized render target in our captures, as Halo-2-HD
  describes, and does any pass sample it with a texel-exact offset?
- Does any retail pass point-sample a half-texel-shifted source? If so, odd
  scales need a per-pass fix or must be refused.
- Does Halo 2 draw lines outside debug paths?
- How large is the second backend's CPU cost on current master, with merged
  passes?
- Whether the view should snap vertices to the host's subpixel grid.
