use super::*;
use crate::nav::{Follower, Poly};

fn floor(x: f32, z: f32) -> Poly {
    Poly::new(vec![
        [x - 2.0, -2.0, z],
        [x + 2.0, -2.0, z],
        [x + 2.0, 2.0, z],
        [x - 2.0, 2.0, z],
    ])
}

#[test]
fn teleporter_flags_pair_by_identifier_and_keep_all_receivers() {
    let mut bytes = vec![0; 0x300];
    bytes[0x118..0x11c].copy_from_slice(&4u32.to_le_bytes());
    bytes[0x11c..0x120].copy_from_slice(&0x1240u32.to_le_bytes());
    for (i, (x, kind, channel)) in [
        (0.0f32, 7i16, 3i16),
        (12.0, 8, 3),
        (20.0, 8, 3),
        (28.0, 8, 4),
    ]
    .into_iter()
    .enumerate()
    {
        let at = 0x240 + i * 0x20;
        bytes[at..at + 4].copy_from_slice(&x.to_le_bytes());
        bytes[at + 0x10..at + 0x12].copy_from_slice(&kind.to_le_bytes());
        bytes[at + 0x12..at + 0x14].copy_from_slice(&8i16.to_le_bytes());
        bytes[at + 0x14..at + 0x16].copy_from_slice(&channel.to_le_bytes());
    }
    let placements = Placements {
        teleporters: flags(&TagView::new(&bytes, 0x1000)).unwrap(),
        lifts: vec![],
    };
    let mut m = MapData {
        nav: NavMesh::new(
            [0.0, 12.0, 20.0, 28.0].map(|x| floor(x, 0.0)).to_vec(),
            vec![],
        ),
        ..Default::default()
    };
    placements.add_links(&mut m);
    assert_eq!(
        m.nav
            .edges
            .iter()
            .map(|e| (e.from, e.to, e.kind))
            .collect::<Vec<_>>(),
        [(0, 1, Kind::Teleporter), (0, 2, Kind::Teleporter)]
    );
    let path = m.nav.path([-1.0, 0.0, 0.0], [13.0, 0.0, 0.0]).unwrap();
    let mut f = Follower::new(path, 0);
    assert_eq!(f.steer([-1.0, 0.0, 0.0], 1).dir, [1.0, 0.0]);
    assert_eq!(f.steer([-0.1, 0.0, 0.0], 2).dir, [1.0, 0.0]);
    let after = f.steer([12.0, 0.0, 0.0], 3);
    assert!(!after.traversing && !after.replan);
    assert_eq!(after.dir, [1.0, 0.0]);
}

#[test]
fn lift_link_lands_on_an_upper_surface_and_steers_to_its_exit() {
    let mut m = MapData {
        nav: NavMesh::new(vec![floor(0.0, 0.0), floor(3.0, 3.0)], vec![]),
        ..Default::default()
    };
    let lifts = Placements {
        lifts: vec![Lift {
            position: [0.0; 3],
            yaw: 0.0,
            kind: LiftKind::Vertical,
        }],
        ..Default::default()
    };
    lifts.add_links(&mut m);
    let [e] = m.nav.edges.as_slice() else {
        panic!("{:?}", m.nav.edges)
    };
    assert_eq!((e.from, e.to, e.kind), (0, 1, Kind::Lift));
    let mut f = Follower::new(m.nav.path([-1.0, 0.0, 0.0], [3.0, 0.0, 3.0]).unwrap(), 0);
    assert_eq!(f.steer([-1.0, 0.0, 0.0], 1).dir, [1.0, 0.0]);
    assert_eq!(f.steer([0.0; 3], 2).dir, [0.0; 2]);
    assert_eq!(f.steer([0.0, 0.0, 1.8], 3).dir, [1.0, 0.0]);
    assert!(!f.steer(e.b, 4).traversing);
}

#[test]
fn the_launcher_lands_in_front_of_its_placement() {
    let m = MapData {
        nav: NavMesh::new(
            vec![floor(0.0, 0.0), floor(12.0, 3.0), floor(-12.0, 2.0)],
            vec![],
        ),
        ..Default::default()
    };
    let e = lift_link(
        &m,
        &Lift {
            position: [0.0; 3],
            yaw: 0.0,
            kind: LiftKind::Launcher,
        },
    )
    .unwrap();
    assert_eq!((e.from, e.to), (0, 1));
}

