//! Sculpt brushes: `MeshOp::Sculpt` (docs/editor/modeling.md,
//! "Sculpting").
//!
//! A stroke is a path of brush centres. Its **footprint** is the path
//! swept by a circle of `radius`, measured across the brush axis (+z by
//! default, so on terrain the footprint is a plan view). A point's weight
//! is the falloff of its distance from the path over the radius, so the
//! result depends on the path, not on how densely it was sampled.
//!
//! A stroke runs in three steps, all inside one op:
//!
//! 1. **Refine**: faces the footprint reaches whose edges are longer than
//!    `detail` (default a quarter of the radius) are split: each long edge
//!    at its midpoint, then the face into quads round a new centre vert.
//!    At most [`MAX_PASSES`] passes; a stroke that would pass
//!    [`MAX_VERTS`] vertices is refused.
//! 2. **Deform** (raise, lower, smooth, flatten): vertices of faces that
//!    face along the axis move along it only, so a terrain's plan stays
//!    where it is.
//! 3. **Cut**: edges crossing the footprint's rim are split on the rim,
//!    each face is split along the rim, and the faces inside go, with any
//!    edge and vertex left with nothing. The hole goes through every layer
//!    under the path, whichever way its faces face.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::attrs::{Corner, blend_skins, dirty};
use crate::error::MeshError;
use crate::ids::{EdgeId, FaceId, VertId};
use crate::math::{self, Vec3};
use crate::mesh::{Mesh, Poly, face_flags};

/// Subdivision passes a stroke may take.
pub const MAX_PASSES: usize = 8;
/// The most vertices a stroke may leave: a Halo 2 section's 16-bit index.
pub const MAX_VERTS: usize = 65_535;

/// What a stroke does.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Brush {
    /// Moves vertices along the axis by `strength` world units at the
    /// centre, times the falloff.
    Raise,
    /// As `Raise`, the other way.
    Lower,
    /// Moves each vertex's height toward its neighbours' mean height, by
    /// the fraction `strength` times the falloff.
    Smooth,
    /// Moves each vertex's height toward `height` (default the first
    /// centre's), by the fraction `strength` times the falloff.
    Flatten,
    /// Removes the faces inside the footprint, splitting the ones on its
    /// rim so the hole's edge follows the rim.
    Cut,
}

impl Brush {
    pub const ALL: [Brush; 5] = [Brush::Raise, Brush::Lower, Brush::Smooth, Brush::Flatten, Brush::Cut];

    pub const fn name(self) -> &'static str {
        match self {
            Brush::Raise => "raise",
            Brush::Lower => "lower",
            Brush::Smooth => "smooth",
            Brush::Flatten => "flatten",
            Brush::Cut => "cut",
        }
    }

    pub fn parse(s: &str) -> Option<Brush> {
        Brush::ALL.into_iter().find(|b| b.name() == s)
    }

    /// Whether `strength` is a fraction (0 to 1) rather than a distance.
    pub const fn fractional(self) -> bool {
        matches!(self, Brush::Smooth | Brush::Flatten)
    }
}

/// How a brush's weight falls from 1 at the centre to 0 at the radius.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Falloff {
    /// `1 - (3t² - 2t³)`: flat at the centre and at the rim.
    #[default]
    Smooth,
    /// `1 - t`.
    Linear,
    /// `sqrt(1 - t²)`: a dome.
    Sphere,
    /// 1 everywhere inside.
    Constant,
}

impl Falloff {
    pub const ALL: [Falloff; 4] = [Falloff::Smooth, Falloff::Linear, Falloff::Sphere, Falloff::Constant];

