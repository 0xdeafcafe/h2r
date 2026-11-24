//! [`build_map`]: [`build`] on a map file's bytes, the one path `h2 package
//! build` and the game's [`PackageRewrite`](crate::PackageRewrite) share, so
//! both write the same bytes for the same inputs.
use std::io::Cursor;

use content::package::MapScope;
use map::build::TagSet;
use map::{CacheType, Map};

use crate::build::{Loaded, Report, Shading, Target, Unpin, build_with, wants_shading};
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
    let mut map = Map::from_reader(Cursor::new(base)).map_err(|e| format!("not a map: {e}"))?;
    let unpin = MapUnpin::new(&mut map);
    let mut set = TagSet::from_map(&mut map).map_err(|e| format!("decomposing the map: {e}"))?;
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
    let report =
        build_with(&mut set, packages, target, reader, shading.as_ref()).map_err(
            |e| match &unpin {
                Ok(_) => e.to_string(),
                Err(why) => format!("{e} (the map's reader: {why})"),
            },
        )?;
    let (bytes, _) = set.build().map_err(|e| format!("building the map: {e}"))?;
    Ok((bytes, report))
}
