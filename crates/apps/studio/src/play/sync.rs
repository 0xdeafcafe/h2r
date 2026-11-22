//! Which scenario edits can go to a running game, and as which `edit.batch`
//! writes.
//!
//! The game holds the loaded map's scenario (`scnr`) in guest memory, and
//! `edit.batch` writes a field there (docs/graphics/hot-iteration.md). The
//! scenario has no tag definition, so a placement's field is reached with
//! raw paths from `scenario_edit::layout`: `+<block>/<stride>[<i>].+<field>:
//! bytes[n]`, the field's exact bytes as hex.
//!
//! [`Baseline`] holds the loaded file's block layouts and bytes plus every
//! acknowledged write. [`diff`] lists field changes only for blocks whose
//! layout revision still matches the loaded copy. Adding or removing an
//! entry changes the revision even when the final count stays the same.
//!
//! A block whose layout revision differs goes to the game as one
//! `world.placement.edit` instead ([`Baseline::transaction`],
//! docs/editor/realtime-edits.md, "Placements"): the game's records, which
//! document placement sits in each slot, and the bytes a removal left past
//! the count are all here, so the transaction brings the game's block to the
//! document's, and [`Baseline::placed`] advances the loaded revision on its
//! receipt.

use std::collections::BTreeMap;
use std::ops::Range;

use agent_proto::sim::{PlacementEditParams, PlacementName, PlacementRecord, PlacementRelink, Was};
use scenario_edit::layout::{self, BLOCKS, FieldDef};
use scenario_edit::scenario::PlacementId;
use scenario_edit::{BlockKind, Scenario};

/// The scenario's placement blocks as the game is taken to hold them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Baseline {
    blocks: BTreeMap<BlockKind, LoadedBlock>,
    object_rows: super::objects::Rows,
    creation: Creation,
}

#[derive(Clone, Debug, PartialEq)]
struct LoadedBlock {
    layout_rev: u64,
    /// The game's records, `0..count`.
    entries: Vec<Vec<u8>>,
    /// The document placement in each slot of `entries`.
    ids: Vec<PlacementId>,
    /// Slots past the count whose bytes are known: what a removal left
    /// there. Slot `entries.len() + i` holds `tail[i]`.
    tail: Vec<Vec<u8>>,
}

impl LoadedBlock {
    fn slot(&self, index: usize) -> Option<&[u8]> {
        match index.checked_sub(self.entries.len()) {
            None => self.entries.get(index),
            Some(past) => self.tail.get(past),
        }
        .map(Vec::as_slice)
    }
}

/// Only slots whose creation fields differ from the last created object need
/// a snapshot. Slot indices here always refer to the loaded scenario.
#[derive(Clone, Debug, Default, PartialEq)]
struct Creation {
    slots: BTreeMap<(BlockKind, usize), Vec<u8>>,
    revision: u64,
}

fn creation_ranges(kind: BlockKind) -> impl Iterator<Item = Range<usize>> {
    let def = layout::block(kind);
    def.fields.iter().filter_map(move |f| {
        if def.object_type.is_none() || matches!(
            f.role, layout::Role::Position | layout::Role::Rotation | layout::Role::Facing
        ) {
            return None;
        }
        Some(f.offset as usize..(f.offset + f.ty.size()) as usize)
    })
}

fn same_creation_fields(kind: BlockKind, a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && creation_ranges(kind).all(|field| a.get(field.clone()) == b.get(field))
}

impl Creation {
    fn write(&mut self, slot: (BlockKind, usize), entry: &mut [u8], at: usize, bytes: &[u8]) {
        let end = at + bytes.len();
        let changed = creation_ranges(slot.0).any(|field| {
            let lo = field.start.max(at);
            let hi = field.end.min(end);
            lo < hi && entry[lo..hi] != bytes[lo - at..hi - at]
        });
        if changed {
            self.slots.entry(slot).or_insert_with(|| entry.to_vec());
        }
        entry[at..end].copy_from_slice(bytes);
        if changed {
            let created = &self.slots[&slot];
            if same_creation_fields(slot.0, entry, created) {
                self.slots.remove(&slot);
            }
            self.revision += 1;
        }
    }
}

