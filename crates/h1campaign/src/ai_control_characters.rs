//! Resolve native character tags and preserve per-variant Halo 1 uncover tuning.

use h1ai::{
    control::{Control, UncoverRule},
    convert::{Plan, rebuilds_character},
};
use h1map::H1Map;
use map::{
    FourCC, TagId,
    build::{FixupKind, TagPayload, TagSet},
};

use crate::{Result, invalid};

/// The `char` tag's major-character reference (`+0x24`, datum at `+0x28`).
fn major_character(set: &TagSet, id: TagId) -> Option<TagId> {
    let tag = set.tag(id)?;
    let major = match &tag.payload {
        TagPayload::Built(block) => {
            block
                .fixups
                .iter()
                .find_map(|fixup| match (fixup.offset, &fixup.kind) {
                    (0x24, FixupKind::TagRef(target)) | (0x28, FixupKind::TagId(target)) => {
                        Some(*target)
                    }
                    _ => None,
                })?
        }
        TagPayload::Pinned { bytes, .. } => {
            let datum = u32::from_le_bytes(bytes.get(0x28..0x2c)?.try_into().ok()?);
            TagId(datum)
        }
        TagPayload::Alias { tag, .. } => return major_character(set, *tag),
        TagPayload::Empty => return None,
    };
    let major = set
        .tag(major)
        .filter(|target| target.group == FourCC(*b"char"))?
        .id;
    (major != TagId::NULL).then_some(major)
}

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
        // Rebuilt weapons keep their Halo 1 paths, so every converted
        // `weap` the set carries under a Halo 1 name brings its authored
        // report loudness (`weap +0x270`) with it. Retail stand-ins
        // already carry theirs from the weapon palette pass.
        for slot in &set.slots {
            let (Some(tag), Some(name)) = (&slot.tag, &slot.name) else {
                continue;
            };
            if tag.group != FourCC(*b"weap") || ai_control.weapon_reports.contains_key(name.as_str()) {
                continue;
            }
            if let Some(source) = h1.find(h1map::Group::new(b"weap"), name) {
                let loud = h1ai::h1::weapon_loudness(h1, source)
                    .map_err(|e| invalid(format!("AI weapon {name}: {e}")))?;
                ai_control.weapon_reports.entry(name.clone()).or_insert(loud);
            }
        }
        // A substitute that upgrades through a Halo 2-only major tag would
        // otherwise lose its authored thresholds: the promoted actor reads
        // the major tag, which no Halo 1 variant names. Variants with a
        // source tag of their own kept theirs above.
        inherit_majors(set, ai_control)?;
    }
    Ok((characters, chars))
}

/// Copy each registered character's authored thresholds onto the Halo 2
/// major tag its promotion installs, so a promoted actor keeps them.
/// A major with its own rule keeps that rule; equal rules are fine; two
/// different authored sets on one major tag is a conversion error.
fn inherit_majors(set: &TagSet, ai_control: &mut Control) -> Result<()> {
    let registered: Vec<(TagId, [u32; 2], String)> = ai_control
        .uncover
        .iter()
        .map(|(&id, rule)| {
            (
                TagId(id),
                rule.requirements.map(f32::to_bits),
                rule.character.clone(),
            )
        })
        .collect();
    for (id, requirements, character) in registered {
        let Some(major) = major_character(set, id) else {
            continue;
        };
        if let Some(existing) = ai_control.uncover.get(&major.0) {
            if existing.requirements.map(f32::to_bits) != requirements {
                return Err(invalid(format!(
                    "AI character {character} upgrades to {} with different Halo 1 uncover thresholds",
                    set.slots[major.index()]
                        .name
                        .as_deref()
                        .unwrap_or("unnamed")
                )));
            }
            continue;
        }
        let name = set.slots[major.index()]
            .name
            .clone()
            .unwrap_or_else(|| "unnamed".into());
        ai_control.uncover.insert(
            major.0,
            UncoverRule {
                character: name,
                requirements: requirements.map(f32::from_bits),
            },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use map::build::{Block, Fixup};

    fn block(fixups: Vec<Fixup>) -> TagPayload {
        TagPayload::Built(Block {
            bytes: vec![0; 0x2c],
            align: 4,
            fixups,
            share: None,
            phase: 0,
            sort_by_string_id: None,
        })
    }

    /// A substitute character whose promotion installs a Halo 2-only
    /// major, over a built payload, a pinned one and an alias of it.
    /// The pinned tag's bytes carry `major`'s datum at the tagref's datum
    /// half, as a verbatim retail payload would.
    fn set_with_major() -> (TagSet, TagId, TagId, TagId, TagId) {
        let mut set = TagSet::new(map::CacheType::Campaign, "t", "t");
        let major = set
            .add_tag(FourCC(*b"char"), "h1\\major", block(Vec::new()))
            .unwrap();
        let built = set
            .add_tag(
                FourCC(*b"char"),
                "retail\\built",
                block(vec![Fixup {
                    offset: 0x24,
                    kind: FixupKind::TagRef(major),
                }]),
            )
            .unwrap();
        let mut pinned = vec![0; 0x2c];
        pinned[0x28..0x2c].copy_from_slice(&major.0.to_le_bytes());
        let pinned = set
            .add_tag(
                FourCC(*b"char"),
                "retail\\pinned",
                TagPayload::Pinned {
                    address: 0,
                    bytes: pinned,
                },
            )
            .unwrap();
        let alias = set
            .add_tag(
                FourCC(*b"char"),
                "h1_ai\\character_0",
                TagPayload::Alias {
                    tag: built,
                    offset: 0,
                },
            )
            .unwrap();
        (set, major, built, pinned, alias)
    }

    #[test]
    fn a_substitutes_promotion_keeps_its_authored_thresholds() {
        let (set, major, built, pinned, alias) = set_with_major();
        assert_eq!(major_character(&set, built), Some(major));
        assert_eq!(major_character(&set, pinned), Some(major));
        assert_eq!(major_character(&set, alias), Some(major));
        let mut control = Control::default();
        control.uncover.insert(
            built.0,
            UncoverRule {
                character: "retail\\built".into(),
                requirements: [0.5, 0.1],
            },
        );
        inherit_majors(&set, &mut control).unwrap();
        assert_eq!(control.uncover[&major.0].requirements, [0.5, 0.1]);
        assert_eq!(control.uncover[&major.0].character, "h1\\major");
    }

    #[test]
    fn a_major_with_its_own_rule_keeps_it_and_a_conflict_errors() {
        let (set, _, built, _, _) = set_with_major();
        let major = major_character(&set, built).unwrap();
        let mut control = Control::default();
        control.uncover.insert(
            built.0,
            UncoverRule {
                character: "retail\\built".into(),
                requirements: [0.5, 0.1],
            },
        );
        control.uncover.insert(
            major.0,
            UncoverRule {
                character: "h1\\major".into(),
                requirements: [0.5, 0.1],
            },
        );
        inherit_majors(&set, &mut control).unwrap();
        control.uncover.insert(
            major.0,
            UncoverRule {
                character: "h1\\major".into(),
                requirements: [0.0, 0.0],
            },
        );
        assert!(inherit_majors(&set, &mut control).is_err());
    }
}
