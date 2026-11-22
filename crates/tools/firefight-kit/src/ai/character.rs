//! The stage-one Elite's tuning. Distances use world units and angles use radians.
use map::{FourCC, TagId, build::Block};
use tags::defs::{self, DefError, Defs, Val};

pub(super) fn elite(unit: TagId, weapon: TagId) -> Result<Block, DefError> {
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
    defs::to_built(def, &root, &mut |_| None)
}
