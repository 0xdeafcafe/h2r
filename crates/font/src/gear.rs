//! The bot mark: a gear at U+2699 (docs/game/bots.md §8.2.2).
//!
//! Halo 2's fonts have no glyph there, so a bot's name (`⚙ ` and its tag)
//! would draw a box. [`replacement`] draws one for a font, sized to the
//! font's capital letters, ready for [`crate::patch::apply`]. The picture is
//! computed from circles and bars here: nothing is copied from a disc font
//! or anyone's artwork.
use std::f32::consts::TAU;

use crate::Font;
use crate::icons::canvas::{Canvas, white};
use crate::patch::Replacement;

/// The code unit: U+2699 GEAR.
pub const GEAR: u16 = 0x2699;
/// The smallest and largest gear drawn, in pixels each way.
pub const MIN_SIZE: u16 = 7;
pub const MAX_SIZE: u16 = 64;

/// The gear for `font`, or `None` when the font draws U+2699 already.
/// Square, as tall as the font's capitals, standing on the baseline, one
/// pixel clear of the character before it.
pub fn replacement(font: &Font<'_>) -> Option<Replacement> {
    if has_glyph(font) {
        return None;
    }
    let size = size(font);
    Some(Replacement {
        unit: GEAR,
        width: size,
        height: size,
        advance: size as i16,
        x_offset: 1,
        y_offset: Some(size as i16),
        pixels: draw(size),
    })
}

/// Whether `font` has its own picture at U+2699: a record of its own (not
/// record 0, which unmapped units take, nor the filler U+FFFF shares) with
/// a bitmap.
pub fn has_glyph(font: &Font<'_>) -> bool {
    let i = font.glyph_index(GEAR);
    i != 0
        && i != font.glyph_index(0xffff)
        && font.record(i).is_ok_and(|r| r.width > 0 && r.height > 0)
}

/// The gear's size in `font`: the height of its 'H' above the baseline
/// (its inked rows), else seven tenths of the ascent; within
/// [`MIN_SIZE`]..=[`MAX_SIZE`].
pub fn size(font: &Font<'_>) -> u16 {
    cap_height(font)
        .unwrap_or_else(|| font.header.ascent.max(0) as u16 * 7 / 10)
        .clamp(MIN_SIZE, MAX_SIZE)
}

fn cap_height(font: &Font<'_>) -> Option<u16> {
    let g = font.glyph(u16::from(b'H')).ok()?;
    let width = usize::from(g.record.width);
    if width == 0 {
        return None;
    }
    let top = g
        .pixels
        .chunks(width)
        .position(|row| row.iter().any(|p| p >> 12 != 0))?;
    u16::try_from(i32::from(g.record.y_offset) - top as i32)
        .ok()
        .filter(|h| *h > 0)
}

