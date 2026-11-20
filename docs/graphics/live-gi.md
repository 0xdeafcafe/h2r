# Live GI: the bounce computed every frame

Status: Design. Nothing is built. Slices G0 and G1 decide whether the rest is.
Code: none. It reads what `crates/formats/tags/src/lighting.rs` and `crates/formats/tags/src/structure.rs` decode and `crates/rewrites/native-render/src/load.rs` loads. Tests: none.
Switch: none. It is what the `dynamic` value of the Lighting row (`H2_LIGHTING`, listed in [switches.md](../runtime/switches.md)) computes for indirect light.

The indirect light of the opt-in `dynamic` lighting mode, solved every
sim tick from the live sun, sky, lights and glowing materials instead of
read from Halo 2's baked lightmaps. [dynamic-lighting.md](dynamic-lighting.md)
designs the mode's direct sun over a baked indirect term `I`; this page
replaces `I` with a live one and keeps that page's cascades, layer-3
composition and setting. Retail lighting stays the default. The rule that
must hold: the solve is a function of the scenes drawn since the last
cut, with no random number and no clock in it, so a frame at `alpha = 1`
is the 30 fps frame of that tick and the simulation cannot tell the mode
is on.

Written from the code and docs on master, the `wgpu` 30.0.2 sources the
workspace builds, and public sources. No leaked build or archive was
opened, and none may be for this work. Evidence labels are those of
[dynamic-lighting.md](dynamic-lighting.md) ("Evidence labels"), with one
more: **Guess**, a cost with no measurement of anyone's behind it.

## Summary

- **Technique: relit transport.** A level's geometry never changes, so
  what each surface sees is traced once per BSP, offline, and cached. At
  run time nothing is traced: compute passes light a set of surface
  samples from the live sources and gather their light through the
  cached links into pages laid out as the retail lightmap is.
- **Cost:** 0.6 to 1.3 ms of GPU time per display frame at 60 fps, half
  that at 120, the same at every render scale (**Estimate**, "Budget").
  The world's indirect light costs no more per pixel than retail's: one
  fetch of a page. The sun's cascades add the 2 to 3.5 ms
  dynamic-lighting.md gives them. Nothing here is measured on an M3.
- **Mood** is kept by a fit, per BSP, of the sources' strengths to the
  retail lightmap, and by keeping the light no source on the disc
  explains as a static fill.
- **It cannot** see geometry that moves: a closed door passes bounce
  until slice G7, and a vehicle neither blocks nor reflects it.
- **Go or stop** after G1, which needs no renderer work: the offline
  solve must reproduce three retail levels' lightmaps from the disc's
  own sources ("The checkpoint").

## Technique choice

Costs are per display frame on a base M3 at render scale 4 (2560x1920,
4.92 M pixels) and 60 fps. None is measured on an M3. The bases:

- **A texture tap over the whole frame costs about 0.44 ms**
  (**Estimate**): shadowlab on an M1 Max at 4x took 2.67 ms with nine
  shadow taps and 1.13 ms with one ([notes/e3-lighting.md](notes/e3-lighting.md)),
  so 0.19 ms a tap, times the 2.3 dynamic-lighting.md takes for an M3.
  That is 0.09 ms per million taps. A comparison tap into an 8192 map is
  dearer than most, so it is an upper figure.
- **DDGI** as published: 100 to 300 rays a probe, 4,096 probes in a
  cascade, 16 bilinear fetches per shaded sample, 1 to 2 ms on an RTX
  2080 Ti at 1920x1080, 97% hysteresis and about 100 ms of lag (**P**,
  McGuire); 6 ms in the paper's worst case (**P\***, Majercik).
- **Hardware rays on the M3:** one ray per pixel against 262,267
  triangles took 12.66 ms of fragment time (**P**, nelari.us). The post
  does not give its resolution, so no ray rate follows from it.

