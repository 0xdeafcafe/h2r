//! The builder registry: the closed list of what turns a package's source
//! into tags (docs/editor/studio-redesign.md, "The model"). A package names
//! a builder by an item's `kind`; it cannot add one.
use std::collections::BTreeMap;

use content::package::{Item, Kind};

use crate::build::{Cx, Failure};

/// Bumped when a builder's output for the same source changes, so that
/// every cached build is stale ([`crate::cache_key`]).
pub const BUILDERS_VERSION: u32 = 3;

/// One item to build: the manifest's entry, its source file's bytes
/// (empty for an item with no source) and the package's files by path, for a
/// builder whose source names another (a model's atlas).
pub struct Job<'a> {
    pub item: &'a Item,
    pub source: &'a [u8],
    pub files: &'a BTreeMap<String, Vec<u8>>,
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

static REGISTRY: [Builder; 18] = [
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
        kind: Kind::Model,
        name: "model",
        apply: crate::model::apply,
        check: crate::model::check,
    },
    Builder {
        kind: Kind::Animation,
        name: "animation",
        apply: crate::animation::apply,
        check: crate::animation::check,
    },
    Builder {
        kind: Kind::Sound,
        name: "sound",
        apply: crate::sound::apply,
        check: crate::sound::check,
    },
    Builder {
        kind: Kind::Data,
        name: "data",
        apply: crate::sound::data_apply,
        check: crate::sound::data_check,
    },
    Builder {
        kind: Kind::Hud,
        name: "hud",
        apply: crate::hud::apply,
        check: crate::hud::check,
    },
    Builder {
        kind: Kind::Screens,
        name: "screens",
        apply: crate::screens::apply,
        check: crate::screens::check,
    },
    Builder {
        kind: Kind::ScreenEdits,
        name: "screen-edits",
        apply: crate::screens::apply_edits,
        check: crate::screens::check_edits,
    },
    Builder {
        kind: Kind::Rules,
        name: "rules",
        apply: crate::modes::apply_rules,
        check: crate::modes::check_rules,
    },
    Builder {
        kind: Kind::Variant,
        name: "variant",
        apply: crate::modes::apply_variant,
        check: crate::modes::check_variant,
    },
    Builder {
        kind: Kind::Playlist,
        name: "playlist",
        apply: crate::modes::apply_playlist,
        check: crate::modes::check_playlist,
    },
    Builder {
        kind: Kind::Map,
        name: "map",
        apply: crate::maps::apply,
        check: crate::maps::check,
    },
    Builder {
        kind: Kind::MapPatch,
        name: "map-patch",
        apply: crate::maps::patch_apply,
        check: crate::maps::patch_check,
    },
    Builder {
        kind: Kind::Convert,
        name: "convert",
        apply: crate::convert::apply,
        check: crate::convert::check,
    },
    Builder {
        kind: Kind::Kit,
        name: "kit",
        apply: crate::kits::apply,
        check: crate::kits::check,
    },
    Builder {
        kind: Kind::TagDelta,
        name: "tag-delta",
        apply: crate::delta::apply,
        check: crate::delta::check,
    },
];

/// Every builder this build has. The other kinds of "Contents" are named
/// by manifests already and have no builder yet. `data` changes no map: a
/// game feature reads the file.
pub fn registry() -> &'static [Builder] {
    &REGISTRY
}

/// The builder for `kind`.
pub fn builder(kind: Kind) -> Option<&'static Builder> {
    REGISTRY.iter().find(|b| b.kind == kind)
}

/// Whether an item of this `kind` overrides an asset whole under
/// `replaces` (docs/engine/asset-store.md, "Mods and packages"): a `tag`
/// item does. The manifest lets `model`, `bitmap`, `sound` and `animation`
/// items carry one too, and the build refuses them by name: their builders
/// write their tags through the map.
pub fn overrides_whole(kind: Kind) -> bool {
    kind == Kind::Tag
}

/// Whether `item` changes the map it is built on. A `hud` or `data` item
/// does not, nor does a `map` or `map-patch` item (it builds a map package
/// at prepare, [`crate::maps`], and changes no other map), nor a `convert`
/// item (its converter writes under the data root at prepare,
/// [`crate::convert`]), nor does a `kit`
/// item (the game runs it on its files, [`crate::kits`]), nor does a `sound` item with no `tag`: the game compiles a
/// HUD's records at install and plays or reads the others from the package,
/// so a package of nothing else leaves every map as it was and the rewrite
/// serves none of them anew. A `sound` item with a `tag` writes a `snd!`
/// and rows of the map's gestalt.
pub fn changes_map(item: &Item) -> bool {
    match item.kind {
        Kind::Hud
        | Kind::Data
        | Kind::Rules
        | Kind::Variant
        | Kind::Playlist
        | Kind::Map
        | Kind::MapPatch
        | Kind::Convert
        | Kind::Kit => false,
        Kind::Sound => item.tag.as_deref().is_some_and(|t| !t.is_empty()),
        _ => true,
    }
}
