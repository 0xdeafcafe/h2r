//! Halo 2 builtins no retail Halo 2 script calls, identified by position
//! and signature (docs/halo1/scripts.md, "Builtins named by position").
//!
//! Halo 2's descriptors carry no names, so `crates/hs` names only builtins
//! some retail script calls. Halo 2's table keeps Halo 1's order with
//! entries inserted between, so a Halo 1 builtin can be found between the
//! named Halo 2 builtins around it: each entry below is the only unnamed
//! Halo 2 builtin, between those neighbours, whose parameter and return
//! types (by name) are Halo 1's (or castable from them, where noted). E:
//! for the recordings, the evaluators of 138, 139 and 140 call one function
//! (`0x001fb360`) with play flags 0, 8 and 0x10, which bounds the
//! recording index by `scnr +0x110` and reads `0x34`-byte elements at
//! `+0x114` (the scenario's recorded animations). Kill searches by unit at
//! `0x1fb760`; time queries `0x1fb4c0`. Hover calls the same function
//! (`0xf0fd0`) as completion of a play-and-hover recording.

use crate::translate::H2Builtin;
use std::collections::HashMap;

/// One identification: the Halo 1 name, the Halo 2 index, and the Halo 2
/// signature by type name (return type, parameters) it must have.
pub struct Identified {
    pub name: &'static str,
    pub index: u16,
    pub returns: &'static str,
    pub parameters: &'static [&'static str],
}

const fn id(
    name: &'static str,
    index: u16,
    returns: &'static str,
    parameters: &'static [&'static str],
) -> Identified {
    Identified {
        name,
        index,
        returns,
        parameters,
    }
}

/// The identifications. Neighbours are named Halo 2 builtins
/// (`crates/hs`), Halo 1's order is its XBE table's.
pub const IDENTIFIED: &[Identified] = &[
    // After object_teleport (105), as in Halo 1 (34, 35).
    id(
        "object_set_facing",
        106,
        "void",
        &["object", "cutscene_flag"],
    ),
    // Halo 1 66..70, between physics_disable_character_ground_follow_forces
    // (129) and texture_camera_set_object_marker (144); E above.
    id(
        "recording_play",
        138,
        "boolean",
        &["unit", "cutscene_recording"],
    ),
    id(
        "recording_play_and_delete",
        139,
        "boolean",
        &["unit", "cutscene_recording"],
    ),
    id(
        "recording_play_and_hover",
        140,
        "boolean",
        &["vehicle", "cutscene_recording"],
    ),
    id("recording_kill", 141, "void", &["unit"]),
    id("recording_time", 142, "short", &["unit"]),
    // Halo 1 90, 91; before scenery_animation_start_loop (185) and after
    // scenery_animation_start_relative (186) and its loop (187).
    id(
        "scenery_animation_start",
        184,
        "void",
        &["scenery", "animation_graph", "string_id"],
    ),
    id(
        "scenery_animation_start_at_frame",
        188,
        "void",
        &["scenery", "animation_graph", "string_id", "short"],
    ),
    // Halo 1 94, 95 (unit_can_blink, unit_open, unit_close, unit_kill);
    // before unit_kill (196).
    id("unit_open", 194, "void", &["unit"]),
    id("unit_close", 195, "void", &["unit"]),
    // Halo 1 100..103; after custom_animation_relative_loop (204).
    id(
        "custom_animation_list",
        205,
        "boolean",
        &["object_list", "animation_graph", "string_id", "boolean"],
    ),
    id(
        "unit_custom_animation_at_frame",
        206,
        "boolean",
        &["unit", "animation_graph", "string_id", "boolean", "short"],
    ),
    id(
        "unit_is_playing_custom_animation",
        207,
        "boolean",
        &["unit"],
    ),
    // Halo 1 107, after unit_set_enterable_by_player (213), before
    // vehicle_test_seat_list (223) as in Halo 1 (108).
    id(
        "unit_enter_vehicle",
        216,
        "void",
        &["unit", "vehicle", "string_id"],
    ),
    // 0x002a3b70 passes the short as the facial-expression selector to
    // 0x0010e9f0, also called by unit_set_emotional_state (220).
    id("unit_set_emotion", 218, "void", &["unit", "short"]),
    // After unit_has_weapon (242), as in Halo 1's table (a30's sniper
    // tutorial).
    id(
        "unit_has_weapon_readied",
        243,
        "boolean",
        &["unit", "object_definition"],
    ),
    // Halo 1 131, right after unit_impervious (248) as in Halo 1 (130).
    id("unit_suspended", 249, "void", &["unit", "boolean"]),
    // Between device_set_power (254) and device_set_position (256); E: its
    // evaluator (0x002a4780) returns the device's float at +0x134 (d40's
    // control rooms wait on it).
    id("device_get_power", 255, "real", &["device"]),
    id(
        "cs_stack_command_script",
        367,
        "void",
        &["ai", "ai_command_script"],
    ),
    id(
        "cs_command_script_running",
        370,
        "boolean",
        &["ai", "ai_command_script"],
    ),
    id(
        "cs_command_script_queued",
        371,
        "boolean",
        &["ai", "ai_command_script"],
    ),
    id("cs_look_object", 394, "void", &["boolean", "object"]),
    id("cs_face_object", 400, "void", &["boolean", "object"]),
    id(
        "cs_approach",
        429,
        "void",
        &["object", "real", "real", "real"],
    ),
    // Halo 1 247..250 after camera_set (450); camera_set_dead takes an
    // object in Halo 2 (a unit casts to it).
    id(
        "camera_set_relative",
        451,
        "void",
        &["cutscene_camera_point", "short", "object"],
    ),
    id(
        "camera_set_animation",
        452,
        "void",
        &["animation_graph", "string_id"],
    ),
    id("camera_set_first_person", 456, "void", &["unit"]),
    id("camera_set_dead", 457, "void", &["object"]),
    // Halo 1 86, between pvs_set_object (470) and pvs_clear (472) as in
    // Halo 1 (85, 87).
    id(
        "object_pvs_set_camera",
        471,
        "void",
        &["cutscene_camera_point"],
    ),
    // Halo 1 300, right after cinematic_set_title (560) as in Halo 1 (299).
    id(
        "cinematic_set_title_delayed",
        561,
        "void",
        &["cutscene_title", "real"],
    ),
    // Halo 1 336, after sound_class_set_gain (615) as in Halo 1 (335): the
    // only (vehicle, boolean) builtin there.
    id("vehicle_hover", 621, "void", &["vehicle", "boolean"]),
    // Halo 1 356, 357 (show_hud, show_hud_help_text, both boolean ->
    // boolean): the first of the three such builtins after
    // hud_cinematic_fade (624). The first is the one Halo 1's order gives.
    id("show_hud", 625, "boolean", &["boolean"]),
];

