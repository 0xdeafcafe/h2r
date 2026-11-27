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
| while answering | each run of consecutive particle or decal entries (handlers `0x00173c90`; `0x00041cc0`, `0x000423c0`) goes to `RunHost::draw_display_entries` at `TRANSPARENT_DISPATCH_NEXT` after its last entry |

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

Still the guest's: every handler body (the particle draw `0x00174f80` and
its property evaluation, `0x00035b00`, the decal-list replays), the calls
of `0x00174420` themselves, and `0x00040e30`. No `RunHost` is installed,
so every run counts `entry_runs_no_host_<kind>`: the display copy's own
particles and decals need a host that draws them.


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

Not built: nothing draws these items. The decal vertex program (engine
slot 3) and the 192 constants are not ported or compared; a plate needs the placement
test `0x0003a5c0` and a snapshot of the view's colour. Undrawn items are
`scene_dropped_decal_<cause>` and `scene_dropped_weather_plate_<cause>`
once a host takes the slots; until then the guest draws them.

## Related

- [retire-d3d.md](retire-d3d.md): the plan and the other slices.
