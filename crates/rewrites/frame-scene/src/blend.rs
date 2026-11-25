//! Blending two scenes for display frames between ticks
//! (docs/graphics/framescene.md, section 7 and slice 8).
//!
//! Pure: no clock of its own, no guest, no GPU. [`plan`] decides from the
//! two scenes' stamps whether a display frame draws the newer scene `B`
//! itself or a blend toward it. [`blend`] computes the blended view
//! cameras, node matrices, palette rows and light candidates, each array
//! parallel to `B`'s so the renderer substitutes values and keeps `B`'s
//! lists, materials and carried ops. Nothing is extrapolated and nothing
//! is blended at `alpha >= 1`: [`Plan::Newest`] is `B`'s own values.

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::camera::Camera;
use crate::light::LightCandidate;
use crate::node::NodeMatrix;
use crate::scene::{FrameScene, Row, View, ViewKey, ViewKind};

/// Scenes published further apart than this are never blended.
pub const MAX_GAP: Duration = Duration::from_millis(45);

/// A node or palette row that moved further than this (world units, or
/// camera-space units for the first person) between the two scenes is
/// a jump, not motion: it draws at `B`'s value.
pub const MAX_MOVE: f32 = 5.0;

/// What a display frame draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Plan {
    /// `B` itself: `alpha >= 1`, a late next scene, a pause or load, a
    /// cut, or scenes too far apart.
    Newest,
    /// A blend from `A` toward `B`; `0 <= alpha < 1`.
    Blend(f32),
}

/// The plan for a display frame due at `due`, with `a` the older scene
/// and `b` the newer: `alpha = clamp((due - B.published) / (B.published -
/// A.published), 0, 1)`, one tick of delay.
pub fn plan(a: &FrameScene, b: &FrameScene, due: Instant) -> Plan {
    let (Some(pa), Some(pb)) = (a.stamp.published, b.stamp.published) else {
        return Plan::Newest;
    };
    let Some(gap) = pb.checked_duration_since(pa) else {
        return Plan::Newest;
    };
    plan_for(a, b, gap, due.saturating_duration_since(pb))
}

/// [`plan`] with the publish gap and the time since `B`'s publish given.
pub fn plan_for(a: &FrameScene, b: &FrameScene, gap: Duration, since: Duration) -> Plan {
    if b.stamp.ticks_run == 0
        || !b.cut.is_empty()
        || a.id.map_epoch != b.id.map_epoch
        || gap.is_zero()
        || gap > MAX_GAP
    {
        return Plan::Newest;
    }
    let alpha = since.as_secs_f32() / gap.as_secs_f32();
    if alpha >= 1.0 { Plan::Newest } else { Plan::Blend(alpha.max(0.0)) }
}

/// What a blend did, for `render.*` reporting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct BlendCounts {
    pub cameras_blended: u32,
    pub cameras_snapped: u32,
    pub instances_blended: u32,
    pub instances_snapped: u32,
    pub parts_blended: u32,
    pub parts_snapped: u32,
    pub lights_blended: u32,
}

/// The blended values, parallel to `B`'s arrays. A value that did not
/// blend (the snap rules) is `B`'s own.
#[derive(Clone, Debug, PartialEq)]
pub struct Blended {
    pub alpha: f32,
    /// Parallel to `B.views`.
    pub cameras: Vec<Camera>,
    /// Parallel to `B.nodes`.
    pub nodes: Vec<NodeMatrix>,
    /// Parallel to `B.rows`.
    pub rows: Vec<Row>,
    /// Parallel to `B.lights`; `None` where the light did not blend.
    pub lights: Vec<Option<LightCandidate>>,
    pub counts: BlendCounts,
}

type V3 = [f32; 3];

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp3(a: V3, b: V3, t: f32) -> V3 {
    std::array::from_fn(|i| lerp(a[i], b[i], t))
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}

fn normalize(a: V3) -> Option<V3> {
    let l = len(a);
    (l.is_finite() && l > 1e-6).then(|| [a[0] / l, a[1] / l, a[2] / l])
}

fn dist(a: V3, b: V3) -> f32 {
    len([a[0] - b[0], a[1] - b[1], a[2] - b[2]])
}

/// A unit quaternion, `[x, y, z, w]`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Quat([f32; 4]);

