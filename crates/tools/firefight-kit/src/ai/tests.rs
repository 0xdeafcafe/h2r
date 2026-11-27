use super::*;
use std::io::Cursor;

use h1campaign::ai::h2::{Area, Character, FiringPosition};
use map::build::{TagPayload, TagSet};
use tags::geometry::TagView;

fn zone() -> Zone {
    let firing_positions = [[1.0, 2.0, 0.0], [2.0, 2.0, 0.0]]
        .into_iter()
        .enumerate()
        .map(|(i, position)| FiringPosition {
            position,
            reference_frame: -1,
            flags: 0,
            area: 0,
            cluster: 0,
            sector: i as u32,
            normal: [0.0, std::f32::consts::FRAC_PI_2],
        })
        .collect();
    let mut clusters = [0; 16];
    clusters[0] = 1;
    Zone {
        name: "invented_edge".into(),
        flags: 1,
        manual_bsp: 0,
        firing_positions,
        areas: vec![Area {
            name: "approach".into(),
            flags: 0,
            mean: [1.5, 2.0, 0.0],
            reference_frame: -1,
            deviation: 0.5,
            first: 0,
            count: 2,
            clusters,
            manual_reference_frame: -1,
        }],
    }
}

/// Base N's zone: the invented zone moved along x, one area.
fn base(n: usize) -> Zone {
    let mut z = zone();
    z.name = format!("ff_base_{n:02}");
    let dx = 40.0 * (n + 1) as f32;
    for p in &mut z.firing_positions {
        p.position[0] += dx;
    }
    z.areas[0].name = "base".into();
    z.areas[0].mean[0] += dx;
    z
}

/// Both modes start with a ported multiplayer fixture. Its unrelated script
/// and the previously placed weapon keep their original indices.
fn kit_map(probe: bool) -> Map<Cursor<Vec<u8>>> {
    kit_map_with(probe, &[])
}

/// [`kit_map`] with these classes after the Elite: each a `char` tag
/// named after it.
fn kit_map_with(probe: bool, classes: &[&str]) -> Map<Cursor<Vec<u8>>> {
    let mut set = TagSet::new(map::CacheType::Multiplayer, "arena", "invented\\arena");
    for group in [b"scnr", b"char", b"bipd", b"weap"] {
        set.add_group(FourCC(*group), None, None);
    }
    let mut add = |group, path| {
        set.add_tag(FourCC(group), path, TagPayload::Built(Block::zeroed(4)))
            .unwrap()
    };
    add(*b"bipd", ELITE_UNIT);
    add(*b"weap", WEAPONS[0]);
    let placed = add(*b"weap", r"invented\sprayer");
    let mut root = Block::zeroed(layout::SCENARIO_MIN);
    let mut palette = Block::zeroed(h1campaign::scenario::PALETTE);
    palette.tag_ref(0, placed);
    palette.put_u32(0xc, 37);
    root.block(0x98, 1, palette);
    // (script startup arena_start none) with a single void atom.
    let mut script = Block::zeroed(hs::Script::SIZE);
    script.bytes[..11].copy_from_slice(b"arena_start");
    script
        .put_u16(34, hs::casts::types::VOID)
        .put_u32(36, 0x8000_0000);
    let mut node = Block::zeroed(hs::Node::SIZE);
    node.put_u16(0, 0x8000)
        .put_u16(2, hs::casts::types::VOID)
        .put_u16(4, hs::casts::types::VOID)
        .put_u16(6, 9)
        .put_u32(8, hs::Datum::NULL.0);
    root.block(field::SCRIPTS, 1, script);
    root.block(field::NODES, 1, node);
    root.data(field::STRINGS, Block::new(b"none\0".to_vec()));
    let scnr = set
        .add_tag(FourCC(*b"scnr"), r"invented\arena", TagPayload::Built(root))
        .unwrap();
    set.scenario = scnr;
    let bytes = set.build().unwrap().0;
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let elite = set
        .add_tag(FourCC(*b"char"), ELITE_PATH, TagPayload::Empty)
        .unwrap();
    let tags: Vec<(&str, TagId, Faction)> = classes
        .iter()
        .map(|name| {
            let path = format!(r"invented\{name}");
            (
                *name,
                set.add_tag(FourCC(*b"char"), &path, TagPayload::Built(Block::zeroed(4)))
                    .unwrap(),
                faction_of(name),
            )
        })
        .collect();
    let kit = prepare(
        &mut map,
        vec![zone()],
        vec![base(0), base(1)],
        elite,
        &character::invented_donor(),
        &tags,
        probe,
    )
    .unwrap();
    let TagPayload::Built(root) = &mut set.tag_mut(scnr).unwrap().payload else {
        unreachable!()
    };
    kit.fields.apply(root).unwrap();
    set.tag_mut(elite).unwrap().payload = TagPayload::Built(kit.character);
    Map::from_reader(Cursor::new(set.build().unwrap().0)).unwrap()
}

