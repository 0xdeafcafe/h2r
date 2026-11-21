//! The operations' bodies, called by `Mesh::apply` (`op.rs`).
//!
//! Every operation is written with whole-face edits: record the faces it
//! changes as `Poly`s, then rebuild them (keeping their ids), add new ones
//! and kill old ones. The disk and radial links are kept by those
//! primitives alone. An operation may fail half-way: `apply` runs it on a
//! copy, so a failure leaves the mesh as it was.

mod cut;
mod delete;
mod extrude;
mod merge;
mod surface;
mod transform;

pub use transform::Affine;
