//! The field panel's row model: one element of a tag as rows a person can
//! read, with no UI calls. The panel draws these rows and `tag.fields`
//! returns them, so the two never disagree (`docs/editor/tag-editing.md`,
//! "One row model").
use std::collections::BTreeMap;

use map::TagId;
use serde_json::{Value, json};
use tags::defs::FieldView;
use tags::registry::{Field, Hint, Kind, Layout, Unit};

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

/// What became of a changed field with a game connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sending {
    /// Edit is on and the game took it.
    Sent,
    /// Viewing: the change waits in Pending until Edit is turned on.
    Waiting,
    /// Sent, and the game reads it when the map reloads.
    WaitingForReload,
    /// The game refused it, or holds another value (`live` says which).
    NotSent,
}

impl Sending {
    /// The id in `tag.fields`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Waiting => "waiting",
            Self::WaitingForReload => "waiting_for_reload",
            Self::NotSent => "not_sent",
        }
    }

    /// The row's chip: `Sent` with when the game reads it (`Sent · Not
    /// known`), else the state in words.
    pub fn chip(self, effect: Effect) -> &'static str {
        match (self, effect) {
            (Self::Sent, Effect::Now) => "Sent · Now",
            (Self::Sent, Effect::NextUse) => "Sent · Next use",
            (Self::Sent, Effect::NextSpawn) => "Sent · Next spawn",
            (Self::Sent, Effect::Reload) => "Sent · After reload",
            (Self::Sent, Effect::NotKnown) => "Sent · Not known",
            (Self::Waiting, _) => "Waiting",
            (Self::WaitingForReload, _) => "Waiting for reload",
            (Self::NotSent, _) => "Not sent",
        }
    }
}

/// A changed field's [`Sending`]: none unless a game is connected and the
/// field is changed and editable. `refused` is a mismatch or refusal mark
/// on the field; `reloading` that the tag has a change waiting for a
/// reload.
pub fn sending(
    link: Link,
    edited: bool,
    read_only: bool,
    effect: Effect,
    refused: bool,
    reloading: bool,
) -> Option<Sending> {
    if !link.connected() || !edited || read_only {
        return None;
    }
    Some(if refused {
        Sending::NotSent
    } else if link == Link::Viewing {
        Sending::Waiting
    } else if effect == Effect::Reload && reloading {
        Sending::WaitingForReload
    } else {
        Sending::Sent
    })
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
    /// The chip at the row's right edge: only with a game connected; a
    /// changed field's [`Sending`], else when a change takes effect (for a
    /// `not_known` field, none).
    pub chip: Option<&'static str>,
    /// A changed field's state in the connected game.
    pub sending: Option<Sending>,
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
    let now = doc.read(tag, a, width);
    let edited = match (&now, doc.read_source(tag, a, width)) {
        (Some(now), Some(file)) => now.as_slice() != file,
        (Some(_), None) => doc.is_changed(tag),
        _ => false,
    };
    let word = |at: usize, n: usize| now.as_deref().map_or(0, |b| le(&b[at..at + n]));
    let scene = matches!(f.kind, Kind::Block(_)) && doc.scene_block(tag, a).is_some();
    let read_only = reason(&f.kind, scene, refusal);
    let (effect, why) = tag_sync::effect(doc, tag, a, f);
    let live = cx
        .link
        .connected()
        .then(|| cx.issues.tag_field(tag, a).next())
        .flatten()
        .map(|issue| Live {
            kind: issue.kind.name(),
            text: issue.text.clone(),
        });
    let state = sending(
        cx.link,
        edited,
        read_only.is_some(),
        effect,
        live.is_some(),
        cx.issues.tag_waiting(tag),
    );
    let shown = match state {
        Some(s) => Some(s.chip(effect)),
        None => chip(cx.link, effect, edited, read_only.is_some()),
    };
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
        Kind::Enum { bytes: n, values } => (
            None,
            Some(choice_text(values, word(0, usize::from(*n)) as u32)),
        ),
        Kind::Flags { bytes: n, bits } => {
            let n = usize::from(*n);
            (None, Some(switches_text(bits, word(0, n), 8 * n as u32)))
        }
        Kind::TagRef { .. } => (None, Some(reference_text(doc, word(4, 4) as u32))),
        Kind::Datum { .. } => (None, Some(reference_text(doc, word(0, 4) as u32))),
        _ => (None, None),
    };
    Row {
        field: f,
        address: a,
        label: label(f),
        // A colour field of any form reads as a colour.
        type_words: if super::colour::form(f).is_some() {
            "Colour"
        } else {
            type_words(&f.kind)
        },
        unit: unit(f),
        edited,
        read_only,
        effect,
        why,
        chip: shown,
        sending: state,
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
    if let Some(label) = f.label.as_ref().filter(|_| f.named) {
        return label.clone();
    }
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

/// The unit in words: the definition's `unit=`, else `degrees` for an
/// angle (stored in radians, shown in degrees).
pub fn unit(f: &Field) -> Option<&'static str> {
    if let Some(u) = f.unit {
        return Some(unit_words(u));
    }
    match f.kind {
        Kind::Float {
            hint: Hint::Angle, ..
        } => Some("degrees"),
        _ => None,
    }
}

