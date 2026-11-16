//! Invented BSPs only; the real-map checks are in `tests/real_maps.rs`.

use h1map::Space;
use h1map::bsp::CollisionBsp;
use h1map::testing::collision_room;

use crate::collision::convert;
use crate::query::{first_hit, leaf_at};
use crate::sandbox::{H2Item, h2_equivalent, h2_name_matches};
use crate::*;

fn room() -> CollisionBsp {
    let base = 0x8190_0000;
    let (bytes, at) = collision_room(base);
    CollisionBsp::read(&Space::new(&bytes, base), &bytes[at..at + 0x60]).unwrap()
}

#[test]
fn the_room_converts_and_reads_back_as_halo_2_blocks() {
    let src = room();
    let h2 = convert(&src).unwrap();
    let bytes = h2.encode_blocks(0x8010_0000);
    let back = tags::collision::Bsp::decode_blocks(&bytes, 0x8010_0000, 0).unwrap();
    assert_eq!(back, h2);
    assert_eq!(back.validate(1), Vec::<String>::new());
    assert_eq!(back.nodes[5].front, tags::collision::Child::Leaf(0));
    assert_eq!(back.nodes[0].back, tags::collision::Child::None);
    assert_eq!(back.refs[0].node, 0x8000);
    assert_eq!(back.surfaces[0].breakable_surface, 0xff);
    assert_eq!(back.edges[0].right_surface, 0xffff);
    assert_eq!(back.surface_ring(0), vec![0, 1, 2, 3]);
}

#[test]
fn points_and_segments_agree_between_the_source_and_the_conversion() {
    let src = room();
    let h2 = convert(&src).unwrap();
    for p in [
        [0.5, 0.5, 0.5],
        [-0.5, 0.5, 0.5],
        [0.5, 0.5, 1.5],
        [1.0, 1.0, 1.0],
    ] {
        assert_eq!(leaf_at(&src, p), leaf_at(&h2, p), "{p:?}");
        assert_eq!(leaf_at(&src, p), src.leaf_at(p).map(|l| l as usize));
    }
    // Straight down from the middle: the floor is half way.
    let down = first_hit(&src, [0.5, 0.5, 0.5], [0.5, 0.5, -0.5]).unwrap();
    assert!((down - 0.5).abs() < 1e-9, "{down}");
    assert_eq!(
        first_hit(&h2, [0.5, 0.5, 0.5], [0.5, 0.5, -0.5]),
        Some(down)
    );
    // Inside the room: no hit. Starting in solid: hit at 0.
    assert_eq!(first_hit(&h2, [0.2, 0.2, 0.2], [0.8, 0.8, 0.8]), None);
    assert_eq!(first_hit(&h2, [2.0, 0.5, 0.5], [0.5, 0.5, 0.5]), Some(0.0));
}

#[test]
fn an_index_past_halo_2s_fields_is_refused() {
    let mut src = room();
    src.surfaces[0].plane = 0x8000;
    assert!(matches!(
        convert(&src),
        Err(Error::TooBig {
            what: "surface plane",
            ..
        })
    ));
    let mut src = room();
    src.leaves[0].ref_count = 300;
    assert!(matches!(convert(&src), Err(Error::TooBig { .. })));
    let mut src = room();
    src.nodes[0].front = h1map::bsp::Child::Node(0x80_0000);
    assert!(matches!(
        convert(&src),
        Err(Error::TooBig {
            what: "3D node",
            ..
        })
    ));
}

#[test]
fn flags_move_from_bit_31_to_bit_15() {
    let mut src = room();
    src.refs[0].plane = 0x8000_0004;
    src.surfaces[0].plane = 0x8000_0004;
    let h2 = convert(&src).unwrap();
    assert_eq!(h2.refs[0].plane, 0x8004);
    assert_eq!(h2.surfaces[0].plane, 0x8004);
}

