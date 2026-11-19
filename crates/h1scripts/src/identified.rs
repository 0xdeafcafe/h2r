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
//! `+0x114` (the scenario's recorded animations).
//!
//! Every slot this table claims is listed in docs/halo1/scripts.md beside
//! each entry, for cross-lane clash checks.

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
    // 0x2a19a0 sets +0x19 bit 1) and 67 melee (0x2a1920, bit 0).
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
    // After object_teleport (105), as in Halo 1 (34, 35).
    id(
        "object_set_facing",
        106,
        "void",
        &["object", "cutscene_flag"],
    ),
    // Halo 1 65, between real_random_range (125) and recording_play (138).
    // E: the evaluator 0x2a2c70 stores the boolean byte into one global
    // (0x4ed280) — byte for byte Halo 1's 0xf1f10 (into 0x27824c).
    id("breakable_surfaces_enable", 136, "void", &["boolean"]),
    // Halo 1 148, the pair of the one above. E: the evaluator 0x2a2cb0
    // fills the same global's breakable-surface array with -1 (rep stosl),
    // as Halo 1's reset (0xed670 via 0x88f90) restores every surface.
    id("breakable_surfaces_reset", 137, "void", &[]),
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
    // Halo 1 105 has no slot in Halo 2's order (nothing between
    // unit_is_playing_custom_animation (207) and unit_set_enterable_by_player
    // (213) has its signature), but between unit_enter_vehicle (216) and
    // unit_set_emotional_state (220) sits the only unnamed (unit, short)
    // -> void, and its evaluator 0x2a3b70 drives the emotion core 0x10e9f0
    // with the short and a default transition time — the by-index emotion
    // setter beside emotional_state's by-name one (0x10e9a0), as Halo 1
    // has both (105 set_emotion, 109 set_emotional_state).
    id("unit_set_emotion", 218, "void", &["unit", "short"]),
    // Halo 1 119, between vehicle_unload (233) and vehicle_driver (237). E:
    // the evaluator 0x2a41a0 none-checks and stores the seat label string
    // ID into the unit object, as Halo 1's 0xf11d0 resolves the label
    // against the unit's own seats.
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
    // Halo 1 341, between player_action_test_reset (479) and
    // player_action_test_primary_trigger (481). E: the evaluator 0x2a9040
    // tests bit 1 of the action-test field (0x4ed284's word), the bit
    // Halo 1's jump reads (bit 1 of 0x276794's word, 0xed4f0).
    id("player_action_test_jump", 480, "boolean", &[]),
    // Halo 1 345. E: the evaluator 0x2a91c0 latches the consumed bit and
    // tests the flag word's bit 0 through the object at 0x4ed284 — the
    // same shape as Halo 1's action test (0xed420 on 0x276794), unique in
    // the family (every other member is a plain bit test).
    id("player_action_test_action", 488, "boolean", &[]),
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
        assert_eq!(IDENTIFIED.len(), 42);
    }
}
