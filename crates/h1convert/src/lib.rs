//! Halo 1 multiplayer maps converted to Halo 2 tags
//! (docs/game/h1-multiplayer.md).
//!
//! - [`collision`]: the structure collision BSP, Halo 1 layout to Halo 2's.
//! - [`query`]: point and segment tests over either game's BSP tree, which
//!   is how a conversion is checked against its source.
//! - [`sandbox`]: Halo 1 multiplayer items to their nearest Halo 2 ones.
//! - [`placements`]: spawns, flags and item spawns for the Halo 2 sandbox.
//! - [`apply`]: a plan written into a base map's scenario.
//! - [`structure`]: the converted structure BSP and lightmap.
//! - [`shaders`]: the base shader each Halo 1 shader draws with.
//! - [`convert`]: a whole map, onto a base, with its package manifest.
//!
//! No game data is in this crate or its tests. Tests build invented BSPs;
//! the `#[ignore]`d ones read maps from `$H1_MAP_DIR` at run time.
#![forbid(unsafe_code)]

pub mod apply;
pub mod collision;
pub mod convert;
pub mod placements;
pub mod query;
pub mod sandbox;
pub mod shaders;
pub mod structure;

/// Why a conversion was refused.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A value doesn't fit the Halo 2 field it goes into.
    #[error("{what} {value:#x} doesn't fit (Halo 2 holds at most {max:#x})")]
    TooBig {
        /// What the value is.
        what: &'static str,
        /// The value.
        value: u32,
        /// The largest the field holds.
        max: u32,
    },
    /// The Halo 1 map couldn't be read.
    #[error(transparent)]
    H1(#[from] h1map::Error),
    /// Something in either map isn't as a conversion needs.
    #[error("{0}")]
    Invalid(&'static str),
    /// The structure has more surfaces than its MOPP code can key.
    #[error("{0} structure surfaces; Havok MOPP code keys at most 65536")]
    TooManySurfaces(usize),
    /// The Halo 1 sandbox (phase 4) isn't built.
    #[error("the Halo 1 sandbox isn't built yet; convert with the Halo 2 sandbox")]
    Halo1Sandbox,
    /// The base map couldn't be read or the map couldn't be built.
    #[error(transparent)]
    Map(#[from] map::Error),
    /// The base scenario couldn't be read or written.
    #[error(transparent)]
    Scenario(#[from] scenario_edit::Error),
    /// The placements couldn't be written.
    #[error(transparent)]
    Apply(#[from] apply::ApplyError),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod structure_tests;
#[cfg(test)]
mod tests;
