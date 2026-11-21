//! Portable Forge map documents and the host-stamped edit log.
//!
//! The format carries authored values and references. The materialiser or
//! runtime implements [`Apply`] against a map it already holds.
//! See [the crate README](../README.md) for the wire format and limits.
#![forbid(unsafe_code)]

mod document;
mod log;
mod validate;
mod wire;

pub use content::{MapId, Sha256};
pub use document::*;
pub use log::*;
pub use map::scenario_layout::BlockKind;
pub use validate::{Limits, WorldBounds, validate};

use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Refusal>;

/// A named refusal suitable for a host response or an offline validator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Refusal {
    #[error("Forge format {0} is unsupported (this build reads {FORMAT})")]
    Format(u32),
    #[error("invalid JSON: {0}")]
    Json(String),
    #[error("{what} exceeds its limit of {max}")]
    Limit { what: String, max: usize },
    #[error("{what}: {reason}")]
    Invalid { what: String, reason: String },
    #[error("{0} edits are reserved for a later Forge slice")]
    Unsupported(String),
    #[error("proposal starts at sequence {proposed}, host is at {head}")]
    Stale { proposed: u32, head: u32 },
    #[error("the target's current value disagrees with `was`")]
    Mismatch,
    #[error("edit log at sequence {seq}: {reason}")]
    Chain { seq: u32, reason: String },
    #[error("operation {0} is already in the document")]
    DuplicateOp(OpId),
    #[error("operation {0} is not the latest active edit")]
    UndoOrder(OpId),
    #[error("operation {0} belongs to another author")]
    UndoAuthor(OpId),
}

impl Refusal {
    pub(crate) fn invalid(what: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Invalid {
            what: what.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn limit(what: &str, max: usize) -> Self {
        Self::Limit {
            what: what.into(),
            max,
        }
    }
}

/// An adapter applies one document edit to its own map. It resolves tag keys
/// and field paths, checks permissions, `was`, the map's current bounds and
/// counts, and its budget where applicable. A refusal leaves its state as it
/// was before the edit.
pub trait Apply {
    fn placement(&mut self, edit: &Placement) -> Result<()>;
    fn tag_field(&mut self, write: &TagWrite) -> Result<()>;
}

/// Validate the entire document before handing its edits to the adapter.
/// If the adapter refuses an edit, replay stops there.
pub fn replay(doc: &ForgeDoc, limits: &Limits, target: &mut impl Apply) -> Result<()> {
    validate(doc, limits)?;
    for op in &doc.ops {
        match &op.edit {
            Edit::Placement(edit) => target.placement(edit)?,
            Edit::TagField(write) => target.tag_field(write)?,
            edit => return Err(edit.unsupported()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
