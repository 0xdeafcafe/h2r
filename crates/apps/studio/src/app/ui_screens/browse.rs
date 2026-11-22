//! The outliner's folders, as pure data: our screens grouped by the first
//! dotted segment of their names, the disc's retail screens by their tag
//! path, and what a folder or a root shows as a grid of previews
//! ([`Browse`]). No disc, no UI toolkit. docs/editor/editor.md "UI Screens".

use std::collections::HashMap;

use serde_json::{Value, json};

/// The two lists of the outliner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Root {
    Ours,
    Retail,
}

impl Root {
    /// The word the agent API uses.
    pub fn name(self) -> &'static str {
        match self {
            Root::Ours => "ours",
            Root::Retail => "retail",
        }
    }

    /// The outliner's header.
    pub fn title(self) -> &'static str {
        match self {
            Root::Ours => "Our screens",
            Root::Retail => "Retail screens",
        }
    }

    pub fn parse(s: &str) -> Option<Root> {
        match s {
            "ours" => Some(Root::Ours),
            "retail" => Some(Root::Retail),
            _ => None,
        }
    }
}

/// What the centre panel shows as a grid: a root, or a folder under it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Browse {
    pub root: Root,
    /// The folder's [`Folder::id`] under the root (empty: the root itself).
    pub folder: String,
}

impl Browse {
    pub fn root(root: Root) -> Browse {
        Browse {
            root,
            folder: String::new(),
        }
    }

    /// The text form the agent API takes: `ours`, `ours/forge`,
    /// `retail/game_shell/main_menu_screen`.
    pub fn path(&self) -> String {
        if self.folder.is_empty() {
            self.root.name().to_owned()
        } else {
            format!("{}/{}", self.root.name(), self.folder)
        }
    }

    /// [`Browse::path`] read back, `None` for an unknown root.
    pub fn parse(path: &str) -> Option<Browse> {
        let (root, folder) = path.split_once('/').unwrap_or((path, ""));
        Some(Browse {
            root: Root::parse(root)?,
            folder: folder.trim_matches('/').to_owned(),
        })
    }
}

/// A screen in a folder: ours by name, retail by tag name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    /// What the outliner and a tile call it inside its folder.
    pub label: String,
}

/// A folder of screens and folders.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Folder {
    /// Its path under the root, `/` between folders (empty for the root).
    pub id: String,
    pub label: String,
    pub folders: Vec<Folder>,
    pub items: Vec<Item>,
}

/// Our design-system components (`design.<component>`).
pub const DESIGN: &str = "design";
/// Our layout gallery (`layout.<name>`).
pub const LAYOUT: &str = "layout";

/// The folders that list first, in order, with their labels.
const SPECIAL: [(&str, &str); 2] = [(DESIGN, "Design system"), (LAYOUT, "Layouts")];

fn special_rank(key: &str) -> Option<usize> {
    SPECIAL.iter().position(|(k, _)| *k == key)
}

/// Our screens' folders: a screen goes in the folder of its name's first
/// dotted segment (`forge.hud` in `forge`). A name with no dot that is
/// also that segment of other names joins their folder (`skulls` with
/// `skulls.mp`); a name alone stays at the root. `design` and `layout`
/// are always folders, labelled "Design system" and "Layouts", and list
/// first; the others follow by name, then the root's own screens.
pub fn ours(names: &[&str]) -> Folder {
    let head = |n: &str| n.split('.').next().unwrap_or(n).to_owned();
    let mut groups: Vec<(String, Vec<&str>)> = Vec::new();
    for n in names {
        let h = head(n);
        match groups.iter_mut().find(|(k, _)| *k == h) {
            Some((_, v)) => v.push(n),
            None => groups.push((h, vec![n])),
        }
    }
    let mut root = Folder::default();
    for (key, members) in groups {
        let special = special_rank(&key).is_some() && members.iter().any(|n| n.contains('.'));
        if members.len() < 2 && !special {
            root.items.extend(members.iter().map(|n| Item {
                name: (*n).to_owned(),
                label: (*n).to_owned(),
            }));
            continue;
        }
        let label = SPECIAL
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(key.clone(), |(_, l)| (*l).to_owned());
        let mut items: Vec<Item> = members
            .iter()
            .map(|n| Item {
                name: (*n).to_owned(),
                label: n
                    .strip_prefix(&format!("{key}."))
                    .unwrap_or(*n)
                    .to_owned(),
            })
            .collect();
        items.sort_by(|a, b| a.name.cmp(&b.name));
        root.folders.push(Folder {
            id: key,
            label,
            folders: Vec::new(),
            items,
        });
    }
    root.folders.sort_by(|a, b| {
        let rank = |f: &Folder| special_rank(&f.id).unwrap_or(SPECIAL.len());
        rank(a).cmp(&rank(b)).then_with(|| a.id.cmp(&b.id))
    });
    root.items.sort_by(|a, b| a.name.cmp(&b.name));
    root
}

