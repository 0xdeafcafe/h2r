use super::*;
use crate::{DecodedTag, TagData};
use map::CacheType;
use std::io::Cursor;

type M = Map<Cursor<Vec<u8>>>;

/// An invented map with a menu-picture-like bitmap: 6x3 linear
/// `ARGB8888`, kind 4, flags `linear | 0x280`, LOD bias 11.
fn fixture() -> (M, TagId) {
    let mut set = TagSet::new(CacheType::Multiplayer, "pictures", "invented\\pictures");
    for g in [b"matg", b"scnr", b"bitm"] {
        set.add_group(FourCC(*g), None, None);
    }
    set.globals = set
        .add_tag(
            FourCC(*b"matg"),
            "globals",
            TagPayload::Built(Block::zeroed(0x284)),
        )
        .unwrap();
    set.scenario = set
        .add_tag(
            FourCC(*b"scnr"),
            "scenario",
            TagPayload::Built(Block::zeroed(0x3e0)),
        )
        .unwrap();
    let raw = set.add_raw(vec![7; 128], 0x200);
    let mut image = Block::zeroed(IMAGE_LEN);
    image
        .put_u32(0, FourCC(*b"bitm").as_u32())
        .put_u16(4, 6)
        .put_u16(6, 3)
        .put_u16(0xc, FORMAT_ARGB8888)
        .put_u16(0xe, 0x290)
        .put_u32(0x34, 128)
        .raw(0x1c, RawRef::Local(raw));
    image.bytes[8] = 1;
    image.bytes[0x16] = 11;
    image.bytes[0x17] = 16;
    for w in at::NONE_WORDS {
        image.put_u32(w, u32::MAX);
    }
    let mut header = Block::zeroed(HEADER_LEN);
    header
        .put_u16(0, 4)
        .put_u16(2, 4)
        .put_u16(4, 1)
        .put_u16(0x18, 6)
        .put_u16(0x1a, 3)
        .block(0x3c, 0, Block::zeroed(0))
        .block(0x44, 1, image);
    let id = set
        .add_tag(FourCC(*b"bitm"), "ui\\picture0", TagPayload::Built(header))
        .unwrap();
    let bytes = set.build().unwrap().0;
    (Map::from_reader(Cursor::new(bytes)).unwrap(), id)
}

fn bitmap(map: &mut M, id: TagId) -> crate::Bitmap {
    match DecodedTag::read(map, id).unwrap().data {
        TagData::Bitmap(b) => *b,
        _ => unreachable!(),
    }
}

#[test]
fn rows_are_bgra_padded_to_64_bytes_and_the_lod_to_128() {
    let rgba: Vec<u8> = (0..3 * 2 * 4).map(|i| i as u8).collect();
    let out = argb8888_linear(3, 2, &rgba).unwrap();
    assert_eq!(out.len(), 128);
    assert_eq!(out[..4], [2, 1, 0, 3]);
    assert_eq!(out[8..12], [10, 9, 8, 11]);
    assert_eq!(out[12..64], [0; 52], "row padding");
    assert_eq!(out[64..68], [14, 13, 12, 15]);
    // The menu picture's size: 880-byte rows to 896, V 0x2d480 a LOD.
    let big = argb8888_linear(220, 207, &vec![0; 220 * 207 * 4]).unwrap();
    assert_eq!(big.len(), 0x2d480);
    assert!(argb8888_linear(3, 2, &rgba[1..]).is_err());
}

#[test]
fn a_new_bitmap_takes_the_template_s_shape_and_its_own_pixels() {
    let (mut map, template_id) = fixture();
    let template = Template::read(&mut map, template_id).unwrap();
    assert_eq!(template.size(), (6, 3));
    let original = map.read_tag_bytes(template_id).unwrap();

    // Onto the decomposed map, as the menu rewrite does.
    let mut set = TagSet::from_map(&mut map).unwrap();
    let p = set.pinned.as_mut().unwrap();
    p.clear_index_tail();
    p.index_entries_end = p.index.len() as u32;
    let rgba: Vec<u8> = (0..6 * 3)
        .flat_map(|i| [i as u8, 0x80, 255 - i as u8, 0xff])
        .collect();
    let id = install(&mut set, &template, "ui\\h1_a10", 6, 3, &rgba).unwrap();
    let mut out = Map::from_reader(Cursor::new(set.build().unwrap().0)).unwrap();
    out.validate_checksum().unwrap();

    // The template is untouched.
    assert_eq!(out.read_tag_bytes(template_id).unwrap(), original);
    let t = out.tag(id).unwrap().clone();
    assert_eq!(
        (t.group, t.name.as_deref()),
        (FourCC(*b"bitm"), Some("ui\\h1_a10"))
    );

    let b = bitmap(&mut out, id);
    assert_eq!(b.header.kind.0, 4);
    assert_eq!(b.header.source_width, 6);
    let img = &b.images[0];
    assert_eq!(img.dimensions, [6, 3, 1]);
    assert_eq!(img.wire.format.0, FORMAT_ARGB8888);
    assert_eq!(img.wire.flags.0, 0x290);
    assert_eq!((img.wire.lod_bias, img.wire.cache_usage), (11, 16));
    assert_eq!((img.wire.registration_x, img.wire.registration_y), (3, 1));
    assert_eq!(img.wire.runtime_id, id.0);
    assert_eq!(img.wire.lod_pointers[1..], [u32::MAX, u32::MAX]);
    assert!(b.sequences.is_empty());
    let lod = img.lods[0].unwrap();
    assert_eq!(lod.size, 256, "3 rows of 64 bytes, to 128");
    let px = out
        .read_file_range(u64::from(lod.pointer.offset), u64::from(lod.size))
        .unwrap();
    assert_eq!(px, argb8888_linear(6, 3, &rgba).unwrap());
}

#[test]
fn a_template_must_be_a_bitmap_with_an_image() {
    let (mut map, template_id) = fixture();
    let scenario = map.scenario().unwrap().id;
    assert!(Template::read(&mut map, scenario).is_err());
    let mut t = Template::read(&mut map, template_id).unwrap();
    t.image.pop();
    let mut set = TagSet::new(CacheType::Multiplayer, "x", "x\\x");
    assert!(install(&mut set, &t, "x", 1, 1, &[0; 4]).is_err());
}
