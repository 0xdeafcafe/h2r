//! The approach guide's rules: the impact against the sim's fall model,
//! the depth rule against the note's cut, the call, the chance, the fade
//! and the seam it is on (docs/game/superbounce.md, "The approach
//! guide").

use sim_bench::drops::{FALL_OFFSET, FALL_STILL_ROWS, FALL_V0, fall_model};

use super::*;

/// Ascension's proven seam: its edge at y 4.0 and z −3.2, drifting +Y,
/// its landing measured at y 3.976
/// (docs/game/notes/2026-11-22-seam-drop-depth.md).
fn ascension() -> Spot {
    Spot {
        kind: Kind::Bounce,
        edge: [[-11.10, 4.0, -3.2], [-9.98, 4.0, -3.2]],
        stand: [-10.457, 3.976, -3.2],
        landed: true,
        drift: [0.0, 1.0],
        ..Spot::default()
    }
}

fn seam() -> Seam {
    Seam::of(&ascension(), [-10.457, 3.976])
}

/// A flat floor at height 0.
fn flat(_: [f32; 2]) -> f32 {
    CLEARANCE
}

fn row(y: f32, z: f32) -> Impact {
    Impact {
        row: [-10.457, y, z],
        ticks: 0,
        speed: 5.3,
    }
}

fn near(a: f32, b: f32, eps: f32) {
    assert!((a - b).abs() <= eps, "{a} is not within {eps} of {b}");
}

fn near3(a: Vec3, b: Vec3) {
    for k in 0..3 {
        near(a[k], b[k], 1e-4);
    }
}

/// The impact is the sim's fall model: a drop from each of its heights,
/// taken up where the model's fall starts (the still rows over, at its
/// first speed), touches down from the model's row at the model's speed.
#[test]
fn the_impact_is_the_sims_fall_model() {
    for height in [2.43, 3.45, 3.65, 3.82, 4.5, 5.5] {
        let (n, v) = fall_model(height);
        let i = impact([0.0, 0.0, height + FALL_OFFSET], [0.0, 0.0, -FALL_V0], flat).unwrap();
        assert_eq!(i.ticks, n - FALL_STILL_ROWS, "{height}");
        near(i.speed, v, 1e-3);
        // The last free row is over the floor, and the next would not be.
        assert!(i.row[2] > CLEARANCE && i.row[2] - (i.speed + GRAVITY * TICK) * TICK <= CLEARANCE);
    }
    // The fall speed gains 0.13904 units/s a tick.
    near(GRAVITY * TICK, 0.13904, 1e-4);
}

/// The pipper for a known velocity and height: a 3.65 drop carried at
/// (0.3, −0.6) comes down 37 ticks on, that carry times 37 ticks from
/// where it is, at 5.378 units/s.
#[test]
fn the_pipper_is_the_carry_over_the_falls_ticks() {
    let i = impact([1.0, 2.0, 3.65 + FALL_OFFSET], [0.3, -0.6, -FALL_V0], flat).unwrap();
    assert_eq!(i.ticks, 37);
    near(i.row[0], 1.0 + 0.3 * 37.0 / 30.0, 1e-4);
    near(i.row[1], 2.0 - 0.6 * 37.0 / 30.0, 1e-4);
    near(i.speed, 5.378, 1e-3);
    // Twice the carry, twice as far.
    let fast = impact([1.0, 2.0, 3.65 + FALL_OFFSET], [0.6, -1.2, -FALL_V0], flat).unwrap();
    assert_eq!(fast.ticks, 37);
    near(fast.row[0] - 1.0, 2.0 * (i.row[0] - 1.0), 1e-4);
}

/// On the ground the flight is a jump from here: up at the jump's speed,
/// down onto the floor it left 43 ticks on at a standing jump's 2.9
/// units/s, the run's speed carried all the way.
#[test]
fn a_jump_now_lands_where_the_run_carries_it() {
    let v = jump([1.2, 0.0, 0.0]);
    assert_eq!(v, [1.2, 0.0, JUMP_VZ]);
    let i = impact([0.0, 0.0, CLEARANCE], v, flat).unwrap();
    assert_eq!(i.ticks, 43);
    near(i.row[0], 1.2 * 43.0 / 30.0, 1e-3);
    near(i.speed, 2.9, 0.05);
    // Standing still, it comes down where it went up.
    let still = impact([3.0, 4.0, CLEARANCE], jump([0.0; 3]), flat).unwrap();
    assert_eq!((still.row[0], still.row[1], still.ticks), (3.0, 4.0, 43));
}

/// No landing: from under the floor a jump cannot clear, on a rise that
/// never comes down in time, or with a number that is not one.
#[test]
fn a_flight_that_never_lands_has_no_impact() {
    assert_eq!(impact([0.0, 0.0, -2.0], jump([0.0; 3]), flat), None);
    assert_eq!(impact([0.0, 0.0, 5.0], [0.0, 0.0, 200.0], flat), None);
    assert_eq!(impact([f32::NAN, 0.0, 5.0], [0.0; 3], flat), None);
    assert_eq!(impact([0.0, 0.0, 5.0], [0.0, f32::INFINITY, 0.0], flat), None);
    // From a little under a floor a jump clears, there is one.
    assert!(impact([0.0, 0.0, -0.5], jump([0.0; 3]), flat).is_some());
}