/// The retail screens' folders: each tag name split at its `\` (or `/`)
/// into folders and the screen, with the folders every tag shares
/// stripped. Returns the tree and the stripped path (`ui/screens`).
pub fn retail(tags: &[String]) -> (Folder, String) {
    let split: Vec<(Vec<&str>, &str)> = tags
        .iter()
        .map(|t| {
            let mut parts: Vec<&str> = t.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
            let leaf = parts.pop().unwrap_or(t.as_str());
            (parts, leaf)
        })
        .collect();
    let mut common = split.first().map_or(0, |(d, _)| d.len());
    if let Some((first, _)) = split.first() {
        for (dirs, _) in &split[1..] {
            common = common.min(dirs.iter().zip(first).take_while(|(a, b)| a == b).count());
        }
    }
    let stripped = split
        .first()
        .map(|(d, _)| d[..common].join("/"))
        .unwrap_or_default();
    let mut root = Folder::default();
    for (tag, (dirs, leaf)) in tags.iter().zip(&split) {
        let mut at = &mut root;
        for (i, seg) in dirs[common..].iter().enumerate() {
            let id = dirs[common..=common + i].join("/");
            let pos = match at.folders.iter().position(|f| f.id == id) {
                Some(p) => p,
                None => {
                    at.folders.push(Folder {
                        id,
                        label: (*seg).to_owned(),
                        folders: Vec::new(),
                        items: Vec::new(),
                    });
                    at.folders.len() - 1
                }
            };
            at = &mut at.folders[pos];
        }
        at.items.push(Item {
            name: tag.clone(),
            label: (*leaf).to_owned(),
        });
    }
    root.sort();
    (root, stripped)
}

impl Folder {
    fn sort(&mut self) {
        self.folders.sort_by(|a, b| a.id.cmp(&b.id));
        self.items.sort_by(|a, b| a.name.cmp(&b.name));
        for f in &mut self.folders {
            f.sort();
        }
    }

    /// The folder `id` names under this one (empty: this one).
    pub fn find(&self, id: &str) -> Option<&Folder> {
        if id.is_empty() {
            return Some(self);
        }
        self.folders.iter().find_map(|f| {
            if f.id == id {
                Some(f)
            } else if id.starts_with(&format!("{}/", f.id)) {
                f.find(id)
            } else {
                None
            }
        })
    }

    /// Every screen in it and under it, its own first.
    pub fn all_items(&self) -> Vec<&Item> {
        let mut out: Vec<&Item> = self.items.iter().collect();
        for f in &self.folders {
            out.extend(f.all_items());
        }
        out
    }

    /// Whether screen `name` is in it or under it.
    pub fn contains(&self, name: &str) -> bool {
        self.items.iter().any(|i| i.name == name) || self.folders.iter().any(|f| f.contains(name))
    }

    /// The id of the folder holding screen `name` directly.
    pub fn folder_of(&self, name: &str) -> Option<String> {
        if self.items.iter().any(|i| i.name == name) {
            return Some(self.id.clone());
        }
        self.folders.iter().find_map(|f| f.folder_of(name))
    }

