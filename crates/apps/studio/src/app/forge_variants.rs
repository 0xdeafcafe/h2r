//! Forge map variants as documents (docs/game/forge-map-variants.md, "The
//! map variant file"; docs/editor/studio-flow.md, "Forge variants and
//! items").
//!
//! A map variant is a Forge document (`forge_doc::ForgeDoc`) in the store
//! `forge::files::variants` owns: `<data>/map-variants/<map>/<file>.h2mapvariant`.
//! Studio reads and writes it through that store and the document's own
//! types; there is no second reader. Its asset id is `forge:<map>:<file>`.
//!
//! The document lists the variant's **things**: each entry a
//! `Placement::Add` op made and each entry of the base map an entry op
//! names, with the fields the variant writes ([`things`]). A field edit
//! replaces the value of the write that gives the field its value
//! ([`set_field`]), keeps the value's type and is refused when the
//! document no longer validates. Save writes the store's file again
//! (`variants::save` with `replace`), which is the variant's edit target:
//! map variants are user files, not package items.
use std::path::Path;

use forge::files::variants::{self as store, Source, Suggestable};
use forge_doc::{
    BlockKind, Edit, Endpoint, EntryOp, F32, Forge, ForgeDoc, Limits, OpId, Placement,
    TagKey as ForgeTag, Value as FieldValue,
};
use serde_json::{Value, json};
use tags::defs::fourcc_text;
use tags::repo::TagKey;

use super::links::{Link, role_of};
use super::{App, Doc};

/// The Browser kind that lists the map variants.
pub const VARIANTS_KIND: &str = "Forge variants";
/// The Browser kind that lists the Forge palette.
pub const ITEMS_KIND: &str = "Forge items";
/// The role a variant has in a tag's `used_by`.
pub const ROLE: &str = "Forge variant";
/// The folder the block-out definitions' tags are named under.
pub const BLOCKOUT: &str = "forge\\blockout\\";

/// A variant's asset id.
pub fn asset_id(map: &str, file: &str) -> String {
    format!("forge:{map}:{file}")
}

/// One thing of a variant: an entry it added, or an entry of the base map
/// it edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thing {
    /// The entry, without its print.
    pub target: Endpoint,
    pub block: BlockKind,
    /// A base entry the variant removes.
    pub removed: bool,
    /// The fields the variant writes, in the order first written, each
    /// with the value the last write gives it.
    pub fields: Vec<(String, FieldValue)>,
    /// What the variant says of the entry beside its fields: a property,
    /// an area, a shown flag.
    pub notes: Vec<String>,
    /// The block-out definition a piece has.
    pub piece: Option<String>,
}

impl Thing {
    pub fn added(&self) -> bool {
        matches!(self.target, Endpoint::Added(_))
    }

    /// The tag the entry places: the first tag value among its fields, or
    /// its block-out definition's.
    pub fn item(&self) -> Option<ForgeTag> {
        if let Some(name) = &self.piece {
            return Some(ForgeTag {
                group: forge_doc::PIECE_GROUP.into(),
                path: format!("{BLOCKOUT}{name}"),
            });
        }
        self.fields.iter().find_map(|(_, v)| match v {
            FieldValue::Tag(Some(tag)) => Some(tag.clone()),
            _ => None,
        })
    }
}

/// An entry as the API and the window name it: `added:<op>` or
/// `base:<block>:<index>`.
pub fn target_text(target: Endpoint) -> String {
    match target.identity() {
        Endpoint::Added(id) => format!("added:{id}"),
        Endpoint::Base { block, index, .. } => format!("base:{}:{index}", block.name()),
    }
}

/// [`target_text`] read back.
pub fn parse_target(text: &str) -> Result<Endpoint, String> {
    let bad = || format!("{text:?} is not an entry (added:<op> or base:<block>:<index>)");
    let (kind, rest) = text.split_once(':').ok_or_else(bad)?;
    match kind {
        "added" => Ok(Endpoint::Added(OpId(rest.parse().map_err(|_| bad())?))),
        "base" => {
            let (block, index) = rest.rsplit_once(':').ok_or_else(bad)?;
            let block = BlockKind::from_name(block).ok_or_else(bad)?;
            Ok(Endpoint::base(block, index.parse().map_err(|_| bad())?))
        }
        _ => Err(bad()),
    }
}

