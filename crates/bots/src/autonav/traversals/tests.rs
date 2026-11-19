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
