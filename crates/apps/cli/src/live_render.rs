//! The pre-drawn map views: a small software renderer for `live_geometry`.
//!
//! Orthographic only, so every view's world-to-pixel mapping is an exact
//! affine matrix (`views.json`). Each view is rasterised at
//! [`SUPERSAMPLE`]× into a depth and normal buffer (back faces culled, so
//! the level's outer shell does not hide it), then shaded: Lambert from a
//! fixed sun with a shadow map, hemispheric ambient, screen-space ambient
//! occlusion from the depth buffer, a height tint, and thin lines at
//! silhouettes, depth steps and creases. Samples are averaged down to the
//! output size; uncovered pixels stay transparent.

use crate::live_geometry::{Bounds, Class, LevelMesh, cross, dot, linear_to_srgb, normalize};

/// Elevation of the iso views, in degrees.
pub const ELEVATION_DEG: f32 = 30.0;
/// Samples per output pixel along each axis.
pub const SUPERSAMPLE: usize = 2;
/// Shadow map size, texels per side.
pub const SHADOW_SIZE: usize = 2048;

/// One view to draw.
#[derive(Clone, Copy, Debug)]
pub struct ViewSpec {
    pub id: &'static str,
    pub width: u32,
    pub height: u32,
    /// Where the camera stands, in degrees from +x (east) towards +y
    /// (north); `None` is straight down with north up.
    pub azimuth: Option<f32>,
}

/// The v1 views (contract).
pub const VIEWS: [ViewSpec; 5] = [
    ViewSpec {
        id: "top",
        width: 2048,
        height: 2048,
        azimuth: None,
    },
    ViewSpec {
        id: "iso-ne",
        width: 2048,
        height: 1536,
        azimuth: Some(45.0),
    },
    ViewSpec {
        id: "iso-nw",
        width: 2048,
        height: 1536,
        azimuth: Some(135.0),
    },
    ViewSpec {
        id: "iso-sw",
        width: 2048,
        height: 1536,
        azimuth: Some(225.0),
    },
    ViewSpec {
        id: "iso-se",
        width: 2048,
        height: 1536,
        azimuth: Some(315.0),
    },
];

/// Towards the sun: high in the north-west, the hillshade convention, so
/// shadows fall to the lower right of the top view.
pub fn sun() -> [f32; 3] {
    normalize([-0.42, 0.38, 0.82])
}

/// An orthographic camera. `px = scale·(p·right) + offset[0]`,
/// `py = offset[1] − scale·(p·up)`, depth `p·forward` (smaller is nearer).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
    pub scale: f32,
    pub offset: [f32; 2],
}

impl Camera {
    /// Pixel x, pixel y, depth.
    pub fn project(&self, p: [f32; 3]) -> [f32; 3] {
        [
            self.scale * dot(p, self.right) + self.offset[0],
            self.offset[1] - self.scale * dot(p, self.up),
            dot(p, self.forward),
        ]
    }

    /// The world point at pixel `(x, y)` and `depth`.
    pub fn unproject(&self, x: f32, y: f32, depth: f32) -> [f32; 3] {
        let a = (x - self.offset[0]) / self.scale;
        let b = (self.offset[1] - y) / self.scale;
        std::array::from_fn(|k| self.right[k] * a + self.up[k] * b + self.forward[k] * depth)
    }

    /// The contract's 2×4 matrix.
    pub fn world_to_pixel(&self) -> [[f64; 4]; 2] {
        let s = f64::from(self.scale);
        let r = self.right.map(f64::from);
        let u = self.up.map(f64::from);
        [
            [s * r[0], s * r[1], s * r[2], f64::from(self.offset[0])],
            [-s * u[0], -s * u[1], -s * u[2], f64::from(self.offset[1])],
        ]
    }
}

/// Right, up and forward for a view.
pub fn basis(azimuth: Option<f32>) -> [[f32; 3]; 3] {
    let Some(az) = azimuth else {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]];
    };
    let (a, e) = (az.to_radians(), ELEVATION_DEG.to_radians());
    let to_camera = [e.cos() * a.cos(), e.cos() * a.sin(), e.sin()];
    let forward = to_camera.map(|v| -v);
    let right = normalize(cross(forward, [0.0, 0.0, 1.0]));
    let up = cross(right, forward);
    [right, up, forward]
}

