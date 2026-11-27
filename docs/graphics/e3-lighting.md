# E3 lighting: stencil shadows and dynamic lighting

Status: Partly built. The native frame graph accepts the Lighting mode, draws the guest's shadow maps as tiles with the Sharp edge, plans and draws sun tiles from objects onto their receivers, chooses its own lights and draws those beyond the guest's. One atlas shared by the sun and the local lights is Design. The scaled D3D backend draws harder retail shadows.
Code: `crates/rewrites/native-render/src/frame_graph.rs` (display and private lighting), `crates/platform/d3d/src/gfx_backend/guest_draw_pass/tiles.rs` (the tile plan), `crates/platform/d3d/src/scaled/private_scene.rs` (tiles at render scale), `crates/platform/d3d/src/scaled.rs` (`Lighting`, `follow_lighting`, `shadow_scale`), `crates/formats/shaders/src/psh/shadow.rs` (the shadow comparison), `crates/rewrites/engine/src/render.rs` (views of the light record), `crates/rewrites/native-render/src/light_look.rs` (retail's light passes, native). The sun, light-choice and cone files are named on the sub-pages. Tests: `crates/rewrites/native-render/tests/world_layers_gpu/frame_graph.rs`, `crates/platform/d3d/tests/render_scale.rs`, `crates/formats/shaders/tests/shadow.rs`.
Switch: `H2_LIGHTING` (`retail`), listed in [switches.md](../runtime/switches.md).

An opt-in lighting mode in the spirit of the Halo 2 E3 2003 demo: hard-edged shadows cast by and onto everything, objects shadowing themselves under the sun, every lamp, headlight and muzzle flash lighting the scene per pixel. It is a mode of the native renderer ([native-renderer.md](native-renderer.md)) drawing from `FrameScene` ([framescene.md](framescene.md)). The rule that must hold: the mode is read on the render thread only, and the scene, the guest and the simulation are the same with it on and off.

It is not a recovery of Bungie's renderer. Retail has no shadow-volume path ("Stencil: every use, named"), the original maths is not public, and only the retail disc's content is in scope: not the demo's level, scripts or assets. What retail keeps is the cut renderer's lighting half: every lit material carries per-pixel, bump-mapped light passes ("What retail already has"). So the mode is mostly a different use of passes the content already has, plus one new shadow path for the sun.

| Page | What it holds |
|---|---|
| this page | the technique, what retail has, the mode's rules, the interfaces, the budget, the slices, the retail findings (Phase 0) |
| [e3-lighting-target.md](e3-lighting-target.md) | the target look with its sources, and the evidence labels |
| [e3-lighting-sun.md](e3-lighting-sun.md) | sun shadows from objects |
| [e3-lighting-lights.md](e3-lighting-lights.md) | the renderer's choice of lights, the flashlight and the first-person pass |
| [e3-lighting-cones.md](e3-lighting-cones.md) | shadows from local lights: the guest's tiles and the renderer's cones |
| [e3-lighting-setting.md](e3-lighting-setting.md) | the Lighting row on the D3D path |
| [notes/e3-lighting.md](notes/e3-lighting.md) | measurements and research records |

The setting is `[graphics] lighting = "retail" | "e3"`, the Video screen's **Lighting** row ([settings-menu.md](../game/settings-menu.md)), default Retail and live. `H2_LIGHTING` locks it. The history of the cut renderer is in [pstencil-history.md](pstencil-history.md), the Digsite tag set's lights in [pstencil.md](pstencil.md), and the review this design answers in [2026-11-20-e3-lighting.md](../reviews/2026-11-20-e3-lighting.md). No leaked build, source or archive is used for this work, and none may be.

Evidence labels (**P**, **C**, **R**, **S**, **Design**, **Estimate**) are defined in [e3-lighting-target.md](e3-lighting-target.md).

## What this means for the technique

**Shadow maps, not stencil volumes** (**Design**). Every public description of the look is of properties a dense shadow map with a steep edge also has (hard edges, everything casting, self-shadowing). Whether the stage demo itself still ran volumes is not settled in the public record ([pstencil-history.md](pstencil-history.md), section 5), so the look is the target ([e3-lighting-target.md](e3-lighting-target.md)) and the mechanism is ours.

| # | Reason | Detail |
|---:|---|---|
| 1 | Cost | the prototype put volumes at or above a one-tap map at every scale; volume fill grows with the pixels, a map with its edge ([notes/e3-lighting.md](notes/e3-lighting.md), "shadow prototype") |
| 2 | Open meshes | 37% of the casters' triangles lie in closed pieces and none of the level's clusters do (same note). Bungie's own reasons for leaving volumes are these two: Perez's "geometry/lighting fidelity compromises" (**P**), Jones on the Pelican's volumes filling the screen (**P\***), Griesemer on exteriors (**P\***) |
| 3 | Blended frames | at 60 and 120 fps the renderer draws poses blended between two scenes. A map is the caster drawn again with the vertex program it has. A volume needs a silhouette per light, per caster, per display frame from skinned positions, and wgpu has no geometry stage |
| 4 | The stencil buffer is taken | retail's marks write 0 or `0xff` over whole draws ("Stencil: every use, named"); counting volumes would need a second depth-stencil at the display scale |
| 5 | The content | ordinary lit templates can supply the depth and receiver programs, and a caster's alpha-tested depth pass can punch holes in the map (census per LOD in the notes) |
| 6 | The sun is baked | it is not a light in retail data, so a volume would end in the same multiply over baked light that a map ends in |

What volumes would still give is an exact edge at any magnification and no bias. On maps that is met by density (a map fitted to each caster, [e3-lighting-sun.md](e3-lighting-sun.md)) and by "The edge". No volume fallback is kept; the experiment's row is in [notes/not-adopted.md](notes/not-adopted.md). Ray queries are not used.

One community claim to discount: that retail still has true stencil lights (Zanzibar's swinging lights, Foundation's fan room; ResetEra, 2018, **C**). The retail reading found no shadow-volume path (**R**); those are lights without the no-shadow flag, drawn with shadow maps.

## What retail already has

Read from the retail image and the disc ("Phase 0 findings", [native-lights.md](native-lights.md), [native-shadows.md](native-shadows.md)).

| What | Detail (**R** unless marked) |
|---|---|
| Per-pixel light passes on every lit material | Layer 12 is a diffuse pass per light: the base map, a normalisation cube, the material's bump map, and a gel or a distance-attenuation volume, added to the frame. Layer 13 is a tangent-space specular pass. 10,137 resolved lists on each against 16,622 on the lightmap layer ("Pass taxonomy: the layers"). `native_render::light_look` translates them and the native lights stage draws them |
| Shadow maps for dynamic lights | A shadowed light draws its casters' depth (layer 10) and then its receivers (layer 11), which write the shadow term into the frame's alpha (**S**: the light passes that follow read it from there) |
| Object shadows | Before layer 7, up to 32 candidates enter a history targeting four. Ordinary caster draws use a silhouette mask filtered twice and multiplied onto receivers. A mask has no depth, so it cannot self-shadow. Creatures have a separate ground-shadow draw |
| A first-person light pass | Up to eight of the frame's lights drawn again over the first-person view |
| A history of selected lights | Up to 128 eligible lights feed a 32-entry view history and a fade targeting three ordinary lights. Fading and exempt entries can take the draw count above three. 1,275 of the disc's 1,596 light definitions have shadows flagged off ("The light render path", "Definition flags read off the code") |

What retail does not have: volumes, the sun as a light, depth in object shadows. So the mode is four changes to how the renderer draws one scene:

| Change | Section |
|---|---|
| a depth map per caster for the sun | "Sun shadows from objects" |
| the renderer's own choice of lights | "Lights: which, and how many" |
| shadows from every cone light | "Shadows from local lights" |
| a different edge | "The edge" |

## How it works

| Part | State |
|---|---|
| Frame graph, candidate extraction, the guest's shadowed lights as tiles | Built |
| Sun tiles and receivers (marked shadows and nearby offscreen roots) | Built on invented scenes; live check pending |
| The renderer's choice of lights, counted | Built |
| Chosen lights beyond the guest's, over the structure; cones the renderer shadows on their own tiles | Built |
| First-person lights retail's pass did not record, over the first-person parts | Built |
| Chosen lights on object (model) parts; one atlas for the sun and the local lights | Design |

### The scene does not know the mode

`FrameScene` holds what both modes need, extracted the same way whatever the setting: every live light that can reach the view, not only those the guest chose to draw; each instance's node matrices and lighting record; the guest's own shadow lists. The lighting mode is a parameter of the render thread's frame (`native_render`, per display frame). Retail mode draws the lights the guest drew and its shadow lists as they are. E3 mode makes its own choices from the same scene. `scene-extract`, `feature` and `boot`'s frame step never read the setting.

So one sealed scene can be drawn both ways, which is how pictures are compared ("Interfaces"), and the scene's summary hash is the same with the mode on and off, which is how "render-only" is checked.

### Sun shadows from objects

One depth map per caster, in place of retail's object-shadow masks (**Design**). Detail and what is built: [e3-lighting-sun.md](e3-lighting-sun.md).

| Rule | Value |
|---|---|
| Casters | ordinary object-mask draws in the guest's shadow list, plus up to 32 nearby offscreen roots (`scene_extract::layout::nearby_casters`); creatures keep retail's ground shadow |
| Direction and strength | the object's lighting sample (`0x0003dc90`), strength above retail's 0.09 |
| Map | orthographic depth tile fitted to the caster's sphere, 128 to 2048 texels, in a 4096-square atlas |
| Receivers | the caster, the instances and the structure parts the tile's box meets |
| Where | at retail's object-shadow point, before layer 7 |
| Refused | a creature, or a caster with no model, direction or world receiver, keeps retail's mask, its cause named |

### Lights: which, and how many

The renderer chooses the lit lights from the scene's candidates. Detail and what is built: [e3-lighting-lights.md](e3-lighting-lights.md).

| Rule | Value |
|---|---|
| Candidates | every active light reaching the view that passes its owner and camera tests |
| Score | retail's projected radius times intensity, ties by lower datum index (**Design**) |
| Lit | the 16 highest scores a frame (`LIT_CAP`), shared between split-screen views; the view's own lights first |
| Shading | the material's own layer-12 and 13 passes; no new shading maths |
| Host lights | Forge monitors' lights rank after every game light, unshadowed |

Retail mode draws the candidates the guest drew, in its order. The renderer's lights beyond the guest's are E3-only scene stages (`d3d::ExternalPass::lighting_only`).

### Shadows from local lights

Cone lights cast and sphere lights do not (**Design**); the no-shadow flag is ignored for cones, at most four shadowed lights a frame. Detail and what is built: [e3-lighting-cones.md](e3-lighting-cones.md).

| Rule | Value |
|---|---|
| The map | a tile at `d3d::scaled::shadow_scale` times the guest's level (1024 at 1x and 2x, 2048 at 4x); the flashlight's is at least 2048 |
| Passes | retail's: layer 10 into the tile, layer 11 with the Sharp edge, then 14, 12 and 13 |
| The guest's shadowed lights | their maps replay as tiles at the tile factor; a level the graph cannot redraw keeps retail's size and comparison (`tiles_refused`) |
| The renderer's cones | its own slots of a 4096-square atlas, `SHADOW_CAP` (4) |

### The edge

The shadow comparison in `shaders::psh::shadow` has three forms:

| Form | What it does | Used by |
|---|---|---|
| Retail | the NV2A's comparison with its 2x2 filter | retail lighting |
| Hard | one nearest tap (`PixelOptions::hard_shadow`) | the Lighting setting on the D3D path |
| Sharp | four bilinear comparison taps over a 3x3 texel tent, then a ramp centred on half coverage and one screen pixel wide (from the term's screen derivative) | E3 lighting on the native frame graph: every receiver of a tile (`ShaderProvider::set_sharp_shadows`) |

A nearest tap is hard but stepped wherever a map texel is larger than a pixel, and a lamp's cone at 4x always has such texels. Filtering and then steepening gives a crisp, anti-aliased edge at any density and rounds off only features under about a texel and a half. The community restoration's "stencil-like" shaders make their edge the same way, with a far wider filter ([notes/e3-lighting.md](notes/e3-lighting.md), "Sev's shadow-apply shaders"). Sun tiles are dense enough that the two forms differ by less than a pixel there.

### The flashlight and the first-person pass

The view's active flashlight holds one lit and one shadowed slot (**Design**), and the chosen first-person lights retail's pass did not record draw over the first-person parts. The first-person model stays outside the shadow maps. Disc facts on flashlights and headlights, the definition bits the view test reads, and the first-person pass: [e3-lighting-lights.md](e3-lighting-lights.md).

### Blended frames

At 60 and 120 fps the renderer draws between two scenes ([framescene.md](framescene.md), section 7). Lights are matched by datum and blended; a light with no match is drawn as the newer scene has it, so a flash appears on its tick. A tile is drawn again only when its caster's blended rows or its direction changed, so still casters cost nothing after their first frame. Each view's lit set is chosen from the newer scene.

### The lightmap problem

Retail lightmaps hold the sun, its shadows and the bounce together. The mode leaves them alone: the level's own shadows stay baked and soft, and a dynamic sun is not added over them. What that costs is target item 9, and a visible difference between a hard object shadow and the soft building shadow beside it.

An object shadow can fall where the bake is already dark. **R**. The ordinary shadow's strength depends on the caster's lighting scalar and two fades. The sample producer can use a function of the caster's weighted colour or set the scalar to one (`0x000d3e92`, `0x000d4682`).

The computation at `0x00032426` has no receiver-lightmap input. **Design**. The tile shadow takes the same strength, so a bright caster can still darken a receiver already in baked shade.

Sharpening the baked shadows is research and not scheduled. The two routes are a static depth map of the level along the sun, used to steepen the lightmap's penumbra where the two agree, and a re-bake on the host, which is a lightmapper.

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
| The object shadows: caster, projection, weight, creature branch | `FrameScene::object_shadows`, `ObjectShadow` | Built |
| The guest's local-light shadow lists: casters, receivers | `FrameScene::light_groups` (layers 10 and 11) | Built |
| First-person parts | the `ViewKind::FirstPerson` view's layer items | Built; the chosen first-person lights retail did not record draw over them ("The flashlight and the first-person pass") |
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
- `render.frame` reports five lighting stages under the completed E3
  graph's `e3.passes`: `e3.tiles`, `e3.sun_tiles`, `e3.sun`, `e3.marks` and
  `e3.lights`. Each has its submitted draw count and an optional GPU
  time. `e3.tiles` counts the caster draws into tiles and `e3.marks` the
  receivers compared with the Sharp edge. `e3.sun_tiles` counts
  the caster draws into the sun's tiles and `e3.sun` their receivers. `e3.lights`
  counts the ranges of the chosen lights' stages beyond the guest's. No stage has a timestamp query yet, so `gpu_ns` is null.
- `render.stats` and `render.scene` expose the E3 graph's per-draw
  counters under `display_frame.e3` and `private_frame.e3`:
  `lights_lit`, `lights_shadowed`, `lights_over_cap`, `lights_beyond_guest`, `sun_tiles`,
  `sun_tiles_redrawn`, `sun_tiles_offscreen`, `atlas_halved`, `tiles_refused`,
  `cones_shadowed`, `cones_flagged_off`, `flashlight_shadowed`,
  `shadows_over_cap`, `cones_refused`, `cones_drawn` and `cones_kept`. They count the
  renderer's own light and atlas decisions. `lights_shadowed` is the
  tiles drawn, one per shadowed light of the guest's, and `tiles_refused`
  the shadow maps left at the guest's size. `lights_lit`,
  `lights_over_cap` and `lights_beyond_guest` are the renderer's choice
  ("Built: the choice"); an E3 draw also draws the lights beyond the
  guest's, a retail draw the guest's alone. The five cone
  counters are the renderer's choice of shadowed lights ("Built: the
  shadowed cones"); `cones_drawn` counts the cone tiles drawn for the
  shadowed chosen lights and `cones_kept` those lights left undrawn
  because their cone could not be.

The display worker samples the Lighting row at the frame boundary.
`settings.set` with `graphics.lighting` changes the mode on a window or GPU
serve with a settings store. A capture with an explicit mode leaves the
row untouched.


## Budget

At render scale 4 (2560x1920), one view, per display frame, on the M3 lab laptop. All **Estimate**: scaled from the prototype's M1 Max timings ([notes/e3-lighting.md](notes/e3-lighting.md), "Budget arithmetic"), to be replaced by the first lab A/B ("Measurements"). The stages have no timestamp queries, so the first numbers are the difference in render-thread and frame time between the retail and E3 runs, at 1x and 4x.

| Part | GPU ms |
|---|---:|
| Atlas tiles for the sun (32 casters, most not redrawn) | 0.4 |
| Sun receivers | 0.6 |
| Atlas tiles for local lights (four) | 0.4 |
| Layer-11 marks with the sharp edge, over retail's | 0.4 |
| Lit lights beyond the guest's own (to 16) | 1.0 |
| Margin | 0.2 |
| **Total** | **3.0** |

That is 18% of a 60 fps frame. The render thread's own time is under 0.5 ms (gather, pack, encode) and the sim thread's under 0.1 ms (the candidate lights and their constants, paid in both modes).

The caps are constants, not a governor: a picture must not depend on how fast the last frame was. If a measurement is over, they give way in this order: shadowed lights four to two, lit lights 16 to 8, the largest tile 2048 to 1024. At 120 fps the mode is offered only if the measured p99 fits, the rule FrameScene slice 8 uses for the rate itself. The sun atlas is 64 MB as 32-bit depth.

## Opt-in, and the simulation untouched

| Rule | Meaning |
|---|---|
| Off by default | retail mode draws what the guest drew |
| Render thread only | no guest memory, tag, scene list, guest entry or tick-visible write depends on the mode; the scene's summary hash is the same with it on and off |
| Nothing returns | the mode adds nothing to the render thread's feedback channel |
| Not recorded | films, snapshots and co-op do not record the mode; peers may differ |
| A failure is local | a light or tile the mode cannot draw is drawn as retail would, and counted |
| No game data in the repo | passes are tested on invented scenes |

The checks: `sim.hashes` identical between two launches with `H2_LIGHTING` `retail` and `e3`; `render.scene`'s hash identical at every compared tick; a test in `scene-extract` that its inputs do not include the setting.

## Slices

One lane each. L0 and L1 depend on nothing. L2 starts after FrameScene slice 3, and L3 onward after FrameScene slice 6, when the renderer owns the frame.

Nothing is built on the tenant path or on `d3d::scaled`. Every picture check is a `render.capture` pair through `render.compare`; lanes write code and tests and the coordinator runs the game.

| Slice | Work | State | Testable result | Measure |
|---|---|---|---|---|
| L0 | Static research (`h2 disasm`, `h2 shader`, `h2 lights`): the object-shadow chain, template coverage on layers 10 and 11, the receiver programs' vertex gate, `0x000c17f0`'s context, the candidate list and its trim, which attachment lights are flashlights and headlights | Built | each **S** becomes **R** or the design changes; readers of any new record tested on invented bytes | counts per map, in the notes |
| L1 | `shaders::psh::shadow` provides Sharp beside Retail and Hard; `PixelOptions::sharp_shadow` selects it per stage | Built | golden WGSL for the three forms; a GPU test with a slanted caster at map-to-screen ratios 1:1, 1:2 and 1:4: the lit-to-dark ramp is at most 1.5 pixels wide and its centre within a texel of the caster's outline; retail's golden file unchanged | none |
| L2 | `frame_scene::Light` holds each candidate and both constant sets; `scene_extract::lights` reads the view's visibility gather; `render.scene` reports the counts | Built | a synthetic guest with 20 lights gives 20 candidates with the guest's five marked; the scene hash and `sim.hashes` do not change with `H2_LIGHTING` | sim-thread extraction time, both modes |
| L3 | The mode as a render-thread parameter; `render.capture {lighting}`; the pass rows and counters | Built | with no E3 pass, a retail and an E3 capture of one tick are identical (`max_channel == 0`) | none |
| L4 | Local-light shadows: the guest's shadow maps as tiles at `shadow_scale`, the Sharp edge on layer 11 ([e3-lighting-cones.md](e3-lighting-cones.md)) | Built | `render_scale::e3_tiles_give_a_replayed_shadow_the_sharp_edge`; the plan's tests in `tiles.rs`. Game: the E3 capture differs from retail's only within eight pixels of retail's shadow edges, `e3.tiles` and `e3.marks` have draws, `lights_shadowed` is one per shadowed light, `tiles_refused` is 0 | frame and render-thread time of E3 against retail at 1x and 4x; the first numbers for the budget |
| L5 | Sun tiles: plan, cache, atlas, caster and receiver draws, the scene's marks, the models and world receivers, the frame graph's stage ([e3-lighting-sun.md](e3-lighting-sun.md)). The materials' layer-10 and layer-11 programs are not used yet | Built on invented scenes | `a_box_on_a_plane_casts_a_hard_shadow`, `a_box_shadows_the_structure_it_stands_on`, `an_l_shaped_caster_shadows_itself`, `nonpositive_normal_terms_keep_rgb_and_mixed_terms_interpolate`, `a_still_casters_tile_is_not_redrawn_and_a_moved_one_is` (`tests/world_layers_gpu/sun_tiles.rs`). Game, pending: an outdoor tick differs from retail around every caster, and no retail mask pass is drawn for a caster with a tile | GPU ms with 8, 16 and 32 casters (`sim.spawn`) at 1x and 4x; `sun_tiles_redrawn` at 60 fps |
| L6 | The renderer's light choice and the first-person pass ([e3-lighting-lights.md](e3-lighting-lights.md)). Lights on third-person objects are not drawn | Partly built | `twenty_candidates_draw_the_sixteen_highest_in_score_order_every_run`, `the_lit_lights_the_guest_did_not_draw_are_drawn_unshadowed`, `the_cone_the_renderer_shadows_is_drawn_shadowed`, `the_first_person_set_the_guest_did_not_record_is_drawn_over_the_eye`. Game, pending: `scene_e3_lights_drawn` above zero, `e3.lights` draws in the E3 capture and none in retail; firing in a dark street (`input.pad`) lights the firer and the ground | GPU ms of `e3.lights` against `lights_lit` |
| L7 | Shadows from cones the content flagged off: choice, projection, gather, atlas and programs, counters, tiles in a game frame for the chosen lights drawn shadowed; four slots; the flashlight. A cone the guest draws unshadowed keeps retail's passes | Built | `a_no_shadow_cone_casts_and_a_sphere_does_not`, `a_no_shadow_cone_shadows_the_ground_in_e3_and_not_in_retail`, `a_shadowed_chosen_light_reads_its_cone_in_e3_and_not_in_retail`. Game, pending: a lamp placement and the flashlight cast in the E3 capture; `cones_shadowed`, `cones_drawn` and `scene_e3_lights_shadowed` above zero, `cones_kept` zero | the flashlight frame: all of the mode's passes at 4x, the worst case the budget must hold |
| L8 | Extract nearby offscreen casters and shadow visible ground | Built; live check pending | seeded pool and lifted lighting-sample differentials; GPU test `a_caster_behind_the_camera_shadows_the_ground_in_view`. Live: `render.stats` `private_frame.e3.sun_tiles_offscreen > 0` and `e3.sun` draws on an outdoor tick with the caster behind the camera | lab sim-thread frame-step increase under 0.1 ms |
| L9 | Tuning against the target look on captures at pstencil-history.md's footage times; then the D3D-path row's code goes with `scaled.rs` (FrameScene slice 10) | Design | a table in the notes: each target item, met or not, with the capture's tick | the laptop A/B below, three pairs |

L8 extends the set of caster instances. The light-gather sequence uses its own L8 for submitting typed passes for lights the guest has already gathered ([native-light-gather.md](native-light-gather.md#l8-the-passes-typed-l)). Split screen and 120 fps are checked in L6 and L7 with the caps shared.

**Measurements.** Numbers come from the lab laptop ([lab.md](../runtime/lab.md)). Both arms use the native game-frame owner; its completed game-view counters in `render.scene` identify the frames the native graph presented. Local runs supply the pictures.

```sh
for mode in retail e3; do
  h2 lab run --name "Lighting $mode" --host laptop --branch "$B" --level 03a_oldmombasa --difficulty heroic \
    --frames 5000 --stats-from 3000 --build-env H2_OPT=2 --fresh-data \
    --env H2_FEATURES=frame-scene.sink --env H2_RENDER_SCALE=4 --env H2_LIGHTING=$mode \
    --wait --to "$RUSH_KEEP/lab/$B-$mode"
done
```

Read fps, p50 and p99 frame time and the render thread's time per display frame; repeat at `H2_RENDER_SCALE=1` and with `--env H2_FPS=60`. For one frame, on a serve:

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

Each slice adds a dated entry to [notes/e3-lighting.md](notes/e3-lighting.md) with its commit and these numbers.

## The Lighting setting

| Where the row is read | Effect |
|---|---|
| D3D path ([e3-lighting-setting.md](e3-lighting-setting.md)) | the scaled backend keeps a copy of each guest shadow map at the nearest integral vertical render scale (at least 2x, `d3d::scaled::shadow_scale`) and compares it with one nearest tap (`PixelOptions::hard_shadow`): the Hard form of "The edge". The guest sees retail; only the edge of retail's own dynamic-light shadows changes |
| Native frame graph, at the display frame | the row supplies the mode; a scaled display hands the frame to the graph on any grid `RenderScale::replayable_display` accepts, whole or fractional, otherwise the D3D path draws it |
| An explicit private capture | supplies its own mode |

In E3 mode the graph's draws on fresh attachments draw the guest's shadow maps as tiles with the Sharp edge ([e3-lighting-cones.md](e3-lighting-cones.md)).

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

- Every number under "Budget". Only a frame on the lab laptop can give them.
- That retail's layer-11 receiver pass gives an acceptable terminator when a caster receives its own tile ([e3-lighting-sun.md](e3-lighting-sun.md)).
- That 16 lit lights are enough for a firefight to read as the footage does, and that four shadowed ones are.
- That a game frame's shadow maps reach the replay in a form the tile plan admits. A refused level stays retail; `tiles_refused` on a capture says whether any is.
- The Lighting setting's pictures in a level: the runs so far caught no frame where the two modes differ on screen ([notes/e3-lighting.md](notes/e3-lighting.md)).

## Related

[framescene.md](framescene.md), [native-renderer.md](native-renderer.md),
[native-lights.md](native-lights.md), [native-shadows.md](native-shadows.md),
[native-gels.md](native-gels.md), [high-resolution.md](high-resolution.md),
[shaders.md](shaders.md), [effects.md](../formats/effects.md),
[pstencil.md](pstencil.md), [pstencil-history.md](pstencil-history.md),
[e3-lighting-target.md](e3-lighting-target.md), [e3-lighting-sun.md](e3-lighting-sun.md),
[e3-lighting-lights.md](e3-lighting-lights.md), [e3-lighting-cones.md](e3-lighting-cones.md),
[notes/e3-lighting.md](notes/e3-lighting.md),
[notes/not-adopted.md](notes/not-adopted.md),
[the review](../reviews/2026-11-20-e3-lighting.md).
