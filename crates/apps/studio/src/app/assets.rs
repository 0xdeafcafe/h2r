//! The browser's rows (docs/editor/studio-redesign.md, "The browser"):
//! one tree of everything Studio can open, read from the tags tree
//! (`tags::repo::TagRepo`) and the package set (`workspace::GameSet`),
//! and what `asset.list`, `asset.thumb` and `asset.copy` answer. The
//! window's browser and the API read the same rows.
//!
//! A tag's **In maps** and **Where used** come from the store's asset
//! catalogue (`tags::repo::catalogue`, docs/engine/asset-store.md, "The
//! catalogue") when the store has a current one: a tag's contents are its
//! assets by hash, so two files that differ only in what ties them to
//! their map are one content, and `used` names the keys that reference it.
//! Without one, or with a stale one (a line in `problems`), the rows are
//! read from the manifests and a content is a file.
//!
//! An asset is named by an id: `tag:<group>:<path>` for a tag of the tree
//! or one a package adds, `package:<id>` for a package, and
//! `item:<package>:<n>` for the n-th item of a package's manifest, and
//! `voice:<id>` for a voice (a character of `voice::dataset::CHARACTERS`,
//! or `announcer`), which the **Voices** kind lists
//! ([`App::voice_asset_list`]), and `forge:<map>:<file>` for a Forge map
//! variant, which the **Forge variants** kind lists
//! (`app::forge_variants`). The **Forge items** kind lists the Forge
//! palette as tags (`assets/forge.rs`).
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use ::workspace::{GameSet, Loaded, added_tag, group_of_source, parse_edits, scope_of};
use content::package::{
    Item, Kind as ItemKind, LOCK_FILE, MANIFEST_FILE, Manifest, MapScope, Maps,
};
use serde_json::{Value, json};
use tags::defs::{fourcc_text, parse_fourcc};
use tags::repo::catalogue::{self, Catalogue};
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
    // Package item kinds with no tag groups.
    ("Game types", &[]),
    ("Playlists", &[]),
];

/// The kind of the voices (`voice:<id>` rows): not a tag group, so it is
/// listed by [`App::voice_asset_list`] rather than from the tree.
pub const VOICES_KIND: &str = "Voices";

/// The Kind is the Voices kind.
pub fn is_voices_kind(kind: Option<&str>) -> bool {
    kind.is_some_and(|k| k.trim().eq_ignore_ascii_case(VOICES_KIND))
}

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
    KINDS
        .iter()
        .map(|(k, _)| *k)
        .chain([
            "Other",
            VOICES_KIND,
            super::forge_variants::VARIANTS_KIND,
            super::forge_variants::ITEMS_KIND,
        ])
        .collect()
}

/// The kind a package item lists under: the kind of the tag it adds (a
/// `tag`, `bitmap`, `model`, `animation` or `sound` item names its group
/// in its source or `tag`), the kind of what it edits when it says one,
/// and the item kinds' own (**Game types** for a `rules` or `variant`
/// script, **Playlists** for a `playlist`). An item of no other kind is
/// `Other`.
pub fn item_kind(item: &Item) -> &'static str {
    match item.kind {
        ItemKind::Tag => item
            .source
            .as_deref()
            .and_then(|s| group_of_source(s).ok())
            .map(|g| kind_of(&fourcc_text(g)))
            .unwrap_or("Other"),
        ItemKind::Bitmap => "Bitmaps",
        ItemKind::Model => "Models",
        ItemKind::Animation => "Animations",
        ItemKind::Sound => "Sounds",
        ItemKind::Screens | ItemKind::ScreenEdits => "Screens",
        ItemKind::Hud => "HUDs",
        ItemKind::Strings => "Strings",
        ItemKind::Rules | ItemKind::Variant => "Game types",
        ItemKind::Playlist => "Playlists",
        _ => "Other",
    }
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
    /// A voice: a character id or `announcer`.
    Voice(String),
    /// A Forge map variant: its base map and its file's stem in the store
    /// (`app::forge_variants`).
    Forge(String, String),
}

