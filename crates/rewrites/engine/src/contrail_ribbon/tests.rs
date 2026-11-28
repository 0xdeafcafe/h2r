use super::*;
use guest_layout::memory::FlatMemory;

const CW: u16 = 0x027f;
const MXCSR: u32 = 0x1f80;
const ESP: u32 = 0x004f_0000;
const OUT: u32 = 0x004d_0000;
const CTX: u32 = 0x004c_0000;
const C: u32 = 0x004c_1000;
const DEF: u32 = 0x004c_2000;
const MODE: u32 = 0x004c_3000;
const STAGES: u32 = 0x004c_4000;
const ARRAY: u32 = 0x004c_5000;
const DATA: u32 = 0x004c_6000;
const OBJECT_ARRAY: u32 = 0x004c_8000;
const OBJECTS_DATA: u32 = 0x004c_9000;
const DATUM: u32 = 0x004c_a000;
const TAG_ARRAY: u32 = 0x004c_b000;
const TAG: u32 = 0x004c_c000;
const BLOCK: u32 = 0x004c_d000;
const WHITE_VEC: u32 = 0x004c_e000;
const UP_VEC: u32 = 0x004c_e010;

fn f(v: f32) -> u32 {
    v.to_bits()
}

fn put(m: &mut FlatMemory, at: u32, v: f32) {
    m.set_u32(at, f(v)).unwrap();
}

/// A white ribbon of `points` points along x at `(i, 0, 0)`, facing
/// `facing`, width 1, step 0.25 from 1, no fade, no object, the camera
/// above at `(0, 0, 10)`.
fn world(facing: u16, points: u32) -> FlatMemory {
    let mut m = FlatMemory::new(0x0041_0000, 0x0010_0000);
    for (at, v) in [
        (ONE, 1.0),
        (ZERO, 0.0),
        (SMALL, 1e-4),
        (constant::HALF, 0.5),
        (constant::PACK, 255.0),
    ] {
        put(&mut m, at, v);
    }
    m.set_u32(constant::POINTS, ARRAY).unwrap();
    m.set_u32(ARRAY + 0x44, DATA).unwrap();
    m.set_u32(constant::WHITE, WHITE_VEC).unwrap();
    m.set_u32(constant::UP, UP_VEC).unwrap();
    for i in 0..3 {
        put(&mut m, WHITE_VEC + 4 * i, 1.0);
    }
    put(&mut m, UP_VEC + 8, 1.0);
    put(&mut m, constant::CAMERA + 8, 10.0);

    m.set_u32(CTX, C).unwrap();
    m.set_u32(CTX + 4, DEF).unwrap();
    m.set_u16(CTX + 8, 0).unwrap();
    m.set_u32(CTX + 0xc, MODE).unwrap();
    m.set_u32(CTX + 0x14, 2 * points).unwrap();

    m.set_u32(C + 8, u32::MAX).unwrap();
    put(&mut m, C + 0x14, 2.0);
    put(&mut m, C + 0x1c, 1.0);
    put(&mut m, C + 0x20, 0.0);
    m.set_u16(C + 0x30, 3).unwrap();
    m.set_u32(C + 0x38, if points == 0 { u32::MAX } else { 0 }).unwrap();

    m.set_u16(DEF + 0x18, facing).unwrap();
    put(&mut m, DEF + 0x1c, 0.25);
    put(&mut m, DEF + 0x20, 1.0);
    m.set_u32(DEF + 0xe8, 1).unwrap();
    m.set_u32(DEF + 0xec, STAGES).unwrap();
    put(&mut m, STAGES + 0x18, 1.0);
    for i in 0..8 {
        put(&mut m, STAGES + 0x1c + 4 * i, 1.0);
    }

    for i in 0..points {
        let p = DATA + 0x38 * i;
        put(&mut m, p + 0x1c, i as f32);
        let next = if i + 1 == points { u32::MAX } else { i + 1 };
        m.set_u32(p + 0x34, next).unwrap();
    }
    let args = [OUT, 0, CTX];
    for (i, a) in args.into_iter().enumerate() {
        m.set_u32(ESP + 4 + 4 * i as u32, a).unwrap();
    }
    m
}

fn word(p: &Prediction, at: u32) -> u32 {
    let i = (at - p.out) as usize;
    u32::from_le_bytes(p.vertices[i..i + 4].try_into().unwrap())
}

