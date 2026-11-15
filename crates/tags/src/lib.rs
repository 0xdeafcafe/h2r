//! Reviewed Halo 2 retail Xbox tag definitions, generated fixed-record codecs,
//! and owned domain types. No community XML or game data is a build input.
//!
//! The five supported groups validate reviewed fields; opaque header spans are
//! preserved without claiming validation of their internal pointers. Only
//! bitmap image/sequence/sprite records are decoded recursively in this stage.
//! Localized text and compiled `unic` lists have a separate decoder in
//! [`localization`]. Unknown nested records remain opaque; see
//! `docs/map-format.md` for evidence.
#![forbid(unsafe_code)]

pub mod wire;
/// Object collision and physics models (`coll`, `phmo`); see `docs/collision.md`.
pub mod collision;
/// Sound tags (`snd!`, `ugh!`); self-contained, see `docs/sound.md`.
pub mod sound;
/// User-interface tags (`wgit`, `wgtz`, `wigl`, `skin`); self-contained, see `docs/ui.md`.
pub mod ui;
/// Animation graphs (`jmad`) and their compressed frame data; see `docs/animation.md`.
pub mod anim;
pub mod anim_sample;
pub mod records {
    include!(concat!(env!("OUT_DIR"), "/records.rs"));
}
// Localized text decoding is isolated from the generated tag codecs.
pub mod localization;
pub use localization::{Language, Localization, Unic};
pub use wire::{StringId, StringIdParts};

mod decode;
pub use decode::*;

/// A malformed or unsupported record, with context added at field boundaries.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Map(#[from] map::Error),
    #[error("record is {actual} bytes, expected exactly {expected}")]
    RecordSize { actual: usize, expected: usize },
    #[error("invalid tag: {0}")]
    Invalid(&'static str),
    #[error("unsupported tag group {0}")]
    UnsupportedGroup(map::FourCC),
    #[error("nonempty block {0} has no reviewed element layout")]
    UnsupportedBlock(&'static str),
    #[error("{record}.{field}: {source}")]
    Field {
        record: &'static str,
        field: &'static str,
        #[source]
        source: Box<Error>,
    },
    /// A decode followed more data than its source holds (aliased blocks
    /// counted every time they are followed).
    #[error("decoded {kind} exceed the budget of {limit}")]
    Budget { kind: &'static str, limit: u64 },
    #[error("range {address:#x} + {size:#x} is outside this tag's payload")]
    Bounds { address: u32, size: u64 },
    #[error("resource file {0:?} size was not supplied")]
    MissingResource(map::ResourceFile),
    #[error("resource {file:?} range {offset:#x}+{size:#x} exceeds file length {length:#x}")]
    ResourceBounds {
        file: map::ResourceFile,
        offset: u32,
        size: u32,
        length: u64,
    },
}
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod anim_tests;
