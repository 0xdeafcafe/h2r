//! A definition's render mesh: the drawn faces of its shell as triangles,
//! one part a shader (docs/game/forge-blockout.md, "The dev textures").
//!
//! A face draws with the material of its role: `Dev` the tile of the
//! role's family, `Map` the named shader, `Nodraw` and `Clip` not at all.
//! The dev view ([`View::Dev`]) draws every face `Dev`.
//!
//! **Plates.** A `Dev` face that is a rectangle carries the plate
//! `dev_texture::label::layout` gives for its size, unless the shape is a
//! rock. The plate is cut out of the face: the face becomes four frame
//! quads around a hole, drawn with the tile, and the hole is filled by the
//! plate's 48 cell quads, drawn with the font atlas, each mapped to its
//! glyph's cell. Nothing is coplanar with a surface behind it, and a
//! face's vertex count does not depend on its size. The plate's axes are
//! the face's own: on a wall, up is the edge nearest world up and x runs to
//! the right seen from outside; on a floor or a ceiling, x is the edge
//! nearest world x.
//!
//! **Coordinates.** A tile face's texture coordinates are the shell's
//! (`forge_shapes::Face::uv`: position along the face's two dominant axes
//! over a tile), so pieces whose origins are a whole number of tiles apart
//! continue each other's grid. A cell's are its atlas cell's, `v` down.
//!
//! **Normals** are each face's own, but on a rock, where a vertex takes the
//! mean of the faces at it within 40 degrees of its face.

use std::collections::{BTreeMap, HashMap};

use dev_texture::label::{self, COLUMNS, ROWS};
use forge_shapes::{Face, Material, PieceDef, Role, Shape, Solid, newell, units};

/// What a part is drawn with.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Paint {
    /// The dev tile of a role's family.
    Tile(Role),
    /// The plates' font atlas.
    Font,
    /// A shader of the base map, by tag path.
    Map(String),
}

/// Which render model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// The definition's materials.
    Look,
    /// Every role `Dev`.
    Dev,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub tangent: [f32; 3],
    pub binormal: [f32; 3],
    pub uv: [f32; 2],
}

/// The triangles of one paint, indices into its own vertices.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub paint: Paint,
    pub vertices: Vec<Vertex>,
    pub triangles: Vec<[u16; 3]>,
}

/// A plate on a face: which face, its size in units, its layout, and where
/// its cells start in the font part (four vertices a cell, row by row).
#[derive(Clone, Debug, PartialEq)]
pub struct FacePlate {
    pub face: usize,
    pub width: u32,
    pub height: u32,
    pub plate: label::Plate,
    pub first_vertex: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    /// In [`Paint`] order.
    pub parts: Vec<Part>,
    pub plates: Vec<FacePlate>,
}

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    pub fn part(&self, paint: &Paint) -> Option<&Part> {
        self.parts.iter().find(|p| p.paint == *paint)
    }

    pub fn triangles(&self) -> usize {
        self.parts.iter().map(|p| p.triangles.len()).sum()
    }

    pub fn vertices(&self) -> usize {
        self.parts.iter().map(|p| p.vertices.len()).sum()
    }
}

/// The name a plate prints for a shape: its spelling in capitals, words
/// apart (`FLOOR PLATE`).
pub fn label_name(shape: Shape) -> String {
    shape.name().replace('_', " ").to_ascii_uppercase()
}

/// The paint of a face of `role` in `view`, `None` when it is not drawn.
pub fn paint(def: &PieceDef, role: Role, view: View) -> Option<Paint> {
    let material = match view {
        View::Dev => &Material::Dev,
        View::Look => def.materials.get(role),
    };
    match material {
        Material::Dev => Some(Paint::Tile(role)),
        Material::Map(key) => Some(Paint::Map(key.path.clone())),
        Material::Nodraw | Material::Clip => None,
    }
}

