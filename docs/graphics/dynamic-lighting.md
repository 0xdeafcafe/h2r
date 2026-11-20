# Dynamic lighting: a live sun over the baked lightmaps

Status: Design (investigation). Nothing is built. The recommendation is go, in stages, for the hybrid in "How it works"; slices 0 and 1 decide whether the rest is built.
Code: none. The data it reads is decoded by `crates/formats/tags/src/lighting.rs` and `crates/formats/tags/src/structure.rs` and loaded by `crates/rewrites/native-render/src/load.rs`. Tests: none.
Switch: none. The design adds the value `dynamic` to the Lighting row (`H2_LIGHTING`), listed in [switches.md](../runtime/switches.md).

An opt-in mode that takes the sun out of Halo 2's baked lightmaps and
lights it live, with shadow maps, so that the level and everything in it
cast and receive the sun's shadow. Retail lighting stays the default. The
baked lightmap stays the source of all indirect light: an offline tool,
run on the owner's disc, splits each lightmap into the sun's part and the
rest, and the renderer puts the two back together with a live shadow term.
The rule that must hold: with the baked shadow term in place of the live
one, the mode draws retail's image.

This page answers whether the mode is worth building, compares the ways
to do it, and gives the slices. It was written from the code and docs on
master and from public sources. No leaked build or archive was opened,
and none may be for this work.

## Evidence labels

| Label | Meaning |
|---|---|
| **Code** | read in this repository, or in the `wgpu` 30.0.2 sources the workspace builds (`Cargo.lock`); the path is given |
| **V** | checked on the owner's disc, in the doc named |
| **E** | read from the retail image, in the doc named |
| **P** | a public source, listed under "Sources". **P\*** was read through a search excerpt, so its wording is unverified |
| **S** | supposed; what would verify it is given |
| **Estimate** | a cost derived from a measurement elsewhere; the derivation is stated |
| **Design** | our choice |

## Summary

- **Go, staged, for the hybrid.** Keep the retail lightmap for indirect
  light, subtract the sun's direct part offline, and add it back live
  under a shadow map. It is the only approach that starts from retail's
  image and changes one thing.
- **Not now for our own bake of indirect light.** The disc lacks part of
  what Bungie's bake was lit by, so a bake of ours is a relight, not a
  restoration.
- **No to runtime global illumination and to hardware ray tracing at run
  time.** One shadow ray per pixel misses the frame budget on a base M3
  at 4x, wgpu's ray queries on Metal are experimental, and runtime GI
  replaces the art's ambient light with something else.
- **Cost of the hybrid:** about 2 to 3.5 ms of GPU time per display frame
  on a base M3 at 2560x1920 (**Estimate**, "Approaches"). The retail
  frame's own GPU time at that scale is not measured; slice 0 measures it.
- **The runtime slices sit on the FrameScene renderer**
  ([framescene.md](framescene.md)), not on the tenant path. The offline
  slices need nothing and can start at once.

## What the disc holds

