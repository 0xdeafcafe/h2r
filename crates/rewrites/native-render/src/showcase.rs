//! One model shown on its own, as a menu picture: the Skulls page's skull
//! (docs/game/skulls.md, "The model picture"). Pure: where the camera sits,
//! where the forehead decal goes and how the look is lit, so the GPU pass
//! ([`crate::showcase_gpu`]) and its tests agree on them.
//!
//! Model space is the game's: x forward (the model's face), y left, z up.
//! The camera stands in front of the face (+x) looking back along -x, so
//! the model's +y is the picture's right. The model turns about z through
//! its bounds' centre; the decal is placed in model space, so it turns with
//! it. A model authored another way round is first turned into this space
//! by an [`Orientation`].

use crate::camera::{Mat4, RenderCamera, transform};
use crate::world::Aabb;

/// Vertical field of view of the picture's camera.
pub const FOV_DEGREES: f32 = 30.0;
/// How much of the picture's height the model's bounding sphere fills.
pub const FILL: f32 = 0.92;
/// The forehead decal's centre, as a fraction of the bounds' height from
/// the bottom.
pub const DECAL_HEIGHT: f32 = 0.70;
/// The decal's radius as a fraction of the smaller of the bounds' width
/// (y) and height (z).
pub const DECAL_SIZE: f32 = 0.19;
/// A surface takes the decal only where its model-space normal's forward
/// part is at least this: the forehead, not the crown or the temples.
pub const DECAL_FACING: f32 = 0.35;
/// The sway: the most the model turns either way, in degrees, and the
/// seconds a full sway takes. It sways rather than spinning so the glyph
/// stays in sight.
pub const SWAY_DEGREES: f32 = 32.0;
pub const SWAY_SECONDS: f64 = 9.0;

/// Where the model sits and where its decal goes, from its bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The bounds' centre: the turn's pivot and the camera's target.
    pub centre: [f32; 3],
    /// The bounding sphere's radius (half the bounds' diagonal).
    pub radius: f32,
    /// The decal's centre in model space: on the front (`x` the bounds'
    /// front), the bounds' middle across (`y`), [`DECAL_HEIGHT`] up (`z`).
    pub decal_centre: [f32; 3],
    /// The decal's radius in model units: the glyph's box is `2 *
    /// decal_radius` across.
    pub decal_radius: f32,
    /// Points behind this `x` never take the decal (the back of the head).
    pub decal_front: f32,
}

