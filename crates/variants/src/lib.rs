//! Halo 2 game variants for the editor suite: the 0x130-byte variant
//! record ([`record`]), the settings schema read from the disc
//! ([`schema`]), saved-variant files and the save store ([`save`]), a
//! portable sharing format ([`portable`]) and the editor
//! ([`VariantEditor`]: UI-free state and commands, drawn by `h2-editor`). docs/editor/variants.md has the format and the evidence.

pub mod panel;
pub mod portable;
pub mod record;
pub mod save;
pub mod schema;
pub mod sha1;

pub use panel::VariantEditor;
pub use record::{GameType, GameVariant, SettingId};
pub use schema::Schema;
