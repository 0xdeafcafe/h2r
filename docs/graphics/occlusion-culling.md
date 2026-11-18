# Occlusion culling of objects

The game's render front-end is CPU-bound guest code: each draw costs
about 8 to 14 µs on the game thread before it reaches the GPU, and heavy
Old Mombasa frames have about 2,000 draws at about 36 ms. The GPU has
headroom. This page is the read of how the game picks what to draw each
frame, where a per-object "skip" can go so a hidden object costs almost
nothing, and the design and status of a Hi-Z occlusion test against the
previous frame's depth.

Evidence labels as in [feature-engine.md](../engine/feature-engine.md):
**Code** (read in the disassembly, `h2 disasm`), **Test**, **S**
(supposed). No game data: addresses and offsets are research points.

## The read: how a view's objects are chosen (Code unless marked)

All of this runs in `render_view_scene` `0x0002ba10`, **before**
`render_scene_passes`:

```text
render_view_scene 0x0002ba10
  0x0002b990
    0x0003f2c0   ECX the camera 0x004b9da0, EAX the projection 0x004b9e14,
                 EDI the view's player unit; ret 4
      0x000441b0   the view frustum (0x1bc bytes)
      0x001320f0   visibility: EAX 0 = the main struct 0x00547f88
                   (EAX 1 = 0x0054aa68); stores it to [0x004c0b80]; ret 0x1c
        0x00132b80   sets the mode: 0 portal flood 0x00173130 from the
                     start cluster (clusters with clipped frusta), 1 sphere
                     0x00132d60, 2 everything 0x001733e0
        0x00132220   gather: per visible cluster, both object
                     cluster-reference partitions -> 0x00133050 per object
      0x0003f33c..   appends the view's player unit to the object list
                     if it is not there
      0x0002cb30   resets the render record counters
      0x00133390   records for the BSP clusters
      0x00133520   records for the instanced geometry
      0x00133720   records for the objects  <- OBJECT_RENDER_RECORDS
        0x00134240   reorders the object list
        per listed object: 0x00133790      <- OBJECT_RENDER_SETUP
      0x00044720
render_scene_passes 0x0002bcd0
  0x00040e90   "visibility" at site 0x0002bd2d: consumes the records:
               0x00041c80 object attachments (lens flares, light volumes,
               liquids, recursing into children), 0x00044ac0 walks the
               0x004ba138 records; effects 0x00176bb0/0x00176cb0/0x00174220,
               item lists 0x000420a0/0x00042760
  layers: 0x00044550 -> 0x0003bc30 -> per entry handler -> shader draw
          0x0001bbf0, geometry 0x00041040 (reads the 0x004ba138 records)
```

So `0x00040e90` (0.9 ms a frame in the census) is not where objects are
chosen; it consumes what `0x0003f2c0` built.

### The gather and the per-object test

- `0x00132220`: for each visible cluster `i < [[vis]+0xa6c]` (cluster
  index the word at `[vis]+0xa6e+0x1a·i`), walks partition 1 (heads at
  `[0x004de2e0][cluster]`, nodes in the datum array `[0x004de2e4]`: node
  `+4` the object handle, `+8` the next) and partition 2 (`[0x004de2d4]`,
  `[0x004de2d8]`), and calls `0x00133050` for each object. Objects in the
  sky cluster are force-added (`0x00132681`).
- `0x00133050` (EAX the visibility struct, stack `(handle, cluster
  ordinal, skip-frustum byte, force byte)`, `ret 0x10`): dedups by a
  per-pass stamp (`[0x004de300 + 4·index]` against `[0x004de2fc]`), reads
  the object's sphere (body `+0x30` centre, `+0x3c` radius in the main
  view; `+0x50`/`+0x5c` otherwise), tests it (`0x00132e60`: the cluster's
  portal-clipped frusta through `0x00165010`/`0x00181800`, or a sphere
  test) and at `0x0013312d` (`test al, al / je 0x0013329a`) accepts or
  rejects. Accepted: `0x00132fd0`, the flags word `0x00132b10`,
  `0x00134300` (body `+4` bit 31), the list append `0x00163c80` and the
  stamp `0x000bec70`.
- The list (`[vis+0x14]`): `+0` capacity, `+4` u16 count, `+8` u16
  flags, `+0xc` u32 handles, `+0x10` u32, `+0x14` u16.

