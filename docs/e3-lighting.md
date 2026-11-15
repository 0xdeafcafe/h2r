# E3 lighting: stencil shadows and dynamic lighting

A design for an opt-in lighting mode in the spirit of the Halo 2 E3 2003 demo:
hard-edged shadows from every direct light, cast by and onto everything, and
per-pixel lighting with normal maps. Nothing here is built.

The short version: **the evidence says there is nothing of the cut renderer to
switch back on in the retail game.** What retail kept is the vocabulary (a
light flag still called "stencil" in the official tools) and a shadow-buffer
system that took its place. So this is a new host-side renderer feature that
reads the retail scene, not a recovery. The README's word "recovering" should
be read that way.

It was written from the code and docs on master and from public sources,
without building or running the game. No leaked build, game file or archive
was downloaded or examined, and none may be for this work.

The setting is `lighting = "retail" | "e3"` in the settings design
(`docs/settings-menu.md`, in progress on another branch). This page proposes
`H2_LIGHTING` as the developer flag.

## Evidence labels

- **P**: a developer's own statement, with the source. **P\*** means we read
  it through a secondary quotation (a wiki, a mirror or a search excerpt)
  because the original was unreachable, so the wording is unverified.
- **C**: a community finding or a secondary characterisation.
- **R**: read from the retail image or disc by this project, in the doc
  named.
- **S**: our inference or speculation.

## What the E3 2003 demo did

| Claim | Label | Source |
|---|---|---|
| "pstencil" began as Bernie Freidin's research project after Halo 1: stencil shadows added to the Halo engine. The Halo 2 renderer was being designed around it, with Adrian Perez and Chris Butcher. He also mentions new per-pixel effects and approximating real lighting in the pixel shader. | P | Xbox.com profile, 2003, mirrored at <https://halo.bungie.org/misc/xbox.bfreidin/> |
| It handled direct light sources; lightmaps still supplied ambient light. | P\* | Adrian Perez, via <https://www.halopedia.org/Pstencil>, citing Vice and Bungie.net's "One Final Effort" |
| The aim was one stencil-based model for all lighting, fully dynamic, inspired by SIGGRAPH papers. | P\* | Marty O'Donnell and Jaime Griesemer in Vice's oral history, <https://www.vice.com/en/article/the-complete-untold-history-of-halo-an-oral-history/> |
| It was tested with weapon-fire and grenade lighting on a night version of a Halo 1 map, in January 2003. | P\* | Perez, via Halopedia citing Bungie.net |
| Visible in the demo: sharp sun shadows, self-shadowing on characters and vehicles, shadow-casting street lamps, headlight and muzzle-flash lighting. | C | <https://www.halopedia.org/Halo_2_E3_demo> |
| "Doom 3-style" unified lighting. | C | Wiki and forum wording. No developer statement using it was found. |
| Normal maps and specular under those lights, light counts, the shader maths. | None | No developer statement was found. Retail has bump mapping regardless (below). |

