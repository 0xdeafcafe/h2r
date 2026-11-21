//! End-to-end fixtures. No input files, borrowed payloads or known disc hashes.
use super::*;
use content::Library;
use h1campaign::ai::h2::ScenarioAi;
use map::build::{BspField, FixupKind};
use std::fs;
use tags::pathfinding::PathfindingData;

fn fixture() -> Vec<u8> {
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
        let mut floor = nav::test_floor();
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
    let mut start = Block::zeroed(0x34);
    start.put_f32(0, 14.).put_f32(4, 14.);
    root.block(0x100, 1, start).block(0x210, 2, descriptors);
    set.tag_mut(scenario).unwrap().payload = TagPayload::Built(root);
    set.build().unwrap().0
}

#[test]
fn two_builds_of_one_map_are_byte_equal() {
    let base = fixture();
    let dir = std::env::temp_dir().join(format!(
        "ff-kit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let first = build_package(&base, "court", Mode::Kit, &dir, "one", false).unwrap();
    let second = build_package(&base, "court", Mode::Kit, &dir, "two", false).unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(first.costs, second.costs);
    let lib = Library::open(&dir);
    assert_eq!(lib.packages().len(), 2);
    let manifest = &lib.packages()[0].manifest;
    assert_eq!(manifest.base.sha256, Some(Sha256::of(&base)));
    assert_eq!(manifest.patches.len(), 1);
    for file in ["manifest.toml", &manifest.patches[0].file] {
        assert_eq!(
            fs::read(dir.join("one").join(file)).unwrap(),
            fs::read(dir.join("two").join(file)).unwrap()
        );
    }
    let result = lib
        .resolve_package(&lib.packages()[0], &mut &base[..])
        .unwrap();
    let bytes = result.read_all(&mut &base[..]).unwrap();
    let mut map = Map::from_reader(Cursor::new(&bytes[..])).unwrap();
    map.validate_checksum().unwrap();
    assert!(
        map.strings()
            .iter()
            .flatten()
            .any(|s| s == &format!("firefight_kit_v{KIT_VERSION}"))
    );
    let ai = ScenarioAi::read_map(&mut map).unwrap();
    assert_eq!(ai.squads.len(), first.squads);
    assert_eq!(ai.squads.len(), 16);
    assert_eq!(ai.zones.len(), first.zones);
    assert!((6..=12).contains(&first.zones));
    assert!(
        ai.squads
            .iter()
            .all(|s| s.normal_count == 0 && s.insane_count == 0)
    );
    assert_eq!(ai.character_palette, [map.tag(ai::ELITE_PATH).unwrap().id]);
    for index in 0..2 {
        let name = format!("court_{index}");
        let bsp = map.tag(name.as_str()).unwrap().id;
        let sectors = PathfindingData::read(&mut map, bsp).unwrap().unwrap();
        assert_eq!(sectors.checksum, 100 + index);
        assert!(!sectors.sectors.is_empty());
        assert!(sectors.validate(None).is_empty());
    }
    let probe = build_package(&base, "court", Mode::Probe, &dir, "probe", false).unwrap();
    assert_ne!(probe.id, first.id);
    let lib = Library::open(&dir);
    let p = lib.packages().iter().find(|p| p.id == probe.id).unwrap();
    let bytes = lib
        .resolve_package(p, &mut &base[..])
        .unwrap()
        .read_all(&mut &base[..])
        .unwrap();
    let mut map = Map::from_reader(Cursor::new(&bytes[..])).unwrap();
    let ai = ScenarioAi::read_map(&mut map).unwrap();
    assert_eq!(ai.squads.len(), 1);
    assert_eq!(ai.squads[0].normal_count, 2);
    assert_eq!(
        hs::Scenario::read(&mut map)
            .unwrap()
            .script("ff_probe_place")
            .unwrap()
            .kind(),
        0
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_map_without_headroom_is_refused_with_the_cost() {
    let base = fixture();
    let mut map = Map::from_reader(Cursor::new(&base)).unwrap();
    let mut set = TagSet::from_map(&mut map).unwrap();
    let max = max_metadata_size(CacheType::Multiplayer).unwrap();
    let pin = set.pinned.as_mut().unwrap();
    pin.metadata_size += max - map.header().meta_size;
    let (full, layout) = set.build().unwrap();
    assert_eq!(layout.memory_size, max);
    let error = build(&full, "court", Mode::Kit)
        .err()
        .expect("must refuse")
        .to_string();
    for part in [
        "exceeds tag headroom",
        "sectors ",
        "zones ",
        "squads ",
        "characters and palettes ",
        "available 0 B",
    ] {
        assert!(error.contains(part), "{error}");
    }
    assert!(!error.contains("tag growth 0 B"), "{error}");
}

#[test]
fn a_base_without_its_own_elite_is_refused() {
    let base = fixture();
    let mut map = Map::from_reader(Cursor::new(base)).unwrap();
    let mut set = TagSet::from_map(&mut map).unwrap();
    let id = map.tag(r"objects\characters\elite\elite_mp").unwrap().id;
    set.slots[id.index()].name = Some(r"invented\other_unit".into());
    let base = set.build().unwrap().0;
    let error = build(&base, "court", Mode::Probe)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("required Elite biped"), "{error}");
}
