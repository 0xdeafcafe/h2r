//! The content stage (docs/editor/content-stage.md): a multiplayer map of
//! ours that Studio builds on the user's machine and puts content on.
//!
//! - [`layout`]: the layout from `art/stage/stage.yaml` (bays, pads,
//!   references, cameras), resolved against the player's size and checked
//!   for overlaps. It serialises as the build report's `layout`.
//! - [`mesh`]: the floors, scale and movement references and enclosures as
//!   convex faces, each drawn with one of two surfaces; the same faces are
//!   the render triangles and the collision polygons.
//! - [`texture`]: the textures, drawn in code: the floor grid with its
//!   distance labels and pads, and the ruler.
//!
//! No image, mesh or tag of the stage comes from a disc: the YAML is ours
//! and the textures are drawn here. What the stage needs from the user's
//! disc (the player's size, the globals, the shader templates) is read when
//! the map is built.
#![forbid(unsafe_code)]

pub mod layout;
pub mod mesh;
pub mod texture;

pub use layout::{Layout, Player};

/// The stage's build version: part of every build key. Bump it when the
/// same sources would give other bytes.
pub const VERSION: u32 = 1;

/// The layout shipped with the repository.
pub const SOURCE: &str = include_str!("../../../../art/stage/stage.yaml");

/// What can go wrong building the stage.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The layout is not as the stage needs it.
    #[error("stage layout: {0}")]
    Layout(String),
    /// Two things on one bay take the same floor.
    #[error("stage layout: bay {bay}: {a} overlaps {b}")]
    Overlap { bay: String, a: String, b: String },
    /// The YAML does not parse.
    #[error("stage layout: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
    /// The faces do not compile to a collision BSP.
    #[error("stage collision: {0}")]
    Collision(#[from] collision_build::Error),
    /// A compiled collision BSP fails `collision_build::verify::check`.
    #[error("stage collision: {0}")]
    Verify(String),
}

/// Result for the stage.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
