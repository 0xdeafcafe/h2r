# E3 lighting: measurements and prototype results

Evidence for [e3-lighting.md](../e3-lighting.md). Entries are dated, newest
first. Nothing here states what the code does today; the design page does.
The page's text was recorded at docs revision `af931e2fe`; the shadow
prototype is `crates/tools/viewer/src/bin/shadowlab` (last changed at `25c47e967`).
Both read the owner's disc at run time and write nothing from it into Git.

## 2026-11-20: object shadows, template coverage and attached lights

Read on the owner's Xbox disc at source revision `ad498abc7`. The
guest-record readers are in `b9344dc5e` and the template census in
`66892e770`. These are static counts. A template is counted once in a map
even when several shader instances use it.

**Coverage.** Of 2,581 map-local templates, 987 have a layer-10 model
depth writer and the same 987 have a BSP depth writer. In 975, a
layer-11 multiplying depth receiver and both writers occur at the same
LOD. Only 142 have all three at every LOD. Across maps these groups
represent 72, 69 and 15 distinct names respectively out of 165. This
includes unreferenced templates. The counts come from the template LOD
mask and pass ranges, the vertex-program reference, render states and
texture sources. A layer-11 alpha-only draw or a filtered-colour-mask
draw cannot qualify as a depth-map RGB receiver.

- `00a_introduction`: of 72 templates, 31 write depth, 30 pair at one or more LODs and 2 pair at every LOD.
- `01a_tutorial`: of 60 templates, 21 write depth, 21 pair at one or more LODs and 2 pair at every LOD.
- `01b_spacestation`: of 100 templates, 39 write depth, 37 pair at one or more LODs and 4 pair at every LOD.
- `03a_oldmombasa`: of 101 templates, 41 write depth, 41 pair at one or more LODs and 5 pair at every LOD.
- `03b_newmombasa`: of 107 templates, 46 write depth, 46 pair at one or more LODs and 6 pair at every LOD.
- `04a_gasgiant`: of 96 templates, 38 write depth, 37 pair at one or more LODs and 5 pair at every LOD.
- `04b_floodlab`: of 88 templates, 34 write depth, 34 pair at one or more LODs and 7 pair at every LOD.
- `05a_deltaapproach`: of 100 templates, 42 write depth, 42 pair at one or more LODs and 6 pair at every LOD.
- `05b_deltatowers`: of 99 templates, 40 write depth, 40 pair at one or more LODs and 6 pair at every LOD.
- `06a_sentinelwalls`: of 101 templates, 42 write depth, 40 pair at one or more LODs and 5 pair at every LOD.
- `06b_floodzone`: of 93 templates, 38 write depth, 38 pair at one or more LODs and 6 pair at every LOD.
- `07a_highcharity`: of 100 templates, 37 write depth, 35 pair at one or more LODs and 5 pair at every LOD.
- `07b_forerunnership`: of 104 templates, 42 write depth, 40 pair at one or more LODs and 6 pair at every LOD.
- `08a_deltacliffs`: of 87 templates, 30 write depth, 30 pair at one or more LODs and 5 pair at every LOD.
- `08b_deltacontrol`: of 107 templates, 41 write depth, 40 pair at one or more LODs and 6 pair at every LOD.
- `ascension`: of 78 templates, 28 write depth, 27 pair at one or more LODs and 5 pair at every LOD.
- `beavercreek`: of 80 templates, 28 write depth, 28 pair at one or more LODs and 4 pair at every LOD.
- `burial_mounds`: of 75 templates, 26 write depth, 26 pair at one or more LODs and 4 pair at every LOD.
- `coagulation`: of 79 templates, 27 write depth, 27 pair at one or more LODs and 4 pair at every LOD.
- `colossus`: of 78 templates, 29 write depth, 29 pair at one or more LODs and 5 pair at every LOD.
- `cyclotron`: of 81 templates, 29 write depth, 29 pair at one or more LODs and 4 pair at every LOD.
- `foundation`: of 81 templates, 27 write depth, 27 pair at one or more LODs and 4 pair at every LOD.
- `headlong`: of 87 templates, 33 write depth, 33 pair at one or more LODs and 5 pair at every LOD.
- `lockout`: of 77 templates, 29 write depth, 29 pair at one or more LODs and 4 pair at every LOD.
- `mainmenu`: of 49 templates, 17 write depth, 17 pair at one or more LODs and 2 pair at every LOD.
- `midship`: of 75 templates, 27 write depth, 27 pair at one or more LODs and 4 pair at every LOD.
- `shared`: of 72 templates, 26 write depth, 26 pair at one or more LODs and 4 pair at every LOD.
- `single_player_shared`: of 85 templates, 33 write depth, 33 pair at one or more LODs and 5 pair at every LOD.
- `waterworks`: of 84 templates, 34 write depth, 34 pair at one or more LODs and 6 pair at every LOD.
- `zanzibar`: of 85 templates, 32 write depth, 32 pair at one or more LODs and 6 pair at every LOD.

