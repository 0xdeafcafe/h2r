//! `MeshOp`: the operations as values, and `Mesh::apply`.
//!
//! An op is what the op log stores, what `doc.diff` prints and what an
//! agent sends. Ops act on the selection (set it with the `Select*` ops
//! first), and leave the result selected. Applying the same ops to the same
//! mesh gives the same mesh, ids included.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::error::MeshError;
use crate::ids::{EdgeId, FaceId, VertId};
use crate::math::{self, Vec3};
use crate::mesh::Mesh;
use crate::ops::Affine;
use crate::select::{SelectHow, SelectMode};

/// Which elements `MeshOp::Delete` removes.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteWhat {
    /// The selected faces, then edges left with no face and verts left
    /// with no edge.
    Faces,
    /// The selected edges and their faces.
    Edges,
    /// The selected verts, their edges and faces.
    Verts,
    /// The selected faces only; their edges stay as wire.
    OnlyFaces,
}

/// Where `MeshOp::Merge` puts the merged vertex.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeAt {
    /// The average position; skins blended equally. The lowest id stays.
    Center,
    /// The lowest selected id, where it is.
    First,
    /// The highest selected id, where it is.
    Last,
    /// The lowest id, moved to this point.
    Cursor([f32; 3]),
}

fn one() -> u32 {
    1
}

fn half() -> f32 {
    0.5
}

/// A mesh operation as a value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MeshOp {
    /// Changes the selection mode, converting the selection.
    Mode { mode: SelectMode },
    /// Ids of the mode's kind.
    Select {
        ids: Vec<u32>,
        #[serde(default)]
        how: SelectHow,
    },
    SelectAll,
    SelectNone,
    SelectInvert,
    SelectMore,
    SelectLess,
    /// Everything connected over edges to the selection.
    SelectLinked,
    /// Adds the faces of a material.
    SelectMaterial { material: u16 },
    /// Adds the faces of a group.
    SelectGroup { group: u16 },
    /// The loop (or with `ring`, the ring) through an edge, in edge mode.
    SelectLoop {
        edge: EdgeId,
        #[serde(default)]
        ring: bool,
    },
    /// The selection's verts by a 4x3 matrix about `pivot` (default: the
    /// selection's centre).
    Transform {
        matrix: [[f32; 4]; 3],
        #[serde(default)]
        pivot: Option<[f32; 3]>,
    },
    /// The selected faces as a region; the extruded faces then move by
    /// `offset`.
    ExtrudeFaces {
        #[serde(default)]
        offset: [f32; 3],
    },
    /// The selected boundary or wire edges; the copies move by `offset`.
    ExtrudeEdges {
        #[serde(default)]
        offset: [f32; 3],
    },
    /// A disconnected copy of the selection, moved by `offset`.
    Duplicate {
        #[serde(default)]
        offset: [f32; 3],
    },
    Delete { what: DeleteWhat },
    /// By mode: verts with two edges join them; edges with two faces join
    /// the faces; in face mode, the edges between selected faces.
    Dissolve,
    Merge { at: MergeAt },
    /// Selected verts within `distance`, clustered, merged at each
    /// cluster's lowest id.
    Weld { distance: f32 },
    /// A new loop across the ring through `edge`: `cuts` loops, at
    /// `factor` along the ring edges when `cuts` is 1, else evenly.
    LoopCut {
        edge: EdgeId,
        #[serde(default = "one")]
        cuts: u32,
        #[serde(default = "half")]
        factor: f32,
    },
    /// The selected edges, each cut into `cuts + 1` equal parts.
    Subdivide {
        #[serde(default = "one")]
        cuts: u32,
    },
    /// One face from an ordered vert cycle, or (`verts` absent) from the
    /// closed chain of selected edges.
    Fill {
        #[serde(default)]
        verts: Option<Vec<VertId>>,
    },
    /// A band of quads joining the boundary loops through two edges.
    Bridge { a: EdgeId, b: EdgeId },
    /// Reverses the selected faces.
    Flip,
    /// Orients the selected faces' components alike and outward, then
    /// recomputes every smooth fan.
    RecalcNormals,
    /// The selected faces into triangles.
    Triangulate,
    SetMaterial { material: u16 },
}

/// What an op did: element counts after it, the ids it made, and the
/// selection's size.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct OpResult {
    pub verts: usize,
    pub edges: usize,
    pub faces: usize,
    pub new_verts: Vec<VertId>,
    pub new_edges: Vec<EdgeId>,
    pub new_faces: Vec<FaceId>,
    pub selected: usize,
}

