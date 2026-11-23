//! An invented multiplayer map the kit builds on, for this crate's tests
//! and other crates' through the `testkit` feature: the host data bundle
//! (crates/live/host-data) and the server-map strip (crates/apps/cli). No
//! input files, borrowed payloads or known disc hashes.
use map::{
    CacheType, FourCC, TagId,
    build::{Block, BspField, FixupKind, TagPayload, TagSet},
};

use crate::{Config, Mode};

/// The map's name, in its header and as the kit's base name.
pub const NAME: &str = "court";

/// Invented starts: one by each flag stand on its team, one untagged in the
/// middle.
pub const STARTS: [(f32, f32, i16); 3] = [(3., 13., 0), (25., 15., 1), (14., 14., 8)];

/// `court`: two structure BSPs over the same 7x7 floor, three starts, a
/// CTF flag stand at each end, and the Elite biped the kit needs.
pub fn court() -> Vec<u8> {
    let mut set = TagSet::new(CacheType::Multiplayer, "court", r"invented\court");
    for group in [b"scnr", b"bipd", b"weap", b"sbsp"] {
        set.add_group(FourCC(*group), None, None);
    }
    // Make the scenario the metadata anchor, so moving it would break the map.
    let scenario = set
        .add_tag(FourCC(*b"scnr"), "court", TagPayload::Empty)
        .unwrap();
    set.scenario = scenario;
    for (group, path) in [
        (b"bipd", r"objects\characters\elite\elite_mp"),
        (b"weap", r"objects\weapons\rifle\plasma_rifle\plasma_rifle"),
    ] {
        set.add_tag(FourCC(*group), path, TagPayload::Built(Block::zeroed(16)))
            .unwrap();
    }

    let mut descriptors = Block::zeroed(2 * 0x44);
    for i in 0..2u32 {
        let mut floor = crate::nav::fixture::test_floor();
        floor.put_u32(8, 100 + i);
        let id = set
            .add_tag(
                FourCC(*b"sbsp"),
                &format!("court_{i}"),
                TagPayload::Built(floor),
            )
            .unwrap();
        let section = set.add_bsp(id, None);
        let d = i * 0x44;
        for (offset, field) in [
            (0, BspField::FileOffset),
            (4, BspField::Size),
            (8, BspField::Address),
        ] {
            descriptors.fixup(d + offset, FixupKind::Bsp { section, field });
        }
        descriptors
            .tag_id(d + 0x14, id)
            .tag_id(d + 0x1c, TagId::NULL);
    }
    let mut root = Block::zeroed(0x3e0);
    let mut starts = Block::zeroed(3 * 0x34);
    for (i, (x, y, team)) in STARTS.into_iter().enumerate() {
        let at = i * 0x34;
        starts
            .put_f32(at, x)
            .put_f32(at + 4, y)
            .put_u16(at + 0x10, team as u16);
    }
    // A CTF flag stand at each end, and a flag of another type.
    let mut flags = Block::zeroed(3 * 0x20);
    for (i, (x, kind, team)) in [(2., 0u16, 0u16), (14., 1, 0), (26., 0, 1)]
        .into_iter()
        .enumerate()
    {
        let at = i * 0x20;
        flags
            .put_f32(at, x)
            .put_f32(at + 4, 14.)
            .put_u16(at + 0x10, kind)
            .put_u16(at + 0x12, team);
    }
    root.block(0x100, 3, starts)
        .block(0x118, 3, flags)
        .block(0x210, 2, descriptors);
    set.tag_mut(scenario).unwrap().payload = TagPayload::Built(root);
    set.build().unwrap().0
}

/// [`court`] with the stage-one kit built on it (red home): the whole map
/// the kit package resolves to.
pub fn court_kit() -> Vec<u8> {
    crate::build(&court(), NAME, Mode::Kit, &Config::default())
        .expect("the kit builds on court")
        .bytes
}
