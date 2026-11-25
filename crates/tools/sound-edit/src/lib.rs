//! Waveform edits as a list over a source WAV that is never rewritten
//! (docs/editor/studio-redesign.md, "The sound editor"). See the README.
mod doc;
mod edit;
mod wave;

#[cfg(test)]
mod tests;

pub use doc::{Permutation, PermutationPatch, SoundDoc};
pub use edit::{
    DEFAULT_CEILING_DBFS, Edit, MAX_GAIN_DB, RenderError, apply, insert, remove, render, reorder, undo,
};
pub use wave::Wave;
