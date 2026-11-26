//! Block-out pieces (docs/game/forge-blockout.md): a definition, its canonical
//! name and the generators that turn it into a closed shell of convex faces
//! and the convex hulls Havok collides with.
//!
//! A definition is integers and tag keys. The geometry is `+ - * /` on `f32`
//! and integer work, so the same definition gives the same bits on every
//! machine. Nothing here reads a disc or a file.

pub mod circle;
mod def;
mod error;
mod generate;
pub mod hull;
pub mod natural;
pub mod noise;
mod solid;
pub mod units;

pub use def::{Material, Materials, ParsedName, PieceDef, Shape, TagKey};
pub use error::{Error, Result};
pub use solid::{Face, Hull, P3, Role, Solid, newell};

/// The sizes the kit installs ahead of any document, so a fresh session
/// places the common pieces with no rebuild: shape, size in units, param.
pub const STARTER: [(Shape, [u16; 3], u16); 24] = [
    (Shape::Block, [32, 32, 32], 0),
    (Shape::Block, [64, 64, 64], 0),
    (Shape::Block, [128, 128, 128], 0),
    (Shape::Block, [256, 256, 256], 0),
    (Shape::Block, [128, 64, 64], 0),
    (Shape::Wall, [64, 16, 128], 0),
    (Shape::Wall, [128, 16, 128], 0),
    (Shape::Wall, [256, 16, 128], 0),
    (Shape::Wall, [256, 16, 256], 0),
    (Shape::FloorPlate, [64, 64, 16], 0),
    (Shape::FloorPlate, [128, 128, 16], 0),
    (Shape::FloorPlate, [256, 256, 16], 0),
    (Shape::FloorPlate, [512, 512, 16], 0),
    (Shape::Ramp, [128, 128, 64], 0),
    (Shape::Ramp, [256, 128, 64], 0),
    (Shape::Ramp, [128, 128, 128], 0),
    (Shape::Wedge, [128, 128, 64], 0),
    (Shape::Wedge, [128, 128, 128], 0),
    (Shape::Stairs, [128, 64, 64], 4),
    (Shape::Stairs, [256, 64, 128], 8),
    (Shape::Cylinder, [64, 64, 128], 16),
    (Shape::Cylinder, [128, 128, 128], 24),
    (Shape::Arch, [256, 32, 192], 8),
    (Shape::Arch, [128, 32, 128], 8),
];

/// The starter set as dev definitions, in the order of [`STARTER`].
pub fn starter() -> Vec<PieceDef> {
    STARTER
        .iter()
        .map(|&(shape, size, param)| PieceDef::dev(shape, size, param))
        .collect()
}

#[cfg(test)]
mod tests;
