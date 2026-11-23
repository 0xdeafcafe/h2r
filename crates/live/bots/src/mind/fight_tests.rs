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
fn a_shotgun_only_fires_at_clear_targets_inside_its_projectile_reach() {
    use crate::cache::MapData;
    use crate::los::Walls;

    let mut arms = holding(Trigger::Automatic, rounds(6, 24));
    let held = arms.held.as_mut().unwrap();
    held.profile.pellets = 15;
    held.profile.cone_range = 15.0;
    held.profile.damage_reach = 3.0;
    let me = bot(Some(arms));
    let wall = MapData {
        walls: Walls::new(vec![[[1.0, -10.0, -5.0], [1.0, 10.0, -5.0], [1.0, 0.0, 10.0]]]),
        ..Default::default()
    };

    for (distance, map, expect_fire) in [
        (6.0, None, false),
        (2.0, Some(&wall), false),
        (2.0, None, true),
    ] {
        let mut mind = Mind::new(Skill::Legendary, 3);
        let mut fires = false;
        for tick in 0..60 {
            let p = Perception {
                me: Some(me),
                others: vec![enemy(1, [distance, 0.0, 0.0])],
                tick,
                ..Default::default()
            };
            fires |= mind.step(&p, map).right_trigger > 0.0;
        }
        assert_eq!(
            fires, expect_fire,
            "distance {distance}, wall {}",
            map.is_some()
        );
    }
}