/// Identifications not used yet; a held builtin stays missing and compiles
/// as a stub. Empty: the recordings (138 to 142) were held at converter
/// version 27, when Halo 1's event streams copied as they are faulted
/// Halo 2's player (a30's intro: a call through the handler table past its
/// end, from `0x001fb61e`), and play from version 28 with the streams
/// rewritten by `h1campaign::recordings`.
pub const HELD: &[&str] = &[];

/// Adds each identification whose Halo 2 builtin is unnamed and has the
/// signature it expects to `builtins`, except those [`HELD`].
pub fn name_unnamed(builtins: &mut HashMap<String, H2Builtin>, c: &hs::Catalog) {
    let ty = |t: u16| c.type_name(t).ok();
    for e in IDENTIFIED {
        let Ok(f) = c.function(e.index) else {
            continue;
        };
        let signature = ty(f.return_type()) == Some(e.returns)
            && f.parameters().len() == e.parameters.len()
            && f.parameters()
                .iter()
                .zip(e.parameters)
                .all(|(&p, want)| ty(p) == Some(*want));
        if f.name().is_some()
            || !signature
            || builtins.contains_key(e.name)
            || HELD.contains(&e.name)
        {
            continue;
        }
        builtins.insert(
            e.name.to_owned(),
            H2Builtin {
                index: e.index,
                return_type: f.return_type(),
                parameters: f.parameters().to_vec(),
                special: f.flags() & 2 != 0,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::IDENTIFIED;

    /// No two identifications share a Halo 2 index or a Halo 1 name, and
    /// they are in index order.
    #[test]
    fn identifications_do_not_collide() {
        for (i, a) in IDENTIFIED.iter().enumerate() {
            for b in &IDENTIFIED[i + 1..] {
                assert_ne!(a.index, b.index, "{} and {}", a.name, b.name);
                assert_ne!(a.name, b.name);
            }
        }
        assert!(IDENTIFIED.windows(2).all(|w| w[0].index < w[1].index));
    }
}
