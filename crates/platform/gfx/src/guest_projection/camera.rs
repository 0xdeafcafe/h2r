//! Perspective matrices split into a projection and a rigid camera, and
//! put back together in between two ticks.
//!
//! A view-projection matrix (or a world-view-projection one) is
//! `M = K * E`, with `E = [R t; 0 1]` rigid and `K` a perspective
//! projection, possibly off-centre, with the depth mapping in its third
//! row:
//!
//! ```text
//! K = | sx  k  cx  0 |
//!     |  0  sy cy  0 |
//!     |  0  0  a   b |
//!     |  0  0  w   0 |
//! ```
//!
//! The split is exact for any such product: the last row gives `w` and
//! the camera's forward row, the third row must be parallel to it (that is
//! the test that a matrix is a perspective one at all), and the first two
//! rows give the other axes by Gram-Schmidt. Orthographic matrices (the
//! HUD's) have a last row of `0 0 0 1` and are not perspective. A split is
//! accepted only when putting it back gives the matrix again within
//! [`REBUILD_TOLERANCE`] of its size.

use super::math::{
    Mat4, Quat, Similarity, dot3, len3, lerp3, max_abs, max_diff, scale3, sub3, xyz,
};

/// How close (relative to the matrix's largest entry) a rebuilt matrix must
/// come to the one it was split from.
pub const REBUILD_TOLERANCE: f64 = 1e-5;

/// A perspective matrix, split.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Persp {
    /// The projection `K`.
    pub projection: Mat4,
    /// The rigid part `E` (scale 1): world (or object) to camera space.
    pub view: Similarity,
}

impl Persp {
    /// Splits `m`, or `None` when it is not a perspective matrix of the
    /// form above.
    pub fn split(m: &Mat4) -> Option<Persp> {
        if !m.iter().flatten().all(|v| v.is_finite()) {
            return None;
        }
        let size = max_abs(m);
        if size == 0.0 {
            return None;
        }
        let tiny = 1e-6 * size;
        let w = len3(xyz(m[3]));
        if w <= tiny {
            return None;
        }
        let r2 = scale3(xyz(m[3]), 1.0 / w);
        let t2 = m[3][3] / w;
        // The third row is a R2 plus a constant: parallel to the last.
        let a = dot3(xyz(m[2]), r2);
        if len3(sub3(xyz(m[2]), scale3(r2, a))) > REBUILD_TOLERANCE * size {
            return None;
        }
        let b = m[2][3] - a * t2;
        let cy = dot3(xyz(m[1]), r2);
        let v1 = sub3(xyz(m[1]), scale3(r2, cy));
        let sy = len3(v1);
        if sy <= tiny {
            return None;
        }
        let r1 = scale3(v1, 1.0 / sy);
        let t1 = (m[1][3] - cy * t2) / sy;
        let cx = dot3(xyz(m[0]), r2);
        let k = dot3(xyz(m[0]), r1);
        let v0 = sub3(sub3(xyz(m[0]), scale3(r2, cx)), scale3(r1, k));
        let mut sx = len3(v0);
        if sx <= tiny {
            return None;
        }
        let mut r0 = scale3(v0, 1.0 / sx);
        let mut t0 = (m[0][3] - cx * t2 - k * t1) / sx;
        // A right-handed rotation; a mirrored view goes into sx's sign.
        let det = dot3(r0, cross(r1, r2));
        if det < 0.0 {
            r0 = scale3(r0, -1.0);
            t0 = -t0;
            sx = -sx;
        }
        let p = Persp {
            projection: [
                [sx, k, cx, 0.0],
                [0.0, sy, cy, 0.0],
                [0.0, 0.0, a, b],
                [0.0, 0.0, w, 0.0],
            ],
            view: Similarity {
                rotation: Quat::from_rotation(&[r0, r1, r2]),
                scale: 1.0,
                translation: [t0, t1, t2],
            },
        };
        (max_diff(&p.matrix(), m) <= REBUILD_TOLERANCE * size).then_some(p)
    }

