//! The stage's geometry: every floor, reference and enclosure as convex,
//! planar faces.
//!
//! One face list is both the render mesh ([`Mesh::triangles`], a fan per
//! face) and the collision ([`Mesh::polygons`]), so what the player sees is
//! what the player stands on. Faces wind counter-clockwise seen from their
//! open side, as `collision_build` takes them; a face is built from its
//! corners and its outward normal, and wound to match.
//!
//! Shapes are closed against the floor: a box or ramp has no bottom face, so
//! its solid is the space between its faces and the floor's. An enclosure is
//! a shell: an inner box facing in (four walls and a ceiling, standing on
//! the floor's edge) and an outer box facing out.
//!
//! Every face is drawn with one of two [`Surface`]s. The floor's
//! coordinates span the grid texture once ([`crate::stage::texture::grid`]); every
//! other face takes world-unit coordinates over the tiling ruler
//! ([`crate::stage::texture::ruler`]), so its marks are tenths of a world unit.

use collision_build::{Compiled, Polygon};

use crate::stage::layout::{Bay, Layout, Shape};
use crate::stage::{Error, Result};

/// What a face is drawn with; its index is its material index in the
/// structure and the collision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Surface {
    /// The floor grid.
    Grid,
    /// The ruler.
    Ruler,
}

impl Surface {
    /// Both, in material order.
    pub const ALL: [Surface; 2] = [Surface::Grid, Surface::Ruler];

    /// The material index.
    pub fn index(self) -> u16 {
        match self {
            Surface::Grid => 0,
            Surface::Ruler => 1,
        }
    }

    /// The name its bitmap and shader take.
    pub fn name(self) -> &'static str {
        match self {
            Surface::Grid => "grid",
            Surface::Ruler => "ruler",
        }
    }
}

/// One convex, planar face.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// Its corners, counter-clockwise seen from its open side.
    pub points: Vec<[f32; 3]>,
    /// Texture coordinates per corner.
    pub uvs: Vec<[f32; 2]>,
    /// The unit normal of its open side.
    pub normal: [f32; 3],
    pub surface: Surface,
    /// The bay it belongs to.
    pub bay: usize,
}

/// The whole stage's faces, in a fixed order: bay by bay, the floor, the
/// references in layout order, then the enclosure.
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub faces: Vec<Face>,
}

/// A triangle of the render mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triangle {
    pub points: [[f32; 3]; 3],
    pub uvs: [[f32; 2]; 3],
    pub normal: [f32; 3],
    pub surface: Surface,
}

/// A point or direction.
pub type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 0.0 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        a
    }
}

/// The Newell normal of a ring (not unit).
fn newell(points: &[V3]) -> V3 {
    let mut n = [0.0f32; 3];
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    n
}

/// A placement: turn by `facing` about +z, then move to `at`.
#[derive(Clone, Copy)]
struct Frame {
    at: V3,
    cos: f32,
    sin: f32,
}

impl Frame {
    fn new(at: V3, facing: f32) -> Self {
        Frame {
            at,
            cos: facing.cos(),
            sin: facing.sin(),
        }
    }

    fn dir(&self, v: V3) -> V3 {
        [
            v[0] * self.cos - v[1] * self.sin,
            v[0] * self.sin + v[1] * self.cos,
            v[2],
        ]
    }

    fn point(&self, v: V3) -> V3 {
        let d = self.dir(v);
        [d[0] + self.at[0], d[1] + self.at[1], d[2] + self.at[2]]
    }
}

/// A face in a shape's own frame: corners in any winding and the outward
/// normal.
struct Local {
    points: Vec<V3>,
    out: V3,
}

fn local(points: Vec<V3>, out: V3) -> Local {
    Local { points, out }
}

/// An axis-aligned rectangle on the plane `axis = at`, spanning `a` on the
/// next axis and `b` on the one after, facing `out`.
fn rect(axis: usize, at: f32, a: [f32; 2], b: [f32; 2], out: V3) -> Local {
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let corner = |p: f32, q: f32| {
        let mut c = [0.0; 3];
        c[axis] = at;
        c[u] = p;
        c[v] = q;
        c
    };
    local(
        vec![
            corner(a[0], b[0]),
            corner(a[1], b[0]),
            corner(a[1], b[1]),
            corner(a[0], b[1]),
        ],
        out,
    )
}

