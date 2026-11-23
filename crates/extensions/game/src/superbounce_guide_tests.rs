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
    assert_eq!(
        impact([0.0, 0.0, 5.0], [0.0, f32::INFINITY, 0.0], flat),
        None
    );
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
    near(
        s.contact([-10.4, 3.96]),
        -3.2 + DEPTH_CUT - DEPTH_BAND - SEAM_SLOPE * 0.04,
        1e-5,
    );
    assert!(s.contact([-10.4, 4.1]) > s.contact([-10.4, 4.0]));
    // A seam with no drift has its floor alone.
    let plain = Seam {
        drift: [0.0; 2],
        ..s
    };
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
    assert_eq!(
        depth(
            &Seam {
                drift: [0.0; 2],
                ..s
            },
            &row(3.95, -2.9)
        ),
        None
    );
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
    assert!(!caught(
        &s,
        &Impact {
            row: [-9.5, 3.976, -2.8],
            ..row(3.976, -2.8)
        }
    ));
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
    let edge = Impact {
        speed: DEPTH_SPEED,
        ..deep
    };
    assert_eq!(call(&s, Some(&edge), true, true), Call::WillBounce);
    // Deep but outside the landing window (caught all the same): no
    // promise. Shallow there is still shallow.
    assert_eq!(call(&s, Some(&row(3.91, -2.9)), true, true), Call::LinedUp);
    assert_eq!(
        call(&s, Some(&row(3.91, -2.6)), true, true),
        Call::TooShallow
    );
    // A seam with no drift has no depth to call.
    let plain = Seam {
        drift: [0.0; 2],
        ..s
    };
    assert_eq!(call(&plain, Some(&deep), true, true), Call::LinedUp);
}

/// The guide is for a primed player alone: full strength primed, none
/// otherwise, at any distance.
#[test]
fn the_guide_shows_while_primed_only() {
    assert_eq!(strength(true), 1.0);
    assert_eq!(strength(false), 0.0);
    let falling = Body {
        position: [-10.457, 3.95, -2.83],
        velocity: [0.0, 0.3, -5.3],
        airborne: true,
        primed: false,
    };
    assert_eq!(guide(&falling, &ascension()), None);
    let standing = Body {
        position: [-10.457, 3.0, -3.2 + CLEARANCE],
        velocity: [0.0; 3],
        airborne: false,
        primed: false,
    };
    assert_eq!(guide(&standing, &ascension()), None);
    assert_eq!(
        guide(
            &Body {
                primed: true,
                ..standing
            },
            &ascension()
        )
        .unwrap()
        .strength,
        1.0
    );
}

/// The seam the guide is on: the one a fall is predicted onto, else
/// while primed the chosen strip, else the nearest target within `FAR`.
#[test]
fn the_guide_is_on_the_seam_the_player_is_going_for() {
    let at = [0.0, 0.0, 0.0];
    let (landing, chosen) = ([9.0, 0.0, -2.0], [20.0, 0.0, 0.0]);
    let targets = [[5.0, 0.0, 0.0], [3.0, 0.0, 0.0], [40.0, 0.0, 0.0]];
    assert_eq!(
        target(at, true, Some(landing), Some(chosen), &targets),
        Some(landing)
    );
    assert_eq!(
        target(at, false, Some(landing), None, &targets),
        Some(landing)
    );
    assert_eq!(target(at, true, None, Some(chosen), &targets), Some(chosen));
    // Not primed there is no chosen strip: the nearest target in range.
    assert_eq!(
        target(at, false, None, Some(chosen), &targets),
        Some(targets[1])
    );
    assert_eq!(target(at, true, None, None, &targets), Some(targets[1]));
    assert_eq!(target(at, false, None, None, &targets[2..]), None);
    assert_eq!(target(at, false, None, None, &[]), None);
}

