//! The stepped runs of docs/game/notes/2026-11-23-why-bounces-work.md
//! ("Tick by tick"): each case starts on a trace's row 0 (origin and
//! velocity copied from the stepped game) with no stick, as the harness
//! flew it, over two floor polygons built round the site's edge. The
//! polygons are not the map's: each is a plane through the site's point
//! with the slope the trace's contacts show, cut at the edge, placed so
//! the crossing sits about where the trace's contact depths put it.

use super::*;

/// A quad on the plane through `q` with normal `n`: from `q − along` to
/// `q + along`, out `away` (in the plane) from that edge.
fn quad(q: Vec3, along: Vec3, away: Vec3, n: Vec3) -> Poly {
    let l = norm(n);
    Poly {
        verts: vec![
            sub(q, along),
            add(q, along),
            add(add(q, along), away),
            add(sub(q, along), away),
        ],
        normal: scale(n, 1.0 / l),
    }
}

/// Two coplanar quads either side of a seam through `q` along `along`;
/// `away` is one side's in-plane direction, unit.
fn seam(q: Vec3, along: Vec3, away: Vec3, n: Vec3) -> Vec<Poly> {
    vec![
        quad(q, along, away, n),
        quad(q, along, scale(away, -1.0), n),
    ]
}

/// A run: the origin and velocity on its row 0.
struct Run {
    pos: Vec3,
    vel: Vec3,
}

fn verdict(r: &Run, polys: &[Poly]) -> Verdict {
    predict(r.pos, r.vel, [0.0; 2], polys)
}

/// `s007-slot`: a seam across a 26.6° ramp along X. Asked: `h` 0.107,
/// 5.225 down, drift 0.58.
fn s007() -> Vec<Poly> {
    seam(
        [3.0, 10.8505, -1.93668],
        [1.0, 0.0, 0.0],
        [0.0, -0.894_427_2, 0.447_213_6],
        [0.0, 0.447_213_6, 0.894_427_2],
    )
}

#[test]
fn s007_slot_bounces_on_the_tick_the_sweep_comes_on() {
    let polys = s007();
    let pos = [2.998_907_8, 10.803_658, -1.818_667_6];
    let bounce = verdict(
        &Run {
            pos,
            vel: [0.048_880_547, -0.581_668_1, -5.224_673],
        },
        &polys,
    );
    assert_eq!(
        bounce.outcome,
        Outcome::Bounce(Route::Threshold),
        "{bounce:?}"
    );
    // Row 1's tick: the trace's launch is on row 2.
    assert_eq!(bounce.tick, 1);
    assert!((bounce.row[2] - -1.997_458).abs() < 1e-3, "{bounce:?}");
    let p = bounce.pierce.unwrap();
    assert!(p.edge < EDGE_SCALE * p.depth && p.across > 0.4, "{p:?}");
    // The bounce moves at 5.39 on row 0's tick, under the threshold, and
    // 5.53 on row 1's.
    assert!(
        bounce.speed > SWEEP_SPEED && bounce.speed < 5.55,
        "{bounce:?}"
    );

    // 0.2 faster: the sweep is on row 0's tick and the capsule is caught.
    let faster = verdict(
        &Run {
            pos,
            vel: [0.048_880_547, -0.581_668_1, -5.424_673],
        },
        &polys,
    );
    assert_eq!(
        (faster.outcome, faster.tick),
        (Outcome::Caught, 0),
        "{faster:?}"
    );
    // 0.2 slower: no sweep on row 1's tick, the body already in the floor.
    let slower = verdict(
        &Run {
            pos,
            vel: [0.048_880_547, -0.581_668_1, -5.024_673],
        },
        &polys,
    );
    assert_eq!(
        (slower.outcome, slower.tick),
        (Outcome::Stopped, 1),
        "{slower:?}"
    );
    // `h` 0.030: inside the tolerance on row 0's tick.
    let low = verdict(
        &Run {
            pos: [2.998_907_8, 10.803_658, -1.895_473_2],
            vel: [0.048_880_547, -0.581_668_1, -5.224_673],
        },
        &polys,
    );
    assert_eq!((low.outcome, low.tick), (Outcome::Landing, 0), "{low:?}");
    // No drift: the same pinch, level.
    let still = verdict(
        &Run {
            pos: [2.999_005_6, 10.802_492, -1.818_667_6],
            vel: [0.0, 0.0, -5.224_673],
        },
        &polys,
    );
    assert_eq!(
        (still.outcome, still.tick),
        (Outcome::LevelPinch, 1),
        "{still:?}"
    );
}

