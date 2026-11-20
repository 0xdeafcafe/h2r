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

/// `char` root fields (definition `char`): flags, and the references to
/// the unit, creature, style and major character.
const FLAGS: usize = 0x0;
const UNIT: usize = 0xc;
const CREATURE: usize = 0x14;
const STYLE: usize = 0x1c;
const MAJOR: usize = 0x24;

/// The datum of the `char` tag's root reference at `at` (its group word;
/// the datum at `at + 4`), through an alias to the tag holding the bytes.
fn root_reference(set: &TagSet, id: TagId, at: usize) -> Option<TagId> {
    match &set.tag(id)?.payload {
        TagPayload::Built(block) => block.fixups.iter().find_map(|fixup| {
            match &fixup.kind {
                FixupKind::TagRef(target) if fixup.offset as usize == at => Some(*target),
                FixupKind::TagId(target) if fixup.offset as usize == at + 4 => Some(*target),
                _ => None,
            }
        }),
        TagPayload::Pinned { bytes, .. } => Some(TagId(u32::from_le_bytes(
            bytes.get(at + 4..at + 8)?.try_into().ok()?,
        ))),
        TagPayload::Alias { tag, .. } => root_reference(set, *tag, at),
        TagPayload::Empty => None,
    }
}

/// The `char` tag's root flags, through an alias.
fn root_flags(set: &TagSet, id: TagId) -> Option<u32> {
    match &set.tag(id)?.payload {
        TagPayload::Built(block) => Some(u32::from_le_bytes(
            block.bytes.get(FLAGS..FLAGS + 4)?.try_into().ok()?,
        )),
        TagPayload::Pinned { bytes, .. } => Some(u32::from_le_bytes(
            bytes.get(FLAGS..FLAGS + 4)?.try_into().ok()?,
        )),
        TagPayload::Alias { tag, .. } => root_flags(set, *tag),
        TagPayload::Empty => None,
    }
}

/// The tag holding `id`'s bytes: `id`, or what its alias names.
fn payload_owner(set: &TagSet, id: TagId) -> TagId {
    match set.tag(id).map(|t| &t.payload) {
        Some(TagPayload::Alias { tag, .. }) => payload_owner(set, *tag),
        _ => id,
    }
}

/// The `char` tag's major-character reference (`+0x24`, datum at `+0x28`).
fn major_character(set: &TagSet, id: TagId) -> Option<TagId> {
    let major = root_reference(set, id, MAJOR)?;
    let major = set
        .tag(major)
        .filter(|target| target.group == FourCC(*b"char"))?
        .id;
    (major != TagId::NULL).then_some(major)
}

/// A `char` built from its definition as a child of `parent`: it inherits
/// every property block from `parent`, carries `parent`'s flags and the
/// unit, creature and style the engine reads from the character itself
/// (squad setup reads the unit directly, `0x00200d11`), and promotes to
/// `major`.
fn child_character(set: &TagSet, parent: TagId, major: TagId) -> Result<map::build::Block> {
    use tags::defs::{self, Defs, Val};
    let def = Defs::builtin()
        .group(FourCC(*b"char"))
        .expect("char definition");
    let name = set.slots[parent.index()].name.clone().unwrap_or_default();
    let err = |e: defs::DefError| invalid(format!("child character of {name}: {e}"));
    let flags = root_flags(set, parent)
        .ok_or_else(|| invalid(format!("character {name} has no root to inherit")))?;
    let reference = |at| -> Val {
        let id = root_reference(set, parent, at).unwrap_or(TagId::NULL);
        match set.tag(id) {
            Some(t) => Val::tag(t.group, id),
            None => Val::tag(FourCC(*b"char"), TagId::NULL),
        }
    };
    let mut root = defs::default_record(def);
    root.set("flags", Val::Int(i64::from(flags))).map_err(err)?;
    root.set("parent", Val::tag(FourCC(*b"char"), parent))
        .map_err(err)?;
    root.set("unit", reference(UNIT)).map_err(err)?;
    root.set("creature", reference(CREATURE)).map_err(err)?;
    root.set("style", reference(STYLE)).map_err(err)?;
    root.set("major", Val::tag(FourCC(*b"char"), major))
        .map_err(err)?;
    defs::to_built(def, &root, &mut |_| None).map_err(err)
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
        inherit_majors(set, ai_control, &mut characters, &mut chars)?;
    }
    Ok((characters, chars))
}

