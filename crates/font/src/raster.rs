//! CPU text layout and rasterisation into straight-alpha RGBA8.
//!
//! Layout follows the engine's measuring and drawing routines (`0x13ef40`,
//! `0x13f700`): the glyph bitmap is drawn at `pen_x + kerning + x_offset`,
//! `baseline - y_offset`, and the pen then moves by
//! `kerning + x_offset + advance`. Lines are `ascent + descent + leading` tall.
//! Not implemented: the engine's inline markup (`|` escapes), tab stops,
//! alignment and the private-use icon range; those code units are drawn as
//! ordinary characters (or skipped when they are control codes).
use std::collections::HashMap;

use crate::{Error, Font, Glyph, Result};

/// Longest edge of a rendered bitmap.
pub const MAX_EDGE: i64 = 16_384;
/// Most pixels in a rendered bitmap.
pub const MAX_PIXELS: i64 = 16 * 1024 * 1024;
/// Most decoded glyph pixels held at once (2 bytes each) while rendering.
pub const MAX_CACHED_GLYPH_PIXELS: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct RenderOptions {
    /// Text colour; glyph coverage is multiplied into its alpha.
    pub color: [u8; 4],
    /// Use each pixel's own 12-bit colour (set by literal words in the glyph
    /// stream, so coloured glyphs such as icons keep their colours) instead of
    /// `color`; `color`'s alpha still scales coverage.
    pub glyph_colors: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            color: [255, 255, 255, 255],
            glyph_colors: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, straight (non-premultiplied) alpha.
    pub rgba: Vec<u8>,
}

struct Placed {
    x: i64,
    y: i64,
    index: u32,
}

/// Lay out `text` (UTF-16 code units) in `font` and rasterise it.
///
/// `\r`, `\n` and `\r\n` start a new line; other control units below 0x20 are
/// skipped. A unit without a mapping draws record 0, as the engine does. The
/// result always covers every pen position and glyph, so it can start at a
/// negative offset only via a transparent margin, never be clipped.
pub fn render(font: &Font<'_>, text: &[u16], opts: RenderOptions) -> Result<Bitmap> {
    let lh = i64::from(font.line_height()).max(1);
    let ascent = i64::from(font.header.ascent);
    let mut placed: Vec<Placed> = Vec::new();
    let mut cache: HashMap<u32, Glyph> = HashMap::new();
    let mut cached_pixels = 0usize;
    let (mut line, mut pen, mut prev) = (0i64, 0i64, 0u16);
    let (mut min_x, mut max_x) = (0i64, 0i64);
    let mut i = 0;
    while i < text.len() {
        let u = text[i];
        i += 1;
        if u == 0x0d || u == 0x0a {
            if u == 0x0d && text.get(i) == Some(&0x0a) {
                i += 1;
            }
            line += 1;
            pen = 0;
            prev = 0;
            continue;
        }
        if u < 0x20 {
            continue;
        }
        let index = font.glyph_index(u);
        if let std::collections::hash_map::Entry::Vacant(v) = cache.entry(index) {
            let rec = font.record(index)?;
            cached_pixels += usize::from(rec.width) * usize::from(rec.height);
            if cached_pixels > MAX_CACHED_GLYPH_PIXELS {
                return Err(Error::TooLarge);
            }
            v.insert(font.glyph_by_index(index)?);
        }
        let r = cache[&index].record;
        let kern = i64::from(font.kerning(prev, u));
        let x = pen + kern + i64::from(r.x_offset);
        let y = line * lh + ascent - i64::from(r.y_offset);
        pen = x + i64::from(r.advance);
        prev = u;
        min_x = min_x.min(x).min(pen);
        max_x = max_x.max(pen).max(x + i64::from(r.width));
        placed.push(Placed { x, y, index });
        if placed.len() > 1 << 20 || max_x - min_x > MAX_EDGE {
            return Err(Error::TooLarge);
        }
    }
    let mut min_y = 0i64;
    let mut max_y = (line + 1) * lh;
    for p in &placed {
        let r = cache[&p.index].record;
        min_y = min_y.min(p.y);
        max_y = max_y.max(p.y + i64::from(r.height));
    }
    let (w, h) = (max_x - min_x, max_y - min_y);
    if w > MAX_EDGE || h > MAX_EDGE || w * h > MAX_PIXELS {
        return Err(Error::TooLarge);
    }
    let (w, h) = (w.max(1), h.max(1));
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let [cr, cg, cb, ca] = opts.color;
    for p in &placed {
        let g = &cache[&p.index];
        let gw = i64::from(g.record.width);
        for (n, &px) in g.pixels.iter().enumerate() {
            let a4 = u32::from(px >> 12);
            if a4 == 0 {
                continue;
            }
            let sa = a4 * 17 * u32::from(ca) / 255;
            if sa == 0 {
                continue;
            }
            let (gx, gy) = ((n as i64) % gw, (n as i64) / gw);
            let (x, y) = (p.x + gx - min_x, p.y + gy - min_y);
            let o = ((y * w + x) * 4) as usize;
            let src = if opts.glyph_colors {
                let c = u32::from(px);
                [((c >> 8) & 15) * 17, ((c >> 4) & 15) * 17, (c & 15) * 17]
            } else {
                [u32::from(cr), u32::from(cg), u32::from(cb)]
            };
            // Straight-alpha "over": the destination is whatever was drawn before.
            let da = u32::from(rgba[o + 3]);
            let oa = sa + da * (255 - sa) / 255;
            for k in 0..3 {
                let dst = u32::from(rgba[o + k]);
                rgba[o + k] = ((src[k] * sa + dst * da * (255 - sa) / 255) / oa) as u8;
            }
            rgba[o + 3] = oa as u8;
        }
    }
    Ok(Bitmap {
        width: w as u32,
        height: h as u32,
        rgba,
    })
}
