//! The bounce search over fakes: the samplers, the refinement, the
//! region and its window, the ranking and the report's formats, one trial
//! row by row, and the whole job against a small made-up machine whose
//! fork is a copy of its state.

use std::collections::BTreeMap;

use super::region::{self, Class};
use super::report::Verdict;
use super::sample::{self, Axis, Rng};
use super::*;
use crate::drops::{FALL_GRAVITY, Outcome, PRIMED, PairSense, SeatSense, Sense, Target, TargetSpec};

fn axes() -> [Axis; N] {
    let mut out = [Axis::default(); N];
    for (a, d) in DOMAIN.iter().enumerate() {
        out[a] = Axis {
            lo: d[0],
            hi: d[1],
            step: d[2],
        };
    }
    out
}

// ---- the samplers ----

#[test]
fn a_latin_hypercube_puts_one_point_in_every_stratum_of_every_axis() {
    let mut ax = axes();
    // No grid, so a value's stratum is exact; one axis held.
    for a in ax.iter_mut() {
        a.step = 0.0;
    }
    ax[YAW] = Axis {
        lo: 12.0,
        hi: 12.0,
        step: 0.0,
    };
    let n = 50;
    let points = sample::lhs(&ax, n, &mut Rng::new(7));
    assert_eq!(points.len(), n);
    for a in 0..N {
        if ax[a].fixed() {
            assert!(points.iter().all(|p| p[a] == 12.0));
            continue;
        }
        let mut seen = vec![0u32; n];
        for p in &points {
            assert!(p[a] >= ax[a].lo && p[a] <= ax[a].hi, "{} {}", AXES[a], p[a]);
            let u = (p[a] - ax[a].lo) / ax[a].span();
            seen[((u * n as f32) as usize).min(n - 1)] += 1;
        }
        // A value on a stratum's edge may round into its neighbour.
        assert!(
            seen.iter().filter(|&&c| c == 1).count() >= n - 4,
            "{}: {seen:?}",
            AXES[a]
        );
    }
}

#[test]
fn the_samplers_are_the_same_for_a_seed_and_differ_between_sites() {
    let ax = axes();
    let a = sample::lhs(&ax, 30, &mut Rng::new(sample::seed_for(1, "s000-slot")));
    let b = sample::lhs(&ax, 30, &mut Rng::new(sample::seed_for(1, "s000-slot")));
    let c = sample::lhs(&ax, 30, &mut Rng::new(sample::seed_for(1, "s001-slot")));
    let d = sample::lhs(&ax, 30, &mut Rng::new(sample::seed_for(2, "s000-slot")));
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_ne!(a, d);
}

#[test]
fn positions_are_sampled_on_their_grid() {
    let ax = axes();
    for p in sample::lhs(&ax, 200, &mut Rng::new(3)) {
        for a in [ACROSS, ALONG, DEPTH] {
            let steps = (p[a] - ax[a].lo) / ax[a].step;
            assert!((steps - steps.round()).abs() < 1e-2, "{} {}", AXES[a], p[a]);
        }
    }
    let axis = Axis {
        lo: -0.1,
        hi: 0.1,
        step: 0.05,
    };
    assert!((axis.snap(0.02) - 0.0).abs() < 1e-6);
    assert!((axis.snap(0.03) - 0.05).abs() < 1e-6);
    assert_eq!(axis.snap(7.0), 0.1);
}

#[test]
fn a_grid_has_every_combination_with_the_ends() {
    let mut ax = axes();
    for a in ax.iter_mut() {
        *a = Axis {
            lo: a.lo,
            hi: a.lo,
            step: 0.0,
        };
    }
    ax[ACROSS] = Axis {
        lo: -0.1,
        hi: 0.1,
        step: 0.0,
    };
    ax[V_DRIFT] = Axis {
        lo: 0.0,
        hi: 0.8,
        step: 0.0,
    };
    let mut counts = [1u32; N];
    counts[ACROSS] = 5;
    counts[V_DRIFT] = 3;
    let g = sample::grid(&ax, &counts).unwrap();
    assert_eq!(g.len(), 15);
    assert_eq!(g[0][ACROSS], -0.1);
    assert_eq!(g[0][V_DRIFT], 0.0);
    assert_eq!(g[1][V_DRIFT], 0.4);
    assert_eq!(g[14][ACROSS], 0.1);
    assert_eq!(g[14][V_DRIFT], 0.8);
    // One value: the middle.
    counts[ACROSS] = 1;
    assert!(sample::grid(&ax, &counts).unwrap()[0][ACROSS].abs() < 1e-6);
    // Too many points are refused by name.
    let wide = axes();
    let e = sample::grid(&wide, &[100; N]).unwrap_err();
    assert!(e.contains("at most"), "{e}");
}

// ---- the refinement ----

/// Points on the across axis alone, the rest of the vector 0.
fn across(values: &[f32]) -> Vec<Vector> {
    values
        .iter()
        .map(|&v| {
            let mut p = [0.0; N];
            p[ACROSS] = v;
            // Inside the default ranges, as every other axis's 0 is.
            p[VZ] = 5.4;
            p
        })
        .collect()
}

#[test]
fn refinement_halves_the_widest_gaps_between_opposite_outcomes() {
    let mut ax = axes();
    ax[ACROSS].step = 0.0;
    // Bounces under 0: the edge is between -0.02 and 0.04. Every other
    // nearest pair of opposite outcomes has a point between its two.
    let points = across(&[-0.09, -0.05, -0.02, 0.04, 0.07]);
    let labels = [Some(true), Some(true), Some(true), Some(false), Some(false)];
    let new = sample::refine(&points, &labels, &ax, &HUMAN, 10, MIN_GAP);
    assert_eq!(new.len(), 1, "{new:?}");
    assert!((new[0][ACROSS] - 0.01).abs() < 1e-6, "{:?}", new[0]);
    // Every new point lies between a bounce and a failure.
    for p in &new {
        assert!(p[ACROSS] > -0.09 && p[ACROSS] < 0.07);
    }
    // The budget holds, and the widest pair comes first.
    let one = sample::refine(&points, &labels, &ax, &HUMAN, 1, MIN_GAP);
    assert_eq!(one.len(), 1);
    assert_eq!(one[0], new[0]);
    // A second round with the new point's outcome narrows the edge again.
    let mut points2 = points.clone();
    points2.push(new[0]);
    let mut labels2 = labels.to_vec();
    labels2.push(Some(false));
    let next = sample::refine(&points2, &labels2, &ax, &HUMAN, 1, MIN_GAP);
    assert!((next[0][ACROSS] - (-0.005)).abs() < 1e-6, "{:?}", next[0]);
}

#[test]
fn refinement_needs_both_outcomes_and_a_gap() {
    let mut ax = axes();
    // No grid: a midpoint is exactly its pair's mean.
    ax[ACROSS].step = 0.0;
    let points = across(&[-0.05, 0.0, 0.05]);
    // All one outcome, or none known: nothing to refine.
    assert!(sample::refine(&points, &[Some(true); 3], &ax, &HUMAN, 10, MIN_GAP).is_empty());
    assert!(sample::refine(&points, &[Some(false); 3], &ax, &HUMAN, 10, MIN_GAP).is_empty());
    assert!(sample::refine(&points, &[None, Some(true), None], &ax, &HUMAN, 10, MIN_GAP).is_empty());
    // A pair already nearer than the gap is left alone: 0.0004 is 0.04
    // human steps.
    let close = across(&[0.0, 0.0004]);
    assert!(sample::refine(&close, &[Some(true), Some(false)], &ax, &HUMAN, 10, MIN_GAP).is_empty());
    // A point with no outcome takes no part.
    let points = across(&[-0.05, 0.0, 0.05]);
    let new = sample::refine(&points, &[Some(true), None, Some(false)], &ax, &HUMAN, 10, MIN_GAP);
    assert_eq!(new.len(), 0, "the midpoint 0.0 is already a point");
}

