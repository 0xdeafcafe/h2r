//! Compare whole fields against the loaded file plus acknowledged game writes.
//! The sparse baseline keeps addresses after undo removes a document patch.
//!
//! Also the game side of the fan-out ([`super::Play::fan_out`],
//! docs/editor/live-editors.md): each frame it takes the document's
//! changes, has `app::live` route them, and carries out each delivery
//! over the link. A Pending row says its fan-out class (`class`) beside
//! its effect.
use super::link::Purpose;
use super::playtest::{self, Waiting};
use super::{raw_sync, sync};
use crate::app::live::{Change, Class, Host, Route, Target, View};
use crate::app::{App, TagDoc};
use agent_proto::hot::encode_base64;
use map::TagId;
use scenario_edit::BspPatch;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tags::registry::{Field, Kind, Layout};

/// When a change to a field takes effect in a running game
/// (docs/editor/tag-editing.md, "When it takes effect"): an id for the
/// API, a short chip for a row and a sentence for tooltips, details and
/// the Pending tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Effect {
    /// The game reads the field each frame (`applies=frame`).
    Now,
    /// The game reads it the next time it uses it (`applies=use`).
    NextUse,
    /// The game copies it into an object when the object is made
    /// (`applies=creation`).
    NextSpawn,
    /// The game reads it at map load: the change needs a reload.
    Reload,
    /// No definition says when the game reads it (no `applies`).
    NotKnown,
}

impl Effect {
    pub const ALL: [Self; 5] = [
        Self::Now,
        Self::NextUse,
        Self::NextSpawn,
        Self::Reload,
        Self::NotKnown,
    ];

    /// The API's id (`mirror.pending`, `tag.fields`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Now => "now",
            Self::NextUse => "next_use",
            Self::NextSpawn => "next_spawn",
            Self::Reload => "reload",
            Self::NotKnown => "not_known",
        }
    }

    /// The effect an id names.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.id() == id)
    }

    /// The row's chip.
    pub fn chip(self) -> &'static str {
        match self {
            Self::Now => "Now",
            Self::NextUse => "Next use",
            Self::NextSpawn => "Next spawn",
            Self::Reload => "After reload",
            Self::NotKnown => "Not known",
        }
    }

    /// The sentence for tooltips, details and the Pending tab's headings
    /// (a reload's reason follows it, apart).
    pub fn sentence(self) -> &'static str {
        match self {
            Self::Now => "Applies now.",
            Self::NextUse => "Applies the next time the game uses it.",
            Self::NextSpawn => "Applies when the object next spawns.",
            Self::Reload => "Applies after reload.",
            Self::NotKnown => "Not known; reload to be sure.",
        }
    }

    /// A pending row's effect (`effect`, its id).
    pub fn of_row(row: &Value) -> Option<Self> {
        row["effect"].as_str().and_then(Self::from_id)
    }

    /// Whether `row` waits for a reload.
    pub fn is_reload(row: &Value) -> bool {
        Self::of_row(row) == Some(Self::Reload)
    }

    /// The effect a definition's `applies` word gives.
    pub fn of_applies(applies: Option<&str>) -> Self {
        match applies {
            Some("frame") => Self::Now,
            Some("use") => Self::NextUse,
            Some("creation") => Self::NextSpawn,
            _ => Self::NotKnown,
        }
    }
}

#[derive(Default, Clone)]
pub struct TagBaseline {
    bytes: BTreeMap<u32, u8>,
    bsp_patches: Vec<BspPatch>,
    // Operations already baked into a Play copy, including provisional
    // addresses whose actual storage was allocated by the map writer.
    structural: Vec<(String, Vec<(u32, u32)>)>,
    strings: Vec<String>,
    /// What the game holds of each block it was given by a move, by
    /// `(tag, pair)`; a block not here holds the source's.
    blocks: BTreeMap<(u32, u32), Held>,
}

/// A block as the game holds it.
#[derive(Clone, Debug, PartialEq)]
struct Held {
    count: u32,
    /// Where the elements are in the game, when known: a play copy's
    /// grown blocks lie where its writer put them, which the document
    /// does not learn.
    address: Option<u32>,
    bytes: Vec<u8>,
    size: u32,
}

impl TagBaseline {
    pub fn of(doc: &TagDoc) -> Self {
        let mut base = Self {
            bsp_patches: doc.bsp_patches(),
            ..Self::default()
        };
        for (at, bytes) in doc.patches() {
            base.wrote(at, &bytes);
        }
        for (label, ranges) in doc.structural_edits() {
            for &(at, n) in ranges {
                if let Some(bytes) = doc.read_metadata(at, n) {
                    base.wrote(at, &bytes);
                }
            }
            base.structural.push((label.clone(), ranges.clone()));
        }
        base.strings = doc.new_strings().to_vec();
        for b in doc.live_blocks().into_iter().filter(|b| b.live) {
            if let Some((count, bytes)) = block_now(doc, b.tag, b.pair, b.size) {
                base.blocks.insert(
                    (b.tag.0, b.pair),
                    Held {
                        count,
                        address: None,
                        bytes,
                        size: b.size,
                    },
                );
            }
        }
        base
    }

    /// The game took `moves`: it holds each block where `reply`'s
    /// `moves` (in the same order) says.
    pub fn moved(&mut self, moves: &[BlockMove], reply: &Value) {
        let places = reply["moves"].as_array().map_or(&[][..], Vec::as_slice);
        for (m, place) in moves.iter().zip(places) {
            let address = place["address"]
                .as_u64()
                .and_then(|a| u32::try_from(a).ok());
            self.blocks.insert(
                (m.tag.0, m.pair),
                Held {
                    count: m.count,
                    address,
                    bytes: m.bytes.clone(),
                    size: m.size,
                },
            );
        }
    }

    pub fn wrote(&mut self, at: u32, bytes: &[u8]) {
        self.bytes.extend((at..).zip(bytes.iter().copied()));
    }
}

pub struct Write {
    pub tag: TagId,
    pub path: String,
    pub nav_path: String,
    pub at: u32,
    pub value: Vec<u8>,
    pub was: Vec<u8>,
    pub effect: Effect,
}