impl Quat {
    /// From the columns of a rotation matrix.
    fn from_columns(c: [V3; 3]) -> Quat {
        let r = |row: usize, col: usize| c[col][row];
        let (r00, r01, r02) = (r(0, 0), r(0, 1), r(0, 2));
        let (r10, r11, r12) = (r(1, 0), r(1, 1), r(1, 2));
        let (r20, r21, r22) = (r(2, 0), r(2, 1), r(2, 2));
        let tr = r00 + r11 + r22;
        let q = if tr > 0.0 {
            let s = (tr + 1.0).sqrt() * 2.0;
            [(r21 - r12) / s, (r02 - r20) / s, (r10 - r01) / s, s / 4.0]
        } else if r00 > r11 && r00 > r22 {
            let s = (1.0 + r00 - r11 - r22).sqrt() * 2.0;
            [s / 4.0, (r01 + r10) / s, (r02 + r20) / s, (r21 - r12) / s]
        } else if r11 > r22 {
            let s = (1.0 + r11 - r00 - r22).sqrt() * 2.0;
            [(r01 + r10) / s, s / 4.0, (r12 + r21) / s, (r02 - r20) / s]
        } else {
            let s = (1.0 + r22 - r00 - r11).sqrt() * 2.0;
            [(r02 + r20) / s, (r12 + r21) / s, s / 4.0, (r10 - r01) / s]
        };
        let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        Quat(q.map(|v| v / n))
    }

    fn columns(self) -> [V3; 3] {
        let [x, y, z, w] = self.0;
        [
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)],
            [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)],
            [2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y)],
        ]
    }

    /// The shortest-path spherical interpolation.
    fn slerp(self, other: Quat, t: f32) -> Quat {
        let mut b = other.0;
        let mut d: f32 = self.0.iter().zip(&b).map(|(x, y)| x * y).sum();
        if d < 0.0 {
            d = -d;
            b = b.map(|v| -v);
        }
        let (wa, wb) = if d > 0.9995 {
            (1.0 - t, t)
        } else {
            let theta = d.clamp(-1.0, 1.0).acos();
            let s = theta.sin();
            (((1.0 - t) * theta).sin() / s, (t * theta).sin() / s)
        };
        let q: [f32; 4] = std::array::from_fn(|i| self.0[i] * wa + b[i] * wb);
        let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        Quat(q.map(|v| v / n))
    }
}

/// A 3x4 transform: `columns[j]` is the image of axis `j` (scale
/// included), `t` the translation.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Affine {
    columns: [V3; 3],
    t: V3,
}

/// An [`Affine`] taken apart: rotation, per-axis scale and translation.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Parts {
    rotation: Quat,
    scale: V3,
    t: V3,
}

impl Affine {
    /// From a palette row's three registers (each a row of the 3x4).
    fn from_rows(r: &[Row]) -> Option<Affine> {
        let [r0, r1, r2] = r else { return None };
        Some(Affine {
            columns: std::array::from_fn(|j| [r0[j], r1[j], r2[j]]),
            t: [r0[3], r1[3], r2[3]],
        })
    }

    fn to_rows(self) -> [Row; 3] {
        let c = self.columns;
        std::array::from_fn(|i| [c[0][i], c[1][i], c[2][i], self.t[i]])
    }

    fn from_node(n: &NodeMatrix) -> Affine {
        let s = n.scale;
        Affine {
            columns: [n.forward, n.left, n.up].map(|v| [v[0] * s, v[1] * s, v[2] * s]),
            t: n.position,
        }
    }

    /// `None` when the scales differ: a node matrix has one.
    fn to_node(self) -> Option<NodeMatrix> {
        let s = self.columns.map(len);
        let mean = (s[0] + s[1] + s[2]) / 3.0;
        if s.iter().any(|v| (v - mean).abs() > mean * 1e-3) {
            return None;
        }
        let [forward, left, up] = self.columns.map(|c| [c[0] / mean, c[1] / mean, c[2] / mean]);
        Some(NodeMatrix { scale: mean, forward, left, up, position: self.t })
    }

    fn apply(&self, v: V3) -> V3 {
        std::array::from_fn(|i| {
            self.columns[0][i] * v[0] + self.columns[1][i] * v[1] + self.columns[2][i] * v[2] + self.t[i]
        })
    }

    /// `self ∘ inner`: first `inner`, then `self`.
    fn compose(&self, inner: &Affine) -> Affine {
        let rot = |v: V3| -> V3 {
            std::array::from_fn(|i| {
                self.columns[0][i] * v[0] + self.columns[1][i] * v[1] + self.columns[2][i] * v[2]
            })
        };
        Affine { columns: inner.columns.map(rot), t: self.apply(inner.t) }
    }

    /// The inverse of a rigid transform (an orthonormal frame).
    fn rigid_inverse(&self) -> Affine {
        let c = self.columns;
        let columns = [
            [c[0][0], c[1][0], c[2][0]],
            [c[0][1], c[1][1], c[2][1]],
            [c[0][2], c[1][2], c[2][2]],
        ];
        let inv = Affine { columns, t: [0.0; 3] };
        let t = inv.apply([-self.t[0], -self.t[1], -self.t[2]]);
        Affine { columns, t }
    }

