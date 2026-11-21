# E3 lighting: stencil shadows and dynamic lighting

Status: Partly built. The scaled D3D backend draws harder retail shadows. The native frame graph accepts the Lighting mode and draws sealed scenes into private targets. Both native modes use the guest's light passes. Native atlas tiles, sun shadows and light selection are Design.
Code: `crates/rewrites/native-render/src/frame_graph.rs` (display and private lighting), `crates/platform/d3d/src/scaled.rs` (`Lighting`, `follow_lighting`, `shadow_scale`), `crates/formats/shaders/src/psh/shadow.rs` (the shadow comparison), `crates/rewrites/engine/src/render.rs` (views of the light record), `crates/rewrites/native-render/src/light_look.rs` (retail's light passes, native). Tests: `crates/rewrites/native-render/tests/world_layers_gpu/frame_graph.rs`, `crates/platform/d3d/tests/render_scale.rs`, `crates/formats/shaders/tests/shadow.rs`.
Switch: `H2_LIGHTING` (`retail`), listed in [switches.md](../runtime/switches.md).

An opt-in lighting mode in the spirit of the Halo 2 E3 2003 demo: hard-edged
shadows cast by and onto everything, objects shadowing themselves under the
sun, every lamp, headlight and muzzle flash lighting the scene per pixel. It
is a mode of the native renderer ([native-renderer.md](native-renderer.md))
drawing from `FrameScene` ([framescene.md](framescene.md)). The rule that
must hold: the mode is read on the render thread only, and the scene, the
guest and the simulation are the same with it on and off.

It is not a recovery of Bungie's renderer. Retail has no shadow-volume
path ("Stencil: every use, named"), the original maths is not public, and
only the retail disc's content is in scope: not the demo's level, scripts
or assets. What retail does keep is the cut renderer's lighting half: every
lit material carries per-pixel, bump-mapped light passes ("What retail
already has"). So the mode is mostly a different use of passes the content
already has, plus one new shadow path for the sun.

The setting is `[graphics] lighting = "retail" | "e3"`, the Video screen's
**Lighting** row ([settings-menu.md](../game/settings-menu.md)), default
Retail and live. `H2_LIGHTING` locks it. The history of the cut renderer is
in [pstencil-history.md](pstencil-history.md), the Digsite tag set's lights
in [pstencil.md](pstencil.md), measurements and research records in
[notes/e3-lighting.md](notes/e3-lighting.md), and the review this design
answers in [2026-11-20-e3-lighting.md](../reviews/2026-11-20-e3-lighting.md).
No leaked build, source or archive is used for this work, and none may be.

## Evidence labels

- **P**: a developer's own statement, with the source. **P\*** means we read
  it through a secondary quotation (a wiki, a mirror or a search excerpt)
  because the original was unreachable, so the wording is unverified.
- **C**: a community finding or a secondary characterisation.
- **R**: read from the retail image or disc by this project, in the doc
  named.
- **S**: our inference or speculation.
- **Design**: our choice, not a fact about the game.
- **Estimate**: arithmetic on a measurement taken elsewhere, to be replaced
  by a measurement in a frame.

## Target look

What the pictures of every slice are judged against. Each item has its source;
the footage times are rows of the table in [pstencil-history.md](pstencil-history.md), section 12.

1. **Hard shadow edges, no visible filtering**, from the sun and from local
   lights, on large and small casters. Halopedia's "sharp rather than
   softened" (**C**); Steam commenter's "razor sharp" (**C**); footage 2:00,
   5:24, 7:12 (**S**).
2. **Characters and vehicles cast onto the world and onto themselves under
   the sun**, not only under dynamic lights. Staten (**P\***), Halopedia
   (**C**), Beyond3D (**C**).
3. **Large vehicles cast large shadows** (the Pelican). Jones (**P\***),
   footage 1:07 (**S**).
4. **Several shadows per object where several lights reach it.** Beyond3D on
   the Ghost (**C**).
5. **Lamps cast shadows and throw pools of light** that read as discrete
   shapes on the ground. Halopedia (**C**), footage 5:00, 7:12 (**S**).
6. **Vehicle headlights light the road in front** and move with the vehicle.
   Halopedia (**C**), footage 5:44-5:49 (**S**).
7. **Weapon fire and explosions light their surroundings**, including the
   firer. Perez via Halopedia (**P\***), Halopedia on the turret (**C**),
   footage 3:19 (**S**).
8. **Shadowed areas are very dark**: low ambient, high contrast, characters
   close to silhouettes in shade. Footage 2:00-2:09, 6:27 (**S**) only; no
   statement.
9. **The level's own sun shadows are baked, and sharp.** Halopedia: the
   demo's lightmap shadows are sharp and retail's were softened (**C**).
   Perez's "combine lightmaps for ambient lighting with stencil for
   directional sources" (**P**)
   is the plan the demo's level did not show. Not met: retail's bake is
   what the disc has ("The lightmap problem").
10. **Dynamic lights shade per pixel through normal maps**, so armour and
    walls show relief under a moving light. Freidin (**P**), normal maps
    everywhere (**C**), footage 6:33, 8:36 (**S**).
11. **Specular highlights from dynamic lights** on armour and visors. Staten
    on the visor (**P\***), footage 8:36-8:42 (**S**).
12. **Coloured, projected lights (gels)** where a light has one. Digsite
    (**C**).
13. **Out of scope but part of the look:** the demo's different bloom
    (Halopedia, **C**).

## What this means for the technique

**Shadow maps, not stencil volumes** (**Design**). Nothing public argues
that the look needs volumes: every description of it is of properties a
dense shadow map with a steep edge also has (hard edges, everything
casting, self-shadowing). Whether the stage demo itself still ran volumes
is not settled in the public record (pstencil-history.md, section 5), so
the look is the target and the mechanism is ours. The reasons:

1. **Cost.** The prototype put volumes at or above a one-tap map at every
   scale, and volume fill grows with the pixels while a map grows with its
   edge ([notes/e3-lighting.md](notes/e3-lighting.md), "shadow prototype").
2. **Open meshes.** 37% of the casters' triangles lie in closed pieces and
   none of the level's clusters do (the same note). Bungie's own reasons for
   leaving volumes are these two: Perez's "geometry/lighting fidelity
   compromises" (**P**), Jones on the Pelican's volumes filling the screen
   (**P\***), Griesemer on exteriors (**P\***).
3. **Blended frames.** At 60 and 120 fps the renderer draws poses blended
   between two scenes. A map is the caster drawn again with the vertex
   program it has. A volume needs a silhouette per light, per caster, per
   display frame from skinned positions, and wgpu has no geometry stage.
4. **The stencil buffer is taken.** Retail's marks write 0 or `0xff` over
   whole draws ("Stencil: every use, named"); counting volumes would need a
   second depth-stencil at the display scale.
5. **The content.** Ordinary lit templates can supply the depth and
   receiver programs. The census below measures coverage per LOD. Where
   a caster has an alpha-tested depth pass, its material can punch holes
   in the map.
6. **The sun is baked.** It is not a light in retail data, so a volume
   would end in the same multiply over baked light that a map ends in.

What volumes would still give is an exact edge at any magnification and no
bias. On maps that is met by density (a map fitted to each caster, "Sun
shadows from objects") and by the edge ("The edge"). No volume fallback is
kept; the experiment's row is in [notes/not-adopted.md](notes/not-adopted.md).
Ray queries are not used: wgpu's support is not something to depend on.

One community claim to discount: that retail still has true stencil lights
(Zanzibar's swinging lights, Foundation's fan room; ResetEra, 2018, **C**).
The retail reading found no shadow-volume path (**R**); those are lights
without the no-shadow flag, drawn with shadow maps.

## What retail already has

Read from the retail image and the disc ("Phase 0 findings",
[native-lights.md](native-lights.md), [native-shadows.md](native-shadows.md)):

- **Per-pixel light passes on every lit material** (**R**). Layer 12 is a
  diffuse pass per light: the base map, a normalization cube, the
  material's bump map, and a gel or a distance-attenuation volume, added to
  the frame. Layer 13 is a tangent-space specular pass. There are 10,137
  resolved lists on each against 16,622 on the lightmap layer ("Pass
  taxonomy: the layers"). `native_render::light_look` translates them and
  the native lights stage draws them.
- **Shadow maps for dynamic lights** (**R**). A shadowed light draws its
  casters' depth (layer 10) and then its receivers (layer 11), which write
  the shadow term into the frame's alpha (**S**: the light passes that
  follow read it from there).
- **Object shadows** (**R**). Before layer 7, up to 32 candidates enter
  a history targeting four. Ordinary caster draws use a silhouette mask
  filtered twice and multiplied onto receivers. A mask has no depth, so
  it cannot self-shadow. Creatures have a separate ground-shadow draw.
- **A first-person light pass** (**R**): up to eight of the frame's lights
  drawn again over the first-person view.
- **A history of selected lights** (**R**). Up to 128 eligible lights feed
  a 32-entry view history and a fade targeting three ordinary lights.
  Fading and exempt entries can take the draw count above three.
  1,275 of the disc's 1,596 light definitions have shadows flagged off
  ("The light render path", "Definition flags read off the code").

What retail does not have: volumes, the sun as a light, depth in object
shadows. So the mode is four changes to how the renderer draws one scene:
a depth map per caster for the sun, the renderer's own choice of lights,
shadows from every cone light, and a different edge.

## How it works

The frame graph and candidate extraction are built. The sections on new shadow maps and light selection describe the design.

### The scene does not know the mode

`FrameScene` holds what both modes need, extracted the same way whatever
the setting: every live light that can reach the view, not only those the
guest chose to draw; each instance's node matrices and lighting record; the
guest's own shadow lists. The lighting mode is a parameter of the render
thread's frame (`native_render`, per display frame). Retail mode draws the
lights the guest drew and its shadow lists as they are. E3 mode makes its
own choices from the same scene. `scene-extract`, `feature` and `boot`'s
frame step never read the setting.

So one sealed scene can be drawn both ways, which is how pictures are
compared ("Interfaces"), and the scene's summary hash is the same with the
mode on and off, which is how "render-only" is checked.

### Sun shadows from objects

One depth map per caster, in place of retail's object-shadow masks
(**Design**).

- **Casters.** Start with the ordinary object-mask draws in the guest's
  shadow list. **R**. With an active lightmap and the shadow mode enabled,
  `0x000335e0` calls `0x00033400`. The gather applies the view-mode bits
  and `0x0003d7f0`'s object, definition, distance and fade tests. It
  admits up to 32 objects, then ranks them with history and a target
  of four. The draw at `0x00032350` requires a root object and a valid
  visible hierarchy. Creature roots take a separate ground-shadow path
  at `0x00032ce0`. **Design**. Keep that path for creatures. A later
  slice gathers eligible casters just outside the view.
- **Direction and strength.** **R**. The ordinary draw resolves its
  lighting sample at `0x0003dc90` from a valid object lighting-cache
  entry or the default lighting. It normalizes the vector at sample
  `+0x0c`. Strength is the positive scalar at sample `+0x1c` times
  the current object fade and the ranked entry's faded weight. The
  product must clear the draw threshold. The renderer uses the selected
  direction and final strength for the tile. [Object chain](native-shadows.md#object-chain).
- **The map.** An orthographic depth tile fitted to the caster's bounding
  sphere along that direction. Its edge is the next power of two above 1.5
  times the sphere's diameter in display pixels, between 128 and 2048, so a
  texel is never larger than a pixel on the caster. Tiles live in one
  4096-square depth atlas shared with the local lights, packed each frame
  by size; if they do not fit, the largest class halves and a counter
  says so.
- **Drawing the caster.** Select the material's layer-10 depth writer
  at the LOD actually drawn, with the blended palette rows and its alpha
  test. **Design**. Retain the retail shadow for a caster whose visible
  silhouette includes a part with no supported depth writer.
- **Receivers.** The caster itself, the instances whose spheres meet the
  tile's box swept along the direction, and the cluster parts that do.
  Each is drawn with its material's layer-11 receiver pass that samples a
  depth map and multiplies the frame (**R**: implementation 3 in
  native-shadows.md's table), at equal depth. That pass exists for retail's
  own shadows. **R**. Across 165 distinct template names, 72 have a
  layer-10 depth writer and 69 have it and the multiplying receiver in
  the same LOD. Only 15 have the pair in every LOD. At the selected LOD,
  a missing receiver uses an E3 receiver based on the material's depth
  vertex path, with the same position, normal and coverage rules. Admit
  the replacement only when it can reproduce those inputs.
  [Counts per map](notes/e3-lighting.md).
- **Where.** At retail's object-shadow point, before layer 7, so
  self-illumination and environment maps are added over the shadow as they
  are in retail. Retail's own mask chain is not drawn for a caster that has
  a tile.
- **Self-shadowing** is the caster receiving its own tile. **R**.
  Implementation 3 writes the vertex normal dotted with hardware
  `c[41]` to diffuse RGB. Its six populated vertex classifications use
  the normal in world space, transformed by the rigid or skinning rows
  where needed. The shadow setup at `0x00048f85` supplies the direction
  towards the light. The pixel combiner interpolates between unity and the
  depth comparison using that vertex colour. A triangle whose three
  vertex terms clamp to zero preserves destination RGB. At a terminator
  the interpolated gate can be fractional. The alpha-only receivers have
  a different gate. [Program evidence](notes/e3-lighting.md).
- **Not done:** the first-person model neither casts into tiles nor
  receives them; it has its own projection.

### Lights: which, and how many

The lights are retail's: 428 scenario placements, 1,033 object attachments
(headlights, the flashlight, projectiles) and 834 effect parts (muzzle
flashes, explosions) over the disc's 30 maps, of 1,349 sphere and 245 cone
definitions ([effects.md](../formats/effects.md), "ligh"). At most 350 are
live at once ("The light record").

- **Candidates.** Every active light reaching the view that passes its
  owner and camera tests, before retail's ranking. In particular, the two
  flashlight attachments are mutually exclusive for a given owner and view.
- **Score** (**Design**). Multiply retail's projected sphere radius by
  the light's intensity and break ties by lower datum index. Retail itself
  stores projected size and fade weight separately
  ([The light render path](#the-light-render-path)).
- **Lit.** The 16 highest scores in a frame (**Design**; split screen
  shares the 16 between views). A light whose owner chain reaches the
  view's own unit (its flashlight, its weapon's flash) is placed first.
  The Xbox drew a handful and the MCC restoration reads dark for culling
  early (**C**, pstencil.md); 16 is set by the budget, not by taste.
- **Shading.** The material's own layer-12 and layer-13 passes, the
  implementation picked by retail's choosers (`engine::light_passes`) with
  the renderer's shadow decision in place of the guest's. Specular is drawn
  where retail would draw it. No new shading maths, and the bump map is
  whatever the material's pass binds, so there is no table of normal maps
  to build.
- **Extent.** Receivers are the instances and cluster parts that meet the
  light's bounds, under a scissor from the sphere's screen rectangle.
- **Constants.** `engine::light_constants` and `engine::shadow_constants`
  are pure. Extraction runs them for every candidate, so the scene carries
  each light's constants for both shadow decisions.

Retail mode draws the candidates the guest drew, in its order.

### Shadows from local lights

- **Cone lights cast; sphere lights do not** (**Design**). A cone has one
  projection, which is what retail's shadow block builds. A sphere needs
  six maps (2.9 ms for one in the prototype at 4x,
  [notes/e3-lighting.md](notes/e3-lighting.md)), and nothing in the record
  describes shadows from a muzzle flash.
- **The no-shadow flag is ignored for cones** (**Design**). This is the
  demo's lamps and headlights casting again (target look, items 5 and 6).
  It is a choice made on the render thread about a scene; no tag or guest
  memory changes.
- **At most four shadowed lights in a frame**, by score. The view's own
  flashlight holds a slot while it is on.
- **The map** is a tile of the atlas, 1024 square, 2048 for the flashlight
  at render scale 3 and above.
- **Casters.** The guest's list for a light the guest shadowed. Otherwise
  the renderer gathers the instances and cluster parts inside the cone.
  The level casts here; open meshes are no trouble for a map.
- **Passes.** Retail's: layer 10 into the tile, layer 11 with the sharp
  edge, then 14, 12 and 13.

### The edge

The shadow comparison in `shaders::psh::shadow` has three forms:

| Form | What it does | Used by |
|---|---|---|
| Retail | the NV2A's comparison with its 2x2 filter | retail lighting |
| Hard | one nearest tap (`PixelOptions::hard_shadow`) | the Lighting setting on the D3D path |
| Sharp | four bilinear comparison taps over a 3x3 texel tent, then a ramp centred on half coverage and one screen pixel wide (from the term's screen derivative) | E3 lighting on the native renderer (**Design**) |

A nearest tap is hard but stepped wherever a map texel is larger than a
pixel, and a lamp's cone at 4x always has such texels. Filtering and then
steepening gives a crisp, anti-aliased edge at any density and rounds off
only features under about a texel and a half. The community restoration's
"stencil-like" shaders make their edge the same way, with a far wider
filter ([notes/e3-lighting.md](notes/e3-lighting.md), "Sev's shadow-apply
shaders"). Sun tiles are dense enough that the two forms differ by less
than a pixel there.

### The flashlight and the first-person pass

**R**. The Chief's `flashlight` marker carries both `flashlight_1p` and
`flashlight_3p`. The Warthog attaches `warthog_headlights` at
`headlights_light` and the Scorpion attaches `scorpion_headlight` at
`headlight`. All four are frusta with bitmap gels and the no-shadow bit.
The unsuffixed flashlight definition in two maps has no attachment.
[Disc counts](notes/e3-lighting.md).

**R**. The view test at `0x00031520` requires the local first-person
owner for definition bit 5 (the first-person light) and excludes that
owner for bit 6 (the third-person light). `0x0003e9c0` tests camera type
and the root biped's player against the view user. **Design**. Reserve
one lit and shadowed slot for the view's active flashlight. Drawing
the lit set over first-person parts follows retail's recorded-light pass.
The first-person model stays outside the shadow maps.

### Weapon fire, plasma and explosions

These are effect-part lights (timed spheres) and lights attached to
projectiles. They are unshadowed and take their place by score, so a
firefight lights the street and the firer (target look, item 7). The
fade of a timed light is in its record's intensity.

### Blended frames

At 60 and 120 fps the renderer draws between two scenes
([framescene.md](framescene.md), section 7). Lights are matched by datum
and blended; a light with no match is drawn as the newer scene has it, so
a flash appears on its tick. A tile is drawn again only when its caster's
blended rows or its direction changed, so still casters cost nothing after
their first frame. Each view's lit set is chosen from the newer scene.

### The lightmap problem

Retail lightmaps hold the sun, its shadows and the bounce together. The
mode leaves them alone: the level's own shadows stay baked and soft, and a
dynamic sun is not added over them. What that costs is target item 9, and
a visible difference between a hard object shadow and the soft building
shadow beside it.

An object shadow can fall where the bake is already dark. **R**.
The ordinary shadow's strength depends on the caster's lighting scalar
and two fades. The sample producer can use a function of the caster's
weighted colour or set the scalar to one (`0x000d3e92`, `0x000d4682`).
The computation at `0x00032426` has no receiver-lightmap input. **Design**. Apply this strength to the tile shadow. A bright
caster can still darken a receiver already in baked shade.

Sharpening the baked shadows is research and not scheduled. The two
routes are a static depth map of the level along the sun, used to steepen
the lightmap's penumbra where the two agree, and a re-bake on the host,
which is a lightmapper.

## Interfaces

**What it needs from the scene.** `FrameScene::lights` holds the visible
candidates. Each `frame_scene::Light` carries its candidate data and marks
the guest's selections in their draw order. `scene_extract::lights`
uses the view's pre-trim eligibility tests and computes both constant sets
from the unfaded record weight. The guest's faded weight, layer steps and
gathered groups come from the actual pass.

| Need | In the scene | State |
|---|---|---|
| Camera, depth range | `View::camera` | Built |
| Instances, model, node matrices, bounding sphere | `Instance`, `FrameScene::nodes` | Built |
| The object's lighting record (direction, colours) | `Instance::lighting` | Built |
| Materials per part and layer | `PartDraw::key`, the loaders' templates for layers 10 to 14 | Layer 3 built; the rest with FrameScene slice 3 |
| Every candidate light: record frame, colours, intensity, bounds, effective volume shape and extent after placement overrides, definition flags, gel, owner instance and per-view eligibility, the guest's choice and weight, constants for both shadow decisions | `FrameScene::lights` | Built |
| The guest's shadow lists: casters, receivers, projection, strength | `FrameScene::shadows` | Design (FrameScene slice 3) |
| First-person parts | `View::first_person` | Design (FrameScene slice 3) |
| Cluster parts with bounds | the renderer's loaded world (`native_render::world`, `cull`) | Built |

**Where its passes sit** in the renderer's pass graph (FrameScene slice 6):
atlas tiles first, in one pass sequence at the start of the view's stage;
the sun receivers at the object-shadow point; the lit lights where retail's
light pass runs.

**The agent API** ([agent-api.md](../runtime/agent-api.md)) reports
`lights_candidates`, `lights` (the guest selections) and
`lights_with_constants` per view in `render.scene`. The extraction counters
report views attempted, elapsed nanoseconds and unread batches.

On a replayable game view the render thread draws one sealed scene into a private target with either mode. A completed native draw publishes its own report.

- `render.capture` takes `lighting: "retail" | "e3"`: the same sealed scene
  drawn in that mode into a private target, the setting untouched. A retail
  and an E3 capture of one tick go to `render.compare`.
- `render.frame` reports four lighting stages under the completed E3
  graph's `e3.passes`: `e3.tiles`, `e3.sun`, `e3.marks` and
  `e3.lights`. Each has its submitted draw count and an optional GPU
  time. L3 issues no draws for these stages.
- `render.stats` and `render.scene` expose the E3 graph's per-draw
  counters under `display_frame.e3` and `private_frame.e3`:
  `lights_lit`, `lights_shadowed`, `lights_over_cap`, `sun_tiles`,
  `sun_tiles_redrawn` and `atlas_halved`. They count the renderer's own
  light and atlas decisions. L3 replays guest selections in both modes.

The display worker samples the Lighting row at the frame boundary.
`settings.set` with `graphics.lighting` changes the mode on a window or GPU
serve with a settings store. A capture with an explicit mode leaves the
row untouched.

## Budget

At render scale 4 (2560x1920), one view, per display frame, on the M3 lab
laptop. All **Estimate**: scaled from the prototype's M1 Max timings
([notes/e3-lighting.md](notes/e3-lighting.md), "Budget arithmetic"), and
replaced by slice L4's first measurement.

| Part | GPU ms |
|---|---:|
| Atlas tiles for the sun (32 casters, most not redrawn) | 0.4 |
| Sun receivers | 0.6 |
| Atlas tiles for local lights (four) | 0.4 |
| Layer-11 marks with the sharp edge, over retail's | 0.4 |
| Lit lights beyond the guest's own (to 16) | 1.0 |
| Margin | 0.2 |
| **Total** | **3.0** |

That is 18% of a 60 fps frame. The render thread's own time is under 0.5
ms (gather, pack, encode) and the sim thread's under 0.1 ms (the candidate
lights and their constants, paid in both modes).

The caps are constants, not a governor: a picture must not depend on how
fast the last frame was. If a measurement is over, they give way in this
order: shadowed lights four to two, lit lights 16 to 8, the largest tile
2048 to 1024. At 120 fps the mode is offered only if the measured p99
fits, the rule FrameScene slice 8 uses for the rate itself.

Memory: the atlas is 64 MB as 32-bit depth.

## Opt-in, and the simulation untouched

- **Off by default**, and retail mode draws what the guest drew.
- **The mode is read on the render thread only.** No guest memory, tag,
  scene list, guest entry or tick-visible write depends on it. The scene's
  summary hash is the same with it on and off.
- **Nothing returns.** The mode adds nothing to the render thread's
  feedback channel.
- **Films, snapshots and co-op** do not record the mode. Peers may differ.
- **A failure is local.** A light or tile the mode cannot draw is drawn as
  retail would, and counted.
- **No game data in the repo.** Passes are tested on invented scenes.

The checks: `sim.hashes` identical between two launches with `H2_LIGHTING`
`retail` and `e3`; `render.scene`'s hash identical at every compared tick;
a test in `scene-extract` that its inputs do not include the setting.

## Slices

One lane each, in this order. L0 and L1 depend on nothing and can start
at once. L2 starts after FrameScene slice 3, and L3 onward after FrameScene
slice 6, when the renderer owns the frame. Nothing is built on the tenant
path or on `d3d::scaled`. Every picture check is a `render.capture` pair
through `render.compare`; lanes write code and tests and the coordinator
runs the game.

| Slice | Work | Testable result | Measure |
|---|---|---|---|
| L0 | Static research (`h2 disasm`, `h2 shader`, `h2 lights`): the object-shadow chain's direction, strength and caster tests (`0x000335e0`, `0x00032350`, `0x00033400`); how many templates have a depth-map receiver on layer 11 and a depth pass on layer 10; the receiver programs' vertex gate; the context `0x000c17f0` returns; the candidate list and its trim; which attachment lights are flashlights and headlights | Each **S** in "How it works" becomes **R** or the design changes. Readers of any new record, tested on invented bytes | Counts per map, in the notes |
| L1 | `shaders::psh::shadow` provides Sharp beside Retail and Hard. `PixelOptions::sharp_shadow` selects it per stage | Golden WGSL for the three forms; a GPU test with a slanted caster at map-to-screen ratios 1:1, 1:2 and 1:4: the lit-to-dark ramp is at most 1.5 pixels wide and its centre within a texel of the caster's outline; retail's golden file unchanged | none |
| L2 | `frame_scene::Light` holds each candidate and both constant sets. `scene_extract::lights` reads the view's visibility gather, and `render.scene` reports the counts | A synthetic guest with 20 lights gives 20 candidates with the guest's five marked; the scene hash and `sim.hashes` do not change with `H2_LIGHTING` | sim-thread extraction time, both modes |
| L3 | The mode as a render-thread parameter; `render.capture {lighting}`; the pass rows and counters | With no E3 pass yet, a retail and an E3 capture of one tick are identical (`max_channel == 0`) | none |
| L4 | Local-light shadows: atlas tiles for the guest's shadowed lights, the Sharp edge on layer 11 | Synthetic: a tile's shadow has the L1 edge. Game: on a tick with a shadowed light, the E3 capture differs from retail's only within eight pixels of retail's shadow edges, and `e3.tiles` lists one tile per shadowed light | GPU ms of `e3.tiles` and `e3.marks` at 1x and 4x; the first numbers for the budget |
| L5 | Sun shadows from objects: tiles, receivers, self-shadowing, the tile cache | Synthetic: a box on a plane casts a hard shadow; an L-shaped caster shadows itself; a triangle with three nonpositive normal terms keeps its RGB, and mixed vertex terms interpolate; a still caster's tile is not redrawn; a moved one is. Game: an outdoor gameplay tick differs from retail around every caster, and no retail mask pass is drawn for a caster with a tile | GPU ms with 8, 16 and 32 casters (`sim.spawn`) at 1x and 4x; `sun_tiles_redrawn` at 60 fps |
| L6 | The renderer's light choice: score, the cap of 16, the view's own lights first, the first-person pass | Synthetic: 20 candidates draw the 16 highest in score order, the same every run. Game: firing in a dark street (`input.pad`) lights the firer and the ground in the E3 capture; `lights_lit` and `lights_over_cap` | GPU ms of `e3.lights` against `lights_lit` |
| L7 | Shadows from cones the content flagged off, with the renderer's caster gather; four slots; the flashlight | Synthetic: a no-shadow cone casts in E3 and not in retail. Game: a lamp placement and the flashlight cast in the E3 capture | The flashlight frame: all of the mode's passes at 4x. This is the worst case the budget must hold |
| L8 | Casters outside the view: extraction of instances near it | A caster behind the camera shadows the ground in view | sim-thread frame step, rise under 0.1 ms |
| L9 | Tuning against "Target look" on captures at pstencil-history.md's footage times; then the D3D-path row's code goes with `scaled.rs` (FrameScene slice 10) | A table in the notes: each target item, met or not, with the capture's tick | The laptop A/B below, three pairs |

Split screen and 120 fps are checked in L6 and L7 with the caps shared.

**Measurements.** Numbers come from the lab laptop
([lab.md](../runtime/lab.md)). Both arms use the native game-frame
owner. Its completed game-view counters in `render.scene` identify the
frames the native graph presented. Local runs supply the pictures.

```sh
for mode in retail e3; do
  h2 lab run --host laptop --branch "$B" --level 03a_oldmombasa --difficulty heroic \
    --frames 5000 --stats-from 3000 --build-env H2_OPT=2 --fresh-data \
    --env H2_FEATURES=frame-scene.sink --env H2_RENDER_SCALE=4 --env H2_LIGHTING=$mode \
    --wait --to "$RUSH_KEEP/lab/$B-$mode"
done
```

Read fps, p50 and p99 frame time and the render thread's time per display
frame; repeat at `H2_RENDER_SCALE=1` and with `--env H2_FPS=60`. For one
frame, on a serve:

```sh
retail=$(h2 agent render.capture '{"lighting":"retail"}')
tick=$(printf '%s\n' "$retail" | jq -r '.tick')
h2 agent render.frame '{"source":"private","arm":true}'
e3=$(h2 agent render.capture "$(jq -nc --argjson t "$tick" '{tick:$t,lighting:"e3"}')")
retail_path=$(printf '%s\n' "$retail" | jq -r '.path')
e3_path=$(printf '%s\n' "$e3" | jq -r '.path')
h2 agent render.compare "$(jq -nc --arg a "$retail_path" --arg b "$e3_path" '{a:$a,b:$b}')"
h2 agent render.frame '{"source":"private"}'   # frame.private_frame.e3.passes
h2 agent render.stats                        # private_frame.e3
```

Each slice adds a dated entry to [notes/e3-lighting.md](notes/e3-lighting.md)
with its commit and these numbers.

## The Lighting setting

The row also selects the shadow treatment on the [D3D path](e3-lighting-setting.md). With E3 chosen, the scaled backend keeps a copy of each of the guest's shadow maps at the
render scale (at least 2x, `d3d::scaled::shadow_scale`) and the pixel
translator compares it with one nearest tap
(`shaders::psh::PixelOptions::hard_shadow`). The guest sees retail. It
changes the edge of retail's own dynamic-light shadows and nothing else,
and it is the Hard form of "The edge".

Where the native frame graph owns the game view, the row supplies its
mode at the display frame. An explicit private capture supplies its own
mode. The native graph draws the same lighting in both modes through L3.

## Phase 0 findings

Static reading of the retail XBE (`h2 disasm`, a linear disassembly of
`.text` searched for call sites, and the Ghidra function list) and of the
disc's tags (`h2 shader`, `h2 effect`), done without building or running the
game. Labels are this page's: **R** is read from engine code or
from the disc by this project, **S** is our inference. Names are ours unless
said otherwise. Read-only views of the structures below are in
`crates/rewrites/engine/src/render.rs`, tested on invented records.

The short version: **the light data is found, and it is a plain salted datum
array the render code reads at fixed points.** The guest's dynamic lights are
additive per-light passes over a shadow buffer, on numbered layers that the
shader templates and the engine share, so draws can be told apart by layer.
Retail stencil does masking only. Nothing in it is a shadow volume.

### Stencil: every use, named

| Site | Use | Label |
|---|---|---|
| `0x00015370` `stencil_mode_set`, mode in BX, stored at `0x00467010` | The only engine code that configures a stencil test. Seven modes (`stencil::for_mode`): 0 off; 1 and 2 write 0 or `0xff` wherever a pixel is drawn (ALWAYS, REPLACE on pass); 3, 4 and 6 draw only where bit 0 is clear, set, or bits 0 and 1 are both clear (EQUAL/NOTEQUAL, KEEP); 5 sets bit 1 where bit 0 is clear. Ops are only KEEP and REPLACE: no increment, decrement or z-fail counting, which shadow volumes need. | R |
| 42 direct calls of `0x00015370` | Every caller passes 0, 1 or 2. Mode 1 is set before the base-depth (1) and lightmap (3) layers in the scene loop and the structure draw `0x0004d720`; mode 2 marks the sky (`0x0002c0b2`, before `0x0003ede0`) and structure draws with a section flag (`0x0004d6d8`, flag `0x4000`). No direct caller passes 3 to 6. | R; what reads the marks is not traced (S: modes 3 to 6 are for that, or unused) |
| `0x0003bf20` | Colour and depth writes off, stencil ALWAYS/REPLACE with reference 0, then a four-vertex quad from world points at `0x004c1b08`: resets the stencil under a quad. Called through a pointer (no direct caller). | R; purpose S |
| 24 other `SetRenderState_StencilEnable` sites, one per function (`0x0001e9cd`, `0x0002524b`, ..., `0x000314e3`, ..., `0x00052e6d`) | `StencilEnable(FALSE)`, mostly just after a `CullMode` set: state resets around a draw family. 23 pass a literal 0 or a zeroed register; `0x00026998` was not resolved. | R |
| `0x00017b60`, `0x00018ee0`, `0x0001a170`, `0x000222d0` | Generic render-state-by-index setters: every state setter is called from each of them, stencil included. They apply a tag pass's render-state list and the engine's own lists. | R |
| Tag passes | No pass of any `shad` on the disc sets a stencil state: across all 30 maps the resolved passes set only `ZFUNC`, `COLORWRITEENABLE`, `ALPHABLENDENABLE`, `ZWRITEENABLE`, blend, `ZENABLE`, `BLENDCOLOR`, alpha test, `CULLMODE`, `SAMPLEALPHA` and `FRONTFACE`. The decoder names stencil states, so their absence is real. | R |
| `ShadowFunc` | Set to `GREATER` (`0x204`) by `0x000496f0` at the start of the shadow-apply layer (`0x00031328`), back to `NEVER` (`0x200`) by `0x00049740`, which the layer end's table reaches (`0x00044649`). | R; which layer reaches `0x00049740` S |

Engine-driven render states (table `0x00467020` via `0x0001a170`) were not
enumerated. If any of them sets stencil it would be per template; nothing
above suggests it.

### The light record

| Fact | Evidence | Label |
|---|---|---|
| Lights live in a datum array: 350 records of `0x110` bytes, allocated from game state by `0x000bffa0`, header pointer at `0x004e030c`. | `0x000bffac`..`0x000bfffb` (array name at `0x0045323c`) | R |
| Created by `0x000c0350` from the effect part switch (`0x0017ab2e` → `0x0017ac70`), either attached to an object and marker or free in the world; and by `0x000c0520` from a placement record. Both call the update `0x000c0c80` once. | call sites | R |
| `+0x02` u16 flags: 1 live; 4 placed; `0x10` timed (definition `+0xb0` above zero); `0x20` copied from a placement bit. | `0x000c03b8`..`0x000c03f4`, `0x000c05cd`..`0x000c05ff` | R |
| `+0x04` `ligh` datum; `+0x14` creation tick (from the game-time global at `0x00510c54`). | constructors; age in `0x000c0e5d` | R |
| `+0x18` centre and `+0x24` radius form the sphere tested against the cluster clip. Projected radius supplies screen priority. Field `+0x100` supplies the separate starting weight. | `0x00132190` cluster walk, `0x000319f3` to `0x00031a86` | R |
| `+0x4c`/`+0x50`/`+0x54`/`+0x56`: an object whose functions drive the light's intensity (`0x000bab40` into `+0xd0`). `+0x58` parent object, `+0x5c` marker; `+0x60`/`+0x6c`/`+0x78` frame relative to the marker. | `0x000c0d15`..`0x000c0dc4`, `0x000c048c`..`0x000c04e7` | R |
| `+0x84` position, `+0xac` forward, `+0xb8` up, `+0x90` = position + forward. Attached lights are re-posed from the marker in the render pass itself (`0x00031d4b`..`0x00031dac`), not in the tick. | constructors, `0x00031d6f` | R |
| `+0xcc` base scale; `+0xd0` intensity (fades `1 - shape(age / duration)` with definition `+0xb0` duration and `+0xb6` shape for timed lights). | `0x000c0e53`..`0x000c0ea3` | R |
| `+0xd4` and `+0xe0`: two RGB colours, each interpolated by intensity between a pair in the definition (`+0x58`/`+0x64` and `+0x40`/`+0x4c`, mode at `+0x34`), then clamped to `[0, 1]`. `+0xec` a scalar from definition `+0x70`/`+0x74`. `+0xf0`.. computed by `0x000c0b00` from definition `+0xc0`. | `0x000c0dc9`..`0x000c11d7` | R; what each colour is for S |
| `+0xc8` the parent's fade: for a biped or vehicle parent, both colours are scaled by `1 - object +0x2b0`, unless definition flag bit 7. | `0x000c0f8c`..`0x000c108f` | R (S: active camouflage) |
| `+0xf8` f32: must be above zero for the light to cast a shadow. | `0x00031e14` | R |

**Stability.** Records are salted and fixed-size; a handle stays valid until
the light is deleted. The frame of an attached light is rewritten during the
render pass, so an observer must read it there (at the per-light pass
`0x00031170` or after `0x00031dac`), not at tick end. `light::LightRecord::check`
states what an observer can rely on (live, finite, colours in range, target
equal to position plus forward) and refuses anything else.

### The light render path

`0x00031c80` (`render_dynamic_lights`, called by the scene-pass function
`0x0002bcd0` at `0x0002c03e`) does, per frame:

1. **R**. The view's cluster walk at `0x00132190` tests light spheres
   against the cluster clip and stamps the accepted handles to avoid
   duplicates. The view can also supply up to four unclustered player
   lights. From the resulting handle list at `[0x00547f98]`,
   `0x00031910` admits up to 128 eligible entries. Each carries the
   light datum, root owner, projected radius (using clamped absolute view
   depth), starting weight from light `+0x100` and four classification
   bytes. `0x0004a7c0` merges into a 32-entry history per view.
   `0x0004ac30` gives the local player's lights priority, handles
   same-owner duplicates, and eases weights towards a target of three
   with a fade increment of 0.01. Bit 20 exempts a light from fade-out.
   The draw walks every surviving weight above threshold.
2. For each light with weight above a threshold: re-pose attached lights,
   record up to eight per frame at `0x004c6b00` (stride `0x408`), decide
   **shadow casting** (`0x00031e02`): definition flag bit 3 clear, the global
   at `0x004ba020` set, record `+0xf8` above zero, and the effective
   light shape nonzero. **R**. `0x000c17f0` returns success in AL and
   fills a caller-owned light volume. Its first word is the definition
   shape with any placement override applied (`0x000c1930`, `0x000c1980`).
   `0x000c1a80` builds its posed dimensions. A sphere (shape zero)
   fails the test at `0x00031e25`. Gather the objects it touches (`0x000c1d90`, `0x001320f0`,
   `0x00031660`), then draw it with `0x00031170`.
3. `0x00031170` (`render_light_passes`) computes per-light flags at
   `0x004ba014` (bit 0 when the effective shape is a sphere, bit 1 when the colour
   at `0x00485950` is above a threshold, bit 2 shadow, bit 3 and bit 5 from
   further tests; bit 4 is cleared), then draws layers
   on the light's list (`0x004c1a60`): shadow generate (10) when bit 2,
   shadow apply (11), layer 14, light diffuse (12), and light specular (13)
   when bits 0 and 1. It ends by turning stencil off and clearing the scissor
   (`0x000314e3`, `0x000314eb`).

**Shadow depth levels.** Layer 10's begin (`0x00044408`..`0x0004449f`) picks
render target 10, cleared to colour `0xffffff`, when per-light flag bit 4 is
set and bit 5 clear, and otherwise target 9, cleared to depth 1.0. Since
`0x00031170` clears bit 4, the per-light path always takes target 9. `0x00048ee0` builds the light's shadow
projection (stores `0x24` dwords at `0x004c9798`, uploaded to vertex constants `c[40]`..`c[48]`) before the layer draws.
Shadow apply (11) samples it with `ShadowFunc` `GREATER`. The render target
table is at `0x004b4b58`, stride `0x98`; target 9 is S as the 512x512 depth
level and the other as the 128x128 target d3d.md reports.

### Definition flags read off the code

`ligh +0` is a u32. The bits the render path tests (`light::definition_flags`),
with counts over the 1,596 `ligh` records on the disc (shared tags counted per
map):

| Bit | Engine use | Set on | Label |
|---|---|---|---|
| 3 | Set: no shadow (`0x00031e02`). This is the flag the MCC tools call "no stencil shadow". | 1,275 (321 can cast) | R; the tools name P (343), the match S |
| 5 | Requires a local first-person owner (`0x00031520`). With bit 18 clear, refreshes the frame from its marker (`0x00031d3a`). Forces shadow apply (`0x000312d4`) and is copied to the visible list. | 98 | R |
| 7 | Parent fade ignored (`0x000c0fe7`). | 378 | R |
| 18 | Suppresses the marker refresh with bit 5. | 27 | R |
| 20 | Copied to the visible list (`0x00031ad5`), exempts the light from ordinary duplicate suppression and fade-out in the ranking pass. | 1 | R |
| 22 | An extra per-light call (`0x00031c20`) and a wider object gather. | 217 | R |

Most lights have shadows switched off by bit 3, which agrees with the
community claim about the E3 level's light objects ([pstencil-history.md](pstencil-history.md), section 12), but the disc
cannot say which lights those are and we did not try. The same bit is the
no-shadow bit in the Digsite E3 tag set's paired light tags
([pstencil.md](pstencil.md)). `h2 lights <map>` prints each light's shape and
this bit, with its placements, attachments and effect parts.

### Pass taxonomy: the layers

The layer index a draw carries (`0x00016b90`'s `layer`) is one numbering for
both sides: shader templates list passes per layer, and the engine draws
layer by layer, holding the current one at `0x00467130` (`layer::CURRENT_LAYER`).
`0x00044370` begins a layer (descriptor table at `0x0043fa1c`, `0x24` bytes
per layer: target, clear, list, multisample; `layer::Descriptor`),
`0x00044550` draws the current list's entries for it through `0x0003bc30`,
`0x000445d0` ends it. Roles below come from the passes the disc's 30 maps put
on each layer (their blend and depth state, and what they bind) and from the
engine's order; the numbers of passes are resolved `(lod, layer)` lists.

| Layer | Role (`layer::Role`) | Passes | List | Notes |
|---:|---|---:|---|---|
| 1 | Base depth: opaque base colour, depth write | 4,244 | scene | stencil mode 1 |
| 2 | Base overlay: blended detail | 339 | scene | gated by `0x004ba023` |
| 3 | Lightmap and per-vertex static lighting | 16,622 | scene | stencil mode 1; palette reset at end |
| 5 | Transfer (precomputed) lighting | 126 | scene | |
| 6 | Water edge blend | 2 | scene | |
| 7 | Self-illumination, additive | 3,417 | scene | |
| 8 | Environment map, additive | 977 | scene | gated by `0x004ba022` |
| 10 | Shadow generate | 10,006 | shadow casters | per light |
| 11 | Shadow apply | 9,989 | light | per light |
| 12 | Dynamic light diffuse, additive, projected or spherical | 10,137 | light | per light |
| 13 | Dynamic light specular, additive | 10,137 | light | per light |
| 14 | Opaque, depth write, on the light's list | 538 | light | role unknown |
| 15 | Transparents | 2,898 | scene | |
| 16 | Planar fog | 16,668 | scene | special case in `0x00044550` |
| 17 | Bloom | 250 | scene | gated by `0x004ba024` |
| 18, 19 | Active camouflage, two layers | 111, 30 | scene | |
| 21 | Hologram | 6 | scene | |
| 0, 4, 9, 20, 22, 23 | no passes on the disc | 0 | | 9 is on the shadow list; 20 is drawn by the scene loop |

So the order in a view (`0x0002bcd0`) is: base layers 1, 3, 5, 8, 20 on the
scene list, then the dynamic lights (10 to 14 per light, `0x00031c80`), then
sky, transparents, fog and the rest. The view can tag every draw with
`[0x00467130]` at the shader draw and composite on that.

### What is still unknown

- What reads the stencil marks of modes 1 and 2, and whether modes 3 to 6 are
  reached at all (indirect calls were not searched).
- Layers 14 and 20, and the per-light flag bits 3 to 5 at `0x004ba014`.
- Which definition fields the light passes' vertex constants come from.
  The fields themselves are named: kind at `+0x04` (0 sphere, 2
  frustum), sphere radius `+0x18`, cone width, stretch, angle, falloff and
  cutoff `+0x20` to `+0x30`, colours `+0x40` to `+0x6f`, brightness `+0x70`
  ([effects.md](../formats/effects.md) "ligh"). `+0x14` was not confirmed as
  anything; it is zero on the study tags. These are
  in `0x00031170`'s callees (`0x0004a1e0` takes 1024.0 and the light, S: the
  scissor) and the engine vector inputs ([shader-instances.md](../formats/shader-instances.md)).
- Engine-driven render states (`0x00467020`).

## Not verified

- Every number under "Budget". Only a frame on the lab laptop can give
  them.
- That retail's layer-11 receiver pass gives an acceptable terminator when
  a caster receives its own tile. Slice L5's captures show it; the
  fallback is the receiver program of ours.
- That 16 lit lights are enough for a firefight to read as the footage
  does, and that four shadowed ones are.
- The Lighting setting's pictures in a level: the runs so far caught no
  frame where the two modes differ on screen
  ([notes/e3-lighting.md](notes/e3-lighting.md)).

## Related

[framescene.md](framescene.md), [native-renderer.md](native-renderer.md),
[native-lights.md](native-lights.md), [native-shadows.md](native-shadows.md),
[native-gels.md](native-gels.md), [high-resolution.md](high-resolution.md),
[shaders.md](shaders.md), [effects.md](../formats/effects.md),
[pstencil.md](pstencil.md), [pstencil-history.md](pstencil-history.md),
[notes/e3-lighting.md](notes/e3-lighting.md),
[notes/not-adopted.md](notes/not-adopted.md),
[the review](../reviews/2026-11-20-e3-lighting.md).
