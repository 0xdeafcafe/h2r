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
        "Normal also switches instead of reloading under fire"
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
    let down: Vec<_> = (0..75).map(|k| t.trigger(k, pull, true, true)).collect();
    let starts: Vec<_> = (1..down.len())
        .filter(|&k| !down[k - 1] && down[k])
        .collect();
    assert!(starts.len() >= 7, "{starts:?}");
    assert!(starts.windows(2).all(|w| (6..=9).contains(&(w[1] - w[0]))));
    for k in [0, starts[0], starts[1]] {
        assert!((2..=3).contains(&down[k..].iter().take_while(|d| **d).count()));
    }
    let mut t = Tactics::default();
    assert!(t.trigger(0, pull, true, true));
    assert!(t.trigger(1, pull, false, false), "finishes an allowed pull");
    t.stop_trigger(2);
    assert!(!t.trigger(2, pull, false, false));
}

#[test]
fn a_held_trigger_bursts_and_a_blade_never_pulls() {
    let mut t = Tactics::default();
    let burst = Pull::Hold(Some((12, 8)));
    let down: Vec<_> = (0..35).map(|k| t.trigger(k, burst, true, true)).collect();
    let on = down.iter().take_while(|d| **d).count();
    let off = down[on..].iter().take_while(|d| !**d).count();
    assert!((10..=14).contains(&on), "{down:?}");
    assert!((6..=10).contains(&off), "{down:?}");
    assert!(down[on + off]);
    let mut t = Tactics::default();
    assert!(
        !t.trigger(3, Pull::Hold(None), false, true),
        "pressed on target only"
    );
    assert!(t.trigger(4, Pull::Hold(None), true, true));
    t.stop_trigger(5);
    assert!(!t.trigger(5, Pull::Hold(None), true, true), "let go");
    assert!(!t.trigger(8, Pull::Never, true, true));
}

/// The longest run of trigger-down ticks.
fn longest(down: &[bool]) -> usize {
    down.split(|d| !d).map(<[bool]>::len).max().unwrap_or(0)
}

#[test]
fn a_tick_off_target_does_not_drop_the_trigger() {
    for pull in [Pull::Hold(None), Pull::Hold(Some((40, 8)))] {
        // Off the cone and out of the wider one for three ticks mid-hold:
        // still down.
        let mut t = Tactics::new(3);
        let on = |k| !(10..13).contains(&k);
        let down: Vec<_> = (0..30).map(|k| t.trigger(k, pull, on(k), on(k))).collect();
        assert!(down.iter().all(|&d| d), "{pull:?} {down:?}");
        // For eight: it lets go after its grace, and presses again once
        // back on.
        let mut t = Tactics::new(3);
        let on = |k| !(10..18).contains(&k);
        let down: Vec<_> = (0..30).map(|k| t.trigger(k, pull, on(k), on(k))).collect();
        let grace = 10 + HOLD_GRACE as usize;
        assert!(down[..grace].iter().all(|&d| d), "{pull:?} {down:?}");
        assert!(!down[grace..18].iter().any(|&d| d), "{pull:?} {down:?}");
        assert!(down[20..].iter().all(|&d| d), "{pull:?} {down:?}");
        // Off the cone but near it, however long: held.
        let mut t = Tactics::new(3);
        let down: Vec<_> = (0..30).map(|k| t.trigger(k, pull, k < 5, true)).collect();
        assert!(down.iter().all(|&d| d), "{pull:?} {down:?}");
    }
    // A burst let go early is finished on the next press, with no gap.
    let mut t = Tactics::new(3);
    let pull = Pull::Hold(Some((20, 30)));
    let on = |k| !(5..12).contains(&k);
    let down: Vec<_> = (0..40).map(|k| t.trigger(k, pull, on(k), on(k))).collect();
    let second = down[9..].iter().position(|&d| d).map(|k| k + 9);
    assert_eq!(second, Some(12), "{down:?}");
    let total = down[..40].iter().filter(|&&d| d).count();
    assert!((18..=26).contains(&total), "one burst: {total} {down:?}");
    // The pattern changing as the target crosses the band's end keeps
    // the hold down.
    let mut t = Tactics::new(3);
    assert!(t.trigger(0, Pull::Hold(None), true, true));
    assert!(t.trigger(1, Pull::Hold(Some((12, 8))), true, true));
}

#[test]
fn an_automatic_is_held_inside_its_band() {
    for skill in Skill::ALL {
        let aim = skill.aim();
        for spray in [0.0, 1.0] {
            // An SMG at 4 units: inside its band at every skill.
            let hold = hold_pattern(4.0, 9.0, aim.burst, spray);
            assert_eq!(hold, None, "{skill:?}");
            let smg = gun(Trigger::Automatic, 0, 15.0, 300.0, 60);
            let shot = Shot::of(Some(smg), true, &Style::of(skill), hold, 90.0);
            assert_eq!(shot.pull, Pull::Hold(None));
            // A thumb that wanders round the cone's edge: on four ticks
            // in five, near all but one in thirty.
            let mut t = Tactics::new(skill as u64 + 1);
            let down: Vec<_> = (0..120u32)
                .map(|k| t.trigger(k, shot.pull, k % 5 != 0, k % 30 != 29))
                .collect();
            assert!(longest(&down) >= 45, "{skill:?} {down:?}");
        }
    }
}