impl Placement {
    /// `None` for an empty or degenerate box.
    pub fn of(bounds: &Aabb) -> Option<Self> {
        if bounds.is_empty() {
            return None;
        }
        let size = [0, 1, 2].map(|k| bounds.max[k] - bounds.min[k]);
        if size.iter().any(|s| !s.is_finite()) || size.iter().all(|s| *s <= 0.0) {
            return None;
        }
        let centre = [0, 1, 2].map(|k| 0.5 * (bounds.min[k] + bounds.max[k]));
        let radius = 0.5 * (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt();
        let decal_radius = DECAL_SIZE * size[1].min(size[2]).max(f32::EPSILON);
        Some(Self {
            centre,
            radius,
            decal_centre: [
                bounds.max[0],
                centre[1],
                bounds.min[2] + DECAL_HEIGHT * size[2],
            ],
            decal_radius,
            decal_front: centre[0],
        })
    }

    /// The picture's camera for a `width` by `height` target: in front of
    /// the face, far enough that the bounding sphere fills [`FILL`] of the
    /// height (or the width, if narrower).
    pub fn camera(&self, width: u32, height: u32) -> RenderCamera {
        let fov = FOV_DEGREES.to_radians();
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let half = (fov * 0.5).tan() * FILL * aspect.min(1.0);
        let r = self.radius.max(1e-3);
        let distance = r / half.max(1e-3);
        RenderCamera {
            position: [distance, 0.0, 0.0],
            forward: [-1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov,
            projection_xy: None,
            viewport: [
                0,
                0,
                height.min(i16::MAX as u32) as i16,
                width.min(i16::MAX as u32) as i16,
            ],
            near: (distance - 2.0 * r).max(distance * 0.05),
            far: distance + 2.0 * r,
        }
    }

    /// Model space to the turned, centred space the camera sees: the
    /// model moved so its centre is the origin, then turned `yaw` radians
    /// about z (positive turns the face towards the picture's right).
    pub fn model_matrix(&self, yaw: f32) -> Mat4 {
        let (s, c) = yaw.sin_cos();
        let [x, y, z] = self.centre;
        // Columns: R's columns, then -R·centre.
        [
            [c, s, 0.0, 0.0],
            [-s, c, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [-(c * x - s * y), -(s * x + c * y), -z, 1.0],
        ]
    }

    /// Where model-space point `p` lands in a `width` by `height` picture
    /// at `yaw`: pixel `(x, y)` from the top left, `None` behind the camera.
    pub fn project(&self, p: [f32; 3], yaw: f32, width: u32, height: u32) -> Option<[f32; 2]> {
        let camera = self.camera(width, height);
        let world = transform(&self.model_matrix(yaw), [p[0], p[1], p[2], 1.0]);
        let clip = transform(&camera.view_projection(), world);
        if clip[3] <= 0.0 {
            return None;
        }
        let ndc = [clip[0] / clip[3], clip[1] / clip[3]];
        Some([
            (ndc[0] * 0.5 + 0.5) * width as f32,
            (0.5 - ndc[1] * 0.5) * height as f32,
        ])
    }

    /// The glyph texture coordinate of model-space point `p` (the decal's
    /// planar projection along x): `u` right across the face (+y), `v`
    /// down (-z), both 0..1 inside the decal's box. The shader does the
    /// same arithmetic.
    pub fn decal_uv(&self, p: [f32; 3]) -> [f32; 2] {
        let d = 2.0 * self.decal_radius;
        [
            (p[1] - self.decal_centre[1]) / d + 0.5,
            0.5 - (p[2] - self.decal_centre[2]) / d,
        ]
    }
}

/// A model axis: one of ±x, ±y, ±z.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl Axis {
    pub const ALL: [Axis; 6] = [
        Axis::PosX,
        Axis::NegX,
        Axis::PosY,
        Axis::NegY,
        Axis::PosZ,
        Axis::NegZ,
    ];

    pub fn vector(self) -> [f32; 3] {
        match self {
            Axis::PosX => [1.0, 0.0, 0.0],
            Axis::NegX => [-1.0, 0.0, 0.0],
            Axis::PosY => [0.0, 1.0, 0.0],
            Axis::NegY => [0.0, -1.0, 0.0],
            Axis::PosZ => [0.0, 0.0, 1.0],
            Axis::NegZ => [0.0, 0.0, -1.0],
        }
    }

    /// `x`, `-x`, ... (the preview's label font has no `+`).
    pub fn label(self) -> &'static str {
        match self {
            Axis::PosX => "x",
            Axis::NegX => "-x",
            Axis::PosY => "y",
            Axis::NegY => "-y",
            Axis::PosZ => "z",
            Axis::NegZ => "-z",
        }
    }
}

/// Which way a model's face and crown point in its own space. The picture
/// turns the model so `forward` becomes +x (towards the camera) and `up`
/// becomes +z, then fine-tunes it by `roll` about x, `pitch` about y and
/// `yaw` about z, in that order, in degrees ([`fine`]), before it frames
/// it and places the decal. A model authored lying down (a dropped
/// pickup) needs one other than [`Orientation::IDENTITY`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orientation {
    pub forward: Axis,
    pub up: Axis,
    /// Positive turns the face towards +y, the picture's right.
    pub yaw: f32,
    /// Positive tips the face up.
    pub pitch: f32,
    /// Positive tips the crown towards -y, the picture's left.
    pub roll: f32,
}

/// A 3 by 3 rotation as rows; it acts on column vectors.
pub type Mat3 = [[f32; 3]; 3];

