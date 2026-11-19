use super::*;
use crate::mind::Skill;
use crate::vehicle::tests::{ghost, sit, vehicle, warthog};

fn foot(at: Vec3) -> Foot {
    Foot {
        at,
        team: 0,
        teams: true,
    }
}
fn sight(vehicles: Vec<Vehicle>) -> Sight {
    Sight {
        riding: None,
        vehicles,
    }
}
fn still(_: u32) -> f32 {
    0.0
}
fn none(_: u32) -> bool {
    false
}
fn normal() -> Wheels {
    Wheels::of(Skill::Normal)
}

#[test]
fn the_nearest_free_vehicle_in_range_is_taken_for_its_drivers_seat() {
    let s = sight(vec![ghost(1, [15.0, 0.0, 0.0]), ghost(2, [6.0, 0.0, 0.0])]);
    let p = take(&foot([0.0; 3]), &s, &[], &normal(), still, none).unwrap();
    assert_eq!(
        p,
        Pick {
            why: Why::Take,
            vehicle: 2,
            root: 2,
            role: SeatRole::Driver
        }
    );
    // Out of Easy's range.
    let far = sight(vec![ghost(1, [15.0, 0.0, 0.0])]);
    assert_eq!(
        take(
            &foot([0.0; 3]),
            &far,
            &[],
            &Wheels::of(Skill::Easy),
            still,
            none
        ),
        None
    );
    // One on another floor costs more than it is across.
    let up = sight(vec![ghost(1, [8.0, 0.0, 5.0])]);
    assert_eq!(
        take(&foot([0.0; 3]), &up, &[], &normal(), still, none),
        None
    );
}

#[test]
fn a_wreck_a_flipped_one_a_moving_one_or_a_shunned_one_is_not_taken() {
    let me = foot([0.0; 3]);
    let w = normal();
    let mut hurt = ghost(1, [5.0, 0.0, 0.0]);
    hurt.health = 0.3;
    assert_eq!(take(&me, &sight(vec![hurt]), &[], &w, still, none), None);
    let mut flipped = ghost(1, [5.0, 0.0, 0.0]);
    flipped.up = [0.0, 1.0, 0.0];
    assert_eq!(take(&me, &sight(vec![flipped]), &[], &w, still, none), None);
    let s = sight(vec![ghost(1, [5.0, 0.0, 0.0])]);
    assert_eq!(take(&me, &s, &[], &w, |_| 3.0, none), None, "moving");
    assert_eq!(take(&me, &s, &[], &w, still, |id| id == 1), None);
    // An empty Banshee can be taken. A turret needs a contested area.
    let banshee = vehicle(1, Class::Fighter, [5.0, 0.0, 0.0], &[SeatRole::Driver]);
    let turret = vehicle(2, Class::Turret, [5.0, 0.0, 0.0], &[SeatRole::Gunner]);
    assert_eq!(
        take(&me, &sight(vec![banshee, turret]), &[], &w, still, none).map(|p| p.vehicle),
        Some(1)
    );
}

#[test]
fn one_with_an_enemy_aboard_is_not_taken_and_a_teammate_nearer_has_it() {
    let me = foot([0.0; 3]);
    let w = normal();
    let [mut hog, gun] = warthog(10, [8.0, 0.0, 0.0]);
    sit(&mut hog, 1, 5, 1);
    assert_eq!(
        take(&me, &sight(vec![hog, gun]), &[], &w, still, none),
        None
    );
    let s = sight(vec![ghost(1, [8.0, 0.0, 0.0])]);
    assert_eq!(take(&me, &s, &[[7.0, 1.0, 0.0]], &w, still, none), None);
    // A teammate not much nearer, or far from it, does not.
    assert!(take(&me, &s, &[[1.0, 0.0, 0.0]], &w, still, none).is_some());
    assert!(take(&me, &s, &[[8.0, 20.0, 0.0]], &w, still, none).is_some());
}

#[test]
fn a_teammates_warthog_is_manned_as_its_gunner() {
    let me = foot([0.0; 3]);
    let [mut hog, gun] = warthog(10, [8.0, 0.0, 0.0]);
    sit(&mut hog, 0, 5, 0);
    let s = sight(vec![hog.clone(), gun.clone()]);
    assert_eq!(
        take(&me, &s, &[], &normal(), still, none),
        Some(Pick {
            why: Why::Take,
            vehicle: 11,
            root: 10,
            role: SeatRole::Gunner
        })
    );
    // Easy does not ride as a gunner.
    assert_eq!(
        take(&me, &s, &[], &Wheels::of(Skill::Easy), still, none),
        None
    );
    // A free Warthog a little further beats a teammate's gun.
    let s = sight(vec![hog, gun, warthog(20, [10.0, 0.0, 0.0])[0].clone()]);
    assert_eq!(
        take(&me, &s, &[], &normal(), still, none).unwrap().vehicle,
        20
    );
}