/// A white gear `size` pixels square, ARGB4444: a wheel with a hole and
/// eight teeth (six below 14 pixels, where eight run together).
pub fn draw(size: u16) -> Vec<u16> {
    let n = usize::from(size);
    let s = f32::from(size);
    let c = s / 2.0;
    let tip = c - 0.25;
    let wheel = tip * 0.70;
    let hole = tip * 0.30;
    let half_tooth = (tip * 0.24).max(0.75);
    let teeth = if size < 14 { 6.0 } else { 8.0 };
    let step = TAU / teeth;
    let shape = move |x: f32, y: f32| {
        let (dx, dy) = (x - c, y - c);
        let r = dx.hypot(dy);
        // The nearest tooth: a bar from the centre out to the tip.
        let (sin, cos) = ((dy.atan2(dx) / step).round() * step).sin_cos();
        let along = dx * cos + dy * sin;
        let across = dy * cos - dx * sin;
        let tooth = (across.abs() - half_tooth).max(along - tip);
        (r - wheel).min(tooth).max(hole - r)
    };
    let mut canvas = Canvas::new(n, n);
    canvas.fill_flat(shape, white(1.0));
    canvas.to_white_argb4444()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{G, build};

    /// A font with a space, an 'H' whose ink starts 3 rows into a 12-row
    /// bitmap drawn from 12 above the baseline (so capitals 9 tall), and a
    /// filler at U+FFFF.
    fn font() -> Vec<u8> {
        let mut h = vec![0x0fffu16; 4 * 3];
        h.extend(std::iter::repeat_n(0xffffu16, 4 * 9));
        let glyphs = [
            G {
                unit: 0x20,
                advance: 4,
                x_offset: 0,
                y_offset: 12,
                width: 0,
                height: 0,
                data: vec![],
            },
            G {
                unit: u16::from(b'H'),
                advance: 5,
                x_offset: 0,
                y_offset: 12,
                width: 4,
                height: 12,
                data: crate::encode::encode_pixels(&h),
            },
            G {
                unit: 0xe3ff,
                advance: 3,
                x_offset: 0,
                y_offset: 12,
                width: 2,
                height: 2,
                data: vec![0x44],
            },
        ];
        let mut f = build(12, 3, 0, &glyphs, &[]);
        let o = crate::CHARMAP_OFFSET + 0xffff * 4;
        f[o..o + 4].copy_from_slice(&2u32.to_le_bytes());
        f
    }

    #[test]
    fn the_gear_is_as_tall_as_the_capitals_and_stands_on_the_baseline() {
        let data = font();
        let f = Font::parse(&data).unwrap();
        assert!(!has_glyph(&f));
        assert_eq!(size(&f), 9);
        let r = replacement(&f).unwrap();
        assert_eq!((r.unit, r.width, r.height), (GEAR, 9, 9));
        assert_eq!(r.y_offset, Some(9), "the bottom row on the baseline");
        let out = crate::patch::apply(&data, &[r]).unwrap();
        let p = Font::parse(&out).unwrap();
        p.validate().unwrap();
        let g = p.glyph(GEAR).unwrap();
        assert_eq!((g.record.width, g.record.height), (9, 9));
        // Patched once, it has its glyph: a second patch adds nothing.
        assert!(has_glyph(&p));
        assert_eq!(replacement(&p), None);
        // The rest of the font is as it was, though its pixel data sits
        // further on behind the new glyph record.
        let (was, now) = (
            f.glyph(u16::from(b'H')).unwrap(),
            p.glyph(u16::from(b'H')).unwrap(),
        );
        assert_eq!(now.pixels, was.pixels);
        assert_eq!(
            (now.record.advance, now.record.width, now.record.height),
            (was.record.advance, was.record.width, was.record.height)
        );
        assert_eq!(
            (now.record.x_offset, now.record.y_offset),
            (was.record.x_offset, was.record.y_offset)
        );
    }

    /// A font of one empty glyph (the space) and no 'H'.
    fn spaces(ascent: i16) -> Vec<u8> {
        let space = G {
            unit: 0x20,
            advance: 4,
            x_offset: 0,
            y_offset: ascent,
            width: 0,
            height: 0,
            data: vec![],
        };
        build(ascent, 5, 0, &[space], &[])
    }

    #[test]
    fn with_no_capital_the_ascent_sizes_it() {
        assert_eq!(size(&Font::parse(&spaces(20)).unwrap()), 14);
        assert_eq!(size(&Font::parse(&spaces(4)).unwrap()), MIN_SIZE);
    }

    #[test]
    fn the_gear_has_a_hole_a_wheel_and_teeth() {
        for size in [MIN_SIZE, 9, 12, 16, 24, MAX_SIZE] {
            let px = draw(size);
            let n = usize::from(size);
            assert_eq!(px.len(), n * n);
            let alpha = |x: usize, y: usize| px[y * n + x] >> 12;
            let mid = n / 2;
            // White wherever it is drawn.
            assert!(px.iter().all(|p| p & 0xfff == 0xfff), "{size}");
            // The hole: clear at the centre (odd sizes: the centre pixel).
            if n % 2 == 1 {
                assert_eq!(alpha(mid, mid), 0, "{size}: the hole");
            }
            // The wheel: solid between the hole and the teeth.
            let ring = (f32::from(size) / 2.0 * 0.5) as usize;
            assert!(
                alpha(mid, mid - ring) > 0 || alpha(mid, mid + ring) > 0,
                "{size}: the wheel"
            );
            // The corners are clear: teeth, not a disc.
            assert_eq!(alpha(0, 0), 0, "{size}: a corner");
            // Symmetric left to right (teeth at 0 and 180 degrees).
            for y in 0..n {
                for x in 0..n {
                    assert!(
                        alpha(x, y).abs_diff(alpha(n - 1 - x, y)) <= 4,
                        "{size}: ({x}, {y})"
                    );
                }
            }
        }
    }

    #[test]
    fn every_font_size_patches() {
        // The engine's limits (128 pixels, 0x1000 bytes) hold at the largest.
        let data = spaces(100);
        let f = Font::parse(&data).unwrap();
        let r = replacement(&f).unwrap();
        assert_eq!(r.width, MAX_SIZE);
        let out = crate::patch::apply(&data, &[r]).unwrap();
        Font::parse(&out).unwrap().validate().unwrap();
    }
}