/// Copy each registered character's authored thresholds onto the Halo 2
/// major tag its promotion installs, so a promoted actor keeps them.
/// A major with its own rule keeps that rule; equal rules share it.
///
/// The runtime finds a promoted actor's thresholds by its character, the
/// major, so one major cannot carry two authored sets. A palette character
/// whose thresholds differ from those its major already carries is
/// rebuilt: a child of it ([`child_character`]) that promotes to a child of
/// the major, each carrying the variant's thresholds, and the palette
/// entries naming it name the child. A registered character no palette
/// entry names (so none to repoint) with a conflicting major stays a
/// conversion error.
fn inherit_majors(
    set: &mut TagSet,
    ai_control: &mut Control,
    characters: &mut [TagId],
    chars: &mut [(String, String)],
) -> Result<()> {
    fn name(set: &TagSet, id: TagId) -> String {
        set.slots[id.index()].name.clone().unwrap_or_else(|| "unnamed".into())
    }
    let mut inherited = std::collections::BTreeMap::<u32, UncoverRule>::new();
    let mut rebuilt = Vec::new();
    for (&id, rule) in &ai_control.uncover {
        let Some(major) = major_character(set, TagId(id)) else {
            continue;
        };
        if ai_control.uncover.contains_key(&major.0) {
            continue;
        }
        if let Some(existing) = inherited.get(&major.0) {
            if existing.requirements.map(f32::to_bits) != rule.requirements.map(f32::to_bits) {
                if !characters.contains(&TagId(id)) {
                    return Err(invalid(format!(
                        "AI character {} upgrades to {} with different Halo 1 uncover thresholds, and no palette entry names it to rebuild",
                        rule.character, existing.character,
                    )));
                }
                rebuilt.push((TagId(id), major, rule.requirements));
            }
            continue;
        }
        inherited.insert(major.0, UncoverRule {
            character: name(set, major),
            requirements: rule.requirements,
        });
    }
    ai_control.uncover.extend(inherited);
    for (k, (id, major, requirements)) in rebuilt.into_iter().enumerate() {
        let parent = payload_owner(set, id);
        let major_parent = payload_owner(set, major);
        let new_major = set.add_tag(
            FourCC(*b"char"),
            &format!("h1_ai\\promoted_{k}_major"),
            TagPayload::Empty,
        )?;
        let child = set.add_tag(
            FourCC(*b"char"),
            &format!("h1_ai\\promoted_{k}"),
            TagPayload::Empty,
        )?;
        let next = root_reference(set, major_parent, MAJOR)
            .filter(|m| set.tag(*m).is_some_and(|t| t.group == FourCC(*b"char")))
            .unwrap_or(TagId::NULL);
        let major_root = child_character(set, major_parent, next)?;
        let child_root = child_character(set, parent, new_major)?;
        set.tag_mut(new_major).expect("added").payload = TagPayload::Built(major_root);
        set.tag_mut(child).expect("added").payload = TagPayload::Built(child_root);
        let child_name = name(set, child);
        for (entry, report) in characters.iter_mut().zip(chars.iter_mut()) {
            if *entry == id {
                *entry = child;
                report.1 = child_name.clone();
            }
        }
        ai_control.uncover.insert(child.0, UncoverRule {
            character: child_name,
            requirements,
        });
        ai_control.uncover.insert(new_major.0, UncoverRule {
            character: name(set, new_major),
            requirements,
        });
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
        let (mut set, major, built, pinned, alias) = set_with_major();
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
        inherit_majors(&mut set, &mut control, &mut [], &mut []).unwrap();
        assert_eq!(control.uncover[&major.0].requirements, [0.5, 0.1]);
        assert_eq!(control.uncover[&major.0].character, "h1\\major");
    }

    /// Two palette characters with different thresholds promote through
    /// one major: the second is rebuilt as a child of its character that
    /// promotes to a child of the major, both carrying its thresholds, and
    /// its palette entry names the child. The first keeps the major.
    #[test]
    fn a_conflicting_promotion_gets_its_own_rebuilt_major() {
        let (mut set, major, built, pinned, _) = set_with_major();
        let mut control = Control::default();
        for (id, name, requirements) in [
            (built, "retail\\built", [0.5, 0.1]),
            (pinned, "retail\\pinned", [0.0, 0.0]),
        ] {
            control.uncover.insert(id.0, UncoverRule {
                character: name.into(),
                requirements,
            });
        }
        let mut characters = [built, pinned, built];
        let mut chars = [
            ("a".to_string(), "retail\\built".to_string()),
            ("b".to_string(), "retail\\pinned".to_string()),
            ("c".to_string(), "retail\\built".to_string()),
        ];
        inherit_majors(&mut set, &mut control, &mut characters, &mut chars).unwrap();

        // The first registered keeps the shared major; the other is rebuilt.
        let (kept, moved) = if characters[1] == pinned { (pinned, built) } else { (built, pinned) };
        let kept_rule = control.uncover[&kept.0].requirements;
        let moved_rule = control.uncover[&moved.0].requirements;
        assert_eq!(control.uncover[&major.0].requirements, kept_rule);
        let entries: Vec<usize> = [built, pinned, built]
            .iter()
            .enumerate()
            .filter(|(_, id)| **id == moved)
            .map(|(i, _)| i)
            .collect();
        let child = characters[entries[0]];
        assert!(entries.iter().all(|&i| characters[i] == child));
        assert_ne!(child, moved);
        assert!(matches!(set.tag(child).unwrap().payload, TagPayload::Built(_)));
        assert_eq!(chars[entries[0]].1, "h1_ai\\promoted_0");
        // The child inherits from the character it replaces and promotes to
        // its own major, a child of the shared one with no major of its own.
        assert_eq!(root_reference(&set, child, 0x4), Some(moved));
        let own_major = major_character(&set, child).unwrap();
        assert_ne!(own_major, major);
        assert_eq!(root_reference(&set, own_major, 0x4), Some(major));
        assert_eq!(major_character(&set, own_major), None);
        assert_eq!(set.slots[own_major.index()].name.as_deref(), Some("h1_ai\\promoted_0_major"));
        // Both carry the rebuilt variant's thresholds; the kept one is as it was.
        assert_eq!(control.uncover[&child.0].requirements, moved_rule);
        assert_eq!(control.uncover[&own_major.0].requirements, moved_rule);
        assert_eq!(control.uncover[&major.0].requirements, kept_rule);
        assert!(characters.contains(&kept));
    }

    #[test]
    fn a_major_with_its_own_rule_keeps_it_and_a_conflict_errors() {
        let (mut set, _, built, pinned, _) = set_with_major();
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
                requirements: [0.0, 0.0],
            },
        );
        inherit_majors(&mut set, &mut control, &mut [], &mut []).unwrap();
        assert_eq!(control.uncover[&major.0].requirements, [0.0, 0.0]);

        control.uncover.remove(&major.0);
        control.uncover.insert(pinned.0, UncoverRule {
            character: "retail\\pinned".into(),
            requirements: [0.0, 0.0],
        });
        // No palette entry names either, so neither can be rebuilt.
        assert!(inherit_majors(&mut set, &mut control, &mut [], &mut []).is_err());
    }
}
