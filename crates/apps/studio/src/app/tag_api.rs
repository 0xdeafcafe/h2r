//! The tag editor as plain functions over a [`TagDoc`], for scripts and
//! the studio's agent API: a tag's fields as JSON with their names, and
//! commands built from a field path and a JSON value. Nothing here draws.
//!
//! A path names fields from the tag's root, `.`-separated; a block field
//! takes an element index: `parts[1].speed`. A field may also be named by
//! its offset in its element, `+0x14` (unnamed fields are labelled so).
//!
//! The commands are [`TagAction`]s: run them with
//! `app.dispatch(Action::Tag(..))`, which applies them as undo steps and
//! hands the patches to the session that saves them.
use serde_json::{Value, json};
use std::sync::Arc;

use map::{FourCC, TagId};
use tags::registry::{Field, Kind, Layout};

use super::tag_doc::{Access, BlockOp, TagAction, TagDoc, TagWrite};

mod chain;
pub use chain::{Chain, DEFAULT_DEPTH, MAX_DEPTH, Node, chain, placement_root};

/// Elements of a block [`read_tag`] lists at most.
pub const ELEMENTS: u32 = 64;
/// Bytes of a data reference [`read_tag`] shows at most.
pub const DATA: u32 = 4096;

fn le(b: &[u8]) -> u64 {
    b.iter().rev().fold(0, |v, x| v << 8 | u64::from(*x))
}

fn sign(v: u64, bytes: usize) -> i64 {
    let shift = 64 - 8 * bytes as u32;
    ((v << shift) as i64) >> shift
}

/// A tag reference's JSON: `"group:name"`, or null.
fn tag_json(doc: &TagDoc, id: u32) -> Value {
    match doc.tag(TagId(id)) {
        Some(t) => json!(format!("{}:{}", tags::defs::fourcc_text(t.group), t.name)),
        None if id == u32::MAX => Value::Null,
        None => json!(format!("{id:#010x}")),
    }
}

/// The tag's fields as JSON: `{id, group, name, access, fields}`, every
/// field by its label (blocks as arrays of elements, at most [`ELEMENTS`];
/// data as hex, at most [`DATA`] bytes).
pub fn read_tag(doc: &TagDoc, tag: TagId) -> Result<Value, String> {
    let t = doc.tag(tag).ok_or("no such tag")?;
    let access = match &t.access {
        Access::Edit => json!("edit"),
        Access::Read(why) => json!({ "read_only": why }),
        Access::None(why) => json!({ "none": why }),
    };
    let fields = match (&t.access, doc.layout(tag)) {
        (Access::None(_), _) | (_, None) => Value::Null,
        (_, Some(l)) => element(doc, t.address, l),
    };
    Ok(json!({
        "id": format!("{:#010x}", tag.0),
        "group": tags::defs::fourcc_text(t.group),
        "name": t.name,
        "access": access,
        "edited": doc.is_changed(tag),
        "fields": fields,
    }))
}

fn element(doc: &TagDoc, at: u32, l: &Layout) -> Value {
    let mut out = serde_json::Map::new();
    for f in &l.fields {
        if matches!(f.kind, Kind::Pad { .. }) {
            continue;
        }
        out.insert(f.name.clone(), value(doc, at, f));
    }
    Value::Object(out)
}