#[test]
fn the_elevator_holds_the_platform_until_it_is_level_with_the_exit() {
    let mut m = MapData {
        nav: NavMesh::new(vec![floor(0.0, 0.0), floor(3.0, 4.0)], vec![]),
        ..Default::default()
    };
    let lifts = Placements {
        lifts: vec![Lift {
            position: [0.0; 3],
            yaw: 0.0,
            kind: LiftKind::Elevator,
        }],
        ..Default::default()
    };
    lifts.add_links(&mut m);
    let [edge] = m.nav.edges.as_slice() else {
        panic!("{:?}", m.nav.edges)
    };
    assert_eq!(edge.kind, Kind::Elevator);
    let mut f = Follower::new(m.nav.path([0.0; 3], [3.0, 0.0, 4.0]).unwrap(), 0);
    f.steer([0.0; 3], 1);
    assert_eq!(f.steer([0.0, 0.0, 2.0], 2).dir, [0.0; 2]);
    assert_eq!(f.steer([0.0, 0.0, 3.7], 3).dir, [1.0, 0.0]);
}

#[test]
fn converted_h1_one_way_and_reciprocal_channels_survive_link_building() {
    let pad = |x, channel, sender| Teleporter {
        position: [x, 0., 0.],
        channel,
        sender,
    };
    let placements = Placements {
        teleporters: vec![
            pad(0., 4, true),
            pad(20., 4, false),
            pad(20., 9, true),
            pad(0., 9, false),
            pad(10., 1, true),
            pad(30., 1, false),
        ],
        ..Default::default()
    };
    let mut map = MapData {
        nav: NavMesh::new([0., 10., 20., 30.].map(|x| floor(x, 0.)).to_vec(), vec![]),
        ..Default::default()
    };
    placements.add_links(&mut map);
    assert!(map.nav.corridor(0, 2).is_some() && map.nav.corridor(2, 0).is_some());
    assert!(map.nav.corridor(1, 3).is_some() && map.nav.corridor(3, 1).is_none());
}

#[test]
fn high_receiver_must_have_a_clear_fall_and_the_follower_waits_for_it() {
    let pair = Placements {
        teleporters: vec![
            Teleporter {
                position: [0.; 3],
                channel: 0,
                sender: true,
            },
            Teleporter {
                position: [10., 0., 3.1],
                channel: 0,
                sender: false,
            },
        ],
        ..Default::default()
    };
    let mut map = MapData {
        nav: NavMesh::new(vec![floor(0., 0.), floor(10., 0.)], vec![]),
        ..Default::default()
    };
    let mut blocked = map.clone();
    blocked.walls = crate::los::Walls::new(vec![[[8., -2., 2.], [12., -2., 2.], [10., 2., 2.]]]);
    pair.add_links(&mut blocked);
    assert!(blocked.nav.edges.is_empty());
    pair.add_links(&mut map);
    let mut f = Follower::new(map.nav.path([0.; 3], [11., 0., 0.]).unwrap(), 0);
    f.steer([0.; 3], 1);
    let falling = f.steer([10., 0., 3.1], 2);
    assert!(falling.traversing && !falling.replan);
    assert!(!f.steer([10., 0., 0.], 12).traversing);
}

#[test]
fn placed_lift_families_select_their_own_decks() {
    for (path, dx, z, kind) in [
        ("power_boost", -12., 4., Kind::Lift),
        ("man_cannon", 12., 4., Kind::Lift),
        ("elevator_cyclotron", 2., 5., Kind::Elevator),
        ("elevator", 2., 2., Kind::Elevator),
        ("grav_colossus", 2., 2., Kind::Lift),
        ("steam_lift", 2., 6.5, Kind::Lift),
        ("backwash_lift", 2., 2.4, Kind::Lift),
    ] {
        let mut map = MapData {
            nav: NavMesh::new(vec![floor(0., 0.), floor(dx, z)], vec![]),
            ..Default::default()
        };
        let placements = Placements {
            lifts: vec![Lift {
                position: [0.; 3],
                yaw: 0.,
                kind: lift_kind(path).unwrap(),
            }],
            ..Default::default()
        };
        placements.add_links(&mut map);
        assert_eq!(
            map.nav
                .edges
                .iter()
                .map(|e| (e.from, e.to, e.kind))
                .collect::<Vec<_>>(),
            [(0, 1, kind)],
            "{path}"
        );
    }
    for effect in [
        "high_lift_effect",
        "grav_column_nogarbage_top",
        "backwash_lift_movt",
    ] {
        assert!(lift_kind(effect).is_none(), "{effect}");
    }
}