/// One field write: what `edit.batch` gets, and what it is for the log.
#[derive(Clone, Debug, PartialEq)]
pub struct Write {
    pub block: BlockKind,
    pub index: usize,
    pub field: &'static str,
    /// The raw path into `scnr`.
    pub path: String,
    /// The field's new bytes, as hex.
    pub value: String,
    /// The bytes the baseline had, as hex: what the game should answer as
    /// `old`.
    pub was: String,
}

/// What [`diff`] found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Diff {
    pub writes: Vec<Write>,
    /// Blocks whose slot layout differs from the loaded copy.
    pub reload: Vec<BlockKind>,
}

impl Baseline {
    pub fn source(s: &Scenario) -> Self {
        Self {
            object_rows: super::objects::source(s),
            creation: Creation::default(),
            blocks: BLOCKS
                .iter()
                .map(|b| {
                    let entries = s.block(b.kind).source_entries().to_vec();
                    let block = LoadedBlock {
                        layout_rev: 0,
                        ids: (0..entries.len()).map(PlacementId::Source).collect(),
                        entries,
                        tail: Vec::new(),
                    };
                    (b.kind, block)
                })
                .collect(),
        }
    }

    /// Everything the scenario's placement blocks hold now.
    pub fn of(s: &Scenario) -> Baseline {
        let blocks = BLOCKS
            .iter()
            .map(|b| {
                let block = s.block(b.kind);
                let (entries, ids) = (0..block.len())
                    .filter_map(|i| Some((block.entry(i)?.to_vec(), block.placement_id(i)?)))
                    .unzip();
                let loaded = LoadedBlock {
                    layout_rev: block.layout_rev(),
                    entries,
                    ids,
                    tail: Vec::new(),
                };
                (b.kind, loaded)
            })
            .collect();
        Baseline {
            blocks,
            object_rows: super::objects::document(s),
            creation: Creation::default(),
        }
    }

    /// Placement frames in the file, independent of subsequent field acknowledgements.
    pub(super) fn object_rows(&self) -> &super::objects::Rows {
        &self.object_rows
    }

    /// The frame a respawn reads after acknowledged field writes.
    pub(super) fn object_frame(
        &self,
        kind: BlockKind,
        index: usize,
    ) -> Option<super::objects::Frame> {
        let bytes = self.blocks.get(&kind)?.entries.get(index)?;
        super::objects::Frame::from_entry(kind, bytes)
    }

