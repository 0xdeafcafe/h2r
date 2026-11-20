//! Turn retail tags into relocatable [`map::build::Block`] trees, using the
//! reviewed layouts in this crate. A tag can only move when every pointer in
//! it is known; these functions check that the known records cover the whole
//! payload and decline (return `None`) otherwise, leaving the tag pinned.
//! See `docs/formats/map-builder.md`.
use crate::records::{BitmapHeader, BitmapImage, BitmapSequence, BitmapSprite};
use crate::wire::{BinaryRecord, TagBlock};
use crate::{Error, Result};
use map::build::{Block, RawRef, TagPayload, TagSet};
use map::{FourCC, Map, RawPointer, ResourceFile, TagId};
use std::io::{Read, Seek};

/// Alignment given to bitmap pixel data moved into new raw resources.
pub const BITMAP_RAW_ALIGN: u32 = 0x200;

/// What [`relocate_bitmap`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Relocated {
    /// Image LODs whose local pixel data was copied into new raw resources.
    pub moved_lods: u32,
    /// Bytes of pixel data copied.
    pub moved_bytes: u64,
}

/// Replace a pinned bitmap in `set` with a built block tree read from
/// `map`: header, sequences (with their sprites) and images. With
/// `move_raw`, each image's first three local LODs are copied into new raw
/// resources and their pointers become [`RawRef::Local`] fixups. Returns
/// `None` (and changes nothing) when the reviewed records do not cover the
/// tag's payload exactly once, i.e. when the tag may hold bytes or pointers
/// this crate does not know.
pub fn relocate_bitmap<R: Read + Seek>(
    set: &mut TagSet,
    map: &mut Map<R>,
    id: TagId,
    move_raw: bool,
) -> Result<Option<Relocated>> {
    let tag = map.tag(id)?.clone();
    if tag.group != FourCC(*b"bitm") {
        return Err(Error::UnsupportedGroup(tag.group));
    }
    let loc = tag.location.ok_or(map::Error::NoTagData)?;
    let bytes = map.read_tag_bytes(id)?;
    if bytes.len() < BitmapHeader::SIZE {
        return Ok(None);
    }
    let header = BitmapHeader::decode(&bytes[..BitmapHeader::SIZE])?;
    let mut spans = vec![(0usize, BitmapHeader::SIZE)];
    let span = |b: TagBlock, stride: usize, spans: &mut Vec<(usize, usize)>| {
        if b.count == 0 {
            return Some(None);
        }
        let start = b.address.checked_sub(loc.address)? as usize;
        let end = start.checked_add(b.count as usize * stride)?;
        (end <= bytes.len()).then(|| {
            spans.push((start, end));
            Some(start..end)
        })
    };
    let Some(sequence_range) = span(header.sequences, BitmapSequence::SIZE, &mut spans) else {
        return Ok(None);
    };
    let Some(image_range) = span(header.images, BitmapImage::SIZE, &mut spans) else {
        return Ok(None);
    };

    let mut root = Block::new(bytes[..BitmapHeader::SIZE].to_vec());
    if let Some(range) = sequence_range {
        let mut sequences = Block::new(bytes[range.clone()].to_vec());
        for (i, s) in bytes[range]
            .as_chunks::<{ BitmapSequence::SIZE }>()
            .0
            .iter()
            .enumerate()
        {
            let wire = BitmapSequence::decode(s)?;
            let Some(sprites) = span(wire.sprites, BitmapSprite::SIZE, &mut spans) else {
                return Ok(None);
            };
            let at = (i * BitmapSequence::SIZE + 0x34) as u32;
            match sprites {
                Some(r) => sequences.block(at, wire.sprites.count, Block::new(bytes[r].to_vec())),
                None => sequences.block(at, 0, Block::zeroed(0)),
            };
        }
        root.block(0x3c, header.sequences.count, sequences);
    } else {
        root.block(0x3c, 0, Block::zeroed(0));
    }
    // The payload must be exactly the union of the known records.
    spans.sort_unstable();
    let mut end = 0;
    for (lo, hi) in &spans {
        if *lo != end {
            return Ok(None);
        }
        end = *hi;
    }
    if end != bytes.len() {
        return Ok(None);
    }

    let mut moved = Relocated::default();
    let mut images_block = None;
    if let Some(range) = image_range {
        let mut images = Block::new(bytes[range.clone()].to_vec());
        for (i, b) in bytes[range]
            .as_chunks::<{ BitmapImage::SIZE }>()
            .0
            .iter()
            .enumerate()
        {
            let wire = BitmapImage::decode(b)?;
            if !move_raw {
                continue;
            }
            for lod in 0..3 {
                let size = wire.lod_sizes[lod];
                let Some(p) = RawPointer::decode(wire.lod_pointers[lod]) else {
                    continue;
                };
                if size == 0 || p.file != ResourceFile::Local {
                    continue;
                }
                let data = map.read_file_range(p.offset.into(), size.into())?;
                let raw = set.add_raw(data, BITMAP_RAW_ALIGN);
                images.raw(
                    (i * BitmapImage::SIZE + 0x1c + lod * 4) as u32,
                    RawRef::Local(raw),
                );
                moved.moved_lods += 1;
                moved.moved_bytes += u64::from(size);
            }
        }
        images_block = Some(images);
    }
    match images_block {
        Some(images) => root.block(0x44, header.images.count, images),
        None => root.block(0x44, 0, Block::zeroed(0)),
    };
    let entry = set
        .tag_mut(id)
        .ok_or(Error::Invalid("bitmap not in the tag set"))?;
    entry.payload = TagPayload::Built(root);
    Ok(Some(moved))
}

