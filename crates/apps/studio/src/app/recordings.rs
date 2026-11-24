//! The scenario's recordings (docs/editor/rich-editors.md, "Recordings",
//! slices K1 and K3): the open map's `recorded_animation` block, each
//! stream decoded as Halo 2's player reads it (`hs::recording`), and which
//! scripts play each one; and the edits to them.
//!
//! The rows are read with the scripts' tables, when the session opens
//! another map (`App::sync_scripts`), so a map with no executable still
//! lists its recordings; only `played_by` needs the scripts.
//!
//! An edit replaces a stream (`hs::recording_edit::Stream`) or removes a
//! recording. The block's entries as edited are diffed against the source
//! block (`hs::recording_edit::changes`) and handed to the scenario
//! session ([`scenario_edit::Session::set_recording_changes`]), which
//! writes them with the map and counts them as unsaved. The undo stack is
//! the [`Doc`]'s: a snapshot of the entries before each edit.

use hs::recording::{self, Block, Decoded, Game, Recorded};
use hs::recording_edit::{self, Entry, Stream};

/// The tool's sentence for a map whose block is empty.
pub const NONE: &str = "This map has no recordings.";

/// The one stream version the player reads (the converter writes it, and
/// both games' decoders select it for versions 1 to 4's table entry).
pub const STREAM_VERSION: u8 = 4;

/// What a script names a recording by: its name as a symbol, or its index
/// in the block as a number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Name(String),
    Index(i64),
}

/// One recording and what its stream holds.
#[derive(Clone, Debug)]
pub struct Row {
    pub recorded: Recorded,
    /// The decoded stream, or why it does not read (with the offset).
    pub decoded: Result<Decoded, String>,
}

impl Row {
    pub fn new(recorded: Recorded) -> Self {
        let decoded = if recorded.version != STREAM_VERSION {
            Err(format!(
                "stream version {} is not read; only version {STREAM_VERSION} is",
                recorded.version
            ))
        } else {
            recording::decode_stream(Game::Halo2, recorded.control_version, &recorded.stream)
                .map_err(|e| e.to_string())
        };
        Self { recorded, decoded }
    }

    /// Whether a script's `key` names this recording, which is at `index`
    /// in the block. Names match without case, as the converter's do.
    pub fn named_by(&self, index: usize, key: &Key) -> bool {
        match key {
            Key::Name(n) => self.recorded.name.eq_ignore_ascii_case(n),
            Key::Index(i) => *i == index as i64,
        }
    }
}

/// The block as the source holds it and as it is now: what the edits
/// work on.
#[derive(Clone, Debug)]
pub struct Doc {
    pub source: Block,
    pub entries: Vec<Entry>,
    undo: Vec<Vec<Entry>>,
    redo: Vec<Vec<Entry>>,
}

impl Doc {
    pub fn new(source: Block) -> Self {
        let entries = recording_edit::entries(&source);
        Self {
            source,
            entries,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// The rows the entries read as.
    pub fn rows(&self) -> Vec<Row> {
        self.entries
            .iter()
            .map(|e| Row::new(e.recorded()))
            .collect()
    }
}

/// Read and decode the block of `map`.
pub fn read<R: std::io::Read + std::io::Seek>(
    map: &mut map::Map<R>,
) -> Result<(Vec<Row>, Doc), String> {
    let block = recording::read_block(map)
        .map_err(|e| format!("the recordings do not read: {e}"))?;
    let doc = Doc::new(block);
    Ok((doc.rows(), doc))
}

/// Why an edit did not happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The request is wrong: an event that is not there, a value that
    /// does not fit.
    Params(String),
    /// The request is fine and the editor will not: no map, a stream that
    /// does not read, growth off, a recording a script plays.
    Refused(String),
}

impl Refusal {
    fn refused(why: impl Into<String>) -> Self {
        Refusal::Refused(why.into())
    }
}

/// One edit to a stream's events and initial state, applied in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Move event `index` (the ones after it keep their ticks) and/or
    /// replace its payload.
    Set {
        index: usize,
        delay: Option<u16>,
        payload: Option<Vec<u8>>,
    },
    /// An event of `kind` before event `index`.
    Insert {
        index: usize,
        kind: u8,
        delay: u16,
        payload: Vec<u8>,
    },
    Remove {
        index: usize,
    },
    Initial {
        name: String,
        value: recording::Value,
    },
    Trailer(Vec<u8>),
}

impl Op {
    fn apply(&self, s: &mut Stream) -> Result<(), String> {
        match self {
            Op::Set {
                index,
                delay,
                payload,
            } => {
                if let Some(p) = payload {
                    s.set_payload(*index, p)?;
                }
                if let Some(d) = delay {
                    s.set_delay(*index, *d)?;
                }
                Ok(())
            }
            Op::Insert {
                index,
                kind,
                delay,
                payload,
            } => s.insert(*index, *kind, *delay, payload),
            Op::Remove { index } => s.remove(*index),
            Op::Initial { name, value } => s.set_initial(name, value),
            Op::Trailer(b) => s.set_trailer(b),
        }
    }
}

/// Hand the session what `doc` changes.
fn push(session: &mut scenario_edit::Session, doc: &Doc) -> Result<(), String> {
    let c = recording_edit::changes(&doc.source, &doc.entries)?;
    let grown = c
        .grown
        .into_iter()
        .map(|g| scenario_edit::TagGrown {
            root: g.root,
            count: g.bytes.len() as u32,
            bytes: g.bytes,
        })
        .collect();
    session
        .set_recording_changes(c.patches, grown)
        .map_err(|e| e.to_string())
}