/// A pending row: what changed (`doc`, `tag`, `path`), its fan-out class
/// (`class`, by `doc` until [`set_class`] says otherwise), when it takes
/// effect (`effect`, the id, and `text`, its sentence) and why.
pub fn row(doc: &str, tag: Value, path: &str, effect: Effect, why: &str) -> Value {
    json!({
        "doc": doc,
        "tag": tag,
        "path": path,
        "class": Class::of_doc(doc).id(),
        "effect": effect.id(),
        "text": effect.sentence(),
        "why": why,
    })
}

/// Sets `row`'s fan-out class.
pub fn set_class(row: &mut Value, class: Class) {
    row["class"] = json!(class.id());
}

/// Sets `row`'s effect and its sentence.
pub fn set_effect(row: &mut Value, effect: Effect) {
    row["effect"] = json!(effect.id());
    row["text"] = json!(effect.sentence());
}

pub fn effect(doc: &TagDoc, tag: TagId, at: u32, field: &Field) -> (Effect, &'static str) {
    let reload = |why| (Effect::Reload, why);
    let Some(entry) = doc.tag(tag) else {
        return reload("tag is absent");
    };
    if entry.bsp.is_some() || matches!(&entry.group.0, b"sbsp" | b"ltmp" | b"coll" | b"phmo") {
        return reload("geometry or collision is built at map load");
    }
    if doc.is_grown_room(at) {
        return reload("the block grew beyond its loaded address");
    }
    match &field.kind {
        Kind::Block(_) | Kind::Data | Kind::Raw | Kind::RawSize { .. } | Kind::Bsp => {
            return reload("block layout or resource storage changes at map load");
        }
        Kind::TagRef { .. } | Kind::Datum { .. } => {
            let offset = if matches!(field.kind, Kind::TagRef { .. }) {
                4
            } else {
                0
            };
            let datum = doc
                .read(tag, at + offset, 4)
                .and_then(|v| Some(u32::from_le_bytes(v.try_into().ok()?)));
            if datum.is_none_or(|d| d != u32::MAX && !doc.resident(TagId(d))) {
                return reload("reference target is absent from the loaded metadata");
            }
        }
        Kind::StringId => {
            let id = doc
                .read(tag, at, 4)
                .and_then(|v| Some(u32::from_le_bytes(v.try_into().ok()?)));
            if id.is_none_or(|id| id != u32::MAX && !doc.source_string(id)) {
                return reload("string ID is absent from the loaded string table");
            }
        }
        _ => {}
    }
    let effect = Effect::of_applies(field.applies);
    let why = match effect {
        Effect::Now => "read on the next frame",
        Effect::NextUse => "read on the next use",
        Effect::NextSpawn => "read when the object is made",
        Effect::Reload | Effect::NotKnown => "when the game reads this field is not known",
    };
    (effect, why)
}