fn put(fields: &mut Vec<(String, FieldValue)>, field: &str, value: &FieldValue) {
    match fields.iter_mut().find(|(name, _)| name == field) {
        Some((_, old)) => *old = value.clone(),
        None => fields.push((field.to_owned(), value.clone())),
    }
}

/// The note of an entry op that writes no field, and the word that names
/// its kind (a later note of the kind replaces an earlier one).
fn note_of(op: &EntryOp) -> Option<(String, String)> {
    match op {
        EntryOp::Property { property, .. } => {
            let text = format!("{property:?}");
            let kind = text.split('(').next().unwrap_or("").to_owned();
            Some((kind, text))
        }
        EntryOp::Area { area, .. } => Some((
            "Area".into(),
            if area.is_some() { "Area set".into() } else { "Area cleared".into() },
        )),
        EntryOp::Show { on, .. } => Some((
            "Show".into(),
            format!("Shown in a match: {}", if *on { "yes" } else { "no" }),
        )),
        _ => None,
    }
}

/// The things of a document, in the order its ops first name them. A
/// reset takes a base entry's row out, as it takes its edits out; a
/// removed added entry has no row.
pub fn things(doc: &ForgeDoc) -> Vec<Thing> {
    let mut out: Vec<Thing> = Vec::new();
    fn row(out: &mut Vec<Thing>, target: Endpoint) -> Option<&mut Thing> {
        if let Some(i) = out.iter().position(|t| t.target == target) {
            return out.get_mut(i);
        }
        let Endpoint::Base { block, .. } = target else {
            return None;
        };
        out.push(Thing {
            target,
            block,
            removed: false,
            fields: Vec::new(),
            notes: Vec::new(),
            piece: None,
        });
        out.last_mut()
    }
    for op in &doc.ops {
        match &op.edit {
            Edit::Placement(Placement::Add { block, fields, .. }) => {
                let mut thing = Thing {
                    target: Endpoint::Added(op.id),
                    block: *block,
                    removed: false,
                    fields: Vec::new(),
                    notes: Vec::new(),
                    piece: None,
                };
                for write in fields {
                    put(&mut thing.fields, &write.field, &write.value);
                }
                out.push(thing);
            }
            Edit::Entry(entry) => {
                let target = entry.target().identity();
                match entry {
                    EntryOp::Reset { .. } => out.retain(|t| t.target != target),
                    EntryOp::Remove { .. } if matches!(target, Endpoint::Added(_)) => {
                        out.retain(|t| t.target != target);
                    }
                    EntryOp::Remove { .. } => {
                        if let Some(thing) = row(&mut out, target) {
                            thing.removed = true;
                        }
                    }
                    EntryOp::Set { write, .. } => {
                        if let Some(thing) = row(&mut out, target) {
                            put(&mut thing.fields, &write.field, &write.value);
                        }
                    }
                    other => {
                        if let (Some((kind, text)), Some(thing)) =
                            (note_of(other), row(&mut out, target))
                        {
                            thing.notes.retain(|n| !n.starts_with(kind.as_str()));
                            thing.notes.push(text);
                        }
                    }
                }
            }
            Edit::Forge(Forge::Piece { target, def }) => {
                if let Some(thing) = row(&mut out, target.identity()) {
                    thing.piece = Some(def.name());
                }
            }
            _ => {}
        }
    }
    out
}

/// A value's type, as the API names it.
pub fn value_type(value: &FieldValue) -> &'static str {
    match value {
        FieldValue::Bool(_) => "bool",
        FieldValue::Int(_) => "int",
        FieldValue::Float(_) => "float",
        FieldValue::Vec2(_) => "vec2",
        FieldValue::Vec3(_) => "vec3",
        FieldValue::Vec4(_) => "vec4",
        FieldValue::Text(_) => "text",
        FieldValue::Tag(_) => "tag",
    }
}

fn tag_text(tag: &ForgeTag) -> String {
    format!("{}:{}", tag.group, tag.path)
}

