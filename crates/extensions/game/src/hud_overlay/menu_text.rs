//! Shapes menu labels from the same font records their overlay pages contain.
use font::Font;
use frame_scene::OverlayGlyph;
use render_bridge::native_hud::ui_overlay::{BUTTON_GLYPHS, Justify, TextLayout, UiText};

use crate::ui_kit::preview::text_layout;

pub(crate) struct MenuText(pub(crate) std::sync::Arc<hud_edit::render::Fonts>);

impl UiText for MenuText {
    fn shape(
        &self,
        font: u16,
        units: &[u16],
        rect: [f32; 4],
        alignment: Justify,
    ) -> Option<Vec<OverlayGlyph>> {
        let face = self.0.font(i16::try_from(font).ok()?)?;
        shape(&face, units, rect, alignment, false, &TextLayout::default())
    }

    fn shape_buttons(
        &self,
        font: u16,
        units: &[u16],
        rect: [f32; 4],
        alignment: Justify,
    ) -> Option<Vec<OverlayGlyph>> {
        let face = self.0.font(i16::try_from(font).ok()?)?;
        shape(&face, units, rect, alignment, true, &TextLayout::default())
    }

    fn shape_layout(
        &self,
        font: u16,
        units: &[u16],
        rect: [f32; 4],
        alignment: Justify,
        layout: &TextLayout,
    ) -> Option<Vec<OverlayGlyph>> {
        let face = self.0.font(i16::try_from(font).ok()?)?;
        shape(&face, units, rect, alignment, true, layout)
    }

    fn glyph(&self, font: u16, unit: u16) -> Option<u32> {
        Some(self.0.font(i16::try_from(font).ok()?)?.glyph_index(unit))
    }
}

