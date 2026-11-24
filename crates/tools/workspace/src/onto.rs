//! [`build_map`]: [`build`] on a map file's bytes, the one path `h2 package
//! build` and the game's [`PackageRewrite`](crate::PackageRewrite) share, so
//! both write the same bytes for the same inputs.
//!
//! [`build_map_settled`] is the same build for the game
//! (docs/engine/asset-store.md, "Conflicts"): a package refused for this map
//! is off for it, whole, and the rest still apply.
use std::collections::BTreeSet;
use std::io::Cursor;

use content::package::{MapScope, Refusal};
use map::build::{BuildLayout, TagSet};
use map::{CacheType, Map};

use crate::assets::Assets;
use crate::build::{
    Loaded, Report, Shading, Target, Unpin, build_refusing, build_with, wants_shading,
};
use crate::unpin::MapUnpin;

/// A map's class, from its header's cache type. The two shared maps are
/// `shared`.
pub fn scope_of(cache_type: CacheType) -> MapScope {
    match cache_type {
        CacheType::Multiplayer => MapScope::Multiplayer,
        CacheType::Campaign => MapScope::Campaign,
        CacheType::MainMenu => MapScope::Mainmenu,
        _ => MapScope::Shared,
    }
}

/// The class `bytes`' header gives, if it reads as a map header.
pub fn scope_of_bytes(bytes: &[u8]) -> Option<MapScope> {
    map::Header::read_region(&mut Cursor::new(bytes), 0, bytes.len() as u64)
        .ok()
        .map(|h| scope_of(h.cache_type))
}

/// Decompose `base`, apply `packages` for `target` through [`build`] and
/// lay the map out again. A tag the base pins is laid out anew through the
/// map's reader ([`MapUnpin`]) when an item changes it; when the reader
/// cannot be made, such an item stops the build instead.
pub fn build_map(
    base: &[u8],
    packages: &[Loaded],
    target: &Target<'_>,
) -> Result<(Vec<u8>, Report), String> {
    build_map_laid_out(base, packages, target).map(|b| (b.bytes, b.report))
}

/// A map [`build_map_laid_out`] built, with where things went in it.
#[derive(Debug)]
pub struct BuiltMap {
    pub bytes: Vec<u8>,
    pub report: Report,
    /// The builder's layout of the file.
    pub layout: BuildLayout,
    /// Each raw resource the build placed, as (file offset, length).
    pub raw: Vec<(u64, u64)>,
}

/// [`build_map`], keeping the layout: the level images
/// ([`crate::level`]) name the raw resources a build placed by where they
/// went.
pub fn build_map_laid_out(
    base: &[u8],
    packages: &[Loaded],
    target: &Target<'_>,
) -> Result<BuiltMap, String> {
    match attempt(base, packages, target, None, None) {
        Ok(built) => Ok(built),
        Err(Broke::Failed(why)) => Err(why),
        // Nothing is refused here: every refusal is a failure.
        Err(Broke::Again { package, why }) => unreachable!("{package}: {why}"),
    }
}

/// [`build_map_laid_out`] with the conflict table
/// (docs/engine/asset-store.md, "Conflicts"). An item that is refused (a
/// `from`, `was` or `replaces` that does not match what the base holds, a
/// key the map lacks, a second override of one asset, a patch of an asset
/// an earlier package overrides whole without naming its hash) turns its
/// package off for this map, whole, with the reason in
/// [`Report::refused`], and so is every package that requires one off; the
/// other packages still apply. A package in `strict` is ours: its refusal
/// stops the build instead, with the item's line. Nothing is applied in
/// part: a package refused after one of its items ran is taken out by
/// building the map again without it.
///
/// `assets` gives the asset hashes of the base, what a `was` or `replaces`
/// names ([`crate::assets`]); with `None` a tag's own hash stands for its
/// asset's, so `h2 package build`'s checks and a build on an invented map
/// run with no store.
pub fn build_map_settled(
    base: &[u8],
    packages: &[Loaded],
    target: &Target<'_>,
    strict: &BTreeSet<String>,
    mut assets: Option<&mut dyn Assets>,
) -> Result<BuiltMap, String> {
    let mut left: Vec<Loaded> = packages.to_vec();
    let mut refused: Vec<Refusal> = Vec::new();
    loop {
        match attempt(
            base,
            &left,
            target,
            Some((strict, &mut refused)),
            assets.as_deref_mut(),
        ) {
            Ok(mut built) => {
                built.report.refused = refused;
                return Ok(built);
            }
            Err(Broke::Failed(why)) => return Err(why),
            Err(Broke::Again { package, why }) => {
                crate::build::turn_off(&mut refused, &left, &package, why);
                left.retain(|p| !refused.iter().any(|r| r.package == p.package.name()));
            }
        }
    }
}

/// Why an attempt to build the map stopped: an item failed, or a package
/// was refused after one of its items ran.
enum Broke {
    Failed(String),
    Again { package: String, why: String },
}

/// Decompose `base` once and apply `packages`. With `off`, refusals leave
/// packages off ([`build_map_settled`]); without, they stop the build.
fn attempt(
    base: &[u8],
    packages: &[Loaded],
    target: &Target<'_>,
    off: Option<(&BTreeSet<String>, &mut Vec<Refusal>)>,
    mut assets: Option<&mut dyn Assets>,
) -> Result<BuiltMap, Broke> {
    let mut map = Map::from_reader(Cursor::new(base))
        .map_err(|e| Broke::Failed(format!("not a map: {e}")))?;
    let unpin = MapUnpin::new(&mut map);
    let mut set = TagSet::from_map(&mut map)
        .map_err(|e| Broke::Failed(format!("decomposing the map: {e}")))?;
    // A tag laid out anew can land past the old metadata's end; the same
    // lift `ui_kit::retail_edits` makes, and the build's size check still
    // runs.
    set.check_tag_memory = false;
    if let Some(p) = set.pinned.as_mut() {
        // Room in the index for the tags the packages add.
        p.clear_index_tail();
    }
    let reader = unpin.as_ref().ok().map(|u| u as &dyn Unpin);
    // What a `model` item draws with is read from the base map, once.
    let shading: Option<Result<Shading, String>> = wants_shading(packages, target)
        .then(|| Shading::read(&mut map).map_err(|e| format!("{e:#}")));
    let stopped = |e: crate::build::BuildError| match &unpin {
        Ok(_) => Broke::Failed(e.to_string()),
        Err(why) => Broke::Failed(format!("{e} (the map's reader: {why})")),
    };
    let report = match off {
        Some((strict, off)) => build_refusing(
            &mut set,
            packages,
            target,
            reader,
            shading.as_ref(),
            strict,
            off,
            assets.as_deref_mut(),
        )
        .map_err(|s| match s {
            crate::build::Stopped::Failed(e) => stopped(e),
            crate::build::Stopped::Again { package, why } => Broke::Again { package, why },
        })?,
        None => {
            build_with(&mut set, packages, target, reader, shading.as_ref()).map_err(stopped)?
        }
    };
    let (bytes, layout) = set
        .build()
        .map_err(|e| Broke::Failed(format!("building the map: {e}")))?;
    let raw = layout
        .raw
        .iter()
        .zip(&set.raw)
        .map(|(&at, r)| (u64::from(at), r.bytes.len() as u64))
        .collect();
    Ok(BuiltMap {
        bytes,
        report,
        layout,
        raw,
    })
}