/// The seam's frame and surface: past the edge along the drift, and
/// over it; the body is stopped on the floor well before the edge and by
/// the far face near it, higher the further past.
#[test]
fn the_seam_stops_the_body_on_its_floor_or_its_far_face() {
    let s = seam();
    near3(s.point, [-10.457, 3.976, -3.2]);
    assert_eq!((s.drift, s.along()), ([0.0, 1.0], [1.0, 0.0]));
    let [past, over] = s.frame([-10.3, 3.9, -2.8]);
    near(past, -0.1, 1e-5);
    near(over, 0.4, 1e-5);
    // Past the edge's end the nearest point is the end.
    assert_eq!(s.edge_point([-20.0, 4.0]), [-11.10, 4.0, -3.2]);
    near(s.contact([-10.4, 3.0]), -3.2 + CLEARANCE, 1e-6);
    near(s.contact([-10.4, 4.0]), -3.2 + DEPTH_CUT - DEPTH_BAND, 1e-5);
    near(s.contact([-10.4, 3.96]), -3.2 + DEPTH_CUT - DEPTH_BAND - SEAM_SLOPE * 0.04, 1e-5);
    assert!(s.contact([-10.4, 4.1]) > s.contact([-10.4, 4.0]));
    // A seam with no drift has its floor alone.
    let plain = Seam { drift: [0.0; 2], ..s };
    assert!(!plain.drifts());
    near(plain.contact([-10.4, 4.0]), -3.2 + CLEARANCE, 1e-6);
    assert_eq!(plain.frame([-10.3, 3.9, -2.8])[0], 0.0);
}

/// The depth rule is the note's cut: at y 3.94 a last free row bounces
/// under z −2.80, at y 3.98 under −2.77; the landing window is y 3.93 to
/// 3.99. A seam with no drift has no depth.
#[test]
fn the_depth_rule_is_the_notes_cut() {
    let s = seam();
    let deep = |y, z| depth(&s, &row(y, z)).unwrap();
    for (y, bounced, failed) in [(3.94, -2.81, -2.79), (3.98, -2.78, -2.76)] {
        assert!(deep(y, bounced).deep(), "y {y} z {bounced}");
        assert!(!deep(y, failed).deep(), "y {y} z {failed}");
    }
    // The cut is the note's line: z − 0.75·y = −5.755.
    let d = deep(3.95, -2.80);
    near(d.margin, -5.755 - (-2.80 - 0.75 * 3.95), 1e-4);
    near(d.past, -0.05, 1e-5);
    // The measured rows: the lowest that bounced, the highest that failed.
    assert!(deep(3.95, -2.875).deep() && !deep(3.97, -2.738).deep());
    for y in [3.935, 3.96, 3.985] {
        assert!(deep(y, -2.8).in_window(), "{y}");
    }
    for y in [3.90, 3.92, 3.995, 4.04] {
        assert!(!deep(y, -2.8).in_window(), "{y}");
    }
    assert_eq!(depth(&Seam { drift: [0.0; 2], ..s }, &row(3.95, -2.9)), None);
    // Every measured last free row is over where the seam stops the body.
    for (y, z) in [(3.97, -2.875), (3.99, -2.875), (3.93, -2.875)] {
        assert!(z > s.contact([-10.457, y]), "y {y}");
    }
}

/// Lined up is the catch, either side of the edge; the drift arrow is
/// lit by any speed the drift's way.
#[test]
fn lined_up_is_the_catch_and_the_drift_a_hint() {
    let s = seam();
    for y in [3.91, 3.976, 4.0, 4.09] {
        assert!(caught(&s, &row(y, -2.8)), "{y}");
    }
    for y in [3.85, 4.15] {
        assert!(!caught(&s, &row(y, -2.8)), "{y}");
    }
    // Past the edge's end is off it.
    assert!(!caught(&s, &Impact { row: [-9.5, 3.976, -2.8], ..row(3.976, -2.8) }));
    assert!(drift_lit(0.1) && drift_lit(0.8) && drift_lit(2.0));
    assert!(!drift_lit(0.0) && !drift_lit(-0.3) && !drift_lit(f32::NAN));
}

/// The box's call: off the line; lined up on the ground; in a primed
/// fall at the measured speed, will bounce when deep in the window and
/// too shallow when not deep; and no call where nothing is measured.
#[test]
fn the_call_is_the_depths_in_a_primed_fall() {
    let s = seam();
    let (deep, shallow) = (row(3.95, -2.83), row(3.95, -2.77));
    assert_eq!(call(&s, None, true, true), Call::OffLine);
    assert_eq!(call(&s, Some(&row(3.7, -2.83)), true, true), Call::OffLine);
    assert_eq!(call(&s, Some(&deep), true, true), Call::WillBounce);
    assert_eq!(call(&s, Some(&shallow), true, true), Call::TooShallow);
    // On the ground the landing is a jump's: lined up, no more.
    assert_eq!(call(&s, Some(&deep), false, true), Call::LinedUp);
    assert_eq!(call(&s, Some(&shallow), false, true), Call::LinedUp);
    // Not primed, nothing bounces: no call.
    assert_eq!(call(&s, Some(&deep), true, false), Call::LinedUp);
    assert_eq!(call(&s, Some(&shallow), true, false), Call::LinedUp);
    // Slower than the rule is measured at: no call.
    let slow = Impact { speed: 3.0, ..deep };
    assert_eq!(call(&s, Some(&slow), true, true), Call::LinedUp);
    let edge = Impact { speed: DEPTH_SPEED, ..deep };
    assert_eq!(call(&s, Some(&edge), true, true), Call::WillBounce);
    // Deep but outside the landing window (caught all the same): no
    // promise. Shallow there is still shallow.
    assert_eq!(call(&s, Some(&row(3.91, -2.9)), true, true), Call::LinedUp);
    assert_eq!(call(&s, Some(&row(3.91, -2.6)), true, true), Call::TooShallow);
    // A seam with no drift has no depth to call.
    let plain = Seam { drift: [0.0; 2], ..s };
    assert_eq!(call(&plain, Some(&deep), true, true), Call::LinedUp);
}