// ---- the region ----

#[test]
fn a_points_class_is_over_every_variant() {
    use Outcome::*;
    assert_eq!(region::class([Some(Bounced), Some(Bounced)]), Class::All);
    assert_eq!(region::class([Some(Bounced), Some(NoBounce)]), Class::Some);
    // No outcome on a variant is not a proof.
    assert_eq!(region::class([Some(Bounced), None]), Class::Some);
    assert_eq!(region::class([Some(Bounced), Some(PrimeLost)]), Class::Some);
    assert_eq!(region::class([Some(NoBounce), Some(PrimeLost)]), Class::None);
    assert_eq!(region::class([Some(PrimeLost), Some(SetupFailed), None]), Class::Unknown);
    assert_eq!(region::class([]), Class::Unknown);
}

/// A grid of points over across and depth, bouncing inside a box.
fn boxed() -> Vec<(Vector, Class)> {
    let mut out = Vec::new();
    for i in 0..=40 {
        for j in 0..=20 {
            let mut p = [0.0; N];
            p[ACROSS] = -0.10 + 0.005 * i as f32;
            p[DEPTH] = -0.05 + 0.005 * j as f32;
            let inside = p[ACROSS] > -0.0625 && p[ACROSS] < -0.0075 && p[DEPTH] < 0.0125;
            out.push((p, if inside { Class::All } else { Class::None }));
        }
    }
    out
}

#[test]
fn the_window_is_the_largest_failure_free_cube_in_human_steps() {
    let points = boxed();
    let w = region::window(&points, &HUMAN, 8).expect("a window");
    // The box is 0.055 across (-0.06 to -0.01 hold bounces; the first
    // failures are at -0.065 and -0.005) and its depth runs from the
    // sampled floor (-0.05) to 0.01. The cube that fits is 3 steps of
    // 0.01 either way, centred across the box.
    assert!((w.radius - 3.0).abs() < 0.26, "{w:?}");
    assert!(w.centre[ACROSS] > -0.045 && w.centre[ACROSS] < -0.025, "{w:?}");
    assert!(w.support >= 8);
    // Each axis's own range reaches the first failure either way.
    assert!((w.bounds[ACROSS][0] - -0.065).abs() < 1e-4, "{w:?}");
    assert!((w.bounds[ACROSS][1] - -0.005).abs() < 1e-4, "{w:?}");
    assert!((w.bounds[DEPTH][1] - 0.015).abs() < 1e-4, "{w:?}");
    // An axis nobody sampled has no range.
    assert_eq!(w.bounds[YAW], [0.0, 0.0]);
    // The extent is the bounces' own.
    let e = region::extent(&points).unwrap();
    assert!((e[ACROSS][0] - -0.06).abs() < 1e-4 && (e[ACROSS][1] - -0.01).abs() < 1e-4);
    // Depths at the aim: -0.05 to 0.01 bounce, 0.015 to 0.05 don't.
    let (share, n) = region::share_along(&points, &w, &HUMAN, DEPTH).unwrap();
    assert!(n > 20);
    assert!((share - 13.0 / 21.0).abs() < 0.02, "{share}");
}

#[test]
fn a_window_needs_support_and_no_failure_inside() {
    // One bounce among failures: no cube around it has 8 points.
    let mut points = boxed();
    for p in points.iter_mut() {
        p.1 = Class::None;
    }
    points[300].1 = Class::All;
    assert!(region::window(&points, &HUMAN, 8).is_none());
    assert!(region::window(&points, &HUMAN, 1).is_some());
    // A point that bounced on some variants only is a failure: it cuts
    // the window.
    let whole = region::window(&boxed(), &HUMAN, 8).unwrap();
    let mut cut = boxed();
    let mid = cut
        .iter()
        .position(|(p, _)| p == &whole.centre)
        .expect("the centre is a point");
    cut[mid].1 = Class::Some;
    let w = region::window(&cut, &HUMAN, 8).unwrap();
    assert!(w.radius < whole.radius, "{} {}", w.radius, whole.radius);
    // Points with no outcome are not failures and not support.
    let mut holes = boxed();
    for p in holes.iter_mut().filter(|p| p.1 == Class::None) {
        p.1 = Class::Unknown;
    }
    let w = region::window(&holes, &HUMAN, 8).unwrap();
    assert!(w.radius > 0.0);
    // Nothing bounced: no window, no extent.
    let none: Vec<(Vector, Class)> = boxed().into_iter().map(|(p, _)| (p, Class::None)).collect();
    assert!(region::window(&none, &HUMAN, 1).is_none());
    assert!(region::extent(&none).is_none());
}

// ---- the table ----

fn table(text: &str) -> BouncesConfig {
    toml::from_str(text).unwrap()
}

#[test]
fn the_table_has_defaults_and_names_its_problems() {
    let s = BouncesConfig::default().settings().unwrap();
    assert_eq!((s.points, s.rounds, s.refine), (POINTS, ROUNDS, REFINE));
    assert_eq!((s.lead, s.flight, s.seats), (LEAD, FLIGHT, 15));
    assert_eq!(s.human, HUMAN);
    assert_eq!(s.variants.len(), 3);
    assert_eq!(s.axes[ACROSS].step, 0.0005);
    // 60 sites at the defaults are over 100,000 trials.
    assert_eq!(s.trials_per_site(), 3 * (400 + 600 + 8 + 2) + 16);
    assert!(60 * s.trials_per_site() > 100_000);

    let t = table(
        r#"
points = 50
crouch = "both"
sample = "grid"
[axes]
across = [-0.08, 0.02, 0.001]
yaw = [0.0, 0.0]
[grid]
across = 11
depth = 5
[human]
across = 0.005
[[variant]]
name = "only"
height = 2.0
carry_along = 0.4
rest = 50
"#,
    );
    let s = t.settings().unwrap();
    assert_eq!(s.axes[ACROSS], Axis { lo: -0.08, hi: 0.02, step: 0.001 });
    assert!(s.axes[YAW].fixed());
    assert_eq!(s.human[ACROSS], 0.005);
    assert_eq!(s.crouch, Crouch::Both);
    assert_eq!(s.first_points(), 55);
    assert_eq!(s.variants[0].rest, Some(50));

    let bad = table(
        r#"
lead = 1
crouch = "maybe"
sample = "random"
[axes]
sideways = [0.0, 1.0]
across = [0.2, 0.1]
vz = [0.0, 6.0]
[human]
across = 0.0
[[variant]]
name = "a"
height = 30.0
[[variant]]
name = "a"
height = 1.0
"#,
    );
    let p = bad.settings().unwrap_err().join("\n");
    for want in [
        "lead is 2 to 12",
        "crouch = \"maybe\"",
        "sample = \"random\"",
        "sideways is not an axis",
        "across is [low, high]",
        "vz is inside 0.5..20",
        "bounces.human: across is over 0",
        "height is 0.2 to 20",
        "a name of its own",
    ] {
        assert!(p.contains(want), "{want}: {p}");
    }
    // A grid's counts need the grid sampler.
    let p = table("[grid]\nacross = 3\n").settings().unwrap_err().join("\n");
    assert!(p.contains("only with sample = \"grid\""), "{p}");
    // The other runs are refused beside it.
    let p = BouncesConfig::default().problems("ascension", true, false, false, true);
    assert_eq!(p.len(), 2, "{p:?}");
}

