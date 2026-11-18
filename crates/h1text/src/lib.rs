//! Halo 1 text as Halo 2 localized text.
//!
//! - [`h1`] reads Halo 1 `ustr` (unicode string list) and `hmt ` (HUD
//!   message text) tags through [`h1map::H1Map`], decoding their UTF-16.
//! - [`convert`] turns them into Halo 2 tags: one `unic` built block per
//!   list, the locale rows behind it, and the string IDs that address them.
//!
//! Layouts and evidence: `docs/halo1/text.md`. No game data is in this
//! crate or its tests; real maps are read only by the `#[ignore]` tests,
//! from `$H1_ISO`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod convert;
pub mod h1;

/// Conversion failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The Halo 1 map could not be read.
    #[error("halo 1 map: {0}")]
    H1(#[from] h1map::Error),
    /// The Halo 2 map builder failed.
    #[error("map builder: {0}")]
    Build(#[from] map::Error),
    /// A string list is malformed.
    #[error("invalid {0}")]
    Invalid(String),
    /// A string list uses something this reader does not know.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// A tag is of the wrong group.
    #[error("tag is a {0}, want {1}")]
    WrongGroup(String, &'static str),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
