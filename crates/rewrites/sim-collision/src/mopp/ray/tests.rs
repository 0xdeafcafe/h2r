//! The float MOPP ray on invented programs: each opcode family by hand
//! (values chosen so every clip is exact), misses, rays ending on a
//! threshold, the fraction cutting a walk short, both precisions, the
//! fail-closed cases, the parity arm, and random programs against a
//! recursive transcription of the interpreter.
use super::*;
use crate::synth::{self, Rng};

const ONE: u32 = 0x3f80_0000;

fn f(x: f32) -> u32 {
    x.to_bits()
}

/// A frame-space point with lane 3 as the root setup leaves it for a ray
/// input whose fourth lanes are zero (`(0 - scale) * scale`).
fn p(x: f32, y: f32, z: f32, w: f32) -> Vec4 {
    [f(x), f(y), f(z), f(w)]
}

fn constants() -> Constants {
    Constants {
        zero: 0,
        one: ONE,
        unit: [ONE; 4],
        root_scale: ONE,
    }
}

/// Origin zero and scale one: frame units are world units, and the root
/// segment's fourth lane is -1.
fn ray(from: [f32; 3], to: [f32; 3], precision: Precision) -> Ray {
    Ray {
        origin: [0, 0, 0, ONE],
        from: [f(from[0]), f(from[1]), f(from[2]), 0],
        to: [f(to[0]), f(to[1]), f(to[2]), 0],
        precision,
        mxcsr: 0x1fa0,
        constants: constants(),
    }
}

fn root_frame() -> Frame {
    Frame {
        offset: [0; 4],
        bits: 0,
        scale: ONE,
        key: 0,
        property: 0,
    }
}

/// Every event of a walk; `hit` gives the fraction after a key.
fn events(code: &[u8], r: Ray, mut hit: impl FnMut(u32, u32) -> u32) -> Result<Vec<Event>, Bail> {
    let root = r.root()?;
    let mut walk = RayWalk::new(code, r, &root);
    let mut fraction = ONE;
    let mut out = Vec::new();
    while let Some(e) = walk.next(fraction)? {
        if let Event::Key(k) = e {
            fraction = hit(k, fraction);
        }
        out.push(e);
    }
    Ok(out)
}

fn misses(_: u32, f: u32) -> u32 {
    f
}

fn keys(events: &[Event]) -> Vec<u32> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Key(k) => Some(*k),
            Event::Enter { .. } => None,
        })
        .collect()
}

fn along_x(a: f32, b: f32) -> Ray {
    ray([a, 0.0, 0.0], [b, 0.0, 0.0], Precision::Double)
}

fn enter(pc: u32, frame: Frame, start: Vec4, end: Vec4) -> Event {
    Event::Enter {
        pc,
        frame,
        segment: Segment { start, end },
    }
}

/// `(1 - w) * a + w * b` per lane, in binary32, as the clips do it.
fn blend(w: f32, a: Vec4, b: Vec4) -> Vec4 {
    std::array::from_fn(|i| f((1.0 - w) * f32::from_bits(a[i]) + w * f32::from_bits(b[i])))
}

const SPLIT: [u8; 6] = [0x10, 10, 20, 1, 0x31, 0x32];

#[test]
fn a_ray_on_one_side_of_a_split_visits_that_child_only() {
    let low = events(&SPLIT, along_x(0.0, 5.0), misses).unwrap();
    assert_eq!(keys(&low), [1]);
    assert_eq!(low.len(), 2, "the root call and the key");
    assert_eq!(
        keys(&events(&SPLIT, along_x(30.0, 40.0), misses).unwrap()),
        [2]
    );
    // The children overlap between the thresholds: wholly below the high
    // one is the low child's, wholly above the low one the high child's.
    assert_eq!(
        keys(&events(&SPLIT, along_x(12.0, 18.0), misses).unwrap()),
        [1]
    );
    assert_eq!(
        keys(&events(&SPLIT, along_x(15.0, 25.0), misses).unwrap()),
        [2]
    );
    assert_eq!(
        keys(&events(&SPLIT, along_x(5.0, 25.0), misses).unwrap()),
        [1, 2]
    );
}

#[test]
fn a_rising_ray_calls_the_low_child_with_its_end_clipped_at_the_low_threshold() {
    let got = events(&SPLIT, along_x(0.0, 40.0), misses).unwrap();
    let (start, end) = (p(0.0, 0.0, 0.0, -1.0), p(40.0, 0.0, 0.0, -1.0));
    assert_eq!(
        got,
        [
            enter(0, root_frame(), start, end),
            enter(4, root_frame(), start, p(10.0, 0.0, 0.0, -1.0)),
            Event::Key(1),
            Event::Key(2),
        ]
    );
}

