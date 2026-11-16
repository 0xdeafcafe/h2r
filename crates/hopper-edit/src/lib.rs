//! The playlist (hopper) editor's logic, with no UI: `multiplayer_hopper.dat`
//! as a working copy of its 16 playlists ([`model`]), the map list read
//! from the user's disc ([`catalog`]), the variant picker's sources
//! ([`choices`]), historical preset playlists ([`presets`]), and the
//! editor state with undo and file handling ([`session::HopperSession`]).
//! h2-editor's playlist tool draws it
//! (docs/live/live-admin.md, docs/editor/editor.md).
//!
//! Nothing here contains game data: map names and variants come from the
//! user's own disc at run time; [`fixture`] is invented test data.

pub mod catalog;
pub mod choices;
pub mod fixture;
pub mod model;
pub mod presets;
pub mod session;

pub use session::{EditView, Error, Format, HopperSession, read_file, write_file};
