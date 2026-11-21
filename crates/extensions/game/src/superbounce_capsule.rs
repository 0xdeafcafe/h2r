//! Whether the standing capsule fits at a point: the test the superbounce
//! charge's `under` phase and `world.object`'s `physics.standing_fits` use
//! (docs/game/superbounce.md, "Charge").
//!
//! The engine has its own answer, output flags bit 1 of the character
//! physics update (`0x001e55d0`), but keeps it in no field a reader can
//! find, so this is the scan's test run live: a standing capsule (bottom at
//! the origin, `superbounce::STANDING` tall, `superbounce::RADIUS` round)
//! against the loaded structure BSP's collision polygons, in deeper than
//! 5 mm, faces under the feet that look up being support. It is the
//! forced-crouch scan's `capsule_hits` (`crates/live/superbounce/src/crouch.rs`)
//! with the same geometry helpers (`geom.rs`), over a grid of the map's
//! polygons kept from the scan.

use std::collections::HashMap;

use superbounce::{Poly, RADIUS, STANDING, Vec3};

/// The grid's cell, world units.
const CELL: f32 = 2.0;
/// A polygon wider than this many cells on either axis is kept in a list
/// every query tests, not in the grid.
const MAX_SPAN: i32 = 32;
/// A capsule hits a polygon when it is in deeper than this (the scan's).
const HIT_EPS: f32 = 0.005;
/// A face looking up more than this whose nearest point is under the
/// capsule's lower sphere centre is support (the scan's).
const SUPPORT_NZ: f32 = 0.3;

struct Face {
    verts: Vec<Vec3>,
    normal: Vec3,
    min: Vec3,
    max: Vec3,
}

/// A map's collision polygons, gridded for the capsule test.
#[derive(Default)]
pub struct Room {
    faces: Vec<Face>,
    grid: HashMap<(i32, i32), Vec<u32>>,
    big: Vec<u32>,
}

impl std::fmt::Debug for Room {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Room")
            .field("faces", &self.faces.len())
            .finish()
    }
}

fn cell(v: f32) -> i32 {
    (v / CELL).floor() as i32
}

impl Room {
    pub fn new(polys: &[Poly]) -> Room {
        let mut room = Room::default();
        for p in polys {
            if p.verts.len() < 3 || !p.verts.iter().flatten().all(|v| v.is_finite()) {
                continue;
            }
            let mut min = p.verts[0];
            let mut max = p.verts[0];
            for v in &p.verts {
                for i in 0..3 {
                    min[i] = min[i].min(v[i]);
                    max[i] = max[i].max(v[i]);
                }
            }
            let i = room.faces.len() as u32;
            let (x0, x1, y0, y1) = (cell(min[0]), cell(max[0]), cell(min[1]), cell(max[1]));
            if x1 - x0 > MAX_SPAN || y1 - y0 > MAX_SPAN {
                room.big.push(i);
            } else {
                for x in x0..=x1 {
                    for y in y0..=y1 {
                        room.grid.entry((x, y)).or_default().push(i);
                    }
                }
            }
            room.faces.push(Face {
                verts: p.verts.clone(),
                normal: p.normal,
                min,
                max,
            });
        }
        room
    }

    /// No polygons: nothing to test against.
    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    /// Whether a standing capsule with its bottom at `origin` is clear of
    /// every polygon.
    pub fn standing_fits(&self, origin: Vec3) -> bool {
        if !origin.iter().all(|v| v.is_finite()) {
            return true;
        }
        let z0 = origin[2] + RADIUS;
        let z1 = origin[2] + STANDING - RADIUS;
        let (p0, p1) = ([origin[0], origin[1], z0], [origin[0], origin[1], z1]);
        let hits = |i: &u32| {
            let f = &self.faces[*i as usize];
            if f.min[0] > origin[0] + RADIUS
                || f.max[0] < origin[0] - RADIUS
                || f.min[1] > origin[1] + RADIUS
                || f.max[1] < origin[1] - RADIUS
                || f.min[2] > z1 + RADIUS
                || f.max[2] < z0 - RADIUS
            {
                return false;
            }
            let (d, nearest) = seg_poly(p0, p1, &f.verts);
            d < RADIUS - HIT_EPS && !(f.normal[2] > SUPPORT_NZ && nearest[2] < z0 + 1e-3)
        };
        let (x0, x1) = (cell(origin[0] - RADIUS), cell(origin[0] + RADIUS));
        let (y0, y1) = (cell(origin[1] - RADIUS), cell(origin[1] + RADIUS));
        for x in x0..=x1 {
            for y in y0..=y1 {
                if self.grid.get(&(x, y)).is_some_and(|v| v.iter().any(hits)) {
                    return false;
                }
            }
        }
        !self.big.iter().any(hits)
    }
}

