//! Ranges from the tags, the noob combo, the melee finish, the left hand.

use super::*;
use crate::arms::{Ammo, Gun, Profile, Trigger};

fn profile(trigger: Trigger, zoom: i16, pellets: i16, shot_speed: f32, reach: f32) -> Profile {
    Profile {
        trigger,
        zoom,
        pellets,
        shot_speed,
        cone_range: reach,
        pull_range: reach,
        rate: 6.0,
        ..Profile::default()
    }
}
fn gun(p: Profile, ammo: Ammo) -> Gun {
    Gun {
        def: 1,
        ammo,
        profile: p,
    }
}
fn full() -> Ammo {
    Ammo::Rounds {
        loaded: 36,
        capacity: 36,
        reserve: 108,
        reserve_max: 108,
    }
}
fn br() -> Gun {
    gun(profile(Trigger::Single, 1, 1, 400.0, 20.0), full())
}
fn magnum() -> Gun {
    gun(profile(Trigger::Single, 0, 1, 400.0, 10.0), full())
}
fn smg() -> Gun {
    gun(profile(Trigger::Automatic, 0, 1, 300.0, 8.0), full())
}
fn pistol(battery: f32) -> Gun {
    gun(
        Profile {
            dual: Some(crate::arms::DualGun::PlasmaPistol),
            ..profile(Trigger::Charge, 0, 1, 60.0, 10.0)
        },
        Ammo::Battery(battery),
    )
}

#[test]
fn the_aim_assist_reach_sets_where_a_gun_stops_being_good() {
    // The battle rifle's band ends at its reach, the magnum's sooner
    // (kept to half the role's at least), the SMG's at its own.
    assert_eq!(br().reach().band, (5.0, 20.0));
    assert_eq!(br().reach().fire, (0.0, 32.0));
    assert_eq!(magnum().reach().band, (5.0, 14.0));
    assert!(magnum().reach().band.1 < br().reach().band.1);
    assert_eq!(smg().reach().band, (1.5, 8.0));
    assert!((smg().reach().fire.1 - 12.8).abs() < 1e-4);
    // Nothing read: the role's.
    let blind = gun(profile(Trigger::Single, 1, 1, 400.0, 0.0), full());
    assert_eq!(blind.reach().band, Role::Precise.band());
    assert_eq!(blind.reach().fire, Role::Precise.fire_range());
    // A sniper and rockets keep their role's, whatever their reach.
    let sniper = gun(profile(Trigger::Single, 2, 1, 900.0, 5.0), full());
    assert_eq!(sniper.reach().band, Role::Sniper.band());
    // A sword lunges as far as its aim assist reaches, within limits.
    let sword = |r: f32| {
        gun(
            Profile {
                trigger: Trigger::Absent,
                cone_range: r,
                ..Profile::default()
            },
            Ammo::None,
        )
    };
    assert_eq!(sword(2.0).reach().lunge, 2.0);
    assert_eq!(sword(9.0).reach().lunge, LUNGE_LIMITS.1);
    assert_eq!(sword(2.0).reach().band, (0.0, 2.0));
    assert_eq!(br().reach().lunge, 0.0);
    assert_eq!(misfit((5.0, 20.0), 10.0), 0.0);
    assert!((misfit((5.0, 20.0), 30.0) - 0.5).abs() < 1e-6);
}

#[test]
fn guns_are_short_or_long() {
    assert_eq!(class(&br()), Class::Long);
    assert_eq!(class(&smg()), Class::Short);
    assert_eq!(class(&pistol(1.0)), Class::Short);
    let shotgun = gun(profile(Trigger::Single, 0, 15, 300.0, 0.0), full());
    assert_eq!(class(&shotgun), Class::Short);
    let blind = Gun {
        def: 1,
        ammo: full(),
        ..Gun::default()
    };
    assert_eq!(class(&blind), Class::Other);
}

fn input(tick: u32, held: Gun, other: Gun, dist: f32, firing: bool) -> ComboIn {
    ComboIn {
        tick,
        target: Some(7),
        dist,
        on_target: true,
        reacted: true,
        shield_down: false,
        combo_chance: None,
        held: Some(held),
        other: Some(other),
        firing,
        glitch: Default::default(),
    }
}

/// A combo whose generator starts from a spread-out seed.
fn seeded(seed: u64) -> Combo {
    Combo::new(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15))
}