| Technique | GPU ms at 4x and 60 fps | Basis | Verdict |
|---|---|---|---|
| DDGI probes, hardware ray queries | trace, shade and blend 2 to 5; lookup 3 to 7 at full resolution, or about 1.3 resolved at 1x and upsampled | trace: **Guess**. Lookup: **Estimate**, 16 taps at the tap cost, or under half of it if a small atlas is that much cheaper to tap | No. Over budget before the sun, on an experimental API (below), and probes interpolate across Halo's thin walls (dynamic-lighting.md, approach b2) |
| DDGI probes, a BVH walked in a compute shader | as above with the trace two to three times dearer | **Guess** | No. Same lookup, slower rays |
| Distance fields or voxels, traced in software (Lumen-like) | 8 to 20 | **Guess**; no public figure was read for this page | No. Its final gather works per screen probe, so it grows with the pixels |
| Surfel GI spawned from the screen | 3 to 8 | **Guess** | No. Needs ray queries each frame and a temporal filter |
| Radiance cascades | screen space: about 3 ms on a GTX 1050 at an unstated size (**P\***, Sannikov), so 4 to 10 here; world space: about 5 ms on an unstated GPU (**P\***) | **Guess** from those | No. The screen-space form loses light from behind the camera, which is most of an interior's fill |
| Screen-space GI | 1.5 to 4 at half resolution | **Guess** | Not as the base. It needs a temporal filter, and the renderer has no motion vectors |
| Light propagation volumes | 0.3 to 1 | **Guess** | No. Nothing occludes inside the volume |
| **Relit transport** (this page) | **0.6 to 1.3**, no added per-pixel work | **Estimate**, link reads at the tap cost ("Budget") | **Yes** |

Combinations: probes are kept, for objects only, and are relit by the
same solve instead of traced. A screen-space pass is left as an option
for contact shade under objects ("Risks and open choices") and is not
part of the base.

Why relit transport fits this game:

- **The level is small and static.** Zanzibar's BSP is 12,462 cluster
  triangles and 3,727 in instance definitions (**V**,
  [notes/e3-lighting.md](notes/e3-lighting.md)). Tracing it once is
  cheap, and the level BVH is the tool for it: a CPU BVH of the
  structure geometry with a ray query and sun visibility, which this
  design depends on and does not build.
- **4x pixels are the scarce thing.** Every per-pixel technique pays
  4.92 M times. This one pays per surface sample and per lightmap texel,
  and the frame samples the result as retail samples its lightmap.
- **Nothing is stochastic at run time**, so there is no noise to filter,
  no hysteresis and no flicker. Published probe systems trade exactly
  that for their rays.
- **No interpolation across walls for the world.** Each texel's links
  were traced from that texel against the real triangles.
- It is the method of shipped precomputed-transfer systems: lightmap
  pixels with stored sample points and weights, relit by a runtime solve
  (**P\***, Enlighten), and probes that hold a small G-buffer of surfels
  relit each frame, about 56 a probe, with the further bounces taken
  from the last result (**P**, Sprentall on The Division), in real time
  on that generation's consoles (**P\***, the session's listing).

### wgpu's ray queries on Metal, as built

Read in the registry sources of `wgpu` 30.0.2 (**Code**):

- `wgpu-hal` `src/metal/adapter.rs` sets `EXPERIMENTAL_RAY_QUERY` when
  the OS is macOS 15 or later and the device answers `supportsRaytracing`
  and `supportsRaytracingFromRender`. The feature's own doc comment in
  `wgpu-types` `src/features.rs` still lists Vulkan only.
- Inline queries work from compute and fragment stages
  (`src/metal/command.rs` binds acceleration structures for both). Ray
  tracing pipelines are `unimplemented!` (`src/metal/device.rs`).
- `naga` `src/back/msl/ray.rs` lowers queries to
  `metal::raytracing::intersection_query` and needs MSL 2.4. It carries a
  `TODO` on triangle winding.
- **No refit.** The Metal backend implements it
  (`refitAccelerationStructure...` in `src/metal/command.rs`), but
  `wgpu-core` `src/command/ray_tracing.rs` always passes
  `AccelerationStructureBuildMode::Build` and logs "only rebuild is
  implemented". A skinned caster would be rebuilt whole every frame.
- The hit's vertices are not returned on Metal
  (`EXPERIMENTAL_RAY_HIT_VERTEX_RETURN` is Vulkan only), so hit shading
  needs the index and vertex data bound again as storage buffers.
- The device must be made with `ExperimentalFeatures::enabled()`, which
  is `unsafe` (`wgpu-types` `src/tokens.rs`). `gfx` asks for no such
  feature (`Capabilities::wanted_features`, `crates/platform/gfx/src/translate.rs`).

