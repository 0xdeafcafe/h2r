# E3 lighting: measurements and prototype results

Evidence for [e3-lighting.md](../e3-lighting.md). Entries are dated, newest
first. Nothing here states what the code does today; the design page does.
The page's text was recorded at docs revision `af931e2fe`; the shadow
prototype is `crates/viewer/src/bin/shadowlab` (last changed at `25c47e967`).
Both read the owner's disc at run time and write nothing from it into Git.

## 2026-11-16: the Lighting setting in levels

Method: `h2 run` with `H2_BOOT_GFX` at `H2_RENDER_SCALE=2`, retail and E3
lighting, the first level (Old Mombasa, `03a_oldmombasa`, to present 4087) and
The Heretic. Reproduce with `H2_BOOT_GFX_SHADOWS=N` and
`H2_BOOT_GFX_LIGHTING_AB=1` (see "The Lighting setting" on the design
page). Machine: the owner's Mac, Metal.

The first level runs, Old Mombasa
(`03a_oldmombasa`, to present 4087) and The Heretic, at 2x, showed every
Old Mombasa frame byte for byte the same with and without E3. That is the
frames, not the path:

- Layers 10 and 11 are the game's only shadow-map path. Shadow generate
  draws into the 512x512 `LIN_D24S8` level with the 128x128
  `LIN_A8R8G8B8` target as its colour; shadow apply draws the same
  casters' receivers into the frame and samples the map (an old draw log
  has them in pairs of equal vertex counts). Old Mombasa: 5,217 draws into
  128x128, 5,217 draws with a `LinD24S8` texture, 5,123 repeated into the
  view's copy, 109 scaled textures sampled against the device's 111
  in-place depth textures (one per shadow pass), no refill. The Heretic:
  1,260, 1,260, 90 and 90. Every shadow pass the game made was drawn and
  sampled at 2x with one tap.
- The 160x120 and 320x240 targets are the colour levels of the pool over
  the shadow map (bloom and the like), not shadows. The map goes through
  the alias mirror between frames (438 stores, 436 loads), which marks the
  view's copy stale; the next shadow pass clears it whole first, so it is
  never refilled.
- About 110 shadow passes in 2,340 frames of the level: the dynamic lights
  of the opening cinematic. The run wrote one frame in 250, so its nine
  in-level frames most likely held none. The outdoor shadows of Old
  Mombasa (buildings, vehicles on the road) are in the lightmaps (layer 3,
  "What replaced it"), which phase 1 does not touch; sun shadows from
  objects are phase 4. The Heretic's ten differing frames differ in small
  spots around the cinematic's lights; whether those frames held a shadow
  pass was not recorded.

