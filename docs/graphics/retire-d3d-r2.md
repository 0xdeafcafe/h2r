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

The dispatch arm leaves the singletons out: `0x00137bd0` sets one before
every read, so the loop's last value is never read. The handlers' other
writes (`0x00035b00`'s `0x004670bc`, D3D state) are not compared.

The gate, per frame of `0x004ba034`:

| Rule | Value |
|---|---|
| frames compared | 1 in 64 (`SAMPLE`) |
| answers after | 8 sampled frames in a row compared clean (`CLEAN`); a sampled frame with nothing to compare neither counts nor breaks the run |
| stops answering | at the first difference or refusal |
| while answering | on frames not sampled, "R2's answer"; each run of consecutive decal entries (`0x00041cc0`, `0x000423c0`) goes to `RunHost::draw_display_entries` at `TRANSPARENT_DISPATCH_NEXT` after its last entry |

The model refuses, by name: `fault`, `layout`, `no_definition`,
`no_model`, `singleton_vtable`, `object_path` (a shader flag besides
`0x8000` calls `0x00040e30`), `x87` (the distance sum at 64-bit precision
or another rounding mode), `distance_range`, `depth`, `cycle`, `mxcsr`.

`render.stats`, each `layer_plan_15_effects_shadow_<outcome>`:

| Counter | Counts |
|---|---|
| `stamps_matched`, `stamps_mismatched_{seed,systems,particles,singletons,list,entries}`, `stamps_refused_<reason>`, `stamps_unbalanced`, `stamps_entries_queued` | outermost `0x00174420` calls on sampled frames |
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
dispatch, the runs).

Still the guest's: the handlers' material setup (`0x00174f80`,
`0x002243c0`, `0x00035b00`), the decal-list replays, the calls of
`0x00174420` and `0x00040e30`.


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
the bound colour (`snapshot_gpu`), as the layer calls' are. An item whose
`0x0003bea0` is the frame's first call (`TARGET_COPIED`) is answered only
with nothing cached (the call's shader-cache clear then writes nothing),
and with a structure parameter block the pass copies the bound colour into
the refraction target before its first draw
(`snapshot_gpu::Snapshots::into_target`); with a cached implementation it
is refused `object_target`. 
A copy beside a draw sampling the view is
refused `object_shader_flag_target_view`.

The other refusals are the
object plan's (extraction's cause, `object_model`, `object_texaccum`, the
plan's names) and `object_scene`, `object_item`, `object_layer_begin`,
`object_restore`, `object_shader_flag_target`.

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
in 64 stays the guest's own and is the comparison. The callback `0x00044040`
is not cut: the host draws the guest's quads from its resolved draws, not
from the scene's items.

Not built: nothing draws these items. The decal vertex program is compared with the engine slot 3's `vrtx` programs read from the tags (`vertex_program` mismatch field). A plate needs the placement
test `0x0003a5c0` and a snapshot of the view's colour. Undrawn items are
`scene_dropped_decal_<cause>` and `scene_dropped_weather_plate_<cause>`
once a host takes the slots; until then the guest draws them.

## R2's answer

Built (**Code**): `render-bridge/src/effects_shadow/answer.rs`. While the
gate answers, on the frames it does not sample, the modelled bodies do not
run. A sampled frame runs the guest, so the arms go on comparing.

| Body | The answer | Kept, `scene_kept_15_<kind>_<cause>` |
|---|---|---|
| particle callback `0x00224550` | the emission from `particle_emission`, drawn through the native particle path's direct draw (`native_particles::draw_emission`); its writes made in the guest; `ret 0x1c`. The emitter `0x0023ed30` never runs | `particle_<refusal>`, `particle_{refused,no_host,declaration,register,unreadable,...}`, `particle_stack` |
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
| Constants | `c78..c80`, the view matrix | `c78..c80`, the identity |
| Vertex program | program `class + 7` of implementation 0's `vrtx` | engine slot 4's program 0 |

`compare` names each differing field: `render_<index>`, `stage<n>_<index>`,
`combiner_w<n>`, `texture<n>`, `constant<register>`, `vertex_program`. A
state set twice is compared at its last value. `render.scene` carries
`<kind>_material_{sampled,matched,mismatched,open,fallback,cut}`,
`<kind>_material_mismatched_<field>` and `<kind>_material_refused_<reason>`
for `particle` and `contrail` (`render-bridge/src/effect_materials.rs`).

Refused, by name, rather than approximated:

| Reason | Kind | Branch |
|---|---|---|
| `particle_model`, `not_shader` | particle | a `PRTM`/`prt3` or other tag in the shader's place |
| `reset` | particle | `0x0001bbf0`'s device reset (`[0x004670bc]` set) |
| `vertex_override` | particle | `0x0001ccb0(0x2b)` (`[0x00470a38]` and `[0x00470a39]` set) |
| `animated`, `engine_variant`, `engine_state`, `engine_texture`, `target_copy` | particle | animated parameters, a combiner variant or a state the draw's inputs choose, an engine texture or the lightmap, the frame copy |
| `tags`, `no_vertex_shader`, `library_loading`, `library_unread` | particle | the shader does not resolve, or the map's library is not read yet |
| `kind`, `blend_mode`, `view_matrix` | contrail | a block type other than 1, a blend mode past the tables, option bit 5 |
| `texture_streaming`, `texture_unbound` | both | a bitmap image that is not resident, or a bitmap with no images |
| `vertex_builtin`, `vertex_not_vrtx`, `vertex_program_index` | both | a program not in a `vrtx` tag |
| `no_setup` | both | no setup read for the draw |

Not compared: the stipple pattern (`0x003f8cc0`), the polygon offsets
`SetRenderState_ZBias` derives (slots 77 to 81), the vertex declaration
and the other vertex constants.

### The cut

Built (**Code**): `render-bridge/src/effect_materials.rs`. A gate per kind:

| | |
|---|---|
| Opens | 8 matched comparisons in a row |
| Closes | any mismatch or refusal; closing an open gate counts `<kind>_material_fallback` |
| Open | at the setup's call (`0x0023ea80` returning to `0x0022441e`, `0x00043720` returning to `0x00035b23`) one call in 64 runs the guest's setup and its first draw is compared; the others ask the host (`Host::material_cut`) |
| Cut | the setup does not run: `0x0023ea80` returns AL 1 (`ret 0xc`), `0x00043720` returns (`ret 4`); its draws are not compared; `<kind>_material_cut` |
| Kept | the host's refusal, `scene_kept_15_<kind>_material_<cause>`, closes the gate |

Kept on every call, by name:

| Kept | Why |
|---|---|
| `0x0002c3b0`, `0x0001bd50` | they write no device state, but later draws read their writes: `0x0001bd50`'s environment cache `[0x00485a58]` and dirty words, and `0x00016b90`'s engine inputs from `0x0055edd8` |
| `0x00035b00`'s own setup (`0x00016b10`, the declaration from entry `+0x10`, `0x0001c6b0`, `0x0001c710`, `0x0001cf50`) | the declaration is not in the material |
| `material_device_state` | the game's host refuses every cut after the material, its textures and program are made: the device resolves a draw's program, vertex format, combiner, textures and constants from its own state, and no admission draws from a supplied one, so a cut setup's draw would take the state the draw before it left |

The material's own refusals reach the same counter
(`scene_kept_15_particle_material_tags`, `..._library_loading`, ...).
Tests: `effect_materials.rs` (`a_proven_material_cuts_the_handler_setup`,
`one_draw_in_64_keeps_the_guest_setup_and_compares`,
`a_refused_material_keeps_the_guest_setup_by_name`).

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
