//! The field panel's row model: one element of a tag as rows a person can
//! read, with no UI calls. The panel draws these rows and `tag.fields`
//! returns them, so the two never disagree (`docs/editor/tag-editing.md`,
//! "One row model").
use std::collections::BTreeMap;

use map::TagId;
use serde_json::{Value, json};
use tags::registry::{Field, Hint, Kind, Layout};

use crate::App;
use crate::app::TagDoc;
use crate::app::tag_api;
use crate::app::tag_doc::STRUCTURAL;
use crate::play::LinkState;
use crate::play::issues::IssueIndex;
use crate::play::tag_sync::{self, Effect};

/// Why a scenario placement list is read-only in the Tags tool.
pub const EDITED_IN_SCENE: &str = "Edited in Scene.";
/// Why padding is read-only.
pub const PADDING: &str = "Padding: shown, not edited.";

/// The line over the open Unnamed fields group of an element nothing
/// names.
pub const NO_NAMES: &str = "No field of this kind of tag has a name yet.";

/// Words written in upper case in a derived label.
const ACRONYMS: [&str; 7] = ["lod", "hud", "ai", "bsp", "id", "fov", "uv"];

/// The game link as the panel shows it: effects show only with a game
/// connected on the same map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Link {
    /// No game, a game on another map, or a browsed map.
    #[default]
    Off,
    Viewing,
    Editing,
}

impl Link {
    /// The open map's link, as the bridge last published it.
    pub fn of(app: &App) -> Self {
        if app.browsing().is_some() {
            return Self::Off;
        }
        match app.pairing.link.as_ref().map(|l| &l.state) {
            Some(LinkState::Viewing { .. }) => Self::Viewing,
            Some(LinkState::Editing { .. }) => Self::Editing,
            _ => Self::Off,
        }
    }

    pub fn connected(self) -> bool {
        self != Self::Off
    }
}

/// What a row needs besides its element.
pub struct Context<'a> {
    pub link: Link,
    /// The link's marks (empty with no game).
    pub issues: &'a IssueIndex,
    /// The element's `tag.set` path: empty for the root, else ending in a
    /// dot (`parts[1].`).
    pub prefix: &'a str,
}

/// A changed field's state in the connected game.
#[derive(Clone, Debug, PartialEq)]
pub struct Live {
    /// `mismatch` or `refused` (`play::issues::Kind`).
    pub kind: &'static str,
    pub text: String,
}

/// A list's elements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct List {
    pub count: u32,
    /// The first element's address.
    pub address: u32,
    /// Elements its source has room for.
    pub capacity: u32,
}

/// One field of an element, as the panel shows it.
pub struct Row<'a> {
    pub field: &'a Field,
    /// The field's address.
    pub address: u32,
    /// The name in words.
    pub label: String,
    pub type_words: &'static str,
    pub unit: Option<&'static str>,
    /// Whether the value differs from the loaded file.
    pub edited: bool,
    /// Why the field is not edited here, as a sentence.
    pub read_only: Option<&'static str>,
    pub effect: Effect,
    pub why: &'static str,
    /// The chip at the row's right edge: only with a game connected, and
    /// for a `not_known` field only once it is changed.
    pub chip: Option<&'static str>,
    pub live: Option<Live>,
    /// The `tag.set` path.
    pub path: String,
    /// A list's or data's size in words (`2 items`, `16 bytes`).
    pub summary: Option<String>,
    pub list: Option<List>,
}

/// What the panel drew for a row, kept for tests and the agent API to
/// compare with [`Row::json`].
#[derive(Clone, Debug, PartialEq)]
pub struct Drawn {
    pub path: String,
    pub label: String,
    pub edited: bool,
    pub chip: Option<&'static str>,
    pub summary: Option<String>,
}

