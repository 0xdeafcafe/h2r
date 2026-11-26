//! Rational points of the regular polygons the cylinder and the arch use.
//!
//! A table, never `sin` or `cos` of the host's library: a point is a pair
//! of integers over [`SCALE`], so a coordinate is one `f32` product and
//! one exact division by a power of two. The cardinal points are exact and
//! each table is symmetric about both axes, so a polygon is too.

/// The denominator of every table entry; one is the unit radius.
pub const SCALE: i32 = 65536;

/// The upper half of the circle, from angle 0 to pi, for polygons of 8 to 24
/// sides (even counts), as `(cos, sin)` over [`SCALE`].
#[rustfmt::skip]
const HALVES: [&[(i32, i32)]; 9] = [
    &[(65536, 0), (46341, 46341), (0, 65536), (-46341, 46341), (-65536, 0)],
    &[(65536, 0), (53020, 38521), (20252, 62328), (-20252, 62328), (-53020, 38521), (-65536, 0)],
    &[(65536, 0), (56756, 32768), (32768, 56756), (0, 65536), (-32768, 56756), (-56756, 32768), (-65536, 0)],
    &[(65536, 0), (59046, 28435), (40861, 51238), (14583, 63893), (-14583, 63893), (-40861, 51238), (-59046, 28435), (-65536, 0)],
    &[(65536, 0), (60547, 25080), (46341, 46341), (25080, 60547), (0, 65536), (-25080, 60547), (-46341, 46341), (-60547, 25080), (-65536, 0)],
    &[(65536, 0), (61584, 22415), (50203, 42126), (32768, 56756), (11380, 64540), (-11380, 64540), (-32768, 56756), (-50203, 42126), (-61584, 22415), (-65536, 0)],
    &[(65536, 0), (62328, 20252), (53020, 38521), (38521, 53020), (20252, 62328), (0, 65536), (-20252, 62328), (-38521, 53020), (-53020, 38521), (-62328, 20252), (-65536, 0)],
    &[(65536, 0), (62881, 18464), (55132, 35431), (42917, 49529), (27225, 59614), (9327, 64869), (-9327, 64869), (-27225, 59614), (-42917, 49529), (-55132, 35431), (-62881, 18464), (-65536, 0)],
    &[(65536, 0), (63303, 16962), (56756, 32768), (46341, 46341), (32768, 56756), (16962, 63303), (0, 65536), (-16962, 63303), (-32768, 56756), (-46341, 46341), (-56756, 32768), (-63303, 16962), (-65536, 0)],
];

/// The polygon sides the table holds: every even count from 8 to 24.
pub fn supported(sides: u16) -> bool {
    (8..=24).contains(&sides) && sides % 2 == 0
}

/// Vertex `k` of the regular polygon of `sides` sides (counter-clockwise from
/// angle 0), as integers over [`SCALE`]. `k` wraps.
///
/// Panics if `sides` is not [`supported`]; the generators check first.
pub fn point(sides: u16, k: u16) -> (i32, i32) {
    let table = HALVES[usize::from((sides - 8) / 2)];
    let k = k % sides;
    let half = sides / 2;
    if k <= half {
        table[usize::from(k)]
    } else {
        let (x, y) = table[usize::from(sides - k)];
        (x, -y)
    }
}

/// `radius * value / SCALE` as an `f32`: one product, one exact division.
pub fn scaled(radius: f32, value: i32) -> f32 {
    (radius * value as f32) / SCALE as f32
}