/// The faction a test class belongs to by its name.
fn faction_of(name: &str) -> Faction {
    match name.split('_').next() {
        Some("flood") => Faction::Flood,
        Some("marine") => Faction::Human,
        _ => Faction::Covenant,
    }
}

fn tag_bytes(map: &mut Map<Cursor<Vec<u8>>>, id: TagId) -> (Vec<u8>, u32) {
    let address = map.tag(id).unwrap().location.unwrap().address;
    (map.read_tag_bytes(id).unwrap(), address)
}

#[test]
fn squad_blocks_read_back_through_h1campaign() {
    let mut map = kit_map(false);
    let scenario = map.scenario().unwrap().id;
    let (bytes, address) = tag_bytes(&mut map, scenario);
    let view = TagView::new(&bytes, address);
    let ai = ScenarioAi::read(&view).unwrap();
    assert_eq!(ai.zones, [zone(), base(0), base(1)]);
    let elite = map.tag(ELITE_PATH).unwrap().id;
    assert_eq!(ai.character_palette, [elite]);
    // Sixteen slots, then the class record: the Elite's row.
    assert_eq!(ai.squads.len(), 17);
    assert_eq!(
        (ai.squads[16].name.as_str(), ai.squads[16].character),
        ("ff_class_elite", 0)
    );
    for (index, squad) in ai.squads[..16].iter().enumerate() {
        assert_eq!(squad.name, format!("ff_slot_{index:02}"));
        assert_eq!((squad.normal_count, squad.insane_count), (0, 0));
        assert_eq!(squad.team, COVENANT_TEAM);
        assert_eq!(squad.character, -1);
        assert_eq!(squad.initial_weapon, 1);
    }
    let palette = view
        .block(&bytes, 0x98, h1campaign::scenario::PALETTE)
        .unwrap();
    assert_eq!(palette.len(), 2);
    assert_eq!(
        palette[0][4..8],
        map.tag(r"invented\sprayer").unwrap().id.0.to_le_bytes()
    );
    assert_eq!(palette[0][0xc..0x10], 37u32.to_le_bytes());
    let weapon = map.tag(WEAPONS[0]).unwrap().id;
    assert_eq!(palette[1][4..8], weapon.0.to_le_bytes());

    let (bytes, address) = tag_bytes(&mut map, elite);
    let view = TagView::new(&bytes, address);
    let character = Character::read(&view).unwrap();
    assert_eq!(character.kind, Some(0)); // Elite.
    assert_eq!(character.unit, map.tag(ELITE_UNIT).unwrap().id);
    assert_eq!(character.parent, TagId::NULL);
    let weapons = view.block(&bytes, 0xcc, 0xcc).unwrap();
    assert_eq!(weapons[0][8..12], weapon.0.to_le_bytes());
    let sets = view.block(&bytes, 0xd4, 0x10).unwrap();
    assert_eq!(sets[0][4..8], weapon.0.to_le_bytes());
    assert_eq!(view.block(sets[0], 8, 0x40).unwrap().len(), 1);
    assert_eq!(hs::Scenario::read(&mut map).unwrap().scripts().len(), 1);
}

