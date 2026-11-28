//! The stage's lightmap, computed from the layout's lights
//! (`Layout::lights`): direct light and a constant fill, no bounce.
//!
//! Every bay is lit by its [`Light`]:
//!
//! - **flat**: `lights.flat` grey everywhere, no direction and no shadow;
//! - **dark**: `lights.dark` grey everywhere, near-black;
//! - **day**: the sun (`lights.sun`) on every face square to it, cut by the
//!   shadow of the bay's own faces ([`Mesh::occluded`]), and the sky fill,
//!   which a face takes by how far up it looks: `fill * (0.5 + 0.5 * n.z)`.
//!
//! The lights of a bay reach only that bay, so the three read the same
//! whatever stands beside them. A flat or dark bay needs no texels to say
//! so: all its faces share one 2 by 2 cell of the page. A day face has a
//! cell of its own, `texels_per_unit` texels to the unit across its
//! plane, with a texel's centre at each end of the face's extent, so the
//! face's corners read texels exactly and bilinear sampling never reaches
//! a neighbour's cell. The cells are packed in shelves, the tallest first,
//! into the smallest square page of a power-of-two side that holds them
//! (the page is swizzled by [`crate::stage::kit`], which wants powers of two); if
//! 2048 does not hold them the density halves.
//!
//! A texel is a light level as the engine reads it: a byte of 128 is full
//! light (`half`, Halo 2's lightmap texels are half scale). The same
//! layout gives the same page.

use crate::stage::data::{LightData, data};
use crate::stage::layout::{Layout, Light};
use crate::stage::mesh::{Mesh, V3};
use crate::stage::texture::Image;
use crate::stage::{Error, Result};

/// The stage manifest's lightmap section (`content/stage/kit.yaml`).
pub fn light_data() -> &'static LightData {
    &data().light
}








/// A rectangle of the page, in texels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cell {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Where a face's light is.
#[derive(Clone, Debug, PartialEq)]
enum Place {
    /// Every texel of the cell is the face's light.
    Constant(Cell),
    /// The face's plane laid over the cell: a texel `(i, j)` is the point
    /// `origin + e1 * a + e2 * b`, `a` running over `min[0]..min[0] + size[0]`
    /// as `i` runs over the cell's width.
    Mapped {
        cell: Cell,
        origin: V3,
        e1: V3,
        e2: V3,
        min: [f32; 2],
        size: [f32; 2],
    },
}

/// The lightmap page and where each face's light is on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Baked {
    /// The page, RGBA, rows from the top; 128 is full light.
    pub page: Image,
    /// Texels to the world unit on a day face.
    pub density: f32,
    places: Vec<Place>,
    /// The direction towards the sun, for the faces a bay lights by it.
    day: Vec<Option<V3>>,
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
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

fn unit(a: V3, or: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 1e-6 { scale(a, 1.0 / l) } else { or }
}

fn byte(v: f32) -> u8 {
    (v * light_data().half).round().clamp(0.0, 255.0) as u8
}

/// A packing of `sizes` into a square page of side `side`: each cell's
/// corner, or `None` when they do not fit. Shelves, the tallest first.
fn pack(sizes: &[(u32, u32)], side: u32) -> Option<Vec<(u32, u32)>> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| {
        (
            std::cmp::Reverse(sizes[i].1),
            std::cmp::Reverse(sizes[i].0),
            i,
        )
    });
    let mut at = vec![(0, 0); sizes.len()];
    let (mut x, mut y, mut row) = (0u32, 0u32, 0u32);
    for i in order {
        let (w, h) = sizes[i];
        if w > side || h > side {
            return None;
        }
        if x + w > side {
            x = 0;
            y += row + light_data().gap;
            row = 0;
        }
        if y + h > side {
            return None;
        }
        at[i] = (x, y);
        x += w + light_data().gap;
        row = row.max(h);
    }
    Some(at)
}

/// The texels a day face takes: its extent over its plane, at `density`.
fn extent(size: f32, density: f32) -> u32 {
    ((size * density).ceil() as u32 + 1).clamp(2, light_data().max_page - 2 * light_data().gap)
}

