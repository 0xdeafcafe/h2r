//! Halo 1 surface substances and their Halo 2 globals material indices.

use map::{FourCC, Map};
use std::io::{Read, Seek};
use tags::object::Metadata;

/// Halo 1's material types in the order of its responses, each with the
/// Halo 2 global material its response is written for: the Halo 2
/// material of the same substance and, where Halo 2 has one, the same
/// character. Every name is one of the base's global materials (V:
/// `weapon_rebuild_disc` on every base).
pub const MATERIALS: [(&str, &str); 33] = [
    ("dirt", "tough_terrain_dirt"),
    ("sand", "tough_terrain_sand"),
    ("stone", "hard_terrain_stone"),
    ("snow", "soft_terrain_snow"),
    ("wood", "tough_organic_wood"),
    ("metal (hollow)", "hard_metal_thin"),
    ("metal (thin)", "hard_metal_thin"),
    ("metal (thick)", "hard_metal_thick"),
    ("rubber", "tough_inorganic_rubber"),
    ("glass", "brittle_glass"),
    ("force field", "energy_shield_thick_for"),
    ("grunt", "soft_organic_flesh_grunt"),
    ("hunter armor", "hard_metal_thick_cov_hunter"),
    ("hunter skin", "soft_organic_flesh_hunter"),
    ("elite", "soft_organic_flesh_elite"),
    ("jackal", "soft_organic_flesh_jackal"),
    ("jackal energy shield", "energy_shield_thick_cov_jackal"),
    ("engineer skin", "soft_organic_flesh"),
    ("engineer force field", "energy_shield_thin"),
    ("flood combat form", "tough_floodflesh_combatform"),
    ("flood carrier form", "tough_floodflesh_carrierform"),
    ("cyborg armor", "hard_metal_thin_hum_masterchief"),
    ("cyborg energy shield", "energy_shield_thin_hum_masterchief"),
    ("human armor", "tough_inorganic_armor_hum"),
    ("human skin", "soft_organic_flesh_human"),
    ("sentinel", "hard_metal_thin_for_sentinel"),
    ("monitor", "hard_metal_thin_for_monitor"),
    ("plastic", "tough_inorganic_plastic"),
    ("water", "liquid_thin_water"),
    ("leaves", "soft_organic_plant_leafy"),
    ("elite energy shield", "energy_shield_thin_cov_elite"),
    ("ice", "hard_terrain_ice"),
    ("hunter shield", "hard_metal_solid_cov_hunter"),
];

/// `matg +0x150`: the global materials block; elements of `0xb4`, the
/// name a string ID at `+0`. V: on the disc's maps every projectile
/// material response's material index (`proj +0x19c` element `+0x10`) is
/// the position of its name here (`tests/weapon_rebuild_disc.rs`).
pub const GLOBAL_MATERIALS: usize = 0x150;
pub const GLOBAL_MATERIAL_SIZE: usize = 0xb4;

/// The global material names of `map`'s globals (none without globals).
/// Globals whose material block or a name does not read are an error,
/// not empty names.
pub fn global_materials<R: Read + Seek>(
    map: &mut Map<R>,
    meta: &Metadata,
) -> std::result::Result<Vec<String>, String> {
    let Some(t) = map.tags().iter().find(|t| t.group == FourCC(*b"matg")) else {
        return Ok(Vec::new());
    };
    let at = t
        .location
        .map(|l| l.address)
        .ok_or_else(|| "base globals: no metadata".to_owned())?;
    let word = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().expect("4"));
    let head = meta
        .slice(at, (GLOBAL_MATERIALS + 8) as u64)
        .map_err(|e| format!("base globals: materials block: {e}"))?;
    let (n, first) = (
        word(head, GLOBAL_MATERIALS),
        word(head, GLOBAL_MATERIALS + 4),
    );
    let strings = map.strings();
    (0..n)
        .map(|i| {
            let b = meta
                .slice(first + i * GLOBAL_MATERIAL_SIZE as u32, 4)
                .map_err(|e| format!("base globals: material {i}: {e}"))?;
            strings
                .get((word(b, 0) & 0xffff) as usize)
                .cloned()
                .flatten()
                .ok_or_else(|| format!("base globals: material {i}: name does not resolve"))
        })
        .collect()
}