| Data | Where | How much | What a dynamic system can take from it |
|---|---|---|---|
| Lightmap pages | the `ltmp` group's bitmap, P8 indices through 256-entry palettes ([lighting.md](../formats/lighting.md)); expanded by `native_render::load::expand_page` | 13,658 pages, 2,042 palettes, 78 groups (**V**) | Every baked source summed into one colour per texel: sun, sky, bounce, emitters. The base of the split |
| Lightmap coordinates | stream `1f` on clusters, buckets for placements ([structure-bsp.md](../formats/structure-bsp.md)) | every lit vertex | Texel to world position, so a tool can trace visibility per texel |
| Incident vector | stream `30`, signed 11:11:10, length at most 1, not unit (**V**) | 5,196,110 vertices (**V**) | A dominant light direction per vertex whose length says how directional the light is (**P\***, Chen). The layer-3 pass dots the bump map with it ([native-world.md](native-world.md), "What a world pass reads") |
| Per-vertex colours | bucket streams `2e` and `2f` (**E**, lighting.md) | 1,748,301 vertices (**V**) | Baked light for geometry lit per vertex. The engine reaches these buckets for object records (**E**, native-world.md) |
| Probes | `ltmp` group `+0x38`: position and nine SH coefficients per channel | 147 in 26 groups (**V**) | Too sparse for an irradiance volume. An input to the guest's object lighting |
| Object lighting record | `0x78` bytes per drawn object, computed by the guest each frame; `frame_scene::Lighting` | per frame | What the lit object programs read. Retail fills it with fake lights taken from lightmap samples (**P\***, Chen). What each input means is not read ([native-objects.md](native-objects.md), "Object lighting") |
| Sky lights | `sky +0x78`: yaw and pitch (**E**); a parameters record with flags, colour, power, falloff distance and cutoff angle (**V**, values only; `crates/formats/tags/src/defs/sky_.txt`) | 71 lights on 91 skies: 67 skies have one, 2 have two, 22 none (**V**, [sky.md](../halo1/sky.md)) | The sun's direction. Its colour and power are on the disc; how the bake scaled them is not known, and slice 1 fits it |
| Light definitions | `ligh`: shape, radius or cone, falloff, cutoff, two colour pairs, brightness, gel, the no-shadow bit ([effects.md](../formats/effects.md)) | 1,596 records (**V**) | Local lights, with their scenario placements, attachments and effect parts (`h2 lights`) |
| Live lights | the light datum array, 350 records, settled at the per-light pass ([e3-lighting.md](e3-lighting.md), "The light record") | per frame | Retail already draws these as additive per-light passes over shadow buffers (layers 10 to 13) |
| Material bake inputs | the `shad +0x0c` parameter block: a self-illumination map, a colour and scalars (`crates/formats/tags/src/defs/shad.txt`) | 239 of 2,288 distinct tags have the colour and the map (**V**) | The draw path never reads the block (**E**). **S**: these are the bake's emitter inputs; the official tools name per-material lightmap power, half-life and diffuse scale (**P**, c20 materials) |
| Self-illumination, environment maps, transfer lighting | layers 7, 8 and 5 (e3-lighting.md, "Pass taxonomy") | 3,417, 977 and 126 passes (**V**) | Additive and independent of the lightmap. Unchanged by the mode |
| Geometry | `tags::structure`, `tags::model`, [collision.md](../formats/collision.md) | the whole level and every model | Shadow casters and rays, including what the camera cannot see |

### What is missing

- **The parts of the sum.** A texel holds one colour. Nothing says how
  much of it is the sun, the sky, a bounce or an emitter, or which light
  lit it.
- **Emitters that are not in the map.** The official tools have a
  "lightmap only" material: it emits in the lightmapper and is otherwise
  absent, neither drawn nor collidable (**P**, c20 materials). Light from
  such geometry is in the retail lightmap and its source is not on the
  disc. The same page lists geometry the lightmapper ignored and
  materials that refused sky photons.
- **The bake's settings.** The lightmapper is photon-based with a photon
  count, a gather distance and a bounce switch per quality level (**P**,
  c20 H2V tool). Which level each retail map shipped with is not known.
- **The sun's size.** A sky light's cutoff angle is 2 to 20 degrees
  (**V**). That it is the sun's disc, which would set the width of a
  baked penumbra, is **S**; slice 1's edge fit checks it.
- **The sky light flags.** Four values occur (**V**); none is named.
- **Lightmap resolution per level.** Not recorded in any doc here. Slice
  0 measures it.

## Approaches

Costs are for a base Apple M3 at render scale 4 (2560x1920, 4.92 M
pixels, [high-resolution.md](high-resolution.md), "Cost"), per display
frame, against a 16.7 ms frame at 60 fps. All are **Estimate**. The base:

- shadowlab on an M1 Max at 4x: a sun map with one tap 1.13 ms, nine taps
  2.67 ms; a six-face point light 2.93 ms and 6.26 ms
  ([notes/e3-lighting.md](notes/e3-lighting.md)). Its sun map is 8192
  texels on a side at 4x and the level does not cast.
- A base M3 scores about 46,000 to 48,000 in Geekbench Metal against
  105,000 to 112,000 for an M1 Max (**P\***), so M1 Max times are
  multiplied by 2.3.
- One ray per pixel, full screen, against a 262,267-triangle scene takes
  12.66 ms of fragment time on a base M3 and 25.20 ms on an M2 (**P**,
  nelari.us).

