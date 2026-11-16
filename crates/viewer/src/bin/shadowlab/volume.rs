//! Shadow volumes from welded caster meshes.
//!
//! The volume is built from the triangles that face the light (lit faces).
//! Each lit face sweeps a prism away from the light: the face itself as the
//! near cap, its copy at infinity (reversed) as the far cap, and one quad per
//! edge as the sides. Where two lit faces share an edge their side quads have
//! opposite orientations and cancel, so a side is emitted only for edges whose
//! lit uses do not balance: on a closed mesh exactly the silhouette, and on an
//! open mesh the silhouette plus the open border edges of lit faces. The
//! result is a closed volume whatever the caster's topology, which is what
//! depth-fail (Carmack's reverse) needs; the cost of open geometry shows up
//! as extra side quads (`open_sides`), which is the fill that open meshes add.
//!
//! Vertex `i` of the mesh is volume vertex `i` (`w = 1`); vertex `n + i` is
//! the same point pushed to infinity away from the light (`w = 0`, the
//! extrusion direction in `xyz`). The vertex shader may also do that push
//! itself; the CPU form here is what the tests reason about.
use crate::geom::{Topology, TriMesh, V3, dot, normalize, scale, sub};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Light {
    /// A light at infinity; `dir` is the direction its light travels.
    Directional { dir: V3 },
    /// A point light.
    Point { pos: V3 },
}

impl Light {
    /// The direction from `p` towards the light (not normalised for points).
    pub fn to_light(self, p: V3) -> V3 {
        match self {
            Light::Directional { dir } => scale(dir, -1.0),
            Light::Point { pos } => sub(pos, p),
        }
    }
    /// The direction a shadow is cast from `p`.
    pub fn extrude(self, p: V3) -> V3 {
        match self {
            Light::Directional { dir } => normalize(dir),
            Light::Point { pos } => normalize(sub(p, pos)),
        }
    }
}

