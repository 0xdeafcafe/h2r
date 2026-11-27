# E3 lighting: sun shadows from objects

Status: Partly built. The tile plan, cache, atlas, caster and receiver draws, the scene's marks, the models and world receivers, and the frame graph's stage are built and tested on invented scenes. The materials' layer-10 and layer-11 programs are not used yet, and the scaled display and the mixed-size private draw take no sun stage. Live captures and timing are pending.
Code: `crates/rewrites/native-render/src/sun_tiles.rs` (plan and cache), `sun_tiles_gpu.rs` (atlas, tiles, receivers), `sun_casters.rs` (casters and receivers from the scene, world receivers), `frame_graph/sun.rs` (the stage). Tests: `crates/rewrites/native-render/tests/world_layers_gpu/sun_tiles.rs`, `tests/world_layers_gpu/frame_graph.rs`, the unit tests in `sun_tiles.rs` and `sun_casters.rs`.
Switch: `H2_LIGHTING` (`retail`), listed in [switches.md](../runtime/switches.md).

One depth map per caster replaces retail's object-shadow masks (**Design**). The section "Sun shadows from objects" of [e3-lighting.md](e3-lighting.md) is the summary; this page is the design and what is built.

| Stage | Where | Code |
|---|---|---|
| Mark each guest object shadow in the replay | scene sink | `SceneSink::with_object_shadows`, `render_bridge::scene_step::object_shadow_at` |
| Admit casters, name refusals | render thread | `sun_casters::gather`, `CasterRefusal` |
| Place tiles in a 4096-square depth atlas | render thread | `SunTilePlanner::plan` |
| Draw changed tiles, depth only | first stage of the frame | `SunShadows::draw_tiles` |
| Multiply the scene by each tile at the object-shadow point | scene stage | `frame_graph::sun` |

## How it works

### Casters

- **Which objects.** **R**. With an active lightmap and the shadow mode enabled, `0x000335e0` calls `0x00033400`. The gather applies the view-mode bits and `0x0003d7f0`'s object, definition, distance and fade tests. It admits up to 32 objects, then ranks them with a history and a target of four.
- **The draw.** The draw at `0x00032350` requires a root object and a valid visible hierarchy. Creature roots take a separate ground-shadow path at `0x00032ce0`, which E3 keeps for creatures.
- **Offscreen casters.** Once per game view at layer 3, `scene_extract::layout::nearby_casters` walks live root objects in slot order. It keeps up to 32 outside-frustum spheres whose sweep away from the light meets the view within eight world units. It checks the definition's shadow policy and uses the default or owner-checked cache sample from `0x0003dc90`. Node matrices are copied only after the sweep passes.
- **Where they live.** The extra instances are in `FrameScene::nearby_casters`. Each caster without a drawn guest mask draws on opaque parts of visible BSP clusters after that view's opaque slot, with the same atlas and W-depth test. Test: `a_caster_behind_the_camera_shadows_the_ground_in_view`.

### Direction and strength

**R**. The ordinary draw resolves its lighting sample at `0x0003dc90` from a valid object lighting-cache entry or the default lighting. It normalises the vector at sample `+0x0c`. Strength is the positive scalar at sample `+0x1c` times the current object fade and the ranked entry's faded weight, and the product must clear the draw threshold. The renderer uses that direction and final strength for the tile ([object chain](native-shadows.md#object-chain)).

### The map

| Property | Value |
|---|---|
| Shape | orthographic depth tile fitted to the caster's bounding sphere along the direction |
| Edge | next power of two above 1.5 times the sphere's diameter in display pixels, 128 to 2048, so a texel is never larger than a pixel on the caster |
| Atlas | one 4096-square `Depth32Float` atlas, tiles packed each frame by size. **Design**: shared with the local lights' tiles; built: the sun has its own atlas |
| Overflow | the largest class halves and `atlas_halved` counts it |
| Memory | 64 MB as 32-bit depth |

### Drawing and receiving

