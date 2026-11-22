use super::*;
use std::collections::BTreeMap;
use tags::defs::Kind;

#[derive(Debug)]
pub enum BatchError {
    Field(Error),
    /// The fields, raw entries and moved blocks whose game bytes are not
    /// the `was` the request gave: nothing was written.
    Mismatch { fields: Vec<Value>, raw: Vec<Value>, moves: Vec<Value> },
    /// The edit arena has no room for the move of the pair at `root`:
    /// `need` bytes asked for, `largest_free` the largest it could give.
    /// Nothing was written.
    Full { root: u32, need: u32, largest_free: u32, why: String },
}

/// Which `(count, address)` pair a move repoints.
#[derive(Clone, Debug)]
pub enum MoveRoot {
    /// A block field by path (`materials`, `+0x50:u32[2]`).
    Path { tag: TagSel, path: String },
    /// The pair's address: inside a loaded tag, or inside a block an
    /// earlier move put in the arena.
    At(u32),
}

/// One `moves` entry of `edit.batch` (docs/editor/realtime-edits.md,
/// "Four write primitives"): the block's new elements, copied into an
/// arena extent with room for `capacity` of them, and the root repointed
/// at it with the new count.
#[derive(Clone, Debug)]
pub struct Move {
    pub root: MoveRoot,
    /// Bytes per element.
    pub stride: u32,
    /// Elements in `bytes`, the root's new count.
    pub count: u32,
    /// Elements of room (`count` when not given).
    pub capacity: Option<u32>,
    /// `count * stride` bytes.
    pub bytes: Vec<u8>,
    /// The `(count, address)` the root holds before the move.
    pub was: (u32, u32),
}

/// A checked move, before its extent is allocated.
struct PlannedMove {
    tag: u32,
    root: u32,
    path: String,
    old: (u32, u32),
    count: u32,
    capacity: u32,
    stride: u32,
    bytes: Vec<u8>,
}

/// A journalled move.
#[derive(Clone, Debug)]
pub(crate) struct Moved {
    pub id: u64,
    pub scenario: u32,
    pub tag: u32,
    pub root: u32,
    pub path: String,
    /// The pair before and after.
    pub old: (u32, u32),
    pub new: (u32, u32),
    /// The arena extent the block went to.
    pub extent: u32,
}

impl Moved {
    fn json(&self) -> Value {
        json!({"id": self.id, "tag": format!("{:#010x}", self.tag), "root": self.root,
            "path": self.path, "was": {"count": self.old.0, "address": self.old.1},
            "count": self.new.0, "address": self.new.1})
    }
}

fn pair(count: u32, address: u32) -> [u8; 8] {
    let mut b = [0u8; 8];
    b[..4].copy_from_slice(&count.to_le_bytes());
    b[4..].copy_from_slice(&address.to_le_bytes());
    b
}

/// Whether `[addr, end)` overlaps a range of `ranges` (start to end).
fn overlapping(ranges: &BTreeMap<u32, u64>, addr: u32, end: u64) -> bool {
    ranges
        .range(..=addr)
        .next_back()
        .is_some_and(|(_, last)| *last > u64::from(addr))
        || ranges
            .range(addr..)
            .next()
            .is_some_and(|(next, _)| u64::from(*next) < end)
}

impl From<Error> for BatchError {
    fn from(error: Error) -> Self {
        Self::Field(error)
    }
}

impl Editor {
    /// Resolve and compare the entire request while the guest is locked. Check write
    /// permissions before the first store, including pages crossed by a field.
    pub fn batch(
        &mut self,
        g: &mut Guest,
        writes: &[Value],
        label: Option<&str>,
    ) -> std::result::Result<Value, BatchError> {
        self.batch_with(g, writes, &RawBatch::default(), label)
    }

