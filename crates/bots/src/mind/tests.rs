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
            me: me([0.0; 3], aim),
            others,
            teams: false,
            tick,
            pickups: vec![],
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
        me: me([0.0, 0.0, 0.0], aim),
        others: vec![enemy(1, at)],
        teams: false,
        tick: 0,
        pickups: vec![],
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
        let r = skill.aim().reaction;
        // Off to the left, so it has to turn.
        let at = [4.0, 4.0, 0.0];
        let first = look(&mut m, X, vec![enemy(1, at)], 100, None);
        assert_eq!(first.right_stick, (0.0, 0.0), "{skill:?} turns at once");
        let before = look(&mut m, X, vec![enemy(1, at)], 100 + r - 1, None);
        assert_eq!(before.right_stick, (0.0, 0.0), "{skill:?}");
        let after = look(&mut m, X, vec![enemy(1, at)], 100 + r, None);
        assert!(
            after.right_stick.0 < -0.5,
            "{skill:?}: {:?}",
            after.right_stick
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
    let r = Skill::Easy.aim().reaction;
    let mut fired = Vec::new();
    for t in 0..(r + on + off) {
        fired.push(look(&mut m, X, vec![enemy(1, [6.0, 0.0, 0.0])], t, None).right_trigger > 0.0);
    }
    let shots = fired.iter().filter(|f| **f).count() as u32;
    assert!(shots > 0 && shots <= on + 1, "{shots} of {}", fired.len());
}

#[test]
fn it_leads_a_moving_target_more_at_higher_skill() {
    let yaw_after = |skill: Skill| {
        let mut m = Mind::new(skill, 9);
        m.err = (0.0, 0.0);
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
        me: me([1.0, 1.0, 0.0], X),
        others: vec![],
        teams: false,
        tick: 0,
        pickups: vec![],
    };
    let pad = m.step(&p, Some(&map));
    assert_eq!(m.doing, Doing::Roam);
    let g = m.goal.expect("a goal");
    assert!(g == map.nav.polys[0].center || g == map.nav.polys[1].center);
    let moving = pad.left_stick.0.abs() + pad.left_stick.1.abs();
    assert!(moving > 0.5 || g == map.nav.polys[0].center, "{pad:?}");
}

#[test]
fn a_teammate_is_never_a_target() {
    let mut m = Mind::new(Skill::Legendary, 1);
    let p = Perception {
        me: me([0.0; 3], X),
        others: vec![Seen {
            id: 1,
            position: [3.0, 0.0, 0.0],
            team: 0,
        }],
        teams: true,
        tick: 0,
        pickups: vec![],
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
        }),
        other: other.map(|ammo| Gun { def: PISTOL, ammo }),
        grenades: [2, 0],
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
        },
        None,
    )
}

#[test]
fn an_empty_magazine_is_reloaded_with_spaced_taps() {
    let mut m = Mind::new(Skill::Normal, 1);
    let a = armed(rounds(0, 40), None);
    let mut presses = Vec::new();
    for t in 0..(RELOAD_COOLDOWN * 2 + 5) {
        if with(&mut m, [0.0; 3], a, vec![], vec![], t)
            .buttons
            .contains(RELOAD)
        {
            presses.push(t);
        }
    }
    assert_eq!(m.kit, Kit::Reload);
    // Two ticks a tap, a cooldown apart.
    assert_eq!(
        presses,
        vec![
            0,
            1,
            RELOAD_COOLDOWN,
            RELOAD_COOLDOWN + 1,
            2 * RELOAD_COOLDOWN,
            2 * RELOAD_COOLDOWN + 1
        ]
    );
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
    // ...then calm.
    let pad = with(&mut m, [0.0; 3], a, vec![], vec![], 10 + CALM_TICKS);
    assert_eq!(m.kit, Kit::Reload);
    assert!(pad.buttons.contains(RELOAD));
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
    assert!(pad.buttons.contains(SWITCH));
    assert!(!pad.buttons.contains(RELOAD));
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
    // In reach: melee.
    let pad = with(&mut m, [0.0; 3], a, foe(1.0), vec![], 40);
    assert!(pad.buttons.contains(MELEE));
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
        },
    };
    let pad = with(&mut m, [0.0; 3], a, vec![], vec![smg], 0);
    assert_eq!(m.doing, Doing::Rearm(smg.position));
    assert!(pad.left_stick.1 > 0.9, "walks to it: {:?}", pad.left_stick);
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
    assert!(at(2).1.left_stick.1 > 0.9);
    // A sidestep: sideways and back, still facing the way.
    let side = at(w + 2).1;
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
            me: me([1.0, 0.0, 0.0], X),
            others: vec![],
            teams: false,
            tick: w + 3,
            pickups: vec![],
        },
        None,
    );
    assert_eq!(m.stuck.level, crate::stuck::Level::Moving);
    assert!(pad.left_stick.1 > 0.9, "{:?}", pad.left_stick);
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
