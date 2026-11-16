//! Edits as commands with undo and redo.
//!
//! A command is a list of field sets applied together (moving a selection,
//! say). The history keeps, for each applied command, the value every field
//! had before, so undo restores exactly those bytes. Consecutive commands
//! with the same merge key (one drag gesture) collapse into one undo step.
use crate::Result;
use crate::layout::BlockKind;
use crate::scenario::{Scenario, Value};

/// One field change.
#[derive(Clone, Debug, PartialEq)]
pub struct SetField {
    pub block: BlockKind,
    pub index: usize,
    pub field: &'static str,
    pub value: Value,
}

/// A user action: one or more field sets, undone as one step.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub label: String,
    pub sets: Vec<SetField>,
    /// Commands with equal merge keys in a row become one undo step (for
    /// example every frame of one drag). `None` never merges.
    pub merge: Option<u64>,
}
impl Command {
    pub fn set(block: BlockKind, index: usize, field: &'static str, value: Value) -> Self {
        Self {
            label: format!("set {field}"),
            sets: vec![SetField {
                block,
                index,
                field,
                value,
            }],
            merge: None,
        }
    }
    pub fn merging(mut self, key: u64) -> Self {
        self.merge = Some(key);
        self
    }
}

#[derive(Clone, Debug)]
struct Applied {
    command: Command,
    /// Field values before the command, in application order.
    before: Vec<SetField>,
}

/// Undo and redo stacks over a [`Scenario`].
#[derive(Clone, Debug, Default)]
pub struct History {
    done: Vec<Applied>,
    undone: Vec<Command>,
}

impl History {
    /// Apply a command. If any set fails, the ones before it are rolled
    /// back and the scenario is unchanged.
    pub fn apply(&mut self, scenario: &mut Scenario, command: Command) -> Result<()> {
        let before = apply_all(scenario, &command.sets)?;
        self.undone.clear();
        if let (Some(key), Some(last)) = (command.merge, self.done.last_mut())
            && last.command.merge == Some(key)
        {
            // Keep the oldest "before" for each field; take the new targets.
            for b in before {
                if !last
                    .before
                    .iter()
                    .any(|x| x.block == b.block && x.index == b.index && x.field == b.field)
                {
                    last.before.push(b);
                }
            }
            for s in command.sets {
                match last
                    .command
                    .sets
                    .iter_mut()
                    .find(|x| x.block == s.block && x.index == s.index && x.field == s.field)
                {
                    Some(x) => x.value = s.value,
                    None => last.command.sets.push(s),
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
        let mut restore = applied.before.clone();
        restore.reverse();
        if let Err(e) = apply_with(scenario, &restore, false) {
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
        match apply_all(scenario, &command.sets) {
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

fn apply_all(scenario: &mut Scenario, sets: &[SetField]) -> Result<Vec<SetField>> {
    apply_with(scenario, sets, true)
}

fn apply_with(scenario: &mut Scenario, sets: &[SetField], validate: bool) -> Result<Vec<SetField>> {
    let mut before = Vec::with_capacity(sets.len());
    for s in sets {
        let r = if validate {
            scenario.set(s.block, s.index, s.field, s.value)
        } else {
            scenario.write(s.block, s.index, s.field, s.value)
        };
        match r {
            Ok(old) => before.push(SetField {
                value: old,
                ..s.clone()
            }),
            Err(e) => {
                for b in before.iter().rev() {
                    // Restores values just read back, so the types match.
                    let _ = scenario.write(b.block, b.index, b.field, b.value);
                }
                return Err(e);
            }
        }
    }
    Ok(before)
}
