//! Halo 2 retail Xbox (cache v8) reader and writer, with no game data or unsafe code.
//!
//! [`Map::open`] opens a file; [`Map::from_reader`] accepts a seekable stream,
//! and [`Map::from_region`] reads directly from a bounded region of a disc.
//! Only indexes and strings are loaded eagerly. Tag payloads are read on demand.
//! All file offsets are relative to the map, even when it lives inside a disc.
//!
//! See `docs/formats/map-format.md` for provenance, addressing rules and limitations.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Whole-cache builder: write a complete map from a set of tags.
pub mod build;
/// Engine checksum and streaming checksum state.
pub mod checksum;
mod format;
mod reader;
/// Pass-through cache writing (not a rebuild) and bounded edits.
pub mod writer;

pub use format::*;
pub use reader::Map;

/// A malformed, unsupported or unreadable cache file.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The backing stream failed.
    #[error("map I/O: {0}")]
    Io(#[from] std::io::Error),
    /// A structural invariant was violated.
    #[error("invalid map: {0}")]
    Invalid(&'static str),
    /// Cache version or platform is outside this reader's scope.
    #[error("unsupported cache: {0}")]
    Unsupported(String),
    /// A file range falls outside the map or a metadata pointer's region.
    #[error("{what} range {offset:#x}+{size:#x} exceeds {limit:#x}")]
    Bounds {
        /// The range's purpose.
        what: &'static str,
        /// Requested offset.
        offset: u64,
        /// Requested byte count.
        size: u64,
        /// Exclusive upper bound.
        limit: u64,
    },
    /// A built map's tag memory (its index, BSP region and metadata,
    /// header `+0x1c`) is over the engine's for its cache type; says by how
    /// much and which tag groups take the most.
    #[error("metadata exceeds the engine's tag memory for this cache type: {0}")]
    TagMemory(String),
    /// An allocation or configured table-size limit was exceeded.
    #[error("map allocation exceeds supported limits")]
    Limit,
    /// A string is not UTF-8 (retail tables are ASCII).
    #[error("invalid UTF-8 in {0}")]
    Encoding(&'static str),
    /// No live tag matches a query, including its datum salt.
    #[error("tag not found")]
    TagNotFound,
    /// Several groups share the requested name; qualify with a group or ID.
    #[error("ambiguous tag name; use TagQuery::NameInGroup or a TagId")]
    AmbiguousName,
    /// A tag has no local payload, e.g. an unresolved null index address.
    #[error("tag has no payload in this map")]
    NoTagData,
}

/// Result type for this crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod build_tests;