/// The rows of the element at `at` laid out by `layout`, one per field in
/// offset order.
pub fn rows<'a>(
    doc: &TagDoc,
    tag: TagId,
    at: u32,
    layout: &'a Layout,
    cx: &Context,
) -> Vec<Row<'a>> {
    let refusal = doc.refusal(tag);
    layout
        .fields
        .iter()
        .map(|f| row(doc, tag, at, f, refusal, cx))
        .collect()
}

fn row<'a>(
    doc: &TagDoc,
    tag: TagId,
    at: u32,
    f: &'a Field,
    refusal: Option<&'static str>,
    cx: &Context,
) -> Row<'a> {
    let a = at + f.offset;
    let width = f.kind.width();
    let edited = match (doc.read(tag, a, width), doc.read_source(tag, a, width)) {
        (Some(now), Some(file)) => now.as_slice() != file,
        (Some(_), None) => doc.is_changed(tag),
        _ => false,
    };
    let scene = matches!(f.kind, Kind::Block(_)) && doc.scene_block(tag, a).is_some();
    let read_only = reason(&f.kind, scene, refusal);
    let (effect, why) = tag_sync::effect(doc, tag, a, f);
    let shown = chip(cx.link, effect, edited, read_only.is_some());
    let live = cx
        .link
        .connected()
        .then(|| cx.issues.tag_field(tag, a).next())
        .flatten()
        .map(|issue| Live {
            kind: issue.kind.name(),
            text: issue.text.clone(),
        });
    let (list, summary) = match &f.kind {
        Kind::Block(child) => match doc.block(tag, a, child) {
            Some((count, address)) => {
                let capacity = doc.capacity(tag, a);
                let list = List {
                    count,
                    address,
                    capacity,
                };
                (Some(list), Some(items(count, capacity)))
            }
            None => (None, None),
        },
        Kind::Data => {
            let n = doc
                .read(tag, a, 4)
                .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            (None, Some(bytes(n)))
        }
        _ => (None, None),
    };
    Row {
        field: f,
        address: a,
        label: label(f),
        type_words: type_words(&f.kind),
        unit: unit(f),
        edited,
        read_only,
        effect,
        why,
        chip: shown,
        live,
        path: format!("{}{}", cx.prefix, f.name),
        summary,
        list,
    }
}

/// The read-only sentence: a placement list belongs to Scene, a refused
/// tag gives its reason, and internal values and padding are shown only.
pub fn reason(kind: &Kind, scene: bool, refusal: Option<&'static str>) -> Option<&'static str> {
    if scene {
        Some(EDITED_IN_SCENE)
    } else if refusal.is_some() {
        refusal
    } else {
        match kind {
            Kind::Raw | Kind::RawSize { .. } | Kind::Bsp => Some(STRUCTURAL),
            Kind::Pad { .. } => Some(PADDING),
            _ => None,
        }
    }
}

/// The chip a row shows: none with no game or on a read-only row; a
/// known effect (or `reload`) always; `not_known` once changed.
pub fn chip(link: Link, effect: Effect, edited: bool, read_only: bool) -> Option<&'static str> {
    if !link.connected() || read_only || (effect == Effect::NotKnown && !edited) {
        None
    } else {
        Some(effect.chip())
    }
}

/// The label: a named field's name in words; an unnamed one by its type
/// and offset (`Value at +0x14`, `Padding, 8 bytes at +0x18`).
pub fn label(f: &Field) -> String {
    if f.named {
        return words(&f.name);
    }
    match f.kind {
        Kind::Pad { len } => format!("Padding, {} at +{:#x}", bytes(len), f.offset),
        Kind::Unknown { .. } => format!("Value at +{:#x}", f.offset),
        ref k => format!("{} at +{:#x}", type_words(k), f.offset),
    }
}

/// A name in words: underscores as spaces, the first letter upper case,
/// and [`ACRONYMS`] upper case.
pub fn words(name: &str) -> String {
    let mut out = String::new();
    for (i, w) in name.split('_').filter(|w| !w.is_empty()).enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if ACRONYMS.contains(&w.to_ascii_lowercase().as_str()) {
            out.push_str(&w.to_ascii_uppercase());
        } else if i == 0 {
            let mut c = w.chars();
            if let Some(first) = c.next() {
                out.extend(first.to_uppercase());
                out.push_str(c.as_str());
            }
        } else {
            out.push_str(w);
        }
    }
    out
}