/// The fields a change wrote and when the game reads each: the field's
/// name and its [`Effect`], once per field (the subject's follow-ups,
/// docs/editor/live-editors.md). A range that no field covers is
/// [`Effect::NotKnown`], named by its address.
pub(crate) fn change_effects(doc: &TagDoc, change: &Change) -> Vec<(Effect, String)> {
    let mut out: Vec<(Effect, String)> = Vec::new();
    let Some(tag) = change.tag else {
        return out;
    };
    let (Some(entry), Some(layout)) = (doc.tag(tag), doc.layout(tag)) else {
        return out;
    };
    for &(at, _) in &change.ranges {
        let item = match field_at(doc, tag, entry.address, layout, at, 12) {
            Some((start, field, _)) => (effect(doc, tag, start, field).0, field.name.to_string()),
            None => (Effect::NotKnown, format!("{at:#x}")),
        };
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

/// Find a field in a tag's root or a reachable block. Resource payloads and
/// addresses outside the owning instance have no writable field here.
fn field_at<'a>(
    doc: &TagDoc,
    tag: TagId,
    root: u32,
    layout: &'a Layout,
    at: u32,
    depth: u8,
) -> Option<(u32, &'a Field, String)> {
    if let Some(f) = at.checked_sub(root).and_then(|o| layout.field_covering(o)) {
        return Some((root.checked_add(f.offset)?, f, format!("+{:#x}", f.offset)));
    }
    if depth == 0 {
        return None;
    }
    for field in &layout.fields {
        let Kind::Block(child) = &field.kind else {
            continue;
        };
        let Some((count, first)) = root
            .checked_add(field.offset)
            .and_then(|p| doc.block(tag, p, child))
        else {
            continue;
        };
        if child.size == 0 {
            continue;
        }
        if let Some(index) = at
            .checked_sub(first)
            .map(|o| o / child.size)
            .filter(|i| *i < count)
        {
            let start = first + index * child.size;
            if let Some(f) = child.field_covering(at - start) {
                return Some((
                    start + f.offset,
                    f,
                    format!("+{:#x}[{index}].+{:#x}", field.offset, f.offset),
                ));
            }
        }
        if child
            .fields
            .iter()
            .any(|f| matches!(f.kind, Kind::Block(_)))
        {
            for i in 0..count {
                if let Some((start, nested, path)) =
                    field_at(doc, tag, first + i * child.size, child, at, depth - 1)
                {
                    return Some((start, nested, format!("+{:#x}[{i}].{path}", field.offset)));
                }
            }
        }
    }
    None
}

pub fn diff(base: &TagBaseline, doc: &TagDoc) -> (Vec<Write>, Vec<Value>) {
    let mut addresses: BTreeSet<u32> = base.bytes.keys().copied().collect();
    for (at, bytes) in doc.patches() {
        addresses.extend((at..).take(bytes.len()));
    }
    let mut rows = Vec::new();
    let bsp_patches = doc.bsp_patches();
    let spaces: BTreeSet<_> = bsp_patches
        .iter()
        .chain(&base.bsp_patches)
        .map(|p| p.space)
        .collect();
    for space in spaces {
        let now = bsp_patches.iter().filter(|p| p.space == space);
        let was = base.bsp_patches.iter().filter(|p| p.space == space);
        if !now.eq(was) {
            rows.push(row(
                "tags",
                Value::Null,
                &format!("BSP region at file offset {:#x}", space.file_offset),
                Effect::Reload,
                "geometry or collision is built at map load",
            ));
        }
    }
    let mut excluded = Vec::new();
    // A block the game is given as a move, with its elements, is sent as
    // one ([`block_moves`]), not as writes at the addresses the document
    // keeps for it.
    for (&(_, pair), &(tag, size)) in &live_candidates(base, doc) {
        excluded.push((pair, 8));
        excluded.extend(block_regions(doc, tag, pair, size));
    }
    let mut structures = BTreeSet::new();
    for (label, ranges) in doc.structural_edits().chain(&base.structural) {
        if !structures.insert((label, ranges)) {
            continue;
        }
        excluded.extend_from_slice(ranges);
        if ranges.iter().any(|&(at, n)| {
            let Some(now) = doc.read_metadata(at, n) else {
                return true;
            };
            let Some(src) = doc.read_metadata_source(at, n) else {
                return true;
            };
            now.iter()
                .zip(src)
                .enumerate()
                .any(|(i, (b, original))| b != base.bytes.get(&(at + i as u32)).unwrap_or(original))
        }) {
            let mut layout = row(
                "tags",
                Value::Null,
                label,
                Effect::Reload,
                "the block layout changed",
            );
            set_class(&mut layout, Class::Shape);
            rows.push(layout);
        }
    }
    for string in doc
        .new_strings()
        .iter()
        .filter(|s| !base.strings.contains(s))
    {
        let mut added = row("tags", Value::Null, string, Effect::Reload, "new map string");
        set_class(&mut added, Class::Shape);
        rows.push(added);
    }
    for string in base
        .strings
        .iter()
        .filter(|s| !doc.new_strings().contains(s))
    {
        let mut removed = row(
            "tags",
            Value::Null,
            string,
            Effect::Reload,
            "remove string from the play copy",
        );
        set_class(&mut removed, Class::Shape);
        rows.push(removed);
    }
    let mut writes = Vec::new();
    let mut covered = 0u64;
    for at in addresses {
        if u64::from(at) < covered
            || excluded
                .iter()
                .any(|&(a, n)| at.checked_sub(a).is_some_and(|i| i < n))
        {
            continue;
        }
        let Some(entry) = doc
            .tags()
            .iter()
            .find(|t| t.bsp.is_none() && at.checked_sub(t.address).is_some_and(|o| o < t.size))
        else {
            rows.push(row(
                "tags",
                Value::Null,
                &format!("{at:#x}"),
                Effect::Reload,
                "outside a loaded tag",
            ));
            continue;
        };
        let id = json!(format!("{:#010x}", entry.id.0));
        let Some((start, f, nav_path)) = doc
            .layout(entry.id)
            .and_then(|l| field_at(doc, entry.id, entry.address, l, at, 12))
        else {
            rows.push(row(
                "tags",
                id,
                &format!("{at:#x}"),
                Effect::Reload,
                "resource or unlocated field",
            ));
            continue;
        };
        let n = f.kind.width();
        covered = u64::from(start) + u64::from(n);
        let Some(mut was) = doc.read_source(entry.id, start, n).map(<[u8]>::to_vec) else {
            continue;
        };
        for (a, b) in (start..).zip(&mut was) {
            if let Some(v) = base.bytes.get(&a) {
                *b = *v;
            }
        }
        let Some(value) = doc.read(entry.id, start, n) else {
            continue;
        };
        if value == was {
            continue;
        }
        let (when, why) = if covered > u64::from(entry.address) + u64::from(entry.size) {
            (Effect::Reload, "the field extends outside its loaded tag")
        } else {
            effect(doc, entry.id, start, f)
        };
        let path = format!("{} · {} ({start:#x})", entry.name, f.name);
        let mut pending = row("tags", id, &path, when, why);
        // What the Pending tab shows: the tag and the field, no address.
        pending["label"] = json!(format!("{} · {}", entry.name, f.name));
        pending["at"] = json!(start);
        if crate::app::tag_api::field_focus(doc, entry.id, &nav_path).is_ok() {
            pending["target"] = json!({"tag": entry.id.0, "path": nav_path});
        }
        rows.push(pending);
        if when != Effect::Reload {
            writes.push(Write {
                tag: entry.id,
                path,
                nav_path,
                at: start,
                value,
                was,
                effect: when,
            });
        }
    }
    (writes, rows)
}

/// The most bytes of elements a move carries; a larger block waits for a
/// reload (the request is one line, base64 doubling the bytes).
const MOVE_MAX: usize = 64 * 1024;

/// The blocks the game is, or is to be, given as moves: those the undo
/// stack's operations changed and those the game already holds moved,
/// with the tag and the element size. A block an operation shifted
/// elements of stays under reload and is not here.
fn live_candidates(base: &TagBaseline, doc: &TagDoc) -> BTreeMap<(u32, u32), (TagId, u32)> {
    let mut found: BTreeMap<(u32, u32), (TagId, u32, bool)> = BTreeMap::new();
    for b in doc.live_blocks() {
        found.insert((b.tag.0, b.pair), (b.tag, b.size, b.live));
    }
    for (&(tag, pair), held) in &base.blocks {
        found
            .entry((tag, pair))
            .or_insert((TagId(tag), held.size, true));
    }
    found
        .into_iter()
        .filter(|(_, (_, _, live))| *live)
        .map(|(key, (tag, size, _))| (key, (tag, size)))
        .collect()
}

/// Where a block's elements are in the document: the source's room and
/// the current place, when it is in the loaded metadata.
fn block_regions(doc: &TagDoc, tag: TagId, pair: u32, size: u32) -> Vec<(u32, u32)> {
    let mut regions = Vec::new();
    if let Some(src) = doc.read_source(tag, pair, 8) {
        let count = u32::from_le_bytes([src[0], src[1], src[2], src[3]]);
        let at = u32::from_le_bytes([src[4], src[5], src[6], src[7]]);
        regions.push((at, count.saturating_mul(size)));
    }
    if let Some((count, at)) = doc.pair(tag, pair)
        && !doc.is_grown_room(at)
    {
        regions.push((at, count.saturating_mul(size)));
    }
    regions
}

/// The block's count and elements as the document holds them.
fn block_now(doc: &TagDoc, tag: TagId, pair: u32, size: u32) -> Option<(u32, Vec<u8>)> {
    let (count, at) = doc.pair(tag, pair)?;
    let len = count.checked_mul(size)?;
    let bytes = if len == 0 {
        Vec::new()
    } else {
        doc.read(tag, at, len)?
    };
    Some((count, bytes))
}

/// The block as the game holds it before any move: the source's.
fn block_source(doc: &TagDoc, tag: TagId, pair: u32, size: u32) -> Option<Held> {
    let src = doc.read_source(tag, pair, 8)?;
    let count = u32::from_le_bytes([src[0], src[1], src[2], src[3]]);
    let address = u32::from_le_bytes([src[4], src[5], src[6], src[7]]);
    let len = count.checked_mul(size)?;
    let bytes = if len == 0 {
        Vec::new()
    } else {
        doc.read_source(tag, address, len)?.to_vec()
    };
    Some(Held {
        count,
        address: Some(address),
        bytes,
        size,
    })
}

/// The path `moves` names the block whose `(count, address)` pair lies at
/// `pair` by, as raw segments from the tag's root element (they need no
/// definition and follow the game's own pointers), with the block field's
/// `applies` and name.
fn pair_path(
    doc: &TagDoc,
    tag: TagId,
    root: u32,
    layout: &Layout,
    pair: u32,
    prefix: &str,
    depth: u8,
) -> Option<(String, Option<&'static str>, String)> {
    let offset = pair.checked_sub(root)?;
    if let Some(f) = layout
        .fields
        .iter()
        .find(|f| f.offset == offset && matches!(f.kind, Kind::Block(_)))
    {
        return Some((
            format!("{prefix}+{offset:#x}:bytes[8]"),
            f.applies,
            f.name.clone(),
        ));
    }
    if depth == 0 {
        return None;
    }
    for field in &layout.fields {
        let Kind::Block(child) = &field.kind else {
            continue;
        };
        let Some((count, first)) = root
            .checked_add(field.offset)
            .and_then(|p| doc.block(tag, p, child))
        else {
            continue;
        };
        if child.size == 0 {
            continue;
        }
        let Some(index) = pair
            .checked_sub(first)
            .map(|o| o / child.size)
            .filter(|i| *i < count)
        else {
            continue;
        };
        let seg = format!("{prefix}+{:#x}/{:#x}[{index}].", field.offset, child.size);
        if let Some(found) = pair_path(
            doc,
            tag,
            first + index * child.size,
            child,
            pair,
            &seg,
            depth - 1,
        ) {
            return Some(found);
        }
    }
    None
}

/// One block the game is to be given again, as a `moves` entry of
/// `edit.batch`: the whole block as the document holds it, put in the
/// edit arena, its pair repointed.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockMove {
    pub tag: TagId,
    pub pair: u32,
    /// Bytes per element.
    pub size: u32,
    /// The block's path from the tag's root (raw segments).
    pub path: String,
    /// The Pending row's path.
    pub row_path: String,
    pub count: u32,
    pub bytes: Vec<u8>,
    /// The `(count, address)` the game holds now.
    pub was: (u32, u32),
}