**Attachments.** Every map has both `flashlight_1p` and `flashlight_3p`
on the Chief's flashlight marker. The 12 multiplayer maps and the two
shared caches also attach both to the multiplayer biped. That gives
two attachment relationships per campaign or menu map and four per
multiplayer or shared map, 88 in total. All 14 multiplayer or shared
maps have one Warthog and one Scorpion headlight attachment each. In
campaign, `01b_spacestation` and `03a_oldmombasa` have the Warthog
only. `03b_newmombasa`, `05a_deltaapproach` and `06b_floodzone`
have both. The other campaign maps and the menu have neither.
The totals are 19 Warthog attachments and 17 Scorpion attachments.
Every attached beam is a gelled frustum flagged against shadow casting.
The unsuffixed flashlight tag on `04a_gasgiant` and `04b_floodlab`
has no placement, attachment or effect-part source in either map.

**Shadow direction and gate.** The ordinary object-mask draw uses a
normalized caster lighting direction. The depth receiver compares the
vertex normal with its opposite, towards the source. All six populated
classifications of the multiplying receiver write this dot product to
diffuse RGB. Classification 0 uses the incoming normal directly and the
model programs first transform it by the rigid or skinning rows. The
combiner blends the projected comparison with unity by the interpolated
vertex colour, then applies the strength constants and corner fade.
All-zero vertex weights leave the destination RGB untouched. In the
one- and three-tap alpha-only receivers the vertex alpha is the
homogeneous dot of the posed position with hardware `c[36]`. The caster's final fade
includes the lighting sample, object fade and history fade. The creature
ground shadow bypasses that calculation. [Object chain](../native-shadows.md#object-chain).

**Reproduction.** Use a CLI containing the template census. The validator
walks all disc maps including the two shared caches. The individual material
command shows the vertex-program name, render state and first RGB combiner
of each implementation.

```sh
h2 shader --all all --disc "$H2_DISC"
h2 shader zanzibar 'objects\weapons\rifle\battle_rifle\shaders\battle_rifle_lod' --disc "$H2_DISC"
```

Extract the shader corpus to private scratch. In the generated manifest,
column four identifies the vertex program and classification. Inspect the
six files for the multiplying receiver and compare the upload of hardware
constant 41 with the shadow direction. The same procedure on the rigid
`lightmap_perpixel_tex_detail` program shows that the tangent frame rotates
into world space before the dot with incident input `v5`.

```sh
h2 extract-shaders --disc "$H2_DISC" --out "$TMPDIR/e3-vsh"
awk -F '\t' '$4 ~ /shadow_buffer_application_lightmap:[0-9]+$/ {print $1}' "$TMPDIR/e3-vsh/manifest.tsv" |
  while read -r file; do cargo run -p shaders --example corpus -- "$TMPDIR/e3-vsh" --dump "$file"; done
h2 disasm 0x00048ee0 110
```

For attachments, enumerate map names with `h2 maps "$H2_DISC"`. Run
`h2 lights <map> --disc "$H2_DISC"` for each. Count attached-to lines
under each definition, keeping the marker and owner together. Use
`h2 effect <map> '<light tag name>' ligh --disc "$H2_DISC"` to check
the gel reference and flags. For the camera/owner test, volume shape,
caster gather and both ranking budgets, read these entries from the same
disc. The disassembler reads `H2_DISC`.

```sh
h2 disasm 0x00031520 40
h2 disasm 0x0003e9c0 40
h2 disasm 0x000c17f0 410
h2 disasm 0x00132190 750
h2 disasm 0x00031910 470
h2 disasm 0x0004a7c0 900
h2 disasm 0x000335e0 43
h2 disasm 0x00033400 118
h2 disasm 0x00032350 410
h2 disasm 0x0003d7f0 208
h2 disasm 0x0003dc90 38
h2 disasm 0x00032ce0 395
```

### Dynamic-lighting census on the same disc

The installed lighting validator completed all 30 maps with zero failures.
It counted 78 lightmap groups, 13,658 pages and 5,196,110 vector entries
in lightmap buckets. That bucket count is independent of which vertices
the world geometry references. The validator used for this reading did
not emit per-BSP densities or angular shares.

The measurement command at `4ccd7f264` reports the page-size histogram,
density, cluster skies, sky-light flags and alignment per BSP. The
matching, denominator and checksum rules are in
[the lighting format](../../formats/lighting.md#cli-validation-and-limits).
The per-BSP output from that revision has not been measured here.

```sh
cargo run -p cli --bin h2 -- lighting --all --disc "$H2_DISC" > "$TMPDIR/e3-lighting-census.txt"
```

## 2026-11-20: budget arithmetic for the native design

Read at `00dd7430f`. No run: arithmetic on the 2026-11-16 prototype results
below, for the "Budget" table on the design page. Every figure is an
estimate until slice L4 of the design measures a frame on the lab laptop
(`h2 lab run --env H2_LIGHTING=e3`, the design page's "Slices").

- **Machine factor.** The prototype ran on an M1 Max and the target is an
  M3. The M3's GPU is taken as half as fast, so prototype times are
  doubled. That factor is an assumption, not a measurement.
- **Sun.** The prototype's one-tap sun at 4x cost 1.13 ms against 0.16 ms
  with no shadow: 0.97 ms for one 8192-square map (2048 times the scale)
  of four casters and a tap over every pixel. Doubled, 1.9 ms. The
  design's tiles total at most a 4096-square atlas, a quarter of those
  texels, and a still caster's tile is not redrawn: 0.4 ms for tiles and
  0.6 ms for receivers, which are drawn only where a shadow can fall.
- **Local lights.** The prototype's point light at 4x cost 2.93 ms against
  0.13 ms: 2.8 ms for six 4096-square faces, 0.47 ms a face. A cone has
  one face and the design gives it a 1024-square tile, a sixteenth of the
  texels. Doubled and with the receivers' marks, 0.2 ms a light: 0.8 ms
  for four.
- **Lit lights.** Nothing was measured. One additive light pass over the
  whole 2560x1920 frame is taken as 0.5 ms on the M3; 16 lights at a mean
  of an eighth of the screen come to 1.0 ms.
- **Not in the arithmetic.** The prototype shades flat, draws unposed
  models and has no materials, so alpha-killed casters, skinning and the
  receivers' real programs are all unmeasured.

## 2026-11-20: Sev's shadow-apply shaders

Method: `777Sev777/Halo-2-Custom-Shaders` at its head `9be89edf` (17 July
2025), cloned into a temporary directory outside the repository, read and
deleted. Public, used as reference only; nothing of it is copied here.
Our tree was at `00dd7430f`. To repeat: clone it the same way
([pstencil.md](../pstencil.md), "Fetching the study copies") and read the
eight pixel shaders and the README.

- Eight files: a Retail set and an E3 set, each in a Full and a
  Performance version, each replacing the kit's two shadow-apply passes
  (its one-sample and three-sample slots).
- Every file does the same lookup: the receiver's projected depth, less a
  small constant bias, compared against the map at a five-by-five grid of
  texel offsets, each offset jittered by a fixed Poisson set (12 points in
  Full, 4 in Performance), and the passes averaged. The map is assumed
  1024 square. So Full makes 300 comparisons a pixel and Performance 100.
- After the lookup, every file fades the term by a constant, multiplies
  by a fog term from a second texture, and writes zero where the vertex
  alpha is at or below half.
- The two modes differ only after the lookup. The soft mode blends the
  filtered term with the fade as it is. The sharp mode widens the kernel by
  a fifth and applies the fade with a gain around half coverage, so a
  pixel under about half lit goes fully dark and one a little over goes
  fully lit: a steep ramp across a filtered edge.
- The E3 set is the same two files in the opposite slots, for that tag set.
- The README names the sharp mode "Very Sharp, Aliasing-free,
  Stencil-like", and says the shaders are not a replacement for
  lightmapping.

What it changes here: pstencil.md had read the sharp mode as a single tap.
At this head it is not, in any file. The restoration's "stencil-like" edge
is a filtered edge made steep, which is why it shows no texel steps on a
1024-square map. The design's Sharp form takes the idea with four
comparison taps and a ramp one pixel wide. The vertex-alpha gate is
evidence (**C**, for the MCC port) that the receiver pass switches itself
off per vertex, which the design relies on for self-shadowing and slice L0
checks in the Xbox programs.

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


`crates/tools/viewer/src/bin/shadowlab` renders one scene with each shadow
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

## 2026-11-20: L1 sharp shadow edge

Commit: `Add the Sharp shadow comparison` (resolve with
`git log -1 --format=%H -- crates/formats/shaders/tests/golden/shadow_sharp.wgsl`).

Reproduce with `cargo test -p shaders --test shadow` on a GPU adapter.
The golden test covers all three forms. The GPU test sweeps subpixel
positions along a synthetic caster at map-to-screen ratios 1:1, 1:2 and
1:4. It checks a ramp at most 1.5 display pixels wide and a half-coverage
centre within one map texel of the outline. The code-only slice records
acceptance bounds. GPU results are pending coordinator verification.

## 2026-11-20: L2 light candidates

Commit: `Extract the full light candidate set into FrameScene` (resolve
with `git log -1 --format=%H -- crates/rewrites/scene-extract/src/lights.rs`).

Run `cargo test -p scene-extract lights::tests` for the synthetic guest.
It has 20 visible candidates and five recorded guest selections, with
a mixture of spheres and no-shadow cones. The fixture compares sealed
scene hashes in separate processes with each lighting setting.

For the in-game check, use the same image, snapshot and recorded inputs
on the lab laptop with `H2_LIGHTING=retail` and `H2_LIGHTING=e3`.
Keep the native renderer enabled. Take `h2 agent render.scene` at
matching ticks and compare `tick`, `hash` and the per-view counts
`lights_candidates`, `lights` and `lights_with_constants`. Compare
`h2 agent sim.hashes '{"from_tick": T, "max": N}'` over the same completed
tick interval in both runs, substituting the first tick and row count
for `T` and `N`.

For extraction time, take `render.scene` before and after the same
gameplay interval in each mode. Divide the increase in
`counters.scene_light_extract_ns` by the increase in
`counters.scene_light_extract_views` for mean nanoseconds per view.
Record `scene_light_extract_unread` alongside it. Game hash and timing
results are pending coordinator verification.