fn mul3(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalise3(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = dot3(v, v).sqrt();
    (len > 1e-9).then(|| v.map(|c| c / len))
}

fn mean3(points: &[[f32; 3]]) -> [f32; 3] {
    let count = points.len().max(1) as f32;
    points.iter().fold([0.0; 3], |a, p| {
        [
            a[0] + p[0] / count,
            a[1] + p[1] / count,
            a[2] + p[2] / count,
        ]
    })
}

/// The fine-tune rotation: yaw about z after pitch about y after roll
/// about x, degrees, with the signs [`Orientation`] gives them.
pub fn fine(yaw: f32, pitch: f32, roll: f32) -> Mat3 {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    let (sr, cr) = roll.to_radians().sin_cos();
    let z = [[cy, -sy, 0.0], [sy, cy, 0.0], [0.0, 0.0, 1.0]];
    let y = [[cp, 0.0, -sp], [0.0, 1.0, 0.0], [sp, 0.0, cp]];
    let x = [[1.0, 0.0, 0.0], [0.0, cr, -sr], [0.0, sr, cr]];
    mul3(&z, &mul3(&y, &x))
}

/// The yaw, pitch and roll (degrees) that [`fine`] makes `m` from.
pub fn fine_angles(m: &Mat3) -> (f32, f32, f32) {
    let pitch = m[2][0].clamp(-1.0, 1.0).asin();
    let roll = m[2][1].atan2(m[2][2]);
    let yaw = m[1][0].atan2(m[0][0]);
    (yaw.to_degrees(), pitch.to_degrees(), roll.to_degrees())
}

impl Orientation {
    pub const IDENTITY: Self = Self::axes(Axis::PosX, Axis::PosZ);

    /// `forward` and `up`, no fine-tune.
    pub const fn axes(forward: Axis, up: Axis) -> Self {
        Self {
            forward,
            up,
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
        }
    }

    /// The 24 axis-aligned orientations (each forward with its four ups),
    /// no fine-tune.
    pub fn all() -> Vec<Self> {
        let mut out = Vec::new();
        for forward in Axis::ALL {
            for up in Axis::ALL {
                if dot3(forward.vector(), up.vector()) == 0.0 {
                    out.push(Self::axes(forward, up));
                }
            }
        }
        out
    }

    /// `fwd -y up z`.
    pub fn label(&self) -> String {
        format!("fwd {} up {}", self.forward.label(), self.up.label())
    }

    /// The axis-aligned turn alone: rows forward, left, up.
    pub fn base(&self) -> Mat3 {
        let f = self.forward.vector();
        let u = self.up.vector();
        // left = up x forward (z x x = y).
        [f, cross3(u, f), u]
    }

    /// Rows of the rotation: model space to the picture's (x forward, y
    /// left, z up), the fine-tune applied after the axis-aligned turn.
    pub fn matrix(&self) -> Mat3 {
        mul3(&fine(self.yaw, self.pitch, self.roll), &self.base())
    }

    /// `v` (a point or a normal) in the picture's space.
    pub fn apply(&self, v: [f32; 3]) -> [f32; 3] {
        let m = self.matrix();
        m.map(|row| dot3(row, v))
    }
}

/// How far `points` are from their mirror image across the plane through
/// `centre` with unit normal `n`: the mean squared distance from each
/// mirrored point to its nearest point.
fn mirror_error(points: &[[f32; 3]], centre: [f32; 3], n: [f32; 3]) -> f32 {
    let mut sum = 0.0;
    for p in points {
        let d = dot3([p[0] - centre[0], p[1] - centre[1], p[2] - centre[2]], n);
        let m = [
            p[0] - 2.0 * d * n[0],
            p[1] - 2.0 * d * n[1],
            p[2] - 2.0 * d * n[2],
        ];
        let nearest = points
            .iter()
            .map(|q| {
                let e = [q[0] - m[0], q[1] - m[1], q[2] - m[2]];
                dot3(e, e)
            })
            .fold(f32::INFINITY, f32::min);
        sum += nearest;
    }
    sum / points.len().max(1) as f32
}

/// The best mirror plane of `points` (a skull is left-right symmetric):
/// its unit normal, for the plane through their centroid. A search over
/// 600 directions on a hemisphere, then a local refinement of the best
/// few that are well apart (a near miss of the true plane can score worse
/// than a plane the shape is nearly symmetric across). `None` for fewer
/// than four points.
pub fn mirror_plane(points: &[[f32; 3]]) -> Option<[f32; 3]> {
    if points.len() < 4 {
        return None;
    }
    // At most 500 points: the search is quadratic in them.
    let stride = points.len().div_ceil(500);
    let points: Vec<[f32; 3]> = points.iter().step_by(stride).copied().collect();
    let centre = mean3(&points);
    const DIRECTIONS: usize = 600;
    const SEEDS: usize = 6;
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    let mut scored: Vec<([f32; 3], f32)> = (0..DIRECTIONS)
        .map(|i| {
            let z = 1.0 - (i as f32 + 0.5) / DIRECTIONS as f32;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let phi = golden * i as f32;
            let n = [r * phi.cos(), r * phi.sin(), z];
            (n, mirror_error(&points, centre, n))
        })
        .collect();
    scored.sort_by(|a, b| a.1.total_cmp(&b.1));
    // Seeds at least 15 degrees apart (n and -n are the same plane).
    let mut seeds: Vec<([f32; 3], f32)> = Vec::new();
    for s in scored {
        if seeds.len() == SEEDS {
            break;
        }
        if seeds.iter().all(|t| dot3(t.0, s.0).abs() < 0.966) {
            seeds.push(s);
        }
    }
    seeds
        .into_iter()
        .filter_map(|seed| refine_mirror(&points, centre, seed))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|best| best.0)
}