#[test]
fn a_falling_ray_calls_the_high_child_first() {
    let got = events(&SPLIT, along_x(40.0, 0.0), misses).unwrap();
    let start = p(40.0, 0.0, 0.0, -1.0);
    assert_eq!(
        got[1],
        enter(5, root_frame(), start, p(20.0, 0.0, 0.0, -1.0))
    );
    assert_eq!(keys(&got), [2, 1]);
}

#[test]
fn a_dropped_fraction_stops_the_ray_short_of_the_far_child() {
    let quarter = f(0.25);
    let short = events(
        &SPLIT,
        along_x(0.0, 40.0),
        |k, f| if k == 1 { quarter } else { f },
    )
    .unwrap();
    assert_eq!(
        keys(&short),
        [1],
        "the end moves to x = 10, below the high threshold"
    );
    let three = f(0.75);
    let long = events(
        &SPLIT,
        along_x(0.0, 40.0),
        |k, f| if k == 1 { three } else { f },
    )
    .unwrap();
    assert_eq!(keys(&long), [1, 2], "x = 30 still reaches the high side");
    // Falling: the end recomputed at x = 30 is above the low threshold.
    let back = events(
        &SPLIT,
        along_x(40.0, 0.0),
        |k, f| if k == 2 { quarter } else { f },
    )
    .unwrap();
    assert_eq!(keys(&back), [2]);
}

#[test]
fn rays_ending_on_a_threshold() {
    // End exactly on the high threshold: not strictly below it, so both.
    let got = events(&SPLIT, along_x(0.0, 20.0), misses).unwrap();
    assert_eq!(
        got[1],
        enter(
            4,
            root_frame(),
            p(0.0, 0.0, 0.0, -1.0),
            p(10.0, 0.0, 0.0, -1.0)
        )
    );
    assert_eq!(keys(&got), [1, 2]);
    // End exactly on the low threshold: wholly below the high one.
    assert_eq!(
        keys(&events(&SPLIT, along_x(0.0, 10.0), misses).unwrap()),
        [1]
    );
    // Start on the low threshold: a zero distance clips nothing.
    let got = events(&SPLIT, along_x(10.0, 30.0), misses).unwrap();
    assert_eq!(
        got[1],
        enter(
            4,
            root_frame(),
            p(10.0, 0.0, 0.0, -1.0),
            p(30.0, 0.0, 0.0, -1.0)
        )
    );
    assert_eq!(keys(&got), [1, 2]);
    // Start on the high threshold going down past the low one: the high
    // child first, with a zero distance that clips nothing.
    let got = events(&SPLIT, along_x(20.0, 5.0), misses).unwrap();
    assert_eq!(
        got[1],
        enter(
            5,
            root_frame(),
            p(20.0, 0.0, 0.0, -1.0),
            p(5.0, 0.0, 0.0, -1.0)
        )
    );
    assert_eq!(keys(&got), [2, 1]);
    // Start on the low threshold going down: wholly below the high one.
    assert_eq!(
        keys(&events(&SPLIT, along_x(10.0, 5.0), misses).unwrap()),
        [1]
    );
}

#[test]
fn a_one_plane_split_has_its_low_side_one_unit_up() {
    let code = [0x20, 10, 1, 0x31, 0x32];
    assert_eq!(
        keys(&events(&code, along_x(0.0, 9.5), misses).unwrap()),
        [1]
    );
    assert_eq!(
        keys(&events(&code, along_x(11.5, 30.0), misses).unwrap()),
        [2]
    );
    // Between S and S + 1 the ray is on both sides.
    assert_eq!(
        keys(&events(&code, along_x(10.5, 10.75), misses).unwrap()),
        [1, 2]
    );
    let got = events(&code, along_x(0.0, 22.0), misses).unwrap();
    assert_eq!(
        got[1],
        enter(
            3,
            root_frame(),
            p(0.0, 0.0, 0.0, -1.0),
            p(11.0, 0.0, 0.0, -1.0)
        )
    );
}

#[test]
fn a_wide_split_jumps_to_both_children() {
    let code = [0x23, 10, 20, 0, 2, 0, 3, 0xee, 0xee, 0x31, 0x32];
    assert_eq!(
        keys(&events(&code, along_x(0.0, 5.0), misses).unwrap()),
        [1]
    );
    assert_eq!(
        keys(&events(&code, along_x(30.0, 40.0), misses).unwrap()),
        [2]
    );
    let got = events(&code, along_x(0.0, 40.0), misses).unwrap();
    assert_eq!(
        got[1],
        enter(
            9,
            root_frame(),
            p(0.0, 0.0, 0.0, -1.0),
            p(10.0, 0.0, 0.0, -1.0)
        )
    );
    assert_eq!(keys(&got), [1, 2]);
    // On y: the same thresholds on another axis.
    let code = [0x24, 10, 20, 0, 0, 0, 1, 0x31, 0x32];
    let r = ray([0.0, 30.0, 0.0], [0.0, 40.0, 0.0], Precision::Single);
    assert_eq!(keys(&events(&code, r, misses).unwrap()), [2]);
}

