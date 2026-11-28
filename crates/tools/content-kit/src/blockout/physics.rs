//! A definition's `phmo`: its hulls as one fixed rigid body.
//!
//! A piece is a crate because the engine gives a rigid body only to
//! bipeds, vehicles, machines, crates and creatures; a crate's body keeps
//! the motion its tag gives (docs/formats/collision.md, "Which objects get
//! bodies"). The body is written by `h1objects::rebuild::physics_model`,
//! its shape a polyhedron a hull (a list of them for more than one), with
//! the words of [`piece_body`].

use anyhow::{Result, anyhow, ensure};
use forge_shapes::{Hull, newell};
use h1objects::physics;
use h1objects::rebuild::{self, Body, BodyWords};
use map::build::Block;


/// The size class (`+0x1e`) of a body whose bounding sphere has `radius`
/// world units, by the retail bands (V, every rigid body on coagulation,
/// beavercreek and 05a_deltaapproach: 3 from 0.30 to 0.70, 4 from 0.73 to
/// 1.23, 5 from 1.39 to 1.96, 6 from 2.13), each band reaching to the next.
pub fn size_class(radius: f32) -> u16 {
    if radius < 0.72 {
        3
    } else if radius < 1.3 {
        4
    } else if radius < 2.0 {
        5
    } else {
        6
    }
}

/// The body words of a piece whose bounding sphere has `radius`.
pub fn piece_body(radius: f32) -> BodyWords {
    BodyWords {
        flags: crate::blockout::data::data().physics.flags,
        motion: crate::blockout::data::data().physics.motion,
        word_1e: size_class(radius),
    }
}

type V = [f64; 3];

fn wide(p: [f32; 3]) -> V {
    p.map(f64::from)
}
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// A hull as `h1objects`' convex piece: its corners, its face planes
/// (`n . p + w <= 0` inside, `n` a unit normal out), and its volume,
/// centroid and inertia about the centroid for a density of 1, summed over
/// tetrahedra from the corners' mean to each face's fan.
pub fn piece(hull: &Hull) -> Result<physics::Hull> {
    ensure!(!hull.points.is_empty(), "a hull with no corners");
    let n = hull.points.len() as f64;
    let mut c = [0.0f64; 3];
    for p in &hull.points {
        c = add(c, wide(*p));
    }
    let c = c.map(|v| v / n);
    let mut planes = Vec::with_capacity(hull.faces.len());
    let (mut volume, mut first, mut second) = (0.0f64, [0.0f64; 3], [[0.0f64; 3]; 3]);
    for i in 0..hull.faces.len() {
        let ring = hull.ring(i);
        ensure!(ring.len() >= 3, "a hull face of {} corners", ring.len());
        let nv = wide(newell(&ring));
        let len = dot(nv, nv).sqrt();
        ensure!(len > 0.0, "a hull face with no area");
        let unit = nv.map(|v| v / len);
        let w = -dot(unit, wide(ring[0]));
        planes.push([unit[0] as f32, unit[1] as f32, unit[2] as f32, w as f32]);
        let a = sub(wide(ring[0]), c);
        for pair in ring[1..].windows(2) {
            let (b, d) = (sub(wide(pair[0]), c), sub(wide(pair[1]), c));
            let det = dot(a, cross(b, d));
            volume += det / 6.0;
            let s = add(add(a, b), d);
            for (f, sv) in first.iter_mut().zip(s) {
                *f += det / 24.0 * sv;
            }
            for (x, row) in second.iter_mut().enumerate() {
                for (y, m) in row.iter_mut().enumerate() {
                    *m += det / 120.0 * (a[x] * a[y] + b[x] * b[y] + d[x] * d[y] + s[x] * s[y]);
                }
            }
        }
    }
    ensure!(volume > 0.0, "a hull of volume {volume}");
    let g = first.map(|v| v / volume);
    // The second moment about the centroid, then the inertia from it.
    let mut m = second;
    for (x, row) in m.iter_mut().enumerate() {
        for (y, v) in row.iter_mut().enumerate() {
            *v -= volume * g[x] * g[y];
        }
    }
    let trace = m[0][0] + m[1][1] + m[2][2];
    let inertia = std::array::from_fn(|x| {
        std::array::from_fn(|y| ((if x == y { trace } else { 0.0 }) - m[x][y]) as f32)
    });
    Ok(physics::Hull {
        vertices: hull.points.clone(),
        planes,
        volume: volume as f32,
        centroid: std::array::from_fn(|k| (c[k] + g[k]) as f32),
        inertia,
    })
}

/// The radius of the sphere about the corners' box centre holding every
/// corner, as `physics_model` writes the body's bounding sphere.
pub fn bounding_radius(pieces: &[physics::Hull]) -> f32 {
    let all: Vec<[f32; 3]> = pieces.iter().flat_map(|p| p.vertices.iter().copied()).collect();
    let (centre, _) = physics::aabb(&all);
    all.iter()
        .map(|v| {
            ((v[0] - centre[0]).powi(2) + (v[1] - centre[1]).powi(2) + (v[2] - centre[2]).powi(2))
                .sqrt()
        })
        .fold(0.0f32, f32::max)
}

/// The `phmo` of `hulls`: one body on no node, fixed.
pub fn phmo(hulls: &[Hull]) -> Result<Block> {
    let pieces = hulls.iter().map(piece).collect::<Result<Vec<_>>>()?;
    let words = piece_body(bounding_radius(&pieces));
    let body = Body {
        node: -1,
        pieces,
        mass: crate::blockout::data::data().physics.mass,
    };
    rebuild::physics_model(&[body], &[], words, &[]).map_err(|e| anyhow!("phmo: {e}"))
}
