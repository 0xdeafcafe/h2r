use super::*;
use crate::arms::{Ammo, Arms, Gun, Profile, Trigger};

const PAD: Vec3 = [10.0, 0.0, 0.0];
const SUPPLY: Supply = Supply {
    site: 4,
    object: 72,
    spawn: PAD,
    at: [10.4, 0.0, 0.0],
    on_disturbance: true,
    respawn: Some(300),
    powerup: None,
};

fn bot() -> Me {
    Me {
        arms: Some(Arms::default()),
        ..Me::default()
    }
}

fn armed() -> Me {
    Me {
        arms: Some(Arms {
            held: Some(Gun {
                profile: Profile {
                    trigger: Trigger::Single,
                    zoom: 2,
                    ..Profile::default()
                },
                ammo: Ammo::Battery(1.0),
                ..Gun::default()
            }),
            ..Arms::default()
        }),
        ..Me::default()
    }
}

fn check(t: &mut Timers, tick: u32, seen: &[Supply]) {
    t.observe(tick, Some(seen), |_| true);
}

fn update(
    t: &mut Timers,
    tick: u32,
    clocks: &[Clock],
    players: &[Player],
    held: &[u32],
    takes: &[Take],
    visible: impl Fn(Vec3) -> bool,
) {
    let p = crate::brain::Perception {
        tick,
        observations: crate::brain::Observations {
            placement_clocks: clocks.to_vec(),
            supply_players: players.to_vec(),
            held_power: held.to_vec(),
            takes: takes.to_vec(),
            ..Default::default()
        },
        ..Default::default()
    };
    t.update_known(&p, visible);
}

#[test]
fn a_witnessed_pickup_starts_a_skill_scaled_trip_then_expires() {
    let mut t = Timers::default();
    check(&mut t, 0, &[SUPPLY]);
    assert_eq!(
        t.due(bot(), 0, Skill::Legendary, |_| false),
        None,
        "arms chooses guns on the floor"
    );
    check(&mut t, 1, &[]);
    assert_eq!(t.due(bot(), 120, Skill::Legendary, |_| false), None);
    assert_eq!(t.due(bot(), 240, Skill::Legendary, |_| false), Some(PAD));
    assert_eq!(t.due(bot(), 240, Skill::Easy, |_| false), None);
    assert_eq!(t.due(bot(), 296, Skill::Easy, |_| false), Some(PAD));
    assert_eq!(t.due(armed(), 296, Skill::Legendary, |_| false), None);
    assert_eq!(
        t.due(bot(), 296, Skill::Legendary, |_| true),
        None,
        "hot placement"
    );
    assert_eq!(t.due(bot(), 365, Skill::Legendary, |_| false), None);
}

#[test]
fn powerups_draw_an_armed_bot_to_the_item_and_later_to_the_placement() {
    let item = Supply {
        powerup: Some(crate::decisions::supplies::Powerup::Overshield),
        ..SUPPLY
    };
    let mut t = Timers::default();
    check(&mut t, 5, &[item]);
    assert_eq!(
        t.due(armed(), 5, Skill::Legendary, |_| false),
        Some(item.at)
    );
    check(&mut t, 6, &[]);
    assert_eq!(t.due(armed(), 7, Skill::Legendary, |_| false), None);
    assert_eq!(
        t.due(armed(), 300, Skill::Legendary, |_| false),
        Some(item.spawn)
    );
    check(&mut t, 306, &[item]);
    assert_eq!(
        t.due(armed(), 306, Skill::Legendary, |_| false),
        Some(item.at)
    );
}

#[test]
fn missing_scans_hidden_pads_and_long_gaps_do_not_date_a_pickup() {
    for gap in 0..3 {
        let mut t = Timers::default();
        t.observe(0, Some(&[SUPPLY]), |_| true);
        match gap {
            0 => t.observe(1, None, |_| true),
            1 => t.observe(1, Some(&[]), |_| false),
            _ => {}
        }
        let removed = if gap == 2 { 80 } else { 2 };
        check(&mut t, removed, &[]);
        assert_eq!(
            t.due(bot(), removed + 300, Skill::Legendary, |_| false),
            None,
            "gap {gap}"
        );
    }
}

#[test]
fn unknown_respawn_period_is_learned_only_from_a_watched_cycle() {
    let item = Supply {
        respawn: None,
        ..SUPPLY
    };
    for interrupted in [false, true] {
        let mut t = Timers::default();
        check(&mut t, 10, &[item]);
        check(&mut t, 11, &[]);
        for tick in (21..111).step_by(10) {
            t.observe(tick, Some(&[]), |_| !interrupted || tick != 31);
        }
        check(&mut t, 111, &[Supply { object: 90, ..item }]);
        check(&mut t, 112, &[]);
        assert_eq!(
            t.due(bot(), 210, Skill::Normal, |_| false),
            (!interrupted).then_some(PAD)
        );
    }
}

