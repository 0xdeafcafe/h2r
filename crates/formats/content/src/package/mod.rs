//! Packages (docs/editor/studio-redesign.md, "Packages"): a folder with a
//! `package.toml` and the files it names, the one unit the game build,
//! boot, Studio and Live all read.
//!
//! - [`Manifest`]: `package.toml`, parsed and validated.
//! - [`Lock`]: `package.lock`, the SHA-256 of each source and its inputs.
//! - [`PackageId`]: the SHA-256 of the manifest's identity with each source
//!   replaced by its file hash and additional inputs paired with their hashes.
//! - [`Set`]: the packages of one game in load order, from `requires`,
//!   `after` and the user's `set.toml` ([`SetFile`]); [`Refusal`]: a package
//!   one level leaves out, and why.
//!
//! These are map packages' neighbours, not their replacement: a `map` item
//! builds to a [`crate::Manifest`] package under `<data>/maps/`.
//!
//! A package holds our own content and patches, never bytes from a disc;
//! [`Manifest::validate`] refuses the file kinds that would. A mod's items
//! name the assets they change by key and asset hash (`replaces`, `was`,
//! a `set` line's `from`), so the package says which of the user's own
//! assets it changes and holds only the change
//! (docs/engine/asset-store.md, "Mods and packages").
mod id;
mod manifest;
mod set;
#[cfg(test)]
mod tests;

pub use id::{IdError, Lock, PackageId};
pub use manifest::{
    Credit, Item, ItemScope, Kind, MANIFEST_FILE, Manifest, ManifestError, MapScope, Maps,
    PACKAGE_FORMAT, Package as Header, valid_slug,
};
pub use set::{ALWAYS_ON, Package, PackageError, Problem, Refusal, Set, SetError, SetFile};

/// The lock's file name, beside the manifest.
pub const LOCK_FILE: &str = "package.lock";
/// The user's set file under `<data>/packages/`.
pub const SET_FILE: &str = "set.toml";
