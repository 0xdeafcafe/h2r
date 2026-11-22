//! Each skull's glyph, the mark pressed into the model picture's forehead
//! (docs/game/skulls.md, "The model picture"): our own line art, a simple
//! geometric sign for what the skull does, one per skull in
//! [`skulls::ABOUT`](crate::skulls::ABOUT)'s order. None copies a Halo
//! game's skull symbols. They are vector data, sampled at run time into a
//! signed distance field ([`distance_field`]) that the renderer projects
//! onto the forehead and cuts as a groove, its edge resolved at the
//! picture's own resolution whatever the field's size.
//!
//! A glyph lives in a box from -1 to 1 on both axes, y up. Every stroke,
//! with its width, stays inside [`BOX`], so the field's outer texels are
//! always outside every stroke and the decal has no hard edge.

use crate::skulls::TOTAL;

/// Half a line's width, in glyph units.
pub const HALF_WIDTH: f32 = 0.07;
/// How far from the centre any stroke may reach, its width included.
pub const BOX: f32 = 0.9;
/// The distance field's size in texels (square): a stroke is 18 texels
/// wide, so its ends and joins keep their shape.
pub const FIELD: u32 = 256;

/// One stroke of a glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stroke {
    /// A line from `a` to `b`, [`HALF_WIDTH`] either side.
    Line { a: [f32; 2], b: [f32; 2] },
    /// An arc of the circle at `centre` with `radius`, counter-clockwise
    /// from `from` to `to` degrees (0 is +x; `to` may pass 360), drawn as
    /// a line.
    Arc {
        centre: [f32; 2],
        radius: f32,
        from: f32,
        to: f32,
    },
    /// A filled disc.
    Dot { centre: [f32; 2], radius: f32 },
}

use Stroke::{Arc, Dot, Line};

const fn l(ax: f32, ay: f32, bx: f32, by: f32) -> Stroke {
    Line {
        a: [ax, ay],
        b: [bx, by],
    }
}
const fn arc(cx: f32, cy: f32, radius: f32, from: f32, to: f32) -> Stroke {
    Arc {
        centre: [cx, cy],
        radius,
        from,
        to,
    }
}
const fn ring(cx: f32, cy: f32, radius: f32) -> Stroke {
    arc(cx, cy, radius, 0.0, 360.0)
}
const fn dot(cx: f32, cy: f32, radius: f32) -> Stroke {
    Dot {
        centre: [cx, cy],
        radius,
    }
}

/// Envy (active camouflage): a ring whose right half has faded to dots.
const ENVY: &[Stroke] = &[
    arc(0.0, 0.0, 0.6, 90.0, 270.0),
    dot(0.3, 0.52, 0.08),
    dot(0.6, 0.0, 0.08),
    dot(0.3, -0.52, 0.08),
];

/// Grunt Birthday Party (confetti): a burst of eight rays round a ring.
const GRUNT_BIRTHDAY_PARTY: &[Stroke] = &[
    ring(0.0, 0.0, 0.18),
    l(0.38, 0.0, 0.72, 0.0),
    l(0.269, 0.269, 0.509, 0.509),
    l(0.0, 0.38, 0.0, 0.72),
    l(-0.269, 0.269, -0.509, 0.509),
    l(-0.38, 0.0, -0.72, 0.0),
    l(-0.269, -0.269, -0.509, -0.509),
    l(0.0, -0.38, 0.0, -0.72),
    l(0.269, -0.269, 0.509, -0.509),
];

/// Assassins (enemies cloaked): a diamond with its corners missing round a
/// point.
const ASSASSINS: &[Stroke] = &[
    l(0.18, 0.54, 0.54, 0.18),
    l(0.54, -0.18, 0.18, -0.54),
    l(-0.18, -0.54, -0.54, -0.18),
    l(-0.54, 0.18, -0.18, 0.54),
    dot(0.0, 0.0, 0.1),
];

/// Thunderstorm (enemies promoted): two rank chevrons under a star point.
const THUNDERSTORM: &[Stroke] = &[
    l(-0.6, -0.05, 0.0, 0.45),
    l(0.0, 0.45, 0.6, -0.05),
    l(-0.6, -0.55, 0.0, -0.05),
    l(0.0, -0.05, 0.6, -0.55),
    dot(0.0, 0.72, 0.09),
];