/// A box from `lo` to `hi` without its bottom face.
fn cuboid(lo: V3, hi: V3, faces: &mut Vec<Local>) {
    faces.push(rect(
        2,
        hi[2],
        [lo[0], hi[0]],
        [lo[1], hi[1]],
        [0.0, 0.0, 1.0],
    ));
    faces.push(rect(
        0,
        lo[0],
        [lo[1], hi[1]],
        [lo[2], hi[2]],
        [-1.0, 0.0, 0.0],
    ));
    faces.push(rect(
        0,
        hi[0],
        [lo[1], hi[1]],
        [lo[2], hi[2]],
        [1.0, 0.0, 0.0],
    ));
    faces.push(rect(
        1,
        lo[1],
        [lo[2], hi[2]],
        [lo[0], hi[0]],
        [0.0, -1.0, 0.0],
    ));
    faces.push(rect(
        1,
        hi[1],
        [lo[2], hi[2]],
        [lo[0], hi[0]],
        [0.0, 1.0, 0.0],
    ));
}

/// A shape's faces in its own frame (`layout::Shape`).
fn shape_local(shape: &Shape) -> Vec<Local> {
    let mut f = Vec::new();
    match *shape {
        Shape::Box { size } => {
            let (hx, hy) = (size[0] / 2.0, size[1] / 2.0);
            cuboid([-hx, -hy, 0.0], [hx, hy, size[2]], &mut f);
        }
        Shape::Doorway {
            opening,
            post,
            depth,
        } => {
            let (w, h) = (opening[0] / 2.0, opening[1]);
            let (outer, top, d) = (w + post, h + post, depth / 2.0);
            // Front and back, each as three convex pieces.
            for (y, out) in [(-d, [0.0, -1.0, 0.0]), (d, [0.0, 1.0, 0.0])] {
                f.push(rect(1, y, [0.0, h], [-outer, -w], out));
                f.push(rect(1, y, [0.0, h], [w, outer], out));
                f.push(rect(1, y, [h, top], [-outer, outer], out));
            }
            f.push(rect(0, -outer, [-d, d], [0.0, top], [-1.0, 0.0, 0.0]));
            f.push(rect(0, outer, [-d, d], [0.0, top], [1.0, 0.0, 0.0]));
            f.push(rect(0, -w, [-d, d], [0.0, h], [1.0, 0.0, 0.0]));
            f.push(rect(0, w, [-d, d], [0.0, h], [-1.0, 0.0, 0.0]));
            f.push(rect(2, h, [-w, w], [-d, d], [0.0, 0.0, -1.0]));
            f.push(rect(2, top, [-outer, outer], [-d, d], [0.0, 0.0, 1.0]));
        }
        Shape::Ramp { width, rise, angle } => {
            let l = rise / angle.to_radians().tan();
            let (hx, hy) = (l / 2.0, width / 2.0);
            let slope_out = unit([-rise, 0.0, l]);
            f.push(local(
                vec![
                    [-hx, -hy, 0.0],
                    [hx, -hy, rise],
                    [hx, hy, rise],
                    [-hx, hy, 0.0],
                ],
                slope_out,
            ));
            f.push(rect(0, hx, [-hy, hy], [0.0, rise], [1.0, 0.0, 0.0]));
            for (y, out) in [(-hy, [0.0, -1.0, 0.0]), (hy, [0.0, 1.0, 0.0])] {
                f.push(local(vec![[-hx, y, 0.0], [hx, y, 0.0], [hx, y, rise]], out));
            }
        }
        Shape::Stair {
            width,
            rise,
            tread,
            steps,
        } => {
            let hx = tread * steps as f32 / 2.0;
            let hy = width / 2.0;
            for k in 0..steps {
                let x0 = -hx + tread * k as f32;
                let x1 = x0 + tread;
                let (z0, z1) = (rise * k as f32, rise * (k + 1) as f32);
                f.push(rect(0, x0, [-hy, hy], [z0, z1], [-1.0, 0.0, 0.0]));
                f.push(rect(2, z1, [x0, x1], [-hy, hy], [0.0, 0.0, 1.0]));
                for (y, out) in [(-hy, [0.0, -1.0, 0.0]), (hy, [0.0, 1.0, 0.0])] {
                    f.push(rect(1, y, [0.0, z1], [x0, x1], out));
                }
            }
            let top = rise * steps as f32;
            f.push(rect(0, hx, [-hy, hy], [0.0, top], [1.0, 0.0, 0.0]));
        }
    }
    f
}