#[test]
fn dual_wielding_checks_the_range_of_each_gun() {
    let mut arms = holding(Trigger::Automatic, rounds(36, 108));
    let mut left = arms.held.unwrap();
    left.profile.damage_reach = 3.0;
    arms.left = Some(left);
    let me = bot(Some(arms));
    for (distance, expect_left) in [(6.0, false), (2.0, true)] {
        let mut mind = Mind::new(Skill::Legendary, 3);
        let mut fired = [false; 2];
        for tick in 0..60 {
            let pad = step(&mut mind, me, vec![enemy(1, [distance, 0.0, 0.0])], false, tick);
            fired[0] |= pad.right_trigger > 0.0;
            fired[1] |= pad.left_trigger > 0.0;
        }
        assert_eq!(fired, [true, expect_left]);
    }
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
        let mut me = bot(Some(holding(Trigger::Single, rounds(36, 108))));
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
/// hand models it ([`turn_rate`], the response table), one tick of it.
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
            dual: Some(crate::arms::DualGun::PlasmaPistol),
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
fn it_holds_fire_when_off_the_body_at_range() {
    // A rifle, Legendary (head height: the eye's), the aim 3° to the side.
    let aside = crate::grenades::direction(3f32.to_radians(), 0.0);
    let fires = |distance: f32| {
        let mut m = Mind::new(Skill::Legendary, 3);
        let me = Me {
            aim: aside,
            ..bot(Some(holding(Trigger::Single, rounds(36, 108))))
        };
        (0..60).any(|t| {
            step(&mut m, me, vec![enemy(1, [distance, 0.0, 0.0])], false, t).right_trigger > 0.0
        })
    };
    // 3° is a metre off the body at 20 units, a hand's width at 3.
    assert!(!fires(20.0));
    assert!(fires(3.0));
}

/// A dual-wielding bot: an SMG in each hand, a full magazine in the right
/// and `left` in the left, and four of each grenade.
fn dual(left: Ammo) -> Me {
    let smg = |ammo| Gun {
        def: 0xa003_0003,
        ammo,
        profile: Profile {
            dual: Some(arms::DualGun::Smg),
            trigger: Trigger::Automatic,
            pellets: 1,
            shot_speed: 300.0,
            ..Profile::default()
        },
    };
    bot(Some(Arms {
        held: Some(smg(rounds(60, 120))),
        left: Some(smg(left)),
        grenades: [4, 4],
        ..Arms::default()
    }))
}

#[test]
fn an_advancing_enemy_does_not_cost_the_left_gun() {
    for seed in 1..=6 {
        let mut m = Mind::new(Skill::Normal, seed);
        let mut me = dual(rounds(60, 120));
        let mut left = 0;
        // Walking straight in from 9 units at 1.5 a second, to 2.5.
        for t in 0..150u32 {
            let x = (9.0 - 1.5 * t as f32 / TICKS).max(2.5);
            let pad = step(&mut m, me, vec![enemy(1, [x, 0.0, 0.0])], false, t);
            me.aim = turned(me.aim, &pad);
            assert!(
                !pad.buttons.contains(SWITCH),
                "seed {seed} tick {t}: Y drops the left gun"
            );
            left += usize::from(pad.left_trigger > 0.0);
        }
        assert_eq!(m.grenade.throws, 0, "seed {seed}");
        assert!(left > 60, "seed {seed}: {left}");
    }
}

#[test]
fn both_triggers_are_down_together() {
    // Inside the SMG's band (held) and past it (bursts at Normal).
    for distance in [5.0, 12.0] {
        let mut m = Mind::new(Skill::Normal, 8);
        let mut me = dual(rounds(60, 120));
        let (mut either, mut both) = (0, 0);
        for t in 0..150u32 {
            let pad = step(&mut m, me, vec![enemy(1, [distance, 0.0, 0.0])], false, t);
            me.aim = turned(me.aim, &pad);
            let (r, l) = (pad.right_trigger > 0.0, pad.left_trigger > 0.0);
            either += usize::from(r || l);
            both += usize::from(r && l);
        }
        assert!(either > 40, "{distance}: {either}");
        assert!(both * 10 >= either * 8, "{distance}: {both} of {either}");
    }
}

#[test]
fn an_empty_left_magazine_is_pulled_to_reload() {
    let mut m = Mind::new(Skill::Legendary, 4);
    let me = dual(rounds(0, 120));
    let pulls: Vec<u32> = (0..90u32)
        .filter(|&t| step(&mut m, me, vec![enemy(1, [5.0, 0.0, 0.0])], false, t).left_trigger > 0.0)
        .collect();
    // A short pull, and again only after the retry wait.
    let starts: Vec<u32> = pulls
        .iter()
        .copied()
        .filter(|&t| t == 0 || !pulls.contains(&(t - 1)))
        .collect();
    assert!(!starts.is_empty(), "{pulls:?}");
    assert!(
        pulls.len() as u32 <= starts.len() as u32 * crate::tactics::PULL_TICKS,
        "{pulls:?}"
    );
    assert!(
        starts
            .windows(2)
            .all(|w| w[1] - w[0] >= crate::weapons::LEFT_DRY_RETRY),
        "{pulls:?}"
    );
    // A loaded left gun, the aim held well off the target, is not pulled:
    // the empty one's pull was for the reload, not a shot.
    let mut m = Mind::new(Skill::Legendary, 4);
    let mut me = dual(rounds(60, 120));
    me.aim = crate::grenades::direction(0.5, 0.0);
    for t in 0..20u32 {
        let pad = step(&mut m, me, vec![enemy(1, [5.0, 0.0, 0.0])], false, t);
        assert_eq!(pad.left_trigger, 0.0, "tick {t}");
    }
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
        role: crate::arms::Role::Unknown,
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
fn a_named_easy_seat_holds_y_only_for_accepted_pairs() {
    let mut a = holding(Trigger::Automatic, rounds(36, 108));
    let smg = a.held.as_mut().unwrap();
    smg.profile.dual = Some(arms::DualGun::Smg);
    let pickup = arms::Pickup { id: 9, position: [0.1, 0.0, 0.0], gun: *smg };
    let mut p = Perception {
        me: Some(bot(Some(a))),
        pickups: vec![pickup],
        ..Default::default()
    };
    let mut accepted = 0;
    for seed in 1..=64 {
        let mut brain = crate::Brain::new(Skill::Easy, seed).with_name_profile("Gunslinger");
        p.tick = 1;
        let first = brain.mind.step(&p, None).buttons.contains(SWITCH);
        accepted += usize::from(first);
        for t in 2..=8 {
            p.tick = t;
            let pad = brain.mind.step(&p, None);
            assert_eq!(pad.buttons.contains(SWITCH), first);
            assert!(!pad.buttons.contains(RELOAD));
        }
    }
    assert!(accepted > 0 && accepted < 64);
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

#[test]
fn a_calm_bot_walks_to_visible_grenades_it_can_use() {
    use crate::grenades::Kind;
    let mut m = Mind::new(Skill::Normal, 7);
    let mut a = holding(Trigger::Single, rounds(36, 108));
    a.grenades = [2, 0];
    let mut p = Perception {
        me: Some(bot(Some(a))),
        ..Default::default()
    };
    p.observations.ground_grenades = vec![
        ([2.0, 0.0, 0.0], Kind::Frag),
        ([5.0, 0.0, 0.0], Kind::Plasma),
    ];
    m.step(&p, None);
    assert_eq!(m.doing, Doing::Supply([5.0, 0.0, 0.0]));
    p.tick += 1;
    p.observations.ground_grenades.pop();
    m.step(&p, None);
    assert_ne!(m.doing, Doing::Supply([2.0, 0.0, 0.0]));
}

#[test]
fn a_sword_uses_the_trigger_to_lunge_and_b_to_slash_up_close() {
    for (dist, lunge) in [(1.8, true), (0.6, false)] {
        let mut m = Mind::new(Skill::Normal, 1);
        let mut a = holding(Trigger::Absent, Ammo::Battery(0.8));
        a.held.as_mut().unwrap().profile.cone_range = 2.0;
        let mut me = bot(Some(a));
        let mut pressed = false;
        for tick in 0..90 {
            let pad = step(&mut m, me, vec![enemy(1, [dist, 0.0, 0.0])], false, tick);
            me.aim = turned(me.aim, &pad);
            if lunge {
                pressed |= pad.right_trigger > 0.0;
                assert!(!pad.buttons.contains(MELEE));
            } else {
                pressed |= pad.buttons.contains(MELEE);
                assert_eq!(pad.right_trigger, 0.0);
            }
        }
        assert!(pressed);
    }
}

// ---- the start-of-match warm-up (mind::warmup) ----

#[test]
fn a_warm_up_is_two_to_four_seconds_stable_for_a_name_and_varied_across_names() {
    use super::warmup::{WARMUP_MAX_TICKS, WARMUP_MIN_TICKS, warmup_ticks};
    let names = ["Clanker", "Sprocket", "Rivet", "Gasket", "Widget", "Ratchet"];
    let ticks: Vec<u32> = names
        .iter()
        .map(|n| warmup_ticks(crate::name_profile::NameProfile::for_name(n).seed))
        .collect();
    for (n, t) in names.iter().zip(&ticks) {
        assert!((WARMUP_MIN_TICKS..=WARMUP_MAX_TICKS).contains(t), "{n}: {t}");
        let again = warmup_ticks(crate::name_profile::NameProfile::for_name(n).seed);
        assert_eq!(*t, again, "{n} is the same every match");
    }
    let mut distinct = ticks.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert!(distinct.len() >= 3, "{ticks:?}");
    assert_eq!(WARMUP_MIN_TICKS, 60);
    assert_eq!(WARMUP_MAX_TICKS, 120);
}

#[test]
fn a_warm_up_holds_until_its_tick_and_damage_ends_it() {
    use super::warmup::Warmup;
    let mut w = Warmup::Due;
    assert!(w.holding(100, 60, false));
    assert_eq!(w.left(130), Some(30));
    assert!(w.holding(159, 60, false));
    assert!(!w.holding(160, 60, false));
    assert_eq!(w, Warmup::Over);
    // It never starts again in that mind (a respawn).
    assert!(!w.holding(500, 60, false));
    let mut hurt = Warmup::Due;
    assert!(hurt.holding(0, 60, false));
    assert!(!hurt.holding(5, 60, true), "shot: over at once");
    assert!(!Warmup::default().holding(0, 60, false), "a mind not made for a match has none");
}

#[test]
fn a_bot_in_its_warm_up_does_not_shoot_and_fires_once_it_is_over() {
    let warm = super::warmup::warmup_ticks(3);
    let mut m = Mind::new(Skill::Legendary, 3);
    m.warmup = super::warmup::Warmup::Due;
    let me = Me {
        shield: 1.0,
        health: 1.0,
        ..bot(Some(holding(Trigger::Automatic, rounds(36, 108))))
    };
    let mut first = None;
    for t in 0..warm + 90 {
        let pad = step(&mut m, me, vec![enemy(1, [6.0, 0.0, 0.0])], false, t);
        let attacking = pad.right_trigger > 0.0
            || pad.left_trigger > 0.0
            || pad.buttons.contains(MELEE);
        if t < warm {
            assert!(!attacking, "tick {t} of a {warm}-tick warm-up");
            assert!(!matches!(m.doing, Doing::Fight(_)));
        } else if attacking && first.is_none() {
            first = Some(t);
        }
    }
    assert!(first.is_some(), "it fights after the warm-up");
}

#[test]
fn a_bot_shot_in_its_warm_up_fights_back_at_once() {
    let warm = super::warmup::warmup_ticks(3);
    let mut m = Mind::new(Skill::Legendary, 3);
    m.warmup = super::warmup::Warmup::Due;
    let fresh = Me {
        shield: 1.0,
        health: 1.0,
        ..bot(Some(holding(Trigger::Automatic, rounds(36, 108))))
    };
    let hit = Me {
        shield: 0.6,
        ..fresh
    };
    let mut first = None;
    for t in 0..warm {
        let me = if t < 5 { fresh } else { hit };
        let pad = step(&mut m, me, vec![enemy(1, [6.0, 0.0, 0.0])], false, t);
        if pad.right_trigger > 0.0 && first.is_none() {
            first = Some(t);
        }
    }
    let first = first.expect("it fired before the warm-up would have ended");
    assert!(first >= 5 && first < warm, "{first}");
}