/// Which triangles face the light. A face exactly edge-on is unlit.
pub fn lit_faces(m: &TriMesh, light: &Light) -> Vec<bool> {
    (0..m.tris.len())
        .map(|t| dot(m.face_normal(t), light.to_light(m.corners(t)[0])) > 0.0)
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VolumeStats {
    pub lit_faces: usize,
    /// Edges that got at least one side quad.
    pub silhouette_edges: usize,
    /// Side quads (an edge whose lit uses are off by two gets two).
    pub sides: usize,
    /// Side quads on edges with a single use: open borders of the caster.
    pub open_sides: usize,
    /// Cap triangles (near plus far).
    pub caps: usize,
}

/// A shadow volume in homogeneous coordinates, as an indexed triangle list.
pub struct Volume {
    pub verts: Vec<[f32; 4]>,
    pub tris: Vec<[u32; 3]>,
    pub stats: VolumeStats,
}

impl Volume {
    /// `caps: false` leaves out both caps (a depth-pass, sides-only volume),
    /// which is only right while the camera is outside every volume.
    pub fn build(m: &TriMesh, topo: &Topology, light: &Light, caps: bool) -> Self {
        let n = m.positions.len() as u32;
        let mut verts: Vec<[f32; 4]> = m
            .positions
            .iter()
            .map(|p| [p[0], p[1], p[2], 1.0])
            .collect();
        verts.extend(m.positions.iter().map(|&p| {
            let d = light.extrude(p);
            [d[0], d[1], d[2], 0.0]
        }));
        let lit = lit_faces(m, light);
        let mut stats = VolumeStats {
            lit_faces: lit.iter().filter(|&&l| l).count(),
            ..Default::default()
        };
        let mut tris = Vec::new();
        for e in &topo.edges {
            let uses = topo.uses_of(e);
            let net: i32 = uses
                .iter()
                .filter(|u| lit[u.face as usize])
                .map(|u| if u.forward { 1 } else { -1 })
                .sum();
            if net == 0 {
                continue;
            }
            // Walk the edge the way the unbalanced lit faces do.
            let (a, b) = if net > 0 { (e.a, e.b) } else { (e.b, e.a) };
            for _ in 0..net.unsigned_abs() {
                tris.push([a, n + a, b]);
                tris.push([b, n + a, n + b]);
            }
            stats.silhouette_edges += 1;
            stats.sides += net.unsigned_abs() as usize;
            if uses.len() == 1 {
                stats.open_sides += 1;
            }
        }
        if caps {
            for (t, tri) in m.tris.iter().enumerate() {
                if lit[t] {
                    tris.push(*tri);
                    tris.push([n + tri[2], n + tri[1], n + tri[0]]);
                    stats.caps += 2;
                }
            }
        }
        Self { verts, tris, stats }
    }

    #[cfg(test)]
    /// Replace each point at infinity by its point at distance `far` from the
    /// finite vertex it came from, for CPU reasoning about the volume.
    pub fn finite(&self, far: f32) -> Vec<[V3; 3]> {
        let half = self.verts.len() / 2;
        let at = |i: u32| -> V3 {
            let v = self.verts[i as usize];
            if v[3] != 0.0 {
                return [v[0], v[1], v[2]];
            }
            let base = self.verts[i as usize - half];
            crate::geom::add([base[0], base[1], base[2]], scale([v[0], v[1], v[2]], far))
        };
        self.tris.iter().map(|t| t.map(at)).collect()
    }

    /// Flatten for the GPU (non-indexed `vec4` positions).
    pub fn expanded(&self) -> Vec<[f32; 4]> {
        self.tris
            .iter()
            .flat_map(|t| t.map(|i| self.verts[i as usize]))
            .collect()
    }
}

#[cfg(test)]
/// Every directed edge of the triangle list matched by its reverse: the
/// combinatorial test that a volume is closed and consistently wound.
/// Vertices are compared by index, so points at infinity count as distinct.
pub fn is_closed(tris: &[[u32; 3]]) -> bool {
    use std::collections::HashMap;
    let mut count: HashMap<(u32, u32), i32> = HashMap::new();
    for t in tris {
        for k in 0..3 {
            let (p, q) = (t[k], t[(k + 1) % 3]);
            *count.entry((p.min(q), p.max(q))).or_default() += if p < q { 1 } else { -1 };
        }
    }
    count.values().all(|&c| c == 0)
}

#[cfg(test)]
fn ray_triangle(o: V3, d: V3, t: &[V3; 3]) -> Option<(f32, f32)> {
    // Moller-Trumbore in f64; returns (distance along d, facing sign).
    let f = |v: V3| v.map(f64::from);
    let (o, d) = (f(o), f(d));
    let [a, b, c] = t.map(f);
    let sub = |x: [f64; 3], y: [f64; 3]| [x[0] - y[0], x[1] - y[1], x[2] - y[2]];
    let cross = |x: [f64; 3], y: [f64; 3]| {
        [
            x[1] * y[2] - x[2] * y[1],
            x[2] * y[0] - x[0] * y[2],
            x[0] * y[1] - x[1] * y[0],
        ]
    };
    let dot = |x: [f64; 3], y: [f64; 3]| x[0] * y[0] + x[1] * y[1] + x[2] * y[2];
    let (e1, e2) = (sub(b, a), sub(c, a));
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-12 {
        return None;
    }
    let s = sub(o, a);
    let u = dot(s, p) / det;
    let q = cross(s, e1);
    let v = dot(d, q) / det;
    if !(0.0..=1.0).contains(&u) || v < 0.0 || u + v > 1.0 {
        return None;
    }
    let dist = dot(e2, q) / det;
    let facing = dot(cross(e1, e2), d);
    Some((dist as f32, if facing < 0.0 { 1.0 } else { -1.0 }))
}

#[cfg(test)]
/// The stencil count depth-fail leaves at a surface point `q` seen from
/// `eye`: back faces behind `q` add one, front faces behind it take one away.
/// Nonzero means `q` is in shadow.
pub fn depth_fail_count(volume: &[[V3; 3]], eye: V3, q: V3) -> i32 {
    let d = sub(q, eye);
    let mut n = 0;
    for t in volume {
        if let Some((dist, front)) = ray_triangle(eye, d, t)
            && dist > 1.0
        {
            n += if front > 0.0 { -1 } else { 1 };
        }
    }
    n
}

#[cfg(test)]
/// The count depth-pass leaves: front faces in front of `q` add one, back
/// faces take one away. Wrong whenever the eye is inside a volume.
pub fn depth_pass_count(volume: &[[V3; 3]], eye: V3, q: V3) -> i32 {
    let d = sub(q, eye);
    let mut n = 0;
    for t in volume {
        if let Some((dist, front)) = ray_triangle(eye, d, t)
            && dist > 0.0
            && dist < 1.0
        {
            n += if front > 0.0 { 1 } else { -1 };
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::WELD_EPSILON;
    use crate::geom::shapes::*;

    fn mesh(soup: &[[V3; 3]]) -> (TriMesh, Topology) {
        let m = TriMesh::from_soup(soup, WELD_EPSILON);
        let t = Topology::build(&m);
        (m, t)
    }
    const SUN: Light = Light::Directional {
        dir: [0.0, 0.0, -1.0],
    };

    #[test]
    fn cube_under_a_vertical_sun_has_a_four_edge_silhouette() {
        let (m, t) = mesh(&cube([0.0; 3], [1.0; 3]));
        let v = Volume::build(&m, &t, &SUN, true);
        // The top two triangles are lit; the silhouette is the top rim.
        assert_eq!(v.stats.lit_faces, 2);
        assert_eq!(v.stats.silhouette_edges, 4);
        assert_eq!((v.stats.sides, v.stats.open_sides, v.stats.caps), (4, 0, 4));
        assert_eq!(v.tris.len(), 4 * 2 + 4);
        assert!(is_closed(&v.tris));
        // The silhouette edges are exactly the four rim edges at z = 1.
        let rim = t
            .edges
            .iter()
            .filter(|e| {
                let lit = lit_faces(&m, &SUN);
                let us = t.uses_of(e);
                us.iter().filter(|u| lit[u.face as usize]).count() == 1
            })
            .count();
        assert_eq!(rim, 4);
    }

    #[test]
    fn diagonal_sun_and_point_light_give_closed_volumes() {
        let (m, t) = mesh(&cube([0.0; 3], [1.0; 3]));
        for light in [
            Light::Directional {
                dir: normalize([1.0, 0.5, -1.0]),
            },
            Light::Point {
                pos: [3.0, 2.0, 4.0],
            },
            Light::Point {
                pos: [0.5, 0.5, 5.0],
            },
        ] {
            let v = Volume::build(&m, &t, &light, true);
            assert!(is_closed(&v.tris), "{light:?}");
            assert_eq!(v.stats.open_sides, 0);
            // A convex caster's silhouette is one loop: as many edges as
            // vertices on it, and at least three.
            assert!(v.stats.silhouette_edges >= 3);
        }
    }

    #[test]
    fn open_box_volume_is_closed_with_open_sides_counted() {
        // Lid off, light from the -x side: only the -x wall faces it, and
        // that wall's top edge is the open rim.
        let (m, t) = mesh(&open_box([0.0; 3], [1.0; 3]));
        assert_eq!(t.stats.open_edges, 4);
        let side = Light::Directional {
            dir: normalize([1.0, 0.0, -0.2]),
        };
        let v = Volume::build(&m, &t, &side, true);
        assert!(is_closed(&v.tris));
        assert_eq!(v.stats.open_sides, 1, "{:?}", v.stats);
        // A closed cube under the same light needs no open sides.
        let (c, ct) = mesh(&cube([0.0; 3], [1.0; 3]));
        assert_eq!(Volume::build(&c, &ct, &side, true).stats.open_sides, 0);
    }

    #[test]
    fn plane_with_hole_volume_is_closed_and_counts_every_lit_border() {
        let (m, t) = mesh(&plane(2.0, 1.0, 4, Some((1, 2))));
        let v = Volume::build(&m, &t, &SUN, true);
        assert!(is_closed(&v.tris));
        // Every face is lit and every side comes from an open edge.
        assert_eq!(v.stats.lit_faces, m.tris.len());
        assert_eq!(v.stats.open_sides, 16 + 8);
        assert_eq!(v.stats.sides, 16 + 8);
    }

    #[test]
    fn depth_fail_matches_shadowing_with_the_eye_outside_and_inside() {
        // A cube floating above the ground, sun straight down.
        let (m, t) = mesh(&cube([-0.5, -0.5, 1.0], [0.5, 0.5, 2.0]));
        let v = Volume::build(&m, &t, &SUN, true);
        let fin = v.finite(1000.0);
        let under = [0.1, 0.1, 0.0];
        let beside = [2.0, 0.3, 0.0];
        for eye in [[5.0, -4.0, 6.0], [0.0, 0.0, 0.5], [0.2, -0.1, 0.9]] {
            assert_ne!(depth_fail_count(&fin, eye, under), 0, "eye {eye:?}");
            assert_eq!(depth_fail_count(&fin, eye, beside), 0, "eye {eye:?}");
        }
    }

    #[test]
    fn depth_pass_breaks_inside_the_volume_and_depth_fail_needs_caps() {
        let (m, t) = mesh(&cube([-0.5, -0.5, 1.0], [0.5, 0.5, 2.0]));
        // A short extrusion, so a steep ray from inside can leave through the
        // far end; at infinity that is where a point light's diverging
        // volume is left by most rays.
        let capped = Volume::build(&m, &t, &SUN, true).finite(10.0);
        let uncapped = Volume::build(&m, &t, &SUN, false).finite(10.0);
        let under = [0.02, 0.01, 0.0];
        let outside = [5.0, -4.0, 6.0];
        let inside = [0.0, 0.0, 0.5]; // in the shadow, under the cube
        // Depth-pass is right from outside, wrong from inside.
        assert_ne!(depth_pass_count(&uncapped, outside, under), 0);
        assert_eq!(depth_pass_count(&uncapped, inside, under), 0);
        // Depth-fail without its caps loses the shadow: the ray leaves
        // through the missing far cap.
        assert_eq!(depth_fail_count(&uncapped, inside, under), 0);
        assert_ne!(depth_fail_count(&capped, inside, under), 0);
    }

    #[test]
    fn open_caster_still_shadows_through_its_lit_faces_only() {
        // A plane with a hole, sun straight down: the hole lets light through.
        let (m, t) = mesh(&plane(2.0, 1.0, 4, Some((1, 2))));
        let fin = Volume::build(&m, &t, &SUN, true).finite(1000.0);
        let eye = [6.0, -5.0, 8.0];
        assert_ne!(depth_fail_count(&fin, eye, [-1.7, -1.7, 0.0]), 0);
        assert_eq!(depth_fail_count(&fin, eye, [0.1, 0.2, 0.0]), 0);
        assert_eq!(depth_fail_count(&fin, eye, [3.0, 0.0, 0.0]), 0);
    }
}