| | Approach | GPU cost | Against retail's art |
|---|---|---|---|
| a | Direct lights and shadow maps only, no lightmap | 2 to 3.5 ms for the sun; 0.3 to 0.5 ms per shadowed spot (1024 map), 0.6 to 1 ms per shadowed sphere (six 512 faces) | **Breaks it.** Shadows are black or a flat ambient, interiors lose the light their emitters baked in. The `tex_bump` family carries an implementation lit from engine vectors, which the record builder selects on a BSP without a lightmap group (native-world.md, "Material translation"); this approach is that look |
| b1 | a, plus indirect light baked by us offline (lightmap atlas or an irradiance volume) | as a; the baked term costs one fetch | **A relight.** Consistent, but not Bungie's bounce and missing the emitters that are not on the disc. Needs tuning per level and never converges on retail |
| b2 | a, plus probes updated at run time (DDGI) | 3 to 5 ms more, and it needs ray tracing | **Breaks it.** Leaks through thin walls are the method's known weakness (**P**, Toth), and a level's clusters are open sheets: on Zanzibar none of 28 pieces is closed (**V**, notes/e3-lighting.md) |
| b3 | Screen-space ambient occlusion or GI on top | 1 to 1.5 ms at half resolution for occlusion; more for GI | **Degrades it.** The lightmap already holds occlusion; a second darkening shows as dirt in corners. Screen-space GI needs a temporal filter, and the renderer has no motion vectors |
| c | Hardware ray tracing: shadows or GI by ray query | over 12 ms for one shadow ray per pixel; 4 to 5 ms at half resolution with an upsample | Shadows would match the hybrid's; GI as b2. Over budget, and experimental in wgpu (below) |
| d | Hybrid: the retail lightmap for indirect, the sun's direct part live | 2 to 3.5 ms | **Closest, and better where it differs**: objects cast and receive the sun, edges are as sharp as the shadow map instead of the lightmap. Equal to retail by construction when the live shadow equals the baked one |

Notes on the estimates:

- **The sun in d and a**: three cascades of 2048 texels, static casters
  cached per cascade and redrawn only when the cascade scrolls, object
  casters every frame: 0.6 to 1.2 ms. The lookup in layer 3 and on
  objects, four taps through a comparison sampler: 1 to 2 ms at 4.92 M
  pixels. Three cascades hold a fifth of the texels of shadowlab's one
  map, and its nine taps become four.
- **Local lights**: shadowlab's six-face point light at 1x, with 1024
  faces, is 0.76 ms on the M1 Max. A sphere with 512 faces covers part
  of the screen; a spot is one face.
- **b2, b3 and the half-resolution figure in c** have no measurement of
  ours behind them. b2 is a 16x16x8 grid at 64 rays a probe (131,072
  rays a frame, at the ray cost above) plus eight probe taps per pixel
  in layer 3. c at half resolution is a quarter of the rays plus a
  filter.
- **Memory**: three 2048 depth cascades are 48 MB; the split doubles the
  lightmap pages.
- **Split screen** multiplies the cascades by the views.

### Hardware ray tracing on Apple, as wgpu has it

- `wgpu-hal` 30.0.2's Metal backend builds acceleration structures and
  sets `Features::EXPERIMENTAL_RAY_QUERY` when the OS is macOS 15 or
  later and the device answers `supportsRaytracing` and
  `supportsRaytracingFromRender` (**Code**, `wgpu-hal`
  `src/metal/adapter.rs`, `src/metal/device.rs`). Ray tracing pipelines
  are `unimplemented!` there, so it is inline ray queries from fragment
  and compute shaders only.
- The feature is experimental: the device must be made with
  `ExperimentalFeatures::enabled()`, which is `unsafe` (**Code**,
  `wgpu-types` `src/tokens.rs`). `gfx` asks for no experimental feature
  (`Capabilities::wanted_features`, `crates/platform/gfx/src/translate.rs`).
- Metal support landed in wgpu 29 (**P**, wgpu PR 8071, merged February
  2026). Third parties report open correctness bugs in it (**P\***).
- The M3 family is the first Mac silicon with hardware ray tracing
  (**P\***, Wikipedia). Earlier chips answer the same queries in
  software, at about half the speed (**P**, nelari.us).
- Skinned casters would need a compute pass that writes posed vertices
  for a BLAS refit each frame; the renderer skins in the vertex stage.

