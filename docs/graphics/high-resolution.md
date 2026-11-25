# High resolution: 4K and arbitrary internal resolution

Status: Built. The renderer supports independently sized horizontal and vertical raster grids.
Code: `crates/platform/gfx/src/raster.rs`, `crates/platform/d3d/src/scaled.rs`,
`crates/platform/d3d/src/scaled/single.rs`, `crates/extensions/game/src/d3d_glue.rs`.
The startup D3D log shows the configured raster and drawable. The window's
performance overlay shows the latest presented raster and current drawable.
Tests: `crates/platform/d3d/tests/render_scale.rs`, `crates/platform/gfx/src/raster.rs`,
`crates/extensions/game/src/render_aspect.rs` and `crates/extensions/game/src/lod_distance.rs`.
Switch: Resolution, HUD, LOD distance, single-copy readbacks and glyph smoothing
are listed in [switches.md](../runtime/switches.md#settings).

The play window opens at the primary monitor’s aspect within 1280 × 960 logical
points. Native uses the physical viewport, including Retina backing pixels,
at the full window size. In fullscreen it scales both dimensions together
if the viewport exceeds the 3840 × 2160 pixel budget. On a 5120x2880 Studio
Display the fullscreen Resolution row reads **Native (3840x2160)**.
Choosing **5120x2880** renders at full panel resolution.
Explicit sizes stay fixed on resize and support up to 8192 on each axis.
The desktop shell redraws the latest frame to the resized drawable
(`Shell::redraw`, `crates/apps/app/src/shell.rs`) while the game prepares
its next frame.

The guest still addresses a 640x480 frame. A scaled frame has a separate
horizontal and vertical ratio. The physical rectangle in which the shell
presents that frame supplies the display aspect, including when the user
forces an anamorphic aspect with `--aspect`. See the
[Video screen](../game/settings-menu.md#video-screen) and
[launch overrides](../runtime/switches.md#settings).

## Summary

- **Guest device coordinates.** The device (`d3d::Device`) keeps
  every size, viewport, scissor, header and return value in guest pixels.
  Raster scaling lives below the `d3d::Backend` trait. In offline play the
  guest walk always selects retail geometry whatever the LOD setting; the
  display draws the chosen selection through the substituting render
  paths. See
  [projected size and level of detail](occlusion-culling.md#projected-size-and-level-of-detail).
- **One draw per target in single-copy mode.** At increased resolution
  the display encoder owns the draw stream, including draws to guest-size intermediates. Guest reads sample the
  scaled image at guest pixel positions.
- **Scale with the back buffer:** the frame buffers, the depth buffer, the
  textures the game builds over both, and any render target the size of the
  back buffer. **Leave at guest size:** the small square targets (bloom,
  blur, reflection and shadow buffers).
- **One ratio per axis.** A 3840x2160 frame uses 6 host pixels per guest
  pixel horizontally and 4.5 vertically. Scissors, viewports and clears
  share rounded edges. Fixed-size blur targets retain their texel spacing.
- **Shadow maps** scale only with E3 lighting: the view keeps a scaled copy
  of each depth level and samples it with one tap ([e3-lighting-setting.md](e3-lighting-setting.md)).
- **MSAA and forced anisotropy** are not built; both would be view-only.

## Widescreen projection

For a vertical field of view `v` and a presented viewport aspect `a`,
the horizontal field of view is `2 * atan(tan(v / 2) * a)`.
The Xbox projection builder (`0x0002fb70`) applies this rule to its
640x480 viewport and conditionally multiplies its aspect by the retail
widescreen factor at `0x0045dfdc`. A 16:9 image can therefore occupy
the same 640x480 allocation as a 4:3 image.

`crates/extensions/game/src/render_aspect.rs` reads the Xbox anamorphic
state at both the game and alternate view entries. In exact mode, for a recognised
perspective draw, the host scales the
horizontal clip coordinate by the ratio of the guest's pixel aspect to
the displayed pixel aspect. For translated shaders, the recogniser
follows the X, Y and W rows through partial position writes. It accepts
a four-row perspective transform or a far-plane surface that writes W to Z. Programs with an unrecognised
clip transform keep their original positions. With a retail camera the flare pass translates its
occlusion query to the projected display column, preserving the footprint used
for the expected count, and applies the lens to the reflection billboards and final composite fans.
Light volumes and light trails use the same CPU projector. Both billboard
emitters take the display transform about the centre of the player's viewport.
The depth billboard supplies its guest window position twice. The vertex
translator applies the same transform to its depth lookup in register 1
and to its raster position. Its sprite UVs keep their authored range.
On a perspective draw that samples scene colour or depth with a bump matrix,
both coefficients contributing to horizontal displacement take the display
lens, including when the guest camera already contains the correction. A frame-sized render level such as texaccum
is identified by its backing address, format and dimensions. Asset-space bump matrices keep their material values.
The exact reference draws both at the retail coordinates. The display keeps the
vertical field of view and depth of the simulation camera. Orthographic
menu widgets use a uniform fit inside each player's viewport. A menu quad
is background art when, along the fitted axis, it reaches both edges of the
viewport, or it reaches one edge and its whole bitmap at its drawn scale is
at least as long as the viewport. The second case covers the main menu's
scrolling strips: 1024-texel bitmaps drawn as two pieces that meet where
the texture wraps. A panel or tab at the edge has a shorter bitmap and keeps
the fit. Only the UI rectangle site gives the bitmap's texture coordinates:
an emblem draws a unit quad through the shader quad site, placed by its
transform, so the shader and screen quad sites use the first case alone.
Background art keeps the full viewport, as on a widescreen Xbox. The choice
is made before the native or guest draw at the shader quad
(`HUD_QUAD_READY`), screen quad (`UI_SCREEN_QUAD_READY`) and UI rectangle
sites. The screen and rectangle shaders use raster-local coordinates, so
their art tests use the width and height of their raster rectangle. The UI
rectangle (`0x00052040`) is read at the dynamic upload helper's
`EFFECT_QUAD_READY` when it returns to `RECT_SUBMIT_RETURN`. The next quad or
widget (`UI_ITEM_CALL`) restores the fit.

The full-view fades in `UI_RENDER_LAYERS`, the start-screen transition,
and the window stack's dialog dim all call `UI_MENU_VIEW_FILL`. This helper
subtracts the supplied rectangle's origin. Its quad takes its width and
height from the same raster rectangle as the rectangle shader
(`0x00485648`). The fill uses the full view through submission and then
restores the widget fit. The current view can be a split-screen viewport.
`crates/extensions/game/src/render_aspect.rs` covers these paths with
`menu_view_fades_use_the_rectangle_their_shader_projects_over`,
`screen_art_uses_raster_local_bounds_in_the_right_hand_view` and
`quads_are_read_at_the_shader_screen_and_rect_sites`.
The dialog then blurs the screen under it with the screen convolution blur (`SCREEN_CONVOLUTION_BLUR`, called by the
modal blur at `0x00235756`), which copies whole 640x480 targets back and
forth inside the widget fit. The device draws any draw that samples a
render target the size of its own 640x480 target without the fit: a frame
copy covers the frame (`crates/platform/d3d/src/device.rs`, test
`a_frame_copy_in_a_fitted_ui_scope_keeps_the_whole_frame`). With the fit,
each copy would shrink the picture into the centred box and leave stale
target contents at the sides.

A screen-space quad whose positions reach both edges of the frame along
the axis the fit shrinks is a full-frame pass the same way: the screen
effects' hand-pushed strips (the cutscene tints, the title screen's
backdrop fill, the wide convolution's grid rows), the colour pass's quads
and any menu fill a submission site did not read. The device takes the
window position of each of the draw's unindexed vertices and drops the
widget fit for the draw (`crates/platform/d3d/src/device.rs`,
`spans_the_fitted_frame`). A vertex program's vertices go through the
program, so positions placed from constant rows, passed through, or
written as inline Begin/End words and `D3DDevice_End` strips (the title
screen's backdrop blur and tint) land where the GPU puts them;
pre-transformed vertices are read as given. The reading is
`crates/platform/d3d/src/screen_rect.rs`, the same one
[`render.frame_layout`](../runtime/agent-api.md#renderframe_layout)
reports as `window_xy` (tests
`full_frame_quads_span_the_display_in_a_fitted_ui_scope` and
`immediate_program_quads_over_the_frame_span_the_display_in_a_fitted_ui_scope`
in `crates/platform/d3d/tests/passes.rs`). Without this,
a full-window quad covered only the centred 4:3 box at a wide display
while the scene under it, drawn with the display lens, spanned the whole
raster — hard vertical edges at the box. A quad that covers the cinema
window (full width, letterboxed height) spans the display along the
fitted axis and keeps the fit on the other.

A projected-effect fit (the flare, light volume and billboard scopes in
`crates/extensions/game/src/render_aspect.rs`) keeps frame copies and
frame-edge strips, since a sprite can be bigger than the view. Inside
such a scope one draw kind drops it: four vertices whose window bounds
match the frame's edges to within a guest pixel on both axes, sampling
an image the device rendered (a render target, the depth buffer or a
frame snapshot). That is a screen pass, not a sprite: the title screen's
depth of field, blur composite and frame-snapshot passes draw while a
projected-effect fit is set and otherwise stay in the centred box
(`lies_over_the_frame` in `crates/platform/d3d/src/device.rs`; test
`a_screen_pass_in_a_projected_effect_scope_spans_the_display`). A depth
billboard, whose fit also moves its screen-position input, keeps the
fit. Corner HUD
groups can use the fitted rectangle or the viewport edges. If the game already
uses the Xbox widescreen HUD layout, its artwork receives only the remaining
aspect correction. At 16:9 the Pinned layout retains the retail widescreen fit.

Every session that draws in a window, single-copy or exact, fits the
horizontal projection terms in the guest's active and saved display
cameras at `VIEW_PROJECTION_TERMS`
(`crates/extensions/game/src/render_aspect.rs`) to the window's aspect,
view by view, so a resize or a fullscreen change takes effect at the
next view. Lockstep and input tapes do not register the fit, and a game
of more than one machine (System Link, Live, co-op, or a bot's phantom
machine; `render_rng::networked`) keeps the retail terms at the hook:
their peers simulate the same world through retail cameras.
[Determinism](#determinism) says why the fit leaves the simulation alone. The camera matrix, CPU
pixel scale and root portal bounds derive from these terms. Every wide view
starts with the display supplement's lens; the hook swaps in the guest lens
once it has fitted the view, and a view it does not fit keeps the
supplement described under exact mode below (`starting_lens`;
`render.scene`'s `scene_visibility_lens_*` counts the views by lens). Water and fog
geometry built with this camera, the sky and CPU-projected effects share
its horizontal lens. The BSP flood clips each open portal to its parent
window. Object spheres must intersect one of their cluster's window frusta.
The observer supplies the vertical field of view.

Exact mode keeps the guest gather and visibility readbacks at the retail
lens. The scene step walks the open portals with both the retail and display
cameras, including their off-centre projections. It excludes geometry
already admitted by the retail lens or submitted by the guest. Each rigid
BSP placement needs to intersect a portal window in a cluster that references
it. For layers 6 and 16, a display-only submission needs an empty guest
layer and one supported part in a cluster wholly inside one portal window.
The live material must be visible and have a single shader LOD. For fog,
the cluster's atmosphere and the planar gate must select the loaded
implementation.
The visible water surface uses the sorted transparent geometry handler on
layer 15. That handler needs the guest's sorted entry, so an exact-mode
cluster ID alone cannot supply a missing surface. Decorator placements
also come from the guest's per-view list. A layer-6 material count
describes the edge blend.
Layer 3 draws resident cluster parts and fully opaque rigid placements.
A display-only section is drawn whole or not at all. A cluster is kept
only when every visible material of it, over all its LODs, draws in no
layer but 3, 6 and 16 (`native_world::display_cluster_complete`,
`DISPLAY_LAYERS` in
`crates/rewrites/render-bridge/src/native_world/section.rs`), the layers
the supplement has a display draw for. A part that also draws in layer 1
(the accumulation a combined layer-3 shader samples), an overlay layer
(5, 7, 8, 20, 2, 17), a layer-15 pool surface or a sorted layer (18, 19,
21) leaves its whole cluster out, with its water edge and fog. A placement, or a cluster, is also left out when any of its parts
is not loaded or samples the accumulation target
(`GpuWorld::display_complete_parts` in
`crates/rewrites/native-render/src/gpu.rs`). `render.scene`'s
`scene_visibility_display_incomplete` counts the clusters left out.
For objects, it builds the model sections and node palettes on a private
copy of the record writes. Materials without native layer-3 support, fades,
attachments and objects that require guest animation code can still leave
gaps at the sides. The guest’s flare visibility count comes from its own
reference depth and query. To locate a missing
BSP cluster, compare its ID in the guest visibility list, the structure
list and the layer-3 list for the same view. The sealed scene includes
all three and its camera. The display frame recorder includes the final
scissor and projection factor.
[FrameScene reads](../runtime/agent-api.md#framescene-checks-compare-capture-scene-parity-hashes)
and [display raster](../runtime/agent-api.md#renderframe_layout).

First-person draws use the weapon camera and the section, parts and node
palette selected by the guest's first-person records. On the native layer-3
path the material's fog and specular state resolve over the device state at
the draw boundary (`crates/extensions/game/src/native_first_person_glue.rs`).

The playerless Theater observer has the whole BSP and object array. It
uses the same Hor+ lens for projection and its own frustum test on both.
See [the host film view](../game/film-format.md#choosing-a-players-view).

**Highest** requests the finest model tiers at any resolution; the
structure walks keep their retail projected sizes and pixel radii. To
compare raster cost with retail geometry choices, record both settings
with the timings.
[Lab benchmarks](../runtime/lab.md#--serve-and-bench).

## Single-copy readbacks

The policy is selected at launch (`d3d_glue::single_copy`,
`graphics.single_copy` in `crates/extensions/game/src/setting_defs.rs`). Set
`single_copy = false` under `[graphics]` to request a retail reference in
offline play. Boot requires the reference for network and film sessions, input
tapes and lockstep. Lighting A/B captures also use a separate reference.


The frame buffers, frame-sized RTT levels and their depth attachment are
authoritative at display resolution. An RTT bind shares the actual texture.
Feedback and previous-frame snapshots copy it on the GPU so that later
writes cannot change the snapshot. The guest dimensions in `TextureInfo`
still normalize texel coordinates. The GPU decoder for 32-bit depth aliases
maps guest word addresses (pitch, offset or swizzle) into the scaled depth
and stencil block. Other alias layouts and headers over a colour surface
drawn into the depth allocation decode on the GPU as well. The decoder
addresses guest bytes for each mip, cube face or volume slice. For
reinterpreted cube and volume textures, a uniform integer grid preserves
the layout. At fractional or unequal scales these aliases decode at guest
size so cube faces stay square and mip chains remain valid. Guest-size intermediate
draws sample these same images.

A CPU surface read, lock or writeback transfers one sample per guest pixel
into a guest-size GPU surface, then uses the existing guest format, pitch
and swizzle encoder. Colour, depth and stencil use the same per-axis
mapping. On reduction a read selects the first host texel at the rounded guest-pixel edge. On
enlargement a host texel belongs to the guest cell bounded by those edges.
Visibility counts use the same selection in the GPU coverage reduction, so a covered guest pixel contributes once at 4x.
Mirror stores encode directly from the scaled image into guest-layout words.

Fixed-size intermediates retain their guest dimensions. When a draw pairs
a guest-size colour target with scaled depth, both attachments must use
the same raster grid. The renderer draws the pair once at guest size and
transfers its modified scaled partner back on the GPU. Changing the render
scale transfers the last image to the new allocation. These transfers do
not require a CPU mapping.

**Parity trade-off.** Coverage, interpolated values and the outcome of
depth and stencil tests can differ at scaled sample positions. A value
written to guest memory or returned by a visibility test can therefore
differ from a separate 1x render. The scale report (`ScaleStats`) includes
reference and display draw counts, downsamples and completed CPU reads.
Tests: `single_copy_records_once_and_only_resolves_when_the_cpu_reads`,
`single_copy_depth_and_stencil_read_at_guest_resolution`,
`single_copy_visibility_counts_guest_pixels` and the other `single_copy_*`
tests in `render_scale.rs`.


## Terms

- **Guest pixels**: the coordinates the game uses. The back buffer is 640x480
  ([d3d.md](d3d.md), "Guest structures").
- **Host pixels**: texels of the wgpu surface that stands for a guest surface.
- **Scale** `(sx, sy)`: host pixels per guest pixel, per axis. Halo 2's
  widescreen is anamorphic 640x480 shown at 16:9 ([app.md](../runtime/app.md),
  "Scaling"), so a 16:9 output wants `sx` larger than `sy`.
- **Reference** and **view**: the two backends of exact mode (below). In
  single-copy mode the view is the display encoder and the reference keeps
  the guest-size state.


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

Both write-backs and counts feed guest state. The push-buffer callback
at `0x24970` reads target texels through `0x3fdbc0` and `0x24a70`.
The flare reader at `0x207b0` turns a sample count into a smoothed
visibility byte. See [determinism](#determinism).


## Exact mode: reference and view

In exact mode, `ScaledBackend` wraps two `GfxBackend`s:

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
   consulted and never counts samples.
2. **Mutations** (`draw`, `clear_rects`, `write_surface`, `copy_surface`,
   `snapshot_surface`, `create_surface`, `create_texture_target`,
   `surface_texture`, `depth_alias_texture`): go to both. Handles are pairs.
3. **`upload_texture`**: decoded once and shared by the two backends, which
   share one wgpu device.
4. **`present`**: the view presents. The reference still rotates its swap
   ring, because textures over the display surface read the previous frame.

What scales in the view: surfaces made with `create_surface` at the frame
size (the first colour surface's, the back buffer), `PersistDisplay`'s copy,
back-buffer-sized render-to-texture levels
(`ScaleView::create_scaled_texture_target`) and depth aliases of the whole
depth buffer (`ScaleStats::scaled_depth_aliases`). Smaller render targets
keep their guest dimensions. Exact mode gives each camera its own contents,
including the downsample and blur levels used by the final composite.
Scissors and clears are scaled inside `GfxBackend` for a scaled surface.

A view failure (a draw it cannot make, an allocation that fails at 4K) does
not stop or alter the guest. It is counted (`ScaleStats::view_errors`) and
that frame is shown from the reference (`present_reference`). A draw into a
scaled target that the view cannot make, or makes with another outcome than
the reference's, replaces the view's target with the reference's, each guest
pixel repeated (`color_resyncs`, `depth_resyncs`); each kind of repair is
logged once. A draw that writes depth into a guest-size target with the
scaled depth buffer bound is drawn again into the view's depth through a
scaled colour surface of the depth's size (`proxy_depth_draws`). Guest bytes
written into a scaled depth surface reach the view through `fill_from`.

This differs from the render thread, where a disagreement stops the run,
because there the disagreeing backend is the one the guest observes.
Headless runs (`H2_BOOT_GFX` with `H2_RENDER_SCALE`) write `frame-NNNNN.png`
from the view and `frame-NNNNN-1x.png` from the reference
(`crates/extensions/game/src/d3d_glue.rs`, [BOOT.md](../runtime/BOOT.md)).

### Changing the scale during a run

At each present, `ScaledBackend::follow_resolution` samples the latest
Native target or explicit setting for the next frame
(`crates/extensions/game/src/d3d_glue/output_size.rs`). The window redraws
the latest published raster at each resize. For Source aspect and Fit
scaling, Native takes its presentation aspect from the current physical
drawable, including while it draws a cached frame from an earlier size.
The compositor and the guest's display-size query share this placement
rule (`crates/apps/app/src/scale.rs`).

The backend applies a new scale at the frame boundary on the thread that
presents (the render thread under `ThreadedBackend`). D3D logs each accepted
raster change, including a fullscreen transition. It recreates each
scaled surface at the new size, fills it from the reference, and maps the
device's surface handles onto the new allocations. At 640×480 with retail
lighting and 4:3 presentation, the view sleeps and the compositor shows the reference
(`ScaleStats::reference_frames`). A forced widescreen aspect or stretched
window wakes the exact-mode view at the same raster size. The display gets
the fitted HUD and adjusted projection while guest reads use the retail
reference. Single-copy draws once into its guest-size allocation for this
case. E3 lighting also keeps the view awake (shadow maps at 2x). Tests:
`the_scale_changes_at_a_frame_boundary_with_no_fallback_frames` and
`on_the_render_thread_the_change_is_made_there_and_shown_at_once` in
`render_scale.rs`.

## Which surfaces scale

| Surface | Policy | Why |
|---|---|---|
| Back buffer, display surface, third buffer | Scale | The image. |
| Depth buffer | Scale, always with the frame buffers | Attachments must match. The existing depth-larger-than-colour stand-in (`prepare_target`) scales both sides. |
| Textures over the frame buffers (`0x509354`, `0x509358`; `snapshot_surface`) | Scale: the snapshot is a copy of the scaled buffer | Current-frame feedback and the previous-frame crossfade sample them in screen space. |
| Textures and the colour surface over the depth buffer (`0x509368`, `0x50936C`, `0x509370`) | Scale: built from the view's depth buffer | The menu's fog and distortion pass reads depth per pixel. Made on the GPU (see "Depth as a texture"). |
| A render-to-texture level the size of the back buffer | Scale | Halo-2-HD reports one intermediate target, texaccum, that follows the back-buffer size (see "Prior art"). A full-screen intermediate left at 640x480 would undo the whole feature. |
| Fixed-size square targets (the `0x14850` 64x64 pair, the 128x128 and 512x512 targets seen in the first level) | Do not scale | Bloom and blur kernels are measured in texels. Scaling the target shrinks the blur on screen. |
| Shadow maps (depth levels bound as `pNewZStencil`, sampled as `LIN_D24S8`) | 1x with retail lighting. With E3 lighting the view keeps a copy at the rounded vertical scale, at least 2x (`d3d::scaled::shadow_scale`), sampled with one tap ([e3-lighting-setting.md](e3-lighting-setting.md)) | The comparison indexes the host texels it has (`textureDimensions`) at the guest's coordinate, so a bigger map needs no shader change; a sharper map with retail's 2x2 filter alone only narrows the soft edge. |
| Ordinary textures | Never | They are the game's assets. |

The rule for classifying a render-to-texture level is its guest size against
the back buffer's guest size, checked when the host surface is created. An
allow-list by render-target index (`0x14bc0` selects targets by index) is the
fallback if a level needs an exception.

### Mixed-scale sampling

A draw can sample a scaled surface into an unscaled target; the first
downsample of a bloom chain does this. In single-copy mode a mixed draw runs
once at guest size: `ScaledBackend::guest_image` samples the scaled
partner to its guest-size handle (`ScaleStats::downsamples`), and a scaled
partner that the draw modified is transferred back on the GPU
(`crates/platform/d3d/src/scaled/single.rs`, `draw_single`). In exact mode the
reference and display each draw the chain from their own frame. Both
downsample to the same guest dimensions. The display then composites its
own blurred image, keeping highlights aligned with their sources at each
raster resolution. The guest texture header supplies the texel dimensions
for UV normalization. The compositor supplies the final drawable extent
after the frame is complete.

### Depth as a texture

Each camera decodes textures over its own depth or colour attachment.
The texture header supplies the guest byte offset, dimensions and pitch
or swizzle. The decoder maps each addressed guest texel to the source
attachment's physical grid. This includes subrectangles and texture
headers that reinterpret texaccum or the colour surface drawn over the
depth allocation. The GPU paths are `gfx::guest_words::GuestWords` for
aligned depth words and `gfx::surface_alias::AliasDecoder` for the other
layouts. `crates/platform/d3d/tests/gpu_depth_alias.rs` checks a partial
feedback alias against the two cameras at full and three-quarter raster.

Those textures hold packed bits. They must be point sampled, at any scale.
A stage that samples one with a linear filter is already wrong at 1x.


## Coordinates

### Positions

Translated vertex programs output guest window coordinates, and the fixup maps
them to clip space with the guest target size (`VertexUniforms::surface`,
[shaders.md](shaders.md), "Position fixup"). Nothing in that mapping depends
on the host size. The built-in pre-transformed shader inverts the guest
viewport the same way. So geometry needs no change: `surface` stays the guest
size and the rasteriser does the rest.

Three details:

- **Subpixel truncation.** At 1x the fixup truncates x and y to 1/16 of a
  pixel, as the NV2A does. Scaled, it does not truncate at all: on the
  guest's grid, seams between draws opened into see-through lines (see
  "Subpixel snapping and cracks" below).
- **Slope-scaled depth bias.** The W-buffer path computes the polygon offset
  from screen-space derivatives (`max(|dpdx|, |dpdy|)`). Derivatives per host
  pixel are smaller by the scale, so the slope term shrinks. That is the
  correct behaviour for a bias per sample step, but decals tuned at 480p need
  a test.
- **Stipple.** The translated fragment shader maps each host pixel to the
  guest framebuffer cell bounded by the same rounded edges as a scissor.
  It uses the guest and host dimensions of the bound target on each axis.
  A host pixel centre exactly on a guest edge takes the preceding cell's
  mask bit. The mask stays anchored to the framebuffer's top left when
  the viewport or scissor moves. See `SetStipple` in [d3d.md](d3d.md)
  for the row and bit order.

### Subpixel snapping and cracks

Reported on Burial Mounds at about 4K: a thin see-through line along a seam
in the terrain, over the clear colour, not seen at 640x480.

**The geometry is closed.** `bsp_cracks`, a tool since removed from `viewer`, welded a BSP's drawn cluster triangles by their exact
position bits and classified every edge one triangle uses. Burial Mounds
(9,666 triangles, 27 clusters): no T-junction and no near-coincident
unwelded edge at a 0.001 or 0.05 world-unit tolerance; 58 open edges, all
the level's border. 1,964 shared edges lie between triangles drawn by
different draws (1,083 between clusters, 882 between parts with other
materials), every one with bit-identical ends. Zanzibar and Ascension are
the same: no T-junctions, no unwelded seams.

**So the crack is the projection, not the mesh.** Two draws transform the
same world vertex and land a few ulps apart in window coordinates. The
likely route, not yet confirmed in a capture: another material's template
has another vertex program, or the host compiler contracts the same
`dp4`s differently in two pipelines. The NV2A, and
the fixup at 1x, truncate x and y to 1/16 of a
guest pixel. Where the exact value sits just under a grid line, a few ulps
move one copy a whole step: the two sides of the seam then disagree by
1/16 of a guest pixel along the whole edge. At 1x that sliver covers about
one pixel centre in sixteen along it (retail's occasional sparkle). At a
render scale `s` it is `s/16` of a host pixel wide, a quarter at 4x and
three eighths at 6x, so it covers that share of the host pixels along the
seam: a dashed line. A T-junction on the guest's grid behaves the same,
the T vertex rounded off the long edge by up to 1/16 of a guest pixel.

**Fix: the view does not truncate.** Scaled (`VertexUniforms::shift`
above 0), `xvs_fixup` passes x and y through unrounded, and the host
rasterizer's own sub-pixel precision (Metal and D3D11-class hardware: at
least 8 bits, 1/256 of a host pixel) decides coverage. Two copies a few
ulps apart then disagree by about 1e-5 guest pixels, which covers no
sample in practice. The built-in pre-transformed shader never truncated.
At 1x the expression is unchanged (the `select` takes the truncated
value), so the reference, and everything the guest can observe, is
identical. The shift's tie-free sampling holds without the grid: host
samples sit `(k + 1/4) / s` from a guest pixel centre, so an edge a few
ulps off a centre covers the same samples as one on it.

Snapping to 1/16 of a host pixel instead (the guest's grid times the
scale) only narrows the sliver. Measured by
`crates/platform/d3d/tests/seams_gpu.rs` (a 64x64 frame, a strip meeting a long
edge at 47 T-junction vertices, and a seam whose two draws hold an end one
ulp apart), clear-colour pixels inside the mesh:

| Fixup | 2x | 3x | 4x | 6x |
|---|---|---|---|---|
| guest grid | 8 + 0 | 14 + 22 | 29 + 0 | 56 + 0 |
| 1/16 host pixel | 3 + 0 | 6 + 0 | 5 + 0 | 8 + 0 |
| none (used) | 0 | 0 | 0 | 0 |

(T-junctions + ulp seam.) At 1x the image is, pixel for pixel, the mesh's
with its positions truncated on the CPU first, before and after.

Not chosen: MSAA (narrows a sliver, does not close it, and costs every
pipeline, see "Filtering, anisotropy and MSAA"), a conservative edge
expansion (fattens every edge, alpha-blended ones too), and T-junction
repair on the D3D path (there are none in the drawn world, and the guest's
index buffers cannot be changed). A renderer that owns the world geometry
(the native renderer) loads the same welded mesh; it should draw shared
edges from one vertex buffer with one position transform for every
material, and not snap, so seams match bit for bit.

### Scissor, clears and viewport

`Draw::scissor`, `ScissorRect` clears and `Draw::viewport` are in pixels of the
target. The view maps their endpoints on each axis. For an integer scale
this is exact. At a fractional scale it rounds each edge with the same
function (`floor(edge * s + 0.5)`), so the disjoint pieces of a split scissor
stay disjoint and gapless ([d3d.md](d3d.md), `clip_rects`). xemu had this
wrong for inclusive bounds at one point (xemu issue 1054).

### Half-pixel and texel offsets

The game and D3D put half-pixel offsets in the viewport offset constant, and
the pre-transformed path adds its own (`FF_HALF_PIXEL`). These are lengths in
guest pixels. Scaled, a 0.5 guest-pixel offset is `0.5 * s` host pixels.
Unshifted, they move geometry half a guest pixel off the scaled scissors and
clears, and the screen's first row and column stay empty. The view shifts
positions to fix it (see "Half-pixel shift" below).

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
- **Fractional scales** cannot represent every guest-texel offset as a
  whole host texel. A translated point tap can select an adjacent sample.
  Feedback snapshots on equal-size surfaces copy exact host texels. Explicit
  guest readbacks, fills and mixed-size surface transfers use a separate
  nearest-cell mapping for each axis. Small filter targets operate on their
  guest raster. Arbitrarily shifted point taps in retail draws still need
  a live comparison.

Three rules follow:

1. Texture-size uniforms take the guest size: `tex_scale` for texel-addressed
   (`LIN_`) textures, and the stage dimensions the engine writes into vertex
   constants ([shader-instances.md](../formats/shader-instances.md), "Engine inputs";
   those come from guest slots and need nothing).
2. Code that indexes texels must use the host texture's real dimensions for
   the index and the guest size for the coordinate. The shadow comparison
   (`floor(uv * size)`, [shaders.md](shaders.md), "Shadow maps") is the one
   known case and matters for the scaled shadow maps of E3 lighting.
3. Passes whose source and target are both unscaled are untouched. This is
   the main reason to leave the blur chain at guest size.


### Half-pixel shift

On each magnified axis at scale `s`, positions move by `(2s - 1) / (4s)` guest pixels
(`gfx::renderer::window_shift`, `VertexUniforms::shift`, the built-in
shader's `shift_x/y`), with no 1/16 truncation (see "Subpixel snapping and
cracks"). The shift is zero on an axis at or below 1x. The reference retains retail snapping.

D3D-convention geometry puts edges, and the texel corners of a 1:1 copy,
exactly on pixel centres, so at 1x the tie rules decide. Scaled without a
shift, a full-screen quad leaves the first host column and row empty (the
margin artefact of xemu issues 57 and 508), and a point-sampled 1:1 copy
reads the neighbouring texel. At uniform integer scales, the shifted `s x s` host pixels of a
guest pixel sample it at offsets `(k + 1/4) / s`, all inside the pixel and
none on a tie. Then an edge on a pixel centre covers whole blocks, the
top-left rule included, and a 1:1 copy reads the texel 1x reads. A shift of
exactly `(s - 1) / (2s)` puts the first sample on the tie and fails at 3x
through rounding; the extra quarter is what makes it robust.

The shift moves everything up and left by `(2s - 1) / 4` host pixels, so
geometry that ends on the target's far edge instead of a pixel centre (the
D3D 1:1 texel mapping, a quad from -0.5 to the size less 0.5, which Halo's
menus use to composite their blurred level) would stop short of the last host
columns. A scaled draw therefore holds any position past the last pixel
centre at least on the far edge (`xvs_fixup`, the built-in shader). At 1x
those positions all cover the last pixel exactly as the edge does. Test:
`a_full_screen_pass_reaches_the_far_edges_at_every_scale`.

## Post-processing

What we know about the retail post-processing chain is thin. Our docs record
the targets it uses, not what each pass does: the two 64x64 targets built at
`0x14850`, 128x128 and 512x512 targets in the first level, the previous-frame
crossfade (`0x002ab270`) and the depth-based fog and distortion pass on the
menu. Halo-2-HD's write-up counts about 25 render targets in four or five
allocations, from 512x512 down to 64x64.

Fixed-size blur targets keep their guest texel grids. Frame-sized
intermediates and feedback copies use the display grid
(`crates/platform/d3d/src/scaled.rs`). A frame-sized copy bypasses the
widget fit in draw resolution. The motion sensor's wash, tint and
composite fans (`crates/rewrites/engine/src/ui_backdrop.rs`) draw the sensor
at its HUD placement.

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
- **HUD and UI.** Widget bounds are in a 640x480 frame ([ui.md](../formats/ui.md)).
  The host fits menu draws and their scissors uniformly in a centred safe
  area. Centered places HUD corners in the same area. Pinned translates
  the retail corner-anchored HUD groups to their own viewport edges while
  keeping their physical scale. Centre and world-tracked indicators use
  their own anchor. Each draw records the fit before it moves to the GPU
  thread. A fit change first drains the push buffer under the old fit
  (`Device::set_ui_fit`). When an undelivered guest callback stops that
  drain, the change is queued at the push-buffer address that was PUT
  when it was asked for, and the drain switches there once the callback
  has run. Commands written before the change keep the old fit and the
  draws after it take the new one (test
  `a_fit_change_held_back_by_a_callback_starts_after_the_commands_before_it`).
  A generated game image includes the interior UI entries for
  these scopes. Regenerate a pre-existing image to pick them up.
- **Loading screens and movies.** The compositor presents the supplied frame
  at the selected display aspect. Menu and HUD fits apply inside their
  respective draw scopes.

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

**Anisotropy.** The sampler is anisotropic only when the guest asks
(`gfx::translate`: an `Anisotropic` filter with `MAXANISOTROPY` above 1).
A view-only override (not built) may raise the clamp on a stage when all
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
- render-to-texture surfaces have no MSAA in `gfx`;
- the W-buffer path writes `frag_depth`, which is per pixel, not per sample;
- the sample count is part of the pipeline key, so every pipeline compiles
  again, at about 220 ms per Metal pipeline on a cold cache
  ([gameplay-budget.md](../perf/notes/gameplay-budget.md)).

Supersampling through the scale handles all of those uniformly. MSAA (not built) would be an
option for hosts that cannot afford the fill. It must never reach the
reference, where sample counts are guest-visible.

### Text

All text comes from one texture, the glyph cache ([fonts.md](../formats/fonts.md),
"Glyph cache texture"): glyphs rasterised at 640x480 sizes into a 256x256
`LIN_A4R4G4B4` texture, with 3-bit antialiased alpha, and drawn 1:1 with
screen-aligned quads. Magnified by the guest's filter that text is soft
(bilinear) or stair-stepped (point), whatever the scale.

The view redraws it from the same texels:

- **Detection** (`d3d::glyph`, on the guest side of each draw): a stage's
  texture is the glyph cache when it is 2D, `LIN_A4R4G4B4`, 256x256 with one
  level, and its header is the texture the game's own glyph-cache bitmap
  names (`[0x4B62AC] + 0x50`). The draw carries the stages as
  `ResolvedDraw::glyph_cache`.
- **Filter** (`shaders::psh::GLYPH_WGSL`): only where the target's scale is
  above 1 (`Nv2aShaders::prepare` sets `PixelOptions::glyph`), so the
  reference and a 1x run compile exactly the programs they did. The stage
  reads the 6x6 texels around the sample and rebuilds the edge from the
  alpha as coverage: a cubic B-spline of the alpha, thresholded at half the
  local stroke alpha (the same spline through each texel's brightest
  neighbour) with a ramp one host pixel wide (host pixels per texel from
  the derivatives). Straight edges keep their position, curves come out
  round, and thin strokes that never covered a whole texel keep their
  weight. Where a texel covers at most 1.25 host pixels the guest's own
  sample is used.
- Nothing is uploaded twice or stored: the upscale is computed per
  fragment from the texture the game already uploads, so a glyph the game
  adds to the cache is sharp on the next draw.
- Off by default (`d3d::glyph::smoothing_from`); `H2_SMOOTH_TEXT=1` (or
  `on`, `true`) turns it on; `Device::set_glyph_smoothing` does the same in
  tests.

Measured by `crates/platform/d3d/tests/glyph_text.rs` on a synthetic glyph cache (a
ring, a letter, a thick and a thin slanted stroke, a dot), drawn 1:1 on
the 64x64 corner of an 80x80 target (clear of the target's edge, where
scaled positions are clamped). Along the thick stroke's edge, the largest
distance between where alpha crosses one half and the true edge, and the
edge's width (rows between 0.1 and 0.9):

| Scale | Bilinear | Point | Glyph filter |
|---|---|---|---|
| 2x | 0.28 px, 3.0 rows | 0.88 px, 1.9 rows | 0.19 px, 1.6 rows |
| 3x | 0.49 px, 4.5 rows | 1.58 px, 2.8 rows | 0.28 px, 1.7 rows |
| 4x | 0.65 px, 6.0 rows | 2.22 px, 3.8 rows | 0.39 px, 1.6 rows |

The area drawn wrong against the true outline falls to between a quarter
(4x) and a half (2x) of either filter's. At 1x and in the reference at every
scale the image is byte-identical with the filter on and off.

Risks: another texture with the cache's shape at the header the game's
glyph-cache pointer names would be filtered as text; a different build of
the game with the pointer elsewhere gets no smoothing (nothing worse).
Text the game draws through a render-to-texture surface smaller than the
frame stays at 1x like the rest of that surface. The view's half-pixel
shift (above) moves the redrawn text by the same fraction of a pixel as
everything else.


## Determinism

Boot selects exact readbacks when it installs networking, arms a film recorder,
plays a film, or enables lockstep or an input tape.
The 640x480 reference supplies texels, depth, stencil and visibility counts
using the retail lens and HUD coordinates. The window presents its own
resolution, aspect and HUD layout. Boot selects this policy before the
first frame, including time in the menu before a player joins System Link,
Live or co-op. Changing a display cannot retroactively repair a pixel or
visibility count the guest has already consumed. The same reference applies
when the user requests single-copy mode.

Resolution is a Host setting. A film viewer chooses the display raster
independently of the recorded guest. The Video row itself writes its label
into guest memory. Local films record each size the row or picker observes so
replaying a trip through Settings shows the same choices on another monitor.
See [film playback](../game/film-format.md#choosing-a-players-view).
Every session keeps retail geometry selection on the guest and reference
path: the display choice changes only the drawn geometry, through the
native display passes and, in single-copy rendering, the D3D fallback's
draw-boundary swap (whose records and palettes are per-frame render
scratch). The native opaque object pass builds supported display
sections on a private overlay and submits them to the display attachments.
The overlay keeps the guest object pose, lighting cache, palette and
residency writes private. Marker lookup reads the live object's node
matrices, and film and lockstep checkpoints hash the live arena; both are
retail whatever the setting. Older
films with a scalar render scale and LOD selection reproduce that startup
multiplier on the reference. In offline single-copy play, guest readbacks
sample the display raster.
[Private model sections](../../crates/rewrites/render-bridge/src/native_scene_lists/setup.rs),
[marker lookup](../../crates/rewrites/engine/src/sim_native/marker.rs),
[arena hash](../../crates/extensions/lockstep/src/lib.rs).

The observer produces the vertical field of view from player control.
Aim search and shot assistance use weapon tag angles and zoom, and the
distributed shot path can use transmitted aim data. Resolution and HUD
position supply neither of those inputs. Headless sim and dedicated hosts
install no display-aspect hook. See [aim assist](../game/aim-assist.md) and
[field of view](../game/settings-menu.md#field-of-view).

In exact mode the original object gather stays authoritative. Per-visible-object
setup can reach animation variant selection through
`0x00133790`, `0x000bd610` and `0x001cb410`. With the animation manager's
gameplay RNG bit set, the selector writes the shared seed at `0x001dd6e1`.
The pose evaluation at `0x000bd7b2` computes the object's shared node
matrices and clears its dirty bit; marker lookup reads those matrices.
Changing which objects enter that loop can change a later simulation tick.
A guest camera fitted to the window keeps the simulation's inputs as a
retail camera leaves them:

- **Random numbers.** Every world frame of a one-machine game renders on
  a private random stream (`crates/extensions/game/src/render_rng.rs`):
  `RENDER_FRAME` swaps the shared seed (`[0x004e7408]`) for the render's
  own and puts the simulation's back when the frame ends. Boot registers
  it for every session that renders, except lockstep and input tapes, so
  a recording and its replay take it alike whatever window either had.
  Theater's camera enters the same stream; a frame is entered once.
- **Object setup.** The guest's setup of a main-view object outside the
  retail lens's gather takes the "not drawn" exit before any of it runs
  (`outside_retail_lens` in
  `crates/rewrites/render-bridge/src/native_scene_lists/setup.rs`): no
  section choice, pose evaluation, animation selection, palette or
  record. The retail set is the Rust visibility at the view record's
  retail camera (`scene_step::retail_object`), a function of the
  simulation's state and the view alone. The display supplement draws
  those objects from the mounted model on a private overlay
  (`DisplayVisibility::guest_lens`). Objects inside the set keep their
  retail section choice: the LOD pixel scale `0x004b9ed0` is the
  projection's `+0xbc` word, and the fit writes only the horizontal
  terms. `render.scene` counts the objects left undrawn
  (`scene_guest_lens_objects_skipped`) and the views whose retail set
  declined (`scene_guest_lens_retail_declined`); such a view's setups
  run as the widened gather lists them.
- **Readbacks.** The reference copy draws the guest's camera, so with the
  fit its pixel counts and write-backs see the widened view: the flare
  reader's visibility byte (`0x207b0`) and the push-buffer callback's
  texels (`0x24970`). Both feed only later draws.

What must hold in exact mode, and how each is checked:

| Requirement | Check |
|---|---|
| The device's code path does not depend on the scale | The scale is not visible above `Backend`. `the_guest_sees_the_same_answers_at_every_scale` compares a plain backend with the wrapper at 1x, 2x and 4x: the count, written-back bytes, guest memory and skipped draws. |
| The view never writes guest memory, the logical clock or the scheduler | It holds no guest handle, like the render thread. |
| The view never causes a guest-visible wait | Its work is not on the class-1 read path. Any wait is on wall time and charges no logical time ([render-thread.md](render-thread.md)). |
| View errors do not surface as backend errors | They are counted, and the frame falls back (`a_draw_the_view_cannot_repeat_shows_the_reference_not_a_hole`). |
| Screenshots used as goldens come from the reference | `H2_BOOT_GFX` PNGs and capture snapshots stay 1x unless asked. |

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
| 3840x2160 | 6 x 4.5 | 8.29 M | 27 | about 232 MB |
| 3440x1440 | 5.375 x 3 | 4.95 M | 16.125 | about 139 MB |
| 5120x2880 | 8 x 6 | 14.75 M | 48 | about 413 MB |

A frame-sized colour target adds four bytes per pixel. At 3840x2160 this
amounts to 33.2 MB per target. At 5120x2880 it is 59.0 MB.

- **GPU.** Fragment work on scaled targets grows with the pixel count. Work on
  unscaled targets does not. Snapshots are full copies of the back buffer
  (about 44 MB each at 3840x2880); on GPU-visible copies the menu's five a
  frame ([menu-budget.md](../perf/notes/menu-budget.md)) are the first thing to measure.
- **CPU.** Single-copy mode records each scaled draw once. Exact mode
  repeats per-draw work (uniforms, bind groups, recording, encoding,
  submission) on the second backend but not push-buffer decode, draw resolve,
  texture decode or shader translation. Timings are in
  [menu-budget.md](../perf/notes/menu-budget.md) and [render-thread.md](render-thread.md).
- **Readbacks.** None are added by the view; single-copy mode resolves CPU
  reads on the GPU at guest size.
- **Counts.** In exact mode the count pass (a second draw into an `R16Float`
  target against a scratch depth copy) runs only on the reference.

## Tests

No test needs a game build. Tests invent their data; captures from the
owner's runs stay in `$TMPDIR` and never go into Git. GPU tests skip when no
adapter exists.

- `crates/platform/d3d/tests/render_scale.rs` (headless wgpu): the guest sees the same
  answers at every scale; the view's frame box-filtered equals the reference
  except on geometry edges; only frame-sized surfaces scale; legacy scale overrides outside 1
  to 6 are rejected; frame-sized targets, small targets with the scaled depth
  buffer, depth-level fills, the depth buffer as a texture and colour over
  depth memory match the reference downsampled; the scale and the lighting
  change at a frame boundary; the stipple mask stays aligned in exact
  and single-copy mode; the E3 shadow-map and `single_copy_*` tests.
- `fractional_per_axis_surfaces_keep_fullscreen_edges_and_guest_answers`,
  `fractional_clears_and_scissors_tile_the_same_texels` and
  `fractional_feedback_preserves_patterned_guest_cells_and_snapshot_lifetime`
  cover unequal fractional axes, including a 149x101 surface for a 64x64
  guest frame.
- `crates/platform/d3d/tests/seams_gpu.rs`: clear-colour pixels inside a mesh with
  T-junctions and a one-ulp seam, at 2x to 6x ("Subpixel snapping and
  cracks").
- `crates/platform/d3d/tests/glyph_text.rs`: the glyph filter against a synthetic
  cache ("Text").
- `crates/platform/d3d/tests/gpu_depth_alias.rs`, `external_pass.rs`: depth aliases and
  external passes through `ScaledBackend`.

Whole-frame retail capture replay still lacks test coverage.

## Prior art

Read for behaviour and cited only. No code or data was copied. xemu is GPL.
The xemu source facts are from a summary of the files on its
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
We classify intermediate targets by guest size. In exact mode readbacks come
from a 1x reference. Single-copy mode samples the scaled surface on demand.

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


## Not verified

- Cross-machine and film tick-hash parity for the same input across window
  sizes and HUD settings. Exact mode isolates the display-dependent inputs;
  it does not establish cross-driver raster equivalence.
- Texaccum, refraction, active camouflage and water may derive projective
  texture coordinates independently of the clip transform. A synthetic shader
  covers a lookup derived from the corrected clip X and W. Compare a
  high-contrast surface at the same tick at 4:3 and ultrawide. Use the agent
  API's `render.frame` and `render.capture` to identify the producer of any
  displaced lookup and inspect its projected coordinates after division by W.
  A correction belongs on the display's screen-space lookup. Asset UVs and
  the retail reference keep their original mapping.
- Fractional and odd-scale point taps in retail water, fog and feedback
  draws. A half-texel-shifted point sample can select the adjacent texel.
- HUD corner ownership, projected markers and scissor alignment in split
  screen, cinematics, zoom and ultrawide combat. CPU-positioned effects
  can disagree with geometry at the sides of a wide view.
- Whether Halo 2 draws lines outside debug paths.
- The CPU cost of exact mode's second backend on current master.
- Whether the back-buffer-sized target that Halo-2-HD reports is the only
  full-screen intermediate in our captures.

## Related

[d3d.md](d3d.md), [render-thread.md](render-thread.md),
[e3-lighting.md](e3-lighting.md), [shaders.md](shaders.md),
[settings-menu.md](../game/settings-menu.md).