impl BlockMove {
    pub fn wire(&self) -> Value {
        json!({
            "root": {"tag": format!("{:#010x}", self.tag.0), "path": self.path},
            "stride": self.size,
            "count": self.count,
            "capacity": self.count.max(1),
            "bytes": encode_base64(&self.bytes),
            "was": {"count": self.was.0, "address": self.was.1},
        })
    }
}

/// The block operations of tags the game has loaded as moves
/// (docs/editor/realtime-edits.md, "The editor"), and the
/// Pending rows for them: a move row for each block the game is to be
/// given, a Shape row under After reload, with the reason, for a block it
/// cannot be given (one nested in another changed block, too large, a play
/// copy's block whose place the document does not know, or one the
/// definition cannot name). A block the document holds as the game does
/// has neither.
pub fn block_moves(base: &TagBaseline, doc: &TagDoc) -> (Vec<BlockMove>, Vec<Value>) {
    let found = live_candidates(base, doc);
    let elements: Vec<(u32, Vec<(u32, u32)>)> = found
        .iter()
        .map(|(&(_, pair), &(tag, size))| (pair, block_regions(doc, tag, pair, size)))
        .collect();
    let mut moves = Vec::new();
    let mut rows = Vec::new();
    for (&(_, pair), &(tag, size)) in &found {
        let Some((count, bytes)) = block_now(doc, tag, pair, size) else {
            continue;
        };
        let Some(held) = base
            .blocks
            .get(&(tag.0, pair))
            .cloned()
            .or_else(|| block_source(doc, tag, pair, size))
        else {
            continue;
        };
        if held.count == count && held.bytes == bytes {
            continue;
        }
        let entry = doc.tag(tag);
        let name = entry.map_or_else(String::new, |t| t.name.clone());
        let located =
            entry.and_then(|t| pair_path(doc, tag, t.address, doc.layout(tag)?, pair, "", 12));
        let field = located
            .as_ref()
            .map_or_else(|| "block".to_owned(), |(_, _, f)| f.clone());
        let row_path = format!("{name} · {field} ({pair:#x})");
        let tag_json = json!(format!("{:#010x}", tag.0));
        let nested = elements.iter().any(|(other, regions)| {
            *other != pair
                && regions
                    .iter()
                    .any(|&(at, len)| pair.checked_sub(at).is_some_and(|o| o < len))
        });
        let why = if nested {
            Some("the block lies inside another block that changed")
        } else if bytes.len() > MOVE_MAX {
            Some("the block is too large to send to the game")
        } else if held.address.is_none() {
            Some("the play copy holds this block where the document does not know")
        } else if located.is_none() {
            Some("the block's place in the tag is not known")
        } else {
            None
        };
        let (Some((path, applies, _)), Some(address), None) = (located, held.address, why) else {
            let mut shape = row(
                "tags",
                tag_json,
                &row_path,
                Effect::Reload,
                why.unwrap_or("the block's place in the tag is not known"),
            );
            set_class(&mut shape, Class::Shape);
            rows.push(shape);
            continue;
        };
        let effect = Effect::of_applies(applies);
        let mut pending = row(
            "tags",
            tag_json,
            &row_path,
            effect,
            "the block's elements go to the game's edit arena",
        );
        pending["label"] = json!(format!("{name} · {field}"));
        pending["at"] = json!(pair);
        rows.push(pending);
        moves.push(BlockMove {
            tag,
            pair,
            size,
            path,
            row_path,
            count,
            bytes,
            was: (held.count, address),
        });
    }
    (moves, rows)
}