    pub const fn name(self) -> &'static str {
        match self {
            Falloff::Smooth => "smooth",
            Falloff::Linear => "linear",
            Falloff::Sphere => "sphere",
            Falloff::Constant => "constant",
        }
    }

    pub fn parse(s: &str) -> Option<Falloff> {
        Falloff::ALL.into_iter().find(|f| f.name() == s)
    }

    /// The weight at `t`, the distance over the radius: 0 at 1 and past it.
    pub fn weight(self, t: f32) -> f32 {
        if t.is_nan() || t >= 1.0 {
            return 0.0;
        }
        let t = t.max(0.0);
        match self {
            Falloff::Smooth => 1.0 - t * t * (3.0 - 2.0 * t),
            Falloff::Linear => 1.0 - t,
            Falloff::Sphere => (1.0 - t * t).sqrt(),
            Falloff::Constant => 1.0,
        }
    }

    /// The fraction of the radius at which the weight falls to `w`: the
    /// window's falloff rings. 1 for `Constant`.
    pub fn radius_at(self, w: f32) -> f32 {
        if self == Falloff::Constant {
            return 1.0;
        }
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..32 {
            let mid = 0.5 * (lo + hi);
            if self.weight(mid) > w {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }
}

fn one() -> f32 {
    1.0
}

/// One brush stroke: `MeshOp::Sculpt`'s value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub brush: Brush,
    /// The brush centres in order; one point is a single dab.
    pub points: Vec<Vec3>,
    /// World units, across the axis.
    pub radius: f32,
    /// World units at the centre for raise and lower; a fraction, 0 to 1,
    /// for smooth and flatten; unused by cut.
    #[serde(default = "one")]
    pub strength: f32,
    #[serde(default)]
    pub falloff: Falloff,
    /// Flatten's target height along the axis; default the first centre's.
    #[serde(default)]
    pub height: Option<f32>,
    /// The brush's up; default +z. Need not be unit length.
    #[serde(default)]
    pub axis: Option<Vec3>,
    /// The longest edge the stroke leaves in its footprint; default a
    /// quarter of the radius, at least a 64th of it.
    #[serde(default)]
    pub detail: Option<f32>,
}

impl Stroke {
    /// A one-dab stroke with the default falloff, axis and detail.
    pub fn dab(brush: Brush, centre: Vec3, radius: f32, strength: f32) -> Stroke {
        Stroke {
            brush,
            points: vec![centre],
            radius,
            strength,
            falloff: Falloff::default(),
            height: None,
            axis: None,
            detail: None,
        }
    }

    /// The brush weight at `p`, 0 to 1. A stroke that fails its checks
    /// weighs 0 everywhere.
    pub fn weight_at(&self, p: Vec3) -> f32 {
        Footprint::new(self).map_or(0.0, |fp| fp.weight(p))
    }

    /// The edge length the stroke refines to.
    pub fn detail_length(&self) -> f32 {
        self.detail.unwrap_or(self.radius / 4.0)
    }
}

type Vec2 = [f32; 2];

/// A stroke's checked shape: the axis, a plane basis across it, the path
/// in that plane and the radius.
struct Footprint {
    axis: Vec3,
    u: Vec3,
    v: Vec3,
    path: Vec<Vec2>,
    radius: f32,
    falloff: Falloff,
    /// The path's box in the plane, grown by the radius.
    lo: Vec2,
    hi: Vec2,
}

fn refuse(why: impl Into<String>) -> MeshError {
    MeshError::Stroke(why.into())
}

