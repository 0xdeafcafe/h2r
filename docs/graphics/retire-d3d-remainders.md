# Retiring D3D: what each slice leaves, and what R10 and R11 delete

Status: Design. The remainders of the built parts of R1 and R5, the gaps in the inventory's data, and the code the last two slices delete.
Code: `crates/rewrites/render-bridge/src` (the counters named here). Tests: named per section.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

What still reaches the guest's draws in [retire-d3d.md](retire-d3d.md)'s built slices, by the counter that names it, and what the inventory has no data for. The plan, the census and the slices are on that page.

## R1's remainder

The opaque block's layer draws are `kept` (`residual_census::KEPT_LAYERS`):
the scene's layers 1, 3, 5, 7, 8 and 20, the alternate view's 1, 3 and 5,
the first person's 1, 3, 7, 8 and 20. `frame_step::layer` is
`LAYER_DRAW`'s implementation (`native_layers::layer_turn`): after the
stages' observers, a call none of whose items holds the layer's bit makes
the body's own writes (`0x004858b0` back to -1, the draw byte
`0x004670bc`, the cache `0x00485b48` cleared as `0x00016b10` clears it)
and returns, making none of 5.2's rows (`tests/frame_step_layer.rs`); with
an open shader in the cache it first writes the shader's restore list
("The open shader", below). Any other call runs the guest body for the
items still holding the bit, counted
`layer_return_<site><layer>_guest_<cause>` (`standing_down`, `unread`,
`open_shader`: the cache's open shader with an item held or a restore list
that does not read; or `structure`, `object`, `other` for the first held
item); each return is `_returned` or `_returned_open_shader`.

Every item such a body draws is one
`scene_kept_<site><layer>_<kind>_<cause>` count (`<site>` empty, `alt` or
`fp`; `<kind>` `world`, `object`, `other` or `unreadable`):

| Counted by | When | Items |
|---|---|---|
| `native_layers::count_kept` | before the call | layers 1, 5, 7, 8 and 20 at every site, by the plan's causes (below) |
| `native_world` | before the call | layer 3's structure items: the host's cause, `part`, `view`, `stencil_plan`, `host` |
| `native_objects::keep` | in the body, per object | layer 3's objects at the scene and the alternate view: `unsupported`, `order`, `model`, `inputs`, `state`, `reservation`, `unresident`, `scene`; one `unreadable_list` per walk whose entries do not read |
| `native_layers::layer_return` | at the call, per item | every other held item: the cause a stage left for its kind (`native_layers::leave`: the decorators' `decorator_{host,scene,view,unready,state,declined}`, the walk's `walk_{host,list}`, `host`), else `standing_down`, else `first_person_held` for the first person's layer 3, else `unnamed` |

A kind a stage draws inside the body is that stage's to count
(`native_layers::in_body`): the walk's objects and the resolved world's
structure entries. An item whose render record index is negative builds no
entry and is not counted. Rule 1 of 8.2 holds for no entry, so nothing is
dropped. Tests: `every_item_the_guest_body_draws_is_counted_once_under_its_stages_cause`,
`the_first_persons_kept_items_count_under_its_site`,
`a_layer_with_no_host_counts_its_items_under_host`,
`the_opaque_blocks_layer_draws_return_or_run_the_guest_counted_by_cause`
(`native_layers`),
`the_opaque_blocks_layer_draws_are_kept_the_others_a_stage_draws_partial_and_the_rest_d3d`
(`residual_census`).

What keeps an item, by stage (**Code**):