/// Famine (dropped weapons half empty): a ring with only its lower part
/// filled.
const FAMINE: &[Stroke] = &[
    ring(0.0, 0.0, 0.65),
    l(-0.5, -0.25, 0.5, -0.25),
    l(-0.36, -0.45, 0.36, -0.45),
];

/// I Would Have Been Your Daddy (rare lines): a speech ring with three dots
/// and a tail.
const IWHBYD: &[Stroke] = &[
    arc(0.0, 0.1, 0.62, 265.0, 585.0),
    l(-0.262, -0.462, -0.45, -0.75),
    dot(-0.25, 0.1, 0.08),
    dot(0.0, 0.1, 0.08),
    dot(0.25, 0.1, 0.08),
];

/// Blind (no HUD): an eye struck through.
const BLIND: &[Stroke] = &[
    arc(0.0, -0.5, 0.9, 34.0, 146.0),
    arc(0.0, 0.5, 0.9, 214.0, 326.0),
    l(-0.55, -0.55, 0.55, 0.55),
];

/// Ghost (no flinching): a plumb line hanging dead still.
const GHOST: &[Stroke] = &[
    l(-0.4, 0.7, 0.4, 0.7),
    l(0.0, 0.7, 0.0, -0.25),
    dot(0.0, -0.45, 0.2),
];

/// Black Eye (melee recharges the shield): a shield's arc over a blow.
const BLACK_EYE: &[Stroke] = &[
    arc(0.0, -0.1, 0.7, 20.0, 160.0),
    l(-0.25, -0.35, 0.25, 0.15),
    l(-0.25, 0.15, 0.25, -0.35),
];

/// Catch (more grenades thrown): a dashed throw landing.
const CATCH: &[Stroke] = &[
    arc(0.0, -0.55, 0.95, 40.0, 60.0),
    arc(0.0, -0.55, 0.95, 75.0, 105.0),
    arc(0.0, -0.55, 0.95, 120.0, 140.0),
    dot(0.7, -0.4, 0.11),
];

/// Sputnik (things fly far): a body with an orbit and its satellite.
const SPUTNIK: &[Stroke] = &[
    dot(-0.15, -0.15, 0.2),
    arc(0.0, 0.0, 0.68, 60.0, 330.0),
    dot(0.657, 0.176, 0.1),
];

/// Iron (co-op restarts on a death): a loop that comes round to its
/// start.
const IRON: &[Stroke] = &[
    arc(0.0, 0.0, 0.6, 60.0, 360.0),
    l(0.6, 0.12, 0.38, -0.12),
    l(0.6, 0.12, 0.8, -0.12),
];

/// Mythic (enemies tougher): nested rings.
const MYTHIC: &[Stroke] = &[
    ring(0.0, 0.0, 0.68),
    ring(0.0, 0.0, 0.42),
    ring(0.0, 0.0, 0.16),
];

/// Angry (enemies fire faster): three chevrons pressing forward.
const ANGRY: &[Stroke] = &[
    l(-0.55, 0.45, -0.25, 0.0),
    l(-0.25, 0.0, -0.55, -0.45),
    l(-0.1, 0.45, 0.2, 0.0),
    l(0.2, 0.0, -0.1, -0.45),
    l(0.35, 0.45, 0.65, 0.0),
    l(0.65, 0.0, 0.35, -0.45),
];

/// Bandana (endless ammunition): two rings joined, an endless line.
const BANDANA: &[Stroke] = &[ring(-0.36, 0.0, 0.32), ring(0.36, 0.0, 0.32)];

/// Acrophobia (flight): an arrow rising on two wings.
const ACROPHOBIA: &[Stroke] = &[
    l(0.0, -0.6, 0.0, 0.65),
    l(0.0, 0.65, -0.28, 0.37),
    l(0.0, 0.65, 0.28, 0.37),
    l(-0.6, -0.1, -0.2, 0.1),
    l(0.6, -0.1, 0.2, 0.1),
];