/// [`mirror_plane`]'s refinement from `best`: steps across the sphere,
/// halved whenever none improves.
fn refine_mirror(
    points: &[[f32; 3]],
    centre: [f32; 3],
    mut best: ([f32; 3], f32),
) -> Option<([f32; 3], f32)> {
    // The directions are about 6 degrees apart.
    let mut step = 0.06f32;
    for _ in 0..24 {
        let n = best.0;
        let other = if n[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let a = normalise3(cross3(n, other))?;
        let b = cross3(n, a);
        let mut moved = false;
        for (da, db) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            let tried: [f32; 3] = std::array::from_fn(|k| n[k] + step * (da * a[k] + db * b[k]));
            let Some(m) = normalise3(tried) else {
                continue;
            };
            let e = mirror_error(points, centre, m);
            if e < best.1 {
                best = (m, e);
                moved = true;
            }
        }
        if !moved {
            step *= 0.5;
        }
    }
    Some(best)
}

/// The fine-tune that shows `points` (the model as authored) face-on and
/// upright once `base`'s axes have turned it: the yaw, pitch and roll
/// (degrees) that make its mirror plane ([`mirror_plane`]) the picture's
/// x-z plane, with the longest spread within that plane (a skull's front
/// to back) along x, signed to stay nearest `base`'s axes. With it, how
/// much longer that spread is than the one across it in the plane: near
/// 1 the pitch is a guess. `None` for too few points.
pub fn auto_fine(points: &[[f32; 3]], base: &Orientation) -> Option<((f32, f32, f32), f32)> {
    let axes = Orientation::axes(base.forward, base.up);
    let turned: Vec<[f32; 3]> = points.iter().map(|p| axes.apply(*p)).collect();
    let mut left = mirror_plane(&turned)?;
    if left[1] < 0.0 {
        left = left.map(|c| -c);
    }
    // Two axes in the plane, near x and near z.
    let a = normalise3(cross3(left, [0.0, 0.0, 1.0]))
        .or_else(|| normalise3(cross3(left, [1.0, 0.0, 0.0])))?;
    let a = if a[0] < 0.0 { a.map(|c| -c) } else { a };
    let b = cross3(a, left);
    let centre = mean3(&turned);
    let (mut caa, mut cbb, mut cab) = (0.0f32, 0.0f32, 0.0f32);
    for p in &turned {
        let d = [p[0] - centre[0], p[1] - centre[1], p[2] - centre[2]];
        let (u, v) = (dot3(d, a), dot3(d, b));
        caa += u * u;
        cbb += v * v;
        cab += u * v;
    }
    let theta = 0.5 * (2.0 * cab).atan2(caa - cbb);
    let (s, c) = theta.sin_cos();
    let mut forward: [f32; 3] = std::array::from_fn(|k| c * a[k] + s * b[k]);
    if forward[0] < 0.0 {
        forward = forward.map(|v| -v);
    }
    let up = cross3(forward, left);
    let mean = 0.5 * (caa + cbb);
    let half = (0.25 * (caa - cbb) * (caa - cbb) + cab * cab).sqrt();
    let ratio = ((mean + half) / (mean - half).max(1e-12)).sqrt();
    Some((fine_angles(&[forward, left, up]), ratio))
}

