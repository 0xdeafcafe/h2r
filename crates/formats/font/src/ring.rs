//! The charge ring (docs/game/hud-kit.md, "The charge segments"): nine
//! arcs round the motion tracker's rim, down its left side, along the
//! bottom and up the right, as glyphs at spare private-use code points.
//! A HUD text widget draws one in the HUD's own text colour, as it draws
//! a letter, so the ring is the HUD's teal with no bitmap of ours in a
//! map.
//!
//! Each arc has [`STEPS`] + 1 pictures: step 0 the empty track, faint
//! ([`TRACK`]), and step `k` the first `k` of [`STEPS`] parts of its
//! length lit, from its start. A widget that shows the picture for a fill
//! is a meter of [`STEPS`] steps.
//!
//! ```text
//!        .---.
//!  1 (  /     \  ) 9
//!  2 ( | track | ) 8
//!  3  \ \     / /  7
//!      \ '---' /
//!       ' ___ '
//!       4  5  6
//! ```
//!
//! The geometry is in the pixels of the rim's own rectangle: 83 by 83,
//! its top-left corner (0, 0), its centre ([`RIM`], [`RIM`]), y down. An
//! angle is in degrees from straight down, positive to the right: -90 is
//! the rim's left, 0 its foot, 90 its right.
//!
//! [`replacements`] gives the pictures to [`crate::patch::apply`], each
//! with its top-left corner at the text's point. Drawn here from an
//! annulus and two radii; nothing is copied from a disc bitmap or
//! anyone's artwork.
use std::f32::consts::FRAC_PI_2;

use crate::Font;
use crate::icons::canvas::{Canvas, white};
use crate::patch::Replacement;

/// How many arcs, and how many a side (left, bottom, right).
pub const SEGMENTS: usize = 9;
pub const SIDE: usize = 3;
/// The fill steps of an arc: its pictures are steps 0 (empty) to this
/// (lit whole).
pub const STEPS: u8 = 6;
/// U+E2C0: arc 0's empty picture. Arc `s`'s step `k` is at
/// `FIRST + s × (STEPS + 1) + k`, to U+E2FE. After the direction arrows'
/// U+E2B0..U+E2B7 ([`crate::arrows`]).
pub const FIRST: u16 = 0xe2c0;
/// How many pictures: 63.
pub const COUNT: u16 = SEGMENTS as u16 * (STEPS as u16 + 1);

/// The rim's radius: the motion tracker's rim is 83 across.
pub const RIM: f32 = 41.5;
/// Pixels between the rim and the arcs' inner edge.
pub const GAP: f32 = 4.0;
/// An arc's width, across it.
pub const WIDTH: f32 = 3.0;
/// The arcs' inner and outer radii.
pub const INNER: f32 = RIM + GAP;
pub const OUTER: f32 = INNER + WIDTH;
/// Where the ring starts and ends: this many degrees up each side from
/// straight down. The first arc starts at `-TOP`, the last ends at `TOP`.
pub const TOP: f32 = 110.0;
/// The degrees an arc spans, the gap between two arcs of a side, and the
/// gap between two sides. Nine spans, six gaps and two side gaps are
/// twice [`TOP`].
pub const SPAN: f32 = 18.0;
pub const SEGMENT_GAP: f32 = 6.0;
pub const SIDE_GAP: f32 = 11.0;
/// The empty track's opacity, of the lit part's 1: alpha 4 of 15, one of
/// the eight levels a font's run bytes carry.
pub const TRACK: f32 = 4.0 / 15.0;

/// Each arc's box in the rim's pixels, `[x0, y0, x1, y1]`, ends
/// exclusive: [`bounds`], which a test holds this to.
pub const BOXES: [[i16; 4]; SEGMENTS] = [
    [-7, 24, -1, 40],
    [-7, 44, 0, 60],
    [-2, 62, 10, 77],
    [15, 79, 30, 89],
    [33, 86, 50, 90],
    [53, 79, 68, 89],
    [73, 62, 85, 77],
    [83, 44, 90, 60],
    [84, 24, 90, 40],
];