/// A seam is found by its landing point among the bounce spots, the
/// first list's before the second's.
#[test]
fn a_seam_is_found_by_its_landing_point() {
    let a = ascension();
    let crouch = Spot {
        kind: Kind::Crouch,
        ..a
    };
    let other = Spot {
        stand: [1.0, 2.0, 3.0],
        fall: 2.0,
        ..a
    };
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
    assert_eq!(
        (v.call, v.pipper, v.ticks, v.drift),
        (Call::WillBounce, true, false, Some(true))
    );
    let l = g.layout();
    near3(l.seam, [-10.457, 3.976, -3.2]);
    assert_eq!((l.along, l.drift), ([1.0, 0.0], [0.0, 1.0]));
    assert_eq!(
        (l.half_along, l.impact, l.strength),
        (marks::BOX_ALONG, g.pipper(), 1.0)
    );
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
    assert!(
        over > 0.0 && over <= (i.speed + GRAVITY * TICK) * TICK + 1e-4,
        "{over}"
    );
    assert!(matches!(high.call, Call::WillBounce | Call::TooShallow));
    assert_eq!(high.depth.unwrap().deep(), high.call == Call::WillBounce);
}

/// On the ground the floor underfoot is not the seam's: with no takeoff
/// counting, the guide has no pipper and no ticks, and calls nothing
/// (frames 72, 137 and 159 of the review: a ring and dashes on the wrong
/// floor). Primed it shows from across the map; a crouch spot has no
/// guide.
#[test]
fn on_the_ground_there_is_no_pipper_without_a_count() {
    let spot = ascension();
    let standing = |y: f32, vy: f32| Body {
        position: [-10.457, y, -3.2 + CLEARANCE],
        velocity: [0.0, vy, 0.0],
        airborne: false,
        primed: true,
    };
    for (y, vy) in [(3.95, 0.0), (2.95, 0.0), (2.95, 0.5), (-40.0, 0.0)] {
        let g = guide(&standing(y, vy), &spot).unwrap();
        assert!(g.jump);
        assert_eq!((g.impact, g.call, g.strength), (None, Call::OffLine, 1.0));
        assert!(!g.view().pipper && !g.view().ticks);
    }
    // A crouch spot is not a seam.
    let crouch = Spot {
        kind: Kind::Crouch,
        ..spot
    };
    assert_eq!(guide(&standing(3.95, 0.0), &crouch), None);
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
    let point = Spot {
        edge: [[0.0; 3]; 2],
        ..short
    };
    assert_eq!(
        guide(&body, &point).unwrap().layout().half_along,
        marks::BOX_ALONG
    );
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
    assert_eq!(
        (r.call.as_str(), r.jump, r.caught),
        ("will_bounce", false, true)
    );
    assert_eq!((r.ticks, r.deep, r.strength), (Some(0), Some(true), 1.0));
    near(r.speed.unwrap() as f32, 5.3, 1e-5);
    near(
        r.depth.unwrap() as f32,
        DEPTH_CUT - (0.37 + 0.75 * 0.05),
        1e-4,
    );
    near(r.past.unwrap() as f32, -0.05, 1e-4);
    near(r.across.unwrap() as f32, 0.3, 1e-6);
    near(r.row.unwrap()[2] as f32, -2.83, 1e-6);
    near(r.impact.unwrap()[2] as f32, -3.2, 1e-6);
    near(r.seam[1] as f32, 3.976, 1e-4);
    // The chance: a fall's confidence; on this measured seam it is not
    // capped. A fall has no jump cue and no takeoff line.
    near(r.chance.unwrap() as f32, 0.929, 2e-3);
    assert!(!r.chance_capped);
    assert_eq!(
        (r.jump_in, r.cue.as_deref(), r.takeoff.is_some()),
        (None, None, false)
    );
    let scan = Spot {
        landed: false,
        ..ascension()
    };
    let capped = guide_reply(&guide(&body, &scan).unwrap());
    near(capped.chance.unwrap() as f32, SCAN_ONLY_CAP, 1e-6);
    assert!(capped.chance_capped);
    // On the ground, running at a jump's line: no chance, but the ticks
    // to `Jump now`, the cue's words and the takeoff.
    let t = jump_line();
    let w = guide_reply(&guide_from(&running(&t, 0.5), &ascension(), Some(t)).unwrap());
    assert!(w.jump);
    assert_eq!((w.chance, w.chance_capped), (None, false));
    let n = w.jump_in.unwrap();
    assert!((15..=19).contains(&n), "{n}");
    assert_eq!(w.cue, Some(Cue::JumpIn(tenths(n)).text()));
    let l = w.takeoff.unwrap();
    assert!(!l.walk);
    assert_eq!(l.heading, [0.0, 1.0]);
    near(l.speed as f32, WALK_UP, 1e-6);
    near(l.at[2] as f32, LEDGE, 1e-6);
    // Standing, the line is not counted to: `walk`, `Walk to line`.
    let still = Body {
        velocity: [0.0; 3],
        ..running(&t, 0.5)
    };
    let w = guide_reply(&guide_from(&still, &ascension(), Some(t)).unwrap());
    assert_eq!((w.jump_in, w.cue.as_deref()), (None, Some("Walk to line")));
    assert!(w.takeoff.unwrap().walk);
    // Not primed there is no guide.
    assert_eq!(
        guide(
            &Body {
                primed: false,
                ..body
            },
            &ascension()
        ),
        None
    );
    let shallow = guide(
        &Body {
            position: [-10.457, 3.95, -2.77],
            ..body
        },
        &ascension(),
    )
    .unwrap();
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

/// The ledge's floor, 3.45 over Ascension's seam for a body at z 0.25.
const LEDGE: f32 = 0.25 - CLEARANCE;

/// A ledge at [`LEDGE`] from y `back` to its lip at y `lip`, over the
/// seam's floor (z −3.2) everywhere else: `floor(xy, top)`.
fn ledge(back: f32, lip: f32) -> impl Fn([f32; 2], f32) -> Option<f32> + Copy {
    move |xy, top| {
        let z = if (back..=lip).contains(&xy[1]) {
            LEDGE
        } else {
            -3.2
        };
        (z <= top + 1e-3).then_some(z)
    }
}

/// The lip of a ledge at y `y`, heading up the seam's drift.
fn lip_at(y: f32) -> Lip {
    Lip {
        at: [-10.457, y, LEDGE],
        heading: [0.0, 1.0],
        speed: None,
        run: None,
    }
}

/// The need the guide's tests place takeoffs at (units/s down).
const NEED: f32 = 5.0;

/// The flight of a takeoff `t`: a walk-off from the lip, or a jump that
/// leaves the floor `PRESS_LEAD` ticks of the run past the line.
fn flight(s: &Seam, t: &Takeoff) -> Option<Impact> {
    let h = t.heading;
    let (from, vz) = match t.leave {
        Leave::Drop => (0.0, 0.0),
        Leave::Jump => (t.speed * PRESS_LEAD as f32 * TICK, JUMP_VZ),
    };
    let at = [
        t.at[0] + h[0] * from,
        t.at[1] + h[1] * from,
        t.at[2] + CLEARANCE,
    ];
    impact(at, [h[0] * t.speed, h[1] * t.speed, vz], |xy| s.contact(xy))
}

/// A jump's takeoff on a ledge whose lip is at y 2.6: a walk-off from
/// the lip comes down 0.4 short of the seam, so the jump is kept.
fn jump_line() -> Takeoff {
    place(&seam(), NEED, &lip_at(2.6), ledge(-20.0, 2.6)).unwrap()
}

/// The takeoff is placed on the lip's real floor and kept for the flight
/// that lands: a jump back along a deep ledge whose walk-off falls short,
/// pressed `PRESS_LEAD` ticks of the run before the band it bounces from.
#[test]
fn a_jump_is_kept_where_the_walk_off_falls_short() {
    let s = seam();
    let t = jump_line();
    assert_eq!((t.leave, t.checked), (Leave::Jump, true));
    assert_eq!((t.heading, t.speed), ([0.0, 1.0], WALK_UP));
    near(t.at[2], LEDGE, 1e-6);
    near(t.at[0], -10.457, 1e-6);
    // Back from the lip, on the ledge.
    assert!(t.at[1] < 2.6 && t.half > 0.0 && t.half < 0.06, "{t:?}");
    // Pressed on the line, the jump bounces; the walk-off does not even
    // land in the catch.
    let i = flight(&s, &t).unwrap();
    assert_eq!(call(&s, Some(&i), true, true), Call::WillBounce);
    let drop = Takeoff {
        at: [-10.457, 2.6, LEDGE],
        leave: Leave::Drop,
        ..t
    };
    assert!(!caught(&s, &flight(&s, &drop).unwrap()));
    // Each end of the band, pressed, bounces too.
    for end in [-1.0, 1.0] {
        let edge = Takeoff {
            at: [t.at[0], t.at[1] + end * (t.half - TAKEOFF_STEP), t.at[2]],
            ..t
        };
        let i = flight(&s, &edge).unwrap();
        assert_eq!(call(&s, Some(&i), true, true), Call::WillBounce, "{end}");
    }
}

/// A walk-off is kept where it lands in the catch at the need and no jump
/// can: on a narrow ledge, too short for the jump's run.
#[test]
fn a_walk_off_is_kept_where_it_lands_and_no_jump_does() {
    let s = seam();
    let t = place(&s, NEED, &lip_at(2.97), ledge(2.67, 2.97)).unwrap();
    assert_eq!((t.leave, t.checked), (Leave::Drop, true));
    assert_eq!(t.at, [-10.457, 2.97, LEDGE]);
    assert_eq!(t.half, DROP_HALF);
    let i = flight(&s, &t).unwrap();
    assert!(caught(&s, &i) && i.speed >= NEED, "{i:?}");
}

/// A lip no modelled flight from lands still says where to go, unchecked:
/// a walk-off from an edge, else as the run left it, else a jump. A lip
/// on no floor has no takeoff.
#[test]
fn a_lip_nothing_lands_from_is_still_where_to_go() {
    let s = seam();
    let far = ledge(-20.0, 0.5);
    let t = place(&s, NEED, &lip_at(0.5), far).unwrap();
    assert_eq!(
        (t.leave, t.checked, t.at),
        (Leave::Drop, false, [-10.457, 0.5, LEDGE])
    );
    // Inside the ledge, not at its edge: a jump, or the run's own way.
    let inside = lip_at(0.0);
    assert_eq!(place(&s, NEED, &inside, far).unwrap().leave, Leave::Jump);
    let run = Lip {
        run: Some(Leave::Drop),
        ..inside
    };
    assert_eq!(place(&s, NEED, &run, far).unwrap().leave, Leave::Drop);
    // A run's speed is the one its takeoff is for.
    let quick = Lip {
        speed: Some(1.2),
        ..inside
    };
    assert_eq!(place(&s, NEED, &quick, far).unwrap().speed, 1.2);
    // Off the floor: none; nor with no heading, or a bad point.
    let floating = Lip {
        at: [-10.457, 0.0, 5.0],
        ..inside
    };
    assert_eq!(place(&s, NEED, &floating, far), None);
    let lost = Lip {
        heading: [0.0; 2],
        ..inside
    };
    assert_eq!(place(&s, NEED, &lost, far), None);
    let bad = Lip {
        at: [f32::NAN, 0.0, LEDGE],
        ..inside
    };
    assert_eq!(place(&s, NEED, &bad, far), None);
}

/// The coarse pass, which tries every step only near the landing
/// window, finds the band a step-by-step scan finds.
#[test]
fn the_coarse_pass_finds_the_same_band() {
    let s = seam();
    let good = |i: &Impact| call(&s, Some(i), true, true) == Call::WillBounce;
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
        let fine = band_by(&s, from, dir, jump(v), range, false, &good);
        assert_eq!(
            band_by(&s, from, dir, jump(v), range, true, &good),
            fine,
            "{from:?} {v:?}"
        );
        assert!(!some || fine.is_some(), "{from:?} {v:?}");
    }
}

