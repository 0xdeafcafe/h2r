# Native objects: the first person

Status: Built. The first-person arms and weapon draw from the scene once a frame proves the native path; the lights' layers 12 and 13 draw from the scene ([native-lights.md](native-lights.md)), their layers 11 and 14 on D3D. Part of stage 2 ([native-objects.md](native-objects.md)).
Code: `crates/rewrites/render-bridge/src/native_first_person.rs` (the cut, `cut_call`, `build`), `crates/extensions/game/src/native_first_person_glue.rs`, `native_first_person_load.rs`, `crates/rewrites/engine/src/render/light_gather/{first_person_items,first_person_effects}.rs`. Tests: `crates/rewrites/render-bridge/tests/native_first_person.rs`, `crates/rewrites/engine/tests/light_gather_differential.rs`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

| Piece | Value |
|---|---|
| Guest entry | `0x0003fdf0` (`points::FIRST_PERSON_DRAW`), `ret 4`, AL = 1; the call (0 or 1) is the stack argument, the mode is in `eax` |
| Cut row | `first_person` in `render_bridge::cut::CUTS`; producer `feature::native_first_person` (`cut::OWN_PRODUCER`); census row `partial` |
| Slot | `native_first_person::SLOT`, `ViewSlot::FirstPerson` (slot 10), after the world's transparents (slot 9), before the flare query |
| View | the frame step's `ViewKind::FirstPerson`; its parts, in the view's order, are the arms and weapon |
| Split | the guest's: `0x0003fdf0` tests the stack argument at `0x0003ffa0` |
| Other slot-10 entries | the first-person lights (`0x000404b0`, [native-lights.md](native-lights.md)) and the sorted pass `0x00040870` |
| Call 0 | layers 1, 3, 8, 20 and 7, then layer 2 when `0x00044370` says so (`0x00040284`) |
| Call 1 | layer 16 only (`0x0003ffbf` sets it, `0x00040010` jumps to the same draw at `0x00040284`); `native_first_person::LATE_LAYER` is 16 |

## Third person and first person

The first-person weapon and arms are drawn by `0x0003fdf0`, called twice from `render_scene_passes` (`0x0002c2b3` with argument 0, `0x0002c2c2` with argument 1, the first-person lights `0x000404b0` between them), after the transparents. They are not drawn by the base layers' layer-3 call, which the caller check pins.

They have no camera of their own: `0x0003fdf0` and its call site (`0x0002c1d0`..`0x0002c2c2`) set none, so they draw with the render camera record (`0x004b9da0`) and the projection the game view drew with. In first person that record is the player's: the first-person camera update `0x0023c110` takes its field of view from `0x00187130` (its only caller), so the weapon and the world share one field of view. The arms and weapon sit in front of it in world space, posed by their node matrices.

The local player's third-person body draws natively like any biped when the game queues it.

## The cut

`0x0003fdf0` is a cut from the first frame, for both arguments. The entry makes the frame step's writes for the row, answers the call and draws from the scene (`native_first_person::cut_call`). The body never runs, and a part the native path cannot draw is a named drop (`native_first_person::Dropped`).

## Items

Each call builds its items before it draws, and the answer builds them in Rust (`engine::render::light_gather::first_person_items`):

1. **Prologue** (`prologue`). List 0 at `0x004c1a48` saves its layers at `+0x10`, clears them and starts at its count. The sorted list's mark `0x004b6284` takes its count.
2. **Effects** (below).
3. **Records** (`records`, reading list 0 and the sorted list as the effects left them). Each record index of `0x004c8740` (`[0x0050943c]` of them) that passes the mode's filter (mode 0 all, 2 `0x10`, else `0x20`), part by part as `0x000449e0`, `0x00045ce0`, `0x000460d0` and `0x00041040` in kind 0 make it. One list-0 item per part with layers, and a sorted entry (`0x0001e2d0`, depth along the view's forward vector less the record's bias) for a part with layer 15.
4. **Build** (`native_first_person::build`). It applies those writes, sets the current layer and list for each layer the call draws and puts its items in the first person's view. Then it makes the epilogue (list 0's layers back, its first item 0).

A record or shader the build does not read (a structure record, a virtual shader, a level table with no levels, a record with no node matrices) is the drop `items`.

## Effects

`0x00174220` with argument 1, which the body calls after its prologue, is ported (`engine::render::light_gather::first_person_effects`). It walks the particle systems (`[0x00510c74]`) in index order and selects each one that meets all of:

- its effect's owner is the current user (`[0x004b9ed8]`) with the first-person camera (`0x00155760` answers 0);
- its location reads (`+0x30`);
- it has no parent (`+0x4c`);
- it is in a visible cluster or has the definition's bit 9.