- **Layers 1, 5, 7, 8 and 20** (`layer_plan_<layer>_refused_*`,
  `_alt<layer>_*`, `_fp<layer>_*`; a sorted run counts the held layer's
  `scene_kept_*` when done). Every cause left:

  | Items | Cause | Why the guest keeps the draw |
  |---|---|---|
  | structure | `material`, `instance`, `no_pass`, `pass`, `implementation`, `input`, `animation`, `texture_transform`, `object_animation`, `property`, `wave`, `material_c18`, `time`, `per_draw` | the plan does not resolve it |
  | structure | `program`, `palettes`, `device` | no material of the layer compiled, or its palettes changed |
  | structure | `state`, `stencil`, `warming`, `set` | skipping would leave another state; the stencil does not read or mode 1 is not held; no pipeline yet; no compiled part or fog |
  | structure | `first_person_structure`, `sorted_structure` | the first person's and sorted handlers' calls take objects only |
  | object | `object_shader_flag`, `object_layer5_stream`, `object_subparts`, `object_aux_state`, `object_unread` | extraction's (`object_shader_flag` also where the device or engine does not hold the structure upload) |
  | object | `object_models`, `object_model`, `object_layer` | models not resident, the item's model not uploaded, a layer outside `OBJECT_LAYERS` |
  | object | `object_plan_<cause>` | the material's passes did not plan (`not_compiled`: a constant or register the draw does not give, an object-function property, an engine wave before `Tables::install_engine_slab`, a 2D bitmap at `dot_rflct_spec`; [native-objects.md](native-objects.md), "Object programs") |
  | object | `object_w_buffer`, `object_layer_begin` | the draws are not W-buffered, or the begin's `ZENABLE` did not set |
  | object | `object_shader_flag_target` | a copying item whose refraction target `0x16` does not read |
  | object | `object_texaccum` | no accumulation target or level to bind |
  | object | `object_aux_depth`, `object_fade`, `object_stipple` | the device does not hold the toggle off; a stipple or fade pattern not known exactly; the cached fade or pattern is not the epilogue's |
  | object | `object_state`, `object_prt`, `object_fog`, `object_stencil`, `object_warming` | another state left; the PRT stream; no fog; a marked item outside mode 1; no pipeline yet |
  | any | `order` | after a kept item it does not commute with; at the scene and the alternate view such items draw between the kept sorted entries ([retire-d3d-r5.md](retire-d3d-r5.md), "Layer 2 from the plan") |
  | any | `viewport`, `unread`, `cached_state`, `reservation`, `world_changed`, `masks`, `entry` | the device's clip is not the viewport; the pass, the cached restore or the masks do not read; the device refused the pass; the world changed; the run stopped at a kept entry |

  The first person keeps its flag-bit-20 items before the frame's copy
  (`native_first_person_glue`).
- **Layer 3's world**: untranslated passes and missing program inputs ([native-world.md](native-world.md)).
- **Layer 3's objects** draw before the call where the gate answers
  ("Layer 3's objects before the call", below), else inside the body: the
  walk takes a run at its first geometry handler
  (`native_objects::walk_geometry`), and the layer runs the guest body
  (`layer_return_3_guest_object`). Their refusals: `walk_unsupported` (a
  bit-11 part whose toggle does not read, a bit-6 part with a negative
  first subpart or a mask that does not read, a flagged shader with the
  structure's parameter block or a structure lightmap index, an item's
  `+0x14` or `+0x15`; by part in `walk_part_*`), and a run declined by
  `Model`, `Inputs`, `State`, `Unresident` or `Reservation`
  ([native-objects.md](native-objects.md)).
- **Layer 3's decorators**: a material not ready, a pass with an
  engine-driven state from a per-draw source or a lightmap stage whose
  globals bitmap is not uniform, a skip that would change a render
  state, and a view whose W mode, stage palettes, stencil or inherited
  cull do not read or hold ([native-decorators.md](native-decorators.md)).
- **The first person**: `0x0003fdf0` is a cut from the first frame. No
  layer draw runs under it, and a part it cannot draw is a named drop
  (`native_first_person::Dropped`).

### Layer 3's objects before the call

`render_bridge::native_objects::before`, at the scene's and the alternate
view's layer 3 (**Code**):