So ray queries are usable for an experiment and are not a dependency of
this design. Everything it runs is core WebGPU: compute passes over
storage buffers and buffer-to-texture copies. Slice G0 times ray queries
on the M3 anyway, in a lab tool, so the "No" above rests on a number.

## How it works

Nothing in this section is built. It is the design, in the present tense.

### Transport, offline

`h2 lighting bake <map>` (the verb dynamic-lighting.md adds) reads the
owner's disc and writes one transport cache per BSP into the cache
directory, keyed by map, structure checksum and bake version. Nothing it
writes is committed. It traces with the CPU BVH of the level's render
geometry (clusters and placements, each material's alpha test applied at
the hit), the same on every host. A ray that meets a back face is dark;
a ray that leaves the level is sky.

| Product | What it is | Size (**Design** caps; G0 counts the levels) |
|---|---|---|
| **Surfels** | Senders: one sample per occupied cell of a 0.3 world unit grid, per facing and cluster, over all static surfaces. Position, normal, area, albedo, emission group, cluster | 20,000 to 150,000 a BSP (**S**) |
| **Receivers** | Where the result is read: the retail lightmap pages' covered texels (level 1 of a page when the BSP is over the cap), and the vertices lit per vertex (streams `2e`, `2f`). Position and normal from rasterising each lit triangle in lightmap space, as the split's step 1 does | at most 500,000 |
| **Probes** | For objects: a 1 world unit grid inside each cluster's bounds, less those in solid ([collision.md](../formats/collision.md)) or seeing mostly back faces. Nine SH coefficients per channel, the basis the retail probes use ([lighting.md](../formats/lighting.md)), and a small map of mean hit distance | at most 8,000 |
| **Links** | For each surfel, receiver and probe: 256 cosine-weighted rays, their hits merged by the surfel hit, the weakest dropped. A link is a sender index, a weight and a gate in 32 bits; probe links add a direction | 32 to 48 a receiver; 64 MB a BSP resident |
| **Sky senders** | 32 directions that rays leaving the level are binned to | fixed |
| **Light footprints** | Per placed light, the surfels it reaches and how much of it each sees | small |
| **Gates** | Up to 15 doors per BSP whose closed pose blocks links ("Dynamic objects") | 4 bits a link |
| **Baseline** | The converged solve with the fitted static sources, and the fill ("Calibration") | one value per surfel, receiver and probe |

Tracing is about 170 M rays for a BSP at the caps: a minute or two on
the owner's machine (**Guess**; G1 measures it).

### The solve, once per scene

The solve advances when the render thread takes a new scene, on that
scene's lights, and never per display frame. Its passes, all compute:

1. **Relight.** For every surfel, the direct light arriving: the sun
   through the level sun map, each live light through its footprint or
   its cube ("Light sources"), times the surfel's facing. Radiance out is
   `albedo · (direct + indirect of the last solve) + emission`.
2. **Bounce.** For every surfel, the weighted sum of its links' senders:
   its indirect light for the next relight. One pass adds one bounce, so
   a change reaches its second bounce a tick later. On a cut the pass
   runs four times.
3. **Receive.** For every receiver in the clusters the views can see
   (the union over views and over both blended scenes), the weighted sum
   of its links, written as a colour and a dominant direction. Receivers
   out of sight are not computed; they are not drawn.
4. **Probes.** Every probe's links projected onto the nine SH terms.
5. **Commit.** One buffer-to-texture copy per visible page and its mip
   levels; the colours of receivers that are vertices, and the lighting
   vectors, written to their vertex streams ("The frame").

Sky senders hold the sky's radiance and are never relit. The solve does
not depend on the view: split screen adds receivers, not solves, and a
camera cut costs only the receivers that come into sight.

### The frame

1. The solve's passes for scene `B` are spread over the display frames
   of `B`'s interval (one at 30 fps, two at 60, four at 120) and commit
   before the frame at `alpha = 1`. Frames before it show the last
   commit.
2. Shadow tiles and the sun's cascades, at the start of the view's
   stage, as dynamic-lighting.md and [e3-lighting.md](e3-lighting.md)
   place them.
