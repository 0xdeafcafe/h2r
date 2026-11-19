use super::*;
use crate::los::Walls;
use crate::nav::{NavMesh, Poly};

fn quad(x0: f32, y0: f32, x1: f32, y1: f32, z: f32) -> Poly {
    Poly::new(vec![[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]])
}

/// One wide floor, no walls.
fn open() -> MapData {
    MapData {
        nav: NavMesh::new(vec![quad(-20.0, -20.0, 20.0, 20.0, 0.0)], vec![]),
        walls: Walls::default(),
    }
}

/// A bridge 0.8 wide along X at height 2, over a floor (a pit's bottom)
/// 2 below it.
fn bridge() -> MapData {
    MapData {
        nav: NavMesh::new(
            vec![
                quad(-20.0, -0.4, 20.0, 0.4, 2.0),
                quad(-20.0, -20.0, 20.0, 20.0, 0.0),
            ],
            vec![],
        ),
        walls: Walls::default(),
    }
}

/// An upright wall from `a` to `b` (XY), 3 high.
fn wall(a: [f32; 2], b: [f32; 2]) -> Walls {
    let q = [
        [a[0], a[1], 0.0],
        [b[0], b[1], 0.0],
        [b[0], b[1], 3.0],
        [a[0], a[1], 3.0],
    ];
    Walls::from_polygons([&q[..]])
}

fn input(skill: Skill, tick: u32, at: Vec3, foe: Vec3) -> FightIn {
    FightIn {
        tick,
        skill,
        at,
        foe,
        shield: 1.0,
        rush: false,
        retreat: false,
        reloading: false,
        range: DEFAULT_RANGE,
    }
}

/// Toward the enemy (+) or away (-), for a walk from `at` with the enemy
/// on +X.
fn toward(m: &FightMove) -> f32 {
    m.walk[0]
}

#[test]
fn the_skills_move_better_in_order() {
    let s: Vec<Style> = Skill::ALL.iter().map(|&k| Style::of(k)).collect();
    for w in s.windows(2) {
        assert!(w[1].strafe.1 < w[0].strafe.1, "changes more often");
        assert!(w[1].stand <= w[0].stand);
        assert!(w[1].juke >= w[0].juke);
        assert!(w[1].keep >= w[0].keep);
    }
    // Easy: no tricks at all.
    let easy = Style::of(Skill::Easy);
    assert_eq!((easy.juke, easy.cover), (0.0, false));
}

/// The strafe side each tick, for `ticks` ticks at range 5 (inside the
/// default range: no closing in or backing off).
fn sides(skill: Skill, seed: u64, ticks: u32) -> Vec<f32> {
    let map = open();
    let mut m = Moves::new(seed);
    (0..ticks)
        .map(|t| {
            // It moves as it walks, so it is never stuck.
            let at = [0.0, (t % 7) as f32 * 0.2, 0.0];
            let i = input(skill, t, at, [5.0, at[1], 0.0]);
            m.fight(&i, Some(&map), &[]).walk[1]
        })
        .collect()
}

fn changes(v: &[f32]) -> usize {
    v.windows(2)
        .filter(|w| (w[0].signum() != w[1].signum()) || ((w[0] == 0.0) != (w[1] == 0.0)))
        .count()
}

#[test]
fn legendary_changes_direction_often_and_easy_is_predictable() {
    let easy = sides(Skill::Easy, 9, 3000);
    let legendary = sides(Skill::Legendary, 9, 3000);
    assert!(
        2 * changes(&legendary) > 3 * changes(&easy),
        "legendary {} easy {}",
        changes(&legendary),
        changes(&easy)
    );
    // Easy stands still now and then; Legendary never does.
    assert!(easy.iter().any(|&y| y.abs() < 1e-3));
    assert!(legendary.iter().all(|&y| y.abs() > 0.5));
    // The same seed is the same dance.
    let a = sides(Skill::Heroic, 4, 300);
    assert_eq!(a, sides(Skill::Heroic, 4, 300));
    assert_ne!(a, sides(Skill::Heroic, 5, 300));
    assert!(legendary.iter().any(|&y| (0.6..0.8).contains(&y.abs())));
    assert!(legendary.iter().any(|&y| (0.8..1.0).contains(&y.abs())));
}