/// The type in words, for tooltips, details and `tag.fields`.
pub fn type_words(kind: &Kind) -> &'static str {
    match kind {
        Kind::Int { .. } => "Whole number",
        Kind::Float { hint, .. } => match hint {
            Hint::Plain => "Decimal number",
            Hint::Angle => "Angle",
            Hint::Colour => "Colour",
            Hint::Bounds => "Range",
        },
        Kind::Enum { .. } => "Choice",
        Kind::Flags { .. } => "Switches",
        Kind::StringId => "Name",
        Kind::TagRef { .. } | Kind::Datum { .. } => "Tag reference",
        Kind::Block(_) => "List",
        Kind::Data => "Bytes",
        Kind::Raw | Kind::RawSize { .. } | Kind::Bsp | Kind::Word => "Internal value",
        Kind::Unknown { .. } => "Unnamed value",
        Kind::Pad { .. } => "Padding",
    }
}

/// The unit word: angles are stored in radians and shown in degrees.
pub fn unit(f: &Field) -> Option<&'static str> {
    match f.kind {
        Kind::Float {
            hint: Hint::Angle, ..
        } => Some("degrees"),
        _ => None,
    }
}

/// A list's size in words.
pub fn items(count: u32, capacity: u32) -> String {
    let n = match count {
        0 => "Empty".to_owned(),
        1 => "1 item".to_owned(),
        n => format!("{n} items"),
    };
    if capacity > count {
        format!("{n}, room for {capacity}")
    } else {
        n
    }
}

fn bytes(n: u32) -> String {
    if n == 1 {
        "1 byte".to_owned()
    } else {
        format!("{n} bytes")
    }
}

/// An element's rows split as the panel draws them: the named rows, then
/// the Unnamed fields group in offset order with padding last.
pub fn split<'r, 'a>(rows: &'r [Row<'a>]) -> (Vec<&'r Row<'a>>, Vec<&'r Row<'a>>) {
    let (named, mut unnamed): (Vec<_>, Vec<_>) = rows.iter().partition(|r| r.field.named);
    unnamed.sort_by_key(|r| matches!(r.field.kind, Kind::Pad { .. }));
    (named, unnamed)
}

/// The Unnamed fields group's row label.
pub fn group_label(n: usize) -> String {
    format!("Unnamed fields ({n})")
}

/// Whether an element's Unnamed fields group opens by itself: **Show
/// unnamed fields** is on, nothing in the element has a name, or the group
/// holds a changed field, a field with a mark, or the focused field
/// (`focus`, an address).
pub fn opens(unnamed: &[&Row], toggle: bool, no_names: bool, focus: Option<u32>) -> bool {
    toggle
        || no_names
        || unnamed
            .iter()
            .any(|r| r.edited || r.live.is_some() || focus == Some(r.address))
}

/// What a group holds, counted by type in words (`1 tag reference, 8
/// values`), padding last as its bytes.
pub fn summary<'f>(fields: impl IntoIterator<Item = &'f Field>) -> String {
    let mut counts: BTreeMap<u8, (u32, &'static str, &'static str)> = BTreeMap::new();
    let mut padding = 0;
    for f in fields {
        if let Kind::Pad { len } = f.kind {
            padding += len;
            continue;
        }
        let (order, one, many) = noun(&f.kind);
        counts.entry(order).or_insert((0, one, many)).0 += 1;
    }
    let mut parts: Vec<String> = counts
        .into_values()
        .map(|(n, one, many)| format!("{n} {}", if n == 1 { one } else { many }))
        .collect();
    if padding > 0 {
        parts.push(format!("{} of padding", bytes(padding)));
    }
    parts.join(", ")
}