/// One field's value as JSON.
pub fn value(doc: &TagDoc, at: u32, f: &Field) -> Value {
    let a = at + f.offset;
    let Some(b) = doc.read(a, f.kind.width()) else {
        return Value::Null;
    };
    let many = |v: Vec<Value>| {
        if v.len() == 1 {
            v.into_iter().next().unwrap()
        } else {
            Value::Array(v)
        }
    };
    match &f.kind {
        Kind::Int { bytes, signed, .. } => {
            let n = *bytes as usize;
            many(
                b.chunks(n)
                    .map(|c| {
                        if *signed {
                            json!(sign(le(c), n))
                        } else {
                            json!(le(c))
                        }
                    })
                    .collect(),
            )
        }
        Kind::Float { .. } => many(
            b.chunks(4)
                .map(|c| json!(f32::from_bits(le(c) as u32)))
                .collect(),
        ),
        Kind::Enum { values, .. } => {
            let v = le(&b);
            values
                .get(v as usize)
                .map_or_else(|| json!(v), |s| json!(s))
        }
        Kind::Flags { bits, .. } => {
            let v = le(&b);
            Value::Array(
                (0..8 * b.len())
                    .filter(|i| v >> i & 1 == 1)
                    .map(|i| match bits.get(i).filter(|s| s.as_str() != "_") {
                        Some(s) => json!(s),
                        None => json!(format!("bit{i}")),
                    })
                    .collect(),
            )
        }
        Kind::StringId => {
            let v = le(&b) as u32;
            match doc.string(v) {
                Some(s) => json!(s),
                None if v == u32::MAX => Value::Null,
                None => json!(format!("{v:#010x}")),
            }
        }
        Kind::TagRef { .. } => tag_json(doc, le(&b[4..]) as u32),
        Kind::Datum { .. } => tag_json(doc, le(&b) as u32),
        Kind::Block(child) => match doc.block(a, child) {
            Some((n, start)) => Value::Array(
                (0..n.min(ELEMENTS))
                    .map(|i| element(doc, start + i * child.size, child))
                    .collect(),
            ),
            None => Value::Null,
        },
        Kind::Data => {
            let (n, start) = (le(&b[..4]) as u32, le(&b[4..]) as u32);
            match doc.read(start, n.min(DATA)) {
                Some(d) if n > 0 => json!(crate::fields::hex(&d)),
                _ => json!(""),
            }
        }
        Kind::Raw | Kind::RawSize { .. } | Kind::Word | Kind::Unknown { .. } => json!(le(&b)),
        Kind::Bsp => json!(crate::fields::hex(&b)),
        Kind::Pad { .. } => Value::Null,
    }
}

/// One step of a path: a field name (or `+0x<offset>`) and an element
/// index for a block.
fn step(s: &str) -> Result<(&str, Option<u32>), String> {
    match s.strip_suffix(']').and_then(|s| s.split_once('[')) {
        Some((name, i)) => Ok((
            name,
            Some(i.parse().map_err(|_| format!("bad index in {s:?}"))?),
        )),
        None => Ok((s, None)),
    }
}

fn find<'a>(l: &'a Layout, name: &str) -> Option<&'a Field> {
    if let Some(hex) = name.strip_prefix("+0x")
        && let Ok(o) = u32::from_str_radix(hex, 16)
    {
        return l.field_at(o);
    }
    l.fields.iter().find(|f| f.name == name)
}

/// The field at `path`: the address of its element, the element's layout
/// and the field.
pub fn resolve(doc: &TagDoc, tag: TagId, path: &str) -> Result<(u32, Arc<Layout>, Field), String> {
    resolve_path(doc, tag, path, |_, _| {})
}

pub struct FieldFocus {
    pub blocks: Vec<(u32, u32)>,
    pub address: u32,
}

pub fn field_focus(doc: &TagDoc, tag: TagId, path: &str) -> Result<FieldFocus, String> {
    let t = doc.tag(tag).ok_or("no such tag")?;
    if matches!(t.access, Access::None(_)) || !doc.layout(tag).is_some_and(|l| l.size <= t.size) {
        return Err("the tag has no fields here".into());
    }
    let mut blocks = Vec::new();
    let (at, _, f) = resolve_path(doc, tag, path, |pair, i| blocks.push((pair, i)))?;
    Ok(FieldFocus { blocks, address: at + f.offset })
}

