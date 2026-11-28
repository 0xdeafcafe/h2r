//! The kit added to a multiplayer map: the map's tags decomposed
//! ([`TagSet::from_map`]), the pod's tags added ([`kit::install`]) and the
//! map built again. The same map and kit give the same bytes.
//!
//! The kit's tags take tag memory, which a map type has a fixed amount of
//! (`map::writer::max_metadata_size`). A map without the room is refused,
//! with what the kit takes and what the map has ([`Costs`]).

use std::fmt;
use std::io::Cursor;

use anyhow::{Context, Result, bail, ensure};
use map::build::TagSet;
use map::writer::max_metadata_size;
use map::{CacheType, Map};

use super::kit::{self, Facts, Tags};
use super::{manifest, tag_path};

/// What the kit costs a map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Costs {
    /// The increase in the map's tag memory (header `+0x1c`), with page
    /// rounding and the index's growth.
    pub tag_growth: u32,
    /// The tag memory the map had free before the kit.
    pub available: u32,
    /// The increase in the file's size: the tags, the atlas and the
    /// model's vertex data.
    pub file_growth: i64,
    /// Tags added.
    pub tags: u32,
}

impl fmt::Display for Costs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} tags; tag memory {} B of {} B free; file {:+} B",
            self.tags, self.tag_growth, self.available, self.file_growth
        )
    }
}

/// A map with the kit in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Added {
    pub bytes: Vec<u8>,
    /// The pod's tags, by datum.
    pub tags: Tags,
    pub costs: Costs,
}

/// The string a map carrying this kit interns: the manifest's
/// `<item>_kit_v<kit_version>`.
pub fn marker_string() -> String {
    manifest().marker_string()
}

/// `original` (the multiplayer map `stem`) with the kit added.
pub fn add_flood_pod(original: &[u8], stem: &str) -> Result<Added> {
    add_flood_pod_within(original, stem, None)
}

/// [`add_flood_pod`] with the tag memory a multiplayer map may have given,
/// not the engine's (tests, and tools sizing a map).
pub fn add_flood_pod_within(original: &[u8], stem: &str, tag_memory: Option<u32>) -> Result<Added> {
    let mut map =
        Map::from_reader(Cursor::new(original)).with_context(|| format!("{stem}: not a map"))?;
    let header = map.header().clone();
    if header.cache_type != CacheType::Multiplayer {
        bail!(
            "{stem}: a {:?} map; the Flood pod goes into multiplayer maps",
            header.cache_type
        );
    }
    let facts = Facts::read(&mut map).with_context(|| format!("{stem}: the Flood pod"))?;
    let mut set = TagSet::from_map(&mut map).with_context(|| format!("{stem}: its tags"))?;
    ensure!(
        set.find(map::FourCC(*b"vehi"), tag_path()).is_none(),
        "{stem}: the map already has {}.vehi",
        tag_path()
    );
    let before = set.slots.len();
    // The map's index gains room for the tags.
    crate::kit::make_index_room(&mut set);
    let tags = kit::install(&mut set, &facts).with_context(|| format!("{stem}: the Flood pod"))?;
    // The version travels in the map.
    set.intern(&marker_string())?;
    // The headroom is checked here, against the layout built, so a
    // refusal says what the kit takes.
    set.check_tag_memory = false;
    let added = (set.slots.len() - before) as u32;
    let (bytes, layout) = set.build().with_context(|| format!("{stem}: rebuilding"))?;

    let limit = match tag_memory {
        Some(n) => n,
        None => max_metadata_size(CacheType::Multiplayer)?,
    };
    let costs = Costs {
        tag_growth: layout
            .memory_size
            .checked_sub(header.meta_size)
            .with_context(|| format!("{stem}: the rebuilt map's tag memory shrank"))?,
        available: limit.saturating_sub(header.meta_size),
        file_growth: bytes.len() as i64 - original.len() as i64,
        tags: added,
    };
    ensure!(
        costs.tag_growth <= costs.available,
        "{stem}: no room for the Flood pod: {costs}"
    );
    Ok(Added { bytes, tags, costs })
}