/// A value as the API answers it: a boolean, a number, a list of numbers,
/// a string, or a tag as `<group>:<path>` (null for none).
pub fn value_json(value: &FieldValue) -> Value {
    let list = |v: &[F32]| json!(v.iter().map(|f| f.get()).collect::<Vec<f32>>());
    match value {
        FieldValue::Bool(b) => json!(b),
        FieldValue::Int(i) => json!(i),
        FieldValue::Float(f) => json!(f.get()),
        FieldValue::Vec2(v) => list(v),
        FieldValue::Vec3(v) => list(v),
        FieldValue::Vec4(v) => list(v),
        FieldValue::Text(t) => json!(t),
        FieldValue::Tag(Some(tag)) => json!(tag_text(tag)),
        FieldValue::Tag(None) => Value::Null,
    }
}

/// A value as the window's text box shows it.
pub fn value_text(value: &FieldValue) -> String {
    let list = |v: &[F32]| {
        v.iter()
            .map(|f| f.get().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match value {
        FieldValue::Bool(b) => b.to_string(),
        FieldValue::Int(i) => i.to_string(),
        FieldValue::Float(f) => f.get().to_string(),
        FieldValue::Vec2(v) => list(v),
        FieldValue::Vec3(v) => list(v),
        FieldValue::Vec4(v) => list(v),
        FieldValue::Text(t) => t.clone(),
        FieldValue::Tag(Some(tag)) => tag_text(tag),
        FieldValue::Tag(None) => "none".to_owned(),
    }
}

fn float(v: f64) -> Result<F32, String> {
    F32::new(v as f32).map_err(|e| e.to_string())
}

fn floats<const N: usize>(v: &Value) -> Result<[F32; N], String> {
    let list = v
        .as_array()
        .filter(|a| a.len() == N)
        .ok_or_else(|| format!("the value is a list of {N} numbers"))?;
    let mut out = [F32::new(0.0).map_err(|e| e.to_string())?; N];
    for (slot, n) in out.iter_mut().zip(list) {
        *slot = float(
            n.as_f64()
                .ok_or_else(|| format!("the value is a list of {N} numbers"))?,
        )?;
    }
    Ok(out)
}

fn tag_of(text: &str) -> Result<ForgeTag, String> {
    let text = text.strip_prefix("tag:").unwrap_or(text);
    let (group, path) = text
        .split_once(':')
        .filter(|(g, p)| !g.is_empty() && !p.is_empty())
        .ok_or_else(|| format!("{text:?} is not a tag (<group>:<path>)"))?;
    Ok(ForgeTag {
        group: group.to_owned(),
        path: path.replace('/', "\\"),
    })
}

/// A JSON value as a field value of the type `like` has: an edit keeps a
/// field's type.
pub fn value_from_json(like: &FieldValue, v: &Value) -> Result<FieldValue, String> {
    let want = value_type(like);
    let wrong = || format!("the field is a {want}; {v} is not one");
    Ok(match like {
        FieldValue::Bool(_) => FieldValue::Bool(v.as_bool().ok_or_else(wrong)?),
        FieldValue::Int(_) => FieldValue::Int(v.as_i64().ok_or_else(wrong)?),
        FieldValue::Float(_) => FieldValue::Float(float(v.as_f64().ok_or_else(wrong)?)?),
        FieldValue::Vec2(_) => FieldValue::Vec2(floats(v)?),
        FieldValue::Vec3(_) => FieldValue::Vec3(floats(v)?),
        FieldValue::Vec4(_) => FieldValue::Vec4(floats(v)?),
        FieldValue::Text(_) => FieldValue::Text(v.as_str().ok_or_else(wrong)?.to_owned()),
        FieldValue::Tag(_) => match v {
            Value::Null => FieldValue::Tag(None),
            Value::String(s) => FieldValue::Tag(Some(tag_of(s)?)),
            _ => return Err(wrong()),
        },
    })
}

/// The text of the window's box as a field value of the type `like` has.
pub fn value_from_text(like: &FieldValue, text: &str) -> Result<FieldValue, String> {
    let text = text.trim();
    let numbers = || -> Result<Value, String> {
        text.split([',', ' '])
            .filter(|p| !p.is_empty())
            .map(|p| {
                p.parse::<f64>()
                    .map(|n| json!(n))
                    .map_err(|_| format!("{p:?} is not a number"))
            })
            .collect::<Result<Vec<Value>, String>>()
            .map(Value::Array)
    };
    let v = match like {
        FieldValue::Bool(_) => match text {
            "true" | "yes" | "1" => json!(true),
            "false" | "no" | "0" => json!(false),
            _ => return Err(format!("{text:?} is not true or false")),
        },
        FieldValue::Int(_) => json!(
            text.parse::<i64>()
                .map_err(|_| format!("{text:?} is not a whole number"))?
        ),
        FieldValue::Float(_) => json!(
            text.parse::<f64>()
                .map_err(|_| format!("{text:?} is not a number"))?
        ),
        FieldValue::Vec2(_) | FieldValue::Vec3(_) | FieldValue::Vec4(_) => numbers()?,
        FieldValue::Text(_) => json!(text),
        FieldValue::Tag(_) if text.is_empty() || text.eq_ignore_ascii_case("none") => Value::Null,
        FieldValue::Tag(_) => json!(text),
    };
    value_from_json(like, &v)
}

/// The write that gives `field` of `target` its value: the last entry
/// write of the field, else the field of the add that made the entry.
fn write_of<'a>(doc: &'a mut ForgeDoc, target: Endpoint, field: &str) -> Option<&'a mut FieldValue> {
    let at = doc.ops.iter().rposition(|op| match &op.edit {
        Edit::Entry(EntryOp::Set { target: t, write }) => {
            t.identity() == target && write.field == field
        }
        _ => false,
    });
    if let Some(at) = at {
        return match &mut doc.ops[at].edit {
            Edit::Entry(EntryOp::Set { write, .. }) => Some(&mut write.value),
            _ => None,
        };
    }
    let Endpoint::Added(id) = target else {
        return None;
    };
    doc.ops.iter_mut().find(|op| op.id == id).and_then(|op| match &mut op.edit {
        Edit::Placement(Placement::Add { fields, .. }) => fields
            .iter_mut()
            .rev()
            .find(|w| w.field == field)
            .map(|w| &mut w.value),
        _ => None,
    })
}

