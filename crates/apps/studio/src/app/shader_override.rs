//! WGSL overrides from the Shader tab (docs/editor/rich-editors.md,
//! "Shaders", slice S5): the text of a pass's linked program edited here,
//! checked as the game would parse it, saved as an override file in the
//! directory the game watches, and the game told to read it again
//! (`render.reload`, docs/graphics/hot-iteration.md). A revert removes the
//! file and reloads.
//!
//! An override changes how our renderer draws. It is never written into a
//! map. The id of the game's program is the pipeline cache's hash of a key
//! with draw-time options, so the editor cannot derive it: the caller names
//! it (`render.programs` lists them).
//!
//! The link thread sends what is queued here (`play::send_shader_overrides`)
//! and gives the answer back ([`Overrides::answered`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use map::TagId;
use scenario_edit::native_view::code::{WgslError, check_override};
use serde_json::Value;

/// A pass's implementation of a shader tag: what an edit is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub tag: TagId,
    pub pass: usize,
    pub implementation: usize,
}

/// What became of a pushed override.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Waiting for the link to send it.
    Queued,
    /// The game was asked; it has not answered.
    Sent,
    /// The game read the file (its reload answer lists the program). Whether
    /// the program accepts it is `render.overrides`' `applied`, at the next frame.
    Loaded,
    /// Why it did not go or the game did not take it.
    Refused(String),
    /// A revert is queued or sent.
    Reverting,
}

impl State {
    pub fn id(&self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Sent => "sent",
            State::Loaded => "loaded",
            State::Refused(_) => "refused",
            State::Reverting => "reverting",
        }
    }
    pub fn why(&self) -> Option<&str> {
        match self {
            State::Refused(w) => Some(w),
            _ => None,
        }
    }
}

/// One override as pushed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pushed {
    pub key: Key,
    pub file: PathBuf,
    pub state: State,
}

/// What the link does for a queued request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    /// Write `text` to `file`.
    Write { text: String },
    /// Remove `file`.
    Remove,
}

/// A request for the link: the file change, then `render.reload {dir}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub program: u64,
    pub dir: PathBuf,
    pub file: PathBuf,
    pub job: Job,
}

/// The editor's drafts and pushes.
#[derive(Debug, Default)]
pub struct Overrides {
    drafts: BTreeMap<Key, String>,
    pushed: BTreeMap<u64, Pushed>,
    outbox: Vec<Outgoing>,
}

/// The file a program's override is saved as: `<16 hex digits>_studio.wgsl`.
pub fn file_name(program: u64) -> String {
    format!("{program:016x}_studio.wgsl")
}

/// A program id from 16 hex digits (an optional `0x` first).
pub fn parse_program(s: &str) -> Option<u64> {
    let h = s.strip_prefix("0x").unwrap_or(s);
    (h.len() == 16)
        .then(|| u64::from_str_radix(h, 16).ok())
        .flatten()
}

/// A program id as the game's API writes it.
pub fn program_text(program: u64) -> String {
    format!("{program:016x}")
}

impl Overrides {
    /// The text being edited for `key`, if it has been edited.
    pub fn draft(&self, key: &Key) -> Option<&str> {
        self.drafts.get(key).map(String::as_str)
    }

    /// Sets the draft; the check of it is [`check`].
    pub fn set_draft(&mut self, key: Key, text: String) {
        self.drafts.insert(key, text);
    }

    /// Drops the draft. A pushed override stays until it is reverted.
    pub fn discard(&mut self, key: &Key) -> bool {
        self.drafts.remove(key).is_some()
    }

    /// The overrides pushed for `key`, by program.
    pub fn pushed_for(&self, key: &Key) -> Vec<(u64, &Pushed)> {
        self.pushed
            .iter()
            .filter(|(_, p)| p.key == *key)
            .map(|(&id, p)| (id, p))
            .collect()
    }

    pub fn pushed(&self, program: u64) -> Option<&Pushed> {
        self.pushed.get(&program)
    }

    /// Queues the draft of `key` as the override of `program` in `dir`. A
    /// draft that does not check, or none, is refused here, so the game is
    /// never given text this side can see is wrong.
    pub fn queue_push(&mut self, key: Key, program: u64, dir: &Path) -> Result<PathBuf, String> {
        let text = self
            .drafts
            .get(&key)
            .ok_or("nothing to push: set the text first (shader.override)")?
            .clone();
        check_override(&text).map_err(|e| describe(&e))?;
        if let Some(other) = self.pushed.get(&program).filter(|p| p.key != key) {
            return Err(format!(
                "program {} already carries the override of pass {} of another edit; revert it first",
                program_text(program),
                other.key.pass
            ));
        }
        let file = dir.join(file_name(program));
        self.pushed.insert(
            program,
            Pushed { key, file: file.clone(), state: State::Queued },
        );
        self.outbox.retain(|o| o.program != program);
        self.outbox.push(Outgoing {
            program,
            dir: dir.to_owned(),
            file: file.clone(),
            job: Job::Write { text },
        });
        Ok(file)
    }

