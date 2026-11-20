//! Halo 1 sandbox objects as Halo 2 tags.
//!
//! - [`h1`] reads Halo 1 Xbox `weap`, `proj`, `eqip`, `bipd`, `vehi`,
//!   `coll` and `phys` tags out of a cache file, through [`h1map::H1Map`].
//!   [`refs`] finds any tag's references without knowing its layout (used
//!   for `effe`, `jpt!`, `snd!`, `cont` and `ligh`, whose bodies are not
//!   read).
//! - [`sandbox::Objects`]: the objects a converted multiplayer map's
//!   spawns name, for `h1convert`'s Halo 1 sandbox.
//! - [`coll::convert_collision`] builds a Halo 2 `coll` from a Halo 1 one,
//!   with BSP physics slots (Havok's view) for scenery.
//! - [`query`] answers points and rays on either game's collision BSP.
//! - [`physics`] cuts a collision BSP into its solid convex cells, which
//!   [`rebuild::physics_model`] writes as a Halo 2 physics model for
//!   vehicles and devices.
//! - [`convert`] resolves Halo 1 object dependencies and builds the model,
//!   animation, collision and physics tags used by each object.
//! - [`rebuild`] writes the objects from typed definitions ([`tags::defs`]).
//!
//! [`convert::Tree`] installs the resulting [`h1models::Converted`] tags
//! together into the destination [`map::build::TagSet`].
//!
//! Layouts, evidence and the tuning table: `docs/halo1/objects.md`. No
//! game data is in this crate or its tests; real maps are read only by the
//! `#[ignore]` tests.
#![forbid(unsafe_code)]

pub mod arsenal;
pub mod base;
pub mod coll;
pub mod functions;
pub mod convert;
pub mod h1;
pub mod h1hud;
pub mod health_hud;
mod particle_movement;
pub mod particles;
pub mod physics;
pub mod provenance;
pub mod query;
pub mod ragdoll;
pub mod rebuild;
pub mod refs;
pub mod sandbox;
pub mod scripted_props;
pub mod verify;

pub use convert::Options;
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
