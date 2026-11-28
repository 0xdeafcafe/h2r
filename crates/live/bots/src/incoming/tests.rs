use super::*;
use crate::los::Walls;
use crate::nav::{NavMesh, Poly};
use guest_layout::testing::FakeGuest;

fn floor() -> MapData {
    MapData {
        places: Default::default(),
        nav: NavMesh::new(
            vec![Poly::new(vec![
                [-10.0, -10.0, 0.0],
                [10.0, -10.0, 0.0],
                [10.0, 10.0, 0.0],
                [-10.0, 10.0, 0.0],
            ])],
            vec![],
        ),
        walls: Walls::default(),
    }
}

#[test]
fn grenade_projectiles_come_from_the_loaded_globals() {
    let mut g = FakeGuest::new();
    let globals = 0x1000;
    let block = 0x2000;
    let frag = DatumHandle::new(0xa121, 32);
    let plasma = DatumHandle::new(0xa122, 37);
    assert!(grenade_defs(&g).is_none());
    g.put_u32(GLOBALS_TAG, globals)
        .put_u32(globals + GRENADES, 2)
        .put_u32(globals + GRENADES + 4, block)
        .put_u32(block + PROJECTILE_DATUM, frag.raw())
        .put_u32(block + GRENADE_STRIDE + PROJECTILE_DATUM, plasma.raw());
    assert_eq!(grenade_defs(&g), Some([Some(frag), Some(plasma)]));
    g.put_u32(globals + GRENADES, 1);
    assert_eq!(grenade_defs(&g), Some([Some(frag), None]));
}

#[test]
fn a_visible_grenade_gets_one_jump_and_a_route_away() {
    let mut dodge = Evasion::default();
    let map = floor();
    let me = Me::default();
    let grenades = [Grenade {
        id: 1,
        at: [1.5, 0.0, 0.0],
    }];
    for tick in 0..5 {
        assert_eq!(
            dodge.dodge(tick, Skill::Legendary, &me, Some(&map), &grenades),
            None
        );
    }
    let (dir, jump) = dodge
        .dodge(5, Skill::Legendary, &me, Some(&map), &grenades)
        .unwrap();
    assert!(dir[0] < -0.9 && jump);
    for tick in 6..30 {
        let (dir, jump) = dodge
            .dodge(tick, Skill::Legendary, &me, Some(&map), &grenades)
            .unwrap();
        assert!(dir[0] < -0.9 && !jump);
    }
}

#[test]
fn grenades_behind_a_wall_or_an_edge_do_not_cause_blind_jumps() {
    let wall = [
        [0.75, -2.0, 0.0],
        [0.75, 2.0, 0.0],
        [0.75, 2.0, 2.0],
        [0.75, -2.0, 2.0],
    ];
    let mut map = floor();
    map.walls = Walls::from_polygons([&wall[..]]);
    let mut dodge = Evasion::default();
    let me = Me::default();
    let grenades = [Grenade {
        id: 1,
        at: [1.5, 0.0, 0.0],
    }];
    for tick in 0..30 {
        assert_eq!(
            dodge.dodge(tick, Skill::Legendary, &me, Some(&map), &grenades),
            None
        );
    }
    let map = floor();
    let me = Me {
        position: [-9.8, 0.0, 0.0],
        ..me
    };
    let grenades = [Grenade {
        id: 2,
        at: [-8.3, 0.0, 0.0],
    }];
    for tick in 30..60 {
        assert_eq!(
            dodge.dodge(tick, Skill::Legendary, &me, Some(&map), &grenades),
            None
        );
    }
}

#[test]
fn the_world_scan_only_yields_nearby_grenade_projectiles() {
    use guest_layout::{datum::layout as dl, points};

    let mut g = FakeGuest::new();
    let globals = 0x1000;
    let block = 0x2000;
    let array = 0x3000;
    let headers = 0x4000;
    let bodies = 0x5000;
    let def = DatumHandle::new(0xa121, 32);
    g.put_u32(GLOBALS_TAG, globals)
        .put_u32(globals + GRENADES, 1)
        .put_u32(globals + GRENADES + 4, block)
        .put_u32(block + PROJECTILE_DATUM, def.raw())
        .put_u32(points::OBJECTS.base, array)
        .map(array, 0x48)
        .put_u32(array + dl::ELEMENT_SIZE, 0xc)
        .put_u32(array + dl::HIGH_WATER, 3)
        .put_u32(array + dl::DATA, headers)
        .map(headers, 0x24);
    for (i, (tag, x)) in [(def.raw(), 1.5), (def.raw(), 30.0), (0xa222_0102, 1.5)]
        .into_iter()
        .enumerate()
    {
        let i = i as u32;
        let header = headers + 0xc * i;
        let body = bodies + 0x100 * i;
        g.put_u16(header, 0xb401 + i as u16)
            .put_u8(header + 3, PROJECTILE)
            .put_u32(header + 8, body)
            .map(body, 0x100)
            .put_u32(body, tag)
            .put_f32(body + 0x64, x);
    }
    let crate::arms::Ground {
        weapons: pickups,
        grenades,
        ..
    } = crate::arms::ground(&g, Some([0.0; 3])).unwrap();
    assert!(pickups.is_empty());
    assert_eq!(grenades.len(), 1);
    assert_eq!(grenades[0].id, DatumHandle::new(0xb401, 0).raw());
    assert_eq!(grenades[0].at, [1.5, 0.0, 0.0]);
}