type P = [f32; 3];

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: P, b: P) -> P {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: P, s: f32) -> P {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: P, b: P) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: P, b: P) -> P {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn unit(a: P) -> P {
    let l = dot(a, a).sqrt();
    if l > 0.0 { scale(a, 1.0 / l) } else { a }
}

/// The two axes a face's texture coordinates run along, by its normal: the
/// rule `forge_shapes::Face::uv` is made by.
fn uv_axes(n: P) -> (usize, usize) {
    let (ax, ay, az) = (n[0].abs(), n[1].abs(), n[2].abs());
    if ax >= ay && ax >= az {
        (1, 2)
    } else if ay >= az {
        (0, 2)
    } else {
        (0, 1)
    }
}

fn axis(i: usize) -> P {
    let mut a = [0.0; 3];
    a[i] = 1.0;
    a
}

/// The tile coordinates of `p` on a face of normal `n`.
fn tile_uv(n: P, p: P) -> [f32; 2] {
    let (u, v) = uv_axes(n);
    [p[u] * 100.0 / units::TILE, p[v] * 100.0 / units::TILE]
}

/// A rectangular face's own frame: its lower-left corner, its unit axes
/// (`r` along, `u` up, `r x u` its outward normal), its exact size in
/// units, and its four corners by (x, y) as the shell has them.
struct Frame {
    o: P,
    r: P,
    u: P,
    w: f32,
    h: f32,
    corners: [[P; 2]; 2],
}

impl Frame {
    /// The world point at (`x`, `y`) units on the face; a corner is the
    /// shell's own, so the frame meets the next face without a gap.
    fn at(&self, x: f32, y: f32) -> P {
        let cx = if x == 0.0 { Some(0) } else if x == self.w { Some(1) } else { None };
        let cy = if y == 0.0 { Some(0) } else if y == self.h { Some(1) } else { None };
        if let (Some(i), Some(j)) = (cx, cy) {
            return self.corners[i][j];
        }
        add(add(self.o, scale(self.r, x / 100.0)), scale(self.u, y / 100.0))
    }
}

/// Slack for a rectangle's right angles and closing corner, relative to
/// its sides.
const SQUARE_SLACK: f32 = 1e-5;

/// The face's frame when it is a rectangle.
fn rectangle(face: &Face, n: P) -> Option<Frame> {
    let [p0, p1, p2, p3] = face.ring[..] else {
        return None;
    };
    let (a, b) = (sub(p1, p0), sub(p3, p0));
    let (la, lb) = (dot(a, a).sqrt(), dot(b, b).sqrt());
    if la == 0.0 || lb == 0.0 {
        return None;
    }
    let close = sub(p2, add(p0, add(a, b)));
    if dot(a, b).abs() > SQUARE_SLACK * la * lb
        || dot(close, close).sqrt() > SQUARE_SLACK * (la + lb)
    {
        return None;
    }
    let (ua, ub) = (scale(a, 1.0 / la), scale(b, 1.0 / lb));
    let flip = |v: P, k: usize| if v[k] < 0.0 { scale(v, -1.0) } else { v };
    let (r, u, w, h) = if n[2].abs() > std::f32::consts::FRAC_1_SQRT_2 {
        // A floor or a ceiling: x along the edge nearest world x.
        let (r, lr, lu) = if ua[0].abs() >= ub[0].abs() { (ua, la, lb) } else { (ub, lb, la) };
        let r = flip(r, 0);
        (r, unit(cross(n, r)), lr, lu)
    } else {
        // A wall: up along the edge nearest world up.
        let (u, lu, lr) = if ua[2].abs() >= ub[2].abs() { (ua, la, lb) } else { (ub, lb, la) };
        let u = flip(u, 2);
        (unit(cross(u, n)), u, lr, lu)
    };
    let ring = [p0, p1, p2, p3];
    let o = *ring
        .iter()
        .min_by(|p, q| (dot(**p, r) + dot(**p, u)).total_cmp(&(dot(**q, r) + dot(**q, u))))?;
    let mut corners = [[o; 2]; 2];
    for p in ring {
        let d = sub(p, o);
        let i = usize::from(dot(d, r) > w * 0.5);
        let j = usize::from(dot(d, u) > h * 0.5);
        corners[i][j] = p;
    }
    Some(Frame {
        o,
        r,
        u,
        w: w * 100.0,
        h: h * 100.0,
        corners,
    })
}

#[derive(Default)]
struct Parts {
    parts: BTreeMap<Paint, Part>,
}

impl Parts {
    fn part(&mut self, paint: &Paint) -> &mut Part {
        self.parts.entry(paint.clone()).or_insert_with(|| Part {
            paint: paint.clone(),
            vertices: Vec::new(),
            triangles: Vec::new(),
        })
    }

    /// A convex ring as a fan, wound as given.
    fn fan(&mut self, paint: &Paint, vertices: &[Vertex]) {
        let part = self.part(paint);
        let base = part.vertices.len() as u16;
        part.vertices.extend_from_slice(vertices);
        for k in 1..vertices.len() as u16 - 1 {
            part.triangles.push([base, base + k, base + k + 1]);
        }
    }
}

/// The mesh of `solid`, the shell of `def`, in `view`.
pub fn build(def: &PieceDef, solid: &Solid, view: View) -> Mesh {
    let normals: Vec<P> = solid.shell.iter().map(|f| unit(newell(&f.ring))).collect();
    let smooth = def.shape.is_natural().then(|| smoothing(solid, &normals));
    let name = label_name(def.shape);
    let mut parts = Parts::default();
    let mut plates = Vec::new();
    for (i, face) in solid.shell.iter().enumerate() {
        let Some(paint) = paint(def, face.role, view) else {
            continue;
        };
        let n = normals[i];
        let normal_at = |p: P| match &smooth {
            Some(s) => s.at(p, n),
            None => n,
        };
        let plated = match (&paint, def.shape.is_natural()) {
            (Paint::Tile(_), false) => rectangle(face, n).and_then(|frame| {
                let (wi, hi) = (frame.w.round() as u32, frame.h.round() as u32);
                label::layout(&name, wi, hi).map(|plate| (frame, wi, hi, plate))
            }),
            _ => None,
        };
        let Some((frame, width, height, plate)) = plated else {
            let (ta, tb) = uv_axes(newell(&face.ring));
            let vertices: Vec<Vertex> = face
                .ring
                .iter()
                .zip(&face.uv)
                .map(|(&p, &uv)| Vertex {
                    position: p,
                    normal: normal_at(p),
                    tangent: axis(ta),
                    binormal: axis(tb),
                    uv,
                })
                .collect();
            parts.fan(&paint, &vertices);
            continue;
        };
        let area = newell(&face.ring);
        let (ta, tb) = uv_axes(area);
        let quad = |x0: f32, y0: f32, x1: f32, y1: f32| -> Vec<Vertex> {
            [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
                .into_iter()
                .map(|(x, y)| {
                    let p = frame.at(x, y);
                    Vertex {
                        position: p,
                        normal: n,
                        tangent: axis(ta),
                        binormal: axis(tb),
                        uv: tile_uv(area, p),
                    }
                })
                .collect()
        };
        let cell = plate.h as f32;
        let (l, b) = (plate.left, plate.bottom);
        let (rr, t) = (l + COLUMNS as f32 * cell, b + ROWS as f32 * cell);
        let (w, h) = (frame.w, frame.h);
        let frame_quads = [(0.0, 0.0, w, b), (0.0, t, w, h), (0.0, b, l, t), (rr, b, w, t)];
        for (x0, y0, x1, y1) in frame_quads {
            parts.fan(&paint, &quad(x0, y0, x1, y1));
        }
        let first_vertex = parts.part(&Paint::Font).vertices.len();
        for row in 0..ROWS {
            for col in 0..COLUMNS {
                let [x0, y0, x1, y1] = plate.cell_rect(row, col);
                let [u0, v0, u1, v1] = plate.cell_uv(row, col);
                let corners = [
                    (x0, y0, u0, v1),
                    (x1, y0, u1, v1),
                    (x1, y1, u1, v0),
                    (x0, y1, u0, v0),
                ];
                let vertices: Vec<Vertex> = corners
                    .into_iter()
                    .map(|(x, y, s, v)| Vertex {
                        position: frame.at(x, y),
                        normal: n,
                        tangent: frame.r,
                        binormal: scale(frame.u, -1.0),
                        uv: [s, v],
                    })
                    .collect();
                parts.fan(&Paint::Font, &vertices);
            }
        }
        plates.push(FacePlate {
            face: i,
            width,
            height,
            plate,
            first_vertex,
        });
    }
    Mesh {
        parts: parts.parts.into_values().collect(),
        plates,
    }
}

/// Cosine of the widest angle between two faces whose normals a rock's
/// vertex shares.
const SMOOTH_COS: f32 = 0.766_044_4;

/// Every face normal at each shell position, in shell order.
struct Smoothing {
    at: HashMap<[u32; 3], Vec<P>>,
}

impl Smoothing {
    /// The normal of a vertex at `p` on a face of normal `n`: the mean of
    /// the normals there within 40 degrees of `n`, `n` among them.
    fn at(&self, p: P, n: P) -> P {
        let Some(list) = self.at.get(&p.map(f32::to_bits)) else {
            return n;
        };
        let mut sum = [0.0f32; 3];
        for m in list.iter().filter(|m| dot(**m, n) >= SMOOTH_COS) {
            sum = add(sum, *m);
        }
        unit(sum)
    }
}

fn smoothing(solid: &Solid, normals: &[P]) -> Smoothing {
    let mut at: HashMap<[u32; 3], Vec<P>> = HashMap::new();
    for (face, n) in solid.shell.iter().zip(normals) {
        for p in &face.ring {
            at.entry(p.map(f32::to_bits)).or_default().push(*n);
        }
    }
    Smoothing { at }
}