#[test]
fn diagonal_splits_project_both_ends() {
    // x + y against 2 * 10 and 2 * 20.
    let code = [0x17, 10, 20, 1, 0x31, 0x32];
    let r = ray([0.0, 0.0, 0.0], [20.0, 20.0, 0.0], Precision::Double);
    let got = events(&code, r, misses).unwrap();
    assert_eq!(
        got[1],
        enter(
            4,
            root_frame(),
            p(0.0, 0.0, 0.0, -1.0),
            p(10.0, 10.0, 0.0, -1.0)
        )
    );
    assert_eq!(keys(&got), [1, 2]);
    // x - y against 2 * b - 255: thresholds 1 and 5.
    let code = [0x18, 128, 130, 1, 0x31, 0x32];
    let r = ray([0.0, 0.0, 0.0], [0.0, 10.0, 0.0], Precision::Single);
    assert_eq!(keys(&events(&code, r, misses).unwrap()), [1]);
    let r = ray([10.0, 0.0, 0.0], [20.0, 0.0, 0.0], Precision::Single);
    assert_eq!(keys(&events(&code, r, misses).unwrap()), [2]);
    // Every diagonal opcode, thresholds from operands 0 and 1: a ray whose
    // projections stay above the low threshold reaches the high child
    // only. The sums see a ray far out on every axis; the differences a
    // ray with x large and y, z equal and negative (y - z is 0, above the
    // low threshold -255).
    for op in 0x13..=0x1cu8 {
        let code = [op, 0, 1, 1, 0x31, 0x32];
        let r = if matches!(op, 0x13 | 0x15 | 0x17 | 0x19) {
            ray(
                [400.0, 400.0, 400.0],
                [500.0, 500.0, 500.0],
                Precision::Double,
            )
        } else {
            ray(
                [900.0, -400.0, -400.0],
                [950.0, -450.0, -450.0],
                Precision::Double,
            )
        };
        assert_eq!(
            keys(&events(&code, r, misses).unwrap()),
            [2],
            "opcode {op:#x}"
        );
    }
}

#[test]
fn the_start_projection_is_compared_unrounded_in_double_precision() {
    // (z + y) + x = 764 + (1 - 2^-20): 765 in single, just below in double.
    let code = [0x19, 0, 255, 1, 0x31, 0x32];
    let y = 1.0 - f32::EPSILON * 8.0;
    let single = ray([0.0, y, 764.0], [0.0, 0.0, 0.0], Precision::Single);
    let double = ray([0.0, y, 764.0], [0.0, 0.0, 0.0], Precision::Double);
    assert_eq!(keys(&events(&code, double, misses).unwrap()), [1]);
    assert_eq!(keys(&events(&code, single, misses).unwrap()), [2, 1]);
}

#[test]
fn bounds_clip_or_reject() {
    let code = [0x26, 10, 20, 0x10, 15, 17, 1, 0x31, 0x32];
    // Rising through the bound: both ends clipped to [10, 20].
    let got = events(&code, along_x(0.0, 40.0), misses).unwrap();
    assert_eq!(
        got,
        [
            enter(
                0,
                root_frame(),
                p(0.0, 0.0, 0.0, -1.0),
                p(40.0, 0.0, 0.0, -1.0)
            ),
            enter(
                7,
                root_frame(),
                p(10.0, 0.0, 0.0, -1.0),
                p(15.0, 0.0, 0.0, -1.0)
            ),
            Event::Key(1),
            Event::Key(2),
        ]
    );
    // Falling: the start clips at the high bound, the end at the low one.
    let got = events(&code, along_x(40.0, 0.0), misses).unwrap();
    let (start, end) = (p(20.0, 0.0, 0.0, -1.0), p(10.0, 0.0, 0.0, -1.0));
    let w = 3.0f32 / 10.0;
    assert_eq!(got[1], enter(8, root_frame(), start, blend(w, start, end)));
    assert_eq!(keys(&got), [2, 1]);
    // Misses on either side and either direction.
    for (a, b) in [
        (30.0, 40.0),
        (40.0, 30.0),
        (0.0, 5.0),
        (5.0, 0.0),
        (21.0, 400.0),
    ] {
        assert_eq!(
            events(&code, along_x(a, b), misses).unwrap().len(),
            1,
            "{a} to {b}"
        );
    }
    // Touching a bound is inside it.
    assert_eq!(
        keys(&events(&code, along_x(0.0, 10.0), misses).unwrap()),
        [1]
    );
    assert_eq!(
        keys(&events(&code, along_x(20.0, 30.0), misses).unwrap()),
        [2]
    );
}

