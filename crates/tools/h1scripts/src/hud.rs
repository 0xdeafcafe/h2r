//! The legacy HUD builtins retained in the retail Halo 2 executable.
//! Evaluators and signatures are verified in docs/halo1/text.md.

use std::collections::HashMap;

use crate::translate::H2Builtin;

/// Name, slot, retail evaluator address, return type and parameter types.
pub const BUILTINS: &[(&str, u16, u32, &str, &[&str])] = &[
    ("show_hud", 625, 0x002a_a5e0, "boolean", &["boolean"]),
    (
        "show_hud_help_text",
        626,
        0x002a_a640,
        "boolean",
        &["boolean"],
    ),
    (
        "enable_hud_help_flash",
        628,
        0x002a_a700,
        "void",
        &["boolean"],
    ),
    ("hud_help_flash_restart", 629, 0x002a_a770, "void", &[]),
    ("hud_clear_messages", 664, 0x002a_ae20, "void", &[]),
    (
        "hud_set_help_text",
        665,
        0x002a_ae40,
        "void",
        &["hud_message"],
    ),
    (
        "hud_set_objective_text",
        666,
        0x002a_ae90,
        "void",
        &["hud_message"],
    ),
    (
        "hud_set_timer_time",
        667,
        0x002a_aee0,
        "void",
        &["short", "short"],
    ),
    (
        "hud_set_timer_warning_time",
        668,
        0x002a_af30,
        "void",
        &["short", "short"],
    ),
    (
        "hud_set_timer_position",
        669,
        0x002a_af90,
        "void",
        &["short", "short", "hud_corner"],
    ),
    ("show_hud_timer", 670, 0x002a_b010, "void", &["boolean"]),
    ("pause_hud_timer", 671, 0x002a_b060, "void", &["boolean"]),
    ("hud_get_timer_ticks", 672, 0x002a_b0a0, "short", &[]),
];

/// The slots `boot` installs, kept in `h1-contract`.
pub use h1_contract::hud_builtins::{
    CONTROL_BUILTINS, DISCARDED_EVALUATOR, SCENARIO_HELP_BUILTIN,
};

const BOOL_PARAM: &[&str] = &["boolean"];

pub(crate) fn identify(builtins: &mut HashMap<String, H2Builtin>, catalog: &hs::Catalog) {
    let native = BUILTINS.iter().copied();
    let controls = CONTROL_BUILTINS
        .iter()
        .map(|&(name, index, _, _)| (name, index, DISCARDED_EVALUATOR, "void", BOOL_PARAM));
    for (name, index, evaluate, returns, parameters) in
        native.chain(controls).chain([SCENARIO_HELP_BUILTIN])
    {
        let Ok(builtin) = catalog.function(index) else {
            continue;
        };
        if builtin.evaluate() == evaluate
            && builtin.name().is_none_or(|existing| existing == name)
            && builtin.flags() & 2 == 0
            && catalog.type_name(builtin.return_type()).ok() == Some(returns)
            && builtin.parameters().len() == parameters.len()
            && builtin
                .parameters()
                .iter()
                .zip(parameters)
                .all(|(&ty, want)| catalog.type_name(ty).ok() == Some(*want))
        {
            builtins.insert(
                name.into(),
                H2Builtin {
                    index,
                    return_type: builtin.return_type(),
                    parameters: builtin.parameters().to_vec(),
                    special: false,
                },
            );
        }
    }
}

/// The shared Halo 1 and Halo 2 HUD anchor enumeration.
pub fn corner(token: &str) -> Option<u32> {
    let corners = [
        "top_right",
        "top_left",
        "bottom_left",
        "bottom_right",
        "center",
    ];
    corners
        .iter()
        .position(|name| name.eq_ignore_ascii_case(token))
        .map(|i| i as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hud_builtin_slots_are_distinct() {
        let mut slots = std::collections::BTreeSet::new();
        for slot in BUILTINS
            .iter()
            .map(|entry| entry.1)
            .chain(CONTROL_BUILTINS.iter().map(|entry| entry.1))
            .chain([SCENARIO_HELP_BUILTIN.1])
        {
            assert!(slots.insert(slot));
        }
        assert!(!slots.contains(&300));
        assert_eq!(corner("Bottom_Right"), Some(3));
        assert_eq!(corner("outside"), None);
    }
}
