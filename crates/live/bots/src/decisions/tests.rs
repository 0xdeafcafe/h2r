use super::*;
use crate::Brain;
use crate::brain::{Contact, Observations};
use crate::los::Walls;
use crate::mind::{Doing, Mind};
use crate::nav::{Edge, Kind, NavMesh, Poly};
use crate::objective::{Game, Hill, Item, Objective, Spot, kind};
use crate::stuck::Stuck;
use radio::Radio;
use std::sync::Mutex;

const X: Vec3 = [1.0, 0.0, 0.0];

/// Which way the left stick walks (right, forward), as a unit: the feet
/// ease into a walk over a few ticks (mind::pad::Feet) and an approach
/// slows, so a first pad points the way without full deflection.
fn heading(stick: (f32, f32)) -> (f32, f32) {
    let l = stick.0.hypot(stick.1);
    assert!(l > 0.1, "the stick walks: {stick:?}");
    (stick.0 / l, stick.1 / l)
}

fn player(id: u32, team: i16, x: f32, y: f32) -> Seen {
    Seen {
        id,
        team,
        position: [x, y, 0.0],
    }
}

fn scene(id: u32, tick: u32, players: &[Seen]) -> Perception {
    let own = players.iter().find(|s| s.id == id).unwrap();
    Perception {
        me: Some(Me {
            position: own.position,
            team: own.team,
            ..Default::default()
        }),
        others: players.iter().copied().filter(|s| s.id != id).collect(),
        observations: Observations {
            player: Some((id, own.team)),
            ..Default::default()
        },
        teams: true,
        tick,
        ..Default::default()
    }
}

fn brain(seed: u64, board: &Mutex<Radio>) -> Brain {
    let mut b = Brain::new(Skill::Legendary, seed).with_tuning(crate::tuning::Tuning {
        perception_ticks: Some(0),
        motor_ticks: Some(0),
        button_sigma_ms: Some(0.0),
        ..Default::default()
    });
    b.epoch = radio::lock(board).epoch;
    b
}

#[test]
fn simultaneous_claims_split_without_flipping_in_either_pad_order() {
    let ps = [
        player(0, 0, 0.0, -0.5),
        player(1, 0, 0.0, 0.5),
        player(2, 1, 7.0, 0.0),
        player(3, 1, 8.0, 0.0),
    ];
    for order in [[0, 1], [1, 0]] {
        let board = Mutex::new(Radio::default());
        let mut bots = [brain(0, &board), brain(1, &board)];
        for t in 0..12 {
            for id in order {
                let p = scene(id, t, &ps);
                bots[id as usize].play(&p, None, &[], &board);
                // A repeated render of this tick sees the same calls.
                bots[id as usize].play(&p, None, &[], &board);
            }
            assert_eq!(bots[0].mind.doing, Doing::Fight(2));
            assert_eq!(bots[1].mind.doing, Doing::Fight(if t == 0 { 2 } else { 3 }));
        }
    }
}

#[test]
fn a_call_moves_the_receiver_without_giving_it_a_shot_or_live_enemy_position() {
    let ps = [
        player(0, 0, 0.0, 0.0),
        player(1, 0, 0.0, 2.0),
        player(2, 1, 8.0, 0.0),
    ];
    let board = Mutex::new(Radio::default());
    let mut sender = brain(0, &board);
    let mut receiver = brain(1, &board);
    sender.play(&scene(0, 10, &ps), None, &[], &board);
    let mut p = scene(1, 11, &ps);
    p.me.as_mut().unwrap().aim = [-1.0, 0.0, 0.0];
    p.others.iter_mut().find(|s| s.id == 2).unwrap().position = [15.0, 5.0, 0.0];
    let pad = receiver.play(&p, None, &[], &board);
    assert_eq!(receiver.mind.doing, Doing::Support(ps[2].position));
    assert!(heading(pad.left_stick).1 < -0.5);
    assert_eq!((pad.right_trigger, pad.left_trigger), (0.0, 0.0));
    assert!(
        receiver.mind.decisions.report.is_none(),
        "no relay of hearsay"
    );
    let mut board = radio::lock(&board);
    assert!(board.listen(&scene(2, 11, &ps)).is_empty(), "opposing team");
    p.teams = false;
    assert!(board.listen(&p).is_empty(), "FFA");
    p.teams = true;
    let mut dead = p.clone();
    dead.others.retain(|s| s.id != 2);
    assert_eq!(
        board.listen(&dead).len(),
        1,
        "losing sight cannot retract a call"
    );
    board.advance(12);
    board.forget_deaths(&[Kill {
        seq: 5,
        tick: 12,
        victim: 2,
        attacker: Some(0),
        betrayal: false,
        at: None,
    }]);
    assert!(
        board.listen(&p).is_empty(),
        "respawned slot starts a new life"
    );
    board.publish(Call {
        sender: 0,
        team: 0,
        target: 2,
        at: ps[2].position,
        tick: 12,
        help: false,
    });
    board.advance(100);
    assert!(board.listen(&p).is_empty(), "expired");
    board.advance(1);
    assert!(board.listen(&p).is_empty(), "rewind");
}