#[test]
fn it_closes_in_from_afar_and_backs_off_up_close() {
    let map = open();
    for skill in [Skill::Normal, Skill::Legendary] {
        let mut m = Moves::new(1);
        let far = m.fight(
            &input(skill, 0, [0.0; 3], [20.0, 0.0, 0.0]),
            Some(&map),
            &[],
        );
        assert!(toward(&far) > 0.5, "{skill:?} {far:?}");
        let near = m.fight(&input(skill, 1, [0.0; 3], [1.5, 0.0, 0.0]), Some(&map), &[]);
        assert!(toward(&near) < -0.3, "{skill:?} {near:?}");
    }
    // Easy closes in but never backs off.
    let mut m = Moves::new(1);
    let near = m.fight(
        &input(Skill::Easy, 1, [0.0; 3], [1.5, 0.0, 0.0]),
        Some(&map),
        &[],
    );
    assert!(toward(&near).abs() < 1e-3, "{near:?}");
}

#[test]
fn it_keeps_the_range_it_is_given() {
    let map = open();
    let mut m = Moves::new(1);
    // At 6: inside a long gun's range (10 to 20), it backs off; with the
    // default (2.5 to 8) it holds.
    let long = FightIn {
        range: (10.0, 20.0),
        ..input(Skill::Heroic, 0, [0.0; 3], [6.0, 0.0, 0.0])
    };
    assert!(toward(&m.fight(&long, Some(&map), &[])) < -0.3);
    let default = input(Skill::Heroic, 1, [0.0; 3], [6.0, 0.0, 0.0]);
    assert!(toward(&m.fight(&default, Some(&map), &[])).abs() < 1e-3);
    // A shotgun's (0 to 3): at 6 it closes in.
    let short = FightIn {
        range: (0.0, 3.0),
        ..input(Skill::Heroic, 2, [0.0; 3], [6.0, 0.0, 0.0])
    };
    assert!(toward(&m.fight(&short, Some(&map), &[])) > 0.5);
    assert_eq!(preferred_range(&Me::default()), DEFAULT_RANGE);
}

/// Hit every 25 ticks for `ticks`: the hit reactions and the ticks jump
/// was held.
fn shot_at(skill: Skill, ticks: u32) -> (Moves, usize) {
    let map = open();
    let mut m = Moves::new(5);
    let mut jumps = 0;
    for t in 0..ticks {
        let shield = 1.0 - (t / 25) as f32 * 0.01 - if t % 25 == 0 { 0.1 } else { 0.0 };
        let at = [0.0, (t % 7) as f32 * 0.2, 0.0];
        let i = FightIn {
            shield,
            ..input(skill, t, at, [5.0, at[1], 0.0])
        };
        if m.fight(&i, Some(&map), &[]).jump {
            jumps += 1;
        }
    }
    (m, jumps)
}

#[test]
fn shots_can_make_a_bot_juke_but_do_not_make_it_jump() {
    let (legendary, jumps) = shot_at(Skill::Legendary, 1200);
    assert!(legendary.jukes >= 5, "{}", legendary.jukes);
    assert_eq!(jumps, 0);
    let (easy, jumps) = shot_at(Skill::Easy, 1200);
    assert_eq!((easy.jukes, jumps), (0, 0));
}

#[test]
fn a_hit_at_range_can_crouch_a_heroic_bot() {
    let map = open();
    let mut crouched = false;
    for seed in 0..200 {
        let mut m = Moves::new(seed);
        for t in 0..60u32 {
            let shield = if t < 30 { 1.0 } else { 0.5 };
            let at = [0.0, (t % 7) as f32 * 0.2, 0.0];
            let i = FightIn {
                shield,
                ..input(Skill::Heroic, t, at, [15.0, at[1], 0.0])
            };
            let mv = m.fight(&i, Some(&map), &[]);
            assert!(!(mv.crouch && mv.jump));
            crouched |= mv.crouch;
        }
    }
    assert!(crouched);
}

#[test]
fn on_a_bridge_it_never_strafes_off_the_edge() {
    let map = bridge();
    assert!(!standable(&map, [0.0, 0.0, 2.0], [0.0, 1.0]));
    assert!(!standable(&map, [0.0, 0.0, 2.0], [0.0, -1.0]));
    assert!(standable(&map, [0.0, 0.0, 2.0], [1.0, 0.0]));
    for skill in Skill::ALL {
        let mut m = Moves::new(3);
        for t in 0..300u32 {
            let at = [(t % 5) as f32 * 0.2, 0.0, 2.0];
            let mv = m.fight(&input(skill, t, at, [8.0, 0.0, 2.0]), Some(&map), &[]);
            assert!(mv.walk[1].abs() < 0.05, "{skill:?} tick {t}: {mv:?}");
        }
    }
}

