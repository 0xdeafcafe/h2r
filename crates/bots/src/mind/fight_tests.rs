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
        ..Arms::default()
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
    assert!(soon.0 < -0.1, "{soon:?}");
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
        // It turns to each throw's arc before letting go (the game's
        // turning, roughly).
        let mut me = bot(None);
        let n = (0..180 * 30u32)
            .filter(|&t| {
                let pad = step(&mut m, me, others.clone(), true, t);
                me.aim = turned(me.aim, &pad);
                pad.left_trigger > 0.0
            })
            .count();
        (n, m.grenade.throws)
    };
    assert_eq!(throws(true), (0, 0));
    let (pressed, thrown) = throws(false);
    assert!(pressed > 0 && thrown > 0, "{pressed} {thrown}");
}

/// The aim after a tick of the right stick: the game's turning as the
/// hand models it ([`turn_rate`], the bots' sensitivity), one tick of it.
fn turned(aim: Vec3, pad: &HostPadState) -> Vec3 {
    let flat = (aim[0] * aim[0] + aim[1] * aim[1]).sqrt();
    let (rx, ry) = turn_rate(pad.right_stick);
    let yaw = aim[1].atan2(aim[0]) - rx / TICKS;
    let pitch = (aim[2].atan2(flat) + ry / TICKS).clamp(-1.4, 1.4);
    crate::grenades::direction(yaw, pitch)
}

// ---- weapon skill (crate::weapons, crate::grenades) ----

fn plasma_pistol() -> Gun {
    Gun {
        def: 0xa002_0002,
        ammo: Ammo::Battery(0.9),
        profile: Profile {
            trigger: Trigger::Charge,
            pellets: 1,
            shot_speed: 60.0,
            ..Profile::default()
        },
    }
}
fn rifle() -> Gun {
    holding(Trigger::Single, rounds(36, 108)).held.unwrap()
}

#[test]
fn a_noob_combo_holds_the_charge_lets_go_and_switches() {
    // Most Legendary bots go for it on a target dead ahead.
    let combos = (1..=10u64)
        .filter(|&seed| {
            let mut m = Mind::new(Skill::Legendary, seed);
            let me = bot(Some(Arms {
                held: Some(plasma_pistol()),
                other: Some(rifle()),
                grenades: [0, 0],
                ..Arms::default()
            }));
            let mut held = 0;
            let mut longest = 0;
            let mut switched_after = false;
            for t in 0..90 {
                let pad = step(&mut m, me, vec![enemy(1, [6.0, 0.0, 0.0])], false, t);
                if pad.right_trigger > 0.0 {
                    held += 1;
                    longest = longest.max(held);
                } else {
                    held = 0;
                }
                switched_after |=
                    longest >= crate::weapons::CHARGE_TICKS && pad.buttons.contains(SWITCH);
            }
            longest >= crate::weapons::CHARGE_TICKS && switched_after
        })
        .count();
    assert!(combos >= 6, "{combos} of 10");
    // Easy taps the pistol and stays on it.
    let mut m = Mind::new(Skill::Easy, 2);
    let me = bot(Some(Arms {
        held: Some(plasma_pistol()),
        other: None,
        grenades: [0, 0],
        ..Arms::default()
    }));
    let mut held = 0;
    for t in 0..90 {
        let pad = step(&mut m, me, vec![enemy(1, [6.0, 0.0, 0.0])], false, t);
        held = if pad.right_trigger > 0.0 { held + 1 } else { 0 };
        assert!(held < crate::weapons::CHARGE_TICKS, "tick {t}");
    }
}

