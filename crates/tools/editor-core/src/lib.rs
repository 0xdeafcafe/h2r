//! The UI-free core of `h2-editor` (`crates/apps/studio`), shared by its
//! tools:
//!
//! - [`source`]: where maps come from (a disc image or loose `.map`s).
//! - [`undo`]: a snapshot undo stack any tool can use.
//! - [`variant_sources`]: the game-variant schema, save names and save
//!   storage for a source, for the variant and playlist tools.
//! - [`packages`]: the map packages under a data root, Halo 1
//!   conversions among them.
//! - [`halo1`]: Halo 1 maps, read-only (a disc or a folder).
//! - [`halo1_art`]: a Halo 1 map's base maps and lightmap pages.
//! - [`Status`]: a status-line message.
//!
//! Nothing here contains game data: map names and everything else come
//! from the user's own disc at run time.

pub mod halo1;
pub mod halo1_art;
pub mod mappack;
pub mod packages;
pub mod source;
pub mod undo;
pub mod variant_sources;

pub use mappack::MapPack;
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
