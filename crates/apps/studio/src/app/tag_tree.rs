//! The scenario's tag tree and the searchable group and path views of
//! all tags. The window and the socket use the same tree.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{Read, Seek},
    sync::Arc,
};

use map::{Map, TagId};
use scenario_edit::{BlockKind, Role, Scenario, geometry, layout::BLOCKS, scene};

use super::{
    tag_api::{self, Node},
    tag_doc::{TagDoc, TagEntry},
};

/// How the outliner arranges the tags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Arrange {
    /// By group (`bitm`, `weap`, ...), then name.
    #[default]
    Group,
    /// By folder of their paths.
    Path,
}

/// `group:name` lower-cased, what a search matches against.
fn haystack(t: &TagEntry) -> String {
    format!("{}:{}", tags::defs::fourcc_text(t.group), t.name).to_ascii_lowercase()
}

/// Whether `t` matches `query`: every whitespace-separated word of it is
/// in the tag's `group:name` (case-insensitive). An empty query matches
/// all.
pub fn matches(t: &TagEntry, query: &str) -> bool {
    let words: Vec<String> = query
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect();
    if words.is_empty() {
        return true;
    }
    let h = haystack(t);
    words.iter().all(|w| h.contains(w.as_str()))
}

/// Indices of the tags matching `query`, in the map's order.
pub fn search(tags: &[TagEntry], query: &str) -> Vec<usize> {
    tags.iter()
        .enumerate()
        .filter(|(_, t)| matches(t, query))
        .map(|(i, _)| i)
        .collect()
}

/// The tags at `hits` by group text, each group's in name order.
pub fn by_group(tags: &[TagEntry], hits: &[usize]) -> BTreeMap<String, Vec<usize>> {
    let mut out: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for &i in hits {
        out.entry(tags::defs::fourcc_text(tags[i].group))
            .or_default()
            .push(i);
    }
    for list in out.values_mut() {
        list.sort_by(|a, b| tags[*a].name.cmp(&tags[*b].name));
    }
    out
}

/// The scenario roots read from the source index and scenario payload.
pub struct ScenarioRoots {
    bsps: Vec<(TagId, Option<TagId>)>,
    skies: Vec<TagId>,
    globals: Option<TagId>,
}

impl ScenarioRoots {
    pub fn read<R: Read + Seek>(map: &mut Map<R>) -> Self {
        Self {
            bsps: geometry::scenario_bsp_lightmaps(map).unwrap_or_default(),
            skies: scene::scenario_skies(map),
            globals: map.globals().map(|tag| tag.id),
        }
    }
}

pub struct ScenarioTree {
    pub root: Row,
    /// Indices in the tag document, in its order.
    pub unreachable: Vec<usize>,
    pub truncated: bool,
}

#[derive(Default)]
pub struct Row {
    pub tag: Option<TagId>,
    pub label: String,
    pub via: Option<String>,
    /// Present for a palette entry, including one with no placements.
    pub placements: Option<Vec<(BlockKind, usize)>>,
    pub children: Arc<[Row]>,
    pub cycle: bool,
}

impl Row {
    fn section(label: impl Into<String>, children: Vec<Row>) -> Self {
        Self {
            label: label.into(),
            children: children.into(),
            ..Self::default()
        }
    }

    fn tag(id: TagId) -> Self {
        Self {
            tag: Some(id),
            ..Self::default()
        }
    }
}

impl From<Node> for Row {
    fn from(node: Node) -> Self {
        Self {
            tag: Some(node.tag),
            label: node.shortcut.unwrap_or("").into(),
            via: Some(node.via),
            children: node
                .children
                .into_iter()
                .map(Row::from)
                .collect::<Arc<[_]>>(),
            cycle: node.cycle,
            ..Self::default()
        }
    }
}