    /// `None` when the columns are not a rotation times a positive
    /// per-axis scale: a shear, a mirror or a zero scale. Those snap.
    fn decompose(&self) -> Option<Parts> {
        let scale = self.columns.map(len);
        if scale.iter().any(|s| !s.is_finite() || *s < 1e-6) || !self.t.iter().all(|v| v.is_finite()) {
            return None;
        }
        let u: [V3; 3] = std::array::from_fn(|j| {
            let c = self.columns[j];
            [c[0] / scale[j], c[1] / scale[j], c[2] / scale[j]]
        });
        let orthogonal = dot(u[0], u[1]).abs() < 1e-3
            && dot(u[0], u[2]).abs() < 1e-3
            && dot(u[1], u[2]).abs() < 1e-3;
        (orthogonal && dot(u[0], cross(u[1], u[2])) > 0.0)
            .then(|| Parts { rotation: Quat::from_columns(u), scale, t: self.t })
    }

    fn blend(&self, other: &Affine, t: f32) -> Option<Affine> {
        let (a, b) = (self.decompose()?, other.decompose()?);
        let u = a.rotation.slerp(b.rotation, t).columns();
        let s = lerp3(a.scale, b.scale, t);
        Some(Affine {
            columns: std::array::from_fn(|j| [u[j][0] * s[j], u[j][1] * s[j], u[j][2] * s[j]]),
            t: lerp3(a.t, b.t, t),
        })
    }
}

/// A camera's frame: forward, left, up and the eye. The same basis
/// `RenderCamera::view` builds (right is `forward x up`).
fn camera_frame(c: &Camera) -> Option<Affine> {
    if !c.is_usable() {
        return None;
    }
    let f = normalize(c.forward)?;
    let l = normalize(cross(c.up, f))?;
    let u = cross(f, l);
    Some(Affine { columns: [f, l, u], t: c.position })
}

fn blend_camera(a: &Camera, b: &Camera, t: f32) -> Option<Camera> {
    let frame = camera_frame(a)?.blend(&camera_frame(b)?, t)?;
    let [forward, _, up] = frame.columns;
    let projection_xy = match (a.projection_xy, b.projection_xy) {
        (Some(pa), Some(pb)) => Some(std::array::from_fn(|i| lerp(pa[i], pb[i], t))),
        _ => b.projection_xy,
    };
    Some(Camera {
        position: frame.t,
        forward,
        up,
        fov: lerp(a.fov, b.fov, t),
        projection_xy,
        ..*b
    })
}

fn blend_light(a: &LightCandidate, b: &LightCandidate, t: f32) -> LightCandidate {
    let dir = |x: V3, y: V3| normalize(lerp3(x, y, t)).unwrap_or(y);
    LightCandidate {
        position: lerp3(a.position, b.position, t),
        forward: dir(a.forward, b.forward),
        up: dir(a.up, b.up),
        colour_a: lerp3(a.colour_a, b.colour_a, t),
        colour_b: lerp3(a.colour_b, b.colour_b, t),
        intensity: lerp(a.intensity, b.intensity, t),
        ..b.clone()
    }
}

/// Blends `a` toward `b` by `alpha` (from [`Plan::Blend`]).
///
/// With `B` as the base: a view blends with the same-keyed view of `A`
/// unless it is cut or unmatched; an instance blends with the same-keyed
/// instance of the same view unless it has no match, another model or
/// node count, is teleported, or a node moved further than [`MAX_MOVE`];
/// a part's rows blend with the same-keyed part (same row count, three
/// registers a row, none moved further than [`MAX_MOVE`]); a light blends
/// with the light of the same datum and definition. The first person's
/// nodes and rows blend in the blended camera's space, so the weapon
/// stays locked to it. Cameras, nodes and rows that do not blend are
/// `B`'s.
pub fn blend(a: &FrameScene, b: &FrameScene, alpha: f32) -> Blended {
    let t = alpha.clamp(0.0, 1.0);
    let mut out = Blended {
        alpha: t,
        cameras: b.views.iter().map(|v| v.camera).collect(),
        nodes: b.nodes.clone(),
        rows: b.rows.clone(),
        lights: vec![None; b.lights.len()],
        counts: BlendCounts::default(),
    };
    for (vi, vb) in b.views.iter().enumerate() {
        let Some(va) = a.view(vb.key).filter(|_| !vb.cut) else {
            out.counts.cameras_snapped += 1;
            continue;
        };
        match blend_camera(&va.camera, &vb.camera, t) {
            Some(c) => {
                out.cameras[vi] = c;
                out.counts.cameras_blended += 1;
            }
            None => out.counts.cameras_snapped += 1,
        }
        blend_instances(a, b, va, vb, &out.cameras[vi].clone(), &mut out);
        blend_lights(a, b, va, vb, &mut out);
    }
    out
}

