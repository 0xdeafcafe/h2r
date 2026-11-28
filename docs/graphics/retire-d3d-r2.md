# Retiring D3D: R2, transparents and effects

Status: Partly built. Rust answers every frame of the bodies under the sorted dispatch, with no guest comparison draws; the rest of R2 is in retire-d3d.md, "Slices".
Code: see each section. Tests: named per section.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

A slice of [retire-d3d.md](retire-d3d.md), split from it by topic.

## R2's first slice

Built (**Code**): the display queue's order as a plan.
`DisplayTransparentQueue::runs` (`frame-scene/src/transparent.rs`) groups
display geometry by the reference ordinal it sorts after, and refuses a
queue that is not `order_exact` or has an unresolved cluster. Tests:
`display_geometry_runs_follow_the_reference_ordinal_they_sort_after`,
`a_queue_the_guest_must_order_has_no_runs`. `render.stats` carries
`layer_plan_15_display_queue_{built,geometry,unresolved_clusters,inexact}`.

The display water draws from the plan (**Code**):
`render-bridge/src/native_water/display_runs.rs` plans the queue's water
parts at the queue's build, draws the head run, and draws the run after
each ordinal at `TRANSPARENT_DISPATCH_NEXT` (`0x0001e3f9`, the join after
each entry of the loop `0x0001e370`, handler or null). Tests:
`runs_draw_in_order_between_the_entries_the_guest_dispatches`,
`a_refused_queue_draws_nothing_and_leaves_the_old_path`,
`parts_the_layer_6_pass_drew_and_another_frame_are_not_drawn_again`,
`a_host_that_refuses_a_run_is_counted_and_the_loop_goes_on`,
`the_plan_takes_a_late_run_in_order`. `render.stats` carries
`layer_plan_15_display_queue_{runs_planned,runs_drawn,parts_drawn,runs_refused,runs_undrawn,old_path,hook_foreign_frame}`
(native-renderer.md).

The guest's loop still dispatches every entry, so the loop's residual
count does not drop yet. What blocks replacing it is made in Rust beside
the guest in "R2's shadow arm".

## R2's shadow arm

Built (**Code**): `render-bridge/src/effects_shadow.rs`. The bodies under
`0x0001e370`, made in Rust on every frame. No frame is the guest's for
comparison. Switch `frame-scene.effects-shadow` (on).

