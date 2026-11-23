//! The HUD kit's shared core (docs/game/hud-kit.md, "The runtime, for this
//! HUD"): what both the offline preview (`hud-kit`, a tool) and the game's
//! runtime (`game::hud_kit`) need, so the bytes the preview checks are the
//! bytes the game is given.
//!
//! - [`model`]: a [`Hud`] of elements, a state as [`Values`];
//! - [`templates`]: what a compiled element takes from the loaded map,
//!   the grenade row included, resolved through a [`MapView`];
//! - [`map_file`]: a [`MapView`] over an open map file and its resources;
//! - [`motion`]: how a value moves between ticks, on wall time;
//! - [`world`]: a world mark's shape at a pose, as line segments;
//! - [`records`]: elements to `nhdt` widget record bytes;
//! - [`huds`]: the HUDs the kit knows (superbounce, the Forge readout,
//!   Firefight).
//!
//! Guest-free: no guest memory, no hook. The game extension reads and
//! writes the guest; this crate only computes.

pub mod huds;
pub mod map_file;
pub mod model;
pub mod motion;
pub mod records;
pub mod templates;
pub mod world;

pub use model::{
    Anchor, Art, Describe, Element, Font, Hud, HudRef, Justify, Kind, Origin, PillPart, Place,
    Pulse, Style, Value, Values,
};
pub use motion::Motion;
pub use records::{Compiled, Widget, WidgetKind, compile};
pub use templates::{MapView, Templates};

/// The three screen modes a widget carries a placement for: fullscreen,
/// half and quarter screen (split screen). `hud_edit::layout::Mode` is this
/// type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    #[default]
    Full = 0,
    Half = 1,
    Quarter = 2,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Full, Mode::Half, Mode::Quarter];
    pub fn label(self) -> &'static str {
        match self {
            Mode::Full => "fullscreen",
            Mode::Half => "half screen",
            Mode::Quarter => "quarter screen",
        }
    }
    /// The mode's index in a widget's three placements.
    pub fn index(self) -> usize {
        self as usize
    }
}

#[cfg(test)]
mod tests;