/// The fade: full while primed at any distance; not primed, full within
/// `NEAR`, gone at `FAR`.
#[test]
fn the_guide_fades_out_past_a_few_units_unless_primed() {
    assert_eq!(strength(true, 0.5), 1.0);
    assert_eq!(strength(true, 80.0), 1.0);
    assert_eq!(strength(false, 0.5), 1.0);
    assert_eq!(strength(false, NEAR), 1.0);
    near(strength(false, 5.0), 0.5, 1e-6);
    assert_eq!(strength(false, FAR), 0.0);
    assert_eq!(strength(false, 30.0), 0.0);
    assert_eq!(strength(false, f32::NAN), 0.0);
}

/// The seam the guide is on: the one a fall is predicted onto, else
/// while primed the chosen strip, else the nearest target within `FAR`.
#[test]
fn the_guide_is_on_the_seam_the_player_is_going_for() {
    let at = [0.0, 0.0, 0.0];
    let (landing, chosen) = ([9.0, 0.0, -2.0], [20.0, 0.0, 0.0]);
    let targets = [[5.0, 0.0, 0.0], [3.0, 0.0, 0.0], [40.0, 0.0, 0.0]];
    assert_eq!(target(at, true, Some(landing), Some(chosen), &targets), Some(landing));
    assert_eq!(target(at, false, Some(landing), None, &targets), Some(landing));
    assert_eq!(target(at, true, None, Some(chosen), &targets), Some(chosen));
    // Not primed there is no chosen strip: the nearest target in range.
    assert_eq!(target(at, false, None, Some(chosen), &targets), Some(targets[1]));
    assert_eq!(target(at, true, None, None, &targets), Some(targets[1]));
    assert_eq!(target(at, false, None, None, &targets[2..]), None);
    assert_eq!(target(at, false, None, None, &[]), None);
}

/// A seam is found by its landing point among the bounce spots, the
/// first list's before the second's.
#[test]
fn a_seam_is_found_by_its_landing_point() {
    let a = ascension();
    let crouch = Spot { kind: Kind::Crouch, ..a };
    let other = Spot { stand: [1.0, 2.0, 3.0], fall: 2.0, ..a };
    let spots = [crouch, other, a];
    assert_eq!(spot_at(&spots, a.stand), Some(&a));
    assert_eq!(spot_at(&spots, [1.0, 2.0, 3.0]).map(|s| s.fall), Some(2.0));
    assert_eq!(spot_at(&spots, [1.0, 2.0, 3.1]), None);
    assert_eq!(spot_at(spots[..1].iter().chain(&[a]), a.stand), Some(&a));
    assert_eq!(spot_at(&spots[..1], a.stand), None);
}

/// The whole guide in a primed fall onto the seam: the pipper on the
/// seam's floor under the last free row, the box's call from the row's
/// depth, the drift's speed, and the marks' view and layout.
#[test]
fn a_primed_fall_is_called_before_it_lands() {
    let spot = ascension();
    let body = |z: f32, drift: f32| Body {
        position: [-10.457, 3.95, z],
        velocity: [0.0, drift, -5.3],
        airborne: true,
        primed: true,
    };
    // One tick from the seam: this row is the last free one.
    let g = guide(&body(-2.83, 0.3), &spot).unwrap();
    let i = g.impact.unwrap();
    assert_eq!((i.ticks, i.row, i.speed), (0, [-10.457, 3.95, -2.83], 5.3));
    assert_eq!((g.call, g.jump, g.strength), (Call::WillBounce, false, 1.0));
    assert!(g.depth.unwrap().deep() && g.caught());
    assert_eq!(g.pipper(), Some([-10.457, 3.95, -3.2]));
    assert_eq!(g.across, Some(0.3));
    let v = g.view();
    assert_eq!((v.call, v.pipper, v.ticks, v.drift), (Call::WillBounce, true, false, Some(true)));
    let l = g.layout();
    near3(l.seam, [-10.457, 3.976, -3.2]);
    assert_eq!((l.along, l.drift), ([1.0, 0.0], [0.0, 1.0]));
    assert_eq!((l.half_along, l.impact, l.strength), (marks::BOX_ALONG, g.pipper(), 1.0));
    // A row 0.06 higher is too shallow, and says so; still, the arrow is
    // unlit.
    let g = guide(&body(-2.77, 0.0), &spot).unwrap();
    assert_eq!(g.impact.unwrap().ticks, 0);
    assert_eq!(g.call, Call::TooShallow);
    assert_eq!(g.view().drift, Some(false));
    // From higher up the same fall is stepped down to its last free row:
    // somewhere in the tick's fall over the seam, called either way.
    let high = guide(&body(0.4, 0.0), &spot).unwrap();
    let i = high.impact.unwrap();
    assert!(i.ticks > 10 && i.speed > 6.0, "{i:?}");
    let over = i.row[2] - high.seam.contact([i.row[0], i.row[1]]);
    assert!(over > 0.0 && over <= (i.speed + GRAVITY * TICK) * TICK + 1e-4, "{over}");
    assert!(matches!(high.call, Call::WillBounce | Call::TooShallow));
    assert_eq!(high.depth.unwrap().deep(), high.call == Call::WillBounce);
}