    /// Whether the projection is a centred one into normalised device
    /// coordinates, as a game camera's is: no shear, the axis through the
    /// middle of the screen (within 5% of it), and a field of view between
    /// about 2 and 175 degrees. A perspective that maps to pixels (screen
    /// space UI, off-centre by half the screen) or a degenerate one is not
    /// a camera the replay should move.
    pub fn centred(&self) -> bool {
        let k = &self.projection;
        let w = k[3][2];
        if w == 0.0 || !w.is_finite() {
            return false;
        }
        let (sx, sy) = ((k[0][0] / w).abs(), (k[1][1] / w).abs());
        let focal = |f: f64| (0.04..=60.0).contains(&f);
        focal(sx)
            && focal(sy)
            && (k[0][2] / w).abs() <= 0.05
            && (k[1][2] / w).abs() <= 0.05
            && k[0][1].abs() <= 0.05 * k[0][0].abs()
    }

    /// `K * E`.
    pub fn matrix(&self) -> Mat4 {
        super::math::mul(&self.projection, &self.view.to_mat4())
    }

    /// The camera's position in the space `E` maps from (`-Rᵀ t`).
    pub fn eye(&self) -> [f64; 3] {
        let r = self.view.rotation.to_rotation();
        let t = self.view.translation;
        let mut e = [0.0; 3];
        for (j, v) in e.iter_mut().enumerate() {
            *v = -(r[0][j] * t[0] + r[1][j] * t[1] + r[2][j] * t[2]);
        }
        e
    }

    /// The in-between camera: projection entries lerped, the eye lerped
    /// along a straight line and the orientation slerped (so the eye
    /// does not swing on an arc when the camera turns), `self` at 0.
    pub fn lerp_camera(&self, o: &Persp, t: f64) -> Persp {
        let rotation = self.view.rotation.slerp(o.view.rotation, t);
        let eye = lerp3(self.eye(), o.eye(), t);
        let r = rotation.to_rotation();
        let translation = [-dot3(r[0], eye), -dot3(r[1], eye), -dot3(r[2], eye)];
        Persp {
            projection: lerp_mat(&self.projection, &o.projection, t),
            view: Similarity {
                rotation,
                scale: 1.0,
                translation,
            },
        }
    }

    /// The in-between pose in camera space: `E`'s translation lerped and
    /// rotation slerped as they are (for a first-person weapon drawn with a
    /// projection of its own, which must stay locked to the camera).
    pub fn lerp_local(&self, o: &Persp, t: f64) -> Persp {
        Persp {
            projection: lerp_mat(&self.projection, &o.projection, t),
            view: self.view.lerp(&o.view, t),
        }
    }

    /// Whether `self` to `o` is a cut to snap to: the eye moved more than
    /// `distance`, turned more than `angle` radians, or the projection
    /// changed by more than `zoom` relatively (a scope zoom, a cinematic's
    /// lens).
    pub fn cut(&self, o: &Persp, distance: f64, angle: f64, zoom: f64) -> bool {
        len3(sub3(self.eye(), o.eye())) > distance
            || self.view.rotation.angle_to(o.view.rotation) > angle
            || projection_change(&self.projection, &o.projection) > zoom
    }
}

/// The largest relative change of the projection's focal and depth terms.
fn projection_change(a: &Mat4, b: &Mat4) -> f64 {
    [(0, 0), (1, 1), (2, 2), (2, 3), (3, 2)]
        .iter()
        .map(|&(i, j)| {
            let (x, y) = (a[i][j], b[i][j]);
            let s = x.abs().max(y.abs());
            if s == 0.0 { 0.0 } else { (x - y).abs() / s }
        })
        .fold(0.0, f64::max)
}

