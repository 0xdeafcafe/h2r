//! The stage-one Elite's tuning. Distances use world units and angles use radians.
use map::{FourCC, TagId, build::Block};
use tags::defs::{self, DefError, Defs, Val};

/// The character property blocks the engine looks up for every actor,
/// each through a walk up the parent chain that returns the first row or
/// none (`0x1e4a50`: block count at the property's offset, then the
/// `parent` reference). A caller reads the row without checking, so an
/// empty block faults: the movement row at `+0x5c` was the first, read by
/// `0x1f9119` at an actor's first update (`[0+4]`). The authored Elite has
/// no parent, so it carries one row of each, zeroed where the kit sets
/// nothing. Not here: `variants` (names), `boss` and `boarding` (a row
/// would opt the class in), and the rows set below.
pub(super) const PROPERTY_ROWS: &[&str] = &[
    "general",
    "vitality",
    "placement",
    "perception",
    "look",
    "movement",
    "swarm",
    "ready",
    "engage",
    "charge",
    "evasion",
    "cover",
    "retreat",
    "search",
    "pre_search",
    "idle",
    "vocalize",
    "weapons",
];

pub(super) fn elite(unit: TagId, weapon: TagId) -> Result<Block, DefError> {
    let def = Defs::builtin()
        .group(FourCC(*b"char"))
        .expect("char definition");
    defs::to_built(def, &elite_record(unit, weapon)?, &mut |_| None)
}

/// The Elite before it is built: every [`PROPERTY_ROWS`] block has a row.
pub(super) fn elite_record(unit: TagId, weapon: TagId) -> Result<defs::Record, DefError> {
    let def = Defs::builtin()
        .group(FourCC(*b"char"))
        .expect("char definition");
    let mut root = defs::default_record(def);
    root.set("unit", Val::tag(FourCC(*b"bipd"), unit))?;
    let mut property = |name: &str, fields: &[(&str, Val)]| -> Result<(), DefError> {
        let mut row = defs::default_record(def.child(name).expect("character property"));
        for (key, value) in fields {
            row.set(key, value.clone())?;
        }
        root.set(name, Val::Block(vec![row]))
    };
    property("general", &[("actor_type", Val::Int(0))])?;
    property(
        "vitality",
        &[
            ("body", Val::Float(72.0)),
            ("shield", Val::Float(84.0)),
            ("legendary_body", Val::Float(90.0)),
            ("legendary_shield", Val::Float(110.0)),
        ],
    )?;
    property(
        "perception",
        &[
            ("sight", Val::Float(30.0)),
            ("central_angle", Val::Float(50_f32.to_radians())),
            ("peripheral_angle", Val::Float(95_f32.to_radians())),
            ("peripheral_distance_angle", Val::Float(65_f32.to_radians())),
            ("peripheral_distance", Val::Float(13.0)),
            ("hearing", Val::Float(20.0)),
            ("notice_projectile", Val::Float(1.0)),
            ("notice_vehicle", Val::Float(1.0)),
        ],
    )?;
    property(
        "weapons",
        &[
            ("weapon", Val::tag(FourCC(*b"weap"), weapon)),
            ("max_distance", Val::Float(26.0)),
            ("combat_range", Val::floats(&[4.0, 14.0])),
        ],
    )?;
    let pattern_set = def.child("patterns").expect("weapon patterns");
    let mut burst = defs::default_record(pattern_set.child("patterns").expect("firing pattern"));
    burst.set("burst_duration", Val::floats(&[0.35, 0.65]))?;
    property(
        "patterns",
        &[
            ("weapon", Val::tag(FourCC(*b"weap"), weapon)),
            ("patterns", Val::Block(vec![burst])),
        ],
    )?;
    for name in PROPERTY_ROWS {
        let empty = !matches!(root.get(name), Some(Val::Block(rows)) if !rows.is_empty());
        if empty {
            let row = defs::default_record(def.child(name).expect("character property"));
            root.set(name, Val::Block(vec![row]))?;
        }
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every property block the engine reads without a check has a row:
    /// an empty movement block faulted at an Elite's first update
    /// (`0x1f9119` reads `[0+4]`). Rows the kit tunes keep their values;
    /// boss and boarding stay empty.
    #[test]
    fn the_elite_has_a_row_of_every_property_the_engine_reads() {
        let root = elite_record(TagId(0xe1a0_0001), TagId(0xe1a0_0002)).unwrap();
        for name in PROPERTY_ROWS {
            assert!(
                matches!(root.get(name), Some(Val::Block(rows)) if rows.len() == 1),
                "{name}"
            );
        }
        for name in ["boss", "boarding", "variants"] {
            assert!(
                !matches!(root.get(name), Some(Val::Block(rows)) if !rows.is_empty()),
                "{name}"
            );
        }
        let Some(Val::Block(vitality)) = root.get("vitality") else {
            panic!("vitality")
        };
        assert_eq!(vitality[0].get("body"), Some(&Val::Float(72.0)));
        let def = Defs::builtin().group(FourCC(*b"char")).unwrap();
        assert!(defs::to_built(def, &root, &mut |_| None).is_ok());
    }
}