#[test]
fn the_table_round_trips_through_toml_with_its_targets_listed() {
    let mut t = table(
        r#"
points = 10
[axes]
across = [-0.05, 0.05]
[[variant]]
name = "fall"
height = 3.65
"#,
    );
    t.targets = Targets::List(vec![TargetSpec {
        id: Some("s000-slot".into()),
        point: Some([1.0, 2.0, 3.0]),
        drift: Some([0.0, 1.0]),
        ..TargetSpec::default()
    }]);
    t.shard = 2;
    t.shards = 4;
    let text = toml::to_string(&t).unwrap();
    assert_eq!(toml::from_str::<BouncesConfig>(&text).unwrap(), t);
}

// ---- sites ----

fn point_target(id: &str, x: f32) -> TargetSpec {
    TargetSpec {
        id: Some(id.into()),
        point: Some([x, 0.0, 0.0]),
        drift: Some([0.0, 1.0]),
        ..TargetSpec::default()
    }
}

#[test]
fn a_sites_frame_is_its_edge_and_its_drift() {
    // Ascension's seam: the edge along X at y 4, drifting toward +Y.
    let t = Target::from_spec(
        0,
        &TargetSpec {
            id: Some("seam".into()),
            edge: Some([[-11.10, 4.0, -3.2], [-9.98, 4.0, -3.2]]),
            drift: Some([0.0, 1.0]),
            ..TargetSpec::default()
        },
    )
    .unwrap();
    let sites = Site::of(std::slice::from_ref(&t), 1);
    assert_eq!(sites.len(), 1);
    let s = &sites[0];
    assert_eq!(s.id, "seam");
    assert!((s.point[0] - -10.54).abs() < 1e-4);
    assert_eq!(s.drift, [0.0, 1.0]);
    let mut v = [0.0; N];
    v[ACROSS] = -0.03;
    v[ALONG] = 0.02;
    v[DEPTH] = 0.01;
    v[V_DRIFT] = 0.4;
    v[V_SIDE] = 0.1;
    v[VZ] = 5.3;
    v[YAW] = 90.0;
    let at = s.place(0.4, &v);
    assert!((at.pos[0] - (-10.54 + 0.02)).abs() < 1e-4, "{at:?}");
    assert!((at.pos[1] - 3.97).abs() < 1e-4, "{at:?}");
    assert!((at.pos[2] - (-3.2 + 0.41)).abs() < 1e-4, "{at:?}");
    // Along the drift (+Y) at 0.4, to its left (-X) at 0.1, down at 5.3.
    assert!((at.vel[0] - -0.1).abs() < 1e-5 && (at.vel[1] - 0.4).abs() < 1e-5);
    assert_eq!(at.vel[2], -5.3);
    // Facing a quarter turn left of the drift: -X.
    assert!((at.facing[0] - -1.0).abs() < 1e-5 && at.facing[1].abs() < 1e-5);
    // What is measured back is what was asked.
    let m = s.measured(0.4, at.pos, at.vel);
    for (got, want) in m.iter().zip([-0.03, 0.02, 0.01, 5.3]) {
        assert!((got - want).abs() < 1e-4, "{m:?}");
    }
    // Several sites an edge are named by where they are.
    let three = Site::of(&[t], 3);
    assert_eq!(three.len(), 3);
    assert_eq!(three[1].id, "seam@0.50");
}

#[test]
fn a_start_is_the_last_row_moved_back_along_the_fall() {
    let last = Placed {
        pos: [1.0, 2.0, 0.4],
        vel: [0.3, -0.6, -5.4],
        facing: [1.0, 0.0],
    };
    assert_eq!(back(&last, 0), last);
    let start = back(&last, 2);
    // Forward again by the fall model: the tick adds the gravity, then
    // moves by the new speed.
    let mut p = start;
    for _ in 0..2 {
        p.vel[2] -= FALL_GRAVITY / 30.0;
        for a in 0..3 {
            p.pos[a] += p.vel[a] / 30.0;
        }
    }
    for a in 0..3 {
        assert!((p.pos[a] - last.pos[a]).abs() < 1e-5, "{p:?}");
        assert!((p.vel[a] - last.vel[a]).abs() < 1e-5, "{p:?}");
    }
    assert!(start.pos[2] > last.pos[2] + 0.34);
}

#[test]
fn batches_keep_their_sites_clear_of_each_other() {
    let points = [[0.0, 0.0], [0.5, 0.0], [2.0, 0.0], [2.2, 0.0], [4.0, 0.0], [6.0, 0.0]];
    let b = batches(&points, 3, 1.2);
    assert_eq!(b, vec![vec![0, 2, 4], vec![1, 3, 5]]);
    // Two sites nearer than the clearance never share a batch.
    assert_eq!(batches(&[[0.0, 0.0], [0.5, 0.0], [0.9, 0.0]], 3, 1.2).len(), 3);
    // Every site is in one batch.
    let mut all: Vec<usize> = b.concat();
    all.sort_unstable();
    assert_eq!(all, [0, 1, 2, 3, 4, 5]);
    assert!(batches(&[], 3, 1.2).is_empty());
}

// ---- one trial ----

fn sense(pos: [f32; 3], vz: f32, word: u32) -> Sense {
    Sense {
        pos: Some(pos),
        vel: Some([0.0, 0.0, vz]),
        airborne: true,
        pairs: vec![PairSense { agent: 9, word }],
        asleep: false,
        teleported: None,
    }
}

fn plan(flight: u32) -> Plan {
    let last = Placed {
        pos: [0.0, -0.03, 0.2],
        vel: [0.0, 0.4, -5.4],
        facing: [0.0, 1.0],
    };
    Plan {
        start: back(&last, flight),
        last,
        flight,
        primed: true,
        crouch: false,
    }
}

/// Runs a trial whose teleport lands, flies `flight` rows and touches
/// down, leaving at `out`.
fn fly(p: Plan, out: [f32; 4], after_word: u32) -> (TrialRun, Vec<Act>) {
    let mut run = TrialRun::new(p, 3.5);
    let mut acts = vec![run.step(&sense([0.0, 0.0, 1.0], -4.0, PRIMED))];
    let mut landed = sense(p.start.pos, -4.0, PRIMED);
    landed.teleported = Some(Ok(()));
    acts.push(run.step(&landed));
    let mut at = p.start;
    for _ in 0..p.flight {
        at.vel[2] -= FALL_GRAVITY / 30.0;
        for a in 0..3 {
            at.pos[a] += at.vel[a] / 30.0;
        }
        acts.push(run.step(&sense(at.pos, at.vel[2], PRIMED)));
    }
    for vz in out {
        assert!(!run.done());
        acts.push(run.step(&sense(at.pos, vz, after_word)));
    }
    (run, acts)
}

#[test]
fn a_trial_teleports_writes_its_velocity_flies_and_is_judged() {
    let p = plan(1);
    let (run, acts) = fly(p, [9.0, 12.3, 11.9, 11.5], PRIMED);
    assert!(run.done());
    // Row 0: the teleport and the word.
    assert_eq!(acts[0].base.teleport, Some((p.start.pos, p.start.facing)));
    assert_eq!(acts[0].base.word, Some(PRIMED));
    // The landing row: the start's whole velocity.
    assert_eq!(acts[1].velocity, Some(p.start.vel));
    // The planned last row: its whole velocity, and the word again.
    assert_eq!(acts[2].velocity, Some(p.last.vel));
    assert_eq!(acts[2].base.word, Some(PRIMED));
    // Nothing is written from the touchdown on.
    assert!(acts[3..].iter().all(|a| a.velocity.is_none() && a.base.word.is_none()));
    let r = run.result();
    assert_eq!(r.outcome, Outcome::Bounced);
    assert_eq!(r.launch, 12.3);
    assert_eq!(r.rows, 1);
    // The last airborne row is the planned one, with the velocity its
    // tick ran from.
    let (pos, vel) = r.last.unwrap();
    assert!((pos[2] - p.last.pos[2]).abs() < 1e-5);
    assert_eq!(vel, p.last.vel);
}

