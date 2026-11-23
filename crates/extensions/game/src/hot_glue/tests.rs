//! The metadata overlay over a synthetic map: invented groups, names and
//! bytes, built by `map::build`. No game data.

use std::io::Cursor;

use map::build::{BspField, Block, FixupKind, TagPayload, TagSet};
use map::{CacheType, FourCC, Map, TagId};

use super::*;

struct Tiny {
    bytes: Vec<u8>,
    thing: TagId,
    bsp: TagId,
}

/// A map with globals, a scenario, a structure BSP in its own region and
/// a tag of an invented group in the ordinary metadata.
fn tiny() -> Tiny {
    let cc = |s: &[u8; 4]| FourCC(*s);
    let mut set = TagSet::new(CacheType::Multiplayer, "tiny", "invented\\scenarios\\tiny");
    for g in [b"matg", b"scnr", b"sbsp", b"thng"] {
        set.add_group(cc(g), None, None);
    }
    set.strings = vec![Some(String::new())];
    let globals = set
        .add_tag(cc(b"matg"), "globals\\globals", TagPayload::Built(Block::zeroed(0x40)))
        .unwrap();
    let scenario = set.add_tag(cc(b"scnr"), "invented\\scenarios\\tiny", TagPayload::Empty).unwrap();
    let bsp = set
        .add_tag(cc(b"sbsp"), "invented\\bsp", TagPayload::Built(Block::new((0u8..0x40).collect())))
        .unwrap();
    let thing = set
        .add_tag(cc(b"thng"), "invented\\thing", TagPayload::Built(Block::new(vec![0x11; 0x30])))
        .unwrap();
    let section = set.add_bsp(bsp, None);
    let mut descriptor = Block::zeroed(0x44);
    for (at, field) in [(0, BspField::FileOffset), (4, BspField::Size), (8, BspField::Address)] {
        descriptor.fixup(at, FixupKind::Bsp { section, field });
    }
    descriptor.tag_id(0x14, bsp);
    let mut scen = Block::zeroed(0x3e0);
    scen.block(0x210, 1, descriptor);
    set.tag_mut(scenario).unwrap().payload = TagPayload::Built(scen);
    set.scenario = scenario;
    set.globals = globals;
    let (bytes, _) = set.build().unwrap();
    Tiny { bytes, thing, bsp }
}

/// The map as a live open reads it: through `o`.
fn open(bytes: &[u8], o: &vfs::RawOverlay) -> Map<vfs::OverlaidReader<Cursor<Vec<u8>>>> {
    let reader = vfs::OverlaidReader::new(Cursor::new(bytes.to_vec()), o.clone(), "tiny", 0).unwrap();
    Map::from_reader(reader).unwrap()
}

fn address(map: &Map<impl Read + Seek>, tag: TagId) -> (u32, u64) {
    let loc = map.tag(tag).unwrap().location.unwrap();
    (loc.address, loc.file_offset().unwrap())
}

#[test]
fn a_map_opened_after_a_write_reads_it_and_a_revert_restores_the_files() {
    let t = tiny();
    let file = Map::from_reader(Cursor::new(t.bytes.clone())).unwrap();
    let spaces = Spaces::of(&file);
    let (at, _) = address(&file, t.thing);
    let o = vfs::RawOverlay::new();
    // Guest memory holds the write; the notice's span is copied out.
    assert_eq!(overlay_metadata(&o, "tiny", &spaces, t.thing.0, &[(at + 4, vec![9, 9, 9, 9])]), 1);
    assert!(!o.holds_raw(), "a metadata copy refuses no snapshot");
    let mut edited = open(&t.bytes, &o);
    let bytes = edited.read_tag_bytes(t.thing).unwrap();
    assert_eq!(&bytes[..10], &[0x11, 0x11, 0x11, 0x11, 9, 9, 9, 9, 0x11, 0x11]);
    // A revert: guest memory holds the old bytes again.
    overlay_metadata(&o, "tiny", &spaces, t.thing.0, &[(at + 4, vec![0x11; 4])]);
    assert_eq!(open(&t.bytes, &o).read_tag_bytes(t.thing).unwrap(), vec![0x11; 0x30]);
    // Another salt of the same slot is the same tag, as the native maps match.
    let other_salt = (t.thing.0 & 0xffff) | 0x1234_0000;
    assert_eq!(overlay_metadata(&o, "tiny", &spaces, other_salt, &[(at, vec![7])]), 1);
}

#[test]
fn a_bsp_region_write_lands_at_that_regions_offset() {
    let t = tiny();
    let file = Map::from_reader(Cursor::new(t.bytes.clone())).unwrap();
    let spaces = Spaces::of(&file);
    let (at, offset) = address(&file, t.bsp);
    assert_ne!(
        file.tag(t.bsp).unwrap().location.unwrap().space,
        file.metadata_space().unwrap(),
        "the BSP has its own region"
    );
    assert_eq!(spaces.offset(t.bsp.0, at + 8, 2), Some(offset + 8));
    let o = vfs::RawOverlay::new();
    overlay_metadata(&o, "tiny", &spaces, t.bsp.0, &[(at + 8, vec![0xee, 0xef])]);
    assert_eq!(o.summary().metadata_bytes, 2);
    let bytes = open(&t.bytes, &o).read_tag_bytes(t.bsp).unwrap();
    let mut want: Vec<u8> = (0u8..0x40).collect();
    want[8] = 0xee;
    want[9] = 0xef;
    assert_eq!(bytes, want);
}

#[test]
fn a_span_outside_its_tags_region_is_left_out() {
    let t = tiny();
    let file = Map::from_reader(Cursor::new(t.bytes.clone())).unwrap();
    let spaces = Spaces::of(&file);
    let o = vfs::RawOverlay::new();
    // An address in the edit arena: no file holds it.
    let arena = hot_tags::arena::WINDOW.start;
    assert_eq!(overlay_metadata(&o, "tiny", &spaces, t.thing.0, &[(arena, vec![1, 2])]), 0);
    assert!(o.is_empty());
}