#[test]
fn two_identical_items_keep_independent_clocks() {
    let other = Supply {
        site: 5,
        spawn: [14.0, 0.0, 0.0],
        at: [14.0, 0.0, 0.0],
        ..SUPPLY
    };
    let mut t = Timers::default();
    check(&mut t, 1, &[SUPPLY, other]);
    check(&mut t, 2, &[other]);
    for tick in (12..=102).step_by(10) {
        check(&mut t, tick, &[other]);
    }
    check(&mut t, 103, &[]);
    assert_eq!(t.due(bot(), 260, Skill::Legendary, |_| false), Some(PAD));
    assert_eq!(
        t.due(bot(), 390, Skill::Legendary, |_| false),
        Some(other.spawn)
    );
}

#[test]
fn bringing_the_same_gun_back_does_not_teach_a_respawn_period() {
    let gun = Supply {
        respawn: None,
        ..SUPPLY
    };
    let mut t = Timers::default();
    check(&mut t, 0, &[gun]);
    check(&mut t, 1, &[]);
    for tick in (11..101).step_by(10) {
        check(&mut t, tick, &[]);
    }
    check(&mut t, 101, &[gun]);
    check(&mut t, 102, &[]);
    assert_eq!(t.due(bot(), 201, Skill::Legendary, |_| false), None);
}

#[test]
fn the_mind_walks_to_visible_equipment_and_returns_for_its_next_spawn() {
    use crate::brain::{Observations, Perception};
    use crate::mind::{Doing, Mind};
    let item = Supply {
        powerup: Some(crate::decisions::supplies::Powerup::Overshield),
        ..SUPPLY
    };
    let mut mind = Mind::new(Skill::Legendary, 13);
    let mut p = Perception {
        me: Some(armed()),
        observations: Observations {
            supplies: Some(vec![item]),
            pickups_scanned: true,
            ..Default::default()
        },
        ..Perception::default()
    };
    let mut facing_away = Mind::new(Skill::Legendary, 13);
    let mut other = p.clone();
    other.me.as_mut().unwrap().aim = [-1.0, 0.0, 0.0];
    facing_away.step(&other, None);
    assert_eq!(facing_away.doing, Doing::Roam, "no sighting behind the bot");

    let pad = mind.step(&p, None);
    assert_eq!(mind.doing, Doing::Supply(item.at));
    // Walking toward it (the stick eases in over a few ticks).
    assert!(pad.left_stick.1 > 0.25 && pad.left_stick.1 > pad.left_stick.0.abs());
    p.observations.supplies = Some(vec![]);
    p.tick = 1;
    mind.step(&p, None);
    assert_eq!(mind.doing, Doing::Roam);
    p.tick = 280;
    let pad = mind.step(&p, None);
    assert_eq!(mind.doing, Doing::Supply(item.spawn));
    // Walking toward it (the stick eases in over a few ticks).
    assert!(pad.left_stick.1 > 0.25 && pad.left_stick.1 > pad.left_stick.0.abs());
}

#[test]
fn a_witnessed_power_weapon_uses_the_live_clock_and_remembers_its_wielder() {
    let mut t = Timers::default();
    check(&mut t, 0, &[SUPPLY]);
    check(&mut t, 1, &[]);
    let holder = Player {
        id: 2,
        unit: 0xf0020004,
        position: PAD,
        hands: [None, Some(SUPPLY.object)],
        overshield: false,
        camo: false,
    };
    let mut clocks = [Clock::Idle {
        service: 85,
        rate: 30,
        deleted: false,
    }; 5];
    clocks[4] = Clock::Due(190);
    update(&mut t, 1, &clocks, &[holder], &[], &[], |_| true);
    assert!(t.holder(2));
    assert_eq!(t.due(bot(), 1, Skill::Legendary, |_| false), None);
    assert_eq!(t.due(bot(), 170, Skill::Legendary, |_| false), Some(PAD));
    update(&mut t, 71, &clocks, &[holder], &[], &[], |_| false);
    assert!(t.holder(2));
    let new_life = Player {
        unit: holder.unit + 1,
        hands: [None; 2],
        ..holder
    };
    update(&mut t, 72, &clocks, &[new_life], &[], &[], |_| true);
    assert!(!t.holder(2));
    for clock in [
        Clock::Disabled,
        Clock::Idle {
            service: 85,
            rate: 30,
            deleted: false,
        },
    ] {
        clocks[4] = clock;
        update(&mut t, 73, &clocks, &[], &[], &[], |_| true);
        assert_eq!(t.due(bot(), 295, Skill::Legendary, |_| false), None);
    }
}

