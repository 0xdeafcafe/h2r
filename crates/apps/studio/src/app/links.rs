//! What an asset uses, what uses it and which editors open it
//! (docs/editor/studio-flow.md, "Links"): one answer for any asset id,
//! read from the tags tree, the package set and the dialogue rules each
//! time. Nothing is stored, so a link cannot go stale. `asset.links` and
//! the document header's **Links** and **Open with** menus read it.
//!
//! The tables, by asset:
//!
//! | Asset | Uses | Used by |
//! |---|---|---|
//! | a tag | the tags it references (role of the target's kind); for a dialogue sound, the voice that says it | the tags that reference it; the packages that edit or add it (and, as `changed_by`, the packages that patch it in load order) |
//! | a `rules` or `variant` item | (a variant) the `rules` items of its key | (a script) the `variant` items of its key |
//! | any other item | the tag it adds or edits | its package |
//! | a package | its items, the packages it `requires` | the packages that require it |
//! | a voice | | the dialogue sounds its rules pick, and (from the batch) the lines it made |
//!
//! A `rules` item and a `variant` item of one script share the file stem
//! the registry keys them by (`rules::builtin::package_sources`); that
//! key is the link between them.

use std::collections::BTreeSet;
use std::path::Path;

use ::workspace::{GameSet, parse_edits};
use content::package::Kind as ItemKind;
use serde_json::{Value, json};
use tags::defs::fourcc_text;
use tags::repo::{Body, TagKey, TagRepo, TagSource, Value as FieldValue};

use super::assets::{AssetId, kind_of, overlay, tag_id};
use super::kinds::{self, Editor};
use super::mods::{ChangedBy, changed_by};
use crate::voice::dataset::{CHARACTERS, Rules, is_dialogue};

/// One link: the asset at its end, the plain word the menu groups it by
/// and where the link was read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub asset: String,
    pub role: &'static str,
    pub via: String,
}

/// One editor of an asset. `why_not` is the reason it cannot open, drawn
/// in the row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorRow {
    pub id: &'static str,
    pub label: &'static str,
    pub why_not: Option<String>,
}

/// The answer for one asset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Links {
    pub uses: Vec<Link>,
    pub used_by: Vec<Link>,
    /// A tag: the packages with a patch on it, in load order, with each
    /// field's value and whether it wins (`app::mods::changed_by`).
    pub changed_by: Vec<ChangedBy>,
    pub editors: Vec<EditorRow>,
    /// What could not be read, one line each: a tag file that does not
    /// decode, a missing tree. The answer is whole for everything else.
    pub problems: Vec<String>,
}

impl Links {
    pub fn json(&self, asset: &str) -> Value {
        let rows = |l: &[Link]| -> Vec<Value> {
            l.iter()
                .map(|l| json!({"asset": l.asset, "role": l.role, "via": l.via}))
                .collect()
        };
        json!({
            "asset": asset,
            "uses": rows(&self.uses),
            "used_by": rows(&self.used_by),
            "changed_by": self.changed_by.iter().map(ChangedBy::json).collect::<Vec<_>>(),
            "editors": self.editors.iter().map(|e| json!({
                "id": e.id,
                "label": e.label,
                "why_not": e.why_not,
            })).collect::<Vec<_>>(),
            "problems": self.problems,
        })
    }
}

/// The role of a link to a tag of `group`: the plain word a menu groups
/// by. It is the target's kind (`assets::kind_of`), with the names the
/// weapon's journey uses.
pub fn role_of(group: &str) -> &'static str {
    if group == "proj" {
        return "Projectile";
    }
    match kind_of(group) {
        "Models" => "Model",
        "HUDs" => "HUD",
        k => k,
    }
}

const ROLE_ORDER: &[&str] = &[
    "Model",
    "Animations",
    "Sounds",
    "HUD",
    "Projectile",
    "Script",
    "Variant",
    "Voice",
    "Package",
];