/// The sway at `seconds` since the picture first showed: radians.
pub fn sway(seconds: f64) -> f32 {
    let phase = (seconds / SWAY_SECONDS).fract() * std::f64::consts::TAU;
    (SWAY_DEGREES.to_radians() as f64 * phase.sin()) as f32
}

/// How the model is drawn this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// Radians about z ([`sway`]).
    pub yaw: f32,
    /// The bone's colour (multiplies the base map's brightness).
    pub tint: [f32; 3],
    /// A dark shape with a faint rim, no texture and no decal: a skull not
    /// yet collected.
    pub silhouette: bool,
    /// Whether the decal (the glyph texture) is drawn. Ignored for a
    /// silhouette.
    pub glyph: bool,
}

/// The silhouette's fill and the most its rim adds (each channel, 0..1).
/// The GPU test holds every drawn pixel of a silhouette to
/// `SILHOUETTE_FILL + SILHOUETTE_RIM` per channel.
pub const SILHOUETTE_FILL: f32 = 0.03;
pub const SILHOUETTE_RIM: f32 = 0.14;

/// The uniform the shader reads (`showcase.wgsl`, `Frame`): 16-byte rows.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniform {
    pub view_proj: Mat4,
    pub model: Mat4,
    /// xyz the camera's position (turned space), w 1 for a silhouette.
    pub eye: [f32; 4],
    /// rgb the tint, w 1 when the decal is drawn.
    pub tint: [f32; 4],
    /// xyz the decal's centre (model space), w its radius.
    pub decal: [f32; 4],
    /// x the front cut, y the facing threshold, z the silhouette fill, w its rim.
    pub decal_cut: [f32; 4],
}