/// Build the named sections and the complement of the full reference walk.
/// `depth` limits chains below palette entries; it does not limit reachability.
pub fn scenario_tree(
    doc: &TagDoc,
    scenario: &Scenario,
    roots: &ScenarioRoots,
    depth: usize,
) -> ScenarioTree {
    let mut named = HashSet::from([scenario.tag]);
    let mut sections = Vec::new();
    let mut structure = Vec::new();
    for (i, &(bsp, lightmap)) in roots.bsps.iter().enumerate() {
        let mut row = Row::section(format!("BSP {i}"), Vec::new());
        if bsp != TagId::NULL && bsp.0 != 0 {
            row.tag = Some(bsp);
            named.insert(bsp);
        }
        if let Some(lightmap) = lightmap.filter(|id| id.0 != 0) {
            named.insert(lightmap);
            row.children = Arc::from([Row {
                label: "Lightmap".into(),
                ..Row::tag(lightmap)
            }]);
        }
        structure.push(row);
    }
    sections.push(Row::section("Structure", structure));

    let skies = roots
        .skies
        .iter()
        .copied()
        .filter(|id| *id != TagId::NULL && id.0 != 0)
        .map(|id| {
            named.insert(id);
            Row::tag(id)
        })
        .collect();
    sections.push(Row::section("Skies", skies));

    let mut chains: HashMap<TagId, Arc<[Row]>> = HashMap::new();
    let mut truncated = false;
    let mut objects = Vec::new();
    for def in BLOCKS.iter().filter(|def| def.palette.is_some()) {
        let block = scenario.block(def.kind);
        let Some(field) = def.role(Role::PaletteIndex) else {
            continue;
        };
        let mut placements = vec![Vec::new(); block.palette.len()];
        for index in 0..block.len() {
            if let Ok(value) = scenario.get(def.kind, index, field.name)
                && let Some(slot) = value.as_int().and_then(|v| usize::try_from(v).ok())
                && let Some(entries) = placements.get_mut(slot)
            {
                entries.push((def.kind, index));
            }
        }
        let mut rows = Vec::with_capacity(block.palette.len());
        for (slot, (reference, placed)) in block.palette.iter().zip(placements).enumerate() {
            let mut row = Row {
                placements: Some(placed),
                ..Row::default()
            };
            if reference.is_none() || reference.id.0 == 0 {
                row.label = format!("Empty entry {slot}");
            } else {
                let id = reference.id;
                named.insert(id);
                row.tag = Some(id);
                row.children = Arc::clone(chains.entry(id).or_insert_with(|| {
                    let chain = tag_api::chain(doc, Some((id, "")), depth);
                    truncated |= chain.truncated;
                    chain
                        .root
                        .into_iter()
                        .flat_map(|node| node.children)
                        .map(Row::from)
                        .collect()
                }));
            }
            rows.push(row);
        }
        objects.push(Row::section(
            format!("{} ({})", def.label, rows.len()),
            rows,
        ));
    }
    sections.push(Row::section("Objects", objects));

    let mut globals = Row::section("Globals", Vec::new());
    globals.tag = roots.globals;
    named.extend(roots.globals);
    sections.push(globals);

    let mut seen = named;
    let other: Vec<_> = doc
        .tag_refs(scenario.tag)
        .into_iter()
        .filter(|link| doc.tag(link.tag).is_some() && seen.insert(link.tag))
        .map(|link| Row {
            via: Some(link.via),
            ..Row::tag(link.tag)
        })
        .collect();
    sections.push(Row::section(
        format!("Other references ({})", other.len()),
        other,
    ));

    // Seed the known roots even when a scenario layout does not name them.
    // A datum is expanded once, independently of display depth and cycles.
    let mut todo: Vec<_> = seen
        .iter()
        .copied()
        .filter(|id| *id != scenario.tag)
        .collect();
    while let Some(id) = todo.pop() {
        for link in doc.tag_refs(id) {
            if doc.tag(link.tag).is_some() && seen.insert(link.tag) {
                todo.push(link.tag);
            }
        }
    }
    let unreachable = doc
        .tags()
        .iter()
        .enumerate()
        .filter_map(|(i, tag)| (!seen.contains(&tag.id)).then_some(i))
        .collect();
    ScenarioTree {
        root: Row {
            tag: Some(scenario.tag),
            ..Row::section("Scenario", sections)
        },
        unreachable,
        truncated,
    }
}

/// A folder of the path tree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folder {
    /// Sub-folders by name.
    pub folders: BTreeMap<String, Folder>,
    /// Tags whose path ends here, by name then group.
    pub tags: Vec<usize>,
    /// Tags here and below.
    pub count: usize,
}

impl Folder {
    /// The folder at `path` (`objects\weapons`), if there is one.
    pub fn find(&self, path: &str) -> Option<&Folder> {
        let mut at = self;
        for part in path.split(['\\', '/']).filter(|p| !p.is_empty()) {
            at = at.folders.get(part)?;
        }
        Some(at)
    }
}

