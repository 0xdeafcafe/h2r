# Graphics: not adopted

Evidence for [native-renderer.md](../native-renderer.md) and
[graphics.md](../graphics.md). The switches that remain are in
[switches.md](../../runtime/switches.md).

Experiments and switches that were built, measured and removed, one row
each, so that the next lane does not build them again. The native GPU
renderer has one switch, `impl.render.native` (on; `=off` turns every
stage off together). Naming a removed key in `H2_FEATURES` changes nothing.

## Removed

Do not re-add these without new evidence; each commit message has the
numbers.

| Commit | What | Why |
|---|---|---|
| `5b6b9aad7`..`f1e9f43f1` | every per-stage and per-walk native key (`impl.render.native_world`, `_world_programs`, `_world_layers`, `_world_skip_build`, `_objects`, `_object_walk`, `_first_person(_walk)`, `_transparents(_walk)`, `_decals(_walk)`, `_particles(_walk)`, `native_effects_walk`, `_lights`, `native_light_walk`, `_gels`, `_shadows`, `_fog`, `_water`, `_post(_walk)`, `_ui`, `_hud(_walk)`, `_residual`, `_scene_lists`) | all on with the master since 2026-11-20 after the all-native A/B (identical frames, 15.4 against 25.6 fps); `impl.render.native=off` is the one A/B left |
| `7ec468498` | `impl.render.native_lights_split` and the split path | off-by-default diagnostic, not in the all-native A/B |
| `e3dc77f44`, `98a9b9055` | the native world's `=off` stage-1 look (base map times lightmap) and `H2_NATIVE_WORLD_TRACE`, `_TRACE_EVERY`, `_NO_CULL`, `_PROBE`, `_DUMP`, `_OWNERS`, `_INHERIT_CULL` | A/B and investigation aids for a settled stage |
| `63c5e226a`..`86c3480c7`, `729ee00a5`, `ec2f3e06a` | the per-stage compares, probes and censuses: `H2_NATIVE_{DECALS,FIRST_PERSON,TRANSPARENTS,PARTICLES,FOG,GELS,SHADOWS,LIGHTS,HUD,LISTS,OBJECTS,WORLD_LAYERS}_COMPARE`, `H2_NATIVE_*_PROBE`, `H2_NATIVE_OBJECTS_EXCEPT`, `H2_NATIVE_WATER_CENSUS` (it printed by default), `H2_NATIVE_WORLD_LAYERS_OWNERS`, the residual census, `H2_POST_AB`, `_AB_AT`, `_AB_COUNT`, `H2_POST_TRACE`, `H2_POST_SHAPES`, and the `render.shadows` agent method | each stage matched the D3D path in the A/B; the compares were for bringing stages up |
| `414c07c21` | the d3d device trace (`Device::trace`, `Traced`), `uniform_compare` and `Swallow::capture` | only the removed compares and `H2_POST_TRACE` used them |
| `ebaa4a0b2` | the stage 7a ports `impl.render.views`, `layer_draw`, `layer_entries`, `game_view`, `scene_passes`, `states`, `states_skip`, the `render-loop` feature and the agent's `paths` counters | off since built; none saved measurable time and each cost a hooked call ([render-loop.md](../render-loop.md)). `RenderStates` stays: the native world's BSP walk runs it |
| `9df79b8e3` | `H2_PIPELINE_RENDER` | an alias of `impl.render.pipelined_frame` |
| `33b6dde1e` | `H2_D3D_DRAW_SHARING`, `_LAZY_DEPTH`, `_LEAVE_READ`, `_ALIAS_MIRROR`, `_MIRROR_DEPTH`, `_VISIBILITY_POLL`, `H2_VERTEX_CACHE=0` | in-game A/Bs of defaults that won (lazy depth 17.07 against 14.78 fps; mirror 10.15 to 5.24 ms; the visibility wait was 8.3% of the game thread). The `Device::set_*` setters stay for capability fallbacks and tests |
| `41044ef42` | `H2_GFX_SWAP_FINISH`, `_SIDE_SUBMIT`, `_STAGED_UPLOADS`, `_VERTEX_SLABS`, `_IDLE_COUNTED_PASS`, `H2_D3D_GPU_DEPTH_ALIAS`, `H2_VERTEX_FETCH`, `H2_GFX_MERGE_PASSES` and its `fast` rule | defaults that won; `fast` was superseded by `loose` (193 render passes to 41 on Old Mombasa). Per-draw and exact merging stay as setters for tests and `H2_GFX_MERGE_CHECK` |
| `ce288e75` | `H2_GFX_SUBMIT=batch\|<us>`, `GfxBackend::set_submit_after`, `set_submit_per_pass`, `SUBMIT_AFTER_PASSES` | slower in game: Old Mombasa 4x 30.2 against 33.5 fps per pass (3e8276b2); off since |
| `2d562b08` | the lazy 1x reference at render scale (`H2_LAZY_REFERENCE`, `ScaledBackend::set_lazy_reference`), `scale_bench` | 1-10% saved on unread frames, 15-25% lost on frames with a back-buffer snapshot, which Halo's frames take (09590f5c); off since |
| `7e4529f0` | `GfxBackend::without_shaders` | no caller |
| `ff6f7d6b` | `impl.render.layer_sort` and `impl.render.transparent_sort`: `engine::render_loop::sort` and `sort_indexed`, their differential tests, the `render-loop` feature's two placements and path counters | the profile puts both sorts at about 0.01 ms a frame (30 samples over 142 s, Old Mombasa 2026-11-17), and each port cost a hooked call per sort; off since built. The points `LAYER_SORT` and `TRANSPARENT_SORT` stay in `engine::points` as research notes |