#[test]
fn a_trial_tells_no_bounce_from_a_lost_prime() {
    let (run, _) = fly(plan(1), [0.0, 0.0, 0.0, 0.0], PRIMED);
    assert_eq!(run.result().outcome, Outcome::NoBounce);
    // The pair came out of the touchdown tick at 0: the prime was lost.
    let (run, _) = fly(plan(1), [0.0, 0.0, 0.0, 0.0], 0);
    let r = run.result();
    assert_eq!(r.outcome, Outcome::PrimeLost);
    assert_eq!(r.why.as_deref(), Some("reset (word 0)"));
    // A launch under the bar is no bounce: a jump's 3.08.
    let (run, _) = fly(plan(1), [3.08, 2.9, 2.8, 2.6], PRIMED);
    assert_eq!(run.result().outcome, Outcome::NoBounce);
    // An unprimed trial writes the word 0 and has no prime to lose.
    let mut p = plan(1);
    p.primed = false;
    let (run, acts) = fly(p, [0.0, 0.0, 0.0, 0.0], 0);
    assert_eq!(acts[0].base.word, Some(0));
    assert_eq!(run.result().outcome, Outcome::NoBounce);
    // With no flight row the start is the last row.
    let p = plan(0);
    assert_eq!(p.start, p.last);
    let (run, acts) = fly(p, [8.0, 8.0, 8.0, 8.0], PRIMED);
    assert_eq!(acts[1].velocity, Some(p.last.vel));
    assert_eq!((run.result().outcome, run.result().rows), (Outcome::Bounced, 0));
    // Crouch is held on every row.
    let mut p = plan(1);
    p.crouch = true;
    let (_, acts) = fly(p, [0.0, 0.0, 0.0, 0.0], PRIMED);
    assert!(acts.iter().all(|a| a.crouch));
}

#[test]
fn a_trial_names_its_setup_failures() {
    let p = plan(1);
    // The teleport is refused.
    let mut run = TrialRun::new(p, 3.5);
    run.step(&sense([0.0; 3], -4.0, PRIMED));
    let mut s = sense([0.0; 3], -4.0, PRIMED);
    s.teleported = Some(Err("no player".into()));
    run.step(&s);
    assert_eq!(
        (run.result().outcome, run.result().why.as_deref()),
        (Outcome::SetupFailed, Some("teleport: no player"))
    );
    // It never runs.
    let mut run = TrialRun::new(p, 3.5);
    for _ in 0..=TELEPORT_WAIT {
        run.step(&sense([0.0; 3], -4.0, PRIMED));
    }
    assert!(run.done());
    assert_eq!(run.result().why.as_deref(), Some("teleport: never ran"));
    // It lands somewhere else, and again once asked against the miss:
    // the failure names the miss on each axis.
    let mut run = TrialRun::new(p, 3.5);
    run.step(&sense([0.0; 3], -4.0, PRIMED));
    let at = |d: [f32; 3]| {
        let mut s = sense(
            [
                p.start.pos[0] + d[0],
                p.start.pos[1] + d[1],
                p.start.pos[2] + d[2],
            ],
            -4.0,
            PRIMED,
        );
        s.teleported = Some(Ok(()));
        s
    };
    run.step(&at([0.0, 0.0, 9.0]));
    assert!(!run.done());
    run.step(&sense([0.0; 3], -4.0, PRIMED));
    run.step(&at([0.03, 0.0, -0.4]));
    assert!(run.done());
    assert_eq!(
        run.result().why.as_deref(),
        Some("teleport: landed 0.401 off (x +0.030 y +0.000 z -0.400) with its aim corrected")
    );
    assert_eq!(run.result().aim, None);
    // The body never touches down.
    let mut run = TrialRun::new(p, 3.5);
    run.step(&sense([0.0; 3], -4.0, PRIMED));
    let mut s = sense(p.start.pos, -4.0, PRIMED);
    s.teleported = Some(Ok(()));
    run.step(&s);
    for _ in 0..20 {
        run.step(&sense(p.start.pos, -5.0, PRIMED));
    }
    assert!(run.done());
    let r = run.result();
    assert_eq!(r.outcome, Outcome::SetupFailed);
    assert!(r.why.unwrap().starts_with("no touchdown"));
}

/// A placement off by a steady amount: the trial asks again against the
/// miss, lands on its start and carries on; the aim it landed with puts
/// the next trial's first teleport on the start.
#[test]
fn a_steady_miss_is_asked_again_and_its_aim_kept() {
    let p = plan(1);
    let lift = [0.002, -0.001, 0.4];
    let land = |asked: [f32; 3]| {
        let mut s = sense(
            [asked[0] + lift[0], asked[1] + lift[1], asked[2] + lift[2]],
            -4.0,
            PRIMED,
        );
        s.teleported = Some(Ok(()));
        s
    };
    let mut run = TrialRun::new(p, 3.5);
    let first = run.step(&sense([0.0; 3], -4.0, PRIMED));
    let (asked, facing) = first.base.teleport.unwrap();
    assert_eq!((asked, facing), (p.start.pos, p.start.facing));
    // The miss: asked again, the miss taken off; no velocity yet.
    let again = run.step(&land(asked));
    assert!(again.velocity.is_none());
    assert_eq!(again.base.word, Some(PRIMED));
    let (asked, _) = again.base.teleport.expect("asked again");
    for a in 0..3 {
        assert!((asked[a] - (p.start.pos[a] - lift[a])).abs() < 1e-5, "{asked:?}");
    }
    // A row with no answer yet, then the landing on the start.
    let waiting = run.step(&sense([0.0; 3], -4.0, PRIMED));
    assert!(waiting.base.teleport.is_none() && waiting.velocity.is_none());
    let landed = run.step(&land(asked));
    assert_eq!(landed.velocity, Some(p.start.vel));
    let aim = run.result().aim.expect("an aim that landed");
    for a in 0..3 {
        assert!((aim[a] + lift[a]).abs() < 1e-5, "{aim:?}");
    }
    // The next trial asks with it and lands first time.
    let mut next = TrialRun::new(p, 3.5).aimed(aim);
    let (asked, _) = next.step(&sense([0.0; 3], -4.0, PRIMED)).base.teleport.unwrap();
    let landed = next.step(&land(asked));
    assert!(landed.base.teleport.is_none());
    assert_eq!(landed.velocity, Some(p.start.vel));
    assert_eq!(next.result().aim, Some(aim));
}

// ---- the whole job over a made-up machine ----

/// The origin rests this far over the floor (z 0).
const REST: f32 = 0.03;
/// The made-up seam: a primed body whose last airborne row is this far
/// across and under this height bounces.
const BAND: [f32; 2] = [-0.06, -0.01];
const DEEP: f32 = 0.15;

fn bounces(y: f32, z: f32) -> bool {
    (BAND[0]..=BAND[1]).contains(&y) && z < DEEP
}

#[derive(Clone, Debug)]
struct Body {
    pos: [f32; 3],
    vel: [f32; 3],
    airborne: bool,
    word: u32,
    teleport: Option<[f32; 3]>,
    landed: bool,
}

/// Bodies that fall under the drops' fall model onto a floor at z 0.
#[derive(Clone, Debug)]
struct World {
    tick: u64,
    bodies: Vec<Body>,
}

impl World {
    fn new(seats: usize) -> World {
        World {
            tick: 100,
            bodies: (0..seats)
                .map(|i| Body {
                    pos: [50.0 + i as f32, 50.0, REST],
                    vel: [0.0; 3],
                    airborne: false,
                    word: 0,
                    teleport: None,
                    landed: false,
                })
                .collect(),
        }
    }

