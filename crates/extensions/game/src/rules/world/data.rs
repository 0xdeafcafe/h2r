//! Advanced data on the host (docs/game/rules-world-api.md, "Advanced
//! data"): an object's vitality, `on damage` and the field resolver of the
//! tag reads.
//!
//! - [`vitality`] and [`to_fixed`] answer `object_health` and
//!   `object_shield` from the object datum.
//! - [`damage_inputs`] turns the tick's `SimEvent::Damage` events into the
//!   VM's `Input::Damage`, at most [`MAX_DAMAGE`] a tick, in the order the
//!   game applied them. The runtime has no sim-event drain of its own, so
//!   what drains the tick's events calls it and fires the inputs.
//! - [`DefsSchema`] resolves a script's `tag_int` and `tag_fixed` field
//!   paths against our typed tag definitions (`tags::defs`) at load, for
//!   the start gate (`::rules::fields::check`, called by
//!   `Runtime::start`). `super::map_tags` finds the tag by its path and
//!   reads the field from the tag's data in guest memory.

use ::rules::Input;
use ::rules::fields::{Field, Kind, Schema, valid_path};
use ::rules::ty::fixed;
use guest_layout::view::objects;
use guest_layout::{DatumHandle, GuestMemory};
use sim_state::SimEvent;
use tags::defs::{self, StructDef};

/// The most `on damage` events a tick reports; the rest of the tick's
/// damage is dropped, as a handler's cap is
/// (`::rules::Event::worst_per_tick`).
pub const MAX_DAMAGE: usize = 16;

/// A fraction of full vitality as the `fixed` a script reads: 0 to 1
/// (an overshield reads 1), rounded once, 0 for a NaN.
pub fn to_fixed(v: f32) -> i32 {
    if v.is_nan() {
        return 0;
    }
    (f64::from(v.clamp(0.0, 1.0)) * f64::from(fixed::ONE)).round() as i32
}

/// The body and shield of the live object `object` (its salted datum) as
/// the script reads them; `None` for an object that is gone or has no
/// body.
pub fn vitality(m: &dyn GuestMemory, object: u32) -> Option<(i32, i32)> {
    let (_, body) = objects::get(m, DatumHandle(object)).ok().flatten()?;
    let health = body.health(m).ok()?;
    let shield = body.shield(m).ok()?;
    Some((to_fixed(health), to_fixed(shield)))
}

/// The tick's `Input::Damage` events: one for each `SimEvent::Damage`, the
/// first [`MAX_DAMAGE`], in event order. `by` is the player slot the
/// damage is credited to, `none` for none, and `amount` the body and
/// shield damage together as a fraction of full vitality.
pub fn damage_inputs(events: &[SimEvent]) -> Vec<Input> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::Damage(d) => Some(Input::Damage {
                object: d.victim.raw(),
                by: d.owner_player.get().map(|p| (p & 0xffff) as u8),
                amount: to_fixed(d.body + d.shield),
            }),
            _ => None,
        })
        .take(MAX_DAMAGE)
        .collect()
}

/// Our typed tag definitions as the [`Schema`] of a script's tag reads.
/// `group` gives the definition of the group of the map's tag at a path,
/// or `None` for a tag the map lacks or a group with no definition.
pub struct DefsSchema<'a, F: Fn(&str) -> Option<&'a StructDef>> {
    pub group: F,
}

impl<'a, F: Fn(&str) -> Option<&'a StructDef>> Schema for DefsSchema<'a, F> {
    fn field(&self, tag: &str, path: &str) -> Result<Field, String> {
        valid_path(path)?;
        let root = (self.group)(tag).ok_or_else(|| "the map has no such tag, or its group has no definition".to_string())?;
        resolve(root, path)
    }
}

