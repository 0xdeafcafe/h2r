//! The curve kernel against its register-level oracle
//! (`crate::function_eval`, the instruction-for-instruction port of
//! `13b390`), on synthetic memory: no image, no disc. The oracle runs
//! under its own SSE model; the kernel under the float rules; every
//! answerable call must agree bit for bit, and the kinds the kernel does
//! not carry must decline named.

use crate::function_eval::{self, constant};
use crate::light_constants::Sse;
use guest_layout::memory::{FlatMemory, GuestMemory};
use sim_objects::functions::curve::{self, Answer, Decline, Image};

const REFERENCE: u32 = 0x0048_7ff0;
const DATA: u32 = 0x0048_8000;

/// The constants the evaluator reads and four sub-types of each table:
/// ramps up (1 and 2, distinguishable slopes) and ramps down (6 and 7,
/// the wrapping sub-types).
fn image() -> FlatMemory {
    let mut m = FlatMemory::new(0x0041_0000, 0x0008_0000);
    for (at, v) in [
        (constant::ONE, 1.0f32),
        (constant::SHAPE_SCALE, 1023.0),
        (constant::SHAPE_BIAS, 0.1),
        (constant::WAVE_SCALE, 1024.0 / 28.0),
        (constant::BYTE, 1.0 / 255.0),
        (constant::WRAP_HIGH, 0.75),
        (constant::WRAP_LOW, 0.25),
    ] {
        m.set_f32(at, v).unwrap();
    }
    for sub in 1..=2u32 {
        for i in 0..1024u32 {
            m.set_u8(constant::SHAPES + (sub << 10) + i, ((i / 4) * sub) as u8).unwrap();
            m.set_u8(constant::WAVES + (sub << 10) + i, ((i / 4) * sub) as u8).unwrap();
        }
    }
    for sub in 6..=7u32 {
        for i in 0..1024u32 {
            let v = 255 - ((i / 4) * (sub - 5)) as u8;
            m.set_u8(constant::SHAPES + (sub << 10) + i, v).unwrap();
            m.set_u8(constant::WAVES + (sub << 10) + i, v).unwrap();
        }
    }
    m
}

fn kernel_image(m: &FlatMemory) -> Image {
    Image {
        one: m.u32(constant::ONE).unwrap(),
        shape_scale: m.u32(constant::SHAPE_SCALE).unwrap(),
        shape_bias: m.u32(constant::SHAPE_BIAS).unwrap(),
        wave_scale: m.u32(constant::WAVE_SCALE).unwrap(),
        byte: m.u32(constant::BYTE).unwrap(),
        wrap_high: m.u32(constant::WRAP_HIGH).unwrap(),
        wrap_low: m.u32(constant::WRAP_LOW).unwrap(),
        shapes: constant::SHAPES,
        waves: constant::WAVES,
    }
}

/// A blob of `kind` with `flags` and the two sub-type bytes, its params
/// `words` at `+0x14` on.
fn blob(m: &mut FlatMemory, kind: u8, flags: u8, sub: [u8; 2], words: &[f32]) {
    m.write(DATA, &[0u8; 0x200]).unwrap();
    m.set_u8(DATA, kind).unwrap();
    m.set_u8(DATA + 1, flags).unwrap();
    m.set_u8(DATA + 2, sub[0]).unwrap();
    m.set_u8(DATA + 3, sub[1]).unwrap();
    for (i, &w) in words.iter().enumerate() {
        m.set_f32(DATA + 0x14 + 4 * i as u32, w).unwrap();
    }
    m.set_u32(REFERENCE, 0x100).unwrap();
    m.set_u32(REFERENCE + 4, DATA).unwrap();
}

fn oracle(m: &FlatMemory, x: f32, r: f32) -> u32 {
    function_eval::evaluate(
        m,
        &mut Sse { mxcsr: 0x1f80 },
        0x27f,
        REFERENCE,
        x.to_bits(),
        r.to_bits(),
    )
    .unwrap()
}

fn kernel(m: &FlatMemory, x: f32, r: f32) -> Answer {
    let image = kernel_image(m);
    curve::evaluate(
        &image,
        0x1fa0,
        0x27f,
        REFERENCE,
        x.to_bits(),
        r.to_bits(),
        |at| m.u32(at).map_err(|_| ()),
        |at| m.u8(at).map_err(|_| ()),
    )
    .unwrap()
}

