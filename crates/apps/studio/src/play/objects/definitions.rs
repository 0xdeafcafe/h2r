//! Respawn by definition (docs/editor/realtime-edits.md, "Respawn by
//! definition"): the object definitions behind the `next_spawn` rows of the
//! Pending tab. A row on an object tag names that tag; a row on a `hlmt` or
//! a `phmo` names the object tags that reach it. The game remakes every
//! placed object of each (`world.object.respawn {definition}`).

use std::collections::BTreeSet;

use map::TagId;
use serde_json::Value;

use crate::app::TagDoc;
use crate::app::tag_api::uses;
use crate::play::tag_sync::Effect;

/// Whether `group` is a placeable object group.
fn is_object(group: [u8; 4]) -> bool {
    tags::object::OBJECT_TYPES
        .iter()
        .any(|t| t.placement.is_some() && t.group.as_bytes() == &group[..])
}

/// The object tags whose placed objects the `next_spawn` rows in `rows`
/// change, in tag order.
pub(in crate::play) fn definitions(doc: &TagDoc, rows: &[Value]) -> Vec<TagId> {
    let mut out = BTreeSet::new();
    for row in rows {
        if row["doc"] != "tags" || Effect::of_row(row) != Some(Effect::NextSpawn) {
            continue;
        }
        let Some(id) = row["target"]["tag"].as_u64().and_then(|t| u32::try_from(t).ok()) else {
            continue;
        };
        let tag = TagId(id);
        let Some(entry) = doc.tag(tag) else { continue };
        match &entry.group.0 {
            g if is_object(*g) => {
                out.insert(tag);
            }
            b"hlmt" | b"phmo" => {
                for used in uses(doc, None, tag).tags {
                    if doc.tag(used.tag).is_some_and(|t| is_object(t.group.0)) {
                        out.insert(used.tag);
                    }
                }
            }
            _ => {}
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::tag_sync;
    use serde_json::json;

    #[test]
    fn rows_that_are_not_next_spawn_tag_fields_name_nothing() {
        let doc = TagDoc::default();
        let mut moved = tag_sync::row("scenario", Value::Null, "p", Effect::NextSpawn, "");
        moved["target"] = json!({"tag": 1});
        let mut reload = tag_sync::row("tags", Value::Null, "p", Effect::Reload, "");
        reload["target"] = json!({"tag": 1});
        let unknown = tag_sync::row("tags", Value::Null, "p", Effect::NextSpawn, "");
        assert!(definitions(&doc, &[moved, reload, unknown]).is_empty());
    }

    #[test]
    fn object_groups_are_the_placeable_ones() {
        assert!(is_object(*b"vehi") && is_object(*b"weap") && is_object(*b"scen"));
        assert!(!is_object(*b"hlmt") && !is_object(*b"shad"));
    }
}
