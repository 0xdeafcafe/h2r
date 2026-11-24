//! The builder registry: the closed list of what turns a package's source
//! into tags (docs/editor/studio-redesign.md, "The model"). A package names
//! a builder by an item's `kind`; it cannot add one.
use content::package::{Item, Kind};

use crate::build::{Cx, Failure};

/// Bumped when a builder's output for the same source changes, so that
/// every cached build is stale ([`crate::cache_key`]).
pub const BUILDERS_VERSION: u32 = 1;

/// One item to build: the manifest's entry and its source file's bytes
/// (empty for an item with no source).
pub struct Job<'a> {
    pub item: &'a Item,
    pub source: &'a [u8],
}

/// A builder: what it does to a build, and the checks that need no map.
pub struct Builder {
    pub kind: Kind,
    /// What `h2 package check` calls it.
    pub name: &'static str,
    /// Changes the map in `cx`.
    pub apply: fn(&mut Cx<'_>, &Job<'_>) -> Result<(), Failure>,
    /// Refuses a source that cannot build, with no map to build it on.
    pub check: fn(&Job<'_>) -> Result<(), Failure>,
}

static REGISTRY: [Builder; 4] = [
    Builder {
        kind: Kind::Tag,
        name: "tag",
        apply: crate::tagitem::apply,
        check: crate::tagitem::check,
    },
    Builder {
        kind: Kind::Edits,
        name: "edits",
        apply: crate::edit::apply,
        check: crate::edit::check,
    },
    Builder {
        kind: Kind::Bitmap,
        name: "bitmap",
        apply: crate::bitmap::apply,
        check: crate::bitmap::check,
    },
    Builder {
        kind: Kind::Hud,
        name: "hud",
        apply: crate::hud::apply,
        check: crate::hud::check,
    },
];

/// Every builder this build has. The other kinds of "Contents" are named
/// by manifests already and have no builder yet.
pub fn registry() -> &'static [Builder] {
    &REGISTRY
}

/// The builder for `kind`.
pub fn builder(kind: Kind) -> Option<&'static Builder> {
    REGISTRY.iter().find(|b| b.kind == kind)
}

/// Whether an item of `kind` changes the map it is built on. A `hud` item
/// does not: the game compiles its records at install, so a package of
/// nothing else leaves every map as it was and the rewrite serves none
/// of them anew.
pub fn changes_map(kind: Kind) -> bool {
    kind != Kind::Hud
}