/// A unit as the row writes it after the value.
pub fn unit_words(u: Unit) -> &'static str {
    match u {
        Unit::Seconds => "seconds",
        Unit::Ticks => "ticks",
        Unit::WorldUnits => "world units",
        Unit::WorldUnitsPerSecond => "world units per second",
        Unit::Degrees => "degrees",
        Unit::DegreesPerSecond => "degrees per second",
        Unit::Fraction => "fraction",
        Unit::Percent => "percent",
        Unit::Pixels => "pixels",
        Unit::Count => "count",
        Unit::Index => "index",
    }
}

/// Little-endian bytes as a number.
pub(crate) fn le(b: &[u8]) -> u64 {
    b.iter().rev().fold(0, |v, x| v << 8 | u64::from(*x))
}

/// Whether an enum value or flag bit name names something.
pub fn is_name(name: &str) -> bool {
    !name.is_empty() && name != "_"
}

/// What a choice row shows: the value's name in words, or `Value 7 (not
/// named)`.
pub fn choice_text(values: &[String], v: u32) -> String {
    match values.get(v as usize).filter(|n| is_name(n)) {
        Some(name) => words(name),
        None => format!("Value {v} (not named)"),
    }
}

/// A bit as the panel names it: its name in words, or `Bit 7`.
pub fn bit_text(bits: &[String], bit: u32) -> String {
    match bits.get(bit as usize).filter(|n| is_name(n)) {
        Some(name) => words(name),
        None => format!("Bit {bit}"),
    }
}

/// What a switches row shows: how many named switches are on, and which
/// (`2 of 9 on: Loud, Quiet`); an unnamed bit that is set is listed by
/// number. With no names, the count is of every bit.
pub fn switches_text(bits: &[String], v: u64, width: u32) -> String {
    let named_bits = (0..width).filter(|&b| bits.get(b as usize).is_some_and(|n| is_name(n)));
    let total = match named_bits.clone().count() {
        0 => width as usize,
        n => n,
    };
    let on: Vec<u32> = (0..width).filter(|b| v >> b & 1 == 1).collect();
    let counted = if named_bits.clone().count() == 0 {
        on.len()
    } else {
        named_bits.filter(|b| v >> b & 1 == 1).count()
    };
    let mut out = format!("{counted} of {total} on");
    if !on.is_empty() {
        let names: Vec<String> = on.iter().map(|&b| bit_text(bits, b)).collect();
        out.push_str(": ");
        out.push_str(&names.join(", "));
    }
    out
}

/// A tag kind in words: the group's `title=`, else its code.
pub fn kind_words(doc: &TagDoc, group: map::FourCC) -> String {
    match doc.registry().group(group).and_then(|l| l.title.as_ref()) {
        Some(tags::registry::Title::Text(t)) => t.clone(),
        _ => tags::defs::fourcc_text(group),
    }
}

/// What a tag reference row shows: the kind and the last segment of the
/// tag's path (`Model  blaster`), `No tag`, or the datum of a tag the map
/// lacks.
pub fn reference_text(doc: &TagDoc, id: u32) -> String {
    if id == u32::MAX {
        return "No tag".to_owned();
    }
    match doc.tag(TagId(id)) {
        Some(t) => {
            let last = t.name.rsplit('\\').next().unwrap_or(&t.name);
            format!("{}  {last}", kind_words(doc, t.group))
        }
        None => format!("Missing tag {id:#010x}"),
    }
}

/// Fields of a tag [`unsaved`] looks at, at most.
const UNSAVED_BUDGET: usize = 20_000;

/// How many of a tag's fields differ from the loaded file, list elements
/// included: the header's `3 unsaved changes`. A list whose size or place
/// changed counts once (its new elements are not in the file to compare).
/// Zero when the tag is unchanged; none when the tag is too big to count.
pub fn unsaved(doc: &TagDoc, tag: TagId) -> Option<usize> {
    if !doc.is_changed(tag) {
        return Some(0);
    }
    let (t, l) = (doc.tag(tag)?, doc.layout(tag)?);
    let mut budget = UNSAVED_BUDGET;
    changed_in(doc, tag, t.address, l, &mut budget)
}

fn changed_in(doc: &TagDoc, tag: TagId, at: u32, l: &Layout, budget: &mut usize) -> Option<usize> {
    let mut n = 0;
    for f in &l.fields {
        *budget = budget.checked_sub(1)?;
        let a = at + f.offset;
        let w = f.kind.width();
        if let (Some(now), Some(file)) = (doc.read(tag, a, w), doc.read_source(tag, a, w))
            && now.as_slice() != file
        {
            n += 1;
        }
        if let Kind::Block(child) = &f.kind
            && let Some((count, first)) = doc.block(tag, a, child)
        {
            for i in 0..count {
                n += changed_in(doc, tag, first + i * child.size, child, budget)?;
            }
        }
    }
    Some(n)
}

