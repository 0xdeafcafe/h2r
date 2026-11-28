# Light gather: counters, tests and live checks

Status: Partly built. The checks of L1 to L8 exist; those of L9 and the live parity and timing gates are open.
Code: `crates/rewrites/engine/tests/light_gather.rs`, `light_gather_differential.rs`, `crates/rewrites/render-bridge/tests/native_lights.rs`, `crates/rewrites/render-bridge/src/native_lights/handoff.rs`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

This page is the check list for the slices of [native-light-gather.md](native-light-gather.md): the counters that report each body, and per slice the tests, the live check and the refusal. Every slice's gate starts with the hash gate: `sim.hashes` equal row by row on two launches, `impl.render.native=off` against the default (framescene.md 9.3). "Differential" means the lifted original on the IR interpreter from seeded states, compared as `crates/rewrites/engine/tests/sim_objects_differential.rs` compares: the callout transcript (target, return site, registers, all memory outside the dead stack) and the final state.

| Slice | Differential | Unit tests in |
|---|---|---|
| L1 | roots `0x00031910`, `0x00031660`, `0x00031c20`, `0x000c17f0` | moved with the code |
| L2 | `frame_transcripts_match_the_lifted_original` | `engine/tests/light_gather.rs`, `guest-layout`, `hooks` |
| L3 | `light_record_walks_match_the_lifted_original` | `world_walk.rs`, `light_gather.rs` |
| L4 | `item_lists_match_the_lifted_original` | `light_gather.rs` |
| L5 | `passes_transcripts_match_the_lifted_original` | `engine/tests/light_gather.rs`, `native_lights/handoff.rs`, `light_gather/passes.rs` |
| L6 | `first_person_lights_match_the_lifted_original`, `first_person_records_match_the_lifted_original` | `engine/tests/light_gather.rs`, `light_gather/passes.rs` |
| L7 | `object_shadow_list_matches_the_lifted_original`, `object_shadow_rows_match_the_lifted_original`, `object_shadow_caster_matches_the_lifted_original` | `object_shadows/` |
| L8 | seeded XBE differentials, opacity and first-person exit differentials | graph tests, mixed GPU fixture |

## Counters

Counters follow the scene lists' scheme (`timeline::native::SceneList`, `SceneListResult`). Each stage reports `_native`, `_fallback` and a reason. `render.stats.counters` reports per-frame means for the stage columns.

