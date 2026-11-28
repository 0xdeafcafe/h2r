//! The package build (docs/editor/studio-redesign.md, "Packages"; the layers
//! and edits of docs/editor/tag-workspace.md).
//!
//! - [`registry`]: the closed list of builders: `tag` (a new tag from a
//!   YAML file, or with `replaces` an override of an asset whole), `edits`
//!   (changed fields of tags that exist, each line naming the value it
//!   replaces), `bitmap` (a PNG as a `bitm`), `model` (a primitive-YAML model as `bitm`, `shad`,
//!   `mode` and `hlmt`), `sound` (a WAV clip a game feature plays, or with a
//!   `tag` a `snd!` and its gestalt rows: [`import`]), `data`
//!   (a file a game feature reads), `hud` (a HUD kit YAML, read by the
//!   game's install; [`huds`] lists a set's), `screens` and `screen-edits`
//!   (UI screens and changes to the map's screens), `animation` (authored
//!   keys baked to a `jmad` resource), `rules` (Firefight's wave sets, as
//!   [`wave_sets`] lists a set's), `tag-delta` (a BPS patch of one asset's
//!   image: [`delta`]) and `kit` (a named builder of ours,
//!   [`kits`]). A package's `[[item]]` names
//!   one by its `kind`.
//! - [`script_registry`] and [`hopper_fragments`]: what a set's `rules`, `variant` and
//!   `playlist` items give the game (the scripts, the list entries) and the
//!   server (the default hopper's playlists); the builders only check them.
//! - [`build`]: an ordered list of [`Loaded`] packages applied to a map's
//!   [`map::build::TagSet`] for one [`Target`]. A tag path added twice stops
//!   it naming both; an edit with a stale `was` or a path that does not
//!   resolve stops it with the file's line; two packages setting one field
//!   leave the later's value and an entry in [`Report::overrides`].
//! - [`build_map_settled`]: [`build_map`] for the game, where a package
//!   refused for the map is off for it whole, with its reason in
//!   [`Report::refused`], and the rest apply
//!   (docs/engine/asset-store.md, "Conflicts").
//! - [`build_map_streamed`]: [`build_map_settled`] on a map read by offset
//!   ([`base::Base`]), leaving its raw data and BSP regions in the source:
//!   the game's rewrite of a map of any size.
//! - [`assets`]: the asset hashes of a base level (`tags::repo::normal`),
//!   what a mod's `was` and `replaces` name.
//! - [`cache_key`]: the key of a build's output.
//! - [`Layered`]: tag sources in order, first hit wins.
//! - [`edits`]: edit files (`assets/tag-edits/`, `<data>/workspace/edits/`)
//!   applied to loose tags, [`edits::Edited`] a tag source with them over
//!   the layers (docs/editor/tag-workspace.md, "Edits are data").
//! - [`project`]: projects, each a package's `map` item, and the stack a
//!   build reads: edits over the authored layer ([`authored`], the `tag`
//!   items the map takes, generated under `<data>/workspace/generated/`)
//!   over import layers over the tree; [`project_build`] turns a project
//!   into its map package, the kits as its steps ("Projects", "Kits are
//!   build steps").
//! - [`check_package`], [`check_set`]: what `h2 package check` reports;
//!   [`store_findings`]: the files of a package that are a raw resource or a
//!   tag file of the user's own store.
//! - [`prepare`]: `map` and `map-patch` items built to map packages under
//!   `<data>/maps/` ([`maps`](crate::maps) module docs).
//! - [`convert`]: `convert` items run with the converters the caller
//!   has (the Halo 1 import, the Map Pack install), each map stamped under
//!   `<data>/packages/stamps/` ([`convert`](mod@crate::convert) module docs).
//! - [`converted`]: a converter's output map kept in the tag store as a level
//!   of its own (`converted::store_converted`; docs/engine/asset-store.md,
//!   slice AS10).
//! - [`build_map`]: [`build`] on a map file's bytes, laid out again.
//! - [`GameSet`]: the game's packages, baked then `<data>/packages/`, under
//!   `set.toml` and the game's features; [`PackageRewrite`] serves each map
//!   the game reads with the set applied, cached under
//!   `<data>/packages/cache/`.
//! - [`level`]: a level of the tag store with the set laid over it, built
//!   once into an image under `<data>/store/images/` and kept by its
//!   `LevelId` (docs/engine/asset-store.md, "Runtime assembly").
//!
//! Nothing here reads a disc: the base map is the caller's (the VFS's, for
//! the rewrite), and the tests build their own.
mod animation;
pub mod assets;
pub mod base;
pub mod authored;
mod bitmap;
mod build;
mod builders;
mod convert;
pub mod converted;
pub mod delta;
mod edit;
pub mod edits;
mod firefight_sets;
mod game;
mod ground;
mod hash;
mod hud;
pub mod import;
pub mod kits;
mod layer;
pub mod level;
mod load;
mod map_variants;
mod maps;
mod model;
mod modes;
mod onto;
pub mod project;
pub mod project_build;
mod recipe;
mod rewrite;
mod sound;
pub mod stage;
mod screens;
mod tagitem;
mod unpin;