/// A jump's line at the origin, heading +Y, for the cue's tests.
fn line(leave: Leave) -> Takeoff {
    Takeoff {
        at: [0.0, 0.0, 0.0],
        heading: [0.0, 1.0],
        half: 0.03,
        speed: WALK_UP,
        leave,
        checked: true,
    }
}

/// A body on the line's floor at y `y`, moving `v` along it.
fn on_floor(y: f32, v: [f32; 2]) -> (Vec3, Vec3) {
    ([0.0, y, CLEARANCE], [v[0], v[1], 0.0])
}

/// The cue is the body against the fixed takeoff: under its floor,
/// `Climb up`; over it, or on it and not running at a jump's line, `Walk
/// to line`; running at it, the count, then `Jump now` a tick before the
/// band and through it; past it, `Walk to line` again. On a walk-off's
/// floor it is `Walk off`.
#[test]
fn the_cue_is_the_body_against_the_takeoff() {
    let t = line(Leave::Jump);
    let cue = |(p, v): (Vec3, Vec3)| t.cue(p, v);
    // Below the takeoff's floor: climb. Over it: walk to it.
    assert_eq!(t.cue([0.0, -1.0, -1.0], [0.0, 0.8, 0.0]), Cue::Climb);
    assert_eq!(t.cue([0.0, -1.0, 1.0], [0.0, 0.8, 0.0]), Cue::WalkToLine);
    // A step up or down within `LEVEL` is the same floor.
    assert!(t.cue([0.0, -1.0, 0.2], [0.0, 0.8, 0.0]).counting());
    // Standing, walking away, crabbing, off to the side, past the band:
    // walk to the line.
    assert_eq!(cue(on_floor(-1.0, [0.0, 0.0])), Cue::WalkToLine);
    assert_eq!(cue(on_floor(-1.0, [0.0, -0.8])), Cue::WalkToLine);
    assert_eq!(cue(on_floor(-1.0, [0.8, 0.1])), Cue::WalkToLine);
    assert_eq!(cue(on_floor(-1.0, [0.0, MOVING / 2.0])), Cue::WalkToLine);
    assert_eq!(
        t.cue([1.0, -1.0, CLEARANCE], [0.0, 0.8, 0.0]),
        Cue::WalkToLine
    );
    assert_eq!(cue(on_floor(0.1, [0.0, 0.8])), Cue::WalkToLine);
    // Running at it from a unit back: 0.97 to the band, 37 ticks less
    // the one early, 1.2 s.
    assert_eq!(t.jump_in([0.0, -1.0, CLEARANCE], [0.0, 0.8, 0.0]), Some(36));
    assert_eq!(cue(on_floor(-1.0, [0.0, 0.8])), Cue::JumpIn(12));
    // Within a tick of the band, and in it: jump now.
    assert_eq!(cue(on_floor(-0.05, [0.0, 0.8])), Cue::JumpNow);
    assert_eq!(cue(on_floor(0.0, [0.0, 0.8])), Cue::JumpNow);
    assert_eq!(cue(on_floor(0.029, [0.0, 0.8])), Cue::JumpNow);
    // The count falls a tick at a time to `Jump now`, after 0.1 s, and
    // holds through the band.
    let step = WALK_UP * TICK;
    let mut last: Option<u32> = None;
    let mut ticks = Vec::new();
    for k in 0..60 {
        let (p, v) = on_floor(-1.0 + step * k as f32, [0.0, WALK_UP]);
        let Some(n) = t.jump_in(p, v) else { break };
        if let Some(m) = last {
            assert_eq!(n, m.saturating_sub(1), "tick {k}");
        }
        ticks.push(t.cue(p, v));
        last = Some(n);
    }
    let first_now = ticks.iter().position(|c| *c == Cue::JumpNow).unwrap();
    assert_eq!(ticks[first_now - 1], Cue::JumpIn(1));
    assert!(ticks[first_now..].iter().all(|c| *c == Cue::JumpNow));
    // A walk-off: on its floor, walk off; under it, climb; no count.
    let d = line(Leave::Drop);
    assert_eq!(d.cue([0.0, -1.0, CLEARANCE], [0.0, 0.8, 0.0]), Cue::WalkOff);
    assert_eq!(d.cue([0.0, -1.0, CLEARANCE], [0.0; 3]), Cue::WalkOff);
    assert_eq!(d.cue([0.0, -1.0, -2.0], [0.0; 3]), Cue::Climb);
    assert_eq!(d.jump_in([0.0, -1.0, CLEARANCE], [0.0, 0.8, 0.0]), None);
}

