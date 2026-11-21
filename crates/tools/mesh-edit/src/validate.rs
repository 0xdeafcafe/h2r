//! `Mesh::validate`: the invariants of docs/editor/modeling.md, checked
//! without trusting any link (it never panics, whatever the arenas hold).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::ids::{EdgeId, FaceId, LoopId, VertId};
use crate::mesh::{Edge, Loop, Mesh, Vert};

impl Mesh {
    fn vert_opt(&self, v: VertId) -> Option<&Vert> {
        self.verts.get(v.index())?.as_ref()
    }

    fn edge_opt(&self, e: EdgeId) -> Option<&Edge> {
        self.edges.get(e.index())?.as_ref()
    }

    fn loop_opt(&self, l: LoopId) -> Option<&Loop> {
        self.loops.get(l.index())?.as_ref()
    }

    /// Checks every invariant; returns each broken one.
    ///
    /// 1. A live edge joins two different live verts; at most one edge
    ///    joins a pair.
    /// 2. A face has at least three corners; its loop cycle closes;
    ///    `next.prev` is the loop itself; every loop names its face;
    ///    consecutive corners' verts are the loop's edge's ends; no vert
    ///    appears twice in a face.
    /// 3. An edge's radial ring holds exactly the loops that name the edge.
    ///    A vert's disk cycle holds exactly its live edges.
    /// 4. No two faces have the same vert cycle in either direction.
    /// 5. Positions and corner values are finite; a normal is unit or zero;
    ///    skin weights sum to 255 or the vert has none.
    /// 6. The selection names live elements only.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        // 1, and the edges each vert should have.
        let mut pairs = HashSet::new();
        let mut incident: BTreeMap<VertId, BTreeSet<EdgeId>> = BTreeMap::new();
        for (i, edge) in self.edges.iter().enumerate() {
            let Some(edge) = edge else { continue };
            let e = EdgeId(i as u32);
            let [a, b] = edge.v;
            if a == b {
                errors.push(format!("{e:?} joins {a:?} to itself"));
            }
            for v in [a, b] {
                if self.vert_opt(v).is_none() {
                    errors.push(format!("{e:?} ends at dead {v:?}"));
                }
                incident.entry(v).or_default().insert(e);
            }
            if !pairs.insert((a.min(b), a.max(b))) {
                errors.push(format!("{e:?} doubles another edge from {a:?} to {b:?}"));
            }
        }

        // 3, disk cycles.
        for (i, vert) in self.verts.iter().enumerate() {
            let Some(vert) = vert else { continue };
            let v = VertId(i as u32);
            let want = incident.remove(&v).unwrap_or_default();
            let mut got = BTreeSet::new();
            if let Some(first) = vert.e {
                let mut e = first;
                for _ in 0..=self.edges.len() {
                    let Some(edge) = self.edge_opt(e) else {
                        errors.push(format!("{v:?}'s disk reaches dead {e:?}"));
                        break;
                    };
                    let end = if edge.v[0] == v {
                        0
                    } else if edge.v[1] == v {
                        1
                    } else {
                        errors.push(format!("{v:?}'s disk holds {e:?}, which does not end at it"));
                        break;
                    };
                    if !got.insert(e) {
                        errors.push(format!("{v:?}'s disk does not close at its start"));
                        break;
                    }
                    let next = edge.disk[end].next;
                    match self.edge_opt(next) {
                        Some(n) => {
                            let back = if n.v[0] == v { n.disk[0].prev } else { n.disk[1].prev };
                            if back != e {
                                errors.push(format!("{v:?}: disk next of {e:?} does not lead back"));
                            }
                        }
                        None => {
                            errors.push(format!("{v:?}'s disk reaches dead {next:?}"));
                            break;
                        }
                    }
                    e = next;
                    if e == first {
                        break;
                    }
                }
            }
            if got != want {
                errors.push(format!("{v:?}'s disk holds {got:?}, its edges are {want:?}"));
            }
            if !vert.co.iter().all(|x| x.is_finite()) {
                errors.push(format!("{v:?} has a non-finite position"));
            }
            if let Some(skin) = vert.skin {
                if skin.total() != 255 {
                    errors.push(format!("{v:?}'s skin weights sum to {}", skin.total()));
                }
            }
        }
        for (v, _) in incident {
            errors.push(format!("an edge ends at {v:?}, which is dead"));
        }

