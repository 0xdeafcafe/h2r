//! The browser's rows (docs/editor/studio-redesign.md, "The browser"):
//! one tree of everything Studio can open, read from the tags tree
//! (`tags::repo::TagRepo`) and the package set (`workspace::GameSet`),
//! and what `asset.list`, `asset.thumb` and `asset.copy` answer. The
//! window's browser and the API read the same rows.
//!
//! An asset is named by an id: `tag:<group>:<path>` for a tag of the tree
//! or one a package adds, `package:<id>` for a package, and
//! `item:<package>:<n>` for the n-th item of a package's manifest.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ::workspace::{GameSet, Loaded, added_tag, parse_edits, scope_of};
use content::package::{
    Item, Kind as ItemKind, LOCK_FILE, MANIFEST_FILE, Manifest, MapScope, Maps,
};
use serde_json::{Value, json};
use tags::defs::{fourcc_text, parse_fourcc};
use tags::repo::{TagKey, TagRepo};

use super::kinds::{self, Editor};
use super::shell::Doc;
use super::{App, tag_api};

/// The kinds the Kind picker names, and the groups of each. A group in
/// none of them is `Other`; any four-character group is also a kind.
const KINDS: &[(&str, &[&str])] = &[
    ("Maps", &["scnr"]),
    (
        "Objects",
        &[
            "weap", "vehi", "bipd", "scen", "eqip", "proj", "mach", "ctrl", "lifi", "ssce", "crea",
            "garb", "obje", "unit", "item", "devi",
        ],
    ),
    ("Models", &["hlmt", "mode", "coll", "phmo"]),
    ("Animations", &["jmad", "antr"]),
    ("Bitmaps", &["bitm"]),
    ("Shaders", &["shad", "stem", "spas"]),
    ("Effects", &["effe", "prt3", "cntl", "ltvl", "decs", "lens"]),
    ("Sounds", &["snd!", "lsnd", "udlg", "sndg"]),
    ("Screens", &["wgit", "wgtz", "wigl"]),
    ("HUDs", &["nhdt", "grhi", "hudg", "hud#"]),
    ("Strings", &["unic", "ustr", "str#"]),
];

/// The kind a group belongs to.
pub fn kind_of(group: &str) -> &'static str {
    if group.starts_with("rm") && group.len() == 4 {
        return "Shaders";
    }
    KINDS
        .iter()
        .find(|(_, groups)| groups.contains(&group))
        .map_or("Other", |(k, _)| k)
}

/// The Kind picker's choices.
pub fn kind_names() -> Vec<&'static str> {
    KINDS.iter().map(|(k, _)| *k).chain(["Other"]).collect()
}

/// Which root a list reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Root {
    Tags,
    Packages,
    Live,
}

impl Root {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "tags" => Some(Self::Tags),
            "packages" => Some(Self::Packages),
            "live" => Some(Self::Live),
            _ => None,
        }
    }
}

/// A list's parameters.
#[derive(Clone, Debug)]
pub struct Query {
    pub root: Root,
    pub package: Option<String>,
    pub kind: Option<String>,
    pub map: Option<String>,
    pub text: String,
    pub limit: usize,
}

/// An asset id, parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetId {
    Tag(TagKey),
    Package(String),
    Item(String, usize),
}

impl AssetId {
    pub fn parse(s: &str) -> Result<Self, String> {
        let bad = || {
            format!(
                "{s:?} is not an asset id (tag:<group>:<path>, package:<id> or item:<package>:<n>)"
            )
        };
        let (kind, rest) = s.split_once(':').ok_or_else(bad)?;
        match kind {
            "tag" => {
                let (g, path) = rest.split_once(':').ok_or_else(bad)?;
                let group = parse_fourcc(g).ok_or_else(bad)?;
                if path.is_empty() {
                    return Err(bad());
                }
                Ok(Self::Tag(TagKey::new(group, path.replace('/', "\\"))))
            }
            "package" if !rest.is_empty() => Ok(Self::Package(rest.to_owned())),
            "item" => {
                let (p, n) = rest.rsplit_once(':').ok_or_else(bad)?;
                Ok(Self::Item(p.to_owned(), n.parse().map_err(|_| bad())?))
            }
            _ => Err(bad()),
        }
    }
}

