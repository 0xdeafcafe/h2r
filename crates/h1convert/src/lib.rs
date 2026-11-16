//! Halo 1 multiplayer maps converted to Halo 2 tags
//! (docs/game/h1-multiplayer.md).
//!
//! - [`collision`]: the structure collision BSP, Halo 1 layout to Halo 2's.
//! - [`query`]: point and segment tests over either game's BSP tree, which
//!   is how a conversion is checked against its source.
//! - [`sandbox`]: Halo 1 multiplayer items to their nearest Halo 2 ones.
//!
//! No game data is in this crate or its tests. Tests build invented BSPs;
//! the `#[ignore]`d ones read maps from `$H1_MAP_DIR` at run time.
#![forbid(unsafe_code)]

pub mod collision;
pub mod query;
pub mod sandbox;

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
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