fn sort(links: &mut Vec<Link>) {
    let rank = |r: &str| ROLE_ORDER.iter().position(|x| *x == r).unwrap_or(ROLE_ORDER.len());
    links.sort_by(|a, b| {
        (rank(a.role), a.role, &a.asset, &a.via).cmp(&(rank(b.role), b.role, &b.asset, &b.via))
    });
    links.dedup();
}

fn tag_link(key: &TagKey, via: String) -> Link {
    Link {
        asset: tag_id(key),
        role: role_of(&fourcc_text(key.group)),
        via,
    }
}

/// The tags `key` names, with where in the file each is: the array and
/// the byte offset of the reference (`node 1 +0x8`). A tag the tree does
/// not hold names none.
fn refs_of(repo: &mut TagRepo, key: &TagKey) -> Result<Vec<(TagKey, String)>, String> {
    let tag = match repo.read(key) {
        Ok(Some(t)) => t,
        Ok(None) => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    Ok(match tag.body {
        Body::Alias { owner, .. } => vec![(owner, "alias of".to_owned())],
        Body::Tree(t) => t
            .nodes
            .iter()
            .enumerate()
            .flat_map(|(i, n)| {
                n.fields.iter().filter_map(move |(off, v)| match v {
                    FieldValue::Ref(k) => Some((k.clone(), format!("node {i} +{off:#x}"))),
                    _ => None,
                })
            })
            .collect(),
    })
}

/// The voice that says a dialogue sound: the first character whose rules
/// pick its path.
pub(crate) fn voice_of(path: &str) -> Option<&'static str> {
    if !is_dialogue(path) {
        return None;
    }
    CHARACTERS
        .iter()
        .find(|c| Rules::of(c).matches(path))
        .map(|c| c.id)
}

fn editor(e: Editor, label: &'static str) -> EditorRow {
    EditorRow {
        id: e.id(),
        label,
        why_not: None,
    }
}

fn editor_label(e: Editor) -> &'static str {
    match e {
        Editor::FieldPanel => "Fields",
        Editor::Scenario => "Map",
        Editor::Modeling => "Model",
        Editor::Textures => "Textures",
        Editor::Audition => "Sound",
        Editor::Hud => "HUD",
        Editor::UiScreens => "UI screens",
        Editor::Scripts => "Scripts",
        Editor::Animation => "Animation",
    }
}

/// The editors of a tag: its kind's own, **Fields** (every tag has it)
/// and the editors of other kinds that work on it.
fn tag_editors(key: &TagKey) -> Vec<EditorRow> {
    let group = fourcc_text(key.group);
    let own = kinds::kind(key.group).editor;
    let mut out = vec![editor(own, editor_label(own))];
    if own != Editor::FieldPanel {
        out.push(editor(Editor::FieldPanel, "Fields"));
    }
    if kind_of(&group) == "Objects" {
        out.push(editor(Editor::Modeling, "Model"));
        out.push(editor(Editor::Hud, "HUD"));
    }
    if group == "snd!" && voice_of(&key.path).is_some() {
        out.push(EditorRow {
            id: "voices",
            label: "Voices",
            why_not: None,
        });
    }
    out
}

fn tag_links(tags_root: &Path, set: &GameSet, key: &TagKey) -> Links {
    let mut out = Links {
        editors: tag_editors(key),
        ..Links::default()
    };
    match TagRepo::open(tags_root) {
        Ok(mut repo) => {
            match refs_of(&mut repo, key) {
                Ok(refs) => out
                    .uses
                    .extend(refs.into_iter().map(|(k, via)| tag_link(&k, via))),
                Err(e) => out.problems.push(format!("{}: {e}", tag_id(key))),
            }
            for other in repo.keys() {
                if &other == key {
                    continue;
                }
                match refs_of(&mut repo, &other) {
                    Ok(refs) => out.used_by.extend(
                        refs.into_iter()
                            .filter(|(k, _)| k == key)
                            .map(|(_, via)| tag_link(&other, via)),
                    ),
                    Err(e) => out.problems.push(format!("{}: {e}", tag_id(&other))),
                }
            }
        }
        Err(e) => out
            .problems
            .push(format!("no tags tree at {}: {e}", tags_root.display())),
    }
    let voice = if fourcc_text(key.group) == "snd!" {
        voice_of(&key.path)
    } else {
        None
    };
    if let Some(voice) = voice {
        out.uses.push(Link {
            asset: format!("voice:{voice}"),
            role: "Voice",
            via: "dialogue rules".to_owned(),
        });
    }
    out.changed_by = changed_by(set, key, None, None);
    let o = overlay(set);
    for (word, by) in [("adds", &o.added), ("edits", &o.edited)] {
        for (package, _) in by.get(key).into_iter().flatten() {
            out.used_by.push(Link {
                asset: format!("package:{package}"),
                role: "Package",
                via: word.to_owned(),
            });
        }
    }
    out
}