/// The cue's words, and the HUD kit's box's names for them.
#[test]
fn the_cues_say_what_to_do() {
    assert_eq!(Cue::Climb.text(), "Climb up");
    assert_eq!(Cue::WalkToLine.text(), "Walk to line");
    assert_eq!(Cue::JumpIn(7).text(), "Jump in 0.7 s");
    assert_eq!(Cue::JumpIn(12).text(), "Jump in 1.2 s");
    assert_eq!(Cue::JumpIn(200).text(), "Jump in 9.9 s");
    assert_eq!(Cue::JumpIn(7).label(), "Jump in");
    assert_eq!(Cue::JumpNow.text(), "Jump now");
    assert_eq!(Cue::WalkOff.text(), "Walk off");
    assert_eq!(
        (tenths(1), tenths(3), tenths(4), tenths(1000)),
        (1, 1, 2, CUE_MAX)
    );
    assert!(Cue::JumpIn(3).counting() && Cue::JumpNow.counting());
    assert!(!Cue::Climb.counting() && !Cue::WalkToLine.counting() && !Cue::WalkOff.counting());
    assert_eq!(
        (Leave::Jump.label(), Leave::Drop.label()),
        ("Jump from here", "Drop from here")
    );
    assert_eq!(kit_cue(Cue::JumpIn(7)), Some(marks::Cue::In(7)));
    assert_eq!(kit_cue(Cue::JumpNow), Some(marks::Cue::Now));
    assert_eq!(kit_cue(Cue::WalkToLine), Some(marks::Cue::Walk));
    assert_eq!((kit_cue(Cue::Climb), kit_cue(Cue::WalkOff)), (None, None));
}

