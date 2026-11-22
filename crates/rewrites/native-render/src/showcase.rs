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
pub const DECAL_HEIGHT: f32 = 0.69;
/// The decal's radius as a fraction of the smaller of the bounds' width
/// (y) and height (z).
pub const DECAL_SIZE: f32 = 0.17;
/// A surface takes the decal only where its model-space normal's forward
/// part is at least this: the forehead, not the crown or the temples.
pub const DECAL_FACING: f32 = 0.35;
/// The sway: the most the model turns either way, in degrees, and the
/// seconds a full sway takes. It sways rather than spinning so the glyph
/// stays in sight.
pub const SWAY_DEGREES: f32 = 20.0;
pub const SWAY_SECONDS: f64 = 9.0;

/// A picture is drawn at twice its rectangle's texels each way, and
/// filtered down two by two, while its longer side stays within this;
/// a larger one is drawn at the rectangle's own texels. Either way each
/// pixel resolves `showcase_gpu::SAMPLES` samples.
pub const SUPERSAMPLED_SIDE: u32 = 1280;
/// No picture is drawn larger than this a side.
pub const LARGEST_SIDE: u32 = 4096;

/// A picture's place and size for a rectangle of a target: the rectangle
/// moved onto whole texels, and the picture a whole number of times its
/// size, so the composite never stretches the picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// `x, y, width, height` in the target's texels, all whole.
    pub rect: [f32; 4],
    /// The picture's size in texels.
    pub width: u32,
    pub height: u32,
}

impl Fit {
    /// `rect` is `x, y, width, height` in the target's texels. `None` for
    /// a rectangle that is not finite or rounds to nothing.
    pub fn of(rect: [f32; 4]) -> Option<Self> {
        if rect.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let [x, y, w, h] = rect;
        let (left, top) = (x.round(), y.round());
        let (width, height) = ((x + w).round() - left, (y + h).round() - top);
        if width < 1.0 || height < 1.0 {
            return None;
        }
        let (tw, th) = (width as u32, height as u32);
        let scale = if tw.max(th) * 2 <= SUPERSAMPLED_SIDE {
            2
        } else {
            1
        };
        Some(Self {
            rect: [left, top, width, height],
            width: (tw * scale).min(LARGEST_SIDE),
            height: (th * scale).min(LARGEST_SIDE),
        })
    }

    /// How many of the picture's texels cover one of the rectangle's,
    /// each way (below 1 only past [`LARGEST_SIDE`]).
    pub fn scale(&self) -> f32 {
        self.width as f32 / self.rect[2]
    }
}

/// Where the model sits and where its decal goes, from its bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The bounds' centre: the turn's pivot and the camera's target.
    pub centre: [f32; 3],
    /// The radius of the sphere about `centre` that the framing fits:
    /// half the bounds' diagonal ([`Placement::of`]) or the farthest
    /// point's distance ([`Placement::fitted`]).
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

    /// [`Placement::of`] with the sphere drawn tight round `points` (the
    /// model's drawn vertices): the farthest one's distance from the
    /// centre, so the model fills the picture at any turn about its
    /// centre. A box's corners are farther out than a rounded model
    /// reaches, and framing those leaves it small.
    pub fn fitted(bounds: &Aabb, points: impl IntoIterator<Item = [f32; 3]>) -> Option<Self> {
        let mut placement = Self::of(bounds)?;
        let farthest = points
            .into_iter()
            .map(|p| {
                let d = [0, 1, 2].map(|k| p[k] - placement.centre[k]);
                dot3(d, d).sqrt()
            })
            .filter(|d| d.is_finite())
            .fold(0.0, f32::max);
        if farthest > 0.0 {
            placement.radius = farthest.min(placement.radius);
        }
        Some(placement)
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
        let top = p.decal_uv([1.0, 0.0, p.decal_centre[2] + r]);
        assert!((top[0] - 0.5).abs() < 1e-6 && top[1].abs() < 1e-6, "{top:?}");
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
    fn a_picture_fits_its_rectangle_on_whole_texels() {
        // The picture box in a 640 by 480 target: twice its texels.
        let small = Fit::of([359.0, 63.0, 220.0, 207.0]).unwrap();
        assert_eq!(small.rect, [359.0, 63.0, 220.0, 207.0]);
        assert_eq!((small.width, small.height), (440, 414));
        assert_eq!(small.scale(), 2.0);
        // A fractional rectangle moves onto texels; its far edge is the
        // rounded far edge, not the rounded size.
        let odd = Fit::of([10.4, 20.6, 100.3, 50.2]).unwrap();
        assert_eq!(odd.rect, [10.0, 21.0, 101.0, 50.0]);
        assert_eq!((odd.width, odd.height), (202, 100));
        // The box in a 3840 by 2160 window: its own texels, sample for
        // sample.
        let large = Fit::of([2095.5, 283.5, 990.0, 931.5]).unwrap();
        assert_eq!(large.rect, [2096.0, 284.0, 990.0, 931.0]);
        assert_eq!((large.width, large.height), (990, 931));
        assert_eq!(large.scale(), 1.0);
        // The last size drawn twice over.
        let edge = Fit::of([0.0, 0.0, 640.0, 600.0]).unwrap();
        assert_eq!((edge.width, edge.height), (1280, 1200));
        assert_eq!(Fit::of([0.0, 0.0, 641.0, 600.0]).unwrap().width, 641);
        // Never past the largest side.
        let huge = Fit::of([0.0, 0.0, 9000.0, 4500.0]).unwrap();
        assert_eq!((huge.width, huge.height), (LARGEST_SIDE, LARGEST_SIDE));
        assert!(Fit::of([0.0, 0.0, 0.4, 10.0]).is_none());
        assert!(Fit::of([f32::NAN, 0.0, 10.0, 10.0]).is_none());
        assert!(Fit::of([0.0, 0.0, f32::INFINITY, 10.0]).is_none());
    }

    #[test]
    fn a_fitted_placement_frames_the_points_not_the_boxs_corners() {
        let boxed = Placement::of(&bounds()).unwrap();
        assert!((boxed.radius - 3f32.sqrt()).abs() < 1e-6);
        // A unit sphere's poles: the sphere, not its box's diagonal.
        let poles = [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]];
        let tight = Placement::fitted(&bounds(), poles).unwrap();
        assert!((tight.radius - 1.0).abs() < 1e-6);
        assert_eq!(tight.centre, boxed.centre);
        assert_eq!(tight.decal_centre, boxed.decal_centre);
        // The farthest point is inside the picture and nearer its edge
        // than the box's framing put it.
        let top = |p: &Placement| p.project([0.0, 0.0, 1.0], 0.0, 128, 128).unwrap()[1];
        assert!(top(&tight) > 0.0 && top(&tight) < top(&boxed) - 10.0);
        // No points, or points outside the box: the box's own sphere.
        let none: [[f32; 3]; 0] = [];
        assert_eq!(Placement::fitted(&bounds(), none), Some(boxed));
        assert_eq!(Placement::fitted(&bounds(), [[9.0, 0.0, 0.0]]), Some(boxed));
        assert!(Placement::fitted(&Aabb::EMPTY, poles).is_none());
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
}
