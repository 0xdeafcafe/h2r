//! Live edits of the running game's tags (docs/graphics/hot-iteration.md).
//!
//! The loaded map's tags sit in guest memory at the addresses their own
//! pointers use ([`index`]); the editor crates' definitions
//! (`tags::defs`) name their fields. An [`Editor`] reads and writes a
//! field by path ([`path`]) and keeps a journal of what it changed, so a
//! change can be listed and reverted. Nothing is cached on the host: every
//! call reads the guest as it is.
//!
//! **When an edit shows.** Writes go through `Guest::write_bytes`, the
//! runtime's write barrier, like any guest store. So:
//! * a field the engine reads each frame (a shader's postprocess colours
//!   and vectors, its bitmap datums) shows on the next frame;
//! * bytes the D3D device made a host copy of (texture or vertex data)
//!   are re-read at the next D3D call, through the device's write tracker
//!   (crates/d3d `dirty`): no restart, no explicit invalidation;
//! * a field the engine reads once, when an object or the map is made
//!   (placements, most object definitions), shows when it is made again.
//!
//! Edits are tied to the map they were made on (the scenario's datum): a
//! journal entry from another map is never written back.

pub mod index;
pub mod names;
pub mod path;
pub mod value;

use map::FourCC;
use runtime::Guest;
use serde::Serialize;
use serde_json::{Value, json};
use tags::defs::{Defs, fourcc_text, parse_fourcc};

pub use names::{NameSource, Names};