/// On the ground the guide is a jump's landing from here: never called
/// for depth, lined up when the jump comes down in the catch, with ticks
/// toward the box when it does not. Not primed it fades with distance,
/// and a crouch spot has no guide.
#[test]
fn on_the_ground_the_guide_is_a_jump_from_here() {
    let spot = ascension();
    let standing = |y: f32, vy: f32, primed: bool| Body {
        position: [-10.457, y, -3.2 + CLEARANCE],
        velocity: [0.0, vy, 0.0],
        airborne: false,
        primed,
    };
    // Standing still just before the seam: the jump comes down where it
    // went up, in the catch.
    let g = guide(&standing(3.95, 0.0, true), &spot).unwrap();
    assert!(g.jump && g.caught());
    assert_eq!(g.call, Call::LinedUp);
    assert_eq!(g.pipper(), Some([-10.457, 3.95, -3.2]));
    assert!(!g.view().ticks);
    // A unit back, the same jump lands a unit short: off the line, with
    // ticks toward the box.
    let g = guide(&standing(2.95, 0.0, true), &spot).unwrap();
    assert_eq!(g.call, Call::OffLine);
    assert!(g.view().pipper && g.view().ticks);
    // Not primed: full strength near, fading further out, then nothing.
    assert_eq!(guide(&standing(1.0, 0.0, false), &spot).unwrap().strength, 1.0);
    let fading = guide(&standing(-1.024, 0.0, false), &spot).unwrap();
    near(fading.strength, 0.5, 1e-3);
    assert_eq!(guide(&standing(-4.0, 0.0, false), &spot), None);
    // Primed it shows from across the map, with no pipper that far out.
    let far = guide(&standing(-40.0, 0.0, true), &spot).unwrap();
    assert_eq!((far.strength, far.impact, far.call), (1.0, None, Call::OffLine));
    assert!(!far.view().pipper && !far.view().ticks);
    // A crouch spot is not a seam.
    let crouch = Spot { kind: Kind::Crouch, ..spot };
    assert_eq!(guide(&standing(3.95, 0.0, true), &crouch), None);
}

/// The box is as long as the catch along a short edge, the full box on a
/// busy point, and a seam with no drift has no arrow.
#[test]
fn the_box_fits_the_edge() {
    let body = Body {
        position: [0.0, -0.5, CLEARANCE],
        velocity: [0.0; 3],
        airborne: false,
        primed: true,
    };
    let short = Spot {
        kind: Kind::Bounce,
        edge: [[-0.1, 0.0, 0.0], [0.1, 0.0, 0.0]],
        stand: [0.0; 3],
        ..Spot::default()
    };
    let g = guide(&body, &short).unwrap();
    near(g.layout().half_along, 0.1, 1e-6);
    assert_eq!((g.across, g.depth, g.view().drift), (None, None, None));
    let point = Spot { edge: [[0.0; 3]; 2], ..short };
    assert_eq!(guide(&body, &point).unwrap().layout().half_along, marks::BOX_ALONG);
    // The marks' poses: no arrow without a drift.
    let poses = marks::poses(&g.layout());
    assert!(poses.iter().any(|(id, _)| *id == marks::BOX));
    assert!(!poses.iter().any(|(id, _)| *id == marks::DRIFT));
}

/// The agent's reply carries the guide's call and its numbers, and the
/// lock-on's brackets give way to the guide's box fullscreen only.
#[test]
fn the_agent_reply_carries_the_call_and_the_depth() {
    use crate::superbounce_mode::{State, guide_reply, reply};
    let body = Body {
        position: [-10.457, 3.95, -2.83],
        velocity: [0.0, 0.3, -5.3],
        airborne: true,
        primed: true,
    };
    let g = guide(&body, &ascension()).unwrap();
    let r = guide_reply(&g);
    assert_eq!((r.call.as_str(), r.jump, r.caught), ("will_bounce", false, true));
    assert_eq!((r.ticks, r.deep, r.strength), (Some(0), Some(true), 1.0));
    near(r.speed.unwrap() as f32, 5.3, 1e-5);
    near(r.depth.unwrap() as f32, DEPTH_CUT - (0.37 + 0.75 * 0.05), 1e-4);
    near(r.past.unwrap() as f32, -0.05, 1e-4);
    near(r.across.unwrap() as f32, 0.3, 1e-6);
    near(r.row.unwrap()[2] as f32, -2.83, 1e-6);
    near(r.impact.unwrap()[2] as f32, -3.2, 1e-6);
    near(r.seam[1] as f32, 3.976, 1e-4);
    // The chance: a fall's confidence; on this measured seam it is not
    // capped. A fall has no jump cue and no takeoff line.
    near(r.chance.unwrap() as f32, 0.929, 2e-3);
    assert!(!r.chance_capped);
    assert_eq!((r.jump_in, r.cue.as_deref(), r.takeoff.is_some()), (None, None, false));
    let scan = Spot { landed: false, ..ascension() };
    let capped = guide_reply(&guide(&body, &scan).unwrap());
    near(capped.chance.unwrap() as f32, SCAN_ONLY_CAP, 1e-6);
    assert!(capped.chance_capped);
    // On the ground: no chance, but the ticks to `Jump now`, the cue as
    // the box reads it, and the takeoff line along the walk.
    let w = guide_reply(&guide(&walking(3.205), &ascension()).unwrap());
    assert!(w.jump);
    assert_eq!((w.chance, w.chance_capped), (None, false));
    let n = w.jump_in.unwrap();
    assert!((2..=4).contains(&n), "{n}");
    assert_eq!(w.cue, Some(marks::cue_text(Cue::In(marks::tenths(n)))));
    let t = w.takeoff.unwrap();
    assert!(!t.walk);
    assert_eq!(t.heading, [0.0, 1.0]);
    near(t.speed as f32, 0.3, 1e-6);
    near(t.at[2] as f32, 0.25 - CLEARANCE, 1e-6);
    // Not primed there is none of it.
    let idle = guide_reply(&guide(&Body { primed: false, ..body }, &ascension()).unwrap());
    assert_eq!((idle.chance, idle.chance_capped), (None, false));
    assert_eq!((idle.jump_in, idle.cue, idle.takeoff), (None, None, None));
    let shallow = guide(&Body { position: [-10.457, 3.95, -2.77], ..body }, &ascension()).unwrap();
    assert_eq!(guide_reply(&shallow).call, "too_shallow");
    assert_eq!(guide_reply(&shallow).deep, Some(false));
    // The state's reply has it while the guide shows.
    let s = State {
        active: true,
        guide: Some(g),
        ..State::default()
    };
    assert_eq!(reply(&s).guide, Some(r));
    assert_eq!(reply(&State::default()).guide, None);
    // The lock-on has no landing here to bracket either way.
    assert_eq!(crate::superbounce_world::lock_shown(&s, true), None);
    assert_eq!(crate::superbounce_world::lock_shown(&s, false), None);
}