#[test]
fn cover_and_a_trade_override_the_normal_target_split() {
    let ps = [
        player(0, 0, 0.0, 0.0),
        player(1, 0, 0.0, 1.0),
        player(2, 1, 7.0, 0.0),
        player(3, 1, 8.0, 0.0),
    ];
    let board = Mutex::new(Radio::default());
    let mut scout = brain(10, &board);
    let mut buddy = brain(12, &board);
    for t in 0..3 {
        let mut scout_view = scene(0, t, &ps);
        if t == 2 {
            scout_view.me.as_mut().unwrap().shield = 0.72;
        }
        scout.play(&scout_view, None, &[], &board);
        buddy.play(&scene(1, t, &ps), None, &[], &board);
    }
    assert_eq!(buddy.mind.doing, Doing::Fight(3));
    buddy.play(&scene(1, 3, &ps), None, &[], &board);
    assert_eq!(
        buddy.mind.doing,
        Doing::Fight(2),
        "covers a teammate taking damage"
    );

    let mut after = scene(1, 4, &ps);
    after.others.retain(|s| s.id != 0);
    let death = Kill {
        seq: 1,
        tick: 4,
        attacker: Some(3),
        victim: 0,
        betrayal: false,
        at: Some(ps[0].position),
    };
    buddy.play(&after, None, &[death], &board);
    assert_eq!(buddy.mind.doing, Doing::Fight(3), "trades for the teammate");
    // Trade memory cannot pick an enemy outside personal sight.
    after.tick += 1;
    after
        .others
        .iter_mut()
        .find(|s| s.id == 3)
        .unwrap()
        .position = [-8.0, 0.0, 0.0];
    buddy.play(&after, None, &[], &board);
    assert_eq!(buddy.mind.doing, Doing::Fight(2));
}

#[test]
fn a_two_on_one_backs_off_then_regroups_when_sight_breaks() {
    let ps = [
        player(0, 0, -18.0, 0.0),
        player(1, 0, 0.0, 0.0),
        player(2, 1, 8.0, 0.0),
        player(3, 1, 9.0, 1.0),
    ];
    let mut p = scene(1, 0, &ps);
    // A teammate too far away to take part in this fight.
    p.others[0].position[0] = -24.0;
    let mut m = Mind::new(Skill::Legendary, 42);
    m.step(&p, None);
    p.tick = 12;
    let pad = m.step(&p, None);
    assert_eq!(m.decisions.posture, Posture::Withdraw);
    assert!(heading(pad.left_stick).1 < -0.5);
    p.tick += 1;
    p.me.as_mut().unwrap().aim = [-1.0, 0.0, 0.0];
    m.step(&p, None);
    assert_eq!(m.doing, Doing::Support([-24.0, 0.0, 0.0]));
    p.me.as_mut().unwrap().aim = X;
    p.others[0].position = [-2.0, 0.0, 0.0];
    p.tick += 1;
    m.step(&p, None);
    assert_eq!(m.decisions.posture, Posture::Normal, "local two on two");
}

fn cue(id: u32, aim: Vec3, shield: f32, empty_magazine: bool) -> Contact {
    Contact {
        weapon: None,
        id,
        aim,
        shield: Some(shield),
        empty_magazine,
        crouching: false,
    }
}