    pub(super) fn creation_slots(&self) -> impl Iterator<Item = (BlockKind, usize)> + '_ {
        self.creation.slots.keys().copied()
    }

    pub(super) fn needs_creation(&self, kind: BlockKind, index: usize) -> bool {
        self.creation.slots.contains_key(&(kind, index))
    }

    pub(super) fn creation_revision(&self) -> u64 {
        self.creation.revision
    }

    /// Snapshot of the slot a dispatched respawn reads. A preview outside the
    /// loaded scenario has no creation fields to reconcile.
    pub(super) fn creation_fields(&self, kind: BlockKind, index: usize) -> Vec<u8> {
        self.blocks
            .get(&kind)
            .and_then(|b| b.entries.get(index))
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn creation_matches(&self, kind: BlockKind, index: usize, fields: &[u8]) -> bool {
        self.blocks.get(&kind).and_then(|b| b.entries.get(index))
            .is_some_and(|entry| same_creation_fields(kind, entry, fields))
    }

    pub(super) fn object_created(&mut self, kind: BlockKind, index: usize, created: &[u8]) {
        let Some(entry) = self.blocks.get(&kind).and_then(|b| b.entries.get(index)) else {
            return;
        };
        if created.len() != entry.len() {
            return;
        }
        let slot = (kind, index);
        if same_creation_fields(kind, entry, created) {
            self.creation.slots.remove(&slot);
        } else {
            self.creation.slots.insert(slot, created.to_vec());
        }
        self.creation.revision += 1;
    }

    /// The game now holds `w` (it was written).
    pub fn wrote(&mut self, w: &Write) {
        let Some(f) = layout::block(w.block).field(w.field) else {
            return;
        };
        let Some(entry) = self
            .blocks
            .get_mut(&w.block)
            .and_then(|b| b.entries.get_mut(w.index))
        else {
            return;
        };
        let (at, n) = (f.offset as usize, f.ty.size() as usize);
        if let Some(bytes) = unhex(&w.value)
            && bytes.len() == n && at + n <= entry.len()
        {
            self.creation.write((w.block, w.index), entry, at, &bytes);
        }
    }

    /// Apply bytes observed in the game to its placement baseline. Return the
    /// ranges of the input belonging to placements so the caller can reconcile
    /// the remaining bytes with the tag document.
    pub fn observe(&mut self, s: &Scenario, at: u32, bytes: &[u8]) -> Vec<Range<usize>> {
        let start = u64::from(at);
        let end = start + bytes.len() as u64;
        let mut spans = Vec::new();
        for (kind, loaded) in &mut self.blocks {
            let entries = &mut loaded.entries;
            let block = s.block(*kind);
            // The map writer relocates a grown block in a play copy. Only a
            // path resolved by the game identifies its slots in that case.
            if block.address == 0 || entries.len() > block.capacity() {
                continue;
            }
            let first = u64::from(block.address);
            let stride = u64::from(block.def.stride);
            let lo = start.max(first);
            let hi = end.min(first + stride * entries.len() as u64);
            if lo >= hi {
                continue;
            }
            let span = (lo - start) as usize..(hi - start) as usize;
            let mut src = &bytes[span.clone()];
            let mut offset = ((lo - first) % stride) as usize;
            let first_index = ((lo - first) / stride) as usize;
            for (index, entry) in entries.iter_mut().enumerate().skip(first_index) {
                let n = src.len().min(entry.len() - offset);
                self.creation.write((*kind, index), entry, offset, &src[..n]);
                src = &src[n..];
                if src.is_empty() {
                    break;
                }
                offset = 0;
            }
            spans.push(span);
        }
        spans.sort_unstable_by_key(|span| span.start);
        spans
    }

    /// Whether an index in this document block still names the game's slot.
    pub fn layout_matches(&self, s: &Scenario, kind: BlockKind) -> bool {
        self.blocks
            .get(&kind)
            .is_some_and(|loaded| loaded.layout_rev == s.block(kind).layout_rev())
    }

    /// Entries the baseline has in `kind`.
    pub fn len(&self, kind: BlockKind) -> usize {
        self.blocks.get(&kind).map_or(0, |b| b.entries.len())
    }

    /// The `world.placement.edit` that brings the game's block `kind` to the
    /// document's, or `None` when its layout already matches. `objects`: the
    /// block's records make objects, so the transaction deletes, relinks and
    /// creates them and repoints object names; other blocks take records and
    /// the count only.
    pub fn transaction(&self, now: &Scenario, kind: BlockKind, objects: bool) -> Option<Transaction> {
        let old = self.blocks.get(&kind)?;
        let block = now.block(kind);
        if old.layout_rev == block.layout_rev() {
            return None;
        }
        let (entries, ids): (Vec<Vec<u8>>, Vec<PlacementId>) = (0..block.len())
            .map(|i| Some((block.entry(i)?.to_vec(), block.placement_id(i)?)))
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .unzip();
        let old_slot = |id: &PlacementId| old.ids.iter().position(|o| o == id);
        let records = entries
            .iter()
            .enumerate()
            .filter_map(|(index, bytes)| {
                let was = old.slot(index);
                (was != Some(bytes.as_slice())).then(|| Record {
                    index,
                    bytes: bytes.clone(),
                    was: was.map(<[u8]>::to_vec),
                })
            })
            .collect();
        let mut t = Transaction {
            block: kind,
            was_count: old.entries.len(),
            count: entries.len(),
            records,
            names: Vec::new(),
            delete: Vec::new(),
            relink: Vec::new(),
            create: Vec::new(),
            layout_rev: block.layout_rev(),
            ids,
            entries,
        };
        let ty = layout::block(kind).object_type.filter(|_| objects);
        let Some(ty) = ty else {
            return Some(t);
        };
        t.delete = (0..old.ids.len())
            .filter(|&i| !t.ids.contains(&old.ids[i]))
            .collect();
        for (to, id) in t.ids.iter().enumerate() {
            match old_slot(id) {
                Some(from) if from != to => t.relink.push((from, to)),
                Some(_) => {}
                None => t.create.push(to),
            }
        }
        // The game's names follow its placements, as the document's follow
        // a removal: a name of a placement that moved slot or is new is
        // repointed, and a name the document cleared is cleared when the
        // transaction deletes the placement it may still name.
        for name in 0..now.names.len() {
            let Some((t_ty, placement)) = now.names.target(name) else {
                continue;
            };
            if t_ty != ty {
                continue;
            }
            let repoint = match usize::try_from(placement) {
                Ok(at) => t.ids.get(at).is_some_and(|id| old_slot(id) != Some(at)),
                Err(_) => !t.delete.is_empty(),
            };
            if repoint {
                t.names.push((name, placement));
            }
        }
        Some(t)
    }

    /// The game applied `t`: its block now holds the document's layout at
    /// the time `t` was made. Object rows and creation snapshots follow the
    /// new numbering; a placement that kept its object keeps the frame the
    /// object has, and a created one starts at its record's frame.
    pub fn placed(&mut self, t: &Transaction) {
        let Some(old) = self.blocks.get_mut(&t.block) else {
            return;
        };
        let mut slots: Vec<Vec<u8>> = std::mem::take(&mut old.entries);
        slots.append(&mut old.tail);
        for r in &t.records {
            if r.index < slots.len() {
                slots[r.index].clone_from(&r.bytes);
            } else {
                slots.resize(r.index, Vec::new());
                slots.push(r.bytes.clone());
            }
        }
        old.tail = slots.split_off(t.count.min(slots.len()));
        old.entries = slots;
        let previous = std::mem::replace(&mut old.ids, t.ids.clone());
        old.layout_rev = t.layout_rev;

        if layout::block(t.block).object_type.is_some() {
            let (mut kept, mut rows): (super::objects::Rows, super::objects::Rows) =
                std::mem::take(&mut self.object_rows)
                    .into_iter()
                    .partition(|(key, _)| key.block == t.block);
            for (index, id) in t.ids.iter().enumerate() {
                let key = super::objects::Key { block: t.block, id: *id };
                let frame = kept.remove(&key).map_or_else(
                    || super::objects::Frame::from_entry(t.block, &t.entries[index]),
                    |row| row.frame,
                );
                rows.insert(key, super::objects::Row { index, frame });
            }
            self.object_rows = rows;
        }

        let slot = |from: usize| {
            let id = previous.get(from)?;
            t.ids.iter().position(|new| new == id)
        };
        let creation = std::mem::take(&mut self.creation.slots);
        self.creation.slots = creation
            .into_iter()
            .filter_map(|((kind, index), bytes)| {
                if kind != t.block {
                    return Some(((kind, index), bytes));
                }
                Some(((kind, slot(index)?), bytes))
            })
            .collect();
        self.creation.revision += 1;
    }
}