/// A combo that rolled to go on its first target (Legendary goes nine
/// times in ten).
fn eager(skill: Skill) -> Combo {
    (1..200u64)
        .map(seeded)
        .find(|c| {
            let mut c = c.clone();
            c.step(&input(0, pistol(0.9), br(), 6.0, false), skill)
                .trigger
                == Some(true)
        })
        .expect("one goes")
}

#[test]
fn the_noob_combo_charges_lets_go_and_switches_to_the_rifle() {
    let skill = Skill::Legendary;
    let k = ComboSkill::of(skill);
    let mut c = eager(skill);
    let mut log = Vec::new();
    let mut held = pistol(0.9);
    let mut switched_at = None;
    for t in 0..120 {
        let out = c.step(&input(t, held, br(), 6.0, false), skill);
        if out.switch && switched_at.is_none() {
            switched_at = Some(t);
        }
        // The game switches a few ticks after the press.
        if switched_at.is_some_and(|s| t >= s + 4) {
            held = br();
        }
        log.push(out);
    }
    // Charging: held from the start for the charge time.
    let charged = log.iter().take_while(|o| o.trigger == Some(true)).count();
    assert_eq!(charged as u32, CHARGE_TICKS, "{log:?}");
    // Let go, then the switch, the skill's delay later.
    assert_eq!(log[charged].trigger, Some(false));
    let s = switched_at.expect("switched") as usize;
    assert_eq!(s, charged + k.swap_delay as usize, "{log:?}");
    assert!(log[charged..s].iter().all(|o| o.trigger == Some(false)));
    // Busy (the weapons decision holds off) until the follow-up is over.
    assert!(log[s + 10].busy);
    assert!(!log[119].busy);
    assert_eq!(log[s + 10].trigger, None, "the rifle fires as usual");
    assert_eq!(c.hits, 0, "a trigger release alone cannot confirm a hit");
}

#[test]
fn an_overcharge_waits_for_the_aim_but_not_for_ever() {
    let skill = Skill::Legendary;
    let mut c = eager(skill);
    let off = |t: u32| ComboIn {
        on_target: false,
        ..input(t, pistol(0.9), br(), 6.0, false)
    };
    let held: Vec<bool> = (0..60)
        .map(|t| c.step(&off(t), skill).trigger == Some(true))
        .collect();
    let n = held.iter().take_while(|&&h| h).count() as u32;
    assert_eq!(n, CHARGE_HOLD, "{held:?}");
}

#[test]
fn with_the_rifle_in_hand_it_draws_the_pistol_to_open() {
    let skill = Skill::Legendary;
    let mut c = (1..200u64)
        .map(seeded)
        .find(|c| {
            let mut c = c.clone();
            c.step(&input(0, br(), pistol(0.9), 6.0, false), skill)
                .switch
        })
        .expect("one goes");
    // Drawn: it charges.
    let out = c.step(&input(1, br(), pistol(0.9), 6.0, false), skill);
    assert!(out.switch && out.busy);
    let out = c.step(&input(5, pistol(0.9), br(), 6.0, false), skill);
    assert_eq!(out.trigger, Some(true));
    // A flat battery: no combo at all.
    let mut c = Combo::new(1);
    for t in 0..60 {
        let out = c.step(&input(t, pistol(0.05), br(), 6.0, false), skill);
        assert_eq!(out, ComboOut::default(), "tick {t}");
    }
}

#[test]
fn easy_rarely_combos_and_legendary_nearly_always() {
    let goes = |skill: Skill| {
        (1..=200u64)
            .filter(|&seed| {
                let mut c = seeded(seed);
                c.step(&input(0, pistol(0.9), br(), 6.0, false), skill)
                    .trigger
                    == Some(true)
            })
            .count()
    };
    let (easy, legendary) = (goes(Skill::Easy), goes(Skill::Legendary));
    assert!(easy < 50, "{easy}");
    assert!(legendary > 150, "{legendary}");
    assert!(goes(Skill::Normal) < goes(Skill::Heroic));
}

#[test]
fn a_melee_finishes_a_target_it_has_been_hitting() {
    let run = |skill: Skill, pulls: u32| {
        let mut c = Combo::new(9);
        let mut melee = false;
        // Pulls at 5 away, then at arm's length.
        for t in 0..pulls * 6 {
            let fire = t % 6 < 2;
            c.step(&input(t, br(), smg(), 5.0, fire), skill);
        }
        for t in pulls * 6..pulls * 6 + 30 {
            melee |= c.step(&input(t, br(), smg(), 1.0, false), skill).melee;
        }
        melee
    };
    let k = ComboSkill::of(Skill::Legendary);
    assert!(run(Skill::Legendary, k.finish_pulls));
    assert!(!run(Skill::Legendary, k.finish_pulls - 1), "not yet");
    assert!(!run(Skill::Easy, 10), "Easy never finishes");
}