#[test]
fn weak_or_empty_foes_shorten_the_band_and_unknown_cues_do_not() {
    let ps = [player(0, 0, 0.0, 0.0), player(1, 1, 11.0, 0.0)];
    for (shield, empty, posture) in [
        (1.0, false, Posture::Normal),
        (0.05, false, Posture::Press),
        (1.0, true, Posture::Press),
    ] {
        let mut p = scene(0, 0, &ps);
        p.observations
            .contacts
            .push(cue(1, [-1.0, 0.0, 0.0], shield, empty));
        let mut m = Mind::new(Skill::Legendary, 9);
        m.step(&p, None);
        assert_eq!(m.decisions.posture, posture);
        let (_, far) = m.fight.band;
        assert_eq!(far < 10.0, posture == Posture::Press);
        p.me.as_mut().unwrap().shield = 0.1;
        p.tick += 1;
        m.step(&p, None);
        assert_ne!(m.decisions.posture, Posture::Press, "protect own shield");
    }
}

#[test]
fn a_free_for_all_pair_facing_each_other_is_an_opening() {
    let ps = [
        player(0, 0, 0.0, 0.0),
        player(1, 1, 8.0, -2.0),
        player(2, 1, 8.0, 2.0),
    ];
    let mut p = scene(0, 0, &ps);
    p.teams = false;
    p.observations.contacts = vec![
        cue(1, [0.0, 1.0, 0.0], 1.0, false),
        cue(2, [0.0, -1.0, 0.0], 1.0, false),
    ];
    let mut m = Mind::new(Skill::Legendary, 1);
    m.step(&p, None);
    assert_eq!(m.decisions.posture, Posture::Press);
    p.observations.contacts[0].aim = [-1.0, 0.0, 0.0];
    p.observations.contacts[1].aim = [-1.0, 0.0, 0.0];
    p.tick += 1;
    m.step(&p, None);
    assert_eq!(m.decisions.posture, Posture::Withdraw);
}

#[test]
fn traits_replay_per_seed_and_change_choices_independently_of_skill() {
    let traits = Personality::seeded(0);
    assert_eq!(traits, Personality::seeded(0));
    assert_ne!(traits, Personality::seeded(1));
    let ps = [player(0, 0, 0.0, 0.0), player(1, 1, 6.0, 0.0)];
    let mut p = scene(0, 10, &ps);
    p.observations.contacts.push(cue(1, X, 0.32, false));
    let choices = |skill| {
        (0..40)
            .filter(|&seed| {
                let mut m = Mind::new(skill, seed);
                m.step(&p, None);
                m.decisions.posture == Posture::Press
            })
            .count()
    };
    let legendary = choices(Skill::Legendary);
    assert!(legendary > 0 && legendary < 40);
    assert_eq!(choices(Skill::Legendary), legendary);
    assert_eq!(choices(Skill::Easy), 0);
}

fn square(x: f32, y: f32, width: f32) -> Poly {
    Poly::new(vec![
        [x, y, 0.0],
        [x + width, y, 0.0],
        [x + width, y + width, 0.0],
        [x, y + width, 0.0],
    ])
}