/// A body on a ledge 3.45 over Ascension's seam, walking at 0.3 units/s
/// along its drift from `y`: a jump from anywhere on the walk lands 64
/// ticks later, 0.64 on, at 5.82 units/s.
fn walking(y: f32) -> Body {
    Body {
        position: [-10.457, y, 0.25],
        velocity: [0.0, 0.3, 0.0],
        airborne: false,
        primed: true,
    }
}

/// The ground a run at `speed` covers in `ticks`.
fn run(speed: f32, ticks: u32) -> f32 {
    speed * ticks as f32 * TICK
}

/// Whether a jump from `from` at `velocity` bounces on `s`, as the box
/// would call its fall.
fn bounces(s: &Seam, from: Vec3, velocity: Vec3) -> bool {
    impact(from, velocity, |xy| s.contact(xy))
        .is_some_and(|i| call(s, Some(&i), true, true) == Call::WillBounce)
}

/// The takeoff line on a known line: walking at 0.3 from y 3.205, the
/// jumps that bounce leave the floor from y 3.29 to 3.35 (the landing
/// window, y 3.93 to 3.99, less the 0.64 carry), the jumps of ticks 9 to
/// 14. The line is where to press for those: 5 ticks (0.05) short, from
/// 0.035 to 0.095 ahead, on the ledge under the body.
#[test]
fn the_takeoff_line_is_solved_along_the_path() {
    let s = seam();
    let b = walking(3.205);
    let t = takeoff(&s, b.position, b.velocity).unwrap();
    assert!(!t.walk());
    assert_eq!(t.heading, [0.0, 1.0]);
    near(t.speed, 0.3, 1e-6);
    let lead = run(0.3, PRESS_LEAD);
    near(lead, 0.05, 1e-6);
    let [lo, hi] = t.ahead.unwrap();
    near(3.205 + lo + lead, 3.29, 0.008);
    near(3.205 + hi + lead, 3.35, 0.008);
    near3(t.at, [-10.457, 3.205 + (lo + hi) / 2.0, 0.25 - CLEARANCE]);
    near(t.half, (hi - lo) / 2.0, 1e-6);
    // A jump on each of the next 20 ticks bounces when it leaves the
    // floor inside the band, and only then: ticks 9 to 14.
    let up = jump(b.velocity);
    for k in 0..20 {
        let y = 3.205 + 0.01 * k as f32;
        let inside = (lo + lead..=hi + lead).contains(&(y - 3.205));
        assert_eq!(bounces(&s, [-10.457, y, 0.25], up), inside, "tick {k}");
        assert_eq!(inside, (9..=14).contains(&k), "tick {k}");
    }
    // A press on the line's centre leaves the floor the lead on, and
    // bounces.
    assert!(bounces(&s, [t.at[0], t.at[1] + lead, 0.25], up));
    // Twice as fast from a ledge 3.0 over the seam the band is as wide
    // (the landing moves with the takeoff), the lead twice as long.
    let brisk = takeoff(&s, [-10.457, 2.505, -0.2], [0.0, 0.6, 0.0]).unwrap();
    let [lo, hi] = brisk.ahead.unwrap();
    assert!(hi - lo > 0.04 && hi - lo < 0.08, "{lo} {hi}");
    let at = [brisk.at[0], brisk.at[1] + run(0.6, PRESS_LEAD), -0.2];
    assert!(bounces(&s, at, jump([0.0, 0.6, 0.0])));
    // Wide of the edge's end the same walk has no band ahead: the
    // walk-up's line, up the drift to the seam's point.
    let wide = takeoff(&s, [-9.5, 3.205, 0.25], [0.0, 0.3, 0.0]).unwrap();
    assert!(wide.walk());
    near(wide.at[0], -10.457, 1e-3);
}

/// The coarse pass, which tries every step only near the landing
/// window, finds the band a step-by-step scan finds.
#[test]
fn the_coarse_pass_finds_the_same_band() {
    let s = seam();
    for (from, v, some) in [
        ([-10.457, 2.9, 0.25], [0.0, 0.3, 0.0], true),
        ([-10.457, 2.0, -0.2], [0.0, 0.6, 0.0], true),
        // At a run (2 u/s) the window is under a tick wide: the band may
        // be empty, and only the two passes' agreement is checked.
        ([-10.457, 1.0, 0.25], [0.0, 2.0, 0.0], false),
        ([-10.9, 1.5, 0.25], [0.2, 0.9, 0.0], false),
        ([-10.457, 3.6, 0.25], [0.0, -0.5, 0.0], false),
    ] {
        let speed = f32::hypot(v[0], v[1]);
        let dir = [v[0] / speed, v[1] / speed];
        let range = [-0.3, speed * 3.0 + 0.3];
        let fine = band_by(&s, from, dir, jump(v), range, 0.0, false);
        assert_eq!(band_by(&s, from, dir, jump(v), range, 0.0, true), fine, "{from:?} {v:?}");
        assert!(!some || fine.is_some(), "{from:?} {v:?}");
    }
}

