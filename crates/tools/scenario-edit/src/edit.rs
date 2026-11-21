//! Edits as commands with undo and redo.
//!
//! A command is a list of operations applied together (moving a selection,
//! say): field sets, swap-removals and appends, of the placement blocks or
//! of the volume blocks ([`crate::volumes::VolumeKind`]). The history keeps, for each
//! applied command, what every operation replaced, so undo restores exactly
//! those bytes and counts. Consecutive commands with the same merge key (one
//! drag gesture) collapse into one undo step.
use crate::Result;
use crate::layout::BlockKind;
use crate::scenario::{LayoutChange, Removed, Scenario, Value, VolumeRemoved};
use crate::volumes::VolumeKind;

/// One field change.
#[derive(Clone, Debug, PartialEq)]
pub struct SetField {
    pub block: BlockKind,
    pub index: usize,
    pub field: &'static str,
    pub value: Value,
}

/// One operation of a command.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Set(SetField),
    /// Swap-remove entry `index`: the last entry takes its slot.
    Remove {
        block: BlockKind,
        index: usize,
    },
    /// Append one entry (exactly one stride of bytes).
    Add {
        block: BlockKind,
        entry: Vec<u8>,
    },
    /// Replace entry `index` of a volume block with `entry`.
    VolumeSet {
        kind: VolumeKind,
        index: usize,
        entry: Vec<u8>,
    },
    /// Swap-remove entry `index` of a volume block.
    VolumeRemove {
        kind: VolumeKind,
        index: usize,
    },
    /// Append one entry to a volume block.
    VolumeAdd {
        kind: VolumeKind,
        entry: Vec<u8>,
    },
}

/// A user action: one or more operations, undone as one step.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub label: String,
    pub ops: Vec<Op>,
    /// Commands with equal merge keys in a row become one undo step (for
    /// example every frame of one drag). `None` never merges. Only commands
    /// made of field sets merge.
    pub merge: Option<u64>,
}
impl Command {
    pub fn set(block: BlockKind, index: usize, field: &'static str, value: Value) -> Self {
        Self {
            label: format!("set {field}"),
            ops: vec![Op::Set(SetField {
                block,
                index,
                field,
                value,
            })],
            merge: None,
        }
    }
    /// Remove entry `index` of a block (the last entry moves into its slot).
    pub fn remove(block: BlockKind, index: usize) -> Self {
        Self {
            label: format!("remove {block:?} {index}"),
            ops: vec![Op::Remove { block, index }],
            merge: None,
        }
    }
    /// Append an entry to a block; it becomes the block's last entry.
    pub fn add(block: BlockKind, entry: Vec<u8>) -> Self {
        Self {
            label: format!("add {block:?}"),
            ops: vec![Op::Add { block, entry }],
            merge: None,
        }
    }
    pub fn merging(mut self, key: u64) -> Self {
        self.merge = Some(key);
        self
    }
    fn only_sets(&self) -> bool {
        self.ops
            .iter()
            .all(|o| matches!(o, Op::Set(_) | Op::VolumeSet { .. }))
    }
}

/// What an applied operation replaced.
#[derive(Clone, Debug, PartialEq)]
enum Undo {
    Set(SetField),
    Removed(Removed),
    Added {
        block: BlockKind,
        entry: Vec<u8>,
        layout: LayoutChange,
    },
    /// The volume entry's old bytes.
    VolumeSet {
        kind: VolumeKind,
        index: usize,
        entry: Vec<u8>,
    },
    VolumeRemoved(VolumeRemoved),
    VolumeAdded {
        kind: VolumeKind,
        entry: Vec<u8>,
    },
}

#[derive(Clone, Debug)]
struct Applied {
    command: Command,
    /// What each operation replaced, in application order.
    before: Vec<Undo>,
}

/// Undo and redo stacks over a [`Scenario`].
#[derive(Clone, Debug, Default)]
pub struct History {
    done: Vec<Applied>,
    // The revision of each structural operation, in replay order.
    undone: Vec<(Command, Vec<LayoutChange>)>,
}

impl History {
    /// Apply a command. If any operation fails, the ones before it are
    /// rolled back and the scenario is unchanged.
    pub fn apply(&mut self, scenario: &mut Scenario, command: Command) -> Result<()> {
        let before = apply_all(scenario, &command.ops, &[])?;
        self.undone.clear();
        if let (Some(key), Some(last)) = (command.merge, self.done.last_mut())
            && last.command.merge == Some(key)
            && command.only_sets()
            && last.command.only_sets()
        {
            // Keep the oldest "before" for each field; take the new targets.
            for b in before {
                if !last.before.iter().any(|x| same_undo(x, &b)) {
                    last.before.push(b);
                }
            }
            for op in command.ops {
                match last.command.ops.iter_mut().find(|x| same_op(x, &op)) {
                    Some(x) => *x = op,
                    None => last.command.ops.push(op),
                }
            }
            return Ok(());
        }
        self.done.push(Applied { command, before });
        Ok(())
    }