#[test]
fn own_death_changes_the_carriers_corridor_until_the_zone_cools() {
    // Ring of four rooms: the north route crosses Y=10 first.
    let mut edges = Vec::new();
    for (from, to, a, b) in [
        (0, 1, [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]),
        (0, 2, [0.0, 10.0, 0.0], [10.0, 10.0, 0.0]),
        (1, 3, [10.0, 10.0, 0.0], [20.0, 10.0, 0.0]),
        (2, 3, [10.0, 10.0, 0.0], [10.0, 20.0, 0.0]),
    ] {
        edges.push(Edge {
            from,
            to,
            a,
            b,
            kind: Kind::Walk,
        });
        edges.push(Edge {
            from: to,
            to: from,
            a,
            b,
            kind: Kind::Walk,
        });
    }
    let nav = NavMesh::new(
        vec![
            square(0.0, 0.0, 10.0),
            square(10.0, 0.0, 10.0),
            square(0.0, 10.0, 10.0),
            square(10.0, 10.0, 10.0),
        ],
        edges,
    );
    let start = [6.0, 1.0, 0.0];
    let home = [18.0, 13.0, 0.0];
    let p = scene(4, 10, &[player(4, 0, start[0], start[1])]);
    let mut m = Mind::new(Skill::Legendary, 1);
    let death = Kill {
        seq: 8,
        tick: 10,
        at: Some([15.0, 5.0, 0.0]),
        attacker: Some(8),
        victim: 4,
        betrayal: false,
    };
    m.hear_deaths(&p, &[death], None);
    m.step(
        &Perception {
            me: None,
            ..p.clone()
        },
        None,
    );
    let mut route = |t, threat| {
        m.decisions.map.new_route_threat(threat);
        m.decisions
            .map
            .path(&nav, start, home, t, &Stuck::new(), 1.0)
            .unwrap()
    };
    let north = |e: &Edge| e.from == 0 && e.to == 2;
    let east = |e: &Edge| e.from == 0 && e.to == 1;
    let via_north = nav.path_avoiding(start, home, &east).unwrap();
    let via_east = nav.path_avoiding(start, home, &north).unwrap();
    assert_ne!(via_north, via_east, "different physical paths");
    assert_eq!(route(20, None), via_north, "death survives respawn");
    assert_eq!(
        route(700, None),
        nav.path(start, home).unwrap(),
        "zone cooled"
    );
    assert_eq!(
        route(700, Some([5.0, 15.0, 0.0])),
        via_east,
        "avoid the carrier's fresh sighting"
    );
    assert!(
        route(20, Some([5.0, 15.0, 0.0]))
            .last()
            .is_some_and(|w| w.at == home),
        "both routes dangerous: home is still reachable"
    );
    // Respawn as the actual flag carrier. Its first pad follows the safe
    // corridor, with no enemy currently visible.
    let mut carrier = p;
    carrier.tick = 21;
    carrier.objective = Some(Objective {
        state: Default::default(),
        game: Game::Ctf,
        me: 4,
        items: vec![Item {
            id: 51,
            team: 1,
            position: start,
            carrier: Some(4),
        }],
        spots: vec![Spot {
            kind: kind::CTF_RETURN,
            team: Some(0),
            position: home,
        }],
        hill: None,
    });
    let dir = crate::nav::Follower::new(via_north, 21)
        .steer(start, 21)
        .dir;
    let map = MapData {
        places: Default::default(),
        nav,
        walls: Walls::default(),
    };
    let pad = m.step(&carrier, Some(&map));
    assert_eq!(m.doing, Doing::Carry(home));
    assert!(
        heading(pad.left_stick).1 * dir[0] - heading(pad.left_stick).0 * dir[1] > 0.95,
        "pad {:?} should follow safe corridor {dir:?}",
        pad.left_stick
    );
}

#[test]
fn a_ball_holder_finds_and_holds_a_screened_position() {
    let wall = [
        [-8.0, 6.0, 0.0],
        [8.0, 6.0, 0.0],
        [8.0, 6.0, 3.0],
        [-8.0, 6.0, 3.0],
    ];
    let map = MapData {
        places: Default::default(),
        nav: NavMesh::new(vec![square(-10.0, -10.0, 20.0)], vec![]),
        walls: Walls::from_polygons([&wall[..]]),
    };
    let mut p = scene(0, 1, &[player(0, 0, 0.0, 0.0)]);
    p.objective = Some(Objective {
        state: Default::default(),
        game: Game::Oddball,
        me: 0,
        items: vec![Item {
            id: 20,
            team: 0,
            position: [0.0; 3],
            carrier: Some(0),
        }],
        spots: vec![],
        hill: None,
    });
    let mut m = Mind::new(Skill::Heroic, 1);
    let pad = m.step(&p, Some(&map));
    assert_eq!(m.doing, Doing::Keep);
    assert!(
        heading(pad.left_stick).0 < -0.5,
        "wall to the left: {pad:?}"
    );
    p.me.as_mut().unwrap().position = [0.0, 3.0, 0.0];
    p.tick += 1;
    assert_eq!(m.step(&p, Some(&map)).left_stick, (0.0, 0.0));
}

