//! The scenario's recordings, read (docs/editor/rich-editors.md,
//! "Recordings", slice K1): the open map's `recorded_animation` block, each
//! stream decoded as Halo 2's player reads it (`hs::recording`), and which
//! scripts play each one. Read only: nothing encodes a stream yet.
//!
//! The rows are read with the scripts' tables, when the session opens
//! another map (`App::sync_scripts`), so a map with no executable still
//! lists its recordings; only `played_by` needs the scripts.

use hs::recording::{self, Decoded, Game, Recorded};

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

/// Read and decode the block of `map`.
pub fn read<R: std::io::Read + std::io::Seek>(
    map: &mut map::Map<R>,
) -> Result<Vec<Row>, String> {
    recording::read(map)
        .map(|rows| rows.into_iter().map(Row::new).collect())
        .map_err(|e| format!("the recordings do not read: {e}"))
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

    pub(crate) fn read_recordings(&self) -> Result<Vec<Row>, String> {
        let source = self.session.source().ok_or("no map is open")?;
        let mut map = source.open().map_err(|e| e.to_string())?;
        read(&mut map)
    }
}

#[cfg(test)]
#[path = "recordings_tests.rs"]
mod tests;