#[test]
fn the_left_hand_charges_taps_or_holds_by_its_trigger() {
    let mut l = LeftHand::default();
    let pp = pistol(0.9);
    // Charges for the charge time, then lets go on target.
    let held: Vec<bool> = (0..40).map(|t| l.trigger(t, &pp, true)).collect();
    assert!(held[..CHARGE_TICKS as usize].iter().all(|&h| h));
    assert!(!held[CHARGE_TICKS as usize]);
    // An automatic: while it wants to.
    let mut l = LeftHand::default();
    assert!(l.trigger(0, &smg(), true) && !l.trigger(1, &smg(), false));
    // Once per pull: pulled and let go.
    let mut l = LeftHand::default();
    let pulls: Vec<bool> = (0..20).map(|t| l.trigger(t, &magnum(), true)).collect();
    assert!(pulls[0] && pulls[1] && !pulls[2], "{pulls:?}");
    // Empty: never.
    let mut l = LeftHand::default();
    assert!(!l.trigger(0, &pistol(0.0), true));
}

#[test]
fn an_identified_smg_closes_distance_past_its_effective_reach() {
    let mut gun = smg();
    gun.profile.dual = Some(crate::arms::DualGun::Smg);
    assert_eq!(gun.reach().fire.1, 8.0);
    gun.profile.cone_range = 25.0;
    assert_eq!(gun.reach().fire.1, 9.0);
    assert_eq!(gun.reach().band.1, 9.0);
}

#[test]
fn the_combo_roll_is_once_per_viable_encounter_and_uses_the_name_weight() {
    let opportunities = |chance| {
        (0..256).filter(|&seed| {
            let mut combo = seeded(seed);
            let mut c = input(0, pistol(0.9), smg(), 6.0, false);
            c.combo_chance = Some(chance);
            assert!(!combo.step(&c, Skill::Normal).busy, "SMG is not a headshot follow-up");
            c.other = Some(br());
            c.dist = 40.0;
            assert!(!combo.step(&c, Skill::Normal).busy);
            c.tick = 1;
            c.dist = 6.0;
            let accepted = combo.step(&c, Skill::Normal).busy;
            if !accepted {
                // Changing the threshold cannot reroll an existing choice,
                // even when the same player ducks briefly around a corner.
                c.combo_chance = Some(1.0);
                for tick in 2..8 {
                    c.tick = tick;
                    assert!(!combo.step(&c, Skill::Normal).busy);
                }
                combo.leave_fight(8);
                c.tick = 10;
                assert!(!combo.step(&c, Skill::Normal).busy);
                combo.leave_fight(10 + crate::tactics::GRACE + 1);
                c.tick = 12 + crate::tactics::GRACE;
                assert!(combo.step(&c, Skill::Normal).busy);
            }
            accepted
        }).count()
    };
    let low = opportunities(0.2);
    let high = opportunities(0.8);
    assert!(low > 0 && low + 80 < high && high < 256, "{low} vs {high}");
}

#[test]
fn a_broken_shield_switches_to_a_loaded_headshot_weapon_and_raises_the_aim() {
    let mut combo = Combo::new(7);
    let mut c = input(0, pistol(0.8), br(), 7.0, false);
    c.combo_chance = Some(1.0);
    assert_eq!(combo.step(&c, Skill::Normal).trigger, Some(true));
    c.tick = 2;
    c.shield_down = true;
    let swap = combo.step(&c, Skill::Normal);
    assert!(swap.switch);
    assert_eq!(swap.trigger, Some(false));
    std::mem::swap(&mut c.held, &mut c.other);
    c.tick = 4;
    assert!(combo.step(&c, Skill::Normal).busy);
    assert!(matches!(combo.phase, Phase::Follow(_)));
    assert_eq!(target_height(c.held, true, crate::tactics::CHEST), crate::tactics::HEAD);
    assert_eq!(target_height(c.held, false, crate::tactics::CHEST), crate::tactics::CHEST);
    assert_eq!(target_height(Some(smg()), true, crate::tactics::CHEST), crate::tactics::CHEST);

    let mut dry = br();
    dry.ammo = Ammo::Rounds { loaded: 0, capacity: 36, reserve: 108, reserve_max: 108 };
    let mut c = input(0, pistol(0.8), dry, 7.0, false);
    c.combo_chance = Some(1.0);
    assert!(!Combo::new(9).step(&c, Skill::Normal).busy);
}
