use super::*;
use crate::arms::{Ammo, Profile, Trigger};

fn gun(zoom: i16, shot_speed: f32) -> Gun {
    Gun {
        def: 1,
        ammo: Ammo::Rounds {
            loaded: 2,
            capacity: 2,
            reserve: 4,
            reserve_max: 8,
        },
        profile: Profile {
            trigger: Trigger::Single,
            zoom,
            pellets: 1,
            shot_speed,
            ..Profile::default()
        },
    }
}

#[test]
fn only_the_rocket_launcher_locks_on() {
    assert!(locks_on(&gun(1, 40.0)), "rocket launcher");
    assert!(!locks_on(&gun(0, 40.0)), "brute shot, fuel rod: no zoom");
    assert!(!locks_on(&gun(1, 400.0)), "battle rifle");
    assert!(!locks_on(&gun(2, 400.0)), "sniper rifle");
}

#[test]
fn a_vehicle_in_the_reticle_is_on_target_and_one_beside_it_is_not() {
    let eye = [0.0, 0.0, 0.6];
    let x = [1.0, 0.0, 0.0];
    assert!(on_target(eye, x, [20.0, 0.0, 0.3], 0.0));
    // The vehicle's size counts: half a unit off at 20 is in it.
    assert!(on_target(eye, x, [20.0, 0.5, 0.6], 0.0));
    assert!(!on_target(eye, x, [20.0, 3.0, 0.6], 0.0));
    assert!(on_target(eye, x, [20.0, 3.0, 0.6], 0.2), "a wider cone");
    assert!(!on_target(eye, x, [-20.0, 0.0, 0.6], 0.5), "behind");
    assert!(!on_target(eye, x, eye, 0.5), "no distance");
}

#[test]
fn it_locks_after_holding_the_vehicle_for_the_skills_time() {
    let k = LockSkill::of(Skill::Normal);
    let mut l = Lock::default();
    for t in 0..k.hold() {
        assert_eq!(l.step(7, true, 100 + t, &k), Locking::Holding, "tick {t}");
    }
    assert_eq!(l.step(7, true, 100 + k.hold(), &k), Locking::Locked);
    assert_eq!(l.on(), Some(7));
    // Off it this tick: not locked now, but the hold is kept...
    assert_eq!(l.step(7, false, 101 + k.hold(), &k), Locking::Holding);
    // ...and back on, locked again at once.
    assert_eq!(l.step(7, true, 102 + k.hold(), &k), Locking::Locked);
}

#[test]
fn off_target_past_the_grace_or_another_vehicle_starts_again() {
    let k = LockSkill::of(Skill::Heroic);
    let mut l = Lock::default();
    for t in 0..20 {
        l.step(7, true, t, &k);
    }
    // Off for longer than the grace.
    for t in 20..20 + k.grace + 2 {
        l.step(7, false, t, &k);
    }
    assert_eq!(l.on(), None);
    let t0 = 30;
    for t in t0..t0 + k.hold() {
        assert_eq!(l.step(7, true, t, &k), Locking::Holding);
    }
    // Another vehicle: from the start.
    assert_eq!(l.step(8, true, t0 + k.hold(), &k), Locking::Holding);
    assert_eq!(l.on(), Some(8));
    l.reset();
    assert_eq!(l.on(), None);
}

#[test]
fn a_shaky_hand_within_the_grace_keeps_the_hold() {
    let k = LockSkill::of(Skill::Legendary);
    let mut l = Lock::default();
    let mut locked_at = None;
    for t in 0..80 {
        // Off one tick in five.
        let on = t % 5 != 4;
        if l.step(7, on, t, &k) == Locking::Locked && locked_at.is_none() {
            locked_at = Some(t);
        }
    }
    let at = locked_at.expect("it locks");
    assert!(at >= k.hold() && at <= k.hold() + 1, "{at}");
}