/// A shape's bounds placed at `at` turned by `facing`:
/// `[x min, x max, y min, y max, z min, z max]`.
pub fn shape_bounds(shape: &Shape, at: V3, facing: f32) -> [f32; 6] {
    let frame = Frame::new(at, facing);
    let mut b = [
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ];
    for face in shape_local(shape) {
        for p in face.points {
            let p = frame.point(p);
            for a in 0..3 {
                b[2 * a] = b[2 * a].min(p[a]);
                b[2 * a + 1] = b[2 * a + 1].max(p[a]);
            }
        }
    }
    b
}

/// World-unit coordinates for a ruler face: the face's two in-plane axes
/// most square to its normal, `v` down the wall on a vertical face.
fn ruler_uv(p: V3, n: V3) -> [f32; 2] {
    let a = [n[0].abs(), n[1].abs(), n[2].abs()];
    if a[2] >= a[0] && a[2] >= a[1] {
        [p[0], -p[1]]
    } else if a[0] >= a[1] {
        [p[1], -p[2]]
    } else {
        [p[0], -p[2]]
    }
}

impl Mesh {
    /// The stage's faces (module docs).
    pub fn build(layout: &Layout) -> Mesh {
        let mut faces = Vec::new();
        for (i, bay) in layout.bays.iter().enumerate() {
            bay_faces(layout, i, bay, &mut faces);
        }
        Mesh { faces }
    }

    /// The render triangles: a fan per face.
    pub fn triangles(&self) -> Vec<Triangle> {
        let mut out = Vec::new();
        for f in &self.faces {
            for k in 1..f.points.len() - 1 {
                out.push(Triangle {
                    points: [f.points[0], f.points[k], f.points[k + 1]],
                    uvs: [f.uvs[0], f.uvs[k], f.uvs[k + 1]],
                    normal: f.normal,
                    surface: f.surface,
                });
            }
        }
        out
    }

    /// The collision polygons, a face each, its surface as its material.
    pub fn polygons(&self) -> Vec<Polygon> {
        self.faces
            .iter()
            .map(|f| Polygon::new(f.points.clone(), f.surface.index()))
            .collect()
    }

    /// Compiles the collision and runs `collision_build::verify::check` on
    /// it with both materials.
    pub fn collision(&self) -> Result<Compiled> {
        let compiled = collision_build::build(&self.polygons())?;
        let problems = collision_build::verify::check(&compiled.bsp, Surface::ALL.len());
        if !problems.is_empty() {
            return Err(Error::Verify(problems.join("; ")));
        }
        Ok(compiled)
    }

    /// `[x min, x max, y min, y max, z min, z max]` over every face.
    pub fn bounds(&self) -> [f32; 6] {
        let mut b = [
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ];
        for f in &self.faces {
            for p in &f.points {
                for a in 0..3 {
                    b[2 * a] = b[2 * a].min(p[a]);
                    b[2 * a + 1] = b[2 * a + 1].max(p[a]);
                }
            }
        }
        b
    }

    /// The nearest face a ray meets: its distance along `dir` (a unit
    /// vector) and the face's index. Faces are hit from either side.
    pub fn first_hit(&self, origin: V3, dir: V3) -> Option<(f32, usize)> {
        let mut best: Option<(f32, usize)> = None;
        for (i, f) in self.faces.iter().enumerate() {
            for k in 1..f.points.len() - 1 {
                if let Some(t) =
                    ray_triangle(origin, dir, f.points[0], f.points[k], f.points[k + 1])
                    && best.is_none_or(|(b, _)| t < b)
                {
                    best = Some((t, i));
                }
            }
        }
        best
    }

