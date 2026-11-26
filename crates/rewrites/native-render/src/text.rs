//! Text shaping into our own glyph atlas (docs/graphics/framescene.md, 4.2).
//!
//! A [`FontFace`] holds what a font tag says about its glyphs (metrics and
//! coverage bitmaps) by value, built once at map load by whoever parses the
//! tag; the guest's glyph cache is never read. [`FontFace::shape`] turns a
//! string into the pens of one [`OverlayGlyph`] run (the font engine's
//! rule: the bitmap is drawn at `pen + kerning + x_offset`, `baseline -
//! y_offset`, and the pen then moves by `kerning + x_offset + advance`),
//! and [`FontFace::pack`] puts every glyph into the [`GlyphPages`] once.
//! The run is pushed with `Overlay::push_text_run`, so it is one draw.
//! CPU only: nothing here touches the GPU.

use std::collections::HashMap;

use frame_scene::OverlayGlyph;

use crate::overlay::GlyphPages;

/// One glyph of a font tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceGlyph {
    /// The glyph's index in the font's own table: the atlas key.
    pub index: u32,
    pub advance: i16,
    pub x_offset: i16,
    pub y_offset: i16,
    pub width: u16,
    pub height: u16,
    /// One coverage byte per texel, `width * height`, row-major.
    pub coverage: Vec<u8>,
}

/// A font's metrics and glyphs, keyed by UTF-16 code unit.
#[derive(Clone, Debug, Default)]
pub struct FontFace {
    /// The engine font this face fills in the atlas.
    pub font: u16,
    pub ascent: i16,
    pub descent: i16,
    pub leading: i16,
    pub glyphs: HashMap<u16, FaceGlyph>,
    /// `(first, second)` to the pen change between them.
    pub kerning: HashMap<(u16, u16), i16>,
}

/// Why a string was not shaped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeError {
    /// The font has no glyph for this unit: named, never substituted.
    MissingGlyph(u16),
}

impl FontFace {
    /// `ascent + descent + leading`: the height of one line.
    pub fn line_height(&self) -> i32 {
        i32::from(self.ascent) + i32::from(self.descent) + i32::from(self.leading)
    }

    /// The pens of `text` with its first line's top left at `origin` (guest
    /// pixels), every length times `scale`. A `\n` starts the next line.
    /// Glyphs with no bitmap move the pen and draw nothing.
    pub fn shape(&self, text: &str, origin: [f32; 2], scale: f32) -> Result<Vec<OverlayGlyph>, ShapeError> {
        let mut out = Vec::new();
        let mut line = 0i32;
        let mut pen = 0i32;
        let mut previous: Option<u16> = None;
        for unit in text.encode_utf16() {
            if unit == u16::from(b'\n') {
                line += 1;
                pen = 0;
                previous = None;
                continue;
            }
            let glyph = self.glyphs.get(&unit).ok_or(ShapeError::MissingGlyph(unit))?;
            let kern = previous.map_or(0, |p| i32::from(self.kerning.get(&(p, unit)).copied().unwrap_or(0)));
            if glyph.width > 0 && glyph.height > 0 {
                let x = pen + kern + i32::from(glyph.x_offset);
                let y = line * self.line_height() + i32::from(self.ascent) - i32::from(glyph.y_offset);
                let (x0, y0) = (origin[0] + x as f32 * scale, origin[1] + y as f32 * scale);
                out.push(OverlayGlyph {
                    index: glyph.index,
                    rect: [x0, y0, x0 + f32::from(glyph.width) * scale, y0 + f32::from(glyph.height) * scale],
                });
            }
            pen += kern + i32::from(glyph.x_offset) + i32::from(glyph.advance);
            previous = Some(unit);
        }
        Ok(out)
    }

    /// Puts every glyph into `pages` under this face's font; false when
    /// one did not fit or its coverage was the wrong size.
    pub fn pack(&self, pages: &mut GlyphPages) -> bool {
        let mut units: Vec<_> = self.glyphs.keys().copied().collect();
        units.sort_unstable();
        units.iter().all(|u| {
            let g = &self.glyphs[u];
            pages.insert(self.font, g.index, g.width, g.height, &g.coverage)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(index: u32, advance: i16, x_offset: i16, y_offset: i16, w: u16, h: u16) -> FaceGlyph {
        FaceGlyph { index, advance, x_offset, y_offset, width: w, height: h, coverage: vec![200; usize::from(w) * usize::from(h)] }
    }

    fn face() -> FontFace {
        let mut f = FontFace { font: 4, ascent: 10, descent: 3, leading: 1, ..FontFace::default() };
        f.glyphs.insert(u16::from(b'A'), glyph(1, 6, 1, 9, 5, 9));
        f.glyphs.insert(u16::from(b'V'), glyph(2, 7, 0, 10, 7, 10));
        f.glyphs.insert(u16::from(b' '), glyph(0, 4, 0, 0, 0, 0));
        f.kerning.insert((u16::from(b'A'), u16::from(b'V')), -2);
        f
    }

    #[test]
    fn shaped_pens_match_the_font_metrics() {
        let f = face();
        assert_eq!(f.line_height(), 14);
        let run = f.shape("AV A", [100.0, 50.0], 1.0).unwrap();
        assert_eq!(run.len(), 3, "the space draws nothing");
        // A: pen 0, x 0 + 1, top at ascent - y_offset = 1.
        assert_eq!(run[0].rect, [101.0, 51.0, 106.0, 60.0]);
        // The pen is 1 + 6 = 7; V kerns -2: x = 5, top at 0.
        assert_eq!(run[1].rect, [105.0, 50.0, 112.0, 60.0]);
        // The pen is 5 + 0 + 7 = 12, the space moves it 4 to 16, A at 17.
        assert_eq!(run[2].rect, [117.0, 51.0, 122.0, 60.0]);
        let doubled = f.shape("AV", [0.0, 0.0], 2.0).unwrap();
        assert_eq!(doubled[1].rect, [10.0, 0.0, 24.0, 20.0]);
        let lines = f.shape("A\nA", [0.0, 0.0], 1.0).unwrap();
        assert_eq!(lines[1].rect[1] - lines[0].rect[1], 14.0);
        assert_eq!(f.shape("AQ", [0.0; 2], 1.0), Err(ShapeError::MissingGlyph(u16::from(b'Q'))));
    }

    #[test]
    fn packing_puts_every_glyph_in_its_fonts_page() {
        let mut pages = GlyphPages::new(64);
        assert!(face().pack(&mut pages));
        assert!(pages.has(4, 1) && pages.has(4, 2));
        let mut broken = face();
        broken.glyphs.get_mut(&u16::from(b'A')).unwrap().coverage.pop();
        assert!(!broken.pack(&mut GlyphPages::new(64)));
    }
}