3. **Layer 3** is the game's own pass. The stage bound to the lightmap
   (engine texture source 11, [shader-instances.md](../formats/shader-instances.md))
   samples the live page: its colour is the indirect light `I`, its alpha
   the sun's unshadowed facing term, and the fetch returns
   `I + sun · alpha · S` with `S` the cascade lookup, the pixel
   translator option dynamic-lighting.md adds. A live page has the retail
   page's size and coordinates (`native_render::world::LightmapKey`), as
   `Rgba16Float` clamped to retail's range when sampled.
4. **The lighting vector.** The pass dots its bump map with a per-vertex
   incident vector whose length says how directional the light is
   ([native-world.md](native-world.md), "What a world pass reads"). The
   commit writes that vector from the receivers' dominant direction and
   the sun's share at the vertex, as the packed word the program reads,
   into a vertex buffer of its own bound to `v5`
   (`native_render::program::INPUTS`). The translated vertex programs do
   not change.
5. Objects draw with lighting constants built from the probes
   ("Dynamic objects"). Lights, self-illumination, environment maps, sky,
   transparents, fog and post are retail's.

### Light sources

| Source | From | In the solve | Lost or unknown |
|---|---|---|---|
| Sun | The sky light's yaw and pitch (**E**), colour and power (**V**, `crates/formats/tags/src/defs/sky_.txt`). Each cluster names its sky ([structure-bsp.md](../formats/structure-bsp.md), cluster `6c` and `6e`), and a surfel takes its cluster's | Direct onto surfels through a level sun map: one depth map of the whole BSP's static geometry along each sky light, drawn when that light moves. Onto pixels through the cascades | How the bake scaled colour and power; the four flag values (**S**: where the light applies in the bake). 22 of 91 skies have no light (**V**) |
| Sky | The sky's model drawn once into a small map and averaged per sender direction; `sky +34`'s fallback colour where there is no model | The 32 sky senders | The bake shot sky photons from sky quads in the level (**P**, c20 materials). Those faces are not in the render geometry (**S**), so a ray that leaves is taken as sky |
| Glowing materials | The `shad +0x0c` block's self-illumination map and colour, on 239 of 2,288 tags (**V**, `crates/formats/tags/src/defs/shad.txt`) | Emission per surfel: colour times the map's mean over the surfel | That the block is the bake's input is **S**. The tools' per-material power, half-life and diffuse scale (**P**, c20) are not known to be on the disc |
| Placed lights | `ligh` and the scenario's 428 placements (**V**, [effects.md](../formats/effects.md)), as the scene's live lights | Onto surfels through the traced footprint, at the live colour and intensity. Their direct light on pixels stays retail's own per-light passes | Whether the bake counted them (**S**; the fit tests it) |
| Attached and timed lights | Flashlights, headlights, projectiles, muzzle flashes, explosions: the live light array, 350 records ([e3-lighting.md](e3-lighting.md), "The light record") | The four strongest by retail's weight get a cube of 128-texel depth faces of the static world; the rest reach only surfels in clusters their own cluster can see, facing them, inside their radius | Lights past the four can light a floor behind a wall inside their radius |
| Bake-only emitters | The tools' "lightmap only" material emits in the lightmapper and is otherwise absent (**P**, c20) | Not sources. Their light is the fill | Gone from the disc |

Only indirect light is written to the pages for lights that retail draws
per pixel, so nothing is counted twice. The sky and glowing surfaces
reach the pages through the links, direct light and bounce together.

### Calibration and the fill

The solve is linear in each source's strength, so the bake solves once
per source group at unit strength (each sky light, each sky, each
glowing material, each light definition) and fits their scales to the
retail lightmap, texel for texel in its own units (base times lightmap
times two, no gamma step; dynamic-lighting.md, "The split"):

1. **Fit.** Non-negative least squares per colour channel over texels
   away from chart edges, with robust weights and with light over
   retail's costing four times light under it (**Design**). Two more
   parameters by search: one gain on every albedo, and the transfer
   between the solve's linear light and the lightmap's stored value
   (linear or a 2.2 power; whichever fits, **S**).
2. **Hold out.** A third of the charts take no part in the fit and give
   the reported error.
3. **Fill.** Where the fitted solve is under retail, the bake finds the
   static direct term whose light plus its own bounce closes the gap,
   low-passed, with the sun's edge bands filled from both sides as the
   split's step 5 does. It is added to the pages and emitted by the
   surfels, so it bounces and reaches objects. This is the light of the
   emitters the disc lacks, kept as what they were: static and unseen.
