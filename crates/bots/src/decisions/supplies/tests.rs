use super::*;
use crate::arms::{Ammo, Arms, Gun, Profile, Trigger};

const PAD: Vec3 = [10.0, 0.0, 0.0];
const SUPPLY: Supply = Supply {
    site: 4,
    object: 72,
    spawn: PAD,
    at: [10.4, 0.0, 0.0],
    disabled: false,
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
    rate: Option<u16>,
    players: &[Player],
    held: &[u32],
    takes: &[Take],
    visible: impl Fn(Vec3) -> bool,
) {
    let p = crate::brain::Perception {
        tick,
        observations: crate::brain::Observations {
            service_rate: rate,
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
fn a_witnessed_pickup_starts_a_skill_scaled_trip() {
    let mut t = Timers::default();
    check(&mut t, 0, &[SUPPLY]);
    assert_eq!(
        t.due(bot(), 0, Skill::Legendary, Trip::Search, |_| Some(0.)),
        None,
        "arms chooses guns on the floor"
    );
    check(&mut t, 1, &[]);
    assert_eq!(
        t.due(bot(), 120, Skill::Legendary, Trip::Search, |_| Some(0.)),
        None
    );
    assert_eq!(
        t.due(bot(), 240, Skill::Legendary, Trip::Search, |_| Some(0.)),
        Some(PAD)
    );
    assert_eq!(
        t.due(bot(), 240, Skill::Easy, Trip::Search, |_| Some(0.)),
        None
    );
    assert_eq!(
        t.due(bot(), 360, Skill::Easy, Trip::Search, |_| Some(0.)),
        Some(PAD)
    );
    assert_eq!(
        t.due(armed(), 296, Skill::Legendary, Trip::Search, |_| Some(0.)),
        None
    );
    assert_eq!(
        t.due(bot(), 296, Skill::Legendary, Trip::Search, |_| None),
        None,
        "hot placement"
    );
    assert_eq!(
        t.due(bot(), 365, Skill::Legendary, Trip::Search, |_| Some(0.)),
        Some(PAD)
    );
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
        t.due(armed(), 5, Skill::Legendary, Trip::Search, |_| Some(0.)),
        Some(item.at)
    );
    check(&mut t, 6, &[]);
    assert_eq!(
        t.due(armed(), 7, Skill::Legendary, Trip::Search, |_| Some(0.)),
        None
    );
    assert_eq!(
        t.due(armed(), 300, Skill::Legendary, Trip::Search, |_| Some(0.)),
        Some(item.spawn)
    );
    check(&mut t, 306, &[item]);
    assert_eq!(
        t.due(armed(), 306, Skill::Legendary, Trip::Search, |_| Some(0.)),
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
            t.due(bot(), removed + 300, Skill::Legendary, Trip::Search, |_| {
                Some(0.)
            }),
            Some(PAD),
            "a missed pickup still leaves an empty-pad estimate, gap {gap}"
        );
    }
}

#[test]
fn seeing_only_one_side_of_a_displaced_item_cannot_confirm_a_pickup() {
    let mut timers = Timers::default();
    check(&mut timers, 0, &[SUPPLY]);
    timers.observe(1, Some(&[]), |at| at == PAD);
    let site = timers.0[0];
    assert!(site.present && !site.watched);
    assert_eq!(site.taken, None);
    assert!(site.probability(1, 0., 1.) < 1.);
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
            t.due(bot(), 210, Skill::Normal, Trip::Search, |_| Some(0.)),
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
    assert_eq!(
        t.due(bot(), 260, Skill::Legendary, Trip::Search, |_| Some(0.)),
        Some(PAD)
    );
    t.observe(390, Some(&[SUPPLY]), |at| at == PAD || at == SUPPLY.at);
    assert_eq!(
        t.due(bot(), 390, Skill::Legendary, Trip::Search, |_| Some(0.)),
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
    assert_eq!(
        t.due(bot(), 201, Skill::Legendary, Trip::Search, |_| Some(0.)),
        None
    );
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
    p.me.as_mut().unwrap().aim = [-1., 0., 0.];
    p.tick = 280;
    let pad = mind.step(&p, None);
    assert_eq!(mind.doing, Doing::Supply(item.spawn));
    assert_ne!(pad, input::host::HostPadState::default());
}

#[test]
fn probability_uses_a_prior_and_only_direct_sight_can_be_certain() {
    let mut t = Timers::default();
    check(
        &mut t,
        5,
        &[Supply {
            respawn: None,
            ..SUPPLY
        }],
    );
    assert_eq!(t.sites[0].probability(5, 0., 1.), 1.);
    check(&mut t, 100, &[]);
    assert_eq!(t.sites[0].taken, None);
    assert_eq!(t.sites[0].probability(100, 0., 1.), 0.);
    let early = t.sites[0].probability(300, 0., 1.);
    let late = t.sites[0].probability(2000, 0., 1.);
    assert!(0. < early && early < late && late < 1.);
    t.hear(SUPPLY, true, 2001, 2002);
    assert!(t.sites[0].probability(2002, 0., 1.) < 1.);
}

#[test]
fn radio_is_team_scoped_and_hearsay_is_never_rebroadcast_or_timed() {
    use crate::brain::Perception;
    use crate::decisions::radio::{Radio, SupplyCall};
    let mut board = Radio::default();
    board.advance(3);
    board.publish_supply(
        SupplyCall {
            sender: 1,
            team: 0,
            item: SUPPLY,
            present: false,
            tick: 3,
        },
        3,
    );
    let mut p = Perception {
        teams: true,
        ..Default::default()
    };
    p.observations.player = Some((2, 0));
    assert!(board.supply_calls(&p).is_empty());
    board.advance(4);
    let mut t = Timers::default();
    let c = board.supply_calls(&p)[0];
    t.hear(c.item, c.present, c.tick, 4);
    assert!(t.reports(4).is_empty());
    assert_eq!(t.sites[0].learned, None);
    p.observations.player = Some((2, 1));
    assert!(board.supply_calls(&p).is_empty());
}

#[test]
fn delayed_radio_preserves_the_tick_the_teammate_observed() {
    use crate::brain::Perception;
    use crate::decisions::radio::{Radio, SupplyCall};
    let mut board = Radio::default();
    board.advance(10);
    board.publish_supply(
        SupplyCall {
            sender: 1,
            team: 0,
            item: SUPPLY,
            present: false,
            tick: 6,
        },
        10,
    );
    let mut p = Perception {
        teams: true,
        ..Default::default()
    };
    p.observations.player = Some((2, 0));
    assert!(board.supply_calls(&p).is_empty());
    board.advance(11);
    assert_eq!(board.supply_calls(&p)[0].tick, 6);
    board.advance(82);
    assert!(board.supply_calls(&p).is_empty());
}

#[test]
fn a_visible_carrier_is_weak_evidence_for_a_unique_known_pad() {
    use crate::brain::{Contact, Perception};
    let mut t = Timers::default();
    check(&mut t, 0, &[SUPPLY]);
    t.sites[0].model = Some(3);
    t.observe(50, None, |_| false);
    let mut p = Perception {
        tick: 50,
        ..Default::default()
    };
    p.observations.contacts.push(Contact {
        id: 1,
        aim: [1., 0., 0.],
        weapon: Some(3),
        shield: None,
        empty_magazine: false,
    });
    t.carriers(&p, bot());
    assert!(t.sites[0].probability(50, 0., 1.) < 0.3);
    assert_eq!(t.sites[0].taken, None);
    assert!(t.sites[0].probability(600, 0., 1.) > 0.7);
}

#[test]
fn an_unvisited_power_pad_can_be_worth_the_trip_and_a_visible_empty_one_is_not() {
    let mut timers = Timers::default();
    for (site, x) in [(1, 8.), (2, 10.)] {
        timers.familiar(
            crate::places::PowerPad {
                site,
                at: [x, 0., 0.],
                period: Some(1500),
                powerup: Some(Powerup::Overshield),
                model: None,
            },
            0,
        );
    }
    assert_eq!(
        timers.due(armed(), 0, Skill::Legendary, Trip::Search, |p| Some(
            if p[0] == 10. { 0.8 } else { 0. }
        )),
        Some([10., 0., 0.])
    );
    assert_eq!(
        timers.due(armed(), 0, Skill::Legendary, Trip::Contest, |_| Some(1.)),
        None
    );
    timers.observe(1, Some(&[]), |_| true);
    assert_eq!(
        timers.due(armed(), 1, Skill::Legendary, Trip::Search, |_| Some(1.)),
        None
    );
}

#[test]
fn a_witnessed_power_weapon_uses_estimated_service_and_remembers_its_wielder() {
    let mut t = Timers::default();
    check(&mut t, 0, &[SUPPLY]);
    check(&mut t, 1, &[]);
    let holder = Player {
        id: 2,
        unit: 24,
        position: PAD,
        hands: [None, Some(SUPPLY.object)],
        overshield: false,
        camo: false,
    };
    update(&mut t, 1, Some(30), &[holder], &[], &[], |_| true);
    assert_eq!(t.sites[0].expected_return, Some(274));
    assert!(t.holder(2));
    assert_eq!(
        t.due(bot(), 1, Skill::Legendary, Trip::Search, |_| Some(0.)),
        None
    );
    assert_eq!(
        t.due(bot(), 250, Skill::Legendary, Trip::Search, |_| Some(0.)),
        Some(PAD)
    );
    update(&mut t, 71, None, &[], &[], &[], |_| false);
    assert!(
        t.holder(2),
        "leaving sight does not reveal who still has the gun"
    );
    let new_life = Player {
        unit: 25,
        hands: [None; 2],
        ..holder
    };
    update(&mut t, 72, None, &[new_life], &[], &[], |_| true);
    assert!(!t.holder(2));
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
            None,
            &players[..if other { 2 } else { 1 }],
            &[],
            &[],
            |_| true,
        );
        assert_eq!(t.holder(1), expected);
        update(
            &mut t,
            12,
            None,
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
    update(&mut t, 1, Some(30), &[], &[], &[], |_| true);
    let expected = t.sites[0].expected_return.unwrap();
    let mut arrived = None;
    for tick in 2..=expected {
        if t.due(me, tick, Skill::Legendary, Trip::Search, |_| Some(0.)) == Some(PAD) {
            me.position[0] = (me.position[0] + 1.0 / 12.0).min(PAD[0]);
            if me.position == PAD {
                arrived.get_or_insert(tick);
            }
        } else {
            assert!(
                arrived.is_none(),
                "left the pad before the estimated return"
            );
        }
    }
    assert!((18..=21).contains(&(expected - arrived.unwrap())));
}

#[test]
fn only_an_eligible_witnessed_removal_projects_a_service() {
    for (disturbed, kind, disabled, expected) in [
        (true, None, false, Some(304)),
        (false, Some(Powerup::Camo), false, Some(304)),
        (false, None, false, None),
        (true, None, true, None),
    ] {
        let mut t = Timers::default();
        check(
            &mut t,
            3,
            &[Supply {
                on_disturbance: disturbed,
                powerup: kind,
                disabled,
                ..SUPPLY
            }],
        );
        check(&mut t, 4, &[]);
        update(&mut t, 4, Some(30), &[], &[], &[], |_| true);
        assert_eq!(t.sites[0].expected_return, expected);
        if disabled {
            assert!(
                t.due(bot(), 295, Skill::Legendary, Trip::Search, |_| Some(0.))
                    .is_none()
            );
        }
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
    update(&mut t, 0, None, &[a], &[77], &[], |_| false);
    assert!(!t.holder(1));
    update(&mut t, 1, None, &[a], &[77], &[], |_| true);
    assert!(t.holder(1));
    update(
        &mut t,
        2,
        None,
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
    update(&mut t, 302, None, &[b], &[], &[], |_| false);
    assert!(!t.holder(2));
}

#[test]
fn a_recognized_offhand_updates_only_its_own_unwatched_pad() {
    let mut t = Timers::default();
    check(
        &mut t,
        1,
        &[
            SUPPLY,
            Supply {
                site: 5,
                object: 73,
                ..SUPPLY
            },
        ],
    );
    t.observe(2, Some(&[]), |_| false);
    let holder = Player {
        id: 3,
        unit: 15,
        position: PAD,
        overshield: false,
        camo: false,
        hands: [None, Some(SUPPLY.object)],
    };
    update(&mut t, 2, Some(30), &[holder], &[], &[], |_| true);
    assert_eq!(t.sites[0].indirect, Some((false, 2)));
    assert_eq!(t.sites[0].taken, None);
    assert_eq!(t.sites[1].indirect, None);
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
        at: PAD,
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
            None,
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