/// One record of a [`Transaction`]: the slot in the new numbering, its new
/// bytes and, when known, the bytes the game's slot holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub index: usize,
    pub bytes: Vec<u8>,
    pub was: Option<Vec<u8>>,
}

/// One `world.placement.edit` (docs/runtime/agent-api.md, "Placement edits"):
/// indices in `delete` and the `from` of `relink` are the game's numbering,
/// the rest the document's.
#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    pub block: BlockKind,
    pub was_count: usize,
    pub count: usize,
    pub records: Vec<Record>,
    /// `(object name index, placement)`, -1 for none.
    pub names: Vec<(usize, i16)>,
    pub delete: Vec<usize>,
    /// `(from, to)`.
    pub relink: Vec<(usize, usize)>,
    pub create: Vec<usize>,
    /// The document's layout the transaction brings the game to.
    layout_rev: u64,
    ids: Vec<PlacementId>,
    entries: Vec<Vec<u8>>,
}

impl Transaction {
    /// The document placement the transaction puts in slot `index`.
    pub fn id(&self, index: usize) -> Option<PlacementId> {
        self.ids.get(index).copied()
    }

    /// The new slot of a placement, or `None` when the transaction removes it.
    pub fn slot_of(&self, id: PlacementId) -> Option<usize> {
        self.ids.iter().position(|i| *i == id)
    }

