//! Invented shapes, a seeded random source and a brute-force ray, for the
//! tests. Every polygon winds counter-clockwise seen from its open side.

use crate::Polygon;

/// A closed box, solid inside `[lo, hi]`, faces out.
pub fn cube(lo: [f32; 3], hi: [f32; 3]) -> Vec<Polygon> {
    let c = |i: usize| -> [f32; 3] {
        [
            if i & 1 != 0 { hi[0] } else { lo[0] },
            if i & 2 != 0 { hi[1] } else { lo[1] },
            if i & 4 != 0 { hi[2] } else { lo[2] },
        ]
    };
    let q = |is: [usize; 4]| Polygon::new(is.iter().map(|&i| c(i)).collect(), 0);
    vec![
        q([0, 4, 6, 2]), // -x
        q([1, 3, 7, 5]), // +x
        q([0, 1, 5, 4]), // -y
        q([2, 6, 7, 3]), // +y
        q([0, 2, 3, 1]), // -z
        q([4, 5, 7, 6]), // +z
    ]
}

/// The inside of an L-shaped room, open inside, solid outside: the
/// footprint is `[0,4]x[0,2]` and `[0,2]x[2,4]`, from `z = 0` to 3. The
/// floor and ceiling are two convex pieces each (a T-junction at
/// `(0, 2)`).
pub fn l_room() -> Vec<Polygon> {
    let h = 3.0;
    let outline: [[f32; 2]; 6] = [[0.0, 0.0], [4.0, 0.0], [4.0, 2.0], [2.0, 2.0], [2.0, 4.0], [0.0, 4.0]];
    let mut out = Vec::new();
    let floor_a: Vec<[f32; 2]> = vec![[0.0, 0.0], [4.0, 0.0], [4.0, 2.0], [2.0, 2.0], [0.0, 2.0]];
    let floor_b: Vec<[f32; 2]> = vec![[0.0, 2.0], [2.0, 2.0], [2.0, 4.0], [0.0, 4.0]];
    for piece in [&floor_a, &floor_b] {
        // Floor faces up (+z): counter-clockwise from above.
        out.push(Polygon::new(piece.iter().map(|p| [p[0], p[1], 0.0]).collect(), 1));
        // Ceiling faces down: the other way round.
        out.push(Polygon::new(piece.iter().rev().map(|p| [p[0], p[1], h]).collect(), 1));
    }
    for i in 0..outline.len() {
        let (p, q) = (outline[i], outline[(i + 1) % outline.len()]);
        // Inside is left of a counter-clockwise outline; this winding's
        // normal points there.
        out.push(Polygon::new(
            vec![[p[0], p[1], 0.0], [p[0], p[1], h], [q[0], q[1], h], [q[0], q[1], 0.0]],
            0,
        ));
    }
    out
}

/// A closed wedge: from `z = 0` at `x = 0` up to `z = 2` at `x = 4`,
/// `y` in `[0, 2]`.
pub fn ramp() -> Vec<Polygon> {
    let a = [0.0, 0.0, 0.0];
    let b = [4.0, 0.0, 0.0];
    let c = [4.0, 2.0, 0.0];
    let d = [0.0, 2.0, 0.0];
    let e = [4.0, 0.0, 2.0];
    let f = [4.0, 2.0, 2.0];
    vec![
        Polygon::new(vec![a, d, c, b], 0), // bottom, down
        Polygon::new(vec![a, e, f, d], 2), // slope, up and back
        Polygon::new(vec![b, c, f, e], 0), // back wall, +x
        Polygon::new(vec![a, b, e], 0),    // side, -y
        Polygon::new(vec![d, f, c], 0),    // side, +y
    ]
}

/// A two-sided square sheet at `z = 1` over `[0, 2]x[0, 2]`, in two
/// triangles and a quad.
pub fn sheet() -> Vec<Polygon> {
    vec![
        Polygon::new(vec![[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 1.0, 1.0]], 0).two_sided(),
        Polygon::new(vec![[0.0, 0.0, 1.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0]], 0).two_sided(),
        Polygon::new(vec![[1.0, 0.0, 1.0], [2.0, 0.0, 1.0], [2.0, 2.0, 1.0], [1.0, 2.0, 1.0]], 0)
            .two_sided(),
        Polygon::new(vec![[0.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 2.0, 1.0], [0.0, 2.0, 1.0]], 0)
            .two_sided(),
    ]
}

/// A seeded generator (SplitMix64).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `[lo, hi)`.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        let u = (self.next() >> 40) as f32 / (1u64 << 24) as f32;
        lo + (hi - lo) * u
    }

    pub fn point(&mut self, lo: [f32; 3], hi: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|a| self.range(lo[a], hi[a]))
    }
}

/// What a brute-force ray found.
#[derive(Debug, PartialEq)]
pub enum Brute {
    /// First hit: fraction along the segment and the polygon.
    Hit(f64, usize),
    Clear,
    /// Too near an edge, a corner or a plane to call.
    Unsure,
}

type V = [f64; 3];

fn wide(p: [f32; 3]) -> V {
    p.map(f64::from)
}

fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: V, b: V) -> V {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// How close to an edge, corner or plane a segment may pass and still be
/// called.
pub const MARGIN: f64 = 2e-3;

/// The first polygon the segment `a`-`b` meets from its open side (or
/// either side when two-sided), as the engine's projectile test reports.
pub fn brute(polys: &[Polygon], a: [f32; 3], b: [f32; 3]) -> Brute {
    let (a, b) = (wide(a), wide(b));
    let mut best: Option<(f64, usize)> = None;
    for (i, poly) in polys.iter().enumerate() {
        let pts: Vec<V> = poly.points.iter().map(|&p| wide(p)).collect();
        let mut nv = [0.0; 3];
        for k in 0..pts.len() {
            let c = cross(pts[k], pts[(k + 1) % pts.len()]);
            nv = [nv[0] + c[0], nv[1] + c[1], nv[2] + c[2]];
        }
        let l = dot(nv, nv).sqrt();
        let n = nv.map(|c| c / l);
        let d = dot(n, pts[0]);
        let (da, db) = (dot(n, a) - d, dot(n, b) - d);
        if da.abs() < MARGIN || db.abs() < MARGIN {
            // An end on the plane: the walks' sides are a matter of
            // rounding there.
            if (da.signum() != db.signum()) || da.abs() < MARGIN && db.abs() < MARGIN {
                return Brute::Unsure;
            }
            continue;
        }
        let crosses = if poly.flags & crate::TWO_SIDED != 0 {
            (da > 0.0) != (db > 0.0)
        } else {
            da > 0.0 && db < 0.0
        };
        if !crosses {
            continue;
        }
        let t = da / (da - db);
        let p: V = std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t);
        // Signed distance inside every edge (positive inside).
        let mut inside = f64::INFINITY;
        for k in 0..pts.len() {
            let (e0, e1) = (pts[k], pts[(k + 1) % pts.len()]);
            let e = sub(e1, e0);
            let el = dot(e, e).sqrt();
            let inward = cross(n, e).map(|c| c / el);
            inside = inside.min(dot(inward, sub(p, e0)));
        }
        if inside.abs() < MARGIN {
            return Brute::Unsure;
        }
        if inside > 0.0 && best.is_none_or(|(bt, _)| t < bt) {
            best = Some((t, i));
        }
    }
    match best {
        Some((t, i)) => Brute::Hit(t, i),
        None => Brute::Clear,
    }
}