#[test]
fn an_enemy_vehicle_close_and_slow_is_boarded() {
    let me = foot([0.0; 3]);
    let w = Wheels::of(Skill::Heroic);
    let mut g = ghost(1, [2.0, 0.0, 0.3]);
    sit(&mut g, 0, 5, 1);
    let s = sight(vec![g.clone()]);
    assert_eq!(
        board(&me, &s, &w, |_| 2.0, none),
        Some(Pick {
            why: Why::Board,
            vehicle: 1,
            root: 1,
            role: SeatRole::Driver
        })
    );
    assert_eq!(board(&me, &s, &w, |_| 8.0, none), None, "too fast");
    assert_eq!(board(&me, &s, &Wheels::of(Skill::Easy), still, none), None);
    assert_eq!(
        board(&me, &s, &normal(), still, none),
        None,
        "out of Normal's reach"
    );
    // A teammate's is not boarded; nor an empty one; nor one overhead.
    let mut friend = g.clone();
    sit(&mut friend, 0, 5, 0);
    assert_eq!(board(&me, &sight(vec![friend]), &w, still, none), None);
    assert_eq!(
        board(
            &me,
            &sight(vec![ghost(1, [2.0, 0.0, 0.0])]),
            &w,
            still,
            none
        ),
        None
    );
    let mut above = g;
    above.position[2] = 3.0;
    assert_eq!(board(&me, &sight(vec![above]), &w, still, none), None);
}

#[test]
fn in_free_for_all_anyone_driving_is_boarded() {
    let me = Foot {
        at: [0.0; 3],
        team: 0,
        teams: false,
    };
    let mut g = ghost(1, [2.0, 0.0, 0.0]);
    sit(&mut g, 0, 5, 0);
    assert!(
        board(
            &me,
            &sight(vec![g]),
            &Wheels::of(Skill::Heroic),
            still,
            none
        )
        .is_some()
    );
}

#[test]
fn the_approach_uses_the_resolved_marker_and_radius() {
    let g = ghost(1, [10.0, 0.0, 1.0]);
    let pick = Pick {
        why: Why::Take,
        vehicle: 1,
        root: 1,
        role: SeatRole::Driver,
    };
    let (first, entry) = approach(&g, pick, g.position, 0, None).unwrap();
    let (retry, _) = approach(&g, pick, g.position, 1, None).unwrap();
    assert_eq!(first[0], 10.0);
    assert!(first[1] > retry[1] && retry[1] > entry.at[1]);
    assert!(in_entry(&g, entry, first));
    assert!(!in_entry(
        &g,
        entry,
        [first[0], first[1] + entry.radius, first[2]]
    ));
    assert!(!in_entry(&g, entry, [10.0, -0.2, 1.0]));
    let [_, gun] = warthog(10, [0.0; 3]);
    let pick = Pick {
        why: Why::Take,
        vehicle: 11,
        root: 10,
        role: SeatRole::Gunner,
    };
    assert!(approach(&gun, pick, [0.0; 3], 0, None).unwrap().0[0] < -1.15);
}

#[test]
fn a_turret_is_taken_when_it_covers_the_contest_and_abandoned_when_flanked() {
    let at = [4.0, 0.0, 0.0];
    let turret = vehicle(1, Class::Turret, at, &[SeatRole::Gunner]);
    let s = sight(vec![turret]);
    let me = foot([0.0; 3]);
    assert_eq!(
        take_turret(&me, &s, [15.0, 0.0, 0.0], None, none)
            .unwrap()
            .vehicle,
        1
    );
    assert!(take_turret(&me, &s, [35.0, 0.0, 0.0], None, none).is_none());
    let walls = crate::los::Walls::from_polygons([&[
        [7.0, -2.0, 0.0],
        [7.0, 2.0, 0.0],
        [7.0, 2.0, 2.0],
        [7.0, -2.0, 2.0],
    ][..]]);
    let map = crate::cache::MapData {
        walls,
        ..Default::default()
    };
    assert!(take_turret(&me, &s, [15.0, 0.0, 0.0], Some(&map), none).is_none());
    assert!(flanked(at, [1.0, 0.0, 0.0], [[4.0, 3.0, 0.0]]));
    assert!(!flanked(
        at,
        [1.0, 0.0, 0.0],
        [[7.0, 0.0, 0.0], [4.0, 3.0, 3.0]]
    ));
}

#[test]
fn a_mounted_turret_needs_a_friendly_driver() {
    let [mut hog, gun] = warthog(10, [4.0, 0.0, 0.0]);
    let me = foot([0.0; 3]);
    let contact = [15.0, 0.0, 0.0];
    assert!(
        take_turret(
            &me,
            &sight(vec![hog.clone(), gun.clone()]),
            contact,
            None,
            none
        )
        .is_none()
    );
    sit(&mut hog, 0, 4, 0);
    assert_eq!(
        take_turret(&me, &sight(vec![hog, gun]), contact, None, none)
            .unwrap()
            .vehicle,
        11
    );
}