    /// Whether a ray from `origin` along `dir` (a unit vector) meets a
    /// face of bay `bay` other than face `skip`, from either side: the
    /// sun's shadow test (`crate::stage::light`).
    pub fn occluded(&self, bay: usize, skip: usize, origin: V3, dir: V3) -> bool {
        self.faces.iter().enumerate().any(|(i, f)| {
            i != skip
                && f.bay == bay
                && (1..f.points.len() - 1).any(|k| {
                    ray_triangle(origin, dir, f.points[0], f.points[k], f.points[k + 1]).is_some()
                })
        })
    }

    /// Every face as little-endian bytes: the build's identity.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for f in &self.faces {
            out.extend_from_slice(&(f.points.len() as u32).to_le_bytes());
            out.extend_from_slice(&f.surface.index().to_le_bytes());
            out.extend_from_slice(&(f.bay as u32).to_le_bytes());
            for v in &f.normal {
                out.extend_from_slice(&v.to_le_bytes());
            }
            for (p, uv) in f.points.iter().zip(&f.uvs) {
                for v in p.iter().chain(uv) {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        out
    }
}

/// Möller–Trumbore: the distance along `dir`, if the ray meets the
/// triangle in front of `origin`.
fn ray_triangle(origin: V3, dir: V3, a: V3, b: V3, c: V3) -> Option<f32> {
    let (e1, e2) = (sub(b, a), sub(c, a));
    let p = cross(dir, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(origin, a);
    let u = dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(dir, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot(e2, q) * inv;
    (t > 1e-6).then_some(t)
}

/// Winds `points` counter-clockwise about `out` and adds the face.
fn push(
    faces: &mut Vec<Face>,
    bay: usize,
    mut points: Vec<V3>,
    out: V3,
    surface: Surface,
    uv: impl Fn(V3) -> [f32; 2],
) {
    if dot(newell(&points), out) < 0.0 {
        points.reverse();
    }
    let normal = unit(newell(&points));
    let uvs = points.iter().map(|&p| uv(p)).collect();
    faces.push(Face {
        points,
        uvs,
        normal,
        surface,
        bay,
    });
}

fn bay_faces(layout: &Layout, index: usize, bay: &Bay, faces: &mut Vec<Face>) {
    let o = bay.origin;
    let h = layout.floor.size / 2.0;
    // The floor: the grid texture spans it once, row 0 at +y.
    let size = layout.floor.size;
    push(
        faces,
        index,
        vec![
            [o[0] - h, o[1] - h, o[2]],
            [o[0] + h, o[1] - h, o[2]],
            [o[0] + h, o[1] + h, o[2]],
            [o[0] - h, o[1] + h, o[2]],
        ],
        [0.0, 0.0, 1.0],
        Surface::Grid,
        |p| [(p[0] - o[0] + h) / size, (h - (p[1] - o[1])) / size],
    );
    for r in &bay.references {
        let frame = Frame::new(r.point, r.facing);
        for l in shape_local(&r.shape) {
            let out = frame.dir(l.out);
            let points = l.points.iter().map(|&p| frame.point(p)).collect();
            push(faces, index, points, out, Surface::Ruler, |p| {
                ruler_uv(p, out)
            });
        }
    }
    if let Some(e) = bay.enclosure {
        let mut shell = Vec::new();
        // The room, facing in: walls on the floor's edge, and the ceiling.
        let (lo, hi) = (
            [o[0] - h, o[1] - h, o[2]],
            [o[0] + h, o[1] + h, o[2] + e.height],
        );
        cuboid(lo, hi, &mut shell);
        for l in &mut shell {
            l.out = [-l.out[0], -l.out[1], -l.out[2]];
        }
        let t = e.thickness;
        cuboid(
            [lo[0] - t, lo[1] - t, lo[2]],
            [hi[0] + t, hi[1] + t, hi[2] + t],
            &mut shell,
        );
        for l in shell {
            let out = l.out;
            push(faces, index, l.points, out, Surface::Ruler, |p| {
                ruler_uv(p, out)
            });
        }
    }
}
