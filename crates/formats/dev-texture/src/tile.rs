//! The grid tiles.
//!
//! One 512 by 512 tile covers 256 units, two texels a unit. A checker of
//! 32-unit squares in two tones, a faint line every 16 units, a stronger
//! one every 64 and a heavy one every 256, which is the tile's own edge.
//! Each line is centred on its coordinate, so a tile wraps without a seam:
//! its first and last rows and columns are equal.

use crate::{Family, Image};

pub const TILE_SIZE: u32 = 512;
pub const TEXELS_PER_UNIT: u32 = 2;
pub const TILE_UNITS: u32 = TILE_SIZE / TEXELS_PER_UNIT;

const SQUARE: u32 = 32 * TEXELS_PER_UNIT;

fn scale(c: [u8; 3], n: u32) -> [u8; 3] {
    [
        (c[0] as u32 * n / 256) as u8,
        (c[1] as u32 * n / 256) as u8,
        (c[2] as u32 * n / 256) as u8,
    ]
}

/// True when texel `c` is within a line `w` texels wide centred on a
/// multiple of `step`: the `w / 2` texels before it and after it.
fn on(c: u32, step: u32, w: u32) -> bool {
    (c + w / 2) % step < w
}

/// 0 for no line, 1 faint (16 units), 2 strong (64), 3 heavy (256).
pub(crate) fn level(c: u32) -> u8 {
    if on(c, TILE_SIZE, 6) {
        3
    } else if on(c, 64 * TEXELS_PER_UNIT, 4) {
        2
    } else if on(c, 16 * TEXELS_PER_UNIT, 2) {
        1
    } else {
        0
    }
}

/// The colours of the three line levels of a family.
pub fn line_colours(family: Family) -> [[u8; 3]; 3] {
    let (_, dark) = family.tones();
    [scale(dark, 215), scale(dark, 150), scale(dark, 80)]
}

/// Draws the tile of a family.
pub fn tile(family: Family) -> Image {
    let (light, dark) = family.tones();
    let lines = line_colours(family);
    let mut image = Image::filled(TILE_SIZE, TILE_SIZE, light);
    for y in 0..TILE_SIZE {
        let ly = level(y);
        for x in 0..TILE_SIZE {
            let lvl = ly.max(level(x));
            let colour = if lvl > 0 {
                lines[lvl as usize - 1]
            } else if (x / SQUARE + y / SQUARE) % 2 == 0 {
                light
            } else {
                dark
            };
            image.put(x, y, colour);
        }
    }
    image
}
