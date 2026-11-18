//! Halo 1 skies: read from Xbox cache files, converted to Halo 2 `sky `
//! tags for the map builder.
//!
//! - [`h1`] reads a Halo 1 `sky ` tag (model, animation graph, ambient
//!   lighting scalars, fog reference, shader functions, sky lights) through
//!   [`h1map`].
//! - [`h2`] decodes a Halo 2 `sky ` tag's payload back into values: the
//!   read-back check for conversions, and a reader for retail maps.
//! - [`atmosphere`] writes and reads Halo 2's per-cluster atmosphere
//!   element (scenario `+0x340`), the home of Halo 1's indoor fog.
//! - [`convert`] builds a Halo 2 `sky ` as a [`h1models::Converted`] block
//!   tree. The sky's render model comes from [`h1models::model_from_map`].
//!
//! Layouts, offsets and their evidence are in
//! `docs/formats/h1-sky.md`. Field names here are ours: the disc evidence
//! fixes offsets, sizes and value ranges, not official names. No game data
//! is in this crate or its tests; real maps are read only by the `#[ignore]`
//! tests, from the owner's discs (`$H1_ISO`, `$H2_ISO`).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod atmosphere;
pub mod convert;
pub mod h1;
pub mod h2;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod disc_tests;

/// A malformed or unreadable sky tag.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The Halo 1 map could not be read.
    #[error("halo 1 map: {0}")]
    H1(#[from] h1map::Error),
    /// The Halo 2 map could not be read.
    #[error("halo 2 map: {0}")]
    Map(#[from] map::Error),
    /// The render model could not be converted.
    #[error("model: {0}")]
    Model(#[from] h1models::Error),
    /// A structural invariant was violated.
    #[error("invalid sky: {0}")]
    Invalid(String),
    /// The tag is not a `sky `.
    #[error("tag is a {0}, want sky ")]
    WrongGroup(String),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;