#[test]
fn the_hill_is_contested_while_firing_and_a_defender_stays_inside() {
    let ps = [player(0, 0, 0.0, 0.0), player(1, 1, 8.0, 0.0)];
    let mut p = scene(0, 0, &ps);
    let corners = square(-6.0, -2.0, 4.0).verts;
    p.objective = Some(Objective {
        state: Default::default(),
        game: Game::Koth,
        me: 0,
        items: vec![],
        spots: vec![],
        hill: Some(Hill { corners }),
    });
    let mut m = Mind::new(Skill::Legendary, 4);
    m.step(&p, None);
    p.tick = 10;
    let pad = m.step(&p, None);
    assert_eq!(m.doing, Doing::Fight(1));
    assert_eq!(pad.right_trigger, 1.0);
    assert!(
        heading(pad.left_stick).1 < -0.8,
        "toward the hill behind the bot"
    );
    p.me.as_mut().unwrap().position = [-4.0, 0.0, 0.0];
    p.tick += 1;
    let pad = m.step(&p, None);
    assert_eq!(pad.left_stick, (0.0, 0.0));
    assert_eq!(pad.right_trigger, 1.0);
}

#[test]
fn a_visible_enemy_cannot_pull_an_escort_away_from_the_carrier() {
    let ps = [
        player(0, 0, 0.0, 0.0),
        player(1, 0, -10.0, 0.0),
        player(2, 1, 13.0, 0.0),
    ];
    let mut p = scene(0, 0, &ps);
    p.objective = Some(Objective {
        state: Default::default(),
        game: Game::Ctf,
        me: 0,
        items: vec![Item {
            id: 20,
            team: 1,
            position: ps[1].position,
            carrier: Some(1),
        }],
        spots: vec![Spot {
            kind: kind::CTF_SPAWN,
            team: Some(0),
            position: [-15.0, 0.0, 0.0],
        }],
        hill: None,
    });
    let mut m = Mind::new(Skill::Legendary, 4);
    m.step(&p, None);
    p.tick = 15;
    let pad = m.step(&p, None);
    assert_eq!(m.doing, Doing::Fight(2));
    assert!(
        heading(pad.left_stick).1 < -0.8,
        "walk toward teammate while shooting across the map"
    );
    assert_eq!(pad.right_trigger, 1.0);
}

#[test]
fn the_next_round_forgets_old_calls_chases_and_trade_targets() {
    let ps = [
        player(0, 0, 0.0, 0.0),
        player(1, 0, 0.0, 1.0),
        player(2, 1, 8.0, 0.0),
    ];
    let board = Mutex::new(Radio::default());
    let mut a = brain(3, &board);
    let mut b = brain(5, &board);
    a.mind.emote.taunt = 0.7;
    a.play(&scene(0, 300, &ps), None, &[], &board);
    b.play(&scene(1, 300, &ps), None, &[], &board);
    let mut next = scene(0, 5, &ps);
    next.me.as_mut().unwrap().aim = [-1.0, 0.0, 0.0];
    a.play(&next, None, &[], &board);
    assert_eq!(a.mind.doing, Doing::Roam);
    assert!(a.mind.decisions.inbox.is_empty());
    assert_eq!(a.mind.emote.taunt, 0.7);
    assert_eq!(
        a.mind.decisions.personality,
        Personality::for_skill(3, Skill::Legendary)
    );
}

#[test]
fn an_oddball_hunter_prioritizes_the_visible_carrier() {
    let ps = [
        player(0, 0, 0.0, 0.0),
        player(1, 1, 6.0, 0.0),
        player(2, 1, 9.0, 0.0),
    ];
    let mut p = scene(0, 0, &ps);
    p.teams = false;
    p.objective = Some(Objective {
        state: Default::default(),
        game: Game::Oddball,
        me: 0,
        items: vec![Item {
            id: 20,
            team: 0,
            position: ps[2].position,
            carrier: Some(2),
        }],
        spots: vec![],
        hill: None,
    });
    let mut m = Mind::new(Skill::Heroic, 1);
    m.step(&p, None);
    assert_eq!(m.doing, Doing::Fight(2));
    p.tick += 1;
    p.me.as_mut().unwrap().aim = [-1.0, 0.0, 0.0];
    let pad = m.step(&p, None);
    assert_eq!(m.doing, Doing::Play(Goal::Hunt(2, ps[2].position)));
    assert_eq!(pad.right_trigger, 0.0);
}

