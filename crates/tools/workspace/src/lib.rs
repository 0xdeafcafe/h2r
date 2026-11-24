//! The package build (docs/editor/studio-redesign.md, "Packages"; the layers
//! and edits of docs/editor/tag-workspace.md).
//!
//! - [`registry`]: the closed list of builders: `tag` (a new tag from a
//!   YAML file), `edits` (changed fields of tags that exist), `bitmap`
//!   (a PNG as a `bitm`), `model` (a primitive-YAML model as `bitm`, `shad`,
//!   `mode` and `hlmt`), `sound` (a WAV clip a game feature plays), `data`
//!   (a file a game feature reads) and `hud` (a HUD kit YAML, read by the
//!   game's install; [`huds`] lists a set's). A package's `[[item]]` names
//!   one by its `kind`.
//! - [`build`]: an ordered list of [`Loaded`] packages applied to a map's
//!   [`map::build::TagSet`] for one [`Target`]. A tag path added twice stops
//!   it naming both; an edit with a stale `was` or a path that does not
//!   resolve stops it with the file's line; two packages setting one field
//!   leave the later's value and an entry in [`Report::overrides`].
//! - [`cache_key`]: the key of a build's output.
//! - [`Layered`]: tag sources in order, first hit wins.
//! - [`check_package`], [`check_set`]: what `h2 package check` reports.
//! - [`build_map`]: [`build`] on a map file's bytes, laid out again.
//! - [`GameSet`]: the game's packages, baked then `<data>/packages/`, under
//!   `set.toml` and the game's features; [`PackageRewrite`] serves each map
//!   the game reads with the set applied, cached under
//!   `<data>/packages/cache/`.
//!
//! Nothing here reads a disc: the base map is the caller's (the VFS's, for
//! the rewrite), and the tests build their own.
mod bitmap;
mod build;
mod builders;
mod edit;
mod game;
mod hash;
mod hud;
mod layer;
mod load;
mod model;
mod onto;
mod rewrite;
mod sound;
mod tagitem;
mod unpin;

#[cfg(test)]
mod pogo_tests;
#[cfg(test)]
mod rewrite_tests;
#[cfg(test)]
mod tests;

pub use bitmap::{Picture, decode as decode_png};
pub use build::{
    BuildError, Cx, Failure, Loaded, Override, Report, Shading, Target, Unpin, build, build_with,
    cache_key, wants_shading,
};
pub use builders::{BUILDERS_VERSION, Builder, Job, builder, changes_map, registry};
pub use edit::{BlockLine, Op, SetLine, TagEdit, parse_edits};
pub use game::{CACHE_DIR, Entry, GameSet, Listed, Off, Origin, PACKAGES_DIR, baked};
pub use hash::tag_hash;
pub use hud::huds;
pub use layer::Layered;
pub use load::{
    Finding, LoadError, MAX_FILE_BYTES, added_tag, check_package, check_set, forbidden_files,
    lock_findings, read_lock,
};
pub use onto::{build_map, scope_of, scope_of_bytes};
pub use rewrite::{Observer, PackageRewrite, Rebuilt, Served, ServedMap, stem_of};
pub use tagitem::group_of_source;
pub use unpin::MapUnpin;
