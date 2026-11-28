//! The content stage (docs/editor/content-stage.md): a multiplayer map of
//! ours that Studio builds on the user's machine and puts content on.
//!
//! - [`layout`]: the layout from `content/stage/layout.yaml` (bays, pads,
//!   references, cameras), resolved against the player's size and checked
//!   for overlaps. It serialises as the build report's `layout`.
//! - [`mesh`]: the floors, scale and movement references and enclosures as
//!   convex faces, each drawn with one of two surfaces; the same faces are
//!   the render triangles and the collision polygons.
//! - [`texture`]: the textures, drawn in code: the floor grid with its
//!   distance labels and pads, and the ruler.
//! - [`light`]: the lightmap page, computed from the layout's lights: flat
//!   and dark bays constant, the day bay lit by its sun and sky fill with
//!   the sun's shadows.
//! - [`sky`]: the sky dome, its picture (gradient, horizon, compass
//!   letters, the sun's disc), model and tag.
//! - [`kit`]: the stage's own tags added to a tag set (bitmaps, shaders,
//!   the structure BSP and lightmap, the sky), and what they take from the
//!   base.
//! - [`map`](self::map): the map: the base's scenario made the stage's, a `fresh`
//!   build over a `TagSource` (the disc map or the tag tree), the report
//!   and the package.
//!
//! - [`data`]: the manifest, `content/stage/kit.yaml`: the stage's version,
//!   biped, floor material, salt, and the sky's, the lightmap's and the
//!   textures' values. The builders read it; none of those values is a
//!   constant here.
//!
//! The stage as a package's `map` item (`Fresh`) and as a step of a project
//! build (`Kit`) are `workspace::stage`, which adapts [`map::build`](self::map::build).
//!
//! No image, mesh or tag of the stage comes from a disc: the YAML is ours
//! and the textures are drawn here. What the stage needs from the user's
//! disc (the player's size, the globals, the shader templates) is read when
//! the map is built.
#![forbid(unsafe_code)]

pub mod data;
pub mod kit;
pub mod layout;
pub mod light;
pub mod map;
pub mod mesh;
pub mod sky;
pub mod texture;

pub use self::map::{Base, Built, Project, build};
pub use layout::{Layout, Lights, Player, SubjectPlace};

/// The subject pad of the bay `bay` in the shipped layout. The pad does
/// not depend on the player's size, so a stand-in player parses the
/// layout; a caller that needs the pad (Studio's subject placement,
/// docs/editor/live-editors.md) does not need a disc to ask for it.
pub fn subject_place(bay: &str) -> Result<SubjectPlace> {
    const STAND_IN: Player = Player {
        height: 0.7,
        radius: 0.2,
    };
    Layout::builtin(STAND_IN)?.subject_place(bay)
}

/// The stage's build version (`version` in `content/stage/kit.yaml`): part of
/// every build key. Bump it when the same sources would give other bytes.
pub fn version() -> u32 {
    data::data().version
}

/// The layout shipped with the repository.
pub const SOURCE: &str = include_str!("../../../../../content/stage/layout.yaml");

/// The stage's `map` item shipped with the repository: its package's
/// `map.toml`, also its tree build's project.
pub const PROJECT: &str = include_str!("../../../../../content/stage/map.toml");

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
    /// The base map lacks something the stage takes from it, or holds it
    /// in a form the stage cannot take.
    #[error("stage base: {0}")]
    Base(String),
    /// A step of the map's build failed.
    #[error("stage build: {0}")]
    Build(String),
    /// The project file is not as the stage needs it.
    #[error("stage project: {0}")]
    Project(String),
    /// Reading the base's tags failed.
    #[error("stage base: {0}")]
    Repo(#[from] tags::repo::Error),
}

/// Result for the stage.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