/// `s001-slot`: a seam along Y on a 23.4° ramp. Asked: `h` 0.127, 5.273
/// down, drift 0.39.
fn s001() -> Vec<Poly> {
    seam(
        [-15.96119, -3.2123, -1.940_975],
        [0.0, 1.0, 0.0],
        [-0.917_921_3, 0.0, 0.396_762_2],
        [0.396_762_2, 0.0, 0.917_921_3],
    )
}

#[test]
fn s001_slot_splits_at_the_same_branch() {
    let polys = s001();
    let pos = [-15.999_511, -3.212_530_6, -1.818_739_4];
    let drift = [-0.391_891_3, 0.082_464_12];
    let run = |pos: Vec3, vz: f32, d: [f32; 2]| {
        verdict(
            &Run {
                pos,
                vel: [d[0], d[1], vz],
            },
            &polys,
        )
    };
    let bounce = run(pos, -5.272_52, drift);
    assert_eq!(
        (bounce.outcome, bounce.tick),
        (Outcome::Bounce(Route::Threshold), 1),
        "{bounce:?}"
    );
    assert_eq!(run(pos, -5.472_52, drift).outcome, Outcome::Caught);
    assert_eq!(run(pos, -5.072_52, drift).outcome, Outcome::Stopped);
    assert_eq!(
        run([-15.999_511, -3.212_530_6, -1.915_931], -5.272_52, drift).outcome,
        Outcome::Landing
    );
    assert_eq!(
        run(
            [-16.000_652, -3.212_290_5, -1.818_739_4],
            -5.272_52,
            [0.0; 2]
        )
        .outcome,
        Outcome::LevelPinch
    );
}

/// `s020-crease`: a level floor meeting a 20° ramp that rises away from
/// it, the crease square to (0.857, 0.515), the trace's level normal.
/// Asked: `h` 0.124, 5.176 down, drift 0.44.
fn s020() -> Vec<Poly> {
    let m = [0.857_487_7, 0.514_504, 0.0];
    let q = [2.70798, 5.53544, 1.037_786];
    let along = [-0.514_504, 0.857_487_7, 0.0];
    vec![
        quad(q, along, m, [0.0, 0.0, 1.0]),
        quad(
            q,
            along,
            [-0.805_775, -0.483_475, 0.342_02],
            [0.293_278, 0.175_971, 0.939_693],
        ),
    ]
}

#[test]
fn s020_crease_splits_at_the_same_branch() {
    let polys = s020();
    let pos = [2.750_016_5, 5.564_64, 1.161_748_4];
    let drift = [-0.339_960_55, -0.286_869_9];
    let run = |pos: Vec3, vz: f32, d: [f32; 2]| {
        verdict(
            &Run {
                pos,
                vel: [d[0], d[1], vz],
            },
            &polys,
        )
    };
    let bounce = run(pos, -5.176_481, drift);
    assert_eq!(
        (bounce.outcome, bounce.tick),
        (Outcome::Bounce(Route::Threshold), 1),
        "{bounce:?}"
    );
    assert_eq!(bounce.pierce.unwrap().poly, 0);
    assert_eq!(run(pos, -5.376_481, drift).outcome, Outcome::Caught);
    assert_eq!(run(pos, -4.976_481, drift).outcome, Outcome::Stopped);
    assert_eq!(
        run([2.750_016_5, 5.564_64, 1.067_786_2], -5.176_481, drift).outcome,
        Outcome::Landing
    );
    assert_eq!(
        run([2.749_122_6, 5.563_886, 1.161_748_4], -5.176_481, [0.0; 2]).outcome,
        Outcome::LevelPinch
    );
}

