//! The combat boost in the mind (docs/game/bots.md, "Built: combat
//! boost"): the trigger per weapon, no dry fire, no re-reaction after a
//! moment out of view, retargeting a close threat, careful grenades, and
//! the movement's signals.

use super::*;
use crate::arms::{Ammo, Arms, Gun, Profile, Trigger};
use crate::brain::Me;

const X: Vec3 = [1.0, 0.0, 0.0];

fn enemy(id: u32, at: Vec3) -> Seen {
    Seen {
        id,
        position: at,
        team: 1,
    }
}
fn rounds(loaded: i16, reserve: i16) -> Ammo {
    Ammo::Rounds {
        loaded,
        capacity: 36,
        reserve,
        reserve_max: 108,
    }
}
fn holding(trigger: Trigger, ammo: Ammo) -> Arms {
    Arms {
        held: Some(Gun {
            def: 0xa001_0001,
            ammo,
            profile: Profile {
                trigger,
                zoom: 1,
                pellets: 1,
                shot_speed: 400.0,
                ..Profile::default()
            },
        }),
        other: None,
        grenades: [2, 0],
    }
}
fn step(m: &mut Mind, me: Me, others: Vec<Seen>, teams: bool, tick: u32) -> HostPadState {
    m.step(
        &Perception {
            observations: Default::default(),
            me: Some(me),
            others,
            teams,
            tick,
            pickups: vec![],
            objective: None,
        },
        None,
    )
}
fn bot(arms: Option<Arms>) -> Me {
    Me {
        position: [0.0; 3],
        aim: X,
        team: 0,
        arms,
        ..Me::default()
    }
}

#[test]
fn a_once_per_pull_gun_is_pulled_again_and_again() {
    // Holding the trigger of a battle rifle fires once: it pulls, lets
    // go and pulls again. Head height at Legendary is the eye's, so the
    // target dead ahead is on the crosshair.
    let mut m = Mind::new(Skill::Legendary, 3);
    let me = bot(Some(holding(Trigger::Single, rounds(36, 108))));
    let mut down = Vec::new();
    for t in 0..60 {
        let pad = step(&mut m, me, vec![enemy(1, [6.0, 0.0, 0.0])], false, t);
        down.push(pad.right_trigger > 0.0);
    }
    let pulls = down.windows(2).filter(|w| !w[0] && w[1]).count();
    assert!(pulls >= 4, "{down:?}");
    assert!(down[10..].iter().any(|d| !d), "it lets go");
    // An automatic is held down.
    let mut m = Mind::new(Skill::Legendary, 3);
    let me = bot(Some(holding(Trigger::Automatic, rounds(36, 108))));
    let held = (0..60)
        .map(|t| step(&mut m, me, vec![enemy(1, [6.0, 0.0, 0.0])], false, t).right_trigger)
        .filter(|&r| r > 0.0)
        .count();
    assert!(held >= 50, "{held}");
}

#[test]
fn it_never_fires_an_empty_magazine() {
    let mut m = Mind::new(Skill::Legendary, 3);
    let me = bot(Some(holding(Trigger::Automatic, rounds(0, 40))));
    let mut reloaded = false;
    for t in 0..60 {
        let pad = step(&mut m, me, vec![enemy(1, [6.0, 0.0, 0.0])], false, t);
        assert_eq!(pad.right_trigger, 0.0, "tick {t}");
        reloaded |= pad.buttons.contains(RELOAD);
    }
    assert!(reloaded);
    assert_eq!(m.kit, Kit::Reload);
}

#[test]
fn a_moment_out_of_view_does_not_freeze_it_again() {
    let r = Skill::Easy.aim().reaction;
    let at = [4.0, 4.0, 0.0];
    let run = |gone: u32| {
        let mut m = Mind::new(Skill::Easy, 7);
        for t in 0..=r + 5 {
            step(&mut m, bot(None), vec![enemy(1, at)], false, t);
        }
        for t in r + 6..r + 6 + gone {
            step(&mut m, bot(None), vec![], false, t);
        }
        step(&mut m, bot(None), vec![enemy(1, at)], false, r + 6 + gone).right_stick
    };
    // Back after a moment: turns at once.
    let soon = run(10);
    assert!(soon.0 < -0.5, "{soon:?}");
    // Back after long: a fresh reaction, standing still.
    assert_eq!(run(tactics::GRACE + 30), (0.0, 0.0));
}

#[test]
fn a_close_threat_takes_over_from_a_far_target() {
    let mut m = Mind::new(Skill::Normal, 1);
    for t in 0..12 {
        step(
            &mut m,
            bot(None),
            vec![enemy(1, [20.0, 0.0, 0.0])],
            false,
            t,
        );
    }
    assert_eq!(m.doing, Doing::Fight(1));
    let both = vec![enemy(1, [20.0, 0.0, 0.0]), enemy(2, [1.5, 1.0, 0.0])];
    step(&mut m, bot(None), both, false, 12);
    assert_eq!(m.doing, Doing::Fight(2));
    // Two at about the same range: it keeps the one it has.
    let mut m = Mind::new(Skill::Normal, 1);
    step(&mut m, bot(None), vec![enemy(1, [6.0, 0.0, 0.0])], false, 0);
    let both = vec![enemy(1, [6.0, 0.0, 0.0]), enemy(2, [5.5, 0.3, 0.0])];
    for t in 1..30 {
        step(&mut m, bot(None), both.clone(), false, t);
        assert_eq!(m.doing, Doing::Fight(1), "tick {t}");
    }
}

#[test]
fn no_grenade_into_a_teammate() {
    let throws = |mate: bool| {
        let mut m = Mind::new(Skill::Legendary, 5);
        let mut others = vec![enemy(1, [6.0, 0.0, 0.0])];
        if mate {
            others.push(Seen {
                id: 2,
                position: [6.5, 1.0, 0.0],
                team: 0,
            });
        }
        (0..180 * 30u32)
            .filter(|&t| step(&mut m, bot(None), others.clone(), true, t).left_trigger > 0.0)
            .count()
    };
    assert_eq!(throws(true), 0);
    assert!(throws(false) > 0);
}

#[test]
fn the_movement_is_told_the_range_and_when_to_take_cover() {
    let mut m = Mind::new(Skill::Normal, 1);
    let me = bot(Some(holding(Trigger::Single, rounds(36, 108))));
    step(&mut m, me, vec![enemy(1, [6.0, 0.0, 0.0])], false, 0);
    assert_eq!(m.fight.band, crate::arms::Role::Precise.band());
    assert!(!m.fight.wants_cover);
    // Shields down in a fight.
    let hurt = Me { shield: 0.1, ..me };
    step(&mut m, hurt, vec![enemy(1, [6.0, 0.0, 0.0])], false, 1);
    assert!(m.fight.wants_cover);
    // Reloading an empty magazine with the enemy close.
    let mut m = Mind::new(Skill::Normal, 1);
    let empty = bot(Some(holding(Trigger::Single, rounds(0, 40))));
    step(&mut m, empty, vec![enemy(1, [6.0, 0.0, 0.0])], false, 0);
    assert_eq!(m.kit, Kit::Reload);
    assert!(m.fight.wants_cover);
}