#[test]
fn a_contested_hill_draws_fire_to_the_opponent_scoring_in_it() {
    let ps = [
        player(0, 0, 0.0, 0.0),
        player(1, 1, 5.0, 1.0),
        player(2, 1, 9.0, 0.0),
    ];
    let mut p = scene(0, 0, &ps);
    let corners = vec![
        [7.0, -2.0, 0.0],
        [11.0, -2.0, 0.0],
        [11.0, 2.0, 0.0],
        [7.0, 2.0, 0.0],
    ];
    p.objective = Some(Objective {
        state: Default::default(),
        game: Game::Koth,
        me: 0,
        items: vec![],
        spots: vec![],
        hill: Some(Hill { corners }),
    });
    let mut m = Mind::new(Skill::Heroic, 5);
    m.step(&p, None);
    assert_eq!(m.doing, Doing::Fight(2));
}

#[test]
fn an_attacker_keeps_progressing_when_a_close_enemy_baits_it_away_from_the_flag() {
    let ps = [player(0, 0, 0.0, 0.0), player(1, 1, 3.0, 0.0)];
    let mut p = scene(0, 0, &ps);
    p.objective = Some(Objective {
        state: Default::default(),
        game: Game::Ctf,
        me: 0,
        items: vec![Item {
            id: 31,
            team: 1,
            position: [-40.0, 1.0, 0.0],
            carrier: None,
        }],
        spots: vec![],
        hill: None,
    });
    let mut m = Mind::new(Skill::Heroic, 5);
    m.step(&p, None);
    p.tick = 10;
    let pad = m.step(&p, None);
    assert_eq!(m.doing, Doing::Fight(1));
    assert_eq!(pad.right_trigger, 1.0);
    assert!(heading(pad.left_stick).1 < -0.8);
}

#[test]
fn a_roaming_bot_prefers_the_sighting_away_from_its_last_death() {
    let map = MapData {
        places: Default::default(),
        nav: NavMesh::new(vec![square(-20.0, -20.0, 40.0)], vec![]),
        walls: Walls::default(),
    };
    let mut m = Mind::new(Skill::Legendary, 10);
    let p = scene(2, 30, &[player(2, 1, 0.0, 0.0)]);
    let hot = [10.0, 0.0, 0.0];
    let safe = [-12.0, 0.0, 0.0];
    m.roam.saw(safe, 0);
    m.roam.saw(hot, 29);
    m.hear_deaths(
        &p,
        &[Kill {
            seq: 1,
            tick: 29,
            at: Some(hot),
            victim: 2,
            attacker: Some(1),
            betrayal: false,
        }],
        Some(&map),
    );
    let pad = m.step(&p, Some(&map));
    assert_eq!(m.doing, Doing::Roam);
    assert!(
        heading(pad.left_stick).1 < -0.8,
        "investigate the safe sighting to the west"
    );
}

#[test]
fn a_dry_bot_holds_a_scoring_hill_with_the_only_gun_outside() {
    use crate::arms::{Ammo, Arms, Gun, Pickup};
    let mut p = scene(0, 10, &[player(0, 0, 0.0, 0.0)]);
    p.me.as_mut().unwrap().arms = Some(Arms::default());
    p.pickups.push(Pickup {
        id: 11,
        position: [12.0, 0.0, 0.0],
        gun: Gun {
            def: 8,
            ammo: Ammo::Battery(1.0),
            ..Default::default()
        },
    });
    p.objective = Some(Objective {
        state: Default::default(),
        game: Game::Koth,
        me: 0,
        items: vec![],
        spots: vec![],
        hill: Some(Hill {
            corners: square(-2.0, -2.0, 4.0).verts,
        }),
    });
    let mut m = Mind::new(Skill::Normal, 3);
    let pad = m.step(&p, None);
    assert!(matches!(m.doing, Doing::Play(Goal::Hold(..))));
    assert_eq!(pad.left_stick, (0.0, 0.0));
}

fn temperament_death(tick: u32, victim: u32, attacker: u32) -> Kill {
    Kill {
        seq: u64::from(tick),
        tick,
        victim,
        attacker: Some(attacker),
        betrayal: false,
        at: Some([0.0; 3]),
    }
}