/// The cue holds steady: a count down shows as it is; a step a tenth
/// back up, `Jump now` against 0.1, and a count against a missing line
/// are held for `CUE_GRACE` ticks at most; anything else shows at once,
/// and `Jump now` never outlasts its band.
#[test]
fn the_cue_holds_steady() {
    use Cue::{Climb, JumpIn as In, JumpNow as Now, WalkOff, WalkToLine as Walk};
    let shown = |raw: &[Option<Cue>]| {
        let (mut c, mut h) = (None, 0);
        raw.iter()
            .map(|r| {
                (c, h) = settle(c, h, *r);
                c
            })
            .collect::<Vec<_>>()
    };
    let counting = [
        Some(In(5)),
        Some(In(5)),
        Some(In(4)),
        Some(In(4)),
        Some(In(3)),
    ];
    assert_eq!(shown(&counting), counting);
    // Jitter a tenth up and back down is held.
    let jitter = [
        Some(In(5)),
        Some(In(6)),
        Some(In(5)),
        Some(In(6)),
        Some(In(5)),
    ];
    assert_eq!(shown(&jitter), [Some(In(5)); 5]);
    // A step up that lasts shows after `CUE_GRACE` ticks.
    assert_eq!(CUE_GRACE, 3);
    let up = [
        Some(In(5)),
        Some(In(6)),
        Some(In(6)),
        Some(In(6)),
        Some(In(6)),
    ];
    assert_eq!(
        shown(&up),
        [
            Some(In(5)),
            Some(In(5)),
            Some(In(5)),
            Some(In(5)),
            Some(In(6))
        ]
    );
    // A bigger step is a new line: at once.
    assert_eq!(
        shown(&[Some(In(5)), Some(In(9))]),
        [Some(In(5)), Some(In(9))]
    );
    // `Jump now` holds against a count of 0.1 flickering back.
    let flicker = [Some(In(1)), Some(Now), Some(In(1)), Some(Now)];
    assert_eq!(
        shown(&flicker),
        [Some(In(1)), Some(Now), Some(Now), Some(Now)]
    );
    // A count holds through a dropped tick, but not for good.
    assert_eq!(
        shown(&[Some(In(4)), None, Some(In(3))]),
        [Some(In(4)), Some(In(4)), Some(In(3))]
    );
    let gone = [Some(In(4)), None, None, None, None];
    assert_eq!(
        shown(&gone),
        [Some(In(4)), Some(In(4)), Some(In(4)), Some(In(4)), None]
    );
    // `Jump now` goes with its band; `Walk to line`, a count, `Climb up`
    // and `Walk off` show at once.
    assert_eq!(shown(&[Some(Now), None]), [Some(Now), None]);
    assert_eq!(shown(&[Some(Now), Some(Walk)]), [Some(Now), Some(Walk)]);
    assert_eq!(
        shown(&[Some(Walk), Some(In(20))]),
        [Some(Walk), Some(In(20))]
    );
    assert_eq!(
        shown(&[Some(In(4)), Some(Climb)]),
        [Some(In(4)), Some(Climb)]
    );
    assert_eq!(
        shown(&[Some(Climb), Some(WalkOff)]),
        [Some(Climb), Some(WalkOff)]
    );
}

