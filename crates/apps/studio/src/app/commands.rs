//! The command layer's additions to [`Action`]: the edits the inspector,
//! the playlist table and the agent API share, and [`App::perform`], which
//! runs an action for a caller that wants its outcome (the agent API)
//! rather than a log line and a prompt (the window).
//!
//! The window turns clicks into actions and [`App::dispatch`]es them; the
//! API ([`crate::api`]) turns JSON calls into the same actions and
//! [`App::perform`]s them. Both reach [`App::run`], so nothing the window
//! edits is out of the API's reach.
use hopper_edit::model::{self, EditHopper, MAX_WEIGHT};
use live_proto::hopper::defaults;
use live_proto::hopper::{MAX_DESCRIPTION, MAX_RECORDS, MAX_TITLE};
use scenario_edit::layout::{self, Role};
use scenario_edit::{BlockKind, Command, Value};

use super::{Action, App, ToolKind};

/// One playlist edit, applied to the working copy as one undo step. The
/// playlist table's row buttons and weight drags, and `playlist.*` calls,
/// are these.
#[derive(Clone, Debug, PartialEq)]
pub enum PlaylistEdit {
    Title {
        playlist: usize,
        text: String,
    },
    Description {
        playlist: usize,
        text: String,
    },
    /// An entry's weight, 1..=[`MAX_WEIGHT`].
    Weight {
        playlist: usize,
        entry: usize,
        weight: u32,
    },
    /// Move an entry to a new position.
    Move {
        playlist: usize,
        entry: usize,
        to: usize,
    },
    Duplicate {
        playlist: usize,
        entry: usize,
    },
    Remove {
        playlist: usize,
        entry: usize,
    },
    /// Add an entry (a copy of the last one).
    Add {
        playlist: usize,
    },
    /// Make the playlist one of a template's kind
    /// ([`live_proto::hopper::defaults::templates`], by default playlist
    /// id): an unused slot becomes a playlist with one entry; a playlist
    /// keeps its text and entries.
    Kind {
        playlist: usize,
        template: u32,
    },
    /// Make the playlist an unused slot.
    Clear {
        playlist: usize,
    },
}

impl PlaylistEdit {
    pub fn playlist(&self) -> usize {
        match self {
            PlaylistEdit::Title { playlist, .. }
            | PlaylistEdit::Description { playlist, .. }
            | PlaylistEdit::Weight { playlist, .. }
            | PlaylistEdit::Move { playlist, .. }
            | PlaylistEdit::Duplicate { playlist, .. }
            | PlaylistEdit::Remove { playlist, .. }
            | PlaylistEdit::Add { playlist }
            | PlaylistEdit::Kind { playlist, .. }
            | PlaylistEdit::Clear { playlist } => *playlist,
        }
    }

    /// Applies the edit to its playlist, after checking it.
    pub fn apply(&self, hoppers: &mut [EditHopper]) -> Result<(), String> {
        let n = hoppers.len();
        let h = hoppers
            .get_mut(self.playlist())
            .ok_or_else(|| format!("no playlist {} (1..{n})", self.playlist() + 1))?;
        self.apply_to(h)
    }

    /// Applies the edit to `h`, which must be its playlist.
    pub fn apply_to(&self, h: &mut EditHopper) -> Result<(), String> {
        let len = h.records.len();
        let entry_ok = |e: usize| {
            if e < len {
                Ok(())
            } else {
                Err(format!("no entry {} (the playlist has {len})", e + 1))
            }
        };
        let done = match self {
            PlaylistEdit::Title { text, .. } => {
                h.title = model::clip(text, MAX_TITLE);
                true
            }
            PlaylistEdit::Description { text, .. } => {
                h.description = model::clip(text, MAX_DESCRIPTION);
                true
            }
            PlaylistEdit::Weight { entry, weight, .. } => {
                entry_ok(*entry)?;
                if !(1..=MAX_WEIGHT).contains(weight) {
                    return Err(format!("a weight is 1..={MAX_WEIGHT}, not {weight}"));
                }
                h.records[*entry].dwords[0] = *weight;
                true
            }
            PlaylistEdit::Move { entry, to, .. } => {
                entry_ok(*entry)?;
                entry_ok(*to)?;
                model::move_record_to(h, *entry, *to)
            }
            PlaylistEdit::Duplicate { entry, .. } => {
                entry_ok(*entry)?;
                if len >= MAX_RECORDS {
                    return Err(format!("a playlist holds at most {MAX_RECORDS} entries"));
                }
                model::duplicate_record(h, *entry)
            }
            PlaylistEdit::Remove { entry, .. } => {
                entry_ok(*entry)?;
                model::remove_record(h, *entry)
            }
            PlaylistEdit::Add { .. } => {
                if len >= MAX_RECORDS {
                    return Err(format!("a playlist holds at most {MAX_RECORDS} entries"));
                }
                model::add_record(h)
            }
            PlaylistEdit::Kind { template, .. } => {
                if defaults::template(*template).is_none() {
                    return Err(format!("no playlist template {template}"));
                }
                model::create_playlist(h, *template)
            }
            PlaylistEdit::Clear { .. } => {
                model::clear_playlist(h);
                true
            }
        };
        if done || matches!(self, PlaylistEdit::Move { entry, to, .. } if entry == to) {
            Ok(())
        } else {
            Err("the playlist refused the edit".into())
        }
    }
}

