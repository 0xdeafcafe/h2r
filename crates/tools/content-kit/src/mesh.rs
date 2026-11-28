//! A source's parts as triangles: positions, normals, tangent frames and
//! texture coordinates, each part rigid on one node.
//!
//! Triangles are wound anticlockwise seen from outside, agreeing with their
//! vertex normals: Halo 2's convention for a triangle list
//! (docs/formats/structure-bsp.md, "Triangle lists").
//!
//! - A **lathe** turns a profile of (radius, distance along the axis)
//!   points about its axis. u runs around the axis, v along the profile
//!   from its far end to its near one, so a picture stands upright on an
//!   upright part.
//! - A **sweep** carries a section along a path, the section's plane kept
//!   square to the path and not twisting. u runs along the path, v around
//!   the section.
//! - A **box** has six faces, and with a bevel twelve edge strips and eight
//!   corners. Each face shows the whole region.
//!
//! A smooth outline shares a normal between two faces unless they meet at
//! more than [`CREASE_DEGREES`]; a flat one never shares.

use anyhow::{Result, bail, ensure};

use crate::source::{self, BoxPart, Lathe, Part, Section, Shade, Source, Sweep};

/// Two faces of a smooth outline meeting at more than this keep their own
/// normals: a hard edge.
pub const CREASE_DEGREES: f32 = 40.0;

type V3 = [f32; 3];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub position: V3,
    pub normal: V3,
    /// The direction u grows in.
    pub tangent: V3,
    pub binormal: V3,
    pub uv: [f32; 2],
}

/// One part's triangles.
#[derive(Clone, Debug, PartialEq)]
pub struct PartMesh {
    /// The node the part moves with.
    pub node: usize,
    /// The part's atlas rectangle (u0, v0, u1, v1).
    pub rect: [f32; 4],
    pub vertices: Vec<Vertex>,
    pub triangles: Vec<[u16; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub parts: Vec<PartMesh>,
}

impl Mesh {
    pub fn triangles(&self) -> usize {
        self.parts.iter().map(|p| p.triangles.len()).sum()
    }

    pub fn vertices(&self) -> usize {
        self.parts.iter().map(|p| p.vertices.len()).sum()
    }

    /// (min, max) over every vertex.
    pub fn bounds(&self) -> (V3, V3) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for v in self.parts.iter().flat_map(|p| &p.vertices) {
            for a in 0..3 {
                lo[a] = lo[a].min(v.position[a]);
                hi[a] = hi[a].max(v.position[a]);
            }
        }
        (lo, hi)
    }