The per-view update `0x00174420` of each selected system stays a guest call (its stamps, which the effects shadow keeps render-private, and particle draws, [framescene.md](framescene.md)). It is made at the stack depth the guest makes it (`RESERVE`), with the selection bytes' upper stack bytes as the guest pushes them.

The selection is read again after each update. A particle model's definition (`PRTM`, `prt3`) is the guest's `0x001751d0`, called once per system. A system whose definition does not read stops the walk: the drop `effects`.

## Drops

There is no D3D draw and no mask edit. A part that does not draw is a named drop (`native_first_person::Dropped`), counted as `first_person_dropped_<cause>` (`native_first_person::drops`) and in the stage's `scene_dropped`.

| Drop | Cause |
|---|---|
| `items` | the item build does not read a record or shader |
| `effects` | a particle system's definition does not read |
| `no_view`, `no_host` | no first-person view, no host |
| `unreadable` | its instance, rows or order range are not in the scene |
| `unresident` | its model is asked for |
| `unsupported` | the model is resident, not this part or implementation |
| `reservation` | the host's |

`render.scene` reports them with `first_person_calls`, `first_person_drawn` and `first_person_dropped_<cause>` (`native_first_person::counters`). A part is readable when its instance and rows are in the scene.

## Host

`native_first_person_glue` loads the models (`native_first_person_load`: a utility-priority worker opens the map, waits for requested record models and uploads each into private GPU storage). It implements `Host::draw` for the offered parts.

Record `+4`, not the owner definition's third-person model, names the `mode` to load. Every section of that mode is built because the record key selects the permutation and LOD.

`Host::draw` draws the offered parts as one external pass, the way the scene's objects draw theirs. It reads the device's entry state at the call, plans each part from its instance and rows as extraction copied them (`plan_parts`, `LitObjects::plan_part_of`), warms the pipelines, reserves the pass on the bound attachments and commits it while the load it planned against is current. A part the plan refuses is `unsupported` (its reason logged once). A pass that cannot be reserved drops its parts as `reservation`.

The per-part rules are those of scene objects: the topology and subparts the record picks (`part_ranges`), the fade byte and the flagged shader ([native-objects-fade.md](native-objects-fade.md)), the auxiliary depth state (`aux_depth::read`) and the texture accumulation target (`sampled_texaccum`).

The stencil follows layer 3's walk ([native-objects.md](native-objects.md)):

| At the call | A part draws | Needs |
|---|---|---|
| stencil-off flags set (`skip::STENCIL_OFF_FLAGS`, bits 8..24) | with the test off (mode 0) | the device in mode 1 |
| a marked record (`PartFlags::STENCIL_MARK`) | in mode 2 (`object_state::mark`) | the device in mode 1 |
| neither | under the device's stencil | its fields read |

Mode 1 is what both brackets' epilogues leave (`native_objects_glue::in_stencil_mode_1`); without it the part is `unsupported`. The pass reports a depth-stencil write when a draw writes either (`draws_write_depth_stencil`).

## Tests

| Test | Shows |
|---|---|
| `the_plan_places_the_cut_when_on` | the plan places the cut |
| `a_frame_passes_when_both_calls_draw_every_part_the_guest_would` | the gate passes |
| `a_frame_with_a_dropped_or_missing_part_holds_the_cut` | the cut stays held |
| `the_cut_is_released_by_the_first_passing_frame_and_counted` | release and its counter |
| `the_guests_parts_are_its_items_in_the_layers_the_call_draws` | the guest's part count |
| `first_person_draws_after_the_world_transparents` | slot order |
| `each_call_draws_its_layers` | the call split |
| `an_unreadable_part_is_a_named_drop` | the drop name |
| `the_first_persons_view_takes_the_render_camera_the_game_view_drew_with` | the camera |
| `a_committed_pass_draws_every_planned_part_and_names_each_drop` (`native_first_person_glue`) | the host's pass |
| `first_person_items_match_the_lifted_original` (`engine/tests/light_gather_differential.rs`) | runs `0x0003fdf0` lifted to the end of its record loop against the item build, byte for byte |

The tests down to `the_first_persons_view_takes_the_render_camera_the_game_view_drew_with`, and the cut row's, are in `render-bridge/tests/native_first_person.rs`. The unit tests of `first_person_effects` drive the walk with `entry::testing::run` (index order, an update that retires a later system, a third-person camera, a particle model's guest definition). The first-person lights are the lights stage's ([native-lights.md](native-lights.md)).