#[test]
fn each_palette_row_has_a_class_squad_naming_it() {
    let mut map = kit_map_with(false, &["grunt", "hunter"]);
    let ai = ScenarioAi::read_map(&mut map).unwrap();
    assert_eq!(ai.character_palette.len(), 3);
    let record: Vec<(&str, i16)> = ai
        .squads
        .iter()
        .filter_map(|s| Some((s.name.strip_prefix(CLASS_PREFIX)?, s.character)))
        .collect();
    assert_eq!(record, [("elite", 0), ("grunt", 1), ("hunter", 2)]);
    // Each row's tag is the class's own `char`.
    assert_eq!(ai.character_palette[1], map.tag(r"invented\grunt").unwrap().id);
    assert_eq!(ai.character_palette[2], map.tag(r"invented\hunter").unwrap().id);
    // The record places no one.
    assert!(
        ai.squads
            .iter()
            .all(|s| s.normal_count == 0 && s.insane_count == 0)
    );
}

#[test]
fn a_class_carries_its_faction_through_the_kit() {
    let mut map = kit_map_with(false, &["grunt", "flood_carrier", "marine"]);
    let ai = ScenarioAi::read_map(&mut map).unwrap();
    let record: Vec<(&str, u16)> = ai
        .squads
        .iter()
        .filter_map(|s| Some((s.name.strip_prefix(CLASS_PREFIX)?, s.team)))
        .collect();
    // The record squad's team is the faction's: Covenant 3, Flood 4, humans 2.
    assert_eq!(
        record,
        [("elite", 3), ("grunt", 3), ("flood_carrier", 4), ("marine", 2)]
    );
    // Each team reads back as the faction that was written.
    for (_, team) in record {
        assert!(Faction::from_team(team as u8).is_some());
    }
    // The slot squads stay on the Covenant team: the game sets a placed
    // squad's team from its class.
    assert!(
        ai.squads
            .iter()
            .filter(|s| s.name.starts_with("ff_slot_"))
            .all(|s| s.team == 3)
    );
}

#[test]
fn a_class_name_the_record_cannot_hold_is_refused_by_name() {
    let zones = [zone()];
    for bad in ["Grunt", "", "has space", "a_name_that_is_far_too_long_to_fit"] {
        let error = class_squads(&zones, 1, &[("elite", Faction::Covenant), (bad, Faction::Covenant)]).unwrap_err();
        assert!(error.contains(&format!("{bad:?}")), "{error}");
    }
    let error = class_squads(
        &zones,
        1,
        &[
            ("elite", Faction::Covenant),
            ("grunt", Faction::Covenant),
            ("grunt", Faction::Flood),
        ],
    ).unwrap_err();
    assert!(error.contains("grunt is named twice"), "{error}");
}

#[test]
fn the_probe_recipe_places_one_squad() {
    let mut map = kit_map(true);
    let ai = ScenarioAi::read_map(&mut map).unwrap();
    let [squad] = ai.squads.as_slice() else {
        panic!("expected one probe squad")
    };
    assert_eq!(squad.name, PROBE_SQUAD);
    assert_eq!((squad.normal_count, squad.insane_count), (2, 2));
    assert_eq!(
        (squad.character, squad.initial_zone, squad.initial_weapon),
        (0, 0, 1)
    );
    assert_eq!(squad.starting_locations.len(), 2);
    for (start, floor) in squad
        .starting_locations
        .iter()
        .zip(&ai.zones[0].firing_positions)
    {
        assert_eq!(start.position, floor.position);
        assert_eq!(start.character, -1);
    }

    let hs = hs::Scenario::read(&mut map).unwrap();
    assert_eq!(hs.scripts().len(), 2);
    assert_eq!(hs.scripts()[0].name().unwrap(), "arena_start");
    assert_eq!(hs.scripts()[0].root(), hs::Datum(0x8000_0000));
    assert_eq!(hs.script(PROBE_SCRIPT).unwrap().kind(), 0);
    let call = hs.node(hs.script(PROBE_SCRIPT).unwrap().root()).unwrap();
    assert!(!call.atom());
    assert_eq!(call.opcode(), h1campaign::scripts::aicontrol::AI_PLACE);
    let head = hs.node(hs::Datum(call.data())).unwrap();
    assert_eq!(hs.string(head.string_offset()).unwrap(), "ai_place");
    let actor = hs.node(head.next()).unwrap();
    assert_eq!(hs.string(actor.string_offset()).unwrap(), squad.name);
    assert_eq!(actor.value_type(), hs::casts::types::AI);
    assert_eq!(actor.data(), 0);
    assert_eq!(actor.next(), hs::Datum::NULL);
}