    /// The start of a tick: last tick's teleports land, `lift` over the
    /// point asked for, then every seat is read.
    fn senses(&mut self, lift: [f32; 3]) -> Vec<SeatSense> {
        self.bodies
            .iter_mut()
            .enumerate()
            .map(|(i, b)| {
                if let Some(to) = b.teleport.take() {
                    b.pos = [to[0] + lift[0], to[1] + lift[1], to[2] + lift[2]];
                    b.airborne = b.pos[2] > REST;
                    b.landed = true;
                }
                SeatSense {
                    seat: i as u8,
                    sense: Sense {
                        pos: Some(b.pos),
                        vel: Some(b.vel),
                        airborne: b.airborne,
                        pairs: vec![PairSense {
                            agent: 40 + i as u32,
                            word: b.word,
                        }],
                        asleep: false,
                        teleported: std::mem::take(&mut b.landed).then_some(Ok(())),
                    },
                }
            })
            .collect()
    }

    fn apply(&mut self, acts: &[SeatAct]) {
        for a in acts {
            let b = &mut self.bodies[a.seat as usize];
            if let Some(w) = a.act.base.word {
                b.word = w;
            }
            if a.act.base.stop {
                b.vel = [0.0; 3];
            }
            if let Some(xy) = a.act.base.level {
                b.vel[0] = xy[0];
                b.vel[1] = xy[1];
            }
            if let Some(v) = a.act.velocity {
                b.vel = v;
            }
            if let Some((to, _)) = a.act.base.teleport {
                b.teleport = Some(to);
            }
        }
    }

    /// The tick itself.
    fn run(&mut self) {
        self.tick += 1;
        for b in &mut self.bodies {
            if !b.airborne {
                continue;
            }
            let from = b.pos;
            b.vel[2] -= FALL_GRAVITY / 30.0;
            for a in 0..3 {
                b.pos[a] += b.vel[a] / 30.0;
            }
            if b.pos[2] > REST {
                continue;
            }
            if b.word == PRIMED && b.vel[2] < -1.0 && bounces(from[1], from[2]) {
                b.pos = from;
                b.vel[2] = 9.0;
            } else {
                b.pos[2] = REST;
                b.vel = [0.0; 3];
                b.airborne = false;
            }
        }
    }

    fn hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64 ^ self.tick;
        let mut mix = |v: u32| {
            h = (h ^ u64::from(v)).wrapping_mul(0x0000_0100_0000_01b3);
        };
        for b in &self.bodies {
            for v in b.pos.iter().chain(&b.vel) {
                mix(v.to_bits());
            }
            mix(b.word);
            mix(u32::from(b.airborne));
        }
        h
    }
}

/// The world and its forks: a capture is a copy.
struct Machine {
    world: World,
    saved: BTreeMap<u64, World>,
    /// Captures answered busy before one is taken.
    busy: u32,
    /// Forks are never answered.
    deaf: bool,
    /// Where a teleport puts the origin over the point asked for: the
    /// game's placement, not the world's state, so no fork puts it back.
    /// The game's own is [`PLACE_MISS`].
    lift: [f32; 3],
    /// What came of the last fork, for the next tick.
    done: Option<ForkDone>,
}

impl Machine {
    fn fork(&mut self, ask: ForkAsk) -> Option<ForkDone> {
        if self.deaf {
            return None;
        }
        Some(match ask {
            ForkAsk::Capture { key } => {
                assert!(
                    self.world.bodies.iter().all(|b| b.teleport.is_none()),
                    "a capture with a teleport in flight"
                );
                if self.busy > 0 {
                    self.busy -= 1;
                    ForkDone::Busy("fake: a thread is in a callback".into())
                } else {
                    self.saved.insert(key, self.world.clone());
                    ForkDone::Captured { key }
                }
            }
            ForkAsk::Restore { key, check } => match self.saved.get(&key) {
                Some(w) => {
                    let end_hash = check.then(|| self.world.hash());
                    self.world = w.clone();
                    ForkDone::Restored {
                        key,
                        end_hash,
                        ram_equal: check.then(|| self.world.hash() == w.hash()),
                    }
                }
                None => ForkDone::Failed(format!("no entry {key}")),
            },
        })
    }

    /// Runs `job` to its end, a hook and a tick at a time.
    fn drive(&mut self, job: &mut Job) {
        self.drive_until(job, |_| false);
    }

    /// Runs `job` until it finishes (true) or, after a tick, `stop`
    /// says so (false).
    fn drive_until(&mut self, job: &mut Job, stop: impl Fn(&Job) -> bool) -> bool {
        for _ in 0..400_000 {
            let senses = self.world.senses(self.lift);
            let step = job.tick(self.world.tick, &senses, self.done.take());
            if job.finished() {
                return true;
            }
            self.world.apply(&step.acts);
            // The fork is between two slices, just after the hooks: the
            // acts are in the saved state, the tick is not.
            if let Some(ask) = step.fork {
                self.done = self.fork(ask);
            }
            self.world.run();
            if stop(job) {
                return false;
            }
        }
        panic!("the job did not finish: {:?}", job.at());
    }
}

const JOB: &str = r#"
seats = 3
points = 60
rounds = 2
refine = 30
replay = 4
unprimed = 4
min_support = 3
[axes]
along = [0.0, 0.0]
v_drift = [0.3, 0.3]
v_side = [0.0, 0.0]
vz = [5.4, 5.4]
yaw = [0.0, 0.0]
[[variant]]
name = "low"
height = 1.0
[[variant]]
name = "high"
height = 1.5
"#;

fn job_config() -> BouncesConfig {
    let mut c = table(JOB);
    c.targets = Targets::List(vec![
        point_target("a", 0.0),
        point_target("b", 2.0),
        point_target("c", 4.0),
        point_target("d", 6.0),
    ]);
    c
}

fn machine() -> Machine {
    Machine {
        world: World::new(3),
        saved: BTreeMap::new(),
        busy: 0,
        deaf: false,
        lift: PLACE_MISS,
        done: None,
    }
}

