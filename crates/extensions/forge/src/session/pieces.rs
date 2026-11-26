//! Block-out pieces in a session (docs/game/forge-blockout.md, "In the
//! document" and "Reaching the game"; the types are `crate::piece`).
//!
//! Placing a piece is two ops in one undo step: a `Placement::Add` on the
//! crates block whose palette index is the piece tag, and the
//! `Forge::Piece` that gives it its definition. Changing one is a single
//! `Forge::Piece` on the same target (with the placement's own `Set`s when
//! the origin moved too), so undoing it restores the definition the log
//! gave before. A removal is the placement's, and it clears the definition.
//!
//! The session keeps no state of its own for a piece: its definition is
//! the document's, and its state is whether the world has an object for it
//! (`Session::objects`). The world makes the object of a definition it
//! holds ([`crate::World::piece_loaded`]) and no other, so a piece whose
//! definition the map lacks is pending; [`Session::adopt_copy`] takes the
//! objects of a rebuilt map and nothing is pending after.
use forge_doc::{BlockKind, Edit, Endpoint, FieldWrite, Forge, OpId, Placement};
use forge_shapes::PieceDef;

use super::{Session, scene, vector};
use crate::piece::{self, PieceInfo, PieceState};
use crate::{Frame, World};

/// What placing a piece says in a shared session: the host admits one op
/// at a time, and a placement of the piece tag is refused until its
/// definition's op follows.
pub const PIECES_LOCAL_ONLY: &str =
    "Block-out pieces can only be placed in a local session.";

impl Session {
    /// Every piece of the session in the order of the crates block, with
    /// its state: `Live` when the world has its object, else `Pending`.
    pub fn pieces(&self) -> Vec<PieceInfo> {
        let Ok(scene) = scene::project(&self.doc, &self.limits, &self.palette, self.budget) else {
            return vec![];
        };
        scene
            .objects
            .iter()
            .filter_map(|o| {
                let def = o.piece.clone()?;
                let live = self.objects.iter().any(|p| p.id == o.id);
                Some(PieceInfo {
                    id: o.id,
                    index: *scene.index.get(&o.id)?,
                    name: def.name(),
                    bounds: piece::bounds(&def, o.frame),
                    def,
                    state: if live {
                        PieceState::Live
                    } else {
                        PieceState::Pending
                    },
                    frame: o.frame,
                })
            })
            .collect()
    }

    /// The piece the thing `id` names.
    pub fn piece(&self, id: Endpoint) -> Option<PieceInfo> {
        let Endpoint::Added(op) = id.identity() else {
            return None;
        };
        self.pieces().into_iter().find(|p| p.id == op)
    }

    /// How many pieces have no object: those **Rebuild map** makes.
    pub fn pending_pieces(&self) -> usize {
        self.pieces()
            .iter()
            .filter(|p| p.state == PieceState::Pending)
            .count()
    }

    /// Place a piece of `def` with its origin at `frame`: the add and the
    /// definition's op, in one undo step. The world makes its object when
    /// it holds the definition; otherwise the piece is pending and the
    /// world is not called. Refused with the validator's sentence for a
    /// definition it does not take, with the budget's when the crates are
    /// full, and in a shared session.
    pub fn place_piece(
        &mut self,
        world: &mut impl World,
        tick: u32,
        def: PieceDef,
        frame: Frame,
    ) -> Result<OpId, String> {
        if self.shared.is_some() {
            return Err(PIECES_LOCAL_ONLY.into());
        }
        self.restore(world)?;
        check(&def)?;
        if let Some(budget) = self.budget_for(BlockKind::Crates)
            && budget.left == 0
        {
            return Err(budget.full());
        }
        let add = Edit::Placement(Placement::Add {
            block: BlockKind::Crates,
            like: None,
            fields: vec![
                FieldWrite {
                    field: "palette index".into(),
                    value: forge_doc::piece_value(),
                },
                vector("position", frame.position)?,
                vector("rotation", frame.rotation)?,
            ],
        });
        // The definition names the add by its ID, which the ops give.
        let mut ops = self.ops(vec![add, Edit::Undo])?;
        let id = ops[0].id;
        ops[1].edit = Edit::Forge(Forge::Piece {
            target: Endpoint::Added(id),
            def,
        });
        self.edit(world, tick, ops, None)?;
        Ok(id)
    }

    /// Change a piece: its definition (a size, a material, a seed) as one
    /// `Forge::Piece`, and with `origin` the placement's position and turn
    /// as one `Placement::Set` each that changes, in the same undo step. A
    /// change that changes nothing writes nothing. The object, when there
    /// is one, is made again: a different definition is a different
    /// `bloc`, and a moved piece is made again where it lands.
    pub fn set_piece(
        &mut self,
        world: &mut impl World,
        tick: u32,
        id: OpId,
        def: PieceDef,
        origin: Option<Frame>,
    ) -> Result<(), String> {
        self.restore(world)?;
        check(&def)?;
        let (object, index) = self.target(id)?;
        let current = object.piece.ok_or("Aim at a block-out piece.")?;
        let mut edits = vec![];
        if current != def {
            edits.push(Edit::Forge(Forge::Piece {
                target: Endpoint::Added(id),
                def,
            }));
        }
        if let Some(frame) = origin {
            for (name, old, new) in [
                ("position", object.frame.position, frame.position),
                ("rotation", object.frame.rotation, frame.rotation),
            ] {
                if old.map(f32::to_bits) != new.map(f32::to_bits) {
                    edits.push(Edit::Placement(Placement::Set {
                        block: BlockKind::Crates,
                        index,
                        write: vector(name, new)?,
                    }));
                }
            }
        }
        if edits.is_empty() {
            return Ok(());
        }
        let ops = self.ops(edits)?;
        self.edit(world, tick, ops, None)
    }

    /// Duplicate the piece `source` at `frame`: a piece of the same
    /// definition, as [`Session::place_piece`].
    pub(super) fn duplicate_piece(
        &mut self,
        world: &mut impl World,
        tick: u32,
        source: Endpoint,
        frame: Frame,
    ) -> Result<OpId, String> {
        let def = self
            .piece(source)
            .ok_or("Aim at a block-out piece.")?
            .def;
        self.place_piece(world, tick, def, frame)
    }
}

/// The definition's own checks, which the validator makes too: a refusal
/// here is the same sentence, before an op is stamped.
fn check(def: &PieceDef) -> Result<(), String> {
    def.validate().map_err(|e| e.to_string())
}