#[cfg(test)]
mod convert_tests;
#[cfg(test)]
mod converted_tests;
#[cfg(test)]
mod edits_tests;
#[cfg(test)]
mod import_tests;
#[cfg(test)]
mod modes_tests;
#[cfg(test)]
mod map_variant_tests;
#[cfg(test)]
mod maps_tests;
#[cfg(test)]
mod kit_tests;
#[cfg(test)]
mod level_tests;
#[cfg(test)]
mod mod_tests;
#[cfg(test)]
mod pogo_tests;
#[cfg(test)]
mod project_tests;
#[cfg(test)]
mod recipe_tests;
#[cfg(test)]
mod rewrite_tests;
#[cfg(test)]
mod screen_tests;
#[cfg(test)]
mod tests;

pub use assets::Assets;
pub use bitmap::{Picture, decode as decode_png};
pub use build::{
    BuildError, Cx, Failure, Loaded, Override, Report, Shading, Target, Unpin, WHOLE, build,
    build_with, cache_key, wants_shading,
};
pub use builders::{
    BUILDERS_VERSION, Builder, Job, builder, changes_map, overrides_whole, registry,
};
pub use content::package::Refusal;
pub use convert::{
    CONVERTERS, ConvertFailed, ConvertSpec, Converted, Converter, Input, InputOff, ItemStamp, Ran,
    STAMPS_DIR, Skipped, Stamp, Unit, UnitFailed, convert, inputs_off, stamp_path, unit_key,
};
pub use delta::{image, unimage};
pub use edit::{BlockLine, Op, SetLine, TagEdit, edits_text, parse_edits};
pub use firefight_sets::sets as wave_sets;
pub use game::{CACHE_DIR, Entry, GameSet, Listed, Off, Origin, PACKAGES_DIR, baked};
pub use ground::{GroundFile, GroundLabel, LabelName, Side};
pub use hash::tag_hash;
pub use hud::huds;
pub use map_variants::{MapVariant, map_variants, read as read_map_variant, stem_of_source as map_variant_stem};
pub use modes::{hopper_fragments, script_registry};
pub use layer::Layered;
pub use load::{
    Finding, LoadError, MAX_FILE_BYTES, added_tag, check_package, check_set, forbidden_files,
    lock_findings, read_lock, store_findings,
};
pub use maps::{
    BUILT_FILE, BaseMaps, TREE_KEY, Failed, Fresh, FreshJob, GroundJob, Held, HeldIn, MapPackage, MapSpec,
    MapsBuilt, Mode, NoFresh, PatchBase, Prepared, Wrote, build_maps, build_maps_held, built_names,
    held_in, prepare, restrict, without_taken,
    write as write_map_package,
};
pub use onto::{
    BuiltMap, StreamedMap, build_map, build_map_laid_out, build_map_settled, build_map_streamed,
    scope_of, scope_of_bytes,
};
pub use recipe::{TestMap, records_edits, scenario_edits, test_map_package};
pub use rewrite::{
    FontObserver, KitInputs, Observer, PackageRewrite, Rebuilt, RebuiltOver, Served, ServedMap,
    stem_of,
};
pub use tagitem::group_of_source;
pub use unpin::MapUnpin;
