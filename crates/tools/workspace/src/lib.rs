//! The package build (docs/editor/studio-redesign.md, "Packages"; the layers
//! and edits of docs/editor/tag-workspace.md).
//!
//! - [`registry`]: the closed list of builders: `tag` (a new tag from a
//!   YAML file), `edits` (changed fields of tags that exist) and `bitmap`
//!   (a PNG as a `bitm`). A package's `[[item]]` names one by its `kind`.
//! - [`build`]: an ordered list of [`Loaded`] packages applied to a map's
//!   [`map::build::TagSet`] for one [`Target`]. A tag path added twice stops
//!   it naming both; an edit with a stale `was` or a path that does not
//!   resolve stops it with the file's line; two packages setting one field
//!   leave the later's value and an entry in [`Report::overrides`].
//! - [`cache_key`]: the key of a build's output.
//! - [`Layered`]: tag sources in order, first hit wins.
//! - [`check_package`], [`check_set`]: what `h2 package check` reports.
//!
//! Nothing here reads a disc: the base map is the caller's, and the tests
//! build their own.
mod bitmap;
mod build;
mod builders;
mod edit;
mod hash;
mod layer;
mod load;
mod tagitem;
mod unpin;

#[cfg(test)]
mod tests;

pub use bitmap::{Picture, decode as decode_png};
pub use build::{
    BuildError, Cx, Failure, Loaded, Override, Report, Target, Unpin, build, cache_key,
};
pub use builders::{BUILDERS_VERSION, Builder, Job, builder, registry};
pub use edit::{BlockLine, Op, SetLine, TagEdit, parse_edits};
pub use hash::tag_hash;
pub use layer::Layered;
pub use load::{
    Finding, LoadError, MAX_FILE_BYTES, check_package, check_set, forbidden_files, lock_findings,
    read_lock,
};
pub use tagitem::group_of_source;
pub use unpin::MapUnpin;
