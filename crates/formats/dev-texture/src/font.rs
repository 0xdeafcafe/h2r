//! The font: a 5 by 7 pixel face of ours, drawn from the table below.
//!
//! Capitals, digits, the point, the times sign and a blank, in a 128 by 128
//! atlas of 16-texel cells. Each cell is a dark plate with the glyph
//! on it at twice its size, so a row of cells reads as one plate. The
//! glyph shapes are original to this repository.

use crate::Image;

pub const ATLAS_SIZE: u32 = 128;
pub const CELL: u32 = 16;
pub const CELLS_PER_ROW: u32 = ATLAS_SIZE / CELL;
/// The times sign between a face's two sizes.
pub const TIMES: char = '\u{d7}';

const BACKGROUND: [u8; 3] = [22, 26, 38];
const INK: [u8; 3] = [245, 245, 235];
/// Glyph size on the cell, in texels per font pixel.
const SCALE: u32 = 2;
const INSET_X: u32 = (CELL - 5 * SCALE) / 2;
const INSET_Y: u32 = (CELL - 7 * SCALE) / 2;

/// The cell index of a character, or the blank's for one the font lacks.
/// Lower case draws as capitals.
pub fn index_of(c: char) -> u8 {
    match c.to_ascii_uppercase() {
        d @ '0'..='9' => 1 + (d as u8 - b'0'),
        l @ 'A'..='Z' => 11 + (l as u8 - b'A'),
        '.' => 37,
        TIMES => 38,
        _ => 0,
    }
}

/// The character of a cell index; `None` past the glyphs.
pub fn char_of(index: u8) -> Option<char> {
    match index {
        0 => Some(' '),
        1..=10 => Some((b'0' + index - 1) as char),
        11..=36 => Some((b'A' + index - 11) as char),
        37 => Some('.'),
        38 => Some(TIMES),
        _ => None,
    }
}

/// The atlas UVs of a cell, `[u0, v0, u1, v1]`, v down. Exact in `f32`.
pub fn cell_uv(index: u8) -> [f32; 4] {
    let (cx, cy) = (index as u32 % CELLS_PER_ROW, index as u32 / CELLS_PER_ROW);
    let s = ATLAS_SIZE as f32;
    [
        (cx * CELL) as f32 / s,
        (cy * CELL) as f32 / s,
        ((cx + 1) * CELL) as f32 / s,
        ((cy + 1) * CELL) as f32 / s,
    ]
}

/// Draws the atlas.
pub fn font() -> Image {
    let mut image = Image::filled(ATLAS_SIZE, ATLAS_SIZE, BACKGROUND);
    for (index, rows) in GLYPHS.iter().enumerate() {
        let ox = (index as u32 % CELLS_PER_ROW) * CELL + INSET_X;
        let oy = (index as u32 / CELLS_PER_ROW) * CELL + INSET_Y;
        for (gy, row) in rows.iter().enumerate() {
            for (gx, b) in row.bytes().enumerate() {
                if b != b'#' {
                    continue;
                }
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        image.put(
                            ox + gx as u32 * SCALE + dx,
                            oy + gy as u32 * SCALE + dy,
                            INK,
                        );
                    }
                }
            }
        }
    }
    image
}

/// Reads the glyph of a cell back from the raster: the 5 by 7 samples are
/// matched against the table. `None` when no glyph matches, so a damaged
/// atlas is noticed.
pub fn read_cell(atlas: &Image, index: u8) -> Option<char> {
    let ox = (index as u32 % CELLS_PER_ROW) * CELL + INSET_X;
    let oy = (index as u32 / CELLS_PER_ROW) * CELL + INSET_Y;
    let mut seen = [[false; 5]; 7];
    for (gy, row) in seen.iter_mut().enumerate() {
        for (gx, set) in row.iter_mut().enumerate() {
            *set = atlas.pixel(ox + gx as u32 * SCALE, oy + gy as u32 * SCALE)[0] > 128;
        }
    }
    GLYPHS
        .iter()
        .position(|rows| {
            rows.iter().enumerate().all(|(gy, row)| {
                row.bytes()
                    .enumerate()
                    .all(|(gx, b)| (b == b'#') == seen[gy][gx])
            })
        })
        .and_then(|i| char_of(i as u8))
}

/// The glyphs in cell order: blank, `0` to `9`, `A` to `Z`, `.`, times.
pub const GLYPHS: [[&str; 7]; 39] = [
    [".....", ".....", ".....", ".....", ".....", ".....", "....."],
    [".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###."],
    ["..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###."],
    [".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####"],
    [".###.", "#...#", "....#", "..##.", "....#", "#...#", ".###."],
    ["...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#."],
    ["#####", "#....", "####.", "....#", "....#", "#...#", ".###."],
    ["..##.", ".#...", "#....", "####.", "#...#", "#...#", ".###."],
    ["#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#..."],
    [".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###."],
    [".###.", "#...#", "#...#", ".####", "....#", "...#.", ".##.."],
    [".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"],
    ["####.", "#...#", "#...#", "####.", "#...#", "#...#", "####."],
    [".###.", "#...#", "#....", "#....", "#....", "#...#", ".###."],
    ["####.", "#...#", "#...#", "#...#", "#...#", "#...#", "####."],
    ["#####", "#....", "#....", "####.", "#....", "#....", "#####"],
    ["#####", "#....", "#....", "####.", "#....", "#....", "#...."],
    [".###.", "#...#", "#....", "#.###", "#...#", "#...#", ".####"],
    ["#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"],
    [".###.", "..#..", "..#..", "..#..", "..#..", "..#..", ".###."],
    ["..###", "...#.", "...#.", "...#.", "...#.", "#..#.", ".##.."],
    ["#...#", "#..#.", "#.#..", "##...", "#.#..", "#..#.", "#...#"],
    ["#....", "#....", "#....", "#....", "#....", "#....", "#####"],
    ["#...#", "##.##", "#.#.#", "#.#.#", "#...#", "#...#", "#...#"],
    ["#...#", "##..#", "#.#.#", "#..##", "#...#", "#...#", "#...#"],
    [".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."],
    ["####.", "#...#", "#...#", "####.", "#....", "#....", "#...."],
    [".###.", "#...#", "#...#", "#...#", "#.#.#", "#..#.", ".##.#"],
    ["####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#"],
    [".####", "#....", "#....", ".###.", "....#", "....#", "####."],
    ["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."],
    ["#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."],
    ["#...#", "#...#", "#...#", "#...#", "#...#", ".#.#.", "..#.."],
    ["#...#", "#...#", "#...#", "#.#.#", "#.#.#", "##.##", "#...#"],
    ["#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#"],
    ["#...#", "#...#", ".#.#.", "..#..", "..#..", "..#..", "..#.."],
    ["#####", "....#", "...#.", "..#..", ".#...", "#....", "#####"],
    [".....", ".....", ".....", ".....", ".....", "..##.", "..##."],
    [".....", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "....."],
];