fn resolve_path(
    doc: &TagDoc,
    tag: TagId,
    path: &str,
    mut block: impl FnMut(u32, u32),
) -> Result<(u32, Arc<Layout>, Field), String> {
    let t = doc.tag(tag).ok_or("no such tag")?;
    let mut layout = doc.layout(tag).ok_or("no layout for this group")?.clone();
    let mut at = t.address;
    let parts: Vec<&str> = path.split('.').collect();
    for (k, part) in parts.iter().enumerate() {
        let (name, index) = step(part)?;
        let f = find(&layout, name)
            .ok_or_else(|| format!("no field {name:?} in {}", layout.name))?
            .clone();
        let last = k + 1 == parts.len();
        match (index, &f.kind) {
            (None, _) if last => return Ok((at, layout, f)),
            (Some(i), Kind::Block(child)) => {
                let child = child.clone();
                let (n, a) = doc
                    .block(at + f.offset, &child)
                    .ok_or("the block is outside the metadata")?;
                if i >= n {
                    return Err(format!("{name} has {n} elements, not {}", i + 1));
                }
                if last {
                    return Err(format!("{path} is an element, not a field"));
                }
                block(at + f.offset, i);
                at = a + i * child.size;
                layout = child;
            }
            _ => {
                return Err(format!(
                    "{part}: only a block takes an index, and only a block goes deeper"
                ));
            }
        }
    }
    Err("empty path".into())
}

/// The command that sets the field at `path` to `v` (the JSON
/// [`read_tag`] gives for it): numbers or arrays of them, enum names or
/// values, flag-name lists, strings the map has for string IDs,
/// `"group:name"` or null for tag references, hex for data of the same
/// length.
pub fn set_field(doc: &TagDoc, tag: TagId, path: &str, v: &Value) -> Result<TagAction, String> {
    let (at, _, f) = resolve(doc, tag, path)?;
    let a = at + f.offset;
    let w = f.kind.width();
    let nums = |n: usize| -> Result<Vec<f64>, String> {
        let list: Vec<&Value> = match v {
            Value::Array(a) => a.iter().collect(),
            x => vec![x],
        };
        if list.len() != n {
            return Err(format!("{}: {n} numbers wanted", f.name));
        }
        list.iter()
            .map(|x| {
                x.as_f64()
                    .ok_or_else(|| format!("{}: {x} is not a number", f.name))
            })
            .collect()
    };
    let lookup = |text: &str| -> Result<(FourCC, u32), String> {
        doc.tags()
            .iter()
            .find(|t| format!("{}:{}", tags::defs::fourcc_text(t.group), t.name) == text)
            .map(|t| (t.group, t.id.0))
            .ok_or_else(|| format!("no tag {text:?} in this map"))
    };
    let bytes: Vec<u8> = match &f.kind {
        Kind::Int {
            bytes,
            signed,
            count,
        } => {
            let n = *bytes as usize;
            let mut out = Vec::new();
            for x in nums(*count as usize)? {
                let x = x as i64;
                let (lo, hi) = if *signed {
                    (-(1i64 << (8 * n - 1)), (1i64 << (8 * n - 1)) - 1)
                } else {
                    (0, (1i64 << (8 * n)) - 1)
                };
                if !(lo..=hi).contains(&x) {
                    return Err(format!("{}: {x} is out of range", f.name));
                }
                out.extend_from_slice(&x.to_le_bytes()[..n]);
            }
            out
        }
        Kind::Float { count, .. } => nums(*count as usize)?
            .into_iter()
            .flat_map(|x| (x as f32).to_le_bytes())
            .collect(),
        Kind::Enum { bytes, values } => {
            let i = match v {
                Value::String(s) => values
                    .iter()
                    .position(|x| x == s)
                    .ok_or_else(|| format!("{}: no value {s:?}", f.name))?
                    as u64,
                x => x
                    .as_u64()
                    .ok_or_else(|| format!("{}: a name or a number", f.name))?,
            };
            i.to_le_bytes()[..*bytes as usize].to_vec()
        }
        Kind::Flags { bytes, bits } => {
            let list = v
                .as_array()
                .ok_or_else(|| format!("{}: a list of flag names", f.name))?;
            let mut m = 0u64;
            for x in list {
                let s = x.as_str().unwrap_or_default();
                let bit = bits
                    .iter()
                    .position(|b| b == s)
                    .or_else(|| s.strip_prefix("bit").and_then(|n| n.parse().ok()))
                    .filter(|b| *b < 8 * *bytes as usize)
                    .ok_or_else(|| format!("{}: no flag {x}", f.name))?;
                m |= 1 << bit;
            }
            m.to_le_bytes()[..*bytes as usize].to_vec()
        }
        Kind::StringId => match v {
            Value::Null => u32::MAX.to_le_bytes().to_vec(),
            Value::String(s) => match doc.string_id(s) {
                Some(id) => id.to_le_bytes().to_vec(),
                // A string the map lacks: added with the write.
                None => {
                    return Ok(TagAction::SetString {
                        tag,
                        address: a,
                        text: s.clone(),
                        label: f.name.clone(),
                    });
                }
            },
            _ => return Err(format!("{}: a string or null", f.name)),
        },
        Kind::TagRef { groups } | Kind::Datum { groups } => {
            let (g, id) = match v {
                Value::Null => (None, u32::MAX),
                Value::String(s) => {
                    let (g, id) = lookup(s)?;
                    if !doc.fits(g, groups) {
                        return Err(format!("{}: {s} is not of an allowed group", f.name));
                    }
                    (Some(g), id)
                }
                _ => return Err(format!("{}: \"group:name\" or null", f.name)),
            };
            if matches!(f.kind, Kind::TagRef { .. }) {
                let old = doc.read(a, 4).ok_or("unreadable")?;
                let mut out = g.map_or(old.clone(), |g| g.as_u32().to_le_bytes().to_vec());
                out.extend_from_slice(&id.to_le_bytes());
                out
            } else {
                id.to_le_bytes().to_vec()
            }
        }
        Kind::Data => {
            let (n, start) = doc.pair(a).ok_or("unreadable")?;
            let new = v
                .as_str()
                .and_then(crate::fields::parse_hex)
                .ok_or_else(|| format!("{}: hex bytes", f.name))?;
            if new.len() != n as usize {
                return Err(format!(
                    "{}: {n} bytes wanted (data keeps its length)",
                    f.name
                ));
            }
            return Ok(TagAction::Write(TagWrite {
                tag,
                address: start,
                bytes: new,
                label: f.name.clone(),
                merge: None,
            }));
        }
        Kind::Word | Kind::Unknown { .. } => {
            let x = v
                .as_u64()
                .ok_or_else(|| format!("{}: an unsigned integer", f.name))?;
            if w < 8 && x >> (8 * w) != 0 {
                return Err(format!("{}: {x} does not fit {w} bytes", f.name));
            }
            x.to_le_bytes()[..w as usize].to_vec()
        }
        k => {
            return Err(format!(
                "{} ({}) is structural: {}",
                f.name,
                k.label(),
                super::tag_doc::STRUCTURAL
            ));
        }
    };
    Ok(TagAction::Write(TagWrite {
        tag,
        address: a,
        bytes,
        label: f.name.clone(),
        merge: None,
    }))
}

