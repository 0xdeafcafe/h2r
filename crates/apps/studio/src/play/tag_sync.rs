//! Compare whole fields against the loaded file plus acknowledged game writes.
//! The sparse baseline keeps addresses after undo removes a document patch.
use super::sync;
use crate::app::TagDoc;
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
        base
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

/// A pending row: what changed (`doc`, `tag`, `path`), when it takes
/// effect (`effect`, the id, and `text`, its sentence) and why.
pub fn row(doc: &str, tag: Value, path: &str, effect: Effect, why: &str) -> Value {
    json!({
        "doc": doc,
        "tag": tag,
        "path": path,
        "effect": effect.id(),
        "text": effect.sentence(),
        "why": why,
    })
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
            rows.push(row(
                "tags",
                Value::Null,
                label,
                Effect::Reload,
                "the block layout changed",
            ));
        }
    }
    for string in doc
        .new_strings()
        .iter()
        .filter(|s| !base.strings.contains(s))
    {
        rows.push(row(
            "tags",
            Value::Null,
            string,
            Effect::Reload,
            "new map string",
        ));
    }
    for string in base
        .strings
        .iter()
        .filter(|s| !doc.new_strings().contains(s))
    {
        rows.push(row(
            "tags",
            Value::Null,
            string,
            Effect::Reload,
            "remove string from the play copy",
        ));
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
        assert!(pending.get("class").is_none());
        assert_eq!(Effect::of_row(&pending), Some(Effect::NextUse));
    }
}