/// The page for `mesh`'s faces (module docs).
pub fn bake(layout: &Layout, mesh: &Mesh) -> Result<Baked> {
    let mut density = light_data().texels_per_unit;
    while density >= light_data().min_density {
        if let Some(b) = try_bake(layout, mesh, density) {
            return Ok(b);
        }
        density /= 2.0;
    }
    Err(Error::Build(format!(
        "the lightmap does not fit a {} texel page at {} texels a unit",
        light_data().max_page,
        light_data().min_density
    )))
}

/// A day face's frame: its plane's two axes and its extent over them.
struct Frame {
    origin: V3,
    e1: V3,
    e2: V3,
    min: [f32; 2],
    size: [f32; 2],
}

fn frame(points: &[V3], normal: V3) -> Frame {
    let origin = points[0];
    let e1 = unit(sub(points[1], origin), [1.0, 0.0, 0.0]);
    let e2 = unit(cross(normal, e1), [0.0, 1.0, 0.0]);
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    for p in points {
        let d = sub(*p, origin);
        for (k, e) in [e1, e2].into_iter().enumerate() {
            let c = dot(d, e);
            lo[k] = lo[k].min(c);
            hi[k] = hi[k].max(c);
        }
    }
    Frame {
        origin,
        e1,
        e2,
        min: lo,
        size: [hi[0] - lo[0], hi[1] - lo[1]],
    }
}

fn try_bake(layout: &Layout, mesh: &Mesh, density: f32) -> Option<Baked> {
    let lights = &layout.lights;
    let sun = lights.sun.direction();

    // What each face needs: a shared constant cell, or a cell of its own.
    enum Need {
        Flat,
        Dark,
        Mapped(Frame, u32, u32),
    }
    let needs: Vec<Need> = mesh
        .faces
        .iter()
        .map(|f| match layout.bays[f.bay].light {
            Light::Flat => Need::Flat,
            Light::Dark => Need::Dark,
            Light::Day => {
                let fr = frame(&f.points, f.normal);
                let (w, h) = (extent(fr.size[0], density), extent(fr.size[1], density));
                Need::Mapped(fr, w, h)
            }
        })
        .collect();

    // The cells, the shared ones first: flat, dark, then the faces.
    let mut sizes = vec![(light_data().constant_side, light_data().constant_side); 2];
    let mut face_cell = Vec::with_capacity(needs.len());
    for need in &needs {
        face_cell.push(match need {
            Need::Flat => 0,
            Need::Dark => 1,
            Need::Mapped(_, w, h) => {
                sizes.push((*w, *h));
                sizes.len() - 1
            }
        });
    }
    let mut side = 64;
    let (side, at) = loop {
        if let Some(at) = pack(&sizes, side) {
            break (side, at);
        }
        if side >= light_data().max_page {
            return None;
        }
        side *= 2;
    };
    let cell = |k: usize| Cell {
        x: at[k].0,
        y: at[k].1,
        w: sizes[k].0,
        h: sizes[k].1,
    };

    let mut page = Image::new(side, side, [0, 0, 0, 0xff]);
    for (k, level) in [(0, lights.flat), (1, lights.dark)] {
        let c = cell(k);
        let b = byte(level);
        for y in c.y..c.y + c.h {
            for x in c.x..c.x + c.w {
                page.set(x, y, [b, b, b, 0xff]);
            }
        }
    }
    let mut places = Vec::with_capacity(needs.len());
    let mut day = Vec::with_capacity(needs.len());
    for (idx, need) in needs.iter().enumerate() {
        match need {
            Need::Flat | Need::Dark => {
                places.push(Place::Constant(cell(face_cell[idx])));
                day.push(None);
            }
            Need::Mapped(fr, w, h) => {
                let c = cell(face_cell[idx]);
                for j in 0..*h {
                    for i in 0..*w {
                        let a = fr.min[0] + fr.size[0] * i as f32 / (*w - 1) as f32;
                        let b = fr.min[1] + fr.size[1] * j as f32 / (*h - 1) as f32;
                        let p = add(fr.origin, add(scale(fr.e1, a), scale(fr.e2, b)));
                        let rgb = day_light(layout, mesh, idx, p, sun);
                        page.set(
                            c.x + i,
                            c.y + j,
                            [byte(rgb[0]), byte(rgb[1]), byte(rgb[2]), 0xff],
                        );
                    }
                }
                places.push(Place::Mapped {
                    cell: c,
                    origin: fr.origin,
                    e1: fr.e1,
                    e2: fr.e2,
                    min: fr.min,
                    size: fr.size,
                });
                day.push(Some(sun));
            }
        }
    }
    Some(Baked {
        page,
        density,
        places,
        day,
    })
}