impl super::App {
    /// The open map's recordings, or why there are none to show.
    pub fn recordings_loaded(&self) -> Result<&[Row], String> {
        match &self.scripts.recordings {
            None => Err("open a map to see its recordings".to_owned()),
            Some(Err(e)) => Err(e.clone()),
            Some(Ok(rows)) => Ok(rows),
        }
    }

    pub(crate) fn read_recordings(&self) -> Result<(Vec<Row>, Doc), String> {
        let source = self.session.source().ok_or("no map is open")?;
        let mut map = source.open().map_err(|e| e.to_string())?;
        read(&mut map)
    }
}

impl super::App {
    fn recording_doc(&mut self) -> Result<&mut Doc, Refusal> {
        self.sync_scripts();
        self.scripts
            .recording_doc
            .as_mut()
            .ok_or_else(|| Refusal::refused("open a map to edit its recordings"))
    }

    /// Replace the entries with `new` and hand the session the diff; on a
    /// refusal nothing changes. `record` pushes the old entries on the undo
    /// stack.
    fn recording_apply(&mut self, new: Vec<Entry>, record: bool) -> Result<(), Refusal> {
        self.sync_scripts();
        let doc = self
            .scripts
            .recording_doc
            .as_mut()
            .ok_or_else(|| Refusal::refused("open a map to edit its recordings"))?;
        let before = std::mem::replace(&mut doc.entries, new);
        if let Err(why) = push(&mut self.session, doc) {
            doc.entries = before;
            return Err(Refusal::refused(why));
        }
        if record {
            doc.undo.push(before);
            doc.redo.clear();
        }
        self.scripts.recordings = Some(Ok(doc.rows()));
        Ok(())
    }

    /// Apply `ops` to the stream of recording `index`, as one undo step. A
    /// stream that does not read, or is not version 4, is refused by name;
    /// a stream that grows needs Edit > Allow growing full blocks. The
    /// recording's tick count becomes the stream's.
    pub fn recording_write(&mut self, index: usize, ops: &[Op]) -> Result<(), Refusal> {
        let doc = self.recording_doc()?;
        let mut entries = doc.entries.clone();
        let e = entries
            .get_mut(index)
            .ok_or_else(|| Refusal::Params(format!("no recording {index}")))?;
        let name = e.name();
        if e.element[0x20] != STREAM_VERSION {
            return Err(Refusal::refused(format!(
                "recording {name}: stream version {} is not read; only version {STREAM_VERSION} is",
                e.element[0x20]
            )));
        }
        let mut s = Stream::parse(Game::Halo2, e.element[0x22], &e.stream)
            .map_err(|why| Refusal::refused(format!("recording {name}: {why}")))?;
        for (i, op) in ops.iter().enumerate() {
            op.apply(&mut s)
                .map_err(|why| Refusal::Params(format!("edit {i}: {why}")))?;
        }
        s.check().map_err(Refusal::refused)?;
        e.ticks = i16::try_from(s.ticks()).map_err(|_| {
            Refusal::refused(format!(
                "recording {name}: {} ticks is past the {} the element holds",
                s.ticks(),
                i16::MAX
            ))
        })?;
        e.stream = s.encode();
        self.recording_apply(entries, true)
    }

    /// Remove recording `index`: the ones after it move down. Refused when
    /// a script plays it, or plays a later one by its number (which would
    /// then name another), or when who plays them cannot be known (the
    /// scripts do not read).
    pub fn recording_remove(&mut self, index: usize) -> Result<(), Refusal> {
        let doc = self.recording_doc()?;
        let mut entries = doc.entries.clone();
        let rows = doc.rows();
        if index >= rows.len() {
            return Err(Refusal::Params(format!("no recording {index}")));
        }
        let name = rows[index].recorded.name.clone();
        let loaded = self.scripts_loaded().map_err(|why| {
            Refusal::refused(format!(
                "recording {name}: who plays it cannot be known without the scripts ({why})"
            ))
        })?;
        if let Some((_, script, _)) = loaded.recording_uses(&rows, index).first() {
            return Err(Refusal::refused(format!(
                "recording {name} is played by {script}: change that first"
            )));
        }
        for later in index + 1..rows.len() {
            let key = Key::Index(later as i64);
            if let Some(e) = loaded.entries.iter().find(|e| e.recordings.contains(&key)) {
                return Err(Refusal::refused(format!(
                    "{} plays recording {later} ({}) by its number, which removing {name} \
                     would change",
                    e.source.name, rows[later].recorded.name
                )));
            }
        }
        entries.remove(index);
        self.recording_apply(entries, true)
    }

    /// Undo (or, with `redo`, redo) the last recording edit; `false` when
    /// there is none.
    pub fn recording_step(&mut self, redo: bool) -> Result<bool, Refusal> {
        let doc = self.recording_doc()?;
        let Some(target) = (if redo { doc.redo.pop() } else { doc.undo.pop() }) else {
            return Ok(false);
        };
        let now = doc.entries.clone();
        if let Err(e) = self.recording_apply(target.clone(), false) {
            let doc = self.recording_doc()?;
            let stack = if redo { &mut doc.redo } else { &mut doc.undo };
            stack.push(target);
            return Err(e);
        }
        let doc = self.recording_doc()?;
        let stack = if redo { &mut doc.undo } else { &mut doc.redo };
        stack.push(now);
        Ok(true)
    }
}

#[cfg(test)]
#[path = "recordings_tests.rs"]
mod tests;
