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
    // A Banshee is not taken yet, nor a turret on its own.
    let banshee = vehicle(1, Class::Fighter, [5.0, 0.0, 0.0], &[SeatRole::Driver]);
    let turret = vehicle(2, Class::Turret, [5.0, 0.0, 0.0], &[SeatRole::Gunner]);
    assert_eq!(
        take(&me, &sight(vec![banshee, turret]), &[], &w, still, none),
        None
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
fn the_approach_is_beside_the_seat_and_the_other_side_mirrors_it() {
    let mut g = ghost(1, [10.0, 0.0, 1.0]);
    g.forward = [0.0, 1.0, 0.0];
    let spec = Class::Scout.spec();
    let a = approach(&g, SeatRole::Driver, 0);
    // Facing +Y, left is -X.
    let want = [10.0 - spec.driver_at[1], spec.driver_at[0], 1.0];
    for k in 0..3 {
        assert!((a[k] - want[k]).abs() < 1e-5, "{a:?} {want:?}");
    }
    let b = approach(&g, SeatRole::Driver, 1);
    assert!((b[0] - (10.0 + spec.driver_at[1])).abs() < 1e-5, "{b:?}");
    // A Warthog's gunner gets in from behind.
    let hog = warthog(10, [0.0; 3])[0].clone();
    let back = approach(&hog, SeatRole::Gunner, 0);
    assert!(back[0] < -1.0 && back[1].abs() < 1e-5, "{back:?}");
}