impl Footprint {
    fn new(s: &Stroke) -> Result<Footprint, MeshError> {
        if s.points.is_empty() {
            return Err(refuse("a stroke needs at least one point"));
        }
        if !s.points.iter().flatten().all(|c| c.is_finite()) {
            return Err(refuse("a stroke point is not finite"));
        }
        if !(s.radius.is_finite() && s.radius > 0.0) {
            return Err(refuse("the radius must be above 0"));
        }
        let axis = math::normalize(s.axis.unwrap_or([0.0, 0.0, 1.0]));
        if axis == [0.0; 3] || !axis.iter().all(|c| c.is_finite()) {
            return Err(refuse("the axis must be a finite direction"));
        }
        let helper = if axis[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        let u = math::normalize(math::cross(axis, helper));
        let v = math::cross(axis, u);
        let flat = |p: Vec3| [math::dot(p, u), math::dot(p, v)];
        let path: Vec<Vec2> = s.points.iter().map(|&p| flat(p)).collect();
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for p in &path {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k] - s.radius);
                hi[k] = hi[k].max(p[k] + s.radius);
            }
        }
        Ok(Footprint { axis, u, v, path, radius: s.radius, falloff: s.falloff, lo, hi })
    }

    fn flat(&self, p: Vec3) -> Vec2 {
        [math::dot(p, self.u), math::dot(p, self.v)]
    }

    fn height(&self, p: Vec3) -> f32 {
        math::dot(p, self.axis)
    }

    /// The path's segments; a one-point path is one zero-length segment.
    fn segments(&self) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
        let n = self.path.len();
        (0..n.saturating_sub(1).max(1)).map(move |i| (self.path[i], self.path[(i + 1).min(n - 1)]))
    }

    /// Distance across the axis from `p` to the path.
    fn distance(&self, p: Vec3) -> f32 {
        let q = self.flat(p);
        self.segments().map(|(a, b)| point_segment(q, a, b)).fold(f32::INFINITY, f32::min)
    }

    fn weight(&self, p: Vec3) -> f32 {
        self.falloff.weight(self.distance(p) / self.radius)
    }

    /// Whether the footprint reaches the polygon `points` (seen down the
    /// axis).
    fn touches(&self, points: &[Vec3]) -> bool {
        let poly: Vec<Vec2> = points.iter().map(|&p| self.flat(p)).collect();
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for p in &poly {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        if (0..2).any(|k| hi[k] < self.lo[k] || lo[k] > self.hi[k]) {
            return false;
        }
        if self.path.iter().any(|&c| inside(&poly, c)) {
            return true;
        }
        let n = poly.len();
        self.segments().any(|(a, b)| {
            (0..n).any(|i| segment_segment(a, b, poly[i], poly[(i + 1) % n]) < self.radius)
        })
    }
}

fn sub2(a: Vec2, b: Vec2) -> Vec2 {
    [a[0] - b[0], a[1] - b[1]]
}

fn cross2(a: Vec2, b: Vec2) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

fn point_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = sub2(b, a);
    let ap = sub2(p, a);
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 > 0.0 { ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let d = [ap[0] - ab[0] * t, ap[1] - ab[1] * t];
    d[0].hypot(d[1])
}

fn segment_segment(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> f32 {
    let (d1, d2) = (cross2(sub2(b, a), sub2(c, a)), cross2(sub2(b, a), sub2(d, a)));
    let (d3, d4) = (cross2(sub2(d, c), sub2(a, c)), cross2(sub2(d, c), sub2(b, c)));
    if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
        return 0.0;
    }
    point_segment(a, c, d).min(point_segment(b, c, d)).min(point_segment(c, a, b)).min(point_segment(d, a, b))
}

/// Even-odd point in polygon.
fn inside(poly: &[Vec2], p: Vec2) -> bool {
    let mut odd = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                odd = !odd;
            }
        }
    }
    odd
}

/// Where a cut classes a vertex.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Side {
    In,
    On,
    Out,
}

impl Mesh {
    /// `MeshOp::Sculpt`. See the module docs.
    pub(crate) fn sculpt(&mut self, s: &Stroke) -> Result<(), MeshError> {
        let fp = Footprint::new(s)?;
        if !(s.strength.is_finite() && s.strength > 0.0) && s.brush != Brush::Cut {
            return Err(refuse("the strength must be above 0"));
        }
        if s.brush.fractional() && s.strength > 1.0 {
            return Err(refuse(format!("{}'s strength is a fraction, 0 to 1", s.brush.name())));
        }
        if s.height.is_some_and(|h| !h.is_finite()) {
            return Err(refuse("the height is not finite"));
        }
        let detail = s.detail_length();
        if !(detail.is_finite() && detail >= s.radius / 64.0) {
            return Err(refuse("the detail must be at least a 64th of the radius"));
        }
        let front_only = s.brush != Brush::Cut;
        self.refine(&fp, detail, front_only)?;
        match s.brush {
            Brush::Cut => self.cut_footprint(&fp),
            _ => self.deform(&fp, s),
        }
    }

    fn faces_along(&self, f: FaceId, axis: Vec3) -> bool {
        math::dot(self.face_normal(f), axis) > 1e-6
    }

    fn edge_length(&self, e: EdgeId) -> f32 {
        let [a, b] = self.edge_verts(e);
        math::dist2(self.position(a), self.position(b)).sqrt()
    }

