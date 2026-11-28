# Retiring D3D: R2, transparents and effects

Status: Partly built. The display queue's runs and the shadow arm are built; the rest of R2 is in retire-d3d.md, "Slices".
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

Built (**Code**): `render-bridge/src/effects_shadow.rs`. The state that
blocks replacing `0x0001e370`, made in Rust and compared with the guest's.
Switch `frame-scene.effects-shadow` (on).

| Arm | Rust | Compared at the return with |
|---|---|---|
| Stamps | `effects_shadow/view_update.rs`: each outermost `0x00174420` call, its callees `0x001751d0`, `0x00137bd0`, `0x0003eb20`, `0x00165010`, `0x00181800`, `0x00249bb0` and its queue call `0x0001e4e0` | systems, particles, the two particle-model singletons, the list's end and overflow byte, the rows it queues, the seeds |
| Dispatch | `effects_shadow/dispatch.rs`: the loop's entries in order, each handler named | the Rust sort's order (at `TRANSPARENT_ORDER_READY`); seeds, systems, particles, the list and its live rows unchanged across the loop |
| Emission | `engine/src/particle_emission.rs`: the particle callback `0x00224550` to its emitter call, its callees `0x00173ba0`, `0x00248df0`, `0x0004efb0`, `0x00246cd0`, `0x00246d80`, `0x00131bb0`, `0x00131e50`, `0x00016ae0` | at the emitter `0x0023ed30` called from `0x00224cd7`: the eight words, EBX, the two windows and the corners they point at, the input block and the draw context |
| Ribbon | `engine/src/contrail_ribbon.rs`: the contrail builder `0x00042cf0` to its return, its callees `0x000bad50`, `0x00042c50`, `0x0017ca10` ("R2's ribbon arm") | at its return: the vertices from its first argument, AL, every other byte it wrote, the draw context |
| Effects seed | no path under `0x00174420` or the named handlers loads `[0x004e7408]` (the lifted image's call graph) | both arms: the seeds do not move |

What the stamps arm writes:

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

The dispatch arm leaves the singletons out: `0x00137bd0` sets one before
every read, so the loop's last value is never read. The handlers' other
writes (`0x00035b00`'s `0x004670bc`, D3D state) are not compared.

The gate, per frame of `0x004ba034`:

| Rule | Value |
|---|---|
| frames compared | 1 in 64 (`SAMPLE`) |
| answers after | 8 sampled frames in a row compared clean (`CLEAN`); a sampled frame with nothing to compare neither counts nor breaks the run |
| stops answering | at the first difference or refusal |
| while answering | on frames not sampled, "R2's answer" (the outermost `0x00174420` call too); each run of consecutive decal entries (`0x00041cc0`, `0x000423c0`) goes to `RunHost::draw_display_entries` at `TRANSPARENT_DISPATCH_NEXT` after its last entry |

The model refuses, by name: `fault`, `layout`, `no_definition`,
`no_model`, `singleton_vtable`, `object_path` (a shader flag besides
`0x8000` calls `0x00040e30`), `x87` (the distance sum at 64-bit precision
or another rounding mode), `distance_range`, `depth`, `cycle`, `mxcsr`.

`render.stats`, each `layer_plan_15_effects_shadow_<outcome>`:

| Counter | Counts |
|---|---|
| `stamps_matched`, `stamps_mismatched_{seed,systems,particles,singletons,list,entries}`, `stamps_refused_<reason>`, `stamps_unbalanced`, `stamps_entries_queued` | outermost `0x00174420` calls on sampled frames (`stamps_entries_queued` counts answered calls too) |
| `answer_stamps`, `stamps_unlent`, `stamps_unbalanced_lend` | answered calls; guest bodies run without the table (the arrays unreadable); an outer call whose return was not seen |
| `dispatch_matched`, `dispatch_mismatched_{order,seed,systems,particles,list,rows}`, `dispatch_refused_{plan,order,mxcsr,<reason>}`, `dispatch_unbalanced` | sorted dispatches on sampled frames |
| `dispatch_entries_{geometry,particle,contrail,decal,null,other}` | entries of sampled dispatches, by handler |
| `emission_matched`, `emission_mismatched_{words,window,corners,inputs,sequence,writes}`, `emission_refused_<reason>`, `emission_unbalanced` | particle callbacks on sampled frames |
| `ribbon_matched`, `ribbon_mismatched_{vertices,return,writes,context}`, `ribbon_refused_<reason>`, `ribbon_unbalanced` | contrail builds on sampled frames |
| `frames_sampled`, `frames_clean`, `frames_unclean`, `frames_empty`, `opened`, `fallback`, `answering_frames` | the gate |
| `entry_runs_planned_<kind>`, `entry_runs_drawn_<kind>`, `entry_runs_entries_<kind>`, `entry_runs_refused_<kind>`, `entry_runs_no_host_<kind>`, `entry_runs_undrawn` | the runs while answering |

Tests: `view_update.rs` (`a_forced_system_stamps_and_queues_each_particle`,
`the_other_view_stamps_only_and_a_rejected_particle_is_not_stamped`,
`children_take_the_parents_forced_bit_and_are_updated`,
`what_the_model_cannot_make_is_a_named_refusal`,
`the_distance_keeps_the_x87_precision_or_refuses`,
`the_fade_and_the_frustum_follow_the_guests_compares`), `dispatch.rs`,
`effects_shadow.rs` (the gate, a matching and a differing call and
dispatch, the runs, `the_frame_step_leaves_the_particle_arrays_as_the_tick_left_them`),
`stamps.rs`.

Still the guest's: the handlers' material setup (`0x00174f80`,
`0x002243c0`, `0x00035b00`), the decal-list replays, the calls of
`0x00174420` on sampled frames and refusals, and `0x00040e30`.


## R2's geometry arm

Built (**Code**): `render-bridge/src/effects_shadow/geometry.rs`. The
standard transparent handler `0x00041020` runs `0x00040f60` with the build
`0x0004cbf0` and the draw `0x0004d0b0`: it takes the shader datum (`+0x24`)
and render argument (`+0x28`), reads the template's layer-15 word (pass
count `>> 9`), and per pass `i` calls the build `(shader, 0, 0, 15, i,
argument, slot)` and, when its AL is set, the draw. Layer 15 takes the
build's generic arm (the jump table `0x0004d054` sends layers 4 to 15 to
`0x0004ce0f`), which `native_world::scene_build::build` ports for layers 1,
3, 5 to 8 and 16 and now for 15.