/// Low Gravity: a ball floating above the ground.
const LOW_GRAVITY: &[Stroke] = &[
    ring(0.0, 0.3, 0.32),
    l(-0.7, -0.6, 0.7, -0.6),
    l(-0.25, -0.42, -0.25, -0.22),
    l(0.25, -0.42, 0.25, -0.22),
];

/// Fall Damage: an arrow striking the ground and cracking it.
const FALL_DAMAGE: &[Stroke] = &[
    l(0.0, 0.7, 0.0, -0.3),
    l(0.0, -0.3, -0.25, -0.05),
    l(0.0, -0.3, 0.25, -0.05),
    l(-0.7, -0.55, 0.7, -0.55),
    l(-0.15, -0.55, -0.35, -0.78),
    l(0.15, -0.55, 0.35, -0.78),
];

/// Unlimited Ammo: three rounds standing on a line.
const UNLIMITED_AMMO: &[Stroke] = &[
    l(-0.4, -0.4, -0.4, 0.3),
    dot(-0.4, 0.5, 0.1),
    l(0.0, -0.4, 0.0, 0.3),
    dot(0.0, 0.5, 0.1),
    l(0.4, -0.4, 0.4, 0.3),
    dot(0.4, 0.5, 0.1),
    l(-0.65, -0.62, 0.65, -0.62),
];

/// Unlimited Grenades: a banded sphere with its pin.
const UNLIMITED_GRENADES: &[Stroke] = &[
    ring(0.0, -0.15, 0.5),
    l(-0.35, -0.15, 0.35, -0.15),
    l(0.0, 0.35, 0.0, 0.6),
    l(-0.25, 0.6, 0.25, 0.6),
];

/// Invincible: a shield with a spine.
const INVINCIBLE: &[Stroke] = &[
    l(-0.6, 0.65, 0.6, 0.65),
    l(-0.6, 0.65, -0.6, 0.05),
    l(0.6, 0.65, 0.6, 0.05),
    l(-0.6, 0.05, 0.0, -0.7),
    l(0.6, 0.05, 0.0, -0.7),
    l(0.0, 0.45, 0.0, -0.35),
];

/// Every skull's glyph, by skull index (`skulls::ABOUT`'s order).
pub static GLYPHS: [&[Stroke]; TOTAL] = [
    ENVY,
    GRUNT_BIRTHDAY_PARTY,
    ASSASSINS,
    THUNDERSTORM,
    FAMINE,
    IWHBYD,
    BLIND,
    GHOST,
    BLACK_EYE,
    CATCH,
    SPUTNIK,
    IRON,
    MYTHIC,
    ANGRY,
    BANDANA,
    ACROPHOBIA,
    LOW_GRAVITY,
    FALL_DAMAGE,
    UNLIMITED_AMMO,
    UNLIMITED_GRENADES,
    INVINCIBLE,
];

/// Skull `index`'s glyph.
pub fn glyph(index: usize) -> Option<&'static [Stroke]> {
    GLYPHS.get(index).copied()
}

fn length(v: [f32; 2]) -> f32 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

