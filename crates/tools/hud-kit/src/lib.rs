//! The HUD kit (docs/game/hud-kit.md): HUD elements of ours, drawn by
//! Halo 2's own HUD code with the disc's bitmaps, shaders and fonts.
//!
//! - [`model`]: a [`Hud`] of elements, a state as [`Values`];
//! - [`rects`]: the retail rectangles an element sits beside;
//! - [`hudg`]: the HUD globals the kit reads;
//! - [`templates`]: what a compiled element takes from the loaded map;
//! - [`compile`]: elements to `nhdt` widget record bytes;
//! - [`source`], [`synthetic`]: the disc and the invented map the
//!   preview draws from;
//! - [`preview`]: the 640 by 480 picture, its JSON, the sheet;
//! - [`check`]: the ten checks;
//! - [`huds`]: the HUDs the kit knows.
//!
//! The kit draws nothing itself in the game: the runtime (slice 3 on)
//! hands the compiled records to the game's HUD. No pixels, strings or
//! colour values from the disc are in this crate.

pub mod check;
pub mod compile;
pub mod hudg;
pub mod huds;
pub mod model;
pub mod preview;
pub mod rects;
pub mod source;
pub mod synthetic;
pub mod templates;

pub use compile::{Compiled, compile};
pub use hud_edit::Mode;
pub use model::{
    Anchor, Art, Describe, Element, Font, Hud, HudRef, Justify, Kind, Place, Pulse, Style, Value,
    Values,
};
pub use source::Source;
pub use templates::Templates;

#[cfg(test)]
mod tests;
