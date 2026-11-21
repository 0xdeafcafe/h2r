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

/// Both modes start with a ported multiplayer fixture. Its unrelated script
/// and the previously placed weapon keep their original indices.
fn kit_map(probe: bool) -> Map<Cursor<Vec<u8>>> {
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
    let base = set.build().unwrap().0;
    let mut map = Map::from_reader(Cursor::new(base)).unwrap();
    let elite = set
        .add_tag(FourCC(*b"char"), ELITE_PATH, TagPayload::Empty)
        .unwrap();
    let kit = prepare(&mut map, vec![zone()], elite, probe).unwrap();
    let TagPayload::Built(root) = &mut set.tag_mut(scnr).unwrap().payload else {
        unreachable!()
    };
    kit.fields.apply(root).unwrap();
    set.tag_mut(elite).unwrap().payload = TagPayload::Built(kit.character);
    Map::from_reader(Cursor::new(set.build().unwrap().0)).unwrap()
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
    assert_eq!(ai.zones, [zone()]);
    let elite = map.tag(ELITE_PATH).unwrap().id;
    assert_eq!(ai.character_palette, [elite]);
    assert_eq!(ai.squads.len(), 16);
    for (index, squad) in ai.squads.iter().enumerate() {
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
