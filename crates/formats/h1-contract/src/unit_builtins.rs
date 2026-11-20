//! The retail Halo 2 script slots the host answers in converted campaigns.
//! `h1scripts` names them for the compiler; `boot::h1_script_builtins`
//! installs their handling. Slot evidence is in
//! docs/halo1/script-builtins.md.

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
