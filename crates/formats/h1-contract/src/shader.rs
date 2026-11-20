//! What converted Halo 1 shaders name for the game to answer:
//! `h1convert::shad` writes them; `boot` (`h1_script_builtins::countdown`,
//! `h1_cinematics`) looks them up in guest memory.

/// Names of the object functions that provide the Halo 1 engine timer's
/// individual digits. The boot feature resolves these names in the loaded
/// cache's StringID table before answering the shader's function lookups.
pub const COUNTDOWN_DIGITS: [&str; 9] = [
    "h1_countdown_digit_0",
    "h1_countdown_digit_1",
    "h1_countdown_digit_2",
    "h1_countdown_digit_3",
    "h1_countdown_digit_4",
    "h1_countdown_digit_5",
    "h1_countdown_digit_6",
    "h1_countdown_digit_7",
    "h1_countdown_digit_8",
];

/// Final unbound colour of a converted model reflection parameter block.
pub const MODEL_REFLECTION_SIGNATURE: u32 = u32::from_le_bytes(*b"H1rf");

/// Facing/grazing RGB followed by facing/grazing alpha, before the saved copy.
pub fn model_reflection_start(original_colors: usize) -> Option<usize> {
    match original_colors {
        7 => Some(1),
        8 => Some(4),
        _ => None,
    }
}