| Stage | `_native` counts | `_fallback` and reason count |
|---|---|---|
| `light_frame` | a frame the body makes (with `visible_lights_native`) | a refused plan |
| `light_passes` | a passes call the passes body runs | a plan that refuses (the original runs whole) |
| `first_person_lights` | a call of `0x000404b0` the body runs | a plan that refuses |
| `first_person_records` | a slot record that passes the mode's filter, appended in Rust | one the guest's part calls make, and a call whose plan refuses |
| `light_items` | a record of a light that passes the walk's filters, appended in Rust | one the guest's part calls make, and a call whose plan refuses |
| `light_records` | each of a light's two record walks | a walk the guest makes (a cold section is `residency`) |
| `light_volume`, `light_geometry`, `light_extra_caster` | a step made in Rust | a step given to the guest (the last two run inside the guest call) |
| `light_handoff` | a light the scene takes from the hand-off | a light read from memory (the item body declined, or no item lists' call took the hand-off: `layout`) |
| `object_shadow_list`, `object_shadow_rows`, `object_shadow_caster` | one per completed body | one per refused plan, with its reason |

Each light taken from a hand-off is checked against guest memory once its lists are made: `_compared` per light, `_mismatch` per difference.

L8 counts one `native_scene_lists_typed_light_passes_native` per committed dynamic sequence and `native_scene_lists_typed_first_person_native` per committed final first-person slot; the `_fallback` and reason columns count refused typed offers. On the layer route, `native_scene_lists_light_additional_casters_native` counts an empty additional-caster loop and its `_fallback` a loop left to the guest.

`render.scene` reports the filter and layer draws ([agent-api.md](../runtime/agent-api.md)).

## L1: the model and the built leaves

- **Tests.** Differential roots `0x00031910` (its tree `0x0004a7c0`, `0x0004ac30`, `0x0004b160`, `0x00031590`, `0x0003e9c0` lifted, no stub), `0x00031660`, `0x00031c20`, `0x000c17f0`: seeded pools of lights, owners and histories, more than 32 and more than 128 candidates, the frozen override, each definition bit the code tests.
- **Live check.** `render.stats`: `native_scene_lists_visible_lights_native` and the two others unchanged in rate against master.
- **Refusal.** A seam that refuses leaves its guest function to run.

## L2: the frame body

- **Tests.** Differential root `0x00031c80` (`frame_transcripts_match_the_lifted_original`). Its tree is the root and the leaves of L1, lifted; every other callee is a seeded stub that writes what the frame reads after it (the re-posed frame, the frusta count, a later row's weight). Equal transcripts (target, return site, the registers each callee is handed, its stack arguments), equal EAX and equal memory outside the dead stack and the private spills, over 0, 1, 8, 9 and 32 drawn lights, NaN and threshold weights, each branch of steps 5, 8 and 9, and frames ranked from candidates.
- **Unit.** Over `entry::testing::run`: `a_light_frame_calls_the_gather_in_the_guests_order`, `the_ninth_light_takes_no_first_person_slot`, `a_refused_plan_writes_nothing`. The reserve: `a_reserved_call_is_made_below_the_reserved_bytes` (guest-layout) and `a_reserved_guest_call_leaves_the_bodys_frame_alone` (hooks).
- **Live check.** `light_frame_native` per view per frame on a lit interior, `light_frame_fallback` zero; `render.scene` lists the same lights, weights and shadow flags as master's observation; `render.parity` within the bar.
- **Refusal.** `plan` refuses before any write (memory, layout, unsupported float state: `fp_supported` and the x87 control word as `read_visible` checks) and the guest function runs whole. After publication only steps 6, 12 and 13 can refuse, each to its guest function.

## L3: the light's records

- **Tests.** Differential `light_record_walks_match_the_lifted_original`: roots `0x00133390` (EBX `V`) and `0x00133520(V)` with the record writer `0x0002cbf0` and its tree lifted, over a seeded structure, a light's struct of kind 1 to 3, fog views, full subpart pools, cold sections and a cursor at the cap; equal memory, and the main view's count left. The frame differential tallies how each walk was made.
- **Unit.** `a_lights_records_start_at_the_main_views_end`, `a_lights_walk_leaves_the_main_count`, `a_struct_whose_kind_does_not_match_it_is_left_to_the_guest` (`world_walk.rs`), `a_lights_walks_are_made_in_rust_in_its_kind` (`light_gather.rs`).
- **Live check.** `light_records_native` at two per drawn light outside cold sections and `light_records_fallback` with `residency` only for them in `render.stats`; hash gate; `render.parity`.
- **Refusal.** A walk that refuses gives that walk to its guest function, whose hooks retry row by row as in the main view: the row hook at `WORLD_VISIBILITY_FLAGS` takes a light's rows as the main view's.

## L4: the item lists and the first-person table

- **Tests.** Differential `item_lists_match_the_lifted_original`: root `0x00040e90` in the light's kind (kinds 1 to 3) over synthetic sections, shaders and level tables, with `0x00044ac0`, `0x000449e0`, `0x00045ce0`, `0x000460d0`, `0x00041040`, `0x00040de0`, `0x00040e30`, `0x0003b9d0`, `0x0001bf50`, `0x00184000` and the plane lookup lifted and `0x00041c80` stubbed: equal transcripts, EAX and memory.
- **Unit** (`light_gather.rs`). `a_receiver_goes_to_list_one_and_a_caster_to_list_two`, `a_first_person_record_is_copied_to_the_current_slot`, `records_past_the_eighth_light_join_the_last_slot`, `a_full_slot_drops_the_record`, `a_record_the_port_does_not_take_is_made_by_the_guests_part_calls`, `the_main_views_kind_is_not_the_bodys`.
- **Live check.** `light_items_native` on a lit interior, its `_fallback` only from objects' records; `scene_light_entry_remainder_*` unchanged; the groups `render.scene` dumps are equal to master's on a paused tick.
- **Refusal.** A record the port does not take is made by the guest's per-part calls for that record, in place. A plan that refuses (a read, a record range past `0x8000`) runs the guest function whole.

## L5: the scene's lights and the passes body

- **Unit, hand-off.** `each_light_is_handed_over_just_before_its_passes_with_their_call` (`engine/tests/light_gather.rs`: one hand-off per drawn light, equal to its passes call, with no guest call between), `the_scene_light_from_the_hand_off_equals_the_one_read_from_memory` (the same frame read back after the lists are made: equal sources and equal steps for every flag), `at_the_passes_entry_the_lists_are_the_last_lights` and `a_hand_off_is_taken_only_by_its_own_passes_item_lists` (`render-bridge/src/native_lights/handoff.rs`).
- **Unit, passes body.** `each_layer_block_runs_in_the_guests_order` and `a_return_slot_names_its_site_only_while_its_call_runs` (`light_gather/passes.rs`), and `passes_body` in `engine/tests/light_gather.rs` (a shadowed light's whole call order, a shadow the casters do not take, a refused plan that only reads).
- **Differential.** `passes_transcripts_match_the_lifted_original` (`engine/tests/light_gather_differential.rs`), root `0x00031170` with its fourteen callees stubbed, equal transcripts and memory over random flags, mask bits, the override byte and the additional casters' condition; the existing `engine/tests/light_passes*.rs` stay.
- **Live check, hand-off.** `native_scene_lists_light_handoff_native` equal to the drawn lights (`light_frame_native` frames times their lights), `_compared` equal to it and `_mismatch` zero, `_fallback` zero on a lit interior; `render.scene` lists each view's lights with the master's handles and weights, and steps whose `draws` follow the light's own lists.
- **Live check, passes body.** `native_scene_lists_light_passes_native` equals the drawn lights and `_fallback` is zero; `lights_walked_native` and `scene_light_layers_alone` unchanged; `render.parity`.
- **Refusal.** An item body that declines, a passes' entry that is not the hand-off's call, or a hand-off no item lists' call takes leaves the light to the read from memory, counted. A passes plan that refuses runs the original whole, counted; none after entry, since every step has its guest call.

## L6: the first-person lights

- **Differential** (`engine/tests/light_gather_differential.rs`). `first_person_lights_match_the_lifted_original`: root `0x000404b0` with every callee stubbed but the light volume (lifted in both runs), over no slot, one, eight and a count a callee changes, empty and negative slots, each layer's mask bit, a kind-0 volume, colour B dark, negative, NaN or bright, and a full push buffer. `first_person_records_match_the_lifted_original`: root `0x000402b0` with its part calls lifted over the item lists' synthetic structure (records appended in Rust) and stubbed over objects' records (the transcripts), over the three modes, an empty slot, a slot of 32 and a negative count.
- **Unit** (`engine/tests/light_gather.rs`, `first_person_body`). `the_first_person_lights_call_the_guest_in_its_order`, `a_full_push_buffer_asks_for_room_first`, `no_slots_draw_nothing_and_more_than_eight_are_the_guests`, `a_replayed_record_reads_the_bytes_at_its_new_index`, `an_empty_slot_zeroes_the_lists_and_returns_its_count`; and `a_call_named_inside_another_puts_the_outer_naming_back` (`light_gather/passes.rs`).
- **Live check.** `native_scene_lists_first_person_lights_native` once per view with a first-person weapon, `_fallback` zero, and `native_scene_lists_first_person_records_native` with its `_fallback` only from objects' records, on a level with a flashlight or a muzzle light; `scene_first_person_light_layers_alone` unchanged; the `first_person` producer's entries in `render.residual` unchanged (layers 11 and 14 stay D3D); `render.parity` over the weapon's rectangle.
- **Refusal.** A plan that refuses runs the guest function whole. After it, every step is a guest call or a write; a record the port does not take is the guest's part calls for that record.

## L7: the object shadows

- **Differential.** `object_shadow_list_matches_the_lifted_original`, `object_shadow_rows_match_the_lifted_original` and `object_shadow_caster_matches_the_lifted_original` compare named call transcripts, EAX, stack cleanup and live memory. The seeds cover both failed list gates, up to 32 draws, stale keep bytes, histories changed by predicates, culling, the strength threshold, and layer and filter calls that change later inputs.
- **Unit.** `object_shadow_caster_refuses_before_writing` compares the full image on refusal. `an_object_shadow_row_keeps_the_buffers_stale_keep_byte` checks all 256 byte values.
- **Live check.** On an outdoor view with mask casters, `render.stats` reports positive `native_scene_lists_object_shadow_list_native`, `native_scene_lists_object_shadow_rows_native` and `native_scene_lists_object_shadow_caster_native`. Check each `_fallback` against its reason, including `_unsupported` for creatures and `_residency` for cold lighting. Compare `shadow_walk_layers`, the scene's `scene_shadow_layers_alone_10`, `_11` and `scene_shadow_filters_native` against the reference on the same view. `render.parity` covers the caster and receiver outdoors.

## L8: the passes typed

- **Differentials.** The seeded XBE differentials cover sphere bounds, empty additional-caster loops and the layer and target state a following guest light reads, including the draw byte when layer 13 is masked in but the current light only reaches layer 12. The opacity differential checks both forms of the target-1 bind and the following view restore. The first-person exit differential checks earlier-slot refusals and the final slot's return registers, stack and surviving memory.
- **Graph and fixture.** Graph tests check each site's draw order and late refusal. The mixed GPU fixture compares a diffuse/specular pair beside guest draws against all-D3D pixels, including a late specular refusal.
- **Gate.** Check the per-site native and fallback counts above and compare lit interiors with `render.parity` at 1x and 4x. L9 depends on the remaining shadow and gel paths and this gate.

## Related

[native-light-gather.md](native-light-gather.md), [retire-d3d.md](retire-d3d.md), [native-lights.md](native-lights.md), [native-shadow-layers.md](native-shadow-layers.md).
