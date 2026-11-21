//! Merge and weld.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::attrs::{Corner, blend_skins, dirty};
use crate::error::MeshError;
use crate::ids::VertId;
use crate::math::{self, Vec3};
use crate::mesh::{Mesh, Poly};
use crate::op::MergeAt;

impl Mesh {
    /// `Merge`: the selected verts become one (V -(n-1)). Faces left with
    /// under three corners go, a face pinched at the merged vert splits
    /// there, and a face doubling another goes. At each collapsed corner,
    /// the kept vert's corner attributes win.
    pub(crate) fn merge(&mut self, at: MergeAt) -> Result<(), MeshError> {
        let verts = self.selected_verts();
        let (Some(&first), Some(&last)) = (verts.first(), verts.last()) else {
            return Err(MeshError::NothingSelected);
        };
        let target = if at == MergeAt::Last { last } else { first };
        match at {
            MergeAt::Center => {
                let points: Vec<Vec3> = verts.iter().map(|&v| self.position(v)).collect();
                let w = 1.0 / verts.len() as f32;
                let skins: Vec<_> = verts.iter().map(|&v| (self.skin(v), w)).collect();
                self.set_position(target, math::centroid(&points));
                self.v_mut(target).skin = blend_skins(&skins);
            }
            MergeAt::Cursor(p) => self.set_position(target, p),
            MergeAt::First | MergeAt::Last => {}
        }
        for l in self.vert_loops(target) {
            self.l_mut(l).attrs.dirty |= dirty::POSITION | dirty::SKIN;
        }
        let map: BTreeMap<VertId, VertId> =
            verts.iter().copied().filter(|&v| v != target).map(|v| (v, target)).collect();
        self.weld_in(&map)?;
        self.select_result(&[target], &[], &[]);
        Ok(())
    }

    /// `Weld`: selected verts within `distance` of each other
    /// (transitively), clustered by a grid hash and merged at each
    /// cluster's lowest id, which stays where it is.
    pub(crate) fn weld(&mut self, distance: f32) -> Result<(), MeshError> {
        let verts = self.selected_verts();
        if verts.is_empty() {
            return Err(MeshError::NothingSelected);
        }
        let points: Vec<Vec3> = verts.iter().map(|&v| self.position(v)).collect();
        let roots = cluster(&points, distance);
        let mut map = BTreeMap::new();
        let mut kept = BTreeSet::new();
        for (i, &v) in verts.iter().enumerate() {
            let root = verts[roots[i]];
            kept.insert(root);
            if root != v {
                map.insert(v, root);
            }
        }
        self.weld_in(&map)?;
        let kept: Vec<VertId> = kept.into_iter().collect();
        self.select_result(&kept, &[], &[]);
        Ok(())
    }

    /// Replaces each key of `map` by its value (values are not keys) in
    /// every face and edge, then removes the keys.
    pub(crate) fn weld_in(&mut self, map: &BTreeMap<VertId, VertId>) -> Result<(), MeshError> {
        if map.is_empty() {
            return Ok(());
        }
        let remap = |v: VertId| *map.get(&v).unwrap_or(&v);
        let mut faces = BTreeSet::new();
        let mut wires = Vec::new();
        for &v in map.keys() {
            faces.extend(self.vert_faces(v));
            for e in self.vert_edges(v) {
                if self.edge_face_count(e) == 0 {
                    let [a, b] = self.edge_verts(e);
                    wires.push((remap(a), remap(b)));
                }
            }
        }
        // Each touched face keeps its id as its first piece; further pieces
        // are new faces. A piece doubling a face goes.
        for f in faces {
            let poly = self.face_poly(f);
            let mapped: Vec<VertId> = poly.verts.iter().map(|&v| remap(v)).collect();
            let pieces: Vec<Poly> = simple_loops(&poly.verts, &mapped, &poly.corners)
                .into_iter()
                .filter(|(verts, _)| verts.len() >= 3)
                .map(|(verts, corners)| Poly { verts, corners, src: None, ..poly.clone() })
                .collect();
            let mut pieces = pieces.into_iter();
            match pieces.next() {
                None => self.kill_face(f),
                Some(first) => match self.rebuild_face(f, &first) {
                    Ok(()) => {}
                    Err(MeshError::DuplicateFace) => self.kill_face(f),
                    Err(err) => return Err(err),
                },
            }
            for piece in pieces {
                match self.add_face(&piece) {
                    Ok(_) | Err(MeshError::DuplicateFace) => {}
                    Err(err) => return Err(err),
                }
            }
        }
        for &v in map.keys() {
            self.kill_vert(v);
        }
        for (a, b) in wires {
            if a != b {
                self.add_edge(a, b)?;
            }
        }
        Ok(())
    }
}