    /// The mesh as a Wavefront OBJ, one object per part, for looking at.
    pub fn obj(&self, name: &str) -> String {
        use std::fmt::Write;
        let mut out = format!("# {name}: {} triangles\n", self.triangles());
        let mut base = 1;
        for (i, p) in self.parts.iter().enumerate() {
            let _ = writeln!(out, "o {name}_{i}");
            for v in &p.vertices {
                let [x, y, z] = v.position;
                let _ = writeln!(out, "v {x} {y} {z}");
            }
            for v in &p.vertices {
                let _ = writeln!(out, "vt {} {}", v.uv[0], 1.0 - v.uv[1]);
            }
            for v in &p.vertices {
                let [x, y, z] = v.normal;
                let _ = writeln!(out, "vn {x} {y} {z}");
            }
            for t in &p.triangles {
                let [a, b, c] = t.map(|k| usize::from(k) + base);
                let _ = writeln!(out, "f {a}/{a}/{a} {b}/{b}/{b} {c}/{c}/{c}");
            }
            base += p.vertices.len();
        }
        out
    }
}

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn mul(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn length(a: V3) -> f32 {
    dot(a, a).sqrt()
}

/// `a` at unit length; `fallback` for a vector too short to have a
/// direction.
fn unit_or(a: V3, fallback: V3) -> V3 {
    let l = length(a);
    if l > 1e-9 { mul(a, 1.0 / l) } else { fallback }
}

/// A unit vector square to the unit vector `t`, as near `prefer` as it can
/// be.
fn square_to(t: V3, prefer: V3) -> V3 {
    let pick = |h: V3| sub(h, mul(t, dot(h, t)));
    let n = pick(prefer);
    if length(n) > 0.2 {
        return unit_or(n, [1.0, 0.0, 0.0]);
    }
    let other = if t[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    unit_or(pick(other), [0.0, 0.0, 1.0])
}

/// One point of an outline: where it is, which way its face looks, and how
/// far along the outline it is (0 to 1).
#[derive(Clone, Copy, Debug, PartialEq)]
struct OutlinePoint {
    p: [f32; 2],
    n: [f32; 2],
    s: f32,
}

/// A 2D outline's points with their normals, and its edges as pairs of
/// them. Going along an edge, its face looks to the right: an anticlockwise
/// loop looks outward. Two edges that share a normal at a corner share the
/// point; a hard corner, and a loop's seam, have two.
fn outline(
    pts: &[[f32; 2]],
    closed: bool,
    shade: Shade,
) -> Result<(Vec<OutlinePoint>, Vec<[usize; 2]>)> {
    let n = pts.len();
    ensure!(n >= if closed { 3 } else { 2 }, "an outline of {n} points");
    let edges = if closed { n } else { n - 1 };
    let mut normals = Vec::with_capacity(edges);
    let mut lengths = Vec::with_capacity(edges);
    for k in 0..edges {
        let (a, b) = (pts[k], pts[(k + 1) % n]);
        let d = [b[0] - a[0], b[1] - a[1]];
        let l = d[0].hypot(d[1]);
        ensure!(
            l > 1e-7,
            "outline points {k} and {} are the same",
            (k + 1) % n
        );
        normals.push([d[1] / l, -d[0] / l]);
        lengths.push(l);
    }
    let total: f32 = lengths.iter().sum();
    let crease = CREASE_DEGREES.to_radians().cos();
    // Whether edge `k` and the next share a normal where they meet.
    let joined = |k: usize| {
        let next = (k + 1) % edges;
        (closed || k + 1 < edges)
            && shade == Shade::Smooth
            && normals[k][0] * normals[next][0] + normals[k][1] * normals[next][1] > crease
    };
    let blend = |a: [f32; 2], b: [f32; 2]| {
        let s = [a[0] + b[0], a[1] + b[1]];
        let l = s[0].hypot(s[1]);
        [s[0] / l, s[1] / l]
    };
    let mut out: Vec<OutlinePoint> = Vec::new();
    let mut pairs = Vec::with_capacity(edges);
    let mut along = 0.0f32;
    for k in 0..edges {
        let before = (k + edges - 1) % edges;
        let start = if k > 0 && joined(before) {
            out.len() - 1
        } else {
            let n0 = if k == 0 && closed && joined(before) {
                blend(normals[before], normals[k])
            } else {
                normals[k]
            };
            out.push(OutlinePoint {
                p: pts[k],
                n: n0,
                s: along / total,
            });
            out.len() - 1
        };
        along += lengths[k];
        let n1 = if joined(k) {
            blend(normals[k], normals[(k + 1) % edges])
        } else {
            normals[k]
        };
        out.push(OutlinePoint {
            p: pts[(k + 1) % n],
            n: n1,
            s: along / total,
        });
        pairs.push([start, out.len() - 1]);
    }
    Ok((out, pairs))
}

/// Twice the signed area of a loop: positive when anticlockwise.
fn area2(pts: &[[f32; 2]]) -> f32 {
    (0..pts.len())
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

/// Vertices and triangles being gathered for one part.
#[derive(Default)]
struct Builder {
    vertices: Vec<Vertex>,
    triangles: Vec<[u16; 3]>,
}

impl Builder {
    fn vertex(&mut self, position: V3, normal: V3, tangent: V3, uv: [f32; 2]) -> usize {
        let normal = unit_or(normal, [0.0, 0.0, 1.0]);
        // The tangent square to the normal, then the binormal completes
        // the frame.
        let tangent = square_to(normal, tangent);
        self.vertices.push(Vertex {
            position,
            normal,
            tangent,
            binormal: cross(normal, tangent),
            uv,
        });
        self.vertices.len() - 1
    }

    /// Adds a triangle wound to agree with its vertices' normals; one with
    /// no area is left out.
    fn triangle(&mut self, a: usize, b: usize, c: usize) {
        let p = |i: usize| self.vertices[i].position;
        let face = cross(sub(p(b), p(a)), sub(p(c), p(a)));
        if length(face) < 1e-12 {
            return;
        }
        let n = |i: usize| self.vertices[i].normal;
        let toward = add(add(n(a), n(b)), n(c));
        let t = if dot(face, toward) >= 0.0 {
            [a, b, c]
        } else {
            [a, c, b]
        };
        self.triangles.push(t.map(|i| i as u16));
    }

    fn quad(&mut self, a0: usize, a1: usize, b0: usize, b1: usize) {
        self.triangle(a0, a1, b1);
        self.triangle(a0, b1, b0);
    }

    /// A flat polygon of `points` (in order around it) looking along
    /// `normal`, as a fan.
    fn polygon(&mut self, points: &[(V3, [f32; 2])], normal: V3, tangent: V3) {
        let ids: Vec<usize> = points
            .iter()
            .map(|(p, uv)| self.vertex(*p, normal, tangent, *uv))
            .collect();
        for k in 1..ids.len().saturating_sub(1) {
            self.triangle(ids[0], ids[k], ids[k + 1]);
        }
    }

    fn finish(self, node: usize, rect: [f32; 4]) -> Result<PartMesh> {
        ensure!(
            self.vertices.len() <= usize::from(u16::MAX),
            "a part of {} vertices",
            self.vertices.len()
        );
        ensure!(!self.triangles.is_empty(), "a part with no triangles");
        Ok(PartMesh {
            node,
            rect,
            vertices: self.vertices,
            triangles: self.triangles,
        })
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lathe(l: &Lathe, rect: [f32; 4]) -> Result<Builder> {
    ensure!(
        (3..=64).contains(&l.segments),
        "a lathe of {} segments",
        l.segments
    );
    ensure!(
        l.profile.iter().all(|p| p[0] >= 0.0),
        "a lathe profile with a negative radius"
    );
    let axis = unit_or(l.axis, [0.0, 0.0, 0.0]);
    ensure!(length(axis) > 0.5, "a lathe with no axis");
    let mut pts = l.profile.clone();
    if l.closed {
        ensure!(pts.len() >= 3, "a closed profile of {} points", pts.len());
        if area2(&pts) < 0.0 {
            pts.reverse();
        }
    } else {
        ensure!(pts.len() >= 2, "a profile of {} points", pts.len());
        // Going up the axis, the faces look away from it.
        if pts[pts.len() - 1][1] < pts[0][1] {
            pts.reverse();
        }
        if l.caps {
            if pts[0][0] > 0.0 {
                pts.insert(0, [0.0, pts[0][1]]);
            }
            let last = pts[pts.len() - 1];
            if last[0] > 0.0 {
                pts.push([0.0, last[1]]);
            }
        }
    }
    let (ring, edges) = outline(&pts, l.closed, l.shade)?;
    // e1, e2 and the axis are right-handed, so the angle grows anticlockwise
    // seen from the axis's far end and u runs left to right from outside.
    let e1 = square_to(axis, [1.0, 0.0, 0.0]);
    let e2 = cross(axis, e1);
    let n = l.segments as usize;
    let mut b = Builder::default();
    for q in &ring {
        // An open profile is laid top to bottom: the far end is v0.
        let v = if l.closed { q.s } else { 1.0 - q.s };
        for j in 0..=n {
            let a = std::f32::consts::TAU * j as f32 / n as f32;
            let (sin, cos) = a.sin_cos();
            let radial = add(mul(e1, cos), mul(e2, sin));
            b.vertex(
                add(add(l.origin, mul(axis, q.p[1])), mul(radial, q.p[0])),
                add(mul(radial, q.n[0]), mul(axis, q.n[1])),
                add(mul(e1, -sin), mul(e2, cos)),
                [
                    lerp(rect[0], rect[2], j as f32 / n as f32),
                    lerp(rect[1], rect[3], v),
                ],
            );
        }
    }
    let at = |q: usize, j: usize| q * (n + 1) + j;
    for [q0, q1] in edges {
        for j in 0..n {
            b.quad(at(q0, j), at(q0, j + 1), at(q1, j), at(q1, j + 1));
        }
    }
    Ok(b)
}

fn section_points(s: &Section) -> Result<Vec<[f32; 2]>> {
    Ok(match s {
        Section::Circle(c) => {
            ensure!((3..=32).contains(&c.sides), "a circle of {} sides", c.sides);
            ensure!(c.radius > 0.0, "a circle of radius {}", c.radius);
            (0..c.sides)
                .map(|k| {
                    let a = std::f32::consts::TAU * k as f32 / c.sides as f32;
                    [c.radius * a.cos(), c.radius * a.sin()]
                })
                .collect()
        }
        Section::Polygon(p) => {
            ensure!(p.len() >= 3, "a section of {} points", p.len());
            let mut p = p.clone();
            if area2(&p) < 0.0 {
                p.reverse();
            }
            p
        }
    })
}

fn sweep(s: &Sweep, rect: [f32; 4]) -> Result<Builder> {
    let n = s.path.len();
    ensure!(n >= 2, "a sweep along {n} points");
    ensure!(
        s.scales.is_empty() || s.scales.len() == n,
        "a sweep of {n} points with {} scales",
        s.scales.len()
    );
    let section = section_points(&s.section)?;
    let (ring, edges) = outline(&section, true, s.shade)?;
    // The path's direction at each point, and how far along it the point is.
    let mut tangents = Vec::with_capacity(n);
    let mut along = vec![0.0f32; n];
    for i in 0..n {
        let (a, b) = (s.path[i.saturating_sub(1)], s.path[(i + 1).min(n - 1)]);
        let t = unit_or(sub(b, a), [0.0, 0.0, 0.0]);
        ensure!(
            length(t) > 0.5,
            "sweep path points {i} and its neighbours are the same"
        );
        tangents.push(t);
        if i > 0 {
            along[i] = along[i - 1] + length(sub(s.path[i], s.path[i - 1]));
        }
    }
    let total = along[n - 1];
    ensure!(total > 1e-7, "a sweep along no distance");
    // The section's axes carried along without twisting.
    let mut side = square_to(tangents[0], [0.0, 0.0, 1.0]);
    let mut b = Builder::default();
    let mut frames = Vec::with_capacity(n);
    for i in 0..n {
        let t = tangents[i];
        side = square_to(t, side);
        // side, over and the tangent are right-handed: the section is
        // anticlockwise seen from ahead, and its faces look outward.
        let over = cross(t, side);
        frames.push((side, over));
        let k = s.scales.get(i).copied().unwrap_or(1.0);
        for q in &ring {
            b.vertex(
                add(s.path[i], mul(add(mul(side, q.p[0]), mul(over, q.p[1])), k)),
                add(mul(side, q.n[0]), mul(over, q.n[1])),
                t,
                [
                    lerp(rect[0], rect[2], along[i] / total),
                    lerp(rect[1], rect[3], q.s),
                ],
            );
        }
    }
    let at = |i: usize, q: usize| i * ring.len() + q;
    for i in 0..n - 1 {
        for &[q0, q1] in &edges {
            b.quad(at(i, q0), at(i, q1), at(i + 1, q0), at(i + 1, q1));
        }
    }
    if s.caps {
        let centre = [(rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0];
        for (i, facing) in [(0, -1.0f32), (n - 1, 1.0)] {
            let k = s.scales.get(i).copied().unwrap_or(1.0);
            if k <= 0.0 {
                continue;
            }
            let (side, over) = frames[i];
            let points: Vec<(V3, [f32; 2])> = section
                .iter()
                .map(|p| {
                    (
                        add(s.path[i], mul(add(mul(side, p[0]), mul(over, p[1])), k)),
                        centre,
                    )
                })
                .collect();
            b.polygon(&points, mul(tangents[i], facing), side);
        }
    }
    Ok(b)
}

fn boxed(p: &BoxPart, rect: [f32; 4]) -> Result<Builder> {
    let size = sub(p.max, p.min);
    ensure!(
        size.iter().all(|s| *s > 0.0),
        "a box with no size: {:?} to {:?}",
        p.min,
        p.max
    );
    let smallest = size.iter().copied().fold(f32::MAX, f32::min);
    ensure!(
        p.bevel >= 0.0 && p.bevel < smallest / 2.0,
        "a bevel of {} on a box {smallest} thick",
        p.bevel
    );
    let e = p.bevel;
    // The corner whose signs are `c`, pulled in by the bevel on every axis
    // but `flat`: its point on the face square to `flat`.
    let point = |c: [f32; 3], flat: usize| -> V3 {
        std::array::from_fn(|a| {
            let edge = if c[a] > 0.0 { p.max[a] } else { p.min[a] };
            if a == flat { edge } else { edge - c[a] * e }
        })
    };
    // A face square to axis `k` shows the region with u along one of the
    // other axes and v down the other; the upright faces put v down z.
    let axes = |k: usize| match k {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let uv = |q: V3, k: usize| {
        let (ua, va) = axes(k);
        let f = |a: usize| (q[a] - p.min[a]) / size[a];
        [
            lerp(rect[0], rect[2], f(ua)),
            lerp(rect[1], rect[3], 1.0 - f(va)),
        ]
    };
    let unit = |a: usize, s: f32| -> V3 { std::array::from_fn(|i| if i == a { s } else { 0.0 }) };
    let mut b = Builder::default();
    for k in 0..3 {
        let (ua, va) = axes(k);
        for s in [-1.0f32, 1.0] {
            let corners =
                [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]].map(|[u, v]: [f32; 2]| {
                    let mut c = [0.0; 3];
                    c[k] = s;
                    c[ua] = u;
                    c[va] = v;
                    let q = point(c, k);
                    (q, uv(q, k))
                });
            b.polygon(&corners, unit(k, s), unit(ua, 1.0));
        }
    }
    if e > 0.0 {
        // The strip along each edge, between its two faces.
        for k in 0..3 {
            for l in k + 1..3 {
                let m = 3 - k - l;
                for sk in [-1.0f32, 1.0] {
                    for sl in [-1.0f32, 1.0] {
                        let corner = |sm: f32| {
                            let mut c = [0.0; 3];
                            c[k] = sk;
                            c[l] = sl;
                            c[m] = sm;
                            c
                        };
                        let (lo, hi) = (corner(-1.0), corner(1.0));
                        let quad = [point(lo, k), point(hi, k), point(hi, l), point(lo, l)]
                            .map(|q| (q, uv(q, k)));
                        b.polygon(&quad, add(unit(k, sk), unit(l, sl)), unit(m, 1.0));
                    }
                }
            }
        }
        // The triangle at each corner, between its three faces.
        for i in 0..8 {
            let c: V3 = std::array::from_fn(|a| if i >> a & 1 == 1 { 1.0 } else { -1.0 });
            let tri = [point(c, 0), point(c, 1), point(c, 2)].map(|q| (q, uv(q, 2)));
            b.polygon(&tri, c, unit(0, 1.0));
        }
    }
    Ok(b)
}

/// The source's parts as meshes, in the source's order, on the stick's
/// atlas ([`source::region`]).
pub fn build(source: &Source) -> Result<Mesh> {
    build_on(source, &source::region)
}

/// [`build`] on another atlas: `region` gives a region's rectangle in
/// texture coordinates (u0, v0, u1, v1) by name (`flood_pod`'s atlas).
pub fn build_on(source: &Source, region: &dyn Fn(&str) -> Option<[f32; 4]>) -> Result<Mesh> {
    let mut parts = Vec::with_capacity(source.parts.len());
    for (i, p) in source.parts.iter().enumerate() {
        let Some(rect) = region(p.region()) else {
            bail!("{}: part {i}: no region {:?}", source.name, p.region());
        };
        let Some(node) = source.node(p.node()) else {
            bail!("{}: part {i}: no node {:?}", source.name, p.node());
        };
        let built = match p {
            Part::Lathe(l) => lathe(l, rect),
            Part::Sweep(s) => sweep(s, rect),
            Part::Box(b) => boxed(b, rect),
        };
        let mesh = built
            .and_then(|b| b.finish(node, rect))
            .map_err(|e| anyhow::anyhow!("{}: part {i}: {e}", source.name))?;
        parts.push(mesh);
    }
    Ok(Mesh { parts })
}
