//! Animation sampling and node matrices for the simulation rewrite
//! (docs/engine/sim-rewrite.md phases 1a and 1b; the contracts are
//! docs/engine/anim-sampling.md and docs/engine/skeleton-walk.md).
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
//! adapter is `engine::sim_anim::skeleton`.
//!
//! The bar is bit-exact with retail. A call the kernel cannot reproduce
//! exactly (an operand off the fast path, an unknown codec, the broken
//! evaluator pair) returns [`Unsupported`] and the guest runs it.
#![deny(clippy::disallowed_methods)]

pub mod clip;
pub mod codec;
pub mod kernel;
pub mod skeleton;
pub mod store;

pub use clip::{Channel, Clip, Layout, Shape, Sizes, Stream, Which};
pub use codec::{Frame, RotOp, VecOp, Vector};
pub use kernel::{Effects, Mode, Ops, Record, Request, Secondary, Unsupported, sample};
pub use skeleton::{Constants, Walk, build_matrices};
pub use store::ClipStore;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod skeleton_tests;