    /// Undo the last command. Returns its label.
    pub fn undo(&mut self, scenario: &mut Scenario) -> Result<Option<String>> {
        let Some(applied) = self.done.pop() else {
            return Ok(None);
        };
        if let Err(e) = revert(scenario, &applied.before) {
            self.done.push(applied);
            return Err(e);
        }
        let revisions = applied
            .before
            .into_iter()
            .filter_map(|u| match u {
                Undo::Removed(r) => Some(r.layout),
                Undo::Added { layout, .. } => Some(layout),
                _ => None,
            })
            .collect();
        let label = applied.command.label.clone();
        let mut command = applied.command;
        command.merge = None;
        self.undone.push((command, revisions));
        Ok(Some(label))
    }

    /// Redo the last undone command. Returns its label.
    pub fn redo(&mut self, scenario: &mut Scenario) -> Result<Option<String>> {
        let Some((command, revisions)) = self.undone.pop() else {
            return Ok(None);
        };
        match apply_all(scenario, &command.ops, &revisions) {
            Ok(before) => {
                let label = command.label.clone();
                self.done.push(Applied { command, before });
                Ok(Some(label))
            }
            Err(e) => {
                self.undone.push((command, revisions));
                Err(e)
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }
    /// Stop the last command from merging with the next one (end of a drag).
    pub fn seal(&mut self) {
        if let Some(last) = self.done.last_mut() {
            last.command.merge = None;
        }
    }
}

fn same_field(a: &SetField, b: &SetField) -> bool {
    a.block == b.block && a.index == b.index && a.field == b.field
}

/// Whether two set operations write the same field or volume entry.
fn same_op(a: &Op, b: &Op) -> bool {
    match (a, b) {
        (Op::Set(a), Op::Set(b)) => same_field(a, b),
        (
            Op::VolumeSet { kind, index, .. },
            Op::VolumeSet {
                kind: k, index: i, ..
            },
        ) => kind == k && index == i,
        _ => false,
    }
}

fn same_undo(a: &Undo, b: &Undo) -> bool {
    match (a, b) {
        (Undo::Set(a), Undo::Set(b)) => same_field(a, b),
        (
            Undo::VolumeSet { kind, index, .. },
            Undo::VolumeSet {
                kind: k, index: i, ..
            },
        ) => kind == k && index == i,
        _ => false,
    }
}

fn apply_all(
    scenario: &mut Scenario,
    ops: &[Op],
    revisions: &[LayoutChange],
) -> Result<Vec<Undo>> {
    let mut replay = revisions.iter().copied();
    let mut before = Vec::with_capacity(ops.len());
    for op in ops {
        let r = match op {
            Op::Set(s) => scenario.set(s.block, s.index, s.field, s.value).map(|old| {
                Undo::Set(SetField {
                    value: old,
                    ..s.clone()
                })
            }),
            Op::Remove { block, index } => scenario
                .remove_with_rev(*block, *index, replay.next())
                .map(Undo::Removed),
            Op::Add { block, entry } => scenario
                .add_with_rev(*block, entry.clone(), replay.next())
                .map(|(_, layout)| Undo::Added {
                    block: *block,
                    entry: entry.clone(),
                    layout,
                }),
            Op::VolumeSet { kind, index, entry } => scenario
                .set_volume(*kind, *index, entry.clone())
                .map(|old| Undo::VolumeSet {
                    kind: *kind,
                    index: *index,
                    entry: old,
                }),
            Op::VolumeRemove { kind, index } => scenario
                .remove_volume(*kind, *index)
                .map(Undo::VolumeRemoved),
            Op::VolumeAdd { kind, entry } => {
                scenario
                    .add_volume(*kind, entry.clone())
                    .map(|_| Undo::VolumeAdded {
                        kind: *kind,
                        entry: entry.clone(),
                    })
            }
        };
        match r {
            Ok(u) => before.push(u),
            Err(e) => {
                // Restores what was just replaced, so it cannot fail.
                let _ = revert(scenario, &before);
                return Err(e);
            }
        }
    }
    Ok(before)
}

/// Undo operations newest first. Field values are written without
/// validation, so a damaged source value comes back exactly.
fn revert(scenario: &mut Scenario, before: &[Undo]) -> Result<()> {
    for u in before.iter().rev() {
        match u {
            Undo::Set(s) => {
                scenario.write(s.block, s.index, s.field, s.value)?;
            }
            Undo::Removed(r) => scenario.unremove(r)?,
            Undo::Added { block, entry, .. } => scenario.unadd(*block, entry)?,
            Undo::VolumeSet { kind, index, entry } => {
                scenario.write_volume(*kind, *index, entry.clone())?;
            }
            Undo::VolumeRemoved(r) => scenario.unremove_volume(r)?,
            Undo::VolumeAdded { kind, entry } => scenario.unadd_volume(*kind, entry)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod layout_revision_tests {
    use super::*;
    use crate::fixture::fixture;
    use map::Map;
    use std::io::Cursor;

    fn scenario() -> Scenario {
        let mut map = Map::from_reader(Cursor::new(fixture())).unwrap();
        Scenario::read(&mut map).unwrap()
    }

    #[test]
    fn direct_structural_inverses_restore_only_their_own_layout() {
        let mut s = scenario();
        let block = BlockKind::Weapons;
        let entry = s.block(block).entry(0).unwrap().to_vec();
        s.add(block, entry.clone()).unwrap();
        let appended = s.block(block).layout_rev();
        let removed = s.remove(block, 0).unwrap();
        assert_ne!(s.block(block).layout_rev(), appended);
        s.unremove(&removed).unwrap();
        assert_eq!(s.block(block).layout_rev(), appended);
        s.set(block, 0, "scale", Value::Float(2.0)).unwrap();
        assert_eq!(s.block(block).layout_rev(), appended);
        s.unadd(block, &entry).unwrap();
        assert_eq!(s.block(block).layout_rev(), 0);

        let removed = s.remove(block, 0).unwrap();
        s.add(block, entry).unwrap();
        let intervening = s.block(block).layout_rev();
        s.unremove(&removed).unwrap();
        let inserted = s.block(block).layout_rev();
        assert_ne!(inserted, 0);
        assert_ne!(inserted, intervening);
        let entries = s.block(block);
        let last = entries.entry(entries.len() - 1).unwrap().to_vec();
        s.unadd(block, &last).unwrap();
        assert!(![0, intervening, inserted].contains(&s.block(block).layout_rev()));

        let mut source = scenario();
        let last = source.block(block).entry(1).unwrap().to_vec();
        source.unadd(block, &last).unwrap();
        assert_ne!(source.block(block).layout_rev(), 0);
    }

    #[test]
    fn structural_redo_reuses_a_revision_only_on_its_parent_layout() {
        let mut s = scenario();
        let mut h = History::default();
        let block = BlockKind::Weapons;
        let entry = s.block(block).entry(1).unwrap().to_vec();
        h.apply(&mut s, Command::remove(block, 0)).unwrap();
        let recorded = s.block(block).layout_rev();
        assert_ne!(recorded, 0);
        assert_eq!(s.block(BlockKind::Scenery).layout_rev(), 0);
        h.undo(&mut s).unwrap();
        assert_eq!(s.block(block).layout_rev(), 0);
        h.redo(&mut s).unwrap();
        assert_eq!(s.block(block).layout_rev(), recorded);

        h.undo(&mut s).unwrap();
        s.remove(block, 1).unwrap();
        s.add(block, entry).unwrap();
        let parent = s.block(block).layout_rev();
        assert_eq!(s.block(block).len(), 2);
        h.redo(&mut s).unwrap();
        let diverged = s.block(block).layout_rev();
        assert_ne!(diverged, recorded);
        assert_ne!(diverged, parent);
        h.undo(&mut s).unwrap();
        assert_eq!(s.block(block).layout_rev(), parent);
        h.apply(&mut s, Command::remove(block, 0)).unwrap();
        assert_ne!(s.block(block).layout_rev(), diverged);
        assert!(!h.can_redo());
    }

    #[test]
    fn failed_structural_commands_restore_slots_and_the_redo_branch() {
        let mut s = scenario();
        let mut h = History::default();
        let block = BlockKind::Weapons;
        let flag = BlockKind::NetgameFlags;
        let entry = s.block(block).entry(1).unwrap().to_vec();
        let before = s.changes();
        let replace = Command {
            label: "replace weapon and remove flag".into(),
            ops: vec![
                Op::Remove { block, index: 0 },
                Op::Add { block, entry },
                Op::Remove {
                    block: flag,
                    index: 0,
                },
            ],
            merge: None,
        };
        h.apply(&mut s, replace.clone()).unwrap();
        let recorded = [block, flag].map(|kind| s.block(kind).layout_rev());
        h.undo(&mut s).unwrap();

        let mut failed = replace;
        failed.ops.push(Op::Remove { block, index: 99 });
        assert!(h.apply(&mut s, failed).is_err());
        assert_eq!(s.changes(), before);
        assert_eq!(s.block(block).layout_rev(), 0);
        assert_eq!(s.block(flag).layout_rev(), 0);
        h.redo(&mut s).unwrap();
        assert_eq!(
            [block, flag].map(|kind| s.block(kind).layout_rev()),
            recorded
        );
    }

    #[test]
    fn reopening_a_saved_layout_makes_its_slot_order_the_source() {
        let mut source = Map::from_reader(Cursor::new(fixture())).unwrap();
        let mut s = Scenario::read(&mut source).unwrap();
        let block = BlockKind::Weapons;
        let replacement = s.new_entry(block, None, None).unwrap();
        let moved = s.block(block).entry(1).unwrap().to_vec();
        s.remove(block, 0).unwrap();
        s.add(block, replacement.clone()).unwrap();
        assert_ne!(s.block(block).layout_rev(), 0);

        let mut bytes = Cursor::new(Vec::new());
        crate::save::write_patched(&mut source, &s, &mut bytes).unwrap();
        bytes.set_position(0);
        let mut map = Map::from_reader(bytes).unwrap();
        let reopened = Scenario::read(&mut map).unwrap();
        assert_eq!(reopened.block(block).layout_rev(), 0);
        assert_eq!(
            reopened.block(block).source_entries(),
            &[moved, replacement]
        );
    }
}