/// A camera with the given axes that fits `b` into `w`×`h` pixels, centred,
/// with `margin` pixels clear on the tighter side.
pub fn fit_box(axes: [[f32; 3]; 3], b: &Bounds, w: f32, h: f32, margin: f32) -> Camera {
    fit_points(axes, b.corners(), w, h, margin)
}

/// A camera with the given axes that fits every point into `w`×`h` pixels,
/// centred, with `margin` pixels clear on the tighter side.
pub fn fit_points(
    axes: [[f32; 3]; 3],
    points: impl IntoIterator<Item = [f32; 3]>,
    w: f32,
    h: f32,
    margin: f32,
) -> Camera {
    let [right, up, forward] = axes;
    let (mut r0, mut r1, mut u0, mut u1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for c in points {
        let (r, u) = (dot(c, right), dot(c, up));
        r0 = r0.min(r);
        r1 = r1.max(r);
        u0 = u0.min(u);
        u1 = u1.max(u);
    }
    let scale =
        ((w - 2.0 * margin) / (r1 - r0).max(1e-3)).min((h - 2.0 * margin) / (u1 - u0).max(1e-3));
    Camera {
        right,
        up,
        forward,
        scale,
        offset: [
            w / 2.0 - scale * (r0 + r1) / 2.0,
            h / 2.0 + scale * (u0 + u1) / 2.0,
        ],
    }
}

/// The camera for a view of a level: fitted to what is drawn, the drawn
/// triangles cut to the clip box of `frame`, not to the box itself, so the
/// level fills the picture from every side.
pub fn fit(spec: &ViewSpec, mesh: &LevelMesh, frame: &Bounds) -> Camera {
    let (w, h) = (spec.width as f32, spec.height as f32);
    let axes = basis(spec.azimuth);
    let margin = 0.03 * w.min(h);
    let points = drawn_points(mesh, &clip_box(frame));
    if points.is_empty() {
        return fit_box(axes, frame, w, h, margin);
    }
    fit_points(axes, points, w, h, margin)
}

/// The corners of every drawn (non-glass) triangle cut to `clip`.
pub fn drawn_points(mesh: &LevelMesh, clip: &Bounds) -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    for (class, p) in mesh.primitives.iter().chain(&mesh.objects) {
        if *class == Class::Glass {
            continue;
        }
        for t in p.indices.as_chunks::<3>().0 {
            let world = t.map(|i| p.positions[i as usize]);
            if world.iter().all(|&v| inside(clip, v)) {
                out.extend(world);
            } else {
                out.extend(clip_polygon(&world, clip));
            }
        }
    }
    out
}

/// A convex polygon cut to a box (Sutherland–Hodgman, one plane at a time).
pub fn clip_polygon(poly: &[[f32; 3]], b: &Bounds) -> Vec<[f32; 3]> {
    let mut cur = poly.to_vec();
    for k in 0..3 {
        for (bound, keep_below) in [(b.min[k], false), (b.max[k], true)] {
            if cur.is_empty() {
                return cur;
            }
            let ins = |p: &[f32; 3]| {
                if keep_below {
                    p[k] <= bound
                } else {
                    p[k] >= bound
                }
            };
            let mut next = Vec::with_capacity(cur.len() + 2);
            for i in 0..cur.len() {
                let (a, c) = (cur[i], cur[(i + 1) % cur.len()]);
                if ins(&a) {
                    next.push(a);
                }
                if ins(&a) != ins(&c) {
                    let t = (bound - a[k]) / (c[k] - a[k]);
                    let mut q: [f32; 3] = std::array::from_fn(|j| a[j] + (c[j] - a[j]) * t);
                    q[k] = bound;
                    next.push(q);
                }
            }
            cur = next;
        }
    }
    cur
}