/// Block moves the game refused, held until the block changes so the
/// same move is not sent again.
#[derive(Default)]
pub(super) struct MoveRefusals {
    held: Vec<(BlockMove, String)>,
}

impl MoveRefusals {
    pub(super) fn record(&mut self, moves: &[BlockMove], why: &str) {
        self.held
            .extend(moves.iter().map(|m| (m.clone(), why.to_owned())));
    }

    /// Marks the rows of refused moves, and takes the moves out of
    /// `moves`; a record whose block has changed is dropped.
    pub(super) fn hold(&mut self, moves: &mut Vec<BlockMove>, rows: &mut [Value]) {
        self.held.retain(|(h, _)| moves.contains(h));
        if self.held.is_empty() {
            return;
        }
        for row in rows {
            let path = row["path"].as_str();
            if let Some((_, why)) = self
                .held
                .iter()
                .find(|(h, _)| path == Some(h.row_path.as_str()))
            {
                row["refused"] = json!(true);
                row["why"] = json!(why);
            }
        }
        moves.retain(|m| !self.held.iter().any(|(h, _)| h == m));
    }
}

/// Re-resolve the submitted tag field, including every indexed block along its path.
pub(crate) fn check_conflict_path(doc: &TagDoc, row: &Value) -> Result<(), &'static str> {
    if row["doc"] == "tags" {
        let current = row["target"]["path"].as_str().and_then(|path| {
            let tag = crate::app::tag_api::lookup(doc, &row["tag"]).ok()?;
            crate::app::tag_api::field_focus(doc, tag, path).ok()
        });
        if !current.is_some_and(|field| row["at"].as_u64() == Some(u64::from(field.address))) {
            return Err("the conflicted tag field moved. Undo the layout change or reload the map");
        }
    }
    Ok(())
}

impl Write {
    pub fn wire(&self) -> Value {
        json!({"at":self.at,"value":sync::hex(&self.value),"was":sync::hex(&self.was)})
    }
}

/// The scenario session's revisions, as the fan-out compares them: the
/// open document, its edits (and selection), and each block's layout.
fn scenario_seen(app: &App) -> (u64, u64, u64) {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    if let Some(s) = app.session.scenario() {
        for b in &s.blocks {
            b.layout_rev().hash(&mut h);
        }
    }
    (app.session.world_rev(), app.session.markers_rev(), h.finish())
}

impl super::Play {
    /// Where the game view's changes go: Studio's own game, an attached
    /// one (in Edit or not), or none.
    pub(crate) fn live_target(&self) -> Target {
        if !self.is_paired() {
            Target::Off
        } else if self.own_game() {
            Target::Own
        } else {
            Target::Attached {
                edit: self.live_edits,
            }
        }
    }

    /// Once a frame, after the edits are pushed: takes every write path's
    /// changes (the tag document's own; the scenario session's and the raw
    /// documents' by their revisions), routes them by class
    /// (`app::live::route`) and carries out the game's deliveries. Field
    /// and Raw deliveries are one send of what the documents hold
    /// (`edit.batch`, the raw edits); a Shape delivery is that send and,
    /// for what it leaves under After reload, one play-copy reload; a
    /// Host delivery is the source's reload method. A delivery the game
    /// cannot take stays a Pending row.
    pub(crate) fn fan_out(&mut self, app: &mut App) {
        let target = self.live_target();
        app.live.set_target(target);
        for change in app.tags.take_changes() {
            app.live.emit(change);
        }
        see_documents(app);
        if target == Target::Off {
            app.live.owed = Default::default();
        }
        let mut written = Vec::new();
        for d in app.live.flush() {
            if target == Target::Own && d.view == View::Game && d.route == Route::EditBatch {
                written.push(d.change.clone());
            }
            match d.route {
                Route::Reread | Route::Pending => {}
                // An attached game in Edit takes these by its own push.
                Route::EditBatch | Route::RawEdit if target == Target::Own => {
                    app.live.owed.send = true;
                }
                Route::EditBatch | Route::RawEdit => {}
                Route::BuildLoad => {
                    app.live.owed.send = true;
                    app.live.owed.build = true;
                }
                Route::HostReload(host) => match host.method() {
                    Some(method) if self.has_method(method) => {
                        let purpose = if host == Host::Rules {
                            Purpose::Rules
                        } else {
                            Purpose::Other
                        };
                        self.call(method, d.change.params.clone(), purpose);
                    }
                    _ => app.live.hold_refused(d.change),
                },
            }
        }
        if app.live.owed.send && !self.edits.interrupted_write() {
            app.live.owed.send = false;
            self.refresh_pending(app, true);
        }
        if app.live.owed.build && !app.live.owed.send && !self.edits.interrupted_write() {
            app.live.owed.build = false;
            let after_reload = app.pairing.pending.iter().any(|r| {
                Class::of_row(r) == Class::Shape && Effect::is_reload(r)
            });
            if after_reload
                && target == Target::Own
                && self.waiting == Waiting::Idle
                && self.reload_copy.is_none()
                && let Err(why) = self.reload_for_play(app, playtest::Reload::Method)
            {
                app.error(format!("the layout change stays in Pending: {why}"));
            }
        }
        publish_held(app);
        // What the game does not show of its own after the writes it took.
        self.subject_follow(app, &written);
        self.subject_update(app);
    }
}

