//! The natural set (docs/game/forge-blockout.md, "The natural set"): five
//! seeded rocks, each one convex hull.
//!
//! A generator makes a few dozen points on a form that suits the shape,
//! pushes them about with seeded noise so the outline is irregular and
//! weathered, fits the cloud to the unit box (so the bounds are the size and
//! the lowest point is on z = 0) and takes the exact hull of the points
//! rounded to a grid. With at most 34 points a hull has at most 64 faces.
//!
//! Only `+ - * /`, `floor`, `round` and `sqrt` of `f32` are used, and the
//! round points of a ring come from [`crate::circle`], never `sin` or `cos`.

use crate::circle;
use crate::def::Shape;
use crate::error::{Error, Result};
use crate::hull::{self, IPoint};
use crate::noise::{self, Stream};
use crate::solid::{Hull, P3, Solid};
use crate::units::world;

/// The grid the points are rounded to, per axis, across the bounds.
const GRID: f32 = 4096.0;

/// The most points a generator makes; a hull of `n` points has at most
/// `2n - 4` faces, so 34 points keep a rock within 64.
pub const MAX_POINTS: usize = 34;

/// The geometry of a natural shape of `size` units from `seed`.
pub fn generate(shape: Shape, size: [u16; 3], seed: u16) -> Result<Solid> {
    let key = noise::hash(u32::from(seed), shape as i32 + 1, 0x5eed, 3);
    let points = match shape {
        Shape::Boulder => boulder(key),
        Shape::Slab => slab(key),
        Shape::Cliff => cliff(key),
        Shape::TerrainWedge => terrain_wedge(key),
        Shape::Spire => spire(key),
        _ => {
            return Err(Error::NoRock {
                shape,
                why: "not a natural shape",
            });
        }
    };
    debug_assert!(points.len() <= MAX_POINTS);
    let grid = fit(&points).ok_or(Error::NoRock {
        shape,
        why: "its points are flat",
    })?;
    let rock = hull::convex_hull(&grid).ok_or(Error::NoRock {
        shape,
        why: "its points are flat",
    })?;
    let [ux, uy, uz] = size;
    let (sx, sy, sz) = (world(ux), world(uy), world(uz));
    let scale = |p: &IPoint| -> P3 {
        [
            (p[0] as f32 * sx) / GRID,
            (p[1] as f32 * sy) / GRID,
            (p[2] as f32 * sz) / GRID,
        ]
    };
    let hull = Hull {
        points: rock.points.iter().map(scale).collect(),
        faces: rock.faces,
    };
    Ok(Solid::convex(hull))
}

/// The points stretched so each axis spans the unit box exactly, then rounded
/// to the grid. `None` when an axis has no extent.
fn fit(points: &[P3]) -> Option<Vec<IPoint>> {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    if (0..3).any(|k| hi[k] - lo[k] < 1e-4) {
        return None;
    }
    Some(
        points
            .iter()
            .map(|p| {
                let at = |k: usize| (((p[k] - lo[k]) / (hi[k] - lo[k])) * GRID).round() as i64;
                [at(0), at(1), at(2)]
            })
            .collect(),
    )
}

