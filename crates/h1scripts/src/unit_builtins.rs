//! Retail equivalents for Halo 1's unit, object and miscellaneous builtins.
//! Slot evidence is in docs/halo1/script-builtins.md.

use crate::identified::{Identified, id};

/// Additional retail descriptors identified by reading their evaluators.
pub(crate) const RETAIL_BUILTINS: &[Identified] = &[
    // Halo 1 22, between wake (22) and unit (24) as in Halo 1 (21, 23). E:
    // the evaluator 0x208e10 is the one hs_vm.md documents as inspect (the
    // console thread's value printer), and its descriptor's usage text is
    // "<expression>".
    id("inspect", 23, "void", &[]),
    // Halo 1 44, between object_destroy_containing (53) and
    // object_destroy_type_mask (55). E: its evaluator 0x2a1570 pushes -1
    // (every type) into the same destroy core (0x29fe10) the type-mask
    // builtin uses: "destroys all non player objects".
    id("object_destroy_all", 54, "void", &[]),
    // Halo 1 71, 72, between object_hide (57) and object_get_parent (73):
    // the two (object, boolean) setters of the object flag byte's lowest
    // bits, as Halo 1's pair set the two attack-inhibition flag bits (71's
    // evaluator 0xf1cc0 sets +0xb7 bit 0; 72's 0xf1c40, +0xb6 bit 7). Halo
    // 2's table keeps Halo 1's order, so 66 is ranged (its evaluator
    // 0x2a1920 sets +0x19 bit 1) and 67 melee (0x2a19a0, bit 0).
    id(
        "object_set_ranged_attack_inhibited",
        66,
        "void",
        &["object", "boolean"],
    ),
    id(
        "object_set_melee_attack_inhibited",
        67,
        "void",
        &["object", "boolean"],
    ),
    // Halo 1 74. E: the evaluator 0x2a1b50 sets the object's +0xc0 byte bit
    // 0x80 when false and clears it when true — Halo 1's collideable sets
    // its flag word's bit 0x1000000 the same way round (0xf1ba0). The
    // region is one Halo 2 reordered (set_scale sits past detach), so the
    // position gives the family and the polarity the member.
    id("object_set_collideable", 72, "void", &["object", "boolean"]),
    // Halo 1 81, between object_can_take_damage (91) and objects_predict
    // (99). E: the evaluator 0x2a2190 calls 0xbc100, which sets the object
    // header's flags word +4 bit 0x100000 from the boolean (clearing it
    // when false, then redrawing) — the same one-bit beautify the Halo 1
    // evaluator 0xf1a20 performs on the same word (its bit 0x400000).
    id("object_beautify", 93, "void", &["object", "boolean"]),
    // Halo 1 65, between real_random_range (125) and recording_play (138).
    // E: the evaluator 0x2a2c70 stores the boolean byte into one global
    // (0x4ed280) — byte for byte Halo 1's 0xf1f10 (into 0x27824c).
    id("breakable_surfaces_enable", 136, "void", &["boolean"]),
    // Halo 1 148, the pair of the one above. E: the evaluator 0x2a2cb0
    // fills the same global's breakable-surface array with -1 (rep stosl),
    // as Halo 1's reset (0xed670 via 0x88f90) restores every surface.
    id("breakable_surfaces_reset", 137, "void", &[]),
    // The same dynamic-light switch: H1 0x0f18b0 stores the boolean and
    // returns it, as H2 0x2a2ea0 does. H2 0x0c1859 checks the resulting
    // flag before collecting the object's dynamic lights.
    id("render_lights", 143, "boolean", &["boolean"]),
    // Halo 1 119, between vehicle_unload (233) and vehicle_driver (237). E:
    // the evaluator 0x2a41a0 none-checks and sets the animation mode ID.
    // Halo 1's 0xf11d0 looks up a pose in the six-name table at 0x1f7684.
    id("unit_set_seat", 234, "void", &["unit", "string_id"]),
    // Halo 1 121, in the same window. E: the evaluator 0x2a4230 builds the
    // rider list through one call (0x11b040), exactly Halo 1's shape
    // (0xf1190 via 0x3de10).
    id("vehicle_riders", 236, "object_list", &["unit"]),
    // Halo 1 123, right after vehicle_driver (237). E: the evaluator
    // 0x2a42c0 pushes seat type 3 into the occupant getter (0xbadc0), as
    // Halo 1's 0xf10f0 does (0x8b910).
    id("vehicle_gunner", 238, "unit", &["unit"]),
    // Halo 1 126, between unit_get_shield (240) and unit_has_weapon (242).
    id("unit_get_total_grenade_count", 241, "short", &["unit"]),
    // H1 0x0edab0 clears zoom across all four local controls. H2 0x2a8e90
    // does the same for the four zoom values and their associated latches.
    id("players_unzoom_all", 473, "void", &[]),
    // Halo 1 341, between player_action_test_reset (479) and
    // player_action_test_primary_trigger (481). E: the evaluator 0x2a9040
    // tests bit 1 of the action-test field (0x4ed284's word), the bit
    // Halo 1's jump reads (bit 1 of 0x276794's word, 0xed4f0).
    id("player_action_test_jump", 480, "boolean", &[]),
    // H1 0x0ed460 checks its zoom latch. H2 0x2a9100 reads the
    // same action from bit 9, latched by the zoom command at 0x187b8c.
    id("player_action_test_zoom", 484, "boolean", &[]),
    // Halo 1 345. E: the evaluator 0x2a91c0 latches the consumed bit and
    // tests the flag word's bit 0 through the object at 0x4ed284 — the
    // same shape as Halo 1's action test (0xed420 on 0x276794), unique in
    // the family (every other member is a plain bit test).
    id("player_action_test_action", 488, "boolean", &[]),
    // Both games latch the signed pitch and yaw deltas into the action
    // test flags. H1 0x0f8aed..0x0f8b4f, H2 0x187bda..0x187c25.
    id("player_action_test_look_relative_up", 491, "boolean", &[]),
    id("player_action_test_look_relative_down", 492, "boolean", &[]),
    id("player_action_test_look_relative_left", 493, "boolean", &[]),
    id("player_action_test_look_relative_right", 494, "boolean", &[]),
    // H1 0x0ef220 sets the cinematic spawn suppression bit. In H2
    // 0x0d4e4a checks the byte set by 0x2a9910 to skip BSP object creation.
    id("cinematic_suppress_bsp_object_creation", 562, "void", &["boolean"]),
    // H1 0x0ed200/0x108880 and H2 0x2a9bd0 both check nearby enemy
    // threats and player state before allowing incidental dialogue.
    id("game_safe_to_speak", 586, "boolean", &[]),
    // H1 0x0f2430/0x108b50 and H2 0x2a9c10/0x225fe0 both check
    // five danger conditions (enemies, projectiles, etc.).
    id("game_all_quiet", 587, "boolean", &[]),
    // Both retail console builds discard this debug operation. H1
    // 0x0ec730 returns void immediately, as H2 slot 644 does at 0x2ab130.
    id("cls", 644, "void", &[]),
    // Halo 1 371, between deactivate_team_nav_point_object (643) and
    // player_effect_set_max_rotation (652). E: the evaluator 0x2aac10
    // stores the three reals into the player-effect global (0x510c5c
    // +0x8..+0x10) — Halo 1's 0xeea20 into 0x2f9130 +0x3c4..+0x3cc — one
    // slot before set_max_rotation, as in Halo 1's table.
    id(
        "player_effect_set_max_translation",
        651,
        "void",
        &["real", "real", "real"],
    ),
    // Halo 1 406, right after cinematic_screen_effect_stop (709) and one
    // before cinematic_set_far_clip_distance (711). E: the evaluator
    // 0x2ab3b0 stores the real into the rasterizer global's +0x190, as
    // Halo 1's 0xee0b0 stores into its own.
    id("cinematic_set_near_clip_distance", 710, "void", &["real"]),
];

