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
fn back_in_view_soon_keeps_the_original_reaction() {
    let mut t = Tactics::new(7);
    let first = t.focus(1, 100, Skill::Normal);
    assert!((107..=113).contains(&first));
    assert_eq!(
        t.focus(1, 102, Skill::Normal),
        first,
        "keeps a pending reaction"
    );
    assert_eq!(t.focus(1, 102 + GRACE, Skill::Normal), first);
    let back = 102 + 3 * GRACE;
    assert!((back + 7..=back + 13).contains(&t.focus(1, back, Skill::Normal)));
    let newcomer = t.focus(2, back + 1, Skill::Legendary);
    assert!(newcomer - (back + 1) >= MIN_REACTION);
    t.reset();
    assert!(t.focus(1, newcomer + 1, Skill::Normal) > newcomer + 1);
}

#[test]
fn reaction_delays_are_bounded_variable_and_scale_by_skill() {
    let mut means = Vec::new();
    for (skill, range) in Skill::ALL.into_iter().zip([10..=18, 7..=13, 5..=9, 5..=8]) {
        let sample = |seed| Tactics::new(seed).focus(1, 204, skill) - 204;
        let delays: Vec<_> = (0..256).map(sample).collect();
        assert!(delays.iter().all(|d| range.contains(d)), "{skill:?}");
        assert_eq!(delays.iter().min(), Some(range.start()));
        assert_eq!(delays.iter().max(), Some(range.end()));
        assert_eq!(delays, (0..256).map(sample).collect::<Vec<_>>());
        means.push(delays.iter().sum::<u32>());
    }
    assert!(means.windows(2).all(|pair| pair[0] > pair[1]), "{means:?}");

    let mut t = Tactics::new(1);
    let origin = u32::MAX - 2;
    let ready = t.focus(1, origin, Skill::Legendary);
    assert!(!reached(origin, ready));
    assert!(reached(origin.wrapping_add(8), ready));
    assert_eq!(t.focus(1, 10, Skill::Legendary), ready);
}

#[test]
fn a_once_per_pull_trigger_is_pulled_and_let_go() {
    let mut t = Tactics::new(12);
    let pull = Pull::Tap(6);
    let down: Vec<_> = (0..75).map(|k| t.trigger(k, pull, true)).collect();
    let starts: Vec<_> = (1..down.len())
        .filter(|&k| !down[k - 1] && down[k])
        .collect();
    assert!(starts.len() >= 7, "{starts:?}");
    assert!(starts.windows(2).all(|w| (6..=9).contains(&(w[1] - w[0]))));
    for k in [0, starts[0], starts[1]] {
        assert!((2..=3).contains(&down[k..].iter().take_while(|d| **d).count()));
    }
    let mut t = Tactics::default();
    assert!(t.trigger(0, pull, true));
    assert!(t.trigger(1, pull, false), "finishes an allowed pull");
    t.stop_trigger(2);
    assert!(!t.trigger(2, pull, false));
}

#[test]
fn a_held_trigger_bursts_and_a_blade_never_pulls() {
    let mut t = Tactics::default();
    let burst = Pull::Hold(Some((12, 8)));
    let down: Vec<_> = (0..35).map(|k| t.trigger(k, burst, true)).collect();
    let on = down.iter().take_while(|d| **d).count();
    let off = down[on..].iter().take_while(|d| !**d).count();
    assert!((10..=14).contains(&on), "{down:?}");
    assert!((6..=10).contains(&off), "{down:?}");
    assert!(down[on + off]);
    let mut t = Tactics::default();
    assert!(t.trigger(4, Pull::Hold(None), true));
    assert!(!t.trigger(4, Pull::Hold(None), false));
    assert!(!t.trigger(4, Pull::Never, true));
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