/// What went wrong, for the API's error code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The request can't be met as asked (bad tag, path or value).
    Invalid,
    /// The game can't answer now (no map loaded, a read faulted).
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl Error {
    fn invalid(m: impl Into<String>) -> Error {
        Error {
            kind: ErrorKind::Invalid,
            message: m.into(),
        }
    }
    fn unavailable(m: impl Into<String>) -> Error {
        Error {
            kind: ErrorKind::Unavailable,
            message: m.into(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// One field the editor changed.
#[derive(Clone, Debug, Serialize)]
pub struct Change {
    pub id: u64,
    /// The scenario datum of the map it was made on.
    #[serde(serialize_with = "hex32")]
    pub scenario: u32,
    #[serde(serialize_with = "hex32")]
    pub tag: u32,
    pub group: String,
    pub name: Option<String>,
    pub path: String,
    #[serde(serialize_with = "hex32")]
    pub addr: u32,
    #[serde(rename = "type")]
    pub kind: String,
    pub old: Value,
    pub new: Value,
    #[serde(skip)]
    old_bytes: Vec<u8>,
    #[serde(skip)]
    new_bytes: Vec<u8>,
    #[serde(skip)]
    kind_def: tags::defs::Kind,
}

fn hex32<S: serde::Serializer>(v: &u32, s: S) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_str(&format!("{v:#010x}"))
}

/// Which tag: a datum (`"0xe1230045"`), or a name (from the map file;
/// `group` narrows it, or a `.group` suffix: `"...\\rifle.weap"`).
#[derive(Clone, Debug, Default)]
pub struct TagSel {
    pub tag: String,
    pub group: Option<String>,
}

/// Reads and writes the loaded map's tags, with a journal (crate docs).
#[derive(Default)]
pub struct Editor {
    journal: Vec<Change>,
    next_id: u64,
    names: Option<NameSource>,
    cached: Option<Names>,
}

fn fault(e: runtime::GuestFault) -> Error {
    Error::unavailable(format!("guest fault at {:#010x}", e.addr))
}

fn read_bytes(g: &Guest, addr: u32, len: u32) -> Result<Vec<u8>> {
    let mut b = vec![0; len as usize];
    g.read_bytes(addr, &mut b, 1).map_err(fault)?;
    Ok(b)
}

fn group_text(g: FourCC) -> String {
    fourcc_text(g)
}

impl Editor {
    pub fn new() -> Editor {
        Editor::default()
    }

    /// Names come from `source` (the loaded map's file).
    pub fn with_names(mut self, source: NameSource) -> Editor {
        self.names = Some(source);
        self
    }

    fn index(&self, g: &Guest) -> Result<index::TagIndex> {
        index::index(g).map_err(Error::unavailable)
    }

    /// The names table for the map loaded now, when there is a source.
    fn names(&mut self, g: &Guest, idx: &index::TagIndex) -> Option<&Names> {
        let fresh = self
            .cached
            .as_ref()
            .is_some_and(|n| n.scenario == idx.scenario);
        if !fresh {
            self.cached = self
                .names
                .as_ref()
                .and_then(|s| s(g))
                .filter(|n| n.scenario == idx.scenario);
        }
        self.cached.as_ref()
    }

    fn select(&mut self, g: &Guest, idx: &index::TagIndex, sel: &TagSel) -> Result<index::Entry> {
        let t = sel.tag.trim();
        let hex = t.strip_prefix("0x").unwrap_or(t);
        if hex.len() == 8 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            let d = u32::from_str_radix(hex, 16).unwrap();
            return index::by_datum(g, idx, d).map_err(Error::invalid);
        }
        let (name, group) = match &sel.group {
            Some(gr) => (t, Some(gr.as_str())),
            None => match t.rsplit_once('.') {
                Some((n, gr)) if parse_fourcc(gr).is_some() && !gr.contains('\\') => (n, Some(gr)),
                _ => (t, None),
            },
        };
        let group = group
            .map(|gr| {
                parse_fourcc(gr).ok_or_else(|| Error::invalid(format!("{gr:?} is not a group")))
            })
            .transpose()?;
        let Some(names) = self.names(g, idx) else {
            return Err(Error::invalid(format!(
                "{t:?}: no tag names for this map (name tags by datum, from edit.tags)"
            )));
        };
        match names.find(name, group).as_slice() {
            [] => Err(Error::invalid(format!("no tag named {name:?}"))),
            [(d, _)] => index::by_datum(g, idx, *d).map_err(Error::invalid),
            many => Err(Error::invalid(format!(
                "{name:?} names {} tags ({}): give the group",
                many.len(),
                many.iter()
                    .map(|(_, g)| group_text(*g))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    fn describe(&mut self, g: &Guest, idx: &index::TagIndex, e: &index::Entry) -> Value {
        let name = self
            .names(g, idx)
            .and_then(|n| n.name(e.datum))
            .map(str::to_owned);
        json!({
            "tag": format!("{:#010x}", e.datum),
            "group": group_text(e.group),
            "name": name,
            "address": format!("{:#010x}", e.address),
            "size": e.size,
            "defined": Defs::builtin().group(e.group).is_some(),
        })
    }

    /// `edit.tags`: the loaded map's tags, of `group` and with `contains`
    /// in their name when given, at most `limit`.
    pub fn tags(
        &mut self,
        g: &Guest,
        group: Option<&str>,
        contains: Option<&str>,
        limit: usize,
    ) -> Result<Value> {
        let idx = self.index(g)?;
        let group = group
            .map(|gr| {
                parse_fourcc(gr).ok_or_else(|| Error::invalid(format!("{gr:?} is not a group")))
            })
            .transpose()?;
        let contains = contains.map(names::fold);
        let mut out = Vec::new();
        let mut total = 0usize;
        for e in index::all(g, &idx).map_err(Error::unavailable)? {
            if group.is_some_and(|w| w != e.group) {
                continue;
            }
            if let Some(c) = &contains {
                let name = self
                    .names(g, &idx)
                    .and_then(|n| n.name(e.datum))
                    .map(names::fold);
                if !name.is_some_and(|n| n.contains(c.as_str())) {
                    continue;
                }
            }
            total += 1;
            if out.len() < limit {
                out.push(self.describe(g, &idx, &e));
            }
        }
        Ok(json!({
            "scenario": format!("{:#010x}", idx.scenario),
            "names": self.names(g, &idx).is_some(),
            "total": total,
            "tags": out,
        }))
    }

    fn locate(
        &mut self,
        g: &Guest,
        sel: &TagSel,
        path: &str,
    ) -> Result<(index::TagIndex, index::Entry, path::Located<'static>)> {
        let idx = self.index(g)?;
        let e = self.select(g, &idx, sel)?;
        let loc = match Defs::builtin().group(e.group) {
            Some(root) => path::resolve(g, root, e.address, path).map_err(Error::invalid)?,
            // No definition: only raw fields of the root.
            None => path::raw(g, e.address, path).map_err(|m| {
                Error::invalid(format!(
                    "{} has no definition: use a raw field, +<offset>:<type> (e.g. +0x10:f32[3]); {m}",
                    group_text(e.group)
                ))
            })?,
        };
        Ok((idx, e, loc))
    }

    /// `edit.get`: a field's value, or an element's fields with theirs.
    pub fn get(&mut self, g: &Guest, sel: &TagSel, path: &str) -> Result<Value> {
        let (idx, e, loc) = self.locate(g, sel, path)?;
        let mut out = self.describe(g, &idx, &e);
        out["path"] = json!(path);
        out["addr"] = json!(format!("{:#010x}", loc.addr));
        out["type"] = json!(value::kind_text(&loc.kind));
        if !loc.meaning.is_empty() {
            out["meaning"] = json!(loc.meaning);
        }
        match loc.element {
            Some(el) => {
                let mut fields = Vec::new();
                for f in &el.fields {
                    if matches!(f.kind, tags::defs::Kind::Pad { .. }) {
                        continue;
                    }
                    let w = f.kind.width();
                    let v = match &f.kind {
                        tags::defs::Kind::Struct(_) => json!("(struct: name it)"),
                        k => value::read(k, &read_bytes(g, loc.addr + f.offset, w)?),
                    };
                    fields.push(json!({
                        "name": f.name,
                        "offset": format!("{:#x}", f.offset),
                        "type": value::kind_text(&f.kind),
                        "value": v,
                    }));
                }
                out["fields"] = json!(fields);
            }
            None => {
                let b = read_bytes(g, loc.addr, value::width(&loc.kind))?;
                out["value"] = value::read(&loc.kind, &b);
            }
        }
        Ok(out)
    }

    /// `edit.set`: writes a field and journals it. Returns the change.
    pub fn set(&mut self, g: &mut Guest, sel: &TagSel, path: &str, v: &Value) -> Result<Value> {
        let (idx, e, loc) = self.locate(g, sel, path)?;
        if !value::writable(&loc.kind) {
            return Err(Error::invalid(format!(
                "{path} is {}: edit its fields",
                value::kind_text(&loc.kind)
            )));
        }
        let w = value::width(&loc.kind);
        let current = read_bytes(g, loc.addr, w)?;
        let group_of = |d: u32| index::by_datum(g, &idx, d).ok().map(|e| e.group);
        let bytes = value::encode(&loc.kind, v, &current, &group_of).map_err(Error::invalid)?;
        debug_assert_eq!(bytes.len(), w as usize);
        g.write_bytes(loc.addr, &bytes, 1).map_err(fault)?;
        let name = self
            .names(g, &idx)
            .and_then(|n| n.name(e.datum))
            .map(str::to_owned);
        let new = value::read(&loc.kind, &bytes);
        // A second edit of the same bytes keeps the first one's original.
        let change = match self.journal.iter_mut().find(|c| {
            c.scenario == idx.scenario && c.addr == loc.addr && c.new_bytes.len() == bytes.len()
        }) {
            Some(c) => {
                c.new_bytes = bytes;
                c.new = new;
                c.path = path.into();
                c.clone()
            }
            None => {
                self.next_id += 1;
                let c = Change {
                    id: self.next_id,
                    scenario: idx.scenario,
                    tag: e.datum,
                    group: group_text(e.group),
                    name,
                    path: path.into(),
                    addr: loc.addr,
                    kind: value::kind_text(&loc.kind),
                    old: value::read(&loc.kind, &current),
                    new,
                    old_bytes: current,
                    new_bytes: bytes,
                    kind_def: loc.kind.clone(),
                };
                self.journal.push(c.clone());
                c
            }
        };
        Ok(serde_json::to_value(change).unwrap_or(Value::Null))
    }

    /// `edit.changes`: the journal, each with whether its value is still
    /// there (`live`) and whether it was made on another map (`stale`).
    pub fn changes(&self, g: &Guest) -> Value {
        let scenario = index::index(g).ok().map(|i| i.scenario);
        let list: Vec<Value> = self
            .journal
            .iter()
            .map(|c| {
                let stale = scenario != Some(c.scenario);
                let now = (!stale)
                    .then(|| read_bytes(g, c.addr, c.new_bytes.len() as u32).ok())
                    .flatten();
                let mut v = serde_json::to_value(c).unwrap_or(Value::Null);
                v["stale"] = json!(stale);
                v["live"] = json!(now.as_ref() == Some(&c.new_bytes));
                if let Some(now) = &now {
                    v["now"] = value::read(&c.kind_def, now);
                }
                v
            })
            .collect();
        json!({ "changes": list })
    }

    /// `edit.revert`: puts back the original bytes of one change (`id`) or
    /// of all (newest first). A change from another map is dropped
    /// unwritten; one whose bytes the game has since changed is left alone.
    pub fn revert(&mut self, g: &mut Guest, id: Option<u64>) -> Result<Value> {
        if let Some(id) = id
            && !self.journal.iter().any(|c| c.id == id)
        {
            return Err(Error::invalid(format!(
                "no change {id} (edit.changes lists them)"
            )));
        }
        let scenario = index::index(g).ok().map(|i| i.scenario);
        let mut reverted = Vec::new();
        let mut dropped = Vec::new();
        let mut kept = Vec::new();
        for c in std::mem::take(&mut self.journal).into_iter().rev() {
            if id.is_some_and(|i| i != c.id) {
                kept.push(c);
                continue;
            }
            if scenario != Some(c.scenario) {
                dropped.push(json!({"id": c.id, "why": "made on another map"}));
                continue;
            }
            let now = read_bytes(g, c.addr, c.new_bytes.len() as u32)?;
            if now != c.new_bytes {
                dropped.push(json!({"id": c.id, "why": "the game changed it since; left alone"}));
                continue;
            }
            g.write_bytes(c.addr, &c.old_bytes, 1).map_err(fault)?;
            reverted.push(json!({"id": c.id, "path": c.path, "value": c.old}));
        }
        kept.reverse();
        self.journal = kept;
        Ok(json!({ "reverted": reverted, "dropped": dropped }))
    }
}

#[cfg(test)]
mod tests;