4. **Report**, per BSP and per cluster: each scale, the *explained
   share* (the part of the retail lightmap's energy the fitted sources
   give without the fill), median and 90th centile luminance error,
   colour error, and the fit against the split's `k` for the sun.

With every source at its authored state and the fill on, the pages are
retail's wherever the fit was under it. No exposure step is known in
retail's post chain (**S**; [native-post-hud.md](native-post-hud.md)
has none), so a level's exposure is the scale of its lightmap, which the
fit sets. A cluster's explained share says how live it is. A BSP below the
checkpoint's share is refused by name and draws the hybrid of
dynamic-lighting.md or retail.

### Albedo and normals

- **The picture needs no albedo.** Layer 3 multiplies the game's own
  base and detail maps by the page, as retail does, so shading painted
  into a base map shows exactly as it does under the retail lightmap.
- **The bounce needs one colour per surfel**: the base map's mean over
  the surfel's footprint, read from the mip level whose texel is the
  surfel's size, times the material's tint where its pass multiplies one
  (**S**), with luminance held between 0.05 and 0.85. Painted shadow
  darkens it a little; the fitted gain takes the average of that out.
  If the fit shows albedo to be its largest error, the exact route is to
  draw each chart with the material's own layer-3 pass under a constant
  lightmap and average that.
- **Alpha-tested materials** let rays through where the map says so.
- **Normals** for surfels and receivers are the vertex frame's. Bump
  maps play no part in transport and keep their part in the picture
  through the live lighting vector.
- **Objects send nothing**, so their materials are not read.

### Dynamic objects

- **Receive.** An object samples the eight probes around it, without
  those in a cluster its own cannot see and those whose distance map
  says a wall is between. The renderer builds the object's lighting
  constants from that sample in place of `Instance::lighting`'s. What
  each input of the record means is not read
  ([native-objects.md](native-objects.md), "Object lighting"); slice G6
  reads it first. Until then objects keep retail's record.
- **Shadow.** Under the sun, the cascades. Elsewhere retail's own
  object-shadow chain, its direction taken from the probe.
- **Contribute.** The lights an object carries bounce: a plasma bolt
  lights the corridor it flies down. An object's body neither blocks nor
  reflects.
- **Doors.** A device that closes a portal is a gate: the bake traces
  with it shut, marks the links it cuts, and the solve scales those by
  how far it is open, which the scene carries.
- **First person** takes the probe sample at the camera.

### Stability, leaks and caches

- **60 and 120 fps.** Between commits the indirect light holds; direct
  light, shadows and objects move at the display rate. The solve is
  keyed to scenes, so in locked hand-off, where every scene is drawn
  ([framescene.md](framescene.md), section 6), two runs give the same
  pages. In paced play a dropped scene shifts convergence by a tick.
- **Lag.** Direct light is immediate. Its first bounce is one tick
  behind, each further bounce one more.
- **Leaks.** World surfaces have none by interpolation. Receivers buried
  in geometry are found by their back-face share and filled from their
  chart. Objects are guarded by cluster and by the distance maps. The
  leaks left are named in "Light sources" and "Dynamic objects".
- **BSP switches and loads.** The cache is per BSP and loads with the
  world. The solve starts from the cached baseline, so the first frame
  after a switch, a revert or a `state.load` (`Cut` in
  `crates/rewrites/frame-scene/src/scene.rs`) is already lit, and the
  four bounce passes of a cut bring in the live lights. Only the active
  BSP's cache is resident.
- **No cache.** The level draws retail lighting, says so once by name,
  and the bake runs in the background.

## Interfaces

