//! The HUD kit (docs/game/hud-kit.md): HUD elements of ours, drawn by
//! Halo 2's own HUD code with the disc's bitmaps, shaders and fonts.
//!
//! The model, the templates, the compiler and the HUDs are
//! `hud_kit_core`'s, shared with the game's runtime; this crate draws and
//! checks them offline:
//!
//! - [`model`], [`views`]: `hud_kit_core`'s, re-exported;
//! - [`rects`]: the retail rectangles an element sits beside;
//! - [`hudg`]: the HUD globals the kit reads;
//! - [`templates`]: the templates resolved through the HUD editor's
//!   reader;
//! - [`compile`]: the compiled records, read back as the HUD editor
//!   reads a map's;
//! - [`source`], [`synthetic`]: the disc and the invented map the
//!   preview draws from;
//! - [`preview`]: the 640 by 480 picture, its JSON, the sheet;
//! - [`check`]: the eleven checks.
//!
//! The kit draws nothing itself in the game: the runtime
//! (`game::hud_kit`) hands the compiled records to the game's HUD. No
//! pixels, strings or colour values from the disc are in this crate.

pub mod check;
pub mod compile;
pub mod hudg;
pub mod preview;
pub mod rects;
pub mod source;
pub mod synthetic;
pub mod templates;

pub use compile::{AsHudEdit, Compiled, compile};
pub use hud_edit::Mode;
pub use hud_kit_core::{model, views};
pub use model::{
    Anchor, Art, Describe, Element, Font, Hud, HudRef, Justify, Kind, Origin, PillPart, Place,
    Pulse, Style, Value, Values,
};
pub use source::Source;
pub use templates::Templates;

#[cfg(test)]
mod tests;
