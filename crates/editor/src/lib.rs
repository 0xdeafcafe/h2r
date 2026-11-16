//! h2-editor: a native editor suite for h2recomp (egui + wgpu).
//!
//! The shell ([`shell::Shell`]) hosts tools behind the small [`Tool`] trait:
//! a sidebar lists them, the central area shows the open one, and the top
//! bar's Open/Save/Save As/Undo/Redo go to it. Tools see the opened disc or
//! loose map, and the wgpu render state for 3D viewports, through
//! [`ToolContext`]. The hopper (playlist) tool ships here; other tools live
//! in their own crates and plug in through adapters (README.md,
//! docs/editor/editor.md).
//!
//! Nothing here contains game data: map names and everything else come
//! from the user's own disc at run time.

pub mod dialog;
pub mod hopper;
pub mod scenario;
#[cfg(test)]
mod screenshots;
pub mod shell;
pub mod tool;
pub mod variants;

/// Map sources and the undo stack live in `editor-core`, shared with
/// `h2-studio`.
pub use editor_core::{source, undo};

pub use source::{Disc, MapSource};
pub use tool::{Handoff, Tool, ToolContext, ToolError};
pub use undo::UndoStack;