#[test]
fn halo_1_items_map_to_their_nearest_halo_2_items() {
    let w = |p| h2_equivalent(p);
    assert_eq!(
        w("weapons\\assault rifle\\assault rifle"),
        Some(H2Item::Weapon("smg"))
    );
    assert_eq!(w("weapons\\pistol\\pistol"), Some(H2Item::Weapon("magnum")));
    assert_eq!(
        w("weapons\\plasma pistol\\plasma pistol"),
        Some(H2Item::Weapon("plasma_pistol"))
    );
    assert_eq!(
        w("powerups\\pistol ammo\\pistol ammo"),
        Some(H2Item::Dropped)
    );
    assert_eq!(w("powerups\\health pack"), Some(H2Item::Dropped));
    assert_eq!(
        w("vehicles\\ghost\\ghost_mp"),
        Some(H2Item::Vehicle("ghost"))
    );
    assert_eq!(w("something\\unknown"), None);
    assert!(h2_name_matches(
        "objects\\weapons\\rifle\\plasma_rifle\\plasma_rifle",
        "plasma_rifle"
    ));
    assert!(!h2_name_matches(
        "objects\\weapons\\rifle\\smg\\smg",
        "plasma_rifle"
    ));
}

fn mp() -> h1map::scenario::Multiplayer {
    use h1map::scenario::*;
    use h1map::{Group, TagId, TagRef};
    let null = TagRef {
        group: Group::new(b"itmc"),
        name_address: 0,
        name_len: 0,
        id: TagId::NONE,
    };
    let flag = |kind, team| NetgameFlag {
        position: [1.0, 2.0, 3.0],
        facing: 0.5,
        kind,
        team,
        weapon_group: null,
    };
    let equipment = |respawn| NetgameEquipment {
        flags: 0,
        game_types: [GameType::ALL; 4],
        team: 0,
        spawn_time: respawn,
        position: [4.0, 5.0, 6.0],
        facing: 1.0,
        item: null,
    };
    Multiplayer {
        skies: vec![],
        starting_locations: vec![StartingLocation {
            position: [7.0, 8.0, 9.0],
            facing: 0.25,
            team: 1,
            bsp: 0,
            game_types: [GameType::ALL; 4],
        }],
        flags: vec![
            flag(FlagKind::CtfFlag, 1),
            flag(FlagKind::TeleportFrom, 3),
            flag(FlagKind::RaceTrack, 0),
        ],
        equipment: vec![equipment(30), equipment(0)],
        vehicle_palette: vec![],
        vehicles: vec![],
    }
}

#[test]
fn the_halo_2_sandbox_plan_maps_flags_and_items_and_says_what_it_drops() {
    use crate::placements::*;
    let names = [
        vec!["weapons\\assault rifle\\assault rifle".to_string()],
        vec!["powerups\\health pack".to_string()],
    ];
    let p = plan(&mp(), |i| names[i].clone());
    assert_eq!(
        p.spawns,
        vec![Spawn {
            position: [7.0, 8.0, 9.0],
            facing: 0.25,
            team: 1
        }]
    );
    let kinds: Vec<_> = p.flags.iter().map(|f| (f.kind, f.team)).collect();
    assert_eq!(
        kinds,
        vec![
            (H2Flag::CtfFlagSpawn, 1),
            (H2Flag::CtfFlagReturn, 1),
            (H2Flag::TeleporterSource, 3)
        ]
    );
    assert_eq!(p.items.len(), 1);
    assert_eq!(p.items[0].item, H2Item::Weapon("smg"));
    assert_eq!(p.items[0].respawn, 30);
    assert_eq!(p.dropped.len(), 2, "{:?}", p.dropped);
    assert!(p.dropped[0].contains("RaceTrack"));
    assert!(p.dropped[1].contains("health pack"));
}

#[test]
fn roles_resolve_to_the_base_maps_collections() {
    use crate::placements::resolve;
    let tags: Vec<(String, String)> = [
        ("weap", "objects\\weapons\\rifle\\smg\\smg"),
        ("itmc", "multiplayer\\single_weapons\\plasma_rifle"),
        ("itmc", "multiplayer\\single_weapons\\smg"),
        ("vehc", "multiplayer\\vehicles\\warthog"),
    ]
    .iter()
    .map(|(g, n)| (g.to_string(), n.to_string()))
    .collect();
    assert_eq!(
        resolve(H2Item::Weapon("smg"), &tags),
        Some("multiplayer\\single_weapons\\smg")
    );
    assert_eq!(
        resolve(H2Item::Vehicle("warthog"), &tags),
        Some("multiplayer\\vehicles\\warthog")
    );
    assert_eq!(resolve(H2Item::Weapon("needler"), &tags), None);
    assert_eq!(resolve(H2Item::Dropped, &tags), None);
}
