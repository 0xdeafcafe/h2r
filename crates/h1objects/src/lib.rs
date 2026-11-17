//! Halo 1 sandbox objects as Halo 2 tags.
//!
//! - [`h1`] reads Halo 1 Xbox `weap`, `proj`, `eqip`, `bipd`, `vehi`,
//!   `coll` and `phys` tags out of a cache file, through [`h1map::H1Map`].
//!   [`refs`] finds any tag's references without knowing its layout (used
//!   for `effe`, `jpt!`, `snd!`, `cont` and `ligh`, whose bodies are not
//!   read).
//! - [`coll::convert_collision`] builds a Halo 2 `coll` from a Halo 1 one.
//! - [`convert`] builds each object as a Halo 2 tag on a **retail Halo 2
//!   template** of the same role, read from the base map ([`base::Base`]):
//!   the template's root is copied, the fields this crate knows in both
//!   games are overwritten with Halo 1's values, the model chain is
//!   replaced by converted tags (`hlmt` -> `mode`, `coll`, `jmad`, with the
//!   template's `phmo`), and everything else (effects, sounds, HUD, first
//!   person, physics) stays the template's, listed in the report.
//!
//! The output is a [`convert::Tree`] of [`h1models::Converted`] tags,
//! installed together into a [`map::build::TagSet`] made from the base map
//! (`TagSet::from_map`): template child blocks that are not rewritten stay
//! where the pinned base has them, so the tree needs that base.
//!
//! Layouts, evidence and the tuning table: `docs/formats/h1-objects.md`. No
//! game data is in this crate or its tests; real maps are read only by the
//! `#[ignore]` tests.
#![forbid(unsafe_code)]

pub mod base;
pub mod coll;
pub mod convert;
pub mod h1;
pub mod refs;

pub use h1models::Converted;

/// Conversion failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("halo 1 map: {0}")]
    H1(#[from] h1map::Error),
    #[error("halo 2 map: {0}")]
    H2(#[from] map::Error),
    #[error("halo 2 tags: {0}")]
    Tags(#[from] tags::Error),
    #[error("model or animation: {0}")]
    Model(#[from] h1models::Error),
    #[error("collision BSP: {0}")]
    Bsp(#[from] h1convert::Error),
    #[error("truncated {0}")]
    Truncated(&'static str),
    #[error("tag is a {0}, want {group}", group = String::from_utf8_lossy(.1))]
    WrongGroup(String, [u8; 4]),
    #[error("no template: {0}")]
    NoTemplate(String),
    #[error("invalid: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
