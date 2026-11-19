# Halo 1 unit, object and miscellaneous script builtins

The converter resolves retail equivalents in `h1scripts::unit_builtins`
and `h1scripts::identified`. The runtime feature in
`boot::h1_script_builtins` adds behavior on converted campaign maps.
Addresses below are from the owner's retail Xbox executables.

## Retail equivalents

| Status | Halo 1 builtins | Halo 2 evidence |
|---|---|---|
| Built | `unit_open`, `unit_close`, `unit_suspended` | Slots 194, 195, 249 call the unit open, close and suspension routines from `0x2a32a0`, `0x2a32e0` and `0x2a4570`. The first two share the converter's existing checks for absent named units. |
| Built | `unit_enter_vehicle` | Slot 216 dispatches the vehicle entry routine from `0x2a3ad0`. At `0x11ade0` the engine compares the label at offset 4 of each vehicle seat. The converter resolves the script's seat text with the same name function as the rebuilt vehicle seats. |
| Built | `unit_set_seat` | Halo 1's `0xf11d0` resolves a pose through the six-entry table at `0x1f7684`. Halo 2 slot 234 at `0x2a41a0` writes an animation mode ID. The converter reads the pose names from the Halo 1 XBE and resolves each recognised pose through the converted animation graph's naming rules. An unrecognised pose selects the default mode. |
| Built | `unit_set_emotion`, `vehicle_riders`, `vehicle_gunner`, `unit_get_total_grenade_count` | Slots 218, 236, 238 and 241. The first sets emotion by its numeric index (`0x10e9f0`), the next returns the rider list (`0x11b040`), and the third searches for the gunner seat type (`0xbadc0`). Slot 241 reads the unit's grenade counts. |
| Built | `object_destroy_all`, `object_set_ranged_attack_inhibited`, `object_set_melee_attack_inhibited`, `object_set_collideable`, `object_beautify` | Slots 54, 66, 67, 72 and 93. The first sends the all-types mask to `0x29fe10`. The next two write the attack flags at `0x2a1920` and `0x2a19a0`. The collision flag at `0x2a1b50` and the object-header flag at `0xbc100` have the same polarity as in Halo 1. |
| Built | `object_pvs_activate`, `game_lost` | Alias to `pvs_set_object` and `game_revert`. The Halo 1 XBE describes the former as an older name for setting the PVS object and the latter as reverting to the previous save. |
| Built | `breakable_surfaces_enable`, `breakable_surfaces_reset`, `render_lights` | Slots 136 and 137 update the enable byte and clear the breakable-surface state array. Slot 143 at `0x2a2ea0` writes and returns the dynamic-light toggle. The light gather checks it at `0xc1859`. |
| Built | `player_effect_set_max_translation`, `player_effect_set_max_rotation`, `player_effect_set_max_rumble`, `player_effect_start`, `player_effect_stop` | Slots 651–655. The translation setter at `0x2aac10` writes three reals to the same player-effect state used by the other four retail Halo 2 calls. |
| Built | `players_unzoom_all`, `player_action_test_jump`, `player_action_test_zoom`, `player_action_test_action`, `player_action_test_look_relative_up`, `_down`, `_left`, `_right` | Slots 473, 480, 484, 488 and 491–494. The unzoom evaluator at `0x2a8e90` clears the four local zoom controls and their latches. The look-direction evaluators read the signed pitch and yaw latches set at `0x187bda`–`0x187c25`, corresponding to Halo 1's latches at `0xf8aed`–`0xf8b4f`. |
| Built | `player0_look_invert_pitch`, `player0_look_pitch_is_inverted`, `player_action_test_back` | Uses the Halo 2 controller inversion setter and getter (726 and 727) and the tutorial dismissal action (490). |
| Built | `unit_solo_player_integrated_night_vision_is_active` | In converted campaign the tutorial queries that player's flashlight, using slot 476. The Halo 1 and Halo 2 evaluators delegate to the local player flag tests at `0x3c030` and `0x14ed80` respectively. |
| Built | `cinematic_suppress_bsp_object_creation`, `cinematic_set_near_clip_distance` | Slots 562 and 710. The switch at `0x2a9910` sets the spawn-suppression flag checked at `0xd4e4a`. The clip evaluator at `0x2ab3b0` stores the real clip distance in the active rasterizer state. |
| Built | `game_safe_to_speak`, `game_all_quiet` | Slots 586 and 587. Player state and nearby enemy threat tests at `0x2a9bd0` and `0x225fe0` correspond to Halo 1's dialogue-safety and danger queries at `0x108880` and `0x108b50`. |
| Built | `inspect`, `cls` | Slot 23 at `0x208e10` is the console expression printer. The debug clear at slot 644 returns void immediately, matching its behavior in the retail Halo 1 console executable at `0xec730`. |
| Built | `player_add_equipment` | Slot 250 (`unit_add_equipment`). Every surveyed Halo 2 call appends `true` as its fourth argument. The equipment path reads that flag and indexes the scenario profile block at `0xcce00`. The converter resolves each profile name to its rebuilt block index and retains Halo 1's reset argument. |

