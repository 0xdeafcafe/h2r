//! The retired Halo 2 script slots a converted mission HUD is driven
//! through. `h1scripts` compiles Halo 1's HUD calls onto these slots;
//! `boot::h1_hud_text::controls` installs their runtime handling. The
//! evaluators and signatures are verified in docs/halo1/text.md.

/// Retired Halo 2 slots with the original Boolean signatures. Runtime
/// handling is installed only for maps with a converted mission HUD.
pub const CONTROL_BUILTINS: &[(&str, u16, u32, bool)] = &[
    ("hud_show_health", 657, 1, true),
    ("hud_blink_health", 658, 2, false),
    ("hud_show_shield", 659, 4, true),
    ("hud_blink_shield", 660, 8, false),
    ("hud_show_motion_sensor", 661, 16, true),
    ("hud_blink_motion_sensor", 662, 32, false),
    ("hud_show_crosshair", 663, 64, true),
];

/// Retired slot for the scenario Unicode help list.
pub const SCENARIO_HELP_BUILTIN: (&str, u16, u32, &str, &[&str]) =
    ("display_scenario_help", 903, DISCARDED_EVALUATOR, "void", &["short"]);

/// The discarded evaluator shared by these slots on the retail XBE.
pub const DISCARDED_EVALUATOR: u32 = 0x002a_b130;