| Compared | Rust | At |
|---|---|---|
| Passes | the template's layer-15 count | the handler's return: the builds it called, their pass numbers in order |
| Build | `scene_build::build` for layer 15 | each build's return (`0x00040ff7`): the 24-byte sort slot and AL |

The arm opens on sampled frames (the shared 1 in 64) and has a gate of its
own: 16 sampled frames in a row compared clean, then it answers.

A difference stops it
(`geometry_fallback`); a refusal is counted and leaves that build to the
guest. While it answers, on the frames not sampled, the handler's build does
not run: the slot and AL are made in Rust and the build returns. The draw
`0x0004d0b0`, the material's textures, samplers and constants stay the
guest's. The build's answer hooks in `native_world/scene_walk.rs`
(`native_world_build_entry`), so the arm runs where the native world is on.

Refusals, by name, `geometry_refused_<name>`: `fault`, `particle_material`
(`PRTM`, `prt3`: a virtual call, left to the particle lane), `not_shader`,
`record`, `passes`, `layout`. `render.stats`, each
`layer_plan_15_effects_shadow_<outcome>`: `geometry_{build,passes}_matched`,
`geometry_{build,passes}_mismatched_<field>` (`slot`, `return`, `fault`,
`count`, `order`), `geometry_refused_<name>`, `geometry_opened`,
`geometry_fallback`, `answer_geometry`, `geometry_{handler,build}_unbalanced`;
kept builds count `scene_kept_15_geometry_<name>`.

### The draw's material