/// A frame to blend in: the identity for the world, the view's camera
/// for the first person.
struct Space {
    a: Affine,
    b: Affine,
    blended: Affine,
}

fn space(va: &View, vb: &View, blended: &Camera) -> Option<Space> {
    (vb.key.kind == ViewKind::FirstPerson).then_some(())?;
    Some(Space {
        a: camera_frame(&va.camera)?,
        b: camera_frame(&vb.camera)?,
        blended: camera_frame(blended)?,
    })
}

/// Blends `a` to `b` in `space`: both into its frame, blended, back out
/// by the blended frame. `None` when either does not decompose or the
/// motion is a jump.
fn blend_affine(a: &Affine, b: &Affine, t: f32, space: Option<&Space>) -> Option<Affine> {
    let (la, lb) = match space {
        Some(s) => (s.a.rigid_inverse().compose(a), s.b.rigid_inverse().compose(b)),
        None => (*a, *b),
    };
    if dist(la.t, lb.t) > MAX_MOVE {
        return None;
    }
    let l = la.blend(&lb, t)?;
    Some(match space {
        Some(s) => s.blended.compose(&l),
        None => l,
    })
}

fn blend_instances(a: &FrameScene, b: &FrameScene, va: &View, vb: &View, camera: &Camera, out: &mut Blended) {
    let space = space(va, vb, camera);
    let t = out.alpha;
    for ib in b.instances_of(vb) {
        let Some(ia) = a.instance(va, ib.key) else {
            out.counts.instances_snapped += 1;
            continue;
        };
        let (na, nb) = (a.nodes_of(ia), b.nodes_of(ib));
        let blended: Option<Vec<NodeMatrix>> = (!ib.teleported
            && ia.model == ib.model
            && na.len() == nb.len())
        .then(|| {
            na.iter()
                .zip(nb)
                .map(|(x, y)| {
                    blend_affine(&Affine::from_node(x), &Affine::from_node(y), t, space.as_ref())?
                        .to_node()
                })
                .collect()
        })
        .flatten();
        match blended {
            Some(nodes) => {
                let start = ib.nodes.start as usize;
                out.nodes[start..start + nodes.len()].copy_from_slice(&nodes);
                out.counts.instances_blended += 1;
            }
            None => out.counts.instances_snapped += 1,
        }
        if ib.teleported || ia.model != ib.model {
            out.counts.parts_snapped += ib.parts.len() as u32;
            continue;
        }
        for pb in b.parts_of(ib) {
            let pa = a.parts_of(ia).iter().find(|p| p.key == pb.key && p.layer == pb.layer);
            let rows = pa.and_then(|pa| blend_rows(a.rows_of(pa), b.rows_of(pb), t, space.as_ref()));
            match rows {
                Some(rows) => {
                    let start = pb.rows.start as usize;
                    out.rows[start..start + rows.len()].copy_from_slice(&rows);
                    out.counts.parts_blended += 1;
                }
                None => out.counts.parts_snapped += 1,
            }
        }
    }
}

fn blend_rows(a: &[Row], b: &[Row], t: f32, space: Option<&Space>) -> Option<Vec<Row>> {
    if a.len() != b.len() || b.len() % 3 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(b.len());
    for (ra, rb) in a.chunks_exact(3).zip(b.chunks_exact(3)) {
        let m = blend_affine(&Affine::from_rows(ra)?, &Affine::from_rows(rb)?, t, space)?;
        out.extend(m.to_rows());
    }
    Some(out)
}

fn blend_lights(a: &FrameScene, b: &FrameScene, va: &View, vb: &View, out: &mut Blended) {
    let first = vb.lights.start as usize;
    for (k, lb) in b.lights_of(vb).iter().enumerate() {
        let la = a
            .lights_of(va)
            .iter()
            .find(|l| l.handle == lb.handle && l.definition == lb.definition);
        if let (Some(ca), Some(cb)) = (la.and_then(|l| l.candidate.as_ref()), lb.candidate.as_ref()) {
            out.lights[first + k] = Some(blend_light(ca, cb, out.alpha));
            out.counts.lights_blended += 1;
        }
    }
}

/// The view's blended camera, by key.
impl Blended {
    pub fn camera(&self, scene: &FrameScene, key: ViewKey) -> Option<&Camera> {
        scene.views.iter().position(|v| v.key == key).map(|i| &self.cameras[i])
    }
}

#[cfg(test)]
mod tests;
