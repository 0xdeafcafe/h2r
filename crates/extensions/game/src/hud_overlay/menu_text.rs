//! Shapes menu labels from the same font records their overlay pages contain.
use font::Font;
use frame_scene::OverlayGlyph;
use render_bridge::native_hud::ui_overlay::{Justify, UiText};

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
        shape(&face, units, rect, alignment)
    }
}

fn origin(left: i32, room: i32, line: i32, alignment: Justify) -> i32 {
    left + match alignment {
        Justify::Left => 0,
        Justify::Right => room - line,
        Justify::Centre => (room - line) / 2,
    }
}

/// The line scanner handles private-use substitutions, tabs and inline
/// formatting separately. The ordinary label path accepts printable ASCII
/// and the engine's line separator, including its CRLF spelling.
fn shape(
    font: &Font<'_>,
    units: &[u16],
    rect: [f32; 4],
    alignment: Justify,
) -> Option<Vec<OverlayGlyph>> {
    if !units
        .iter()
        .all(|u| matches!(*u, 10 | 13 | 32..=123 | 125..=126))
        || !rect
            .iter()
            .all(|v| v.is_finite() && v.fract() == 0.0 && v.abs() < 65536.0)
    {
        return None;
    }
    let [left, top, right, bottom] = rect.map(|v| v as i32);
    let room = right - left;
    let string = String::from_utf16(units).ok()?.replace('\n', "");
    let record = |c: char| font.record(font.glyph_index(c as u16)).ok();
    // Prove the metrics before entering the scanner's infallible callback.
    for c in string.chars().filter(|&c| c != '\r') {
        record(c)?;
    }
    let kern = |prev: Option<char>, c: char| {
        prev.map_or(0, |p| i32::from(font.kerning(p as u16, c as u16)))
    };
    let lines = text_layout::lines(&string, room, &|prev, c| {
        let r = record(c).expect("validated label record");
        kern(prev, c) + i32::from(r.x_offset) + i32::from(r.advance)
    });
    let ascent = i32::from(font.header.ascent);
    let shown = text_layout::lines_drawn(bottom - top, ascent, font.line_height());
    let mut glyphs = Vec::new();
    for (n, line) in lines.iter().take(shown).enumerate() {
        let baseline = top + ascent + n as i32 * font.line_height();
        let mut x = origin(left, room, line.width, alignment);
        let mut prev = None;
        for c in line.text.chars() {
            let r = record(c)?;
            x += kern(prev, c) + i32::from(r.x_offset);
            let y = baseline - i32::from(r.y_offset);
            if r.width != 0 && r.height != 0 {
                glyphs.push(OverlayGlyph {
                    index: font.glyph_index(c as u16),
                    rect: [
                        x as f32,
                        y as f32,
                        (x + i32::from(r.width)) as f32,
                        (y + i32::from(r.height)) as f32,
                    ],
                });
            }
            prev = Some(c);
            x += i32::from(r.advance);
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
        let label: Vec<u16> = "x x".encode_utf16().collect();
        let run = shape(&face, &label, [10., 20., 14., 27.], Justify::Right).unwrap();
        assert_eq!(
            run,
            [OverlayGlyph {
                index: 1,
                rect: [11., 21., 12., 22.]
            }]
        );
        let run = shape(&face, &label, [10., 20., 14., 28.], Justify::Right).unwrap();
        assert_eq!(run[1].rect, [12., 25., 13., 26.]);
        assert!(shape(&face, &[0xe501], [0.; 4], Justify::Left).is_none());
    }

    #[test]
    fn aligns_measured_lines_individually_in_an_offset_rectangle() {
        assert_eq!(origin(19, 31, 12, Justify::Left), 19);
        assert_eq!(origin(19, 31, 12, Justify::Right), 38);
        assert_eq!(origin(19, 31, 12, Justify::Centre), 28);
        assert_eq!(origin(19, 31, 35, Justify::Centre), 17);
    }
}