#[test]
fn wide_bounds_scale_their_full_precision_operands_into_the_frame() {
    // lo 10, hi 20 in full-precision units; the code scale 2 makes the
    // root frame twice the world and the inverse scale one half.
    let code = [0x29, 0, 0, 10, 0, 0, 20, 0x10, 15, 17, 1, 0x31, 0x32];
    let mut r = along_x(0.0, 20.0);
    r.origin[3] = f(2.0);
    let root = r.root().unwrap();
    assert_eq!(root.inverse_scale, f(0.5));
    assert_eq!(root.frame.scale, f(2.0));
    let got = events(&code, r, misses).unwrap();
    let frame = Frame {
        scale: f(2.0),
        ..root_frame()
    };
    // Lane 3: (0 - 2) * 2.
    assert_eq!(
        got[1],
        enter(11, frame, p(10.0, 0.0, 0.0, -4.0), p(15.0, 0.0, 0.0, -4.0))
    );
    // A ray wholly past the bound: frame x from 30 to 40.
    let mut past = along_x(15.0, 20.0);
    past.origin[3] = f(2.0);
    assert_eq!(events(&code, past, misses).unwrap().len(), 1);
}

#[test]
fn a_rescale_moves_the_segment_and_makes_a_new_frame() {
    let code = [0x01, 1, 2, 3, 0x10, 50, 60, 1, 0x31, 0x32];
    let r = ray([1.0, 2.0, 3.0], [101.0, 2.0, 3.0], Precision::Double);
    let got = events(&code, r, misses).unwrap();
    let frame = Frame {
        offset: p(2.0, 4.0, 6.0, 0.0),
        bits: 1,
        scale: f(2.0),
        key: 0,
        property: 0,
    };
    assert_eq!(
        got[1],
        enter(8, frame, p(0.0, 0.0, 0.0, -2.0), p(50.0, 0.0, 0.0, -2.0))
    );
    assert_eq!(keys(&got), [1, 2]);
    // The recomputed end goes through the rescaled frame: x = 51 in the
    // world is 100 here, above the high threshold; x = 26 is 50, below.
    let half = f(0.5);
    let got = events(&code, r, |k, f| if k == 1 { half } else { f }).unwrap();
    assert_eq!(keys(&got), [1, 2]);
    let quarter = f(0.25);
    let got = events(&code, r, |k, f| if k == 1 { quarter } else { f }).unwrap();
    assert_eq!(keys(&got), [1]);
}

#[test]
fn key_offsets_properties_and_jumps() {
    let r = along_x(0.0, 1.0);
    let cases: [(&[u8], u32); 6] = [
        (
            &[
                0x09, 5, 0x60, 7, 0x05, 2, 0xee, 0xee, 0x0a, 0x01, 0x00, 0x33,
            ],
            0x108,
        ),
        (&[0x0b, 0, 0, 1, 0, 0x50, 9], 0x109),
        (&[0x09, 1, 0x0b, 0, 0, 0, 2, 0x51, 1, 0], 0x102),
        (&[0x06, 0, 1, 0xee, 0x31], 1),
        (
            &[
                0x07, 0, 0, 2, 0xee, 0xee, 0x64, 1, 2, 0x68, 1, 2, 3, 4, 0x52, 0, 0, 7,
            ],
            7,
        ),
        (
            &[0x0b, 0xff, 0xff, 0xff, 0xff, 0x09, 2, 0x53, 0, 0, 0, 1],
            2,
        ),
    ];
    for (code, key) in cases {
        assert_eq!(keys(&events(code, r, misses).unwrap()), [key], "{code:x?}");
    }
    // The key offset and the property ride in the frame a call gets.
    let code = [0x09, 5, 0x60, 7, 0x10, 10, 20, 1, 0x31, 0x32];
    let got = events(&code, along_x(0.0, 40.0), misses).unwrap();
    let frame = Frame {
        key: 5,
        property: 7,
        ..root_frame()
    };
    assert_eq!(
        got[1],
        enter(8, frame, p(0.0, 0.0, 0.0, -1.0), p(10.0, 0.0, 0.0, -1.0))
    );
    assert_eq!(keys(&got), [6, 7]);
    // A frame that returns without a terminal.
    assert_eq!(events(&[0x00], r, misses).unwrap().len(), 1);
}

#[test]
fn what_the_original_asserts_on_fails_closed() {
    let r = along_x(0.0, 40.0);
    for code in [
        &[0x08][..],
        &[0x10, 10],
        &[0x61, 1, 0x31],
        &[0x6a, 0x31],
        &[0x05, 0],
        &[],
        &[0x29, 0, 0, 1, 0, 0],
        &[0x30 - 1],
    ] {
        assert_eq!(events(code, r, misses), Err(Bail::Data), "{code:x?}");
    }
    // A trap reached only on one side is reached only by rays going there.
    let code = [0x10, 10, 20, 1, 0x31, 0x08];
    assert_eq!(
        keys(&events(&code, along_x(0.0, 5.0), misses).unwrap()),
        [1]
    );
    assert_eq!(events(&code, along_x(0.0, 40.0), misses), Err(Bail::Data));
}