    /// Queues removing the override of `program`.
    pub fn queue_revert(&mut self, program: u64) -> Result<PathBuf, String> {
        let p = self.pushed.get_mut(&program).ok_or_else(|| {
            format!("program {} has no override pushed from here", program_text(program))
        })?;
        p.state = State::Reverting;
        let file = p.file.clone();
        let dir = file.parent().map(Path::to_owned).unwrap_or_default();
        self.outbox.retain(|o| o.program != program);
        self.outbox.push(Outgoing {
            program,
            dir,
            file: file.clone(),
            job: Job::Remove,
        });
        Ok(file)
    }

    /// What the link has yet to send, taken.
    pub fn take_outgoing(&mut self) -> Vec<Outgoing> {
        std::mem::take(&mut self.outbox)
    }

    /// Whether anything waits for the link.
    pub fn waiting(&self) -> bool {
        !self.outbox.is_empty()
    }

    /// The link could not send `out` (not paired, the game lacks the method).
    pub fn refused(&mut self, out: &Outgoing, why: String) {
        if let Some(p) = self.pushed.get_mut(&out.program) {
            p.state = match out.job {
                // A revert that cannot go leaves the override as it was.
                Job::Remove => State::Loaded,
                Job::Write { .. } => State::Refused(why),
            };
        }
    }

    /// `out` was handed to the game.
    pub fn sent(&mut self, out: &Outgoing) {
        if let Some(p) = self.pushed.get_mut(&out.program)
            && out.job != Job::Remove
        {
            p.state = State::Sent;
        }
    }

    /// The game's answer to `render.reload` for `program`: a push is
    /// loaded when the game lists the program (and names no problem with
    /// the file); a revert drops the entry once the game no longer lists it.
    pub fn answered(&mut self, program: u64, result: Result<Value, String>) {
        let Some(p) = self.pushed.get_mut(&program) else {
            return;
        };
        let name = file_name(program);
        let id = program_text(program);
        let report = match result {
            Ok(v) => v,
            Err(why) => {
                p.state = State::Refused(why);
                return;
            }
        };
        let listed = report["loaded"]
            .as_array()
            .is_some_and(|l| l.iter().any(|v| v.as_str() == Some(id.as_str())));
        let problem = report["problems"].as_array().and_then(|l| {
            l.iter().find(|v| {
                v["file"].as_str().is_some_and(|f| f.ends_with(&name))
            })
        });
        if p.state == State::Reverting {
            if listed {
                p.state = State::Refused(
                    "the game still lists the override after the file was removed".into(),
                );
            } else {
                self.pushed.remove(&program);
            }
            return;
        }
        p.state = match (problem, listed) {
            (Some(v), _) => State::Refused(v["error"].as_str().unwrap_or("refused").to_owned()),
            (None, true) => State::Loaded,
            (None, false) => State::Refused(
                "the game did not load the file (is it watching a different directory?)".into(),
            ),
        };
    }
}

/// Checks a draft's text; `Ok` when the game can parse it.
pub fn check(text: &str) -> Result<(), WgslError> {
    check_override(text)
}

/// A refusal as one sentence, with its place.
pub fn describe(e: &WgslError) -> String {
    match (e.line, e.column) {
        (Some(l), Some(c)) => format!("line {l}, column {c}: {}", e.message),
        (Some(l), None) => format!("line {l}: {}", e.message),
        _ => e.message.clone(),
    }
}

/// Carries out an [`Outgoing`]'s file change.
pub fn perform(out: &Outgoing) -> Result<(), String> {
    match &out.job {
        Job::Write { text } => {
            std::fs::create_dir_all(&out.dir).map_err(|e| format!("{}: {e}", out.dir.display()))?;
            std::fs::write(&out.file, text).map_err(|e| format!("{}: {e}", out.file.display()))
        }
        Job::Remove => match std::fs::remove_file(&out.file) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", out.file.display())),
        },
    }
}

/// What the Shader tab's override box asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverrideAction {
    /// The draft's new text.
    Edit(Key, String),
    /// Back to the translation as the draft.
    Discard(Key),
    /// Send the draft as the override of a program.
    Push(Key, u64),
    /// Remove the overrides pushed from the pass.
    Revert(Key),
}

impl crate::App {
    /// Runs an [`OverrideAction`]; a refusal is the app's error line.
    pub(crate) fn run_shader_override(&mut self, a: OverrideAction) {
        let o = &mut self.shader_overrides;
        let r = match a {
            OverrideAction::Edit(k, text) => {
                o.set_draft(k, text);
                Ok(())
            }
            OverrideAction::Discard(k) => {
                o.discard(&k);
                Ok(())
            }
            OverrideAction::Push(k, program) => {
                o.queue_push(k, program, &agent_proto::hot::default_override_dir())
                    .map(|_| ())
            }
            OverrideAction::Revert(k) => {
                let ids: Vec<u64> = o.pushed_for(&k).into_iter().map(|(id, _)| id).collect();
                ids.into_iter().try_for_each(|id| o.queue_revert(id).map(|_| ()))
            }
        };
        if let Err(why) = r {
            self.error(format!("shader override: {why}"));
        }
    }
}

#[cfg(test)]
#[path = "shader_override_tests.rs"]
mod tests;
