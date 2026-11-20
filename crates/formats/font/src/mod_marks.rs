//! The Mods list's state marks (docs/game/maps.md "Mod downloads (in
//! game)"): a star for a mod that is new, a tick for one that is
//! installed, a down arrow for one downloading (with its percentage after
//! it). Halo 2's fonts have none of the three, so [`replacements`] draws
//! them for a font, sized and placed as the bot mark's gear is
//! ([`crate::gear`]), ready for [`crate::patch::apply`]. Drawn here from
//! polygons and bars; nothing is copied from a disc font or anyone's
//! artwork.
use std::f32::consts::{FRAC_PI_2, TAU};

use crate::Font;
use crate::icons::canvas::{Canvas, polygon, segment, white};
use crate::patch::Replacement;

/// U+2605 BLACK STAR: new since the list was last opened.
pub const NEW: u16 = 0x2605;
/// U+2713 CHECK MARK: downloaded here.
pub const INSTALLED: u16 = 0x2713;
/// U+2193 DOWNWARDS ARROW: downloading.
pub const DOWNLOADING: u16 = 0x2193;
/// Every mark, in the order they are added.
pub const MARKS: [u16; 3] = [NEW, INSTALLED, DOWNLOADING];

/// The marks `font` lacks, each as tall as its capitals (the gear's
/// size), standing on the baseline, one pixel clear of the character
/// before it.
pub fn replacements(font: &Font<'_>) -> Vec<Replacement> {
    let size = crate::gear::size(font);
    MARKS
        .into_iter()
        .filter(|unit| !has_glyph(font, *unit))
        .map(|unit| Replacement {
            unit,
            width: size,
            height: size,
            advance: size as i16 + 1,
            x_offset: 1,
            y_offset: Some(size as i16),
            pixels: draw(unit, size),
        })
        .collect()
}

/// Whether `font` has its own picture at `unit` (as
/// [`crate::gear::has_glyph`] asks for the gear).
pub fn has_glyph(font: &Font<'_>, unit: u16) -> bool {
    let i = font.glyph_index(unit);
    i != 0
        && i != font.glyph_index(0xffff)
        && font.record(i).is_ok_and(|r| r.width > 0 && r.height > 0)
}

/// Mark `unit` in white, `size` pixels square, ARGB4444. An empty picture
/// for a unit that is not a mark.
pub fn draw(unit: u16, size: u16) -> Vec<u16> {
    let n = usize::from(size);
    let s = f32::from(size);
    let mut canvas = Canvas::new(n, n);
    let paint = white(1.0);
    match unit {
        NEW => {
            // A five-pointed star: its inner pentagon and a triangle per
            // point (each convex, filled one over another).
            let (c, outer) = (s / 2.0, s / 2.0 - 0.25);
            let inner = outer * 0.42;
            let at = |r: f32, k: f32| {
                let a = -FRAC_PI_2 + k * TAU / 10.0;
                (c + r * a.cos(), c + 0.6 + r * a.sin())
            };
            let pentagon = [
                at(inner, 1.0),
                at(inner, 3.0),
                at(inner, 5.0),
                at(inner, 7.0),
                at(inner, 9.0),
            ];
            canvas.fill_flat(polygon(pentagon), paint);
            for k in 0..5 {
                let p = 2.0 * k as f32;
                let tip = polygon([at(inner, p - 1.0), at(outer, p), at(inner, p + 1.0)]);
                canvas.fill_flat(tip, paint);
            }
        }
        INSTALLED => {
            // A tick: a short stroke down to the bottom left third, a long
            // one up to the top right.
            let w = (s * 0.16).max(1.5);
            let (a, b, c) = (
                (s * 0.14, s * 0.55),
                (s * 0.40, s * 0.82),
                (s * 0.88, s * 0.18),
            );
            canvas.fill_flat(segment(a, b, w), paint);
            canvas.fill_flat(segment(b, c, w), paint);
        }
        DOWNLOADING => {
            // A shaft and a head, pointing down to a line.
            let w = (s * 0.16).max(1.5);
            let c = s / 2.0;
            canvas.fill_flat(segment((c, s * 0.08), (c, s * 0.56), w), paint);
            canvas.fill_flat(
                polygon([(s * 0.16, s * 0.44), (s * 0.84, s * 0.44), (c, s * 0.80)]),
                paint,
            );
            canvas.fill_flat(
                segment((s * 0.14, s * 0.92), (s * 0.86, s * 0.92), w * 0.8),
                paint,
            );
        }
        _ => {}
    }
    canvas.to_white_argb4444()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ink pixels of a picture.
    fn ink(p: &[u16]) -> usize {
        p.iter().filter(|p| *p >> 12 >= 8).count()
    }

    #[test]
    fn each_mark_is_drawn_and_tells_itself_apart() {
        for size in [crate::gear::MIN_SIZE, 12, 18, crate::gear::MAX_SIZE] {
            let n = usize::from(size) * usize::from(size);
            let pics: Vec<Vec<u16>> = MARKS.iter().map(|u| draw(*u, size)).collect();
            for (u, p) in MARKS.iter().zip(&pics) {
                assert_eq!(p.len(), n, "U+{u:04X} at {size}");
                let k = ink(p);
                assert!(
                    k > n / 12 && k < n * 3 / 4,
                    "U+{u:04X} at {size}: {k} of {n}"
                );
            }
            assert_ne!(pics[0], pics[1]);
            assert_ne!(pics[1], pics[2]);
            assert_ne!(pics[0], pics[2]);
        }
        assert!(draw(0x41, 12).iter().all(|p| p >> 12 == 0), "not a mark");
    }

    #[test]
    fn the_star_points_up_and_the_arrow_down() {
        let size = 16u16;
        let n = usize::from(size);
        let row_ink = |p: &[u16], y: usize| {
            p[y * n..(y + 1) * n]
                .iter()
                .filter(|p| *p >> 12 >= 8)
                .count()
        };
        let star = draw(NEW, size);
        // Its top point is narrow; its arms are wide.
        assert!(row_ink(&star, 2) < row_ink(&star, 6));
        let arrow = draw(DOWNLOADING, size);
        // The head is widest above its point, the shaft narrow.
        assert!(row_ink(&arrow, 3) < row_ink(&arrow, 8));
    }
}
