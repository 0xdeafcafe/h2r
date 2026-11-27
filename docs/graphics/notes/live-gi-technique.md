# Live GI: technique costs and sources

Evidence for [live-gi.md](../live-gi.md), "Technique choice". Nothing here states what the code does; the design page does.

## 2026-11-20: techniques priced for an M3 at 4x

Read from the docs and the `wgpu` 30.0.2 sources at repository revision `da07031d0`. Nothing was run; every cost is an **Estimate** or **Guess** as labelled. To reproduce the registry reads, open `wgpu-hal`, `wgpu-core`, `wgpu-types` and `naga` 30.0.2 at the paths named.

### Costs

Costs are per display frame on a base M3 at render scale 4 (2560x1920,
4.92 M pixels) and 60 fps. None is measured on an M3. The bases:

- **A texture tap over the whole frame costs about 0.44 ms**
  (**Estimate**): shadowlab on an M1 Max at 4x took 2.67 ms with nine
  shadow taps and 1.13 ms with one ([notes/e3-lighting.md](e3-lighting.md)),
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
  [notes/e3-lighting.md](e3-lighting.md)). Tracing it once is
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

## 2026-11-20: wgpu's ray queries on Metal

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
