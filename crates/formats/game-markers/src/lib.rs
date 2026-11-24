//! Game type markers (docs/game/gametype-markers.md): the shapes Forge
//! and Studio draw for the parts of a game type, and the area a hill, a
//! territory or a site covers.
//!
//! A marker is a [`Kind`] and a team. [`Marker::mesh`] gives its lines
//! and translucent triangles in the marker's own space: x forward (the
//! way it faces), y left, z up, in world units, the origin where the
//! placement is. [`Mesh::placed`] puts it in the world. Both drawers draw
//! the same mesh, so a marker is one shape in Forge, in Studio and in a
//! render.
//!
//! Every shape is generated here from numbers of ours.
pub mod area;
pub mod kind;
pub mod mesh;
pub mod palette;
pub mod record;
pub mod shapes;

#[cfg(test)]
mod tests;

pub use area::{Area, AreaError, Shape};
pub use kind::{Item, Kind, Solid, Volume};
pub use mesh::{Line, Mesh, Place, Tri, Vec3};
pub use palette::{Palette, Tint};

/// How far a part that lies on the floor is lifted off it, in world
/// units, so it does not fight the floor it lies on.
pub const LIFT: f32 = 0.02;
/// The alpha of a marker's lines.
pub const LINE_ALPHA: f32 = 1.0;
/// The alpha of a marker's filled faces: a hologram's skin.
pub const FILL_ALPHA: f32 = 0.16;

/// One marker: what it is and whose it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub kind: Kind,
    /// The placement's team, 0 to 7; `None` for neutral or no team.
    pub team: Option<u8>,
}

impl Marker {
    pub fn new(kind: Kind, team: Option<u8>) -> Self {
        Self { kind, team }
    }

    /// The marker's colour: its team's, or its kind's own.
    pub fn colour(&self, palette: &Palette) -> [f32; 3] {
        palette.colour(self.kind.tint(), self.team)
    }

    /// The marker's lines and faces in its own space, in its colour.
    pub fn mesh(&self, palette: &Palette) -> Mesh {
        self.kind.mesh(self.colour(palette))
    }
}