### The per-object render setup `0x00133790` (the cost)

Called only from the loop `0x00133746..0x0013377a` in `0x00133720`. ESI
the visibility struct; stack `(list index as a word, byte, byte*, -1, the
caller's argument)`; `ret 0x14`. The handle is `[[ESI+0x14]+0xc+4·index]`.
It computes the view depth (the projection's depth row, globals
`0x004b9e20`/`0x004b9e2c`/`0x004b9e38`/`0x004b9e44`) and pixel size,
picks the LOD (`0x0004baf0`, `0x0004b5d0`) and calls `0x00133ab0`, which
does the node and mirror passes, recurses into children (body `+0x10`
the first, `+0xc` the next sibling) and adds the object's `0x004ba138`
render records through `0x0002cbf0`. Every layer draws from those
records, so an object without records has no draws in any layer.

Its own "not drawn" exit (`0x00133a98`): `or byte [flags + 2·index], 1`,
ECX the flag's address, EDX the flags' base, `ret 0x14`. The caller
reads only AL: non-zero with view kind 1 re-runs the same index.

### Object fields (body = `[[0x004e0300]+0x44 + 12·index + 8]`)

| Offset | Field | Evidence |
|---|---|---|
| `+0x04` | flags (bit 31 sends the accept through `0x00134300`) | Code |
| `+0x0c` | next sibling | Code |
| `+0x10` | first child | Code |
| `+0x14` | parent handle | Code |
| `+0x30` | bounding sphere centre (3 floats), world space | Code |
| `+0x3c` | bounding sphere radius | Code |
| `+0x50`, `+0x5c` | a second sphere for the other views (S: the dynamic-light sphere) | Code; meaning S |
| `+0x64` | position | docs |
| `+0xaa` | type byte | Code |

### Camera and view (while the records are built)

- The render camera `0x004b9da0` (0x74 bytes): position `+0`, forward
  `+0xc`, up `+0x18`, vertical FOV `+0x28`, viewport `+0x30` (i16 top,
  left, bottom, right), near `+0x40` and far `+0x44` (S). The transparent
  sort reads the same position.
- **Why `+0x28` is vertical (Code, `0x0002fb70`, the projection's
  builder).** It takes `t = tan(+0x28 · 0.5)` with `fptan`, the aspect as
  `(right - left) / (bottom - top)` from `+0x30..+0x36`, and stores the x
  scale as `1 / (t · aspect · x extent)` and the y scale as `1 / (t · y
  extent)` (extents 1 by default): x takes the aspect, so `t` is the
  vertical half-angle's tangent. Two things the read does not settle: with
  `[0x00485ac2]` set (and `[0x00510c50]+5`) the aspect is multiplied by a
  constant (`0x0045dfdc`; S: an anamorphic or widescreen mode), and the
  frustum extents can come from the caller instead of the default. The
  occlusion camera uses the plain viewport aspect. **This is the first
  thing to check in game** (below).
- The projection `0x004b9e14` (built by `0x0002fd90`): world-to-view 4x3
  at `+0`, view-to-world at `+0x34`, four frustum bounds at `+0x68`, pixel
  scales at `+0xb8`/`+0xbc` (`0x004b9ed0` is the LOD pixel scale).
- `0x004ba050` the view index (`render_game_view` stores it), `0x004ba034`
  the frame counter (`render_views` increments it), `0x004b9ed8` the
  view's player unit (S).

### What does not go through the object list

The first-person and player chain after the passes (`0x004b9ed8` ->
`0x0002afa0`), effects and particles, the item lists of `0x000420a0` /
`0x00042760` (S: decals and similar), lights. Dynamic lights re-run
visibility with `0x0054aa68` and `0x00133720`; object shadows
(`0x000335e0` -> `0x00033400` picks up to 32 casters from the main list,
`0x00032350` draws them).

## Where to skip

**`OBJECT_RENDER_SETUP` `0x00133790`, main struct only** (ESI
`0x00547f88` and `[ESI+0x2a78] == 0`): a `before` hook that, for an
occluded object, does the "not drawn" exit in Rust (flag bit 0, AL 0,
ECX/EDX as the original) and returns with `ret 0x14`. Nothing of the
object's setup runs: no LOD, nodes, children or records, so none of its
layer draws, shader begins or geometry submissions. The object stays in
the list, so the attachment pass and the shadow caster choice see it as
they see an object too small to draw.