/// `s025-vee`: a V of two 40° floors meeting at a groove along (0.973,
/// −0.230). Row 0 starts with the capsule already inside the floor.
/// Asked: `h` 0.042, 5.354 down, drift 0.50.
fn s025() -> Vec<Poly> {
    let q = [-4.444_123, 4.245_937, 0.324_991];
    let along = [0.4866, -0.11475, 0.0];
    vec![
        quad(
            q,
            along,
            [0.105_486, 0.447_354, 0.385_68],
            [-0.147_52, -0.625_64, 0.766_04],
        ),
        quad(
            q,
            along,
            [-0.105_486, -0.447_354, 0.385_674],
            [0.147_52, 0.625_64, 0.766_04],
        ),
    ]
}

#[test]
fn s025_vee_bounces_on_its_first_tick() {
    let polys = s025();
    let pos = [-4.437_058_4, 4.254_579, 0.298_949_06];
    let drift = [-0.128_278_45, 0.480_663_87];
    let run = |pos: Vec3, vz: f32, d: [f32; 2]| {
        verdict(
            &Run {
                pos,
                vel: [d[0], d[1], vz],
            },
            &polys,
        )
    };
    let bounce = run(pos, -5.353_851, drift);
    assert_eq!(
        (bounce.outcome, bounce.tick),
        (Outcome::Bounce(Route::Threshold), 0),
        "{bounce:?}"
    );
    // 1.2 cm lower bounces too (the trace's `h` 0.030 run: +12.30).
    assert_eq!(
        run([-4.437_058_4, 4.254_579, 0.286_895_1], -5.353_851, drift).outcome,
        Outcome::Bounce(Route::Threshold)
    );
    // Slower: no sweep, and the body is already in the floor.
    assert_eq!(run(pos, -5.153_851, drift).outcome, Outcome::Stopped);
    assert_eq!(
        run([-4.438_52, 4.255_822_7, 0.298_949_06], -5.353_851, [0.0; 2]).outcome,
        Outcome::LevelPinch
    );
}

/// A level floor at Z 0 cut along Y at `x`.
fn cut_at(x: f32) -> Vec<Poly> {
    let up = [0.0, 0.0, 1.0];
    vec![
        quad([x, 0.0, 0.0], [0.0, 1.0, 0.0], [-1.5, 0.0, 0.0], up),
        quad([x, 0.0, 0.0], [0.0, 1.0, 0.0], [1.5, 0.0, 0.0], up),
    ]
}

/// The fast-fall rows: 9 down, primed, near an edge. The note's table
/// steps 0.03 from the edge and gets +10.68, 0.20 and a pop, 0.20 over
/// the floor and caught.
#[test]
fn a_fast_fall_bounces_from_above_under_its_height() {
    let fall =
        |z: f32, edge: f32| predict([0.0, 0.0, z], [0.3, 0.0, -9.0], [0.3, 0.0], &cut_at(edge));
    let near = fall(0.06, 0.04);
    assert_eq!(
        (near.outcome, near.tick),
        (Outcome::Bounce(Route::Fast), 0),
        "{near:?}"
    );
    assert_eq!(fall(0.06, 0.21).outcome, Outcome::Pop);
    assert_eq!(fall(0.20, 0.04).outcome, Outcome::Caught);
}

