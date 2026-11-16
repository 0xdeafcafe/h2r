//! Halo 2 game variants for the editor suite: the 0x130-byte variant
//! record ([`record`]), the settings schema read from the disc
//! ([`schema`]), saved-variant files and the save store ([`save`]), a
//! portable sharing format ([`portable`]) and the egui panel
//! ([`VariantEditor`]). docs/variants.md has the format and the evidence.

pub mod record;
pub mod schema;
pub mod sha1;

pub use record::{GameType, GameVariant, SettingId};
pub use schema::Schema;
