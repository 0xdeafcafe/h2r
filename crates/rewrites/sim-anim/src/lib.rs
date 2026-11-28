//! Animation sampling and node matrices for the simulation rewrite
//! (docs/engine/sim-rewrite.md phases 1a to 1d; the contracts are
//! docs/engine/anim-sampling.md, docs/engine/skeleton-walk.md and
//! docs/engine/markers.md and docs/engine/animation-manager.md).
//!
//! The guest evaluates an animation at a frame by walking per-node flag
//! arrays and calling a codec function per channel per node. Here each
//! animation is copied out of guest memory once into a [`Clip`] (its flag
//! walk turned into node lists), kept in a [`ClipStore`], and [`sample`]
//! evaluates it into the guest's 32-byte node records with the guest's
//! arithmetic in the guest's order. No guest memory is touched while
//! sampling; the engine adapter (`engine::sim_anim`) reads the request
//! and writes the records back.
//!
//! [`build_matrices`] turns those records into an object's node matrices
//! with the skeleton walk's arithmetic (the pose matrix and the parent
//! composition per node), again without touching guest memory; the
//! adapter is `engine::sim_anim::skeleton`. [`graph_matrices`] is the same
//! arithmetic in the animation graph's own walk `1dd1c0`, which visits the
//! nodes through their child and sibling links; its adapter is
//! `engine::sim_anim::graph_walk`.
//!
//! [`model_markers`] turns a model's marker instances into the guest's
//! marker records (the group scan `1d8f00` and the transform fill
//! `1d8f50` of `b8d30`); the adapter is `engine::sim_anim::markers`.
//!
//! [`build_palette`] fills a first-person seat's bone palette (the
//! identity fill and model-node overlay of `16760c`); the adapter is
//! `engine::sim_anim::palette`.
//!
//! The bar is bit-exact with retail. A call the kernel cannot reproduce
//! exactly (an operand off the fast path, an unknown codec, the broken
//! evaluator pair) returns [`Unsupported`] and the guest runs it.
#![deny(clippy::disallowed_methods)]

pub mod clip;
pub mod codec;
pub mod graph_resolve;
pub mod graph_walk;
pub mod kernel;
pub mod manager_channels;
pub mod markers;
pub mod palette;
pub mod skeleton;
pub mod store;

pub use clip::{Channel, Clip, Layout, Shape, Sizes, Stream, Which};
pub use codec::{Frame, RotOp, VecOp, Vector};
pub use graph_walk::{GraphNode, GraphWalk, graph_matrices, graph_matrices_guest, graph_order};
pub use kernel::{Effects, Mode, Ops, Record, Refusal, Request, Secondary, Unsupported, sample};
pub use markers::{
    Constants as MarkerConstants, Marker, Selection, basis, find_group, model_markers, select,
};
pub use palette::{Overlay, Palette, build as build_palette};
pub use skeleton::{Constants, Walk, build_matrices, build_matrices_guest};
pub use store::ClipStore;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod graph_walk_tests;
#[cfg(test)]
mod markers_tests;
#[cfg(test)]
mod palette_tests;
#[cfg(test)]
mod skeleton_tests;