/// A script item's key: its source's file name up to the first `.`.
fn script_key(source: &str) -> &str {
    let stem = source.rsplit('/').next().unwrap_or(source);
    stem.split('.').next().unwrap_or(stem)
}

fn item_links(set: &GameSet, package: &str, n: usize) -> Result<Links, String> {
    let entry = set
        .on
        .iter()
        .find(|e| e.loaded.package.name() == package)
        .ok_or_else(|| format!("no package {package:?} is on (asset.list root packages lists them)"))?;
    let items = &entry.loaded.package.manifest.items;
    let item = items.get(n).ok_or_else(|| {
        format!(
            "package {package:?} has {} items, none is number {n}",
            items.len()
        )
    })?;
    let mut out = Links::default();
    out.used_by.push(Link {
        asset: format!("package:{package}"),
        role: "Package",
        via: format!("item {n}"),
    });
    match item.kind {
        ItemKind::Rules | ItemKind::Variant => {
            let key = item.source.as_deref().map(script_key).unwrap_or_default();
            let (other, role, editors) = if item.kind == ItemKind::Variant {
                (
                    ItemKind::Rules,
                    "Script",
                    vec![
                        EditorRow {
                            id: "variants",
                            label: "Variant",
                            why_not: None,
                        },
                        EditorRow {
                            id: "scripts",
                            label: "Script",
                            why_not: None,
                        },
                    ],
                )
            } else {
                (
                    ItemKind::Variant,
                    "Variant",
                    vec![EditorRow {
                        id: "scripts",
                        label: "Script",
                        why_not: None,
                    }],
                )
            };
            out.editors = editors;
            for e in &set.on {
                let id = e.loaded.package.name();
                for (i, it) in e.loaded.package.manifest.items.iter().enumerate() {
                    let same = it.kind == other
                        && it.source.as_deref().map(script_key) == Some(key)
                        && !(id == package && i == n);
                    if same {
                        let link = Link {
                            asset: format!("item:{id}:{i}"),
                            role,
                            via: format!("script {key}"),
                        };
                        // A variant uses its script; a script is used by its variants.
                        if item.kind == ItemKind::Variant {
                            out.uses.push(link);
                        } else {
                            out.used_by.push(link);
                        }
                    }
                }
            }
        }
        ItemKind::Edits => {
            out.editors = vec![EditorRow {
                id: Editor::FieldPanel.id(),
                label: "Fields",
                why_not: None,
            }];
            let text = item
                .source
                .as_deref()
                .and_then(|s| entry.loaded.files.get(s))
                .map(|b| String::from_utf8_lossy(b).into_owned());
            match text.as_deref().map(parse_edits) {
                Some(Ok(edits)) => {
                    let mut seen = BTreeSet::new();
                    for (edit, _) in edits {
                        let Some((g, path)) = edit.tag.split_once(':') else {
                            out.problems
                                .push(format!("{:?} is not <group>:<path>", edit.tag));
                            continue;
                        };
                        let Some(group) = tags::defs::parse_fourcc(g) else {
                            out.problems.push(format!("{:?}: no group {g:?}", edit.tag));
                            continue;
                        };
                        let key = TagKey::new(group, path.replace('/', "\\"));
                        if seen.insert(key.clone()) {
                            out.uses.push(tag_link(&key, "edits".to_owned()));
                        }
                    }
                }
                Some(Err(f)) => out.problems.push(f.why.to_string()),
                None => out.problems.push(format!(
                    "{}: not in the package",
                    item.source.as_deref().unwrap_or("(no source)")
                )),
            }
        }
        _ => {
            if let Some(key) = ::workspace::added_tag(item) {
                out.editors = tag_editors(&key);
                out.uses.push(tag_link(&key, "adds".to_owned()));
            } else {
                out.editors = match item.kind {
                    ItemKind::Playlist => vec![EditorRow {
                        id: "playlists",
                        label: "Playlist",
                        why_not: None,
                    }],
                    ItemKind::Hud => vec![editor(Editor::Hud, "HUD")],
                    ItemKind::Screens | ItemKind::ScreenEdits => {
                        vec![editor(Editor::UiScreens, "UI screens")]
                    }
                    _ => Vec::new(),
                };
            }
        }
    }
    Ok(out)
}

