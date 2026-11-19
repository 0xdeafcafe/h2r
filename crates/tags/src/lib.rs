//! Reviewed Halo 2 retail Xbox tag definitions, generated fixed-record codecs,
//! and owned domain types. No community XML or game data is a build input.
//!
//! The five supported groups validate reviewed fields; opaque header spans are
//! preserved without claiming validation of their internal pointers. Only
//! bitmap image/sequence/sprite records are decoded recursively in this stage.
//! Localized text and compiled `unic` lists have a separate decoder in
//! [`localization`]. Unknown nested records remain opaque; see
//! `docs/formats/map-format.md` for evidence. Lighting and atmosphere have a separate
//! decoder in [`lighting`], extending the shared [`structure`] lightmap reader.
#![forbid(unsafe_code)]

/// The effects family (`effe`, `prt3`, `pmov`, ...); see `docs/formats/effects.md`.
pub mod effects;
/// Reviewed edits using the lossless cache writer.
pub mod edit;
/// Relocatable block trees for the whole-cache builder (`map::build`).
pub mod rebuild;
/// A new `bitm` from RGBA pixels, shaped like an existing one.
pub mod rgba_bitmap;
/// The main menu's level lists (multiplayer maps and campaign levels) in
/// the globals tag: names, descriptions, pictures.
pub mod ui_levels;
/// Typed tag definitions: every field named or an explicit unknown span;
/// see `docs/formats/tag-definitions.md`.
pub mod defs;
/// Tag layout schemas inferred from retail maps; relocation of any tag.
pub mod schema;
/// Every group's fields, labelled and typed, for editors: the schema's
/// structure merged with the definitions' names.
pub mod registry;
/// The loose tag repository: one position-independent file per tag, extracted
/// from retail maps and built back into maps; see `docs/formats/tag-repo.md`.
pub mod repo;
pub mod wire;
/// Object collision and physics models (`coll`, `phmo`); see `docs/formats/collision.md`.
pub mod collision;
/// Sound tags (`snd!`, `ugh!`); self-contained, see `docs/formats/sound.md`.
pub mod sound;
/// User-interface tags (`wgit`, `wgtz`, `wigl`, `skin`); self-contained, see `docs/formats/ui.md`.
pub mod ui;
/// Animation graphs (`jmad`) and their compressed frame data; see `docs/formats/animation.md`.
pub mod anim;
pub mod anim_sample;
/// Object family (`obje` subtypes, `hlmt`, `itmc`, `vehc`); see `docs/formats/objects.md`.
pub mod object;
/// Materials resolved from `shad` tags into ordered, ready-to-draw passes.
pub mod material;
/// Shader instances, templates and passes (`shad`, `stem`, `spas`); see
/// `docs/formats/shader-instances.md`.
pub mod shader;
/// Lightmap, sky and atmosphere tags; see `docs/formats/lighting.md`.
pub mod lighting;
#[cfg(test)]
mod lighting_tests;
pub mod records {
    include!(concat!(env!("OUT_DIR"), "/records.rs"));
}
// Localized text decoding is isolated from the generated tag codecs.
pub mod localization;
pub use localization::{Language, Localization, Unic};
pub use wire::{StringId, StringIdParts};

mod decode;
pub use decode::*;
// ---- geometry sections: render models, structure BSPs, lightmaps -----------
// `geometry` is the one section decoder (block info, raw block, fixups,
// parts, indices, vertex streams), driven by a per-owner `SectionLayout`.
// `model` reads `mode` tags, `structure` reads `sbsp`/`ltmp` render blocks.
// Provenance: docs/formats/map-format.md ("Render-model geometry") and
// docs/formats/structure-bsp.md.
pub mod geometry;
pub mod model;
pub mod structure;
#[cfg(test)]
mod structure_tests;
// ---- end geometry sections -------------------------------------------------
// AI pathfinding data in structure BSPs: docs/formats/pathfinding.md.
pub mod pathfinding;
#[cfg(test)]
mod pathfinding_tests;

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
    #[error("section {index}: {source}")]
    Section {
        index: usize,
        #[source]
        source: Box<Error>,
    },
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