/// The count is the walk to the line: from y 3.105 the line's near end
/// is 0.135 on, 14 ticks at 0.3. The count falls one a tick, and reaches
/// `Jump now` as the body comes within a tick ([`NOW_EARLY`]) of the
/// line, after `Jump in 0.1 s`; it stays `Jump now` through the band,
/// and the line stays put. Past the band the line is the walk-up's.
#[test]
fn the_count_reaches_jump_now_at_the_line() {
    let s = seam();
    let step = run(0.3, 1);
    let (mut last, mut first_now, mut passed) = (None::<(u32, Vec3, Cue)>, None, false);
    let (mut shown, mut held) = (None, 0);
    for k in 0..30u32 {
        let b = walking(3.105 + step * k as f32);
        let Some(t) = takeoff(&s, b.position, b.velocity) else { break };
        if t.walk() {
            passed = true;
            break;
        }
        let n = t.jump_in().unwrap();
        let [near_end, far_end] = t.ahead.unwrap();
        assert!(far_end >= 0.0, "tick {k}");
        if let Some((m, at, _)) = last {
            assert!((t.at[1] - at[1]).abs() <= TAKEOFF_STEP + 1e-4, "tick {k}: the line moved");
            assert_eq!(n, m.saturating_sub(1), "tick {k}");
        }
        if n > 0 {
            assert!(near_end > step * NOW_EARLY as f32 - 1e-4, "tick {k}: {near_end}");
            assert_eq!(t.cue(), Cue::In(marks::tenths(n)), "tick {k}");
        } else {
            assert_eq!(t.cue(), Cue::Now, "tick {k}");
            if first_now.is_none() {
                first_now = Some(k);
                assert!(near_end <= step * NOW_EARLY as f32 + 1e-4, "{near_end}");
                assert_eq!(last.map(|l| l.2), Some(Cue::In(1)), "0.1 s before");
                // `Jump now` shows `NOW_EARLY` ticks ahead of the band:
                // a press that many ticks on, or one more, leaves the
                // floor in it, and bounces.
                for late in NOW_EARLY..=NOW_EARLY + 1 {
                    let y = b.position[1] + step * late as f32 + run(0.3, PRESS_LEAD);
                    assert!(bounces(&s, [-10.457, y, 0.25], jump(b.velocity)), "{late}");
                }
            }
        }
        // Settled, a steady count is the same count.
        (shown, held) = settle(shown, held, Some(t.cue()));
        assert_eq!(shown, Some(t.cue()), "tick {k}");
        last = Some((n, t.at, t.cue()));
    }
    let first = first_now.expect("Jump now");
    assert!((12..=14).contains(&first), "{first}");
    assert!(passed, "past the band the line is the walk-up's");
}

/// Standing, the line is a walk-up's: up the seam's drift at the walk
/// speed (0.8), no count, `Walk to line`. A jump at 0.8 carries 1.71
/// in its 64 ticks, so the takeoffs that bounce are about y 2.22 to 2.28,
/// and the line is the lead (0.13) short of them. Moving away is the
/// same; from the seam's own floor, and on a seam with no drift, there
/// is no line.
#[test]
fn standing_the_line_is_a_walk_ups() {
    let s = seam();
    let still = Body {
        velocity: [0.0; 3],
        ..walking(2.6)
    };
    let t = takeoff(&s, still.position, still.velocity).unwrap();
    assert!(t.walk());
    assert_eq!((t.jump_in(), t.cue()), (None, Cue::Walk));
    assert_eq!((t.heading, t.speed), ([0.0, 1.0], WALK_UP));
    let lead = run(WALK_UP, PRESS_LEAD);
    near(t.at[0], -10.457, 1e-3);
    near(t.at[1] + lead, 2.253, 0.04);
    near(t.at[2], 0.25 - CLEARANCE, 1e-6);
    assert!(t.half > 0.0 && t.half < 0.05, "{}", t.half);
    // A walk-up pressing on the line's centre bounces.
    let walk = [0.0, WALK_UP, JUMP_VZ];
    assert!(bounces(&s, [t.at[0], t.at[1] + lead, 0.25], walk));
    // Where on the ledge the body stands does not move it.
    let other = takeoff(&s, [-10.457, 1.0, 0.25], [0.0; 3]).unwrap();
    near3(other.at, t.at);
    // Moving away: the walk-up's line, no count.
    let away = takeoff(&s, still.position, [0.0, -0.3, 0.0]).unwrap();
    assert!(away.walk());
    near3(away.at, t.at);
    // Creeping slower than `MOVING` is standing.
    let creep = takeoff(&s, still.position, [0.0, MOVING / 2.0, 0.0]).unwrap();
    assert!(creep.walk());
    // From the seam's own floor no jump bounces; a seam with no drift
    // has no depth rule.
    assert_eq!(takeoff(&s, [-10.457, 3.9, -3.2 + CLEARANCE], [0.0; 3]), None);
    assert_eq!(takeoff(&Seam { drift: [0.0; 2], ..s }, still.position, [0.0; 3]), None);
    assert_eq!(takeoff(&s, [f32::NAN, 2.6, 0.25], [0.0; 3]), None);
}