#[test]
fn the_job_forks_once_a_variant_and_runs_every_trial_from_the_fork() {
    let mut job = Job::new(&job_config(), "testmap", 3).unwrap();
    assert_eq!((job.sites(), job.batches()), (4, 2));
    let mut m = machine();
    m.drive(&mut job);
    assert_eq!(job.error(), None);
    let run = job.report();
    assert_eq!(run.format, FORMAT);
    assert_eq!(run.done as usize, run.trials.len());
    assert!(run.done <= run.planned);
    // One capture a variant a batch; the first batch's first two waves a
    // variant ended in the same state both times.
    assert_eq!(run.fork.captures, 4);
    assert_eq!(run.fork.busy, 0);
    assert_eq!((run.fork.replay_checks, run.fork.replay_mismatches), (2, 0));
    assert!(run.fork.ram_checks >= 4);
    assert_eq!(run.fork.ram_mismatches, 0);
    assert_eq!(run.fork.restores, run.fork.waves);
    // A wave is seven ticks: the rest of the fork's tick, the teleport's,
    // the start's, the last row's and the launch's three; six of them are
    // rows the job sees after the first.
    let rows = run.fork.ticks as f32 / run.fork.waves as f32;
    assert!((5.5..=7.5).contains(&rows), "{rows}");
    for (i, x) in run.sites.iter().enumerate() {
        let anchor = x.anchor.expect("an anchor height");
        assert!(anchor > 0.0 && anchor < 0.25, "{anchor}");
        assert!(x.bases.iter().all(|b| b.based), "{:?}", x.bases);
        // The base drops land on the edge's line, outside the band.
        assert!(x.bases.iter().all(|b| b.outcome == Some(Outcome::NoBounce)));
        assert!(x.points.len() >= 60, "{}", x.points.len());
        let mine: Vec<&Trial> = run.trials.iter().filter(|t| t.site as usize == i).collect();
        let count = |k: Kind, v: u8| mine.iter().filter(|t| t.kind == k && t.variant == v).count();
        for v in 0..2 {
            // Every point once a variant, the replays, and the anchor
            // (twice in the batch that checks the fork).
            assert_eq!(count(Kind::Sample, v), x.points.len());
            assert_eq!(count(Kind::Replay, v), 4);
            assert_eq!(count(Kind::Anchor, v), if i < 3 { 2 } else { 1 });
        }
        assert_eq!((count(Kind::Unprimed, 0), count(Kind::Unprimed, 1)), (4, 0));
        for t in &mine {
            match t.kind {
                // The base's own last row put back comes out as the
                // base did.
                Kind::Anchor => assert_eq!(t.outcome, Outcome::NoBounce, "{t:?}"),
                Kind::Unprimed => assert_eq!(t.outcome, Outcome::NoBounce, "{t:?}"),
                Kind::Sample | Kind::Replay => {
                    let last = t.last.expect("a touchdown");
                    let (y, z) = (last[0], last[2] + anchor);
                    // Away from the made-up seam's edges, the outcome
                    // is its rule on the last row as measured.
                    let near = [BAND[0], BAND[1]].iter().any(|e| (y - e).abs() < 1e-4)
                        || (z - DEEP).abs() < 1e-4;
                    if !near {
                        let want = if bounces(y, z) {
                            Outcome::Bounced
                        } else {
                            Outcome::NoBounce
                        };
                        assert_eq!(t.outcome, want, "{t:?} y {y} z {z}");
                    }
                }
            }
        }
        // Refinement added points, each on the planned grid and in range.
        assert!(x.points.iter().any(|p| p.round > 0));
        assert!(
            x.points
                .iter()
                .all(|p| p.x[ACROSS] >= -0.1 && p.x[ACROSS] <= 0.1 && p.x[VZ] == 5.4)
        );
    }
    // The pooled report: every site has a window inside the band, on both
    // variants.
    let r = summarize(std::slice::from_ref(&run));
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.sites.len(), 4);
    for (i, s) in r.sites.iter().enumerate() {
        assert_eq!(s.rank as usize, i + 1);
        assert_eq!(s.reproducibility, Some(1.0));
        assert_eq!(s.replay_mismatches, 0);
        assert_eq!((s.unprimed, s.unprimed_bounced), (4, 0));
        assert!(s.variants.iter().all(|v| v.agrees == Some(true)));
        assert_eq!(s.best_launch, 9.0);
        // Both variants agree at every point, so a site is provable when
        // a window has its support among these few points, else fragile.
        if s.verdict != Verdict::Provable {
            assert_eq!(s.verdict, Verdict::Fragile, "{s:?}");
            assert!(s.window.is_none() && s.ease == 0.0, "{s:?}");
            continue;
        }
        let w = s.window.as_ref().unwrap();
        assert!(w.centre[ACROSS] > BAND[0] && w.centre[ACROSS] < BAND[1], "{w:?}");
        // The band is five steps wide; a sparse sample may miss its edge
        // by a step or two.
        assert!(s.ease > 0.0 && s.ease <= 4.5, "{}", s.ease);
        let p = s.press.as_ref().unwrap();
        // The drift is across the edge, so the takeoff's room is the
        // window's range across it: out to the first failure sampled
        // beside the window, so no narrower than the window itself and
        // no wider than what was sampled.
        assert!(p.position >= 2.0 * s.ease * HUMAN[ACROSS] - 1e-4, "{p:?}");
        assert!(p.position <= 0.2 + 1e-4, "{p:?}");
        assert!((p.aim[1] - w.centre[ACROSS]).abs() < 1e-5);
    }
    assert!(r.sites.iter().any(|s| s.verdict == Verdict::Provable), "{r:?}");
    assert!(r.sites.windows(2).all(|w| w[0].ease >= w[1].ease));
    // The same run again is the same block: the search is deterministic.
    let mut again = Job::new(&job_config(), "testmap", 3).unwrap();
    machine().drive(&mut again);
    assert_eq!(again.report(), run);
}

/// The game's placement puts the origin [`PLACE_LIFT`] over the point
/// asked for (`calls::Call::Teleport`, "a little above the point"). Every
/// site asks against it from its first teleport, and a run whose
/// placement lifts by just that has no note of it: the note is for a
/// miss the engine's constant does not explain.
#[test]
fn the_games_placement_lift_is_asked_against_from_the_first_teleport() {
    let job = Job::new(&job_config(), "testmap", 3).unwrap();
    for x in &job.sites {
        assert_eq!(x.aim, [0.0, 0.0, -PLACE_LIFT], "{}", x.site.id);
    }
    let mut job = Job::new(&job_config(), "testmap", 3).unwrap();
    let mut m = machine();
    assert_eq!(m.lift, [0.0, 0.0, 0.0328]);
    m.drive(&mut job);
    assert_eq!(job.error(), None);
    let run = job.report();
    assert!(!run.trials.is_empty());
    assert!(
        run.trials
            .iter()
            .all(|t| t.outcome != Outcome::SetupFailed && !t.reaimed),
        "{:?}",
        run.trials.iter().find(|t| t.outcome == Outcome::SetupFailed || t.reaimed)
    );
    assert!(
        !run.notes.iter().any(|n| n.starts_with("teleport: the placement")),
        "{:?}",
        run.notes
    );
}

/// A placement off by more than the game's lift: the base drops' lifts
/// measure it, so every trial asks against it and lands on its start
/// first time: no trial fails its setup or asks twice, and the replay
/// check still pairs the first two waves. The note names the miss and
/// the part of it past the game's lift.
#[test]
fn a_placement_off_by_a_steady_amount_is_aimed_against() {
    let mut job = Job::new(&job_config(), "testmap", 3).unwrap();
    let mut m = machine();
    m.lift = [0.002, -0.003, PLACE_LIFT + 0.03];
    m.drive(&mut job);
    assert_eq!(job.error(), None);
    let run = job.report();
    assert!(!run.trials.is_empty());
    assert!(
        run.trials
            .iter()
            .all(|t| t.outcome != Outcome::SetupFailed && !t.reaimed),
        "{:?}",
        run.trials.iter().find(|t| t.outcome == Outcome::SetupFailed || t.reaimed)
    );
    assert_eq!((run.fork.replay_checks, run.fork.replay_mismatches), (2, 0));
    assert!(
        run.notes.iter().any(|n| n
            .starts_with("teleport: the placement put the origin 0.063 off (x +0.002 y -0.003 z +0.063)")
            && n.contains("0.030 off (x +0.002 y -0.003 z +0.030) past the game's 0.0328 lift")),
        "{:?}",
        run.notes
    );
    // A miss the base drops did not see (it starts with the first
    // batch's first wave): each of that batch's three sites asks its
    // first trial again and lands, and its later trials ask with the aim
    // that landed. The last batch's base drops see the miss.
    let mut job = Job::new(&job_config(), "testmap", 3).unwrap();
    let mut m = machine();
    assert!(!m.drive_until(&mut job, |j| j.at().0 == "wave"));
    m.lift = [0.0, 0.0, PLACE_LIFT + 0.03];
    m.drive(&mut job);
    assert_eq!(job.error(), None);
    let run = job.report();
    assert!(run.trials.iter().all(|t| t.outcome != Outcome::SetupFailed));
    let reaimed: Vec<&Trial> = run.trials.iter().filter(|t| t.reaimed).collect();
    assert_eq!(reaimed.len(), 3, "{reaimed:?}");
    assert!(reaimed.iter().all(|t| t.site < 3 && t.kind == Kind::Anchor), "{reaimed:?}");
    // The corrected wave is no replay check's: the other variant's pair
    // still is.
    assert_eq!((run.fork.replay_checks, run.fork.replay_mismatches), (1, 0));
}

