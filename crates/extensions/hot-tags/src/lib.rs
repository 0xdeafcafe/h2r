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
//!   (crates/platform/d3d `dirty`): no restart, no explicit invalidation;
//! * a field the engine reads once, when an object or the map is made
//!   (placements, most object definitions), shows when it is made again.
//!
//! Edits are tied to the map they were made on (the scenario's datum): a
//! journal entry from another map is never written back.
//!
//! **Raw data.** Section vertices and indices and bitmap pixels are not in
//! the tags but in the map files' raw blocks, which the guest pages in and
//! evicts. [`raw`] writes them live: an overlay entry in the file view
//! both the guest and the native renderer read, and the resident copy when
//! there is one (docs/editor/realtime-edits.md).
//!
//! **Grown blocks.** A tag block that must hold more elements than the map
//! gave it is copied into the edit [`arena`] and its `(count, address)`
//! pair repointed (`moves` in [`Editor::batch_with`]); writes inside the
//! moved block count as writes to its tag.

pub mod arena;
mod batch;
pub use batch::{BatchError, Move, MoveRoot};
pub mod index;
pub mod names;
pub mod path;
pub mod raw;
pub mod value;

pub use raw::{RawBatch, RawFiles, RawWrite, Refresh, RefreshSink, Was};

use std::collections::BTreeMap;
use std::ops::Range;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(serialize_with = "hex32")]
    pub addr: u32,
    #[serde(rename = "type")]
    pub kind: String,
    pub old: Value,
    pub new: Value,
    #[serde(skip)]
    batch: bool,
    #[serde(skip)]
    tag_address: u32,
    #[serde(skip)]
    changed: bool,
    #[serde(skip)]
    old_bytes: Vec<u8>,
    #[serde(skip)]
    new_bytes: Vec<u8>,
    #[serde(skip)]
    kind_def: tags::defs::Kind,
}

impl Change {
    fn range(&self) -> Range<u64> {
        let start = u64::from(self.addr);
        start..start + self.new_bytes.len() as u64
    }
}