/// The id of a tag.
pub fn tag_id(key: &TagKey) -> String {
    format!("tag:{}:{}", fourcc_text(key.group), key.path)
}

/// What the packages do to the tree.
#[derive(Default)]
pub(super) struct Overlay {
    /// A tag a package adds: the package and where.
    pub(super) added: BTreeMap<TagKey, Vec<(String, Maps)>>,
    /// A tag an edit line touches: the packages, in load order.
    pub(super) edited: BTreeMap<TagKey, Vec<(String, Maps)>>,
    /// Edit files that did not read.
    pub(super) problems: Vec<String>,
}

fn group_ref(text: &str) -> Option<TagKey> {
    let (g, path) = text.split_once(':')?;
    Some(TagKey::new(parse_fourcc(g)?, path.replace('/', "\\")))
}

pub(super) fn overlay(set: &GameSet) -> Overlay {
    let mut out = Overlay::default();
    for e in &set.on {
        let m = &e.loaded.package.manifest;
        let id = m.package.id.clone();
        for item in &m.items {
            match item.kind {
                ItemKind::Tag | ItemKind::Bitmap => {
                    if let Some(key) = added_tag(item) {
                        out.added
                            .entry(key)
                            .or_default()
                            .push((id.clone(), item.maps.clone()));
                    }
                }
                ItemKind::Edits => {
                    let Some(source) = item.source.as_deref() else {
                        continue;
                    };
                    let text = e
                        .loaded
                        .files
                        .get(source)
                        .map(|b| String::from_utf8_lossy(b).into_owned());
                    let Some(text) = text else {
                        out.problems
                            .push(format!("{id}: {source}: not in the package"));
                        continue;
                    };
                    match parse_edits(&text) {
                        Ok(edits) => {
                            for (edit, _) in edits {
                                match group_ref(&edit.tag) {
                                    Some(key) => out
                                        .edited
                                        .entry(key)
                                        .or_default()
                                        .push((id.clone(), item.maps.clone())),
                                    None => out.problems.push(format!(
                                        "{id}: {source}: {:?} is not <group>:<path>",
                                        edit.tag
                                    )),
                                }
                            }
                        }
                        Err(f) => out.problems.push(format!("{id}: {source}: {}", f.why)),
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Whether an item's `maps` reaches a map of this name and class.
fn applies(maps: &Maps, map: &str, scope: Option<MapScope>) -> bool {
    match maps {
        Maps::Scope(MapScope::All) => true,
        Maps::Scope(s) => Some(*s) == scope,
        Maps::Names(n) => n.iter().any(|n| n.eq_ignore_ascii_case(map)),
    }
}

/// The set the browser reads: the baked packages and the folders under
/// the data root.
pub fn read_set(data_root: &Path) -> GameSet {
    let (baked, _) = boot::packages::baked();
    GameSet::read(baked, data_root, &|_| true)
}

/// One tag of the tree: the maps holding each version of it.
type Versions = BTreeMap<Option<String>, BTreeSet<String>>;

/// Every tag of the tree by key, with the maps of each version. A tag all
/// the maps that hold it hold alike has one version, `None`.
fn tree(root: &Path) -> Result<BTreeMap<TagKey, Versions>, String> {
    let repo = TagRepo::open(root).map_err(|e| {
        format!(
            "no tags tree at {}: {e} (workspace.extract builds it)",
            root.display()
        )
    })?;
    let mut out: BTreeMap<TagKey, Versions> = BTreeMap::new();
    for map in repo.maps().map_err(|e| e.to_string())? {
        let m = repo.manifest(&map).map_err(|e| e.to_string())?;
        for (key, variant) in m.tags {
            out.entry(key)
                .or_default()
                .entry(variant)
                .or_default()
                .insert(map.clone());
        }
    }
    Ok(out)
}

/// A map's tags and class, read from its manifest.
fn map_tags(root: &Path, map: &str) -> Result<(BTreeSet<TagKey>, MapScope), String> {
    let repo = TagRepo::open(root).map_err(|e| e.to_string())?;
    let m = repo
        .manifest(map)
        .map_err(|_| format!("{map:?} is not a map of the tree"))?;
    let scope = scope_of(m.cache_type());
    Ok((m.tags.into_iter().map(|(k, _)| k).collect(), scope))
}

fn badge(o: &Overlay, key: &TagKey) -> (Value, Value) {
    let by = |l: &[(String, Maps)]| -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for (id, _) in l {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        ids
    };
    if let Some(l) = o.added.get(key) {
        let ids = by(l);
        return (json!(format!("new in {}", ids.join(", "))), json!(ids));
    }
    if let Some(l) = o.edited.get(key) {
        let ids = by(l);
        return (json!(format!("edited by {}", ids.join(", "))), json!(ids));
    }
    (Value::Null, json!([]))
}

fn words_match(words: &[String], hay: &str) -> bool {
    words.iter().all(|w| hay.contains(w.as_str()))
}

fn tag_row(key: &TagKey, versions: Option<&Versions>, o: &Overlay) -> Value {
    let group = fourcc_text(key.group);
    let name = key.path.rsplit('\\').next().unwrap_or(&key.path).to_owned();
    let (badge, packages) = badge(o, key);
    let maps: BTreeSet<&String> = versions
        .into_iter()
        .flat_map(|v| v.values().flatten())
        .collect();
    let variants: Vec<Value> = match versions {
        Some(v) if v.len() > 1 => v
            .iter()
            .map(|(id, maps)| json!({ "variant": id, "maps": maps }))
            .collect(),
        _ => Vec::new(),
    };
    json!({
        "asset": tag_id(key),
        "root": "tags",
        "kind": kind_of(&group),
        "group": group,
        "path": key.path,
        "name": name,
        "origin": if versions.is_some() { "disc" } else { "package" },
        "new": o.added.contains_key(key),
        "badge": badge,
        "packages": packages,
        "maps": maps,
        "variants": variants,
    })
}

fn package_row(l: &::workspace::Listed) -> Value {
    json!({
        "asset": format!("package:{}", l.id),
        "root": "packages",
        "kind": "Package",
        "name": l.name,
        "id": l.id,
        "package_id": l.package_id,
        "origin": l.origin,
        "on": l.on,
        "why": l.why,
        "items": l.items,
        "scopes": l.scopes,
    })
}

fn item_row(package: &str, n: usize, item: &Item) -> Value {
    let kind = serde_json::to_value(item.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    json!({
        "asset": format!("item:{package}:{n}"),
        "root": "packages",
        "kind": kind,
        "name": item.source.clone().or_else(|| item.tag.clone()).unwrap_or_default(),
        "package": package,
        "source": item.source,
        "tag": item.tag,
        "maps": serde_json::to_value(&item.maps).unwrap_or(Value::Null),
    })
}

/// `asset.list`.
pub fn list(tags_root: &Path, set: &GameSet, q: &Query) -> Result<Value, String> {
    let words: Vec<String> = q
        .text
        .split_whitespace()
        .map(|w| w.to_lowercase().replace('/', "\\"))
        .collect();
    let mut rows: Vec<Value> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    let mut maps: Vec<String> = Vec::new();
    match q.root {
        Root::Live => {
            // The server's offer is read over the network; no list is held.
            return Ok(json!({
                "root": "live", "rows": [], "total": 0, "limit": q.limit,
                "truncated": false, "maps": [], "kinds": kind_names(),
                "problems": [], "offline": true,
            }));
        }
        Root::Packages => match &q.package {
            None => {
                for l in &set.listed {
                    let hay = format!("{} {} {}", l.id, l.name, l.origin).to_lowercase();
                    if words_match(&words, &hay) {
                        rows.push(package_row(l));
                    }
                }
            }
            Some(id) => {
                let e = set
                    .on
                    .iter()
                    .find(|e| e.loaded.package.name() == id)
                    .ok_or_else(|| format!("no package {id:?} is on (root packages lists them)"))?;
                for (n, item) in e.loaded.package.manifest.items.iter().enumerate() {
                    let row = item_row(id, n, item);
                    let hay = format!(
                        "{} {} {}",
                        row["kind"].as_str().unwrap_or(""),
                        item.source.as_deref().unwrap_or(""),
                        item.tag.as_deref().unwrap_or("")
                    )
                    .to_lowercase()
                    .replace('/', "\\");
                    if words_match(&words, &hay) {
                        rows.push(row);
                    }
                }
            }
        },
        Root::Tags => {
            let o = overlay(set);
            problems = o.problems.clone();
            let all = tree(tags_root)?;
            maps = TagRepo::open(tags_root)
                .and_then(|r| r.maps())
                .map_err(|e| e.to_string())?;
            let in_map = match &q.map {
                Some(m) => Some((m.clone(), map_tags(tags_root, m)?)),
                None => None,
            };
            let kind = q.kind.as_deref().map(str::to_lowercase);
            let mut keys: BTreeSet<&TagKey> = all.keys().collect();
            keys.extend(o.added.keys());
            for key in keys {
                let versions = all.get(key);
                let group = fourcc_text(key.group);
                if let Some(k) = &kind {
                    let ok = kind_of(&group).to_lowercase() == *k
                        || group.trim().eq_ignore_ascii_case(k.trim());
                    if !ok {
                        continue;
                    }
                }
                if let Some((name, (tags, scope))) = &in_map {
                    let there = tags.contains(key)
                        || (versions.is_none()
                            && o.added.get(key).is_some_and(|l| {
                                l.iter().any(|(_, m)| applies(m, name, Some(*scope)))
                            }));
                    if !there {
                        continue;
                    }
                }
                if let Some(pkg) = &q.package {
                    let touched = |m: &BTreeMap<TagKey, Vec<(String, Maps)>>| {
                        m.get(key).is_some_and(|l| l.iter().any(|(p, _)| p == pkg))
                    };
                    if !touched(&o.added) && !touched(&o.edited) {
                        continue;
                    }
                }
                let row = tag_row(key, versions, &o);
                let hay = format!(
                    "{} {} {} {}",
                    key.path,
                    group,
                    row["kind"].as_str().unwrap_or(""),
                    row["packages"]
                        .as_array()
                        .map(|a| a
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" "))
                        .unwrap_or_default()
                )
                .to_lowercase();
                if words_match(&words, &hay) {
                    rows.push(row);
                }
            }
        }
    }
    let total = rows.len();
    rows.truncate(q.limit);
    Ok(json!({
        "root": match q.root { Root::Tags => "tags", Root::Packages => "packages", Root::Live => "live" },
        "rows": rows,
        "total": total,
        "limit": q.limit,
        "truncated": total > q.limit,
        "maps": maps,
        "kinds": kind_names(),
        "problems": problems,
    }))
}

/// Why `asset.thumb` has no picture for an asset.
#[derive(Debug)]
pub enum ThumbRefusal {
    /// The asset is not one the call draws yet.
    NoPicture(String),
    Failed(String),
}

fn find<'a>(set: &'a GameSet, id: &str) -> Result<&'a Loaded, String> {
    set.on
        .iter()
        .map(|e| &e.loaded)
        .find(|l| l.package.name() == id)
        .ok_or_else(|| format!("no package {id:?} is on"))
}

/// `asset.thumb`: a PNG source of a package's bitmap item, written to
/// `out`. A tag's own picture (a bitmap, a model, a waveform, a screen)
/// comes from the renderers the browser reads when each draws it.
pub fn thumb(set: &GameSet, id: &AssetId, out: &Path) -> Result<Value, ThumbRefusal> {
    use ThumbRefusal::{Failed, NoPicture};
    let AssetId::Item(pkg, n) = id else {
        return Err(NoPicture(format!(
            "{id:?}: only a package's bitmap item has a thumbnail so far"
        )));
    };
    let loaded = find(set, pkg).map_err(Failed)?;
    let item = loaded
        .package
        .manifest
        .items
        .get(*n)
        .ok_or_else(|| Failed(format!("package {pkg:?} has no item {n}")))?;
    let source = item.source.as_deref().unwrap_or("");
    let bytes = loaded.files.get(source);
    match (item.kind, bytes) {
        (ItemKind::Bitmap, Some(b)) if b.starts_with(&[0x89, b'P', b'N', b'G']) => {
            if let Some(d) = out.parent() {
                std::fs::create_dir_all(d).map_err(|e| Failed(format!("{}: {e}", d.display())))?;
            }
            std::fs::write(out, b).map_err(|e| Failed(format!("{}: {e}", out.display())))?;
            Ok(json!({
                "asset": format!("item:{pkg}:{n}"),
                "path": out.display().to_string(),
                "bytes": b.len(),
                "source": source,
            }))
        }
        _ => Err(NoPicture(format!(
            "item {n} of {pkg:?} ({:?}) is not a PNG bitmap source",
            item.kind
        ))),
    }
}

/// Why a copy is refused.
#[derive(Debug)]
pub enum CopyRefusal {
    /// A parameter is wrong.
    Params(String),
    /// The package or the tag cannot take it.
    Refused(String),
    /// A file did not write.
    Failed(String),
}

/// `asset.copy`: a new `tag` item in a local package, `like:` the tag, so
/// the package holds none of the source's bytes. The new path is `to`, or
/// the source's path with `_copy` added.
pub fn copy(
    tags_root: &Path,
    set: &GameSet,
    id: &AssetId,
    package: &str,
    to: Option<&str>,
) -> Result<Value, CopyRefusal> {
    use CopyRefusal::{Failed, Params, Refused};
    let AssetId::Tag(key) = id else {
        return Err(Params("asset.copy copies a tag: tag:<group>:<path>".into()));
    };
    let dir = set
        .listed
        .iter()
        .find(|l| l.id == package)
        .ok_or_else(|| Params(format!("no package {package:?}")))
        .and_then(|l| match &l.dir {
            Some(d) => Ok(PathBuf::from(d)),
            None => Err(Refused(format!(
                "package {package:?} is built in: copy into a package under <data>/packages/"
            ))),
        })?;
    let repo = TagRepo::open(tags_root).map_err(|e| Refused(format!("no tags tree: {e}")))?;
    if !repo.path_of(key, None).exists() {
        return Err(Refused(format!(
            "{} is not in the tags tree, so there is nothing to start from",
            tag_id(key)
        )));
    }
    let new_path = to
        .map(|t| t.replace('/', "\\"))
        .unwrap_or_else(|| format!("{}_copy", key.path));
    if new_path.trim().is_empty()
        || new_path.contains("..")
        || new_path.starts_with('\\')
        || new_path.contains('\0')
    {
        return Err(Params(format!("{new_path:?} is not a tag path")));
    }
    let group = fourcc_text(key.group);
    if repo
        .path_of(&TagKey::new(key.group, new_path.clone()), None)
        .exists()
    {
        return Err(Refused(format!(
            "{group}:{new_path} is a tag of the tree already: pass `to` for another path"
        )));
    }
    let manifest_path = dir.join(MANIFEST_FILE);
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| Failed(format!("{}: {e}", manifest_path.display())))?;
    let mut manifest = Manifest::from_toml(&text)
        .map_err(|e| Failed(format!("{}: {e}", manifest_path.display())))?;
    let new_key = TagKey::new(key.group, new_path.clone());
    if manifest
        .items
        .iter()
        .any(|i| added_tag(i).as_ref() == Some(&new_key))
    {
        return Err(Refused(format!(
            "package {package:?} adds {group}:{new_path} already"
        )));
    }
    let source = format!("tags/{}.{group}.yaml", new_path.replace('\\', "/"));
    let file = dir.join(&source);
    if file.exists() {
        return Err(Refused(format!("{} exists", file.display())));
    }
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d).map_err(|e| Failed(format!("{}: {e}", d.display())))?;
    }
    let like = format!("{group}:{}", key.path).replace('\'', "''");
    std::fs::write(&file, format!("like: '{like}'\n"))
        .map_err(|e| Failed(format!("{}: {e}", file.display())))?;
    manifest.items.push(Item {
        kind: ItemKind::Tag,
        inputs: Vec::new(),
        source: Some(source.clone()),
        tag: Some(new_path.clone()),
        maps: Maps::default(),
        replaces: None,
        was: None,
    });
    let undo = |e: String| {
        let _ = std::fs::remove_file(&file);
        Failed(e)
    };
    manifest.validate().map_err(|e| undo(e.to_string()))?;
    std::fs::write(&manifest_path, manifest.to_toml())
        .map_err(|e| undo(format!("{}: {e}", manifest_path.display())))?;
    // The lock follows the files, as `package.check` expects.
    let loaded = Loaded::from_dir(&dir).map_err(|e| Failed(e.to_string()))?;
    std::fs::write(dir.join(LOCK_FILE), loaded.package.lock.to_toml())
        .map_err(|e| Failed(format!("{LOCK_FILE}: {e}")))?;
    Ok(json!({
        "package": package,
        "from": tag_id(key),
        "tag": format!("tag:{group}:{new_path}"),
        "source": source,
        "item": manifest.items.len() - 1,
        "dir": dir.display().to_string(),
    }))
}

impl App {
    /// `asset.copy`, and the browser's **Copy into package**: the one call
    /// both make.
    pub fn asset_copy(
        &mut self,
        id: &AssetId,
        package: &str,
        to: Option<&str>,
    ) -> Result<Value, CopyRefusal> {
        let set = read_set(&self.data_root());
        copy(&self.tags_root(), &set, id, package, to)
    }

    /// [`super::Action::CopyAsset`].
    pub(super) fn copy_asset(&mut self, asset: &str, package: &str) {
        let id = match AssetId::parse(asset) {
            Ok(id) => id,
            Err(e) => return self.error(e),
        };
        match self.asset_copy(&id, package, None) {
            Ok(v) => self.info(format!(
                "copied {asset} into {package} as {}",
                v["tag"].as_str().unwrap_or("a new tag")
            )),
            Err(CopyRefusal::Params(m) | CopyRefusal::Refused(m) | CopyRefusal::Failed(m)) => {
                self.error(m)
            }
        }
    }

    /// [`super::Action::OpenAsset`]: open the map that holds the tag (a
    /// Halo 1 map shows read-only, with no document), then the tag in the
    /// editor its group gets. A scenario tag is the map's own tab.
    pub(super) fn open_asset(&mut self, asset: &str, map: Option<&str>) {
        let key = match AssetId::parse(asset) {
            Ok(AssetId::Tag(key)) => key,
            Ok(_) => return self.error(format!("{asset} is not a tag: only a tag opens in an editor")),
            Err(e) => return self.error(e),
        };
        if let Some(name) = map {
            let in_list = |entries: Vec<super::map_cards::MapEntry<'_>>| {
                entries.iter().any(|e| e.stem.eq_ignore_ascii_case(name))
            };
            let halo1 = in_list(self.halo1_map_entries()) && !in_list(self.halo2_map_entries());
            if halo1 {
                if self.loaded_halo1() != Some(name) {
                    self.select_halo1_map(name);
                }
                return;
            }
            if self.loaded.as_deref() != Some(name)
                || self.opened_package.is_some()
                || self.browsing().is_some()
            {
                self.select_map(name);
            }
        }
        if kinds::kind(key.group).editor == Editor::Scenario {
            return self.open_doc(Doc::Map);
        }
        if self.browsing().is_some() {
            return self.error("a browsed map's tags are read-only: open the map to open its tags");
        }
        self.sync_tags();
        let text = format!("{}:{}", fourcc_text(key.group), key.path);
        match tag_api::find_tag(&self.tags, &text) {
            Some(id) => self.open_doc(Doc::Tag(id)),
            None => self.error(format!("{text} is not in the open map")),
        }
    }
}
