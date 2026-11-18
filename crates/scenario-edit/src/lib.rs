//! A Sapien-style scenario placement editor for Halo 2 cache files.
//!
//! - [`layout`]: the `scnr` placement blocks, strides and fields, with
//!   evidence.
//! - [`Scenario`]: those blocks read from a map, typed get/set, and the
//!   minimal fixed-size patches that express the edits.
//! - [`History`]: edits as commands, with undo and redo.
//! - [`save`]: write a new `.map` through `map::writer::MapEditor`: every
//!   byte identical except the patched fields and the header checksum.
//! - [`script`]: JSON edit scripts for headless test maps (`h2 testmap`).
//! - [`package`]: save as a map package (docs/game/maps.md) and open one.
//! - [`Session`]: the editor state and commands with no UI (open, select,
//!   add, remove, drag, gizmo gestures, undo, save, packages), drawn by
//!   `h2-editor` (`crates/studio`).
//! - [`geometry`], [`render`]: the structure BSP and markers, the camera
//!   and picking, and an offscreen wgpu viewport (no UI toolkit).
//! - [`scene`]: the map's art for the viewport: the BSP textured with each
//!   shader's base map, and the placed objects' render models.
//!
//! Entries are removed in place (swap with the last, lower the count) and
//! added in place up to the source size; a block that grows past it is
//! appended to the metadata through `MapEditor::append_metadata`. See
//! docs/editor/scenario-editor.md §6. No game data is in this crate or its tests.
#![forbid(unsafe_code)]

pub mod edit;
/// An invented cache for tests and the editor's screenshots.
#[doc(hidden)]
pub mod fixture;
pub mod geometry;
pub mod layout;
pub mod package;
pub mod render;
pub mod save;
pub mod scenario;
pub mod script;
pub mod scene;
pub mod session;
#[cfg(test)]
mod tests;

pub use edit::{Command, History, SetField};
pub use layout::{BlockKind, FieldType, Role};
pub use package::{PackageForm, SavedPackage};
pub use save::MapSource;
pub use scenario::{Changes, Grown, Removed, Scenario, TagGrown, TagRef, Value};
pub use session::{GROWTH_REFUSAL, Reference, RotationField, Session, Transform};

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
    #[error("refused: {0}")]
    Refused(&'static str),
    #[error("save: {0}")]
    Save(&'static str),
    #[error("package: {0}")]
    Package(String),
    #[error(transparent)]
    Store(#[from] content::store::StoreError),
    #[error(transparent)]
    Resolve(#[from] content::ResolveError),
}
pub type Result<T, E = Error> = std::result::Result<T, E>;
