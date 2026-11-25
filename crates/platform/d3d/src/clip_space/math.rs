//! Small matrix and quaternion helpers for [`super::camera`], in `f64` so that splitting a matrix and putting it back loses less than
//! the `f32` constants it came from.
//!
//! Convention: a [`Mat4`] is four rows and transforms column vectors
//! (`clip = M * v`), which is how a vertex program uses four constant
//! registers with `dp4` (one register per row).

/// Four rows.
pub type Mat4 = [[f64; 4]; 4];

/// The identity.
pub const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// `a * b`.
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut m = [[0.0; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

/// The inverse, by Gauss-Jordan with partial pivoting; `None` when
/// singular (relative to the matrix's own size).
pub fn inverse(m: &Mat4) -> Option<Mat4> {
    let scale = max_abs(m);
    if scale == 0.0 || !scale.is_finite() {
        return None;
    }
    let mut a = *m;
    let mut inv = IDENTITY;
    for col in 0..4 {
        let pivot = (col..4).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[pivot][col].abs() <= 1e-12 * scale {
            return None;
        }
        a.swap(col, pivot);
        inv.swap(col, pivot);
        let p = a[col][col];
        for j in 0..4 {
            a[col][j] /= p;
            inv[col][j] /= p;
        }
        for row in 0..4 {
            if row != col {
                let f = a[row][col];
                if f != 0.0 {
                    for j in 0..4 {
                        a[row][j] -= f * a[col][j];
                        inv[row][j] -= f * inv[col][j];
                    }
                }
            }
        }
    }
    Some(inv)
}

/// Largest absolute entry.
pub fn max_abs(m: &Mat4) -> f64 {
    m.iter().flatten().fold(0.0f64, |a, v| a.max(v.abs()))
}

/// Largest absolute difference between entries.
pub fn max_diff(a: &Mat4, b: &Mat4) -> f64 {
    a.iter()
        .flatten()
        .zip(b.iter().flatten())
        .fold(0.0f64, |m, (x, y)| m.max((x - y).abs()))
}

pub(crate) fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn len3(a: [f64; 3]) -> f64 {
    dot3(a, a).sqrt()
}

pub(crate) fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn scale3(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub(crate) fn lerp3(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

pub(crate) fn xyz(r: [f64; 4]) -> [f64; 3] {
    [r[0], r[1], r[2]]
}

/// A unit quaternion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    /// Scalar part.
    pub w: f64,
    /// Vector part.
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Quat {
    /// No rotation.
    pub const IDENTITY: Quat = Quat {
        w: 1.0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    /// The rotation of orthonormal `r` (rows; determinant +1).
    pub fn from_rotation(r: &[[f64; 3]; 3]) -> Quat {
        let trace = r[0][0] + r[1][1] + r[2][2];
        let q = if trace > 0.0 {
            let s = (trace + 1.0).sqrt() * 2.0;
            Quat {
                w: 0.25 * s,
                x: (r[2][1] - r[1][2]) / s,
                y: (r[0][2] - r[2][0]) / s,
                z: (r[1][0] - r[0][1]) / s,
            }
        } else if r[0][0] > r[1][1] && r[0][0] > r[2][2] {
            let s = (1.0 + r[0][0] - r[1][1] - r[2][2]).sqrt() * 2.0;
            Quat {
                w: (r[2][1] - r[1][2]) / s,
                x: 0.25 * s,
                y: (r[0][1] + r[1][0]) / s,
                z: (r[0][2] + r[2][0]) / s,
            }
        } else if r[1][1] > r[2][2] {
            let s = (1.0 + r[1][1] - r[0][0] - r[2][2]).sqrt() * 2.0;
            Quat {
                w: (r[0][2] - r[2][0]) / s,
                x: (r[0][1] + r[1][0]) / s,
                y: 0.25 * s,
                z: (r[1][2] + r[2][1]) / s,
            }
        } else {
            let s = (1.0 + r[2][2] - r[0][0] - r[1][1]).sqrt() * 2.0;
            Quat {
                w: (r[1][0] - r[0][1]) / s,
                x: (r[0][2] + r[2][0]) / s,
                y: (r[1][2] + r[2][1]) / s,
                z: 0.25 * s,
            }
        };
        q.normalized()
    }

    /// The rotation matrix (rows).
    pub fn to_rotation(self) -> [[f64; 3]; 3] {
        let Quat { w, x, y, z } = self;
        [
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - w * z),
                2.0 * (x * z + w * y),
            ],
            [
                2.0 * (x * y + w * z),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - w * x),
            ],
            [
                2.0 * (x * z - w * y),
                2.0 * (y * z + w * x),
                1.0 - 2.0 * (x * x + y * y),
            ],
        ]
    }

    fn dot(self, o: Quat) -> f64 {
        self.w * o.w + self.x * o.x + self.y * o.y + self.z * o.z
    }

    fn normalized(self) -> Quat {
        let n = self.dot(self).sqrt();
        if n == 0.0 {
            return Quat::IDENTITY;
        }
        Quat {
            w: self.w / n,
            x: self.x / n,
            y: self.y / n,
            z: self.z / n,
        }
    }

    /// The angle between two orientations, in radians (0..=pi).
    pub fn angle_to(self, o: Quat) -> f64 {
        2.0 * self.dot(o).abs().min(1.0).acos()
    }

    /// Spherical interpolation along the shorter arc: `self` at 0, `o` at 1.
    /// Falls back to a normalized lerp for nearly equal rotations.
    pub fn slerp(self, o: Quat, t: f64) -> Quat {
        let mut d = self.dot(o);
        let o = if d < 0.0 {
            d = -d;
            Quat {
                w: -o.w,
                x: -o.x,
                y: -o.y,
                z: -o.z,
            }
        } else {
            o
        };
        let (a, b) = if d > 0.9995 {
            (1.0 - t, t)
        } else {
            let th = d.min(1.0).acos();
            let s = th.sin();
            (((1.0 - t) * th).sin() / s, (t * th).sin() / s)
        };
        Quat {
            w: a * self.w + b * o.w,
            x: a * self.x + b * o.x,
            y: a * self.y + b * o.y,
            z: a * self.z + b * o.z,
        }
        .normalized()
    }
}

/// A rotation, a uniform scale and a translation: `x -> s R x + t`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Similarity {
    pub rotation: Quat,
    pub scale: f64,
    pub translation: [f64; 3],
}

impl Similarity {
    /// Splits an affine matrix whose 3x3 part is a rotation times a uniform
    /// positive scale. `tol` bounds the relative error of that claim and of
    /// the bottom row being `0 0 0 1`; anything else (shear, mirror,
    /// projection, non-uniform scale) is `None`.
    pub fn from_affine(m: &Mat4, tol: f64) -> Option<Similarity> {
        if !m.iter().flatten().all(|v| v.is_finite()) {
            return None;
        }
        let bottom = m[3];
        if bottom[0].abs() > tol || bottom[1].abs() > tol || bottom[2].abs() > tol {
            return None;
        }
        if (bottom[3] - 1.0).abs() > tol {
            return None;
        }
        let a = [xyz(m[0]), xyz(m[1]), xyz(m[2])];
        let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
        if det.is_nan() || det <= 0.0 {
            return None;
        }
        let s = det.cbrt();
        let s2 = s * s;
        // A Aᵀ must be s² I.
        for i in 0..3 {
            for j in 0..3 {
                let want = if i == j { s2 } else { 0.0 };
                if (dot3(a[i], a[j]) - want).abs() > tol * s2 {
                    return None;
                }
            }
        }
        let r = [
            scale3(a[0], 1.0 / s),
            scale3(a[1], 1.0 / s),
            scale3(a[2], 1.0 / s),
        ];
        Some(Similarity {
            rotation: Quat::from_rotation(&r),
            scale: s,
            translation: [m[0][3], m[1][3], m[2][3]],
        })
    }

    /// The matrix.
    pub fn to_mat4(&self) -> Mat4 {
        let r = self.rotation.to_rotation();
        let s = self.scale;
        let t = self.translation;
        [
            [r[0][0] * s, r[0][1] * s, r[0][2] * s, t[0]],
            [r[1][0] * s, r[1][1] * s, r[1][2] * s, t[1]],
            [r[2][0] * s, r[2][1] * s, r[2][2] * s, t[2]],
            [0.0, 0.0, 0.0, 1.0],
        ]
    }

    /// Translation lerped, rotation slerped, scale lerped.
    pub fn lerp(&self, o: &Similarity, t: f64) -> Similarity {
        Similarity {
            rotation: self.rotation.slerp(o.rotation, t),
            scale: self.scale + (o.scale - self.scale) * t,
            translation: lerp3(self.translation, o.translation, t),
        }
    }

    /// Whether going from `self` to `o` in one tick is a jump to snap to
    /// rather than motion to blend: further than `distance`, turned more
    /// than `angle` radians, or scaled by more than a quarter.
    pub fn jumps(&self, o: &Similarity, distance: f64, angle: f64) -> bool {
        len3(sub3(self.translation, o.translation)) > distance
            || self.rotation.angle_to(o.rotation) > angle
            || (o.scale / self.scale - 1.0).abs() > 0.25
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rot(axis: [f64; 3], angle: f64) -> Quat {
        let n = len3(axis);
        let (s, c) = (angle / 2.0).sin_cos();
        Quat {
            w: c,
            x: axis[0] / n * s,
            y: axis[1] / n * s,
            z: axis[2] / n * s,
        }
    }

    #[test]
    fn inverse_undoes() {
        let m = [
            [2.0, 0.5, 0.0, 3.0],
            [0.1, 1.5, 0.2, -1.0],
            [0.0, 0.3, 0.9, 4.0],
            [0.0, 0.0, 1.0, 0.5],
        ];
        let i = inverse(&m).unwrap();
        assert!(max_diff(&mul(&m, &i), &IDENTITY) < 1e-12);
        assert!(inverse(&[[0.0; 4]; 4]).is_none());
    }

    #[test]
    fn quaternions_round_trip_and_slerp() {
        for (axis, angle) in [
            ([0.0, 0.0, 1.0], 0.3),
            ([1.0, 2.0, -0.5], 2.9),
            ([1.0, 0.0, 0.0], 3.1),
        ] {
            let q = rot(axis, angle);
            let back = Quat::from_rotation(&q.to_rotation());
            assert!(q.angle_to(back) < 1e-9, "{q:?} {back:?}");
        }
        let a = rot([0.0, 1.0, 0.0], 0.0);
        let b = rot([0.0, 1.0, 0.0], 1.0);
        let h = a.slerp(b, 0.5);
        assert!((h.angle_to(rot([0.0, 1.0, 0.0], 0.5))).abs() < 1e-9);
        assert!(a.slerp(b, 0.0).angle_to(a) < 1e-9);
        assert!(a.slerp(b, 1.0).angle_to(b) < 1e-9);
    }

    #[test]
    fn similarities_split_and_reject() {
        let s = Similarity {
            rotation: rot([0.3, 1.0, 0.2], 0.7),
            scale: 1.7,
            translation: [1.0, -2.0, 30.0],
        };
        let back = Similarity::from_affine(&s.to_mat4(), 1e-6).unwrap();
        assert!(back.rotation.angle_to(s.rotation) < 1e-9);
        assert!((back.scale - 1.7).abs() < 1e-9);
        assert_eq!(back.translation, s.translation);
        // Non-uniform scale and a projection row are not similarities.
        let mut m = s.to_mat4();
        m[0][0] *= 2.0;
        assert!(Similarity::from_affine(&m, 1e-4).is_none());
        let mut m = s.to_mat4();
        m[3][2] = 1.0;
        assert!(Similarity::from_affine(&m, 1e-4).is_none());
        // A mirror is not either.
        let mut m = IDENTITY;
        m[0][0] = -1.0;
        assert!(Similarity::from_affine(&m, 1e-4).is_none());
    }
}