/// The line renderer `0x0013f0e0` over one font: the scanner's
/// characters (basic Latin and alphabetic European text), its line
/// separator in its CRLF spelling too, tabs and the alignment units
/// (`text_layout::segments`); inline markup (`|`, `0x0013fbd0`) and other
/// controls are not laid out. With `buttons`, a button glyph is a
/// character too: the scanner takes it as any other (`0x0013f5a0`: type
/// 4).
///
/// Every metric is times the layout's scale (`0x0013ef40`, `0x0013f700`):
/// a glyph's place is the pen plus its x offset and kerning, its top the
/// baseline less its y offset, its quad its size times the scale
/// (`0x0001f3a0`), and the pen moves on by its advance, x offset and
/// kerning. Row `n`'s baseline is the rectangle's top plus `n` line
/// heights and the ascent, times the scale; a row is drawn while the
/// rectangle's bottom is below it (`0x0013f331`).
fn shape(
    font: &Font<'_>,
    units: &[u16],
    rect: [f32; 4],
    alignment: Justify,
    buttons: bool,
    layout: &TextLayout,
) -> Option<Vec<OverlayGlyph>> {
    let button = |u: &u16| buttons && BUTTON_GLYPHS.contains(u);
    let laid = |u: &u16| matches!(*u, 9 | 10 | 13 | 32..=123 | 125..=126 | 0x00a0..=0x052f | 0xe405..=0xe407);
    if font.header.unknown[0] != 0
        || !units.iter().all(|u| laid(u) || button(u))
        || !rect.iter().all(|v| v.is_finite() && v.fract() == 0.0 && v.abs() < 65536.0)
        || !layout.scale.is_finite()
    {
        return None;
    }
    let [left, top, right, bottom] = rect;
    let string = String::from_utf16(units).ok()?;
    let drawn = |c: char| !matches!(c, '\r' | '\n' | '\t' | '\u{e405}'..='\u{e407}');
    let record = |c: char| font.record(font.glyph_index(c as u16)).ok();
    // Prove the metrics before entering the scanner's infallible callback.
    for c in string.chars().filter(|&c| drawn(c)) {
        record(c)?;
    }
    let s = layout.scale;
    let kern = |prev: Option<char>, c: char| prev.map_or(0, |p| font.kerning(p as u16, c as u16));
    let parts = |prev: Option<char>, c: char| {
        let r = record(c).expect("validated label record");
        [f32::from(r.advance) * s, f32::from(r.x_offset) * s, f32::from(kern(prev, c)) * s]
    };
    let frame = text_layout::Frame {
        left,
        right,
        stops: &layout.stops,
        indents: layout.indents,
        scale: s,
        align: match alignment {
            Justify::Left => text_layout::Align::Left,
            Justify::Right => text_layout::Align::Right,
            Justify::Centre => text_layout::Align::Centre,
        },
    };
    let ascent = i32::from(font.header.ascent);
    let mut glyphs = Vec::new();
    for segment in text_layout::segments(&string, &frame, &parts) {
        let baseline = (font.line_height() * segment.row + ascent) as f32 * s + top;
        // `comiss; jbe`: unordered is not drawn.
        let shown = bottom > baseline;
        if !shown {
            continue;
        }
        let mut x = segment.x;
        let mut prev = None;
        for c in segment.text.chars().filter(|&c| drawn(c)) {
            let r = record(c)?;
            let [advance, offset, kerning] = parts(prev, c);
            let gx = x + offset + kerning;
            let y = baseline - f32::from(r.y_offset) * s;
            if r.width != 0 && r.height != 0 {
                glyphs.push(OverlayGlyph {
                    index: font.glyph_index(c as u16),
                    rect: [gx, y, gx + f32::from(r.width) * s, y + f32::from(r.height) * s],
                });
            }
            prev = Some(c);
            x = advance + x + offset + kerning;
        }
    }
    Some(glyphs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_a_label_and_omits_lines_whose_baseline_reaches_the_bottom() {
        let data = font::RECORDS_OFFSET + 2 * font::RECORD_LEN;
        let mut bytes = vec![0; data + 1];
        let h = font::HEADER_OFFSET;
        bytes[h..h + 4].copy_from_slice(&font::MAGIC.to_le_bytes());
        bytes[h + 4] = 3; // ascent
        bytes[h + 6] = 1; // descent
        bytes[h + 12] = 2; // records
        bytes[h + 24] = 1; // pixel stream bytes
        bytes[font::CHARMAP_OFFSET + usize::from(b'x') * 4] = 1;
        bytes[font::RECORDS_OFFSET] = 1; // space advances one pixel
        let r = font::RECORDS_OFFSET + font::RECORD_LEN;
        bytes[r] = 2; // advance
        bytes[r + 2] = 1; // stream length
        bytes[r + 4] = 1; // width
        bytes[r + 6] = 1; // height
        bytes[r + 10] = 2; // distance above baseline
        bytes[r + 12..r + 16].copy_from_slice(&(data as u32).to_le_bytes());
        let face = Font::parse(&bytes).unwrap();
        let plain = TextLayout::default();
        let label: Vec<u16> = "x x".encode_utf16().collect();
        let run = shape(&face, &label, [10., 20., 14., 27.], Justify::Right, false, &plain).unwrap();
        assert_eq!(
            run,
            [OverlayGlyph {
                index: 1,
                rect: [11., 21., 12., 22.]
            }]
        );
        let run = shape(&face, &label, [10., 20., 14., 28.], Justify::Right, false, &plain).unwrap();
        assert_eq!(run[1].rect, [12., 25., 13., 26.]);
        assert!(shape(&face, &[0xe501], [0.; 4], Justify::Left, false, &plain).is_none());
        // A button glyph is a character only where the caller checks the result.
        assert!(shape(&face, &[0xe100], [0.; 4], Justify::Left, false, &plain).is_none());
        assert_eq!(shape(&face, &[0xe100], [0.; 4], Justify::Left, true, &plain), Some(Vec::new()), "the font's empty glyph");
        assert!(shape(&face, &[0xe501], [0.; 4], Justify::Left, true, &plain).is_none(), "not a button");
    }

    #[test]
    fn a_tab_moves_to_its_stop_and_the_scale_multiplies_every_metric() {
        let data = font::RECORDS_OFFSET + 2 * font::RECORD_LEN;
        let mut bytes = vec![0; data + 1];
        let h = font::HEADER_OFFSET;
        bytes[h..h + 4].copy_from_slice(&font::MAGIC.to_le_bytes());
        bytes[h + 4] = 4; // ascent
        bytes[h + 6] = 2; // descent
        bytes[h + 12] = 2; // records
        bytes[h + 24] = 1;
        bytes[font::CHARMAP_OFFSET + usize::from(b'x') * 4] = 1;
        let r = font::RECORDS_OFFSET + font::RECORD_LEN;
        bytes[r] = 4; // advance
        bytes[r + 2] = 1;
        bytes[r + 4] = 2; // width
        bytes[r + 6] = 2; // height
        bytes[r + 8] = 2; // x offset
        bytes[r + 10] = 4; // distance above baseline
        bytes[r + 12..r + 16].copy_from_slice(&(data as u32).to_le_bytes());
        let face = Font::parse(&bytes).unwrap();
        let units: Vec<u16> = "x\tx".encode_utf16().collect();
        let layout = TextLayout { scale: 0.5, stops: vec![200], indents: [0; 2] };
        let run = shape(&face, &units, [10., 20., 300., 60.], Justify::Left, false, &layout).unwrap();
        // Column 0 at the left; column 1 at the stop times the scale. The
        // baseline is the top plus the ascent times the scale.
        assert_eq!(run.iter().map(|g| g.rect).collect::<Vec<_>>(), [[11., 20., 12., 21.], [101., 20., 102., 21.]]);
        assert!(shape(&face, &units, [10., 20., 300., 60.], Justify::Left, false, &TextLayout::default()).is_some(),
            "a tab with no stops ends a segment in the one column");
        let aligned: Vec<u16> = "x\u{e406}x".encode_utf16().collect();
        let run = shape(&face, &aligned, [10., 20., 300., 60.], Justify::Left, false, &TextLayout::default()).unwrap();
        // Right-justified: the pen at 300 less the measured 6, plus the x offset.
        assert_eq!(run[1].rect[0], 296.0);
    }
}