#[test]
fn deep_nesting_bails() {
    // A chain of two-sided splits, each low child the next split, one
    // unit lower so the clipped copy still crosses it.
    let mut code = Vec::new();
    for t in (0..=255u8).rev() {
        code.extend([0x10, t, t, 1]);
    }
    code.extend([0x31, 0x32]);
    assert_eq!(events(&code, along_x(0.0, 300.0), misses), Err(Bail::Depth));
}

#[test]
fn values_off_the_fast_path_run_as_the_guest() {
    // A NaN start: the packed work carries it (a quiet NaN raises nothing).
    let nan = ray([f32::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], Precision::Single);
    let root = nan.root().unwrap();
    assert!(f32::from_bits(root.segment.start[0]).is_nan());
    // A subnormal start raises DE in the MXCSR.
    let tiny = ray([1.0e-40, 0.0, 0.0], [1.0, 0.0, 0.0], Precision::Single);
    assert_ne!(tiny.root().unwrap().raised.mxcsr & 0x02, 0);
    // A zero scale divides by zero on the x87: ZE, an infinite inverse.
    let mut zero_scale = along_x(0.0, 1.0);
    zero_scale.origin[3] = 0;
    let root = zero_scale.root().unwrap();
    assert_ne!(root.raised.x87 & 0x04, 0);
    assert_eq!(root.inverse_scale, f32::INFINITY.to_bits());
    // A fraction that is not a number compares unordered: every return
    // site's ordered-less test fails, as the guest's `test ah, 5; jp`.
    let nan_fraction = events(&SPLIT, along_x(0.0, 40.0), |_, _| f(f32::NAN));
    assert!(nan_fraction.is_ok());
    // Extended precision runs on the model and agrees on exact values.
    let mut ext = along_x(0.0, 40.0);
    ext.precision = Precision::Extended;
    assert_eq!(events(&SPLIT, ext, misses), events(&SPLIT, along_x(0.0, 40.0), misses));
}

#[test]
fn precision_comes_from_a_masked_nearest_control() {
    assert_eq!(Precision::from_control(0x007f), Ok(Precision::Single));
    assert_eq!(Precision::from_control(0x027f), Ok(Precision::Double));
    assert_eq!(Precision::from_control(0x037f), Ok(Precision::Extended));
    for control in [0x017f, 0x0a7f, 0x027e, 0x067f] {
        assert_eq!(
            Precision::from_control(control),
            Err(Bail::Env),
            "{control:#x}"
        );
    }
}

/// The guest as the parity arm sees it: every call and key the walk makes,
/// each with the fraction the VM holds at that point.
fn observed(code: &[u8], r: Ray, hit: impl Fn(u32, u32) -> u32) -> (Vec<(u32, Event)>, u32) {
    let root = r.root().unwrap();
    let mut walk = RayWalk::new(code, r, &root);
    let mut fraction = ONE;
    let mut out = Vec::new();
    while let Some(e) = walk.next(fraction).unwrap() {
        out.push((fraction, e));
        if let Event::Key(k) = e {
            fraction = hit(k, fraction);
        }
    }
    (out, fraction)
}

fn check(code: &[u8], r: Ray, keys: bool) -> RayCheck<&[u8]> {
    let root = r.root().unwrap();
    RayCheck::new(RayWalk::new(code, r, &root), keys, ONE)
}

const NEST: [u8; 14] = [
    0x10, 10, 20, 9, 0x11, 5, 30, 1, 0x31, 0x32, 0x00, 0x00, 0x00, 0x33,
];

fn hits_on_one(k: u32, f: u32) -> u32 {
    if k == 1 { 0x3f00_0000 } else { f }
}

#[test]
fn the_parity_arm_matches_a_guest_doing_the_same() {
    let r = ray([0.0, 0.0, 0.0], [40.0, 40.0, 0.0], Precision::Double);
    let (seen, end) = observed(&NEST, r, hits_on_one);
    assert!(seen.len() >= 4, "{seen:?}");
    let mut c = check(&NEST, r, true);
    for &(fraction, e) in &seen {
        c.observe(fraction, e);
    }
    assert_eq!(c.finish(end), Outcome::Matched);
    // Without keys, the walk's keys are skipped and only calls compare,
    // as long as nothing moves the fraction.
    let (seen, end) = observed(&NEST, r, misses);
    let mut c = check(&NEST, r, false);
    for &(fraction, e) in seen
        .iter()
        .filter(|(_, e)| matches!(e, Event::Enter { .. }))
    {
        c.observe(fraction, e);
    }
    assert_eq!(c.finish(end), Outcome::Matched);
}

