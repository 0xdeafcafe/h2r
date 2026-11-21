//! The Modeling tool's targets and save (docs/editor/modeling.md and
//! docs/editor/modeling-targets.md), with no UI.
//!
//! - A [`Target`] names what a mesh comes from: a structure BSP's cluster
//!   or instanced geometry definition, or a render model's section.
//! - [`GeometrySession`] opens targets as `mesh_edit::Mesh`es, applies
//!   `MeshOp`s with snapshot undo (`editor_core::UndoStack`) and keeps the
//!   op log; [`GeometrySession::changes`] is what a save writes.
//! - Writes are **in place** only: moved vertices (and their normals and
//!   frames) re-encoded over their own bytes with
//!   `tags::geometry_write::encode_section`, a structure part's sphere and
//!   box, a cluster's bounds grown to hold its vertices, and a render
//!   model's compression box grown with every section requantised into
//!   it. An edit that changes a count is the grown class, refused and
//!   named: an unedited target writes nothing, and an edit writes only
//!   the bytes it changed.

mod import;
mod session;
mod target;
mod write;

pub use session::{Check, Error, GeometrySession, OpenInfo, Patches, RawWrite, RegionWrite, WriteClass};
pub use target::{Target, TargetInfo, TargetKind, list, list_model};