/// Calls `f(index, depth, barycentrics)` for every pixel whose centre is
/// inside the screen triangle `v` (x, y in pixels, z depth), either winding.
fn raster(v: [[f32; 3]; 3], w: usize, h: usize, mut f: impl FnMut(usize, f32, [f32; 3])) {
    let edge = |a: [f32; 3], b: [f32; 3], x: f32, y: f32| {
        (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0])
    };
    let area = edge(v[0], v[1], v[2][0], v[2][1]);
    if area.is_nan() || area.abs() <= 1e-12 {
        return;
    }
    let lo = |k: usize| v.iter().map(|p| p[k]).fold(f32::INFINITY, f32::min);
    let hi = |k: usize| v.iter().map(|p| p[k]).fold(f32::NEG_INFINITY, f32::max);
    let x0 = (lo(0) - 0.5).ceil().max(0.0) as usize;
    let y0 = (lo(1) - 0.5).ceil().max(0.0) as usize;
    let x1 = ((hi(0) - 0.5).floor() + 1.0).clamp(0.0, w as f32) as usize;
    let y1 = ((hi(1) - 0.5).floor() + 1.0).clamp(0.0, h as f32) as usize;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let inv = 1.0 / area;
    // Barycentrics step linearly across a row.
    let (dx0, dx1) = (-(v[2][1] - v[1][1]) * inv, -(v[0][1] - v[2][1]) * inv);
    let start = x0 as f32 + 0.5;
    for y in y0..y1 {
        let py = y as f32 + 0.5;
        let mut w0 = edge(v[1], v[2], start, py) * inv;
        let mut w1 = edge(v[2], v[0], start, py) * inv;
        let row = y * w;
        for x in x0..x1 {
            let w2 = 1.0 - w0 - w1;
            // A hair of slack, so a sample on an edge shared by two
            // triangles is never missed by both.
            if w0 >= -1e-6 && w1 >= -1e-6 && w2 >= -1e-6 {
                f(
                    row + x,
                    w0 * v[0][2] + w1 * v[1][2] + w2 * v[2][2],
                    [w0, w1, w2],
                );
            }
            w0 += dx0;
            w1 += dx1;
        }
    }
}

/// Depth from the sun, for shadows.
pub struct ShadowMap {
    cam: Camera,
    depth: Vec<f32>,
    /// World units per texel.
    texel: f32,
    sun: [f32; 3],
}

impl ShadowMap {
    /// How lit `p` (normal `n`) is, 0..1, from 3×3 taps.
    pub fn light(&self, p: [f32; 3], n: [f32; 3]) -> f32 {
        let q = self.cam.project(p);
        let slope = 1.0 + 3.0 * (1.0 - dot(n, self.sun).clamp(0.0, 1.0));
        let d = q[2] - self.texel * 1.5 * slope;
        let (cx, cy) = (q[0].floor() as i64, q[1].floor() as i64);
        let mut lit = 0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (x, y) = (cx + dx, cy + dy);
                let inside =
                    (0..SHADOW_SIZE as i64).contains(&x) && (0..SHADOW_SIZE as i64).contains(&y);
                if !inside || d <= self.depth[y as usize * SHADOW_SIZE + x as usize] {
                    lit += 1;
                }
            }
        }
        lit as f32 / 9.0
    }
}

/// Whether `p` is inside `b`.
fn inside(b: &Bounds, p: [f32; 3]) -> bool {
    (0..3).all(|k| p[k] >= b.min[k] && p[k] <= b.max[k])
}

/// Where the views cut the level: the frame and a sliver more.
pub fn clip_box(frame: &Bounds) -> Bounds {
    frame.grown(0.01)
}

/// A world point inside triangle `p` from barycentrics.
fn at(p: &[[f32; 3]; 3], bc: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|k| bc[0] * p[0][k] + bc[1] * p[1][k] + bc[2] * p[2][k])
}

