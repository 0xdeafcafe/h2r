//! Resolve native character tags and preserve per-variant Halo 1 uncover tuning.

use h1ai::{
    control::{Control, UncoverRule},
    convert::{Plan, rebuilds_character},
};
use h1map::H1Map;
use map::{
    FourCC, TagId,
    build::{TagPayload, TagSet},
};

use crate::{Result, invalid};

pub(crate) fn resolve(
    h1: &H1Map,
    set: &mut TagSet,
    plan: &Plan,
    ai_control: &mut Control,
    require_rebuilt: bool,
    scripted: bool,
) -> Result<(Vec<TagId>, Vec<(String, String)>)> {
    let mut chars = Vec::new();
    let resolve = |p: &str| -> Option<TagId> {
        let find = |p: &str| {
            set.slots.iter().find_map(|s| {
                let t = s.tag.as_ref()?;
                (t.group == FourCC(*b"char") && s.name.as_deref() == Some(p)).then_some(t.id)
            })
        };
        if plan
            .characters
            .iter()
            .any(|c| c.h2_character == p && rebuilds_character(c.h1_type))
        {
            return find(p);
        }
        find(p).or_else(|| {
            // Stand-ins for characters the base lacks: the marine for
            // marine kinds, else the folder's first.
            let folder = p.rsplit_once('\\')?.0;
            if p.contains("\\marine\\") {
                find("objects\\characters\\marine\\ai\\marine")
            } else {
                set.slots.iter().find_map(|s| {
                    let t = s.tag.as_ref()?;
                    (t.group == FourCC(*b"char") && s.name.as_deref()?.starts_with(folder))
                        .then_some(t.id)
                })
            }
        })
    };
    let character_ids: Vec<_> = plan
        .characters
        .iter()
        .map(|c| resolve(&c.h2_character))
        .collect();
    let mut characters = Vec::with_capacity(character_ids.len());
    for (index, (c, got)) in plan.characters.iter().zip(character_ids).enumerate() {
        if require_rebuilt && rebuilds_character(c.h1_type) && got.is_none() {
            return Err(invalid(format!(
                "missing rebuilt character {}",
                c.h2_character
            )));
        }
        let selected = if let Some(id) = got
            && scripted
            && let Some(existing) = ai_control.uncover.get(&id.0)
            && existing.requirements.map(f32::to_bits) != c.uncover.map(f32::to_bits)
        {
            // The same retail character can stand in for distinct source
            // variants. Inherit its native properties with a separate tag
            // identity so neither variant loses its authored thresholds.
            let group = FourCC(*b"char");
            let def = tags::defs::Defs::builtin()
                .group(group)
                .expect("char definition");
            let mut character = tags::defs::default_record(def);
            character
                .set("parent", tags::defs::Val::tag(group, id))
                .map_err(|e| invalid(format!("AI character parent: {e}")))?;
            let character = tags::defs::to_built(def, &character, &mut |_| None)
                .map_err(|e| invalid(format!("AI character variant: {e}")))?;
            Some(set.add_tag(
                group,
                &format!("h1_ai\\character_{index}"),
                TagPayload::Built(character),
            )?)
        } else {
            got
        };
        characters.push(selected.unwrap_or(TagId::NULL));
        let name = selected
            .and_then(|id| set.slots[id.index()].name.clone())
            .unwrap_or_else(|| "none".into());
        if scripted && let Some(id) = selected {
            ai_control.uncover.insert(
                id.0,
                UncoverRule {
                    character: name.clone(),
                    requirements: c.uncover,
                },
            );
        }
        chars.push((c.h1_variant.clone(), name));
    }
    if scripted {
        // Rebuilt major variants can be reached via their parent tag
        // without appearing directly in the scenario character palette.
        for slot in &set.slots {
            let (Some(tag), Some(name)) = (&slot.tag, &slot.name) else {
                continue;
            };
            if tag.group != FourCC(*b"char") || ai_control.uncover.contains_key(&tag.id.0) {
                continue;
            }
            if let Some(variant) = h1.find(h1map::Group::new(b"actv"), name) {
                let err = |e| invalid(format!("AI character {name}: {e}"));
                let variant = h1ai::h1::actor_variant(h1, variant).map_err(err)?;
                let actor = h1ai::h1::actor(h1, h1.tag(variant.actor.id)?).map_err(err)?;
                ai_control.uncover.insert(
                    tag.id.0,
                    UncoverRule {
                        character: name.clone(),
                        requirements: actor.uncover,
                    },
                );
            }
        }
    }
    Ok((characters, chars))
}
