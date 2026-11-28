use super::*;
use guest_layout::memory::FlatMemory;

const CW: u16 = 0x027f;
const MXCSR: u32 = 0x1f80;
const ESP: u32 = 0x004f_0000;
const DEF: u32 = 0x004c_0000;
const PART: u32 = 0x004c_1000;
const STATE: u32 = 0x004c_2000;
const RECORD: u32 = 0x004c_3000;
const DRAW: u32 = 0x004c_4000;
const TINT_ARG: u32 = 0x004c_5000;
const BLOBS: u32 = 0x004c_6000;
const BITMAP: u32 = 0x004c_8000;
const DEFAULT: u32 = 0x004c_9000;

fn f(v: f32) -> u32 {
    v.to_bits()
}

/// A property of input `input`, its function the identity mapped to
/// `lo..hi`.
fn property(m: &mut FlatMemory, at: u32, input: i16, lo: f32, hi: f32) {
    let blob = BLOBS + (at - DEF) * 0x10;
    m.set_u16(at, input as u16).unwrap();
    m.set_u16(at + 2, 0).unwrap();
    m.set_u16(at + 4, 0).unwrap();
    m.set_u16(at + 6, u16::MAX).unwrap();
    m.set_u32(at + 8, 1).unwrap();
    m.set_u32(at + 0xc, blob).unwrap();
    m.set_u8(blob, 0).unwrap();
    m.set_u8(blob + 1, 0).unwrap();
    m.set_u32(blob + 4, f(lo)).unwrap();
    m.set_u32(blob + 8, f(hi)).unwrap();
}

/// A definition with flags `flags` reading inputs 0 and 1: alpha the
/// particle's `+8`, size and rotation its `+0x10`, the default colour.
fn world(flags: u32) -> FlatMemory {
    let mut m = FlatMemory::new(0x0041_0000, 0x000f_0000);
    for (at, v) in [
        (ONE, 1.0),
        (ZERO, 0.0),
        (BYTE, 1.0 / 255.0),
        (constant::HALF, 0.5),
        (constant::TINT, 2.0),
        (constant::CORNER, 1.0),
        (constant::ANGLE, 2.0),
        (constant::SPIN, 0.5),
        (constant::WORD, 1.0 / 65535.0),
        (constant::PACK, 255.0),
    ] {
        m.set_u32(at, f(v)).unwrap();
    }
    m.set_u32(0x0046_86cc, DEFAULT).unwrap();
    for i in 1..4 {
        m.set_u32(DEFAULT + 4 * i, f(1.0)).unwrap();
    }
    m.set_u32(DEF, flags).unwrap();
    m.set_u32(DEF + 0x98, 0x3).unwrap();
    // The colour has no function: the default's.
    m.set_u32(DEF + 0x1c + 8, 0).unwrap();
    m.set_u32(DEF + 0x1c + 0xc, BLOBS + 0x800).unwrap();
    m.set_u8(BLOBS + 0x800, 0).unwrap();
    property(&mut m, DEF + 0x2c, 0, 0.0, 1.0);
    property(&mut m, DEF + 0x3c, 1, 0.0, 1.0);
    property(&mut m, DEF + 0x4c, 1, 0.0, 1.0);
    property(&mut m, DEF + 0x5c, 0, 0.0, 1.0);
    for (i, v) in [0.1, 0.2, 0.3, 0.4].into_iter().enumerate() {
        m.set_u32(DEF + 0x9c + 4 * i as u32, f(v)).unwrap();
    }
    for (i, v) in [0.0, 0.5, 0.25, 1.0].into_iter().enumerate() {
        m.set_u32(DEF + 0xac + 4 * i as u32, f(v)).unwrap();
    }
    m.set_u32(STATE + 0x50, RECORD).unwrap();
    m.set_u32(STATE + 0x4c, 0).unwrap();
    m.set_u32(RECORD + 8, f(0.25)).unwrap();
    m.set_u32(RECORD + 0x10, f(0.5)).unwrap();
    m.set_u32(DRAW, DEF).unwrap();
    m.set_u32(DRAW + 0xc, f(1.0)).unwrap();
    for i in 0..4 {
        m.set_u32(TINT_ARG + 4 * i, f(1.0)).unwrap();
    }
    let args = [PART, STATE, 0x004d_0000, 0x004d_0010, f(2.0), TINT_ARG, DRAW];
    for (i, a) in args.into_iter().enumerate() {
        m.set_u32(ESP + 4 + 4 * i as u32, a).unwrap();
    }
    m
}