/// Set a field the variant writes. The value keeps the field's type, and
/// a document that no longer validates is left as it was. The identity
/// changes with the bytes, so `expect` is dropped.
pub fn set_field(
    doc: &mut ForgeDoc,
    target: Endpoint,
    field: &str,
    value: FieldValue,
) -> Result<(), String> {
    let target = target.identity();
    let all = things(doc);
    let thing = all
        .iter()
        .find(|t| t.target == target)
        .ok_or_else(|| format!("the variant has no entry {}", target_text(target)))?;
    let Some((_, old)) = thing.fields.iter().find(|(name, _)| name == field) else {
        let written: Vec<&str> = thing.fields.iter().map(|(n, _)| n.as_str()).collect();
        return Err(format!(
            "the variant writes no {field:?} of {}: it writes {} (Forge sets a field the variant does not write)",
            target_text(target),
            if written.is_empty() { "no field".to_owned() } else { written.join(", ") }
        ));
    };
    if std::mem::discriminant(old) != std::mem::discriminant(&value) {
        return Err(format!(
            "{field:?} is a {}, not a {}",
            value_type(old),
            value_type(&value)
        ));
    }
    let before = doc.clone();
    match write_of(doc, target, field) {
        Some(slot) => *slot = value,
        None => return Err(format!("no write of {field:?} was found")),
    }
    doc.expect = None;
    if let Err(e) = forge_doc::validate(doc, &Limits::default()) {
        *doc = before;
        return Err(e.to_string());
    }
    Ok(())
}

/// How many things of the document place the tag `key`: a tag value among
/// their fields, or a block-out definition of that name.
pub fn places(doc: &ForgeDoc, key: &TagKey) -> usize {
    let group = fourcc_text(key.group);
    things(doc)
        .iter()
        .filter(|t| !t.removed)
        .filter(|t| {
            let piece = t.item().filter(|_| t.piece.is_some());
            let fields = t.fields.iter().filter_map(|(_, v)| match v {
                FieldValue::Tag(Some(tag)) => Some(tag.clone()),
                _ => None,
            });
            piece.into_iter().chain(fields).any(|tag| {
                tag.group.trim() == group.trim() && tag.path.eq_ignore_ascii_case(&key.path)
            })
        })
        .count()
}

