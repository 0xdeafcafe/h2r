//! Block-out units: one hundredth of a world unit (docs/game/forge-blockout.md,
//! "Units and the grid").

/// The smallest size of a piece on an axis, in units.
pub const MIN: u16 = 4;
/// The largest size of a piece on an axis, in units.
pub const MAX: u16 = 2048;
/// The block-out grids, in units. Every one is a power of two.
pub const GRIDS: [u16; 7] = [1, 2, 4, 8, 16, 32, 64];
/// The grid a new profile starts with.
pub const DEFAULT_GRID: u16 = 16;
/// How many units one dev texture tile covers; texture coordinates are
/// positions in units divided by it.
pub const TILE: f32 = 256.0;

/// A length in world units: one IEEE division, the same bits on every host.
pub fn world(units: u16) -> f32 {
    f32::from(units) / 100.0
}

/// `total * i / n` units as world units, with one rounding after the
/// integer product so every face that names the point gets the same bits.
pub(crate) fn fraction(total: u16, i: u16, n: u16) -> f32 {
    let product = u32::from(total) * u32::from(i);
    (product as f32 / f32::from(n)) / 100.0
}
