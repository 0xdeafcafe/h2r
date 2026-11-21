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

use std::collections::BTreeMap;
use std::ops::Range;

use scenario_edit::layout::{self, BLOCKS, FieldDef};
use scenario_edit::{BlockKind, Scenario};

/// The scenario's placement blocks as the game is taken to hold them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Baseline {
    blocks: BTreeMap<BlockKind, LoadedBlock>,
}

#[derive(Clone, Debug, PartialEq)]
struct LoadedBlock {
    layout_rev: u64,
    entries: Vec<Vec<u8>>,
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
            blocks: BLOCKS
                .iter()
                .map(|b| {
                    let block = LoadedBlock {
                        layout_rev: 0,
                        entries: s.block(b.kind).source_entries().to_vec(),
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
                let entries = (0..block.len())
                    .filter_map(|i| block.entry(i).map(<[u8]>::to_vec))
                    .collect();
                let loaded = LoadedBlock {
                    layout_rev: block.layout_rev(),
                    entries,
                };
                (b.kind, loaded)
            })
            .collect();
        Baseline { blocks }
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
        if let (Some(dst), Some(src)) = (entry.get_mut(at..at + n), unhex(&w.value)) {
            if src.len() == n {
                dst.copy_from_slice(&src);
            }
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
            for entry in &mut entries[((lo - first) / stride) as usize..] {
                let n = src.len().min(entry.len() - offset);
                entry[offset..offset + n].copy_from_slice(&src[..n]);
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
    fn hex_round_trips() {
        assert_eq!(hex(&[0, 0xab, 0x10]), "00ab10");
        assert_eq!(unhex("00ab10"), Some(vec![0, 0xab, 0x10]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }
}
