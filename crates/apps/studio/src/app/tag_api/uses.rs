//! Where a tag is used (docs/editor/click-to-edit.md, "Where is this
//! used"): the reverse reference index over the map's tags, cached in
//! [`TagDoc`] by its revision, and the placements whose palette entry
//! reaches the tag.

use std::collections::{HashMap, HashSet, VecDeque};

use map::TagId;
use scenario_edit::Scenario;
use serde_json::{Value, json};

use super::{TagDoc, placement_root};
use crate::app::tag_doc::Access;

/// The reverse index: for each tag, the tags that refer to it and the
/// field path in each that does. The scenario tag is out of it — its
/// references are the placements', which [`uses`] reads from the live
/// scenario.
#[derive(Default)]
pub(crate) struct Index {
    edges: HashMap<TagId, Vec<(TagId, String)>>,
}

/// One tag that reaches the asked-for tag.
pub struct Used {
    pub tag: TagId,
    /// The field path in it that refers to the next tag closer.
    pub via: String,
}

/// One placement whose object tag is the asked-for tag or reaches it.
pub struct UsedPlacement {
    pub kind: scenario_edit::BlockKind,
    pub index: usize,
}

/// The answer to "where is this tag used".
pub struct Uses {
    /// The tags that refer to it, transitively (up through them to
    /// object tags), in the map's tag order.
    pub tags: Vec<Used>,
    /// The placements whose palette entry reaches it, in block-name
    /// order then index.
    pub placements: Vec<UsedPlacement>,
}

impl Uses {
    pub fn json(&self, doc: &TagDoc) -> Value {
        let block_name = crate::api::block_name;
        json!({
            "tags": self.tags.iter().map(|u| json!({
                "tag": format!("{:#010x}", u.tag.0),
                "group": doc.tag(u.tag).map(|t| tags::defs::fourcc_text(t.group)),
                "name": doc.tag(u.tag).map(|t| &t.name),
                "via": u.via,
            })).collect::<Vec<_>>(),
            "placements": self.placements.iter().map(|p| json!({
                "block": block_name(p.kind),
                "index": p.index,
            })).collect::<Vec<_>>(),
            // Live objects whose definition reaches the tag: read when a
            // game is paired (click-to-edit.md, C5).
            "objects": [],
        })
    }
}

/// The index over `doc`'s tags, cached by its revision: a reference edit
/// changes it without changing the document's shape. `skip` leaves a tag
/// out (the scenario's).
pub(crate) fn build(doc: &TagDoc, skip: Option<TagId>) -> Index {
    let mut edges: HashMap<TagId, Vec<(TagId, String)>> = HashMap::new();
    for t in doc.tags() {
        if Some(t.id) == skip || matches!(t.access, Access::None(_)) {
            continue;
        }
        for link in doc.tag_refs(t.id, None).links {
            if link.tag != t.id {
                edges.entry(link.tag).or_default().push((t.id, link.via));
            }
        }
    }
    Index { edges }
}

/// Where `tag` is used: the tags that refer to it — transitively, so the
/// object tags whose chains run through it are named — and the
/// placements whose palette entry reaches it. `scenario`, when the open
/// map's session is given, supplies the placements; the tag graph comes
/// from the index cached in the document.
pub fn uses(doc: &TagDoc, scenario: Option<&Scenario>, tag: TagId) -> Uses {
    let index = doc.reverse_index(scenario.map(|s| s.tag));
    // Walk outward from the tag: each referrer enters once, by the edge
    // that found it, so `via` names its next step toward the tag.
    let mut seen = HashSet::from([tag]);
    let mut queue = VecDeque::from([tag]);
    let mut tags = Vec::new();
    while let Some(current) = queue.pop_front() {
        for (referrer, via) in index.edges.get(&current).into_iter().flatten() {
            let referrer = *referrer;
            if seen.insert(referrer) {
                queue.push_back(referrer);
                tags.push(Used {
                    tag: referrer,
                    via: via.clone(),
                });
            }
        }
    }
    tags.sort_by_key(|u| u.tag.0);
    let reaches = |root: TagId| root == tag || tags.iter().any(|u| u.tag == root);
    let mut placements = Vec::new();
    if let Some(scenario) = scenario {
        for block in &scenario.blocks {
            for index in 0..block.len() {
                if placement_root(scenario, block.def.kind, index)
                    .is_some_and(|(root, _)| reaches(root))
                {
                    placements.push(UsedPlacement {
                        kind: block.def.kind,
                        index,
                    });
                }
            }
        }
    }
    placements.sort_by_key(|p| (crate::api::block_name(p.kind), p.index));
    Uses { tags, placements }
}
