//! Edits as commands with undo and redo.
//!
//! A command is a list of operations applied together (moving a selection,
//! say): field sets, swap-removals and appends. The history keeps, for each
//! applied command, what every operation replaced, so undo restores exactly
//! those bytes and counts. Consecutive commands with the same merge key (one
//! drag gesture) collapse into one undo step.
use crate::Result;
use crate::layout::BlockKind;
use crate::scenario::{Removed, Scenario, Value};

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
        self.ops.iter().all(|o| matches!(o, Op::Set(_)))
    }
}

/// What an applied operation replaced.
#[derive(Clone, Debug, PartialEq)]
enum Undo {
    Set(SetField),
    Removed(Removed),
    Added { block: BlockKind, entry: Vec<u8> },
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
    undone: Vec<Command>,
}

impl History {
    /// Apply a command. If any operation fails, the ones before it are
    /// rolled back and the scenario is unchanged.
    pub fn apply(&mut self, scenario: &mut Scenario, command: Command) -> Result<()> {
        let before = apply_all(scenario, &command.ops)?;
        self.undone.clear();
        if let (Some(key), Some(last)) = (command.merge, self.done.last_mut())
            && last.command.merge == Some(key)
            && command.only_sets()
            && last.command.only_sets()
        {
            // Keep the oldest "before" for each field; take the new targets.
            for b in before {
                let Undo::Set(b) = b else { continue };
                if !last
                    .before
                    .iter()
                    .any(|x| matches!(x, Undo::Set(x) if same_field(x, &b)))
                {
                    last.before.push(Undo::Set(b));
                }
            }
            for op in command.ops {
                let Op::Set(s) = op else { continue };
                match last.command.ops.iter_mut().find_map(|x| match x {
                    Op::Set(x) if same_field(x, &s) => Some(x),
                    _ => None,
                }) {
                    Some(x) => x.value = s.value,
                    None => last.command.ops.push(Op::Set(s)),
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
        let label = applied.command.label.clone();
        let mut command = applied.command;
        command.merge = None;
        self.undone.push(command);
        Ok(Some(label))
    }

    /// Redo the last undone command. Returns its label.
    pub fn redo(&mut self, scenario: &mut Scenario) -> Result<Option<String>> {
        let Some(command) = self.undone.pop() else {
            return Ok(None);
        };
        match apply_all(scenario, &command.ops) {
            Ok(before) => {
                let label = command.label.clone();
                self.done.push(Applied { command, before });
                Ok(Some(label))
            }
            Err(e) => {
                self.undone.push(command);
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

fn apply_all(scenario: &mut Scenario, ops: &[Op]) -> Result<Vec<Undo>> {
    let mut before = Vec::with_capacity(ops.len());
    for op in ops {
        let r = match op {
            Op::Set(s) => scenario.set(s.block, s.index, s.field, s.value).map(|old| {
                Undo::Set(SetField {
                    value: old,
                    ..s.clone()
                })
            }),
            Op::Remove { block, index } => scenario.remove(*block, *index).map(Undo::Removed),
            Op::Add { block, entry } => scenario.add(*block, entry.clone()).map(|_| Undo::Added {
                block: *block,
                entry: entry.clone(),
            }),
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
            Undo::Added { block, entry } => scenario.unadd(*block, entry)?,
        }
    }
    Ok(())
}