    /// [`Self::batch`] with raw writes, evictions and block moves
    /// (docs/editor/realtime-edits.md, "Four write primitives"): every
    /// field, raw and move `was` is checked, and every move's arena extent
    /// allocated, before the first write; tag bytes are written first, then
    /// the moved blocks and their roots, then the raw entries, then the
    /// evictions, all under one journal id. Paths and addresses resolve
    /// against the blocks as they are before the batch: a write into a
    /// block this batch moves lands in its old place, so edits inside a
    /// moved block go in its `bytes` or in a later batch.
    pub fn batch_with(
        &mut self,
        g: &mut Guest,
        writes: &[Value],
        raw: &RawBatch,
        label: Option<&str>,
    ) -> std::result::Result<Value, BatchError> {
        let idx = self.index(g)?;
        let mut entries = None;
        let mut fields = Vec::new();
        let mut changes = Vec::new();
        let mut ranges = BTreeMap::<u32, u64>::new();
        for w in writes {
            let v = w
                .get("value")
                .ok_or_else(|| Error::invalid("missing value"))?;
            let (entry, addr, kind, path) = if let Some(at) = w.get("at") {
                if w.get("tag").is_some() || w.get("path").is_some() {
                    return Err(Error::invalid("give at or tag and path").into());
                }
                let addr = at
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .or_else(|| u32::from_str_radix(at.as_str()?.strip_prefix("0x")?, 16).ok())
                    .ok_or_else(|| Error::invalid("at must be a 32-bit address"))?;
                let s = v
                    .as_str()
                    .filter(|s| {
                        !s.is_empty()
                            && s.len().is_multiple_of(2)
                            && s.bytes().all(|b| b.is_ascii_hexdigit())
                    })
                    .ok_or_else(|| Error::invalid("value must be nonempty hex bytes"))?;
                let len =
                    u32::try_from(s.len() / 2).map_err(|_| Error::invalid("value too large"))?;
                if entries.is_none() {
                    entries = Some(index::all(g, &idx).map_err(Error::unavailable)?);
                }
                // Inside a loaded tag, or inside a block a move put in the
                // arena (which belongs to the moved block's tag).
                let e = self.holder(g, &idx, entries.as_deref().unwrap(), addr, len)?;
                let path = if e.contains(addr, len) {
                    format!("+{:#x}:bytes[{len}]", addr - e.address)
                } else {
                    format!("@{addr:#010x}:bytes[{len}]")
                };
                (
                    e,
                    addr,
                    tags::defs::Kind::Unknown {
                        len,
                        seen: String::new(),
                    },
                    path,
                )
            } else {
                let tag = w["tag"]
                    .as_str()
                    .ok_or_else(|| Error::invalid("missing tag"))?;
                let path = w["path"]
                    .as_str()
                    .ok_or_else(|| Error::invalid("missing path"))?;
                let sel = TagSel {
                    tag: tag.into(),
                    group: w["group"].as_str().map(str::to_owned),
                };
                let (_, e, loc) = self.locate(g, &sel, path)?;
                (e, loc.addr, loc.kind, path.into())
            };
            if !value::writable(&kind) {
                return Err(Error::invalid(format!("{path} is structural")).into());
            }
            let len = value::width(&kind);
            let end = u64::from(addr) + u64::from(len);
            if overlapping(&ranges, addr, end) {
                return Err(Error::invalid("overlapping fields in edit.batch").into());
            }
            ranges.insert(addr, end);
            g.check(addr, len as usize, runtime::guest::Access::Write, 1)
                .map_err(fault)?;
            let current = read_bytes(g, addr, len)?;
            let group_of = |d| index::by_datum(g, &idx, d).ok().map(|e| e.group);
            let encode = |v| value::encode(&kind, v, &current, &group_of).map_err(Error::invalid);
            let bytes = encode(v)?;
            if let Some(was) = w.get("was") {
                let expected = encode(was)?;
                if current != expected && current != bytes {
                    fields.push(json!({"at":addr,"tag":format!("{:#010x}",entry.datum),"path":path,
                        "was":was,"holds":value::read(&kind,&current),"holds_bytes":value::hex(&current)}));
                }
            }
            changes.push(Change {
                id: self.next_id + 1,
                scenario: idx.scenario,
                tag: entry.datum,
                tag_address: entry.address,
                changed: false,
                group: group_text(entry.group),
                name: self
                    .names(g, &idx)
                    .and_then(|n| n.name(entry.datum))
                    .map(str::to_owned),
                path,
                label: label.map(str::to_owned),
                addr,
                kind: value::kind_text(&kind),
                old: value::read(&kind, &current),
                new: value::read(&kind, &bytes),
                old_bytes: current,
                new_bytes: bytes,
                kind_def: kind,
                batch: true,
            });
        }
        let mut move_mismatches = Vec::new();
        let mut moves = Vec::new();
        for m in &raw.moves {
            let p = self.plan_move(g, &idx, m, &mut entries)?;
            let end = u64::from(p.root) + 8;
            if overlapping(&ranges, p.root, end) {
                return Err(Error::invalid(
                    "a move's root overlaps a field or another move in edit.batch",
                )
                .into());
            }
            ranges.insert(p.root, end);
            if p.old != m.was {
                move_mismatches.push(json!({"root": p.root, "tag": format!("{:#010x}", p.tag),
                    "path": p.path, "was": {"count": m.was.0, "address": m.was.1},
                    "holds": {"count": p.old.0, "address": p.old.1}}));
            }
            moves.push(p);
        }
        let mut raw_mismatches = Vec::new();
        let (planned, evict) = self.plan_raw(g, &idx, raw, &mut raw_mismatches)?;
        if !fields.is_empty() || !raw_mismatches.is_empty() || !move_mismatches.is_empty() {
            return Err(BatchError::Mismatch {
                fields,
                raw: raw_mismatches,
                moves: move_mismatches,
            });
        }
        // The extents before the first write: a full arena writes nothing.
        let extents = self.alloc_moves(g, &moves)?;
        self.next_id += 1;
        for c in &changes {
            g.write_bytes(c.addr, &c.new_bytes, 1).map_err(fault)?;
        }
        let mut reply = json!({"id":self.next_id,"applied":changes.len(),"changes":changes});
        self.journal
            .extend(changes.into_iter().filter(|c| c.old_bytes != c.new_bytes));
        if !moves.is_empty() {
            let moved = self.apply_moves(g, idx.scenario, self.next_id, moves, extents)?;
            reply["moves"] = json!(moved);
        }
        if !planned.is_empty() || !evict.is_empty() {
            let (raw, evicted) = self.apply_raw(g, idx.scenario, self.next_id, planned, evict)?;
            reply["raw"] = json!(raw);
            reply["evicted"] = json!(evicted);
        }
        Ok(reply)
    }