fn position(p: &Prediction, vertex: u32) -> [f32; 3] {
    [0, 4, 8].map(|o| f32::from_bits(word(p, OUT + 24 * vertex + o)))
}

#[test]
fn a_ribbon_has_two_vertices_per_point_facing_the_camera() {
    let m = world(4, 3);
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    assert_eq!((p.points, p.ret), (3, 1));
    assert_eq!(p.vertices.len(), 3 * 48);
    // Across the step (x) and the view (z): the pair is split in y.
    for i in 0..3 {
        assert_eq!(position(&p, 2 * i), [i as f32, 0.5, 0.0], "point {i}'s near vertex");
        assert_eq!(position(&p, 2 * i + 1), [i as f32, -0.5, 0.0], "point {i}'s far vertex");
    }
}

#[test]
fn the_texture_words_step_along_and_span_the_width() {
    let mut m = world(4, 3);
    m.set_u8(DEF + 2, 0x40).unwrap();
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    for i in 0..3u32 {
        let along = f(1.0 - 0.5 * i as f32);
        assert_eq!(word(&p, OUT + 48 * i + 0xc), along, "the step times the contrail's +0x14");
        assert_eq!(word(&p, OUT + 48 * i + 0x24), along);
        assert_eq!(word(&p, OUT + 48 * i + 0x10), f(0.0));
        assert_eq!(word(&p, OUT + 48 * i + 0x28), f(1.0));
    }
}

#[test]
fn the_end_points_fade() {
    let m = world(4, 3);
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    let colour = |v: u32| word(&p, OUT + 24 * v + 0x14);
    assert_eq!([0, 1].map(colour), [0x00ff_ffff; 2], "the first pair's alpha");
    assert_eq!([2, 3].map(colour), [0xffff_ffff; 2]);
    assert_eq!([4, 5].map(colour), [0x00ff_ffff; 2], "the last pair's alpha");

    // Definition bits 1 and 2 keep them; a short contrail keeps the first.
    let mut m = world(4, 3);
    m.set_u8(DEF, 3).unwrap();
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    assert!((0..6).all(|v| word(&p, OUT + 24 * v + 0x14) == 0xffff_ffff));
    let mut m = world(4, 3);
    m.set_u16(C + 0x30, 2).unwrap();
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    assert_eq!(word(&p, OUT + 0x14), 0xffff_ffff);
}

#[test]
fn the_vertical_facing_stacks_the_pair_and_the_flat_one_spreads_it() {
    let p = predict(&world(0, 2), ESP, MXCSR, CW).unwrap();
    assert_eq!(position(&p, 0), [0.0, 0.0, 0.5]);
    assert_eq!(position(&p, 1), [0.0, 0.0, -0.5]);
    let p = predict(&world(1, 2), ESP, MXCSR, CW).unwrap();
    // Across the step in x and y: (-dy, dx) is (0, 1).
    assert_eq!(position(&p, 2), [1.0, 0.5, 0.0]);
    assert_eq!(position(&p, 3), [1.0, -0.5, 0.0]);
}

#[test]
fn the_fade_is_the_views_angle_to_the_normal() {
    // The camera at (1, 0, 1) from the first point: 45 degrees off the
    // camera-facing normal (0, 0, 1) when the facing leaves it.
    let mut m = world(4, 2);
    m.set_u8(DEF, 3).unwrap();
    m.set_u16(MODE + 0x2c, 1).unwrap();
    m.set_u8(CTX + 0x10, 1).unwrap();
    put(&mut m, constant::CAMERA, 1.0);
    put(&mut m, constant::CAMERA + 8, 1.0);
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    let alpha = word(&p, OUT + 0x14) >> 24;
    let want = (std::f32::consts::FRAC_1_SQRT_2 * 255.0).round() as u32;
    assert_eq!(alpha, want);
    // Mode 2 inverts it.
    m.set_u16(MODE + 0x2c, 2).unwrap();
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    assert_eq!(word(&p, OUT + 0x14) >> 24, 255 - want);
}

#[test]
fn a_stage_out_of_range_is_black_and_thin() {
    let mut m = world(4, 2);
    m.set_u8(DEF, 3).unwrap();
    m.set_u8(DATA + 3, 1).unwrap();
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    assert_eq!(word(&p, OUT + 0x14), 0);
    assert_eq!(position(&p, 0), position(&p, 1), "no width");
    m.set_u8(DATA + 3, 0xff).unwrap();
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    assert_eq!(word(&p, OUT + 0x14), 0, "a negative stage too");
}

