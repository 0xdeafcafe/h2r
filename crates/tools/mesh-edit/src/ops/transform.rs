//! Moving vertices, and keeping smooth fans' normals right as they move.

use std::collections::{BTreeMap, BTreeSet};

use crate::attrs::dirty;
use crate::ids::{FaceId, LoopId, VertId};
use crate::math::{self, Vec3};
use crate::mesh::{Mesh, face_flags};

/// An affine map `p' = L p + t`, three rows of `[L | t]`: the 4x3 matrix
/// of `MeshOp::Transform`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Affine {
    pub rows: [[f32; 4]; 3],
}

impl Default for Affine {
    fn default() -> Self {
        Affine::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Affine =
        Affine { rows: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]] };

    pub fn translate(t: Vec3) -> Affine {
        let mut a = Affine::IDENTITY;
        for i in 0..3 {
            a.rows[i][3] = t[i];
        }
        a
    }

    pub fn scale(s: Vec3) -> Affine {
        let mut a = Affine::IDENTITY;
        for i in 0..3 {
            a.rows[i][i] = s[i];
        }
        a
    }

    /// A rotation by `angle` radians about `axis` through the origin
    /// (counter-clockwise looking down the axis).
    pub fn rotate(axis: Vec3, angle: f32) -> Affine {
        let k = math::normalize(axis);
        let (s, c) = angle.sin_cos();
        let t = 1.0 - c;
        Affine {
            rows: [
                [c + k[0] * k[0] * t, k[0] * k[1] * t - k[2] * s, k[0] * k[2] * t + k[1] * s, 0.0],
                [k[1] * k[0] * t + k[2] * s, c + k[1] * k[1] * t, k[1] * k[2] * t - k[0] * s, 0.0],
                [k[2] * k[0] * t - k[1] * s, k[2] * k[1] * t + k[0] * s, c + k[2] * k[2] * t, 0.0],
            ],
        }
    }

    /// This map applied about `pivot`: `p' = L (p - pivot) + pivot + t`.
    pub fn about(&self, pivot: Vec3) -> Affine {
        let mut out = *self;
        let moved = self.apply_vector(pivot);
        for i in 0..3 {
            out.rows[i][3] += pivot[i] - moved[i];
        }
        out
    }

    fn row(&self, i: usize) -> Vec3 {
        [self.rows[i][0], self.rows[i][1], self.rows[i][2]]
    }

    pub fn apply(&self, p: Vec3) -> Vec3 {
        let v = self.apply_vector(p);
        [v[0] + self.rows[0][3], v[1] + self.rows[1][3], v[2] + self.rows[2][3]]
    }

    /// The linear part only.
    pub fn apply_vector(&self, v: Vec3) -> Vec3 {
        [math::dot(self.row(0), v), math::dot(self.row(1), v), math::dot(self.row(2), v)]
    }

    /// A normal through the inverse transpose of the linear part,
    /// renormalized. Zero stays zero.
    pub fn apply_normal(&self, n: Vec3) -> Vec3 {
        let (a, b, c) = (self.row(0), self.row(1), self.row(2));
        // det * inverse-transpose has rows b x c, c x a, a x b.
        let cof = [math::cross(b, c), math::cross(c, a), math::cross(a, b)];
        let det = math::dot(a, cof[0]);
        let m = [math::dot(cof[0], n), math::dot(cof[1], n), math::dot(cof[2], n)];
        math::normalize(if det < 0.0 { math::scale(m, -1.0) } else { m })
    }
}

impl Mesh {
    /// Moves `moved` by `xf`, marks their corners' positions dirty, and
    /// updates the normals of the smooth fans whose faces moved.
    pub(crate) fn move_verts(&mut self, moved: &BTreeSet<VertId>, xf: &Affine) {
        for &v in moved {
            let p = xf.apply(self.position(v));
            self.set_position(v, p);
            for l in self.vert_loops(v) {
                self.l_mut(l).attrs.dirty |= dirty::POSITION;
            }
        }
        let faces: BTreeSet<FaceId> = moved.iter().flat_map(|&v| self.vert_faces(v)).collect();
        for &f in &faces {
            self.f_mut(f).flags |= face_flags::LIGHTMAP_STALE;
        }
        self.refresh_fans(&faces, Some((moved, xf)));
    }

    /// Recomputes the smooth fans that include a corner of `faces`. A fan
    /// is the corners of one vert whose normals are bit-equal. A fan all of
    /// whose faces moved rigidly with `rigid`'s map keeps its normal,
    /// turned by the map; any other gets the angle-weighted average of its
    /// faces' normals. Fans' normals and frames are marked dirty.
    pub(crate) fn refresh_fans(
        &mut self,
        faces: &BTreeSet<FaceId>,
        rigid: Option<(&BTreeSet<VertId>, &Affine)>,
    ) {
        let verts: BTreeSet<VertId> = faces.iter().flat_map(|&f| self.face_verts(f)).collect();
        let mut updates: Vec<(LoopId, Vec3)> = Vec::new();
        for v in verts {
            let mut fans: BTreeMap<[u32; 3], Vec<LoopId>> = BTreeMap::new();
            for l in self.vert_loops(v) {
                let n = self.l(l).attrs.normal;
                fans.entry(n.map(f32::to_bits)).or_default().push(l);
            }
            for (bits, fan) in fans {
                if !fan.iter().any(|&l| faces.contains(&self.l(l).f)) {
                    continue;
                }
                let old = bits.map(f32::from_bits);
                let turned = rigid.and_then(|(moved, xf)| {
                    let all_moved = fan
                        .iter()
                        .all(|&l| self.face_verts(self.l(l).f).iter().all(|v| moved.contains(v)));
                    (all_moved && old != [0.0; 3]).then(|| xf.apply_normal(old))
                });
                let n = turned.unwrap_or_else(|| self.fan_normal(&fan, old));
                for l in fan {
                    updates.push((l, n));
                }
            }
        }
        for (l, n) in updates {
            let attrs = &mut self.l_mut(l).attrs;
            attrs.normal = n;
            attrs.dirty |= dirty::NORMAL | dirty::FRAME;
        }
    }

    /// The angle-weighted average of the fan's faces' normals; `fallback`
    /// if that is zero.
    pub(crate) fn fan_normal(&self, fan: &[LoopId], fallback: Vec3) -> Vec3 {
        let mut sum = [0.0f32; 3];
        for &l in fan {
            let n = self.face_normal(self.l(l).f);
            sum = math::add(sum, math::scale(n, self.corner_angle(l)));
        }
        let n = math::normalize(sum);
        if n == [0.0; 3] { fallback } else { n }
    }

    /// The interior angle of `l`'s face at its corner.
    pub(crate) fn corner_angle(&self, l: LoopId) -> f32 {
        let lp = self.l(l);
        let v = self.position(lp.v);
        let a = self.position(self.l(lp.next).v);
        let b = self.position(self.l(lp.prev).v);
        math::angle(math::sub(a, v), math::sub(b, v))
    }

    /// Sets each corner of `f` to the face's flat normal.
    pub(crate) fn flat_normals(&mut self, f: FaceId) {
        let n = self.face_normal(f);
        for l in self.face_loops(f) {
            let attrs = &mut self.l_mut(l).attrs;
            attrs.normal = n;
            attrs.dirty |= dirty::NORMAL | dirty::FRAME;
        }
    }
}
