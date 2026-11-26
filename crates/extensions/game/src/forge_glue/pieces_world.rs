//! Block-out pieces the world has no object for, drawn by the markers' pass
//! (docs/game/forge-blockout.md, "In the session" and "Dev view"): a pending
//! piece's shell as lines and a skin in the pending tint, the ghost of a
//! piece being placed or resized, and with the Block-out overlay on the
//! barriers and `Nodraw` faces in their family's colour.
//!
//! [`draws`] says what is drawn; [`group`] makes one [`Group`] of a draw, as
//! [`super::markers_world::draw`] makes one of a marker, and the markers'
//! source draws them with its own ([`published`]). The groups are built when
//! the driver publishes, not each frame.
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use forge::piece::{PieceInfo, PieceState, turned};
use forge::{Frame, Material, PieceDef};
use forge_shapes::Role;
use game_markers::Tri;

use super::markers_world::Group;

/// A pending piece: its shell and skin.
pub const PENDING: [f32; 3] = [1.0, 0.72, 0.2];
/// The ghost of a piece being placed or resized.
pub const GHOST: [f32; 3] = [0.45, 0.85, 1.0];
/// A barrier (`Clip` on every role), with the Block-out overlay on: the
/// dev set's Clip family, magenta.
pub const BARRIER: [f32; 3] = [1.0, 0.2, 0.85];
/// A piece's `Nodraw` faces, with the overlay on: the Nodraw family, yellow.
pub const NODRAW: [f32; 3] = [1.0, 0.9, 0.2];
/// The alpha of a skin, and of its lines.
pub const SKIN_ALPHA: f32 = 0.18;
pub const LINE_ALPHA: f32 = 0.9;

/// How a piece is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Pending,
    Ghost,
    Barrier,
    /// Only the faces whose role is `Nodraw`.
    Nodraw,
}

/// One piece to draw: its definition, where it stands and how.
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    pub def: PieceDef,
    pub frame: Frame,
    pub look: Look,
}

fn all_clip(def: &PieceDef) -> bool {
    Role::ALL
        .iter()
        .all(|r| *def.materials.get(*r) == Material::Clip)
}

fn any_nodraw(def: &PieceDef) -> bool {
    Role::ALL
        .iter()
        .any(|r| *def.materials.get(*r) == Material::Nodraw)
}

/// What the session's pieces and the piece in hand draw: every pending
/// piece, the ghost, and with `overlay` the barriers and the pieces with a
/// `Nodraw` face. A piece with an object is drawn by the game.
pub fn draws(pieces: &[PieceInfo], overlay: bool, ghost: Option<(PieceDef, Frame)>) -> Vec<Draw> {
    let mut out: Vec<Draw> = pieces
        .iter()
        .filter_map(|p| {
            let look = if p.state == PieceState::Pending {
                Look::Pending
            } else if !overlay {
                return None;
            } else if all_clip(&p.def) {
                Look::Barrier
            } else if any_nodraw(&p.def) {
                Look::Nodraw
            } else {
                return None;
            };
            Some(Draw {
                def: p.def.clone(),
                frame: p.frame,
                look,
            })
        })
        .collect();
    out.extend(ghost.map(|(def, frame)| Draw {
        def,
        frame,
        look: Look::Ghost,
    }));
    out
}

/// The shell of one draw as lines and a skin. `None` when the definition
/// does not generate (it was validated when it was placed).
pub fn group(d: &Draw) -> Option<Group> {
    let solid = d.def.generate().ok()?;
    let colour = match d.look {
        Look::Pending => PENDING,
        Look::Ghost => GHOST,
        Look::Barrier => BARRIER,
        Look::Nodraw => NODRAW,
    };
    let world = |p: [f32; 3]| -> [f32; 3] {
        let t = turned(d.frame.rotation, p);
        std::array::from_fn(|i| d.frame.position[i] + t[i])
    };
    let skin = [colour[0], colour[1], colour[2], SKIN_ALPHA];
    let mut edges = Vec::new();
    let mut tris = Vec::new();
    for face in &solid.shell {
        if d.look == Look::Nodraw && *d.def.materials.get(face.role) != Material::Nodraw {
            continue;
        }
        let ring: Vec<[f32; 3]> = face.ring.iter().map(|p| world(*p)).collect();
        for i in 0..ring.len() {
            edges.push([ring[i], ring[(i + 1) % ring.len()]]);
        }
        for i in 1..ring.len().saturating_sub(1) {
            tris.push(Tri::flat([ring[0], ring[i], ring[i + 1]], skin));
        }
    }
    if edges.is_empty() {
        return None;
    }
    let anchor = edges[0][0];
    Some(Group {
        edges,
        dim: Vec::new(),
        tris,
        anchor,
        colour,
        alpha: LINE_ALPHA,
        fade: None,
        label: None,
    })
}

static PUBLISHED: Mutex<Vec<Group>> = Mutex::new(Vec::new());
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The groups the window draws from now on, in place of the last ones.
pub fn publish(draws: &[Draw]) {
    let groups: Vec<Group> = draws.iter().filter_map(group).collect();
    ACTIVE.store(!groups.is_empty(), Ordering::Release);
    *PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()) = groups;
}

/// No session: nothing is drawn.
pub fn clear() {
    ACTIVE.store(false, Ordering::Release);
    PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Whether any piece is drawn.
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// The groups of the last [`publish`].
pub fn published() -> Vec<Group> {
    PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
#[path = "pieces_world_tests.rs"]
mod tests;