/// `1 unsaved change`, `3 unsaved changes`; none for no change. A tag too
/// big to count reads `Unsaved changes`.
pub fn unsaved_words(n: Option<usize>) -> Option<String> {
    match n {
        Some(0) => None,
        Some(1) => Some("1 unsaved change".to_owned()),
        Some(n) => Some(format!("{n} unsaved changes")),
        None => Some("Unsaved changes".to_owned()),
    }
}

/// A section rule's words: the embedded struct's `title=`, else the last
/// step of its path in words (`unit.obje` reads `Obje`).
pub fn section_title(layout: &Layout, section: &str) -> String {
    match layout.section_titles.get(section) {
        Some(title) => title.clone(),
        None => words(section.rsplit('.').next().unwrap_or(section)),
    }
}

/// An element's title: the value of its block's `title=` field, else of
/// its first named name or tag reference; none when that has no value or
/// the element has neither (the row shows the index alone).
pub fn element_title(doc: &TagDoc, tag: TagId, at: u32, layout: &Layout) -> Option<String> {
    let by_title = match &layout.title {
        Some(tags::registry::Title::Field(name)) => layout.fields.iter().find(|f| &f.name == name),
        _ => None,
    };
    let f = by_title.or_else(|| {
        layout
            .fields
            .iter()
            .find(|f| f.named && matches!(f.kind, Kind::StringId | Kind::TagRef { .. }))
    })?;
    title_text(doc, tag, at, f)
}

/// A field's value as an element title: a name's text, a tag's last path
/// segment, a choice's name, else the value as `tag.read` gives it.
fn title_text(doc: &TagDoc, tag: TagId, at: u32, f: &Field) -> Option<String> {
    let a = at + f.offset;
    let word = |at: u32| doc.read(tag, at, 4).map(|b| le(&b) as u32);
    let tag_name = |id: u32| {
        doc.tag(TagId(id))
            .map(|t| t.name.rsplit('\\').next().unwrap_or(&t.name).to_owned())
    };
    match &f.kind {
        Kind::StringId => doc
            .string(word(a)?)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        Kind::TagRef { .. } => tag_name(word(a + 4)?),
        Kind::Datum { .. } => tag_name(word(a)?),
        Kind::Enum { bytes: n, values } => {
            let v = le(&doc.read(tag, a, u32::from(*n))?) as u32;
            Some(choice_text(values, v))
        }
        Kind::Block(_) | Kind::Data | Kind::Pad { .. } | Kind::Unknown { .. } => None,
        _ => match tag_api::value(doc, tag, at, f) {
            Value::Null => None,
            Value::String(text) => Some(text),
            other => Some(other.to_string()),
        },
    }
}

/// An element row's words: `1  scope`, or the index alone.
pub fn element_text(index: u32, title: Option<&str>) -> String {
    match title {
        Some(t) => format!("{index}  {t}"),
        None => index.to_string(),
    }
}

/// The indices of a list's elements whose row words match `filter` (every
/// word, case-insensitive): all of them for an empty filter, without
/// reading a title.
pub fn matching(doc: &TagDoc, tag: TagId, list: &List, child: &Layout, filter: &str) -> Vec<u32> {
    let wanted: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    if wanted.is_empty() {
        return (0..list.count).collect();
    }
    (0..list.count)
        .filter(|&i| {
            let title = element_title(doc, tag, list.address + i * child.size, child);
            let text = element_text(i, title.as_deref()).to_lowercase();
            wanted.iter().all(|w| text.contains(w.as_str()))
        })
        .collect()
}

/// The breadcrumbs of an element shown alone: the tag (`weap:
/// objects\test\blaster`), then each list step (`Parts 1  scope`), with
/// the element path each returns to (none for the root).
pub fn breadcrumbs(
    doc: &TagDoc,
    tag: TagId,
    steps: &[tag_api::ElementStep],
) -> Vec<(String, Option<String>)> {
    let mut out = vec![(doc.tag_label(tag.0), None)];
    for s in steps {
        let title = element_title(doc, tag, s.at, &s.layout);
        let text = format!(
            "{} {}",
            label(&s.field),
            element_text(s.index, title.as_deref())
        );
        out.push((text, Some(s.path.clone())));
    }
    out
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
            "view": super::rich::view_json(f),
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
            "sending": self.sending.map(Sending::id),
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
            // A function row: its curve's shape (null when the data is not
            // a function blob; the row says why).
            Kind::Data if f.view.is_some_and(|v| v == FieldView::Function) => {
                out["function"] = super::curve::row_json(doc, tag, self.address);
            }
            // A colour row: its form, hex, channels and (hdr) intensity.
            _ => {
                if let Some(c) = super::colour::row_json(doc, tag, self.address, f) {
                    out["colour"] = c;
                }
            }
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
