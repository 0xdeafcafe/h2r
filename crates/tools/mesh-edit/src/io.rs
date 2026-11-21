//! Import from indexed triangles or polygons, and stable export.
//!
//! Export is stable: a face that is still its source triangle with its
//! corners still mapping to their source vertices comes out as that
//! triangle, in its place; a corner comes out as its source vertex while
//! every corner claiming that vertex agrees on its attributes. So a mesh
//! with no topology change exports the source's vertex and index order
//! with only the edited values changed, and an unedited mesh exports the
//! source exactly. New and changed faces follow, per group, triangulated,
//! with a new vertex per distinct (vert, attributes) pair.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use crate::attrs::{Corner, Skin, dirty};
use crate::error::MeshError;
use crate::ids::{FaceId, VertId};
use crate::math::{self, Vec3};
use crate::mesh::{Mesh, Poly};
use crate::triangulate::triangulate;

/// An indexed triangle list with per-vertex attributes: what a writer
/// reads out of a section. `skins`, `groups` and `materials` are empty
/// (absent) or as long as the vertices, triangles and triangles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Source {
    pub positions: Vec<Vec3>,
    /// Per vertex; `src` and `dirty` are ignored on import.
    pub corners: Vec<Corner>,
    pub skins: Vec<Option<Skin>>,
    pub triangles: Vec<[u32; 3]>,
    /// The part each triangle belongs to.
    pub groups: Vec<u16>,
    pub materials: Vec<u16>,
}

impl Source {
    fn group(&self, t: usize) -> u16 {
        self.groups.get(t).copied().unwrap_or(0)
    }

    fn material(&self, t: usize) -> u16 {
        self.materials.get(t).copied().unwrap_or(0)
    }

    fn skin(&self, i: usize) -> Option<Skin> {
        self.skins.get(i).copied().flatten()
    }
}

/// What a mesh exports against: its source, and the source triangles it
/// could not hold as faces (a repeated corner, or the corners of another
/// triangle). Those pass through export unchanged, in place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Imported {
    pub(crate) source: Source,
    pub(crate) passthrough: BTreeSet<u32>,
}

/// `Mesh::export`'s output, ready for a writer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Export {
    pub positions: Vec<Vec3>,
    /// Per vertex: the attributes, `src` (the source vertex it is, if any)
    /// and `dirty` (what the writer must re-encode).
    pub corners: Vec<Corner>,
    pub skins: Vec<Option<Skin>>,
    pub triangles: Vec<[u32; 3]>,
    /// Per triangle.
    pub groups: Vec<u16>,
    /// Per triangle.
    pub materials: Vec<u16>,
    /// Per vertex: the mesh vertex it was made from (`None` for a source
    /// vertex no corner uses any more, written as it was).
    pub vert_ids: Vec<Option<VertId>>,
}

/// What `from_triangles` did with its input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Source vertices merged into an earlier one at the bit-same position
    /// with the same skin.
    pub welded: usize,
    /// Source triangles kept as pass-through instead of faces.
    pub passthrough: usize,
}

impl Mesh {
    /// A mesh from a section's triangles. Vertices with bit-equal
    /// positions and equal skins become one `Vert`; every corner keeps its
    /// source vertex's attributes and index. A triangle with a repeated
    /// corner, or the corners of an earlier one, is kept for export but
    /// is not a face.
    pub fn from_triangles(source: Source) -> Result<(Mesh, ImportReport), MeshError> {
        let n = source.positions.len();
        if source.corners.len() != n || (!source.skins.is_empty() && source.skins.len() != n) {
            return Err(MeshError::BadInput("attribute arrays differ in length from positions".into()));
        }
        let t = source.triangles.len();
        if (!source.groups.is_empty() && source.groups.len() != t)
            || (!source.materials.is_empty() && source.materials.len() != t)
        {
            return Err(MeshError::BadInput("groups or materials differ in length from triangles".into()));
        }
        if let Some(bad) = source.triangles.iter().flatten().find(|&&i| i as usize >= n) {
            return Err(MeshError::BadInput(format!("triangle index {bad} past {n} vertices")));
        }
        let mut mesh = Mesh::new();
        let mut report = ImportReport::default();
        let mut welded: HashMap<([u32; 3], Option<Skin>), VertId> = HashMap::new();
        let mut ids = Vec::with_capacity(n);
        for i in 0..n {
            let p = source.positions[i];
            let key = (p.map(f32::to_bits), source.skin(i));
            let id = match welded.get(&key) {
                Some(&v) => {
                    report.welded += 1;
                    v
                }
                None => {
                    let v = mesh.add_vert(p, source.skin(i));
                    welded.insert(key, v);
                    v
                }
            };
            ids.push(id);
        }
        let mut passthrough = BTreeSet::new();
        for (ti, tri) in source.triangles.iter().enumerate() {
            let corners: Vec<Corner> = tri
                .iter()
                .map(|&i| Corner { src: Some(i), dirty: 0, ..source.corners[i as usize] })
                .collect();
            let poly = Poly {
                verts: tri.iter().map(|&i| ids[i as usize]).collect(),
                corners,
                material: source.material(ti),
                group: source.group(ti),
                flags: 0,
                src: Some(ti as u32),
            };
            if mesh.add_face(&poly).is_err() {
                passthrough.insert(ti as u32);
            }
        }
        report.passthrough = passthrough.len();
        mesh.imported = Some(Arc::new(Imported { source, passthrough }));
        Ok((mesh, report))
    }