    /// Step 1: split the long-edged faces the footprint reaches until
    /// none is left or [`MAX_PASSES`] have run.
    fn refine(&mut self, fp: &Footprint, detail: f32, front_only: bool) -> Result<(), MeshError> {
        for _ in 0..MAX_PASSES {
            let faces: Vec<FaceId> = self
                .face_ids()
                .filter(|&f| !front_only || self.faces_along(f, fp.axis))
                .filter(|&f| self.face_edges(f).iter().any(|&e| self.edge_length(e) > detail))
                .filter(|&f| fp.touches(&self.face_positions(f)))
                .collect();
            if faces.is_empty() {
                return Ok(());
            }
            let long: BTreeSet<EdgeId> = faces
                .iter()
                .flat_map(|&f| self.face_edges(f))
                .filter(|&e| self.edge_length(e) > detail)
                .collect();
            let after = self.vert_count() + long.len() + faces.len();
            if after > MAX_VERTS {
                return Err(refuse(format!(
                    "the stroke needs more than {MAX_VERTS} vertices ({after}): use a larger detail or a smaller radius"
                )));
            }
            let mut mids = BTreeSet::new();
            for e in long {
                let [a, _] = self.edge_verts(e);
                mids.extend(self.split_edge(e, a, &[0.5])?);
            }
            for f in faces {
                self.quad_fan(f, &mids)?;
            }
        }
        Ok(())
    }

    /// Replaces face `f` with quads round a new centre vert, each quad a
    /// corner and its two neighbours (a triangle closes an odd count).
    /// The first quad keeps `f`'s id. The walk starts at a vert in `mids`
    /// when the face has one, so a split quad gives four quads.
    fn quad_fan(&mut self, f: FaceId, mids: &BTreeSet<VertId>) -> Result<(), MeshError> {
        let poly = self.face_poly(f);
        let n = poly.len();
        let start = poly.verts.iter().position(|v| mids.contains(v)).unwrap_or(0);
        let at = |k: usize| (start + k) % n;
        let positions: Vec<Vec3> = poly.verts.iter().map(|&v| self.position(v)).collect();
        let skin = blend_skins(&poly.verts.iter().map(|&v| (self.skin(v), 1.0)).collect::<Vec<_>>());
        let centre = self.add_vert(math::centroid(&positions), skin);
        // The running mean of the corners: a new corner (`Corner::lerp`).
        let mut middle: Corner = poly.corners[0].fresh();
        for (k, c) in poly.corners.iter().enumerate().skip(1) {
            middle = middle.lerp(c, 1.0 / (k as f32 + 1.0));
        }
        let piece = |ks: &[usize]| {
            let mut p = Poly {
                verts: vec![centre],
                corners: vec![middle],
                material: poly.material,
                group: poly.group,
                flags: poly.flags | face_flags::LIGHTMAP_STALE,
                src: None,
            };
            for &k in ks {
                p.verts.push(poly.verts[at(k)]);
                p.corners.push(poly.corners[at(k)]);
            }
            p
        };
        let mut pieces = Vec::new();
        for k in 0..n / 2 {
            pieces.push(piece(&[2 * k, 2 * k + 1, 2 * k + 2]));
        }
        if n % 2 == 1 {
            pieces.push(piece(&[n - 1, n]));
        }
        let mut pieces = pieces.into_iter();
        let first = pieces.next().expect("a face has three corners");
        self.rebuild_face(f, &first)?;
        for p in pieces {
            self.add_face(&p)?;
        }
        Ok(())
    }

