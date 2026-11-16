//! Halo 1 (Halo: Combat Evolved) Xbox cache file reader, cache version 5.
//!
//! An Xbox Halo 1 map is a `0x800`-byte header followed by one zlib stream
//! holding the rest of the file. [`H1Map::from_reader`] inflates it once and
//! keeps the whole decompressed file in memory: header, tag index, tag data
//! and the structure BSPs (which live in their own region, outside the tag
//! data). All offsets this crate reports are offsets into that decompressed
//! file.
//!
//! The reader knows the header, the tag index, the tag array and just enough
//! of the scenario tag to list its structure BSPs. [`scenario`] reads the
//! multiplayer placements and [`bsp`] a structure BSP's region and collision
//! BSP, the parts the converter needs first, and [`bitmap`] bitmap tags and
//! where their pixels are. [`disc`] recognises the Halo 1 disc and picks its maps.
//!
//! No game data is in this crate or its tests: the tests build invented maps
//! in memory. See `docs/formats/h1-map-format.md` for the format, with the
//! evidence for each fact.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod bitmap;
pub mod bsp;
pub mod disc;
mod format;
mod reader;
pub mod scenario;
mod space;
#[doc(hidden)]
pub mod testing;

pub use format::*;
pub use reader::{H1Map, parse_header};
pub use space::Space;

/// A malformed, unsupported or unreadable Halo 1 cache file.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The backing stream failed, or the zlib stream is corrupt.
    #[error("map I/O: {0}")]
    Io(#[from] std::io::Error),
    /// A structural invariant was violated.
    #[error("invalid Halo 1 map: {0}")]
    Invalid(String),
    /// The cache version is not the Xbox one this reader knows.
    #[error("unsupported cache: {0}")]
    Unsupported(String),
    /// An address or range falls outside the region it must be in.
    #[error("{what} range {offset:#x}+{size:#x} exceeds {limit:#x}")]
    Bounds {
        /// The range's purpose.
        what: &'static str,
        /// Requested offset or address.
        offset: u64,
        /// Requested byte count.
        size: u64,
        /// Exclusive upper bound.
        limit: u64,
    },
    /// No tag has this ID (index and salt), or no tag matches a query.
    #[error("tag not found")]
    TagNotFound,
    /// The tag has no data in this file (external, or a null address).
    #[error("tag has no data in this map")]
    NoTagData,
    /// The disc is not the Halo 1 disc, or lacks what an import needs.
    #[error("disc: {0}")]
    Disc(String),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod bitmap_tests;
