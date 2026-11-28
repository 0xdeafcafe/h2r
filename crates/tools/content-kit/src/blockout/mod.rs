//! A block-out definition as tags (docs/game/forge-blockout.md, "A
//! definition as tags").
//!
//! [`install`] adds one definition's tags to a map's tag set, all at the
//! definition's path (`forge_shapes::PieceDef::tag_path`):
//!
//! - **`bloc`** (`object.rs`): the crate, its bounding sphere from the
//!   size, naming the `hlmt`.
//! - **`hlmt`**: one region and permutation, the render model, the
//!   collision model and the physics model, one material.
//! - **`mode`** ([`mesh`]): the shell's drawn faces as triangle lists, one
//!   part a shader in use, a dev face's plate cut out of it as cell quads
//!   over the font atlas. Absent when no face is drawn (every role `Nodraw`
//!   or `Clip`: a barrier). A definition whose roles are not all `Dev` also
//!   has the dev render model, every role `Dev`, at the path of its dev
//!   twin, which definitions that differ only in materials share.
//! - **`coll`** ([`collision`]): the shell compiled by `collision_build`,
//!   which must pass `collision_build::verify::check`; one BSP, one
//!   material, no BSP physics slots.
//! - **`phmo`** ([`physics`]): the hulls as one fixed rigid body.
//!
//! [`dev`] adds the dev set the first time a face needs it: four tiles and
//! the font as `bitm` and `shad` under `forge\blockout\dev`.
//!
//! [`Cost`] is what an install added: the tag memory its tags take as the
//! builder lays them out, and the raw bytes.
//!
//! What a map supplies is read from it ([`Facts`]): the shader template and
//! the global material. Nothing here is read from a disc but those names.
//!
//! [`palette`] surveys the surfaces a level is made of, for the Material
//! picker: each shader's area, texture repeat and class.

pub mod collision;
pub mod data;
pub mod dev;
mod install;
pub mod mesh;
mod object;
pub mod palette;
pub mod physics;
pub mod texture;

/// An invented map for other crates' tests (feature `fixture`).
#[cfg(any(test, feature = "fixture"))]
pub mod fixture;

#[cfg(test)]
mod tests;

pub use install::{Cost, Facts, Installed, cost_since, install, make_index_room, tag_size};
pub use data::{BlockoutData, data};
pub use physics::piece_body;