    /// The same tree with only the screens `keep` accepts, and the
    /// folders that still hold one.
    pub fn filtered(&self, keep: &dyn Fn(&Item) -> bool) -> Folder {
        Folder {
            id: self.id.clone(),
            label: self.label.clone(),
            items: self.items.iter().filter(|i| keep(i)).cloned().collect(),
            folders: self
                .folders
                .iter()
                .map(|f| f.filtered(keep))
                .filter(|f| !f.is_empty())
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.folders.is_empty()
    }

    /// The count of screens in it and under it.
    pub fn count(&self) -> usize {
        self.items.len() + self.folders.iter().map(Folder::count).sum::<usize>()
    }

    /// What a grid of this folder shows: its own screens (no title), then
    /// each folder under it with its screens, titled by its path from here
    /// (`Design system`, `game_shell / main_menu`). Empty folders drop out.
    pub fn sections(&self) -> Vec<(String, Vec<&Item>)> {
        let mut out = Vec::new();
        self.sections_into("", &mut out);
        out
    }

    fn sections_into<'a>(&'a self, title: &str, out: &mut Vec<(String, Vec<&'a Item>)>) {
        if !self.items.is_empty() {
            out.push((title.to_owned(), self.items.iter().collect()));
        }
        for f in &self.folders {
            let t = if title.is_empty() {
                f.label.clone()
            } else {
                format!("{title} / {}", f.label)
            };
            f.sections_into(&t, out);
        }
    }

    /// The path to folder `id`: a (label, id) for each folder from the
    /// root's first child down to it (the root itself is not listed).
    pub fn crumbs(&self, id: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut at = self;
        let mut path = String::new();
        for seg in id.split('/').filter(|s| !s.is_empty()) {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(seg);
            match at.find(&path) {
                Some(f) => {
                    out.push((f.label.clone(), path.clone()));
                    at = f;
                }
                None => break,
            }
        }
        out
    }

    /// The tree as the agent API shows it.
    pub fn json(&self) -> Value {
        json!({
            "id": self.id,
            "label": self.label,
            "count": self.count(),
            "items": self.items.iter().map(|i| json!({"name": i.name, "label": i.label})).collect::<Vec<_>>(),
            "folders": self.folders.iter().map(Folder::json).collect::<Vec<_>>(),
        })
    }
}

/// Our screens' tree and their states, from the registry, built once.
pub struct Ours {
    pub tree: Folder,
    pub states: HashMap<String, Vec<String>>,
}

/// [`ours`] over [`super::UiScreens::tree`].
pub fn ours_tree() -> &'static Ours {
    static OURS: std::sync::OnceLock<Ours> = std::sync::OnceLock::new();
    OURS.get_or_init(|| {
        let list = super::UiScreens::tree();
        let names: Vec<&str> = list.iter().map(|(n, _)| *n).collect();
        let mut tree = ours(&names);
        tree.label = Root::Ours.title().to_owned();
        Ours {
            tree,
            states: list
                .into_iter()
                .map(|(n, s)| (n.to_owned(), s))
                .collect(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(f: &Folder) -> Vec<&str> {
        f.folders.iter().map(|f| f.id.as_str()).collect()
    }

    fn names(f: &Folder) -> Vec<&str> {
        f.items.iter().map(|i| i.name.as_str()).collect()
    }

    /// The first dotted segment is the folder; a bare name that heads
    /// others joins them; a lone name stays at the root.
    #[test]
    fn ours_groups_by_the_first_segment() {
        let t = ours(&[
            "forge.hud",
            "forge.menu",
            "skulls",
            "skulls.mp",
            "campaigns",
            "kit.dialog",
            "kit.dialog.large",
            "solo.only",
        ]);
        assert_eq!(ids(&t), ["forge", "kit", "skulls"]);
        assert_eq!(names(&t), ["campaigns", "solo.only"]);
        let skulls = t.find("skulls").unwrap();
        assert_eq!(names(skulls), ["skulls", "skulls.mp"]);
        assert_eq!(skulls.items[0].label, "skulls");
        assert_eq!(skulls.items[1].label, "mp");
        let kit = t.find("kit").unwrap();
        assert_eq!(kit.items[1].label, "dialog.large");
        assert_eq!(t.count(), 8);
    }

    /// The design system and the layouts are labelled and list first,
    /// even with one screen.
    #[test]
    fn design_and_layouts_come_first() {
        let t = ours(&[
            "forge.hud",
            "forge.menu",
            "layout.lobby",
            "design.button",
            "design.row",
            "account",
        ]);
        assert_eq!(ids(&t), ["design", "layout", "forge"]);
        assert_eq!(t.folders[0].label, "Design system");
        assert_eq!(t.folders[1].label, "Layouts");
        assert_eq!(t.folders[1].items.len(), 1);
        assert_eq!(t.folders[2].label, "forge");
        assert_eq!(names(&t), ["account"]);
        // A bare name does not make a special folder.
        assert_eq!(names(&ours(&["design", "a"])), ["a", "design"]);
    }

    #[test]
    fn retail_nests_by_tag_path_and_strips_the_common_root() {
        let tags: Vec<String> = [
            r"ui\screens\game_shell\main_menu\main_menu",
            r"ui\screens\game_shell\settings\settings",
            r"ui\screens\game_shell\settings\audio",
            r"ui\screens\singleplayer\dialog_pause\dialog_pause",
            r"ui\screens\loose",
        ]
        .map(String::from)
        .to_vec();
        let (t, stripped) = retail(&tags);
        assert_eq!(stripped, "ui/screens");
        assert_eq!(ids(&t), ["game_shell", "singleplayer"]);
        assert_eq!(names(&t), [r"ui\screens\loose"]);
        let shell = t.find("game_shell").unwrap();
        assert_eq!(ids(shell), ["game_shell/main_menu", "game_shell/settings"]);
        let settings = t.find("game_shell/settings").unwrap();
        assert_eq!(settings.label, "settings");
        assert_eq!(
            settings.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(),
            ["audio", "settings"]
        );
        assert_eq!(t.count(), 5);
    }

    #[test]
    fn retail_strips_only_what_every_tag_shares() {
        let tags: Vec<String> = [r"ui\screens\a\x", r"ui\screens\b\y", r"ui\other\z"]
            .map(String::from)
            .to_vec();
        let (t, stripped) = retail(&tags);
        assert_eq!(stripped, "ui");
        assert_eq!(ids(&t), ["other", "screens"]);
        assert_eq!(ids(t.find("screens").unwrap()), ["screens/a", "screens/b"]);
        // One tag: its folders are all common, the screen sits at the root.
        let (one, stripped) = retail(&[r"ui\screens\a\x".to_owned()]);
        assert_eq!(stripped, "ui/screens/a");
        assert_eq!(names(&one), [r"ui\screens\a\x"]);
        assert!(retail(&[]).0.is_empty());
    }

    /// Everything under a folder, in sections titled by their path.
    #[test]
    fn sections_list_the_folder_recursively() {
        let tags: Vec<String> = [r"r\a\x", r"r\a\b\y", r"r\a\b\c\z", r"r\q\w"]
            .map(String::from)
            .to_vec();
        let (t, _) = retail(&tags);
        let all = t.sections();
        let shape: Vec<(&str, Vec<&str>)> = all
            .iter()
            .map(|(title, items)| {
                (title.as_str(), items.iter().map(|i| i.label.as_str()).collect())
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("a", vec!["x"]),
                ("a / b", vec!["y"]),
                ("a / b / c", vec!["z"]),
                ("q", vec!["w"])
            ]
        );
        let b = t.find("a/b").unwrap().sections();
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].0, "");
        assert_eq!(b[1].0, "c");
        assert_eq!(t.find("a").unwrap().all_items().len(), 3);
    }

    #[test]
    fn filtering_keeps_the_path_to_a_match() {
        let tags: Vec<String> = [r"r\a\x", r"r\a\b\y", r"r\q\w"].map(String::from).to_vec();
        let (t, _) = retail(&tags);
        let f = t.filtered(&|i| i.label == "y");
        assert_eq!(f.count(), 1);
        assert_eq!(ids(&f), ["a"]);
        assert_eq!(ids(f.find("a").unwrap()), ["a/b"]);
        assert!(t.filtered(&|_| false).is_empty());
        assert_eq!(t.folder_of(r"r\a\b\y").as_deref(), Some("a/b"));
        assert_eq!(t.folder_of(r"r\a\x").as_deref(), Some("a"));
        assert!(t.contains(r"r\q\w") && !t.contains("nope"));
    }

    #[test]
    fn crumbs_name_each_folder_on_the_way() {
        let t = ours(&["design.a", "design.b"]);
        assert_eq!(
            t.crumbs("design"),
            [("Design system".to_owned(), "design".to_owned())]
        );
        let tags: Vec<String> = [r"r\a\b\y", r"r\q\w"].map(String::from).to_vec();
        let (t, _) = retail(&tags);
        assert_eq!(
            t.crumbs("a/b"),
            [
                ("a".to_owned(), "a".to_owned()),
                ("b".to_owned(), "a/b".to_owned())
            ]
        );
        assert!(t.crumbs("").is_empty());
    }

    #[test]
    fn a_browse_target_round_trips_its_path() {
        let b = Browse::parse("retail/game_shell/settings").unwrap();
        assert_eq!(b.root, Root::Retail);
        assert_eq!(b.folder, "game_shell/settings");
        assert_eq!(b.path(), "retail/game_shell/settings");
        assert_eq!(Browse::parse("ours"), Some(Browse::root(Root::Ours)));
        assert_eq!(Browse::root(Root::Ours).path(), "ours");
        assert_eq!(Browse::parse("mine/x"), None);
    }

    /// The registry's own tree holds every screen once.
    #[test]
    fn the_registry_tree_has_every_screen() {
        let o = ours_tree();
        assert_eq!(o.tree.count(), boot::ui_kit::screens::NAMES.len());
        for n in boot::ui_kit::screens::NAMES {
            assert!(o.tree.contains(n), "{n}");
            assert!(o.states.contains_key(n));
        }
        assert_eq!(o.tree.label, "Our screens");
    }
}