/// The command that changes the block at `path` (a block field).
pub fn block(doc: &TagDoc, tag: TagId, path: &str, op: BlockOp) -> Result<TagAction, String> {
    let (at, _, f) = resolve(doc, tag, path)?;
    let Kind::Block(child) = &f.kind else {
        return Err(format!("{} is not a block", f.name));
    };
    let pair = at + f.offset;
    if let Some(why) = doc.block_refusal(tag, pair, child, &op) {
        return Err(why.into());
    }
    Ok(TagAction::Block {
        tag,
        pair,
        layout: child.clone(),
        op,
    })
}

/// Data references and raw pointers [`resources`] lists at most.
pub const RESOURCES: usize = 4096;

/// A tag's data references and raw-resource pointers, each with its path
/// from the root (as [`set_field`] takes it): `{data: [{path, size,
/// address}], raw: [{path, file, offset, size}], truncated}`. A pointer's
/// size is the element's `RawSize` field for it, when it has one; `file`
/// is the map it selects (`local`, `mainmenu`, `shared`,
/// `single_player_shared`). Empty and none pointers are left out.
pub fn resources(doc: &TagDoc, tag: TagId) -> Result<Value, String> {
    let t = doc.tag(tag).ok_or("no such tag")?;
    if let Access::None(why) = &t.access {
        return Err((*why).to_owned());
    }
    let layout = doc.layout(tag).ok_or("no layout for this group")?.clone();
    let mut data = Vec::new();
    let mut raw = Vec::new();
    walk_resources(doc, t.address, &layout, "", &mut data, &mut raw, 0);
    let truncated = data.len() + raw.len() >= RESOURCES;
    Ok(json!({ "data": data, "raw": raw, "truncated": truncated }))
}