/// Reports the scenario session's and the raw documents' writes as
/// changes, by their revisions: a block's layout is Shape, other scenario
/// edits Field, geometry and pixels Raw. A new document is not a write.
fn see_documents(app: &mut App) {
    let raw = raw_sync::doc_rev(app);
    if app.live.seen.raw.replace(raw).is_some_and(|r| r != raw) {
        let change = Change::new(Class::Raw, None, Vec::new(), "geometry and pixels");
        app.live.emit(change);
    }
    let now = scenario_seen(app);
    let before = app.live.seen.scenario.replace(now);
    if let Some((world, markers, layouts)) = before
        && world == now.0
    {
        let tag = app.session.scenario().map(|s| s.tag);
        if layouts != now.2 {
            app.live
                .emit(Change::new(Class::Shape, tag, Vec::new(), "placement layout"));
        } else if markers != now.1 {
            app.live
                .emit(Change::new(Class::Field, tag, Vec::new(), "placements"));
        }
    }
}

/// Puts the held Host changes in Pending, as rows of class `host`, and
/// takes out the rows of those that went.
fn publish_held(app: &mut App) {
    let rows = app.live.held_rows(|c| match c.host.and_then(Host::method) {
        None => Host::NO_RELOAD,
        Some(_) => "reloaded in the game once the pairing is in Edit",
    });
    let shown: Vec<&Value> = app
        .pairing
        .pending
        .iter()
        .filter(|r| r["doc"] == "host")
        .collect();
    if shown.len() == rows.len() && shown.iter().zip(&rows).all(|(a, b)| *a == b) {
        return;
    }
    app.pairing.pending.retain(|r| r["doc"] != "host");
    app.pairing.pending.extend(rows);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tag_tests;

    /// Each `applies` word and each reload rule gives its effect, with its
    /// id, chip and sentence; no word names a transport.
    #[test]
    fn each_applies_word_and_reload_rule_gives_its_effect() {
        for (word, id, chip, sentence) in [
            (Some("frame"), "now", "Now", "Applies now."),
            (
                Some("use"),
                "next_use",
                "Next use",
                "Applies the next time the game uses it.",
            ),
            (
                Some("creation"),
                "next_spawn",
                "Next spawn",
                "Applies when the object next spawns.",
            ),
            (
                None,
                "not_known",
                "Not known",
                "Not known; reload to be sure.",
            ),
        ] {
            let e = Effect::of_applies(word);
            assert_eq!((e.id(), e.chip(), e.sentence()), (id, chip, sentence));
            assert_eq!(Effect::from_id(id), Some(e));
        }
        let reload = Effect::Reload;
        assert_eq!(
            (reload.id(), reload.chip(), reload.sentence()),
            ("reload", "After reload", "Applies after reload.")
        );

        let t = crate::play::testing::Temp::new("tag-sync-effect");
        let (app, _, _) = tag_tests::app(&t.0);
        let weap = tag_tests::weap_id();
        let layout = app.tags.layout(weap).unwrap();
        let speed = layout.field_covering(0).unwrap();
        assert_eq!(
            effect(&app.tags, weap, tag_tests::WEAP, speed),
            (
                Effect::NotKnown,
                "when the game reads this field is not known"
            )
        );
        let parts = layout.field_covering(8).unwrap();
        assert_eq!(
            effect(&app.tags, weap, tag_tests::WEAP + 8, parts),
            (
                Effect::Reload,
                "block layout or resource storage changes at map load"
            )
        );
        assert_eq!(
            effect(&app.tags, TagId(0x1234_5678), 0, speed),
            (Effect::Reload, "tag is absent")
        );
        let pending = row("tags", Value::Null, "x", Effect::NextUse, "why");
        assert_eq!(pending["effect"], "next_use");
        assert_eq!(pending["text"], "Applies the next time the game uses it.");
        assert_eq!(pending["class"], "field");
        assert_eq!(Effect::of_row(&pending), Some(Effect::NextUse));
    }

    /// The weapon's parts block, operated on directly.
    fn part_op(app: &mut crate::app::App, op: crate::app::BlockOp) -> Result<(), String> {
        let layout = app.tags.layout(tag_tests::weap_id()).unwrap().clone();
        let Kind::Block(child) = &layout.field_at(8).unwrap().kind else {
            panic!("the weapon's parts are a block")
        };
        app.tags
            .block_op(tag_tests::weap_id(), tag_tests::WEAP + 8, child, &op)
    }

    /// An append or a trailing removal of a live tag's block is a move of
    /// the whole block, named by raw segments, `was` the source's pair; a
    /// removal that shifts elements, which other blocks may index, is a
    /// Shape row under reload; once the game holds the move nothing is
    /// left, and undo is a move of the source's bytes back.
    #[test]
    fn block_operations_are_moves_when_no_index_shifts() {
        use crate::app::BlockOp;
        let t = crate::play::testing::Temp::new("tag-sync-moves");
        let (mut app, _, _) = tag_tests::app(&t.0);
        app.tags.allow_growth = true;
        let base = TagBaseline::default();
        assert!(block_moves(&base, &app.tags).0.is_empty());

        part_op(&mut app, BlockOp::Add).unwrap();
        let (moves, rows) = block_moves(&base, &app.tags);
        assert_eq!(moves.len(), 1, "{rows:?}");
        let m = &moves[0];
        assert_eq!(
            (m.path.as_str(), m.count, m.size),
            ("+0x8:bytes[8]", 3, 0xc)
        );
        assert_eq!(m.bytes.len(), 3 * 0xc);
        assert_eq!(m.was, (2, tag_tests::PARTS));
        assert_eq!(rows.len(), 1);
        assert_ne!(rows[0]["effect"], "reload");
        // The elements are the document's, the new one after them.
        assert_eq!(
            m.bytes[..0x18],
            app.tags
                .read(tag_tests::weap_id(), tag_tests::PARTS, 0x18)
                .unwrap()[..]
        );
        // The byte diff leaves the block to the move.
        assert!(diff(&base, &app.tags).1.is_empty());

        let mut held = base.clone();
        held.moved(&moves, &json!({"moves": [{"address": 0xe200_0000u32}]}));
        assert!(block_moves(&held, &app.tags).0.is_empty());

        app.dispatch(crate::app::Action::Undo);
        let (back, _) = block_moves(&held, &app.tags);
        assert_eq!(back.len(), 1);
        assert_eq!((back[0].count, back[0].was), (2, (3, 0xe200_0000)));
        assert_eq!(back[0].bytes.len(), 2 * 0xc);
        // The source's, and nothing to say against the source's own base.
        assert!(block_moves(&base, &app.tags).0.is_empty());
    }

    /// A removal that moves later elements down is not a move.
    #[test]
    fn a_removal_that_shifts_elements_stays_under_reload() {
        use crate::app::BlockOp;
        let t = crate::play::testing::Temp::new("tag-sync-shift");
        let (mut app, _, _) = tag_tests::app(&t.0);
        part_op(&mut app, BlockOp::Remove(0)).unwrap();
        let base = TagBaseline::default();
        assert!(block_moves(&base, &app.tags).0.is_empty());
        let rows = diff(&base, &app.tags).1;
        assert!(
            rows.iter()
                .any(|r| r["class"] == "shape" && r["effect"] == "reload")
        );
    }

    /// A refused move is held until its block changes.
    #[test]
    fn a_refused_move_is_held_until_the_block_changes() {
        use crate::app::BlockOp;
        let t = crate::play::testing::Temp::new("tag-sync-refused");
        let (mut app, _, _) = tag_tests::app(&t.0);
        app.tags.allow_growth = true;
        part_op(&mut app, BlockOp::Add).unwrap();
        let base = TagBaseline::default();
        let (moves, mut rows) = block_moves(&base, &app.tags);
        let mut refused = MoveRefusals::default();
        refused.record(&moves, "no room");
        let mut again = moves.clone();
        refused.hold(&mut again, &mut rows);
        assert!(again.is_empty());
        assert_eq!(
            (rows[0]["refused"].clone(), rows[0]["why"].clone()),
            (json!(true), json!("no room"))
        );
        part_op(&mut app, BlockOp::Add).unwrap();
        let (mut changed, mut rows) = block_moves(&base, &app.tags);
        refused.hold(&mut changed, &mut rows);
        assert_eq!(changed.len(), 1);
        assert!(rows[0].get("refused").is_none());
    }
}