- **Caster draw.** **Design**: select the material's layer-10 depth writer at the LOD drawn, with the blended palette rows and its alpha test, and keep the retail shadow for a caster with a part that has no supported depth writer. Built: depth only, the caster's world-space triangles (blended, skinned) through the tile's rows under a viewport and scissor on its rectangle. The material's writer and alpha test are not used yet.
- **Receivers.** The caster itself, the instances whose spheres meet the tile's box swept along the direction, and the cluster parts that do. **Design**: each is drawn with its material's layer-11 receiver pass, which samples a depth map and multiplies the frame (**R**: implementation 3 in [native-shadows.md](native-shadows.md)), at equal depth.
- **Coverage of that pass.** **R**. Across 165 distinct template names, 72 have a layer-10 depth writer and 69 have it and the multiplying receiver in the same LOD. Only 15 have the pair in every LOD. At the selected LOD a missing receiver uses an E3 receiver based on the material's depth vertex path, admitted only when it reproduces the same position, normal and coverage inputs ([counts per map](notes/e3-lighting.md)).
- **Where.** At retail's object-shadow point, before layer 7, so self-illumination and environment maps are added over the shadow as in retail. Retail's mask chain is not drawn for a caster with a tile.
- **Self-shadowing** is the caster receiving its own tile. **R**. Implementation 3 writes the vertex normal dotted with hardware `c[41]` to diffuse RGB. Its six populated vertex classifications use the normal in world space, transformed by the rigid or skinning rows where needed. The shadow setup at `0x00048f85` supplies the direction towards the light. The pixel combiner interpolates between unity and the depth comparison using that vertex colour. A triangle whose three vertex terms clamp to zero keeps destination RGB, and at a terminator the interpolated gate can be fractional. The alpha-only receivers have a different gate ([program evidence](notes/e3-lighting.md)).
- **Not done.** The first-person model neither casts into tiles nor receives them; it has its own projection.

## Built

### The replay's marks

The scene sink writes `ReplayOp::ObjectShadow(Some(index))` before the first op the guest records inside an object shadow, and `ObjectShadow(None)` before the first op after it. A shadow runs from its `0x00032350` call to the end of its layer 11 (the `Site::Object` layer end), or to the next shadow, the next light's passes or the scene's seal. The index is the shadow's place in `object_shadows`.

The marks are written in both modes, since a capture picks its lighting per frame. They draw nothing: every replay pass treats them as no-ops and they do not cut the frame graph's pass rows. Tests: `object_shadow_marks_wrap_each_shadows_ops_and_change_nothing_else` (`crates/platform/d3d/tests/scene_sink.rs`) and `object_shadow_marks_leave_the_retail_draw_and_its_passes_unchanged` (`tests/world_layers_gpu/frame_graph.rs`).

### The scene's casters

Each `0x00032350` call the hook matches is an `ObjectShadow` of its view. It carries the caster's handle and ranked fade (`identity`), its layers 10 and 11 as groups with the projection they read, whether the caster is a creature (object type 12, header `+3`), and the light weight read at layer 11. **R**. The mask branch stores the weight at `0x00032912` (`movss [0x00485a30], xmm0`, loaded from `[esp+0x14]` at `0x0003290a`; `h2 disasm 0x00032905 40`), the word the material inputs call the light's weight. `ObjectShadow::towards_light` is `c[41]` of layer 11's projection (**R**, `0x00048f85`).

`sun_casters::gather` admits a shadow whose caster is an instance of the view with node matrices and a loaded model, with a direction and a weight above retail's 0.09. Its mesh is the model's opaque parts skinned on the CPU with the instance's nodes. The tile's strength is the weight and its rows hash is over the node matrices. A creature or a caster missing any of these is refused by name (`CasterRefusal`) and keeps retail's mask.

### The models and the world receivers

| What | Built by | Offered as | Read by |
|---|---|---|---|
| Caster models | `crates/extensions/game/src/native_objects_glue.rs` (`CasterSet`), default variant via `model_load::default_model` (the rules `load_object_model` uses: one section per region, highest LOD, a run-time permutation refused) | `sun_casters::set_models` for the map's epoch | `sun_casters::models_for`, for a scene of that epoch only, keyed by object definition |
| World receivers | `crates/extensions/game/src/native_world_glue.rs` (`load_ready`, building `sun_casters::SunWorld`): every opaque part of the BSP, placed, before the native layer-3 filter | `sun_casters::set_world` for the map's epoch | `sun_casters::world_for` (`WorldReceivers::current`) while the BSP it was loaded for is the guest's |

Retail never reads either. A BSP switch withdraws the world until the next one loads. A part's mesh is built at its first ask and kept; the frame graph uploads it once per device and drops the upload when its world is gone.

