use super::*;
use crate::arms::{Ammo, Gun, Profile, Trigger};

const X: Vec3 = [1.0, 0.0, 0.0];

fn foe(id: u32, at: Vec3) -> Seen {
    Seen {
        id,
        position: at,
        team: 1,
    }
}
fn gun(trigger: Trigger, zoom: i16, rate: f32, shot_speed: f32, loaded: i16) -> Gun {
    Gun {
        def: 1,
        ammo: Ammo::Rounds {
            loaded,
            capacity: 36,
            reserve: 36,
            reserve_max: 108,
        },
        profile: Profile {
            trigger,
            zoom,
            rate,
            pellets: 1,
            shot_speed,
            ..Profile::default()
        },
    }
}
fn rifle() -> Gun {
    gun(Trigger::Single, 1, 3.0, 400.0, 36)
}

#[test]
fn the_skills_pace_and_sense_in_order() {
    let s: Vec<Style> = Skill::ALL.iter().map(|&k| Style::of(k)).collect();
    for w in s.windows(2) {
        assert!(w[1].pace < w[0].pace);
    }
    assert!(
        !s[0].by_range && s[1].by_range,
        "Easy does not pick by range"
    );
    assert!(!s[1].head && s[2].head, "Heroic and up aim at the head");
}

#[test]
fn the_target_is_near_and_in_front_and_kept() {
    let eye = [0.0, 0.0, 0.6];
    // One ahead at 6, one just behind at 5: the one ahead.
    let seen = [foe(1, [6.0, 0.0, 0.0]), foe(2, [-5.0, 0.0, 0.0])];
    assert_eq!(choose(&seen, eye, X, None).map(|o| o.id), Some(1));
    // Two ahead: the current one is kept against a slightly nearer one...
    let seen = [foe(1, [6.0, 0.0, 0.0]), foe(2, [5.0, 0.3, 0.0])];
    assert_eq!(choose(&seen, eye, X, Some(1)).map(|o| o.id), Some(1));
    assert_eq!(choose(&seen, eye, X, None).map(|o| o.id), Some(2));
    // ...but not against one in its face while it shoots at a far one.
    let seen = [foe(1, [20.0, 0.0, 0.0]), foe(2, [1.5, 1.0, 0.0])];
    assert_eq!(choose(&seen, eye, X, Some(1)).map(|o| o.id), Some(2));
    assert_eq!(choose(&[], eye, X, Some(1)), None);
}

#[test]
fn back_in_view_soon_needs_no_new_reaction() {
    let mut t = Tactics::default();
    let a = [foe(1, [6.0, 0.0, 0.0])];
    t.see(&a, 100, 10);
    assert_eq!(t.since(1, 100), 100);
    t.see(&a, 110, 10);
    // Out of view for a moment, then back: still counted from 100.
    t.see(&[], 120, 10);
    t.see(&a, 110 + GRACE, 10);
    assert_eq!(t.since(1, 110 + GRACE), 100);
    // Out of view for longer: a new reaction. No one else was in view
    // lately, so the whole of it.
    let back = 110 + 3 * GRACE;
    t.see(&a, back, 10);
    assert_eq!(t.since(1, back), back);
    // In a fight already, a newcomer takes half the reaction.
    t.see(&[a[0], foe(2, [3.0, 3.0, 0.0])], back + 1, 10);
    assert_eq!(t.since(2, back + 1), back + 1 - 5);
    t.reset();
    assert_eq!(t.since(1, 7), 7);
}

#[test]
fn a_once_per_pull_trigger_is_pulled_and_let_go() {
    let mut t = Tactics::default();
    let pull = Pull::Tap(6);
    let down: Vec<bool> = (0..20).map(|k| t.trigger(k, 0, pull, true)).collect();
    let on: Vec<u32> = (0..20).filter(|&k| down[k as usize]).collect();
    assert_eq!(on, vec![0, 1, 6, 7, 12, 13, 18, 19]);
    // Not wanting: no new pull (one under way finishes).
    let mut t = Tactics::default();
    assert!(t.trigger(0, 0, pull, true));
    assert!(t.trigger(1, 0, pull, false), "finishes the pull");
    assert!(!t.trigger(6, 0, pull, false));
    assert!(t.trigger(7, 0, pull, true));
}

#[test]
fn a_held_trigger_bursts_and_a_blade_never_pulls() {
    let mut t = Tactics::default();
    let burst = Pull::Hold(Some((3, 2)));
    let on: Vec<u32> = (0..10).filter(|&k| t.trigger(k, 0, burst, true)).collect();
    assert_eq!(on, vec![0, 1, 2, 5, 6, 7]);
    assert!(t.trigger(4, 0, Pull::Hold(None), true));
    assert!(!t.trigger(4, 0, Pull::Hold(None), false));
    assert!(!t.trigger(4, 0, Pull::Never, true));
}

