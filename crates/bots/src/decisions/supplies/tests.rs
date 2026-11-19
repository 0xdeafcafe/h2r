use super::*;
use crate::arms::{Ammo, Arms, Gun, Profile, Trigger};

const PAD: Vec3 = [10.0, 0.0, 0.0];
const SUPPLY: Supply = Supply {
    site: 4,
    object: 72,
    spawn: PAD,
    at: [10.4, 0.0, 0.0],
    respawn: Some(300),
    powerup: false,
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

#[test]
fn a_witnessed_pickup_starts_a_skill_scaled_trip() {
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
    assert_eq!(t.due(bot(), 360, Skill::Easy, |_| false), Some(PAD));
    assert_eq!(t.due(armed(), 296, Skill::Legendary, |_| false), None);
    assert_eq!(
        t.due(bot(), 296, Skill::Legendary, |_| true),
        None,
        "hot placement"
    );
    assert_eq!(t.due(bot(), 365, Skill::Legendary, |_| false), Some(PAD));
}

#[test]
fn powerups_draw_an_armed_bot_to_the_item_and_later_to_the_placement() {
    let item = Supply {
        powerup: true,
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
            Some(PAD),
            "a missed pickup still leaves an empty-pad estimate, gap {gap}"
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
    check(&mut t, 389, &[SUPPLY]);
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
        powerup: true,
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
    assert_eq!(t.0[0].probability(5, 0., 1.), 1.);
    check(&mut t, 100, &[]);
    assert_eq!(t.0[0].taken, None);
    assert_eq!(t.0[0].probability(100, 0., 1.), 0.);
    let early = t.0[0].probability(300, 0., 1.);
    let late = t.0[0].probability(2000, 0., 1.);
    assert!(0. < early && early < late && late < 1.);
    t.hear(SUPPLY, true, 2001, 2002);
    assert!(t.0[0].probability(2002, 0., 1.) < 1.);
}

#[test]
fn radio_is_team_scoped_and_hearsay_is_never_rebroadcast_or_timed() {
    use crate::brain::Perception;
    use crate::decisions::radio::{Radio, SupplyCall};
    let mut board = Radio::default();
    board.advance(3);
    board.publish_supply(SupplyCall {
        sender: 1,
        team: 0,
        item: SUPPLY,
        present: false,
        tick: 3,
    });
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
    assert_eq!(t.0[0].learned, None);
    p.observations.player = Some((2, 1));
    assert!(board.supply_calls(&p).is_empty());
}

#[test]
fn a_visible_carrier_is_weak_evidence_for_a_unique_known_pad() {
    use crate::brain::{Contact, Perception};
    let mut t = Timers::default();
    check(&mut t, 0, &[SUPPLY]);
    t.0[0].model = Some(3);
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
    assert!(t.0[0].probability(50, 0., 1.) < 0.3);
    assert_eq!(t.0[0].taken, None);
    assert!(t.0[0].probability(600, 0., 1.) > 0.7);
}
