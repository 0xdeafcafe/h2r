# Render switches

Every switch, feature and experimental path on the render side, what it
is, whether it is on by default, what the docs measured, who uses it, and
whether it stays. Written 2026-11-18 as a cleanup pass: what lost or is
dead goes, and the rest is listed here so the next lane can tell an
experiment from the path the game takes.

The native stages default on. Set `H2_FEATURES=impl.render.native=off`
to disable them as a group. An explicit stage switch wins in either
order, so `impl.render.native=off,impl.render.native_world=on` restores
only the world and its material programs. Those programs follow the
world by default and can be disabled separately. The UI stage requires
the master switch. The older `impl.render`
switch controls the lifted render-loop ports independently.

Verdicts: **keep** (the default path, a user setting, a diagnostic or a
test reference), **remove** (done; see "Removed"), **ask** (a decision
for the owner, with a recommendation).

A note on "old behaviour" opt-outs. Many d3d and gfx defaults came with an
environment variable that restores what came before. Most of those are
kept: the old path is still the one a backend without the capability
takes, or the tests compare the new path against it byte for byte. The
environment variable is then only the in-game A/B for a recent default.

## `H2_FEATURES` render features

| Switch | What | Default | Measured | Users | Verdict |
|---|---|---|---|---|---|
| `impl.render.reproject` | E1, rotation-only reprojection of frame N's world image for in-between frames (`d3d::interp`, [high-framerate.md](../game/high-framerate.md) section 8), and its headless path (`install_headless`, `h2 agent render`) | off in code; the user's choice for 60 fps | warp and world-image ms in its exit line; not compared in game against D yet | `boot::d3d_glue`, agent `render.stats` | keep |
| `impl.render.reproject_ahead` | E1 with the turn carried on past N (extrapolation, aim only) | off | none | `boot::d3d_glue` | ask |
| `impl.render.interpolate` | D, host replay of frame N's draws with interpolated matrices ([high-framerate.md](../game/high-framerate.md) section 7) | off | `replay mode` ms in its exit line; geometry replay per display frame does not fit 60/120 yet (section 3) | `boot::d3d_glue` | ask |
| `impl.render.occlusion` | Hi-Z occlusion culling of object setups from the previous frame's depth ([occlusion-culling.md](occlusion-culling.md)) | off (exact key only) | not run in game; expected saving depends on the culled share | `feature::occlusion`, `boot` capture glue, `d3d` depth grid | ask |
| `impl.render.native` | default for every native GPU stage and its optional comparison hooks. Explicit stage switches override it | on | | `feature::switches` | keep |
| `impl.render.native_scene_lists` | camera and light visibility lists, object LOD inputs and supported records, and transparent entry publication and ordering | on with the master native renderer | scene list counters and `H2_NATIVE_LISTS_COMPARE=1` | `feature::native_scene_lists` | keep |
| `impl.render.native_world` | the static world (structure BSP, layer 3) drawn by the native GPU renderer, the game's world entries skipped ([native-renderer.md](native-renderer.md)) | **on since 2026-11-20** (`=off` compares against the D3D path); runs on the pipelined render thread | all-native 03a A/B, 2026-11-19/20: identical frames, 15.4 against 25.6 fps at equal load ([native-renderer.md](native-renderer.md), "The all-native push") | `feature::native_world`, `boot::native_world_glue`, `d3d` external pass | ask |
| `impl.render.native_world_programs` | the native world's look: each world material whose layer-3 pass translates draws with its own vertex program, pixel shader, stages, constants and render states, and every other material keeps its D3D draws; `=off` draws stage 1's base map times lightmap instead, for the A/B only ([native-renderer.md](native-renderer.md), "Order of work" 4) | on with the native world | the all-native A/B above; the load lines give the coverage and each refusal's reason | `native_render::program`, `boot::native_world_glue` | ask |
| `impl.render.native_world_layers` | the world's other base layers (1, 5, 7, 8) and the combined materials in layer 3 recorded natively at the resolved indexed draw ([native-renderer.md](native-renderer.md), "Native opaque layers") | **on since 2026-11-20** (`=off` compares against the D3D path) | the all-native A/B above | `feature::native_world_layers`, `boot::native_world_layers_glue` | ask |
| `impl.render.native_objects` | objects' render models drawn natively instead of their layer-3 entries ([native-renderer.md](native-renderer.md), "Stage 2") | **on since 2026-11-20** (`=off` compares against the D3D path) | the all-native A/B above | `feature::native_objects`, `boot::native_objects_glue` | ask |
| `impl.render.native_object_walk` | consumes complete runs of third-person objects from the sorted layer-3 scene list before guest shader and draw preparation | on with the native renderer and objects stage | set `off` to compare against resolved object draws | `feature::native_objects`, `boot::native_objects_glue` | ask |
| `impl.render.native_transparents` | persistent native vertex bindings for sorted BSP, placement and object geometry, at each resolved indexed draw ([native-renderer.md](native-renderer.md), "Stage 3: transparents") | **on since 2026-11-20** (`=off` compares against the D3D path); the return observer uses the pipelined renderer's game-thread fallback | the all-native A/B above. Object entries also require the native objects switch. | `feature::native_transparents`, `native_render::transparent_streams`, `boot::native_transparents_glue` | ask |
| `impl.render.native_decals` | dynamic decal callbacks scoped for the ordered `d3d` draw admission | **on since 2026-11-20** (`=off` compares against the D3D path) | the all-native A/B above | `feature::native_decals`, `boot::native_decals_glue` | ask |
| `impl.render.native_particles` | the particle helper's single, fully captured draw replaced; the helper and its parents still run | **on since 2026-11-20** (`=off` compares against the D3D path) | the all-native A/B above | `feature::native_particles`, `boot::native_particles_glue` | ask |
| `impl.render.native_lights` | dynamic light passes: resolved geometry draws inside a light admitted natively, the per-light passes skipped ([native-renderer.md](native-renderer.md), "Stage 4") | **on since 2026-11-20** (`=off` compares against the D3D path) | the all-native A/B above | `feature::native_lights`, `boot::native_lights_glue` | ask |
| `impl.render.native_lights_split` | split lights: the host validates the entire sorted array, reserves the exact supported prefix and skips those slots ([native-renderer.md](native-renderer.md), "Stage 4") | off (exact key only; a diagnostic of the split path, not a stage) | not in the all-native A/B (kept off in it) | `feature::native_lights` (`SPLIT_SWITCH`) | ask |
| `impl.render.native_gels` | shader-gel production at its original template and quad sites ([native-renderer.md](native-renderer.md), "Stage 4") | **on since 2026-11-20** (`=off` compares against the D3D path) | the all-native A/B above | `feature::native_gels`, `boot::native_gels_glue` | ask |
| `impl.render.native_shadows` | the shadow-map layers, queued with an exact D3D fallback ([native-renderer.md](native-renderer.md), "Stage 4") | **on since 2026-11-20** (`=off` compares against the D3D path) | the all-native A/B above | `feature::native_shadows`, `boot::native_shadows_glue` | ask |
| `impl.render.native_fog` | planar fog, the scene list's layer 16 ([native-renderer.md](native-renderer.md), "Stage 4b") | **on since 2026-11-20** (`=off` compares against the D3D path) | the all-native A/B above | `feature::native_fog`, `boot::native_fog_glue` | ask |
| `impl.render.native_water` | the water edge blend, layer 6 ([native-renderer.md](native-renderer.md), "Stage 4b: water and the rest") | **on since 2026-11-20** (`=off` compares against the D3D path) | the all-native A/B above | `feature::native_water`, `boot::native_water_glue` | ask |
| `impl.render.native_first_person` | the first-person arms and weapon, the other layer-3 caller's items ([native-renderer.md](native-renderer.md), "First person versus third person") | **on since 2026-11-20** (`=off` compares against the D3D path) | the all-native A/B above | `feature::native_first_person`, `boot::native_first_person_glue` | ask |
| `impl.render.native_post` | the view's post passes, glow and screen effects ([native-renderer.md](native-renderer.md), "Stage 5: post and HUD") | **on since 2026-11-20** (`=off` compares against the D3D path) | the all-native A/B above | `feature::native_post`, `boot::native_post_glue` | ask |
| `impl.render.native_ui` | Menu, HUD, loading and overlay draws with ordered per-draw fallback ([native-renderer.md](native-renderer.md#native-ui)) | on with the master. The old `impl.render.native_hud=off` disables it too | Pending runtime A/B | `feature::native_hud`, `boot::native_hud_glue` | keep |
| `impl.render.native_residual` | view scopes for the native renderer's remaining D3D calls (the sky model's indexed strip among them) | **on since 2026-11-20** (`=off` compares against the D3D path) | the all-native A/B above | `feature::native_residual`, `boot::native_residual_glue` | ask |
| `impl.render.views` | Rust `render_views` (`engine::render_loop`) | off | orchestrator: no saving, one runner round trip per call ([render-loop.md](render-loop.md)) | `feature::render_loop` | ask |
| `impl.render.layer_draw`, `impl.render.layer_entries` | the layer draw `0x00044550` and its entry loop ([layer-draw.md](layer-draw.md)) | off | none in game; hooked calls leave the direct path | `feature::render_loop` | ask |
| `impl.render.game_view`, `impl.render.scene_passes` | Rust `render_game_view` and `render_scene_passes` (stage 7a) | off | "change no draw and save no time on their own" ([render-loop.md](render-loop.md)) | `feature::render_loop`; D and E1 name the pass tag seam | ask |
| `impl.render.states`, `impl.render.states_skip` | the shader draw's render-state list `0x0001a170`, exact or skipping redundant `SetRenderState_Simple` calls | off | synthetic: 12-53% of Simple calls skipped depending on draw order; the hook costs one exit from the direct path per draw; not measured in game | `feature::render_loop` | ask |
| `render-timing` (and `render-timing.deep`) | times the render front-end stages and sites against the frame stats | off | the tool that measured the above ([gameplay-budget.md](../perf/gameplay-budget.md)) | `h2 play`, agent capture, profiling | keep |
| `render-census` | the guest-memory census that decided pipelined rendering ([pipelined-render.md](pipelined-render.md)) | off | its own results | pipelined-render work | keep |
| `render-loop` | the feature that places the `impl.render.*` ports above | on (places nothing unless a port is switched on) | | boot | keep (with the ports) |

## Environment variables

### Paths the game takes

| Variable | What | Default | Measured | Verdict |
|---|---|---|---|---|
| `H2_RENDER_THREAD=0` | backend work on the `d3d-render` thread ([render-thread.md](render-thread.md)) | on | 17.07 fps against 10.58 sync on The Heretic | keep (`0` is the synchronous reference tests use) |
| `H2_RENDER_THREAD_STATS` | print the render thread's waits at exit | off | | keep (diagnostic) |
| `H2_RENDER_SCALE` | render scale 1-6 (`d3d::scaled`, [high-resolution.md](high-resolution.md)) | 1 | | keep (Video setting) |
| `H2_LIGHTING` | `retail` or `e3` ([e3-lighting.md](e3-lighting.md)) | retail | | keep (Video setting) |
| `H2_FPS` | frame rate target; above 30 enables in-between frames | 30 | | keep (setting) |
| `H2_PIPELINE_RENDER` (+ `H2_PIPELINE_WATCH`, `_PREDICT_FRAMES`, `_SEED_REPLAY`, `_REFUSE`, `_CONFLICTS`, `_DEFER_IO`, `_ROUNDS`) | the game's render front-end one frame behind the sim on a second thread ([pipelined-render.md](pipelined-render.md)) | off | 33-34 fps, no better than stock; about 1.3 ms of overlap a frame, and the hand-back costs as much ("Where it stands") | keep: a research switch with a documented next step |
| `H2_PIPELINE_CACHE` | persistent pipeline cache file, `off` disables | on | | keep |
| `H2_GFX_COMPILE_THREADS` | pipeline compile workers, `0` compiles on the game thread | half the cores, 2-4 | | keep |
| `H2_VERTEX_CACHE=0`, `H2_VERTEX_CACHE_MB` | vertex and index snapshot caches ([d3d.md](d3d.md)) and their budget | on, 128 MB | | keep (`0` is the copy-every-draw reference) |
| `H2_SMOOTH_TEXT` | glyph smoothing for host-drawn text | | | keep |
| `H2_RENDER_OVERRIDES` | hot render overrides ([hot-iteration.md](hot-iteration.md)) | unset | | keep |

### Merged passes

| Variable | What | Default | Measured | Verdict |
|---|---|---|---|---|
| `H2_GFX_MERGE_PASSES=loose` | no merge-rule splits | **default since 2026-11-18** | Old Mombasa 1000-draw frame: 193 render passes to 41; 12 pixels of a 970-draw frame off ([graphics.md](graphics.md)) | keep |
| `H2_GFX_MERGE_PASSES=0` | one render pass per draw | off | about 2.5-3x slower; the exact reference | keep (reference, `H2_GFX_MERGE_CHECK`) |
| `H2_GFX_MERGE_PASSES=exact` | a discarding draw gets its own render pass | off | matches per-draw on every menu frame checked; ~200 more passes in a level | keep (pixel-fidelity acceptance tests) |
| `H2_GFX_MERGE_PASSES=fast` | split a discarding draw only after a partly masked draw | off (default until 2026-11-18) | superseded by `loose` | ask |
| `H2_GFX_MERGE_CHECK=N` | render frame N merged and per draw and report the differences | off | | keep (diagnostic) |

### Old-behaviour opt-outs (d3d, gfx)

Each restores the path a newer default replaced. Kept: the old path is
still taken where the new one cannot apply, or a test compares the two.

| Variable | Restores | Measured win of the default | Why the old path stays |
|---|---|---|---|
| `H2_SINGLE_COPY=0` / `graphics.single_copy=false` | separately rendered 1x readbacks at scales above 1 | default saves one recording per duplicated draw, including native passes; frame-time gain unmeasured | scaled colour, depth and visibility samples can differ from 1x at edges; use for exact captures and replays |
| `H2_GFX_SWAP_FINISH=full` | `D3DSWAP_FINISH` waits for this frame too | GPU overlaps the next frame's encode | `SwapFinish::Full` makes `occlusion_grid` deterministic |
| `H2_GFX_SIDE_SUBMIT=1` | side work submitted on its own | two submissions per call saved (about 0.1 ms each) | it is what a render scale above 1 uses |
| `H2_GFX_STAGED_UPLOADS=0` | a queue write per persistent buffer | most of 537 queue writes at 2000 draws | the pool's tests compare staged and not |
| `H2_GFX_VERTEX_SLABS=0` | a buffer per persistent list | wgpu-core per-pass bookkeeping ([gameplay-budget.md](../perf/gameplay-budget.md)) | lists over a slab still get their own buffer; tests compare |
| `H2_GFX_IDLE_COUNTED_PASS=1` | a render pass for a counted draw that writes nothing | one pass fewer per flare test | `round7` compares the two |
| `H2_D3D_DRAW_SHARING=0` | a state copy per draw | ([render-thread.md](render-thread.md)) | `draw_main` bench compares |
| `H2_D3D_LAZY_DEPTH=0` | depth write-back at every binding change | 17.07 against 14.78 fps (render thread) | a backend that cannot sample depth takes it |
| `H2_D3D_ALIAS_MIRROR=0`, `H2_D3D_MIRROR_DEPTH=0` | write-backs between aliased levels | 10.15 to 5.24 ms (render thread, bench) | levels the mirror cannot fill write back as before |
| `H2_D3D_GPU_DEPTH_ALIAS=0` | depth aliases read back on the CPU | 3.37 to 1.21 ms (bench) | formats the GPU pass does not cover |
| `H2_D3D_VISIBILITY_POLL=0` | visibility results waited for | the wait was 8.3% of the game thread | deterministic answers for tests and replays |
| `H2_D3D_LEAVE_READ=1` | a lazy level left as a target is re-read every time | | `rtt_direct` tests compare (undocumented elsewhere) |
| `H2_VERTEX_FETCH=cpu` | vertex formats converted on the CPU | | fixed-function draws always use it; `vertex_fetch` compares |
| `Device::set_rtt_direct(false)` (no variable) | eager render-to-texture write-backs | | a backend that cannot sample surfaces takes it |

### Headless, capture and diagnostics (boot)

All off by default and kept: they are how render changes are checked
without a window.

| Variable | What |
|---|---|
| `H2_BOOT_GFX`, `_EVERY`, `_SHADOWS`, `_BETWEEN` | headless `GfxBackend` writing PNG frames, shadow-map frames and in-between frames |
| `H2_BOOT_GFX_LIGHTING_AB` | retail and E3 lighting views in one run |
| `H2_BOOT_UPLOAD_ALWAYS` | `UploadPolicy::Always`: finds missed stores the write barrier should have caught |
| `H2_BOOT_DRAWLOG`, `_FRAMES`, `_VERTS` | `d3d_diag` draw logs (render-loop's draw-stream checks use them) |
| `H2_BOOT_CAPTURE`, `_FRAME`, `_SNAP`, `_SKIP`, `_LITE`, `_TEX` | `d3d_diag` frame capture |
| `H2_SWEEP_DIAG` | `d3d_diag` sweep report |
| `H2_BOOT_STOP_AT_PRESENT` | end the run at a present |
| `H2_D3D_WRITEBACK_TRACE` | print each write-back with its cause |
| `H2_NATIVE_VERBOSE=1` | the native stages' per-definition and per-material load lines, and their comparison dumps (off: one summary line per stage per level load) |
| `H2_BENCH_*` | knobs of the ignored bench tests in `crates/d3d/tests` |

`H2_BOOT_OLD_GLUE` is kernel and voice glue, not rendering; out of scope
here.

## Dead code

| Item | Verdict |
|---|---|
| `GfxBackend::without_shaders` | removed (no caller) |
| `boot::d3d_glue::map_write_combined` | no caller since `map_reserved_write_combined`; remove with the next boot change (not done here, to keep boot out of this pass) |
| `Device::set_transform`, `Device::set_zbias`, `Device::present_parameters` | no caller in any crate or test; part of the D3D API mirror. Ask before removing |

## Ask: recommendations

- **Stage 7a ports** (`impl.render.views`, `layer_draw`, `layer_entries`,
  `game_view`, `scene_passes`, `states`, `states_skip`). None is on,
  none saved measurable time (the orchestrators add a round trip per
  call), and each
  adds a hook. Recommendation: keep `game_view` and `scene_passes` until
  the native renderer lane (crates/native-render, docs/graphics/native-renderer.md)
  says whether it hooks the pass sequence there (`ScenePasses::at()` is
  the pass tag D and E1 want); measure `states_skip` once in game
  (the coordinator is measuring it live), and remove
  `states`, `states_skip`, `layer_draw` and `layer_entries` if it shows no
  `render` drop. `views` goes with `game_view` either way.
- **D (`impl.render.interpolate`) against E1.** E1 is the choice and
  shares `InterpBackend`'s recording, pacing and safety rules with D; only
  how an in-between frame is made differs. Recommendation: once E1 has
  been played at 60, remove D's replay mode (the interpolated-matrix
  replay and its tests), keeping the shared wrapper.
- **`impl.render.reproject_ahead`.** A few lines on E1 (alpha past N),
  tested with it. Recommendation: keep while E1 is being tuned; drop it
  if aim feels no better than plain E1.
- **`impl.render.occlusion`.** Built and tested, never run in game.
  Recommendation: one in-game check (FOV edges, then `render` and draw
  count on and off) before deciding; if the native renderer culls on the
  GPU, remove it.
- **`H2_GFX_MERGE_PASSES=fast`.** Superseded by `loose` today.
  Recommendation: remove `fast` (`merge_split` and `set_merge_discards`'s
  fast branch) after a few days of play on `loose`; `exact` and `0` stay
  as references.

## Removed

Do not re-add these without new evidence; each commit message has the
numbers.

| Commit | What | Why |
|---|---|---|
| `ce288e75` | `H2_GFX_SUBMIT=batch\|<us>`, `GfxBackend::set_submit_after`, `set_submit_per_pass`, `SUBMIT_AFTER_PASSES` | slower in game: Old Mombasa 4x 30.2 against 33.5 fps per pass (3e8276b2); off since |
| `2d562b08` | the lazy 1x reference at render scale (`H2_LAZY_REFERENCE`, `ScaledBackend::set_lazy_reference`), `scale_bench` | 1-10% saved on unread frames, 15-25% lost on frames with a back-buffer snapshot, which Halo's frames take (09590f5c); off since |
| `7e4529f0` | `GfxBackend::without_shaders` | no caller |
| `ff6f7d6b` | `impl.render.layer_sort` and `impl.render.transparent_sort`: `engine::render_loop::sort` and `sort_indexed`, their differential tests, the `render-loop` feature's two placements and path counters | the profile puts both sorts at about 0.01 ms a frame (30 samples over 142 s, Old Mombasa 2026-11-17), and each port cost a hooked call per sort; off since built. The points `LAYER_SORT` and `TRANSPARENT_SORT` stay in `engine::points` as research notes |
