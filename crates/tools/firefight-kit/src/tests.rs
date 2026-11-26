//! End-to-end fixtures. No input files, borrowed payloads or known disc hashes.
use super::*;
use crate::testkit::{self, STARTS};
use content::Library;
use h1campaign::ai::h2::ScenarioAi;
use std::fs;
use tags::pathfinding::{PathfindingData, Ref};

fn start_teams(map: &mut Map<Cursor<&[u8]>>) -> Vec<i16> {
    let id = map.scenario().unwrap().id;
    let root = map.read_tag_bytes(id).unwrap();
    scenario_edit::scene::metadata_block(map, &root, 0x100, 0x34)
        .unwrap()
        .iter()
        .map(|row| i16::from_le_bytes([row[0x10], row[0x11]]))
        .collect()
}

fn fixture() -> Vec<u8> {
    testkit::court()
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
    let config = Config::default();
    let first = build_package(&base, "court", Mode::Kit, &config, &dir, "one", false).unwrap();
    let second = build_package(&base, "court", Mode::Kit, &config, &dir, "two", false).unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(first.costs, second.costs);
    assert_eq!(first.bases, second.bases);
    assert_eq!(first.bases.source, Source::FlagStands);
    assert_eq!(first.bases.points, [[2., 14., 0.], [26., 14., 0.]]);
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
    // Sixteen slots and the Elite's class record.
    assert_eq!(ai.squads.len(), 17);
    assert_eq!(ai.squads[16].name, "ff_class_elite");
    // The drop zones, then one zone per base.
    assert_eq!(ai.zones.len(), first.zones + 2);
    assert!((6..=12).contains(&first.zones));
    for (i, zone) in ai.zones.iter().enumerate() {
        let want = match i.checked_sub(first.zones) {
            None => format!("ff_drop_{i:02}"),
            Some(b) => format!("ff_base_{b:02}"),
        };
        assert_eq!(zone.name, want);
    }
    let orders: Vec<_> = ai.orders.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(orders, ["ff_hunt_00", "ff_hunt_01"]);
    assert_eq!(start_teams(&mut map), [0, 1, 0]);
    assert!(
        ai.squads
            .iter()
            .all(|s| s.normal_count == 0 && s.insane_count == 0)
    );
    assert_eq!(ai.character_palette, [map.tag(ai::ELITE_PATH).unwrap().id]);
    // Each floor has 51 walkable surfaces: the arena's 49, an island off
    // its east edge and a floor no start reaches.
    assert_eq!(first.costs.sectors_generated, 2 * 51);
    assert!(first.costs.padding < 0x1100, "{}", first.costs);
    let mut kept = 0;
    for index in 0..2 {
        let name = format!("court_{index}");
        let bsp = map.tag(name.as_str()).unwrap().id;
        let sectors = PathfindingData::read(&mut map, bsp).unwrap().unwrap();
        assert_eq!(sectors.checksum, 100 + index);
        assert!(sectors.validate(Some(51)).is_empty());
        // The far floor has no sector on either BSP. The arena's BSP keeps
        // the arena alone; the other keeps what the starts reach.
        assert_eq!(sectors.refs[50], Ref::NONE);
        if index == 0 {
            assert_eq!(sectors.sectors.len(), 49);
        } else {
            assert!((49..=50).contains(&sectors.sectors.len()));
        }
        kept += sectors.sectors.len();
    }
    assert_eq!(first.costs.sectors_kept, kept);
    // Every firing position names a kept sector.
    for zone in &ai.zones {
        for p in &zone.firing_positions {
            assert!(p.sector < 49, "{}: sector {}", zone.name, p.sector);
        }
    }
    let probe = build_package(&base, "court", Mode::Probe, &config, &dir, "probe", false).unwrap();
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
    let error = build(&full, "court", Mode::Kit, &Config::default())
        .err()
        .expect("must refuse")
        .to_string();
    for part in [
        "exceeds tag headroom by ",
        "the biggest part is ",
        "of 102 generated",
        "tag index ",
        "padding ",
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
    let error = build(&base, "court", Mode::Probe, &Config::default())
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("required Elite biped"), "{error}");
}

#[test]
fn the_kit_writes_each_start_its_base_team() {
    let base = fixture();
    let mut map = Map::from_reader(Cursor::new(&base[..])).unwrap();
    assert_eq!(start_teams(&mut map), [0, 1, 8]);
    for (home, teams) in [(Team::Red, [0, 1, 0]), (Team::Blue, [1, 0, 0])] {
        let built = build(&base, "court", Mode::Kit, &Config { home }).unwrap();
        let mut map = Map::from_reader(Cursor::new(&built.bytes[..])).unwrap();
        // The middle start is as near one base as the other: it takes
        // the home base.
        assert_eq!(start_teams(&mut map), teams, "{home:?}");
        assert_eq!(built.bases.home, home);
        // The other start fields are the base's.
        let id = map.scenario().unwrap().id;
        let root = map.read_tag_bytes(id).unwrap();
        let rows = scenario_edit::scene::metadata_block(&mut map, &root, 0x100, 0x34).unwrap();
        for (row, (x, y, _)) in rows.iter().zip(STARTS) {
            assert_eq!(row[..4], x.to_le_bytes());
            assert_eq!(row[4..8], y.to_le_bytes());
        }
    }
    let red = build(&base, "court", Mode::Kit, &Config::default()).unwrap();
    let blue = build(&base, "court", Mode::Kit, &Config { home: Team::Blue }).unwrap();
    assert_eq!(
        blue.bases.points,
        [red.bases.points[1], red.bases.points[0]]
    );
    assert_ne!(red.bytes, blue.bytes);
}

#[test]
fn a_live_index_tail_moves_only_the_group_table() {
    // Retail maps have data right after their tag entries: the new
    // character's entry cannot grow the table in place.
    let base = fixture();
    let mut map = Map::from_reader(Cursor::new(&base)).unwrap();
    let mut set = TagSet::from_map(&mut map).unwrap();
    let pin = set.pinned.as_mut().unwrap();
    let end = pin.index_entries_end as usize;
    pin.index[end] = 0x5a;
    let base = set.build().unwrap().0;
    let built = build(&base, "court", Mode::Kit, &Config::default()).unwrap();
    // Five groups with `char`, 12 bytes each, instead of 16 bytes a tag.
    assert_eq!(built.costs.index, 5 * 12);
    let mut map = Map::from_reader(Cursor::new(&built.bytes[..])).unwrap();
    map.validate_checksum().unwrap();
    assert_eq!(map.groups().len(), 5);
    let ai = ScenarioAi::read_map(&mut map).unwrap();
    assert_eq!(ai.character_palette, [map.tag(ai::ELITE_PATH).unwrap().id]);
    assert_eq!(ai.squads.len(), 17);
}