    /// A mesh from positions and polygons (collision surfaces): default
    /// corners, material and group 0, no source to export against.
    pub fn from_polygons(positions: &[Vec3], polys: &[Vec<u32>]) -> Result<Mesh, MeshError> {
        let mut mesh = Mesh::new();
        let ids: Vec<VertId> = positions.iter().map(|&p| mesh.add_vert(p, None)).collect();
        for poly in polys {
            let verts = poly
                .iter()
                .map(|&i| {
                    ids.get(i as usize)
                        .copied()
                        .ok_or_else(|| MeshError::BadInput(format!("polygon index {i} out of range")))
                })
                .collect::<Result<Vec<_>, _>>()?;
            mesh.add_face(&Poly::new(verts))?;
        }
        Ok(mesh)
    }

    /// The source this mesh was imported from.
    pub fn source(&self) -> Option<&Source> {
        self.imported.as_deref().map(|i| &i.source)
    }

    /// Vertices, triangles and groups for a writer (see the module docs).
    pub fn export(&self) -> Export {
        let empty = Imported::default();
        let imported = self.imported.as_deref().unwrap_or(&empty);
        let source = &imported.source;
        let n_src = source.positions.len();
        let mut out = Export {
            positions: source.positions.clone(),
            corners: (0..n_src as u32)
                .map(|i| Corner { src: Some(i), dirty: 0, ..source.corners[i as usize] })
                .collect(),
            skins: (0..n_src).map(|i| source.skin(i)).collect(),
            vert_ids: vec![None; n_src],
            ..Export::default()
        };

        // Each live loop's output vertex. A source vertex goes to the first
        // loop (by id) that claims it; a later claimant with another vert
        // or other attributes gets a new vertex. Keys are taken before
        // tangent frames are recomputed, so corners that agree share a
        // vertex whatever faces they are on; the vertex's frame is its
        // first corner's.
        let mut claims: HashMap<u32, (VertId, [u32; 16])> = HashMap::new();
        let mut made: HashMap<(VertId, [u32; 16]), u32> = HashMap::new();
        let mut index_of = vec![u32::MAX; self.loops.len()];
        for l in self.loop_ids() {
            let lp = self.l(l);
            let key = lp.attrs.key();
            let mut attrs = lp.attrs;
            if attrs.dirty & dirty::FRAME != 0 {
                (attrs.tangent, attrs.binormal) = self.frame(l);
            }
            let claim = attrs.src.filter(|&s| (s as usize) < n_src);
            let index = match claim {
                Some(s) => match claims.get(&s).copied() {
                    None => {
                        claims.insert(s, (lp.v, key));
                        let i = s as usize;
                        let mut dirt = attrs.dirty;
                        if self.position(lp.v).map(f32::to_bits) != source.positions[i].map(f32::to_bits) {
                            dirt |= dirty::POSITION;
                        }
                        if self.skin(lp.v) != source.skin(i) {
                            dirt |= dirty::SKIN;
                        }
                        out.positions[i] = self.position(lp.v);
                        out.corners[i] = Corner { src: Some(s), dirty: dirt, ..attrs };
                        out.skins[i] = self.skin(lp.v);
                        out.vert_ids[i] = Some(lp.v);
                        Some(s)
                    }
                    Some((v, k)) if v == lp.v && k == key => {
                        out.corners[s as usize].dirty |= attrs.dirty;
                        Some(s)
                    }
                    Some(_) => None,
                },
                None => None,
            };
            index_of[l.index()] = match index {
                Some(s) => s,
                None => *made.entry((lp.v, key)).or_insert_with(|| {
                    let i = out.positions.len() as u32;
                    out.positions.push(self.position(lp.v));
                    out.corners.push(Corner { src: None, dirty: dirty::ALL, ..attrs });
                    out.skins.push(self.skin(lp.v));
                    out.vert_ids.push(Some(lp.v));
                    i
                }),
            };
        }

        // Faces still exactly their source triangle.
        let mut kept: BTreeMap<u32, FaceId> = BTreeMap::new();
        for f in self.face_ids() {
            let Some(t) = self.f(f).src else { continue };
            let Some(tri) = source.triangles.get(t as usize) else { continue };
            let loops = self.face_loops(f);
            if loops.len() == 3 && (0..3).all(|k| index_of[loops[k].index()] == tri[k]) {
                kept.insert(t, f);
            }
        }

        // Groups in order of first appearance in the source, then new ones.
        let mut order: Vec<u16> = Vec::new();
        for t in 0..source.triangles.len() {
            let g = source.group(t);
            if !order.contains(&g) {
                order.push(g);
            }
        }
        let mut changed: BTreeMap<u16, Vec<FaceId>> = BTreeMap::new();
        for f in self.face_ids() {
            if self.f(f).src.is_some_and(|t| kept.get(&t) == Some(&f)) {
                continue;
            }
            changed.entry(self.f(f).group).or_default().push(f);
        }
        for &g in changed.keys() {
            if !order.contains(&g) {
                order.push(g);
            }
        }

        for g in order {
            for (t, tri) in source.triangles.iter().enumerate() {
                if source.group(t) != g {
                    continue;
                }
                let t32 = t as u32;
                if imported.passthrough.contains(&t32) {
                    out.triangles.push(*tri);
                    out.groups.push(g);
                    out.materials.push(source.material(t));
                } else if let Some(&f) = kept.get(&t32) {
                    out.triangles.push(*tri);
                    out.groups.push(g);
                    out.materials.push(self.f(f).material);
                }
            }
            for &f in changed.get(&g).map(Vec::as_slice).unwrap_or(&[]) {
                let loops = self.face_loops(f);
                let points: Vec<Vec3> = loops.iter().map(|&l| self.position(self.l(l).v)).collect();
                for [a, b, c] in triangulate(&points) {
                    out.triangles.push([
                        index_of[loops[a].index()],
                        index_of[loops[b].index()],
                        index_of[loops[c].index()],
                    ]);
                    out.groups.push(g);
                    out.materials.push(self.f(f).material);
                }
            }
        }
        out
    }