The draw `0x0004d0b0` applies the pass's implementation
(`ResolvedImplementation`, from the shader's template at layer 15) to the
device. `native_render::transparent_material::compare` checks a resolved
draw against it: the bound textures (bound or not), the render words by
slot (the later value of a state set twice), the stage words of each bound
stage (address, filters, mip bias and level, anisotropy, border colour),
the 60-word pixel shader definition and the vertex constants `c[18 + i]`
(hardware slot register + 96, the written lanes). What an engine input
drives (`engine_render`, `engine_stage`) and the definition and constants
of an implementation that reads a per-draw input (`combiner`, `constants`)
are skipped by name.

On a sampled frame the handler's latest build names the pass (shader,
selector, the slot's implementation byte). At the indexed call that
follows, `native_transparents_glue` finds the shader in the world's loaded
materials (`LayersView.materials`), compares the implementation with the
draw's snapshot and records the result. Counted
`layer_plan_15_effects_shadow_geometry_material_<outcome>`: `matched`,
`mismatched_<field>` (`texture_<n>_unbound`, `render_<slot>`,
`stage<n>_<field>`, `combiner_w<n>`, `combiner_missing`, `constant_<register>`),
`skipped_<part>`, `refused_{no_layers,not_a_world_material,no_implementation}`
and `proven`, which fires once after 16 matches in a row
(`geometry::material_proven`). A difference ends that streak only; the
build answer's gate is its own.

### The draw's answer

Built (**Code**): `geometry::answer_handler` (`render-bridge`) and
`native_layers_glue::TransparentAnswer` (`game`). Once the build answer and
the material have both proved (16 sampled draws in a row each, no
difference since: `geometry::proven`), on a frame that is not sampled, a
BSP cluster or placement entry's handler `0x00041020` does not run. The world's layer-15
set draws it: `layers_load` compiles layer 15 for every world material with
the planned layers (`TRANSPARENT_LAYER`), and the glue reserves a
`LayerViewPass` of one run, the entry's `(Owner::Cluster or Owner::Instance, part)`, in the
ordered stream at the handler's place; the handler returns (`ret 4`).
Sampled frames run the guest, so the build and material arms keep
comparing.

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
`scene_kept_15_geometry_draw_<cause>` (also `entry`,
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
block from `0x00043a40`). The game proves it: one resolved decal draw in 64
is compared with it (`game/src/native_decals_glue.rs`), `render.scene`
carries `decal_material_{sampled,clean,mismatched,proven}` and
`decal_material_mismatch_<field>` (`combiner_w<n>`, `render_<index>`), and
`native_decals::material_proven()` holds after 8 clean samples in a row.

The view's matrix constants are compared with the same sample
(`native_render::decal_material::compare_constants`). Of the registers the
program reads (`constant_reads`, decoded with `shaders::vsh`), the four rows
its position goes through must equal the open view's view-projection rows
bit for bit and the draw's viewport its camera's rectangle (the relation of
`d3d` `scene_sink/camera.rs`). Mismatch fields: `constant_c<n>`,
`constant_c<n>_scaled` (the row times one factor), `constant_viewport`,
`constant_view_unread`, `constant_relative`, and `constant_c<n>_ungiven`
for a register nothing in Rust sets yet, so the proof stays open until the
live fields name none.

Once the material is proven (`material_proven()`), an admitted decal draw
takes its combiner from the mode's Rust material and its position matrix
rows from the open view (`apply_rust` in `native_decals_glue.rs`); one draw
in 64 stays the guest's own and is the comparison. A call of `0x00044040` is
cut where its records draw from the mode's template, not the scene's items
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
| Rust draws | every plate draw but the sampled ones; `weather_plate_admitted` |
| Compared | the first draw, then one in 64: the guest's draw stands and is compared field by field (`vertex<n>_r<register>`, `constant<slot>`, `combiner_w<n>`); `weather_plate_{compared,matched,mismatched}` |
| Placement | a plate Rust's placement refuses but the guest draws is the mismatch `placement` |
| Fallback | any mismatch: the guest's draw stands for the rest of the session; `weather_plate_fallback` is 1 |
| Refused | the guest's draw stands, `weather_plate_refused_<reason>`: `view_unread`, `element_unread`, `globals_unread`, `geometry`, `vertex_streams`, `vertex_bytes`, `vertex_declaration`, `texture_unbound`, `texture_target`, `texture_binding`, `pass`, `second_draw`, and `unseen` (a call Rust expected to draw whose strip no admission saw) |

Tests: `native_weather.rs`
(`a_plate_is_drawn_at_its_placements_depth_result`,
`rust_draws_one_draw_in_64_is_compared_and_a_difference_falls_back_for_the_session`,
`a_call_hands_its_plan_to_its_first_draw_only`), `native_weather_glue.rs`
(`a_plate_strip_rust_would_draw_compares_clean`,
`a_plate_strip_that_differs_is_named_by_field`,
`rusts_strip_writes_its_floats_and_leaves_the_rest`,
`a_strip_without_the_plates_registers_is_refused_by_name`).

Not built: nothing draws the scene's decal items, and the plates' host
draws the guest's call, not the scene's `WeatherPlate` items. The decal vertex program is compared with the engine slot 3's `vrtx` programs read from the tags (`vertex_program` mismatch field). Undrawn items are
`scene_dropped_decal_<cause>` and `scene_dropped_weather_plate_<cause>`
once a host takes the slots; until then the guest draws them.

## R2's answer

Built (**Code**): `render-bridge/src/effects_shadow/answer.rs`. While the
gate answers, on the frames it does not sample, the modelled bodies do not
run. A sampled frame runs the guest, so the arms go on comparing.

| Body | The answer | Kept, `scene_kept_15_<kind>_<cause>` |
|---|---|---|
| particle callback `0x00224550` | the emission from `particle_emission`, held by [the mirror](#r2s-particle-mirror) or drawn through the native particle path's direct draw (`native_particles::draw_emission`); its writes made in the guest; `ret 0x1c`. The emitter `0x0023ed30` never runs | `particle_<refusal>`, `particle_{refused,no_host,declaration,register,unreadable,...}`, `particle_stack` |
| contrail builder `0x00042cf0` | the ribbon from `contrail_ribbon` written at its first argument, its writes, AL; `ret 0xc`. The polygon handler draws it by the native effect path | `contrail_<refusal>` |
| decal runs | `RunHost` (the seam for the decal replays from the scene) | `decal_replay`, each entry while no host is installed |

A kept body runs as the guest's, with nothing made before it. `render.stats`
also counts `layer_plan_15_effects_shadow_answer_{particles,contrails,fault}`.

The handlers' material setups run unless [R2's materials](#r2s-materials)
cuts them: the native particle draw takes its pipeline from the D3D state
the setup leaves (`native_render::particle_gpu`).

Tests: `answer.rs`
(`an_answered_particle_draws_from_the_rust_emission_and_a_refused_one_keeps_its_guest_body`,
`a_sampled_frame_or_a_closed_gate_runs_the_guest_and_the_arm_compares`),
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
| batch refused: no guest draw is left | `layer_plan_15_effects_shadow_mirror_dropped_<cause>`; the gate stops answering. `unfinished`: a batch no draw's return took. The host's: `uncut`, `capacity`, `no_colour`, `no_depth`, `clip_rects`, `z_buffered`, the state `ParticleDraw` does not draw (`depth_off`, `depth_write`, `depth_func`, `color_write`, `alpha_test`, `opaque`, `blend`: other than alpha or additive), or the supplied draw's (`boundary`, `admission`, ...) |
| refused while recording | `native-particles` refusals, `Mirror/<cause>` (`texture`, `view_size`, `z_scale`, `view_rows`, `capacity`, `sprites`, `viewport`): the batch does not draw |

A batch's system is made for each recording (its births go through the
queue, so two batches never share buffers in one submission); the pipelines, per colour format, depth format, blend and stencil, are kept per device.

Tests: `mirror.rs`
(`a_particle_draws_batched_at_the_draws_return_from_its_state_as_the_callback_leaves_it`,
`a_particle_the_mirror_refuses_draws_the_held_batch_first_and_is_kept_by_name`,
`a_batch_the_host_refuses_is_dropped_by_name_and_the_gate_stops_answering`),
`mirror_host.rs` (`a_system_is_built_once_a_map_by_tag_and_its_refusal_is_kept`,
`a_particle_without_a_cut_setup_or_whose_tag_does_not_build_is_refused_by_name`,
`a_zeroed_tag_is_refused_for_its_missing_functions_and_a_bitmap_not_of_sprites_has_no_sheet`,
`a_state_particle_draw_cannot_draw_is_refused_by_name`,
`the_view_maps_the_guests_rows_to_the_target_and_refuses_a_degenerate_one`),
`particle_mirror.rs` (four), `native-render/tests/particles_compute_gpu.rs`
(`a_mirror_is_its_births_alone_each_evaluated_at_its_own_age`).

## R2's materials

Built (**Code**): the material each handler's setup leaves on the device,
made in Rust as data (`native_render::effect_material::Material`) and
compared with the draw the native admission takes, one in 64 of each kind
(`game/src/native_particles_admission/material_proof.rs`).

| | Particles | Contrails |
|---|---|---|
| Setup | `0x002243c0` → `0x0023ea80` | `0x00035b00` → `0x00043720` → `0x00052d40` |
| Inputs, read at | the draw's entry: shader, layer, pass, the system's `+4`, the view matrix `[0x004687d0]` | the setup's entry: material block `e[+0xc]` (`+0x24` type, `+0x28` flags, `+0x2a` blend mode, `+0x2e` map flags), bitmap `e[+0x18]`, image `e[+0x1c]` |
| Port | `particle_material`: `STIPPLEENABLE` off, then implementation 0 of lod 0, layer, pass from `tags::material` (`0x00016b90`) | `contrail_material`: `0x00052d40` as data, the blend tables `0x004506b8`, `0x004506e8`, `0x00450718` copied from the image |
| Render and stage states | the implementation's, in set order; stage states as `0x000189a0` writes them | cull, colour write, blend, alpha test, stencil, depth, z bias; stage 0's addressing and filters |
| Combiner | the implementation's 60 words; constants its draw-time inputs drive are not compared | the 60 words built at `0x00484f68` |
| Textures | each bound stage's bitmap image (`0x000143c0`) | stage 0's bitmap image |
| Constants (hardware slots, register + 96) | implementation 0's `c[18 + i]` (`0x00018eab`), then `0xae..0xb0`, the view matrix | `0xae..0xb0`, the identity |
| Vertex program | program `class + 7` of implementation 0's `vrtx` | engine slot 4's program 0 |

`compare` names each differing field: `render_<index>`, `stage<n>_<index>`,
`combiner_w<n>`, `texture<n>`, `constant<slot>`, `vertex_program`. A
state set twice is compared at its last value. `render.scene` carries
`<kind>_material_{sampled,matched,mismatched,open,fallback,cut}`,
`<kind>_material_mismatched_<field>`, `<kind>_material_refused_<reason>`
and `<kind>_material_dropped_<cause>` for `particle` and `contrail` (`render-bridge/src/effect_materials.rs`).

Refused, by name, rather than approximated:

| Reason | Kind | Branch |
|---|---|---|
| `particle_model`, `not_shader` | particle | a `PRTM`/`prt3` or other tag in the shader's place |
| `reset` | particle | `0x0001bbf0`'s device reset (`[0x004670bc]` set) |
| `vertex_override` | particle | `0x0001ccb0(0x2b)` (`[0x00470a38]` and `[0x00470a39]` set) |
| `animated`, `engine_variant`, `engine_state`, `engine_texture`, `target_copy` | particle | animated parameters, a combiner variant or a state the draw's inputs choose, an engine texture or the lightmap, the frame copy |
| `tags`, `no_vertex_shader`, `library_loading`, `library_unread` | particle | the shader does not resolve, or the map's library is not read yet |
| `vertex_defaults`, `vertex_constant_driven`, `vertex_constant_lanes` | particle | vertex defaults, a vertex constant from the engine's inputs or a stage's size, a constant row not written whole |
| `state_unread` | both | the state arrays were not read at the draw's setup |
| `kind`, `blend_mode`, `view_matrix` | contrail | a block type other than 1, a blend mode past the tables, option bit 5 |
| `texture_streaming`, `texture_unbound` | both | a bitmap image that is not resident, or a bitmap with no images |
| `vertex_builtin`, `vertex_not_vrtx`, `vertex_program_index` | both | a program not in a `vrtx` tag |
| `no_setup` | both | no setup read for the draw |

Not compared field by field: the stipple pattern (`0x003f8cc0`), the
polygon offsets `SetRenderState_ZBias` derives (slots 77 to 81), the
vertex declaration and the other vertex constants. The whole-state check
in "The cut" covers the render slots.

### The cut

Built (**Code**): `render-bridge/src/effect_materials.rs`. A gate per kind:

| | |
|---|---|
| Opens | 8 matched comparisons in a row |
| Closes | any mismatch, refusal or dropped draw; closing an open gate counts `<kind>_material_fallback` |
| Open | at the setup's call (`0x0023ea80` returning to `0x0022441e`, `0x00043720` returning to `0x00035b23`) one call in 64 runs the guest's setup and its first draw is compared; the others ask the host (`Host::material_cut`) |
| Cut | the host holds the material as a supplied state and the setup does not run: `0x0023ea80` returns AL 1 (`ret 0xc`), `0x00043720` returns (`ret 4`); `<kind>_material_cut` |
| Kept | the host's refusal, `scene_kept_15_<kind>_material_<cause>`, runs the guest's setup and closes the gate |
| Dropped | a cut setup's draw the device refuses: its setup did not run, so the guest does not draw it either; `<kind>_material_dropped_<cause>`, and the gate closes |

A compared draw is checked against its whole state, not only the
material's fields. At the call of the setup whose draw is compared next,
the guest's arrays (`D3D__RenderState` `0x00407498`, 166 words;
`D3D__TextureState` `0x00404dd0`, 4 x 32) are read; the draw's render and
stage words must be those arrays with the material over them
(`effect_material::compare_state`, fields `state_render_<n>`,
`state_stage<s>_<n>`). A setup write the material does not carry (the
restore of the implementation before, a state the tag does not list) so
keeps the gate shut. The draw's program must start at slot 0 and a
particle's vertex must be 8 bytes.

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
| vertex format | particles: `v0` four `S32K` shorts in 8 bytes (`PARTICLE_FORMATS`); contrails: the device's, which the kept declaration sets |