| Step | What |
|---|---|
| The list | each item still holding bit 3 built as `0x0004cbf0` builds it (`native_world::scene_build`), the accepted slots sorted as `0x0013da70` (`layer_sort::geometry`) and read as the walk reads the guest's (`object_walk::read_slots`); another build's item refuses (`foreign`) |
| The entry state | the host's fields (`Host::entry_state`): the entry render states, W buffer, stencil words, stipple, specular, two-sided lighting, shade mode, shader cache, fog words and the auxiliary byte, predicted from the device before the call with the layer begin's `ZENABLE` and the cache `0x00016b10` clears; an open shader refuses |
| The gate | 1 frame in 64 (`0x004ba034`) runs the body as before and compares, at the walk's first handler, the guest's entries with the Rust list and, where no entry drew first, the fields with the prediction; answers after 16 clean samples, the guest's at any difference |
| The draw | while answering, on an unsampled frame whose list holds only objects, the leading runs draw (`Host::draw_before`) and their items' bit 3 is cleared until the next layer draw; from the first object they do not take every object keeps its guest draw (its cause, else `order`) and the walk does not run |

`render.scene` carries `layer_plan_3_objects_before_<outcome>`: `drawn`,
`matched`, `mismatched_<field>` (`entries`, `entry_count`, a render state
by name, `w_buffered`, `cached_implementation`, ...), `state_not_first`,
`refused_{open_shader,list,count,foreign,build,entries,state,scene}`,
`kept_entries` (a structure or other item left: the walk keeps the
order), `frames_{clean,unclean,empty}`, `opened`, `fallback`. Tests:
`the_gate_answers_after_clean_samples_and_stops_at_a_difference`,
`a_prediction_differs_by_field`,
`a_sampled_layer_3_is_compared_at_its_first_handler_by_field`,
`the_leading_runs_draw_before_the_call_and_the_rest_keep_their_order`,
`a_runs_cleared_bits_go_back_at_the_next_layer_draw`,
`the_rust_list_refuses_another_build` (`native_objects::before`).

### The open shader

`render_bridge::open_shader`, at every call `frame_step::layer` keeps only
for the cache's open shader (**Code**). The body's `0x00016b10` calls
`0x00018900` on the implementation's restore range (`+0x11c`: first
`& 0x1ff`, count `>> 9`, five-byte entries in the cached descriptor's
`+0x14` table):

| State | `0x00018900` writes | The replay (`native_layers_glue::restore_list`) |
|---|---|---|
| below `0x5c` | `SetRenderState_Simple` (header from `0x0043e6f0`), then the shadow `0x00407498` | `Device::set_render_state_simple`, then the shadow |
| `0x5c..0x87` | the mask (`0x0043e4d0`) into the dirty word `0x00407490`, the shadow | the same words |
| `0x88` and up | the setter at `0x004038d8 + 4·state` | `Device::set_render_state` on the setter's slot (`d3d::abi::RENDER_STATE_SETTERS`); another setter declines (`declined_setter`) |

Until the gate answers every call is a reference: the layer draw calls the
guest's `0x00018900` (`call_guest`) and compares each listed state's
shadow and the dirty word with the prediction (`open_shader::predict`).
After 16 clean references it answers: one call in 64 stays a reference, the
rest replay. Either way the body's writes follow and the call returns.
`render.scene` carries `open_shader_<outcome>`: `referenced`, `replayed`,
`matched`, `mismatched_<field>` (`state_<hex>`, `dirty`), `opened`,
`fallback`, `declined_<why>`, `unfinished`. Tests:
`the_restore_list_is_read_and_classed_as_the_original_applies_it`,
`the_prediction_is_each_states_last_value_and_the_dirty_masks`,
`the_gate_answers_after_clean_references_and_stops_at_a_difference`,
`a_declined_replay_takes_the_reference` (`open_shader`),
`an_open_shader_with_every_item_native_is_restored_and_returns`
(`native_layers`),
`the_open_shaders_restore_writes_each_entry_as_its_call_would`
(`native_layers_glue`).

Not built: each `scene_kept_*` row becoming `scene_dropped_*` once it is
zero over the census corpus (8.2, rule 1).

R1, R2 and R6 do not depend on each other and can run as three lanes
after R0. framescene.md slice 8 (60 and 120 fps from two scenes) is
independent of all of them once its slice 6 holds.