#[test]
fn dual_wielding_fires_the_left_gun_and_throws_no_grenades() {
    let smg = Gun {
        def: 0xa003_0003,
        ammo: rounds(60, 120),
        profile: Profile {
            trigger: Trigger::Automatic,
            pellets: 1,
            shot_speed: 300.0,
            ..Profile::default()
        },
    };
    let me = bot(Some(Arms {
        held: Some(smg),
        left: Some(smg),
        other: Some(rifle()),
        grenades: [4, 4],
        ..Arms::default()
    }));
    let mut m = Mind::new(Skill::Legendary, 4);
    let mut left = 0;
    let mut aim = me;
    for t in 0..(60 * 30u32) {
        let pad = step(&mut m, aim, vec![enemy(1, [5.0, 0.0, 0.0])], false, t);
        aim.aim = turned(aim.aim, &pad);
        left += usize::from(pad.left_trigger > 0.0);
        assert!(!pad.buttons.contains(SWAP_GRENADES), "tick {t}");
    }
    assert!(left > 100, "{left}");
    assert_eq!(m.grenade.throws, 0);
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

#[test]
fn a_due_powerup_stays_the_destination_during_a_fight() {
    use crate::decisions::supplies::{Powerup, Supply};
    let site = [6.0, 0.0, 0.0];
    let item = Supply {
        site: 0,
        object: 23,
        spawn: site,
        at: site,
        disabled: false,
        on_disturbance: true,
        respawn: Some(30),
        powerup: Some(Powerup::Camo),
    };
    let me = bot(Some(holding(Trigger::Automatic, rounds(36, 108))));
    let mut m = Mind::new(Skill::Legendary, 14);
    let mut p = Perception {
        me: Some(me),
        ..Default::default()
    };
    p.observations.supplies = Some(vec![item]);
    m.step(&p, None);
    p.tick = 1;
    p.observations.supplies = Some(vec![]);
    p.observations.service_rate = Some(30);
    let foe = enemy(2, [13.0, 3.0, 0.0]);
    p.others.push(foe);
    let (_, intent) = m.decide(&p, me, &p.others, None);
    assert_eq!(intent.chosen.unwrap().id, foe.id);
    assert_eq!(intent.anchor, Some((site, crate::nav::REACHED)));
}

#[test]
fn holding_y_requests_a_second_smg_even_when_the_primary_is_full() {
    let mut a = holding(Trigger::Automatic, rounds(36, 108));
    let smg = a.held.as_mut().unwrap();
    smg.profile.dual = Some(arms::DualGun::Smg);
    let pickup = arms::Pickup {
        id: 9,
        position: [0.1, 0.0, 0.0],
        gun: *smg,
    };
    let mut p = Perception {
        me: Some(bot(Some(a))),
        pickups: vec![pickup],
        ..Default::default()
    };
    let mut m = Mind::new(Skill::Heroic, 77);
    for t in 0..10 {
        p.tick = t;
        let pad = m.step(&p, None);
        assert!(pad.buttons.contains(SWITCH));
        assert!(!pad.buttons.contains(RELOAD));
    }
    a.left = Some(pickup.gun);
    p.me = Some(bot(Some(a)));
    p.tick += 1;
    assert!(!m.step(&p, None).buttons.contains(SWITCH));
}

#[test]
fn the_dual_plasma_pistol_charges_and_releases_in_either_hand() {
    use arms::DualGun;
    let pistol = Gun {
        def: 12,
        ammo: Ammo::Battery(1.0),
        profile: Profile {
            dual: Some(DualGun::PlasmaPistol),
            trigger: Trigger::Charge,
            shot_speed: 11.0,
            ..Default::default()
        },
    };
    let smg = Gun {
        def: 13,
        ammo: rounds(36, 108),
        profile: Profile {
            dual: Some(DualGun::Smg),
            trigger: Trigger::Automatic,
            shot_speed: 400.0,
            ..Default::default()
        },
    };
    for right in [true, false] {
        let me = bot(Some(Arms {
            held: Some(if right { pistol } else { smg }),
            left: Some(if right { smg } else { pistol }),
            ..Default::default()
        }));
        let mut m = Mind::new(Skill::Legendary, 4);
        let mut streak = 0;
        let mut charged_release = false;
        let mut automatic_pulls = 0;
        let mut aim = me;
        for tick in 0..180 {
            let pad = step(&mut m, aim, vec![enemy(2, [6.0, 0.0, 0.0])], false, tick);
            aim.aim = turned(aim.aim, &pad);
            let (pistol, smg) = if right {
                (pad.right_trigger, pad.left_trigger)
            } else {
                (pad.left_trigger, pad.right_trigger)
            };
            automatic_pulls += usize::from(smg > 0.0);
            if pistol > 0.0 {
                streak += 1;
            } else {
                charged_release |= streak >= crate::weapons::CHARGE_TICKS;
                streak = 0;
            }
        }
        assert!(
            charged_release && automatic_pulls > 0,
            "pistol in right hand: {right}"
        );
    }
}

#[test]
fn juggernaut_transfer_changes_the_fight_and_the_bot_melees_as_juggernaut() {
    use crate::objective::{Game, Objective, State};
    let mut m = Mind::new(Skill::Legendary, 7);
    let mut p = Perception {
        me: Some(bot(Some(holding(Trigger::Automatic, rounds(36, 108))))),
        others: vec![enemy(2, [8.0, 0.0, 0.0]), enemy(3, [1.0, 0.0, 0.0])],
        objective: Some(Objective {
            game: Game::Juggernaut,
            state: State::Juggernauts(4),
            me: 1,
            items: vec![],
            spots: vec![],
            hill: None,
        }),
        ..Default::default()
    };
    m.step(&p, None);
    assert_eq!(m.doing, Doing::Fight(2));
    p.objective.as_mut().unwrap().state = State::Juggernauts(2);
    p.others.remove(0);
    let mut hit = false;
    for tick in 1..90 {
        p.tick = tick;
        let pad = m.step(&p, None);
        assert_eq!(m.doing, Doing::Fight(3));
        hit |= pad.buttons.contains(MELEE);
        let me = p.me.as_mut().unwrap();
        me.aim = turned(me.aim, &pad);
    }
    assert!(hit);
}