impl Uniform {
    pub fn new(placement: &Placement, look: &Look, width: u32, height: u32) -> Self {
        let camera = placement.camera(width, height);
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        Self {
            view_proj: camera.view_projection(),
            model: placement.model_matrix(look.yaw),
            eye: [
                camera.position[0],
                camera.position[1],
                camera.position[2],
                flag(look.silhouette),
            ],
            tint: [
                look.tint[0],
                look.tint[1],
                look.tint[2],
                flag(look.glyph && !look.silhouette),
            ],
            decal: [
                placement.decal_centre[0],
                placement.decal_centre[1],
                placement.decal_centre[2],
                placement.decal_radius,
            ],
            decal_cut: [
                placement.decal_front,
                DECAL_FACING,
                SILHOUETTE_FILL,
                SILHOUETTE_RIM,
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Aabb {
        Aabb {
            min: [-1.0, -1.0, -1.0],
            max: [1.0, 1.0, 1.0],
        }
    }

    #[test]
    fn the_model_fills_the_picture_and_its_face_looks_at_the_camera() {
        let p = Placement::of(&bounds()).unwrap();
        assert_eq!(p.centre, [0.0; 3]);
        // The centre is the picture's centre, at any yaw.
        for yaw in [0.0, 0.4, -0.4] {
            let [x, y] = p.project([0.0; 3], yaw, 128, 128).unwrap();
            assert!(
                (x - 64.0).abs() < 1e-3 && (y - 64.0).abs() < 1e-3,
                "{x} {y}"
            );
        }
        // The sphere's top is inside the picture, near its top.
        let [_, top] = p.project([0.0, 0.0, p.radius], 0.0, 128, 128).unwrap();
        assert!(top > 0.0 && top < 64.0 * (1.0 - FILL * 0.8), "{top}");
        // +y (the model's left) is the picture's right; +z is up.
        let [right, _] = p.project([0.0, 0.5, 0.0], 0.0, 128, 128).unwrap();
        let [_, up] = p.project([0.0, 0.0, 0.5], 0.0, 128, 128).unwrap();
        assert!(right > 64.0 && up < 64.0);
        // The face is nearer the camera than the back.
        let c = p.camera(128, 128);
        assert!(c.depth_of([1.0, 0.0, 0.0]) < c.depth_of([-1.0, 0.0, 0.0]));
    }

    #[test]
    fn the_decal_sits_on_the_upper_front_and_turns_with_the_model() {
        let p = Placement::of(&bounds()).unwrap();
        assert_eq!(p.decal_centre[0], 1.0);
        assert!((p.decal_centre[2] - (-1.0 + 2.0 * DECAL_HEIGHT)).abs() < 1e-6);
        assert_eq!(p.decal_uv(p.decal_centre), [0.5, 0.5]);
        let r = p.decal_radius;
        assert_eq!(p.decal_uv([1.0, r, p.decal_centre[2]]), [1.0, 0.5]);
        assert_eq!(p.decal_uv([1.0, 0.0, p.decal_centre[2] + r]), [0.5, 0.0]);
        // Turned, the decal's centre moves across the picture.
        let still = p.project(p.decal_centre, 0.0, 128, 128).unwrap();
        let turned = p.project(p.decal_centre, 0.4, 128, 128).unwrap();
        assert!(turned[0] > still[0] + 1.0, "{still:?} {turned:?}");
        assert!(still[1] < 64.0, "above the middle");
    }

    #[test]
    fn the_sway_stays_within_its_swing_and_repeats() {
        let max = SWAY_DEGREES.to_radians() + 1e-6;
        for i in 0..200 {
            let t = f64::from(i) * 0.137;
            assert!(sway(t).abs() <= max);
            assert!((sway(t) - sway(t + SWAY_SECONDS)).abs() < 1e-4);
        }
        assert_eq!(sway(0.0), 0.0);
        assert!(sway(SWAY_SECONDS / 4.0) > 0.0);
    }

    #[test]
    fn an_empty_box_has_no_placement() {
        assert!(Placement::of(&Aabb::EMPTY).is_none());
    }

    #[test]
    fn the_uniform_carries_the_look() {
        let p = Placement::of(&bounds()).unwrap();
        let look = Look {
            yaw: 0.0,
            tint: [1.0, 0.5, 0.25],
            silhouette: false,
            glyph: true,
        };
        let u = Uniform::new(&p, &look, 64, 64);
        assert_eq!(u.tint, [1.0, 0.5, 0.25, 1.0]);
        assert_eq!(u.eye[3], 0.0);
        let dark = Uniform::new(
            &p,
            &Look {
                silhouette: true,
                ..look
            },
            64,
            64,
        );
        assert_eq!(
            (dark.eye[3], dark.tint[3]),
            (1.0, 0.0),
            "a silhouette never draws the glyph"
        );
        assert_eq!(std::mem::size_of::<Uniform>(), 2 * 64 + 4 * 16);
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5)
    }

    #[test]
    fn there_are_24_orientations_each_a_proper_rotation() {
        let all = Orientation::all();
        assert_eq!(all.len(), 24);
        let mut labels: Vec<String> = all.iter().map(Orientation::label).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), 24, "every orientation has its own label");
        for o in &all {
            let m = o.matrix();
            for i in 0..3 {
                for j in 0..3 {
                    let dot: f32 = (0..3).map(|k| m[i][k] * m[j][k]).sum();
                    let want = if i == j { 1.0 } else { 0.0 };
                    assert!((dot - want).abs() < 1e-5, "{}: rows orthonormal", o.label());
                }
            }
            let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
            assert!((det - 1.0).abs() < 1e-5, "{}: no mirror", o.label());
            assert!(
                close(o.apply(o.forward.vector()), [1.0, 0.0, 0.0]),
                "{}: face to +x",
                o.label()
            );
            assert!(
                close(o.apply(o.up.vector()), [0.0, 0.0, 1.0]),
                "{}: crown to +z",
                o.label()
            );
            assert!(
                o.label()
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || " -./_".contains(c)),
                "{}: the label font draws it",
                o.label()
            );
        }
    }

    #[test]
    fn the_identity_leaves_the_model_alone_and_the_fine_tune_turns_it() {
        let p = [0.3, -0.2, 0.7];
        assert!(close(Orientation::IDENTITY.apply(p), p));
        let tipped = Orientation {
            pitch: 90.0,
            ..Orientation::IDENTITY
        };
        assert!(
            close(tipped.apply([1.0, 0.0, 0.0]), [0.0, 0.0, 1.0]),
            "pitch tips the face up"
        );
        assert!(close(tipped.apply([0.0, 1.0, 0.0]), [0.0, 1.0, 0.0]));
        let turned = Orientation {
            yaw: 90.0,
            ..Orientation::IDENTITY
        };
        assert!(
            close(turned.apply([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]),
            "yaw turns the face to +y"
        );
        let rolled = Orientation {
            roll: 90.0,
            ..Orientation::IDENTITY
        };
        assert!(
            close(rolled.apply([0.0, 0.0, 1.0]), [0.0, -1.0, 0.0]),
            "roll tips the crown to -y"
        );
        let lying = Orientation::axes(Axis::NegY, Axis::PosX);
        assert!(close(lying.apply([0.0, -1.0, 0.0]), [1.0, 0.0, 0.0]));
        assert!(close(lying.apply([1.0, 0.0, 0.0]), [0.0, 0.0, 1.0]));
        assert_eq!(lying.label(), "fwd -y up x");
    }

    #[test]
    fn the_fine_angles_come_back_from_their_rotation() {
        let cases = [
            (0.0, 0.0, 0.0),
            (20.0, -10.0, 15.0),
            (-40.0, 35.0, -30.0),
            (170.0, 5.0, 60.0),
        ];
        for (y, p, r) in cases {
            let (y2, p2, r2) = fine_angles(&fine(y, p, r));
            assert!(
                (y - y2).abs() < 1e-3 && (p - p2).abs() < 1e-3 && (r - r2).abs() < 1e-3,
                "({y}, {p}, {r}) came back as ({y2}, {p2}, {r2})"
            );
        }
    }

    /// An invented head, mirror-symmetric across y only: an ellipsoid long
    /// along x, a nose cluster at +x and a crown cluster at +z.
    fn head() -> Vec<[f32; 3]> {
        let mut points = Vec::new();
        for i in 0..12 {
            let theta = std::f32::consts::PI * (i as f32 + 0.5) / 12.0;
            for j in 0..12 {
                let phi = std::f32::consts::PI * (j as f32 + 0.5) / 12.0;
                let p = [
                    2.0 * theta.sin() * phi.cos(),
                    theta.sin() * phi.sin(),
                    1.4 * theta.cos(),
                ];
                points.push(p);
                points.push([p[0], -p[1], p[2]]);
            }
        }
        for k in 0..4 {
            let o = 0.08 * k as f32;
            points.extend([[2.2, o, 0.0], [2.2, -o, 0.0], [0.0, o, 1.6], [0.0, -o, 1.6]]);
        }
        points
    }

    #[test]
    fn the_auto_fine_tune_finds_a_turned_head_face_on() {
        let normal = mirror_plane(&head()).unwrap();
        assert!(
            normal[1].abs() > 0.999,
            "the head mirrors across y: {normal:?}"
        );
        let truth = (20.0, -10.0, 15.0);
        let m = fine(truth.0, truth.1, truth.2);
        // The head turned by the inverse (the transpose): the fine-tune
        // that undoes it is `truth`.
        let turned: Vec<[f32; 3]> = head()
            .iter()
            .map(|p| std::array::from_fn(|i| (0..3).map(|k| m[k][i] * p[k]).sum()))
            .collect();
        let ((yaw, pitch, roll), ratio) = auto_fine(&turned, &Orientation::IDENTITY).unwrap();
        assert!(ratio > 1.2, "the head is longer than it is tall: {ratio}");
        assert!((yaw - truth.0).abs() < 2.0, "yaw {yaw}");
        assert!((roll - truth.2).abs() < 2.0, "roll {roll}");
        assert!((pitch - truth.1).abs() < 3.0, "pitch {pitch}");
    }
}