`e3_shadow_maps_survive_halos_render_target_pool` (render_scale.rs) runs
that order for three frames: shadow pass, a colour level over the map's
bytes drawn, swap. The map goes through the mirror every frame, and the
last frame's E3 image is the single-pass scene's exactly, at 1x, 2x and
4x; it differs from retail's at the shadow's edges and nowhere else
(within two map texels of the caster's outline); the guest's bytes and the
reference's frame are the plain backend's.

To catch the frames that do have shadows, the draw diagnostics list them
(`shadow-map frames K  a-b, c, ...`, frames that drew with a depth texture
bound) and `H2_BOOT_GFX_SHADOWS=N` writes the first N of them, the same
frames in a retail and an E3 run.

The first such run (to present 4000) listed 23 frames, `1745-1747,
3981-4000`, and all 21 pairs were byte for byte the same in retail and E3.
They were also entirely black, at 2x and at 1x, in both runs: the first
frames after the level loads, still faded out, and a cut to black in the
opening cinematic, under which the game still draws its lights' shadow
passes (21 of them sampled the 2x copy). So `H2_BOOT_GFX_SHADOWS` now skips
all-black frames and keeps looking.

Run to present 8000, the lit frames differed between retail and E3, and
between two retail runs just as much, 1x frames included: this deep, two
headless runs are not the same run. Polled visibility tests answer
"incomplete" depending on host timing (d3d.md, "Visibility tests"; the
runs' `visibility_incomplete` counts differ), and the scheduler's slices
and preemptions differ too. So the comparison is made inside one run:
`H2_BOOT_GFX_LIGHTING_AB=1` (`d3d::LightingAb`) puts a second view over the
scaled backend, using it as its reference. The inner backend answers the
guest from its 1x reference and draws the retail view; the outer view draws
the same calls at the same scale with E3. Each captured frame is written as
`frame-N-retail.png` and `frame-N-e3.png`.
`the_lighting_ab_draws_retail_and_e3_from_one_run` (render_scale.rs) runs
the pool order through it at 2x and 4x. Each image is byte for byte what
a run in that mode alone draws, the two differ only within two map texels
of the caster's outline, and the 1x reference and the guest's bytes are
the plain backend's.

The first A/B run (to present 8000, pairs from frame 4012) had all 40
pairs byte for byte the same, with the E3 view drawing 273,913 shadow draws
and sampling 4,021 2x maps. Those frames are a blurred cinematic shot:
each view's image is the 1x reference's, every pixel repeated 2x2
(against a nearest upscale of `frame-N-1x.png` the mean difference is
below 0.2 of 255). The frame is covered, opaque, by a pass upsampling a
small pool level, which keeps its guest size in both views
([high-resolution.md](../high-resolution.md), fixed-size targets), so the
shadows under it never reach the screen in either.
`a_frame_covered_by_a_small_level_shows_no_lighting_difference`
(render_scale.rs) reproduces that: the 2x map drawn and sampled every
frame, and the two views the same. The A/B now compares the views itself
on every shadow-map frame, writes the first `H2_BOOT_GFX_SHADOWS`
(default 40) pairs that differ, and reports `lighting A/B: K of M
shadow-map frames differ between the views (first: ...)`.


## 2026-11-16: shadow prototype (volumes against shadow maps)


`crates/viewer/src/bin/shadowlab` renders one scene with each shadow
technique, offscreen with wgpu, and times them. It reads the owner's disc at
run time and writes only into a directory outside the repository; it refuses
an output path inside it. Nothing it writes from the disc is committed. The
numbers below are filled in by hand from a run.

```sh
cargo run -p viewer --release --bin shadowlab -- --list-models <map>
cargo run -p viewer --release --bin shadowlab -- --out "$OUT" \
    --model <mode tag> [--model <mode tag>@x,y,z[,yaw]]... [--level-casts] <map>
cargo run -p viewer --release --bin shadowlab -- --out "$OUT" --synthetic
```

### What it does

- **Scene.** The scenario's structure BSP (`--bsp N`) as the receiver:
  clusters plus placed instances, flat shaded. Casters are render models
  (`--model`, default level of detail or `--lod N`), stood on the level near
  its centre or placed at a given point. With `--level-casts` the BSP casts
  too, in every technique. `--synthetic` swaps in invented geometry (a cube,
  an open box and a plane with a hole over a ground plane).
- **Lights.** A sun (`--sun YAW,ELEV`) and a point light (`--point`, with a
  range for falloff). The default camera and point light are framed on the
  casters; the default sun is fixed (yaw 35, elevation 50) and not yet read
  from the `sky` tag.
- **Volumes.** Caster triangles are welded by position, so material and
  normal seams do not read as holes, and an edge table is built once. Per
  light, the volume is the lit faces' prisms with shared sides cancelled:
  sides on the silhouette and on lit faces' open edges, the lit faces as the
  near cap and their copies at infinity as the far cap. That volume is closed
  whatever the caster's topology, so depth-fail counting is always well
  formed; open geometry costs extra side quads instead of leaking. Rendered
  with an infinite far plane: ambient pass, two-sided stencil pass, additive
  light pass on stencil 0.
- **Shadow maps.** The sun gets one orthographic map fitted to the view
  frustum out to 2.5 times the target distance, with its depth range
  stretched over every caster, so casters outside the view still cast. A
  point light gets six cube faces. The map edge scales with the resolution
  (default 2048 at 1x for the sun, 1024 per cube face). Sampled with one
  nearest comparison tap (`map-hard`) or nine bilinear comparison taps
  (`map-pcf`), with slope-scaled bias and a normal offset of 1.5 texels.
- **Output.** For each light and scale (default 640x480 times 1, 2 and 4):
  each technique's image, a grid (volume, map-hard, map-pcf, and the
  volume-against-map-hard difference), `report.md` and `timings.csv`. Times
  are GPU timestamps at pass boundaries, median of `--frames` (30) frames,
  or wall time per technique where the adapter has no timestamp queries (the
  report says which).

### What the invented scenes show

These come from the tests (`cargo test -p viewer --bin shadowlab`), which use
invented geometry only.

- A closed cube's volume has a four-edge silhouette under a vertical sun and
  is closed under any light. Depth-pass counting fails with the eye inside a
  volume; depth-fail does not, but needs both caps.
- Open edges are counted per mesh (open, non-manifold, winding flips, closed
  pieces). An open box and a plane with a hole still give closed volumes.
- Where light enters an open mesh and meets a face turned away from it (the
  inside of the open box's far wall), the volume lets it through and the map
  shadows it. That is the open-mesh artefact to look for in the difference
  images: cyan in the grid.
- Elsewhere the volume and the one-tap map agree on almost every pixel.

### Results

One run, 2026-11-16: `--model` Master Chief, Elite, Ghost and Banshee on
Zanzibar's BSP 0, level not casting, default sun and point light, 640x480
times 1, 2 and 4. Apple M1 Max (Metal), GPU timestamps, median of 30 frames.
The images and `report.md` stay outside the repository.

Geometry (welded by position):

| Mesh | Triangles | Open edges | Non-manifold | Closed pieces | Closed triangles |
|---|---:|---:|---:|---:|---:|
| Level clusters | 12,462 | 146 | 1,114 | 0 of 28 | 0% |
| Level instance definitions | 3,727 | 35 | 624 | 55 of 145 | 64% |
| All casters, every level of detail | 8,690 | 308 | 170 | 16 of 93 | 37% |

Per caster the share of closed triangles runs from 0% (Master Chief's lowest
level of detail, Elite's two highest) to 100% (Master Chief level 1); the
Banshee has 204 open edges at every level.

Volumes:

| Light | Silhouette edges | Side quads | Of which open | Volume triangles | CPU build ms |
|---|---:|---:|---:|---:|---:|
| Sun | 2,487 | 2,511 | 238 | 14,596 | 0.25 |
| Point | 2,407 | 2,430 | 229 | 14,228 | 0.27 |

GPU ms, total per technique:

| Light | Scale | none | volume | map-hard | map-pcf |
|---|---:|---:|---:|---:|---:|
| Sun | 1x | 0.11 | 0.71 | 0.25 | 0.37 |
| Sun | 2x | 0.16 | 1.19 | 0.78 | 1.65 |
| Sun | 4x | 0.16 | 1.11 | 1.13 | 2.67 |
| Point | 1x | 0.10 | 1.12 | 0.76 | 0.60 |
| Point | 2x | 0.16 | 3.26 | 2.66 | 4.60 |
| Point | 4x | 0.13 | 3.33 | 2.93 | 6.26 |

Agreement, as a share of pixels facing the light:

| Light | Volume vs map-hard | Volume vs map-pcf | Notes |
|---|---:|---:|---|
| Sun | 0.28%, 0.15%, 0.08% at 1x, 2x, 4x | 0.26%, 0.14%, 0.08% | more pixels shadowed only by the volume than only by the map at every scale |
| Point | 0.09%, 0.05%, 0.03% | 0.09%, 0.05%, 0.03% | as above |

### Deciding

- **Cost.** The stencil pass against the map passes, and how each grows from
  1x to 4x: volume fill grows with pixels, map cost with the map edge.
- **Geometry.** Open edges and the share of closed triangles per caster; the
  side quads on open edges are the extra fill open meshes cost. Per level of
  detail, whether silhouettes change enough to pop.
- **Look.** Red and cyan in the grids: red is shadow only the volume casts,
  cyan only the map. Edge acne or peter-panning in `map-hard`, and how far
  `map-pcf` softens the hard edge the E3 look wants.
- **Not covered yet.** Alpha-tested casters (grates, foliage) are drawn
  solid by both techniques: the prototype does not read materials. Poses: models are
  drawn unposed, as the model viewer draws them. Cascades: one sun map only.

**Decided: shadow maps, one tap.** Volumes cost as much as a one-tap map or
more at every scale measured, Halo 2's casters are mostly open meshes (37% of
caster triangles closed, the level's clusters none), and the two agree on all
but a fraction of a percent of lit pixels. A study note that recommended
depth-fail volumes, and why it is not followed, is in
[pstencil.md](../pstencil.md) ("The technique: why shadow maps"). Volumes for
closed object casters stay the fallback if phase 4's pictures show a case a
map cannot meet.
