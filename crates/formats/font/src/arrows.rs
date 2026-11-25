//! Direction arrows for the HUD (docs/game/hud-kit.md, "Direction
//! arrows"): eight arrows, 45 degrees apart, at spare private-use code
//! points, so a HUD text of ours can say which way: the way a player is
//! moving, the way to steer. Halo 2's fonts have no arrows (U+2191 and
//! its kin are the missing box), and the kit's runtime does not turn a
//! widget, so a direction is a character.
//!
//! [`replacements`] draws them for a font, sized and placed as the bot
//! mark's gear is ([`crate::gear`]), ready for [`crate::patch::apply`].
//! Drawn here from a bar and a triangle; nothing is copied from a disc
//! font or anyone's artwork.
use std::f32::consts::FRAC_PI_4;

use crate::Font;
use crate::icons::canvas::{Canvas, polygon, segment, white};
use crate::patch::Replacement;

/// How many arrows: one each 45 degrees.
pub const COUNT: u16 = 8;
/// U+E2B0: the arrow pointing up. The others follow clockwise: U+E2B1 up
/// and right, U+E2B2 right, to U+E2B7 up and left. After the medals'
/// U+E2A0..U+E2A8 ([`crate::medals`]).
pub const FIRST: u16 = 0xe2b0;

/// The code unit of arrow `step`, counted clockwise from up, modulo
/// [`COUNT`].
pub const fn unit(step: u16) -> u16 {
    FIRST + step % COUNT
}

/// The arrow nearest `angle`, radians clockwise from up: its step, 0 to 7.
/// An angle that is not a number is up.
pub fn step_for(angle: f32) -> u16 {
    if !angle.is_finite() {
        return 0;
    }
    ((angle / FRAC_PI_4).round() as i64).rem_euclid(i64::from(COUNT)) as u16
}

/// The character of the arrow nearest `angle` ([`step_for`]).
pub fn glyph_for(angle: f32) -> char {
    glyph(step_for(angle))
}

/// The character of arrow `step`.
pub fn glyph(step: u16) -> char {
    char::from_u32(u32::from(unit(step))).unwrap_or('^')
}

/// The arrows `font` lacks, each as tall as its capitals (the gear's
/// size), standing on the baseline, one pixel clear of the character
/// before it.
pub fn replacements(font: &Font<'_>) -> Vec<Replacement> {
    let size = crate::gear::size(font);
    (0..COUNT)
        .filter(|step| !has_glyph(font, unit(*step)))
        .map(|step| Replacement {
            unit: unit(step),
            width: size,
            height: size,
            advance: size as i16 + 1,
            x_offset: 1,
            y_offset: Some(size as i16),
            pixels: draw(step, size),
        })
        .collect()
}

/// The font file `data` with the arrows it lacks added; `None` when it
/// is not a font, has them all, or the patch fails. For tools that read
/// the disc's fonts as the game's font rewrite serves them.
pub fn patch(data: &[u8]) -> Option<Vec<u8>> {
    let add = replacements(&Font::parse(data).ok()?);
    if add.is_empty() {
        return None;
    }
    crate::patch::apply(data, &add).ok()
}

/// Whether `font` has its own picture at `unit` (as
/// [`crate::gear::has_glyph`] asks for the gear).
pub fn has_glyph(font: &Font<'_>, unit: u16) -> bool {
    let i = font.glyph_index(unit);
    i != 0
        && i != font.glyph_index(0xffff)
        && font.record(i).is_ok_and(|r| r.width > 0 && r.height > 0)
}

