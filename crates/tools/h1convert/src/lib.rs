//! Halo 1 multiplayer maps converted to Halo 2 tags
//! (docs/halo1/multiplayer.md).
//!
//! - [`collision`]: the structure collision BSP, Halo 1 layout to Halo 2's.
//! - [`query`]: point and segment tests over either game's BSP tree, which
//!   is how a conversion is checked against its source.
//! - [`surface_query`]: the engine's vector test with surface lookup over
//!   either game's BSP, to check a conversion surface for surface.
//! - [`sandbox`]: Halo 1 multiplayer items to their nearest Halo 2 ones.
//! - [`placements`]: spawns, flags and item spawns for the Halo 2 sandbox.
//! - [`apply`]: a plan written into a base map's scenario.
//! - [`structure`]: the converted structure BSP and lightmap.
//! - [`clusters`]: Halo 1's clusters, portals and per-cluster lightmaps.
//! - [`shaders`]: the base shader each Halo 1 shader draws with.
//! - [`slots`]: tag-index slots a conversion may reuse for new tags.
//! - [`convert`]: a whole map, onto a base, with its package manifest.
//! - [`fresh`]: the same map with nothing of the base pinned.
//! - [`scenery_collision`]: placed solid scenery merged into the level's
//!   collision.
//!
//! No game data is in this crate or its tests. Tests build invented BSPs;
//! the `#[ignore]`d ones read maps from `$H1_MAP_DIR` at run time.
#![forbid(unsafe_code)]

pub mod ambience;
pub mod apply;
pub mod clusters;
pub mod collision;
pub mod compact;
pub mod convert;
pub mod fixture;
pub mod flood;
pub mod fresh;
pub mod globals;
pub mod lens;
pub mod materials;
pub mod placements;
pub mod query;
pub mod sandbox;
pub mod scenery_collision;
pub mod scratch;
pub mod shad;
pub mod sky;
pub mod sky_shaders;
pub mod slots;
pub mod structure;
pub mod surface_query;
pub mod textures;
pub mod ui;

/// Why a conversion was refused.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{map}: no rebuild for Halo 1 {tag}: {reason}")]
    NoRebuild {
        map: String,
        tag: String,
        reason: String,
    },
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
    /// A collision BSP is past what the Rust collision walk holds
    /// ([`collision::MAX_RING`], [`collision::MAX_DEPTH`]).
    #[error("collision {what} {value} is over the walk's bound of {max}")]
    CollisionBound {
        /// What is over: a surface's edge ring, or the tree's depth.
        what: &'static str,
        /// The measured size.
        value: usize,
        /// The bound.
        max: usize,
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
    /// The Halo 1 sandbox asked of [`convert::prepare`], which has no Halo 1
    /// objects: [`convert::halo1::prepare_halo1`] builds it.
    #[error(
        "the Halo 1 sandbox needs Halo 1's objects (convert::halo1); or convert with the Halo 2 sandbox"
    )]
    Halo1Sandbox,
    /// A fresh map couldn't be assembled.
    #[error("fresh map: {0}")]
    Fresh(String),
    /// The base map couldn't be read or the map couldn't be built.
    #[error(transparent)]
    Map(#[from] map::Error),
    /// The base scenario couldn't be read or written.
    #[error(transparent)]
    Scenario(#[from] scenario_edit::Error),
    /// The placements couldn't be written.
    #[error(transparent)]
    Apply(#[from] apply::ApplyError),
    /// The lightmap bitmap couldn't be converted.
    #[error("lightmap bitmap: {0}")]
    Bitmap(#[from] h1_assets::bitmaps::Error),
    /// The Halo 1 sandbox's objects couldn't be added.
    #[error("Halo 1 objects: {0}")]
    Objects(String),
    /// Halo 1's textures couldn't be planned (a base shader unreadable).
    #[error("textures: {0}")]
    Textures(String),
    #[error("sky: {0}")]
    Sky(String),
    /// The level's background sounds or sound environments couldn't be
    /// added ([`ambience`]).
    #[error("ambience: {0}")]
    Sound(String),
    #[error("ui.map: {0}")]
    Ui(String),
    /// A converted tag couldn't be installed.
    #[error("installing a converted tag: {0}")]
    Install(#[from] h1_assets::models::Error),
}

impl Error {
    pub fn no_rebuild(map: &str, tag: impl Into<String>, reason: impl std::fmt::Display) -> Self {
        Self::NoRebuild {
            map: map.to_owned(),
            tag: tag.into(),
            reason: reason.to_string(),
        }
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod structure_tests;
#[cfg(test)]
mod tests;