    /// Step 2 for raise, lower, smooth and flatten.
    fn deform(&mut self, fp: &Footprint, s: &Stroke) -> Result<(), MeshError> {
        let target = s.height.unwrap_or_else(|| fp.height(s.points[0]));
        let mut moves: Vec<(VertId, Vec3)> = Vec::new();
        for v in self.vert_ids().collect::<Vec<_>>() {
            let p = self.position(v);
            let w = fp.weight(p);
            if w <= 0.0 || !self.vert_faces(v).iter().any(|&f| self.faces_along(f, fp.axis)) {
                continue;
            }
            let h = fp.height(p);
            let delta = match s.brush {
                Brush::Raise => s.strength * w,
                Brush::Lower => -s.strength * w,
                Brush::Flatten => (target - h) * s.strength * w,
                Brush::Smooth => {
                    let around = self.vert_neighbors(v);
                    if around.is_empty() {
                        continue;
                    }
                    let mean = around.iter().map(|&n| fp.height(self.position(n))).sum::<f32>() / around.len() as f32;
                    (mean - h) * s.strength * w
                }
                Brush::Cut => unreachable!("cut does not deform"),
            };
            if delta != 0.0 {
                moves.push((v, math::add(p, math::scale(fp.axis, delta))));
            }
        }
        if moves.is_empty() {
            return Err(refuse(format!("the {} stroke moves no vertex", s.brush.name())));
        }
        let mut faces = BTreeSet::new();
        for &(v, to) in &moves {
            self.set_position(v, to);
            for l in self.vert_loops(v) {
                self.l_mut(l).attrs.dirty |= dirty::POSITION;
            }
            faces.extend(self.vert_faces(v));
        }
        for &f in &faces {
            self.f_mut(f).flags |= face_flags::LIGHTMAP_STALE;
        }
        self.refresh_fans(&faces, None);
        Ok(())
    }

