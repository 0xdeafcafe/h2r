//! The stage's textures, drawn in code.
//!
//! - [`grid`]: the floor, spread once over a bay's slab: a checker of
//!   world units, a line every unit, a heavy line every `label_every`, the
//!   two axes through the bay's origin (x red, y green), distance labels
//!   along both axes, and the pads outlined (the subject pad orange, the
//!   numbered pads blue) with their numbers.
//! - [`ruler`]: one world unit square, tiled over every other face: a line
//!   every tenth, a heavy line at the half and the edge.
//!
//! Labels are drawn ink on a plate in a 5x7 font, so [`read_label`] reads
//! them back exactly; the tests read every label of [`labels`].

use crate::layout::Layout;

/// An RGBA image, rows from the top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes: R, G, B, A.
    pub pixels: Vec<u8>,
}

pub type Rgba = [u8; 4];

const CHECK_LIGHT: Rgba = [0xb4, 0xb4, 0xb4, 0xff];
const CHECK_DARK: Rgba = [0xa4, 0xa4, 0xa4, 0xff];
const LINE: Rgba = [0x80, 0x80, 0x80, 0xff];
const HEAVY: Rgba = [0x50, 0x50, 0x50, 0xff];
const AXIS_X: Rgba = [0xc0, 0x30, 0x30, 0xff];
const AXIS_Y: Rgba = [0x30, 0xa0, 0x30, 0xff];
const SUBJECT: Rgba = [0xe8, 0x80, 0x20, 0xff];
const PAD: Rgba = [0x30, 0x60, 0xd0, 0xff];
const PLATE: Rgba = [0xec, 0xec, 0xec, 0xff];
const INK: Rgba = [0x10, 0x10, 0x10, 0xff];

/// The font's scale: a glyph is 10 by 14 texels.
const SCALE: u32 = 2;
const GLYPH_W: u32 = 5;
const GLYPH_H: u32 = 7;
/// Texels from one glyph's left edge to the next.
const ADVANCE: u32 = (GLYPH_W + 1) * SCALE;
/// Plate margin round a label, texels.
const MARGIN: u32 = SCALE;

/// The 5x7 glyphs: a row a byte, bit 4 the leftmost column.
const FONT: [(char, [u8; 7]); 11] = [
    ('0', [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e]),
    ('1', [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e]),
    ('2', [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f]),
    ('3', [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e]),
    ('4', [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02]),
    ('5', [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e]),
    ('6', [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e]),
    ('7', [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08]),
    ('8', [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e]),
    ('9', [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c]),
    ('-', [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00]),
];

fn glyph(c: char) -> &'static [u8; 7] {
    &FONT
        .iter()
        .find(|(g, _)| *g == c)
        .unwrap_or_else(|| panic!("the stage font has no {c:?}"))
        .1
}

impl Image {
    pub fn new(width: u32, height: u32, fill: Rgba) -> Self {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            pixels.extend_from_slice(&fill);
        }
        Image {
            width,
            height,
            pixels,
        }
    }

    pub fn get(&self, x: u32, y: u32) -> Rgba {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    pub fn set(&mut self, x: u32, y: u32, c: Rgba) {
        if x < self.width && y < self.height {
            let i = ((y * self.width + x) * 4) as usize;
            self.pixels[i..i + 4].copy_from_slice(&c);
        }
    }

    /// Fills `[x0, x1) x [y0, y1)`, clipped.
    fn fill(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, c: Rgba) {
        let (w, h) = (i64::from(self.width), i64::from(self.height));
        for y in y0.max(0)..y1.min(h) {
            for x in x0.max(0)..x1.min(w) {
                self.set(x as u32, y as u32, c);
            }
        }
    }
}

/// Where a world point of a bay (relative to its origin) lands on the grid
/// texture: `(column, row)`, fractional.
pub fn texel(layout: &Layout, x: f32, y: f32) -> (f32, f32) {
    let h = layout.floor.size / 2.0;
    let k = layout.floor.texels_per_unit();
    ((x + h) * k, (h - y) * k)
}