Names supplied by the unit and object descriptor table use slots 23, 54,
66, 67, 72, 93, 136, 137, 143, 218, 234, 236, 238, 241, 473, 480, 484, 488,
491–494, 562, 586, 587, 644, 651 and 710. Slots 194, 195, 216 and 249
already have names in the shared retail descriptor table.
The AI-control lane also uses slot 241 for a marked vehicle-count call.
Retail grenade counts appear in a10 and the marked AI calls come from
a30 and b30. The builtin-name census skips the marked operation, leaving the
retail grenade count as the descriptor name when both share a cache.

Slot 312 also serves the native `ai_braindead_by_unit` call. The runtime reads
the function name from the current expression and dispatches only the authored
flashlight call to the light handler. Both names share the retail descriptor.

## Converted-campaign hooks

`h1scripts::unit_builtins::RUNTIME_SLOTS` is the table shared by the
converter and the boot feature. The converter checks the retail evaluator
and signature before naming a descriptor. The boot feature checks the
called index and the loaded campaign map.

| Status | Builtin | Slot | Behavior and evidence |
|---|---|---:|---|
| Built | `numeric_countdown_timer_stop`, `_restart` | 68, 169 | Toggle the running flag without changing the stored duration, as at Halo 1 `0xecbc0` and `0xecba0`. |
| Built | `game_speed` | 170 | Sets the simulation rate read by the native tick-budget routine at `0x1467b3`. Halo 1 uses the same clock multiplier. |
| Built | `unit_set_desired_flashlight_state`, `unit_get_current_flashlight_state`, `units_set_desired_flashlight_state` | 192, 214, 312 | Switch or query Halo 2's unit integrated-light flag, with the list form visiting each live unit datum. Halo 1's list evaluator is `0x3c740`. The AI update uses the power ramp at Halo 1 `0x45bc0` and `0x45c1f`; player lights use the native Halo 2 update. |
| Built | `player_action_test_look_relative_all_directions` | 495 | Tests all four signed look latches, matching Halo 1. The retail evaluator at `0x2a9340` tests for any of these bits. |
| Built | `player_action_test_move_relative_all_directions` | 496 | Tests all four movement latches. The retail evaluator at `0x2a9370` tests for any. No stock Halo 1 campaign calls this form. |
| Built | `player0_joystick_set_is_normal` | 722 | Reads the local thumbstick layout and checks the two layouts with the look stick in its normal position. Halo 2's input routing at `0x217f26` sends layout values 2 and 3 through the Legacy path. Halo 1's query is at `0xec5f0`. |
| Built | `game_skip_ticks` | 725 | Queues up to 15 ticks for the active campaign cinematic on the local world or synchronous host. The frame hook consumes them after the ordinary simulation batch, through the native tick loop at `0x137f70`, before per-frame updates at `0x137ed0`. Halo 1's queue and dispatch are at `0xef110` and `0xbd6f6`. Followers simulate the host's sequenced updates. See the co-op protocol, section 15.1. |
| Built | `numeric_countdown_timer_get` | 729 | Returns the requested timer digit as at Halo 1 `0x56aa0`. The runtime uses Halo 2's short-argument descriptor and the interpreter converts its long result to the script's short. |
| Built | `numeric_countdown_timer_set` | 843 | Sets the millisecond count and start flag as at Halo 1 `0xf1fa0`. The compiler inserts an unused leading long to match the descriptor. |
| Built | `game_time` | 888 | Returns the simulation tick from the game clock, corresponding to Halo 1's evaluator at `0xed5b0`. |

