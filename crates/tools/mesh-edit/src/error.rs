//! Why an operation was refused.

use std::fmt;

/// Why an operation was refused. A refused operation changed nothing
/// (`Mesh::apply` works on a copy and swaps it in only on success).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshError {
    /// An id names a dead or never-made element.
    Dead,
    /// A face with fewer than three corners or a repeated corner, or an
    /// edge from a vertex to itself.
    Degenerate,
    /// A face with the same vertex cycle as an existing one (either
    /// direction).
    DuplicateFace,
    /// The operation needs selected elements and none are.
    NothingSelected,
    /// An edge has the wrong number of faces for the operation (extruding
    /// an edge with two faces, filling across a three-face edge).
    NotBoundary,
    /// There is no quad on the edge to cut across.
    NotQuadRing,
    /// Two loops to bridge differ in length, share vertices, or are not
    /// closed boundary loops.
    LoopMismatch,
    /// The two vertices are not non-adjacent corners of one face.
    NoSharedFace,
    /// The input does not describe a mesh.
    BadInput(String),
    /// The operation left the mesh failing `validate`: a kernel bug. The
    /// mesh is unchanged.
    Invalid(String),
}

impl fmt::Display for MeshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MeshError::Dead => write!(f, "id names a dead element"),
            MeshError::Degenerate => write!(f, "degenerate face or edge"),
            MeshError::DuplicateFace => write!(f, "a face with these corners exists"),
            MeshError::NothingSelected => write!(f, "nothing selected"),
            MeshError::NotBoundary => write!(f, "edge has the wrong number of faces"),
            MeshError::NotQuadRing => write!(f, "no quad ring through the edge"),
            MeshError::LoopMismatch => write!(f, "loops cannot be bridged"),
            MeshError::NoSharedFace => write!(f, "vertices share no face to split"),
            MeshError::BadInput(what) => write!(f, "bad input: {what}"),
            MeshError::Invalid(what) => write!(f, "operation broke an invariant: {what}"),
        }
    }
}

impl std::error::Error for MeshError {}
