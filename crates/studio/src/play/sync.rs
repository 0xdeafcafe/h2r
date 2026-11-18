//! Which scenario edits can go to a running game, and as which `edit.set`
//! writes.
//!
//! The game holds the loaded map's scenario (`scnr`) in guest memory, and
//! `edit.set` writes a field there (docs/graphics/hot-iteration.md). The
//! scenario has no tag definition, so a placement's field is reached with
//! raw paths from `scenario_edit::layout`: `+<block>/<stride>[<i>].+<field>:
//! bytes[n]`, the field's exact bytes as hex.
//!
//! [`Baseline`] is what the game is taken to hold (the editor's state when
//! it was launched or paired, then every write sent since). [`diff`] lists
//! the fields that changed since, per element. A block whose length
//! changed (an entry added or removed) can't follow: the game's block is
//! laid out where the map put it, so that needs a reload.

use std::collections::BTreeMap;

use scenario_edit::layout::{self, BLOCKS, FieldDef};
use scenario_edit::{BlockKind, Scenario};

/// The scenario's placement blocks as the game is taken to hold them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Baseline {
    blocks: BTreeMap<BlockKind, Vec<Vec<u8>>>,
}

/// One field write: what `edit.set` gets, and what it is for the log.
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
    /// Blocks whose entry count changed since the baseline: the game needs
    /// the map loaded again (Play again) to see them.
    pub reload: Vec<BlockKind>,
}

impl Baseline {
    /// Everything the scenario's placement blocks hold now.
    pub fn of(s: &Scenario) -> Baseline {
        let blocks = BLOCKS
            .iter()
            .map(|b| {
                let block = s.block(b.kind);
                let entries = (0..block.len())
                    .filter_map(|i| block.entry(i).map(<[u8]>::to_vec))
                    .collect();
                (b.kind, entries)
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
            .and_then(|b| b.get_mut(w.index))
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

    /// Entries the baseline has in `kind`.
    pub fn len(&self, kind: BlockKind) -> usize {
        self.blocks.get(&kind).map_or(0, Vec::len)
    }
}

/// The raw `edit.set` path of a field of element `index` of a block.
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
        if old.len() != block.len() {
            out.reload.push(b.kind);
            continue;
        }
        for (i, was) in old.iter().enumerate() {
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
    if s.len() % 2 != 0 {
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
    fn hex_round_trips() {
        assert_eq!(hex(&[0, 0xab, 0x10]), "00ab10");
        assert_eq!(unhex("00ab10"), Some(vec![0, 0xab, 0x10]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }
}