/// The angle arc `segment` starts at; it ends [`SPAN`] on.
pub fn start(segment: usize) -> f32 {
    -TOP + segment as f32 * (SPAN + SEGMENT_GAP) + (segment / SIDE) as f32 * (SIDE_GAP - SEGMENT_GAP)
}

/// The point at `angle` and `radius` from the rim's centre.
pub fn point(angle: f32, radius: f32) -> (f32, f32) {
    let (sin, cos) = angle.to_radians().sin_cos();
    (RIM + radius * sin, RIM + radius * cos)
}

/// The angle of the point (`x`, `y`) about the rim's centre, -180 to 180.
pub fn angle_of(x: f32, y: f32) -> f32 {
    (x - RIM).atan2(y - RIM).to_degrees()
}

/// Arc `segment`'s box: the whole pixels its shape touches.
pub fn bounds(segment: usize) -> [i16; 4] {
    let (a0, a1) = (start(segment), start(segment) + SPAN);
    let mut points = vec![
        point(a0, INNER),
        point(a0, OUTER),
        point(a1, INNER),
        point(a1, OUTER),
    ];
    // Where the arc crosses an axis its outer edge is the furthest out.
    for (axis, at) in [
        (-90.0, (RIM - OUTER, RIM)),
        (0.0, (RIM, RIM + OUTER)),
        (90.0, (RIM + OUTER, RIM)),
    ] {
        if (a0..=a1).contains(&axis) {
            points.push(at);
        }
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in points {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    [
        x0.floor() as i16,
        y0.floor() as i16,
        x1.ceil() as i16,
        y1.ceil() as i16,
    ]
}

/// Arc `segment`'s picture size, wide and tall.
pub const fn size(segment: usize) -> (u16, u16) {
    let [x0, y0, x1, y1] = BOXES[segment];
    ((x1 - x0) as u16, (y1 - y0) as u16)
}

/// The signed distance, in pixels, from (`x`, `y`) to arc `segment`'s
/// shape: negative inside. The shape is the part of the annulus between
/// [`INNER`] and [`OUTER`] between its two radii, its ends cut square.
pub fn distance(segment: usize, x: f32, y: f32) -> f32 {
    let rho = (x - RIM).hypot(y - RIM);
    let radial = (rho - (INNER + OUTER) / 2.0).abs() - WIDTH / 2.0;
    let mid = start(segment) + SPAN / 2.0;
    let mut off = (angle_of(x, y) - mid).abs();
    if off > 180.0 {
        off = 360.0 - off;
    }
    // The distance to the nearer end's radius.
    let past = (off - SPAN / 2.0).to_radians();
    let end = if past.abs() < FRAC_PI_2 { rho * past.sin() } else { rho };
    radial.max(end)
}

/// How far along arc `segment` the point (`x`, `y`) is: 0 at its start, 1
/// at its end.
pub fn along(segment: usize, x: f32, y: f32) -> f32 {
    (angle_of(x, y) - start(segment)) / SPAN
}

/// The step that shows `fill` (0 to 1): the nearest of 0 to [`STEPS`]. A
/// fill that is not a number is empty.
pub fn step_for(fill: f32) -> u8 {
    if fill.is_nan() {
        return 0;
    }
    (fill.clamp(0.0, 1.0) * f32::from(STEPS)).round() as u8
}

/// The code unit of arc `segment`'s step `step` (a step past [`STEPS`] is
/// [`STEPS`]).
pub const fn unit(segment: usize, step: u8) -> u16 {
    let step = if step > STEPS { STEPS } else { step };
    FIRST + (segment % SEGMENTS) as u16 * (STEPS as u16 + 1) + step as u16
}

/// The character of arc `segment`'s step `step`.
pub fn glyph(segment: usize, step: u8) -> char {
    char::from_u32(u32::from(unit(segment, step))).unwrap_or(' ')
}

/// The arc and step code unit `unit` draws, if it is one of the ring's.
pub fn parts(unit: u16) -> Option<(usize, u8)> {
    let n = unit.checked_sub(FIRST).filter(|n| *n < COUNT)?;
    let per = u16::from(STEPS) + 1;
    Some((usize::from(n / per), (n % per) as u8))
}

/// Arc `segment` at step `step` in white, [`size`] pixels, ARGB4444: the
/// first `step` of [`STEPS`] parts of its length lit, the rest the faint
/// track.
pub fn draw(segment: usize, step: u8) -> Vec<u16> {
    let segment = segment % SEGMENTS;
    let [x0, y0, ..] = BOXES[segment];
    let (w, h) = size(segment);
    let (ox, oy) = (f32::from(x0), f32::from(y0));
    let lit = f32::from(step.min(STEPS)) / f32::from(STEPS);
    let mut canvas = Canvas::new(usize::from(w), usize::from(h));
    canvas.fill(
        |x, y| distance(segment, x + ox, y + oy),
        |x, y| {
            let on = step >= STEPS || (step > 0 && along(segment, x + ox, y + oy) < lit);
            white(if on { 1.0 } else { TRACK })
        },
    );
    canvas.to_white_argb4444()
}

/// A picture of the ring's: its size and its ARGB4444 pixels, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u16,
    pub height: u16,
    pub pixels: Vec<u16>,
}

/// The picture code unit `unit` draws ([`draw`]); `None` for a unit that
/// is not the ring's.
pub fn picture(unit: u16) -> Option<Picture> {
    let (segment, step) = parts(unit)?;
    let (width, height) = size(segment);
    Some(Picture {
        width,
        height,
        pixels: draw(segment, step),
    })
}

/// The ring's pictures `font` lacks. Each has its top-left corner at the
/// text's point: no offset across, and as far above the baseline as the
/// font's ascent, so the picture's top is the line's top in any font.
pub fn replacements(font: &Font<'_>) -> Vec<Replacement> {
    (0..SEGMENTS)
        .flat_map(|s| (0..=STEPS).map(move |k| (s, k)))
        .filter(|(s, k)| !has_glyph(font, unit(*s, *k)))
        .map(|(s, k)| {
            let (width, height) = size(s);
            Replacement {
                unit: unit(s, k),
                width,
                height,
                advance: width as i16,
                x_offset: 0,
                y_offset: Some(font.header.ascent),
                pixels: draw(s, k),
            }
        })
        .collect()
}

/// The font file `data` with the ring's pictures it lacks added; `None`
/// when it is not a font, has them all, or the patch fails. For tools
/// that read the disc's fonts as the game's font rewrite serves them.
pub fn patch(data: &[u8]) -> Option<Vec<u8>> {
    let add = replacements(&Font::parse(data).ok()?);
    if add.is_empty() {
        return None;
    }
    crate::patch::apply(data, &add).ok()
}

/// Whether `font` has its own picture at `unit` (as
/// [`crate::arrows::has_glyph`] asks for an arrow).
pub fn has_glyph(font: &Font<'_>, unit: u16) -> bool {
    let i = font.glyph_index(unit);
    i != 0
        && i != font.glyph_index(0xffff)
        && font.record(i).is_ok_and(|r| r.width > 0 && r.height > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pixel's alpha, 0 to 15.
    fn alpha(p: u16) -> u16 {
        p >> 12
    }

    /// The pixels of a picture with their centres in the rim's pixels.
    fn pixels(segment: usize, step: u8) -> Vec<(f32, f32, u16)> {
        let [x0, y0, ..] = BOXES[segment];
        let (w, _) = size(segment);
        draw(segment, step)
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let (x, y) = (i % usize::from(w), i / usize::from(w));
                (
                    f32::from(x0) + x as f32 + 0.5,
                    f32::from(y0) + y as f32 + 0.5,
                    alpha(p),
                )
            })
            .collect()
    }

    /// Three arcs a side, each side the mirror of the other, the middle
    /// arc under the centre; the spans and gaps add up to the ring.
    #[test]
    fn the_arcs_run_down_the_left_along_the_bottom_and_up_the_right() {
        let starts: Vec<f32> = (0..SEGMENTS).map(start).collect();
        assert_eq!(
            starts,
            [-110.0, -86.0, -62.0, -33.0, -9.0, 15.0, 44.0, 68.0, 92.0]
        );
        assert_eq!(start(SEGMENTS - 1) + SPAN, TOP);
        assert_eq!(
            SEGMENTS as f32 * SPAN + 6.0 * SEGMENT_GAP + 2.0 * SIDE_GAP,
            2.0 * TOP
        );
        for s in 0..SEGMENTS {
            // The mirror of an arc is the arc as far from the other end.
            assert_eq!(start(SEGMENTS - 1 - s), -(start(s) + SPAN), "arc {s}");
            let side = s / SIDE;
            let mid = start(s) + SPAN / 2.0;
            match side {
                0 => assert!(mid < -45.0, "arc {s} is on the left"),
                1 => assert!(mid.abs() < 45.0, "arc {s} is on the bottom"),
                _ => assert!(mid > 45.0, "arc {s} is on the right"),
            }
        }
        // Left of the centre is left on the screen, and 0 is straight down.
        let (x, y) = point(-90.0, OUTER);
        assert!((x - (RIM - OUTER)).abs() < 1e-3 && (y - RIM).abs() < 1e-3);
        let (x, y) = point(0.0, OUTER);
        assert!((x - RIM).abs() < 1e-3 && (y - (RIM + OUTER)).abs() < 1e-3);
        assert!((angle_of(RIM + 10.0, RIM) - 90.0).abs() < 1e-3);
    }

    /// Every arc is a band concentric with the rim: [`GAP`] clear of it,
    /// [`WIDTH`] wide, inside the margin the ring is allowed (7 pixels
    /// left, right and under the rim's rectangle), and no higher than 24
    /// pixels down the rim, under the shield meter's band.
    #[test]
    fn every_arc_hugs_the_rim_inside_its_margin() {
        assert_eq!((INNER, OUTER), (45.5, 48.5));
        for s in 0..SEGMENTS {
            assert_eq!(bounds(s), BOXES[s], "arc {s}");
            let [x0, y0, x1, y1] = BOXES[s];
            assert!(x0 >= -7 && x1 <= 90 && y1 <= 90, "arc {s}: {:?}", BOXES[s]);
            assert!(y0 >= 24, "arc {s}: {:?}", BOXES[s]);
            // Along its middle the shape is half its width deep, and its
            // edges are on the two radii.
            for k in 0..=8 {
                let a = start(s) + SPAN * (0.2 + 0.075 * k as f32);
                let (x, y) = point(a, (INNER + OUTER) / 2.0);
                assert!((distance(s, x, y) + WIDTH / 2.0).abs() < 1e-3, "arc {s} at {a}");
                let (x, y) = point(a, INNER);
                assert!(distance(s, x, y).abs() < 1e-3, "arc {s} at {a}");
                let (x, y) = point(a, OUTER);
                assert!(distance(s, x, y).abs() < 1e-3, "arc {s} at {a}");
                // The rim itself is GAP outside the shape.
                let (x, y) = point(a, RIM);
                assert!((distance(s, x, y) - GAP).abs() < 1e-3, "arc {s} at {a}");
            }
            // No ink on the rim or inside it, none outside the band, none
            // outside the picture's box (the box is the picture).
            for (x, y, a) in pixels(s, STEPS) {
                let rho = (x - RIM).hypot(y - RIM);
                if a > 0 {
                    assert!(rho > INNER - 1.0 && rho < OUTER + 1.0, "arc {s}: ink at radius {rho}");
                    assert!(rho > RIM + GAP - 1.0, "arc {s}: ink {rho} from the centre");
                    let t = along(s, x, y);
                    assert!(t > -0.1 && t < 1.1, "arc {s}: ink at {t} of its length");
                }
            }
        }
    }

    /// No two arcs meet: their boxes are apart, the gap between two of a
    /// side is [`SEGMENT_GAP`] degrees, over 4 pixels across, and the gap
    /// between two sides is wider.
    #[test]
    fn the_arcs_are_apart() {
        for a in 0..SEGMENTS {
            for b in a + 1..SEGMENTS {
                let ([ax0, ay0, ax1, ay1], [bx0, by0, bx1, by1]) = (BOXES[a], BOXES[b]);
                let apart = ax1 <= bx0 || bx1 <= ax0 || ay1 <= by0 || by1 <= ay0;
                assert!(apart, "arcs {a} and {b}: {:?} {:?}", BOXES[a], BOXES[b]);
                // No point of one's shape is in the other's.
                for k in 0..=10 {
                    let (x, y) = point(start(a) + SPAN * 0.1 * k as f32, (INNER + OUTER) / 2.0);
                    assert!(distance(b, x, y) > 3.0, "arcs {a} and {b}");
                }
            }
        }
        for s in 0..SEGMENTS - 1 {
            let gap = start(s + 1) - (start(s) + SPAN);
            let want = if (s + 1) % SIDE == 0 { SIDE_GAP } else { SEGMENT_GAP };
            assert_eq!(gap, want, "after arc {s}");
            // The gap across, at the inner edge where it is narrowest.
            let across = 2.0 * INNER * (gap.to_radians() / 2.0).sin();
            assert!(across > 4.0, "after arc {s}: {across}");
            // The gap is empty: the point midway is outside both.
            let (x, y) = point(start(s) + SPAN + gap / 2.0, (INNER + OUTER) / 2.0);
            assert!(distance(s, x, y) > 2.0 && distance(s + 1, x, y) > 2.0, "after arc {s}");
        }
        assert!(SIDE_GAP > SEGMENT_GAP);
    }

    /// The pictures: each its box's size, the same every time, the empty
    /// one the faint track alone, the full one lit alone, and each step
    /// between lighting more from the arc's start.
    #[test]
    fn the_pictures_fill_from_the_start() {
        assert_eq!(TRACK, 4.0 / 15.0);
        for s in 0..SEGMENTS {
            let (w, h) = size(s);
            let [x0, y0, x1, y1] = BOXES[s];
            assert_eq!((i32::from(w), i32::from(h)), (i32::from(x1 - x0), i32::from(y1 - y0)));
            let n = usize::from(w) * usize::from(h);
            let mut lit_before = 0;
            for k in 0..=STEPS {
                let p = draw(s, k);
                assert_eq!(p.len(), n, "arc {s} step {k}");
                assert_eq!(p, draw(s, k), "arc {s} step {k} is the same twice");
                assert!(p.iter().all(|p| p & 0xfff == 0xfff), "white");
                let lit = p.iter().filter(|p| alpha(**p) > 8).count();
                let faint = p.iter().filter(|p| (3..=4).contains(&alpha(**p))).count();
                match k {
                    0 => {
                        assert_eq!(lit, 0, "arc {s}: the empty track");
                        assert!(p.iter().all(|p| alpha(*p) <= 4));
                        assert!(faint > 20, "arc {s}: {faint}");
                    }
                    STEPS => assert!(lit > 30, "arc {s}: {lit}"),
                    _ => assert!(lit > lit_before, "arc {s} step {k}: {lit} after {lit_before}"),
                }
                assert!(lit >= lit_before, "arc {s} step {k}");
                lit_before = lit;
                // The track's pixels are inked at every step: only the
                // light moves.
                for (q, e) in p.iter().zip(draw(s, 0)) {
                    assert!(alpha(*q) >= alpha(e), "arc {s} step {k}");
                }
            }
            // Half lit: the lit pixels are in the first half, the faint
            // ones in the second.
            let half = STEPS / 2;
            for (x, y, a) in pixels(s, half) {
                let t = along(s, x, y);
                if a > 8 {
                    assert!(t < 0.5, "arc {s}: lit at {t}");
                }
                if a > 0 && t > 0.5 {
                    assert!(a <= 4, "arc {s}: alpha {a} at {t}");
                }
            }
            // A step past the last is the last.
            assert_eq!(draw(s, STEPS + 3), draw(s, STEPS));
        }
        // The right side's pictures are the left side's mirrored, empty
        // and full.
        for s in 0..SEGMENTS {
            let (w, _) = size(s);
            for k in [0, STEPS] {
                let mirrored: Vec<u16> = draw(s, k)
                    .chunks(usize::from(w))
                    .flat_map(|row| row.iter().rev().copied())
                    .collect();
                assert_eq!(mirrored, draw(SEGMENTS - 1 - s, k), "arc {s} step {k}");
            }
        }
    }

    #[test]
    fn a_fill_picks_the_nearest_step_and_a_unit_names_its_arc() {
        assert_eq!(step_for(0.0), 0);
        assert_eq!(step_for(1.0), STEPS);
        assert_eq!(step_for(0.5), 3);
        assert_eq!(step_for(0.08), 0);
        assert_eq!(step_for(0.09), 1);
        assert_eq!(step_for(0.92), STEPS);
        assert_eq!(step_for(-2.0), 0);
        assert_eq!(step_for(7.0), STEPS);
        assert_eq!(step_for(f32::NAN), 0);
        assert_eq!(COUNT, 63);
        assert_eq!((unit(0, 0), unit(0, STEPS), unit(1, 0)), (0xe2c0, 0xe2c6, 0xe2c7));
        assert_eq!(unit(SEGMENTS - 1, STEPS), 0xe2fe);
        assert_eq!(unit(2, 200), unit(2, STEPS));
        assert_eq!(glyph(0, 0), '\u{e2c0}');
        let mut seen = std::collections::HashSet::new();
        for s in 0..SEGMENTS {
            for k in 0..=STEPS {
                let u = unit(s, k);
                assert!(seen.insert(u), "U+{u:04X} twice");
                assert_eq!(parts(u), Some((s, k)));
                let p = picture(u).unwrap();
                assert_eq!((p.width, p.height), size(s));
                assert_eq!(p.pixels, draw(s, k));
            }
        }
        assert_eq!(parts(FIRST - 1), None);
        assert_eq!(parts(FIRST + COUNT), None);
        assert!(picture(u16::from(b'A')).is_none());
        // Clear of the arrows before it and of the medals before them.
        assert!(FIRST > crate::arrows::unit(crate::arrows::COUNT - 1));
        assert!(crate::medals::MEDALS.iter().all(|(u, _)| *u < FIRST));
    }

    /// A font gets all 63, each with its top-left corner at the text's
    /// point, and a second patch adds nothing.
    #[test]
    fn a_font_gets_the_ring_once() {
        use crate::testutil::{G, build};
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
        data[o..o + 4].copy_from_slice(&1u32.to_le_bytes());
        let f = Font::parse(&data).unwrap();
        let add = replacements(&f);
        assert_eq!(add.len(), usize::from(COUNT));
        for r in &add {
            let (s, k) = parts(r.unit).unwrap();
            assert_eq!((r.width, r.height), size(s));
            assert_eq!((r.advance, r.x_offset, r.y_offset), (r.width as i16, 0, Some(12)));
            assert_eq!(r.pixels, draw(s, k));
        }
        let out = patch(&data).unwrap();
        let p = Font::parse(&out).unwrap();
        p.validate().unwrap();
        for s in 0..SEGMENTS {
            for k in 0..=STEPS {
                assert!(has_glyph(&p, unit(s, k)), "arc {s} step {k}");
                assert_eq!(p.glyph(unit(s, k)).unwrap().pixels, draw(s, k));
            }
        }
        assert!(!has_glyph(&f, unit(0, 0)));
        assert!(replacements(&p).is_empty());
        assert_eq!(patch(&out), None);
        assert_eq!(patch(b"not a font"), None);
        // A text of one picture is the picture, its corner at the text's
        // corner, on a line as tall as the taller of the two.
        let (w, h) = size(0);
        let b = crate::render(&p, &[unit(0, STEPS)], crate::RenderOptions::default()).unwrap();
        assert_eq!((b.width, b.height), (u32::from(w), u32::from(h).max(15)));
        let full = draw(0, STEPS);
        for (i, px) in full.iter().enumerate() {
            let (x, y) = (i % usize::from(w), i / usize::from(w));
            let a = b.rgba[(y * b.width as usize + x) * 4 + 3];
            assert_eq!(u16::from(a), alpha(*px) * 17, "({x}, {y})");
        }
    }
}
