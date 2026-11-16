//! Halo 1 sound classes to Halo 2's.
//!
//! Both games number their classes in one list that Halo 2 extended, so
//! most classes keep their number. The names are from public tag
//! definitions (S); the disc census in `docs/formats/h1-sounds.md` checks
//! them against the tag paths that use each value. Classes with no Halo 2
//! equivalent go to the nearest one, listed in [`NEAREST`].

use crate::{Error, Result};

/// Halo 1 class names by value (`None`: unused).
pub const H1_NAMES: [Option<&str>; 51] = {
    let mut n = [None; 51];
    n[0] = Some("projectile_impact");
    n[1] = Some("projectile_detonation");
    n[4] = Some("weapon_fire");
    n[5] = Some("weapon_ready");
    n[6] = Some("weapon_reload");
    n[7] = Some("weapon_empty");
    n[8] = Some("weapon_charge");
    n[9] = Some("weapon_overheat");
    n[10] = Some("weapon_idle");
    n[13] = Some("object_impacts");
    n[14] = Some("particle_impacts");
    n[15] = Some("slow_particle_impacts");
    n[18] = Some("unit_footsteps");
    n[19] = Some("unit_dialog");
    n[22] = Some("vehicle_collision");
    n[23] = Some("vehicle_engine");
    n[26] = Some("device_door");
    n[27] = Some("device_force_field");
    n[28] = Some("device_machinery");
    n[29] = Some("device_nature");
    n[30] = Some("device_computers");
    n[32] = Some("music");
    n[33] = Some("ambient_nature");
    n[34] = Some("ambient_machinery");
    n[35] = Some("ambient_computers");
    n[39] = Some("first_person_damage");
    n[44] = Some("scripted_dialog_player");
    n[45] = Some("scripted_effect");
    n[46] = Some("scripted_dialog_other");
    n[47] = Some("scripted_dialog_force_unspatialized");
    n[50] = Some("game_event");
    n
};

/// Halo 2 class names by value (`None`: unused).
pub const H2_NAMES: [Option<&str>; 54] = {
    let mut n = [None; 54];
    n[0] = Some("projectile_impact");
    n[1] = Some("projectile_detonation");
    n[2] = Some("projectile_flyby");
    n[4] = Some("weapon_fire");
    n[5] = Some("weapon_ready");
    n[6] = Some("weapon_reload");
    n[7] = Some("weapon_empty");
    n[8] = Some("weapon_charge");
    n[9] = Some("weapon_overheat");
    n[10] = Some("weapon_idle");
    n[11] = Some("weapon_melee");
    n[12] = Some("weapon_animation");
    n[13] = Some("object_impacts");
    n[14] = Some("particle_impacts");
    n[18] = Some("unit_footsteps");
    n[19] = Some("unit_dialog");
    n[20] = Some("unit_animation");
    n[22] = Some("vehicle_collision");
    n[23] = Some("vehicle_engine");
    n[24] = Some("vehicle_animation");
    n[26] = Some("device_door");
    n[28] = Some("device_machinery");
    n[29] = Some("device_stationary");
    n[32] = Some("music");
    n[33] = Some("ambient_nature");
    n[34] = Some("ambient_machinery");
    n[36] = Some("huge_ass");
    n[37] = Some("object_looping");
    n[38] = Some("cinematic_music");
    n[45] = Some("cortana_mission");
    n[46] = Some("cortana_cinematic");
    n[47] = Some("mission_dialog");
    n[48] = Some("cinematic_dialog");
    n[49] = Some("scripted_cinematic_foley");
    n[50] = Some("game_event");
    n[51] = Some("ui");
    n[52] = Some("test");
    n[53] = Some("multilingual_test");
    n
};

/// Halo 1 classes whose name Halo 2 doesn't have: `(halo 1, halo 2)`.
pub const NEAREST: [(u16, u8); 10] = [
    (15, 14), // slow_particle_impacts -> particle_impacts
    (27, 28), // device_force_field -> device_machinery
    (29, 29), // device_nature -> device_stationary
    (30, 28), // device_computers -> device_machinery
    (35, 34), // ambient_computers -> ambient_machinery
    (39, 50), // first_person_damage -> game_event
    (44, 47), // scripted_dialog_player -> mission_dialog
    (45, 49), // scripted_effect -> scripted_cinematic_foley
    (46, 47), // scripted_dialog_other -> mission_dialog
    (47, 48), // scripted_dialog_force_unspatialized -> cinematic_dialog
];

/// The Halo 2 class for a Halo 1 class.
pub fn to_h2(h1: u16) -> Result<u8> {
    if let Some(&(_, h2)) = NEAREST.iter().find(|(c, _)| *c == h1) {
        return Ok(h2);
    }
    let name = H1_NAMES
        .get(usize::from(h1))
        .copied()
        .flatten()
        .ok_or_else(|| Error::Unsupported(format!("halo 1 sound class {h1}")))?;
    match H2_NAMES.get(usize::from(h1)).copied().flatten() {
        Some(n) if n == name => Ok(h1 as u8),
        _ => Err(Error::Unsupported(format!(
            "halo 1 sound class {name} ({h1})"
        ))),
    }
}
