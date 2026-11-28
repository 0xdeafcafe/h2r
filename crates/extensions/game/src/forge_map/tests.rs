use super::*;
use guest_layout::testing::FakeGuest;
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
    crate::lobby_start::set_mode(crate::lobby_start::Mode::Forge);
    guest.put_u32(sim + 8, 4)
        .put_u32(squad + crate::variant_ext::SESSION_NETWORK_CLASS, 2)
        .put_u32(squad + bots::phantom::session::at::PEER_COUNT, 1)
        .put_u32(squad + bots::phantom::session::at::LOCAL_PEER, 0);
    assert!(admitted(&guest), "solo Live Forge host");
    guest.put_u32(sim + 8, 5);
    assert!(!admitted(&guest), "a distributed client");
    crate::lobby_start::clear();
    guest.put_u32(sim + 8, 1);
    guest.put_u32(lobby::SESSION_TYPE, 8);
    assert!(!admitted(&guest));
}

/// An invented scenery definition header: type 6, the radius, a centre a
/// quarter unit up and room for the model reference.
fn scenery(radius: f32) -> Block {
    let mut def = Block::zeroed(0xbc);
    def.put_u16(0, 6).put_f32(4, radius).put_f32(0x10, 0.25);
    def
}

/// Build a multiplayer cache with two invented scenery tags. Only the one
/// referenced from the scenario palette can be materialised by F2.
fn map_bytes() -> (Vec<u8>, TagId) {
    let mut set = TagSet::new(CacheType::Multiplayer, "workshop", "test\\workshop");
    for group in [b"matg", b"scnr", b"scen", b"sbsp", b"hlmt"] {
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
    let mut palette = Block::zeroed(0x28 * 3);
    let model = set
        .add_tag(
            FourCC(*b"hlmt"),
            "test\\shelf_model",
            TagPayload::Built(Block::zeroed(4)),
        )
        .unwrap();
    let mut shelf = scenery(0.75);
    shelf.tag_ref(0x34, model);
    let item = set
        .add_tag(FourCC(*b"scen"), "test\\shelf", TagPayload::Built(shelf))
        .unwrap();
    // A definition with no model, and one whose type is not scenery's.
    let mut unseen = scenery(0.5);
    unseen.tag_ref(0x34, TagId::NULL);
    let unseen = set
        .add_tag(FourCC(*b"scen"), "test\\unseen", TagPayload::Built(unseen))
        .unwrap();
    let mut wrong = scenery(0.5);
    wrong.put_u16(0, 2).tag_ref(0x34, model);
    let wrong = set
        .add_tag(FourCC(*b"scen"), "test\\wrong", TagPayload::Built(wrong))
        .unwrap();
    palette.tag_ref(0x28, unseen).tag_ref(0x50, wrong);
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
    root.block(0x210, 1, descriptor).block(0x58, 3, palette);
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
    let opened = decode(&bytes, &loaded, None).unwrap();
    assert_eq!(opened.tags, [item.0]);
    assert_eq!(opened.session.palette().len(), 1);
    assert_eq!(opened.session.palette()[0].tag.path, "test\\shelf");
    assert_eq!(opened.extents[0].radius, 0.75);
    assert_eq!(opened.extents[0].offset, [0., 0., 0.25]);
    assert_eq!(
        opened.refused,
        [
            "test\\unseen: it has no model, so it would be invisible",
            "test\\wrong: its definition is not a scen object",
        ]
    );
    assert_eq!(opened.session.document().base.sha256, Sha256::of(&bytes));
    assert_eq!(opened.session.budget(), 17);
    assert_eq!(opened.walls.len(), 1);
    assert!(!opened.session.has_kit(), "an original map has no kit");
    // A kit package's base is the installed map it was built on.
    let installed = Sha256::of(b"the installed map");
    let kit = decode(&bytes, &loaded, Some(installed)).unwrap();
    assert_eq!(kit.session.document().base.sha256, installed);
    loaded.checksum ^= 1;
    assert!(
        decode(&bytes, &loaded, None)
            .err()
            .unwrap()
            .to_string()
            .contains("differ")
    );
}

#[test]
fn a_palette_entry_must_be_a_visible_object_of_its_own_group() {
    let scen = FourCC(*b"scen");
    let mut def = vec![0u8; 0xbc];
    def[0] = 6;
    def[4..8].copy_from_slice(&1.5f32.to_le_bytes());
    def[0x38..0x3c].copy_from_slice(&0xe001_0002u32.to_le_bytes());
    let (extent, model) = placeable(BlockKind::Scenery, scen, &def).unwrap();
    assert_eq!((extent.radius, model), (1.5, TagId(0xe001_0002)));
    let refusal = |block, group, def: &[u8]| placeable(block, group, def).unwrap_err();
    assert!(refusal(BlockKind::SoundScenery, FourCC(*b"ssce"), &def).contains("sound scenery"));
    assert!(refusal(BlockKind::Scenery, scen, &def[..0x20]).contains("too short"));
    let mut bad = def.clone();
    bad[4..8].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(refusal(BlockKind::Scenery, scen, &bad).contains("bounding sphere"));
    bad[4..8].copy_from_slice(&(-1f32).to_le_bytes());
    assert!(refusal(BlockKind::Scenery, scen, &bad).contains("bounding sphere"));
    let mut bad = def.clone();
    bad[0x38..0x3c].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(refusal(BlockKind::Scenery, scen, &bad).contains("no model"));
    assert!(refusal(BlockKind::Vehicles, FourCC(*b"vehi"), &def).contains("not a vehi object"));
}

/// An item collection's payload at 0x1000: its entries block, then the
/// entries, each a weight and an item of `groups`.
fn collection(groups: &[&[u8; 4]]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend((groups.len() as u32).to_le_bytes());
    b.extend(0x1008u32.to_le_bytes());
    for g in groups {
        b.extend(1f32.to_le_bytes());
        b.extend(u32::from_be_bytes(**g).to_le_bytes());
        b.extend(0xe000_0000u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
    }
    b
}

#[test]
fn a_collection_holds_a_weapon_when_any_entry_is_one() {
    let holds = |groups: &[&[u8; 4]]| holds_weapon(&collection(groups), 0x1000).unwrap();
    assert!(holds(&[b"weap", b"weap"]));
    assert!(holds(&[b"eqip", b"weap"]), "a mixed collection");
    assert!(!holds(&[b"eqip"]), "grenades");
    assert!(!holds(&[]));
    // An entries block past the payload does not read.
    let mut bad = collection(&[b"weap"]);
    bad[4..8].copy_from_slice(&0x2000u32.to_le_bytes());
    assert!(holds_weapon(&bad, 0x1000).is_err());
}
