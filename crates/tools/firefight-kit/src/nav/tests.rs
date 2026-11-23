use super::*;
use std::collections::BTreeMap;
use tags::collision::{Child, Edge, Leaf, Node3d, Plane, Surface, Vertex};

/// A square arena with shared collision edges. A detached square to
/// the east is close enough for a bot to cross the seam, but has no
/// generated engine link into the arena.
fn grid() -> Bsp {
    let mut bsp = Bsp {
        planes: vec![Plane([0., 0., 1., 0.])],
        nodes: vec![Node3d {
            plane: 0,
            back: Child::None,
            front: Child::Leaf(0),
        }],
        leaves: vec![Leaf {
            flags: 0,
            ref_count: 0,
            first_ref: u16::MAX,
        }],
        ..Default::default()
    };
    let mut points = BTreeMap::new();
    let mut edges = BTreeMap::new();
    for (x, y) in (0..7)
        .flat_map(|y| (0..7).map(move |x| (x, y)))
        .chain([(9, 3)])
    {
        let corners = [(x, y), (x + 1, y), (x + 1, y + 1), (x, y + 1)].map(|p| {
            *points.entry(p).or_insert_with(|| {
                let i = bsp.vertices.len() as u16;
                bsp.vertices.push(Vertex {
                    point: [
                        4. * p.0 as f32 - if p.0 >= 9 { 7.8 } else { 0. },
                        4. * p.1 as f32,
                        0.,
                    ],
                    edge: 0,
                });
                i
            })
        });
        let sector = bsp.surfaces.len() as u16;
        let ring: [u16; 4] = std::array::from_fn(|i| {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            if let Some(&k) = edges.get(&(b, a)) {
                bsp.edges[usize::from(k)].right_surface = sector;
                return k;
            }
            let k = bsp.edges.len() as u16;
            edges.insert((a, b), k);
            bsp.vertices[a as usize].edge = k.into();
            bsp.edges.push(Edge {
                start_vertex: a,
                end_vertex: b,
                forward_edge: k,
                reverse_edge: k,
                left_surface: sector,
                right_surface: u16::MAX,
            });
            k
        });
        for (i, &k) in ring.iter().enumerate() {
            let e = &mut bsp.edges[usize::from(k)];
            let next = ring[(i + 1) % 4];
            if e.left_surface == sector {
                e.forward_edge = next;
            } else {
                e.reverse_edge = next;
            }
        }
        bsp.surfaces.push(Surface {
            plane: 0,
            first_edge: ring[0],
            flags: 0,
            breakable_surface: 0xff,
            material: 0,
        });
    }
    bsp
}

pub(crate) fn test_floor() -> Block {
    let mut sbsp = Block::zeroed(0x23c);
    sbsp.block(
        SBSP_COLLISION_FIELD as u32,
        1,
        h1convert::structure::collision_block(&grid()),
    );
    sbsp.block(0x2c, 1, Block::zeroed(8));
    sbsp
}

#[test]
fn every_drop_zone_has_a_path_to_the_middle() {
    let (pf, data) = build_with_pathfinding(&grid(), 0, 7).unwrap();
    let middle = [14., 14., 0.];
    let starts = [middle, [2., 2., 0.]];
    let arena = choose_zones(&pf, &data, &starts, &[], 2, |_| Some(3)).unwrap();
    assert!((6..=12).contains(&arena.zones.len()));
    assert!(arena.bases.is_empty());
    let center = data.nav.poly_near(middle).unwrap();
    assert!(
        data.nav.corridor(49, center).is_some(),
        "the bot can cross the seam"
    );
    let absent_from_engine = |e: &bots::nav::Edge| {
        !pf.links.iter().any(|link| {
            let a = u32::from(link.left_sector);
            let b = u32::from(link.right_sector);
            (a, b) == (e.from, e.to) || (a, b) == (e.to, e.from)
        })
    };
    assert!(
        data.nav
            .corridor_avoiding(49, center, &absent_from_engine)
            .is_none()
    );
    let sectors = arena
        .zones
        .iter()
        .map(|z| z.firing_positions[0].sector)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(sectors.len(), arena.zones.len());
    for zone in arena.zones {
        assert_eq!(zone.manual_bsp, 2);
        assert_eq!(zone.areas[0].clusters[0], 1 << 3);
        assert_eq!(zone.areas[0].count as usize, zone.firing_positions.len());
        assert!(zone.firing_positions.len() >= 2);
        for p in zone.firing_positions {
            assert_ne!(p.sector, 49, "detached island");
            assert!(
                data.nav
                    .corridor_avoiding(p.sector, center, &absent_from_engine)
                    .is_some()
            );
            assert_eq!(data.nav.poly_near(p.position), Some(p.sector));
            assert!(
                starts
                    .iter()
                    .all(|&s| distance2(s, p.position) >= SPAWN_CLEARANCE.powi(2))
            );
        }
    }
}