/// The fan-out on the invented game (docs/editor/live-editors.md, LE1).
#[cfg(all(test, unix))]
mod fan_out_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use agent::Access;
    use agent_proto::Endpoint;
    use agent_proto::sim::SessionKind;
    use agent_proto::sim::method::MAP_RELOAD;
    use serde_json::{Value, json};

    use crate::app::live::{Change, Class, Host, Route, View};
    use crate::app::{Action, App, BlockOp, TagAction, tag_tests};
    use crate::play::Play;
    use crate::play::testing::Temp;
    use crate::play::testing::fake::{Game, until};

    /// The invented game with `render.reload` counted, and the editor
    /// paired with it over the tag fixture; `own` makes it Studio's own.
    fn paired(name: &str, own: bool) -> (Temp, Game, App, Play, Arc<AtomicU32>) {
        let t = Temp::new(name);
        let game = Game::start(name, "example", SessionKind::Local);
        let reloads = Arc::new(AtomicU32::new(0));
        let counted = Arc::clone(&reloads);
        game.agent.add_method(
            agent_proto::hot::method::RENDER_RELOAD,
            Access::Act,
            move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(json!({}))
            },
        );
        let (mut app, _, _) = tag_tests::app(&t.0);
        app.pairing.auto_pair = false;
        let mut play = Play::new();
        play.pair(&mut app, Endpoint::Unix(game.path.clone()));
        until(&mut play, &mut app, "game identity and journal", |p, a| {
            p.edits.journal_ready && p.edit_refusal(a).is_none()
        });
        if own {
            play.child_endpoint = play.endpoint().cloned();
        }
        (t, game, app, play, reloads)
    }

    fn speed(app: &mut App) {
        let action =
            crate::app::tag_api::set_field(&app.tags, tag_tests::weap_id(), "speed", &json!(4.5))
                .unwrap();
        app.dispatch(Action::Tag(action));
    }

    fn remove_part(app: &mut App) {
        part_op(app, BlockOp::Remove(0));
    }

    fn part_op(app: &mut App, op: BlockOp) {
        let field = app
            .tags
            .layout(tag_tests::weap_id())
            .unwrap()
            .field_at(8)
            .unwrap()
            .clone();
        let tags::registry::Kind::Block(layout) = &field.kind else {
            panic!("the weapon's parts are a block")
        };
        app.dispatch(Action::Tag(TagAction::Block {
            tag: tag_tests::weap_id(),
            pair: tag_tests::WEAP + 8,
            layout: layout.clone(),
            op,
        }));
    }

    fn frames(play: &mut Play, app: &mut App, n: usize) {
        for _ in 0..n {
            play.update(app);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn delivered(app: &App, view: View, class: Class) -> Vec<Route> {
        app.live
            .delivered()
            .iter()
            .filter(|(v, _, c)| *v == view && *c == class)
            .map(|(_, r, _)| *r)
            .collect()
    }

    /// A field write to the own game is one `edit.batch`, sent with no
    /// click, and one Preview re-read.
    #[test]
    fn a_field_write_is_one_batch_in_the_own_game() {
        let (_t, game, mut app, mut play, _) = paired("fan-field-own", true);
        speed(&mut app);
        until(&mut play, &mut app, "the field's batch", |p, _| {
            game.calls("edit.batch").len() == 1 && p.edits.sent.is_none()
        });
        frames(&mut play, &mut app, 20);
        assert_eq!(game.calls("edit.batch").len(), 1);
        assert_eq!(game.calls("edit.batch")[0]["writes"].as_array().unwrap().len(), 1);
        assert_eq!(delivered(&app, View::Game, Class::Field), vec![Route::EditBatch]);
        assert_eq!(delivered(&app, View::Preview, Class::Field), vec![Route::Reread]);
    }

    /// The same write to an attached game in View is a Pending row of
    /// class `field`, and nothing is sent.
    #[test]
    fn a_field_write_is_a_pending_row_in_an_attached_game() {
        let (_t, game, mut app, mut play, _) = paired("fan-field-attached", false);
        speed(&mut app);
        until(&mut play, &mut app, "the pending field", |_, a| {
            a.pairing.pending.iter().any(|r| r["doc"] == "tags")
        });
        frames(&mut play, &mut app, 20);
        assert!(game.calls("edit.batch").is_empty());
        let row = app.pairing.pending.iter().find(|r| r["doc"] == "tags").unwrap();
        assert_eq!(row["class"], "field");
        assert_eq!(delivered(&app, View::Attached, Class::Field), vec![Route::Pending]);
        assert!(delivered(&app, View::Game, Class::Field).is_empty());
    }

    /// A Host change is one reload call in the own game; in an attached
    /// game in View it is one Pending row of class `host` and no call,
    /// and it goes once the pairing is in Edit.
    #[test]
    fn a_host_change_is_one_reload_or_one_pending_row() {
        let (_t, _game, mut app, mut play, reloads) = paired("fan-host-own", true);
        app.live
            .emit(Change::host(Host::Shader, json!({}), "shader override"));
        until(&mut play, &mut app, "the reload", |_, _| {
            reloads.load(Ordering::SeqCst) == 1
        });
        frames(&mut play, &mut app, 20);
        assert_eq!(reloads.load(Ordering::SeqCst), 1);
        assert!(app.pairing.pending.iter().all(|r| r["doc"] != "host"));

        let (_t, _game, mut app, mut play, reloads) = paired("fan-host-attached", false);
        app.live
            .emit(Change::host(Host::Shader, json!({}), "shader override"));
        frames(&mut play, &mut app, 20);
        assert_eq!(reloads.load(Ordering::SeqCst), 0);
        let rows: Vec<&Value> = app
            .pairing
            .pending
            .iter()
            .filter(|r| r["doc"] == "host")
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0]["class"].clone(), rows[0]["host"].clone()),
            (json!("host"), json!("shader"))
        );
        play.live_edits = true;
        until(&mut play, &mut app, "the held reload", |_, _| {
            reloads.load(Ordering::SeqCst) == 1
        });
        frames(&mut play, &mut app, 20);
        assert_eq!(reloads.load(Ordering::SeqCst), 1);
        assert!(app.pairing.pending.iter().all(|r| r["doc"] != "host"));
    }

    /// A Host source the game has no reload for waits in Pending in the
    /// own game too, saying so.
    #[test]
    fn a_host_source_without_a_reload_waits_in_pending() {
        let (_t, _game, mut app, mut play, reloads) = paired("fan-host-none", true);
        app.live.emit(Change::host(Host::HudKit, json!({}), "HUD kit"));
        frames(&mut play, &mut app, 20);
        let row = app.pairing.pending.iter().find(|r| r["doc"] == "host").unwrap();
        assert_eq!(row["why"], Host::NO_RELOAD);
        assert_eq!(reloads.load(Ordering::SeqCst), 0);
    }

    /// An element added to a block is one batch with one move, alone, not
    /// a reload; undoing it is a second move of the source's elements, and
    /// the game's last answer is the `was` of the next.
    #[test]
    fn an_added_element_is_one_batch_with_a_move() {
        let (_t, game, mut app, mut play, _) = paired("fan-grow-own", true);
        app.session.set_allow_growth(true);
        app.tags.allow_growth = true;
        part_op(&mut app, BlockOp::Add);
        until(&mut play, &mut app, "the move's batch", |p, _| {
            game.calls("edit.batch").len() == 1 && p.edits.sent.is_none()
        });
        frames(&mut play, &mut app, 20);
        let calls = game.calls("edit.batch");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["writes"].as_array().unwrap().len(), 0);
        let moves = calls[0]["moves"].as_array().unwrap();
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0]["root"]["path"], "+0x8:bytes[8]");
        assert_eq!(
            (moves[0]["count"].clone(), moves[0]["stride"].clone()),
            (json!(3), json!(0xc))
        );
        assert_eq!(
            moves[0]["was"],
            json!({"count": 2, "address": tag_tests::PARTS})
        );
        let bytes = agent_proto::hot::decode_base64(moves[0]["bytes"].as_str().unwrap()).unwrap();
        assert_eq!(bytes.len(), 3 * 0xc);
        assert!(app.pairing.pending.iter().all(|r| r["doc"] != "tags"));
        assert!(game.calls(MAP_RELOAD).is_empty());

        app.dispatch(Action::Undo);
        until(&mut play, &mut app, "the undo's move", |p, _| {
            game.calls("edit.batch").len() == 2 && p.edits.sent.is_none()
        });
        let back = &game.calls("edit.batch")[1]["moves"][0];
        assert_eq!(back["count"], 2);
        assert_eq!(back["was"]["count"], 3);
        assert_ne!(back["was"]["address"], json!(tag_tests::PARTS));
        assert!(app.pairing.pending.iter().all(|r| r["doc"] != "tags"));
    }

    /// A block element removed is a Shape change: one play-copy reload in
    /// the own game, a Pending row of class `shape` in an attached one.
    #[test]
    fn a_shape_change_is_one_reload_or_one_pending_row() {
        let (_t, game, mut app, mut play, _) = paired("fan-shape-attached", false);
        remove_part(&mut app);
        until(&mut play, &mut app, "the layout row", |_, a| {
            a.pairing.pending.iter().any(|r| r["class"] == "shape")
        });
        frames(&mut play, &mut app, 20);
        assert!(game.calls(MAP_RELOAD).is_empty());
        assert_eq!(delivered(&app, View::Attached, Class::Shape), vec![Route::Pending]);

        let (_t, game, mut app, mut play, _) = paired("fan-shape-own", true);
        remove_part(&mut app);
        until(&mut play, &mut app, "the play-copy reload", |_, _| {
            game.calls(MAP_RELOAD).len() == 1
        });
        frames(&mut play, &mut app, 20);
        assert_eq!(game.calls(MAP_RELOAD).len(), 1);
        assert_eq!(delivered(&app, View::Game, Class::Shape), vec![Route::BuildLoad]);
    }
}