/// Enough params for every curve's two sets.
const WORDS: &[f32] = &[
    0.25, -0.5, 2.0, 0.75, // +0x00: kind 2's and 3's first set
    0.1, 1.25, 0.0, -1.5, // +0x10
    0.5, 0.05, 3.0, 0.6, // +0x20 (kind 4's first set at +0x10..+0x14)
    0.35, 0.85, 0.45, 0.95, // +0x30 (kind 7's first cubic at +0x34)
    0.15, 0.65, 0.55, 1.75, // +0x40 (kind 5's weights; kind 7's second at +0x64)
];

#[test]
fn the_kernel_agrees_with_the_port_across_kinds_blends_and_inputs() {
    let mut m = image();
    for (kind, sub) in [
        (0u8, [0u8, 0]),
        (1, [0, 0]),
        (2, [1, 2]),
        (2, [0, 0]),
        (3, [1, 6]),
        (3, [0, 0]),
        (4, [0, 0]),
        (5, [0, 0]),
        (7, [0, 0]),
    ] {
        for flags in [0u8, 1] {
            blob(&mut m, kind, flags, sub, WORDS);
            for x in [-0.5f32, 0.0, 0.1, 1.0 / 3.0, 0.5, 0.9, 1.0, 1.5] {
                for r in [0.0f32, 0.25, 0.75, 1.0] {
                    let (want, got) = (oracle(&m, x, r), kernel(&m, x, r));
                    assert_eq!(
                        got,
                        Answer::Value(want),
                        "kind {kind} flags {flags} subs {sub:?} x {x} r {r}",
                    );
                }
            }
        }
    }
}

#[test]
fn the_keyed_curves_read_their_second_set_at_plus_0x50() {
    let mut m = image();
    // Kind 5, blended: the second set's starts, slopes and weights differ,
    // so a wrong base would show at once. First set: starts at +0x24,
    // slopes at +0x34, constant +0x40, weights +0x44..+0x4c; the second
    // set is the same layout at +0x50, its weights ending at +0x9c.
    let mut words = [0.0f32; 0xa0 / 4];
    for (at, v) in [
        (0x24usize, -0.1f32),
        (0x28, 0.3),
        (0x2c, 0.6),
        (0x34, 1.0),
        (0x38, 2.0),
        (0x3c, -1.0),
        (0x40, 0.1),
        (0x44, 0.2),
        (0x48, 0.3),
        (0x4c, 0.5),
        (0x74, 0.2),
        (0x78, 0.5),
        (0x7c, 0.8),
        (0x84, 3.0),
        (0x88, -2.0),
        (0x8c, 0.5),
        (0x90, -0.2),
        (0x94, 0.6),
        (0x98, 0.1),
        (0x9c, 0.4),
    ] {
        words[at / 4] = v;
    }
    blob(&mut m, 5, 1, [0, 0], &words);
    for (x, r) in [(0.2f32, 0.0), (0.4, 0.5), (0.8, 1.0)] {
        assert_eq!(kernel(&m, x, r), Answer::Value(oracle(&m, x, r)), "x {x} r {r}");
    }
}

#[test]
fn kinds_six_eight_and_above_ten_are_zero_in_both() {
    let mut m = image();
    for kind in [6u8, 8, 11] {
        blob(&mut m, kind, 1, [1, 1], WORDS);
        assert_eq!(kernel(&m, 0.5, 0.5), Answer::Value(0), "kind {kind}");
        assert_eq!(oracle(&m, 0.5, 0.5), 0, "kind {kind}");
    }
}

#[test]
fn an_empty_reference_is_zero_in_both() {
    let mut m = image();
    blob(&mut m, 0, 0, [0, 0], WORDS);
    m.set_u32(REFERENCE, 0).unwrap();
    assert_eq!(kernel(&m, 0.5, 0.5), Answer::Value(0));
    assert_eq!(oracle(&m, 0.5, 0.5), 0);
    m.set_u32(REFERENCE, 0x100).unwrap();
    m.set_u32(REFERENCE + 4, 0).unwrap();
    assert_eq!(kernel(&m, 0.5, 0.5), Answer::Value(0));
    assert_eq!(oracle(&m, 0.5, 0.5), 0);
}

#[test]
fn kinds_nine_and_ten_go_to_the_port() {
    let mut m = image();
    for kind in [9u8, 10] {
        blob(&mut m, kind, 0, [0, 0], WORDS);
        assert_eq!(kernel(&m, 0.5, 0.5), Answer::Declined(Decline::Kind), "kind {kind}");
    }
}
