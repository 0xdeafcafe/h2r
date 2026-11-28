//! File > New map (docs/editor/editor.md, "New maps"): a derived map
//! package (`[base] derived = true`) that boots and plays at once, built
//! from a template of ours ([`SOURCE`], `content/new-map/courtyard.yaml`)
//! over a disc map's tags.
//!
//! [`build::build`] in order:
//!
//! 1. **The floor.** The template's `ground` is a one-bay stage layout:
//!    the content stage's fresh build (`content_kit::stage::map::build_over`) makes
//!    the structure BSP (floor render mesh, collision from the collision
//!    compiler with its MOPP, one cluster), the lightmap from the bay's
//!    sun and sky fill, and the sky, over the base's globals and shaders.
//!    A campaign map first empties the base's AI, script and object-name
//!    blocks the template lists, and gets one squad, one zone of firing
//!    positions and an order ([`ai`]).
//! 2. **The pieces.** The walls, the ramp and the cover are block-out
//!    pieces: each definition installed by the block-out build
//!    (`forge_build::blockout::install`: `bloc`, render model, `coll` from
//!    the collision compiler, a fixed `phmo`) with a crate palette row,
//!    so the map is edited later with the same pieces.
//! 3. **The placements.** A scenario-edit recipe places the crates, the
//!    spawns, the netgame flags and the equipment through a `Session`, so
//!    each is checked as the editor checks it. The scenario read back
//!    must cover every mode the template claims ([`coverage`]).
//! 4. **The package.** [`build::write`] saves it as the editor saves a new
//!    map: the built map as the first layer, the placements as the second.
//!
//! The base's tags come through `tags::repo::TagSource`: a disc map today
//! ([`build::build`] over `MapSource`), the extracted tag tree with
//! [`build::build_over`] and `content_kit::stage::map::Base::from_repo`. A
//! tag-workspace project plugs in at the same place: it is a `TagSource`
//! and a `Base`, and adds its own tags to the closure through
//! `content_kit::stage::map::Over::roots`.
#![forbid(unsafe_code)]

pub mod ai;
pub mod build;
pub mod coverage;
pub mod template;

pub use build::{Built, Request, Written, build, write};
pub use coverage::Census;
pub use template::{FlagKind, Kind, Mode, Template};

/// The starter map's build version: part of every build key.
pub const VERSION: u32 = 1;

/// The template shipped with the repository.
pub const SOURCE: &str = include_str!("../../../../content/new-map/courtyard.yaml");

/// What can go wrong making a new map.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The template is not one this build makes.
    #[error("new map template: {0}")]
    Template(String),
    /// The name asked for is not a map name.
    #[error("new map name: {0}")]
    Name(String),
    /// The floor, lightmap, sky or fresh build failed.
    #[error(transparent)]
    Stage(#[from] content_kit::stage::Error),
    /// The base map lacks what the template names, or the disc lacks the
    /// base.
    #[error("new map base: {0}")]
    Base(String),
    /// The block-out pieces did not install.
    #[error("new map pieces: {0}")]
    Pieces(String),
    /// A placement was refused.
    #[error("new map placements: {0}")]
    Place(String),
    /// The built scenario does not cover a mode the template claims.
    #[error("new map modes: {0}")]
    Coverage(String),
    /// Writing the package failed.
    #[error("new map package: {0}")]
    Package(String),
}

/// Result for new maps.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
