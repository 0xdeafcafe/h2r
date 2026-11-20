//! Characters whose actor variants need their own unit and equipment. Build their
//! unit trees in the placement converter so a cinematic and a squad can share
//! the same model, collision, animation resources and sounds.

use std::collections::HashMap;

use crate::ai::convert::rebuilds_character;
use crate::ai::h1::{H1Ai, actor, actor_variant};
use h1map::{Group, H1Map};
use h1objects::convert::{Converter, local_index};
use map::build::{Block, TagPayload, TagSet};
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
/// `retail` is the set of the Halo 2-assets variant (`crate::h2assets`): a
/// variant's weapon is then its retail counterpart, as every placed weapon
/// is, and a counterpart missing from the set is an error. Otherwise, or
/// when the mapping keeps the weapon Halo 1's, it is Halo 1's own,
/// converted.
pub(crate) fn prepare(
    h1: &H1Map,
    ai: &H1Ai,
    conv: &mut Converter<'_>,
    assets: crate::h2assets::Assets,
    retail: Option<&TagSet>,
    scripts: Option<&crate::scripts::Tables>,
) -> Result<Vec<Variant>> {
    let mut todo = Vec::new();
    for (i, entry) in ai.actor_palette.iter().enumerate() {
        if entry.is_some()
            && (assets == crate::h2assets::Assets::Halo1
                || ai.actor_of(i).is_some_and(|a| rebuilds_character(a.kind)))
        {
            let v = entry.as_ref().expect("actor of variant");
            let t = h1
                .find(Group::new(b"actv"), &v.name)
                .ok_or_else(|| invalid(format!("AI actor variant {}", v.name)))?;
            todo.push(t.id);
        }
    }
    if assets == crate::h2assets::Assets::Halo1
        && let Some(scripts) = scripts
    {
        todo.extend(
            scripts
                .references
                .iter()
                .filter(|r| r.group == Group::new(b"actv") && !r.id.is_none())
                .map(|r| r.id),
        );
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let err = |e: crate::ai::Error| invalid(format!("AI character: {e}"));
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
        let biped = h1.tag(variant.unit.id).map_err(|e| {
            h1convert::Error::no_rebuild(&h1.header().name, &tag.name, format!("actor unit: {e}"))
        })?;
        let field = h1map::Block::read(variant_bytes, 0x22c).expect("validated actv size");
        let colours: Vec<_> = h1
            .block(field, 0x20)?
            .chunks_exact(0x20)
            .map(|rgb| h1objects::h1::ChangeColour {
                function: None,
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
            let source = h1.tag(variant.weapon.id)?;
            let stand_in = match retail {
                Some(set) => crate::h2assets::stand_in(set, source).map_err(|why| {
                    invalid(format!("AI weapon {} of {}: {why}", source.name, tag.name))
                })?,
                None => None,
            };
            match stand_in {
                Some(id) => Some(id),
                None => Some(
                    conv.object(source)
                        .map_err(|e| invalid(format!("AI weapon {}: {e}", tag.name)))?,
                ),
            }
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
        let parent = r"ai\generic";
        let parent = set
            .find(FourCC(*b"char"), parent)
            .ok_or_else(|| invalid(format!("char {} needs parent {parent}", v.name)))?;
        let real = |id| ids[local_index(id).expect("converted object")];
        // A retail weapon is a set tag already, not one of the tree's.
        let weapon = v.weapon.map(|id| local_index(id).map_or(id, |k| ids[k]));
        let major = v.major.as_ref().map(|n| by_name[n]);
        let style = if v.kind == 13 {
            Some(set.find(FourCC(*b"styl"), SENTINEL_STYLE).ok_or_else(|| {
                h1convert::Error::no_rebuild(
                    &set.name,
                    &v.name,
                    "missing native Sentinel behaviour style",
                )
            })?)
        } else {
            None
        };
        let root = character(v, parent, real(v.unit), weapon, major, style)?;
        set.tag_mut(by_name[&v.name]).expect("reserved").payload = TagPayload::Built(root);
        set.record_conversion(FourCC(*b"char"), &v.name, by_name[&v.name]);
        kit.0.insert(v.name.clone(), weapon);
    }
    Ok(kit)
}

impl Loadout {
    pub(crate) fn weapon(&self, variant: &str) -> Option<Option<TagId>> {
        self.0.get(variant).copied()
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