#[test]
fn a_wall_beside_it_turns_the_strafe_the_other_way() {
    let map = MapData {
        walls: wall([-10.0, 0.6], [10.0, 0.6]),
        ..open()
    };
    assert!(!standable(&map, [0.0; 3], [0.0, 1.0]));
    assert!(standable(&map, [0.0; 3], [0.0, -1.0]));
    let mut m = Moves::new(11);
    let mut right = 0;
    for t in 0..300u32 {
        let at = [(t % 5) as f32 * 0.2, 0.0, 0.0];
        let mv = m.fight(
            &input(Skill::Legendary, t, at, [6.0, 0.0, 0.0]),
            Some(&map),
            &[],
        );
        assert!(mv.walk[1] <= 1e-3, "into the wall at tick {t}: {mv:?}");
        if mv.walk[1] < -0.5 {
            right += 1;
        }
    }
    assert!(right > 250, "{right}");
}

/// A wall north-east of the bot at the origin, the enemy at (8, 0): a
/// spot north of the bot is out of the enemy's sight.
fn pillar() -> MapData {
    MapData {
        walls: wall([1.0, 1.2], [1.0, 4.0]),
        ..open()
    }
}

#[test]
fn shields_down_it_backs_off_to_where_the_enemy_cannot_see() {
    let map = pillar();
    let foe = [8.0, 0.0, 0.0];
    let c = cover(&map, [0.0; 3], foe).expect("a spot");
    assert!(
        (c[0]).abs() < 1e-3 && (c[1] - 1.5).abs() < 1e-3,
        "the nearest hidden spot: {c:?}"
    );
    assert!(!map.walls.clear([8.0, 0.0, EYE], [c[0], c[1], CHEST]));
    // Normal and better go there; Easy backs straight off.
    let retreat = |skill| FightIn {
        retreat: true,
        ..input(skill, 0, [0.0; 3], foe)
    };
    let mut m = Moves::new(2);
    let mv = m.fight(&retreat(Skill::Normal), Some(&map), &[]);
    assert!(mv.walk[1] > 0.9, "{mv:?}");
    let mut m = Moves::new(2);
    let mv = m.fight(&retreat(Skill::Easy), Some(&map), &[]);
    assert!(toward(&mv) < -0.5, "{mv:?}");
    // In the open: no cover, straight back (and a strafe).
    let mut m = Moves::new(2);
    assert_eq!(cover(&open(), [0.0; 3], foe), None);
    let mv = m.fight(&retreat(Skill::Legendary), Some(&open()), &[]);
    assert!(toward(&mv) < -0.5, "{mv:?}");
}

#[test]
fn reloading_it_keeps_moving_and_backs_off() {
    let map = open();
    let mut m = Moves::new(8);
    for t in 0..120u32 {
        let at = [0.0, (t % 7) as f32 * 0.2, 0.0];
        let i = FightIn {
            reloading: true,
            ..input(Skill::Normal, t, at, [5.0, at[1], 0.0])
        };
        let mv = m.fight(&i, Some(&map), &[]);
        assert!(toward(&mv) < -0.3, "tick {t}: {mv:?}");
    }
}

#[test]
fn pushing_without_moving_in_a_fight_flips_and_jumps() {
    let map = open();
    let mut m = Moves::new(6);
    let mut log = Vec::new();
    for t in 0..(FIGHT_STUCK + 2) {
        let mv = m.fight(
            &input(Skill::Legendary, t, [0.0; 3], [5.0, 0.0, 0.0]),
            Some(&map),
            &[],
        );
        log.push(mv);
    }
    let stuck = log[FIGHT_STUCK as usize];
    let last = log.last().unwrap();
    assert!(log[..FIGHT_STUCK as usize].iter().all(|m| !m.jump));
    assert!(stuck.jump, "{log:?}");
    // The other way from the next tick.
    assert!(stuck.walk[1].abs() > 0.5, "{log:?}");
    assert_eq!(last.walk[1].signum(), -stuck.walk[1].signum(), "{log:?}");
}

#[test]
fn rushing_runs_straight_in_and_without_a_map_it_walks_straight() {
    let mut m = Moves::new(1);
    let rush = FightIn {
        rush: true,
        ..input(Skill::Legendary, 0, [0.0; 3], [5.0, 0.0, 0.0])
    };
    let mv = m.fight(&rush, Some(&open()), &[]);
    assert!((mv.walk[0] - 1.0).abs() < 1e-4 && mv.walk[1].abs() < 1e-4);
    let mv = m.fight(
        &input(Skill::Normal, 1, [0.0; 3], [5.0, 0.0, 0.0]),
        None,
        &[],
    );
    assert_eq!(mv.walk, [1.0, 0.0]);
    let back = FightIn {
        retreat: true,
        ..input(Skill::Normal, 2, [0.0; 3], [5.0, 0.0, 0.0])
    };
    assert_eq!(m.fight(&back, None, &[]).walk, [-1.0, 0.0]);
}
