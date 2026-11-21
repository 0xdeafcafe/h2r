//! Compare whole fields against the loaded file plus acknowledged game writes.
//! The sparse baseline keeps addresses after undo removes a document patch.
use super::sync;
use crate::app::TagDoc;
use map::TagId;
use scenario_edit::BspPatch;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tags::registry::{Field, Kind, Layout};

pub const UNKNOWN: &str = "Poke, unknown";
pub const CREATION: &str = "Poke, next creation";
pub const RELOAD: &str = "Reload";

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
    pub class: &'static str,
}

pub fn row(doc: &str, tag: Value, path: &str, class: &str, why: &str) -> Value {
    json!({"doc":doc,"tag":tag,"path":path,"class":class,"why":why})
}

pub fn class(doc: &TagDoc, tag: TagId, at: u32, field: &Field) -> (&'static str, &'static str) {
    let reload = |why| (RELOAD, why);
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
    match field.applies {
        Some("frame") => ("Poke, each frame", "read on the next frame"),
        Some("use") => ("Poke, next use", "read on the next use"),
        Some("creation") => (CREATION, "read when the object is made"),
        _ => (UNKNOWN, "when the game reads this field is unknown"),
    }
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
                RELOAD,
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
                RELOAD,
                "the block layout changed",
            ));
        }
    }
    for string in doc
        .new_strings()
        .iter()
        .filter(|s| !base.strings.contains(s))
    {
        rows.push(row("tags", Value::Null, string, RELOAD, "new map string"));
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
            RELOAD,
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
                RELOAD,
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
                RELOAD,
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
        let (class, why) = if covered > u64::from(entry.address) + u64::from(entry.size) {
            (RELOAD, "the field extends outside its loaded tag")
        } else {
            class(doc, entry.id, start, f)
        };
        let path = format!("{} · {} ({start:#x})", entry.name, f.name);
        let mut pending = row("tags", id, &path, class, why);
        pending["at"] = json!(start);
        if crate::app::tag_api::field_focus(doc, entry.id, &nav_path).is_ok() {
            pending["target"] = json!({"tag": entry.id.0, "path": nav_path});
        }
        rows.push(pending);
        if class != RELOAD {
            writes.push(Write {
                tag: entry.id,
                path,
                nav_path,
                at: start,
                value,
                was,
                class,
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