#[test]
fn a_deliberate_mismatch_is_reported_with_where_it_happened() {
    let r = ray([0.0, 0.0, 0.0], [40.0, 40.0, 0.0], Precision::Double);
    let (mut seen, end) = observed(&NEST, r, hits_on_one);
    // The guest's second call has a segment one bit off.
    let Event::Enter { segment, .. } = &mut seen[1].1 else {
        panic!("the second observation is not a call")
    };
    segment.end[0] ^= 1;
    let mut c = check(&NEST, r, true);
    for &(fraction, e) in &seen {
        c.observe(fraction, e);
    }
    let outcome = c.finish(end);
    let Outcome::Mismatch {
        at,
        guest,
        native,
        what,
        ..
    } = &outcome
    else {
        panic!("{outcome:?}")
    };
    assert_eq!((*at, *what), (1, "different step"));
    assert_eq!(*guest, Some(seen[1].1));
    assert_ne!(guest, native);
    let text = outcome.to_string();
    assert!(
        text.starts_with("different step at observation 1: guest enter pc"),
        "{text}"
    );

    // A guest that returns early.
    let (seen, end) = observed(&NEST, r, hits_on_one);
    let mut c = check(&NEST, r, true);
    for &(fraction, e) in &seen[..seen.len() - 1] {
        c.observe(fraction, e);
    }
    assert!(matches!(
        c.finish(end),
        Outcome::Mismatch {
            guest: None,
            native: Some(_),
            ..
        }
    ));
    // A guest that does more than the walk.
    let mut c = check(&NEST, r, true);
    for &(fraction, e) in &seen {
        c.observe(fraction, e);
    }
    c.observe(end, Event::Key(99));
    assert!(matches!(
        c.finish(end),
        Outcome::Mismatch {
            guest: Some(Event::Key(99)),
            native: None,
            ..
        }
    ));
    // A setup word.
    let mut c = check(&NEST, r, true);
    c.expect("inverse scale", ONE, ONE);
    assert!(!c.decided());
    c.expect("inverse scale", ONE, 0);
    assert!(c.decided());
    assert!(matches!(
        c.finish(end),
        Outcome::Mismatch {
            what: "inverse scale",
            ..
        }
    ));
}

#[test]
fn the_parity_arm_leaves_what_it_cannot_follow_unverified() {
    let r = ray([0.0, 0.0, 0.0], [40.0, 40.0, 0.0], Precision::Double);
    // No keys observed and the fraction moved.
    let (seen, end) = observed(&NEST, r, hits_on_one);
    let mut c = check(&NEST, r, false);
    for &(fraction, e) in seen
        .iter()
        .filter(|(_, e)| matches!(e, Event::Enter { .. }))
    {
        c.observe(fraction, e);
    }
    assert_eq!(c.finish(end), Outcome::Unverified(Unverified::Fraction));
    // The walk bails where the guest goes on.
    let code = [0x10, 10, 20, 1, 0x08, 0x32];
    let mut c = check(&code, along_x(0.0, 40.0), true);
    let (start, end) = (p(0.0, 0.0, 0.0, -1.0), p(40.0, 0.0, 0.0, -1.0));
    c.observe(ONE, enter(0, root_frame(), start, end));
    c.observe(ONE, enter(4, root_frame(), start, p(10.0, 0.0, 0.0, -1.0)));
    assert_eq!(
        c.finish(ONE),
        Outcome::Unverified(Unverified::Bail(Bail::Data))
    );
}

/// The interpreter written the way it reads, recursively, with values as
/// `f64` rounded to the working precision after each x87 operation.
struct Reference<'a> {
    code: &'a [u8],
    ray: Ray,
    single: bool,
    inverse_scale: u32,
    fraction: u32,
    hit: &'a dyn Fn(u32, u32) -> u32,
    out: Vec<Event>,
}

fn ld(bits: u32) -> f64 {
    f64::from(f32::from_bits(bits))
}

fn st(v: f64) -> u32 {
    (v as f32).to_bits()
}

fn lanes(a: Vec4, b: Vec4, op: fn(f32, f32) -> f32) -> Vec4 {
    std::array::from_fn(|i| f(op(f32::from_bits(a[i]), f32::from_bits(b[i]))))
}