/// Halo 1 behavior supplied by the host in converted campaigns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeBuiltin {
    ResetLightCaches,
    GameTime,
    GameSpeed,
    GameSkipTicks,
    JoystickNormal,
    LookAllDirections,
    MoveAllDirections,
    CountdownSet,
    CountdownGet,
    CountdownStop,
    CountdownRestart,
    FlashlightSet,
    FlashlightSetList,
    FlashlightGet,
}

/// The exact retail descriptor reused for a converted-map builtin.
pub struct RuntimeSlot {
    pub builtin: RuntimeBuiltin,
    pub name: &'static str,
    pub index: u16,
    pub evaluate: u32,
    pub returns: &'static str,
    pub parameters: &'static [&'static str],
}

/// Shared by the converter and the boot feature. None of these slots are
/// named by the retail Halo 2 maps. Slots 495 and 496 keep their argument
/// and result types, but Halo 1 requires all the bits the retail body tests
/// for any. Slots 192, 214 and 312 supply the types for the flashlight
/// calls, and 729 and 843 for the countdown. The remaining slots have
/// discarded or constant-returning evaluators.
pub const RUNTIME_SLOTS: &[RuntimeSlot] = &[
    RuntimeSlot {
        builtin: RuntimeBuiltin::ResetLightCaches,
        name: "rasterizer_lights_reset_for_new_map",
        index: 687,
        evaluate: 0x002a_b130,
        returns: "void",
        parameters: &[],
    },
    RuntimeSlot {
        builtin: RuntimeBuiltin::CountdownStop,
        name: "numeric_countdown_timer_stop",
        index: 68,
        evaluate: 0x002a_b130,
        returns: "void",
        parameters: &[],
    },
    RuntimeSlot {
        builtin: RuntimeBuiltin::CountdownRestart,
        name: "numeric_countdown_timer_restart",
        index: 169,
        evaluate: 0x002a_b130,
        returns: "void",
        parameters: &[],
    },
    RuntimeSlot {
        builtin: RuntimeBuiltin::GameSpeed,
        name: "game_speed",
        index: 170,
        evaluate: 0x002a_b130,
        returns: "void",
        parameters: &["real"],
    },
    RuntimeSlot {
        builtin: RuntimeBuiltin::FlashlightSet,
        name: "unit_set_desired_flashlight_state",
        index: 192,
        evaluate: 0x002a_3200,
        returns: "void",
        parameters: &["unit", "boolean"],
    },
    RuntimeSlot {
        builtin: RuntimeBuiltin::FlashlightGet,
        name: "unit_get_current_flashlight_state",
        index: 214,
        evaluate: 0x002a_3a10,
        returns: "boolean",
        parameters: &["unit"],
    },
    RuntimeSlot {
        builtin: RuntimeBuiltin::FlashlightSetList,
        name: "units_set_desired_flashlight_state",
        index: 312,
        evaluate: 0x002a_55f0,
        returns: "void",
        parameters: &["object_list", "boolean"],
    },
    RuntimeSlot {
        builtin: RuntimeBuiltin::LookAllDirections,
        name: "player_action_test_look_relative_all_directions",
        index: 495,
        evaluate: 0x002a_9340,
        returns: "boolean",
        parameters: &[],
    },
    RuntimeSlot {
        builtin: RuntimeBuiltin::MoveAllDirections,
        name: "player_action_test_move_relative_all_directions",
        index: 496,
        evaluate: 0x002a_9370,
        returns: "boolean",
        parameters: &[],
    },
    RuntimeSlot {
        builtin: RuntimeBuiltin::JoystickNormal,
        name: "player0_joystick_set_is_normal",
        index: 722,
        evaluate: 0x002a_5d50,
        returns: "boolean",
        parameters: &[],
    },
    RuntimeSlot {
        builtin: RuntimeBuiltin::GameSkipTicks,
        name: "game_skip_ticks",
        index: 725,
        evaluate: 0x002a_b130,
        returns: "void",
        parameters: &["short"],
    },
    RuntimeSlot {
        builtin: RuntimeBuiltin::CountdownGet,
        name: "numeric_countdown_timer_get",
        index: 729,
        evaluate: 0x002a_b760,
        returns: "long",
        parameters: &["short"],
    },
    RuntimeSlot {
        builtin: RuntimeBuiltin::CountdownSet,
        name: "numeric_countdown_timer_set",
        index: 843,
        evaluate: 0x002a_b130,
        returns: "void",
        parameters: &["long", "long", "boolean"],
    },
    RuntimeSlot {
        builtin: RuntimeBuiltin::GameTime,
        name: "game_time",
        index: 888,
        evaluate: 0x002a_c600,
        returns: "long",
        parameters: &[],
    },
];

pub(crate) fn name_runtime_slots(
    builtins: &mut std::collections::HashMap<String, crate::translate::H2Builtin>,
    catalog: &hs::Catalog,
) {
    for slot in RUNTIME_SLOTS {
        let Ok(f) = catalog.function(slot.index) else {
            continue;
        };
        if f.evaluate() == slot.evaluate && f.flags() & 2 == 0 {
            crate::identified::name_one(
                builtins,
                catalog,
                &id(slot.name, slot.index, slot.returns, slot.parameters),
            );
        }
    }
}