// ---- geometry (as crates/live/superbounce/src/geom.rs has it) ----

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn len(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

/// The unit normal of a polygon's own winding (Newell's method).
fn winding_normal(verts: &[Vec3]) -> Option<Vec3> {
    let mut n = [0.0f32; 3];
    for (i, a) in verts.iter().enumerate() {
        let b = verts[(i + 1) % verts.len()];
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let l = len(n);
    (l > 1e-6 && l.is_finite()).then(|| scale(n, 1.0 / l))
}

/// `p` (on the polygon's plane) inside a convex polygon wound
/// counter-clockwise about `n`.
fn inside3(verts: &[Vec3], n: Vec3, p: Vec3) -> bool {
    (0..verts.len()).all(|i| {
        let (a, b) = (verts[i], verts[(i + 1) % verts.len()]);
        dot(cross(sub(b, a), sub(p, a)), n) >= -1e-6
    })
}

/// The closest points of segments `p1 q1` and `p2 q2` (Ericson, Real-Time
/// Collision Detection, 5.1.9).
fn closest_seg_seg(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> (Vec3, Vec3) {
    const EPS: f32 = 1e-12;
    let (d1, d2, r) = (sub(q1, p1), sub(q2, p2), sub(p1, p2));
    let (a, e, f) = (dot(d1, d1), dot(d2, d2), dot(d2, r));
    let (s, t) = if a <= EPS && e <= EPS {
        (0.0, 0.0)
    } else if a <= EPS {
        (0.0, (f / e).clamp(0.0, 1.0))
    } else {
        let c = dot(d1, r);
        if e <= EPS {
            ((-c / a).clamp(0.0, 1.0), 0.0)
        } else {
            let b = dot(d1, d2);
            let denom = a * e - b * b;
            let s = if denom > EPS {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let t = (b * s + f) / e;
            if t < 0.0 {
                ((-c / a).clamp(0.0, 1.0), 0.0)
            } else if t > 1.0 {
                (((b - c) / a).clamp(0.0, 1.0), 1.0)
            } else {
                (s, t)
            }
        }
    };
    (add(p1, scale(d1, s)), add(p2, scale(d2, t)))
}

/// The distance from segment `p0 p1` to a convex polygon, and the
/// polygon's point nearest it.
fn seg_poly(p0: Vec3, p1: Vec3, verts: &[Vec3]) -> (f32, Vec3) {
    let mut best = (f32::INFINITY, verts[0]);
    if let Some(n) = winding_normal(verts) {
        let o = verts[0];
        let (d0, d1) = (dot(n, sub(p0, o)), dot(n, sub(p1, o)));
        if d0 * d1 <= 0.0 && d0 != d1 {
            let x = add(p0, scale(sub(p1, p0), d0 / (d0 - d1)));
            if inside3(verts, n, x) {
                return (0.0, x);
            }
        }
        for (p, d) in [(p0, d0), (p1, d1)] {
            let q = sub(p, scale(n, d));
            if d.abs() < best.0 && inside3(verts, n, q) {
                best = (d.abs(), q);
            }
        }
    }
    for i in 0..verts.len() {
        let (a, b) = (verts[i], verts[(i + 1) % verts.len()]);
        let (c1, c2) = closest_seg_seg(p0, p1, a, b);
        let d = len(sub(c1, c2));
        if d < best.0 {
            best = (d, c2);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(verts: [Vec3; 4], normal: Vec3) -> Poly {
        Poly {
            verts: verts.to_vec(),
            normal,
        }
    }

    /// A floor at z 0 and a ceiling at `ceiling` over x 0..1, y 0..1.
    fn room(ceiling: f32) -> Room {
        let floor = quad(
            [
                [-5.0, -5.0, 0.0],
                [5.0, -5.0, 0.0],
                [5.0, 5.0, 0.0],
                [-5.0, 5.0, 0.0],
            ],
            [0.0, 0.0, 1.0],
        );
        let lid = quad(
            [
                [0.0, 0.0, ceiling],
                [0.0, 1.0, ceiling],
                [1.0, 1.0, ceiling],
                [1.0, 0.0, ceiling],
            ],
            [0.0, 0.0, -1.0],
        );
        Room::new(&[floor, lid])
    }

    #[test]
    fn standing_fits_in_the_open_and_not_under_a_low_ceiling() {
        // Between crouched (0.5) and standing (0.725) height.
        let r = room(0.6);
        assert!(!r.is_empty());
        assert!(!r.standing_fits([0.5, 0.5, 0.0]));
        // Away from the lid; the floor under the feet is support.
        assert!(r.standing_fits([3.0, 3.0, 0.0]));
        // A ceiling over standing height leaves room.
        assert!(room(0.8).standing_fits([0.5, 0.5, 0.0]));
        // Beside the lid, the capsule's radius clear of its edge.
        assert!(r.standing_fits([1.0 + RADIUS + 0.01, 0.5, 0.0]));
        assert!(!r.standing_fits([1.0 + RADIUS - 0.02, 0.5, 0.0]));
    }

    #[test]
    fn an_empty_room_and_a_bad_origin_fit() {
        let r = Room::new(&[]);
        assert!(r.is_empty() && r.standing_fits([0.0; 3]));
        assert!(room(0.6).standing_fits([f32::NAN, 0.5, 0.0]));
    }

    #[test]
    fn a_face_wider_than_the_grid_is_still_tested() {
        let lid = quad(
            [
                [-500.0, -500.0, 0.6],
                [-500.0, 500.0, 0.6],
                [500.0, 500.0, 0.6],
                [500.0, -500.0, 0.6],
            ],
            [0.0, 0.0, -1.0],
        );
        let r = Room::new(&[lid]);
        assert_eq!(r.big.len(), 1);
        assert!(!r.standing_fits([123.0, -77.0, 0.0]));
    }
}