/// What [`moved_variant`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MovedSummary {
    /// Bitmaps with local metadata.
    pub bitmaps: u32,
    /// Bitmaps rebuilt as block trees (the rest stay pinned).
    pub relocated: u32,
    /// Pixel data copied to new raw resources.
    pub moved: Relocated,
}

/// Load-test variant "moved": `map` decomposed, every bitmap rebuilt as a
/// block tree and appended after the original metadata (so every one gets
/// a new address), and every local LOD's pixels copied to new raw data.
pub fn moved_variant<R: Read + Seek>(map: &mut Map<R>) -> Result<(TagSet, MovedSummary)> {
    let mut set = TagSet::from_map(map)?;
    set.reuse_free_space = false;
    let ids: Vec<TagId> = map
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"bitm") && t.location.is_some())
        .map(|t| t.id)
        .collect();
    let mut summary = MovedSummary::default();
    for id in ids {
        summary.bitmaps += 1;
        if let Some(r) = relocate_bitmap(&mut set, map, id, true)? {
            summary.relocated += 1;
            summary.moved.moved_lods += r.moved_lods;
            summary.moved.moved_bytes += r.moved_bytes;
        }
    }
    Ok((set, summary))
}

/// Load-test variant "fresh": every tag pinned where it was, but with the
/// index tail cleared (unless `keep_tail`) and a header holding only what
/// the builder writes plus the low-detail fields (unless `keep_header`).
/// Returns the set and how many nonzero tail bytes were cleared.
///
/// Salts stay retail: pinned tags hold datums in bytes no layout
/// describes, so they cannot be renumbered without full relocation.
pub fn fresh_variant<R: Read + Seek>(
    map: &mut Map<R>,
    keep_tail: bool,
    keep_header: bool,
) -> Result<(TagSet, usize)> {
    let mut set = TagSet::from_map(map)?;
    let pinned = set
        .pinned
        .as_mut()
        .ok_or(Error::Invalid("decomposed map has no pinned image"))?;
    let cleared = if keep_tail {
        0
    } else {
        pinned.clear_index_tail()
    };
    if !keep_header {
        pinned.clear_header();
    }
    Ok((set, cleared))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DecodedTag, TagData};
    use map::build::{Block, RawRef, TagPayload, TagSet};
    use map::{CacheType, FourCC, Map, RawPointer};
    use std::io::Cursor;

    /// An invented map: globals, scenario, and one bitmap with a sequence of
    /// two sprites and one image whose LOD 0 is local raw data. With
    /// `trailing`, the bitmap carries 4 bytes no reviewed record covers.
    fn fixture(trailing: bool) -> (Map<Cursor<Vec<u8>>>, map::TagId) {
        let mut set = TagSet::new(CacheType::Multiplayer, "bitmaps", "invented\\bitmaps");
        for g in [b"matg", b"scnr", b"bitm"] {
            set.add_group(FourCC(*g), None, None);
        }
        let globals = set
            .add_tag(
                FourCC(*b"matg"),
                "globals",
                TagPayload::Built(Block::zeroed(0x284)),
            )
            .unwrap();
        let scenario = set
            .add_tag(
                FourCC(*b"scnr"),
                "scenario",
                TagPayload::Built(Block::zeroed(0x3e0)),
            )
            .unwrap();
        set.globals = globals;
        set.scenario = scenario;
        let raw = set.add_raw((0..=255).collect(), 0x200);
        let mut sprites = Block::zeroed(0x40);
        sprites
            .put_u16(0, 0)
            .put_f32(8, 0.25)
            .put_u16(0x20, 0)
            .put_f32(0x28, 0.75);
        let mut sequence = Block::zeroed(0x3c);
        sequence.bytes[..5].copy_from_slice(b"label");
        sequence.put_u16(0x22, 1).block(0x34, 2, sprites);
        let mut image = Block::zeroed(0x74);
        image
            .put_u32(0, FourCC(*b"bitm").as_u32())
            .put_u16(4, 16)
            .put_u16(6, 16)
            .put_u32(0x34, 256)
            .raw(0x1c, RawRef::Local(raw));
        let mut header = Block::zeroed(if trailing { 0x50 } else { 0x4c });
        header
            .put_u16(0x18, 16)
            .block(0x3c, 1, sequence)
            .block(0x44, 1, image);
        let bitmap = set
            .add_tag(
                FourCC(*b"bitm"),
                "invented\\bitmap",
                TagPayload::Built(header),
            )
            .unwrap();
        let bytes = set.build().unwrap().0;
        (Map::from_reader(Cursor::new(bytes)).unwrap(), bitmap)
    }

    fn bitmap(map: &mut Map<Cursor<Vec<u8>>>, id: map::TagId) -> crate::Bitmap {
        match DecodedTag::read(map, id).unwrap().data {
            TagData::Bitmap(b) => *b,
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_bitmap_becomes_a_tree_that_moves_with_its_pixels() {
        let (mut map, id) = fixture(false);
        let before = bitmap(&mut map, id);
        let mut set = TagSet::from_map(&mut map).unwrap();
        let moved = relocate_bitmap(&mut set, &mut map, id, true)
            .unwrap()
            .unwrap();
        assert_eq!(
            moved,
            Relocated {
                moved_lods: 1,
                moved_bytes: 256
            }
        );
        set.reuse_free_space = false;
        let (bytes, layout) = set.build().unwrap();
        let mut rebuilt = Map::from_reader(Cursor::new(bytes)).unwrap();
        rebuilt.validate_checksum().unwrap();
        let old = map.tag(id).unwrap().location.unwrap();
        let new = rebuilt.tag(id).unwrap().location.unwrap();
        assert_ne!(old.address, new.address);
        assert_eq!(old.size, new.size);
        let after = bitmap(&mut rebuilt, id);
        assert_eq!(after.sequences.len(), 1);
        assert_eq!(after.sequences[0].sprites.len(), 2);
        assert_eq!(after.sequences[0].sprites[1].bounds[0], 0.75);
        assert_eq!(after.images.len(), 1);
        let lod = after.images[0].lods[0].unwrap();
        assert_eq!(lod.pointer, RawPointer::decode(layout.raw[0]).unwrap());
        assert_ne!(lod.pointer, before.images[0].lods[0].unwrap().pointer);
        assert_eq!(
            rebuilt
                .read_file_range(lod.pointer.offset.into(), 256)
                .unwrap(),
            (0..=255).collect::<Vec<u8>>()
        );
        assert_eq!(after.header.source_width, before.header.source_width);
    }

    #[test]
    fn a_bitmap_with_uncovered_bytes_stays_pinned() {
        let (mut map, id) = fixture(true);
        let mut set = TagSet::from_map(&mut map).unwrap();
        assert_eq!(relocate_bitmap(&mut set, &mut map, id, true).unwrap(), None);
        assert!(matches!(
            set.tag(id).unwrap().payload,
            TagPayload::Pinned { .. }
        ));
        assert!(set.raw.is_empty());
    }

    #[test]
    fn load_test_variants_build_and_read_back() {
        let (mut map, id) = fixture(false);
        let (moved, summary) = moved_variant(&mut map).unwrap();
        assert_eq!(summary.bitmaps, 1);
        assert_eq!(summary.relocated, 1);
        assert_eq!(summary.moved.moved_lods, 1);
        let mut rebuilt = Map::from_reader(Cursor::new(moved.build().unwrap().0)).unwrap();
        assert_ne!(
            rebuilt.tag(id).unwrap().location.unwrap().address,
            map.tag(id).unwrap().location.unwrap().address
        );
        assert_eq!(bitmap(&mut rebuilt, id).images.len(), 1);

        let (fresh, _) = fresh_variant(&mut map, false, false).unwrap();
        let bytes = fresh.build().unwrap().0;
        let mut rebuilt = Map::from_reader(Cursor::new(bytes)).unwrap();
        rebuilt.validate_checksum().unwrap();
        assert_eq!(
            rebuilt.read_tag_bytes(id).unwrap(),
            map.read_tag_bytes(id).unwrap()
        );
        // Keeping both gives the source back.
        let (same, cleared) = fresh_variant(&mut map, true, true).unwrap();
        assert_eq!(cleared, 0);
        let mut original = Vec::new();
        map.write_to(&mut original).unwrap();
        let built = same.build().unwrap().0;
        let diff = built.iter().zip(&original).position(|(a, b)| a != b);
        assert_eq!((diff, built.len()), (None, original.len()));
    }
}
