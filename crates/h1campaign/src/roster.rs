//! Characters whose actor variants need their own unit and equipment. Build their
//! unit trees in the placement converter so a cinematic and a squad can share
//! the same model, collision, animation resources and sounds.

use std::collections::HashMap;

use h1ai::convert::{Plan, rebuilds_character};
use h1ai::h1::{H1Ai, actor, actor_variant};
use h1map::{Group, H1Map};
use h1objects::convert::{Converter, local_index};
use map::build::{Block, FixupKind, TagPayload, TagSet};
use map::{FourCC, TagId};
use tags::defs::{self, Defs, Val};

use crate::{Result, invalid};

pub(crate) const SENTINEL_STYLE: &str =
    r"objects\characters\sentinel_aggressor\ai\sentinel_aggressor";

pub(crate) struct Variant {
    name: String,
    kind: u16,
    unit: TagId,
    weapon: Option<TagId>,
    major: Option<String>,
    vitality: [f32; 2],
    perception: [f32; 8],
    max_distance: f32,
    combat_range: [f32; 2],
}

/// Include major variants even when the scenario never names them directly.
pub(crate) fn prepare(h1: &H1Map, ai: &H1Ai, conv: &mut Converter<'_>) -> Result<Vec<Variant>> {
    let mut todo = Vec::new();
    for (i, entry) in ai.actor_palette.iter().enumerate() {
        if ai.actor_of(i).is_some_and(|a| rebuilds_character(a.kind)) {
            let v = entry.as_ref().expect("actor of variant");
            let t = h1
                .find(Group::new(b"actv"), &v.name)
                .ok_or_else(|| invalid(format!("AI actor variant {}", v.name)))?;
            todo.push(t.id);
        }
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let err = |e: h1ai::Error| invalid(format!("AI character: {e}"));
    while let Some(id) = todo.pop() {
        if !seen.insert(id) {
            continue;
        }
        let tag = h1.tag(id)?;
        let variant = actor_variant(h1, tag).map_err(err)?;
        let act = h1.tag(variant.actor.id)?;
        let kind = actor(h1, act).map_err(err)?.kind.0;
        let variant_bytes = h1.tag_data(tag)?;
        let actor_bytes = h1.tag_data(act)?;
        let float = |b: &[u8], off: usize| f32::from_le_bytes(b[off..off + 4].try_into().unwrap());
        let biped = h1.tag(variant.unit.id)?;
        let field = h1map::Block::read(variant_bytes, 0x22c).expect("validated actv size");
        let colours: Vec<_> = h1
            .block(field, 0x20)?
            .chunks_exact(0x20)
            .map(|rgb| h1objects::h1::ChangeColour {
                permutations: vec![h1objects::h1::ColourPermutation {
                    weight: 1.0,
                    lower: [0, 4, 8].map(|o| float(rgb, o)),
                    upper: [12, 16, 20].map(|o| float(rgb, o)),
                }],
            })
            .collect();
        let unit = conv
            .actor_biped(biped, &tag.name, &colours)
            .map_err(|e| invalid(format!("AI unit {}: {e}", tag.name)))?;
        let weapon = if variant.weapon.id.is_none() {
            None
        } else {
            Some(
                conv.object(h1.tag(variant.weapon.id)?)
                    .map_err(|e| invalid(format!("AI weapon {}: {e}", tag.name)))?,
            )
        };
        let major = if variant.major_variant.id.is_none() {
            None
        } else {
            todo.push(variant.major_variant.id);
            Some(h1.tag(variant.major_variant.id)?.name.clone())
        };
        out.push(Variant {
            name: tag.name.clone(),
            kind,
            unit,
            weapon,
            major,
            vitality: [0x200, 0x204].map(|o| float(variant_bytes, o)),
            perception: [0x18, 0x1c, 0x20, 0x28, 0x2c, 0x4c, 0x50, 0x54]
                .map(|o| float(actor_bytes, o)),
            max_distance: float(variant_bytes, 0x74),
            combat_range: [0x9c, 0xa0].map(|o| float(variant_bytes, o)),
        });
    }
    Ok(out)
}

fn character(
    v: &Variant,
    parent: TagId,
    unit: TagId,
    weapon: Option<TagId>,
    major: Option<TagId>,
    style: Option<TagId>,
) -> Result<Block> {
    let def = Defs::builtin()
        .group(FourCC(*b"char"))
        .expect("char definition");
    let mut root = defs::default_record(def);
    let err = |e: defs::DefError| invalid(format!("char {}: {e}", v.name));
    root.set("parent", Val::tag(FourCC(*b"char"), parent))
        .map_err(err)?;
    root.set("unit", Val::tag(FourCC(*b"bipd"), unit))
        .map_err(err)?;
    if let Some(id) = style {
        root.set("style", Val::tag(FourCC(*b"styl"), id))
            .map_err(err)?;
    }
    if let Some(id) = major {
        root.set("major", Val::tag(FourCC(*b"char"), id))
            .map_err(err)?;
    }
    let mut one = |name: &str, fields: &[(&str, Val)]| -> Result<()> {
        let mut entry = defs::default_record(def.child(name).expect("character block"));
        for (key, value) in fields {
            entry.set(key, value.clone()).map_err(err)?;
        }
        root.set(name, Val::Block(vec![entry])).map_err(err)
    };
    one("general", &[("actor_type", Val::Int(i64::from(v.kind)))])?;
    let [body, shield] = v.vitality;
    one(
        "vitality",
        &[
            ("body", Val::Float(body)),
            ("shield", Val::Float(shield)),
            ("legendary_body", Val::Float(body)),
            ("legendary_shield", Val::Float(shield)),
        ],
    )?;
    let fields = [
        "sight",
        "central_angle",
        "peripheral_angle",
        "peripheral_distance_angle",
        "peripheral_distance",
        "hearing",
        "notice_projectile",
        "notice_vehicle",
    ];
    one(
        "perception",
        &std::array::from_fn::<_, 8, _>(|i| (fields[i], Val::Float(v.perception[i]))),
    )?;
    if let Some(weapon) = weapon {
        one(
            "weapons",
            &[
                ("weapon", Val::tag(FourCC(*b"weap"), weapon)),
                ("max_distance", Val::Float(v.max_distance)),
                ("combat_range", Val::floats(&v.combat_range)),
            ],
        )?;
    }
    defs::to_built(def, &root, &mut |_| None).map_err(err)
}

/// Weapon choice per converted actor variant. The scenario stores a palette
/// index on a start, while the character's weapon properties store a tag ref.
#[derive(Default)]
pub(crate) struct Loadout(HashMap<String, Option<TagId>>);

pub(crate) fn install(set: &mut TagSet, ids: &[TagId], variants: Vec<Variant>) -> Result<Loadout> {
    let mut by_name = HashMap::new();
    for v in &variants {
        by_name.insert(
            v.name.clone(),
            set.add_tag(FourCC(*b"char"), &v.name, TagPayload::Empty)?,
        );
    }
    let mut kit = Loadout::default();
    for v in &variants {
        let parent = if v.kind == 8 {
            r"objects\characters\marine\ai\marine"
        } else {
            r"ai\generic"
        };
        let parent = set
            .find(FourCC(*b"char"), parent)
            .ok_or_else(|| invalid(format!("char {} needs parent {parent}", v.name)))?;
        let real = |id| ids[local_index(id).expect("converted object")];
        let weapon = v.weapon.map(real);
        let major = v.major.as_ref().map(|n| by_name[n]);
        let style = (v.kind == 13)
            .then(|| set.find(FourCC(*b"styl"), SENTINEL_STYLE))
            .flatten();
        let root = character(v, parent, real(v.unit), weapon, major, style)?;
        set.tag_mut(by_name[&v.name]).expect("reserved").payload = TagPayload::Built(root);
        kit.0.insert(v.name.clone(), weapon);
    }
    Ok(kit)
}

impl Loadout {
    pub(crate) fn equip(&self, scnr: &mut Block, plan: &mut Plan) -> Result<()> {
        if self.0.is_empty() {
            return Ok(());
        }
        let (_, pal, _) = tags::object::OBJECT_TYPES[2].placement.expect("weapon");
        let pal = u32::from(pal);
        if !scnr.fixups.iter().any(|f| f.offset == pal) {
            scnr.block(pal, 0, Block::zeroed(0));
        }
        let f = scnr.fixups.iter_mut().find(|f| f.offset == pal).unwrap();
        let FixupKind::Block { count, child } = &mut f.kind else {
            return Err(invalid("weapon palette block"));
        };
        let mut weapon_indices = Vec::new();
        for c in &plan.characters {
            let Some(&Some(weapon)) = self.0.get(&c.h2_character) else {
                weapon_indices.push(None);
                continue;
            };
            let entry = child
                .fixups
                .iter()
                .find_map(|f| match &f.kind {
                    FixupKind::TagRef(id) if *id == weapon => Some(f.offset / 0x28),
                    _ => None,
                })
                .unwrap_or_else(|| {
                    let entry = *count;
                    child.bytes.resize(child.bytes.len() + 0x28, 0);
                    child.tag_ref(entry * 0x28, weapon);
                    *count += 1;
                    entry
                });
            weapon_indices.push(Some(
                i16::try_from(entry).map_err(|_| invalid("AI weapon palette overflow"))?,
            ));
        }
        // Write the effective weapon on each location. An unarmed crewman
        // override must not inherit an armed squad member's pistol.
        for q in &mut plan.ai.squads {
            for l in &mut q.starting_locations {
                let character = if l.character < 0 {
                    q.character
                } else {
                    l.character
                };
                if let Some(Some(index)) = weapon_indices.get(character as usize) {
                    l.initial_weapon = *index;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_writes_unit_major_vitality_and_weapon_properties() {
        let id = |i: u32| TagId(0xe1000000 + i);
        let v = Variant {
            name: "test".into(),
            kind: 13,
            unit: id(1),
            weapon: Some(id(2)),
            major: Some("major".into()),
            vitality: [123.0, 27.0],
            perception: [1., 2., 3., 4., 5., 6., 7., 8.],
            max_distance: 50.,
            combat_range: [7., 20.],
        };
        let root = character(&v, id(0), id(1), v.weapon, Some(id(3)), None).unwrap();
        let d = Defs::builtin().group(FourCC(*b"char")).unwrap();
        let rec = defs::from_built(d, &root).unwrap();
        assert_eq!(
            rec.at("unit"),
            Some(&Val::Ref {
                group: 0,
                target: defs::Target::Id(id(1))
            })
        );
        assert_eq!(
            rec.at("major"),
            Some(&Val::Ref {
                group: 0,
                target: defs::Target::Id(id(3))
            })
        );
        assert_eq!(rec.at("general[0].actor_type"), Some(&Val::Int(13)));
        assert_eq!(rec.at("vitality[0].body"), Some(&Val::Float(123.)));
        assert_eq!(rec.at("perception[0].hearing"), Some(&Val::Float(6.)));
        assert_eq!(
            rec.at("weapons[0].weapon"),
            Some(&Val::Ref {
                group: 0,
                target: defs::Target::Id(id(2))
            })
        );
        assert_eq!(
            rec.at("weapons[0].combat_range"),
            Some(&Val::floats(&[7., 20.]))
        );
    }
}