fn lerp_mat(a: &Mat4, b: &Mat4, t: f64) -> Mat4 {
    let mut m = *a;
    for i in 0..4 {
        for j in 0..4 {
            m[i][j] = a[i][j] + (b[i][j] - a[i][j]) * t;
        }
    }
    m
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Test and fixture helpers: cameras built the way a game builds them.
pub mod build {
    use super::super::math::{Mat4, Quat, mul};

    /// A right-handed look-at view (camera looks down -z in its space) from
    /// `eye` with yaw and pitch in radians.
    pub fn view(eye: [f64; 3], yaw: f64, pitch: f64) -> Mat4 {
        let qy = axis([0.0, 0.0, 1.0], yaw);
        let qp = axis([1.0, 0.0, 0.0], pitch);
        let r = mul_q(qy, qp).to_rotation();
        // Camera to world is [R eye]; world to camera is its inverse.
        let rt = [
            [r[0][0], r[1][0], r[2][0]],
            [r[0][1], r[1][1], r[2][1]],
            [r[0][2], r[1][2], r[2][2]],
        ];
        let t = [
            -(rt[0][0] * eye[0] + rt[0][1] * eye[1] + rt[0][2] * eye[2]),
            -(rt[1][0] * eye[0] + rt[1][1] * eye[1] + rt[1][2] * eye[2]),
            -(rt[2][0] * eye[0] + rt[2][1] * eye[1] + rt[2][2] * eye[2]),
        ];
        [
            [rt[0][0], rt[0][1], rt[0][2], t[0]],
            [rt[1][0], rt[1][1], rt[1][2], t[1]],
            [rt[2][0], rt[2][1], rt[2][2], t[2]],
            [0.0, 0.0, 0.0, 1.0],
        ]
    }

    /// A D3D-style perspective projection (depth 0..1) for a camera looking
    /// down -z: vertical field of view `fov`, aspect, near and far.
    pub fn projection(fov: f64, aspect: f64, near: f64, far: f64) -> Mat4 {
        let sy = 1.0 / (fov / 2.0).tan();
        let sx = sy / aspect;
        let a = far / (near - far);
        [
            [sx, 0.0, 0.0, 0.0],
            [0.0, sy, 0.0, 0.0],
            [0.0, 0.0, a, a * near],
            [0.0, 0.0, -1.0, 0.0],
        ]
    }

    /// `projection * view`.
    pub fn view_projection(fov: f64, eye: [f64; 3], yaw: f64, pitch: f64) -> Mat4 {
        mul(
            &projection(fov, 16.0 / 9.0, 0.06, 1024.0),
            &view(eye, yaw, pitch),
        )
    }

    /// A rotation of `angle` radians about `axis`.
    pub fn axis(axis: [f64; 3], angle: f64) -> Quat {
        let n = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
        let (s, c) = (angle / 2.0).sin_cos();
        Quat {
            w: c,
            x: axis[0] / n * s,
            y: axis[1] / n * s,
            z: axis[2] / n * s,
        }
    }

    fn mul_q(a: Quat, b: Quat) -> Quat {
        Quat {
            w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
            x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
            y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
            z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::math::{IDENTITY, mul};
    use super::*;

    #[test]
    fn view_projections_split_and_rebuild() {
        for (eye, yaw, pitch) in [
            ([0.0, 0.0, 0.0], 0.0, 0.0),
            ([10.0, -4.0, 2.5], 1.2, -0.3),
            ([-100.0, 55.0, 7.0], -2.8, 1.1),
        ] {
            let m = build::view_projection(1.2, eye, yaw, pitch);
            let p = Persp::split(&m).expect("a perspective matrix");
            assert!(max_diff(&p.matrix(), &m) < 1e-9 * max_abs(&m));
            let e = p.eye();
            assert!(len3(sub3(e, eye)) < 1e-6, "{e:?} {eye:?}");
        }
    }

    #[test]
    fn non_perspective_matrices_are_refused() {
        assert!(Persp::split(&IDENTITY).is_none(), "orthographic");
        assert!(Persp::split(&[[0.0; 4]; 4]).is_none());
        let mut m = build::view_projection(1.2, [1.0, 2.0, 3.0], 0.4, 0.1);
        m[2][0] += 0.5; // depth row no longer parallel to w
        assert!(Persp::split(&m).is_none());
        let rows = [[1.0, 2.0, 3.0, 4.0]; 4];
        assert!(Persp::split(&rows).is_none(), "four equal rows");
    }

    #[test]
    fn camera_lerp_moves_the_eye_in_a_line() {
        let a = Persp::split(&build::view_projection(1.2, [0.0, 0.0, 0.0], 0.0, 0.0)).unwrap();
        let b = Persp::split(&build::view_projection(1.2, [2.0, 0.0, 0.0], 1.0, 0.0)).unwrap();
        let h = a.lerp_camera(&b, 0.5);
        assert!(len3(sub3(h.eye(), [1.0, 0.0, 0.0])) < 1e-9);
        let want = Persp::split(&build::view_projection(1.2, [1.0, 0.0, 0.0], 0.5, 0.0)).unwrap();
        assert!(max_diff(&h.matrix(), &want.matrix()) < 1e-9);
        assert!(max_diff(&a.lerp_camera(&b, 0.0).matrix(), &a.matrix()) < 1e-9);
        assert!(max_diff(&a.lerp_camera(&b, 1.0).matrix(), &b.matrix()) < 1e-9);
    }

    #[test]
    fn cuts_are_found() {
        let a = Persp::split(&build::view_projection(1.2, [0.0; 3], 0.0, 0.0)).unwrap();
        let near = Persp::split(&build::view_projection(1.2, [0.3, 0.0, 0.0], 0.1, 0.0)).unwrap();
        let far = Persp::split(&build::view_projection(1.2, [30.0, 0.0, 0.0], 0.1, 0.0)).unwrap();
        let turned = Persp::split(&build::view_projection(1.2, [0.0; 3], 2.0, 0.0)).unwrap();
        let zoomed = Persp::split(&build::view_projection(0.4, [0.0; 3], 0.0, 0.0)).unwrap();
        let (d, ang, zoom) = (3.0, 1.0, 0.05);
        assert!(!a.cut(&near, d, ang, zoom));
        assert!(a.cut(&far, d, ang, zoom));
        assert!(a.cut(&turned, d, ang, zoom));
        assert!(a.cut(&zoomed, d, ang, zoom));
    }

    #[test]
    fn world_view_projections_split_too() {
        let vp = build::view_projection(1.2, [3.0, 1.0, 2.0], 0.5, 0.1);
        let world = Similarity {
            rotation: build::axis([0.0, 0.0, 1.0], 0.8),
            scale: 1.0,
            translation: [5.0, 6.0, 0.5],
        };
        assert!(Persp::split(&mul(&vp, &world.to_mat4())).is_some());
    }

    #[test]
    fn game_cameras_are_centred_and_pixel_perspectives_are_not() {
        let game = build::view_projection(1.2, [3.0, -2.0, 1.5], 0.3, 0.05);
        assert!(Persp::split(&game).unwrap().centred());
        // Screen-space UI: the axis half a screen off, focal length in pixels.
        let ui: Mat4 = [
            [400.0, 0.0, 320.0, 0.0],
            [0.0, -400.0, 240.0, 0.0],
            [0.0, 0.0, 1.0, -0.1],
            [0.0, 0.0, 1.0, 0.0],
        ];
        assert!(!Persp::split(&ui).unwrap().centred());
        // Off-centre by a fifth of the screen (a sub-view's lens shift).
        let mut shifted = build::projection(1.2, 4.0 / 3.0, 0.1, 100.0);
        shifted[0][2] = 0.2;
        assert!(!Persp::split(&shifted).unwrap().centred());
    }
}
