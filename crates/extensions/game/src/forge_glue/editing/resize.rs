//! A block-out piece in hand: the Resize draft, the Size page and the
//! changes a Properties row makes to the definition
//! (docs/game/forge-blockout.md, "In the monitor"). The draft's state and
//! its steps are `forge::tool::Draft`; this is its screens and its write.
//! Only A writes: one `Forge::Piece`, and a `Placement::Set` when the origin
//! moved, in one undo step (`Session::set_piece`).
use forge::tool::{Command, Draft};
use forge::{Endpoint, OpId, PieceInfo};

use super::{After, Editor, Kind, run, screens, sheet};
use screens::blockout;
use sheet::PieceField;

impl Editor {
    /// The piece `id` names, or why it is not one.
    pub(super) fn piece_of(&self, id: OpId) -> Result<PieceInfo, String> {
        self.session
            .piece(Endpoint::Added(id))
            .ok_or_else(|| "Aim at a block-out piece.".to_string())
    }

    /// What a piece's Properties rows are built from: none for a thing that
    /// is not a piece.
    pub(super) fn piece_facts(&self, id: Endpoint) -> Option<blockout::PieceFacts> {
        let piece = self.session.piece(id)?;
        Some(blockout::PieceFacts {
            id: piece.id,
            def: piece.def,
            surfaces: self.surfaces.clone(),
        })
    }

    /// **Resize**: the draft of the piece begins.
    pub(super) fn begin_resize(&mut self, id: OpId) -> Result<(), String> {
        self.tool.begin_resize(&self.session, id)
    }

    /// The readout's line for a draft: the aimed face and the size.
    pub(super) fn resize_line(&self, draft: &Draft) -> String {
        match draft.face {
            Some(face) => format!(
                "Resize {} {} units, {}",
                forge::piece::axis_name(face.axis),
                draft.def.size[face.axis],
                blockout::size_line(&draft.def)
            ),
            None => format!("Resize {}", blockout::size_line(&draft.def)),
        }
    }

    /// The draft the Size page edits: the one in hand, or the one the
    /// Properties target's piece begins.
    fn size_draft(&mut self) -> Result<&mut Draft, String> {
        if self.tool.draft.is_none() {
            let Endpoint::Added(id) = self.property_target()?.identity() else {
                return Err("Aim at a block-out piece.".into());
            };
            self.begin_resize(id)?;
        }
        self.tool.draft.as_mut().ok_or_else(|| "Aim at a block-out piece.".into())
    }

    fn show_size(&self) -> Result<(), String> {
        let draft = self.tool.draft.as_ref().ok_or("No piece is being resized.")?;
        screens::screen(Kind::Properties).show(blockout::size_sheet(&draft.def))
    }

    /// X on the draft: the Size page.
    pub(super) fn open_size(&mut self) -> Result<(), String> {
        let draft = self.tool.draft.as_ref().ok_or("No piece is being resized.")?;
        screens::open(blockout::size_sheet(&draft.def))
    }

    /// A number typed on the Size page, in units. A refused size is the
    /// definition's sentence and leaves the draft as it was.
    pub(super) fn size_number(&mut self, field: PieceField, text: &str) -> Result<(), String> {
        let draft = self.size_draft()?;
        match field {
            PieceField::Axis(axis) => {
                let units = text.trim().parse::<i32>().map_err(|_| "Type a whole number.")?;
                draft.set_size(axis, units)?;
            }
            PieceField::Param => {
                let param = text.trim().parse::<u16>().map_err(|_| "Type a whole number.")?;
                draft.set_param(param)?;
            }
        }
        self.show_size()
    }

    /// Left or right on a Size row: the size by one block-out grid, or the
    /// param by one.
    pub(super) fn nudge_size(&mut self, field: PieceField, right: bool) -> Result<(), String> {
        let grid = i32::from(self.tool.block_grid.max(1));
        let draft = self.size_draft()?;
        match field {
            PieceField::Axis(axis) => {
                let now = i32::from(draft.def.size[axis]);
                draft.set_size(axis, now + if right { grid } else { -grid })?;
            }
            PieceField::Param => {
                let now = i32::from(draft.def.param);
                let next = u16::try_from((now + if right { 1 } else { -1 }).max(0))
                    .map_err(|_| "That is past the largest param.")?;
                draft.set_param(next)?;
            }
        }
        self.show_size()
    }

    /// Give the piece `id` the definition `def` (a material or a reroll):
    /// one `Forge::Piece`, the piece staying where it is.
    pub(super) fn write_piece(
        &mut self,
        tick: u32,
        id: OpId,
        def: forge::PieceDef,
    ) -> Result<(), String> {
        let origin = self.piece_of(id)?.frame;
        let cmd = Command::ResizePiece { id, def, origin };
        self.stage(After::Tool(cmd.clone()), move |s, w| run(s, w, tick, cmd))
    }

    /// The pieces the world draws for the markers' pass: what is pending,
    /// the piece in hand and, with the overlay on, the barriers and
    /// `Nodraw` faces ([`super::super::pieces_world`]).
    pub(crate) fn piece_draws(&self) -> Vec<super::super::pieces_world::Draw> {
        let overlay = self.tool.monitor && self.overlay(forge::tool::Overlay::Blockout);
        let ghost = match (&self.tool.draft, &self.tool.placing_piece, self.tool.ghost) {
            (Some(draft), _, _) => Some((draft.def.clone(), draft.frame)),
            (None, Some(def), Some(frame)) => Some((def.clone(), frame)),
            _ => None,
        };
        if !self.tool.monitor {
            return Vec::new();
        }
        super::super::pieces_world::draws(&self.session.pieces(), overlay, ghost)
    }

    /// Sync the block-out settings from the profile's store.
    pub(crate) fn set_block_settings(&mut self, grid: Option<u16>, dev_view: Option<bool>) {
        if let Some(grid) = grid {
            self.tool.set_block_grid(grid);
        }
        if let Some(on) = dev_view {
            self.dev_view = on;
        }
    }

    /// The settings the player changed on a screen since the last call: the
    /// driver writes them to the profile's store.
    pub(crate) fn take_setting_writes(&mut self) -> Vec<(&'static str, settings::Value)> {
        std::mem::take(&mut self.setting_writes)
    }

    /// The map's surfaces for the Material picker, as shader paths.
    pub(crate) fn set_surfaces(&mut self, surfaces: Vec<String>) {
        self.surfaces = surfaces;
    }
}
