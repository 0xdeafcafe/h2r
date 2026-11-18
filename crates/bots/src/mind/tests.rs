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