    /// The method's params. An index past the wire's range saturates: the
    /// game refuses a count that large with its reason before reading one.
    pub fn params(&self, epoch: Option<u64>, confirm_peers: Option<u32>) -> PlacementEditParams {
        let n16 = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
        let n32 = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        PlacementEditParams {
            block: crate::api::block_name(self.block),
            was_count: n32(self.was_count),
            count: n32(self.count),
            records: self
                .records
                .iter()
                .map(|r| PlacementRecord {
                    index: n16(r.index),
                    bytes: hex(&r.bytes),
                    was: r.was.as_deref().map(|b| Was::Bytes(hex(b))),
                })
                .collect(),
            names: self
                .names
                .iter()
                .map(|&(name, placement)| PlacementName { name: n16(name), placement })
                .collect(),
            delete: self.delete.iter().map(|&i| n16(i)).collect(),
            relink: self
                .relink
                .iter()
                .map(|&(from, to)| PlacementRelink { from: n16(from), to: n16(to) })
                .collect(),
            create: self.create.iter().map(|&i| n16(i)).collect(),
            epoch,
            confirm_peers,
        }
    }
}

/// The raw `edit.batch` path of a field of element `index` of a block.
pub fn path(kind: BlockKind, index: usize, f: &FieldDef) -> String {
    let b = layout::block(kind);
    format!(
        "+{:#x}/{:#x}[{index}].+{:#x}:bytes[{}]",
        b.offset,
        b.stride,
        f.offset,
        f.ty.size()
    )
}