#[test]
fn each_base_has_a_hunt_order_over_its_area() {
    for probe in [false, true] {
        let mut map = kit_map(probe);
        let ai = ScenarioAi::read_map(&mut map).unwrap();
        assert_eq!(ai.orders.len(), 4);
        for (n, order) in ai.orders[..2].iter().enumerate() {
            assert_eq!(order.name, hunt_name(n));
            assert_eq!(order.name, format!("ff_hunt_{n:02}"));
            assert_eq!(
                (order.style, order.follow_squad, order.entry_script_index),
                (-1, -1, -1)
            );
            // Base n's zone follows the one drop zone.
            let zone = 1 + n;
            assert_eq!(ai.zones[zone].name, format!("ff_base_{n:02}"));
            let want: Vec<_> = (0..ai.zones[zone].areas.len())
                .map(|area| AreaRef {
                    kind: 0,
                    zone: zone as i16,
                    area: area as i16,
                })
                .collect();
            assert_eq!(order.primary_areas, want);
            assert!(order.secondary_areas.is_empty());
        }
        // Squads start only in drop zones.
        assert!(
            ai.squads
                .iter()
                .all(|s| s.initial_zone <= 0 && s.starting_locations.len() == 2)
        );
    }
}

#[test]
fn a_hunt_order_spans_only_its_base_and_a_base_without_areas_is_refused() {
    let mut zones = vec![zone(), base(0), zone(), base(1)];
    let second = zones[1].areas[0].clone();
    zones[1].areas.push(second);
    let orders = hunt_orders(&zones, &[1, 3]).unwrap();
    let spans: Vec<Vec<(i16, i16)>> = orders
        .iter()
        .map(|o| o.primary_areas.iter().map(|a| (a.zone, a.area)).collect())
        .collect();
    assert_eq!(spans, [vec![(1, 0), (1, 1)], vec![(3, 0)]]);
    zones[3].areas.clear();
    let error = hunt_orders(&zones, &[1, 3]).unwrap_err();
    assert!(error.contains("ff_hunt_01"), "{error}");
    assert!(hunt_orders(&zones, &[]).unwrap_err().contains("Hunt"));
    assert!(
        hunt_orders(&zones, &[9])
            .unwrap_err()
            .contains("ff_hunt_00")
    );
}

#[test]
fn each_base_has_a_hold_order_on_its_first_area() {
    let mut zones = vec![zone(), base(0), zone(), base(1)];
    let second = zones[1].areas[0].clone();
    zones[1].areas.push(second);
    let orders = hold_orders(&zones, &[1, 3]).unwrap();
    let names: Vec<_> = orders.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["ff_hold_00", "ff_hold_01"]);
    let spans: Vec<Vec<(i16, i16)>> = orders
        .iter()
        .map(|o| o.primary_areas.iter().map(|a| (a.zone, a.area)).collect())
        .collect();
    assert_eq!(spans, [vec![(1, 0)], vec![(3, 0)]]);
    zones[3].areas.clear();
    assert!(hold_orders(&zones, &[1, 3]).unwrap_err().contains("ff_hold_01"));
    assert!(hold_orders(&zones, &[]).unwrap_err().contains("Hold"));
}
