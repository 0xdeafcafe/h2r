//! The label plate of a rectangular face.
//!
//! Three rows of 16 cells, centred on the face: the shape's name, the
//! face's size in units, and the same in metres to two places. A cell is
//! `h` units square, `h` the largest of 16, 8, 4 and 2 with the plate
//! inside the face and a margin of `h / 2`. A face too small for `h` = 2,
//! or whose text is longer than 16 cells, has no plate.
//!
//! Coordinates are the face's own: x along, y up, the origin at its
//! lower-left corner. The caller maps them (up on a wall, along x on a
//! floor). The cells are geometry: the render mesh cuts a quad out of the
//! face for each, mapped to its glyph's cell in the font atlas.

use crate::font;

pub const ROWS: usize = 3;
pub const COLUMNS: usize = 16;
/// The plate's cell sizes, largest first.
pub const SIZES: [u32; 4] = [16, 8, 4, 2];
/// Centimetres in one unit (3.048), as thousandths.
const CM_PER_UNIT_THOUSANDTHS: u64 = 3048;

/// The plate of a face.
#[derive(Clone, Debug, PartialEq)]
pub struct Plate {
    /// A cell's side in units.
    pub h: u32,
    /// The plate's left edge on the face.
    pub left: f32,
    /// The plate's lower edge on the face.
    pub bottom: f32,
    /// The font cell index of each cell, row 0 on top.
    pub glyphs: [[u8; COLUMNS]; ROWS],
}

impl Plate {
    /// The cell's rectangle on the face, `[x0, y0, x1, y1]`.
    pub fn cell_rect(&self, row: usize, col: usize) -> [f32; 4] {
        let h = self.h as f32;
        let x0 = self.left + col as f32 * h;
        let y0 = self.bottom + (ROWS - 1 - row) as f32 * h;
        [x0, y0, x0 + h, y0 + h]
    }

    /// The cell's atlas UVs ([`font::cell_uv`]).
    pub fn cell_uv(&self, row: usize, col: usize) -> [f32; 4] {
        font::cell_uv(self.glyphs[row][col])
    }

    /// The plate's text, a row a string, spaces at the ends trimmed.
    pub fn text(&self) -> [String; ROWS] {
        self.glyphs.map(|row| {
            row.iter()
                .map(|&g| font::char_of(g).unwrap_or('?'))
                .collect::<String>()
                .trim()
                .to_string()
        })
    }
}

/// Hundredths of a metre in `units`, rounded half up.
fn hundredths(units: u32) -> u64 {
    (units as u64 * CM_PER_UNIT_THOUSANDTHS + 500) / 1000
}

fn metres(units: u32) -> String {
    let c = hundredths(units);
    format!("{}.{:02}", c / 100, c % 100)
}

/// The three rows of text for a face. The shape's name is cut at 16.
pub fn rows(shape: &str, width: u32, height: u32) -> [String; ROWS] {
    let name: String = shape.chars().take(COLUMNS).collect();
    [
        name,
        format!("{width} {} {height} U", font::TIMES),
        format!("{} {} {} M", metres(width), font::TIMES, metres(height)),
    ]
}

fn fits(h: u32, width: u32, height: u32) -> bool {
    let margin = h / 2;
    COLUMNS as u32 * h + 2 * margin <= width && ROWS as u32 * h + 2 * margin <= height
}

/// The plate of a `width` by `height` face of `shape`, or `None`.
pub fn layout(shape: &str, width: u32, height: u32) -> Option<Plate> {
    let h = SIZES.iter().copied().find(|&h| fits(h, width, height))?;
    let mut glyphs = [[0u8; COLUMNS]; ROWS];
    for (row, text) in rows(shape, width, height).iter().enumerate() {
        let chars: Vec<char> = text.chars().collect();
        if chars.len() > COLUMNS {
            return None;
        }
        let pad = (COLUMNS - chars.len()) / 2;
        for (i, c) in chars.iter().enumerate() {
            glyphs[row][pad + i] = font::index_of(*c);
        }
    }
    Some(Plate {
        h,
        left: (width - COLUMNS as u32 * h) as f32 / 2.0,
        bottom: (height - ROWS as u32 * h) as f32 / 2.0,
        glyphs,
    })
}

/// Reads a plate's text back from the font raster: each cell's glyph is
/// matched from the atlas's texels, not looked up. A row's end spaces are
/// trimmed; a cell no glyph matches reads as `?`.
pub fn read_label(atlas: &crate::Image, plate: &Plate) -> [String; ROWS] {
    plate.glyphs.map(|row| {
        row.iter()
            .map(|&g| font::read_cell(atlas, g).unwrap_or('?'))
            .collect::<String>()
            .trim()
            .to_string()
    })
}