Ray queries are a fit for the offline tool, where time does not matter
and a CPU tracer gives the same answer on every host. They are not a
dependency this design takes at run time.

## How it works

### The split, offline

`h2 lighting bake <map>` (a new verb of the existing command) reads the
owner's disc and writes into the cache directory. Nothing it writes is
committed. For each BSP with a lightmap and a sky light:

1. **Texel positions.** Rasterise every lit triangle in lightmap
   coordinate space, giving each page texel a world position and a
   geometric normal.
2. **Visibility `V`.** Trace from each texel towards the sky light over
   the sun's disc and the texel's footprint, against the level's render
   geometry (clusters and placements, with each material's alpha test)
   and the scenario's scenery at its placed pose. `V` is the lit
   fraction, 0 to 1. Whether the bake saw the scenery is **S**; the
   report gives the fit with and without it.
3. **Unshadowed direct `D`.** `D = k · max(0, n · l)`, in the lightmap's
   stored units. `k` is fitted per sky light and channel from pairs of
   neighbouring texels on either side of a baked shadow edge, where the
   retail difference is the sun alone.
4. **Indirect `I`.** `I = retail - D · V`, clamped at zero. This is
   everything that is not the sun: sky, bounce, emitters, including the
   ones whose source is not on the disc.
5. **Edge bands.** Where `V` changes within a texel's reach, the baked
   edge and the traced one differ in blur, and step 4 leaves a ridge.
   There `I` is filled from both sides, since indirect light is smooth
   across a sun shadow's edge.
6. **Per-vertex data.** The `2e`/`2f` colour buckets are split the same
   way, per vertex.

Two sets of pages come out. The *closure* set is step 4's `I` and the
baked direct page `B = D · V`, multiplied per texel, so that `I + B` is
the retail page to within one step of 255 and stays so under filtering,
which is linear in each page. The *play* set is step 5's `I` and `D`.
Each stored mip level is split on its own. A report gives, per BSP, the
fitted `k`, the share of texels that clamped, and the edge-band share.

The lightmap is in the units the pixel shader multiplies: the layer-3
pass is base times lightmap times two with no gamma step
([editor.md](../editor/editor.md), "Packages and Halo 1 in the Maps
list"; native-world.md, "Material translation"). The split works in those
units, so no colour-space conversion enters.

### The frame

1. **Cascades.** At the start of a view's stage the renderer draws the
   sun's shadow cascades, fitted to the view's camera and snapped to
   their texel grid: the static world from GPU-resident geometry, then
   the scene's casters with the palette rows they are drawn with.
2. **Layer 3.** The world's lightmap pass is the game's own, translated
   as today. One thing changes: the stage bound to the lightmap (engine
   texture source 11, [shader-instances.md](../formats/shader-instances.md))
   fetches `I + D · S`, where `S` is the cascade lookup at the fragment's
   world position, rebuilt from the fragment position and W. The switch
   is an option of the pixel translator, as the hard shadow tap is
   (`shaders::psh::PixelOptions::hard_shadow`,
   `crates/formats/shaders/src/psh/wgsl.rs`). Base maps, detail maps,
   bump modulation, specular, alpha test and every later layer are
   untouched.
3. **Objects.** An object draws with the lighting record the guest
   computed, unchanged, until slice 5. It casts from slice 4.
4. **Local lights.** Retail's per-light passes stay. The renderer draws a
   shadow map for any typed light it chooses, without changing the
   guest's own decision.

With the closure pages bound and `I + B` fetched, the frame is retail's.
That is the parity check for the pages, the coordinates and the
translator option, before any shadow map exists.

## Content

- **Materials are authored against the lightmap, and the mode keeps the
  lightmap's place in them.** The world's layer-3 pass combines base and
  detail maps, the bump map dotted with the incident vector, and the
  lightmap (native-world.md). The mode replaces the lightmap fetch and
  nothing else, so the 165 templates and 147 passes
  (shader-instances.md) are not rewritten.
- **Shading baked into base maps** (occlusion and contact shadows painted
  by the artists, **S** by eye) is why b3 is refused and why the mode
  adds no occlusion of its own.
- **Bump detail.** The world's `tex_bump` family has bump maps, so
  no normal maps need making for it. Retail's bumps respond to one
  direction per vertex and go flat in shadow (**P\***, Chen). The mode
  keeps the retail vector, so a surface inside a live shadow still shows
  bump shading from the sun's side. The share of world parts with no
  bump stage is not counted. Objects are lit per vertex from the record
  (native-objects.md, "Object lighting"), so from slice 5 an object's
  sun is per vertex in colour and per pixel in its shadow.
- **Self-illumination and environment maps** are additive layers and do
  not change. Self-illuminated surfaces lit their surroundings in the
  bake; that light is in `I`.
- **Sky and fog** are tuned to the baked sun. The mode does not move the
  sun or change its colour, so neither needs retuning. A time-of-day
  option would, and is not designed.
- **Levels without a sun.** 22 skies have no light (**V**), and interior
  clusters see no sky. There `D` is zero and the mode draws retail, plus
  whatever local-light shadows slice 6 adds. The visible gain is
  outdoors.
- **Tuning per level: none committed.** The fit of `k` and the edge
  bands run on the owner's machine. A level whose report shows a large
  clamped share is one where the model is wrong (a second sun, an
  occluder the map lacks), and the mode refuses it by name instead of
  drawing a bad split.
- **A bake of our own can fill gaps later**: indirect light for objects
  (an irradiance volume in place of the 147 probes) is the one place the
  retail data is thin. It is not in the slices.

## Interfaces

- **FrameScene.** The typed lights and shadow maps of its slice 3 are
  reused. Two additions (**Design**): the view's sky index in
  `WorldState` (the camera cluster's sky, structure-bsp.md `6e`), and a
  per-view range of *casters*: objects inside the cascades' volume that
  the view itself does not draw, with model, level of detail and node
  matrices. Extraction gathers them from the cluster object partitions
  the scene lists already walk ([native-renderer.md](native-renderer.md),
  "Scene list builders") and runs no guest setup for them, so none of the
  writes the tick reads (framescene.md 5.2) is added.
- **Loaders.** The split pages, the sky lights and `k` are level data,
  not frame data: the world loader reads them from the cache directory
  at map load, keyed by map and structure checksum, and uploads `I` and
  `D` beside the retail layers. A missing or refused bake is a named
  error and the level draws with retail lighting.
- **Stages.** The cascades are the first passes of a view's stage,
  batched as framescene.md's slice 3 batches shadow maps. Layer 3 and the
  object stage read them. Nothing after the opaque block changes.
- **Blending for 60 and 120 fps.** The sun does not move. The cascades
  follow the blended camera, and casters use the blended palette rows
  they are drawn with, so shadows move at the display rate
  (framescene.md 7, "Shadow maps"). The cascade fit is a pure function
  of the camera, so a frame at `alpha = 1` is the 30 fps frame of that
  tick.
- **The setting.** `[graphics] lighting = "dynamic"`, a third value of
  the Lighting row ([settings-menu.md](../game/settings-menu.md)), live
  at a frame boundary as the row is today.
- **Agent API.** Each runtime slice is checked with `render.capture` and
  `render.compare` ([agent-api.md](../runtime/agent-api.md)): `state.save`
  once, then for each arm `state.load`, `settings.set` and
  `render.capture` of the same tick; equal `scene_hash` values show both
  arms drew the same scene. Slice 0 adds GPU time per pass group to
  `render.stats`, because every later render lane needs it too.

## Shared with E3 lighting

[e3-lighting.md](e3-lighting.md) and this page want the same machinery
with different settings. E3 lighting wants hard one-tap edges, per-pixel
dynamic lights and a darker ambient; this mode wants retail's look with a
live sun. Its target look says lightmaps give the ambient only and direct
light is dynamic, which is the split.

| Piece | This page | E3 lighting |
|---|---|---|
| The sun's direction from `sky` | slice 1 | its sun shadows from objects |
| Sun cascades and their caster gather | slices 3 and 4 | the same maps, sampled with one tap |
| Casters beyond the view in the scene | slice 4 | its off-screen caster risk |
| The split pages | slices 1 and 2 | removes its lightmap double count: an object's sun shadow darkens `D` only, with no mask guessed from the lightmap's value |
| Shadow maps for any typed light | slice 6 | its per-pixel lights |
| The filter | soft, sized to the bake's penumbra | one tap |

So the two are values of one row over one implementation, and the slices
here are ordered so that each is useful to both.

## Rules and limits

- **Render only.** The mode reads the scene and tags and writes no guest
  memory. The guest's own object lighting, light list and shadow
  decisions are as with the mode off. The gate is `sim.hashes` equal
  with the row at `retail` and at `dynamic`.
- **The guest cannot see it.** From framescene.md's slice 6 a scene is
  drawn once and nothing in it is read back by the guest (its section
  8.3), so the mode's pixels reach no guest read. Before that slice a
  read of a scaled surface would return them, which is the second reason
  the runtime slices wait for it.
- **Off by default**, and with it off no cascade is drawn and no split
  page is loaded.
- **No disc data in the repository.** The bake's output, its reports and
  every capture stay in the cache directory or scratch. Tests build
  invented levels.
- **No quiet fallback.** A level without a usable bake draws retail and
  says so once, by name.
- **Only typed layer-3 draws take the split.** A world material the
  native stage refuses (native-world.md, "Material translation", lists
  the reasons) keeps the retail lightmap, and a level would mix the two.
  The mode is offered once the opaque block has no remainder, which is
  the gate of framescene.md's slice 3.
- **Offered only where it fits.** As framescene.md's slice 8 does for
  frame rates, the row offers `dynamic` at a render scale only when the
  measured frame fits the display frame.

## Slices

One lane each. Slices 0 and 1 need nothing that is not built. Slices 2
to 6 sit on the FrameScene renderer and start after its slice 6; building
them on the tenant path would be written twice.

| | Slice | Proof |
|---|---|---|
| 0 | **Measure.** GPU time per pass group in `render.stats` (timestamp queries in `gfx`), then the retail frame at 4x on the target machine through `h2 lab` ([lab.md](../runtime/lab.md)). A census in `h2 lighting --all`: page sizes and texels per world unit per BSP, clusters with a sky, sky lights per BSP with their flags, and the share of lit vertices whose incident vector lies within 15 degrees of a sky light. shadowlab with the sun read from `sky`, the level casting and three cascades, timed at 4x on the same machine | numbers in a note under `notes/`; the incident-vector share says whether the sun dominates where the model assumes it does |
| 1 | **The split, offline.** A viewer tool beside shadowlab: steps 1 to 6 of "The split", pages and a report written outside the repository | tests on invented levels built by the forward model (a known `I`, `D` and `V` recovered; closure to one step); the owner looks at before and after pages for three outdoor levels. **Go or stop for everything below** |
| 2 | **Bake and load.** `h2 lighting bake`, the cache, the loader, the translator option, and the closure draw (`I + B`) | `render.compare` of retail against closure at the same tick, inside the parity bar |
| 3 | **The sun on the world.** Cascades from the static world, `I + D · S` in layer 3, the setting's value | GPU tests on an invented wall and occluder; captures differ from retail only near baked sun-shadow edges; the cost from `render.stats` against slice 0's estimate |
| 4 | **Objects cast.** Casters in the scene, skinned depth draws with blended rows | `sim.hashes` equal; a capture with an object's shadow on sunlit ground; the `alpha = 1` frame equal to the 30 fps frame |
| 5 | **Objects receive.** Read what the lighting record's inputs mean from the lit vertex programs, shadow the record's direct term per pixel, split the per-vertex buckets | a differential on invented records; captures of a character under a ledge |
| 6 | **Local lights.** A shadow map for every typed light, no cap on lights in view | shared with E3 lighting's per-pixel lights; `render.compare` with one light, shadow on and off |

## Risks and open choices

Risks, largest first:

1. **The split leaves visible ridges.** Lightmap texels are coarse, the
   palette has 256 colours a page, and the bake's blur is unknown. Slice
   1 exists to find this out before any renderer work.
2. **Double shadows.** A caster the bake saw and the trace does not
   leaves its baked shadow in `I`, and its live shadow then darkens `D`
   as well. Scenery is the likely case, which is why step 2 traces it
   and reports both fits.
3. **The model is wrong for a level**: emitters that are not on the
   disc near a shadow edge, two sky lights, a sun whose baked penumbra is
   wider than its disc. The report catches these; the level is refused.
4. **Objects lag behind the world.** Until slice 5 an object under a
   live shadow is lit as the lightmap sample under it says.
5. **Casters beyond the view.** Whether an unseen object's node matrices
   are current is **S**; if they are not, far casters pop.
6. **The frame may not fit.** The retail frame's GPU time at 4x on a
   base M3 is unknown.
7. **Expectation.** It is retail's art with a live sun, not a remaster,
   and interiors barely change.

Choices for the owner, with the design's pick:

| Choice | Options | Pick |
|---|---|---|
| What "dynamic" means | the hybrid split; our own bake (b1); runtime GI (b2, c) | the hybrid |
| The setting | a third value of the Lighting row; a row of its own | a third value |
| Edge filter in `dynamic` | soft, sized to the bake's penumbra; hard, as E3 | soft; hard stays `e3` |
| When the bake runs | at first load of a level, cached; only by the command | at first load, with the command for all maps at once |
| Where the runtime slices land | after FrameScene slice 6; on the tenant path at once | after slice 6 |
| A moving sun | not designed; designed | not designed |

## Not verified

- That subtracting a traced sun from a retail lightmap leaves a clean
  indirect term on any retail level. Nothing has been run.
- Every cost on this page. None was measured on an M3.
- The role of a sky light's colour, power, cutoff angle and flags in the
  bake, and that the `shad +0x0c` block holds the bake's emitter inputs.
- That the per-vertex colour buckets are scenery, which objects have
  them, and whether the bake counted scenery as casters.
- What the object lighting record's inputs mean.
- Whether node matrices of objects outside the view are current.
- The open wgpu Metal ray-query bugs, read through a third party.
- Halo 1 maps converted by `h1convert`: the split applies to any BSP
  with a lightmap and a sky light, and has not been considered further.

## Sources

Read 2026-11-20, all public, nothing copied, no leaked file opened.

- Hao Chen, "Lighting and Material of Halo 3", GDC 2008 slides (the Halo
  2 recap slide, through a search excerpt):
  <https://www.froyok.fr/blog/2024-01-breakdown-syndicate/resources/presentations/GDC_LightingMaterial_Halo3.pdf>
- Hao Chen and Xinguo Liu, "Lighting and Material of Halo 3", SIGGRAPH
  2008 course notes:
  <https://advances.realtimerendering.com/s2008/SIGGRAPH-Lighting%20of%20Halo%203.pdf>
- c20, H2V Tool (the `lightmaps` quality table):
  <https://c20.reclaimers.net/h2/tools/h2v-ek/h2v-tool/>
- c20, Halo 2 materials (lightmap-only, ignored by the lightmapper,
  resolution, power, half-life and diffuse scales):
  <https://c20.reclaimers.net/h2/source-data/h2-materials>
- wgpu PR 8071, ray tracing acceleration structures on Metal:
  <https://github.com/gfx-rs/wgpu/pull/8071>
- A third-party survey of wgpu ray-query maturity on Metal (the open
  bugs): <https://github.com/htarrido-picart/itsjustcad/issues/258>
- nelari.us, "A quick look at Apple Silicon's hardware accelerated ray
  tracing performance", 2024:
  <https://nelari.us/post/metal-raytracing-performance/>
- Geekbench Metal benchmarks (through a search excerpt):
  <https://browser.geekbench.com/metal-benchmarks>
- Wikipedia, Apple M3 (hardware ray tracing, through a search excerpt):
  <https://en.wikipedia.org/wiki/Apple_M3>
- Majercik and others, "Dynamic Diffuse Global Illumination with
  Ray-Traced Irradiance Fields", JCGT 8(2), 2019:
  <https://www.jcgt.org/published/0008/02/01/>
- Benoit Toth, "The state-of-art of Dynamic Global Illumination in
  video-games", CNAM, 2021 (leaks through thin walls, through a search
  excerpt).

## Related

[framescene.md](framescene.md), [native-renderer.md](native-renderer.md),
[native-world.md](native-world.md), [native-objects.md](native-objects.md),
[native-lights.md](native-lights.md), [native-shadows.md](native-shadows.md),
[e3-lighting.md](e3-lighting.md), [pstencil.md](pstencil.md),
[high-resolution.md](high-resolution.md),
[lighting.md](../formats/lighting.md),
[structure-bsp.md](../formats/structure-bsp.md),
[shader-instances.md](../formats/shader-instances.md),
[notes/e3-lighting.md](notes/e3-lighting.md).