#[test]
fn the_shot_follows_the_gun_and_the_skill() {
    let normal = Style::of(Skill::Normal);
    let heroic = Style::of(Skill::Heroic);
    // The rifle: pulled at its rate (3 a second, every 10 ticks) plus the
    // skill's pace; a 400 a second bullet; head at Heroic, chest at Normal.
    let s = Shot::of(Some(rifle()), true, &normal, Some((24, 8)), 90.0);
    assert_eq!(s.pull, Pull::Tap(10 + normal.pace));
    assert_eq!(s.speed, 400.0);
    assert_eq!(s.height, CHEST);
    assert!(s.ready);
    assert_eq!(
        Shot::of(Some(rifle()), true, &heroic, None, 90.0).height,
        HEAD
    );
    // A very fast trigger still lets go between pulls.
    let fast = gun(Trigger::Single, 0, 30.0, 400.0, 36);
    assert_eq!(
        Shot::of(Some(fast), true, &heroic, None, 90.0).pull,
        Pull::Tap(PULL_MIN + heroic.pace)
    );
    // An automatic is held in the skill's bursts; its speed unknown, the
    // default lead speed.
    let smg = gun(Trigger::Automatic, 0, 15.0, 0.0, 60);
    let s = Shot::of(Some(smg), true, &normal, Some((24, 8)), 90.0);
    assert_eq!((s.pull, s.speed), (Pull::Hold(Some((24, 8))), 90.0));
    // Rockets at the feet, from Heroic; never at point blank.
    let rockets = gun(Trigger::Single, 1, 1.0, 30.0, 2);
    let s = Shot::of(Some(rockets), true, &heroic, None, 90.0);
    assert_eq!(s.height, FEET);
    assert!(!s.in_range(1.0) && s.in_range(10.0));
    // An empty magazine is not ready: no dry fire.
    assert!(
        !Shot::of(
            Some(gun(Trigger::Single, 1, 3.0, 400.0, 0)),
            true,
            &normal,
            None,
            90.0
        )
        .ready
    );
    assert!(
        !Shot::of(None, true, &normal, None, 90.0).ready,
        "nothing in hand"
    );
    // Arms not read: as before, held, ready, to 25.
    let blind = Shot::of(None, false, &normal, Some((24, 8)), 90.0);
    assert_eq!(blind.pull, Pull::Hold(Some((24, 8))));
    assert!(blind.ready && blind.in_range(24.0) && !blind.in_range(25.0));
}

#[test]
fn no_grenade_where_a_teammate_stands() {
    let target = [10.0, 0.0, 0.0];
    let mate = Seen {
        id: 3,
        position: [11.0, 1.0, 0.0],
        team: 0,
    };
    assert!(!grenade_safe(target, &[mate], true, 0));
    assert!(grenade_safe(target, &[mate], true, 1), "theirs, not ours");
    assert!(grenade_safe(target, &[mate], false, 0), "free for all");
    let far = Seen {
        position: [20.0, 0.0, 0.0],
        ..mate
    };
    assert!(grenade_safe(target, &[far], true, 0));
}

#[test]
fn it_pre_aims_where_they_went_when_walking_that_way() {
    let at = [0.0; 3];
    let last = [8.0, 2.0, 0.0];
    assert_eq!(pre_aim(at, last, Some([1.0, 0.0])), Some([8.0, 2.0, CHEST]));
    assert_eq!(pre_aim(at, last, None), Some([8.0, 2.0, CHEST]));
    // Walking away from it (round a wall): looks where it walks.
    assert_eq!(pre_aim(at, last, Some([-1.0, 0.0])), None);
    // Too far, or already there.
    assert_eq!(pre_aim(at, [PRE_AIM_RANGE + 1.0, 0.0, 0.0], None), None);
    assert_eq!(pre_aim(at, [0.5, 0.0, 0.0], None), None);
}

#[test]
fn the_movement_signals_follow_the_gun_and_the_danger() {
    use crate::arms::{Arms, Role};
    let mut t = Tactics::default();
    let a = Arms {
        held: Some(rifle()),
        other: None,
        grenades: [0, 0],
        ..Arms::default()
    };
    t.signal(Some(&a), Kit::Fine, false, Some(10.0));
    assert_eq!(t.band, Role::Precise.band());
    assert!(!t.wants_cover);
    // Reloading with the enemy within the band: cover.
    t.signal(Some(&a), Kit::Reload, false, Some(10.0));
    assert!(t.wants_cover);
    // Reloading with nobody about: no need.
    t.signal(Some(&a), Kit::Reload, false, None);
    assert!(!t.wants_cover);
    // Shields down: cover.
    t.signal(Some(&a), Kit::Fine, true, Some(10.0));
    assert!(t.wants_cover);
    // Melee: arm's length; arms not read: the weapon-blind band.
    t.signal(Some(&a), Kit::Melee, false, Some(2.0));
    assert_eq!(t.band, crate::arms::MELEE_BAND);
    t.signal(None, Kit::Fine, false, None);
    assert_eq!(t.band, Role::Unknown.band());
}
