//! The Modeling tool's targets and save (docs/editor/modeling.md and
//! docs/editor/modeling-targets.md), with no UI.
//!
//! - A [`Target`] names what a mesh comes from: a structure BSP's cluster
//!   or instanced geometry definition, or a render model's section.
//! - [`GeometrySession`] opens targets as `mesh_edit::Mesh`es, applies
//!   `MeshOp`s with snapshot undo (`editor_core::UndoStack`) and keeps the
//!   op log; [`GeometrySession::changes`] is what a save writes.
//! - **In place**: moved vertices (and their normals and frames)
//!   re-encoded over their own bytes with
//!   `tags::geometry_write::encode_section`, a structure part's sphere and
//!   box, a cluster's bounds grown to hold its vertices, and a render
//!   model's compression box grown with every section requantised into
//!   it. An unedited target writes nothing, and an edit writes only the
//!   bytes it changed.
//! - **Grown raw**: an edit that changes a count encodes the whole section
//!   again (parts as lists with one subpart each, a structure section's
//!   visibility code and table rebuilt), appended to the map as a
//!   [`RawAppend`] with the owner's block info, resource entries, counts
//!   and bounds pointed at it. A bucket-lit structure section and a sky
//!   model's strips are refused and named.
//! - **Use for lower LODs** points a model permutation's lower levels of
//!   detail at an edited section.
//! - [`coll_target`]: a `coll` BSP as a polygon mesh, compiled back with
//!   `collision_build::coll::rebuild` and written as patches and grown
//!   blocks in the ordinary metadata.
//! - [`level_collision`]: a solid added to a structure's level collision
//!   (`collision_build::level`), read from the map and written back as
//!   patches and grown blocks in the ordinary metadata, or staged for the
//!   live game.

pub mod coll_target;
mod grow;
mod import;
pub mod level_collision;
mod session;
mod target;
mod visibility;
mod write;

/// The kernel the session edits with: `MeshOp` is the API's op value.
pub use mesh_edit;
/// The level collision compiler: `Polygon` is [`level_collision`]'s solid.
pub use collision_build;
pub use session::{
    Check, Drawn, Error, GeometrySession, OpenInfo, Patches, Preview, RAW_ALIGN, RawAppend,
    RawWrite, RegionWrite, WriteClass,
};
pub use target::{Target, TargetInfo, TargetKind, list, list_model};