fn written(p: &Prediction, at: u32) -> Option<u32> {
    let b: Option<Vec<u8>> = (0..4).map(|i| p.writes.get(&(at + i)).copied()).collect();
    b.map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

#[test]
fn the_inputs_the_definition_reads_are_cached_in_bit_order_and_masked() {
    let m = world(0x1_0000);
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    assert_eq!(written(&p, STATE), Some(f(0.25)), "input 0: the record's +8");
    assert_eq!(written(&p, STATE + 4), Some(f(0.5)), "input 1: its +0x10");
    assert_eq!(written(&p, STATE + 0x44), Some(3));

    // A cached input is read, not made again.
    let mut m = world(0x1_0000);
    m.set_u32(STATE + 0x44, 1).unwrap();
    m.set_u32(STATE, f(0.75)).unwrap();
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    assert_eq!(written(&p, STATE), None);
    assert_eq!(p.emission.words[4] >> 24, 191, "alpha 0.75 of the cached input");
}

#[test]
fn the_emitter_words_point_into_the_callers_frame() {
    let m = world(0x1_2000);
    let e = predict(&m, ESP, MXCSR, CW).unwrap().emission;
    assert_eq!(e.words[0], 0x004d_0000, "position");
    assert_eq!(e.words[1], 0x004d_0010, "axis");
    assert_eq!(e.words[2], f(1.0), "size: 0.5 times the argument 2");
    assert_eq!(e.words[3], f(1.5), "rotation: 0.5 times 2, plus the spin");
    assert_eq!(e.words[4], 0x40ff_ffff, "alpha 0.25, the default white");
    assert_eq!(e.words[5], 0, "no blend without a bitmap");
    assert_eq!(e.words[6], ESP - 0x30);
    assert_eq!(e.words[7], ESP - 0x10);
    assert_eq!(e.ebx, ESP - 0x40);
    assert_eq!(e.window, [0.0, 0.25, 0.5, 0.75].map(f));
    assert_eq!(e.next_window, e.window);
    assert_eq!(e.corners, [0.1, 0.2, 0.3, 0.4].map(f));
}

#[test]
fn a_definition_window_flips_with_the_particle() {
    let mut m = world(0x1_0006);
    m.set_u8(PART + 0x14, 1).unwrap();
    let e = predict(&m, ESP, MXCSR, CW).unwrap().emission;
    assert_eq!(e.window, [0.5, 0.25, -0.5, 0.75].map(f), "u flipped");
    m.set_u8(PART + 0x16, 1).unwrap();
    let e = predict(&m, ESP, MXCSR, CW).unwrap().emission;
    assert_eq!(e.window, [0.5, 1.0, -0.5, -0.75].map(f), "both");
}

#[test]
fn the_colour_lanes_are_clamped_before_they_are_packed() {
    let mut m = world(0x1_0000);
    m.set_u32(TINT_ARG, f(8.0)).unwrap();
    m.set_u32(TINT_ARG + 8, f(-1.0)).unwrap();
    let e = predict(&m, ESP, MXCSR, CW).unwrap().emission;
    assert_eq!(e.words[4], 0xffff_00ff);
}

#[test]
fn a_sequence_bitmap_picks_the_particles_sequence_and_blends_two_frames() {
    let mut m = world(0x10);
    m.set_u32(DRAW + 4, BITMAP).unwrap();
    m.set_u16(BITMAP, 3).unwrap();
    m.set_u32(BITMAP + 0x3c, 2).unwrap();
    let sequences = BITMAP + 0x100;
    m.set_u32(BITMAP + 0x40, sequences).unwrap();
    m.set_u16(PART + 0x16, 3).unwrap();
    m.set_u16(DEF + 0xa, 2).unwrap();
    // Sequence 1 of 2: four square sprites.
    let seq = sequences + 0x3c;
    let sprites = BITMAP + 0x400;
    m.set_u32(seq + 0x34, 4).unwrap();
    m.set_u32(seq + 0x38, sprites).unwrap();
    for s in 0..4u32 {
        let at = sprites + 32 * s;
        let u = s as f32 / 4.0;
        for (i, v) in [u, u + 0.25, 0.0, 0.25, 1.0, 1.0].into_iter().enumerate() {
            m.set_u32(at + 8 + 4 * i as u32, f(v)).unwrap();
        }
    }
    m.set_u32(RECORD + 8, f(0.3)).unwrap();
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    assert_eq!(written(&p, DRAW + 8), Some(seq), "the particle's +0x16 mod 2, clamped");
    let t = 0.3f32 * 4.0;
    let e = p.emission;
    assert_eq!(e.words[5], f(t - 1.0), "the blend past frame 1");
    assert_eq!(e.window, [0.25, 0.0, 0.25, 0.25].map(f));
    assert_eq!(e.next_window, [0.5, 0.0, 0.25, 0.25].map(f));
    // Square frames, unit spread: the corners are the identity's.
    assert!(e.corners.iter().all(|c| f32::from_bits(*c).is_finite()));

    // Flag 0x20 keeps the frame and does not blend.
    m.set_u32(DEF, 0x30).unwrap();
    let e = predict(&m, ESP, MXCSR, CW).unwrap().emission;
    assert_eq!(e.words[5], 0);
}

#[test]
fn what_the_model_cannot_read_is_a_named_refusal() {
    let mut m = world(0x1_0000);
    m.set_u32(DRAW, 0x0100_0000).unwrap();
    let e = predict(&m, ESP, MXCSR, CW).unwrap_err();
    assert_eq!(refusal(e), "fault");
    let mut m = world(0x1_0000);
    m.set_u32(DEF + 0x98, 0x10).unwrap();
    // Bit 4 is outside the callback's mask: nothing is computed for it.
    assert!(predict(&m, ESP, MXCSR, CW).is_ok());
}