**R10 deletes**: `pushbuf.rs`, `state.rs`, `nv2a.rs`, `nv2a_draw.rs`,
`nv2a_hot.rs`, `shader.rs`, `vertex_cache.rs`, `dirty.rs`, `regs.rs`,
`glyph.rs`, `pipeline_store.rs`, `device/` except what R11 lists,
`backend.rs`, `gfx_backend.rs` and `gfx_backend/`, `scaled.rs` and
`scaled/`, `threaded.rs` and `threaded/`, `scene_sink.rs` and
`scene_sink/` (with `retarget.rs`, moved to the native side first),
`scene_stage.rs`, `clip_space/`; `DrawAdmission` and every `render_bridge::native_*` hook;
`frame_types::carried` (the `frame-types` crate's carried part); in `gfx`, what only a resolved D3D draw uses
(`state.rs`, `translate.rs`, `xdk.rs`, the D3D pipeline key); the
`render-timing` and `render-census` features and `render.parity`.
`formats/shaders` stays: the native stages translate tag shaders with it.

**R11 deletes** `abi.rs`, `runtime_glue.rs`, `resource.rs`, `layout.rs`,
`guest.rs`, `hw.rs`, `vblank.rs`, `events.rs`, the crate, its dependents'
edges (`boot`, `game`, `cli`, `h2host`, `uwp-host`, `tools/perf`),
`impl.render.native`, and [d3d.md](d3d.md)'s device model. The entry
inventory in d3d.md is a fact about the retail image and stays as a
reference page.

## R5's remainder

What keeps a guest draw in R5's stages, and how fog and water draw per item in the guest's order: [retire-d3d-r5.md](retire-d3d-r5.md).

What still reaches the guest's draws, by producer:

| Producer | Still the guest's | Counted |
|---|---|---|
| Layers 2, 6, 16, 17, 18, 19, 21 | the body, for a call with an item held; the first person's 16 always | `layer_return_<site><layer>_guest_<cause>`, each item `scene_kept_*` |
| Sky `0x0003ede0` | every pass until the cut's arm is ready, or after it stops for the BSP; a refused cut draws no sky (`scene_dropped_sky_*`) | `sky_cut_*`, `scene_kept_sky_*` |
| Post `0x00013870` | its body: target setup, the colour pass and screen effects the host does not arm | `scene_kept_post_*` |
| Flares `0x0002dd30`, `0x0002df00` | the passes' state setup and the draw pass's reflection walker (a glow, query or read the cut does not make draws nothing, `scene_dropped_flare_{glow,query,read}_*`) | `scene_kept_flare_*` |
| Clears `0x00014b60`, binds `0x0001cd30` | longer rectangle lists, final resource destruction, the viewport reset | none |

The post, the flare passes and the binds need the graph to own the view's
targets before they are cut: their native passes draw into the device's
live targets at the guest's calls.

## The residual callers

The callers `render.residual` names with residual draws in a 16-bot game,
identified in [notes/residual-callers.md](notes/residual-callers.md)
(numbers and reads there).

| Caller | What it is | What claims it, or what claiming it needs |
|---|---|---|
| `0x4dad0` (site `0x4dae0`) | the emitter `0x0004daa0`'s whole-part indexed draw, shared by every layer's geometry handler and the sorted layer-15 handler | nothing of its own: its draws are each stage's refusals. A census with `frame-scene.census` files them by layer draw (`producers`), and `render.scene`'s `scene_kept_*` names their causes |
| `0x4ebd0` | decorator types 1 and 2 (decals) in the transparent layers | the decal-decorator stage: each body's `DrawVertices` drawn natively at the call, in sorted order; once a frame proves it, a call the last body's template stands for returns (`ret 0x1c`) and draws the template with its own vertices; the rest count `scene_kept_decal_decorator_<cause>` ([retire-d3d-decal-decorators.md](retire-d3d-decal-decorators.md)) |
| `0x4c690` | the sky model renderer's strips | the sky stage and its cut ([retire-d3d-r5.md](retire-d3d-r5.md), "The sky's cut"). A strip draws with its own stencil state, the test off on a target with no stencil |
| `0x25d50` | the planar fog's screen quad (`0x00034420` with `0x00026520` or `0x00025e50`) | modes 2 and 3: the quad drawn natively and the setup returned with no fan once a reference matches ([retire-d3d-fog-quad.md](retire-d3d-fog-quad.md)); modes 1, 4 and 5 (`0x00025e50`, not read) stay the guest's, `scene_kept_fog_quad_other_setup` |
| `0x00224fd0` (site `0x225475`) | **S**: particles drawn as model parts, a callback of `0x00175610` | a particle-model stage. Not built |

## External pass refusals

A device refusal includes the producer's Rust pass type, its issue boundary, and the native stage for a typed plan. The first refusal for each producer, boundary, stage and reason is logged. Device counters retain every refusal.

A failed reservation leaves the original guest draws available to its caller. A refusal from a pass with no corresponding guest draw can instead omit that operation.

`d3d::SceneSink` declines untyped passes and unsealed reservations inside a captured scene slot, except the commits of native producers it carries (water, flares and UI; [framescene.md](framescene.md)). The display graph needs retained inputs to record the operation on its own attachments. A one-shot pass carries no replay command.

A swallowed post call whose pass records again (the glow and the screen effects, once their raster is frozen) commits as a carried scene stage (`Device::swallow_commit`, `prepare_carried_stage`, `commit_carried_stage`), which the sink retains as a `ReplayOp::Stage`, so it is not declined; a guest reference request, or a backend that cannot seal the stage, sends the call to `prepare_external` and the decline, and its packets replay. The sink names this case with the producer, so it can be distinguished from a binding failure or a backend recording failure. Outside captured slots these passes still use their normal backend path.

## Missing data

- No per-caller count of residual draws, D3D entries or state and push
  words exists with the native stages on. Every count above is from the
  D3D path or is a per-stage refusal counter.
- No timing of the front-end by caller exists with the native stages on.
  The last A/B gives only the whole game thread: 34.7 ms against 23.9 ms
  in one late window ([notes/native-ab.md](notes/native-ab.md)).
- The HUD, the UI, the fade, the frame end, movies and loading have never
  been timed or counted by caller on either path.
- Multiplayer maps have no residual data at all. Every census run is Old
  Mombasa or the menu.
- `render.scene`'s entry census (`frame-scene.census`) counts
  `d3d_draws` under the layer draws, the light passes, the object shadow
  and the screen effects, and the scene sink (`frame-scene.sink`) counts
  ops by slot. Both exist; neither has a recorded result in `notes/`.

## What R10 and R11 delete

**R10 deletes**: `pushbuf.rs`, `state.rs`, `nv2a.rs`, `nv2a_draw.rs`,
`nv2a_hot.rs`, `shader.rs`, `vertex_cache.rs`, `dirty.rs`, `regs.rs`,
`glyph.rs`, `pipeline_store.rs`, `device/` except what R11 lists,
`backend.rs`, `gfx_backend.rs` and `gfx_backend/`, `scaled.rs` and
`scaled/`, `threaded.rs` and `threaded/`, `scene_sink.rs` and
`scene_sink/` (with `retarget.rs`, moved to the native side first),
`scene_stage.rs`, `clip_space/`; `DrawAdmission` and every `render_bridge::native_*` hook;
`frame_types::carried` (the `frame-types` crate's carried part); in `gfx`, what only a resolved D3D draw uses
(`state.rs`, `translate.rs`, `xdk.rs`, the D3D pipeline key); the
`render-timing` and `render-census` features and `render.parity`.
`formats/shaders` stays: the native stages translate tag shaders with it.

**R11 deletes** `abi.rs`, `runtime_glue.rs`, `resource.rs`, `layout.rs`,
`guest.rs`, `hw.rs`, `vblank.rs`, `events.rs`, the crate, its dependents'
edges (`boot`, `game`, `cli`, `h2host`, `uwp-host`, `tools/perf`),
`impl.render.native`, and [d3d.md](d3d.md)'s device model. The entry
inventory in d3d.md is a fact about the retail image and stays as a
reference page.

## Related

- [retire-d3d.md](retire-d3d.md): the plan, the census and the slices.
- [retire-d3d-display.md](retire-d3d-display.md): R8, the frame step, movies and the display.
- [native-objects.md](native-objects.md), [native-world-walk.md](native-world-walk.md), [native-decorators.md](native-decorators.md): the stages whose refusals are listed.