/// A distance label on the grid texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub text: String,
    /// The top-left texel of its first glyph.
    pub column: u32,
    pub row: u32,
}

impl Label {
    fn width(&self) -> u32 {
        self.text.chars().count() as u32 * ADVANCE - SCALE
    }
}

/// A label of `text` centred on a bay point.
fn label_at(layout: &Layout, text: String, x: f32, y: f32) -> Label {
    let (c, r) = texel(layout, x, y);
    let w = text.chars().count() as u32 * ADVANCE - SCALE;
    Label {
        text,
        column: (c - w as f32 / 2.0).round().max(0.0) as u32,
        row: (r - (GLYPH_H * SCALE) as f32 / 2.0).round().max(0.0) as u32,
    }
}

/// Every label the grid carries: the distances along x (just below the x
/// axis), along y (just right of the y axis; none at 0), the subject pad's
/// `0` and each numbered pad's number.
pub fn labels(layout: &Layout) -> Vec<Label> {
    let mut out = Vec::new();
    let h = layout.floor.size / 2.0;
    let step = layout.floor.label_every;
    let n = (h / step).floor() as i32;
    for k in -n..=n {
        let d = step * k as f32;
        if d.abs() >= h - 1.0 {
            continue;
        }
        out.push(label_at(layout, distance(d), d, -0.75));
    }
    for k in -n..=n {
        let d = step * k as f32;
        if k == 0 || d.abs() >= h - 1.0 {
            continue;
        }
        out.push(label_at(layout, distance(d), 1.0, d));
    }
    if let Some(bay) = layout.bays.first() {
        let o = bay.origin;
        let s = bay.subject;
        out.push(label_at(
            layout,
            "0".into(),
            s.point[0] - o[0],
            s.point[1] - o[1],
        ));
        for p in &bay.pads {
            out.push(label_at(
                layout,
                p.number.to_string(),
                p.point[0] - o[0],
                p.point[1] - o[1],
            ));
        }
    }
    out
}

fn distance(d: f32) -> String {
    format!("{}", d.round() as i64)
}

fn draw_label(img: &mut Image, l: &Label) {
    let (x0, y0) = (i64::from(l.column), i64::from(l.row));
    let m = i64::from(MARGIN);
    img.fill(
        x0 - m,
        y0 - m,
        x0 + i64::from(l.width()) + m,
        y0 + i64::from(GLYPH_H * SCALE) + m,
        PLATE,
    );
    for (i, c) in l.text.chars().enumerate() {
        let g = glyph(c);
        let gx = l.column + i as u32 * ADVANCE;
        for (row, bits) in g.iter().enumerate() {
            for col in 0..GLYPH_W {
                if bits & (0x10 >> col) != 0 {
                    let (px, py) = (gx + col * SCALE, l.row + row as u32 * SCALE);
                    for dy in 0..SCALE {
                        for dx in 0..SCALE {
                            img.set(px + dx, py + dy, INK);
                        }
                    }
                }
            }
        }
    }
}

/// Reads `len` glyphs at a label's place back from an image: each glyph
/// cell's ink against every glyph of the font. `None` when a cell matches
/// no glyph.
pub fn read_label(img: &Image, column: u32, row: u32, len: usize) -> Option<String> {
    let mut out = String::new();
    for i in 0..len as u32 {
        let gx = column + i * ADVANCE;
        let mut seen = [0u8; 7];
        for (r, bits) in seen.iter_mut().enumerate() {
            for col in 0..GLYPH_W {
                let (px, py) = (gx + col * SCALE, row + r as u32 * SCALE);
                let mut all = true;
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        if px + dx >= img.width
                            || py + dy >= img.height
                            || img.get(px + dx, py + dy) != INK
                        {
                            all = false;
                        }
                    }
                }
                if all {
                    *bits |= 0x10 >> col;
                }
            }
        }
        let c = FONT.iter().find(|(_, g)| *g == seen)?.0;
        out.push(c);
    }
    Some(out)
}