/// Two bases at either end of the square arena, a start at each.
const BASES: [V3; 2] = [[2., 14., 0.], [26., 14., 0.]];
const BASE_STARTS: [V3; 2] = [[3., 13., 0.], [25., 15., 0.]];

fn based_arena() -> (PathfindingData, MapData, Arena) {
    let (pf, data) = build_with_pathfinding(&grid(), 0, 7).unwrap();
    let arena = choose_zones(&pf, &data, &BASE_STARTS, &BASES, 2, |_| Some(3)).unwrap();
    (pf, data, arena)
}

#[test]
fn every_drop_zone_has_a_path_to_each_base() {
    let (pf, data, arena) = based_arena();
    assert!((6..=12).contains(&arena.zones.len()));
    let absent_from_engine = |e: &bots::nav::Edge| {
        !pf.links.iter().any(|link| {
            let a = u32::from(link.left_sector);
            let b = u32::from(link.right_sector);
            (a, b) == (e.from, e.to) || (a, b) == (e.to, e.from)
        })
    };
    let goals = BASES.map(|b| data.nav.poly_near(b).unwrap());
    for zone in &arena.zones {
        for p in &zone.firing_positions {
            for &goal in &goals {
                assert!(
                    data.nav
                        .corridor_avoiding(p.sector, goal, &absent_from_engine)
                        .is_some(),
                    "{} to base polygon {goal}",
                    zone.name
                );
            }
        }
    }
    // One zone per base after the drops, around its base point.
    assert_eq!(arena.bases.len(), 2);
    for (i, zone) in arena.bases.iter().enumerate() {
        assert_eq!(zone.name, format!("ff_base_{i:02}"));
        assert_eq!(zone.manual_bsp, 2);
        assert!(zone.firing_positions.len() >= 2);
        assert_eq!(zone.areas[0].count as usize, zone.firing_positions.len());
        assert!(distance2(zone.areas[0].mean, BASES[i]) < BASE_RADIUS.powi(2));
        for p in &zone.firing_positions {
            assert!(data.nav.corridor(p.sector, goals[i]).is_some());
        }
    }
    // A base the drops cannot walk to: the detached island east of the
    // arena has no engine link in, so no component holds both bases.
    let island = [[2., 14., 0.], [30.2, 14., 0.]];
    assert!(choose_zones(&pf, &data, &BASE_STARTS, &island, 2, |_| Some(3)).is_none());
}

#[test]
fn drops_are_chosen_away_from_the_home_base() {
    let (_, _, arena) = based_arena();
    let clearance = base_clearance(&BASES);
    assert_eq!(clearance, 6.0);
    let mut sides = [0usize; 2];
    for zone in &arena.zones {
        assert!(zone.name.starts_with("ff_drop_"));
        // The first firing position is the drop polygon's centre.
        let centre = zone.firing_positions[0].position;
        for base in BASES {
            assert!(
                distance2(centre, base) >= clearance.powi(2),
                "{} at {centre:?} is on a base",
                zone.name
            );
        }
        for s in BASE_STARTS {
            assert!(distance2(centre, s) >= DROP_START_CLEARANCE.powi(2));
            for p in &zone.firing_positions {
                assert!(distance2(p.position, s) >= SPAWN_CLEARANCE.powi(2));
            }
        }
        sides[bases::nearest(&BASES, centre)] += 1;
    }
    // Drops alternate between the bases' sides.
    assert!(sides[0] >= 3 && sides[1] >= 3, "{sides:?}");
    assert!(sides[0].abs_diff(sides[1]) <= 1, "{sides:?}");
    // The same inputs choose the same zones on every machine.
    let (_, _, again) = based_arena();
    assert_eq!(arena.zones, again.zones);
    assert_eq!(arena.bases, again.bases);
}

#[test]
fn firing_positions_face_the_nearer_base() {
    let middle = [14., 14., 0.];
    assert_eq!(facing(&BASES, middle, [20., 3., 0.]), BASES[1]);
    assert_eq!(facing(&BASES, middle, [8., 25., 0.]), BASES[0]);
    // Halfway is the home base's.
    assert_eq!(facing(&BASES, middle, [14., 0., 0.]), BASES[0]);
    assert_eq!(facing(&[], middle, [20., 3., 0.]), middle);
    // The cover probe runs level, two units toward the base, or to it.
    assert_eq!(
        cover_ray([22., 14., 1.], BASES[1]),
        ([22., 14., 1.5], [24., 14., 1.5])
    );
    assert_eq!(
        cover_ray([25., 14., 0.], BASES[1]),
        ([25., 14., 0.5], [26., 14., 0.5])
    );
}