#[test]
fn temperament_deaths_streaks_composure_and_clock_decay() {
    let ps = [player(0, 0, 0.0, 0.0), player(1, 1, 8.0, 0.0)];
    let mut p = scene(0, 10, &ps);
    let mut easy = Mind::new(Skill::Easy, 10);
    let mut legendary = Mind::new(Skill::Legendary, 10);
    for m in [&mut easy, &mut legendary] {
        m.hear_deaths(
            &p,
            &[temperament_death(10, 1, 0), temperament_death(10, 2, 0)],
            None,
        );
        assert_eq!(m.decisions.mood().streak, 2);
        assert!(m.decisions.mood().confidence > 0.0);
    }
    assert!(easy.decisions.mood().confidence > legendary.decisions.mood().confidence);
    p.tick = 11;
    p.me = None;
    let allied_betrayal = Kill {
        betrayal: true,
        ..temperament_death(11, 0, 1)
    };
    for m in [&mut easy, &mut legendary] {
        let before = m.decisions.mood().confidence;
        m.hear_deaths(&p, &[allied_betrayal], None);
        m.step(&p, None);
        let mood = m.decisions.mood();
        assert_eq!(mood.streak, 0);
        assert!(mood.confidence < before);
        assert!(mood.tilt > 0.0 && mood.fear > 0.0);
        assert_eq!(mood.revenge_target, None);
    }
    let easy_tilt = easy.decisions.mood().tilt;
    let legendary_tilt = legendary.decisions.mood().tilt;
    assert!(easy_tilt > legendary_tilt);
    p.tick = 300;
    // The same elapsed game time gives the same decay when frames are skipped.
    let mut every_tick = easy.clone();
    for tick in 12..=p.tick {
        let mut frame = p.clone();
        frame.tick = tick;
        every_tick.step(&frame, None);
    }
    easy.step(&p, None);
    legendary.step(&p, None);
    assert!((every_tick.decisions.mood().tilt - easy.decisions.mood().tilt).abs() < 1e-5);
    assert!(easy.decisions.mood().tilt < easy_tilt);
    assert!(
        legendary.decisions.mood().tilt / legendary_tilt < easy.decisions.mood().tilt / easy_tilt
    );
    p.tick = 1;
    easy.step(&p, None);
    assert_eq!(easy.decisions.mood(), MoodView::default());
}

#[test]
fn temperament_favored_duel_and_two_crouch_presses_raise_tilt() {
    use crate::arms::{Ammo, Arms, Gun};

    let ps = [
        player(0, 0, 0.0, 0.0),
        player(1, 1, 4.0, 0.0),
        player(2, 1, 3.0, 0.0),
    ];
    let mut p = scene(0, 10, &ps[..2]);
    p.me.as_mut().unwrap().arms = Some(Arms {
        held: Some(Gun {
            ammo: Ammo::Battery(1.0),
            ..Gun::default()
        }),
        ..Arms::default()
    });
    p.observations.contacts = vec![cue(1, [-1.0, 0.0, 0.0], 0.4, false)];
    let mut duel = Mind::new(Skill::Easy, 5);
    let mut ordinary = duel.clone();
    duel.step(&p, None);
    p.others.push(ps[2]);
    ordinary.step(&p, None);
    p.tick += 1;
    p.me = None;
    let k = temperament_death(p.tick, 0, 1);
    duel.hear_deaths(&p, &[k], None);
    ordinary.hear_deaths(&p, &[k], None);
    assert!(duel.decisions.mood().tilt > ordinary.decisions.mood().tilt);
    assert_eq!(duel.decisions.mood().revenge_target, Some(1));

    let mut no_taunt = duel.clone();
    for tick in 12..=17 {
        p.tick = tick;
        p.others[0].position = if tick <= 14 {
            [4.0, 0.0, 0.0]
        } else {
            [1.0, 0.0, 0.0]
        };
        p.observations.contacts[0].crouching = matches!(tick, 12 | 14 | 15 | 17);
        duel.hear_deaths(&p, &[], None);
        p.observations.contacts[0].crouching = false;
        no_taunt.hear_deaths(&p, &[], None);
        if tick < 17 {
            assert_eq!(duel.decisions.mood().tilt, no_taunt.decisions.mood().tilt);
        }
    }
    assert!(duel.decisions.mood().tilt > no_taunt.decisions.mood().tilt);
    p.tick += 1;
    let before = duel.decisions.mood().tilt;
    p.observations.contacts[0].crouching = true;
    duel.hear_deaths(&p, &[temperament_death(p.tick, 1, 2)], None);
    assert!(
        duel.decisions.mood().tilt < before,
        "the taunt only counts once"
    );
    assert_eq!(duel.decisions.mood().revenge_target, None);
}