#[test]
fn at_range_a_disciplined_name_bursts_and_easy_sprays() {
    let burst = Skill::Normal.aim().burst;
    assert!(burst.is_some());
    // Past the band, a disciplined name bursts shorter than a sprayer.
    let calm = hold_pattern(14.0, 9.0, burst, 0.0).unwrap();
    let wild = hold_pattern(14.0, 9.0, burst, 1.0).unwrap();
    assert!(calm.0 < burst.unwrap().0 && wild.0 > burst.unwrap().0);
    assert_eq!((calm.1, wild.1), (burst.unwrap().1, burst.unwrap().1));
    let run = |pattern| {
        let mut t = Tactics::new(9);
        let down: Vec<_> = (0..150u32)
            .map(|k| t.trigger(k, Pull::Hold(pattern), true, true))
            .collect();
        (longest(&down), down.iter().filter(|&&d| !d).count())
    };
    let (calm_burst, calm_gaps) = run(Some(calm));
    let (wild_burst, _) = run(Some(wild));
    assert!(calm_burst < wild_burst, "{calm_burst} {wild_burst}");
    assert!(calm_gaps > 0);
    // Easy holds at range too, and so does any skill with bursts tuned off.
    for spray in [0.0, 1.0] {
        let easy = Skill::Easy.aim().burst;
        assert_eq!(hold_pattern(14.0, 9.0, easy, spray), None);
        assert_eq!(run(None), (150, 0));
    }
    // Heroic and Legendary burst at range as well.
    for skill in [Skill::Heroic, Skill::Legendary] {
        assert!(hold_pattern(14.0, 9.0, skill.aim().burst, 0.5).is_some());
    }
    // The name's spray: its aggression and reaction jitter.
    let w = |aggression, reaction_jitter| crate::name_profile::Weights {
        aggression,
        reaction_jitter,
        ..crate::name_profile::Weights::from([0.5; 12])
    };
    assert_eq!(spray(None), 0.5);
    assert!(spray(Some(w(0.1, 0.2))) < spray(Some(w(0.9, 0.8))));
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
    let s = Shot::of(Some(smg), true, &normal, None, 90.0);
    assert_eq!(s.pull, Pull::Hold(None));
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
    // Arms not read: held, ready, to the weapon-blind band's fire range
    // (the rule every gun fires by: 15 times FIRE_PAST_BAND).
    let blind = Shot::of(None, false, &normal, Some((24, 8)), 90.0);
    assert_eq!(blind.pull, Pull::Hold(Some((24, 8))));
    assert!(blind.ready && blind.in_range(18.0) && !blind.in_range(19.0));
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

#[test]
fn a_melee_finisher_requires_reach_and_facing() {
    assert!(melee_window(0.8, 0.1, Some(rifle()), false, false));
    assert!(!melee_window(1.1, 0.1, Some(rifle()), false, false));
    assert!(melee_window(1.1, 0.1, Some(rifle()), true, false));
    let mut plasma = rifle();
    plasma.profile.dual = Some(crate::arms::DualGun::PlasmaRifle);
    assert!(melee_window(1.1, 0.1, Some(plasma), false, false));
    assert!(!melee_window(1.1, 1.5, Some(plasma), true, false));
    assert!(!melee_window(1.8, 0.1, Some(plasma), true, false));
}

#[test]
fn a_softened_target_is_run_down_with_a_short_gun_but_not_a_long_one() {
    let mut plasma = gun(Trigger::Automatic, 0, 10.0, 60.0, 36);
    plasma.profile.dual = Some(crate::arms::DualGun::PlasmaRifle);
    // Shield down within a few metres: in for the beatdown.
    assert!(finish_push(Some(plasma), 4.0, true));
    assert!(!finish_push(Some(plasma), 4.0, false), "full shield: keep strafing");
    assert!(!finish_push(Some(plasma), FINISH_PUSH + 0.5, true), "too far to run down");
    // A rifle's headshot finishes at range; only point blank is a beatdown.
    assert!(!finish_push(Some(rifle()), 4.0, true));
    assert!(finish_push(Some(rifle()), 2.0, true));
    // Nothing loaded: running in is all it has.
    let empty = gun(Trigger::Single, 1, 3.0, 400.0, 0);
    assert!(finish_push(Some(empty), 4.0, true));
    // A sniper or a splash gun never runs in.
    assert!(!finish_push(Some(gun(Trigger::Single, 2, 1.0, 900.0, 4)), 2.0, true));
    assert!(!finish_push(Some(gun(Trigger::Single, 0, 1.0, 40.0, 4)), 2.0, true));
}

#[test]
fn name_jitter_biases_fresh_reactions_within_the_skill_window() {
    let mut fast_sum = 0;
    let mut slow_sum = 0;
    for seed in 0..256 {
        let fast = Tactics::new(seed).focus_with_jitter(1, 0, Skill::Normal, Some(0.26));
        let slow = Tactics::new(seed).focus_with_jitter(1, 0, Skill::Normal, Some(0.79));
        assert!(fast <= slow && fast >= 7 && slow <= 13);
        fast_sum += fast;
        slow_sum += slow;
    }
    assert!(fast_sum < slow_sum);
}