/// The floor grid (module docs).
pub fn grid(layout: &Layout) -> Image {
    let n = layout.floor.texels;
    let k = layout.floor.texels_per_unit();
    let h = layout.floor.size / 2.0;
    let mut img = Image::new(n, n, CHECK_LIGHT);
    // The checker: world units, dark where the unit's x + y is odd.
    for row in 0..n {
        for col in 0..n {
            let x = (col as f32 + 0.5) / k - h;
            let y = h - (row as f32 + 0.5) / k;
            if (x.floor() as i64 + y.floor() as i64).rem_euclid(2) == 1 {
                img.set(col, row, CHECK_DARK);
            }
        }
    }
    // Unit lines, heavy every `label_every`, then the axes over them.
    let units = layout.floor.size.round() as i64;
    let every = layout.floor.label_every.round().max(1.0) as i64;
    for u in 0..=units {
        let d = u - units / 2;
        let at = (u as f32 * k).round() as i64;
        let (c, w) = if d == 0 {
            continue;
        } else if d % every == 0 {
            (HEAVY, 2)
        } else {
            (LINE, 1)
        };
        img.fill(at - w / 2, 0, at - w / 2 + w, i64::from(n), c);
        img.fill(0, at - w / 2, i64::from(n), at - w / 2 + w, c);
    }
    let mid = (h * k).round() as i64;
    img.fill(0, mid - 1, i64::from(n), mid + 2, AXIS_X);
    img.fill(mid - 1, 0, mid + 2, i64::from(n), AXIS_Y);
    // The pads.
    if let Some(bay) = layout.bays.first() {
        let o = bay.origin;
        let mut outline = |p: &crate::layout::Pad, c: Rgba| {
            let (x, y) = (p.point[0] - o[0], p.point[1] - o[1]);
            let (c0, r0) = texel(layout, x - p.radius, y + p.radius);
            let (c1, r1) = texel(layout, x + p.radius, y - p.radius);
            let (c0, r0, c1, r1) = (
                c0.round() as i64,
                r0.round() as i64,
                c1.round() as i64,
                r1.round() as i64,
            );
            let t = 3;
            img.fill(c0, r0, c1, r0 + t, c);
            img.fill(c0, r1 - t, c1, r1, c);
            img.fill(c0, r0, c0 + t, r1, c);
            img.fill(c1 - t, r0, c1, r1, c);
        };
        outline(&bay.subject, SUBJECT);
        for p in &bay.pads {
            outline(p, PAD);
        }
    }
    for l in labels(layout) {
        draw_label(&mut img, &l);
    }
    img
}

/// Texels per world unit on the ruler.
pub const RULER_TEXELS: u32 = 256;

/// The ruler (module docs): one world unit square.
pub fn ruler() -> Image {
    const BASE: Rgba = [0xd8, 0xc8, 0x60, 0xff];
    const TICK: Rgba = [0x40, 0x40, 0x40, 0xff];
    const EDGE: Rgba = [0x18, 0x18, 0x18, 0xff];
    let n = RULER_TEXELS;
    let mut img = Image::new(n, n, BASE);
    for t in 0..10i64 {
        let at = (t as f32 * n as f32 / 10.0).round() as i64;
        let (c, w) = match t {
            0 => (EDGE, 4),
            5 => (EDGE, 3),
            _ => (TICK, 2),
        };
        // Edge lines straddle the tile's seam, half at each side.
        let start = if t == 0 { -w / 2 } else { at - w / 2 };
        for s in [start, start + i64::from(n)] {
            img.fill(s, 0, s + w, i64::from(n), c);
            img.fill(0, s, i64::from(n), s + w, c);
        }
    }
    img
}