fn package_links(set: &GameSet, id: &str) -> Result<Links, String> {
    let entry = set
        .on
        .iter()
        .find(|e| e.loaded.package.name() == id)
        .ok_or_else(|| format!("no package {id:?} is on (asset.list root packages lists them)"))?;
    let mut out = Links {
        editors: vec![EditorRow {
            id: "package",
            label: "Package",
            why_not: None,
        }],
        ..Links::default()
    };
    let m = &entry.loaded.package.manifest;
    for (n, _) in m.items.iter().enumerate() {
        out.uses.push(Link {
            asset: format!("item:{id}:{n}"),
            role: "Package",
            via: format!("item {n}"),
        });
    }
    for r in &m.package.requires {
        out.uses.push(Link {
            asset: format!("package:{r}"),
            role: "Package",
            via: "requires".to_owned(),
        });
    }
    for e in &set.on {
        if e.loaded.package.manifest.package.requires.iter().any(|r| r == id) {
            out.used_by.push(Link {
                asset: format!("package:{}", e.loaded.package.name()),
                role: "Package",
                via: "requires".to_owned(),
            });
        }
    }
    Ok(out)
}

/// A voice: the dialogue sounds of the tree whose paths its rules pick
/// (used by: the sounds it says), and the voice document as its editor.
/// The lines it made are the batch's, which the caller adds
/// (`App::asset_links`).
fn voice_links(tags_root: &Path, id: &str) -> Links {
    let mut out = Links {
        editors: vec![EditorRow {
            id: "voices",
            label: "Voice",
            why_not: None,
        }],
        ..Links::default()
    };
    match TagRepo::open(tags_root) {
        Ok(repo) => {
            for key in repo.keys() {
                if fourcc_text(key.group) == "snd!" && voice_of(&key.path) == Some(id) {
                    out.used_by.push(Link {
                        asset: tag_id(&key),
                        role: "Sounds",
                        via: "dialogue rules".to_owned(),
                    });
                }
            }
        }
        // A voice is read from the data root; a missing tree only means
        // there are no sounds to list.
        Err(_) => {}
    }
    out
}

/// `asset.links`: the answer for `id`.
pub fn links(tags_root: &Path, set: &GameSet, id: &AssetId) -> Result<Links, String> {
    let mut out = match id {
        AssetId::Tag(key) => tag_links(tags_root, set, key),
        AssetId::Item(package, n) => item_links(set, package, *n)?,
        AssetId::Package(p) => package_links(set, p)?,
        AssetId::Voice(v) => voice_links(tags_root, v),
    };
    sort(&mut out.uses);
    sort(&mut out.used_by);
    Ok(out)
}