/// Arrow `step` in white, `size` pixels square, ARGB4444: a shaft and a
/// head, pointing `step` times 45 degrees clockwise from up.
pub fn draw(step: u16, size: u16) -> Vec<u16> {
    let n = usize::from(size);
    let s = f32::from(size);
    let c = s / 2.0;
    let a = f32::from(step % COUNT) * FRAC_PI_4;
    // The way it points, and the way across it, in pixels (y down).
    let (dx, dy) = (a.sin(), -a.cos());
    let (px, py) = (-dy, dx);
    let at = |along: f32, across: f32| (c + s * (along * dx + across * px), c + s * (along * dy + across * py));
    let mut canvas = Canvas::new(n, n);
    let paint = white(1.0);
    let w = (s * 0.16).max(1.5);
    canvas.fill_flat(segment(at(-0.40, 0.0), at(0.10, 0.0), w), paint);
    canvas.fill_flat(polygon([at(0.02, -0.32), at(0.02, 0.32), at(0.46, 0.0)]), paint);
    canvas.to_white_argb4444()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{PI, TAU};

    /// Ink pixels of a picture.
    fn ink(p: &[u16]) -> usize {
        p.iter().filter(|p| *p >> 12 >= 8).count()
    }

    /// The centre of a picture's ink, from its middle, in pixels (x right,
    /// y down).
    fn lean(p: &[u16], size: u16) -> (f32, f32) {
        let n = usize::from(size);
        let c = f32::from(size) / 2.0;
        let (mut x, mut y, mut k) = (0.0, 0.0, 0.0);
        for (i, v) in p.iter().enumerate() {
            if v >> 12 >= 8 {
                x += (i % n) as f32 + 0.5 - c;
                y += (i / n) as f32 + 0.5 - c;
                k += 1.0;
            }
        }
        (x / k, y / k)
    }

    #[test]
    fn each_arrow_is_drawn_and_tells_itself_apart() {
        for size in [crate::gear::MIN_SIZE, 9, 12, 18] {
            let n = usize::from(size) * usize::from(size);
            let pics: Vec<Vec<u16>> = (0..COUNT).map(|k| draw(k, size)).collect();
            for (k, p) in pics.iter().enumerate() {
                assert_eq!(p.len(), n, "arrow {k} at {size}");
                let i = ink(p);
                assert!(i > n / 12 && i < n * 3 / 4, "arrow {k} at {size}: {i} of {n}");
                for q in &pics[..k] {
                    assert_ne!(p, q, "arrow {k} at {size}");
                }
            }
        }
    }

    /// The head is heavier than the shaft, so an arrow's ink leans the way
    /// it points: up is up the picture, right is right, and so round.
    #[test]
    fn an_arrow_leans_the_way_it_points() {
        let size = 16;
        for k in 0..COUNT {
            let a = f32::from(k) * FRAC_PI_4;
            let (x, y) = lean(&draw(k, size), size);
            let along = x * a.sin() - y * a.cos();
            let across = x * a.cos() + y * a.sin();
            assert!(along > 0.3, "arrow {k}: {along}");
            assert!(across.abs() < 0.3, "arrow {k}: {across}");
        }
    }

    #[test]
    fn an_angle_picks_the_nearest_arrow() {
        assert_eq!(step_for(0.0), 0);
        assert_eq!(step_for(0.3), 0);
        assert_eq!(step_for(0.5), 1);
        assert_eq!(step_for(PI / 2.0), 2);
        assert_eq!(step_for(PI), 4);
        assert_eq!(step_for(-PI / 2.0), 6);
        assert_eq!(step_for(-0.3), 0);
        assert_eq!(step_for(TAU - 0.1), 0);
        assert_eq!(step_for(TAU + PI / 4.0), 1);
        assert_eq!(step_for(f32::NAN), 0);
        assert_eq!(glyph_for(PI), '\u{e2b4}');
        assert_eq!(glyph(0), '\u{e2b0}');
        assert_eq!(glyph(9), '\u{e2b1}');
        assert_eq!(unit(7), 0xe2b7);
    }

    /// A font gets all eight, as tall as its capitals, and a second patch
    /// adds nothing.
    #[test]
    fn a_font_gets_the_eight_arrows_once() {
        use crate::testutil::{G, build};
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
        let mut data = build(12, 3, 0, &glyphs, &[]);
        let o = crate::CHARMAP_OFFSET + 0xffff * 4;
        data[o..o + 4].copy_from_slice(&2u32.to_le_bytes());
        let f = Font::parse(&data).unwrap();
        let add = replacements(&f);
        assert_eq!(add.len(), usize::from(COUNT));
        for (k, r) in add.iter().enumerate() {
            assert_eq!((r.unit, r.width, r.height), (unit(k as u16), 9, 9));
            assert_eq!((r.advance, r.x_offset, r.y_offset), (10, 1, Some(9)));
        }
        let out = patch(&data).unwrap();
        let p = Font::parse(&out).unwrap();
        p.validate().unwrap();
        for k in 0..COUNT {
            assert!(has_glyph(&p, unit(k)));
            let g = p.glyph(unit(k)).unwrap();
            assert_eq!((g.record.width, g.record.height), (9, 9));
        }
        assert!(replacements(&p).is_empty());
        assert_eq!(patch(&out), None);
    }
}