/// The tags at `hits` as a tree of their paths: a name `a\b\c` sits in
/// folder `a`, sub-folder `b`, as `c`. A name with no separator (or a
/// `#<index>` with none) sits at the root.
pub fn by_path(tags: &[TagEntry], hits: &[usize]) -> Folder {
    let mut root = Folder::default();
    for &i in hits {
        let parts: Vec<&str> = tags[i].name.split('\\').collect();
        let mut at = &mut root;
        at.count += 1;
        for part in &parts[..parts.len() - 1] {
            at = at.folders.entry((*part).to_owned()).or_default();
            at.count += 1;
        }
        at.tags.push(i);
    }
    fn sort(f: &mut Folder, tags: &[TagEntry]) {
        f.tags.sort_by(|a, b| {
            (leaf(&tags[*a].name), tags[*a].group.as_u32())
                .cmp(&(leaf(&tags[*b].name), tags[*b].group.as_u32()))
        });
        for sub in f.folders.values_mut() {
            sort(sub, tags);
        }
    }
    sort(&mut root, tags);
    root
}

/// The last part of a tag's path.
pub fn leaf(name: &str) -> &str {
    name.rsplit('\\').next().unwrap_or(name)
}

/// Every tag in `folder` and below, in the tree's order.
pub fn under(folder: &Folder) -> Vec<usize> {
    let mut out = Vec::new();
    fn walk(f: &Folder, out: &mut Vec<usize>) {
        for sub in f.folders.values() {
            walk(sub, out);
        }
        out.extend_from_slice(&f.tags);
    }
    walk(folder, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tag_doc::Access;
    use map::{FourCC, TagId};

    fn entry(i: u32, group: &[u8; 4], name: &str) -> TagEntry {
        TagEntry {
            id: TagId(0xe100_0000 | i),
            group: FourCC(*group),
            name: name.to_owned(),
            address: 0,
            size: 0,
            access: Access::Edit,
            elsewhere: None,
        }
    }

    fn sample() -> Vec<TagEntry> {
        vec![
            entry(0, b"weap", "objects\\weapons\\rifle\\rifle"),
            entry(1, b"bitm", "objects\\weapons\\rifle\\bitmaps\\rifle"),
            entry(2, b"weap", "objects\\weapons\\pistol\\pistol"),
            entry(3, b"matg", "globals"),
            entry(4, b"snd!", "sound\\weapons\\rifle\\fire"),
        ]
    }

    #[test]
    fn every_word_of_a_search_must_match_group_or_name() {
        let t = sample();
        assert_eq!(search(&t, ""), vec![0, 1, 2, 3, 4]);
        assert_eq!(search(&t, "rifle"), vec![0, 1, 4]);
        assert_eq!(search(&t, "RIFLE weap:"), vec![0]);
        assert_eq!(search(&t, "rifle weap"), vec![0, 1, 4], "weap is in weapons too");
        assert_eq!(search(&t, "weap:"), vec![0, 2]);
        assert_eq!(search(&t, "snd!"), vec![4]);
        assert_eq!(search(&t, "nothing"), Vec::<usize>::new());
        assert!(matches(&t[3], "matg glob"));
        assert!(!matches(&t[3], "matg rifle"));
    }

    #[test]
    fn groups_list_their_tags_by_name() {
        let t = sample();
        let g = by_group(&t, &search(&t, ""));
        assert_eq!(g.keys().collect::<Vec<_>>(), ["bitm", "matg", "snd!", "weap"]);
        assert_eq!(g["weap"], vec![2, 0]);
    }

    #[test]
    fn paths_make_a_folder_tree_with_counts() {
        let t = sample();
        let root = by_path(&t, &search(&t, ""));
        assert_eq!(root.count, 5);
        assert_eq!(root.tags, vec![3], "a name with no folder sits at the root");
        assert_eq!(root.folders.keys().collect::<Vec<_>>(), ["objects", "sound"]);
        let rifle = root.find("objects\\weapons\\rifle").unwrap();
        assert_eq!(rifle.count, 2);
        assert_eq!(rifle.tags, vec![0]);
        assert_eq!(rifle.folders["bitmaps"].tags, vec![1]);
        assert_eq!(root.find("objects/weapons").unwrap().count, 3);
        assert!(root.find("objects\\vehicles").is_none());
        assert_eq!(under(root.find("objects").unwrap()), vec![2, 1, 0]);
        // A search narrows the tree.
        let only = by_path(&t, &search(&t, "pistol"));
        assert_eq!(only.count, 1);
        assert_eq!(under(&only), vec![2]);
        assert_eq!(leaf("a\\b\\c"), "c");
        assert_eq!(leaf("globals"), "globals");
    }
}
