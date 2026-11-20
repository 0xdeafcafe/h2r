//! The Tags tool's tag list, with no UI: which tags a search matches, and
//! the same tags by group or as a tree of their paths
//! (`objects\weapons\rifle\...`). The outliner draws one of the two;
//! `tag.list` answers with the same search.
use std::collections::BTreeMap;

use super::tag_doc::TagEntry;

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