impl AssetId {
    pub fn parse(s: &str) -> Result<Self, String> {
        let bad = || {
            format!(
                "{s:?} is not an asset id (tag:<group>:<path>, package:<id>, item:<package>:<n>, voice:<id> or forge:<map>:<file>)"
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
            "voice" if crate::voice::dataset::character(rest).is_some() || rest == super::voices::ANNOUNCER => {
                Ok(Self::Voice(rest.to_owned()))
            }
            "forge" => {
                let (map, file) = rest.split_once(':').ok_or_else(bad)?;
                if map.is_empty() || file.is_empty() {
                    return Err(bad());
                }
                Ok(Self::Forge(map.to_owned(), file.to_owned()))
            }
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

pub(super) fn group_ref(text: &str) -> Option<TagKey> {
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
                ItemKind::Tag | ItemKind::Bitmap | ItemKind::Sound | ItemKind::Model => {
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

/// The catalogue read for the last list, with its file's size and
/// modification time: a list reads it again only when the file changed.
static KEPT: Mutex<Option<(PathBuf, u64, std::time::SystemTime, Arc<Catalogue>)>> =
    Mutex::new(None);

/// The store's asset catalogue at `root`: `None` when it has none.
fn catalogue_of(root: &Path) -> Result<Option<Arc<Catalogue>>, String> {
    let path = root.join(catalogue::CATALOGUE_FILE);
    let Ok(md) = std::fs::metadata(&path) else {
        return Ok(None);
    };
    let when = md.modified().unwrap_or(UNIX_EPOCH);
    if let Ok(kept) = KEPT.lock()
        && let Some((p, len, at, c)) = kept.as_ref()
        && *p == path
        && *len == md.len()
        && *at == when
    {
        return Ok(Some(c.clone()));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let c = Arc::new(Catalogue::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?);
    if let Ok(mut kept) = KEPT.lock() {
        *kept = Some((path, md.len(), when, c.clone()));
    }
    Ok(Some(c))
}

/// The catalogue of the tree at `root` for a list: `None`, with a line
/// for `problems`, when it is stale (a map was extracted again and the
/// catalogue was not rebuilt).
fn current_catalogue(root: &Path) -> (Option<Arc<Catalogue>>, Option<String>) {
    match catalogue_of(root) {
        Ok(Some(c)) if c.is_current(root).unwrap_or(false) => (Some(c), None),
        Ok(Some(_)) => (
            None,
            Some(format!(
                "{} is stale; workspace.extract rewrites it",
                root.join(catalogue::CATALOGUE_FILE).display()
            )),
        ),
        Ok(None) => (None, None),
        Err(e) => (None, Some(e)),
    }
}

/// A key as the rows name one: `<group>:<path>`.
fn key_text(k: &TagKey) -> String {
    format!("{}:{}", fourcc_text(k.group), k.path)
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

/// The Browser's origin label for a tag, shared with the document header.
pub(crate) fn badge_for(set: &GameSet, key: &TagKey) -> String {
    badge(&overlay(set), key).0.as_str().unwrap_or("on the disc").to_owned()
}

fn words_match(words: &[String], hay: &str) -> bool {
    words.iter().all(|w| hay.contains(w.as_str()))
}

/// A tag's row. `versions` are its contents as the tree keeps the files,
/// `catalogue` the store's catalogue, whose assets are the contents then.
fn tag_row(
    key: &TagKey,
    versions: Option<&Versions>,
    o: &Overlay,
    catalogue: Option<&Catalogue>,
) -> Value {
    let group = fourcc_text(key.group);
    let name = key.path.rsplit('\\').next().unwrap_or(&key.path).to_owned();
    let (badge, packages) = badge(o, key);
    // A tag the catalogue could not hash has no asset: its files are its
    // contents, as without a catalogue.
    let assets = catalogue.map(|c| c.of_key(key)).filter(|a| !a.is_empty());
    let maps: BTreeSet<&String> = match assets {
        Some(assets) => assets.iter().flat_map(|a| a.maps.iter()).collect(),
        None => versions
            .into_iter()
            .flat_map(|v| v.values().flatten())
            .collect(),
    };
    let variants: Vec<Value> = match assets {
        // One entry per asset: the files the maps read it from.
        Some(assets) if assets.len() > 1 => assets
            .iter()
            .map(|a| {
                json!({
                    "hash": a.hash,
                    "variant": a.files.iter().flatten().next(),
                    "files": a.files.iter().map(|f| f.as_deref()).collect::<Vec<_>>(),
                    "maps": a.maps,
                })
            })
            .collect(),
        None => match versions {
            Some(v) if v.len() > 1 => v
                .iter()
                .map(|(id, maps)| json!({ "variant": id, "maps": maps }))
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    let used: Value = catalogue
        .and_then(|c| c.users(key))
        .map(|u| json!(u.iter().map(key_text).collect::<Vec<_>>()))
        .unwrap_or(Value::Null);
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
        "hash": assets.filter(|a| a.len() == 1).map(|a| a[0].hash.as_str()),
        "maps": maps,
        "variants": variants,
        "used": used,
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

/// The kind an `edits` item lists under: the kind of the first tag its
/// file names. One parse per item of the listing, not per row.
fn edited_kinds(entry: &Loaded) -> BTreeMap<usize, &'static str> {
    let mut out = BTreeMap::new();
    for (n, item) in entry.package.manifest.items.iter().enumerate() {
        if item.kind != ItemKind::Edits {
            continue;
        }
        let Some(source) = item.source.as_deref() else {
            continue;
        };
        let Some(text) = entry
            .files
            .get(source)
            .map(|b| String::from_utf8_lossy(b).into_owned())
        else {
            continue;
        };
        let Ok(edits) = parse_edits(&text) else {
            continue;
        };
        for (edit, _) in edits {
            if let Some(key) = group_ref(&edit.tag) {
                out.insert(n, kind_of(&fourcc_text(key.group)));
                break;
            }
        }
    }
    out
}

fn item_row(package: &str, n: usize, item: &Item, kind: Option<&'static str>) -> Value {
    let kind = kind
        .map(str::to_owned)
        .unwrap_or_else(|| item_kind(item).to_owned());
    json!({
        "asset": format!("item:{package}:{n}"),
        "root": "packages",
        "kind": kind,
        "item_kind": serde_json::to_value(item.kind).unwrap_or(Value::Null),
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
                let edited = edited_kinds(&e.loaded);
                let filter = q.kind.as_deref().map(str::trim);
                for (n, item) in e.loaded.package.manifest.items.iter().enumerate() {
                    let row = item_row(id, n, item, edited.get(&n).copied());
                    if let Some(kind) = filter
                        && !row["kind"].as_str().unwrap_or("").eq_ignore_ascii_case(kind)
                    {
                        continue;
                    }
                    let hay = format!(
                        "{} {} {} {}",
                        row["kind"].as_str().unwrap_or(""),
                        row["item_kind"].as_str().unwrap_or(""),
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
            let (catalogue, problem) = current_catalogue(tags_root);
            problems.extend(problem);
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
                let row = tag_row(key, versions, &o, catalogue.as_deref());
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
    /// Write the `like:` item for **New from this...** and `asset.copy`.
    pub fn asset_copy(
        &mut self,
        id: &AssetId,
        package: &str,
        to: Option<&str>,
    ) -> Result<Value, CopyRefusal> {
        let set = read_set(&self.data_root());
        copy(&self.tags_root(), &set, id, package, to)
    }

    /// [`super::Action::OpenAsset`]: the asset a browser row, a Go to
    /// anything entry or `shell.open {asset}` names. A package item opens
    /// the editor of what it builds ([`Self::open_item`]), and a voice
    /// opens its own document (docs/editor/studio-flow.md, "One way in").
    pub(super) fn open_asset(&mut self, asset: &str, map: Option<&str>) {
        if map.is_none() && self.open_source_asset(asset, false) {
            return;
        }
        let key = match AssetId::parse(asset) {
            Ok(AssetId::Tag(key)) => key,
            Ok(AssetId::Item(p, n)) => return self.open_item(asset, &p, n, map),
            Ok(AssetId::Voice(id)) => return self.open_voice(&id),
            Ok(AssetId::Forge(map, file)) => return self.open_forge_variant(&map, &file),
            Ok(AssetId::Package(id)) => return self.open_package_document(&id),
            Err(e) => return self.error(e),
        };
        self.open_tag(key, map)
    }

    /// [`super::Action::OpenFields`]: the tag as [`Self::open_asset`] opens
    /// it, with its tab marked to show the field panel in place of its
    /// group's editor (a bitmap's tab leaves the Textures canvas, a sound's
    /// the sound editor). The scenario is the map's own tab and is
    /// refused: its fields are the Tags tool's.
    pub(super) fn open_fields(&mut self, asset: &str, map: Option<&str>) {
        let key = match AssetId::parse(asset) {
            Ok(AssetId::Tag(key)) => key,
            Ok(_) => return self.error(format!("{asset}: Open as fields is for a tag")),
            Err(e) => return self.error(e),
        };
        if kinds::kind(key.group).editor == Editor::Scenario {
            return self.error(format!(
                "{asset}: the scenario is the map's own tab; its fields are in the Tags tool"
            ));
        }
        let text = format!("{}:{}", fourcc_text(key.group), key.path);
        self.open_tag(key, map);
        self.sync_tags();
        let Some(id) = tag_api::find_tag(&self.tags, &text) else {
            // `open_tag` has said why the tag did not open.
            return;
        };
        if self.find_doc(&self.doc_id(Doc::Tag(id))) == Some(Doc::Tag(id)) {
            self.fields_only.insert(id);
            self.focus_doc(Doc::Tag(id));
        }
    }

    /// The tag half of [`Self::open_asset`]: open the map that holds the
    /// tag (a Halo 1 map shows read-only, with no document), then the tag
    /// in the editor its group gets. A scenario tag is the map's own tab.
    /// An item whose editor is a tag editor comes here too.
    fn open_tag(&mut self, key: TagKey, map: Option<&str>) {
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
            Some(id) => {
                if let Err(e) = self.navigate(super::NavAction::Open(vec![id])) {
                    return self.error(e);
                }
                // Open shows the tag in its group's editor again.
                self.fields_only.remove(&id);
                self.open_doc(Doc::Tag(id));
            }
            None => self.error(format!("{text} is not in the open map")),
        }
    }

    /// [`Self::open_asset`] for a package item (`item:<package>:<n>`): the
    /// editor of what the item builds (docs/editor/studio-flow.md,
    /// "Everything is an asset"). A `rules` item opens in the script
    /// editor, a `variant` in the variant's settings, a `tag`, `bitmap`,
    /// `model`, `animation` or `sound` item the tag it adds, an `edits`
    /// item the first tag its file names. A `playlist` item's fragment has
    /// no editor (the Playlists tool opens a `multiplayer_hopper.dat`) and
    /// is refused by name. Script and variant tabs carry the item's id.
    /// Items that open in a tag editor use `map` when supplied.
    fn open_item(&mut self, asset: &str, package: &str, n: usize, map: Option<&str>) {
        let set = read_set(&self.data_root());
        let listed = set.listed.iter().find(|l| l.id == package && l.on);
        let entry = set.on.iter().find(|e| e.loaded.package.name() == package);
        let (Some(entry), Some(listed)) = (entry, listed) else {
            return self.error(format!(
                "no package {package:?} is on (the Packages root lists them)"
            ));
        };
        let Some(item) = entry.loaded.package.manifest.items.get(n) else {
            let len = entry.loaded.package.manifest.items.len();
            return self.error(format!(
                "{asset}: package {package:?} has {len} items, none is number {n}"
            ));
        };
        let item_id = format!("item:{package}:{n}");
        // The items whose editor is a tag editor need the map that holds
        // their tag; with none asked for and none open, the refusal names
        // the item.
        if matches!(
            item.kind,
            ItemKind::Tag
                | ItemKind::Bitmap
                | ItemKind::Model
                | ItemKind::Animation
                | ItemKind::Sound
                | ItemKind::Edits
        ) && map.is_none()
            && self.loaded_map().is_none()
        {
            return self.error(format!(
                "{asset}: the tag it builds needs a map that holds it (open one first)"
            ));
        }
        match item.kind {
            ItemKind::Rules | ItemKind::Variant => {
                let Some(dir) = &listed.dir else {
                    return self.error(format!(
                        "{asset}: package {package:?} is built in, so its scripts have no file to open"
                    ));
                };
                let Some(source) = item.source.as_deref() else {
                    return self.error(format!("{asset}: the item names no source file"));
                };
                if source.ends_with(".firefight.json") {
                    return self.error(format!(
                        "{asset}: a wave set builds into the map and has no editor"
                    ));
                }
                let file = std::path::PathBuf::from(dir).join(source);
                match item.kind {
                    ItemKind::Rules => {
                        if let Err(e) = self.rules.open_path(&file) {
                            return self.error(format!("{asset}: {e}"));
                        }
                        // `focus_doc` syncs the tabs (the script's tab
                        // opens, the Variants tool comes forward), then
                        // the tab takes the item's id, which `doc_id`
                        // answers.
                        self.focus_doc(Doc::Rules);
                        self.set_tab_item(
                            Doc::Rules,
                            Some(item_id),
                            Some(file.display().to_string()),
                        );
                    }
                    _ => {
                        let text = entry
                            .loaded
                            .files
                            .get(source)
                            .map(|b| String::from_utf8_lossy(b).into_owned());
                        let Some(text) = text else {
                            return self.error(format!("{asset}: {source} is not in the package"));
                        };
                        if !self.run_variant(super::Action::NewVariantFromScript(text)) {
                            return;
                        }
                        self.focus_doc(Doc::Variant);
                        self.set_tab_item(Doc::Variant, Some(item_id), None);
                    }
                }
            }
            ItemKind::Playlist => {
                self.error(format!(
                    "{asset}: a playlist item's fragment has no editor (the Playlists tool \
                     opens a multiplayer_hopper.dat)"
                ));
            }
            ItemKind::Edits => {
                let Some(source) = item.source.as_deref() else {
                    return self.error(format!("{asset}: the item names no source file"));
                };
                let text = entry
                    .loaded
                    .files
                    .get(source)
                    .map(|b| String::from_utf8_lossy(b).into_owned());
                let Some(text) = text else {
                    return self.error(format!("{asset}: {source} is not in the package"));
                };
                let edits = match parse_edits(&text) {
                    Ok(edits) => edits,
                    Err(f) => return self.error(format!("{asset}: {}", f.why)),
                };
                let mut first: Option<TagKey> = None;
                for (edit, _) in &edits {
                    if let Some(key) = group_ref(&edit.tag) {
                        first = Some(key);
                        break;
                    }
                }
                match first {
                    Some(key) => self.open_tag(key, map),
                    None => self.error(format!(
                        "{asset}: {} names no <group>:<path> tag",
                        item.source.as_deref().unwrap_or("the file")
                    )),
                }
            }
            _ => match added_tag(item) {
                Some(key) => self.open_tag(key, map),
                None => {
                    let kind = serde_json::to_value(item.kind)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_else(|| format!("{:?}", item.kind));
                    self.error(format!("{asset}: a {kind} item has no editor yet"))
                }
            },
        }
    }
}

mod browse;
mod creation;
mod forge;
pub use forge::forge_items;
pub use browse::{Browse, BrowseMode, BrowseRow, BrowseView};
pub use creation::CreateAsset;
pub(crate) use creation::{SourceAsset, SourceView};