/// Splits a remapped face cycle into simple cycles. Runs of one vert
/// collapse to one corner: the one whose vert was not remapped (the kept
/// vert's) if there is one, else the first. A cycle passing a vert twice
/// is cut there in two.
fn simple_loops(
    original: &[VertId],
    mapped: &[VertId],
    corners: &[Corner],
) -> Vec<(Vec<VertId>, Vec<Corner>)> {
    let n = mapped.len();
    // Start at a run boundary so no run wraps round the end.
    let start = (0..n).find(|&i| mapped[i] != mapped[(i + n - 1) % n]).unwrap_or(0);
    let mut verts: Vec<VertId> = Vec::new();
    let mut kept: Vec<Corner> = Vec::new();
    let mut kept_is_own: Vec<bool> = Vec::new();
    for k in 0..n {
        let i = (start + k) % n;
        let own = original[i] == mapped[i];
        if verts.last() == Some(&mapped[i]) {
            let last = kept.len() - 1;
            if own && !kept_is_own[last] {
                kept[last] = corners[i];
                kept_is_own[last] = true;
            }
            continue;
        }
        verts.push(mapped[i]);
        kept.push(corners[i]);
        kept_is_own.push(own);
    }
    let mut out = Vec::new();
    let mut stack = vec![(verts, kept)];
    while let Some((v, c)) = stack.pop() {
        let mut split = None;
        'find: for i in 0..v.len() {
            for j in i + 1..v.len() {
                if v[i] == v[j] {
                    split = Some((i, j));
                    break 'find;
                }
            }
        }
        match split {
            None => out.push((v, c)),
            Some((i, j)) => {
                stack.push((v[i..j].to_vec(), c[i..j].to_vec()));
                let mut rest_v = v[j..].to_vec();
                rest_v.extend_from_slice(&v[..i]);
                let mut rest_c = c[j..].to_vec();
                rest_c.extend_from_slice(&c[..i]);
                stack.push((rest_v, rest_c));
            }
        }
    }
    out
}

/// Groups points within `distance` of each other (transitively) with a
/// grid hash. Returns, per point, the index of its group's first point.
pub(crate) fn cluster(points: &[Vec3], distance: f32) -> Vec<usize> {
    let n = points.len();
    let d = distance.max(0.0);
    let d2 = d * d;
    let cell = d.max(1e-6);
    let key = |p: Vec3| {
        [
            (p[0] / cell).floor() as i64,
            (p[1] / cell).floor() as i64,
            (p[2] / cell).floor() as i64,
        ]
    };
    let mut grid: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    for (i, &p) in points.iter().enumerate() {
        grid.entry(key(p)).or_default().push(i);
    }
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut parent: Vec<usize> = (0..n).collect();
    for (i, &p) in points.iter().enumerate() {
        let k = key(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = grid.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else { continue };
                    for &j in list {
                        if j <= i || math::dist2(p, points[j]) > d2 {
                            continue;
                        }
                        let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                        if ri != rj {
                            // The smaller index stays the root: a group's
                            // root is its first point.
                            parent[ri.max(rj)] = ri.min(rj);
                        }
                    }
                }
            }
        }
    }
    (0..n).map(|i| find(&mut parent, i)).collect()
}
