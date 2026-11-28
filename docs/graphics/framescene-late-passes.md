# FrameScene slice 5: fog, water and post, typed

Status: Partly built. Slice 5 of [framescene.md](framescene.md) (section 10): the late passes are in the scene, and the fog and the water draw from it on the render thread. No entry is replaced: as in slice 4, rule 1 of 8.2 waits on the census corpus, and the post stage draws through the device's swallow.
Code: `crates/rewrites/frame-scene/src/late.rs`, `crates/rewrites/scene-extract/src/late.rs`, `crates/extensions/game/src/{native_fog_glue,native_water_glue,native_post_glue}.rs`.

| Pass | In the scene | Drawn on the render thread | Entry replaced |
|---|---|---|---|
| Fog (layer 16) | `View::fog` (`FogState`) | yes, one queued scene stage | no |
| Water (layer 6) | `View::water`, `View::water_composite` | yes, one queued scene stage | no |
| Post | `View::post` (`PostParams`) | glow and effects, native passes from the Rust bodies ([native-post.md](native-post.md)) | no |

## The types

`frame_scene::late`. `View::fog` and `View::water` are a `LateLayer`:

- the layer's own gate (the fog's `0x00044582` test, the water's bit 4 of `0x004b9fe0` clear);
- the fog words the device holds at the layer's draw, by slot;
- the layer's items in the current list;
- each material the stage draws with and its words resolved over the device's (`LateMaterial`);
- the structure parts the stage draws in item order (`LatePart`);
- whether the stage took the layer.

`View::water_composite` holds the teardown's four u16 corner indices per quad, in submitted order, independently of layer 6. The indexed stage matches them to the rebased vertex window before expanding the quads. The summary hash includes their order.

`View::post` is `PostParams`: what `render_screen_effects` was asked to draw.

- Its two stack bytes, the view kind, the scenario's screen-effect count and the loop's gates, read as `scene_extract::layout::post::PostFrame` reads them.
- The colour pass `0x000214f0`'s seven arguments.
- Each screen effect `0x0002d140` under it (`PostEffect`: its datum, its four values by value, its grid).

The late passes run after the view's opaque block, after the first person's view can open. The builder writes them into the scene's last view of the guest view's key (`SceneBuilder::set_late`, `set_post`); they extend no view range. The summary hash covers them.

## Extraction

The fog's and the water's glue (`boot::native_fog_glue`, `native_water_glue`) read the device's fog words, resolve each material's words over them and plan the skip. Then `render_bridge::scene_step::late_layer` writes the view's layer (with `scene_extract::late::layer_items`) and hands back the scene's copy. The pass is built from that copy: the parts it draws and each material's fog unit.

- For mixed fog and water lists the guest sorts all the entries, and the native renderer offers indexed ranges at the geometry emitter with the handler's state bound.
- The scene stage clears masks only for a fully accepted layer.
- A layer whose gate does not hold, with no structure item, or that the glue leaves to the guest is written as the guest draws it (`late_kept`).
- The frame step observes `RENDER_SCREEN_EFFECTS` for `PostParams`; `native_post`'s colour-pass and screen-effect hooks add their calls (`post_colour`, `post_effect`).

## Recording on the render thread

With a render thread that records scene stages, the fog and the water each queue one (`native_fog_glue::commit_late`: `Device::reserve_scene_stage`, as the world's in [framescene-slices-0-2.md](framescene-slices-0-2.md)). The render thread prepares and records the pass, the water's scene copy and sampled targets included. Not taken, the pass is reserved on the sim thread.

The load is pinned until the masks and the commands commit, and only then does `late_drawn` mark the layer taken. Any refusal before the commit leaves the layer's items to the guest. A stage the render thread cannot record is slice 2's named error and stands the stage down.

## Coverage

- `render.scene` reports each view's `fog` and `water` (gate, items, materials, parts, whether the stage took the layer, the items the guest keeps, the device words that read) and `post`.
- Counters: `scene_fog_*`, `scene_water_*`, `scene_post_*`, `scene_stages_{fog,water}`, `scene_dropped_{fog,water}`.
- The entry census names the late passes' calls `layer.scene.16` and `layer.scene.6` (`entry_census::late_site`) and counts `colour_pass` and each `screen_effect` under `screen_effects`.

## The post passes, replayed

The glow (`GlowPass`) and each screen call (`ScreenPass`, both `game::native_post_glue`) are native passes of `Stage::Post` (`Device::prepare_external_pass`), committed at the call ([native-post.md](native-post.md#the-native-passes)). Both record again on any copy's attachments (`replays_scene`), so the display copy replays them on its own attachments.

- No guest draw is resolved: the raster is the plan's.
- The glow takes its down-sample's entry from the view's slot, copied with alpha one where the guest reads it as `X8` (`OpaqueEntry`), as the device's framebuffer snapshot does.

## Not built

- The replacement of layers 16 and 6 and of `render_screen_effects` (8.2: their census over the corpus, the table of writes, the differentials).
- The post stage drawing from `PostParams` on the render thread, in place of the Rust bodies' passes at the call.
- The deletion of the three stages' glue.

## Tests and gate

Synthetic tests:

| Crate | Tests |
|---|---|
| `frame_scene::late` | `a_views_late_layers_keep_their_words_and_parts_and_count_what_the_guest_keeps`, `a_post_call_keeps_its_colour_pass_and_effects_in_order`, `the_late_passes_count_in_the_hash` |
| `scene_extract::late` | `a_late_layers_items_are_those_with_its_bit_in_the_current_list`, `a_post_call_reads_its_gates_and_a_screen_effect_its_values` |
| `render_bridge::entry_census` | `the_late_passes_fog_and_water_calls_are_the_scenes`, `the_post_callees_nest_under_the_screen_effects` |
| `boot::scene_glue`, `agent_proto` | `a_scene_summarises_per_view`; `a_views_late_passes_print_only_when_they_ran` |

**Gate.** As slice 4 ([framescene-layer-plan.md](framescene-layer-plan.md)). No guest surface other than the back buffer, depth and a carried stage's own targets is bound in a game view (from `render.frame`).