impl Reference<'_> {
    fn x(&self, v: f64) -> f64 {
        if self.single { f64::from(v as f32) } else { v }
    }

    fn mix(&self, w: u32, a: Vec4, b: Vec4) -> Vec4 {
        let w = [w; 4];
        lanes(
            lanes(
                lanes(self.ray.constants.unit, w, |x, y| x - y),
                a,
                |x, y| x * y,
            ),
            lanes(w, b, |x, y| x * y),
            |x, y| x + y,
        )
    }

    fn byte(&self, at: usize) -> Option<u32> {
        self.code.get(at).map(|&b| u32::from(b))
    }

    fn word(&self, at: usize, n: usize) -> Option<u32> {
        (0..n).try_fold(0, |v, i| Some((v << 8) | self.byte(at + i)?))
    }

    /// `None`: the program faults or asserts.
    fn call(&mut self, mut pc: usize, mut frame: Frame, mut seg: Segment) -> Option<()> {
        self.out.push(Event::Enter {
            pc: pc as u32,
            frame,
            segment: seg,
        });
        loop {
            let op = self.code.get(pc).copied()?;
            match op {
                0x00 => return Some(()),
                0x01..=0x04 => {
                    let k = u32::from(op);
                    let bytes = [
                        f(self.byte(pc + 1)? as f32),
                        f(self.byte(pc + 2)? as f32),
                        f(self.byte(pc + 3)? as f32),
                        0,
                    ];
                    let m = [f((1u32 << k) as f32); 4];
                    seg.start = lanes(m, lanes(seg.start, bytes, |x, y| x - y), |x, y| x * y);
                    seg.end = lanes(m, lanes(seg.end, bytes, |x, y| x - y), |x, y| x * y);
                    frame.offset = lanes(m, lanes(frame.offset, bytes, |x, y| x + y), |x, y| x * y);
                    frame.bits += k;
                    frame.scale = st(self.x(f64::from(1u32 << k) * ld(frame.scale)));
                    pc += 4;
                }
                0x05 => pc += 2 + self.word(pc + 1, 1)? as usize,
                0x06 => pc += 3 + self.word(pc + 1, 2)? as usize,
                0x07 => pc += 4 + self.word(pc + 1, 3)? as usize,
                0x09 => {
                    frame.key = frame.key.wrapping_add(self.word(pc + 1, 1)?);
                    pc += 2;
                }
                0x0a => {
                    frame.key = frame.key.wrapping_add(self.word(pc + 1, 2)?);
                    pc += 3;
                }
                0x0b => {
                    frame.key = self.word(pc + 1, 4)?;
                    pc += 5;
                }
                0x60 => {
                    frame.property = self.word(pc + 1, 1)?;
                    pc += 2;
                }
                0x64 => {
                    frame.property = self.word(pc + 1, 2)?;
                    pc += 3;
                }
                0x68 => {
                    frame.property = self.word(pc + 1, 4)?;
                    pc += 5;
                }
                0x30..=0x53 => {
                    let key = if op < 0x50 {
                        u32::from(op - 0x30)
                    } else {
                        self.word(pc + 1, usize::from(op - 0x4f))?
                    };
                    let key = frame.key.wrapping_add(key);
                    self.out.push(Event::Key(key));
                    self.fraction = (self.hit)(key, self.fraction);
                    return Some(());
                }
                0x26..=0x2b => {
                    let wide = op >= 0x29;
                    let a = usize::from(if wide { op - 0x29 } else { op - 0x26 });
                    let (lo, hi) = if wide {
                        let full = |v: u32| {
                            let v = self
                                .x(self.x(f64::from(v) * ld(self.inverse_scale)) * ld(frame.scale));
                            self.x(v - ld(frame.offset[a]))
                        };
                        (full(self.word(pc + 1, 3)?), full(self.word(pc + 4, 3)?))
                    } else {
                        (f64::from(self.byte(pc + 1)?), f64::from(self.byte(pc + 2)?))
                    };
                    pc += if wide { 7 } else { 3 };
                    let (s, e) = (ld(seg.start[a]), ld(seg.end[a]));
                    let falling = s >= e;
                    if (!falling && (e < lo || s > hi)) || (falling && (s < lo || e > hi)) {
                        return Some(());
                    }
                    let orig = seg;
                    let d0 = ld(st(self.x(s - hi)));
                    let d1 = self.x(e - hi);
                    if self.x(d0 * d1) < 0.0 {
                        let w = st(self.x(d0 / self.x(d0 - d1)));
                        let v = self.mix(w, orig.start, orig.end);
                        if falling { seg.start = v } else { seg.end = v }
                    }
                    let d0 = ld(st(self.x(s - lo)));
                    let d1 = self.x(e - lo);
                    if self.x(d0 * d1) < 0.0 {
                        let w = st(self.x(d0 / self.x(d0 - d1)));
                        let v = self.mix(w, orig.start, orig.end);
                        if falling { seg.end = v } else { seg.start = v }
                    }
                }
                0x10..=0x25 if !(0x1d..=0x1f).contains(&op) => {
                    let b1 = i64::from(self.byte(pc + 1)?);
                    let b2 = i64::from(self.byte(pc + 2)?);
                    let (low, high, first, second, axis) = match op {
                        0x10..=0x12 => (
                            b1,
                            b2,
                            pc + 4,
                            pc + 4 + self.byte(pc + 3)? as usize,
                            Some(usize::from(op - 0x10)),
                        ),
                        0x20..=0x22 => (
                            b1 + 1,
                            b1,
                            pc + 3,
                            pc + 3 + b2 as usize,
                            Some(usize::from(op - 0x20)),
                        ),
                        0x23..=0x25 => {
                            let base = pc + 7;
                            (
                                b1,
                                b2,
                                base + self.word(pc + 3, 2)? as usize,
                                base + self.word(pc + 5, 2)? as usize,
                                Some(usize::from(op - 0x23)),
                            )
                        }
                        _ => {
                            let (m, k) = match op {
                                0x13 | 0x15 | 0x17 => (2, 0),
                                0x14 | 0x16 | 0x18 => (2, 255),
                                0x19 => (3, 0),
                                0x1a | 0x1b => (3, 255),
                                _ => (3, 510),
                            };
                            (
                                b1 * m - k,
                                b2 * m - k,
                                pc + 4,
                                pc + 4 + self.byte(pc + 3)? as usize,
                                None,
                            )
                        }
                    };
                    let (low, high) = (low as f64, high as f64);
                    let project = |v: Vec4| -> f64 {
                        let (x, y, z) = (ld(v[0]), ld(v[1]), ld(v[2]));
                        let r = |v: f64| self.x(v);
                        match (axis, op) {
                            (Some(a), _) => ld(v[a]),
                            (None, 0x13) => r(z + y),
                            (None, 0x14) => r(y - z),
                            (None, 0x15) => r(z + x),
                            (None, 0x16) => r(x - z),
                            (None, 0x17) => r(y + x),
                            (None, 0x18) => r(x - y),
                            (None, 0x19) => r(r(z + y) + x),
                            (None, 0x1a) => r(r(y + x) - z),
                            (None, 0x1b) => r(r(x - y) + z),
                            (None, _) => r(r(x - y) - z),
                        }
                    };
                    let s = project(seg.start);
                    let e = ld(st(project(seg.end)));
                    if e < high && s < high {
                        pc = first;
                        continue;
                    }
                    if s > low && e > low {
                        pc = second;
                        continue;
                    }
                    let sl = ld(st(self.x(s - low)));
                    let sh = ld(st(self.x(s - high)));
                    let el = ld(st(self.x(e - low)));
                    let eh = ld(st(self.x(e - high)));
                    let rising = sl < el;
                    let ((ns, ne), (fs, fe)) = if rising {
                        ((sl, el), (sh, eh))
                    } else {
                        ((sh, eh), (sl, el))
                    };
                    let mut copy = seg;
                    if self.x(ne * ns) < 0.0 {
                        let w = st(self.x(ns / self.x(ns - ne)));
                        copy.end = self.mix(w, seg.start, seg.end);
                    }
                    self.call(if rising { first } else { second }, frame, copy)?;
                    if self.x(fe * fs) < 0.0 {
                        let w = st(self.x(fs / self.x(fs - fe)));
                        seg.start = self.mix(w, seg.start, seg.end);
                    }
                    if ld(self.fraction) < 1.0 {
                        let world = self.mix(self.fraction, self.ray.from, self.ray.to);
                        let local = lanes(
                            [frame.scale; 4],
                            lanes(world, self.ray.origin, |x, y| x - y),
                            |x, y| x * y,
                        );
                        seg.end = lanes(local, frame.offset, |x, y| x - y);
                        if let Some(a) = axis {
                            let end = ld(seg.end[a]);
                            if (rising && end < high) || (!rising && end > low) {
                                return Some(());
                            }
                        }
                    }
                    pc = if rising { second } else { first };
                }
                _ => return None,
            }
        }
    }
}

