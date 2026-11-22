//! One model shown on its own, as a menu picture: the Skulls page's skull
//! (docs/game/skulls.md, "The model picture"). Pure: where the camera sits,
//! where the forehead decal goes and how the look is lit, so the GPU pass
//! ([`crate::showcase_gpu`]) and its tests agree on them.
//!
//! Model space is the game's: x forward (the model's face), y left, z up.
//! The camera stands in front of the face (+x) looking back along -x, so
//! the model's +y is the picture's right. The model turns about z through
//! its bounds' centre; the decal is placed in model space, so it turns with
//! it.

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
}