/// Why [`App::perform`] did not do what was asked.
#[derive(Clone, Debug, PartialEq)]
pub enum Refused {
    /// The action would drop `tool`'s unsaved edits.
    Unsaved(ToolKind),
    /// It ran and failed; the log's error lines.
    Failed(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Unsaved(t) => write!(
                f,
                "{} has unsaved edits: save them, or pass \"discard\": true",
                t.title()
            ),
            Refused::Failed(e) => f.write_str(e),
        }
    }
}

/// The `&'static` name of a block's field (commands keep field names
/// static), or why there is none.
pub fn field_name(kind: BlockKind, field: &str) -> Result<&'static str, String> {
    layout::block(kind)
        .fields
        .iter()
        .find(|f| f.name == field)
        .map(|f| f.name)
        .ok_or_else(|| {
            let names: Vec<_> = layout::block(kind).fields.iter().map(|f| f.name).collect();
            format!(
                "{kind:?} has no field {field:?} (fields: {})",
                names.join(", ")
            )
        })
}

impl App {
    /// Uses `editor` as the variant tool's (tests and tools with their own
    /// save storage).
    pub fn set_variant_editor(&mut self, editor: variants::VariantEditor) {
        self.variants = Some(editor);
        self.variants_for = Some(self.source.as_ref().map(|s| s.path().to_owned()));
    }

    /// Runs `action` for a caller that wants the outcome: the info lines
    /// it logged, or why it did nothing. An action that would drop unsaved
    /// edits is refused unless `discard`, never held for a prompt.
    pub fn perform(&mut self, action: Action, discard: bool) -> Result<Vec<String>, Refused> {
        if let Some(tool) = action.discards(self) {
            if !discard {
                return Err(Refused::Unsaved(tool));
            }
            self.close_tool(tool);
        }
        let before = self.logged;
        self.run(action);
        let new = (self.logged - before).min(self.log.len() as u64) as usize;
        let lines = &self.log[self.log.len() - new..];
        let errors: Vec<String> = lines
            .iter()
            .filter(|l| l.status.is_error())
            .map(|l| l.status.text().to_owned())
            .collect();
        if !errors.is_empty() {
            return Err(Refused::Failed(errors.join("; ")));
        }
        Ok(lines.iter().map(|l| l.status.text().to_owned()).collect())
    }

    /// [`Action::SetField`].
    pub(super) fn set_field(
        &mut self,
        block: BlockKind,
        index: usize,
        field: &str,
        value: Value,
        merge: Option<u64>,
    ) {
        let field = match field_name(block, field) {
            Ok(f) => f,
            Err(e) => return self.error(e),
        };
        let mut command = Command::set(block, index, field, value);
        if let Some(key) = merge {
            command = command.merging(key);
        }
        if let Err(e) = self.session.apply(command) {
            self.error(e.to_string());
        }
    }

    /// [`Action::MovePlacement`]: the position and, given, the rotation (or
    /// facing) as one undo step.
    pub(super) fn move_placement(
        &mut self,
        block: BlockKind,
        index: usize,
        position: [f32; 3],
        ypr: Option<[f32; 3]>,
    ) {
        let def = layout::block(block);
        let Some(pos) = def.role(Role::Position) else {
            return self.error(format!("{block:?} has no position"));
        };
        let mut ops = vec![Command::set(block, index, pos.name, Value::Vec3(position)).ops];
        if let Some(ypr) = ypr {
            if let Some(r) = def.role(Role::Rotation) {
                ops.push(Command::set(block, index, r.name, Value::Vec3(ypr)).ops);
            } else if let Some(r) = def.role(Role::Facing) {
                ops.push(Command::set(block, index, r.name, Value::Float(ypr[0])).ops);
            } else {
                return self.error(format!("{block:?} has no rotation or facing"));
            }
        }
        let command = Command {
            label: format!("move {block:?} {index}"),
            ops: ops.concat(),
            merge: None,
        };
        match self.session.apply(command) {
            Ok(()) => self.info(format!("moved {block:?} {index}")),
            Err(e) => self.error(e.to_string()),
        }
    }

    /// [`Action::EditPlaylist`].
    pub(super) fn edit_playlist(&mut self, edit: PlaylistEdit) {
        if !self.hoppers.is_open() {
            return self.error("no playlist file is open");
        }
        let r = self.hoppers.edit_with(|hs, _| (edit.apply(hs), None));
        self.hoppers.seal();
        match r {
            Some(Ok(())) => self.info(format!("playlist {}: edited", edit.playlist() + 1)),
            Some(Err(e)) => self.error(e),
            None => self.error("no playlist file is open"),
        }
    }
}