    /// Checks one move against the guest: its root, its sizes, its tag.
    fn plan_move(
        &mut self,
        g: &Guest,
        idx: &index::TagIndex,
        m: &Move,
        entries: &mut Option<Vec<index::Entry>>,
    ) -> Result<PlannedMove> {
        if self.arena.is_none() {
            return Err(Error::unavailable("the edit arena is not installed in this game"));
        }
        if !(1..=0x1_0000).contains(&m.stride) {
            return Err(Error::invalid("moves: stride must be 1 to 65536 bytes"));
        }
        let capacity = m.capacity.unwrap_or(m.count);
        if capacity == 0 || capacity < m.count {
            return Err(Error::invalid(format!(
                "moves: capacity {capacity} must be at least 1 and at least count {}",
                m.count
            )));
        }
        if m.bytes.len() as u64 != u64::from(m.count) * u64::from(m.stride) {
            return Err(Error::invalid(format!(
                "moves: {} bytes is not count {} times stride {}",
                m.bytes.len(),
                m.count,
                m.stride
            )));
        }
        let window = arena::WINDOW.end - arena::WINDOW.start;
        if u64::from(capacity) * u64::from(m.stride) > u64::from(window) {
            return Err(Error::invalid(format!(
                "moves: {capacity} elements of {} bytes are more than the edit arena's {window:#x} bytes",
                m.stride
            )));
        }
        let (entry, root, path) = match &m.root {
            MoveRoot::Path { tag, path } => {
                let (_, e, loc) = self.locate(g, tag, path)?;
                let pair = match &loc.kind {
                    Kind::Block(elem) => {
                        if elem.size != m.stride {
                            return Err(Error::invalid(format!(
                                "moves: {path} holds {}-byte elements, not {}",
                                elem.size, m.stride
                            )));
                        }
                        true
                    }
                    k @ (Kind::Int { .. } | Kind::Unknown { .. }) => value::width(k) == 8,
                    _ => false,
                };
                if !pair {
                    return Err(Error::invalid(format!(
                        "moves: {path} is {}, not a block's (count, address)",
                        value::kind_text(&loc.kind)
                    )));
                }
                (e, loc.addr, path.clone())
            }
            MoveRoot::At(at) => {
                if entries.is_none() {
                    *entries = Some(index::all(g, idx).map_err(Error::unavailable)?);
                }
                let e = self.holder(g, idx, entries.as_deref().unwrap(), *at, 8)?;
                (e, *at, format!("@{at:#010x}"))
            }
        };
        g.check(root, 8, runtime::guest::Access::Write, 1)
            .map_err(fault)?;
        let old = (
            g.u32(root).map_err(fault)?,
            g.u32(root + 4).map_err(fault)?,
        );
        Ok(PlannedMove {
            tag: entry.datum,
            root,
            path,
            old,
            count: m.count,
            capacity,
            stride: m.stride,
            bytes: m.bytes.clone(),
        })
    }