The countdown advances when the simulation tick changes, using the
whole-millisecond update at Halo 1 `0x56e40`. In d40 the digits are rendered
on the countdown display model. Its four shader permutations select
places 3 through 6 from Halo 1's numeric shader code at `0x67b1e`.
The converter builds four animated `shad` tags with the original bitmap.
The runtime uses the loaded map's StringIDs to supply their input values
at Halo 2's object-function lookup, `0xbab40`. The HUD escape timer has
its own state.

In a50, the marine rifle's flashlight attachment uses output C, scaled
by weapon source 14. Halo 1 copies the holder's integrated-light power
to the active weapon at `0x45741`. The rebuilt weapon's attachment
reads that value at Halo 2's object-function import boundary
(`0x108d90`). The AI flag and current power are stored in guest unit
state and travel with the simulation snapshot.

## Cinematic renderer and light caches

| Status | Builtin | Behavior and evidence |
|---|---|---|
| Built | `cinematic_screen_effect_set_convolution` | H1's blur code at `0x6c4c4`, `0x6b9ff` and `0x6bac0` selects the sample kernel and repeats it. The renderer samples the current viewport and alternates two render targets. a30's opening runs this pass. |
| Built | `cinematic_screen_effect_set_filter`, `_set_filter_desaturation_tint` | The postprocess applies brightness and tinted desaturation ramps, including the additive filter and the green default at `*0x232f7c`, with H1's filter math at `0x6cc64`–`0x6cde8`. Durations use simulation time so a restored checkpoint restores the ramp. |
| Built | `cinematic_screen_effect_set_video` | The writer rebuilds the mask and noise bitmaps from the H1 globals rasterizer block, read by the original effect at `0x61990`. The video pass combines them with the scene and the authored noise and overbright parameters. The noise offset uses H1's two random samples (`0x6bc52`, `0x11910`) indexed by simulation tick. |
| Built | `rasterizer_model_ambient_reflection_tint` | H1 at `0x70986`–`0x70a2f` overrides the model reflection ramp. Rebuilt environment-mapped model shaders retain their original facing and grazing colors and strengths so the draw hook can restore them when the override clears. |
| Built | `rasterizer_lights_reset_for_new_map` | Converted-campaign slot 687 (`0x2ab130`). H1 invalidates lighting work at `0xec680`. The hook clears the validity byte and previous frame on occupied H2 object render states used by `0x3dfe0`, preserving the owner and live light links. The H2 map reset at `0xc0040` would release registrations. |

The screen setters compile to typed scenario globals, ordinary retail HS
operations and the existing game clock (888). Start and stop still call the
retail screen-effect builtins (705 and 709). The renderer reads the state at
the Halo 2 screen-effect call inside the view render, before the retail pass.
Model reflection is independent of the fullscreen effect's lifetime.

`magic_seat_name`, `magic_melee_attack` and `object_set_emotion_animation`
have no stock calls across the ten campaign maps.

The disc tests check the descriptor signatures, campaign callsites, seat labels
on rebuilt vehicles and profile indices. The boot tests use synthetic guest
memory to check the script ABI, game clock, input flags and tick dispatch.
