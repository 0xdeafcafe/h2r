use super::*;
use crate::brain::Me;
use crate::los::Walls;
use crate::nav::{Edge, NavMesh, Poly};

const X: Vec3 = [1.0, 0.0, 0.0];

fn me(at: Vec3, aim: Vec3) -> Option<Me> {
    Some(Me {
        position: at,
        aim,
        team: 0,
        ..Me::default()
    })
}
fn enemy(id: u32, at: Vec3) -> Seen {
    Seen {
        id,
        position: at,
        team: 1,
    }
}
fn look(
    m: &mut Mind,
    aim: Vec3,
    others: Vec<Seen>,
    tick: u32,
    map: Option<&MapData>,
) -> HostPadState {
    m.step(
        &Perception {
            observations: Default::default(),
            me: me([0.0; 3], aim),
            others,
            teams: false,
            tick,
            pickups: vec![],
            objective: None,
        },
        map,
    )
}

/// Two floor squares side by side (0..10 and 10..20 in X, 0..10 in Y).
fn floor() -> NavMesh {
    let a = Poly::new(vec![
        [0.0, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [10.0, 10.0, 0.0],
        [0.0, 10.0, 0.0],
    ]);
    let b = Poly::new(vec![
        [10.0, 0.0, 0.0],
        [20.0, 0.0, 0.0],
        [20.0, 10.0, 0.0],
        [10.0, 10.0, 0.0],
    ]);
    let (p, q) = ([10.0, 0.0, 0.0], [10.0, 10.0, 0.0]);
    NavMesh::new(
        vec![a, b],
        vec![
            Edge {
                from: 0,
                to: 1,
                a: p,
                b: q,
                kind: crate::nav::Kind::Walk,
            },
            Edge {
                from: 1,
                to: 0,
                a: p,
                b: q,
                kind: crate::nav::Kind::Walk,
            },
        ],
    )
}

/// A wall across X = 5 from Y -20 to 20, Z 0 to 3.
fn wall() -> Walls {
    let q = [
        [5.0, -20.0, 0.0],
        [5.0, 20.0, 0.0],
        [5.0, 20.0, 3.0],
        [5.0, -20.0, 3.0],
    ];
    Walls::from_polygons([&q[..]])
}

#[test]
fn the_skills_get_better_in_order() {
    let a: Vec<Aim> = Skill::ALL.iter().map(|s| s.aim()).collect();
    for w in a.windows(2) {
        assert!(w[1].reaction < w[0].reaction);
        assert!(w[1].error < w[0].error);
        assert!(w[1].lead > w[0].lead);
        assert!(w[1].fire_cone < w[0].fire_cone);
    }
    assert_eq!(Skill::parse(" Heroic "), Some(Skill::Heroic));
    assert_eq!(Skill::parse("hard"), None);
}

#[test]
fn a_wall_hides_an_enemy_and_so_does_its_back() {
    let map = MapData {
        nav: NavMesh::default(),
        walls: wall(),
    };
    let p = |aim, at| Perception {
        observations: Default::default(),
        me: me([0.0, 0.0, 0.0], aim),
        others: vec![enemy(1, at)],
        teams: false,
        tick: 0,
        pickups: vec![],
        objective: None,
    };
    assert_eq!(
        Mind::visible(&p(X, [8.0, 0.0, 0.0]), Some(&map)).len(),
        0,
        "behind the wall"
    );
    assert_eq!(Mind::visible(&p(X, [4.0, 0.0, 0.0]), Some(&map)).len(), 1);
    assert_eq!(
        Mind::visible(&p([-1.0, 0.0, 0.0], [4.0, 0.0, 0.0]), Some(&map)).len(),
        0,
        "behind the bot"
    );
    assert_eq!(
        Mind::visible(&p([-1.0, 0.0, 0.0], [2.0, 0.0, 0.0]), Some(&map)).len(),
        1,
        "but near"
    );
    assert_eq!(
        Mind::visible(&p(X, [8.0, 0.0, 0.0]), None).len(),
        1,
        "no map: no walls"
    );
}

#[test]
fn it_waits_its_reaction_time_before_turning_and_firing() {
    for skill in Skill::ALL {
        let mut m = Mind::new(skill, 7);
        let at = [4.0, 4.0, 0.0];
        let mut first = None;
        for tick in 100..130 {
            let pad = look(&mut m, X, vec![enemy(1, at)], tick, None);
            assert_eq!(pad.right_trigger, 0.0, "{skill:?} tick {tick}");
            if pad.right_stick != (0.0, 0.0) {
                first = Some(tick - 100);
                assert!(
                    (-0.26..-0.1).contains(&pad.right_stick.0),
                    "{skill:?} {pad:?}"
                );
                break;
            }
        }
        let delay = first.expect("starts moving the thumb");
        assert!(
            (tactics::MIN_REACTION..=18).contains(&delay),
            "{skill:?} {delay}"
        );
        assert!(matches!(m.doing, Doing::Fight(1)));
    }
}

#[test]
fn on_target_it_fires_and_off_target_it_does_not() {
    let mut m = Mind::new(Skill::Legendary, 3);
    look(&mut m, X, vec![enemy(1, [6.0, 0.0, 0.0])], 0, None);
    let on = look(&mut m, X, vec![enemy(1, [6.0, 0.0, 0.0])], 10, None);
    assert_eq!(on.right_trigger, 1.0);
    let mut m = Mind::new(Skill::Legendary, 3);
    let aim_away = [0.7, 0.7, 0.0];
    look(&mut m, aim_away, vec![enemy(1, [6.0, 0.0, 0.0])], 0, None);
    let off = look(&mut m, aim_away, vec![enemy(1, [6.0, 0.0, 0.0])], 10, None);
    assert_eq!(off.right_trigger, 0.0);
}

#[test]
fn easy_fires_in_bursts() {
    let mut m = Mind::new(Skill::Easy, 3);
    let (on, off) = Skill::Easy.aim().burst.unwrap();
    let r = Skill::Easy.aim().reaction + 4;
    let mut fired = Vec::new();
    for t in 0..(r + on + off) {
        fired.push(look(&mut m, X, vec![enemy(1, [6.0, 0.0, 0.0])], t, None).right_trigger > 0.0);
    }
    let shots = fired.iter().filter(|f| **f).count() as u32;
    let start = fired.iter().position(|f| *f).expect("first burst");
    let burst = fired[start..].iter().take_while(|f| **f).count() as u32;
    assert!((on - 2..=on + 2).contains(&burst), "{fired:?}");
    assert!(fired[start + burst as usize..].contains(&false));
    assert!(shots < fired.len() as u32, "{shots} of {}", fired.len());
}

#[test]
fn it_leads_a_moving_target_more_at_higher_skill() {
    let yaw_after = |skill: Skill| {
        let mut m = Mind::new(skill, 9);
        // Crossing left to right (toward -Y) at 10 wu a second, 15 away.
        for t in 0..=20u32 {
            let y = 2.0 - 10.0 * t as f32 / TICKS;
            look(&mut m, X, vec![enemy(1, [15.0, y, 0.0])], t, None);
        }
        m.tracks[&1].vel
    };
    let v = yaw_after(Skill::Heroic);
    assert!((v[1] + 10.0).abs() < 0.5, "{v:?}");
    let lead = |s: Skill| s.aim().lead;
    assert!(lead(Skill::Legendary) > lead(Skill::Easy));
}

#[test]
fn it_chases_where_it_last_saw_someone_then_forgets() {
    let mut m = Mind::new(Skill::Normal, 1);
    look(&mut m, X, vec![enemy(1, [6.0, 3.0, 0.0])], 0, None);
    let pad = look(&mut m, X, vec![], 5, None);
    assert_eq!(m.doing, Doing::Chase([6.0, 3.0, 0.0]));
    assert!(pad.left_stick.1 > 0.5, "walks there: {:?}", pad.left_stick);
    look(&mut m, X, vec![], 5 + MEMORY_TICKS + 1, None);
    assert_eq!(m.doing, Doing::Roam);
}

#[test]
fn with_a_nav_mesh_it_roams_somewhere_it_can_reach() {
    let map = MapData {
        nav: floor(),
        walls: Walls::default(),
    };
    let mut m = Mind::new(Skill::Normal, 42);
    let p = Perception {
        observations: Default::default(),
        me: me([1.0, 1.0, 0.0], X),
        others: vec![],
        teams: false,
        tick: 0,
        pickups: vec![],
        objective: None,
    };
    let pad = m.step(&p, Some(&map));
    assert_eq!(m.doing, Doing::Roam);
    let g = m.goal.expect("a goal");
    assert!(g == map.nav.polys[0].center || g == map.nav.polys[1].center);
    let moving = pad.left_stick.0.abs() + pad.left_stick.1.abs();
    assert!(moving > 0.3, "{pad:?}");
}

#[test]
fn a_teammate_is_never_a_target() {
    let mut m = Mind::new(Skill::Legendary, 1);
    let p = Perception {
        observations: Default::default(),
        me: me([0.0; 3], X),
        others: vec![Seen {
            id: 1,
            position: [3.0, 0.0, 0.0],
            team: 0,
        }],
        teams: true,
        tick: 0,
        pickups: vec![],
        objective: None,
    };
    for t in 0..20 {
        let pad = m.step(
            &Perception {
                tick: t,
                ..p.clone()
            },
            None,
        );
        assert_eq!(pad.right_trigger, 0.0);
    }
    assert_eq!(m.doing, Doing::Roam);
}

#[test]
fn a_dead_bot_lets_go_of_everything() {
    let mut m = Mind::new(Skill::Normal, 1);
    look(&mut m, X, vec![enemy(1, [6.0, 0.0, 0.0])], 0, None);
    let pad = m.step(&Perception::default(), None);
    assert_eq!(pad, HostPadState::default());
    assert_eq!(m.doing, Doing::Idle);
}

#[test]
fn grenades_are_rare_and_spaced() {
    let mut m = Mind::new(Skill::Legendary, 5);
    let mut throws = Vec::new();
    for t in 0..(60 * 30u32) {
        let pad = look(&mut m, X, vec![enemy(1, [6.0, 0.0, 0.0])], t, None);
        if pad.left_trigger > 0.0 && throws.last().is_none_or(|&l: &u32| t > l + 3) {
            throws.push(t);
        }
    }
    assert!(throws.len() <= 8, "{throws:?}");
    for w in throws.windows(2) {
        assert!(w[1] - w[0] >= GRENADE_COOLDOWN, "{throws:?}");
    }
}

// ---- weapons (section 3.6) ----

use crate::arms::{Ammo, Arms, Gun, Pickup};

const RIFLE: u32 = 0xa001_0001;
const PISTOL: u32 = 0xa002_0002;

fn rounds(loaded: i16, reserve: i16) -> Ammo {
    Ammo::Rounds {
        loaded,
        capacity: 36,
        reserve,
        reserve_max: 108,
    }
}
fn armed(held: Ammo, other: Option<Ammo>) -> Arms {
    Arms {
        held: Some(Gun {
            def: RIFLE,
            ammo: held,
            ..Default::default()
        }),
        other: other.map(|ammo| Gun {
            def: PISTOL,
            ammo,
            ..Default::default()
        }),
        grenades: [2, 0],
        ..Arms::default()
    }
}
/// A bot at `at` with these arms, aiming +X.
fn with(
    m: &mut Mind,
    at: Vec3,
    arms: Arms,
    others: Vec<Seen>,
    pickups: Vec<Pickup>,
    tick: u32,
) -> HostPadState {
    m.step(
        &Perception {
            observations: Default::default(),
            me: Some(Me {
                position: at,
                aim: X,
                arms: Some(arms),
                ..Me::default()
            }),
            others,
            teams: false,
            tick,
            pickups,
            objective: None,
        },
        None,
    )
}

#[test]
fn an_empty_magazine_is_reloaded_with_spaced_taps() {
    let mut m = Mind::new(Skill::Normal, 1);
    let a = armed(rounds(0, 40), None);
    let down: Vec<_> = (0..210)
        .map(|t| {
            with(&mut m, [0.0; 3], a, vec![], vec![], t)
                .buttons
                .contains(RELOAD)
        })
        .collect();
    assert_eq!(m.kit, Kit::Reload);
    let starts: Vec<_> = (1..down.len())
        .filter(|&i| !down[i - 1] && down[i])
        .collect();
    assert!(starts.len() >= 3, "{starts:?}");
    assert!((7..=11).contains(&starts[0]), "{starts:?}");
    for &i in starts.iter().take(3) {
        let hold = down[i..].iter().take_while(|d| **d).count();
        assert!((2..=4).contains(&hold), "tick {i}, {hold}");
    }
    for pair in starts.windows(2) {
        assert!(pair[1] - pair[0] >= RELOAD_COOLDOWN as usize, "{starts:?}");
    }
}

#[test]
fn a_low_magazine_waits_for_a_lull_to_reload() {
    let mut m = Mind::new(Skill::Normal, 1);
    let a = armed(rounds(5, 40), None);
    let foe = || vec![enemy(1, [6.0, 0.0, 0.0])];
    let pad = with(&mut m, [0.0; 3], a, foe(), vec![], 0);
    assert!(!pad.buttons.contains(RELOAD));
    assert_eq!(m.kit, Kit::Fine);
    // Gone from view: not calm yet...
    with(&mut m, [0.0; 3], a, vec![], vec![], 10);
    assert_eq!(m.kit, Kit::Fine);
    // ...then calm. It reaches for X after a short delay.
    let t = 10 + CALM_TICKS;
    let pad = with(&mut m, [0.0; 3], a, vec![], vec![], t);
    assert_eq!(m.kit, Kit::Reload);
    assert!(!pad.buttons.contains(RELOAD));
    assert!((t + 1..=t + 11).any(|t| {
        with(&mut m, [0.0; 3], a, vec![], vec![], t)
            .buttons
            .contains(RELOAD)
    }));
}

#[test]
fn a_dry_gun_switches_to_the_loaded_one() {
    let mut m = Mind::new(Skill::Normal, 1);
    let a = armed(rounds(0, 0), Some(Ammo::Battery(0.9)));
    let pad = with(
        &mut m,
        [0.0; 3],
        a,
        vec![enemy(1, [6.0, 0.0, 0.0])],
        vec![],
        0,
    );
    assert_eq!(m.kit, Kit::Switch);
    assert!(!pad.buttons.contains(SWITCH));
    assert!(!pad.buttons.contains(RELOAD));
    let first = (1..=11).find(|&t| {
        let pad = with(
            &mut m,
            [0.0; 3],
            a,
            vec![enemy(1, [6.0, 0.0, 0.0])],
            vec![],
            t,
        );
        assert!(!pad.buttons.contains(RELOAD), "{pad:?}");
        pad.buttons.contains(SWITCH)
    });
    assert!((7..=11).contains(&first.expect("switches after a delay")));
}

#[test]
fn with_nothing_to_shoot_it_runs_in_and_melees() {
    let mut m = Mind::new(Skill::Legendary, 1);
    let a = armed(rounds(0, 0), Some(Ammo::Battery(0.0)));
    let foe = |x| vec![enemy(1, [x, 0.0, 0.0])];
    with(&mut m, [0.0; 3], a, foe(2.0), vec![], 0);
    let pad = with(&mut m, [0.0; 3], a, foe(2.0), vec![], 20);
    assert_eq!(m.kit, Kit::Melee);
    assert_eq!(pad.right_trigger, 0.0, "nothing to fire");
    assert!(pad.left_stick.1 > 0.9, "straight in: {:?}", pad.left_stick);
    // In reach: melee after reaching for the button.
    let pad = with(&mut m, [0.0; 3], a, foe(1.0), vec![], 40);
    assert!(!pad.buttons.contains(MELEE));
    let first = (41..=45).find(|&t| {
        with(&mut m, [0.0; 3], a, foe(1.0), vec![], t)
            .buttons
            .contains(MELEE)
    });
    assert!((42..=45).contains(&first.expect("swings")));
}

#[test]
fn with_both_dry_it_goes_for_a_gun_and_takes_it() {
    let mut m = Mind::new(Skill::Normal, 1);
    let a = armed(rounds(0, 0), None);
    let smg = Pickup {
        id: 77,
        position: [5.0, 0.0, 0.0],
        gun: Gun {
            def: 0xa003_0003,
            ammo: rounds(36, 36),
            ..Default::default()
        },
    };
    let pad = with(&mut m, [0.0; 3], a, vec![], vec![smg], 0);
    assert_eq!(m.doing, Doing::Rearm(smg.position));
    assert!(
        (0.3..=0.5).contains(&pad.left_stick.1),
        "starts toward it: {pad:?}"
    );
    assert!(!pad.buttons.contains(RELOAD));
    // On it: holds the action button...
    let on = [5.0, 0.1, 0.0];
    for t in 1..TAKE_TICKS {
        assert!(
            with(&mut m, on, a, vec![], vec![smg], t)
                .buttons
                .contains(RELOAD),
            "tick {t}"
        );
    }
    // ...and when it does not come, gives it up for another.
    let pad = with(&mut m, on, a, vec![], vec![smg], TAKE_TICKS + 1);
    assert!(!pad.buttons.contains(RELOAD));
    assert!(m.stuck.blocked(smg.position, TAKE_TICKS + 1));
    with(&mut m, on, a, vec![], vec![smg], TAKE_TICKS + 2);
    assert_ne!(m.doing, Doing::Rearm(smg.position));
}

#[test]
fn a_gun_of_the_same_kind_is_walked_over_not_held_on() {
    let mut m = Mind::new(Skill::Normal, 1);
    let a = armed(rounds(0, 0), None);
    let rifle = Pickup {
        id: 78,
        position: [5.0, 0.0, 0.0],
        gun: Gun {
            def: RIFLE,
            ammo: rounds(36, 36),
            ..Default::default()
        },
    };
    let pad = with(&mut m, [4.9, 0.0, 0.0], a, vec![], vec![rifle], 0);
    assert_eq!(m.doing, Doing::Rearm(rifle.position));
    assert!(!pad.buttons.contains(RELOAD));
}

#[test]
fn without_grenades_it_throws_none() {
    let mut m = Mind::new(Skill::Legendary, 5);
    let mut a = armed(rounds(36, 100), None);
    a.grenades = [0, 0];
    for t in 0..(60 * 30u32) {
        let pad = with(
            &mut m,
            [0.0; 3],
            a,
            vec![enemy(1, [6.0, 0.0, 0.0])],
            vec![],
            t,
        );
        assert_eq!(pad.left_trigger, 0.0, "tick {t}");
    }
}

#[test]
fn shields_down_it_backs_off_instead_of_chasing() {
    let mut m = Mind::new(Skill::Normal, 1);
    let p = |shield, others, tick| Perception {
        observations: Default::default(),
        me: Some(Me {
            position: [0.0; 3],
            aim: X,
            shield,
            ..Me::default()
        }),
        others,
        teams: false,
        tick,
        pickups: vec![],
        objective: None,
    };
    m.step(&p(1.0, vec![enemy(1, [6.0, 0.0, 0.0])], 0), None);
    let pad = m.step(&p(0.1, vec![], 5), None);
    assert_eq!(m.doing, Doing::Recover([6.0, 0.0, 0.0]));
    assert!(pad.left_stick.1 < -0.5, "backs away: {:?}", pad.left_stick);
    // Still low (over the line, under the way back): still recovering.
    m.step(&p(0.5, vec![], 10), None);
    assert!(matches!(m.doing, Doing::Recover(_)));
    // Back up: the chase goes on.
    m.step(&p(1.0, vec![], 15), None);
    assert_eq!(m.doing, Doing::Chase([6.0, 0.0, 0.0]));
}

// ---- getting unstuck (section 3.2) ----

#[test]
fn a_bot_against_a_wall_sidesteps_jumps_then_gives_up_the_chase() {
    let mut m = Mind::new(Skill::Normal, 1);
    // It saw someone at (13, 0) (out of grenade range), then lost them;
    // it cannot move.
    look(&mut m, X, vec![enemy(1, [13.0, 0.0, 0.0])], 0, None);
    let mut log = Vec::new();
    for t in 1..=(4 * crate::stuck::WINDOW + 2) {
        let pad = look(&mut m, X, vec![], t, None);
        log.push((t, pad, m.doing));
    }
    let w = crate::stuck::WINDOW;
    let at = |t: u32| log.iter().find(|l| l.0 == t).unwrap();
    // Walking at first.
    assert!(at(3).1.left_stick.1 > 0.9);
    // A sidestep: sideways and back after a few ticks, still facing the way.
    let side = at(w + 4).1;
    assert!(side.left_stick.0.abs() > 0.5, "{:?}", side.left_stick);
    assert!(side.left_stick.1 < 0.0, "{:?}", side.left_stick);
    assert!(
        side.right_stick.0.abs() < 0.05,
        "keeps facing: {:?}",
        side.right_stick
    );
    // A jump, then a crouch-jump.
    assert!(at(2 * w + 1).1.buttons.contains(JUMP));
    assert!(at(3 * w + 1).1.buttons.contains(JUMP));
    assert!(at(4 * w - 1).1.buttons.contains(CROUCH));
    // Then it gives up on that point: no more chase.
    assert_eq!(m.stuck.repaths, 1);
    assert_eq!(m.doing, Doing::Roam);
    assert!(m.stuck.blocked([13.0, 0.0, 0.0], 4 * w + 1));
    assert_eq!(at(4 * w + 1).2, Doing::Chase([13.0, 0.0, 0.0]));
}

#[test]
fn a_bot_that_gets_free_walks_on() {
    let mut m = Mind::new(Skill::Normal, 1);
    look(&mut m, X, vec![enemy(1, [13.0, 0.0, 0.0])], 0, None);
    let w = crate::stuck::WINDOW;
    for t in 1..=(w + 2) {
        look(&mut m, X, vec![], t, None);
    }
    assert_eq!(m.stuck.level, crate::stuck::Level::Sidestep);
    // Moved on toward it: walking straight again.
    let pad = m.step(
        &Perception {
            observations: Default::default(),
            me: me([1.0, 0.0, 0.0], X),
            others: vec![],
            teams: false,
            tick: w + 3,
            pickups: vec![],
            objective: None,
        },
        None,
    );
    assert_eq!(m.stuck.level, crate::stuck::Level::Moving);
    let still = look(&mut m, X, vec![], w + 6, None);
    assert!(
        still.left_stick.1 > 0.8,
        "returns to the path: {pad:?} then {still:?}"
    );
}

#[test]
fn with_a_nav_mesh_a_path_goes_around_where_it_got_stuck() {
    // The ring of four squares: from the first to the far corner, by the
    // east square unless that way is blacklisted.
    let sq = |x: f32, y: f32| {
        Poly::new(vec![
            [x, y, 0.0],
            [x + 2.0, y, 0.0],
            [x + 2.0, y + 2.0, 0.0],
            [x, y + 2.0, 0.0],
        ])
    };
    let mut edges = Vec::new();
    for (f, t, a, b) in [
        (0, 1, [2.0, 0.0, 0.0], [2.0, 2.0, 0.0]),
        (0, 2, [0.0, 2.0, 0.0], [2.0, 2.0, 0.0]),
        (1, 3, [2.0, 2.0, 0.0], [4.0, 2.0, 0.0]),
        (2, 3, [2.0, 2.0, 0.0], [2.0, 4.0, 0.0]),
    ] {
        for (f, t) in [(f, t), (t, f)] {
            edges.push(Edge {
                from: f,
                to: t,
                a,
                b,
                kind: crate::nav::Kind::Walk,
            });
        }
    }
    let map = MapData {
        nav: NavMesh::new(
            vec![sq(0.0, 0.0), sq(2.0, 0.0), sq(0.0, 2.0), sq(2.0, 2.0)],
            edges,
        ),
        walls: Walls::default(),
    };
    let mut m = Mind::new(Skill::Normal, 1);
    // Stuck east of the start: the portal into the east square is out.
    m.stuck.blacklist([2.0, 1.0, 0.0], 0);
    let mut pad = HostPadState::default();
    let wp = m
        .go(
            [1.0, 0.5, 0.0],
            [3.5, 3.5, 0.0],
            1,
            Some(&map),
            &mut pad,
            &[],
        )
        .expect("a way");
    let f = m.follower.as_ref().unwrap();
    assert!(
        f.path
            .iter()
            .all(|w| w.at[0] <= 2.0 + 1e-4 || w.at[1] >= 2.0 - 1e-4),
        "{:?}",
        f.path
    );
    // North first.
    assert!(wp.face[1] > wp.face[0].abs(), "{wp:?}");
}

// ---- movement (section 3.2: corners, footing, roaming) ----

#[test]
fn roaming_goes_where_it_last_saw_an_enemy() {
    let map = MapData {
        nav: floor(),
        walls: Walls::default(),
    };
    let mut m = Mind::new(Skill::Legendary, 5);
    let p = |others, tick| Perception {
        observations: Default::default(),
        me: me([2.0, 5.0, 0.0], X),
        others,
        teams: false,
        tick,
        pickups: vec![],
        objective: None,
    };
    m.step(&p(vec![enemy(1, [17.0, 5.0, 0.0])], 0), Some(&map));
    // Long after the chase is forgotten, it roams back there.
    m.step(&p(vec![], MEMORY_TICKS + 100), Some(&map));
    assert_eq!(m.doing, Doing::Roam);
    assert_eq!(m.goal, Some([17.0, 5.0, 0.0]));
}

#[test]
fn it_chases_someone_last_seen_in_the_air() {
    // Seen mid-jump, 2.5 over the floor: the chase goes to the floor under
    // that point instead of giving up for want of a path.
    let map = MapData {
        nav: floor(),
        walls: Walls::default(),
    };
    let mut m = Mind::new(Skill::Normal, 1);
    let p = |others, tick| Perception {
        observations: Default::default(),
        me: me([2.0, 5.0, 0.0], X),
        others,
        teams: false,
        tick,
        pickups: vec![],
        objective: None,
    };
    let air = [14.0, 5.0, 2.5];
    m.step(&p(vec![enemy(1, air)], 0), Some(&map));
    for t in 5..8 {
        let pad = m.step(&p(vec![], t), Some(&map));
        assert_eq!(m.doing, Doing::Chase(air), "tick {t}");
        assert!(pad.left_stick.1 > 0.5, "tick {t}: {pad:?}");
    }
}

#[test]
fn fighting_on_a_narrow_bridge_it_does_not_strafe_off() {
    let quad = |x0: f32, y0: f32, x1: f32, y1: f32, z: f32| {
        Poly::new(vec![[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]])
    };
    let map = MapData {
        nav: NavMesh::new(
            vec![
                quad(-20.0, -0.4, 20.0, 0.4, 2.0),
                quad(-20.0, -20.0, 20.0, 20.0, 0.0),
            ],
            vec![],
        ),
        walls: Walls::default(),
    };
    let mut m = Mind::new(Skill::Legendary, 2);
    let mut walked = 0;
    for t in 0..120 {
        let pad = m.step(
            &Perception {
                observations: Default::default(),
                me: me([0.0, 0.0, 2.0], X),
                others: vec![enemy(1, [6.0, 0.0, 2.0])],
                teams: false,
                tick: t,
                pickups: vec![],
                objective: None,
            },
            Some(&map),
        );
        assert!(matches!(m.doing, Doing::Fight(1)));
        assert!(pad.left_stick.0.abs() < 0.05, "tick {t}: {pad:?}");
        if pad.left_stick.1.abs() > 0.5 {
            walked += 1;
        }
    }
    // Not frozen either: it steps along the bridge instead.
    assert!(walked > 60, "{walked}");
}

// ---- objectives (section 3.7) ----

use crate::objective::{Game, Item, Objective, Spot, kind};

const HOME: Vec3 = [2.0, 5.0, 0.0];

/// CTF with our (red, team 0) flag spawn at [`HOME`] and the blue flag
/// carried by `carrier`.
fn carrying(carrier: Option<u32>, at: Vec3) -> Objective {
    Objective {
        game: Game::Ctf,
        me: 1,
        items: vec![Item {
            id: 0xe000_0101,
            team: 1,
            position: at,
            carrier,
        }],
        spots: vec![Spot {
            kind: kind::CTF_SPAWN,
            team: Some(0),
            position: HOME,
        }],
        hill: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn play(
    m: &mut Mind,
    at: Vec3,
    aim: Vec3,
    me: Me,
    others: Vec<Seen>,
    objective: Objective,
    tick: u32,
    map: Option<&MapData>,
) -> HostPadState {
    m.step(
        &Perception {
            observations: Default::default(),
            me: Some(Me {
                position: at,
                aim,
                ..me
            }),
            others,
            teams: true,
            tick,
            pickups: vec![],
            objective: Some(objective),
        },
        map,
    )
}

#[test]
fn the_flag_carrier_walks_home_along_the_nav_mesh() {
    let map = MapData {
        nav: floor(),
        walls: Walls::default(),
    };
    let mut m = Mind::new(Skill::Normal, 7);
    let (mut at, mut yaw) = ([18.0f32, 5.0, 0.0], 0.0f32);
    let mut reached = None;
    for t in 0..900 {
        let aim = [yaw.cos(), yaw.sin(), 0.0];
        let pad = play(
            &mut m,
            at,
            aim,
            Me::default(),
            vec![],
            carrying(Some(1), at),
            t,
            Some(&map),
        );
        assert_eq!(m.doing, Doing::Carry(HOME), "tick {t}");
        // Never a switch (it drops the flag) or a shot.
        assert!(!pad.buttons.contains(SWITCH), "tick {t}");
        assert_eq!(pad.right_trigger, 0.0);
        // The game's turning and walking, roughly.
        yaw -= pad.right_stick.0 * 0.15;
        let (c, s) = (yaw.cos(), yaw.sin());
        let (right, fwd) = pad.left_stick;
        at[0] += 0.15 * (fwd * c + right * s);
        at[1] += 0.15 * (fwd * s - right * c);
        if dist_xy(at, HOME) < 0.5 {
            reached = Some(t);
            break;
        }
    }
    assert!(reached.is_some(), "stopped at {at:?}");
}

#[test]
fn a_carrier_does_not_back_off_rearm_or_switch_and_melees_who_blocks_it() {
    // Dry arms (the flag in hand reads as not a gun), shields popped and
    // an enemy at arm's length: still home, meleeing.
    let me = Me {
        shield: 0.05,
        arms: Some(armed(Ammo::None, Some(rounds(30, 60)))),
        ..Me::default()
    };
    let enemy = Seen {
        id: 5,
        position: [9.0, 5.0, 0.0],
        team: 1,
    };
    let mut m = Mind::new(Skill::Legendary, 3);
    let mut melee = false;
    for t in 0..40 {
        let at = [10.0, 5.0, 0.0];
        let pad = play(
            &mut m,
            at,
            [-1.0, 0.0, 0.0],
            me,
            vec![enemy],
            carrying(Some(1), at),
            t,
            None,
        );
        assert_eq!(m.doing, Doing::Carry(HOME), "tick {t}");
        assert!(!pad.buttons.contains(SWITCH) && !pad.buttons.contains(RELOAD));
        assert_eq!(pad.right_trigger, 0.0);
        melee |= pad.buttons.contains(MELEE);
    }
    assert!(melee, "the blocker gets the flag");
    assert_eq!(m.kit, Kit::Fine);
}

#[test]
fn without_the_flag_a_dry_bot_still_rearms_first() {
    // The priorities hold for everyone but the carrier.
    let me = Me {
        arms: Some(armed(Ammo::None, None)),
        ..Me::default()
    };
    let gun = Pickup {
        id: 9,
        position: [5.0, 5.0, 0.0],
        gun: Gun {
            def: PISTOL,
            ammo: rounds(10, 10),
            ..Default::default()
        },
    };
    let mut m = Mind::new(Skill::Normal, 3);
    m.step(
        &Perception {
            observations: Default::default(),
            me: Some(Me {
                position: [10.0, 5.0, 0.0],
                ..me
            }),
            others: vec![],
            teams: true,
            tick: 0,
            pickups: vec![gun],
            objective: Some(carrying(None, [40.0, 5.0, 0.0])),
        },
        None,
    );
    assert_eq!(m.doing, Doing::Rearm([5.0, 5.0, 0.0]));
}

#[test]
fn the_objective_comes_before_chasing_and_roaming() {
    let mut m = Mind::new(Skill::Normal, 3);
    // Their flag lies at 40: alone, it attacks.
    play(
        &mut m,
        [10.0, 5.0, 0.0],
        X,
        Me::default(),
        vec![],
        carrying(None, [40.0, 5.0, 0.0]),
        0,
        None,
    );
    assert_eq!(m.doing, Doing::Play(Goal::Go([40.0, 5.0, 0.0])));
}

#[test]
fn the_ball_carrier_runs_from_the_nearest_enemy() {
    let mut m = Mind::new(Skill::Normal, 3);
    let ball = Objective {
        game: Game::Oddball,
        me: 1,
        items: vec![Item {
            id: 0xe000_0200,
            team: 0,
            position: [0.0; 3],
            carrier: Some(1),
        }],
        spots: vec![],
        hill: None,
    };
    let enemy = Seen {
        id: 5,
        position: [6.0, 0.0, 0.0],
        team: 1,
    };
    // Facing +X, the enemy ahead: it walks back (-X), not at them.
    let pad = play(
        &mut m,
        [0.0; 3],
        X,
        Me::default(),
        vec![enemy],
        ball.clone(),
        0,
        None,
    );
    assert_eq!(m.doing, Doing::Keep);
    assert!(pad.left_stick.1 < -0.3, "{pad:?}");
    assert_eq!(pad.right_trigger, 0.0);
    // Close up: melee with the ball.
    let mut melee = false;
    for t in 1..30 {
        let near = Seen {
            position: [0.8, 0.0, 0.0],
            ..enemy
        };
        let pad = play(
            &mut m,
            [0.0; 3],
            X,
            Me::default(),
            vec![near],
            ball.clone(),
            t,
            None,
        );
        melee |= pad.buttons.contains(MELEE);
    }
    assert!(melee);
}

#[test]
fn in_the_hill_it_stays_and_out_of_it_it_goes_there() {
    let hill = |c: Vec3| Objective {
        game: Game::Koth,
        me: 1,
        items: vec![],
        spots: vec![],
        hill: Some(crate::objective::Hill {
            corners: vec![
                [c[0] - 3.0, c[1] - 3.0, 0.0],
                [c[0] + 3.0, c[1] - 3.0, 0.0],
                [c[0] + 3.0, c[1] + 3.0, 0.0],
                [c[0] - 3.0, c[1] + 3.0, 0.0],
            ],
        }),
    };
    let mut m = Mind::new(Skill::Normal, 3);
    // In it, near its centre: holds, standing.
    let pad = play(
        &mut m,
        [10.5, 10.0, 0.0],
        X,
        Me::default(),
        vec![],
        hill([10.0, 10.0, 0.0]),
        0,
        None,
    );
    assert!(
        matches!(m.doing, Doing::Play(Goal::Hold(..))),
        "{:?}",
        m.doing
    );
    assert_eq!(pad.left_stick, (0.0, 0.0));
    // The hill moves behind it: it turns back for it.
    let pad = play(
        &mut m,
        [10.5, 10.0, 0.0],
        X,
        Me::default(),
        vec![],
        hill([-20.0, 10.0, 0.0]),
        1,
        None,
    );
    assert_eq!(m.doing, Doing::Play(Goal::Go([-20.0, 10.0, 0.0])));
    assert!(
        (0.1..0.3).contains(&pad.right_stick.0.abs()),
        "starts to turn: {pad:?}"
    );
}

#[test]
fn occluded_positions_never_drive_the_stick_or_projectile_lead() {
    let map = MapData {
        nav: NavMesh::default(),
        walls: wall(),
    };
    let mut east = Mind::new(Skill::Legendary, 13);
    for tick in 0..=8 {
        look(
            &mut east,
            X,
            vec![enemy(7, [4.0, 0.0, 0.0])],
            tick,
            Some(&map),
        );
    }
    let mut west = east.clone();
    for tick in 9..20 {
        let east_pad = look(
            &mut east,
            X,
            vec![enemy(7, [6.0, 1.5, 0.0])],
            tick,
            Some(&map),
        );
        let west_pad = look(
            &mut west,
            X,
            vec![enemy(7, [6.0, -1.5, 0.0])],
            tick,
            Some(&map),
        );
        assert_eq!(east_pad, west_pad, "tick {tick}");
        assert_eq!(east_pad.right_trigger, 0.0);
        assert!(!east.tracks.contains_key(&7));
    }
    look(
        &mut east,
        X,
        vec![enemy(7, [4.0, 0.0, 0.0])],
        20,
        Some(&map),
    );
    assert_eq!(
        east.tracks[&7].vel, [0.0; 3],
        "no hidden sample enters the lead"
    );
}

#[test]
fn a_gunshot_can_turn_the_head_without_creating_a_target() {
    let mut m = Mind::new(Skill::Normal, 15);
    let report = [ShotSound {
        at: [0.0, 4.0, EYE],
        tick: 0,
    }];
    let mut looked = false;
    for tick in 0..26 {
        let p = Perception {
            me: me([0.0; 3], X),
            tick,
            ..Perception::default()
        };
        let pad = m.step_with_cues(&p, None, &report, &[]);
        assert_eq!(pad.left_stick, (0.0, 0.0));
        assert_eq!((pad.left_trigger, pad.right_trigger), (0.0, 0.0));
        assert_ne!(m.doing, Doing::Fight(1));
        assert!(m.target.is_none());
        if tick < 5 {
            assert_eq!(pad.right_stick, (0.0, 0.0));
        }
        looked |= pad.right_stick.0 < -0.1;
    }
    assert!(looked);
}

#[test]
fn a_quiet_objective_approach_uses_partial_crouch_walk() {
    let start = [10.0, 5.0, 0.0];
    let flag = [11.4, 5.0, 0.0];
    let mut m = Mind::new(Skill::Normal, 3);
    let pad = play(
        &mut m,
        start,
        X,
        Me::default(),
        vec![],
        carrying(None, flag),
        0,
        None,
    );
    assert_eq!(m.doing, Doing::Play(Goal::Go(flag)));
    assert!(pad.buttons.contains(CROUCH));
    assert!(!pad.buttons.contains(JUMP));
    assert!((0.2..=0.55).contains(&pad.left_stick.1), "{pad:?}");
}

#[test]
fn duplicate_input_frames_reuse_the_same_simulation_tick_pad() {
    let mut a = Mind::new(Skill::Heroic, 42);
    let mut b = a.clone();
    for tick in 0..100 {
        let at = [6.0, (tick as f32 * 0.19).sin() * 3.0, 0.0];
        let expected = look(&mut a, X, vec![enemy(1, at)], tick, None);
        for _ in 0..3 {
            assert_eq!(look(&mut b, X, vec![enemy(1, at)], tick, None), expected);
        }
    }
}

/// Crouch jumps (a jump, then crouch once A is up) a lone bot does over
/// ten calm minutes with `setting`.
fn hops(setting: crate::glitches::Glitches) -> usize {
    let mut m = Mind::new(Skill::Legendary, 11);
    m.glitches = setting;
    let mut count = 0;
    let mut jumped = false;
    for tick in 0..10 * 60 * 30 {
        let pad = look(&mut m, X, vec![], tick, None);
        if pad.buttons.contains(JUMP) {
            jumped = true;
        } else if jumped && pad.buttons.contains(CROUCH) {
            count += 1;
            jumped = false;
        }
    }
    count
}

#[test]
fn glitches_off_never_shows_off_and_lots_crouch_jumps_often_when_calm() {
    use crate::glitches::Glitches;
    assert_eq!(hops(Glitches::Off), 0);
    let lots = hops(Glitches::Lots);
    assert!(lots >= 20, "{lots}");
    assert!(hops(Glitches::Some) < lots);
}

#[test]
fn glitches_run_only_fighting_or_roaming_on_foot() {
    use crate::glitches::{Glitches, Run, Trick, plan};
    // An allowlist: a vehicle's Mount, Drive and Ride (and anything else
    // added later) are never on it.
    let goal = crate::objective::Goal::Go([0.0; 3]);
    for (d, ok) in [
        (Doing::Fight(1), true),
        (Doing::Roam, true),
        (Doing::Idle, false),
        (Doing::Chase([1.0; 3]), false),
        (Doing::HoldAngle([1.0; 3]), false),
        (Doing::Support([1.0; 3]), false),
        (Doing::Supply([1.0; 3]), false),
        (Doing::Rearm([1.0; 3]), false),
        (Doing::Recover([1.0; 3]), false),
        (Doing::Carry([1.0; 3]), false),
        (Doing::Keep, false),
        (Doing::Play(goal), false),
        (Doing::Mount(100), false),
        (Doing::Drive(100), false),
        (Doing::Ride(100), false),
    ] {
        assert_eq!(glitch_doing(d), ok, "{d:?}");
    }
    // A show-off under way stops when the bot turns to something else.
    let hop = || {
        Stunt::Hop(Run::new(
            plan(Trick::CrouchJump, Skill::Legendary, &mut || 0.5),
            0,
        ))
    };
    let mut m = Mind::new(Skill::Legendary, 4);
    m.glitches = Glitches::Lots;
    look(&mut m, X, vec![enemy(1, [6.0, 3.0, 0.0])], 0, None);
    m.stunt = Some(hop());
    look(&mut m, X, vec![], 5, None);
    assert_eq!(m.doing, Doing::HoldAngle([6.0, 3.0, 0.0]));
    assert!(m.stunt.is_none());
    // The hook a seat's pad calls stops both kinds.
    m.stunt = Some(hop());
    m.drop_glitches();
    assert!(m.stunt.is_none() && !m.combo.tricking());
    // In a seat `ride` makes the whole pad, and drops them first.
    m.stunt = Some(hop());
    let mut g = crate::vehicle::tests::ghost(100, [0.0; 3]);
    crate::vehicle::tests::sit(&mut g, 0, 0, 0);
    m.see_vehicles(crate::vehicle::Sight {
        riding: Some(crate::vehicle::Riding {
            vehicle: 100,
            seat: 0,
            role: crate::vehicle::SeatRole::Driver,
        }),
        vehicles: vec![g],
    });
    look(&mut m, X, vec![], 6, None);
    assert_eq!(m.doing, Doing::Drive(100));
    assert!(m.stunt.is_none() && !m.combo.tricking());
}

#[test]
fn temperament_showboats_attempt_risky_melee_but_still_reach_for_the_button() {
    let mut careful = Mind::new(Skill::Legendary, 1);
    careful.decisions.personality.showboating = 0.15;
    let mut showboat = careful.clone();
    showboat.decisions.personality.showboating = 0.9;
    let foe = || vec![enemy(1, [1.0, 0.0, 0.0])];
    let mut first = None;
    for tick in 0..30 {
        let pad = look(&mut showboat, X, foe(), tick, None);
        assert!(
            !look(&mut careful, X, foe(), tick, None)
                .buttons
                .contains(MELEE)
        );
        if pad.buttons.contains(MELEE) {
            first.get_or_insert(tick);
            assert_eq!(pad.right_trigger, 0.0);
        }
    }
    assert!((3..30).contains(&first.expect("showboat tries the melee")));
}
