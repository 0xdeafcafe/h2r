//! Halo 1 Xbox sounds: decoded to PCM, and converted to Halo 2 `snd!` tags
//! with their entries in the map's `ugh!` sound gestalt.
//!
//! - [`h1`] reads Halo 1 `snd!` tags through [`h1map`] and finds each
//!   permutation's samples in the map file.
//! - [`decode`] turns them into 16-bit PCM with the audio crate's Xbox ADPCM
//!   decoder (the one Halo 2's mixer plays with) and writes WAVs.
//! - [`gestalt`] is the whole Halo 2 `ugh!` tag as an editable model: read
//!   from a map or a [`map::build::TagSet`], extended with new sounds, and
//!   written back as a built block tree.
//! - [`convert`] makes a Halo 2 sound from a Halo 1 one: a 20-byte `snd!`
//!   (a [`h1models::Converted`]) and its pitch ranges, permutations and
//!   chunks in the gestalt. Samples are copied as they are; Xbox ADPCM is
//!   never re-encoded.
//! - [`class`] maps Halo 1 sound classes to Halo 2's.
//! - [`looping`] converts looping sounds (`lsnd`, with the sounds they
//!   play) and sound environments (`snde`).
//!
//! Layouts and evidence: `docs/halo1/sounds.md`. No game data is in
//! this crate or its tests; real maps are read only by `#[ignore]` tests.
#![forbid(unsafe_code)]
// Strides here are format constants; indexed loops mirror the eleven blocks.
#![allow(clippy::chunks_exact_to_as_chunks, clippy::needless_range_loop)]

pub mod audit;
pub mod class;
pub mod convert;
pub mod decode;
pub mod gestalt;
pub mod h1;
pub mod looping;
pub mod merge;
mod mouth;

pub use convert::{ConvertedSound, convert_sound, install_sounds};
pub use decode::{Pcm, decode_all, decode_chain};
pub use gestalt::SoundGestalt;
pub use h1::Sound;
pub use merge::{MergeMap, Moved, merge_sound, transplant, transplant_from};

/// Decode and conversion failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Reading the Halo 1 map failed.
    #[error("halo 1 map: {0}")]
    H1(#[from] h1map::Error),
    /// The ADPCM decoder refused the samples.
    #[error("decode: {0}")]
    Decode(#[from] audio::AudioError),
    /// Building or reading the Halo 2 map failed.
    #[error("map: {0}")]
    Map(#[from] map::Error),
    /// The data is malformed or contradictory.
    #[error("invalid: {0}")]
    Invalid(String),
    /// A value with no Halo 2 equivalent, or a codec not decoded here.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