- **FrameScene.** It uses what dynamic-lighting.md and e3-lighting.md
  ask of the scene (typed lights, the casters beyond the view, the
  view's sky index) and adds two things (**Design**). Every live light
  once per scene, not only those that can reach a view: a light behind
  the camera bounces into it. The gates' positions, for the doors the
  cache names. The mode is not in the scene: it is a parameter of the
  render thread, so the scene's hash is the same with it on and off.
- **Loaders.** The transport cache, the fit and the fill are level data,
  read at BSP load beside the retail pages.
- **Stages.** The solve's compute passes come first in a frame; the rest
  is "The frame".
- **Agent API** ([agent-api.md](../runtime/agent-api.md)). Each addition
  is needed by every slice here and by E3's:
  - `render.capture` takes `lighting` (e3-lighting.md) and, with
    `dynamic`, `gi: "converged"`: the solve is run to convergence for
    that one scene, so a capture depends on the scene alone.
  - `render.gi {page?}` (new, read): the cache's counts, each cluster's
    explained share and the last solve's work; with `page`, that live
    page as a PNG under the cache directory, to compare with the bake's.
  - `render.frame` lists `gi.relight`, `gi.bounce`, `gi.receive`,
    `gi.probes`, `gi.sunmap` and `gi.cubes` with GPU time, and
    `render.stats` counts surfels, links read, receivers computed,
    lights bounced and lights over the cap.

### One engine with E3 lighting

| Parameter | `retail` | `dynamic` | `e3` |
|---|---|---|---|
| Indirect light | the retail page | live pages, fill on | live pages, fill and indirect gain turned down for its dark shade |
| Sun | baked | cascades, soft | cascades and caster tiles, sharp |
| Lights drawn | the guest's choice | the guest's choice | the renderer's 16 |
| Lights that bounce | none | all | all |
| Edge | retail | soft | sharp |

The transport, the sun map, the cascades, the caster gather and the
light list are shared. The same pages answer E3's lightmap problem (its
target look, item 9): with the sun out of the pages, the level's own
shadows are as sharp as the mode's edge.

## Budget

Base M3, per display frame at 60 fps. All **Estimate**: a link read is
priced as a texture tap, 0.09 ms per million. Whether a read from a
storage buffer costs that is the first thing G0 measures.

| Part | Work per scene | GPU ms per frame |
|---|---|---:|
| Relight | 100,000 to 150,000 surfels, a few taps each | 0.03 to 0.1 |
| Bounce | 3 to 5 M link reads | 0.15 to 0.25 |
| Receive | 150,000 to 300,000 visible receivers, 6 to 12 M reads | 0.3 to 0.55 |
| Probes, commit, vectors | under 1 M reads, the page copies | 0.05 to 0.2 |
| Light cubes, the sun map when the sun moves | four cubes of the static world | 0.05 to 0.2 |
| **Live GI** | 1.2 to 2.6 ms per scene | **0.6 to 1.3** |
| Sun cascades and their lookup (dynamic-lighting.md) | | 2 to 3.5 |

At 120 fps the same work spreads over four frames: 0.3 to 0.65 ms. At
30 it is all in one. Memory is the 64 MB of links, about 24 MB of pages
(**S**; G0 counts the texels) and the cubes. On disk, the caches of the
disc's 78 lightmap groups (**V**, [lighting.md](../formats/lighting.md))
come to about 5 GB at the cap. The render thread's own time is the
uploads of lights and gates.

The retail frame's GPU time at 4x on the M3 is not known; the last
whole-frame figure there is 32 fps ([notes/native-ab.md](notes/native-ab.md)).
If the measured frame cannot take the mode, it gives way in this order:
the bounce pass on alternate scenes, receivers at page level 1, then the
baseline pages bound with no solve, which is a static bake of ours.

## Rules and limits

- **Render only.** The mode reads the scene and tags and writes no guest
  memory. Gate: `sim.hashes` equal with the row at `retail` and
  `dynamic`.
- **Deterministic.** No random numbers, no wall clock, no dependence on
  the display rate.
- **Off by default**; off, no cache is loaded and no pass runs.
- **No disc data in the repository.** Caches, fits, reports, pages and
  captures stay in the cache directory or scratch. Tests build invented
  rooms.
- **No quiet fallback.** A BSP without a cache, or refused by the fit,
  draws another mode and is named once.
- **Only typed layer-3 draws take live pages**, so the mode is offered
  when the opaque block has no remainder (framescene.md, slice 3), and
  the runtime slices sit on the renderer of its slice 6.
- **Offered only where it fits**, as framescene.md's slice 8 does for
  frame rates.
- **Static geometry only** in the transport. Specular and glossy bounce
  are not attempted.

## Slices

One lane each. G0 to G2 need only the level BVH and start at once. G3 on
need FrameScene slice 6.