        // 3, radial rings: the loops naming each edge.
        let mut naming: BTreeMap<EdgeId, BTreeSet<LoopId>> = BTreeMap::new();
        for (i, l) in self.loops.iter().enumerate() {
            if let Some(l) = l {
                naming.entry(l.e).or_default().insert(LoopId(i as u32));
            }
        }
        for (i, edge) in self.edges.iter().enumerate() {
            let Some(edge) = edge else { continue };
            let e = EdgeId(i as u32);
            let want = naming.remove(&e).unwrap_or_default();
            let mut got = BTreeSet::new();
            if let Some(first) = edge.l {
                let mut l = first;
                for _ in 0..=self.loops.len() {
                    let Some(lp) = self.loop_opt(l) else {
                        errors.push(format!("{e:?}'s ring reaches dead {l:?}"));
                        break;
                    };
                    if !got.insert(l) {
                        errors.push(format!("{e:?}'s ring does not close at its start"));
                        break;
                    }
                    if self.loop_opt(lp.radial_next).map(|n| n.radial_prev) != Some(l) {
                        errors.push(format!("{e:?}: radial next of {l:?} does not lead back"));
                        break;
                    }
                    l = lp.radial_next;
                    if l == first {
                        break;
                    }
                }
            }
            if got != want {
                errors.push(format!("{e:?}'s ring holds {got:?}, the loops naming it are {want:?}"));
            }
        }
        for (e, _) in naming {
            errors.push(format!("loops name dead {e:?}"));
        }

        // 2 and 4, and 5 for corners.
        let mut owned: BTreeMap<FaceId, usize> = BTreeMap::new();
        for l in self.loops.iter().flatten() {
            *owned.entry(l.f).or_default() += 1;
        }
        let mut cycles = HashSet::new();
        for (i, face) in self.faces.iter().enumerate() {
            let Some(face) = face else { continue };
            let f = FaceId(i as u32);
            let mut verts = Vec::new();
            let mut l = face.l;
            let mut closed = false;
            for _ in 0..=self.loops.len() {
                let Some(lp) = self.loop_opt(l) else {
                    errors.push(format!("{f:?}'s cycle reaches dead {l:?}"));
                    break;
                };
                if lp.f != f {
                    errors.push(format!("{l:?} in {f:?}'s cycle names {:?}", lp.f));
                }
                if !lp.attrs.is_sane() {
                    errors.push(format!("{l:?} has a non-finite value or a non-unit normal"));
                }
                let Some(next) = self.loop_opt(lp.next) else {
                    errors.push(format!("{l:?}'s next is dead"));
                    break;
                };
                if next.prev != l {
                    errors.push(format!("{l:?}: next.prev is {:?}", next.prev));
                }
                match self.edge_opt(lp.e) {
                    Some(edge) => {
                        let ends = [lp.v.min(next.v), lp.v.max(next.v)];
                        if [edge.v[0].min(edge.v[1]), edge.v[0].max(edge.v[1])] != ends {
                            errors.push(format!("{l:?}'s edge {:?} does not join its corners", lp.e));
                        }
                    }
                    None => errors.push(format!("{l:?} names dead {:?}", lp.e)),
                }
                verts.push(lp.v);
                l = lp.next;
                if l == face.l {
                    closed = true;
                    break;
                }
            }
            if !closed {
                errors.push(format!("{f:?}'s cycle does not close"));
                continue;
            }
            if verts.len() < 3 || verts.len() != face.len as usize {
                errors.push(format!("{f:?} has {} corners, records {}", verts.len(), face.len));
            }
            if owned.get(&f).copied().unwrap_or(0) != verts.len() {
                errors.push(format!("{f:?}: loops naming it differ from its cycle"));
            }
            let distinct: BTreeSet<VertId> = verts.iter().copied().collect();
            if distinct.len() != verts.len() {
                errors.push(format!("{f:?} repeats a corner"));
            }
            if !cycles.insert(canonical(&verts)) {
                errors.push(format!("{f:?} has the corners of another face"));
            }
        }

        // 6.
        let sel = &self.select;
        if sel.verts().any(|v| !self.is_vert(v))
            || sel.edges().any(|e| !self.is_edge(e))
            || sel.faces().any(|f| !self.is_face(f))
        {
            errors.push("the selection names a dead element".into());
        }

        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }

    /// Panics with every broken invariant. For tests.
    pub fn assert_valid(&self) {
        if let Err(errors) = self.validate() {
            panic!("mesh invariants broken:\n{}", errors.join("\n"));
        }
    }
}

/// The cycle rotated to start at its least vertex, in whichever direction
/// is smaller: equal for the same cycle either way round.
fn canonical(verts: &[VertId]) -> Vec<VertId> {
    let n = verts.len();
    let Some(start) = (0..n).min_by_key(|&i| verts[i]) else { return Vec::new() };
    let forward: Vec<VertId> = (0..n).map(|k| verts[(start + k) % n]).collect();
    let backward: Vec<VertId> = (0..n).map(|k| verts[(start + n - k) % n]).collect();
    forward.min(backward)
}