After it the device's constants, textures, combiner and inline run are as
before, so the guest's later draws find the state they left; only the
carried attributes stay, as the guest's emitter writes them too. The
host asks `Device::check_supplied` before it cuts: what it names keeps
the guest's setup.

Kept on every call, by name:

| Kept | Why |
|---|---|
| `0x0002c3b0`, `0x0001bd50` | they write no device state, but later draws read their writes: `0x0001bd50`'s environment cache `[0x00485a58]` and dirty words, and `0x00016b90`'s engine inputs from `0x0055edd8` |
| `0x00035b00`'s own setup (`0x00016b10`, the declaration from entry `+0x10`, `0x0001c6b0`, `0x0001c710`, `0x0001cf50`) | the declaration is not in the material |

Refused, keeping the guest's setup (`scene_kept_15_<kind>_material_<cause>`):

| Cause | Why |
|---|---|
| `combiner_driven` | a combiner word the draw's inputs drive |
| `vertex_defaults` | `SET_VERTEX_DATA4F_M` defaults (`0x00018e3b`) |
| `vertex_constant_driven` | a vertex constant from the engine's inputs or a stage's size |
| `vertex_constant_lanes` | a constant row the resolver did not write whole |
| `vertex_program_size` | a program whose length is not its header's |
| `program`, `render_slot`, `stage_slot`, `constant_slot`, `vertex_format`, `texture_unread`, `texture_format`, `palette` | `check_supplied`: a palettized texture would take the device's palette |

