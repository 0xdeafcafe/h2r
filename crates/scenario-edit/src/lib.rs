//! A Sapien-style scenario placement editor for Halo 2 cache files.
//!
//! - [`layout`]: the `scnr` placement blocks, strides and fields, with
//!   evidence.
//! - [`Scenario`]: those blocks read from a map, typed get/set, and the
//!   minimal fixed-size patches that express the edits.
//! - [`History`]: edits as commands, with undo and redo.
//! - [`save`]: write a new `.map` through `map::writer::MapEditor`: every
//!   byte identical except the patched fields and the header checksum.
//! - [`geometry`], [`render`], [`panel`]: the structure BSP and markers, an
//!   offscreen wgpu viewport, and the egui panel [`ScenarioEditor`].
//!
//! Adding and removing entries needs a writer that can grow a tag; see
//! docs/scenario-editor.md. No game data is in this crate or its tests.
#![forbid(unsafe_code)]

pub mod edit;
pub mod geometry;
pub mod layout;
pub mod panel;
pub mod render;
pub mod save;
pub mod scenario;
#[cfg(test)]
mod tests;

pub use edit::{Command, History, SetField};
pub use layout::{BlockKind, FieldType, Role};
pub use panel::ScenarioEditor;
pub use save::MapSource;
pub use scenario::{Scenario, TagRef, Value};

/// Why an edit, read or save was refused.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Map(#[from] map::Error),
    #[error(transparent)]
    Tags(#[from] tags::Error),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid scenario: {0}")]
    Invalid(&'static str),
    #[error("no field {0:?} in this block")]
    NoField(String),
    #[error("{0:?} has no entry {1}")]
    NoEntry(BlockKind, usize),
    #[error("{0} is read-only")]
    ReadOnly(&'static str),
    #[error("{0}: {1}")]
    Value(&'static str, &'static str),
    #[error("save: {0}")]
    Save(&'static str),
}
pub type Result<T, E = Error> = std::result::Result<T, E>;
