//! AI characters use the retail weapon their firing profiles and animations support.

use h1ai::h1::H1Ai;
use h1map::H1Map;
use map::TagId;
use map::build::{Block, TagSet};

use super::{BaseScenario, PALETTE, put_block, u32_at};
use crate::{Result, invalid};

fn field() -> u32 {
    let (_, at, _) = tags::object::OBJECT_TYPES[2]
        .placement
        .expect("weapon palette");
    u32::from(at)
}

pub(super) fn base_palette(base: &BaseScenario) -> Result<Vec<TagId>> {
    base.elements(field(), PALETTE)?
        .into_iter()
        .map(|entry| u32_at(entry, 4).map(TagId))
        .collect()
}

/// Look up retail tags before placing converted objects with similar names.
pub(super) fn retail_tags(
    h1: &H1Map,
    ai: &H1Ai,
    set: &TagSet,
    dropped: &mut Vec<String>,
) -> Result<Vec<Option<TagId>>> {
    let mut tags = Vec::with_capacity(ai.actor_palette.len());
    for variant in &ai.actor_palette {
        let weapon = variant
            .as_ref()
            .map(|v| v.weapon.id)
            .filter(|id| !id.is_none());
        let Some(id) = weapon else {
            tags.push(None);
            continue;
        };
        let tag = h1.tag(id)?;
        let retail = crate::h2assets::stand_in(set, tag).ok().flatten();
        if retail.is_none() {
            dropped.push(format!(
                "ai weapon {}: no retail counterpart in map",
                tag.name
            ));
        }
        tags.push(retail);
    }
    Ok(tags)
}

/// Keep placement indices stable. Multiple variants may share a weapon.
pub(super) fn install(
    root: &mut Block,
    mut palette: Vec<TagId>,
    tags: &[Option<TagId>],
) -> Result<Vec<Option<i16>>> {
    let original_len = palette.len();
    let mut indices = Vec::with_capacity(tags.len());
    for &tag in tags {
        let Some(id) = tag else {
            indices.push(None);
            continue;
        };
        let index = palette.iter().position(|&t| t == id).unwrap_or_else(|| {
            palette.push(id);
            palette.len() - 1
        });
        indices.push(Some(
            i16::try_from(index).map_err(|_| invalid("AI weapon palette overflow"))?,
        ));
    }
    if palette.len() != original_len {
        let entries = palette
            .into_iter()
            .map(|id| {
                let mut entry = Block::zeroed(PALETTE);
                entry.tag_ref(0, id);
                entry
            })
            .collect();
        put_block(root, field(), PALETTE, entries);
    }
    Ok(indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use map::build::FixupKind;

    #[test]
    fn keeps_placed_weapons_and_shares_new_actor_weapons() {
        let (placed, ai_only) = (TagId(42), TagId(54));
        let mut root = Block::zeroed(super::super::ROOT);
        let indices = install(
            &mut root,
            vec![TagId::NULL, placed],
            &[Some(ai_only), None, Some(placed), Some(ai_only)],
        )
        .unwrap();
        assert_eq!(indices, [Some(2), None, Some(1), Some(2)]);
        let FixupKind::Block { count, child } = &root.fixups[0].kind else {
            panic!("missing palette")
        };
        assert_eq!(*count, 3);
        assert_eq!(child.bytes.len(), 3 * PALETTE);
        for (i, id) in [TagId::NULL, placed, ai_only].into_iter().enumerate() {
            assert_eq!(child.fixups[i].offset as usize, i * PALETTE);
            assert_eq!(child.fixups[i].kind, FixupKind::TagRef(id));
        }
    }
}