#[test]
fn random_programs_walk_as_the_recursive_interpreter_does() {
    let mut compared = 0;
    for n in 0..4000u32 {
        let mut rng =
            Rng(0x2545_f491_4f6c_dd1d ^ u64::from(n).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let code = synth::random_mopp(&mut rng, 1 + n % 6);
        let point = |rng: &mut Rng| {
            [
                rng.eighths(160) + 120.0,
                rng.eighths(160) + 120.0,
                rng.eighths(160) + 120.0,
            ]
        };
        let (from, to) = (point(&mut rng), point(&mut rng));
        let precision = if n % 2 == 0 {
            Precision::Single
        } else {
            Precision::Double
        };
        let r = ray(from, to, precision);
        let fractions = [ONE, f(0.875), f(0.5), f(0.25), f(0.125)];
        let hit = move |k: u32, fr: u32| {
            let g = fractions[(k % 5) as usize];
            if k % 3 == 1 && f32::from_bits(g) < f32::from_bits(fr) {
                g
            } else {
                fr
            }
        };
        let Ok(native) = events(&code, r, hit) else {
            continue;
        };
        let root = r.root().unwrap();
        let mut reference = Reference {
            code: &code,
            ray: r,
            single: precision == Precision::Single,
            inverse_scale: root.inverse_scale,
            fraction: ONE,
            hit: &hit,
            out: Vec::new(),
        };
        assert!(
            reference.call(0, root.frame, root.segment).is_some(),
            "case {n}: the reference faults where the walk did not: {code:x?}"
        );
        assert_eq!(
            native, reference.out,
            "case {n}: {code:x?} from {from:?} to {to:?} {precision:?}"
        );
        compared += 1;
    }
    assert!(compared > 2000, "{compared} programs compared");
}