/// The fields that differ between the baseline and the scenario now.
pub fn diff(base: &Baseline, now: &Scenario) -> Diff {
    let mut out = Diff::default();
    for b in BLOCKS {
        let block = now.block(b.kind);
        let Some(old) = base.blocks.get(&b.kind) else {
            continue;
        };
        if old.layout_rev != block.layout_rev() {
            out.reload.push(b.kind);
            continue;
        }
        for (i, was) in old.entries.iter().enumerate() {
            let Some(cur) = block.entry(i) else { continue };
            if cur == was.as_slice() {
                continue;
            }
            for f in b.fields {
                let r = f.offset as usize..(f.offset + f.ty.size()) as usize;
                let (Some(a), Some(c)) = (was.get(r.clone()), cur.get(r)) else {
                    continue;
                };
                if a != c {
                    out.writes.push(Write {
                        block: b.kind,
                        index: i,
                        field: f.name,
                        path: path(b.kind, i, f),
                        value: hex(c),
                        was: hex(a),
                    });
                }
            }
        }
    }
    out
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::testing::session;
    use scenario_edit::Command;

    #[test]
    fn a_moved_placement_is_one_position_write() {
        let (_t, mut s) = session("sync-move");
        let base = Baseline::of(s.scenario().unwrap());
        assert_eq!(diff(&base, s.scenario().unwrap()), Diff::default());
        let kind = BlockKind::NetgameFlags;
        let pos = layout::block(kind).role(layout::Role::Position).unwrap();
        s.apply(Command::set(
            kind,
            0,
            pos.name,
            scenario_edit::Value::Vec3([1.0, 2.0, 3.0]),
        ))
        .unwrap();
        let d = diff(&base, s.scenario().unwrap());
        assert!(d.reload.is_empty());
        assert_eq!(d.writes.len(), 1, "{d:?}");
        let w = &d.writes[0];
        let b = layout::block(kind);
        assert_eq!(
            w.path,
            format!(
                "+{:#x}/{:#x}[0].+{:#x}:bytes[12]",
                b.offset, b.stride, pos.offset
            )
        );
        let want: Vec<u8> = [1.0f32, 2.0, 3.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(unhex(&w.value).unwrap(), want);
        // Once written, the baseline has it: nothing more to send.
        let mut base = base;
        base.wrote(w);
        assert!(diff(&base, s.scenario().unwrap()).writes.is_empty());
        // Undo sends the old bytes back.
        s.undo();
        let d = diff(&base, s.scenario().unwrap());
        assert_eq!(d.writes.len(), 1);
        assert_eq!(d.writes[0].value, w.was);
    }

    #[test]
    fn respawn_tracks_net_creation_fields_including_writes_after_dispatch() {
        let (_t, session) = session("sync-respawn");
        let scenario = session.scenario().unwrap();
        let kind = BlockKind::Weapons;
        let mut base = Baseline::source(scenario);
        let file = base.creation_fields(kind, 0);
        let src = scenario.block(kind).address;
        let scale = layout::block(kind).role(layout::Role::Scale).unwrap().offset;
        let flags = layout::block(kind).field("placement flags").unwrap().offset;
        let old_scale = &file[scale as usize..scale as usize + 4];
        let old_flags = &file[flags as usize..flags as usize + 4];
        let changed = |bytes: &[u8]| bytes.iter().map(|b| !b).collect::<Vec<_>>();

        base.observe(scenario, src + scale, &changed(old_scale));
        base.observe(scenario, src + flags, &changed(old_flags));
        base.observe(scenario, src + scale, old_scale);
        assert!(base.needs_creation(kind, 0), "the flags still differ");
        let dispatched = base.creation_fields(kind, 0);
        base.observe(scenario, src + flags, old_flags);
        assert!(!base.needs_creation(kind, 0), "both undos reached the original body");

        // The respawn read the edited flags before that last field write arrived.
        base.object_created(kind, 0, &dispatched);
        assert_eq!(base.creation_slots().collect::<Vec<_>>(), [(kind, 0)]);
        base.observe(scenario, src + flags, &changed(old_flags));
        assert!(!base.needs_creation(kind, 0), "the slot matches its respawned body");
    }

    #[test]
    fn an_added_entry_needs_a_reload() {
        let (_t, mut s) = session("sync-add");
        let base = Baseline::of(s.scenario().unwrap());
        s.set_allow_growth(true);
        let kind = BLOCKS
            .iter()
            .map(|b| b.kind)
            .find(|k| s.add_refusal(*k).is_none())
            .expect("the fixture has a block that grows");
        s.add_entry(kind, None).unwrap();
        let d = diff(&base, s.scenario().unwrap());
        assert_eq!(d.reload, vec![kind]);
        assert!(d.writes.iter().all(|w| w.block != kind));
    }

    #[test]
    fn undo_and_redo_compare_the_layout_of_the_loaded_copy() {
        let (_t, mut s) = session("sync-copy-layout");
        let original = Baseline::source(s.scenario().unwrap());
        let kind = BlockKind::Weapons;
        s.remove_entry(kind, 0).unwrap();
        s.add_entry(kind, None).unwrap();

        let copy = Baseline::of(s.scenario().unwrap());
        let from_source = diff(&original, s.scenario().unwrap());
        assert_eq!(from_source.reload, [kind]);
        assert!(from_source.writes.is_empty());
        assert_eq!(diff(&copy, s.scenario().unwrap()), Diff::default());

        assert!(s.undo());
        assert!(s.undo());
        assert_eq!(diff(&original, s.scenario().unwrap()), Diff::default());
        let from_copy = diff(&copy, s.scenario().unwrap());
        assert_eq!(from_copy.reload, [kind]);
        assert!(from_copy.writes.is_empty());

        assert!(s.redo());
        assert!(s.redo());
        assert_eq!(diff(&copy, s.scenario().unwrap()), Diff::default());
        s.apply(Command::set(
            kind,
            0,
            "position",
            scenario_edit::Value::Vec3([11.0, 12.0, 13.0]),
        ))
        .unwrap();
        let d = diff(&copy, s.scenario().unwrap());
        assert!(d.reload.is_empty());
        assert_eq!(d.writes.len(), 1);
        assert_eq!(d.writes[0].block, kind);
        assert_eq!(d.writes[0].index, 0);
    }

    #[test]
    fn observed_bytes_update_partial_placement_fields_across_slots() {
        let (_t, s) = session("sync-observe");
        let scenario = s.scenario().unwrap();
        let mut base = Baseline::source(scenario);
        let kind = BlockKind::Weapons;
        let block = scenario.block(kind);
        let stride = block.def.stride as usize;
        let mut bytes = vec![0xc3; stride + 6];
        bytes[0] = 0xff;
        let spans = base.observe(scenario, block.address - 1, &bytes);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0], 1..bytes.len());
        let entries = &base.blocks[&kind].entries;
        assert_eq!(entries[0], bytes[1..1 + stride]);
        assert_eq!(entries[1][..5], bytes[1 + stride..]);
        assert_eq!(entries[1][5..], block.entry(1).unwrap()[5..]);
    }

    #[test]
    fn a_swap_removal_is_one_transaction_with_delete_relink_and_names() {
        let (_t, mut s) = session("sync-tx-remove");
        let kind = BlockKind::Weapons;
        let mut base = Baseline::source(s.scenario().unwrap());
        assert_eq!(base.transaction(s.scenario().unwrap(), kind, true), None);
        let first = s.scenario().unwrap().block(kind).entry(0).unwrap().to_vec();
        let last = s.scenario().unwrap().block(kind).entry(1).unwrap().to_vec();
        s.remove_entry(kind, 0).unwrap();
        let t = base.transaction(s.scenario().unwrap(), kind, true).unwrap();
        assert_eq!((t.was_count, t.count), (2, 1));
        assert_eq!(t.records, [Record { index: 0, bytes: last.clone(), was: Some(first.clone()) }]);
        assert_eq!(t.delete, [0]);
        assert_eq!(t.relink, [(1, 0)]);
        assert!(t.create.is_empty());
        // The fixture's names: weapon 1, scenery 0, weapon 0.
        assert_eq!(t.names, [(0, 0), (2, -1)]);
        let wire = serde_json::to_value(t.params(Some(1), None)).unwrap();
        assert_eq!(wire["block"], "weapons");
        assert_eq!(wire["relink"], serde_json::json!([{"from": 1, "to": 0}]));
        assert_eq!(wire["records"][0]["was"], hex(&first));

        base.placed(&t);
        assert!(base.layout_matches(s.scenario().unwrap(), kind));
        assert_eq!(diff(&base, s.scenario().unwrap()), Diff::default());
        assert_eq!(base.len(kind), 1);
        let rows: Vec<_> = base.object_rows().iter().filter(|(k, _)| k.block == kind).collect();
        assert_eq!(rows.len(), 1);
        let survivor = super::super::objects::key(s.scenario().unwrap(), kind, 0).unwrap();
        assert_eq!((*rows[0].0, rows[0].1.index), (survivor, 0));

        // Undo is the inverse call: the removed placement comes back to its
        // slot and the survivor moves back, over the bytes the removal left.
        assert!(s.undo());
        let back = base.transaction(s.scenario().unwrap(), kind, true).unwrap();
        assert_eq!((back.was_count, back.count), (1, 2));
        assert_eq!(back.relink, [(0, 1)]);
        assert_eq!(back.create, [0]);
        assert!(back.delete.is_empty());
        assert_eq!(
            back.records,
            [Record { index: 0, bytes: first.clone(), was: Some(last.clone()) }],
            "slot 1 still holds the survivor's bytes past the count"
        );
        assert_eq!(back.names, [(0, 1), (2, 0)]);
        base.placed(&back);
        assert_eq!(diff(&base, s.scenario().unwrap()), Diff::default());
    }

    #[test]
    fn an_add_creates_the_appended_slot_and_walked_blocks_take_records_only() {
        let (_t, mut s) = session("sync-tx-add");
        s.set_allow_growth(true);
        let kind = BlockKind::Weapons;
        let mut base = Baseline::source(s.scenario().unwrap());
        let index = s.add_entry(kind, None).unwrap();
        let t = base.transaction(s.scenario().unwrap(), kind, true).unwrap();
        assert_eq!((t.was_count, t.count), (2, 3));
        assert_eq!(t.create, [index]);
        assert_eq!(t.records.len(), 1);
        assert_eq!(t.records[0].index, index);
        assert_eq!(t.records[0].was, None, "nothing is known past the loaded count");
        base.placed(&t);
        assert!(s.undo());
        let undo = base.transaction(s.scenario().unwrap(), kind, true).unwrap();
        assert_eq!((undo.was_count, undo.count), (3, 2));
        assert_eq!(undo.delete, [index]);
        assert!(undo.records.is_empty() && undo.create.is_empty());
        base.placed(&undo);
        assert!(s.redo());
        let redo = base.transaction(s.scenario().unwrap(), kind, true).unwrap();
        assert_eq!(redo.create, [index]);
        assert!(redo.records.is_empty(), "the slot still holds the record the undo left");
        base.placed(&redo);
        assert_eq!(diff(&base, s.scenario().unwrap()), Diff::default());

        let walked = BlockKind::NetgameFlags;
        let mut base = Baseline::source(s.scenario().unwrap());
        s.add_entry(walked, None).unwrap();
        let t = base.transaction(s.scenario().unwrap(), walked, false).unwrap();
        assert!(t.create.is_empty() && t.delete.is_empty() && t.relink.is_empty());
        assert!(t.names.is_empty());
        assert_eq!(t.count, t.was_count + 1);
        base.placed(&t);
        assert!(base.layout_matches(s.scenario().unwrap(), walked));
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(hex(&[0, 0xab, 0x10]), "00ab10");
        assert_eq!(unhex("00ab10"), Some(vec![0, 0xab, 0x10]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }
}