/// The day bay's light at `p` on face `idx`: the sky fill by how far the
/// face looks up, and the sun unless something of the bay is between.
fn day_light(layout: &Layout, mesh: &Mesh, idx: usize, p: V3, sun: V3) -> [f32; 3] {
    let f = &mesh.faces[idx];
    let lights = &layout.lights;
    let up = 0.5 + 0.5 * f.normal[2];
    let facing = dot(f.normal, sun).max(0.0);
    let lit = facing > 0.0 && !mesh.occluded(f.bay, idx, add(p, scale(f.normal, light_data().bias)), sun);
    let direct = if lit { lights.sun.power * facing } else { 0.0 };
    [0, 1, 2].map(|c| lights.fill[c] * up + lights.sun.color[c] * direct)
}

impl Baked {
    /// The page's side.
    pub fn side(&self) -> u32 {
        self.page.width
    }

    /// The lightmap coordinates, 0 to 1 over the page, of `point` on face
    /// `face`.
    pub fn uv(&self, face: usize, point: V3) -> [f32; 2] {
        let side = self.side() as f32;
        match &self.places[face] {
            Place::Constant(c) => [
                (c.x as f32 + c.w as f32 / 2.0) / side,
                (c.y as f32 + c.h as f32 / 2.0) / side,
            ],
            Place::Mapped {
                cell,
                origin,
                e1,
                e2,
                min,
                size,
            } => {
                let d = sub(point, *origin);
                let t = |c: f32, k: usize| {
                    if size[k] > 1e-6 {
                        ((c - min[k]) / size[k]).clamp(0.0, 1.0)
                    } else {
                        0.0
                    }
                };
                let (ta, tb) = (t(dot(d, *e1), 0), t(dot(d, *e2), 1));
                [
                    (cell.x as f32 + ta * (cell.w - 1) as f32 + 0.5) / side,
                    (cell.y as f32 + tb * (cell.h - 1) as f32 + 0.5) / side,
                ]
            }
        }
    }

    /// The direction of the light a vertex of face `face` (normal
    /// `normal`) is lit from: the sun on a day face that looks towards it,
    /// the face's own normal otherwise.
    pub fn incident(&self, face: usize, normal: V3) -> V3 {
        match self.day[face] {
            Some(sun) if dot(normal, sun) > 0.0 => sun,
            _ => normal,
        }
    }

    /// The light level (1 is full) at `point` on face `face`, as the
    /// engine's bilinear sampling of the page reads it.
    pub fn sample(&self, face: usize, point: V3) -> [f32; 3] {
        let [u, v] = self.uv(face, point);
        let side = self.side();
        let (x, y) = (u * side as f32 - 0.5, v * side as f32 - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |x: f32, y: f32| {
            let px = self.page.get(
                (x.max(0.0) as u32).min(side - 1),
                (y.max(0.0) as u32).min(side - 1),
            );
            [px[0], px[1], px[2]].map(|b| f32::from(b) / light_data().half)
        };
        let mut out = [0.0; 3];
        for (dx, dy, w) in [
            (0.0, 0.0, (1.0 - fx) * (1.0 - fy)),
            (1.0, 0.0, fx * (1.0 - fy)),
            (0.0, 1.0, (1.0 - fx) * fy),
            (1.0, 1.0, fx * fy),
        ] {
            let px = at(x0 + dx, y0 + dy);
            for (o, p) in out.iter_mut().zip(px) {
                *o += p * w;
            }
        }
        out
    }

    /// The cell face `face` reads, and whether it is a constant one.
    pub fn cell(&self, face: usize) -> (Cell, bool) {
        match &self.places[face] {
            Place::Constant(c) => (*c, true),
            Place::Mapped { cell, .. } => (*cell, false),
        }
    }

    /// The page as the lightmap bitmap's texels: B, G, R, X rows from the
    /// top.
    pub fn texels(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.page.pixels.len());
        for p in self.page.pixels.chunks(4) {
            out.extend_from_slice(&[p[2], p[1], p[0], 0xff]);
        }
        out
    }
}
