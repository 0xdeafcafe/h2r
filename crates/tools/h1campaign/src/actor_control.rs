//! Rebuild the character parameters of script-attached Halo 1 actors.
//! These characters attach to an already placed unit through the AI VM.

use std::collections::{BTreeSet, HashMap};

use h1ai::{
    convert::character_for,
    h1::{self, ActorType},
};
use h1map::{Group, H1Map};
use h1scripts::{Catalog, Tables, compile::Hook};
use map::{
    FourCC, TagId,
    build::{Block, TagPayload, TagSet},
};
use tags::defs::{self, Defs, Record, Val};

use crate::{Result, h2assets::Aliases, invalid};

const CHAR: FourCC = FourCC(*b"char");

fn variant<'a>(
    map: &'a H1Map,
    name: &str,
) -> Result<(&'a h1map::Tag, h1::ActorVariant, ActorType)> {
    let err = |e: h1ai::Error| invalid(format!("attach-free actor variant {name}: {e}"));
    let tag = map
        .find(Group::new(b"actv"), name)
        .ok_or_else(|| invalid(format!("attach-free actor variant {name} is absent")))?;
    let variant = h1::actor_variant(map, tag).map_err(err)?;
    let actor = map.tag(variant.actor.id)?;
    let kind = h1::actor(map, actor).map_err(err)?.kind;
    Ok((actor, variant, kind))
}

fn parent(kind: ActorType, variant: &str) -> &'static str {
    // Crew and captain animation map onto the marine behavior family.
    if kind.0 == 8 {
        r"objects\characters\marine\ai\marine"
    } else {
        character_for(kind, variant).0
    }
}

pub(crate) fn wanted(map: &H1Map, catalog: &Catalog) -> Result<Vec<(FourCC, String)>> {
    let Some(actor_variant) = catalog.types().iter().position(|ty| ty == "actor_variant") else {
        return Ok(Vec::new());
    };
    let tables = Tables::read(map).map_err(|e| invalid(format!("scripts: {e}")))?;
    let mut names = BTreeSet::new();
    for name in tables.atom_tokens(actor_variant as u16) {
        if name != "none" {
            let (_, v, kind) = variant(map, &name)?;
            names.insert(parent(kind, &v.name));
        }
    }
    Ok(names
        .into_iter()
        .map(|name| (CHAR, name.to_owned()))
        .collect())
}

pub(crate) fn install(
    map: &H1Map,
    set: &mut TagSet,
    aliases: &Aliases,
    assets: crate::h2assets::Assets,
    hooks: &[Hook],
) -> Result<HashMap<String, TagId>> {
    let mut result = HashMap::new();
    for hook in hooks
        .iter()
        .filter(|h| h.h1_type == "actor_variant" && h.type_name == "long")
    {
        let name = &hook.token;
        if name == "none" || result.contains_key(name) {
            continue;
        }
        let (actor, v, kind) = variant(map, name)?;
        if let Some(id) = set.converted(CHAR, &v.name) {
            result.insert(name.clone(), id);
            continue;
        }
        let tag = map
            .find(Group::new(b"actv"), &v.name)
            .expect("parsed above");
        let weapon = if v.weapon.id.is_none() {
            None
        } else {
            let tag = map.tag(v.weapon.id)?;
            let group = FourCC(tag.group.0.to_be_bytes());
            Some(
                set.converted(group, &tag.name)
                    .or_else(|| aliases.get(group, &tag.name))
                    .ok_or_else(|| {
                        h1convert::Error::no_rebuild(
                            &map.header().name,
                            &tag.name,
                            "script-attached actor weapon",
                        )
                    })?,
            )
        };
        let parent_name = if assets == crate::h2assets::Assets::Halo2 {
            parent(kind, &v.name)
        } else {
            r"ai\generic"
        };
        let inherited = Some(set.find(CHAR, parent_name).ok_or_else(|| {
            h1convert::Error::no_rebuild(
                &map.header().name,
                &v.name,
                format!("missing parent {parent_name}"),
            )
        })?);
        let root = character(
            kind,
            inherited,
            weapon,
            map.tag_data(actor)?,
            map.tag_data(tag)?,
        )?;
        let id = set.add_tag(CHAR, &v.name, TagPayload::Built(root))?;
        set.record_conversion(CHAR, &v.name, id);
        result.insert(name.clone(), id);
    }
    Ok(result)
}