/// A held seat that stirs on the row it is let go (Ascension's
/// `released at tick N+1..N+3, planned N`): a still body arms only after
/// [`drops::SETTLE_STILL`] still rows, so its arm, and its release, came
/// up to that many ticks after the plan and the variant lost its base on
/// the site. A held drop arms on the row its hold is let go, the stir
/// stopped, so the release is on the planned tick and the base is kept.
#[test]
fn a_seat_that_stirs_as_it_is_let_go_keeps_its_base() {
    let mut job = Job::new(&job_config(), "testmap", 3).unwrap();
    let mut m = machine();
    // The tick the launch is planned on: every seat is held and still,
    // and the batch's three sites fall alike, so every seat is let go on
    // the next tick.
    assert!(!m.drive_until(&mut job, |j| j.at().0 == "launch"));
    // Seat 0's body stirs on that row (under the stop speed, so the
    // settle writes no stop), and is still again after it.
    m.world.bodies[0].vel = [0.3, 0.0, 0.0];
    assert!(!m.drive_until(&mut job, |_| true));
    m.world.bodies[0].vel = [0.0; 3];
    m.drive(&mut job);
    assert_eq!(job.error(), None);
    let run = job.report();
    assert!(
        !run.notes.iter().any(|n| n.contains("out of step")),
        "{:?}",
        run.notes
    );
    assert!(
        run.sites.iter().all(|x| x.bases.iter().all(|b| b.based)),
        "{:?}",
        run.sites[0].bases
    );
    assert_eq!(run.fork.captures, 4);
    // Its trials ran from the base, and its anchor came out as its drop.
    let anchors: Vec<&Trial> = run
        .trials
        .iter()
        .filter(|t| t.site == 0 && t.variant == 0 && t.kind == Kind::Anchor)
        .collect();
    assert_eq!(anchors.len(), 2);
    assert!(anchors.iter().all(|t| t.outcome == Outcome::NoBounce), "{anchors:?}");
    assert!(run.trials.iter().any(|t| t.site == 0 && t.variant == 0 && t.kind == Kind::Sample));
}

#[test]
fn a_busy_capture_is_asked_again_and_then_given_up_by_name() {
    // Busy twice: the capture is taken a row or two later in the fall.
    let mut job = Job::new(&job_config(), "testmap", 3).unwrap();
    let mut m = machine();
    m.busy = 2;
    m.drive(&mut job);
    assert_eq!(job.error(), None);
    let run = job.report();
    assert_eq!((run.fork.captures, run.fork.busy), (4, 2));
    // The report names what was busy.
    assert_eq!(
        run.fork.last_busy.as_deref(),
        Some("fake: a thread is in a callback")
    );
    let md = markdown(&summarize(std::slice::from_ref(&run)));
    assert!(
        md.contains("2 busy (the last: fake: a thread is in a callback);"),
        "{md}"
    );
    assert!(run.sites.iter().all(|x| x.bases.iter().all(|b| b.based)));
    // Busy for every row the lead allows: that variant has no base on
    // the batch's sites, by name, and the other still runs.
    let mut job = Job::new(&job_config(), "testmap", 3).unwrap();
    let mut m = machine();
    m.busy = 3;
    m.drive(&mut job);
    assert_eq!(job.error(), None);
    let run = job.report();
    assert_eq!((run.fork.captures, run.fork.busy), (3, 3));
    for x in &run.sites[..3] {
        assert!(!x.bases[0].based);
        assert_eq!(
            x.bases[0].why.as_deref(),
            Some("fork: busy: fake: a thread is in a callback")
        );
        assert!(x.bases[1].based);
    }
    assert!(run.sites[3].bases.iter().all(|b| b.based));
    assert!(run.notes.iter().any(|n| n.contains("no base on low")), "{:?}", run.notes);
    assert!(run.trials.iter().all(|t| !(t.site < 3 && t.variant == 0)));
    // A site with one variant is still ranked, on that variant alone.
    let r = summarize(&[run]);
    assert!(
        r.sites
            .iter()
            .all(|s| matches!(s.verdict, Verdict::Provable | Verdict::Fragile)),
        "{r:?}"
    );
    assert!(r.sites.iter().any(|s| s.verdict == Verdict::Provable), "{r:?}");
}

#[test]
fn a_host_with_no_fork_stops_the_run_by_name() {
    let mut job = Job::new(&job_config(), "testmap", 3).unwrap();
    let mut m = machine();
    m.deaf = true;
    m.drive(&mut job);
    assert_eq!(
        job.error(),
        Some("fork: no answer to a capture: this host's loop runs no bounce fork")
    );
    let run = job.report();
    assert!(run.trials.is_empty());
    let r = summarize(&[run]);
    assert_eq!(r.errors.len(), 1);
    assert!(r.sites.iter().all(|s| s.verdict == Verdict::Untested));
    // A scan is the batch's to resolve.
    let e = Job::new(&BouncesConfig::default(), "ascension", 15).err().unwrap();
    assert!(e.contains("resolved by `h2 sim bounces`"), "{e}");
}

// ---- the report ----

/// A block with one site whose points and trials are given.
fn block(id: &str, x: f32, trials: &[(f32, [Outcome; 2])]) -> BouncesRun {
    let targets = [Target::from_spec(0, &point_target(id, x)).unwrap()];
    let site = Site::of(&targets, 1).remove(0);
    let points: Vec<Point> = trials
        .iter()
        .map(|(a, _)| {
            let mut v = [0.0; N];
            v[ACROSS] = *a;
            v[VZ] = 5.4;
            v[V_DRIFT] = 0.3;
            Point {
                x: v,
                crouch: false,
                round: 0,
            }
        })
        .collect();
    let mut out = Vec::new();
    for (p, (a, outcomes)) in trials.iter().enumerate() {
        for (v, o) in outcomes.iter().enumerate() {
            out.push(Trial {
                site: 0,
                variant: v as u8,
                kind: Kind::Sample,
                point: Some(p as u32),
                outcome: *o,
                why: None,
                launch: if *o == Outcome::Bounced { 11.5 } else { 0.0 },
                last: Some([*a, 0.0, 0.0, 5.4]),
                reaimed: false,
            });
        }
    }
    let based = Base {
        based: true,
        outcome: Some(Outcome::Bounced),
        ..Base::default()
    };
    BouncesRun {
        format: FORMAT.into(),
        map: "ascension".into(),
        shards: 1,
        seats: 15,
        planned: 100,
        done: out.len() as u32,
        axes: axes().to_vec(),
        human: HUMAN.to_vec(),
        variants: Variant::defaults()[..2].to_vec(),
        lead: LEAD,
        flight: FLIGHT,
        bounce_vz: 3.5,
        min_support: 3,
        sites: vec![SiteRun {
            site,
            anchor: Some(0.4),
            bases: vec![based.clone(), based],
            points,
        }],
        trials: out,
        ..BouncesRun::default()
    }
}

/// Points 0.01 apart from -0.10 to 0.10, bouncing on both variants
/// between `from` and `to`, and on the first variant only out to `some`
/// further.
fn strip(from: f32, to: f32, some: f32) -> Vec<(f32, [Outcome; 2])> {
    (0..=20)
        .map(|i| {
            let a = -0.10 + 0.01 * i as f32;
            let both = a > from - 0.001 && a < to + 0.001;
            let one = a > from - some - 0.001 && a < to + some + 0.001;
            let o = |yes: bool| if yes { Outcome::Bounced } else { Outcome::NoBounce };
            (a, [o(one), o(both)])
        })
        .collect()
}