#[test]
fn a_powerup_pickup_needs_a_unique_visible_taker_with_its_effect() {
    let gear = Supply {
        powerup: Some(Powerup::Camo),
        ..SUPPLY
    };
    let a = Player {
        id: 1,
        unit: 11,
        position: PAD,
        hands: [None; 2],
        camo: true,
        overshield: false,
    };
    for (other, expected) in [(false, true), (true, false)] {
        let mut t = Timers::default();
        check(&mut t, 10, &[gear]);
        check(&mut t, 11, &[]);
        let players = [
            a,
            Player {
                id: 2,
                unit: 22,
                ..a
            },
        ];
        update(
            &mut t,
            11,
            &[],
            &players[..if other { 2 } else { 1 }],
            &[],
            &[],
            |_| true,
        );
        assert_eq!(t.holder(1), expected);
        update(
            &mut t,
            12,
            &[],
            &[Player { camo: false, ..a }],
            &[],
            &[],
            |_| true,
        );
        assert!(!t.holder(1));
    }
}

#[test]
fn a_timed_trip_reaches_the_pad_before_the_spawn_and_waits_there() {
    let mut t = Timers::default();
    let mut me = bot();
    check(&mut t, 0, &[SUPPLY]);
    check(&mut t, 1, &[]);
    let clocks = [Clock::Due(200); 5];
    update(&mut t, 1, &clocks, &[], &[], &[], |_| true);
    let mut arrived = None;
    for tick in 2..=200 {
        if t.due(me, tick, Skill::Legendary, |_| false) == Some(PAD) {
            me.position[0] = (me.position[0] + 1.0 / 12.0).min(PAD[0]);
            if me.position == PAD {
                arrived.get_or_insert(tick);
            }
        } else {
            assert!(arrived.is_none(), "left the pad before the spawn");
        }
    }
    assert!((179..=182).contains(&arrived.unwrap()));
}

#[test]
fn an_idle_spawner_projects_only_the_first_eligible_service_after_a_pickup() {
    let idle = |service, deleted| {
        [Clock::Idle {
            service,
            rate: 30,
            deleted,
        }; 5]
    };
    for (mode, deleted, expected) in [
        (true, false, true),
        (false, true, true),
        (false, false, false),
    ] {
        let mut t = Timers::default();
        check(
            &mut t,
            3,
            &[Supply {
                on_disturbance: mode,
                ..SUPPLY
            }],
        );
        check(&mut t, 4, &[]);
        update(&mut t, 4, &idle(34, deleted), &[], &[], &[], |_| true);
        assert_eq!(
            t.due(bot(), 295, Skill::Legendary, |_| false),
            expected.then_some(PAD)
        );
        check(&mut t, 34, &[]);
        update(&mut t, 34, &idle(64, deleted), &[], &[], &[], |_| true);
        assert_eq!(t.due(bot(), 295, Skill::Legendary, |_| false), None);
    }
}

#[test]
fn a_visible_sniper_changes_hands_even_without_a_pad_sighting() {
    let a = Player {
        id: 1,
        unit: 10,
        position: PAD,
        hands: [Some(77), None],
        overshield: false,
        camo: false,
    };
    let b = Player {
        id: 2,
        unit: 20,
        hands: [None, Some(77)],
        ..a
    };
    let mut t = Timers::default();
    update(&mut t, 0, &[], &[a], &[77], &[], |_| false);
    assert!(!t.holder(1));
    update(&mut t, 1, &[], &[a], &[77], &[], |_| true);
    assert!(t.holder(1));
    update(
        &mut t,
        2,
        &[],
        &[
            Player {
                hands: [None; 2],
                ..a
            },
            b,
        ],
        &[77],
        &[],
        |_| true,
    );
    assert!(!t.holder(1));
    assert!(t.holder(2));
    update(&mut t, 302, &[], &[b], &[], &[], |_| false);
    assert!(!t.holder(2));
}

#[test]
fn a_witnessed_success_dates_the_pickup_between_scans_and_identifies_the_recipient() {
    let item = Supply {
        powerup: Some(Powerup::Camo),
        ..SUPPLY
    };
    let a = Player {
        id: 2,
        unit: 20,
        position: PAD,
        hands: [None; 2],
        camo: true,
        overshield: false,
    };
    let b = Player {
        id: 3,
        unit: 30,
        ..a
    };
    let event = Take {
        site: item.site,
        object: item.object,
        player: b.id,
        unit: b.unit,
        tick: 13,
    };
    for (visible, event_tick, when, known) in [
        (true, 13, Some(13), true),
        (true, 9, Some(15), false),
        (false, 13, None, false),
    ] {
        let mut t = Timers::default();
        check(&mut t, 10, &[item]);
        t.observe(15, Some(&[]), |_| visible);
        update(
            &mut t,
            15,
            &[],
            &[a, b],
            &[],
            &[Take {
                tick: event_tick,
                ..event
            }],
            |_| true,
        );
        assert_eq!(t.sites[0].taken, when);
        assert_eq!(t.holder(b.id), known);
        assert!(!t.holder(a.id));
    }
}