/// The `item:` id of the first item of `kind` in a package that is on whose
/// source is named `name`: by its script key (`hill` for
/// `scripts/hill.variant.rules`) or its file name, without regard to case.
pub fn item_id(set: &GameSet, kind: ItemKind, name: &str) -> Option<String> {
    for entry in &set.on {
        for (n, item) in entry.loaded.package.manifest.items.iter().enumerate() {
            let source = item.source.as_deref().unwrap_or("");
            let file = source.rsplit('/').next().unwrap_or(source);
            if item.kind == kind
                && (script_key(source).eq_ignore_ascii_case(name) || file.eq_ignore_ascii_case(name))
            {
                return Some(format!("item:{}:{n}", entry.loaded.package.name()));
            }
        }
    }
    None
}

/// What a package item is called in a header's path: its kind word and its source.
pub fn item_label(set: &GameSet, package: &str, n: usize) -> Option<(String, String)> {
    let entry = set.on.iter().find(|e| e.loaded.package.name() == package)?;
    let item = entry.loaded.package.manifest.items.get(n)?;
    Some((format!("{:?}", item.kind).to_lowercase(), item.source.clone().unwrap_or_default()))
}

impl super::App {
    /// The asset a document is, when it is one: a tag by `tag:` id; a
    /// script, a variant and a playlist file as the `item:` of a package
    /// that is on and carries it; the HUD of a package as the package.
    pub fn doc_asset(&self, doc: super::Doc) -> Option<String> {
        use super::Doc;
        let tag = |id| {
            self.tags
                .tag(id)
                .map(|t| format!("tag:{}:{}", fourcc_text(t.group), t.name))
        };
        let set = || super::assets::read_set(&self.data_root());
        match doc {
            Doc::SourceAsset(_) => self.source_asset(doc).map(|s| super::assets::tag_id(&s.key)),
            Doc::Package => self.package_document.id().map(|id| format!("package:{id}")),
            Doc::Voice => self.voices.doc.as_ref().map(|v| format!("voice:{v}")),
            Doc::Tag(id) => tag(id),
            Doc::Map => self.session.scenario().and_then(|s| tag(s.tag)),
            Doc::Ui => match &self.ui_screens.pick {
                Some(super::ui_screens::Pick::Retail { tag, .. }) => Some(format!("tag:wgit:{tag}")),
                _ => None,
            },
            Doc::Rules => {
                let file = self.rules.path.as_ref()?.file_name()?.to_string_lossy().into_owned();
                item_id(&set(), ItemKind::Rules, script_key(&file))
            }
            Doc::Variant => {
                let name = ::variants::panel::strip_icon(&self.variants.as_ref()?.current()?.name());
                item_id(&set(), ItemKind::Variant, &name)
            }
            Doc::Playlists => {
                let file = self.hoppers.path()?.file_name()?.to_string_lossy().into_owned();
                item_id(&set(), ItemKind::Playlist, &file)
            }
            Doc::Hud => {
                let opened = self.hud.opened.as_ref()?;
                let set = set();
                set.on
                    .iter()
                    .any(|e| e.loaded.package.name() == opened.as_str())
                    .then(|| format!("package:{opened}"))
            }
        }
    }

    /// [`links`] for `id`, with what only the app knows: a voice's lines.
    /// Each done row of the batch said by the voice that has a target
    /// sound links to it (role Sounds, `via` the line's number), read from
    /// the batch each time.
    pub fn asset_links(&self, id: &AssetId) -> Result<Links, String> {
        let set = super::assets::read_set(&self.data_root());
        let mut out = links(&self.tags_root(), &set, id)?;
        if let AssetId::Voice(voice) = id {
            let batch = &self.voices.batch;
            for (n, r) in batch.rows.iter().enumerate() {
                if r.state != super::voice_batch::RowState::Done
                    || batch.voice_of(r).as_deref() != Some(voice.as_str())
                {
                    continue;
                }
                if let Some(tag) = &r.tag {
                    out.uses.push(Link {
                        asset: format!("tag:snd!:{tag}"),
                        role: "Sounds",
                        via: format!("line {}", n + 1),
                    });
                }
            }
            sort(&mut out.uses);
        }
        Ok(out)
    }
}
