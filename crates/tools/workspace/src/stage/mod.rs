//! The content stage as a project's kit and as a package's `map` item: the
//! adapters from `content_kit::stage` (the stage's own build) to this
//! crate's [`Fresh`](crate::Fresh) and `project_build::Kit`.
//!
//! - [`fresh`]: the stage as a package's `map` item.
//! - [`project_kit`]: the stage as a step of a project build.

pub mod fresh;
pub mod project_kit;
