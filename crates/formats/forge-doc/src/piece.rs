//! Block-out pieces: a crate placement with a definition
//! (docs/game/forge-blockout.md, "In the document").
//!
//! A piece is a `Placement::Add` on [`BlockKind::Crates`] whose palette
//! index is the placeholder key [`PIECE_TAG`], and a [`Forge::Piece`] that
//! gives that placement its definition ([`PieceDef`]). It names the
//! placement as a label does ([`Endpoint`]), so the definition follows the
//! entry through later appends and swap-removes, and
//! [`ForgeDoc::pieces`](crate::ForgeDoc::pieces) resolves the active pieces
//! to final indices.
//!
//! A definition has no scenario bytes: the materialiser installs the tags
//! of each definition once and points the placement at them. The ordinary
//! placement ops move, turn, duplicate and remove a piece; a duplicate (an
//! add with `like`) takes its source's definition, and a removal clears the
//! definition with the entry. A later `Forge::Piece` on the same target
//! replaces the definition.
//!
//! A document with a piece op is format 6
//! ([`FORMAT_PIECES`](crate::FORMAT_PIECES)). One with none keeps its older
//! number, so every older document keeps its bytes and its identity.
use forge_shapes::{Material, Materials, PieceDef};

use crate::{BlockKind, Endpoint, OpId, Refusal, Result, TagKey, Value};

/// The tag group of [`PIECE_TAG`]: a piece is a crate.
pub const PIECE_GROUP: &str = "bloc";
/// The placeholder path a piece's placement holds as its palette index. It
/// is not a tag of any map: the materialiser replaces it with the
/// definition's own `bloc` (`forge\blockout\<name>`), and it is not a
/// definition's name, so the two never meet.
pub const PIECE_TAG: &str = "forge\\blockout\\piece";
/// The field of an object placement that holds its tag.
pub(crate) const PALETTE_FIELD: &str = "palette index";
/// The block a piece is placed in.
pub(crate) const PIECE_BLOCK: BlockKind = BlockKind::Crates;

/// The placeholder key as a tag key.
pub fn piece_tag() -> TagKey {
    TagKey {
        group: PIECE_GROUP.into(),
        path: PIECE_TAG.into(),
    }
}

/// The value a piece's placement writes to its palette index.
pub fn piece_value() -> Value {
    Value::Tag(Some(piece_tag()))
}

/// Whether `value` is the placeholder key.
pub(crate) fn is_piece_value(value: &Value) -> bool {
    matches!(value, Value::Tag(Some(key)) if key.group == PIECE_GROUP && key.path == PIECE_TAG)
}

/// A piece an active op defines, with its placement's final index in the
/// crates block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivePiece {
    /// The op that gave the definition (for a duplicate, the add that made it).
    pub id: OpId,
    /// The entry's index after every placement edit.
    pub index: u32,
    pub def: PieceDef,
}

/// The document's active pieces, resolved against the base map's counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pieces {
    /// By final index.
    pub active: Vec<ActivePiece>,
}

fn invalid(reason: impl Into<String>) -> Refusal {
    Refusal::invalid("piece", reason)
}

/// The check that needs only the op: the target is a crate, and the
/// definition is one its own validator accepts, with every `Map` material
/// a well-formed tag key.
pub(crate) fn check_op(target: &Endpoint, def: &PieceDef) -> Result<()> {
    match *target {
        Endpoint::Base { block, .. } if block != PIECE_BLOCK => {
            return Err(invalid("a piece is a crate"));
        }
        Endpoint::Base { index, .. } if index > i16::MAX as u32 => {
            return Err(invalid("entry index exceeds block limit"));
        }
        Endpoint::Added(OpId(0)) => {
            return Err(invalid("want the ID of an add op"));
        }
        _ => {}
    }
    def.validate().map_err(|error| invalid(error.to_string()))?;
    let materials: &[Material] = match &def.materials {
        Materials::All(material) => std::slice::from_ref(material),
        Materials::Each(list) => list.as_slice(),
    };
    for material in materials {
        if let Material::Map(key) = material {
            crate::validate::tag_key(&TagKey {
                group: key.group.clone(),
                path: key.path.clone(),
            })?;
        }
    }
    Ok(())
}