/// A kind's place in a summary and its noun, one and many.
fn noun(kind: &Kind) -> (u8, &'static str, &'static str) {
    match kind {
        Kind::TagRef { .. } | Kind::Datum { .. } => (0, "tag reference", "tag references"),
        Kind::Block(_) => (1, "list", "lists"),
        Kind::Data => (2, "data reference", "data references"),
        Kind::StringId => (3, "name", "names"),
        Kind::Enum { .. } => (4, "choice", "choices"),
        Kind::Flags { .. } => (5, "set of switches", "sets of switches"),
        Kind::Int { .. } => (6, "whole number", "whole numbers"),
        Kind::Float { hint, .. } => match hint {
            Hint::Plain => (7, "decimal number", "decimal numbers"),
            Hint::Angle => (8, "angle", "angles"),
            Hint::Colour => (9, "colour", "colours"),
            Hint::Bounds => (10, "range", "ranges"),
        },
        Kind::Unknown { .. } => (11, "value", "values"),
        Kind::Raw | Kind::RawSize { .. } | Kind::Bsp | Kind::Word | Kind::Pad { .. } => {
            (12, "internal value", "internal values")
        }
    }
}

impl Row<'_> {
    /// What the panel draws for this row.
    pub fn drawn(&self) -> Drawn {
        Drawn {
            path: self.path.clone(),
            label: self.label.clone(),
            edited: self.edited,
            chip: self.chip,
            summary: self.summary.clone(),
        }
    }

    /// The row as `tag.fields` returns it. A list's `value` is its count.
    pub fn json(&self, doc: &TagDoc, tag: TagId, at: u32) -> Value {
        let f = self.field;
        let width = f.kind.width();
        let value = match (&f.kind, self.list) {
            (Kind::Block(_), list) => json!(list.map(|l| l.count)),
            _ => tag_api::value(doc, tag, at, f),
        };
        let file_value = match (&f.kind, doc.read_source(tag, self.address, width)) {
            (_, None) | (Kind::Data, _) => Value::Null,
            _ if !self.edited => Value::Null,
            (Kind::Block(_), Some(b)) => json!(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            (_, Some(b)) => tag_api::value_of(doc, tag, at, f, b),
        };
        let mut out = json!({
            "name": f.name,
            "offset": f.offset,
            "kind": f.kind.label(),
            "size": width,
            "value": value,
            "description": f.doc,
            "evidence": f.meaning,
            "section": f.section,
            "label": self.label,
            "named": f.named,
            "type": self.type_words,
            "unit": self.unit,
            "editable": self.read_only.is_none(),
            "read_only": self.read_only,
            "edited": self.edited,
            "file_value": file_value,
            "effect": {
                "id": self.effect.id(),
                "text": self.effect.sentence(),
                "why": self.why,
            },
            "chip": self.chip,
            "live": self.live.as_ref().map(|l| json!({"kind": l.kind, "text": l.text})),
            "path": self.path,
            "summary": self.summary,
        });
        match &f.kind {
            Kind::Enum { values, .. } => {
                out["choices"] = named(values, "value");
            }
            Kind::Flags { bits, .. } => {
                out["bits"] = named(bits, "bit");
            }
            Kind::TagRef { groups } | Kind::Datum { groups } => {
                let g: Vec<String> = groups.iter().map(|g| tags::defs::fourcc_text(*g)).collect();
                out["groups"] = json!(g);
            }
            Kind::Block(_) => {
                out["count"] = json!(self.list.map(|l| l.count));
                out["capacity"] = json!(self.list.map(|l| l.capacity));
            }
            _ => {}
        }
        out
    }
}

/// An enum's values or a flag set's bits that have names, by number.
fn named(names: &[String], key: &str) -> Value {
    Value::Array(
        names
            .iter()
            .enumerate()
            .filter(|(_, n)| !n.is_empty() && n.as_str() != "_")
            .map(|(i, n)| json!({ key: i, "name": n }))
            .collect(),
    )
}

#[cfg(test)]
mod tests;