/// The shadow map of a level: sun-facing, opaque triangles inside the clip
/// box, so nothing cut away casts a shadow.
pub fn shadow_map(mesh: &LevelMesh, frame: &Bounds) -> ShadowMap {
    let clip = clip_box(frame);
    let l = sun();
    let forward = l.map(|v| -v);
    let right = normalize(cross(forward, [0.0, 0.0, 1.0]));
    let up = cross(right, forward);
    let n = SHADOW_SIZE as f32;
    let cam = fit_box([right, up, forward], &frame.grown(0.15), n, n, 0.0);
    let mut depth = vec![f32::INFINITY; SHADOW_SIZE * SHADOW_SIZE];
    for (class, p) in mesh.primitives.iter().chain(&mesh.objects) {
        if *class == Class::Glass {
            continue;
        }
        let q: Vec<[f32; 3]> = p.positions.iter().map(|&v| cam.project(v)).collect();
        for t in p.indices.as_chunks::<3>().0 {
            let [a, b, c] = t.map(|i| i as usize);
            let face = cross(
                sub(p.positions[b], p.positions[a]),
                sub(p.positions[c], p.positions[a]),
            );
            if dot(face, l) <= 0.0 {
                continue;
            }
            let world = [p.positions[a], p.positions[b], p.positions[c]];
            let whole = world.iter().all(|&v| inside(&clip, v));
            raster([q[a], q[b], q[c]], SHADOW_SIZE, SHADOW_SIZE, |i, z, bc| {
                if z < depth[i] && (whole || inside(&clip, at(&world, bc))) {
                    depth[i] = z;
                }
            });
        }
    }
    ShadowMap {
        cam,
        depth,
        texel: 1.0 / cam.scale,
        sun: l,
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn pack(n: [f32; 3], class: usize) -> [i8; 4] {
    let q = |v: f32| (v.clamp(-1.0, 1.0) * 127.0).round() as i8;
    [q(n[0]), q(n[1]), q(n[2]), class as i8]
}

fn unpack(p: [i8; 4]) -> [f32; 3] {
    [p[0], p[1], p[2]].map(|v| f32::from(v) / 127.0)
}

/// Whether two packed normals differ by more than about 37 degrees.
fn crease(a: [i8; 4], b: [i8; 4]) -> bool {
    let d: i32 = (0..3).map(|k| i32::from(a[k]) * i32::from(b[k])).sum();
    d < 12_900 // 0.8 · 127²
}

const SKY: [f32; 3] = [0.40, 0.44, 0.52];
const GROUND: [f32; 3] = [0.20, 0.195, 0.19];
const SUN_COLOUR: [f32; 3] = [0.80, 0.76, 0.68];
/// Ambient-occlusion radius, world units.
const AO_RADIUS: f32 = 1.2;
/// Depth-step line threshold (second difference), world units.
const STEP: f32 = 0.15;

/// Draws one view: `w`×`h` straight-alpha RGBA. With `cutaway`, roofs and
/// ceilings over walkable space are cut away ([`Cutaway`]).
#[allow(clippy::too_many_arguments)]
pub fn render(
    mesh: &LevelMesh,
    frame: &Bounds,
    shadow: &ShadowMap,
    cam: &Camera,
    w: usize,
    h: usize,
    cutaway: bool,
) -> Vec<u8> {
    let ss = SUPERSAMPLE;
    let clip = clip_box(frame);
    let cut = cutaway.then(|| Cutaway::new(mesh, cam, &clip, w, h));
    let (depth, normal) = gbuffer(mesh, cam, &clip, cut.as_ref(), ss, w, h);
    shade(&depth, &normal, frame, shadow, cut.as_ref(), cam, ss, w, h)
}

/// Layers kept per cutaway cell, nearest first.
const LAYERS: usize = 8;
/// Smallest gap between a ceiling and the floor under it that counts as
/// walkable space, world units (a crouching player is about 0.5).
pub const MIN_HEADROOM: f32 = 0.6;
/// Cut regions narrower than twice this (world units) are kept, so bridges,
/// walkways and eaves over open ground stay in the picture.
pub const MIN_CUT_RADIUS: f32 = 0.8;
/// Output pixels per cutaway cell, along each axis.
const CUT_CELL: usize = 2;

/// Where a view cuts the level open to show the floors under roofs.
///
/// Render geometry faces into the playable space, so along a view ray a
/// surface facing the camera starts solid and one facing away (a ceiling,
/// seen from above) ends it. Per cell of a grid over the picture, every
/// layer is rasterised, both facings; the nearest ceiling with a front face
/// above it and a floor at least [`MIN_HEADROOM`] beneath it marks a
/// covered walkable space, and everything nearer than that ceiling is cut.
/// Cut regions are then opened (eroded and dilated by [`MIN_CUT_RADIUS`]) so
/// only room-sized ones stay cut. Cells over walls have no ceiling and keep
/// their top, so walls read as their tops around opened rooms. The cut is a
/// depth per cell; nothing moves, so `world_to_pixel` is unchanged.
pub struct Cutaway {
    cols: usize,
    /// Fragments nearer than this are not drawn; `-inf` where nothing is cut.
    near: Vec<f32>,
}

impl Cutaway {
    pub fn new(mesh: &LevelMesh, cam: &Camera, clip: &Bounds, w: usize, h: usize) -> Cutaway {
        let (cols, rows) = (w.div_ceil(CUT_CELL), h.div_ceil(CUT_CELL));
        let n = cols * rows;
        let mut depth = vec![f32::INFINITY; n * LAYERS];
        let mut front = vec![false; n * LAYERS];
        let mut used = vec![0u8; n];
        let f = 1.0 / CUT_CELL as f32;
        for (class, p) in &mesh.primitives {
            if *class == Class::Glass {
                continue;
            }
            let q: Vec<[f32; 3]> = p
                .positions
                .iter()
                .map(|&v| {
                    let s = cam.project(v);
                    [s[0] * f, s[1] * f, s[2]]
                })
                .collect();
            for t in p.indices.as_chunks::<3>().0 {
                let [a, b, c] = t.map(|i| i as usize);
                let world = [p.positions[a], p.positions[b], p.positions[c]];
                if outside(clip, &world) {
                    continue;
                }
                let facing = dot(
                    cross(sub(world[1], world[0]), sub(world[2], world[0])),
                    cam.forward,
                ) < 0.0;
                let whole = world.iter().all(|&v| inside(clip, v));
                raster([q[a], q[b], q[c]], cols, rows, |i, z, bc| {
                    if !(whole || inside(clip, at(&world, bc))) {
                        return;
                    }
                    let (d, fr) = (
                        &mut depth[i * LAYERS..(i + 1) * LAYERS],
                        &mut front[i * LAYERS..(i + 1) * LAYERS],
                    );
                    let k = usize::from(used[i]);
                    let mut j = k.min(LAYERS - 1);
                    if k == LAYERS && z >= d[LAYERS - 1] {
                        return;
                    }
                    while j > 0 && d[j - 1] > z {
                        d[j] = d[j - 1];
                        fr[j] = fr[j - 1];
                        j -= 1;
                    }
                    d[j] = z;
                    fr[j] = facing;
                    used[i] = (k + 1).min(LAYERS) as u8;
                });
            }
        }
        let mut near = vec![f32::NEG_INFINITY; n];
        for i in 0..n {
            let k = usize::from(used[i]);
            let (d, fr) = (
                &depth[i * LAYERS..i * LAYERS + k],
                &front[i * LAYERS..i * LAYERS + k],
            );
            let mut covered = false;
            for j in 0..k.saturating_sub(1) {
                if fr[j] {
                    covered = true;
                } else if covered && fr[j + 1] && d[j + 1] - d[j] >= MIN_HEADROOM {
                    near[i] = d[j] + 0.02;
                    break;
                }
            }
        }
        // Opening: keep only cells whose whole neighbourhood is cut, then
        // give back every cell next to one of those.
        let r = (MIN_CUT_RADIUS * cam.scale / CUT_CELL as f32)
            .round()
            .max(1.0) as usize;
        let cut: Vec<bool> = near.iter().map(|v| v.is_finite()).collect();
        let core = box_all(&cut, cols, rows, r);
        let kept = box_any(&core, cols, rows, r);
        for (v, k) in near.iter_mut().zip(&kept) {
            if !k {
                *v = f32::NEG_INFINITY;
            }
        }
        // Kept cells the rasteriser missed (a sample exactly on a shared
        // edge) take a neighbour's cut.
        for _ in 0..r {
            let mut changed = false;
            for i in 0..n {
                if !kept[i] || near[i].is_finite() {
                    continue;
                }
                let (x, y) = (i % cols, i / cols);
                let neighbours = [
                    (x > 0).then(|| i - 1),
                    (x + 1 < cols).then(|| i + 1),
                    (y > 0).then(|| i - cols),
                    (y + 1 < rows).then(|| i + cols),
                ];
                if let Some(v) = neighbours
                    .into_iter()
                    .flatten()
                    .map(|j| near[j])
                    .find(|v| v.is_finite())
                {
                    near[i] = v;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        Cutaway { cols, near }
    }

    /// The cut depth at a pixel of a view drawn at `ss`×.
    fn near(&self, x: usize, y: usize, ss: usize) -> f32 {
        let c = CUT_CELL * ss;
        self.near[(y / c) * self.cols + x / c]
    }

    /// Whether anything is cut at the cell of a supersampled pixel.
    fn is_cut(&self, x: usize, y: usize, ss: usize) -> bool {
        self.near(x, y, ss).is_finite()
    }
}

/// Window sums of a mask over (2r+1)² boxes, via a summed-area table; cells
/// outside the grid count as unset.
fn box_count(mask: &[bool], cols: usize, rows: usize, r: usize) -> impl Fn(usize, usize) -> usize {
    let mut sat = vec![0usize; (cols + 1) * (rows + 1)];
    for y in 0..rows {
        let mut row = 0;
        for x in 0..cols {
            row += usize::from(mask[y * cols + x]);
            sat[(y + 1) * (cols + 1) + x + 1] = sat[y * (cols + 1) + x + 1] + row;
        }
    }
    move |x, y| {
        let (x0, y0) = (x.saturating_sub(r), y.saturating_sub(r));
        let (x1, y1) = ((x + r + 1).min(cols), (y + r + 1).min(rows));
        sat[y1 * (cols + 1) + x1] + sat[y0 * (cols + 1) + x0]
            - sat[y0 * (cols + 1) + x1]
            - sat[y1 * (cols + 1) + x0]
    }
}

/// Erosion: set where the whole (2r+1)² box is set and inside the grid.
fn box_all(mask: &[bool], cols: usize, rows: usize, r: usize) -> Vec<bool> {
    let count = box_count(mask, cols, rows, r);
    let full = (2 * r + 1) * (2 * r + 1);
    (0..rows * cols)
        .map(|i| count(i % cols, i / cols) == full)
        .collect()
}

/// Dilation: set where anything in the (2r+1)² box is set.
fn box_any(mask: &[bool], cols: usize, rows: usize, r: usize) -> Vec<bool> {
    let count = box_count(mask, cols, rows, r);
    (0..rows * cols)
        .map(|i| count(i % cols, i / cols) > 0)
        .collect()
}

/// Whether a triangle lies wholly beyond one face of `clip`.
fn outside(clip: &Bounds, world: &[[f32; 3]; 3]) -> bool {
    (0..3).any(|k| {
        world.iter().all(|v| v[k] < clip.min[k]) || world.iter().all(|v| v[k] > clip.max[k])
    })
}

/// Depth (`p·forward`, infinite where empty) and packed normal and class of
/// the nearest front face inside `clip`, and not cut away, at every sample
/// of a `w`×`h` view drawn at `ss`×.
pub fn gbuffer(
    mesh: &LevelMesh,
    cam: &Camera,
    clip: &Bounds,
    cut: Option<&Cutaway>,
    ss: usize,
    w: usize,
    h: usize,
) -> (Vec<f32>, Vec<[i8; 4]>) {
    let (hw, hh) = (w * ss, h * ss);
    let mut depth = vec![f32::INFINITY; hw * hh];
    let mut normal = vec![[0i8; 4]; hw * hh];
    // Placed objects are never cut away.
    let level = mesh.primitives.iter().map(|x| (x, cut));
    let objects = mesh.objects.iter().map(|x| (x, None));
    for ((class, p), cut) in level.chain(objects) {
        if *class == Class::Glass {
            continue;
        }
        let ci = Class::ALL.iter().position(|c| c == class).unwrap_or(0);
        let q: Vec<[f32; 3]> = p
            .positions
            .iter()
            .map(|&v| {
                let s = cam.project(v);
                [s[0] * ss as f32, s[1] * ss as f32, s[2]]
            })
            .collect();
        for t in p.indices.as_chunks::<3>().0 {
            let [a, b, c] = t.map(|i| i as usize);
            let face = cross(
                sub(p.positions[b], p.positions[a]),
                sub(p.positions[c], p.positions[a]),
            );
            if dot(face, cam.forward) >= 0.0 {
                continue;
            }
            let ns = [p.normals[a], p.normals[b], p.normals[c]];
            let world = [p.positions[a], p.positions[b], p.positions[c]];
            if outside(clip, &world) {
                continue;
            }
            let whole = world.iter().all(|&v| inside(clip, v));
            raster([q[a], q[b], q[c]], hw, hh, |i, z, bc| {
                if z < depth[i]
                    && cut.is_none_or(|c| z >= c.near(i % hw, i / hw, ss))
                    && (whole || inside(clip, at(&world, bc)))
                {
                    depth[i] = z;
                    let n: [f32; 3] = std::array::from_fn(|k| {
                        bc[0] * ns[0][k] + bc[1] * ns[1][k] + bc[2] * ns[2][k]
                    });
                    normal[i] = pack(normalize(n), ci);
                }
            });
        }
    }
    (depth, normal)
}

#[allow(clippy::too_many_arguments)]
fn shade(
    depth: &[f32],
    normal: &[[i8; 4]],
    frame: &Bounds,
    shadow: &ShadowMap,
    cut: Option<&Cutaway>,
    cam: &Camera,
    ss: usize,
    w: usize,
    h: usize,
) -> Vec<u8> {
    let (hw, hh) = (w * ss, h * ss);
    let l = sun();
    let hscale = cam.scale * ss as f32;
    let ao_px = (AO_RADIUS * hscale).clamp(3.0, 48.0);
    let zspan = (frame.max[2] - frame.min[2]).max(1e-3);
    let mut sum = vec![[0f32; 3]; w * h];
    let mut count = vec![0u8; w * h];
    let mut cache = vec![(f32::NAN, 0f32, 0f32); w * h];
    let colours = Class::ALL.map(Class::linear);
    // Eight directions at full and half radius, turned by one of eight
    // per-pixel jitters.
    let offsets: [[(i64, i64); 8]; 8] = std::array::from_fn(|j| {
        std::array::from_fn(|k| {
            let a = k as f32 * std::f32::consts::FRAC_PI_4 + j as f32 * 0.098;
            let r = ao_px * if k % 2 == 0 { 1.0 } else { 0.5 };
            ((a.cos() * r) as i64, (a.sin() * r) as i64)
        })
    });
    // One sample's colour (linear), or `None` where nothing was drawn.
    let shade_one = |x: usize, y: usize, cache: &mut [(f32, f32, f32)]| -> Option<[f32; 3]> {
        let i = y * hw + x;
        let d = depth[i];
        if !d.is_finite() {
            return None;
        }
        let n = unpack(normal[i]);
        let p = cam.unproject(
            (x as f32 + 0.5) / ss as f32,
            (y as f32 + 0.5) / ss as f32,
            d,
        );
        let at = |dx: i64, dy: i64| -> Option<usize> {
            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
            ((0..hw as i64).contains(&nx) && (0..hh as i64).contains(&ny))
                .then(|| ny as usize * hw + nx as usize)
        };

        // Ambient occlusion (nearer neighbours around the sample) and
        // shadow, shared by the samples of one output pixel that lie on
        // the same surface.
        let o = (y / ss) * w + x / ss;
        let sun_facing = dot(n, l).max(0.0);
        let (ao, lit) = match cache[o] {
            (cd, a, s) if (cd - d).abs() < 0.25 => (a, s),
            _ => {
                let mut occ = 0.0;
                for &(ox, oy) in &offsets[(x * 7 + y * 13) % 8] {
                    let Some(j) = at(ox, oy) else {
                        continue;
                    };
                    let dz = d - depth[j];
                    if dz > 0.0 && dz.is_finite() {
                        occ += (dz / AO_RADIUS).min(1.0) * (3.0 * AO_RADIUS / dz).min(1.0);
                    }
                }
                let ao = (1.0 - 0.7 * occ / 8.0).clamp(0.3, 1.0);
                // Under a cut-away roof the roof's shadow is gone too.
                let lit = if sun_facing <= 0.0 {
                    0.0
                } else if cut.is_some_and(|c| c.is_cut(x, y, ss)) {
                    1.0
                } else {
                    shadow.light(p, n)
                };
                cache[o] = (d, ao, lit);
                (ao, lit)
            }
        };

        // Lines: silhouettes, depth steps, creases.
        let mut line = 1.0f32;
        let sample = |dx, dy| at(dx, dy).filter(|&j| depth[j].is_finite());
        let (l_, r_, u_, d_) = (sample(-1, 0), sample(1, 0), sample(0, -1), sample(0, 1));
        if let (Some(l_), Some(r_), Some(u_), Some(d_)) = (l_, r_, u_, d_) {
            let lap_x = depth[l_] + depth[r_] - 2.0 * d;
            let lap_y = depth[u_] + depth[d_] - 2.0 * d;
            if lap_x.abs() > STEP || lap_y.abs() > STEP {
                line = 0.66;
            } else if crease(normal[i], normal[r_]) || crease(normal[i], normal[d_]) {
                line = 0.8;
            }
        } else {
            line = 0.62;
        }

        let base = colours[normal[i][3] as usize];
        let up = 0.5 + 0.5 * n[2];
        let sunlit = sun_facing * lit;
        let height = 0.55 + 0.45 * ((p[2] - frame.min[2]) / zspan).clamp(0.0, 1.0);
        Some(std::array::from_fn(|k| {
            let amb = GROUND[k] + (SKY[k] - GROUND[k]) * up;
            base[k] * (amb * ao + SUN_COLOUR[k] * sunlit) * height * line
        }))
    };
    // A pixel whose samples all lie on one flat surface is shaded once.
    let full = (ss * ss) as f32;
    for oy in 0..h {
        for ox in 0..w {
            let o = oy * w + ox;
            let (x0, y0) = (ox * ss, oy * ss);
            let first = y0 * hw + x0;
            let (d0, n0) = (depth[first], normal[first]);
            let uniform = d0.is_finite()
                && (0..ss).all(|sy| {
                    (0..ss).all(|sx| {
                        let j = (y0 + sy) * hw + x0 + sx;
                        normal[j] == n0 && (depth[j] - d0).abs() < 0.1
                    })
                });
            if uniform {
                if let Some(c) = shade_one(x0, y0, &mut cache) {
                    sum[o] = c.map(|v| v * full);
                    count[o] = (ss * ss) as u8;
                }
                continue;
            }
            for sy in 0..ss {
                for sx in 0..ss {
                    if let Some(c) = shade_one(x0 + sx, y0 + sy, &mut cache) {
                        for k in 0..3 {
                            sum[o][k] += c[k];
                        }
                        count[o] += 1;
                    }
                }
            }
        }
    }
    let mut out = vec![0u8; w * h * 4];
    for (o, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let c = count[o];
        if c == 0 {
            continue;
        }
        for k in 0..3 {
            px[k] = (linear_to_srgb(sum[o][k] / f32::from(c)) * 255.0).round() as u8;
        }
        px[3] = (f32::from(c) / full * 255.0).round() as u8;
    }
    out
}
