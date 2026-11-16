//! The UI-free core of the editor suite, shared by `h2-editor` (egui,
//! `crates/editor`) and `h2-studio` (Dear ImGui, `crates/studio`):
//!
//! - [`source`]: where maps come from (a disc image or loose `.map`s).
//! - [`undo`]: a snapshot undo stack any tool can use.
//! - [`Status`]: a status-line message.
//!
//! Nothing here contains game data: map names and everything else come
//! from the user's own disc at run time.

pub mod source;
pub mod undo;

pub use source::{Disc, MapSource, Resources, default_disc};
pub use undo::UndoStack;

/// A status-line message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Info(String),
    Error(String),
}

impl Status {
    pub fn text(&self) -> &str {
        match self {
            Status::Info(s) | Status::Error(s) => s,
        }
    }
    pub fn is_error(&self) -> bool {
        matches!(self, Status::Error(_))
    }
}