fn normalise(v: P3) -> P3 {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 0.0 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Vertex `k` of the regular polygon of `sides` sides, as unit-radius floats.
fn ring_point(sides: u16, k: u16) -> (f32, f32) {
    let (c, s) = circle::point(sides, k);
    (
        c as f32 / circle::SCALE as f32,
        s as f32 / circle::SCALE as f32,
    )
}

/// A rounded lump, widest near its base: the 26 directions of a cube's
/// corners, edges and faces, each turned a little, then set at a radius
/// that wanders with low-frequency noise and a jitter of its own. The nine
/// lowest directions sit on the ground plane, a flat footprint, and the top
/// is drawn in.
fn boulder(key: u32) -> Vec<P3> {
    let mut r = Stream::new(key);
    let lean = [r.signed() * 0.08, r.signed() * 0.08];
    let stretch = [r.range(0.85, 1.15), r.range(0.85, 1.15)];
    let off = [r.range(0.0, 64.0), r.range(0.0, 64.0), r.range(0.0, 64.0)];
    let mut points = Vec::new();
    for dx in -1i32..=1 {
        for dy in -1i32..=1 {
            for dz in -1i32..=1 {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                let turned = [
                    dx as f32 + 0.35 * r.signed(),
                    dy as f32 + 0.35 * r.signed(),
                    dz as f32 + 0.35 * r.signed(),
                ];
                let n = normalise(turned);
                let q = [
                    n[0] * 1.7 + off[0],
                    n[1] * 1.7 + off[1],
                    n[2] * 1.7 + off[2],
                ];
                let radius = 0.5 * (1.0 + 0.30 * noise::fbm(key, q, 3) + 0.12 * r.signed());
                let squeeze = 1.0 - 0.3 * n[2].max(0.0);
                points.push([
                    0.5 + lean[0] * n[2] + n[0] * radius * stretch[0] * squeeze,
                    0.5 + lean[1] * n[2] + n[1] * radius * stretch[1] * squeeze,
                    if dz < 0 {
                        0.0
                    } else {
                        (0.25 + n[2] * radius * 0.95).max(0.0)
                    },
                ]);
            }
        }
    }
    points
}

/// A flat rock with a walkable top: three irregular rings of ten (a base
/// drawn in, a belly, and a top edge drawn in a little with a small noise in
/// its height) and a crown at the full height.
fn slab(key: u32) -> Vec<P3> {
    let mut r = Stream::new(key);
    let aspect = [r.range(0.92, 1.0), r.range(0.8, 1.0)];
    let mut points = Vec::new();
    for (z, inset, rise) in [(0.0, 0.10, 0.0), (0.55, 0.0, 0.05), (0.95, 0.06, 0.04)] {
        for k in 0..10 {
            let (c, s) = ring_point(10, k);
            let radius = (1.0 - inset) * r.range(0.82, 1.0);
            let slide = r.signed() * 0.12;
            points.push([
                0.5 + 0.5 * aspect[0] * radius * (c - slide * s),
                0.5 + 0.5 * aspect[1] * radius * (s + slide * c),
                z + rise * r.signed(),
            ]);
        }
    }
    points.push([0.5 + 0.1 * r.signed(), 0.5 + 0.1 * r.signed(), 1.0]);
    points
}

/// A tall face. The front is a lattice of five columns and four rows pushed
/// out by noise to craggy depths; the columns at either end stay in the planes
/// x = 0 and x = 1 so cliffs stand side by side; the back is the plane y = 1,
/// flat, and a little higher than the front's top so the cap leans forward.
fn cliff(key: u32) -> Vec<P3> {
    let mut r = Stream::new(key);
    let off = [r.range(0.0, 64.0), r.range(0.0, 64.0)];
    let mut points = Vec::new();
    for row in 0u16..4 {
        let zf = f32::from(row) / 3.0;
        let z = if row == 0 {
            0.0
        } else {
            zf * 0.92 + 0.06 * r.signed()
        };
        for col in 0u16..5 {
            let xf = f32::from(col) / 4.0;
            let x = if col == 0 || col == 4 {
                xf
            } else {
                xf + 0.07 * r.signed()
            };
            let crag = noise::fbm(key, [xf * 2.5 + off[0], zf * 3.0 + off[1], 0.5], 3);
            let depth = (0.30 + 0.30 * crag + 0.08 * r.signed()).max(0.0);
            points.push([x, depth + 0.12 * zf, z]);
        }
    }
    for (x, z) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
        points.push([x, 1.0, z]);
    }
    points
}

/// A ramp of ground rising along x, six columns by five rows of an uneven
/// surface and the four corners of its foot. The rows at the edges stay in
/// the planes y = 0 and y = 1, the toe is at ground level and the back is the
/// plane x = 1.
fn terrain_wedge(key: u32) -> Vec<P3> {
    let mut r = Stream::new(key);
    let off = [r.range(0.0, 64.0), r.range(0.0, 64.0)];
    let mut points = Vec::new();
    for col in 0u16..6 {
        let xf = f32::from(col) / 5.0;
        for row in 0u16..5 {
            let yf = f32::from(row) / 4.0;
            let x = if col == 0 || col == 5 {
                xf
            } else {
                xf + 0.04 * r.signed()
            };
            let y = if row == 0 || row == 4 {
                yf
            } else {
                yf + 0.05 * r.signed()
            };
            let bump = noise::fbm(key, [xf * 3.0 + off[0], yf * 3.0 + off[1], 0.5], 3);
            let z = if col == 0 {
                0.0
            } else {
                (x + 0.06 * bump).max(0.0)
            };
            points.push([x, y, z]);
        }
    }
    for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
        points.push([x, y, 0.0]);
    }
    points
}

/// A tall narrow rock: four rings of eight narrowing upward, each leaning a
/// little more than the last with ragged radii and its own ledge, and an apex
/// at the full height.
fn spire(key: u32) -> Vec<P3> {
    let mut r = Stream::new(key);
    let lean = [r.signed() * 0.10, r.signed() * 0.10];
    let mut points = Vec::new();
    for (z, radius) in [(0.0, 0.5), (0.3, 0.40), (0.62, 0.27), (0.86, 0.14)] {
        let ledge = r.range(0.9, 1.15);
        for k in 0..8 {
            let (c, s) = ring_point(8, k);
            let ragged = radius * ledge * r.range(0.7, 1.05);
            let slide = r.signed() * 0.15;
            points.push([
                0.5 + lean[0] * z + ragged * (c - slide * s),
                0.5 + lean[1] * z + ragged * (s + slide * c),
                z,
            ]);
        }
    }
    points.push([
        0.5 + lean[0] * 1.2 + 0.04 * r.signed(),
        0.5 + lean[1] * 1.2 + 0.04 * r.signed(),
        1.0,
    ]);
    points
}
