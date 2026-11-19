//! Halo 1 AI data to Halo 2's (docs/halo1/ai.md).
//!
//! - [`h1`] reads Halo 1's AI data through [`h1map`]: the scenario's actor
//!   palette, encounters (squads, platoons, firing positions, starting
//!   locations), command lists (atoms, points) and conversations, and the
//!   `actr` and `actv` tags.
//! - [`h2`] decodes Halo 2's equivalents out of a v8 scenario: squad
//!   groups, squads and their starting locations, zones with firing
//!   positions and areas, orders, point sets and the character palette, and
//!   `char` tags.
//! - [`convert`] maps the first onto the second and writes the result as
//!   `map::build` blocks for the scenario.
//!
//! No game data is in this crate or its tests. Tests build invented data;
//! the `#[ignore]`d ones read the user's discs at run time.
#![forbid(unsafe_code)]

mod bytes;
pub mod control;
pub mod conversations;
pub mod convert;
pub mod floors;
pub mod h1;
pub mod h2;

/// Why reading or converting failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The Halo 1 map couldn't be read.
    #[error(transparent)]
    H1(#[from] h1map::Error),
    /// A Halo 2 tag couldn't be read.
    #[error(transparent)]
    Tags(#[from] tags::Error),
    /// The map builder refused something.
    #[error(transparent)]
    Map(#[from] map::Error),
    /// Data isn't as the layout says.
    #[error("{0}")]
    Invalid(String),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;