#[test]
fn loose_grenades_use_the_maps_equipment_refs_and_exclude_attached_objects() {
    use crate::grenades::Kind;
    use guest_layout::{datum::layout as dl, points};

    let mut g = FakeGuest::new();
    let [matg, block, array, headers, bodies] = [0x1000, 0x2000, 0x3000, 0x4000, 0x5000];
    let frag = DatumHandle::new(0xaa01, 12);
    let plasma = DatumHandle::new(0xaa02, 13);
    g.put_u32(GLOBALS_TAG, matg)
        .put_u32(matg + GRENADES, 2)
        .put_u32(matg + GRENADES + 4, block)
        .put_u32(block + EQUIPMENT_DATUM, frag.raw())
        .put_u32(block + GRENADE_STRIDE + EQUIPMENT_DATUM, plasma.raw())
        .put_u32(points::OBJECTS.base, array)
        .map(array, 0x48)
        .put_u32(array + dl::ELEMENT_SIZE, 0xc)
        .put_u32(array + dl::HIGH_WATER, 3)
        .put_u32(array + dl::DATA, headers)
        .map(headers, 0x24);
    for (i, (tag, parent)) in (0..).zip([
        (frag, u32::MAX), (plasma, u32::MAX), (plasma, 0xeedd_0002),
    ]) {
        let header = headers + i * 0xc;
        let body = bodies + i * 0x100;
        g.put_u16(header, 0xcc01)
            .put_u8(header + 3, crate::guest::intel::EQUIPMENT)
            .put_u32(header + 8, body)
            .map(body, 0x100)
            .put_u32(body, tag.raw())
            .put_u32(body + 0x14, parent)
            .put_f32(body + 0x64, i as f32);
    }
    let ground = crate::arms::ground(&g, None).unwrap();
    assert_eq!(ground.loose_grenades, [([0.; 3], Kind::Frag), ([1., 0., 0.], Kind::Plasma)]);
    assert!(ground.grenades.is_empty());
}

#[test]
fn a_dodge_jump_never_lands_off_an_edge() {
    // A platform 0 to 11.3 in X at height 2, over a void.
    let map = MapData {
        places: Default::default(),
        nav: NavMesh::new(
            vec![Poly::new(vec![
                [0.0, -10.0, 2.0],
                [11.3, -10.0, 2.0],
                [11.3, 10.0, 2.0],
                [0.0, 10.0, 2.0],
            ])],
            vec![],
        ),
        walls: Walls::default(),
    };
    let me = |aim| Me {
        position: [10.0, 0.0, 2.0],
        aim,
        ..Me::default()
    };
    let dodge_to = |me: Me, grenade: Grenade| {
        let mut dodge = Evasion::default();
        for tick in 0..5 {
            assert!(
                dodge
                    .dodge(tick, Skill::Legendary, &me, Some(&map), &[grenade])
                    .is_none()
            );
        }
        dodge
            .dodge(5, Skill::Legendary, &me, Some(&map), &[grenade])
            .unwrap()
    };
    // The grenade behind it: away is +X, off the edge inside a jump's
    // carry. It still runs, on foot.
    let (dir, jump) = dodge_to(
        me([-1.0, 0.0, 0.0]),
        Grenade {
            id: 1,
            at: [8.5, 0.0, 2.0],
        },
    );
    assert!(dir[0] > 0.9 && !jump, "{dir:?} {jump}");
    // The grenade ahead of it, along the platform: the same dodge jumps.
    let (dir, jump) = dodge_to(
        me([1.0, 0.0, 0.0]),
        Grenade {
            id: 2,
            at: [11.5, 0.0, 2.0],
        },
    );
    assert!(dir[0] < -0.9 && jump, "{dir:?} {jump}");
}