/// The saved variants a document can be opened from, of every map.
pub fn saved(data: &Path) -> Result<Vec<Suggestable>, String> {
    store::suggestable(data).map_err(|e| e.to_string())
}

/// The variants that place `key`, as `used_by` links.
pub fn users(data: &Path, key: &TagKey) -> Vec<Link> {
    let mut out = Vec::new();
    for s in saved(data).unwrap_or_default() {
        if s.source != Source::Saved {
            continue;
        }
        let Ok(doc) = store::load(data, &s.map, &s.file, Some(&s.base)) else {
            continue;
        };
        let n = places(&doc, key);
        if n > 0 {
            out.push(Link {
                asset: asset_id(&s.map, &s.file),
                role: ROLE,
                via: format!("{n} placed in {}", s.name),
            });
        }
    }
    out
}

/// The tags a document places, as `uses` links: one per tag, with the
/// number of entries that place it.
pub fn uses(doc: &ForgeDoc) -> Vec<Link> {
    let mut counts: Vec<(ForgeTag, usize)> = Vec::new();
    for thing in things(doc).iter().filter(|t| !t.removed) {
        let Some(tag) = thing.item() else { continue };
        match counts.iter_mut().find(|(t, _)| *t == tag) {
            Some((_, n)) => *n += 1,
            None => counts.push((tag, 1)),
        }
    }
    counts
        .into_iter()
        .map(|(tag, n)| Link {
            asset: format!("tag:{}:{}", tag.group, tag.path),
            role: role_of(tag.group.trim()),
            via: format!("{n} placed"),
        })
        .collect()
}

/// The variant open as a document.
#[derive(Clone, Debug)]
pub struct Open {
    pub map: String,
    pub file: String,
    pub doc: ForgeDoc,
    /// The document as the store's file has it.
    saved: ForgeDoc,
    undo: Vec<ForgeDoc>,
    redo: Vec<ForgeDoc>,
    /// The entry the window shows the fields of ([`target_text`]).
    pub selected: Option<String>,
}

impl Open {
    pub fn asset(&self) -> String {
        asset_id(&self.map, &self.file)
    }

    pub fn dirty(&self) -> bool {
        self.doc != self.saved
    }

    pub fn can(&self) -> (bool, bool) {
        (!self.undo.is_empty(), !self.redo.is_empty())
    }

    /// The document as `forge.variant.read` answers it.
    pub fn json(&self) -> Value {
        let mut v = doc_json(&self.map, &self.file, &self.doc);
        v["dirty"] = json!(self.dirty());
        v["open"] = json!(true);
        v["selected"] = json!(self.selected);
        v
    }
}