/// The cue holds steady: a count down shows as it is; a step a tenth
/// back up, `Jump now` against 0.1, and a count against a missing line
/// are held for `CUE_GRACE` ticks at most; anything else shows at once,
/// and `Jump now` never outlasts its band.
#[test]
fn the_cue_holds_steady() {
    use Cue::{In, Now, Walk};
    let shown = |raw: &[Option<Cue>]| {
        let (mut c, mut h) = (None, 0);
        raw.iter()
            .map(|r| {
                (c, h) = settle(c, h, *r);
                c
            })
            .collect::<Vec<_>>()
    };
    let counting = [Some(In(5)), Some(In(5)), Some(In(4)), Some(In(4)), Some(In(3))];
    assert_eq!(shown(&counting), counting);
    // Jitter a tenth up and back down is held.
    let jitter = [Some(In(5)), Some(In(6)), Some(In(5)), Some(In(6)), Some(In(5))];
    assert_eq!(shown(&jitter), [Some(In(5)); 5]);
    // A step up that lasts shows after `CUE_GRACE` ticks.
    assert_eq!(CUE_GRACE, 3);
    let up = [Some(In(5)), Some(In(6)), Some(In(6)), Some(In(6)), Some(In(6))];
    assert_eq!(shown(&up), [Some(In(5)), Some(In(5)), Some(In(5)), Some(In(5)), Some(In(6))]);
    // A bigger step is a new line: at once.
    assert_eq!(shown(&[Some(In(5)), Some(In(9))]), [Some(In(5)), Some(In(9))]);
    // `Jump now` holds against a count of 0.1 flickering back.
    let flicker = [Some(In(1)), Some(Now), Some(In(1)), Some(Now)];
    assert_eq!(shown(&flicker), [Some(In(1)), Some(Now), Some(Now), Some(Now)]);
    // A count holds through a dropped tick, but not for good.
    assert_eq!(shown(&[Some(In(4)), None, Some(In(3))]), [Some(In(4)), Some(In(4)), Some(In(3))]);
    let gone = [Some(In(4)), None, None, None, None];
    assert_eq!(shown(&gone), [Some(In(4)), Some(In(4)), Some(In(4)), Some(In(4)), None]);
    // `Jump now` goes with its band, and a walk-up or a count shows at
    // once.
    assert_eq!(shown(&[Some(Now), None]), [Some(Now), None]);
    assert_eq!(shown(&[Some(Now), Some(Walk)]), [Some(Now), Some(Walk)]);
    assert_eq!(shown(&[Some(Walk), Some(In(20))]), [Some(Walk), Some(In(20))]);
}

/// On the ground while primed the guide carries the takeoff line and
/// its cue, which the marks show (`Jump in 0.4 s` or so on the box, the
/// line drawn) in place of a percentage; the cue settles against the
/// last tick's on the same seam only. Not primed, and in the air, there
/// is neither.
#[test]
fn the_guide_carries_the_cue_on_the_ground() {
    use hud_kit_core::model::Values;
    let spot = ascension();
    let g = guide(&walking(3.105), &spot).unwrap().settled(None);
    let t = g.takeoff.unwrap();
    assert_eq!(g.cue, Some(t.cue()));
    assert!(matches!(g.cue, Some(Cue::In(4..=5))), "{:?}", g.cue);
    assert_eq!(g.chance, None);
    let v = g.view();
    assert!(v.takeoff);
    assert_eq!((v.chance, v.cue), (None, g.cue));
    let l = g.layout().takeoff.unwrap();
    assert_eq!((l.at, l.heading, l.half), (t.at, t.heading, t.half));
    let mut out = Values::default();
    marks::set_values(&mut out, Some(&v));
    assert!(out.get(marks::BOX).unwrap().text.starts_with("Jump in 0."));
    assert!(out.get(marks::TAKEOFF).unwrap().shown);
    // Settled: a tenth back up is held, on the same seam alone.
    let Some(Cue::In(n)) = g.cue else { unreachable!() };
    let jitter = Guide {
        cue: Some(Cue::In(n + 1)),
        ..g
    };
    let held = jitter.settled(Some(&g));
    assert_eq!((held.cue, held.held), (Some(Cue::In(n)), 1));
    let elsewhere = Guide {
        seam: Seam {
            edge: [[0.0; 3], [1.0, 0.0, 0.0]],
            ..g.seam
        },
        ..g
    };
    assert_eq!(jitter.settled(Some(&elsewhere)).cue, Some(Cue::In(n + 1)));
    // Not primed: no line, no cue.
    let idle = guide(&Body { primed: false, ..walking(3.105) }, &spot).unwrap();
    assert_eq!((idle.takeoff, idle.cue), (None, None));
    // In the air: the fall's chance instead.
    let falling = Body {
        position: [-10.457, 3.95, -2.83],
        velocity: [0.0, 0.3, -5.3],
        airborne: true,
        primed: true,
    };
    let f = guide(&falling, &spot).unwrap().settled(Some(&g));
    assert_eq!((f.takeoff, f.cue), (None, None));
    assert!(f.chance.is_some());
}


