use super::*;
use crate::arms::{Ammo, Gun};
use crate::nav::Poly;

fn quad(x0: f32, y0: f32, x1: f32, y1: f32) -> Poly {
    Poly::new(vec![
        [x0, y0, 0.0],
        [x1, y0, 0.0],
        [x1, y1, 0.0],
        [x0, y1, 0.0],
    ])
}

/// A 4x4 grid of 10 unit squares, 0 to 40 in X and Y (links do not
/// matter here: the caller checks reachability).
fn grid() -> NavMesh {
    let mut polys = Vec::new();
    for i in 0..4 {
        for j in 0..4 {
            let (x, y) = (i as f32 * 10.0, j as f32 * 10.0);
            polys.push(quad(x, y, x + 10.0, y + 10.0));
        }
    }
    NavMesh::new(polys, vec![])
}

/// A loaded gun on the ground (fields set one by one, so it builds
/// whatever else `Gun` carries).
fn gun_at(id: u32, at: Vec3) -> Pickup {
    let mut p = Pickup {
        id,
        position: at,
        gun: Gun::default(),
    };
    p.gun.def = 0xa001_0001;
    p.gun.ammo = Ammo::Rounds {
        loaded: 10,
        capacity: 36,
        reserve: 50,
        reserve_max: 108,
    };
    p
}

const ME: Vec3 = [5.0, 5.0, 0.0];
const SIGHTING: Vec3 = [33.0, 36.0, 0.0];
/// Every goal is fine.
fn any(_: Vec3) -> bool {
    true
}
const ANY: &dyn Fn(Vec3) -> bool = &any;

#[test]
fn a_fresh_sighting_draws_a_good_bot() {
    let nav = grid();
    let mut r = Roam::new(1);
    r.saw(SIGHTING, 0);
    assert_eq!(
        r.pick(ME, 10, &nav, &[], &[], Skill::Legendary, ANY),
        Some(SIGHTING)
    );
}

#[test]
fn an_old_sighting_is_forgotten() {
    let nav = grid();
    let mut r = Roam::new(1);
    r.saw(SIGHTING, 0);
    let c = r.candidates(ME, SIGHT_TICKS + 5, &nav, &[], &[], Skill::Legendary);
    assert!(!c.is_empty());
    assert!(c.iter().all(|(_, p)| *p != SIGHTING), "{c:?}");
}

#[test]
fn easy_wanders_and_legendary_goes_where_the_fight_was() {
    let nav = grid();
    let hits = |skill| {
        (0..40u64)
            .filter(|&seed| {
                let mut r = Roam::new(seed);
                r.saw(SIGHTING, 0);
                r.pick(ME, 10, &nav, &[], &[], skill, ANY) == Some(SIGHTING)
            })
            .count()
    };
    assert_eq!(hits(Skill::Legendary), 40);
    let easy = hits(Skill::Easy);
    assert!(easy < 40, "{easy}");
    assert!(easy > 0, "{easy}");
}

#[test]
fn it_does_not_go_back_where_it_has_just_been() {
    // One polygon centred on the bot: its centre is too near to be worth
    // it, so the weapons decide.
    let nav = NavMesh::new(vec![quad(-50.0, -50.0, 50.0, 50.0)], vec![]);
    let (a, b) = ([20.0, 0.0, 0.0], [-20.0, 0.0, 0.0]);
    let guns = [gun_at(1, a), gun_at(2, b)];
    for seed in 0..10 {
        let mut r = Roam::new(seed);
        r.visit(a, 0);
        let at = [0.0; 3];
        assert_eq!(r.pick(at, 10, &nav, &guns, &[], Skill::Legendary, ANY), Some(b));
        let mut r = Roam::new(seed);
        r.visit(b, 0);
        assert_eq!(r.pick(at, 10, &nav, &guns, &[], Skill::Legendary, ANY), Some(a));
    }
}

#[test]
fn where_it_has_been_counts_less_as_time_passes() {
    let mut r = Roam::new(1);
    let (at, a) = ([0.0; 3], [20.0, 0.0, 0.0]);
    assert_eq!(r.penalty(at, a, 0), 0.0);
    r.visit(a, 0);
    let soon = r.penalty(at, a, 10);
    let later = r.penalty(at, a, VISIT_TICKS / 2);
    assert!(soon > later && later > 0.0, "{soon} {later}");
    assert_eq!(r.penalty(at, a, VISIT_TICKS + 1), 0.0);
    // Right beside it: barely worth the walk.
    assert_eq!(r.penalty(at, [1.0, 0.0, 0.0], 0), NEAR_PENALTY);
}

#[test]
fn a_goal_the_caller_refuses_is_passed_over() {
    let nav = grid();
    let mut r = Roam::new(1);
    r.saw(SIGHTING, 0);
    let g = r.pick(
        ME,
        10,
        &nav,
        &[],
        &[],
        Skill::Legendary,
        &|p| p != SIGHTING,
    );
    assert!(g.is_some_and(|g| g != SIGHTING), "{g:?}");
    assert_eq!(
        r.pick(ME, 10, &nav, &[], &[], Skill::Legendary, &|_| false),
        None
    );
}

#[test]
fn the_maps_starts_are_candidates_between_a_weapon_and_a_random_polygon() {
    // A big grid, nobody seen, nothing lying: the spawn points are where a
    // bot with no information goes looking, above a random polygon and
    // below a weapon worth taking.
    let nav = grid();
    let starts = [[35.0, 5.0, 0.0], [5.0, 35.0, 0.0]];
    let guns = [gun_at(1, [20.0, 20.0, 0.0])];
    for seed in 0..10u64 {
        let mut r = Roam::new(seed);
        let picked = r
            .pick(ME, 10, &nav, &guns, &starts, Skill::Legendary, ANY)
            .unwrap();
        assert!(picked == guns[0].position || starts.contains(&picked), "{picked:?}");
        let mut r = Roam::new(seed);
        let picked = r
            .pick(ME, 10, &nav, &[], &starts, Skill::Legendary, ANY)
            .unwrap();
        assert!(starts.contains(&picked), "a start over a random polygon: {picked:?}");
    }
    // Just been to one: the other start, not the same one again.
    let mut r = Roam::new(1);
    r.visit(starts[0], 10);
    assert_eq!(
        r.pick(ME, 10, &nav, &[], &starts, Skill::Legendary, ANY),
        Some(starts[1])
    );
}

#[test]
fn the_memory_is_bounded_and_merges_nearby_places() {
    let mut r = Roam::new(1);
    for i in 0..20u32 {
        r.saw([i as f32 * 10.0, 0.0, 0.0], i);
    }
    assert_eq!(r.sightings.len(), SIGHT_MAX);
    assert!(r.sightings.iter().any(|(p, _)| p[0] == 190.0));
    assert!(
        r.sightings.iter().all(|(p, _)| p[0] != 0.0),
        "the oldest went"
    );
    // Close to one already known: the same place, seen again.
    r.saw([191.0, 1.0, 0.0], 30);
    assert_eq!(r.sightings.len(), SIGHT_MAX);
    assert!(r.sightings.contains(&([191.0, 1.0, 0.0], 30)));
    for i in 0..30u32 {
        r.visit([i as f32 * 10.0, 0.0, 0.0], i);
    }
    assert_eq!(r.visited.len(), VISIT_MAX);
}
