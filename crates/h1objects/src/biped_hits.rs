//! The Halo 1 player's marker targets and collision-material damage sections.

use tags::defs::{self, Record, StructDef, Val};

use crate::{Error, Result};

fn set(record: &mut Record, field: &str, value: Val) -> Result<()> {
    record
        .set(field, value)
        .map_err(|e| Error::Invalid(e.to_string()))
}

/// The Halo 2 search and shot bending use named model targets. Use the
/// cyborg's authored body and head markers with the native player target
/// profile (Coagulation's Chief). The radius is for aiming, not collision.
pub(crate) fn write_player_targets(
    def: &StructDef,
    model: &mut Record,
    markers: &[String],
    name: &str,
) -> Result<()> {
    let target_def = def.child("targets").expect("hlmt targets");
    let mut targets = Vec::new();
    for (marker, radius, cone_angle, flags) in [
        ("body", 0.065, 0.0, 0x2),
        ("head", 0.04, std::f32::consts::PI, 0x6),
    ] {
        if !markers.iter().any(|m| m == marker) {
            return Err(Error::Invalid(format!(
                "{name}: missing aim target marker {marker}"
            )));
        }
        let mut target = defs::default_record(target_def);
        set(&mut target, "marker", Val::text(marker))?;
        set(&mut target, "radius", Val::Float(radius))?;
        set(&mut target, "cone_angle", Val::Float(cone_angle))?;
        set(
            &mut target,
            "unknown_c",
            Val::Bytes(u32::MAX.to_le_bytes().to_vec()),
        )?;
        set(&mut target, "weight", Val::Float(1.0))?;
        set(&mut target, "flags", Val::Int(flags))?;
        set(&mut target, "lock_on_distance", Val::Float(15.0))?;
        targets.push(target);
    }
    set(model, "targets", Val::Block(targets))
}

/// Halo 1 checks collision material +0x20 bit 0 for a headshot
/// (0x00095089). Halo 2 looks up material +6 and checks that section's
/// +4 bit 7 (0x000d8d50). Multiple surfaces and nodes can share a material.
pub(crate) fn write_player_damage_sections(
    def: &StructDef,
    model: &mut Record,
    damage: &mut Record,
    materials: &[&[u8]],
) -> Result<()> {
    for (i, material) in materials.iter().enumerate() {
        set(
            model,
            &format!("block_58[{i}].damage_section"),
            Val::Int(i64::from(material[0x20] & 1)),
        )?;
    }
    let section_def = def
        .child("block_60.block_bc")
        .expect("hlmt damage sections");
    let mut sections = Vec::new();
    for (name, flags) in [("body", 0u32), ("head", 0x80)] {
        let mut section = defs::default_record(section_def);
        set(&mut section, "string_0", Val::text(name))?;
        set(
            &mut section,
            "unknown_4",
            Val::Bytes(flags.to_le_bytes().to_vec()),
        )?;
        set(
            &mut section,
            "unknown_34",
            Val::Bytes(0xffffu32.to_le_bytes().to_vec()),
        )?;
        sections.push(section);
    }
    set(damage, "block_bc", Val::Block(sections))
}

#[cfg(test)]
mod tests {
    use super::*;
    use map::FourCC;

    #[test]
    fn player_headshots_follow_source_material_flags() {
        let def = defs::Defs::builtin().group(FourCC(*b"hlmt")).unwrap();
        let mut model = defs::default_record(def);
        let mut damage = defs::default_record(def.child("block_60").unwrap());
        let material = defs::default_record(def.child("block_58").unwrap());
        model
            .set("block_58", Val::Block(vec![material; 3]))
            .unwrap();
        let body = [0; 0x48];
        let mut weak_point = body;
        weak_point[0x20] = 1;
        let mut other_flag = body;
        other_flag[0x20] = 2;
        write_player_damage_sections(
            def,
            &mut model,
            &mut damage,
            &[&body, &weak_point, &other_flag],
        )
        .unwrap();
        assert_eq!(model.at("block_58[0].damage_section"), Some(&Val::Int(0)));
        assert_eq!(model.at("block_58[1].damage_section"), Some(&Val::Int(1)));
        assert_eq!(model.at("block_58[2].damage_section"), Some(&Val::Int(0)));
        assert_eq!(
            damage.at("block_bc[1].unknown_4"),
            Some(&Val::Bytes(vec![0x80, 0, 0, 0]))
        );
    }

    #[test]
    fn player_aim_targets_require_authored_markers() {
        let def = defs::Defs::builtin().group(FourCC(*b"hlmt")).unwrap();
        let mut model = defs::default_record(def);
        let markers = ["body".to_owned(), "head".to_owned()];
        assert!(write_player_targets(def, &mut model, &markers[..1], "unit").is_err());
        write_player_targets(def, &mut model, &markers, "unit").unwrap();
        assert_eq!(model.at("targets[0].marker"), Some(&Val::text("body")));
        assert_eq!(model.at("targets[1].marker"), Some(&Val::text("head")));
    }
}
