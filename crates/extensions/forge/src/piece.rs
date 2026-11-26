//! Block-out pieces in a session (docs/game/forge-blockout.md, "In the
//! document" and "Reaching the game"; the session's methods are
//! `session/pieces.rs`).
//!
//! A piece is a crate the document adds with the piece tag and a
//! definition ([`forge_shapes::PieceDef`]). In the world it is one object of
//! the definition's own `bloc` (`forge\blockout\<name>`), made only when the
//! loaded map holds that definition ([`crate::World::piece_loaded`]);
//! otherwise the piece is **pending**: it is in the document and has no
//! object, until the map is rebuilt and the game reports what it made
//! ([`crate::Session::adopt_copy`]).
//!
//! The three states are [`PieceState`]: `Live` (an object of the
//! definition's `bloc` stands), `Slot` (an object of a spare definition
//! rewritten to this one, slice BO10) and `Pending`.
//!
//! A piece's body is fixed where its object was made, so a piece is never
//! moved in the world: a drop deletes the object and makes it again where
//! the document puts it ([`drop`]), and a piece that is held stays where it
//! was while the monitor draws it as a ghost.
use forge_doc::{BlockKind, Endpoint, OpId, TagKey};
use forge_shapes::{PieceDef, units};

use crate::tool::Extent;
use crate::{Frame, PaletteItem, Session, World};

/// The `bloc` of a definition's tags: what an object of the piece is made
/// from.
pub fn bloc_key(def: &PieceDef) -> TagKey {
    TagKey {
        group: forge_doc::PIECE_GROUP.into(),
        path: def.tag_path(),
    }
}

/// The crate palette entry a definition's object is made from.
pub fn item(def: &PieceDef) -> PaletteItem {
    PaletteItem {
        block: BlockKind::Crates,
        tag: bloc_key(def),
    }
}

/// Whether a piece has an object in the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieceState {
    /// An object of the definition's own `bloc` stands.
    Live,
    /// An object of a spare definition, written to this one (BO10; nothing
    /// in this slice gives it).
    Slot,
    /// In the document, with no object until **Rebuild map**.
    Pending,
}

impl PieceState {
    /// The state as `forge.pieces` spells it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Slot => "slot",
            Self::Pending => "pending",
        }
    }
}

/// The axis-aligned box a piece fills in the world, in world units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// One piece of the session.
#[derive(Clone, Debug, PartialEq)]
pub struct PieceInfo {
    /// The op that added the placement; the target of its `Forge::Piece`.
    pub id: OpId,
    /// The placement's index in the crates block after every op.
    pub index: u32,
    pub def: PieceDef,
    /// The definition's canonical name ([`PieceDef::name`]).
    pub name: String,
    pub state: PieceState,
    /// Where the document puts it: its origin, the lowest corner of its
    /// bounds, and its turn.
    pub frame: Frame,
    pub bounds: PlacedBounds,
}

impl PieceInfo {
    /// The endpoint an op names the piece by.
    pub fn target(&self) -> Endpoint {
        Endpoint::Added(self.id)
    }

    /// The piece's bounding sphere in its own axes: the centre of its box
    /// and the distance to a corner.
    pub fn extent(&self) -> Extent {
        let half = half_size(&self.def);
        Extent {
            radius: (half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt(),
            offset: half,
        }
    }
}

/// The size of a definition in world units.
pub fn size(def: &PieceDef) -> [f32; 3] {
    def.size.map(units::world)
}

fn half_size(def: &PieceDef) -> [f32; 3] {
    size(def).map(|s| s / 2.0)
}

/// The box `def` fills with its origin at `frame.position`, turned by
/// `frame.rotation` (yaw about z, then pitch about y, then roll about x,
/// applied to the box as roll first). A quarter turn of yaw gives the
/// exact box on the grid.
pub fn bounds(def: &PieceDef, frame: Frame) -> PlacedBounds {
    let [sx, sy, sz] = size(def);
    let [yaw, pitch, roll] = frame.rotation;
    let (sr, cr) = roll.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sy_, cy) = yaw.sin_cos();
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for i in 0..8u8 {
        let c = [
            if i & 1 == 0 { 0.0 } else { sx },
            if i & 2 == 0 { 0.0 } else { sy },
            if i & 4 == 0 { 0.0 } else { sz },
        ];
        // Roll about x, pitch about y, yaw about z.
        let a = [c[0], c[1] * cr - c[2] * sr, c[1] * sr + c[2] * cr];
        let b = [a[0] * cp + a[2] * sp, a[1], -a[0] * sp + a[2] * cp];
        let d = [b[0] * cy - b[1] * sy_, b[0] * sy_ + b[1] * cy, b[2]];
        for k in 0..3 {
            let v = frame.position[k] + d[k];
            min[k] = min[k].min(v);
            max[k] = max[k].max(v);
        }
    }
    PlacedBounds { min, max }
}

/// Drop piece `id` at `frame`: the document takes the new position and
/// turn as one undo step (a group goes with it), and the world deletes the
/// object and makes it again there, so its fixed body is built where it
/// lands. A piece with no object (pending) only changes in the document.
pub fn drop(
    session: &mut Session,
    world: &mut impl World,
    tick: u32,
    id: OpId,
    frame: Frame,
) -> Result<(), String> {
    session.move_thing(world, tick, Endpoint::Added(id), frame)
}