| | Slice | Testable result | Measure |
|---|---|---|---|
| G0 | **Measure.** A census in `h2 lighting --all`: per BSP the triangles, lit vertices, lightmap texels and their size in world units, surface area, clusters, glowing materials and their area, light placements. `gilab`, a tool beside `crates/tools/viewer/src/bin/shadowlab`: the solve's passes on invented scenes at set counts; a 16-tap probe lookup at 4x; ray queries through wgpu and a BVH in compute | Tests of the census on invented maps; `gilab`'s passes equal a CPU reference | On the M3 lab laptop ([lab.md](../runtime/lab.md)): ms per million link reads, the lookup, rays per second; the retail frame's GPU time at 4x from `render.stats` (the time per pass group dynamic-lighting.md's slice 0 adds). A note under `notes/` |
| G1 | **Transport, the CPU solve and the fit**, offline: every product of "Transport, offline", "Calibration and the fill" and the report, written outside the repository | Invented rooms: form factors of a box against the closed form; a red wall tints the floor; a sealed room with no source is black; a thin wall passes nothing; the fit recovers known scales; the fill closes a hidden emitter. **The checkpoint** | Bake time and cache size per BSP |
| G2 | **The solve on the GPU**, in `gilab`, on a real cache | Pages equal the CPU solve's to one step of 255; two runs are bit-equal | ms per pass at real counts on the M3, against "Budget" |
| G3 | **Live pages in layer 3**, static sources and fill only: the loader, the pages, the translator option, `render.gi` and the capture options | A page from `render.gi` equals the bake's baseline page. `render.compare` of `retail` against `dynamic` at one tick: mean under 6 of 255 and each cluster's mean luminance within 10% (**Design**); `sim.hashes` equal | `gi.*` from `render.frame` |
| G4 | **The sun**: level sun map, `I + sun · alpha · S`, the lighting vector; shared with dynamic-lighting.md's slices 3 and 4 | An invented wall and occluder; captures differ from G3's only in sunlit clusters; the `alpha = 1` frame equals the 30 fps frame | cascades and lookup at 4x |
| G5 | **Lights bounce**: every live light in the scene, footprints, cubes | Invented: a lamp off removes its pool and its bounce on a wall it does not face, and other fill stays. Game: firing in a dark corridor (`input.pad`) lights the walls in the capture and they are dark again ticks later | `gi.cubes`, lights bounced and over the cap |
| G6 | **Objects receive**: read the lighting record's inputs, build them from probes, the shadow direction, first person | A differential on invented records; a capture of a character crossing from a lit room to a dark one | render-thread time |
| G7 | **Gates, cuts, caps**: doors, BSP switches, reverts, residency | Invented: a shut door darkens the room behind it; a capture one frame after a BSP switch is within the G3 bar of one taken ten seconds later | load time, memory |
| G8 | **One engine**: the `e3` column of "One engine with E3 lighting", the row offered only where the measured frame fits, 120 fps, split screen | The three modes from one sealed scene through `render.capture`; a table of each mode's cost | the lab A/B of e3-lighting.md with `dynamic` added |

### The checkpoint

After G1, with G0's numbers, on three BSPs: an outdoor multiplayer map,
an interior campaign BSP and one that mixes both. Go when all hold:

- the explained share is at least 60% and the median luminance error on
  the held-out charts at most 25%, fill off (**Design**; the owner may
  move both after looking);
- the owner accepts live pages beside retail's for the three;
- G0's constants put the largest BSP's solve under 3 ms per scene on the
  M3, and the retail frame at 4x leaves the mode 4 ms.

Stop otherwise. What stands then is the hybrid of dynamic-lighting.md
and G0's numbers in the notes.

## Risks and open choices

Risks, largest first:

1. **The disc's sources do not explain the lightmaps.** Then most light
   is fill and little is live. G1 finds this before any renderer work.
2. **The frame has no room.** The retail frame's GPU time at 4x on a
   base M3 is unmeasured.
3. **Moving geometry is outside the transport**: doors until G7,
   vehicles and bodies always.
4. **The object lighting record is not understood.** If it cannot be
   built from probes, objects stay lit by the retail lightmap under them
   and disagree with the world when a light changes.