The alternative, rejecting at the gather's accept (`0x0013312d`), also
drops the object from the list: no lens flares or light volumes on it and
no shadow from it onto visible ground. Not used.

Keyed by handle, not list index (`0x00134240` reorders the list). The
light views (`0x0054aa68`) and other view kinds are never touched.

## The design

```text
frame N, game thread                    render thread
OBJECT_RENDER_RECORDS (main view):      .
  arm a capture: view, frame, camera    .
SCENE_TRANSPARENTS 0x0001e370:          .
  capture(tag) -> Device::              .
  request_depth_grid (pending)          .
next D3D entry, after the drain:  --->  Backend::request_depth_grid:
  W-buffer parameters read              DepthGrid::reduce in the command
                                        stream after the opaque draws
Swap                              --->  present: map; a later present:
                                        sink(grid, W parameters)
                                          -> occlusion::deliver: view
                                          depth, ignore_nearer, HiZ,
                                          Frame -> MAILBOX
frame N+1, game thread
OBJECT_RENDER_RECORDS: camera, protected handles, newest Frame
OBJECT_RENDER_SETUP per object: sphere -> footprint -> pyramid max
  -> streak -> skip or run
```

- **Capture point** (`feature::occlusion`, observer `occlusion_capture`
  on `SCENE_TRANSPARENTS` `0x0001e370`): the first transparents call after
  a main view's records were built, so the depth holds the opaque layers
  (and the dynamic lights' passes) but not the transparents or the
  first-person chain, which `render_scene_passes` runs after it. The
  observer tags the ask with the view index, the frame (`[0x004ba034]`)
  and the camera read at the records build.
- **In order with the draws** (`d3d::Device::request_depth_grid`): the
  ask waits on the device until the next D3D entry has drained the push
  buffer, so everything the game queued before the transparents is
  recorded first; the grid is then recorded through
  `Backend::request_depth_grid` (additive, default "cannot"):
  `ThreadedBackend` posts it to the render thread, `ScaledBackend` asks
  the view's scaled depth (what the screen shows) or the reference's,
  `InterpBackend` passes it on (a replay never asks), boot's `DiagBackend`
  passes it on, and `GfxBackend` records `gfx::depth_grid` into the open
  encoder, maps it at the next present and hands it to the sink at a
  present after the map completed (no wait). The viewport in guest pixels
  is scaled to the texture's texels (`grid_rect`).
- **Not recorded** (counted in `Stats::depth_grids_skipped`) when the
  bound depth is not the device's own depth buffer, the depth is not
  W-buffered (`SET_CONTROL0` z-perspective), the W parameters are not
  finite, or the backend cannot (a multisampled or unsampleable depth).
- **W-buffer to view depth** (boot's `w_buffer_mapping`): the stored depth
  is `floor(view_depth · inv_w_far · z_scale) / z_scale`, `inv_w_far` the
  device's `+0x944` and `z_scale` `+0x948` (`0x3f6840` scales the viewport
  matrix by their product), so `DepthMapping::WBuffer { zmax: z_scale,
  scale: inv_w_far · z_scale }`, which takes the far end of each stored
  step.

- **Depth grid on the GPU** (`gfx::depth_grid`, built): one compute
  thread per cell takes the largest raw depth of the pixels whose centre
  falls in the cell; the grid is copied to a mappable buffer and mapped
  without a wait (`PendingGrid::try_take`). 64x32 is 8 KB a frame.
- **Pyramid and test on the CPU** (`engine::render_loop::occlusion`,
  built): `DepthGrid` maps raw depth to view depth (`DepthMapping`; the
  game's W-buffer stores `floor(w·scale)/zmax`, mapped up to the bucket's
  far end so the floor never brings an occluder nearer), `HiZ` is the max
  pyramid, `Camera::project` turns a sphere into a cell rectangle and its
  nearest view depth, `HiZ::test` reads the lowest level where the
  rectangle spans at most 2x2 texels. Linear depth makes the compare a
  plain `near > max + bias`.
- **The test uses the depth's own camera.** The sphere (where the object
  is now) is projected with frame N's camera against frame N's depth, a
  consistent pair, and the radius grows by the camera's displacement since
  then, so parallax cannot uncover an object the old view hid.