/// The field `path` of a group's root: names joined by dots, each but the
/// last an embedded struct, the last a single integer or float. A block, a
/// data reference, an enum, a flag set or an array of values is not a
/// field a script reads.
pub fn resolve(root: &StructDef, path: &str) -> Result<Field, String> {
    let mut at = root;
    let mut base = 0u32;
    let names: Vec<&str> = path.split('.').collect();
    for (i, name) in names.iter().enumerate() {
        let f = at
            .field(name)
            .ok_or_else(|| format!("\"{}\" has no field \"{name}\"", at.name))?;
        base = base.saturating_add(f.offset);
        let last = i + 1 == names.len();
        match (&f.kind, last) {
            (defs::Kind::Struct(s), false) => at = s,
            (defs::Kind::Int { bytes, signed, count: 1 }, true) => {
                return Ok(Field {
                    offset: base,
                    kind: Kind::Int { bytes: *bytes, signed: *signed },
                });
            }
            (defs::Kind::Float { count: 1 }, true) => {
                return Ok(Field { offset: base, kind: Kind::Float });
            }
            (_, false) => return Err(format!("\"{name}\" is not a struct of fields")),
            (_, true) => return Err(format!("\"{name}\" is not a single integer or float")),
        }
    }
    Err("an empty field path".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::Datum;
    use sim_state::Damage;
    use tags::defs::Defs;

    const TEXT: &str = r#"struct vitals 0x8
  0x0 f32 maximum_shield
  0x4 i16 maximum_health
  0x6 u8 grade
  0x7 pad 0x1
group boss 0x24
  0x0 struct vitals vitals
  0x8 f32[3] offset
  0x14 block parts 0xc align 4
    0x0 string name
    0x4 datum owner
    0x8 unknown 0x4 seen=zero
  0x1c enum16 shape values=box|sphere
  0x1e unknown 0x2
  0x20 i32 count
"#;

    fn defs() -> Defs {
        let mut d = Defs::default();
        d.parse_into("test", TEXT).unwrap();
        d
    }

    fn damage(victim: u32, player: Datum, body: f32, shield: f32) -> SimEvent {
        SimEvent::Damage(Damage {
            tick: Some(1),
            victim: Datum(victim),
            victim_kind: 0,
            victim_player: Datum::NONE,
            owner_player: player,
            owner_object: Datum::NONE,
            owner_team: 0,
            code: 0,
            effect: Datum::NONE,
            flags: 0,
            body,
            shield,
            direction: [0.0; 3],
            origin: [0.0; 3],
            position: None,
        })
    }

    #[test]
    fn vitality_is_a_fixed_fraction_between_none_and_all() {
        assert_eq!(to_fixed(0.0), 0);
        assert_eq!(to_fixed(0.5), fixed::ONE / 2);
        assert_eq!(to_fixed(1.0), fixed::ONE);
        // An overshield reads as full; a bad value as empty.
        assert_eq!(to_fixed(3.0), fixed::ONE);
        assert_eq!(to_fixed(-1.0), 0);
        assert_eq!(to_fixed(f32::NAN), 0);
    }

    #[test]
    fn damage_events_become_inputs_in_order_up_to_the_cap() {
        let mut events = vec![SimEvent::ActorKilled {
            unit: Datum(9),
            character: Datum(9),
            killer: Datum::NONE,
            weapon: 0,
        }];
        events.push(damage(0x1234, Datum(2), 0.25, 0.25));
        events.push(damage(0x1235, Datum::NONE, 0.0, 2.0));
        assert_eq!(
            damage_inputs(&events),
            [
                Input::Damage { object: 0x1234, by: Some(2), amount: fixed::ONE / 2 },
                Input::Damage { object: 0x1235, by: None, amount: fixed::ONE },
            ]
        );
        let many: Vec<SimEvent> = (0..40).map(|i| damage(i, Datum::NONE, 0.1, 0.0)).collect();
        let inputs = damage_inputs(&many);
        assert_eq!(inputs.len(), MAX_DAMAGE);
        assert_eq!(inputs[0], Input::Damage { object: 0, by: None, amount: to_fixed(0.1) });
        assert_eq!(inputs[15], Input::Damage { object: 15, by: None, amount: to_fixed(0.1) });
    }

    #[test]
    fn a_path_resolves_through_embedded_structs_to_one_number() {
        let d = defs();
        let root = &d.groups[&map::FourCC(*b"boss")];
        assert_eq!(
            resolve(root, "vitals.maximum_shield"),
            Ok(Field { offset: 0, kind: Kind::Float })
        );
        assert_eq!(
            resolve(root, "vitals.maximum_health"),
            Ok(Field { offset: 4, kind: Kind::Int { bytes: 2, signed: true } })
        );
        assert_eq!(
            resolve(root, "vitals.grade"),
            Ok(Field { offset: 6, kind: Kind::Int { bytes: 1, signed: false } })
        );
        assert_eq!(
            resolve(root, "count"),
            Ok(Field { offset: 0x20, kind: Kind::Int { bytes: 4, signed: true } })
        );
    }

    #[test]
    fn a_field_the_definitions_lack_is_named() {
        let d = defs();
        let root = &d.groups[&map::FourCC(*b"boss")];
        let why = |p| resolve(root, p).unwrap_err();
        assert!(why("vitals.maximum_mana").contains("maximum_mana"));
        assert!(why("nothing").contains("nothing"));
        assert!(why("offset").contains("single integer or float"));
        assert!(why("parts").contains("single integer or float"));
        assert!(why("shape").contains("single integer or float"));
        assert!(why("count.more").contains("not a struct"));
        let schema = DefsSchema { group: |t: &str| (t == "objects/boss").then_some(root) };
        assert!(schema.field("objects/other", "count").is_err());
        assert!(schema.field("objects/boss", "Count").is_err());
        assert!(schema.field("objects/boss", "count").is_ok());
    }
}