`sun_casters::receivers` takes what the shadow's layer 11 draws on, each once: an object skinned like a caster, and a structure cluster's or placement's part from the world receivers. The layer must be in the scene whole. A structure part the world does not hold (a transparent part, another BSP's) or no world for the scene refuses the shadow (`CasterRefusal::WorldReceiver`) and it keeps retail's mask. Tests: `world_entries_receive_from_the_loaded_structure` and `world_receivers_serve_only_their_maps_current_bsp` (`sun_casters.rs`), `a_box_shadows_the_structure_it_stands_on` (`tests/world_layers_gpu/sun_tiles.rs`), `e3_draws_a_shadow_on_the_structure_and_keeps_the_mask_without_its_world` (`tests/world_layers_gpu/frame_graph.rs`).

### The plan and the cache

`SunTilePlanner::plan` takes each caster's sphere, screen diameter, direction towards the light, strength and a hash of its blended rows (`rows_hash`). It sorts the tiles largest first, then by caster, and places them along a Morton curve of 128-texel cells, so they never overlap and the same casters get the same rectangles each frame. While their area exceeds the atlas, every tile of the largest class halves (`atlas_halved`). A caster with no usable sphere, direction or strength, or with no room at 128, is refused, counted in `tiles_refused`, and keeps retail's shadow.

The atlas persists. A tile is cleared and redrawn only when its rows hash, direction, rectangle or bounding sphere changed (`sun_tiles_redrawn`). `SunShadows::draw_tiles` refuses a redraw whose caster mesh is missing and forgets the atlas's contents, so the next frame draws every tile.

### The receiver program

The receiver is E3's own program, the fallback for a material with no layer-11 receiver. Its vertex gate is the world normal dotted with the direction towards the light, clamped to `[0, 1]`. The pixel term is `1 - gate * strength * (1 - lit)`, with `lit` the Sharp comparison ([the edge](e3-lighting.md#the-edge)) kept inside the tile's rectangle. It multiplies the frame's RGB and does not write alpha, and a triangle whose three gates are zero multiplies by exactly one.

The lookup is offset 1.5 texels along the normal with one texel of depth bias, so a caster does not shade its own lit faces. Receivers draw with `LessEqual` and no depth write over the frame's depth, through the same invariant vertex transform that laid it, once for each tile whose box they meet.

### The frame graph's stage

For an E3 draw on the guest-draw pass (`draw_private`, `draw_display`, `draw_display_on`), `frame_graph::sun` takes each marked shadow with its caster admitted, its receivers drawn, a 24-bit scene depth, a viewport, ops that end on the scene's colour and depth, and nothing they write or sample read after them.

- The planner places the admitted casters' tiles, and the frame's first stage draws the tiles that changed.
- For each shadow with a tile, its marked ops are not applied (binds, samples and flare queries excepted). A scene stage after its last op multiplies the scene's colour by its receivers against its tile.
- Receivers write the guest's W-buffer depth for their view depth (full scale, `1 / far`, floored), less `RECEIVER_DEPTH_BIAS` (0.005 world units) towards the eye, and test it `LessEqual` against the guest's depth.
- Each device keeps its atlas and planner. A draw that fails, or a plan no stage drew, invalidates the atlas.
- Every other shadow keeps its ops and retail's mask, its cause named (`frame_graph::SunSkip`). The scaled display and the mixed-size private draw take no sun stage: their shadows keep retail's mask.

Test: `e3_skips_an_admitted_shadows_mask_and_retail_replays_it_unchanged` (`tests/world_layers_gpu/frame_graph.rs`).

### Counters

`E3Report::add_sun`: caster draws are `e3.sun_tiles`, receivers are `e3.sun`, and `sun_tiles`, `sun_tiles_redrawn` and `atlas_halved` are the plan's.

## Not verified

- An outdoor gameplay tick differs from retail around every caster, and no retail mask pass is drawn for a caster with a tile.
- GPU ms with 8, 16 and 32 casters (`sim.spawn`) at 1x and 4x, and `sun_tiles_redrawn` at 60 fps.
- The L8 live check: with the caster behind the camera, compare 1x retail and E3 captures, and read `render.stats` `private_frame.e3.sun_tiles_offscreen` above 0 and `e3.sun` draws. Lab sim-thread frame-step increase under 0.1 ms.
- That retail's layer-11 receiver gives an acceptable terminator when a caster receives its own tile. E3's own receiver is checked on an invented L-shaped caster only (`an_l_shaped_caster_shadows_itself`).

## Related

[e3-lighting.md](e3-lighting.md), [native-shadows.md](native-shadows.md),
[framescene.md](framescene.md), [notes/e3-lighting.md](notes/e3-lighting.md).