#[test]
fn sites_are_ranked_by_verdict_then_ease() {
    let wide = block("wide", 0.0, &strip(-0.06, 0.0, 0.0));
    let narrow = block("narrow", 2.0, &strip(-0.04, -0.02, 0.02));
    let dead = block("dead", 4.0, &strip(1.0, 1.0, 0.0));
    let one: Vec<(f32, [Outcome; 2])> = strip(1.0, 1.0, 0.0)
        .into_iter()
        .map(|(a, o)| (a, if a.abs() < 0.001 { [Outcome::Bounced; 2] } else { o }))
        .collect();
    let lone = block("lone", 6.0, &one);
    let mut untested = block("untested", 8.0, &[]);
    untested.sites[0].bases.iter_mut().for_each(|b| {
        b.based = false;
        b.why = Some("fork: busy: x".into());
    });
    let r = summarize(&[dead, lone, narrow.clone(), untested, wide]);
    let order: Vec<(&str, Verdict)> = r.sites.iter().map(|s| (s.id.as_str(), s.verdict)).collect();
    assert_eq!(
        order,
        [
            ("wide", Verdict::Provable),
            ("narrow", Verdict::Provable),
            ("lone", Verdict::Fragile),
            ("dead", Verdict::None),
            ("untested", Verdict::Untested),
        ]
    );
    assert_eq!(r.runs, 5);
    assert_eq!(r.trials, 4 * 42);
    assert_eq!(r.outcomes["bounced"], 14 + 3 + 7 + 2);
    let wide = &r.sites[0];
    // -0.06 to 0.00 bounce on both: 7 points, the nearest failures at
    // -0.07 and 0.01, so the window is 4 steps of 0.01 around -0.03.
    assert!((wide.ease - 4.0).abs() < 0.01, "{}", wide.ease);
    assert_eq!((wide.all, wide.some, wide.none), (7, 0, 14));
    assert_eq!(wide.reproducibility, Some(1.0));
    assert!((wide.share.unwrap() - 7.0 / 21.0).abs() < 1e-5);
    let p = wide.press.as_ref().unwrap();
    assert!((p.position - 0.08).abs() < 1e-4, "{p:?}");
    // 0.08 at 0.3 units/s along the drift: 0.01 a tick, 8 ticks.
    assert!((p.timing_ticks.unwrap() - 8.0).abs() < 0.05, "{p:?}");
    assert!((p.aim[1] - -0.03).abs() < 1e-4);
    assert_eq!(p.phase_share, Some(1.0));
    // The height is the drop that touches down at the centre's 5.4.
    assert!(drops::fall_model(p.height).1 >= 5.4);
    assert!(drops::fall_model(p.height - 0.05).1 < 5.4);
    // Narrow: 3 points on both variants, 4 more on one only.
    let n = &r.sites[1];
    assert_eq!((n.all, n.some), (3, 4));
    assert!((n.reproducibility.unwrap() - 3.0 / 7.0).abs() < 1e-5);
    assert!((n.share_any.unwrap() - 7.0 / 21.0).abs() < 1e-5);
    assert!((n.ease - 2.0).abs() < 0.01, "{}", n.ease);
    assert_eq!(n.variants[0].bounced, 7);
    assert_eq!(n.variants[1].bounced, 3);
    // A lone bounce has no support: fragile, no window, ease 0.
    assert_eq!((r.sites[2].ease, r.sites[2].window.is_some()), (0.0, false));
    assert_eq!(r.sites[4].variants[0].why.as_deref(), Some("fork: busy: x"));
}

#[test]
fn the_report_counts_replays_anchors_and_the_forks_checks() {
    let mut b = block("wide", 0.0, &strip(-0.06, 0.0, 0.0));
    let extra = |kind: Kind, point: Option<u32>, outcome: Outcome| Trial {
        site: 0,
        variant: 0,
        kind,
        point,
        outcome,
        why: None,
        launch: 0.0,
        last: None,
        reaimed: false,
    };
    // Point 5 (-0.05) bounced; a replay that didn't is a mismatch.
    b.trials.push(extra(Kind::Replay, Some(5), Outcome::Bounced));
    b.trials.push(extra(Kind::Replay, Some(5), Outcome::NoBounce));
    b.trials.push(extra(Kind::Unprimed, Some(5), Outcome::Bounced));
    b.trials.push(extra(Kind::Anchor, None, Outcome::NoBounce));
    b.fork = ForkStats {
        captures: 2,
        restores: 30,
        waves: 30,
        ticks: 180,
        replay_checks: 2,
        replay_mismatches: 1,
        ram_checks: 4,
        ram_mismatches: 1,
        ..ForkStats::default()
    };
    b.error = Some("fork: restore: kernel: bad section".into());
    let r = summarize(&[b]);
    let s = &r.sites[0];
    assert_eq!((s.replays, s.replay_mismatches), (2, 1));
    assert_eq!((s.unprimed, s.unprimed_bounced), (1, 1));
    // The full drop bounced and its anchor didn't: they disagree.
    assert_eq!(s.variants[0].agrees, Some(false));
    assert_eq!(s.variants[1].agrees, None);
    // The extra trials are not the sampled ones.
    assert_eq!(s.trials, 42);
    assert_eq!(r.trials, 46);
    assert_eq!(r.errors.len(), 3, "{:?}", r.errors);
    assert!(r.errors[0].contains("kernel: bad section"));
    assert!(r.errors[1].contains("1 of 2 replayed waves"));
    assert!(r.errors[2].contains("1 of 4 checked restores"));
    // The report is JSON that reads back.
    let text = serde_json::to_string(&r).unwrap();
    assert_eq!(serde_json::from_str::<BouncesReport>(&text).unwrap(), r);
}

#[test]
fn the_markdown_and_the_verified_file_have_their_formats() {
    let r = summarize(&[
        block("wide", 0.0, &strip(-0.06, 0.0, 0.0)),
        block("dead", 4.0, &strip(1.0, 1.0, 0.0)),
    ]);
    let md = markdown(&r);
    let lines: Vec<&str> = md.lines().collect();
    assert_eq!(lines[0], "# Bounce search: ascension");
    let head = lines.iter().position(|l| l.starts_with("| # |")).unwrap();
    assert_eq!(
        lines[head],
        "| # | site | verdict | ease | position cm | press ticks | speed along | phases | provable share | reproducible | drop = anchor | best launch | trials |"
    );
    assert_eq!(
        lines[head + 2],
        "| 1 | wide | provable | 4.00 | 24.4 | 8.0 | 0.30 to 0.30 | 100% | 33% | 100% | 0/0 | 11.50 | 42 |"
    );
    assert_eq!(
        lines[head + 3],
        "| 2 | dead | none | 0.00 | - | - | - | - | 0% | - | 0/0 | 0.00 | 42 |"
    );
    assert!(md.contains("2 sites, 84 trials of 200 planned in 2 runs"), "{md}");
    // Only the provable site is a verified line, its landing point the
    // window's aim from the stand (a point target's stand is its point,
    // 0.01 up).
    let p = proven(&r);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].stand, [0.0, 0.0, 0.01]);
    assert!((p[0].offset[1] - -0.03).abs() < 1e-4);
    assert_eq!((p[0].trials, p[0].ok), (42, 14));
    let text = verified_text(&p);
    let height = p[0].height;
    assert_eq!(
        text,
        format!(
            "# superbounce verified v2\nbounce proven 0.000 0.000 0.010 dx=0.000 dy=-0.030 height={height:.3} trials=42 ok=14 best=11.50\n"
        )
    );
    assert_eq!(verified_text(&[]), "# superbounce verified v2\n");
}