fn character(
    kind: ActorType,
    parent: Option<TagId>,
    weapon: Option<TagId>,
    actr: &[u8],
    actv: &[u8],
) -> Result<Block> {
    let def = Defs::builtin().group(CHAR).expect("char definition");
    let mut root = defs::default_record(def);
    let err = |e: defs::DefError| invalid(format!("attach-free character: {e}"));
    if let Some(id) = parent {
        root.set("parent", Val::tag(CHAR, id)).map_err(err)?;
    }
    let mut put = |name: &str, fill: &dyn Fn(&mut Record) -> Result<()>| -> Result<()> {
        let mut row = defs::default_record(def.child(name).expect("character property block"));
        fill(&mut row)?;
        root.set(name, Val::Block(vec![row])).map_err(err)
    };
    let read = |bytes: &[u8], offset: usize| -> Result<f32> {
        let raw: [u8; 4] = bytes
            .get(offset..offset + 4)
            .ok_or_else(|| invalid("truncated Halo 1 actor tuning"))?
            .try_into()
            .expect("four bytes");
        Ok(f32::from_le_bytes(raw))
    };
    put("general", &|row| {
        row.set("actor_type", Val::Int(i64::from(kind.0)))
            .map_err(err)
    })?;
    put("perception", &|row| {
        for (name, offset) in [
            ("sight", 0x18),
            ("central_angle", 0x1c),
            ("peripheral_angle", 0x20),
            ("peripheral_distance_angle", 0x28),
            ("peripheral_distance", 0x2c),
            ("hearing", 0x4c),
            ("notice_projectile", 0x50),
            ("notice_vehicle", 0x54),
        ] {
            row.set(name, Val::Float(read(actr, offset)?))
                .map_err(err)?;
        }
        Ok(())
    })?;
    put("vitality", &|row| {
        for (name, offset) in [
            ("body", 0x200),
            ("shield", 0x204),
            ("legendary_body", 0x200),
            ("legendary_shield", 0x204),
        ] {
            row.set(name, Val::Float(read(actv, offset)?))
                .map_err(err)?;
        }
        Ok(())
    })?;
    if let Some(weapon) = weapon {
        put("weapons", &|row| {
            row.set("weapon", Val::tag(FourCC(*b"weap"), weapon))
                .map_err(err)?;
            row.set("max_distance", Val::Float(read(actv, 0x74)?))
                .map_err(err)?;
            row.set(
                "combat_range",
                Val::floats(&[read(actv, 0x9c)?, read(actv, 0xa0)?]),
            )
            .map_err(err)
        })?;
    }
    defs::to_built(def, &root, &mut |_| None).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_free_character_carries_the_source_actor_parameters() {
        let mut actor = vec![0; h1::layout::ACTOR_SIZE];
        actor[0x4c..0x50].copy_from_slice(&13.0f32.to_le_bytes());
        let mut variant = vec![0; h1::layout::ACTOR_VARIANT_SIZE];
        variant[0x200..0x204].copy_from_slice(&64.0f32.to_le_bytes());
        variant[0x204..0x208].copy_from_slice(&12.0f32.to_le_bytes());
        variant[0x9c..0xa0].copy_from_slice(&9.0f32.to_le_bytes());
        variant[0xa0..0xa4].copy_from_slice(&27.0f32.to_le_bytes());
        let parent = TagId(0x8100_0001);
        let weapon = TagId(0x8100_0002);
        let block = character(ActorType(8), Some(parent), Some(weapon), &actor, &variant).unwrap();
        let def = Defs::builtin().group(CHAR).unwrap();
        let record = defs::from_built(def, &block).unwrap();
        assert_eq!(record.at("general[0].actor_type"), Some(&Val::Int(8)));
        assert_eq!(record.at("perception[0].hearing"), Some(&Val::Float(13.0)));
        assert_eq!(record.at("vitality[0].body"), Some(&Val::Float(64.0)));
        assert_eq!(record.at("vitality[0].shield"), Some(&Val::Float(12.0)));
        assert_eq!(
            record.at("weapons[0].combat_range"),
            Some(&Val::floats(&[9.0, 27.0]))
        );
    }
}