**Whether the E3 demo itself ran pstencil is disputed.** Perez and O'Donnell
place it in the demo. Marcus Lehto and Nate Walpole say it was dropped after
the earlier announcement trailer (posts cited on Halopedia's Pstencil page).
Press on a December 2025 leak says a build dated 3 May 2003 includes the
pstencil system and that the complete demo is a build dated 11 May; it does
not say outright that the demo build runs pstencil
(<https://www.vice.com/en/article/halo-2s-legendary-e3-demo-has-leaked-after-22-years/>).
We have not examined that material and will not. For this design it does not
matter which account is right. The target is the look, described in the
table above.

## Why it was cut

| Claim | Label | Source |
|---|---|---|
| The whole graphics engine was thrown away because a game could not ship on the Xbox with it; it managed a five-minute demo, not Halo-sized environments. | P\* | Chris Butcher, Eurogamer, 2010, <https://www.eurogamer.net/articles/bungie-halo-2-e3-demo-was-a-fake>, read through quotations |
| In the best case it struggled to hold 30 fps, and shipping it meant large cuts to geometry and lighting. | P\* | Perez, Bungie.net "One Final Effort", via Halopedia |
| It allowed one or two characters on screen and sometimes fell to about 3 fps. It did not work in exteriors, so newer hardware would not have saved it. | P\* | Griesemer, Vice oral history and a later post, via Halopedia |
| The Pelican's shadow volumes were a known cost during demo work. | P\* | The making-of documentary, via <https://www.halopedia.org/Behind_the_Scenes:_Making_of_Halo_2> |
| Fill rate was the specific bottleneck. | S | No developer names it for pstencil. It is the textbook cost of shadow volumes, and the official tools say retail dynamic lights are "predominantly fillrate bound" (below). |
| Memory was the main problem. | C | Forum recollection only. |

## What replaced it

Perez describes a return to Halo 1's approach: lightmaps plus shadow maps
(**P\***, as above). A Bungie GDC 2008 talk, "Lighting and Material of Halo 3"
(Hao Chen), is reported to describe Halo 2 as lightmap colour per pixel with
one dominant light direction per vertex for bump mapping (**P\***, read as a
search excerpt only).

What this project has read from the retail game agrees with that:

| Finding | Label | Doc |
|---|---|---|
| Lightmaps are palettised index textures, selected per structure render key, with the palette bound at stage 3. | R | [lighting.md](lighting.md) |
| Lightmap buckets carry per-vertex colour streams and a per-vertex signed lighting vector (stream type `30`), which the engine interpolates. Its downstream use is not reconstructed. | R | [lighting.md](lighting.md) |
| Lightmap groups hold nine-term spherical-harmonic probes per colour channel, contracted to per-vertex colour by `0x00023ca0`. | R | [lighting.md](lighting.md) |
| Render geometry always has a tangent-frame stream, so tangent-space normal maps are in retail. | R | [map-format.md](map-format.md), [structure-bsp.md](structure-bsp.md) |
| Shadows are depth textures: the first level binds a depth level as the depth target, then samples it as `LIN_D24S8` with the NV2A's hardware comparison (`D3DRS_SHADOWFUNC`). It also draws into 512x512 and 128x128 targets. | R | [d3d.md](d3d.md), [shaders.md](shaders.md) "Shadow maps" |
| `sky` tags carry lights with yaw and pitch, and a fallback object-lighting colour. | R | [lighting.md](lighting.md) |
| There are 1,596 `ligh` tags across the maps. Their flags are recorded as values only, and the light render code has not been located. | R | [effects.md](effects.md) |

## What survives

| Finding | Label | Source |
|---|---|---|
| The official MCC Halo 2 tools document a light flag named "no stencil shadow", meaning the light casts no shadows. The same page describes shadow buffers with a resolution ("shadow quality bias"), antialiasing taps, a specular pass that doubles the cost, and a guideline of at most about four modest lights in view. | P (343 Industries documentation) | <https://learn.microsoft.com/en-us/halo-master-chief-collection/h2/guides/guidedynamiclights> |
| So the name "stencil" survived on a shadow-buffer implementation. | S | Follows from the row above. That page describes the MCC toolset; that the Xbox retail `ligh` flags match it bit for bit is not verified. |
| Modders report that the tools say "stencil" but produce shadow maps, and that true stencil lighting cannot be restored without engine source. | C | A Steam Workshop discussion, read as a search excerpt only |
| The community restoration of the E3 level in MCC says its lights are "not pstencil", and points to replacement pixel shaders with a sharp one-tap mode and a soft three-tap mode over the existing shadow-map path. | C | <https://github.com/digsite/H2E3>, <https://github.com/777Sev777/Halo-2-Custom-Shaders> |
| That level was rebuilt several times because pstencil-era assets and scripts lost function in the retail engine. | P\* | Ken Peters, 2024, via Halopedia's E3 demo page |
| Most of the E3 demo's light objects remain in retail with shadow casting off. | C | Halopedia's E3 demo page, citing developer commentary; not checked against tags |
| A pre-release build from May 2003 contains pstencil code. | C, unverified by us | Vice's report on the 2025 leak (above). The same is reported of <https://tcrf.net/Proto:Halo_2>, which could not be read. This says nothing about retail. |
| The retail game sets stencil render states from game code: `SetRenderState_StencilEnable` has 40 call sites in 30 functions, and `StencilFail` 15 in 6. | R | [d3d.md](d3d.md), inventory |
| What those sites are for is not traced. Stencil has many uses (masking decals, portals, the HUD), so this is not evidence of shadow volumes. | S | |
| Geometry sections hold position, texture coordinate, tangent frame and lightmap coordinate streams, plus a few stream types we have not interpreted. Nothing identifies any of them as shadow-volume geometry. | R | [structure-bsp.md](structure-bsp.md) |
| Shader instances have fields no decoded engine path reads (`shad +0x0c`, `+0x2c`, `+0x34` and others). | R | [shader-instances.md](shader-instances.md), "Open items" |
| Whether any of those unread fields relates to the cut renderer. | S | Unknown. Unread is not the same as vestigial. |
| Halo-2-HD's README and write-up describe render targets and resolution only. They say nothing about stencil use or the lighting pipeline. | C | <https://github.com/grimdoomer/Halo-2-HD> |
| Halo 2 Vista restoring any of it. | None | No claim was found either way. |

**Not found, after looking:** unused shader templates or passes for shadow
volumes; tag fields for volume geometry; a dormant stencil-lighting path in
retail; any technical write-up of the original renderer.

### What that means

- **Re-enable:** only the retail dynamic-light features that exist and are
  switched off or turned down per light. If the community claim holds, some
  lights have shadow casting disabled by flag. Those shadows would be retail
  shadow buffers, not stencil volumes.
- **Newly implement:** everything that makes the E3 look. Shadow volumes or an
  equivalent, per-pixel lighting from those lights, and the scene knowledge
  both need.

Two static checks would firm this up at low cost, and are phase 0 below:
trace the stencil call sites to name each use, and find the light render path
so the `ligh` flags can be read off engine code.

## Goals and non-goals

Goals:

- hard, per-pixel shadows from the sun and from dynamic lights, including
  self-shadowing on characters and vehicles;
- per-pixel diffuse and specular from dynamic lights using the materials'
  normal maps;
- off by default, and incapable of changing the simulation.

Non-goals:

- reproducing the E3 demo's level, scripts or assets. Only the retail disc's
  content is in scope;
- matching the original renderer's maths, which is not public;
- re-lighting the environment's baked lighting in early phases (see "The
  lightmap problem").

## Design

### Where it runs

The device (`d3d::Device`) and everything the guest can observe stay as they
are. [high-resolution.md](high-resolution.md) splits the backend in two: a
1x *reference* that answers every read the guest makes, and a *view* that
draws what is shown. E3 lighting lives only in the view. That one decision is
what keeps the simulation untouched: the guest's D3D calls, their results,
the write-backs and the pixel counts all come from a backend that does not
know this mode exists.

This design therefore depends on phase 1 of that one.

### What the view needs to know

The view sees `ResolvedDraw`s: streams, state, textures and vertex-program
constants. A lighting pass needs more than that, and it comes from
**observers**: read-only hooks at guest addresses, run on the runner thread
at the moment the guest reaches them.

| Need | Source | State |
|---|---|---|
| Camera and projection for the view being drawn | view records built by `0x002234e0` and used by `0x0002b790` ([engine-map.md](engine-map.md)) | Functions located; record layout not decoded |
| Which material and pass each draw belongs to | `0x0001bbf0` begins a shader draw and passes (shader, LOD, layer, pass) to `0x00016b90` ([shader-instances.md](shader-instances.md)) | Decoded |
| Which geometry each draw belongs to | `0x00041040` branches between structure clusters and render models ([engine-map.md](engine-map.md)) | Located, marked "likely" |
| The sun's direction and colour | `sky` lights ([lighting.md](lighting.md)) | Angles decoded; colour and the rest not |
| Dynamic lights: position, direction, cone, colour, range | the guest's live light data, and `ligh` tags ([effects.md](effects.md)) | Not located. The largest gap. |
| Object poses and skinning matrices for shadow casters | the guest's object and render-cache data ([objects.md](objects.md), `0x0003d270`) | Not decoded for this use |
| Level and model geometry, including what the camera cannot see | tags: `tags::structure`, `tags::geometry`, `tags::model`; collision in [collision.md](collision.md) | Decoders exist |
| Which bitmap parameter of a material is its normal map | template and pass bindings ([shader-instances.md](shader-instances.md)) | Bindings decoded; meanings not named |

Observer rules:

- An observer reads guest memory and writes nothing: no guest memory, no CPU
  state, no device state.
- Its reads must not be charged. The runtime counts guest accesses and turns
  them into logical time (`H2_ACCESS_COUNT`, [perf-budget.md](perf-budget.md)),
  so a charged read would shift the schedule. Observers need a read path
  outside that accounting.
- It runs at a point where the data is settled: the guest's own call, not a
  host timer.
- What it produces is a host-side scene description attached to the draw
  stream as side data. The reference backend ignores it.
- An observer that cannot make sense of what it reads disables the mode for
  that frame and counts it. It never stops the guest.

### Shadows: the choice of technique

The view needs, for each direct light, a per-pixel answer to "is this point
lit?". Three ways to get it:

| Technique | For | Against |
|---|---|---|
| **Stencil shadow volumes** (depth-fail), as the original | The authentic look: exact, hard edges at any resolution. No bias tuning. | Needs closed, well-formed caster meshes, and render meshes are often not. Alpha-tested casters (grates, foliage) cast solid shadows. Fill cost grows with volume size and with resolution. Silhouette extraction per light per caster, on skinned meshes too. |
| **Host shadow maps** at high resolution with a one-tap comparison | Works with any mesh and with alpha test. Reuses the vertex programs. This is how the community restoration gets its "stencil-like" mode. | Bias and resolution artefacts. Not exact at grazing angles. Arguably not the feature the README names. |
| **Ray queries** against a host acceleration structure | Exact and simple once geometry is on the host. | wgpu's ray-query support is not something to depend on yet. Needs the same host geometry as volumes. |

Recommendation: prototype volumes and shadow maps side by side **in the
viewer** (`crates/viewer`), which already draws structure BSPs and render
models from tags with no game run. Decide on pictures and timings from that.
The scene-knowledge work is the same for all three, so the choice does not
block anything before it.

For volumes, the caster geometry is the open design point:

- **Level.** The collision BSP is closed geometry and suits depth-fail
  volumes, but it is coarser than what is drawn, so a shadow can part from
  the surface that casts it. The render geometry matches what is seen and is
  not guaranteed closed. A level also mostly shadows itself through its
  lightmap already (below). Start with the level as a receiver only.
- **Objects.** Render models, posed with the guest's matrices. This is where
  the look comes from: characters and vehicles casting onto the world and
  themselves. Open edges need the usual treatment (per-triangle volumes or
  capping), at more fill.
- **Extrusion** on the GPU from a prebuilt edge mesh, so the CPU does not
  touch vertices per frame.

Fill cost killed this on the NV2A. A current GPU has far more fill to spare,
and the volumes can be rasterised at guest resolution even when the view is
scaled, then used as a mask. That is an estimate to be tested in the viewer,
not a result.

### Lighting: per-pixel, from the lights

Hard shadows alone are a mask. The second half is replacing how dynamic
lights shade surfaces:

1. The view builds a host-only buffer of world position (from its depth
   buffer and the observed camera) and per-pixel normal. The normal needs each
   opaque draw's normal map and tangent frame, so it needs the material
   tagging above.
2. For each dynamic light, the view accumulates diffuse and specular under the
   shadow mask.
3. The guest still issues its own dynamic-light passes. The view skips those
   draws, identified by their pass tags, and substitutes its result at the
   same point in the frame. The reference draws them as always.

Step 3 needs the pass taxonomy, which is not recovered: we know draws carry a
layer index, not what each layer is ([engine-map.md](engine-map.md),
`0x00013680`). Compositing at the wrong point lights fog, transparents and the
HUD. This is the part most likely to take longer than planned.

### The lightmap problem

The E3 renderer drew direct light dynamically and kept lightmaps for ambient.
Retail lightmaps hold direct sunlight and its shadows together with bounce.
Adding a dynamic sun on top double-lights sunlit areas, and adding its shadow
mask darkens areas the lightmap already darkened.

There is no clean fix inside the retail data. Options, from cheap to costly:

- **Objects only.** The sun's hard shadows are cast by objects onto the
  level, and onto objects by objects and the level. The level's own shadows
  stay baked. This gives the most recognisable part of the look and avoids
  the double count, apart from where an object's shadow falls on an area
  already in baked shadow (mask by the lightmap's value there).
- **Estimate the direct term** from the per-vertex lighting vector and the
  probes, and subtract it. Speculative: the vector's meaning is not fully
  recovered.
- **Re-bake** the ambient term on the host from level geometry. A lightmapper
  is a project of its own.

This design commits to the first, and treats the others as research.

### The "re-enable" option

Separate from all of the above, and smaller: make retail's own shadow buffers
look harder.

- **Host-side.** In the view only, scale the shadow buffers
  ([high-resolution.md](high-resolution.md), phase 4) and force a one-tap
  comparison. No guest change. This is the cheapest visible step.
- **Guest-side.** Clear a "no shadow" flag on lights that have it. This edits
  tag data the guest reads, so the guest draws more. It is a mod, not a
  renderer option. It is outside the guarantee below until someone shows
  that light flags are read only by rendering code, and it would need its
  own flag, off by default, excluded from deterministic replays.

## Opt-in, and the simulation untouched

- **Off by default.** With the mode off there is no observer, no side data
  and no view-side pass. The code path is today's.
- **One-way data flow.** Guest to observers to view. Nothing returns.
- **The guest-visible values are the reference's.** With the mode on, every
  D3D result, write-back and pixel count is what it is with the mode off.
- **Failure is local.** A frame the view cannot light is shown with retail
  lighting.
- **No game data in the repo.** Observers and passes are tested on invented
  scenes. Viewer prototypes read the owner's disc at run time and write
  nothing into Git.

The acceptance test is the one in high-resolution.md: the same input timeline
with the mode off and on gives the same guest memory digest and the same D3D
return trace. A second test proves the observers are uncharged: the access
count and logical time per frame are identical off and on.

## Phases, effort and risks

Effort figures are guesses for one person who knows the codebase. They are
here to show relative size.

| Phase | Work | Effort | Needs |
|---|---|---|---|
| 0 | Static research, no runtime change: trace the stencil call sites and name each use; find the light creation and render path from the `ligh` part case (`0x0017ab2e`); read the light flags off engine code; find who draws the shadow depth levels; name the layers in the scene-pass dispatch. Deliverables are docs, decoders and invented-data tests. | 2-4 weeks | Nothing |
| 1 | Harder retail shadows in the view: scaled shadow buffers, one-tap comparison. | 1-2 weeks | high-resolution.md phases 1-2, and in-place depth sampling |
| 2 | Observers and the scene description: camera, draw tagging, lights, object poses. Uncharged reads. Tests on invented guest memory. | 3-5 weeks | Phase 0 |
| 3 | Viewer prototype: volumes against shadow maps on tag geometry, with timings. Decide. | 2-3 weeks | Nothing; can run beside phase 2 |
| 4 | Sun shadows from objects in the view, composited as a mask. | 6-10 weeks | Phases 2, 3 |
| 5 | Per-pixel dynamic lights with normal maps and specular, replacing the guest's light passes in the view. | 8-12 weeks or more | Phase 4, the pass taxonomy |
| 6 | The environment's direct light. Research; may never ship. | Unknown | |

Risks, largest first:

1. **The light data is not found or not stable.** Everything from phase 2 on
   needs live light state, and the render code for lights has not been
   located. Phase 0 exists to retire this first.
2. **Pass taxonomy.** Without knowing which draws are opaque base passes,
   light passes, fog and transparents, the view cannot composite correctly.
3. **Material coverage.** There are 165 templates and 147 passes in retail
   ([shader-instances.md](shader-instances.md)). Finding the normal map for
   each needs the engine texture sources and inputs named, which are open
   items there. Expect a long tail, with a fallback to the vertex normal.
4. **The lightmap double count** limits how far the look can go without
   phase 6.
5. **Off-screen casters.** The guest culls to the frustum, so the draw stream
   lacks casters behind the camera. Shadows need tag geometry plus guest
   poses, which is more observer surface than the draw stream alone.
6. **Geometry quality** for volumes: open meshes, alpha-tested casters,
   level-of-detail switches changing silhouettes.
7. **Cost.** The view already costs a second backend. Volumes add fill, and
   the buffers add memory at 4K. A CPU-bound runtime at 5-6 fps on the menu
   has other priorities first; this sits behind 30 fps.
8. **Expectation.** The result will be a new renderer's idea of the E3 look
   on retail content. It should be described that way to users.

## Sources

Cited only. Nothing was copied, and no leaked material was downloaded or
read. Read 2026-11-15. Items marked P\* above were not read at their origin:
Eurogamer and the Steam pages refused the fetch, Bungie.net's article is
gone, the GDC slides were too large to fetch, and TCRF did not serve the
article.

- Bernie Freidin profile, Xbox.com, 2003, mirrored at
  <https://halo.bungie.org/misc/xbox.bfreidin/>
- "The Complete, Untold History of Halo", Vice:
  <https://www.vice.com/en/article/the-complete-untold-history-of-halo-an-oral-history/>
- Halopedia, "Pstencil", "Halo 2 E3 demo" and "Behind the Scenes: Making of
  Halo 2": <https://www.halopedia.org/Pstencil>,
  <https://www.halopedia.org/Halo_2_E3_demo>,
  <https://www.halopedia.org/Behind_the_Scenes:_Making_of_Halo_2>
- Eurogamer, "Bungie: Halo 2 E3 demo was a fake", 2010:
  <https://www.eurogamer.net/articles/bungie-halo-2-e3-demo-was-a-fake>
- Vice on the 2025 leak:
  <https://www.vice.com/en/article/halo-2s-legendary-e3-demo-has-leaked-after-22-years/>
- Microsoft Learn, "Halo 2 Guides: Dynamic Lights Best Practices":
  <https://learn.microsoft.com/en-us/halo-master-chief-collection/h2/guides/guidedynamiclights>
- Digsite's E3 level for MCC and the replacement shaders it points to:
  <https://github.com/digsite/H2E3>,
  <https://github.com/777Sev777/Halo-2-Custom-Shaders>
- Hao Chen, "Lighting and Material of Halo 3", GDC 2008 (title only; read as
  an excerpt)
- grimdoomer, Halo-2-HD: <https://github.com/grimdoomer/Halo-2-HD> and
  <https://icode4.coffee/?p=738>
- The Cutting Room Floor, "Proto:Halo 2": <https://tcrf.net/Proto:Halo_2>
  (not read)