| Body | Rust |
|---|---|
| Stamps | `effects_shadow/view_update.rs`: each outermost `0x00174420` call, its callees `0x001751d0`, `0x00137bd0`, `0x0003eb20`, `0x00165010`, `0x00181800`, `0x00249bb0` and its queue call `0x0001e4e0` |
| Dispatch | `effects_shadow/dispatch.rs`: the loop's entries in order, each handler named; the decal runs between them |
| Emission | `engine/src/particle_emission.rs`: the particle callback `0x00224550` to its emitter call, its callees `0x00173ba0`, `0x00248df0`, `0x0004efb0`, `0x00246cd0`, `0x00246d80`, `0x00131bb0`, `0x00131e50`, `0x00016ae0` |
| Ribbon | `engine/src/contrail_ribbon.rs`: the contrail builder `0x00042cf0` to its return, its callees `0x000bad50`, `0x00042c50`, `0x0017ca10` ("R2's ribbon arm") |
| Effects seed | no path under `0x00174420` or the named handlers loads `[0x004e7408]` (the lifted image's call graph) |

What the stamps body writes:

| Field | Write |
|---|---|
| system `+0xd` bit 2 | set when forced (the view's cluster, or `+0xc` bit `0x400`); cleared when culled in |
| child `+0xc` bit `0x400` | the parent's, before the child's update |
| particle `+0x30` | the frame counter `0x004ba034`, for a particle of the other view or one the cull keeps |
| singletons `0x00479868`, `0x00479874` `+4`, `+8` | the tag and tag data `0x00137bd0` sets before each read |
| list row | class 3, depth, handler `0x00173c90`, payload (system, particle), position, for shader flag `0x8000` |

The system and particle stamps are the render's (`effects_shadow/stamps.rs`,
[sim-render-split.md](../engine/sim-render-split.md#6-render-scratch-and-stamps)):
a table keyed by element and salt holds those that differ from the arena's.
Where the guest body runs, the table is lent to the arena at the outermost
call's entry and taken back at its return; the answer reads the arena through
the table and writes only the singletons and the list.

The gate (`Gate`) counts each frame once (`answering_frames`) and each
named refusal. A body the answer does not take runs as the guest's and is
counted `scene_kept_15_<kind>_<cause>` (["R2's answer"](#r2s-answer)); each
run of consecutive decal entries (`0x00041cc0`, `0x000423c0`) goes to
`RunHost::draw_display_entries` at `TRANSPARENT_DISPATCH_NEXT` after its
last entry.

The stamps model refuses, by name: `fault`, `layout`, `no_definition`,
`no_model`, `singleton_vtable`, `object_path` (a shader flag besides
`0x8000` calls `0x00040e30`), `x87` (the distance sum at 64-bit precision
or another rounding mode), `distance_range`, `depth`, `cycle`, `mxcsr`,
and `frame` (the frame counter does not read).

`render.stats`, each `layer_plan_15_effects_shadow_<outcome>`:

| Counter | Counts |
|---|---|
| `answer_stamps`, `stamps_entries_queued`, `stamps_unlent`, `stamps_unbalanced_lend` | answered calls and the entries they queue; guest bodies run without the table (the arrays unreadable); an outer call whose return was not seen |
| `dispatch_refused_plan`, `dispatch_unbalanced` | sorted dispatches |
| `dispatch_entries_{geometry,particle,contrail,decal,null,other}` | entries of each dispatch, by handler |
| `answering_frames` | the gate |
| `entry_runs_planned_<kind>`, `entry_runs_drawn_<kind>`, `entry_runs_entries_<kind>`, `entry_runs_refused_<kind>`, `entry_runs_no_host_<kind>`, `entry_runs_undrawn` | the decal runs |

Tests: `view_update.rs` (`a_forced_system_stamps_and_queues_each_particle`,
`the_other_view_stamps_only_and_a_rejected_particle_is_not_stamped`,
`children_take_the_parents_forced_bit_and_are_updated`,
`what_the_model_cannot_make_is_a_named_refusal`,
`the_distance_keeps_the_x87_precision_or_refuses`,
`the_fade_and_the_frustum_follow_the_guests_compares`), `dispatch.rs`,
`effects_shadow.rs` (`the_gate_counts_each_frame_once_and_names_its_refusals`,
`a_dispatch_counts_its_entries_by_kind`,
`an_answering_gate_draws_each_decal_run_after_its_last_entry`,
`the_frame_step_leaves_the_particle_arrays_as_the_tick_left_them`),
`stamps.rs`.

Still the guest's: the material setup `0x00174f80`, the decal-list
replays, the calls of `0x00174420` the model refuses, and `0x00040e30`.


## R2's geometry arm

Built (**Code**): `render-bridge/src/effects_shadow/geometry.rs`. The
standard transparent handler `0x00041020` runs `0x00040f60` with the build
`0x0004cbf0` and the draw `0x0004d0b0`: it takes the shader datum (`+0x24`)
and render argument (`+0x28`), reads the template's layer-15 word (pass
count `>> 9`), and per pass `i` calls the build `(shader, 0, 0, 15, i,
argument, slot)` and, when its AL is set, the draw. Layer 15 takes the
build's generic arm (the jump table `0x0004d054` sends layers 4 to 15 to
`0x0004ce0f`), which `native_world::scene_build::build` ports for layers 1,
3, 5 to 8, 15 and 16.

On every frame the handler's build does not run: the 24-byte sort slot
and AL are made in Rust (`scene_build::build` for layer 15) and the build
returns (`ret 28`, `answer_geometry`). The build's answer hooks in
`native_world/scene_walk.rs` (`native_world_build_entry`), so it runs
where the native world is on. A build the model refuses runs as the
guest's, counted `scene_kept_15_geometry_<name>`: `fault`,
`particle_material` (`PRTM`, `prt3`: a virtual call, left to the particle
path), `not_shader`, `record`, `passes`, `layout`.

### The draw's material

The draw `0x0004d0b0` applies the pass's implementation
(`ResolvedImplementation`, from the shader's template at layer 15) to the
device. The answered draw takes it from the world's layer-15 set (below);
no draw is compared with the guest's.

### The draw's answer

Built (**Code**): `geometry::answer_handler` (`render-bridge`) and
`native_layers_glue::TransparentAnswer` (`game`). On every frame a BSP
cluster or placement entry's handler `0x00041020` does not run. The world's layer-15
set draws it: `layers_load` compiles layer 15 for every world material with
the planned layers (`TRANSPARENT_LAYER`), and the glue reserves a
`LayerViewPass` of one run, the entry's `(Owner::Cluster or Owner::Instance, part)`, in the
ordered stream at the handler's place; the handler returns (`ret 4`).

The entry is answered only when:

- the entry reads as a cluster's or a placement's and every pass the
  build model makes accepts with implementation 0, the one the layer's
  compile took (`geometry_draw_pass`);
- the material has no animated property and reads no per-draw input
  (`per_draw`);
- the device holds what the guest's draws would leave: what the cached
  implementation restores, then every state a pass sets holds or is restored
  (`layer_pass::skip_holds`, `state`), the layer's stencil and palettes
  (`stencil`, `palettes`), the viewport clip, the pipelines warm
  (`warming`) and the world unchanged (`world_changed`).

A refusal keeps the guest's handler and is counted
`scene_kept_15_geometry_draw_<cause>` (also `frame`, `entry`,
`no_passes`, the build model's `particle_material`, `not_shader`, `record`,
`passes`, `layout`, and the glue's `world_pending`, `no_layers`, `no_layer`,
`set`, `material`, `layer`, `device`, `viewport`, `cached_state`, `fog`,
`reservation`); an answered entry counts
`layer_plan_15_effects_shadow_answer_geometry_draw`.

An object's entry (`Glue`'s `draw_object`, `TransparentAnswer`) is drawn
as the sorted runs' objects are. The scene of layer 15 is extracted from
the list (`native_layers::extract`), the item found by its owner, record,
part and shader, and planned by `plan_objects` over the cached restore
from the object models, which compile layer 15 with the other object
layers (`OBJECT_LAYERS`). The pass draws the item's passes with its
lighting and transform.

An item that marks the stencil draws under the mode-1 stencil
(`layer_pass::item_stencil`) and is refused `object_stencil` only when the
device does not hold mode 1 where the item would not set it. A draw that
samples the view or the scene copy is recorded as commands with a copy of
the bound colour (`snapshot_gpu`), as the layer calls' are. The stencil,
like the plan's states, is read as the cached restore leaves it
(`native_objects_glue::stencil_with`).

An item whose
`0x0003bea0` is the frame's first call (`TARGET_COPIED`) is answered only
after the cached implementation's restore list is written
(`restore_list`, the open shader's `0x00018900`) and the cache cleared as
`0x00016b10` clears it (`frame_step::layer::clear_cache`), and with a
structure parameter block the pass copies the bound colour into
the refraction target before its first draw
(`snapshot_gpu::Snapshots::into_target`); a restore list the device does
not take keeps the guest's handler. Layer 2's sorted runs skip such an
item the same way: both take the step from `FirstCopy`
(`native_layers_glue.rs`), the list before the pass and the clear with the
commit
(`a_sorted_run_item_making_the_copy_writes_the_restore_list_and_clears_the_cache`).

The other refusals are the
object plan's (extraction's cause, `object_model`, `object_texaccum`, the
plan's names) and `object_scene`, `object_item`, `object_layer_begin`,
`object_shader_flag_target`.

Not built: the handler's other side effects (the lightmap page `0x15c90`
sets for a BSP record) are not made, as the layer's other skipped draws do
not make them; a structure entry's passes are drawn with the load's
constants, not the device's per-draw ones. A shader that is not a world
material is refused `material`. Tests: `effects_shadow/geometry/tests.rs`.

## R2's decals and weather

Built (**Code**): the scene holds both as typed items, read at the view's
layer 3 (`render-bridge/src/scene_step.rs`, `decals_and_weather`).

| Item | Read by | Holds | Slot |
|---|---|---|---|
| `DecalItem` (`frame-scene/src/decal.rs`) | `scene-extract/src/decals.rs` | mode 0 to 6, cluster, record, bitmap and image, tint (`+0x24`), the quad list's vertices | mode 0 in slot 2, 1 in slot 5, 2 to 6 in slot 7 |
| `EffectItem::WeatherPlate` (`frame-scene/src/effect.rs`) | `scene-extract/src/weather.rs` | per `weat` block element: window, three bitmaps, per-player scroll, tint | slot 9 |

Decals follow the guest's order: modes 0 to 6, each over the visible
cluster list `0x00547f94`, each bucket's chain from its head
(`[0x004ea94c] + 4 * (mode * 512 + cluster)`, link `+0x34`); a record
whose owner (`+0x38`) is met again in the view is skipped, as the callback's
frame marker skips it. A record that does not read is the view's
`decal_remainder`, counted `scene_decal_remainder`. `render.scene` carries
`scene_decal_items`, `scene_decal_quads`, `scene_weather_plates` and
`scene_weather_unread`.

`native_render::decal_scene` turns a view's decals into groups per slot with
each mode's depth and blend (`state`); `native_render::weather_scene` turns
a plate into its corners, layers and the depth test GREATER. Tests:
`decal_items_match_the_datum_list`, `decal_modes_draw_in_their_three_slots`,
`a_weather_plate_tests_against_scene_depth`, `a_plate_scrolls_per_player`,
`an_undrawn_plate_counts_scene_dropped_weather_plate`.
`native_decals::scene_draws()` is true when a host draws the decals from the
scene; the callback `0x00044040` is then cut, and `0x00041cc0`, `0x000423c0`
(object and camouflage layers) are not the decals'.

The decal material is ported as data (`native_render::decal_material`:
each mode's render states from `0x000222d0` calls and its 60-word combiner
block from `0x00043a40`). Every admitted decal draw takes its combiner from
the mode's Rust material and its position matrix rows from the open view
(`apply_rust` in `game/src/native_decals_glue.rs`); no draw is the guest's
for comparison. The rows are the four the program's position goes through
(`position_base`, the relation of `d3d` `scene_sink/camera.rs`); other
registers keep the draw's own constants.

| Refused, in the decal refusal tally | Why |
|---|---|
| `material_program` | no vertex program, or a position that does not go through four rows |
| `material_view` | no open view |

A call of `0x00044040` is cut where its
records draw from the mode's template, not the scene's items
([retire-d3d-decals.md](retire-d3d-decals.md)).

The weather plates' host (**Code**): `render-bridge/src/native_weather.rs`
observes each call of the plate draw `0x00037a60` (element at `[esp+4]`)
and makes the plate's draw in Rust (`plan`). The game's admission
(`game/src/native_weather_glue.rs`) takes the call's resolved quad strip.

| Step | Kernel | Inputs |
|---|---|---|
| Placement `0x0003a5c0` | `weather_place::place`, kind 3, flags `(0, 0, 1)` | element `+0x80`, `+0x84`; scale `[0x0048565c]` |
| Definition, setup skipped, result wanted | `weather_place::kind3` | the colours `0x00037a60` packs (`weather_quad::colours`); limit `[0x00485658]` |
| Strip, scroll constants, depth | `weather_quad::quad` | the result; the view's near and far `[0x00485ad4]`, `[0x00485ad8]` |

The guest's setup still runs. In a Rust draw the strip's vertices, its
scroll constants (`c[21..24]`, hardware order) and the definition words
the placement writes (`Plate::written`, 42 of 60) are Rust's, and the
strip is recorded as an indexed pass (`IndexedPass`, two triangles in
`PrimitiveType::to_host`'s corner order).

| Arm | |
|---|---|
| Rust draws | every plate draw; `weather_plate_admitted`. No draw is the guest's for comparison |
| Refused | the guest's draw stands for that call, `weather_plate_refused_<reason>`: `placement` (Rust's placement refuses a plate the guest draws), `view_unread`, `element_unread`, `globals_unread`, `geometry`, `vertex_streams`, `vertex_bytes`, `vertex_declaration`, `texture_unbound`, `texture_target`, `texture_binding`, `pass`, `second_draw`, and `unseen` (a call Rust expected to draw whose strip no admission saw) |

Tests: `native_weather.rs`
(`a_plate_is_drawn_at_its_placements_depth_result`,
`every_plate_is_rusts_and_a_gap_is_a_named_refusal`,
`a_call_hands_its_plan_to_its_first_draw_only`), `native_weather_glue.rs`
(`rusts_strip_writes_its_floats_and_leaves_the_rest`,
`a_strip_without_the_plates_registers_is_refused_by_name`).

Not built: nothing draws the scene's decal items, and the plates' host
draws the guest's call, not the scene's `WeatherPlate` items. Undrawn items are
`scene_dropped_decal_<cause>` and `scene_dropped_weather_plate_<cause>`
once a host takes the slots; until then the guest draws them.

## R2's answer

Built (**Code**): `render-bridge/src/effects_shadow/answer.rs`. On every
frame the modelled bodies do not run.

| Body | The answer | Kept, `scene_kept_15_<kind>_<cause>` |
|---|---|---|
| particle callback `0x00224550` | the emission from `particle_emission`, held by [the mirror](#r2s-particle-mirror) or drawn through the native particle path's direct draw (`native_particles::draw_emission`); its writes made in the guest; `ret 0x1c`. The emitter `0x0023ed30` never runs | `particle_<refusal>`, `particle_{refused,no_host,declaration,register,unreadable,frame,...}`, `particle_stack` |
| contrail builder `0x00042cf0` | the ribbon from `contrail_ribbon` written at its first argument, its writes, AL; `ret 0xc`. The polygon handler draws it by the native effect path | `contrail_<refusal>`, `contrail_frame` |
| decal runs | `RunHost` (the seam for the decal replays from the scene) | `decal_replay`, each entry while no host is installed |

A kept body runs as the guest's, with nothing made before it. `render.stats`
also counts `layer_plan_15_effects_shadow_answer_{particles,contrails,fault}`.

The handlers' material setups run unless [R2's materials](#r2s-materials)
cuts them: the native particle draw takes its pipeline from the D3D state
the setup leaves (`native_render::particle_gpu`).

Tests: `answer.rs`
(`an_answered_particle_draws_from_the_rust_emission_and_a_refused_one_keeps_its_guest_body`,
`every_frame_is_the_answers_and_an_unread_frame_is_kept_by_name`),
`effects_shadow.rs` (`an_answering_gate_draws_each_decal_run_after_its_last_entry`).

## R2's particle mirror

Built (**Code**): `render-bridge/src/effects_shadow/mirror.rs`,
`native_render::particle_mirror`, the host
`game/src/native_particles_admission/mirror_host.rs`. An answered particle
is held as a GPU particle system's birth; when the particle draw
`0x002243c0` returns, the host draws each held batch with `ParticleDraw`.
The guest still owns and simulates the particle; the mirror is that
frame's state alone (`GpuParticles::mirror`: nothing survives, each birth
is evaluated at its own age).

| Spawn | From the callback |
|---|---|
| `position` | its `position` argument; no velocity |
| `age`, `lifetime` | inputs 1 and 0 as the callback caches them; lifetime is age over life (life 0: unbounded), so life matches to float rounding |
| `random` | inputs 2, 3, 14, 15 |
| `words`, `tint` | the particle's `+0x14`; its `+0x3c` with red moved to the low byte |
| system (`Key`) | the definition `[draw]`, bitmap `[draw + 4]`, the `tint` and `size` arguments (`Definition::tint`, `size_scale`) |

The guest's order holds: a particle the mirror does not take, or one of
another `Key`, draws the held batch first, then its own direct draw.

### The host

The host (`mirror_host::Glue`, installed with the native particle path's)
draws a batch under the cut setup: through the device's supplied draw
(`Device::try_native_supplied_draw`) with the material held at the cut
(`material_proof::held`), so the draw its admission is offered carries
the setup's texture and render state and the view's constants. Its pass
is the mirror's own: `GpuParticles::mirror`, then one
`ParticleDraw::w_buffered` into the bound colour and depth.

| Input | From |
|---|---|
| `Definition` | `particle_definition::definition` of the `prt3` at `[draw]` and the sprite sequences of the bitmap at `[draw + 4]` (read as the callback reads them), with `0x0043f17c`, `0x004123ec` and the engine's tables; then `particle_mirror::owned` for the `Key`'s tint and size. Built once a map per (definition, bitmap), refusals too (`Definitions`) |
| view | the draw's view constants (`c[0]`, `c[1]`, `c[3]` and the pixel fix-up `c[10]`, `c[11]`, widened for the display copy) over the whole target; the billboard axes are the x and y rows less their part along the view |
| depth | the W-buffer: `c[3]`'s W held to the draw's W clip (clamped or culled as `D3DRS_DEPTHCLIPCONTROL` says), floored, over the target's Z scale; tested less-equal, never written |
| sprite | the stage 0 texture the cut material binds; a linear sampler |
| stencil | the supplied draw's, test, write and reference (`fog_gpu::layer_stencil`; off on a depth with no stencil aspect); a batch that writes it says so in its writes |

| Outcome | Counter |
|---|---|
| batch drawn | `layer_plan_15_effects_shadow_answer_mirrored` (particles) |
| not taken: direct draw | `scene_kept_15_particle_mirror_<cause>`: `no_host`, `orientation` (`0x0023ea80`'s class, definition `+4`, not 0), `tint_scale` (flag `0x180` with `BYTE * TINT` not 1/255), `unreadable`, or the host's: `uncut` (no cut setup), `tag_unread`, `bitmap_unread`, `bitmap_sequences`, `bitmap_sprites`, or the `particle_definition::Refused` name |
| batch refused: no guest draw is left | `layer_plan_15_effects_shadow_mirror_dropped_<cause>`. `unfinished`: a batch no draw's return took. The host's: `uncut`, `capacity`, `no_colour`, `no_depth`, `clip_rects`, `z_buffered`, the state `ParticleDraw` does not draw (`depth_off`, `depth_write`, `depth_func`, `color_write`, `alpha_test`, `opaque`, `blend`: other than alpha or additive), or the supplied draw's (`boundary`, `admission`, ...) |
| refused while recording | `native-particles` refusals, `Mirror/<cause>` (`texture`, `view_size`, `z_scale`, `view_rows`, `capacity`, `sprites`, `viewport`): the batch does not draw |

A batch's system is made for each recording (its births go through the
queue, so two batches never share buffers in one submission); the pipelines, per colour format, depth format, blend and stencil, are kept per device.

Tests: `mirror.rs`
(`a_particle_draws_batched_at_the_draws_return_from_its_state_as_the_callback_leaves_it`,
`a_particle_the_mirror_refuses_draws_the_held_batch_first_and_is_kept_by_name`,
`a_batch_the_host_refuses_is_dropped_by_name`),
`mirror_host.rs` (`a_system_is_built_once_a_map_by_tag_and_its_refusal_is_kept`,
`a_particle_without_a_cut_setup_or_whose_tag_does_not_build_is_refused_by_name`,
`a_zeroed_tag_is_refused_for_its_missing_functions_and_a_bitmap_not_of_sprites_has_no_sheet`,
`a_state_particle_draw_cannot_draw_is_refused_by_name`,
`the_view_maps_the_guests_rows_to_the_target_and_refuses_a_degenerate_one`),
`particle_mirror.rs` (four), `native-render/tests/particles_compute_gpu.rs`
(`a_mirror_is_its_births_alone_each_evaluated_at_its_own_age`).

## R2's materials

Built (**Code**): the material each handler's setup leaves on the device,
made in Rust as data (`native_render::effect_material::Material`): the
state every cut setup's draws take
(`game/src/native_particles_admission/material_proof.rs`). The routines: [notes/2026-11-28-particle-routines.md](notes/2026-11-28-particle-routines.md).

| | Particles | Contrails |
|---|---|---|
| Setup | `0x002243c0` → `0x0023ea80` | `0x00035b00` → `0x00043720` → `0x00052d40` |
| Inputs, read at | the draw's entry: shader, layer, pass, the system's `+4`, the view matrix `[0x004687d0]` | the setup's entry: material block `e[+0xc]` (`+0x24` type, `+0x28` flags, `+0x2a` blend mode, `+0x2e` map flags), bitmap `e[+0x18]`, image `e[+0x1c]` |
| Port | `particle_material`: `STIPPLEENABLE` off, the reset's states with `[0x004670bc]` set (`RESET_STATES`), then implementation 0 of lod 0, layer, pass from `tags::material` (`0x00016b90`) for what `ShaderLibrary::drawn_shader` draws: a `shad` under its root's inputs, a `prt3`'s embedded postprocess (`+0x8c`) or a `PRTM`'s `shad` (`+0x1c`) under zero inputs; its game-time animation sampled at the render clock per setup | `contrail_material`: `0x00052d40` as data, the blend tables `0x004506b8`, `0x004506e8`, `0x00450718` copied from the image |
| Render and stage states | the implementation's, in set order; stage states as `0x000189a0` writes them | cull, colour write, blend, alpha test, stencil, depth, z bias; stage 0's addressing and filters |
| Combiner | the implementation's 60 words; a word its draw-time inputs drive refuses the cut (`combiner_driven`) | the 60 words built at `0x00484f68` |
| Textures | each bound stage's bitmap image (`0x000143c0`), an animated slot's at its frame over the slot's images (`0x0001b0a0`), the rasterizer data's bitmaps for engine textures 1, 9, 14 and 15, none for 4 and, after a reset, for every stage the implementation leaves | stage 0's bitmap image |
| Constants (hardware slots, register + 96) | implementation 0's `c[18 + i]` (`0x00018eab`), then `0xae..0xb0`, the view matrix; its vertex defaults (`Material::defaults`, `0x00018e3b`) | `0xae..0xb0`, the identity |
| Vertex program | program `class + 7` of implementation 0's `vrtx` | engine slot 4's program 0 |

Not made, each a panic `unimplemented!("<kind>_material: <cause>")`
(`render-bridge/src/effect_materials.rs`), never the guest's setup:

| Reason | Kind | Branch |
|---|---|---|
| `not_shader` | particle | a tag other than a `shad`, `prt3` or `PRTM` in the shader's place |
| `object_animation`, `animation`, `property`, `wave`, `time`, `texture_transform`, `frame_size`, `texture_frame` | particle | an animated parameter that does not sample at the render clock: an object function's input or a ranged curve (the particle provider `0x0055edd8` is not read), an animated render or stage state, a curve that does not parse, a frame that is not finite |
| `engine_variant`, `engine_state`, `engine_texture`, `target_copy` | particle | a combiner variant or a state the draw's inputs choose, the lightmap or an engine texture that is a target or chosen at run time, the frame copy |
| `tags`, `no_vertex_shader`, `map_name` | particle | the shader does not resolve, or the map is not named; the map's library is read at its first particle setup, and one that does not read panics |
| `vertex_constant_driven`, `vertex_constant_lanes` | particle | a vertex constant from the draw's engine inputs or from an engine texture's size, a constant or default row not written whole |
| `state_unread` | both | the state arrays were not read at the draw's setup |
| `kind`, `blend_mode`, `view_matrix` | contrail | a block type other than 1, a blend mode past the tables, option bit 5 |
| `texture_streaming`, `texture_unbound` | both | a bitmap image that is not resident, or a bitmap with no images |
| `vertex_builtin`, `vertex_not_vrtx`, `vertex_program_index` | both | a program not in a `vrtx` tag |
| `no_setup` | both | no setup read for the draw |

Not carried: the stipple pattern (`0x003f8cc0`) and the polygon offsets
`SetRenderState_ZBias` derives (slots 77 to 81); the cut draw takes them
from the device.

### The cut

Built (**Code**): `render-bridge/src/effect_materials.rs`. At every
setup's call (`0x0023ea80` returning to `0x0022441e`, `0x00043720`
returning to `0x00035b23`) the host (`Host::material_cut`) holds the
material as a supplied state and the setup does not run: `0x0023ea80`
returns AL 1 (`ret 0xc`), `0x00043720` returns (`ret 4`). These panic
with their cause: a call from another caller, a material the host cannot
make, a draw the device refuses, an emission or contrail draw before its
kind's first setup. The guest's setup never runs.

The cut draw (`d3d::Device::try_native_supplied_draw`,
`d3d/src/device/supplied.rs`):

| Takes | From |
|---|---|
| render words | the device's pushed state, then render slots 0..56 from the combiner (`SetPixelShader` copies them, `rep movsd` at `0x003fa4d5`), then the material's states |
| stage words | the device's, then the material's |
| combiner | the material's definition |
| constants | the device's, the material's rows over them (`c[18 + i]` from `0x00018eab`, the matrix at `0xae`) |
| program | the tag's, at slot 0 (`LoadVertexShader(program, 0)` at `0x0001c5c7`) |
| textures | the material's headers (an empty stage unbinds), the device's elsewhere |
| vertex format | particles: the declaration of stream type `0x2a`, or `0x2b` (`v0` four `S32K` shorts in 8 bytes) with both `[0x00470a38]` and `[0x00470a39]` set, over the program's inputs (`particle_material::vertex_formats`); contrails: the device's, which the kept declaration sets |

After it the device's constants, textures, combiner and inline run are as
before, so the guest's later draws find the state they left; only the
carried attributes stay, as the guest's emitter writes them too. The
host asks `Device::check_supplied` before it cuts: what it names panics.

Kept on every call, by name:

| Kept | Why |
|---|---|
| `0x0002c3b0`, `0x0001bd50` | they write no device state, but later draws read their writes: `0x0001bd50`'s environment cache `[0x00485a58]` and dirty words, and `0x00016b90`'s engine inputs from `0x0055edd8` |
| `0x00035b00`'s own setup (`0x00016b10`, the declaration from entry `+0x10`, `0x0001c6b0`, `0x0001c710`, `0x0001cf50`) | the declaration is not in the material |

Not made, panicking with the cause:

| Cause | Why |
|---|---|
| `combiner_driven` | a combiner word the draw's inputs drive |
| `vertex_constant_driven` | a vertex constant from the engine's inputs or an engine texture's size |
| `vertex_constant_lanes` | a constant row the resolver did not write whole |
| `vertex_program_size` | a program whose length is not its header's |
| `program`, `render_slot`, `stage_slot`, `constant_slot`, `vertex_format`, `texture_unread`, `texture_format`, `palette` | `check_supplied`: a palettized texture would take the device's palette |

R2's answer (`effects_shadow/answer.rs`) reaches the cut draw through
`native_particles::draw_emission` and the host's `try_emit`.

Tests: `d3d/tests/admission_scope.rs`
(`a_cut_draw_uses_the_supplied_state_not_the_previous_draws`,
`the_device_state_after_a_cut_draw_is_the_guests`,
`a_cut_draw_without_a_program_is_refused_by_name`),
`native_particles_admission.rs`
(`a_cut_setups_material_is_the_state_its_draw_is_supplied`,
`a_material_the_supplied_draw_cannot_hold_keeps_the_guest_setup_by_name`,
`a_cut_particle_draw_names_the_walks_vertex_format`).

Tests: `effect_material.rs`
(`a_resident_image_binds_its_texture_and_others_fall_back_as_the_binder_does`,
`a_program_is_the_vrtx_element_at_its_index`),
`contrail_material.rs` (`the_blend_mode_comes_from_the_images_tables_and_mode_ten_turns_blending_off`,
`the_combiner_has_one_or_three_stages_then_the_blend_modes`),
`particle_material.rs` (`a_shad_or_a_particle_without_reset_reaches_the_shader_setup`,
`each_particle_shader_kind_makes_its_material_or_names_its_cause`,
`the_override_declares_the_emitters_short_corners_and_otherwise_the_float_stream`),
`effect_material.rs` (`a_programs_inputs_are_its_elements_semantic_list_by_register`).

## Related

- [retire-d3d.md](retire-d3d.md): the plan and the other slices.


### R2's emission arm

`engine::particle_emission::predict` makes, from the callback's entry,
what `0x00224550` pushes for the emitter: position, axis, size, rotation,
the packed colour, the frame blend, the two frame windows and the corners,
and its writes (the input cache at `state + 4i`, its mask `+0x44`, the
sequence at `draw + 8`). SSE in the original's operand order under the
guest's MXCSR, x87 under its control word, constants from the image; the
tag function is `function_eval`, the colour `opaque_animation::rgb`.

| Step | Guest |
|---|---|
| inputs the definition reads (`+0x98 & 0xf80f`), not cached, in bit order | `0x00173ba0`, `0x00248df0` cases 0 to 3, 11 to 15 |
| rotation `+0x4c`, colour `+0x1c`, alpha `+0x2c`, size `+0x3c` | `0x00246cd0`, `0x00246d80`; flag `0x180` tints by the particle's `+0x3c` |
| window and corners | flag `0x10000`: the definition's `+0xac`, `+0x9c`; else a type 3 bitmap's sequence (flag `0x10`), frame `+0x5c`, blend unless flag `0x20` |
| flips | flags 2 and 4 with the particle's `+0x14`, `+0x16` bit 0 |

Tests: `particle_emission/tests.rs`
(`the_inputs_the_definition_reads_are_cached_in_bit_order_and_masked`,
`the_emitter_words_point_into_the_callers_frame`,
`a_definition_window_flips_with_the_particle`,
`the_colour_lanes_are_clamped_before_they_are_packed`,
`a_sequence_bitmap_picks_the_particles_sequence_and_blends_two_frames`,
`what_the_model_cannot_read_is_a_named_refusal`),
and against the lifted original
`engine/tests/particle_emission_differential.rs` (feature `differential`,
needs the XBE).

### R2's ribbon arm

`engine::contrail_ribbon::predict` makes, from the builder's entry, what
`0x00042cf0(out, bytes, ribbon)` leaves: two vertices of 24 bytes a point
(position, along, across, packed colour), AL, and every other write. The
sorted handler `0x00035b00` reaches it through `0x0004b220` (stride 24).
SSE in the original's operand order under the guest's MXCSR, x87 under its
control word, constants from the image; the colour is packed as
`colour_pass::pack_colour`, the ease is `function_eval`'s `0x0017ca10`.

| Step | Guest |
|---|---|
| along and across words | the contrail's `+0x1c`, `+0x20`; the definition's `+0x1c` step and `+0x20` width, scaled by the contrail's `+0x14` (definition `+2` bits `0x40`, `0x80`) |
| colour and width | the point's stage (`+3`, the definition's `+0xe8` stages at `+0xec`) lerped by its `+0xc`, blended to the next by `+4` (point flag 2); out of range black and thin |
| tint | the contrail's object (`+8`) function source `0x000bad50`, else `[[0x00468710]]`; not with definition bit `0x80` |
| facing (definition `+0x18`) | 0 vertical; 1 and 2 flat in x and y; 4 across the step and the view `0x004b9da0`; 3 and above 4 return AL 0 after the texture words |
| fade | `0x00042c50`: mode `[[ribbon + 0xc] + 0x2c]` 1 or 2, by the view's angle to the normal the facing leaves (`ribbon + 0x10`); the stack's old bytes otherwise |
| ends | definition bits 1 and 2 clear unless set: the first pair's alpha (the contrail's `+0x30` word above 2), the last pair's (`ribbon + 0x14` vertices back) |

| Refusal | Why |
|---|---|
| `unbounded` | a list longer than 4096 points (`BUDGET`): the original would run on through a cycle |
| `fault`, `simd`, `x87` | an unreadable record, an unmasked SSE or x87 exception |

Tests: `contrail_ribbon/tests.rs`
(`a_ribbon_has_two_vertices_per_point_facing_the_camera`,
`the_texture_words_step_along_and_span_the_width`, `the_end_points_fade`,
`the_vertical_facing_stacks_the_pair_and_the_flat_one_spreads_it`,
`the_fade_is_the_views_angle_to_the_normal`,
`a_stage_out_of_range_is_black_and_thin`,
`a_point_blends_its_stage_into_the_next`,
`an_object_tints_the_colour_by_its_function_source`,
`an_unknown_facing_returns_zero_after_the_texture_words`,
`an_empty_list_still_clears_the_ends_it_is_told_of`,
`a_cycle_and_a_fault_are_named_refusals`), and against the lifted original
`engine/tests/contrail_ribbon_differential.rs` (feature `differential`,
needs the XBE).