impl Mesh {
    /// Applies `op`. It works on a copy, validates it, and swaps it in, so
    /// an op that fails, or would break an invariant, changes nothing.
    pub fn apply(&mut self, op: &MeshOp) -> Result<OpResult, MeshError> {
        let mut next = self.clone();
        let (v0, e0, f0) = next.id_marks();
        next.apply_in(op)?;
        next.flush_selection();
        if let Err(errors) = next.validate() {
            return Err(MeshError::Invalid(errors.join("; ")));
        }
        let (verts, edges, faces) = next.counts();
        let result = OpResult {
            verts,
            edges,
            faces,
            new_verts: next.vert_ids().filter(|v| v.0 >= v0).collect(),
            new_edges: next.edge_ids().filter(|e| e.0 >= e0).collect(),
            new_faces: next.face_ids().filter(|f| f.0 >= f0).collect(),
            selected: next.select.len(),
        };
        *self = next;
        Ok(result)
    }

    /// Applies each op in turn; stops at the first failure, keeping what
    /// the earlier ops did.
    pub fn apply_all(&mut self, ops: &[MeshOp]) -> Result<(), (usize, MeshError)> {
        for (i, op) in ops.iter().enumerate() {
            self.apply(op).map_err(|e| (i, e))?;
        }
        Ok(())
    }

    fn apply_in(&mut self, op: &MeshOp) -> Result<(), MeshError> {
        match op {
            MeshOp::Mode { mode } => self.set_mode(*mode),
            MeshOp::Select { ids, how } => self.select_ids(ids, *how),
            MeshOp::SelectAll => self.select_all(),
            MeshOp::SelectNone => self.select.clear(),
            MeshOp::SelectInvert => self.select_invert(),
            MeshOp::SelectMore => self.select_grow(true),
            MeshOp::SelectLess => self.select_grow(false),
            MeshOp::SelectLinked => self.select_linked(),
            MeshOp::SelectMaterial { material } => {
                let m = *material;
                self.select_faces_where(move |mesh, f| mesh.face_material(f) == m);
            }
            MeshOp::SelectGroup { group } => {
                let g = *group;
                self.select_faces_where(move |mesh, f| mesh.face_group(f) == g);
            }
            MeshOp::SelectLoop { edge, ring } => {
                if !self.is_edge(*edge) {
                    return Err(MeshError::Dead);
                }
                let edges = if *ring { self.edge_ring(*edge) } else { self.edge_loop(*edge) };
                self.select_only(SelectMode::Edge, &[], &edges, &[]);
            }
            MeshOp::Transform { matrix, pivot } => {
                let verts: BTreeSet<VertId> = self.selected_verts().into_iter().collect();
                if verts.is_empty() {
                    return Err(MeshError::NothingSelected);
                }
                let centre = pivot.unwrap_or_else(|| {
                    let points: Vec<Vec3> = verts.iter().map(|&v| self.position(v)).collect();
                    math::centroid(&points)
                });
                let xf = Affine { rows: *matrix }.about(centre);
                self.move_verts(&verts, &xf);
            }
            MeshOp::ExtrudeFaces { offset } => self.extrude_faces(*offset)?,
            MeshOp::ExtrudeEdges { offset } => self.extrude_edges(*offset)?,
            MeshOp::Duplicate { offset } => self.duplicate(*offset)?,
            MeshOp::Delete { what } => self.delete(*what)?,
            MeshOp::Dissolve => self.dissolve()?,
            MeshOp::Merge { at } => self.merge(*at)?,
            MeshOp::Weld { distance } => self.weld(*distance)?,
            MeshOp::LoopCut { edge, cuts, factor } => self.loop_cut(*edge, *cuts, *factor)?,
            MeshOp::Subdivide { cuts } => self.subdivide(*cuts)?,
            MeshOp::Fill { verts } => self.fill(verts.as_deref())?,
            MeshOp::Bridge { a, b } => self.bridge(*a, *b)?,
            MeshOp::Flip => self.flip()?,
            MeshOp::RecalcNormals => self.recalc_normals()?,
            MeshOp::Triangulate => self.triangulate_faces()?,
            MeshOp::SetMaterial { material } => {
                let faces = self.selected_faces();
                if faces.is_empty() {
                    return Err(MeshError::NothingSelected);
                }
                for f in faces {
                    self.f_mut(f).material = *material;
                }
            }
        }
        Ok(())
    }
}