fn segment_distance(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let ap = [p[0] - a[0], p[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 > 0.0 {
        ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    length([ap[0] - t * ab[0], ap[1] - t * ab[1]])
}

fn on_circle(centre: [f32; 2], radius: f32, degrees: f32) -> [f32; 2] {
    let (s, c) = degrees.to_radians().sin_cos();
    [centre[0] + radius * c, centre[1] + radius * s]
}

impl Stroke {
    /// The distance from `p` to the stroke's filled shape: negative
    /// inside.
    pub fn signed_distance(&self, p: [f32; 2]) -> f32 {
        match *self {
            Line { a, b } => segment_distance(p, a, b) - HALF_WIDTH,
            Dot { centre, radius } => length([p[0] - centre[0], p[1] - centre[1]]) - radius,
            Arc {
                centre,
                radius,
                from,
                to,
            } => {
                let d = [p[0] - centre[0], p[1] - centre[1]];
                let sweep = (to - from).clamp(0.0, 360.0);
                let angle = d[1].atan2(d[0]).to_degrees();
                let along = (angle - from).rem_euclid(360.0);
                let distance = if along <= sweep {
                    (length(d) - radius).abs()
                } else {
                    let ends = [
                        on_circle(centre, radius, from),
                        on_circle(centre, radius, to),
                    ];
                    ends.map(|e| length([p[0] - e[0], p[1] - e[1]]))
                        .into_iter()
                        .fold(f32::INFINITY, f32::min)
                };
                distance - HALF_WIDTH
            }
        }
    }

    /// How far the stroke reaches from the centre on either axis, its
    /// width included (an arc measured at every degree).
    pub fn reach(&self) -> f32 {
        let far = |p: [f32; 2], pad: f32| p[0].abs().max(p[1].abs()) + pad;
        match *self {
            Line { a, b } => far(a, HALF_WIDTH).max(far(b, HALF_WIDTH)),
            Dot { centre, radius } => far(centre, radius),
            Arc {
                centre,
                radius,
                from,
                to,
            } => {
                let sweep = (to - from).clamp(0.0, 360.0);
                (0..=sweep as u32)
                    .map(|k| far(on_circle(centre, radius, from + k as f32), HALF_WIDTH))
                    .chain([far(on_circle(centre, radius, to), HALF_WIDTH)])
                    .fold(0.0, f32::max)
            }
        }
    }
}

/// The distance from `p` to the nearest of `strokes`' filled shapes, in
/// glyph units: negative inside one.
pub fn signed_distance(strokes: &[Stroke], p: [f32; 2]) -> f32 {
    strokes
        .iter()
        .map(|s| s.signed_distance(p))
        .fold(f32::INFINITY, f32::min)
}

/// `strokes` as a `size` by `size` signed distance field: each texel the
/// distance from its centre to the strokes ([`signed_distance`]), row 0 at
/// the top (+y). The box's edges are the field's edges. Deterministic.
pub fn distance_field(strokes: &[Stroke], size: u32) -> Vec<f32> {
    let texel = 2.0 / size.max(1) as f32;
    let mut out = Vec::with_capacity((size as usize) * (size as usize));
    for row in 0..size {
        let y = 1.0 - (row as f32 + 0.5) * texel;
        for col in 0..size {
            let x = -1.0 + (col as f32 + 0.5) * texel;
            out.push(signed_distance(strokes, [x, y]));
        }
    }
    out
}

/// Rasterises `strokes` to a `size` by `size` coverage mask, a byte a
/// texel (255 inside a stroke, 0 outside, the edge anti-aliased over one
/// texel), row 0 at the top (+y). Deterministic: the same strokes give the
/// same bytes.
pub fn rasterise(strokes: &[Stroke], size: u32) -> Vec<u8> {
    let texel = 2.0 / size.max(1) as f32;
    let mut out = Vec::with_capacity((size * size) as usize);
    for row in 0..size {
        let y = 1.0 - (row as f32 + 0.5) * texel;
        for col in 0..size {
            let x = -1.0 + (col as f32 + 0.5) * texel;
            let d = signed_distance(strokes, [x, y]);
            let coverage = (0.5 - d / texel).clamp(0.0, 1.0);
            out.push((coverage * 255.0).round() as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skulls::ABOUT;

    /// The masks' size in these tests.
    const SIZE: u32 = 64;

    #[test]
    fn the_distance_field_is_negative_inside_a_stroke_and_measures_out_from_it() {
        let line = [l(-0.5, 0.0, 0.5, 0.0)];
        assert!((signed_distance(&line, [0.0, 0.0]) + HALF_WIDTH).abs() < 1e-6);
        assert!((signed_distance(&line, [0.0, 0.3]) - (0.3 - HALF_WIDTH)).abs() < 1e-6);
        // Past the end the cap is round.
        assert!((signed_distance(&line, [0.8, 0.0]) - (0.3 - HALF_WIDTH)).abs() < 1e-6);
        // The nearest stroke wins.
        let two = [l(-0.5, 0.0, 0.5, 0.0), dot(0.0, 0.6, 0.1)];
        assert!((signed_distance(&two, [0.0, 0.45]) - 0.05).abs() < 1e-6);
        // The field holds the same distances at its texels' centres, row
        // 0 at the top.
        let field = distance_field(&two, 8);
        assert_eq!(field.len(), 64);
        assert_eq!(field[8 + 4], signed_distance(&two, [0.125, 0.625]));
        assert!(field[8 + 4] < field[6 * 8 + 4], "the dot is at the top");
        for (i, strokes) in GLYPHS.iter().enumerate() {
            let field = distance_field(strokes, FIELD);
            assert_eq!(field, distance_field(strokes, FIELD), "deterministic");
            let inside = field.iter().filter(|d| **d < 0.0).count();
            assert!(inside > 600, "{} has almost no ink", ABOUT[i].name);
            // The outer texels are clear of every stroke by the box's
            // margin, less half a texel.
            let n = FIELD as usize;
            let margin = 1.0 - BOX - 1.0 / FIELD as f32;
            for k in 0..n {
                for t in [k, (n - 1) * n + k, k * n, k * n + n - 1] {
                    assert!(field[t] >= margin - 1e-4, "{}: {}", ABOUT[i].name, field[t]);
                }
            }
        }
    }

    #[test]
    fn every_skull_has_a_glyph_and_no_two_are_alike() {
        assert_eq!(GLYPHS.len(), ABOUT.len());
        let masks: Vec<Vec<u8>> = (0..ABOUT.len())
            .map(|i| rasterise(glyph(i).expect("a glyph"), SIZE))
            .collect();
        for (i, m) in masks.iter().enumerate() {
            let inked = m.iter().filter(|c| **c > 127).count();
            assert!(inked > 60, "{} has almost no ink ({inked})", ABOUT[i].name);
            assert!(
                inked < m.len() / 2,
                "{} is mostly ink ({inked})",
                ABOUT[i].name
            );
            for (j, other) in masks.iter().enumerate().skip(i + 1) {
                let differ = m
                    .iter()
                    .zip(other)
                    .filter(|(a, b)| a.abs_diff(**b) > 127)
                    .count();
                assert!(
                    differ > 40,
                    "{} and {} look alike ({differ})",
                    ABOUT[i].name,
                    ABOUT[j].name
                );
            }
        }
        assert!(glyph(ABOUT.len()).is_none());
    }

    #[test]
    fn rasterising_is_deterministic() {
        for strokes in GLYPHS {
            assert_eq!(rasterise(strokes, SIZE), rasterise(strokes, SIZE));
            assert_eq!(rasterise(strokes, 32), rasterise(strokes, 32));
        }
    }

    #[test]
    fn every_glyph_stays_inside_its_box() {
        for (i, strokes) in GLYPHS.iter().enumerate() {
            for s in *strokes {
                assert!(
                    s.reach() <= BOX + 1e-4,
                    "{}: {s:?} reaches {}",
                    ABOUT[i].name,
                    s.reach()
                );
            }
            for size in [32, SIZE, 128] {
                let m = rasterise(strokes, size);
                let border = (0..size).flat_map(|k| [(0, k), (size - 1, k), (k, 0), (k, size - 1)]);
                for (row, col) in border {
                    assert_eq!(
                        m[(row * size + col) as usize],
                        0,
                        "{} inks its edge at {size}",
                        ABOUT[i].name
                    );
                }
            }
        }
    }

    #[test]
    fn strokes_draw_where_they_are() {
        let m = rasterise(&[l(-0.5, 0.0, 0.5, 0.0)], 20);
        // Rows 9 and 10 straddle y = 0 (their centres 0.05 off it); the
        // line is 0.14 wide (1.4 texels), so both are mostly inked.
        assert!(m[9 * 20 + 10] > 127 && m[10 * 20 + 10] > 127);
        assert_eq!(m[2 * 20 + 10], 0);
        assert_eq!(m[9 * 20 + 1], 0, "past the line's end");
        // A half arc on the left: ink at its left, none at its right.
        let a = rasterise(&[arc(0.0, 0.0, 0.6, 90.0, 270.0)], 20);
        assert!(a[9 * 20 + 4] > 127 && a[9 * 20 + 15] == 0);
        // A dot is filled.
        let d = rasterise(&[dot(0.0, 0.0, 0.3)], 20);
        assert_eq!(d[9 * 20 + 9], 255);
    }
}
