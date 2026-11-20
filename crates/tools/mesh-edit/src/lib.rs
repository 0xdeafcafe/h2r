//! The modeling kernel of h2-editor (docs/editor/modeling.md, "The mesh
//! kernel"): one type, `Mesh`, its operations, and `MeshOp`, the
//! operations as values. It knows nothing of Halo formats; `geometry-edit`
//! turns sections and collision surfaces into `Source`s and writes
//! `Export`s back.
//!
//! # The model
//!
//! - A **radial-edge mesh** (Blender's BMesh shape) in index arenas. An
//!   edge may have any number of faces (Halo 2's clusters have three on
//!   one edge where a wall meets a floor) and faces on one edge need not
//!   agree on direction. Faces are n-gons.
//! - **Ids** (`VertId`, `EdgeId`, `LoopId`, `FaceId`) are `u32` slot
//!   indices, allocated by appending, never reused: the same ops on the
//!   same mesh give the same ids.
//! - **Attributes**: per vert a position and optional `Skin` (four node and
//!   weight bytes summing to 255); per face corner a `Corner` (texture and
//!   lightmap coordinates, normal, binormal, tangent, incident light
//!   direction, the source vertex and dirty bits); per face a material,
//!   group, flags and source triangle.
//!
//! # Operations and undo
//!
//! `Mesh::apply(&MeshOp)` runs an op on a copy, validates the copy
//! (`Mesh::validate`, the invariants) and swaps it in, so a failed op
//! changes nothing. Undo is the editor's snapshot stack
//! (`editor_core::UndoStack` of `Arc<Mesh>`), not this crate's; the session
//! keeps the op log, the `MeshOp`s applied since import, and replaying the
//! log on the source gives the current mesh.
//!
//! # Import and export
//!
//! `Mesh::from_triangles` welds a section's bit-equal positions into verts
//! and keeps each source vertex's attributes on its corners.
//! `Mesh::export` is stable: an unedited mesh exports its source exactly,
//! and an edit that keeps the topology keeps the source's vertex and index
//! order.

mod attrs;
mod error;
pub mod fixtures;
mod ids;
mod io;
pub mod math;
mod mesh;
mod op;
mod ops;
mod select;
mod triangulate;
mod validate;

pub use attrs::{Corner, MAX_INFLUENCES, Skin, dirty};
pub use error::MeshError;
pub use ids::{BitSet, EdgeId, FaceId, LoopId, VertId};
pub use io::{Export, ImportReport, Source};
pub use mesh::{Mesh, Poly, edge_flags, face_flags};
pub use op::{DeleteWhat, MergeAt, MeshOp, OpResult};
pub use ops::Affine;
pub use select::{SelectHow, SelectMode, Selection};
pub use triangulate::triangulate;