    /// An extent for each move, or none: a full arena frees what this
    /// batch took and refuses.
    fn alloc_moves(
        &mut self,
        g: &mut Guest,
        moves: &[PlannedMove],
    ) -> std::result::Result<Vec<arena::Extent>, BatchError> {
        if moves.is_empty() {
            return Ok(Vec::new());
        }
        let a = self
            .arena
            .clone()
            .ok_or_else(|| Error::unavailable("the edit arena is not installed in this game"))?;
        self.bind_arena(g);
        let mut held = arena::lock(&a);
        let mut out: Vec<arena::Extent> = Vec::new();
        for p in moves {
            let claim = arena::Claim {
                tag: p.tag,
                root: p.root,
                stride: p.stride,
                capacity: p.capacity,
            };
            match held.alloc(g, claim) {
                Ok(e) => out.push(e),
                Err(full) => {
                    for e in &out {
                        held.free(e.at);
                    }
                    return Err(BatchError::Full {
                        root: p.root,
                        need: full.need,
                        largest_free: full.largest_free,
                        why: full.why,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Copies each block into its extent (the tail zeroed), then writes
    /// its root's new count and address; journalled under `id`.
    fn apply_moves(
        &mut self,
        g: &mut Guest,
        scenario: u32,
        id: u64,
        moves: Vec<PlannedMove>,
        extents: Vec<arena::Extent>,
    ) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for (p, e) in moves.into_iter().zip(extents) {
            let mut block = p.bytes;
            block.resize(e.len as usize, 0);
            g.write_bytes(e.at, &block, 1).map_err(fault)?;
            g.write_bytes(p.root, &pair(p.count, e.at), 1).map_err(fault)?;
            out.push(json!({"root": p.root, "tag": format!("{:#010x}", p.tag), "path": p.path,
                "count": p.count, "address": e.at, "capacity": e.capacity}));
            self.moves.push(Moved {
                id,
                scenario,
                tag: p.tag,
                root: p.root,
                path: p.path,
                old: p.old,
                new: (p.count, e.at),
                extent: e.at,
            });
        }
        Ok(out)
    }

    /// The move journal for `edit.changes`.
    pub(crate) fn moves_json(&self) -> Vec<Value> {
        self.moves.iter().map(Moved::json).collect()
    }

    /// Reverts the moves of `id` (all with `None`), newest first: a root
    /// that still points at its extent gets its old pair back and the
    /// extent is freed; one pointed elsewhere since is left alone.
    pub(crate) fn revert_moves(
        &mut self,
        g: &mut Guest,
        id: Option<u64>,
        scenario: Option<u32>,
        reverted: &mut Vec<Value>,
        dropped: &mut Vec<Value>,
    ) -> Result<()> {
        let mut i = self.moves.len();
        while i > 0 {
            i -= 1;
            if id.is_some_and(|want| self.moves[i].id != want) {
                continue;
            }
            let m = self.moves.remove(i);
            if scenario != Some(m.scenario) {
                dropped.push(json!({"id": m.id, "move": m.json(), "why": "made on another map"}));
                continue;
            }
            let now = (
                g.u32(m.root).map_err(fault)?,
                g.u32(m.root + 4).map_err(fault)?,
            );
            if now.1 != m.new.1 {
                dropped.push(json!({"id": m.id, "move": m.json(),
                    "holds": {"count": now.0, "address": now.1},
                    "why": "the block was moved since; left alone"}));
                continue;
            }
            g.write_bytes(m.root, &pair(m.old.0, m.old.1), 1).map_err(fault)?;
            if let Some(a) = &self.arena {
                arena::lock(a).free(m.extent);
            }
            reverted.push(json!({"id": m.id, "move": m.json()}));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
