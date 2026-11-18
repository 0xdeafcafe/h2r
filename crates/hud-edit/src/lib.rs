//! The HUD editor's logic, with no UI toolkit (docs/editor/hud-editor.md):
//!
//! - [`layout`]: the `nhdt` widget bytes the engine reads, and the field
//!   table the property panel is drawn from;
//! - [`place`]: where the engine puts a widget, each rule with its proof;
//! - [`state`]: the game state a HUD is previewed at, the widgets' inputs
//!   and the engine's state gate;
//! - [`model`]: a map's HUDs and who names them, one HUD's widgets;
//! - [`render`]: the 640 by 480 preview from the map's bitmaps and the
//!   disc's fonts;
//! - [`checks`]: overlaps, art off the screen, text spilling out of its art;
//! - [`session`]: edits, undo and redo, the patched-map save.
//!
//! Nothing here holds game data: maps come from the user's own disc.

pub mod checks;
#[cfg(feature = "fixture")]
pub mod fixture;
pub mod layout;
pub mod model;
pub mod place;
pub mod render;
pub mod session;
pub mod state;

pub use layout::Mode;
pub use model::{Hud, HudEntry, Value, Widget, WidgetKey, WidgetKind};
pub use session::HudSession;
pub use state::HudState;

#[cfg(test)]
mod tests;
