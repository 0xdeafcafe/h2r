use super::*;
use engine::testing::FakeGuest;
use guest_layout::game::GameOptions;
use map::build::{Block, BspField, FixupKind, TagPayload, TagSet};

#[test]
fn only_active_local_offline_multiplayer_is_admitted() {
    use bots::lobby;
    let mut guest = FakeGuest::new();
    let globals = 0x10000;
    let sim = 0x20000;
    let squad = 0x30000;
    let mode = GameOptions::MODE.at(globals + guest_layout::game::OPTIONS);
    assert!(!admitted(&guest));
    guest
        .put_u32(global::GAME_GLOBALS.base, globals)
        .put_u8(GameGlobals::MAP_ACTIVE.at(globals), 1)
        .put_u32(mode, MODE_MULTIPLAYER)
        .put_u32(global::SIM_GLOBALS.base, sim)
        .put_u32(sim + 8, 1)
        .put_u8(lobby::NETWORKING, 0);
    assert!(admitted(&guest));
    assert!(Loaded::read(&guest, true).unwrap_err().contains("Theater"));
    guest.put_u32(mode, guest_layout::game::MODE_CAMPAIGN);
    assert!(!admitted(&guest));
    guest.put_u32(mode, MODE_MULTIPLAYER);
    for net_mode in 2..=5 {
        guest.put_u32(sim + 8, net_mode);
        assert!(!admitted(&guest));
    }
    guest
        .put_u32(sim + 8, 1)
        .put_u8(lobby::NETWORKING, 1)
        .put_u32(lobby::SESSION_TYPE, 3)
        .put_u32(lobby::SESSION, squad)
        .put_u32(squad + lobby::STATE, 8);
    for class in [1, 2] {
        guest.put_u32(squad + crate::variant_ext::SESSION_NETWORK_CLASS, class);
        assert!(!admitted(&guest));
    }
    guest.put_u32(squad + crate::variant_ext::SESSION_NETWORK_CLASS, 0);
    assert!(admitted(&guest));
    guest.put_u32(lobby::SESSION_TYPE, 8);
    assert!(!admitted(&guest));
}

/// Build a multiplayer cache with two invented scenery tags. Only the one
/// referenced from the scenario palette can be materialised by F2.
fn map_bytes() -> (Vec<u8>, TagId) {
    let mut set = TagSet::new(CacheType::Multiplayer, "workshop", "test\\workshop");
    for group in [b"matg", b"scnr", b"scen", b"sbsp"] {
        set.add_group(FourCC(*group), None, None);
    }
    set.globals = set
        .add_tag(
            FourCC(*b"matg"),
            "test\\globals",
            TagPayload::Built(Block::zeroed(4)),
        )
        .unwrap();
    let scenario = set
        .add_tag(FourCC(*b"scnr"), "test\\workshop", TagPayload::Empty)
        .unwrap();
    let mut palette = Block::zeroed(0x28);
    let item = set
        .add_tag(
            FourCC(*b"scen"),
            "test\\shelf",
            TagPayload::Built(Block::zeroed(4)),
        )
        .unwrap();
    set.add_tag(
        FourCC(*b"scen"),
        "test\\loose",
        TagPayload::Built(Block::zeroed(4)),
    )
    .unwrap();
    palette.tag_ref(0, item);
    let mut cluster = Block::zeroed(tags::structure::Cluster::SIZE);
    cluster.put_u32(0x28, u32::MAX);
    for (i, bound) in [-2.0, 2.0, -4.0, 4.0, -8.0, 8.0].into_iter().enumerate() {
        cluster.put_f32(0x54 + 4 * i, bound);
    }
    let mut structure = Block::zeroed(0x200);
    structure.block(0x9c, 1, cluster);
    let bsp = set
        .add_tag(
            FourCC(*b"sbsp"),
            "test\\structure",
            TagPayload::Built(structure),
        )
        .unwrap();
    let section = set.add_bsp(bsp, None);
    let mut descriptor = Block::zeroed(0x44);
    for (offset, field) in [
        (0, BspField::FileOffset),
        (4, BspField::Size),
        (8, BspField::Address),
    ] {
        descriptor.fixup(offset, FixupKind::Bsp { section, field });
    }
    descriptor.tag_id(0x14, bsp).tag_id(0x1c, TagId::NULL);
    let mut root = Block::zeroed(0x3e0);
    root.block(0x210, 1, descriptor).block(0x58, 1, palette);
    set.tag_mut(scenario).unwrap().payload = TagPayload::Built(root);
    set.scenario = scenario;
    (set.build().unwrap().0, item)
}

#[test]
fn opening_binds_the_selected_files_hash_palette_and_budget() {
    let (bytes, item) = map_bytes();
    let checksum = Map::from_reader(Cursor::new(&bytes))
        .unwrap()
        .header()
        .checksum;
    let mut loaded = Loaded {
        name: "workshop".into(),
        base_name: "workshop".into(),
        author: "Editor Test".into(),
        checksum,
        path: "invented".into(),
        scenario: 1,
        free_slots: 17,
    };
    let opened = decode(&bytes, &loaded).unwrap();
    assert_eq!(opened.tags, [item.0]);
    assert_eq!(opened.session.palette()[0].tag.path, "test\\shelf");
    assert_eq!(opened.session.document().base.sha256, Sha256::of(&bytes));
    assert_eq!(opened.session.budget(), 17);
    assert_eq!(opened.walls.len(), 1);
    loaded.checksum ^= 1;
    assert!(
        decode(&bytes, &loaded)
            .err()
            .unwrap()
            .to_string()
            .contains("differ")
    );
}