5. **The largest BSPs** may not fit the link cap without visible loss.
6. **The lighting vector** is per vertex, as retail's is, but live: bump
   relief under a moving light changes at vertex density.
7. **Sky taken from escaping rays** may light clusters the bake kept
   dark. The report shows it as light over retail's.
8. **Albedo** from painted base maps.

| Choice | Options | Pick |
|---|---|---|
| Technique | relit transport; DDGI by ray query; the hybrid alone | relit transport |
| Fill | on; off; only in clusters under a share | on, with the share reported |
| When the bake runs | at first load, in the background, cached; by command only | at first load, and the command for all maps |
| Ray queries | an experiment in `gilab`; a runtime dependency; never | an experiment |
| Link cap per BSP | 32, 64 or 128 MB | 64 MB |
| E3 lighting | a tuning of this engine; its own passes | a tuning, at G8 |
| Indirect light between ticks | hold; blend two pages | hold |
| What objects contribute | their lights only; a screen-space contact pass as well | their lights only |
| A moving sun | not designed; designed | not designed, and the level sun map leaves it open |

## Not verified

- Every cost on this page. Nothing was run.
- That the retail lightmaps can be reproduced from the sources the disc
  holds, on any level.
- The role of the sky light's flags, colour and power in the bake; that
  the `shad +0x0c` block is the bake's emitter input; whether placed
  lights and scenery were in the bake.
- Where the bake's sky quads were, and whether the collision geometry
  keeps them.
- The transfer between linear light and the lightmap's stored value.
- What the object lighting record's inputs mean, and whether the object
  programs can be driven from SH probes.
- That every live light's frame can be read for a light no view draws:
  attached lights are re-posed in the guest's render pass (e3-lighting.md,
  "The light record").
- The lightmap's cycling coefficients ([lighting.md](../formats/lighting.md),
  `ltmp+34`) and what they do to a page.
- Halo 1 maps converted by `h1convert`.

## Sources

Read 2026-11-20, all public, nothing copied, no leaked file opened.

- Morgan McGuire, "Dynamic Diffuse Global Illumination", overview, 2019:
  <https://morgan3d.github.io/articles/2019-04-01-ddgi/overview.html>
- Majercik and others, "Dynamic Diffuse Global Illumination with
  Ray-Traced Irradiance Fields", JCGT 8(2), 2019 (through a search
  excerpt): <https://www.jcgt.org/published/0008/02/01/>
- Bob Sprentall, notes on Nikolay Stefanov's "Global Illumination in Tom
  Clancy's The Division", GDC 2016:
  <https://insomniacgames.github.io/0x00/bsprentall/GI%20-%20The%20Division.html>;
  the session's listing (through a search excerpt):
  <https://gdcvault.com/play/1023273/Global-Illumination-in-Tom-Clancy>
- Enlighten SDK documentation, "How Enlighten works" and "Runtime
  radiosity updates", and the GDC 2011 Battlefield 3 radiosity session
  as summarised by Wolfire (through search excerpts):
  <https://documentation.enlighten.siliconstudio.co.jp/wiki/spaces/SDK402/pages/2485157936>,
  <http://blog.wolfire.com/2011/03/GDC-session-summary-Battlefield-3-Radiosity>
- Alexander Sannikov, "Radiance Cascades" (through a search excerpt):
  <https://github.com/Raikiri/RadianceCascadesPaper>
- nelari.us, "A quick look at Apple Silicon's hardware accelerated ray
  tracing performance", 2024:
  <https://nelari.us/post/metal-raytracing-performance/>
- c20, Halo 2 materials (lightmap only, sky and sun quads, power,
  half-life and diffuse scales):
  <https://c20.reclaimers.net/h2/source-data/h2-materials>

## Related

[dynamic-lighting.md](dynamic-lighting.md), [e3-lighting.md](e3-lighting.md),
[framescene.md](framescene.md), [native-renderer.md](native-renderer.md),
[native-world.md](native-world.md), [native-objects.md](native-objects.md),
[native-lights.md](native-lights.md), [high-resolution.md](high-resolution.md),
[lighting.md](../formats/lighting.md),
[structure-bsp.md](../formats/structure-bsp.md),
[shader-instances.md](../formats/shader-instances.md),
[bitmaps.md](../formats/bitmaps.md),
[notes/e3-lighting.md](notes/e3-lighting.md).
