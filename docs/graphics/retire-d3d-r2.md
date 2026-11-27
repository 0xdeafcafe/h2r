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


## Related

- [retire-d3d.md](retire-d3d.md): the plan and the other slices.