The material's own refusals reach the same counter
(`scene_kept_15_particle_material_tags`, `..._library_loading`, ...).
R2's answer (`effects_shadow/answer.rs`) reaches the cut draw through
`native_particles::draw_emission` and the host's `try_emit`.

Tests: `effect_materials.rs` (`a_proven_material_cuts_the_handler_setup`,
`one_draw_in_64_keeps_the_guest_setup_and_compares`,
`a_refused_material_keeps_the_guest_setup_by_name`,
`a_cut_draw_the_device_refuses_is_dropped_by_name_and_closes_the_gate`,
`the_state_is_read_at_the_setup_of_the_draw_compared_next_only`),
`d3d/tests/admission_scope.rs`
(`a_cut_draw_uses_the_supplied_state_not_the_previous_draws`,
`the_device_state_after_a_cut_draw_is_the_guests`,
`a_cut_draw_without_a_program_is_refused_by_name`),
`effect_material.rs` (`a_setup_write_the_material_does_not_carry_is_named_by_its_slot`),
`native_particles_admission.rs`
(`a_cut_setups_material_is_the_state_its_draw_is_supplied`,
`a_material_the_supplied_draw_cannot_hold_keeps_the_guest_setup_by_name`,
`a_cut_particle_draw_names_the_walks_vertex_format`).

Tests: `effect_material.rs`
(`a_resident_image_binds_its_texture_and_others_fall_back_as_the_binder_does`,
`a_draw_that_differs_is_named_by_field_and_a_state_set_twice_by_its_last_value`),
`contrail_material.rs` (`the_blend_mode_comes_from_the_images_tables_and_mode_ten_turns_blending_off`,
`the_combiner_has_one_or_three_stages_then_the_blend_modes`),
`particle_material.rs` (`only_a_shad_without_reset_or_override_reaches_the_shader_setup`),
`effect_materials.rs` (`one_draw_in_sixty_four_is_compared_and_each_outcome_is_counted_by_name`).

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
`effects_shadow/emission.rs` (a match, a differing word and input, another
caller, a refusal), and against the lifted original
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
`a_cycle_and_a_fault_are_named_refusals`), `effects_shadow/ribbon.rs` (a
match, a differing vertex, result and context, an unseen return, a
refusal), and against the lifted original
`engine/tests/contrail_ribbon_differential.rs` (feature `differential`,
needs the XBE).