fn overlaps(left: &Range<u64>, right: &Range<u64>) -> bool {
    left.start < right.end && right.start < left.end
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
    epoch: Option<u64>,
    names: Option<NameSource>,
    cached: Option<Names>,
    /// The file view and overlay raw writes go to ([`Editor::with_raw`]).
    raw: Option<Box<dyn RawFiles>>,
    /// Where refreshes go after a raw act.
    refresh: Option<RefreshSink>,
    /// Raw writes, evictions and appends, by batch id.
    raw_journal: Vec<raw::RawChange>,
    /// Where moved blocks go ([`Editor::with_arena`]).
    arena: Option<arena::Shared>,
    /// The map activation [`Editor::at_epoch`] last saw.
    map_epoch: Option<u64>,
    /// Block moves, by batch id.
    moves: Vec<batch::Moved>,
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
    /// A map load or restore replaces the journal's base. A BSP switch can
    /// leave allocations in place, including several writes to the same field.
    pub fn at_epoch(&mut self, g: &Guest, epoch: u64, map_epoch: u64) -> Result<()> {
        if self.epoch == Some(epoch) {
            return Ok(());
        }
        self.cached = None;
        self.map_epoch = Some(map_epoch);
        if self.epoch.is_none_or(|e| e < map_epoch) {
            self.journal.clear();
            self.clear_raw();
            // The arena itself is released at its next binding
            // ([`Self::bind_arena`]), which holds the guest mutably.
            self.moves.clear();
        }
        if self.journal.is_empty() {
            self.epoch = Some(epoch);
            return Ok(());
        }
        // During a BSP change the tag table can still be loading. Retry
        // without consuming the epoch or losing the journal if a read faults.
        let idx = self.index(g)?;
        self.moves.retain(|m| m.scenario == idx.scenario);
        let mut slots = BTreeMap::new();
        for c in &self.journal {
            let slot = c.tag & 0xffff;
            if c.scenario == idx.scenario
                && slot < idx.count
                && let std::collections::btree_map::Entry::Vacant(v) = slots.entry(slot)
            {
                v.insert(index::entry(g, &idx, slot).map_err(Error::unavailable)?);
            }
        }
        let held = self.arena.clone();
        let held = held.as_ref().map(|a| arena::lock(a));
        self.journal.retain(|c| {
            c.scenario == idx.scenario
                && slots
                    .get(&(c.tag & 0xffff))
                    .and_then(Option::as_ref)
                    .is_some_and(|entry| {
                        entry.datum == c.tag
                            && entry.address == c.tag_address
                            && group_text(entry.group) == c.group
                            && admits(held.as_deref(), entry, c.addr, c.new_bytes.len() as u32)
                    })
        });
        drop(held);
        // Model reverse undo in a scratch copy: an older write hidden by a
        // later journal entry is still restorable. Never write into the game.
        let mut earlier = BTreeMap::new();
        let mut protected = Vec::new();
        for c in self.journal.iter_mut().rev() {
            let range = c.range();
            let matches = read_bytes(g, c.addr, c.new_bytes.len() as u32).is_ok_and(|bytes| {
                range
                    .clone()
                    .zip(bytes)
                    .zip(&c.new_bytes)
                    .all(|((at, live), &new)| earlier.get(&at).copied().unwrap_or(live) == new)
            });
            if c.changed || !matches || protected.iter().any(|r| overlaps(r, &range)) {
                c.changed = true;
                protected.push(range);
            } else {
                earlier.extend(range.zip(c.old_bytes.iter().copied()));
            }
        }
        self.epoch = Some(epoch);
        Ok(())
    }

    pub fn new() -> Editor {
        Editor::default()
    }

    /// Moved blocks go to `arena` (shared with `world.placement.edit`).
    pub fn with_arena(mut self, arena: arena::Shared) -> Editor {
        self.arena = Some(arena);
        self
    }

    /// The edit arena, for a user that places its own extents in it
    /// (`edit.collision.stage`); `None` in a game without one.
    pub fn shared_arena(&self) -> Option<arena::Shared> {
        self.arena.clone()
    }

    /// Ties the arena to the map activation [`Self::at_epoch`] last saw: a
    /// new map releases every extent and unmaps its pages.
    pub fn bind_arena(&mut self, g: &mut Guest) {
        if let (Some(a), Some(epoch)) = (&self.arena, self.map_epoch) {
            arena::lock(a).bind(g, epoch);
        }
    }

    /// `edit.arena`: the window, what is mapped and used, the largest
    /// extent a move could have now, free physical memory and the moved
    /// blocks.
    pub fn arena(&mut self, g: &mut Guest) -> Result<Value> {
        let a = self
            .arena
            .clone()
            .ok_or_else(|| Error::unavailable("the edit arena is not installed in this game"))?;
        self.bind_arena(g);
        let (mut out, tags) = {
            let held = arena::lock(&a);
            let tags: Vec<u32> = held.extents().map(|e| e.tag).collect();
            (held.summary(g), tags)
        };
        let names = self.names_of(g, &tags);
        if let Some(list) = out["moves"].as_array_mut() {
            for (m, name) in list.iter_mut().zip(names) {
                m["name"] = json!(name);
            }
        }
        Ok(out)
    }

    /// Whether `[addr, addr + len)` lies in the edit arena's window but
    /// no extent of `tag` holds it any more: the block it was in has been
    /// moved back or freed, so the bytes are free arena memory. Addresses
    /// outside the window are never freed: a path write lands wherever
    /// the tag's block pointers lead, which need not be inside the tag
    /// entry's own allocation.
    fn freed(&self, tag: u32, addr: u32, len: u32) -> bool {
        self.arena.as_ref().is_some_and(|a| {
            let h = arena::lock(a);
            let end = u64::from(h.base()) + u64::from(h.len());
            addr >= h.base()
                && u64::from(addr) < end
                && h.containing(addr, len).is_none_or(|x| x.tag != tag)
        })
    }

    /// The loaded tag `[addr, addr + len)` belongs to: inside its
    /// allocation, or inside an arena extent holding one of its blocks.
    fn holder(
        &self,
        g: &Guest,
        idx: &index::TagIndex,
        entries: &[index::Entry],
        addr: u32,
        len: u32,
    ) -> Result<index::Entry> {
        if let Some(e) = entries.iter().find(|e| e.contains(addr, len)) {
            return Ok(e.clone());
        }
        let tag = self
            .arena
            .as_ref()
            .and_then(|a| arena::lock(a).containing(addr, len).map(|x| x.tag));
        let end = u64::from(addr) + u64::from(len);
        match tag {
            Some(tag) => index::by_datum(g, idx, tag).map_err(|e| {
                Error::invalid(format!(
                    "{addr:#010x}..{end:#x} is in a moved block of a tag no longer loaded: {e}"
                ))
            }),
            None => Err(Error::invalid(format!(
                "{addr:#010x}..{end:#x} outside loaded tags"
            ))),
        }
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
        self.select_in(g, idx, sel, &[])
    }

    /// [`Self::select`], a bare name resolved among the tags of `among`
    /// (any group when empty). A name is `name`, `group:name` or
    /// `name.group`; a group given that way is taken as is.
    fn select_in(
        &mut self,
        g: &Guest,
        idx: &index::TagIndex,
        sel: &TagSel,
        among: &[FourCC],
    ) -> Result<index::Entry> {
        let t = sel.tag.trim();
        let hex = t.strip_prefix("0x").unwrap_or(t);
        if hex.len() == 8 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            let d = u32::from_str_radix(hex, 16).unwrap();
            return index::by_datum(g, idx, d).map_err(Error::invalid);
        }
        let (name, group) = match &sel.group {
            Some(gr) => (t, Some(gr.as_str())),
            None => match (t.split_once(':'), t.rsplit_once('.')) {
                (Some((gr, n)), _) if gr.len() <= 4 && !gr.contains('\\') => (n, Some(gr)),
                (_, Some((n, gr))) if parse_fourcc(gr).is_some() && !gr.contains('\\') => {
                    (n, Some(gr))
                }
                _ => (t, None),
            },
        };
        let group = group
            .map(|gr| {
                // `scen:` for `scen`, and `bot:` for `bot ` (three letters).
                parse_fourcc(&format!("{gr:<4}"))
                    .ok_or_else(|| Error::invalid(format!("{gr:?} is not a group")))
            })
            .transpose()?;
        let Some(names) = self.names(g, idx) else {
            return Err(Error::invalid(format!(
                "{t:?}: no tag names for this map (name tags by datum, from edit.tags)"
            )));
        };
        let all = names.find(name, group);
        let found: Vec<_> = if group.is_none() && !among.is_empty() {
            all.iter()
                .filter(|(_, g)| among.contains(g))
                .copied()
                .collect()
        } else {
            all.clone()
        };
        let groups = |l: &[(u32, FourCC)]| {
            l.iter()
                .map(|(_, g)| group_text(*g))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match (found.as_slice(), all.as_slice()) {
            ([], []) => Err(Error::invalid(format!("no tag named {name:?}"))),
            ([], others) => Err(Error::invalid(format!(
                "{name:?} names no {} tag (it names {} tag(s): {})",
                among
                    .iter()
                    .map(|g| group_text(*g))
                    .collect::<Vec<_>>()
                    .join("/"),
                others.len(),
                groups(others)
            ))),
            ([(d, _)], _) => index::by_datum(g, idx, *d).map_err(Error::invalid),
            (many, _) => Err(Error::invalid(format!(
                "{name:?} names {} tags ({}): give the group, as {}:{name}",
                many.len(),
                groups(many),
                group_text(many[0].1)
            ))),
        }
    }

    /// The loaded map's tag `tag` names: a datum (`0xe1230045`), or a name
    /// with an optional `.group` (resolved through the names source, as
    /// `edit.*` resolves it); the entry and its name when known. For
    /// callers that act on a tag rather than edit it (`sim.give`).
    pub fn find_tag(&mut self, g: &Guest, tag: &str) -> Result<(index::Entry, Option<String>)> {
        self.find_tag_in(g, tag, &[])
    }

    /// [`Self::find_tag`], a bare name resolved among the tags of the
    /// groups `among` (any group when empty): a weapon's name finds its
    /// `weap` tag though its model, collision and animations share it. A
    /// `group:name` or `name.group` is taken as given.
    pub fn find_tag_in(
        &mut self,
        g: &Guest,
        tag: &str,
        among: &[&str],
    ) -> Result<(index::Entry, Option<String>)> {
        let among = among
            .iter()
            .map(|gr| {
                parse_fourcc(gr).ok_or_else(|| Error::invalid(format!("{gr:?} is not a group")))
            })
            .collect::<Result<Vec<_>>>()?;
        let idx = self.index(g)?;
        let sel = TagSel {
            tag: tag.to_owned(),
            group: None,
        };
        let e = self.select_in(g, &idx, &sel, &among)?;
        let name = self
            .names(g, &idx)
            .and_then(|n| n.name(e.datum))
            .map(str::to_owned);
        Ok((e, name))
    }

    pub fn loaded_names(&mut self, g: &Guest) -> Result<&Names> {
        let idx = self.index(g)?;
        self.names(g, &idx)
            .ok_or_else(|| Error::unavailable("loaded map names unavailable"))
    }

    /// The names of the tags `datums`, in order (from the loaded map's
    /// file): `None` for one the map does not name, and for all of them
    /// when there are no names (no source, no map loaded, or a file whose
    /// scenario is not the loaded one).
    pub fn names_of(&mut self, g: &Guest, datums: &[u32]) -> Vec<Option<String>> {
        let named = self.index(g).ok().and_then(|idx| {
            let n = self.names(g, &idx)?;
            Some(
                datums
                    .iter()
                    .map(|d| n.name(*d).map(str::to_owned))
                    .collect::<Vec<_>>(),
            )
        });
        named.unwrap_or_else(|| vec![None; datums.len()])
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

    /// `edit.read`: exact bytes inside a single loaded tag allocation.
    pub fn read(&self, g: &Guest, addr: u32, len: u32) -> Result<Value> {
        if !(1..=512 * 1024).contains(&len) {
            return Err(Error::invalid("len must be between 1 and 524288 bytes"));
        }
        let idx = self.index(g)?;
        for slot in 0..idx.count {
            let entry = index::entry(g, &idx, slot).map_err(Error::unavailable)?;
            if entry.is_some_and(|e| e.contains(addr, len)) {
                let bytes = read_bytes(g, addr, len)?;
                return Ok(json!({"at": addr, "holds_bytes": value::hex(&bytes)}));
            }
        }
        // A moved block reads as its tag's bytes do.
        if self
            .arena
            .as_ref()
            .is_some_and(|a| arena::lock(a).containing(addr, len).is_some())
        {
            let bytes = read_bytes(g, addr, len)?;
            return Ok(json!({"at": addr, "holds_bytes": value::hex(&bytes)}));
        }
        Err(Error::invalid("address range outside loaded tags"))
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
        let change = match self
            .journal
            .iter_mut()
            .rev()
            .find(|c| {
                c.scenario == idx.scenario
                    && u64::from(c.addr) < u64::from(loc.addr) + bytes.len() as u64
                    && u64::from(loc.addr) < u64::from(c.addr) + c.new_bytes.len() as u64
            })
            .filter(|c| {
                !c.batch && !c.changed && c.tag == e.datum && c.addr == loc.addr
                    && c.new_bytes == current
            })
        {
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
                    tag_address: e.address,
                    changed: false,
                    group: group_text(e.group),
                    name,
                    path: path.into(),
                    label: None,
                    addr: loc.addr,
                    kind: value::kind_text(&loc.kind),
                    old: value::read(&loc.kind, &current),
                    new,
                    batch: false,
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
        let mut list: Vec<Value> = Vec::new();
        for chunk in self.journal.chunk_by(|a,b| a.id == b.id) {
            let fields: Vec<_> = chunk.iter().map(|c| {
            let stale = scenario != Some(c.scenario);
                let now = (!stale)
                    .then(|| read_bytes(g, c.addr, c.new_bytes.len() as u32).ok())
                    .flatten();
                let mut v = serde_json::to_value(c).unwrap_or(Value::Null);
                v["was_bytes"] = json!(value::hex(&c.old_bytes));
                v["new_bytes"] = json!(value::hex(&c.new_bytes));
                v["holds"] = json!(now.as_ref().map(|b| value::hex(b)));
                v["stale"] = json!(stale);
                v["live"] = json!(!c.changed && now.as_ref() == Some(&c.new_bytes));
                if let Some(now) = &now {
                    v["now"] = value::read(&c.kind_def, now);
                }
                v
            }).collect();
            let first = &chunk[0];
            if first.batch {
                list.push(json!({"id":first.id,"label":first.label,"fields":fields}));
            } else {
                list.extend(fields);
            }
        }
        json!({ "changes": list, "raw": self.raw_changes(), "moves": self.moves_json() })
    }

    /// `edit.revert`: puts back the original bytes of one change (`id`) or
    /// of all (newest first). A change from another map is dropped
    /// unwritten; one whose bytes the game has since changed is left alone.
    pub fn revert(&mut self, g: &mut Guest, id: Option<u64>) -> Result<Value> {
        if let Some(id) = id
            && !self.journal.iter().any(|c| c.id == id)
            && !self.raw_journal.iter().any(|c| c.id() == id)
            && !self.moves.iter().any(|m| m.id == id)
        {
            return Err(Error::invalid(format!(
                "no change {id} (edit.changes lists them)"
            )));
        }
        let scenario = index::index(g).ok().map(|i| i.scenario);
        let mut reverted = Vec::new();
        let mut dropped = Vec::new();
        let mut protected = Vec::new();
        let mut retained = Vec::new();
        for index in (0..self.journal.len()).rev() {
            let c = &mut self.journal[index];
            let range = c.range();
            if protected.iter().any(|r| overlaps(r, &range)) {
                c.changed = true;
            }
            if id.is_some_and(|i| i != c.id) {
                if c.changed {
                    protected.push(range);
                } else {
                    retained.push(range);
                }
                continue;
            }
            if scenario != Some(c.scenario) {
                dropped.push(json!({"id": c.id, "at": c.addr, "tag": format!("{:#010x}", c.tag),
                    "path": c.path, "holds_bytes": null,
                    "why": "made on another map"}));
                self.journal.remove(index);
                continue;
            }
            // A field in a moved block that has since moved back: its
            // bytes are free arena memory now.
            let (tag, addr, len) = (c.tag, c.addr, c.new_bytes.len() as u32);
            let freed = self.freed(tag, addr, len);
            let c = &mut self.journal[index];
            if freed {
                dropped.push(json!({"id": c.id, "at": c.addr, "tag": format!("{:#010x}", c.tag),
                    "path": c.path, "holds_bytes": null,
                    "why": "its block is no longer in the edit arena"}));
                self.journal.remove(index);
                continue;
            }
            let now = read_bytes(g, c.addr, c.new_bytes.len() as u32)?;
            let why = if c.changed || now != c.new_bytes {
                Some("the game changed it since; left alone")
            } else if retained.iter().any(|r| overlaps(r, &range)) {
                Some("overlaps a newer journal entry; left alone")
            } else {
                None
            };
            if let Some(why) = why {
                dropped.push(json!({"id": c.id, "at": c.addr, "tag": format!("{:#010x}", c.tag),
                    "path": c.path, "holds_bytes": value::hex(&now), "why": why}));
                protected.push(range);
            } else {
                g.write_bytes(c.addr, &c.old_bytes, 1).map_err(fault)?;
                reverted.push(json!({"id": c.id, "at": c.addr, "tag": format!("{:#010x}", c.tag),
                    "path": c.path, "value": c.old, "holds_bytes": value::hex(&c.old_bytes)}));
            }
            self.journal.remove(index);
        }
        // Tag bytes first, then the moved blocks' pairs (a write inside a
        // moved block is undone before the block moves back), then raw: a
        // grown section's block info points at its old block again before
        // the section is evicted to page it in.
        self.revert_moves(g, id, scenario, &mut reverted, &mut dropped)?;
        self.revert_raw(g, id, &mut reverted, &mut dropped)?;
        Ok(json!({ "reverted": reverted, "dropped": dropped }))
    }
}

/// Whether `[addr, addr + len)` belongs to tag `e`: inside its allocation
/// or inside an extent of `arena` that holds one of its blocks.
fn admits(arena: Option<&arena::Arena>, e: &index::Entry, addr: u32, len: u32) -> bool {
    e.contains(addr, len)
        || arena
            .and_then(|a| a.containing(addr, len))
            .is_some_and(|x| x.tag == e.datum)
}

#[cfg(test)]
mod tests;