/// A body on the jump line's ledge `back` short of it, running at the
/// line's speed up the drift.
fn running(t: &Takeoff, back: f32) -> Body {
    Body {
        position: [t.at[0], t.at[1] - back, LEDGE + CLEARANCE],
        velocity: [0.0, t.speed, 0.0],
        airborne: false,
        primed: true,
    }
}

/// On the ground the guide carries the takeoff and its cue. Running at
/// a jump's line, the pipper is where that jump comes down, on the seam,
/// lined up, and the box counts (`Jump in 0.6 s`); standing, or under the
/// ledge, there is no pipper and no ticks, the floor underfoot not being
/// the seam's. The cue settles against the last tick's on the same seam
/// only. Not primed there is no guide; in the air no takeoff.
#[test]
fn the_guide_carries_the_cue_on_the_ground() {
    use hud_kit_core::model::Values;
    let spot = ascension();
    let t = jump_line();
    let g = guide_from(&running(&t, 0.5), &spot, Some(t))
        .unwrap()
        .settled(None);
    assert_eq!(g.takeoff, Some(t));
    assert!(matches!(g.cue, Some(Cue::JumpIn(5..=7))), "{:?}", g.cue);
    assert!(g.jump_in.is_some() && g.jump);
    assert_eq!(g.chance, None);
    // The pipper: the jump pressed on the line, in the catch.
    let i = g.impact.unwrap();
    assert!(g.caught() && i.speed > DEPTH_SPEED, "{i:?}");
    assert_eq!(g.call, Call::LinedUp);
    let v = g.view();
    assert!(v.pipper && !v.ticks);
    // The takeoff is the world's bar, not a guide mark.
    assert!(!v.takeoff && g.layout().takeoff.is_none());
    assert_eq!((v.chance, v.cue), (None, g.cue.and_then(kit_cue)));
    let mut out = Values::default();
    marks::set_values(&mut out, Some(&v));
    assert!(out.get(marks::BOX).unwrap().text.starts_with("Jump in 0."));
    assert!(!out.get(marks::TAKEOFF).unwrap().shown);
    // Standing on the ledge: walk to the line; no pipper.
    let still = Body {
        velocity: [0.0; 3],
        ..running(&t, 0.5)
    };
    let w = guide_from(&still, &spot, Some(t)).unwrap();
    assert_eq!(
        (w.cue, w.impact, w.jump_in),
        (Some(Cue::WalkToLine), None, None)
    );
    assert_eq!(w.call, Call::OffLine);
    assert!(!w.view().pipper && !w.view().ticks);
    // On the seam's floor under the ledge: climb up; no pipper.
    let below = Body {
        position: [-10.457, 3.5, -3.2 + CLEARANCE],
        velocity: [0.0, 0.3, 0.0],
        ..still
    };
    let c = guide_from(&below, &spot, Some(t)).unwrap();
    assert_eq!((c.cue, c.impact), (Some(Cue::Climb), None));
    // With no takeoff: no cue, no pipper.
    let bare = guide(&running(&t, 0.5), &spot).unwrap();
    assert_eq!((bare.cue, bare.impact, bare.takeoff), (None, None, None));
    // Settled: a tenth back up is held, on the same seam alone.
    let Some(Cue::JumpIn(n)) = g.cue else {
        unreachable!()
    };
    let jitter = Guide {
        cue: Some(Cue::JumpIn(n + 1)),
        ..g
    };
    let held = jitter.settled(Some(&g));
    assert_eq!((held.cue, held.held), (Some(Cue::JumpIn(n)), 1));
    let elsewhere = Guide {
        seam: Seam {
            edge: [[0.0; 3], [1.0, 0.0, 0.0]],
            ..g.seam
        },
        ..g
    };
    assert_eq!(
        jitter.settled(Some(&elsewhere)).cue,
        Some(Cue::JumpIn(n + 1))
    );
    // Not primed: no guide at all.
    assert_eq!(
        guide_from(
            &Body {
                primed: false,
                ..running(&t, 0.5)
            },
            &spot,
            Some(t)
        ),
        None
    );
    // In the air: the fall's chance, and no takeoff.
    let falling = Body {
        position: [-10.457, 3.95, -2.83],
        velocity: [0.0, 0.3, -5.3],
        airborne: true,
        primed: true,
    };
    let f = guide_from(&falling, &spot, Some(t))
        .unwrap()
        .settled(Some(&g));
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
    assert!(
        under > 0.5 && under < 0.6 && over < 0.5 && over > 0.4,
        "{under} {over}"
    );
    near(under + over, 1.0, 1e-6);
    // One softness under the cut, and one inside an end.
    near(at(DEPTH_SOFT, -0.04), 0.5 + 0.45 * 1f32.tanh(), 1e-5);
    near(
        at(0.2, LANDING_WINDOW[0] + WINDOW_SOFT),
        0.5 + 0.45 * 1f32.tanh(),
        1e-4,
    );
    // Clearly shallow, and clearly off the window: low.
    assert!(at(-0.05, -0.04) < 0.06 && at(0.08, -0.10) < 0.06 && at(0.08, 0.03) < 0.06);
    // Outside the catch: the bottom, whatever the depth.
    near(
        confidence(
            &Depth {
                margin: 0.08,
                past: -0.04,
            },
            false,
        ),
        low,
        1e-6,
    );
    near(
        confidence(
            &Depth {
                margin: f32::NAN,
                past: -0.04,
            },
            true,
        ),
        low,
        1e-6,
    );
    near(
        confidence(
            &Depth {
                margin: 0.08,
                past: f32::NAN,
            },
            true,
        ),
        low,
        1e-6,
    );
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
        assert!(
            d.deep() && d.in_window() && confidence(&d, true) >= 0.5,
            "{d:?}"
        );
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
    let scan = Spot {
        landed: false,
        ..ascension()
    };
    assert!(!Seam::of(&scan, [-10.457, 3.976]).measured);
    // A primed fall, deep in the window: 93% on the measured seam, the
    // cap on the scan's. The call is the same on both.
    let body = |z: f32| Body {
        position: [-10.457, 3.95, z],
        velocity: [0.0, 0.3, -5.3],
        airborne: true,
        primed: true,
    };
    let (m, s) = (
        guide(&body(-2.83), &ascension()).unwrap(),
        guide(&body(-2.83), &scan).unwrap(),
    );
    assert_eq!((m.call, s.call), (Call::WillBounce, Call::WillBounce));
    let (mc, sc) = (m.chance.unwrap(), s.chance.unwrap());
    near(mc.value, 0.929, 2e-3);
    assert!(!mc.capped);
    assert_eq!((sc.value, sc.capped), (SCAN_ONLY_CAP, true));
    assert_eq!((m.view().chance, s.view().chance), (Some(93), Some(60)));
    // Too shallow is under the cap on either: the same on both.
    let (m, s) = (
        guide(&body(-2.77), &ascension()).unwrap(),
        guide(&body(-2.77), &scan).unwrap(),
    );
    assert_eq!(m.call, Call::TooShallow);
    near(m.chance.unwrap().value, 0.136, 2e-3);
    assert_eq!(m.chance, s.chance);
    assert_eq!(m.view().chance, Some(14));
    // A fall slower than the depth rule is measured at, and one not
    // primed, have no chance.
    let slow = Body {
        velocity: [0.0, 0.3, -3.0],
        ..body(-2.83)
    };
    assert_eq!(guide(&slow, &ascension()).unwrap().chance, None);
    let idle = Body {
        primed: false,
        ..body(-2.83)
    };
    assert_eq!(guide(&idle, &ascension()), None);
}