/// In the air the chance is a confidence: 50% on the depth cut and on
/// either end of the landing window, about 95% deep and centred, low
/// when clearly shallow or off the window, rising all the way between.
#[test]
fn the_air_confidence_is_half_on_the_cut_and_rises_with_the_margin() {
    let at = |margin: f32, past: f32| confidence(&Depth { margin, past }, true);
    let low = 1.0 - CONFIDENCE_TOP;
    // On the cut in the window's middle, and deep on either end of the
    // window.
    near(at(0.0, -0.04), 0.5, 1e-6);
    near(at(0.08, LANDING_WINDOW[0]), 0.5, 1e-6);
    near(at(0.08, LANDING_WINDOW[1]), 0.5, 1e-6);
    // Deep and centred.
    let top = at(0.07, -0.04);
    assert!(top > 0.94 && top <= CONFIDENCE_TOP, "{top}");
    // The cut is measured to 0.003: that far either side of it is just
    // either side of half, the same distance from it.
    let (under, over) = (at(0.003, -0.04), at(-0.003, -0.04));
    assert!(under > 0.5 && under < 0.6 && over < 0.5 && over > 0.4, "{under} {over}");
    near(under + over, 1.0, 1e-6);
    // One softness under the cut, and one inside an end.
    near(at(DEPTH_SOFT, -0.04), 0.5 + 0.45 * 1f32.tanh(), 1e-5);
    near(at(0.2, LANDING_WINDOW[0] + WINDOW_SOFT), 0.5 + 0.45 * 1f32.tanh(), 1e-4);
    // Clearly shallow, and clearly off the window: low.
    assert!(at(-0.05, -0.04) < 0.06 && at(0.08, -0.10) < 0.06 && at(0.08, 0.03) < 0.06);
    // Outside the catch: the bottom, whatever the depth.
    near(confidence(&Depth { margin: 0.08, past: -0.04 }, false), low, 1e-6);
    near(confidence(&Depth { margin: f32::NAN, past: -0.04 }, true), low, 1e-6);
    near(confidence(&Depth { margin: 0.08, past: f32::NAN }, true), low, 1e-6);
    // It never falls as the row goes deeper, rises through the cut, and
    // stays inside the bottom and the top.
    let margins: Vec<f32> = (-20..=27).map(|k| k as f32 * 0.005).collect();
    for pair in margins.windows(2) {
        let (a, b) = (at(pair[0], -0.04), at(pair[1], -0.04));
        assert!(b >= a, "{pair:?}: {a} {b}");
        if pair[0].abs() <= 0.045 && pair[1].abs() <= 0.045 {
            assert!(b > a, "{pair:?}: {a} {b}");
        }
        assert!((low..=CONFIDENCE_TOP).contains(&a) && (low..=CONFIDENCE_TOP).contains(&b));
    }
    // Nor as the row moves from an end of the window to its middle.
    for end in [0usize, 1] {
        let sign = if end == 0 { 1.0 } else { -1.0 };
        let mut last = at(0.08, LANDING_WINDOW[end] - sign * 0.04);
        for k in -7..=6 {
            let c = at(0.08, LANDING_WINDOW[end] + sign * k as f32 * 0.005);
            assert!(c >= last, "end {end} step {k}: {last} {c}");
            last = c;
        }
    }
    // The call and the confidence agree: will bounce is half or over,
    // too shallow half or under.
    for (margin, past) in [(0.001, -0.069), (0.05, -0.011), (0.12, -0.04)] {
        let d = Depth { margin, past };
        assert!(d.deep() && d.in_window() && confidence(&d, true) >= 0.5, "{d:?}");
    }
    for (margin, past) in [(-0.001, -0.04), (-0.05, -0.02), (-0.001, -0.09)] {
        assert!(confidence(&Depth { margin, past }, true) <= 0.5);
    }
}

/// The seam's evidence caps the chance: as it is on a seam with a
/// measured landing, 60% at most on a scan's.
#[test]
fn a_scan_only_seam_is_capped() {
    assert_eq!(SCAN_ONLY_CAP, 0.6);
    assert_eq!(cap(0.95, true), (0.95, false));
    assert_eq!(cap(1.0, true), (1.0, false));
    assert_eq!(cap(0.95, false), (SCAN_ONLY_CAP, true));
    assert_eq!(cap(0.61, false), (SCAN_ONLY_CAP, true));
    // At or under the cap nothing is cut.
    assert_eq!(cap(SCAN_ONLY_CAP, false), (SCAN_ONLY_CAP, false));
    assert_eq!(cap(0.3, false), (0.3, false));
    assert_eq!(cap(0.0, false), (0.0, false));
    // The seam takes its evidence from the spot.
    assert!(seam().measured);
    let scan = Spot { landed: false, ..ascension() };
    assert!(!Seam::of(&scan, [-10.457, 3.976]).measured);
    // A primed fall, deep in the window: 93% on the measured seam, the
    // cap on the scan's. The call is the same on both.
    let body = |z: f32| Body {
        position: [-10.457, 3.95, z],
        velocity: [0.0, 0.3, -5.3],
        airborne: true,
        primed: true,
    };
    let (m, s) = (guide(&body(-2.83), &ascension()).unwrap(), guide(&body(-2.83), &scan).unwrap());
    assert_eq!((m.call, s.call), (Call::WillBounce, Call::WillBounce));
    let (mc, sc) = (m.chance.unwrap(), s.chance.unwrap());
    near(mc.value, 0.929, 2e-3);
    assert!(!mc.capped);
    assert_eq!((sc.value, sc.capped), (SCAN_ONLY_CAP, true));
    assert_eq!((m.view().chance, s.view().chance), (Some(93), Some(60)));
    // Too shallow is under the cap on either: the same on both.
    let (m, s) = (guide(&body(-2.77), &ascension()).unwrap(), guide(&body(-2.77), &scan).unwrap());
    assert_eq!(m.call, Call::TooShallow);
    near(m.chance.unwrap().value, 0.136, 2e-3);
    assert_eq!(m.chance, s.chance);
    assert_eq!(m.view().chance, Some(14));
    // A fall slower than the depth rule is measured at, and one not
    // primed, have no chance.
    let slow = Body { velocity: [0.0, 0.3, -3.0], ..body(-2.83) };
    assert_eq!(guide(&slow, &ascension()).unwrap().chance, None);
    let idle = Body { primed: false, ..body(-2.83) };
    assert_eq!(guide(&idle, &ascension()).unwrap().chance, None);
}
