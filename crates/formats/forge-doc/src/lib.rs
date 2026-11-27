//! Portable Forge map documents and the host-stamped edit log.
//!
//! The format carries authored values and references. The materialiser or
//! runtime implements [`Apply`] against a map it already holds.
//! See [the crate README](../README.md) for the wire format and limits.
#![forbid(unsafe_code)]

mod area;
mod document;
mod entry;
pub mod games;
mod ground;
mod groups;
mod labels;
mod links;
mod log;
mod piece;
mod prefab;
mod resolve;
pub mod siege_layout;
mod validate;
mod variant;
pub mod waves;
mod wire;

pub use area::{Area, MarkerArea};
pub use content::{MapId, Sha256};
pub use document::*;
pub use entry::{
    EntryOp, OBJECT_BLOCKS, PROPERTY_TEAMS, Print, Property, SPARE_CLIPS_MAX, check_print,
    for_game, object_games, print, spare_clips, teams,
};
pub use ground::*;
pub use groups::*;
pub use labels::*;
pub use links::*;
pub use log::*;
pub use piece::{ActivePiece, PIECE_GROUP, PIECE_TAG, Pieces, piece_tag, piece_value};
pub use prefab::*;
pub use map::scenario_layout::BlockKind;
pub use resolve::{Printed, Resolved, resolve};
pub use validate::{Limits, WorldBounds, validate};
pub use variant::{KIT_RECIPE, MAX_VARIANT_NAME, Tier, kit, play_form, session_form, variant_name};
pub use waves::FORMAT_WAVES;

use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Refusal>;

/// A named refusal suitable for a host response or an offline validator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Refusal {
    #[error(
        "Forge format {0} is unsupported (this build reads 1, 2 for documents with labels, 3 for documents with entry edits or prints, 4 for documents with groups, 5 for documents with a wave set and 6 for documents with block-out pieces)"
    )]
    Format(u32),
    /// The base entry an edit names is not the one it was made on: its
    /// typed fields give another print. Nothing is written.
    #[error(
        "the map's {} entry {index} is not the one this edit was made on",
        .block.name()
    )]
    Print {
        #[serde(with = "wire::block")]
        block: BlockKind,
        index: u32,
    },
    /// A document offered as a map variant holds more than one may: a
    /// recipe other than the kit's, an import slot, a source map, a tag
    /// write or a placement op by running index other than an add.
    #[error("the document is a Forge map, not a map variant")]
    Tier,
    #[error("invalid JSON: {0}")]
    Json(String),
    #[error("{what} exceeds its limit of {max}")]
    Limit { what: String, max: usize },
    #[error("{what}: {reason}")]
    Invalid { what: String, reason: String },
    #[error("{0}")]
    Area(String),
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

    pub(crate) fn area(error: game_markers::AreaError) -> Self {
        Self::Area(error.to_string())
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

    /// Write the active links as retail data, after every placement and tag
    /// edit. Indices are the entries' final ones.
    fn links(&mut self, links: &Links) -> Result<()> {
        if links.active.is_empty() {
            Ok(())
        } else {
            Err(Refusal::Unsupported("link".into()))
        }
    }

    /// Hand over the active labels, after the links. Indices are the
    /// entries' final ones. Labels are document data with no scenario
    /// bytes, so an adapter keeps them in its own table.
    fn labels(&mut self, labels: &Labels) -> Result<()> {
        if labels.active.is_empty() {
            Ok(())
        } else {
            Err(Refusal::Unsupported("label".into()))
        }
    }

    /// Hand over the active groups, after the labels. Indices are the
    /// entries' final ones. Groups are document data with no scenario
    /// bytes, so an adapter keeps them in its own table.
    fn groups(&mut self, groups: &GroupSet) -> Result<()> {
        if groups.active.is_empty() {
            Ok(())
        } else {
            Err(Refusal::Unsupported("group".into()))
        }
    }

    /// Hand over the active block-out pieces, after the groups. Indices are
    /// the entries' final ones. A piece's definition has no scenario bytes
    /// of its own: the adapter installs the tags of each definition and
    /// points the placement at them.
    fn pieces(&mut self, pieces: &Pieces) -> Result<()> {
        if pieces.active.is_empty() {
            Ok(())
        } else {
            Err(Refusal::Unsupported("piece".into()))
        }
    }

    /// Check every base entry the document names with a print, before the
    /// first edit: the entry as the base map has it must give the print
    /// (`check_print`), or the refusal is `Refusal::Print` and nothing is
    /// written. An adapter that cannot read its entries' fields refuses a
    /// document with prints.
    fn prints(&mut self, prints: &[Printed]) -> Result<()> {
        if prints.is_empty() {
            Ok(())
        } else {
            Err(Refusal::Unsupported("print".into()))
        }
    }
}

/// Validate the entire document before handing its edits to the adapter.
/// The adapter checks the prints first, then takes the edits in op order,
/// each entry op as the placement op [`resolve`] lowers it to. If the
/// adapter refuses an edit, replay stops there.
pub fn replay(doc: &ForgeDoc, limits: &Limits, target: &mut impl Apply) -> Result<()> {
    validate(doc, limits)?;
    let resolved = resolve(&doc.ops, &limits.base_counts)?;
    target.prints(&resolved.prints)?;
    let mut linked = false;
    let mut labelled = false;
    let mut grouped = false;
    let mut pieced = false;
    for op in &doc.ops {
        match &op.edit {
            Edit::Placement(edit) => target.placement(edit)?,
            Edit::Entry(_) => {
                if let Some(edit) = resolved.lowered.get(&op.id) {
                    target.placement(edit)?;
                }
            }
            Edit::TagField(write) => target.tag_field(write)?,
            Edit::Forge(Forge::Label { .. }) => labelled = true,
            Edit::Forge(Forge::Group(_) | Forge::Ungroup { .. } | Forge::GroupLock { .. }) => {
                grouped = true
            }
            Edit::Forge(Forge::Piece { .. }) => pieced = true,
            Edit::Forge(_) => linked = true,
            edit => return Err(edit.unsupported()),
        }
    }
    if linked {
        target.links(&doc.links(&limits.base_counts)?)?;
    }
    if labelled {
        target.labels(&doc.labels(&limits.base_counts)?)?;
    }
    if grouped {
        target.groups(&doc.groups(&limits.base_counts)?)?;
    }
    if pieced {
        target.pieces(&doc.pieces(&limits.base_counts)?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