#[test]
fn temperament_confidence_presses_tilt_pursues_and_fear_regroups() {
    let ps = [
        player(0, 0, 0.0, 0.0),
        player(1, 1, 7.0, 0.0),
        player(2, 1, 8.0, 0.0),
        player(3, 0, -20.0, 0.0),
    ];
    let mut p = scene(0, 10, &[ps[0], ps[2], ps[3]]);
    p.me.as_mut().unwrap().shield = 0.7;
    let mut calm = Mind::new(Skill::Easy, 10);
    calm.decisions.personality.aggression = 0.55;
    calm.decisions.personality.composure = 0.08;
    calm.decisions.personality.caution = 0.9;
    let mut confident = calm.clone();
    let mut fearful = calm.clone();
    let mut vengeful = calm.clone();
    vengeful.decisions.personality.caution = 0.1;
    let mut dead = p.clone();
    dead.me = None;
    for tick in [p.tick - 1, p.tick] {
        dead.tick = tick;
        let death = temperament_death(tick, 0, 2);
        for bot in [&mut vengeful, &mut fearful] {
            bot.hear_deaths(&dead, &[death], None);
            bot.step(&dead, None);
        }
    }
    confident.hear_deaths(
        &p,
        &[
            temperament_death(p.tick, 1, 0),
            temperament_death(p.tick, 2, 0),
        ],
        None,
    );
    let mut both = p.clone();
    both.others.push(ps[1]);
    let mut avenger = vengeful.clone();
    avenger.step(&both, None);
    assert_eq!(
        avenger.doing,
        Doing::Fight(2),
        "retaliates against the farther visible target"
    );

    for (bot, expected) in [
        (&mut calm, Posture::Normal),
        (&mut confident, Posture::Press),
        (&mut vengeful, Posture::Press),
        (&mut fearful, Posture::Withdraw),
    ] {
        bot.step(&p, None);
        assert_eq!(
            bot.decisions.posture,
            expected,
            "{:?}",
            bot.decisions.mood()
        );
    }
    p.tick += 1;
    p.others.push(ps[1]);
    vengeful.step(&p, None);
    assert_eq!(vengeful.doing, Doing::Fight(2));
    p.tick += 1;
    p.me.as_mut().unwrap().aim = [-1.0, 0.0, 0.0];
    fearful.step(&p, None);
    assert_eq!(fearful.doing, Doing::Support(ps[3].position));
    // Turning away gives the avenger no new position from the kill feed.
    p.others[0].position = [15.0, 2.0, 0.0];
    vengeful.step(&p, None);
    assert_eq!(vengeful.doing, Doing::Chase(ps[2].position));
}

#[test]
fn temperament_patience_holds_an_angle_and_changes_radio_cadence() {
    let ps = [player(0, 0, 0.0, 0.0), player(1, 1, 8.0, 0.0)];
    let mut p = scene(0, 0, &ps);
    let mut patient = Mind::new(Skill::Legendary, 2);
    patient.decisions.personality.patience = 0.9;
    patient.decisions.personality.chattiness = 0.8;
    let mut eager = patient.clone();
    eager.decisions.personality.patience = 0.35;
    eager.decisions.personality.chattiness = 0.36;

    for t in 0..=20 {
        p.tick = t;
        for (m, interval) in [(&mut patient, 8), (&mut eager, 17)] {
            m.step(&p, None);
            assert_eq!(m.decisions.report.is_some(), t % interval == 0);
        }
    }
    p.tick += 1;
    p.me.as_mut().unwrap().shield = 0.35;
    eager.step(&p, None);
    assert!(
        eager.decisions.report.is_some_and(|c| c.help),
        "help goes out immediately"
    );

    p.tick += 1;
    p.me.as_mut().unwrap().shield = 1.0;
    p.others.clear();
    assert_eq!(patient.step(&p, None).left_stick, (0.0, 0.0));
    assert_eq!(patient.doing, Doing::HoldAngle(ps[1].position));
    assert!(eager.step(&p, None).left_stick.1 > 0.0);
    assert_eq!(eager.doing, Doing::Chase(ps[1].position));
    p.tick += 35;
    patient.step(&p, None);
    assert_eq!(patient.doing, Doing::Chase(ps[1].position));
}