fn walk_resources(
    doc: &TagDoc,
    at: u32,
    l: &Layout,
    prefix: &str,
    data: &mut Vec<Value>,
    raw: &mut Vec<Value>,
    depth: usize,
) {
    if depth > 16 {
        return;
    }
    for f in &l.fields {
        if data.len() + raw.len() >= RESOURCES {
            return;
        }
        let path = if prefix.is_empty() {
            f.name.clone()
        } else {
            format!("{prefix}.{}", f.name)
        };
        let a = at + f.offset;
        match &f.kind {
            Kind::Data => {
                if let Some((n, start)) = doc.pair(a)
                    && n > 0
                {
                    data.push(json!({
                        "path": path,
                        "size": n,
                        "address": format!("{start:#010x}"),
                    }));
                }
            }
            Kind::Raw => {
                let Some(b) = doc.read(a, 4) else { continue };
                let v = le(&b) as u32;
                let Some(p) = map::RawPointer::decode(v).filter(|_| v != 0) else {
                    continue;
                };
                let size = l
                    .fields
                    .iter()
                    .find(|g| matches!(g.kind, Kind::RawSize { of } if of == f.offset))
                    .and_then(|g| doc.read(at + g.offset, 4))
                    .map(|b| le(&b));
                raw.push(json!({
                    "path": path,
                    "file": editor_core::tag_bundle::file_name(p.file),
                    "offset": p.offset,
                    "size": size,
                }));
            }
            Kind::Block(child) => {
                let Some((n, start)) = doc.block(a, child) else {
                    continue;
                };
                for i in 0..n {
                    let e = format!("{path}[{i}]");
                    walk_resources(doc, start + i * child.size, child, &e, data, raw, depth + 1);
                    if data.len() + raw.len() >= RESOURCES {
                        return;
                    }
                }
            }
            _ => {}
        }
    }
}

/// A tag of `doc` by any of the names the agent API takes: an ID (`0x...`
/// or a number), `group:name`, a name, `name.group`, or a group the map
/// has one tag of (`scnr`, `matg`).
pub fn lookup(doc: &TagDoc, want: &Value) -> Result<TagId, String> {
    let by_id = |id: u32| {
        doc.tag(TagId(id))
            .map(|t| t.id)
            .ok_or_else(|| format!("no tag {id:#010x}"))
    };
    if let Some(n) = want.as_u64() {
        return by_id(u32::try_from(n).map_err(|_| format!("no tag {n}"))?);
    }
    let s = want
        .as_str()
        .ok_or_else(|| format!("tag: an id or a name, not {want}"))?;
    if let Some(hex) = s.strip_prefix("0x")
        && let Ok(id) = u32::from_str_radix(hex, 16)
    {
        return by_id(id);
    }
    if let Some(id) = find_tag(doc, s) {
        return Ok(id);
    }
    let text = |t: &super::tag_doc::TagEntry| tags::defs::fourcc_text(t.group);
    let mut hits: Vec<TagId> = doc
        .tags()
        .iter()
        .filter(|t| {
            t.name.eq_ignore_ascii_case(s)
                || format!("{}.{}", t.name, text(t)).eq_ignore_ascii_case(s)
        })
        .map(|t| t.id)
        .collect();
    if hits.is_empty() {
        hits = doc
            .tags()
            .iter()
            .filter(|t| text(t).eq_ignore_ascii_case(s))
            .map(|t| t.id)
            .collect();
    }
    match hits[..] {
        [one] => Ok(one),
        [] => Err(format!("no tag named {s:?} (tag.list finds them)")),
        _ => Err(format!("{s:?} names {} tags; use group:name or an id", hits.len())),
    }
}

/// A tag by `group:name`.
pub fn find_tag(doc: &TagDoc, text: &str) -> Option<TagId> {
    doc.tags()
        .iter()
        .find(|t| format!("{}:{}", tags::defs::fourcc_text(t.group), t.name) == text)
        .map(|t| t.id)
}