    /// The tangent and binormal at corner `l`: the face's texture-space
    /// axes, made orthogonal to the corner's normal.
    fn frame(&self, l: crate::ids::LoopId) -> (Vec3, Vec3) {
        let lp = self.l(l);
        let (prev, next) = (self.l(lp.prev), self.l(lp.next));
        let p0 = self.position(lp.v);
        let e1 = math::sub(self.position(next.v), p0);
        let e2 = math::sub(self.position(prev.v), p0);
        let d1 = [next.attrs.uv[0] - lp.attrs.uv[0], next.attrs.uv[1] - lp.attrs.uv[1]];
        let d2 = [prev.attrs.uv[0] - lp.attrs.uv[0], prev.attrs.uv[1] - lp.attrs.uv[1]];
        let det = d1[0] * d2[1] - d2[0] * d1[1];
        let n = lp.attrs.normal;
        if det.abs() < 1e-12 || n == [0.0; 3] {
            return (lp.attrs.tangent, lp.attrs.binormal);
        }
        let r = 1.0 / det;
        let t = math::scale(math::sub(math::scale(e1, d2[1]), math::scale(e2, d1[1])), r);
        let b = math::scale(math::sub(math::scale(e2, d1[0]), math::scale(e1, d2[0])), r);
        let tangent = math::normalize(math::sub(t, math::scale(n, math::dot(n, t))));
        let side = if math::dot(math::cross(n, tangent), b) < 0.0 { -1.0 } else { 1.0 };
        (tangent, math::scale(math::cross(n, tangent), side))
    }
}
