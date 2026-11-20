//! Resolve native character tags and preserve per-variant Halo 1 uncover tuning.

use h1ai::{
    control::{Control, UncoverRule},
    convert::Plan,
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
    let character_ids: Vec<_> = plan.characters.iter().map(|c| {
        if c.h2_character == c.h1_variant {
            set.converted(FourCC(*b"char"), &c.h1_variant)
        } else {
            set.find(FourCC(*b"char"), &c.h2_character)
        }
    }).collect();
    let mut characters = Vec::with_capacity(character_ids.len());
    for (index, (c, got)) in plan.characters.iter().zip(character_ids).enumerate() {
        if require_rebuilt && got.is_none() {
            return Err(h1convert::Error::no_rebuild(
                &h1.header().name,
                &c.h1_variant,
                format!("missing character {}", c.h2_character),
            )
            .into());
        }
        let selected = if let Some(id) = got
            && scripted
            && let Some(existing) = ai_control.uncover.get(&id.0)
            && existing.requirements.map(f32::to_bits) != c.uncover.map(f32::to_bits)
        {
            // Squad setup reads the character's unit directly (0x200d11).
            // A second native tag identity shares the entire character
            // payload, including fields the engine does not inherit.
            Some(set.add_tag(
                FourCC(*b"char"),
                &format!("h1_ai\\character_{index}"),
                TagPayload::Alias { tag: id, offset: 0 },
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