- **Results used next frame.** The decision in frame N+1 uses frame N's
  depth (at most two frames old, else nothing is culled).

### Rules (`Rules::default`)

| Rule | Value | Why |
|---|---|---|
| skip after consecutive occluded frames | 2 | one bad frame never culls; the first frame an object is hidden it still draws |
| radius x, + margin | 1.1, + 0.05 wu | sphere inflation |
| + camera displacement since the depth | per frame | parallax |
| depth bias | 0.15 wu | an object just behind a wall is kept |
| cells nearer than a floor are far | `DepthGrid::ignore_nearer` | the first-person weapon and the player's body must not occlude the world |
| spheres reaching the near plane, off screen, non-finite | never culled | the test cannot say |
| radius >= 50 wu | never culled | level-sized objects |
| the player's unit, its parents (vehicle), anything attached to them | never culled | `guest::protected`, `guest::exempt` |
| camera cut (moved > 2 wu or turned > ~45° in a frame), time backwards | all streaks reset, the depth dropped | teleports, cinematics, snapshot restore |
| light views, other view kinds | not touched | |

Transparent objects behind opaque depth are hidden too, so they need no
rule; only opaque layers are in the depth if it is taken before the
transparents (`0x0001e370`). Effects, decals and lights are not in the
object list. Shadows: a skipped object casts no dynamic shadow that frame
(the same as the game's own "too small" case); a caster hidden behind a
wall whose shadow falls on visible ground is the one visible difference
(S: rare, and the inflation narrows it).

### Counters

Once a second, with the switch on:

```text
occlusion: tested T culled C (P%) unknown U exempt E resets R cull X ms/frame over F frames
```

`tested` counts setup calls in the main view, `unknown` the ones without
usable depth or footprint, `cull` the hooks' own time per frame; the line
ends with `grids G`, the depth frames published so far. `unknown` close
to `tested` with `grids` not growing means no depth arrives (see "Not
recorded" above).

## Status

Built and tested, all behind `impl.render.occlusion` (exactly that key;
`impl.render=on` does not turn it on), off by default:

- the pure pyramid, projection, test, tracker and meter, and the raw grid
  to view depth (`cargo test -p engine --lib occlusion`), with synthetic
  depth buffers and spheres;
- the camera's FOV reading (`camera_fov_is_vertical_and_x_takes_the_aspect`):
  a synthetic camera block at `0x004b9da0` with a 16:9 viewport projects
  points at the vertical and horizontal edges where `0x0002fb70` would put
  them. It proves the reading follows the code above, not that the game's
  live block has no other aspect in it;
- the GPU grid reduction against a CPU max per cell (`cargo test -p gfx
  --test depth_grid`, any wgpu adapter);
- the device's grid: the depth the draws before the ask left, not what
  comes after, the W parameters, nothing when not W-buffered, on the
  device's thread, through the render thread and through `InterpBackend`
  (`cargo test -p d3d --test occlusion_grid`, needs
  `DEPTH32FLOAT_STENCIL8`), and the viewport-to-texel rectangle
  (`cargo test -p d3d --lib grid_rect`);
- the guest side (`occlusion::guest`) and the feature on the runtime
  (`cargo test -p feature --test occlusion`): the records, setup and
  transparents hooks over the synthetic generated module, one capture per
  main view and frame, grids delivered as the device would, skipping from
  the second occluded frame and running the original otherwise.

Not covered by a test: boot's glue (`install_occlusion_capture`, a few
lines from the feature's capture to the device's ask and back) and a real
frame.

**First check in game: the FOV.** With the switch on, look for objects
that vanish while visible near the left and right screen edges (an x
scale that is too narrow) or that are never culled while hidden (too
wide). Then compare `render` and the draw count with the switch on and off.

## Expected saving (S)

The setup and its records are per object; each object is several draws
(one per mesh part and layer). With a culled share of P% of the objects'
draws, the saving is about P% of the object part of layer 3 and the
transparents, plus the setup itself. Old Mombasa streets with buildings
between the player and most props are the good case; open terrain is the
bad case (nothing to hide behind). The hooks cost one hooked call per
listed object (it leaves the direct path) and one observer per view.