/// A variant's things as the API answers them.
pub fn doc_json(map: &str, file: &str, doc: &ForgeDoc) -> Value {
    let rows: Vec<Value> = things(doc)
        .iter()
        .map(|t| {
            json!({
                "target": target_text(t.target),
                "block": t.block.name(),
                "added": t.added(),
                "removed": t.removed,
                "item": t.item().map(|tag| format!("tag:{}:{}", tag.group, tag.path)),
                "piece": t.piece,
                "notes": t.notes,
                "fields": t.fields.iter().map(|(name, value)| json!({
                    "field": name,
                    "type": value_type(value),
                    "value": value_json(value),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "asset": asset_id(map, file),
        "map": map,
        "file": file,
        "name": doc.title,
        "author": doc.author,
        "description": doc.description,
        "ops": doc.ops.len(),
        "things": rows,
    })
}

/// A field edit, a save or a selection of the open variant.
#[derive(Clone, Debug, PartialEq)]
pub enum ForgeAction {
    /// Set a field the variant writes of the entry `target`.
    Set {
        target: String,
        field: String,
        value: FieldValue,
    },
    /// Write the variant's file in the store.
    Save,
    /// Show an entry's fields.
    Select(Option<String>),
}

/// The Forge variant document's state.
#[derive(Default)]
pub struct ForgeVariants {
    pub open: Option<Open>,
}

/// A variant's id in parts.
fn split(asset: &str) -> Option<(&str, &str)> {
    asset.strip_prefix("forge:")?.split_once(':')
}

impl App {
    /// The saved variant `map`/`file` as the store reads it.
    pub fn forge_variant_doc(&self, map: &str, file: &str) -> Result<ForgeDoc, String> {
        let data = self.data_root();
        let listed = saved(&data)?;
        let Some(s) = listed
            .iter()
            .find(|s| s.map == map && s.file == file && s.source == Source::Saved)
        else {
            return Err(format!(
                "no saved map variant {} (the Forge variants kind lists them; one from Live is duplicated in the game first)",
                asset_id(map, file)
            ));
        };
        store::load(&data, map, file, Some(&s.base)).map_err(|e| e.to_string())
    }

    /// The **Forge variants** kind's rows: every map variant the store
    /// lists, of every map.
    pub fn forge_variant_list(&self, text: &str, limit: usize) -> Result<Value, String> {
        let data = self.data_root();
        let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        let mut rows = Vec::new();
        for s in saved(&data)? {
            let hay = format!("{} {} {} forge variant", s.name, s.map, s.file).to_lowercase();
            if !words.iter().all(|w| hay.contains(w.as_str())) {
                continue;
            }
            let is_saved = s.source == Source::Saved;
            let things = is_saved
                .then(|| store::load(&data, &s.map, &s.file, Some(&s.base)).ok())
                .flatten()
                .map(|doc| things(&doc).len());
            rows.push(json!({
                "asset": asset_id(&s.map, &s.file),
                "root": "forge",
                "kind": VARIANTS_KIND,
                "name": s.name,
                "map": s.map,
                "file": s.file,
                "source": if is_saved { "saved" } else { "live" },
                "editable": is_saved,
                "badge": format!("variant of {}", s.map),
                "id": s.id.to_string(),
                "base": s.base.to_string(),
                "things": things,
                "maps": [s.map],
            }));
        }
        let total = rows.len();
        rows.truncate(limit);
        Ok(json!({
            "root": "forge",
            "rows": rows,
            "total": total,
            "limit": limit,
            "truncated": total > limit,
            "maps": [],
            "kinds": super::assets::kind_names(),
            "problems": [],
        }))
    }

    /// [`super::Action::OpenAsset`] for `forge:<map>:<file>`: the variant
    /// as a document. The open one comes forward; another is refused while
    /// the open one has unsaved edits.
    pub fn open_forge_variant(&mut self, map: &str, file: &str) {
        if let Some(open) = &self.forge_variants.open {
            if open.map == map && open.file == file {
                return self.open_doc(Doc::Forge);
            }
            if open.dirty() {
                return self.error(format!(
                    "{} has unsaved edits: save it, or undo them, before opening another",
                    open.doc.title
                ));
            }
        }
        match self.forge_variant_doc(map, file) {
            Ok(doc) => {
                self.forge_variants.open = Some(Open {
                    map: map.to_owned(),
                    file: file.to_owned(),
                    saved: doc.clone(),
                    doc,
                    undo: Vec::new(),
                    redo: Vec::new(),
                    selected: None,
                });
                self.open_doc(Doc::Forge);
            }
            Err(e) => self.error(e),
        }
    }

    /// [`super::Action::Forge`].
    pub(super) fn run_forge(&mut self, action: ForgeAction) {
        let Some(open) = self.forge_variants.open.as_mut() else {
            return self.error("no Forge variant is open (shell.open {asset: forge:<map>:<file>})");
        };
        match action {
            ForgeAction::Select(target) => open.selected = target,
            ForgeAction::Set { target, field, value } => {
                let before = open.doc.clone();
                let done = parse_target(&target)
                    .and_then(|t| set_field(&mut open.doc, t, &field, value));
                match done {
                    Ok(()) if open.doc != before => {
                        open.undo.push(before);
                        open.redo.clear();
                    }
                    Ok(()) => {}
                    Err(e) => self.error(e),
                }
            }
            ForgeAction::Save => {
                let doc = open.doc.clone();
                match store::save(&self.data_root(), &doc, true) {
                    Ok(file) => {
                        let name = doc.title.clone();
                        if let Some(open) = self.forge_variants.open.as_mut() {
                            open.file = file;
                            open.saved = doc;
                        }
                        self.info(format!("saved the map variant {name}"));
                    }
                    Err(e) => self.error(e.to_string()),
                }
            }
        }
    }

    /// Undo or redo a field edit of the open variant.
    pub(super) fn forge_undo_redo(&mut self, undo: bool) {
        let Some(open) = self.forge_variants.open.as_mut() else {
            return;
        };
        let (from, to) = if undo {
            (&mut open.undo, &mut open.redo)
        } else {
            (&mut open.redo, &mut open.undo)
        };
        match from.pop() {
            Some(doc) => {
                to.push(std::mem::replace(&mut open.doc, doc));
                self.info(if undo { "undid a field edit" } else { "redid a field edit" });
            }
            None => self.info(if undo { "nothing to undo" } else { "nothing to redo" }),
        }
    }

    /// What a variant uses: the tags its things place. The open document
    /// answers for itself, edits and all.
    pub(super) fn forge_variant_uses(&self, map: &str, file: &str) -> Result<Vec<Link>, String> {
        if let Some(open) = &self.forge_variants.open
            && open.map == map
            && open.file == file
        {
            return Ok(uses(&open.doc));
        }
        Ok(uses(&self.forge_variant_doc(map, file)?))
    }

    /// The variant an asset id names, read from the open document when it
    /// is the one, else from the store.
    pub fn forge_variant_read(&self, asset: &str) -> Result<Value, String> {
        let (map, file) = split(asset)
            .ok_or_else(|| format!("{asset:?} is not a Forge variant (forge:<map>:<file>)"))?;
        if let Some(open) = &self.forge_variants.open
            && open.map == map
            && open.file == file
        {
            return Ok(open.json());
        }
        let doc = self.forge_variant_doc(map, file)?;
        let mut v = doc_json(map, file, &doc);
        v["dirty"] = json!(false);
        v["open"] = json!(false);
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_doc::{BuilderVersions, FieldWrite, MapRef, Op, Sha256};

    fn tag(group: &str, path: &str) -> FieldValue {
        FieldValue::Tag(Some(ForgeTag {
            group: group.into(),
            path: path.into(),
        }))
    }

    fn vec3(v: [f32; 3]) -> FieldValue {
        FieldValue::Vec3(v.map(|f| F32::new(f).unwrap()))
    }

    /// An invented variant: two weapons added, the second moved by a later
    /// entry write.
    pub(crate) fn doc(title: &str) -> ForgeDoc {
        let add = |id: u32, path: &str, at: [f32; 3]| Op {
            id: OpId(id),
            edit: Edit::Placement(Placement::Add {
                block: BlockKind::Weapons,
                like: None,
                fields: vec![
                    FieldWrite { field: "palette index".into(), value: tag("weap", path) },
                    FieldWrite { field: "position".into(), value: vec3(at) },
                ],
            }),
        };
        let mut doc = ForgeDoc {
            format: forge_doc::FORMAT,
            builder: BuilderVersions::CURRENT,
            base: MapRef {
                map: "fixture".into(),
                sha256: Sha256::of(b"an invented map"),
            },
            sources: vec![],
            title: title.into(),
            author: "Editor".into(),
            description: String::new(),
            recipes: vec![],
            imports: vec![],
            ops: vec![
                add(1, "objects\\invented\\crate", [1.0, 2.0, 3.0]),
                add(2, "objects\\invented\\barrel", [4.0, 5.0, 6.0]),
                Op {
                    id: OpId(3),
                    edit: Edit::Entry(EntryOp::Set {
                        target: Endpoint::Added(OpId(2)),
                        write: FieldWrite { field: "position".into(), value: vec3([7.0, 8.0, 9.0]) },
                    }),
                },
            ],
            expect: None,
            waves: None,
        };
        doc.format = doc.required_format();
        doc
    }

    #[test]
    fn the_things_are_the_added_entries_with_the_last_value_of_each_field() {
        let doc = doc("Castle");
        let rows = things(&doc);
        assert_eq!(rows.len(), 2);
        assert_eq!(target_text(rows[0].target), "added:1");
        assert_eq!(rows[1].fields[1], ("position".to_owned(), vec3([7.0, 8.0, 9.0])));
        assert_eq!(
            rows[0].item().map(|t| t.path),
            Some("objects\\invented\\crate".to_owned())
        );
        assert_eq!(parse_target("added:2"), Ok(Endpoint::Added(OpId(2))));
        let own = target_text(Endpoint::base(BlockKind::Weapons, 4));
        assert_eq!(parse_target(&own).map(target_text), Ok(own));
        assert!(parse_target("crate 4").is_err());
    }

    #[test]
    fn a_field_edit_replaces_the_write_that_gives_the_value_and_keeps_its_type() {
        let mut doc = doc("Castle");
        // The second weapon's position comes from the entry write, the
        // first's from its add.
        set_field(&mut doc, Endpoint::Added(OpId(2)), "position", vec3([0.0, 0.0, 1.0])).unwrap();
        set_field(&mut doc, Endpoint::Added(OpId(1)), "position", vec3([9.0, 9.0, 9.0])).unwrap();
        let rows = things(&doc);
        assert_eq!(rows[1].fields[1].1, vec3([0.0, 0.0, 1.0]));
        assert_eq!(rows[0].fields[1].1, vec3([9.0, 9.0, 9.0]));
        assert_eq!(doc.ops.len(), 3, "an edit adds no op");
        // Another type, a field the variant does not write and an entry it
        // does not have are refused, and the document is as it was.
        let before = doc.clone();
        for (target, field, value) in [
            (Endpoint::Added(OpId(1)), "position", FieldValue::Int(3)),
            (Endpoint::Added(OpId(1)), "scale", vec3([1.0, 1.0, 1.0])),
            (Endpoint::Added(OpId(9)), "position", vec3([1.0, 1.0, 1.0])),
        ] {
            assert!(set_field(&mut doc, target, field, value).is_err(), "{field}");
        }
        assert_eq!(doc, before);
    }

    #[test]
    fn values_read_from_json_and_text_in_the_fields_own_type() {
        let position = vec3([1.0, 2.0, 3.0]);
        assert_eq!(value_json(&position), json!([1.0, 2.0, 3.0]));
        assert_eq!(value_from_json(&position, &json!([4, 5, 6])), Ok(vec3([4.0, 5.0, 6.0])));
        assert_eq!(value_from_text(&position, "4, 5 6"), Ok(vec3([4.0, 5.0, 6.0])));
        assert!(value_from_json(&position, &json!([4, 5])).is_err());
        let item = tag("weap", "objects\\invented\\crate");
        assert_eq!(value_json(&item), json!("weap:objects\\invented\\crate"));
        assert_eq!(
            value_from_text(&item, "tag:weap:objects/invented/barrel"),
            Ok(tag("weap", "objects\\invented\\barrel"))
        );
        assert_eq!(value_from_text(&item, "none"), Ok(FieldValue::Tag(None)));
        assert_eq!(value_from_text(&FieldValue::Int(1), "12"), Ok(FieldValue::Int(12)));
        assert!(value_from_text(&FieldValue::Int(1), "1.5").is_err());
        assert_eq!(value_from_text(&FieldValue::Bool(false), "yes"), Ok(FieldValue::Bool(true)));
    }

    #[test]
    fn a_variant_uses_the_tags_it_places_and_a_tag_counts_its_places() {
        let doc = doc("Castle");
        let key = TagKey::new(map::FourCC(*b"weap"), "objects\\invented\\crate");
        assert_eq!(places(&doc, &key), 1);
        let other = TagKey::new(map::FourCC(*b"weap"), "objects\\invented\\nothing");
        assert_eq!(places(&doc, &other), 0);
        let links = uses(&doc);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].asset, "tag:weap:objects\\invented\\crate");
        assert_eq!(links[0].via, "1 placed");
    }
}
