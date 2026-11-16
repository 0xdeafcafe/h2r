//! Tag-index slots a conversion may reuse.
//!
//! A map built from a retail one (`TagSet::from_map`) can't grow its tag
//! index: uninterpreted bytes follow the entries (docs/formats/map-builder.md,
//! "Pinned maps"). New tags (a vehicle collection the base lacks, the
//! converted lightmap bitmap) therefore take the slot of a tag the converted
//! map no longer uses. Those are the tags only the base's structure BSP and
//! lightmap name: the conversion replaces both, so their own shaders,
//! lightmap bitmap, decorators, fog and sound environments fall out of use.
//!
//! "Only named by" is decided by scanning words: a tag counts as used when
//! its datum appears as a 4-aligned word in any metadata tag but itself (a
//! tag names its own datum, a bitmap in every image). A word that happens
//! to equal a datum only keeps a tag in use, so the scan errs on the safe
//! side. References from the shared caches into a map's own tags don't
//! exist (a datum indexes the map's own index).

use std::collections::BTreeSet;
use std::io::{Read, Seek};

use map::{Map, TagId};

use crate::{Error, Result};

fn words(bytes: &[u8]) -> impl Iterator<Item = u32> + '_ {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|w| u32::from_le_bytes(*w))
}

/// The datums named by `bytes`, one 4-aligned word at a time, among
/// `known`.
fn named(bytes: &[u8], own: TagId, known: &BTreeSet<u32>) -> BTreeSet<u32> {
    words(bytes)
        .filter(|w| *w != own.0 && known.contains(w))
        .collect()
}

/// Tags that only the structure BSP `sbsp` and lightmap `ltmp` name, in
/// slot order, except `keep` (tags the converted structure still names).
pub fn spare<R: Read + Seek>(
    map: &mut Map<R>,
    sbsp: TagId,
    ltmp: TagId,
    keep: &[TagId],
) -> Result<Vec<TagId>> {
    let space = map
        .metadata_space()
        .ok_or(Error::Invalid("the base has no metadata region"))?;
    let tags: Vec<_> = map
        .tags()
        .iter()
        .filter_map(|t| t.location.map(|l| (t.id, l.space == space)))
        .collect();
    let known: BTreeSet<u32> = tags.iter().map(|(id, _)| id.0).collect();
    let mut used = BTreeSet::new();
    for &(id, in_metadata) in &tags {
        if in_metadata && id != sbsp && id != ltmp {
            used.extend(named(&map.read_tag_bytes(id)?, id, &known));
        }
    }
    let mut bsp = named(&map.read_tag_bytes(sbsp)?, sbsp, &known);
    bsp.extend(named(&map.read_tag_bytes(ltmp)?, ltmp, &known));
    let mut out: Vec<TagId> = bsp
        .into_iter()
        .map(TagId)
        .filter(|id| !used.contains(&id.0) && *id != sbsp && *id != ltmp && !keep.contains(id))
        .collect();
    out.sort_by_key(|id| id.index());
    Ok(out)
}

/// The lightmap bitmap the base's lightmap names (its first group's
/// `+0x18` reference), if any.
pub fn lightmap_bitmap<R: Read + Seek>(map: &mut Map<R>, ltmp: TagId) -> Result<Option<TagId>> {
    let light = tags::structure::Lightmap::read(map, ltmp)
        .map_err(|_| Error::Invalid("the base lightmap couldn't be read"))?;
    Ok(light.groups.first().and_then(|g| g.bitmap))
}