#[test]
fn a_point_blends_its_stage_into_the_next() {
    let mut m = world(4, 2);
    m.set_u8(DEF, 3).unwrap();
    m.set_u32(DEF + 0xe8, 2).unwrap();
    put(&mut m, STAGES + 0x40 + 0x18, 3.0);
    m.set_u8(DATA + 2, 2).unwrap();
    put(&mut m, DATA + 4, 0.5);
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    // Width 1 to 3 halfway; the next stage's colour 0 halves the lanes.
    assert_eq!(position(&p, 0)[1], 1.0);
    assert_eq!(word(&p, OUT + 0x14), 0x8080_8080);
}

#[test]
fn an_object_tints_the_colour_by_its_function_source() {
    let mut m = world(4, 2);
    m.set_u8(DEF, 3).unwrap();
    m.set_u32(C + 8, 0x0001_0002).unwrap();
    m.set_u16(C + 0xc, 1).unwrap();
    m.set_u32(constant::OBJECTS, OBJECT_ARRAY).unwrap();
    m.set_u32(OBJECT_ARRAY + 0x44, OBJECTS_DATA).unwrap();
    m.set_u32(OBJECTS_DATA + 2 * 12 + 8, DATUM).unwrap();
    m.set_u32(DATUM, 5).unwrap();
    m.set_u32(constant::TAGS, TAG_ARRAY).unwrap();
    m.set_u32(TAG_ARRAY + 5 * 16 + 8, TAG).unwrap();
    m.set_u32(TAG + 0x98, BLOCK).unwrap();
    m.set_u16(BLOCK + 24 + 0xc, 1).unwrap();
    // Two sources (48 bytes over 24), their colours after the count.
    m.set_u16(DATUM + 0x124, 48).unwrap();
    m.set_u16(DATUM + 0x126, 0x200).unwrap();
    let colour = DATUM + 0x200 + 3 * 12;
    for (i, v) in [0.5, 0.0, 1.0].into_iter().enumerate() {
        put(&mut m, colour + 4 * i as u32, v);
    }
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    assert_eq!(word(&p, OUT + 0x14), 0xff80_00ff);

    // An index past the sources keeps the untinted colour.
    m.set_u16(BLOCK + 24 + 0xc, 2).unwrap();
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    assert_eq!(word(&p, OUT + 0x14), 0xffff_ffff);
}

#[test]
fn an_unknown_facing_returns_zero_after_the_texture_words() {
    let m = world(3, 2);
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    assert_eq!((p.points, p.ret), (1, 0));
    assert_eq!(p.writes.len(), 16, "the four texture words only");
    assert_eq!(word(&p, OUT + 0xc), f(1.0));
}

#[test]
fn an_empty_list_still_clears_the_ends_it_is_told_of() {
    // No points, a count of 0: both ends are read around `out`.
    let mut m = world(4, 0);
    m.set_u32(CTX + 0x14, 0).unwrap();
    m.set_u32(OUT + 0x14, 0xffff_ffff).unwrap();
    m.set_u32(OUT - 4, 0x1234_5678).unwrap();
    let p = predict(&m, ESP, MXCSR, CW).unwrap();
    assert_eq!((p.points, p.ret), (0, 1));
    assert!(p.vertices.is_empty());
    let at = |a: u32| u32::from_le_bytes([0, 1, 2, 3].map(|i| p.writes[&(a + i)]));
    assert_eq!([at(OUT + 0x14), at(OUT + 0x2c)], [0x00ff_ffff; 2], "the first pair's");
    assert_eq!([at(OUT - 4), at(OUT - 0x1c)], [0x0034_5678; 2], "the last pair's, from the end");
}

#[test]
fn a_cycle_and_a_fault_are_named_refusals() {
    let mut m = world(4, 2);
    m.set_u32(DATA + 0x38 + 0x34, 0).unwrap();
    assert_eq!(refusal(predict(&m, ESP, MXCSR, CW).unwrap_err()), "unbounded");
    let mut m = world(4, 2);
    m.set_u32(CTX + 4, 0x0100_0000).unwrap();
    assert_eq!(refusal(predict(&m, ESP, MXCSR, CW).unwrap_err()), "fault");
}