/// The drop table: from the apex, 3.45 down bounces on most phases a
/// couple of centimetres from the edge, 3.0 never reaches the sweep and
/// 4.5 has it on too high.
#[test]
fn the_drop_from_the_apex_has_a_window() {
    // A full drift of 0.3 across the edge, held; the body crosses about
    // 0.4 in the fall, so the edge at 0.42 is 2 to 3 cm off the crossing.
    let polys = cut_at(0.42);
    let bounces = |drop: f32| {
        (0..20)
            .filter(|i| {
                let phase = (*i as f32 + 0.5) / 20.0;
                let v = [0.3, 0.0, -phase * GRAVITY * TICK];
                predict([0.0, 0.0, drop], v, [0.3, 0.0], &polys)
                    .outcome
                    .bounces()
            })
            .count()
    };
    assert!(bounces(DROP_BEST) >= 8, "{}", bounces(DROP_BEST));
    assert_eq!(bounces(3.0), 0);
    assert_eq!(bounces(4.5), 0);
    assert!(drop_in_window(DROP_BEST) && !drop_in_window(3.0) && !drop_in_window(3.7));
}

/// A body on the floor jumping up does not land on its own takeoff.
#[test]
fn a_jump_comes_down_before_it_is_judged() {
    let polys = cut_at(0.0);
    let v = predict([0.0, 0.0, 0.01], [0.0, 0.0, 3.08], [0.0; 2], &polys);
    assert!(
        matches!(v.outcome, Outcome::Landing | Outcome::Stopped),
        "{v:?}"
    );
    assert!(v.tick > 30, "{v:?}");
    let none = predict([0.0, 0.0, 1.0], [0.0, 0.0, -1.0], [0.0; 2], &[]);
    assert_eq!(none.outcome, Outcome::NoFloor);
}

#[test]
fn the_edge_under_a_point_is_the_nearest_of_its_floor() {
    let polys = cut_at(0.42);
    let e = edge_under(&polys, [0.40, 0.3, 1.0]).unwrap();
    assert!((e.distance - 0.02).abs() < 1e-5, "{e:?}");
    assert!((e.across[0].abs() - 1.0).abs() < 1e-5, "{e:?}");
    assert_eq!(e.poly, 0);
    // A floor over the point is not under it.
    assert_eq!(edge_under(&polys, [0.40, 0.3, -1.0]), None);
    assert_eq!(floors_near(&polys, [10.0, 0.0], 1.0).len(), 0);
    assert_eq!(floors_near(&polys, [0.40, 0.0], 0.1).len(), 2);
}

/// The launch rule against the traced launches (the notes' "The sideways
/// normal" and "Faster falls bounce from above").
#[test]
fn launch_fits_the_traces() {
    let near = |a: f32, b: f32, tol: f32| assert!((a - b).abs() <= tol, "{a} vs {b}");
    // By drift at 5.39 in.
    near(launch(5.39, 0.10), 0.07, 0.1);
    near(launch(5.39, 0.15), 6.2, 0.3);
    near(launch(5.39, 0.20), 9.4, 0.3);
    near(launch(5.39, 0.30), 13.0, 0.3);
    near(launch(5.39, 0.60), 12.6, 0.5);
    // By fall speed at a full drift.
    near(launch(9.0, 0.30), 10.68, 0.3);
    near(launch(13.0, 0.30), 8.75, 0.3);
}

#[test]
fn no_drift_no_launch() {
    assert_eq!(launch(5.4, 0.09), 0.0);
    assert_eq!(launch(5.4, 0.0), 0.0);
    assert_eq!(launch(f32::NAN, 0.3), 0.0);
}

/// The launch falls as the fall speed rises, and rises with the drift up
/// to a full lean.
#[test]
fn launch_is_monotonic() {
    let mut last = f32::INFINITY;
    for s in [5.4, 7.0, 9.0, 11.0, 13.0, 16.0] {
        let l = launch(s, 0.3);
        assert!(l <= last, "{s}: {l} over {last}");
        last = l;
    }
    let mut last = 0.0;
    for d in [0.1, 0.12, 0.15, 0.2, 0.25, 0.3] {
        let l = launch(5.4, d);
        assert!(l >= last, "{d}: {l} under {last}");
        last = l;
    }
}