    /// Step 3: the cut.
    fn cut_footprint(&mut self, fp: &Footprint) -> Result<(), MeshError> {
        let eps = (fp.radius * 1e-4).max(1e-6);
        let signed = |p: Vec3| fp.distance(p) - fp.radius;
        // Split every edge where it crosses the rim: once where one end is
        // inside, and twice where an edge with neither end inside dips in.
        let mut rim: BTreeSet<VertId> = BTreeSet::new();
        for e in self.edge_ids().collect::<Vec<_>>() {
            let [a, b] = self.edge_verts(e);
            let (pa, pb) = (self.position(a), self.position(b));
            let (da, db) = (signed(pa), signed(pb));
            let at = |t: f32| signed(math::lerp(pa, pb, t));
            // The rim crossing between `lo` and `hi`, whose sides differ.
            let cross = |mut lo: f32, mut hi: f32| {
                let inside_lo = at(lo) < 0.0;
                for _ in 0..40 {
                    let mid = 0.5 * (lo + hi);
                    if (at(mid) < 0.0) == inside_lo {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                0.5 * (lo + hi)
            };
            let ts: Vec<f32> = if (da < -eps && db > eps) || (da > eps && db < -eps) {
                vec![cross(0.0, 1.0)]
            } else if da >= -eps && db >= -eps {
                // The closest approach, sampled then refined.
                let mut t_min = 0.0f32;
                for k in 0..=32 {
                    let t = k as f32 / 32.0;
                    if at(t) < at(t_min) {
                        t_min = t;
                    }
                }
                let step = 1.0 / 32.0;
                let (mut lo, mut hi) = ((t_min - step).max(0.0), (t_min + step).min(1.0));
                for _ in 0..40 {
                    let (m1, m2) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
                    if at(m1) < at(m2) {
                        hi = m2;
                    } else {
                        lo = m1;
                    }
                }
                let t_min = 0.5 * (lo + hi);
                if at(t_min) >= -eps {
                    Vec::new()
                } else {
                    let mut ts = Vec::new();
                    if da > eps {
                        ts.push(cross(0.0, t_min));
                    }
                    if db > eps {
                        ts.push(cross(t_min, 1.0));
                    }
                    ts
                }
            } else {
                Vec::new()
            };
            if !ts.is_empty() {
                rim.extend(self.split_edge(e, a, &ts)?);
            }
        }
        let side = |m: &Mesh, v: VertId| {
            if rim.contains(&v) {
                return Side::On;
            }
            let d = signed(m.position(v));
            if d < -eps {
                Side::In
            } else if d > eps {
                Side::Out
            } else {
                Side::On
            }
        };
        // An edge between corners is inside the footprint when an end is,
        // or, between two rim corners, when its middle is.
        let inside_edge = |m: &Mesh, a: VertId, b: VertId| match (side(m, a), side(m, b)) {
            (Side::In, _) | (_, Side::In) => true,
            (Side::Out, _) | (_, Side::Out) => false,
            _ => signed(math::lerp(m.position(a), m.position(b), 0.5)) < 0.0,
        };
        // Split each face along the rim: its inside edges form chains from
        // rim corner to rim corner; the chains joined by chords are the
        // inside part, and each run of outside edges closed by a chord is
        // an outside part.
        let mut doomed: Vec<FaceId> = Vec::new();
        for f in self.face_ids().collect::<Vec<_>>() {
            let poly = self.face_poly(f);
            let n = poly.len();
            let inside: Vec<bool> =
                (0..n).map(|k| inside_edge(self, poly.verts[k], poly.verts[(k + 1) % n])).collect();
            if inside.iter().all(|&x| x) {
                doomed.push(f);
                continue;
            }
            if !inside.iter().any(|&x| x) {
                continue;
            }
            // Start just after an outside edge, at a chain's first corner.
            let start = (0..n).find(|&k| !inside[k] && inside[(k + 1) % n]).map(|k| (k + 1) % n).expect("both kinds");
            let mut chains: Vec<Vec<usize>> = Vec::new();
            let mut outs: Vec<Vec<usize>> = Vec::new();
            let mut k = 0;
            while k < n {
                let i = (start + k) % n;
                let into = inside[i];
                let mut run = vec![i];
                while k < n && inside[(start + k) % n] == into {
                    k += 1;
                    run.push((start + k) % n);
                }
                if into { chains.push(run) } else { outs.push(run) }
            }
            let part = |corners: Vec<usize>| Poly {
                verts: corners.iter().map(|&c| poly.verts[c]).collect(),
                corners: corners.iter().map(|&c| poly.corners[c]).collect(),
                src: None,
                flags: poly.flags | face_flags::LIGHTMAP_STALE,
                ..poly.clone()
            };
            let mut inner: Vec<usize> = Vec::new();
            for c in &chains {
                for &x in c {
                    if inner.last() != Some(&x) && inner.first() != Some(&x) {
                        inner.push(x);
                    }
                }
            }
            let outer: Vec<Poly> = outs.into_iter().filter(|o| o.len() >= 3).map(part).collect();
            let mut pieces = outer.into_iter();
            let first = pieces.next();
            match first {
                Some(p) => self.rebuild_face(f, &p)?,
                None => {
                    doomed.push(f);
                    continue;
                }
            }
            for p in pieces {
                self.add_face(&p)?;
            }
            if inner.len() >= 3 {
                doomed.push(self.add_face(&part(inner))?);
            }
        }
        if doomed.is_empty() {
            return Err(refuse("the cut reaches no face"));
        }
        let mut edges = BTreeSet::new();
        let mut verts = BTreeSet::new();
        for &f in &doomed {
            edges.extend(self.face_edges(f));
            verts.extend(self.face_verts(f));
        }
        for f in doomed {
            self.kill_face(f);
        }
        for e in edges {
            self.kill_edge_if_wire(e);
        }
        for v in verts {
            self.kill_vert_if_loose(v);
        }
        Ok(())
    }

    /// The first face (lowest t, then lowest face id) a ray meets: the
    /// window's brush cursor. `dir` need not be unit length; `t` is in its
    /// units. Faces are taken as fans from their first corner.
    pub fn raycast(&self, origin: Vec3, dir: Vec3) -> Option<(f32, FaceId)> {
        let mut best: Option<(f32, FaceId)> = None;
        for f in self.face_ids() {
            let p = self.face_positions(f);
            for k in 1..p.len().saturating_sub(1) {
                if let Some(t) = ray_triangle(origin, dir, p[0], p[k], p[k + 1]) {
                    if best.is_none_or(|(b, _)| t < b) {
                        best = Some((t, f));
                    }
                }
            }
        }
        best
    }
}

/// Möller-Trumbore, both sides.
fn ray_triangle(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let (e1, e2) = (math::sub(b, a), math::sub(c, a));
    let p = math::cross(d, e2);
    let det = math::dot(e1, p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = math::sub(o, a);
    let u = math::dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = math::cross(s, e1);
    let v = math::dot(d, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = math::dot(e2, q) * inv;
    (t > 0.0).then_some(t)
}
