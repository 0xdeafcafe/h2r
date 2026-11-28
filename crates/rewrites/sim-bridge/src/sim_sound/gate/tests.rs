//! Hand vectors for the gate and its helpers. The image constants are
//! test values, each named where it is set.

use super::*;
use guest_layout::points::{GLOBALS_TAG, TAG_ENTRIES};
use guest_layout::testing::FakeGuest;

const TAG: u32 = 0xe100_0003;
const ENTRIES: u32 = 0x0060_0000;
const DEF: u32 = 0x0060_1000;
const GLOBALS: u32 = 0x0060_2000;
const GEST: u32 = 0x0060_3000;
const DEFAULTS: u32 = 0x0060_4000;
const SCALES: u32 = 0x0060_5000;
const ROWS: u32 = 0x0060_6000;
const RNG: u32 = 0x0060_7000;
const USERS: u32 = 0x0060_8000;
const GTAG: u32 = 0x0060_9000;
const CLASSES_TAG: u32 = 0x0060_a000;
const CLASSES: u32 = 0x0060_b000;
const STATES: u32 = 0x0060_c000;
const RECORD: u32 = 0x0060_d000;
const SOURCE: u32 = RECORD + 0xc;
const B: u32 = 0x0060_e000;
const FINE: u32 = 0x0060_f000;
const TABLE: u32 = 0x0061_0000;
const COARSE: u32 = 0x0061_1000;
const CLASS: u32 = 5;
const FAR_TEST: f32 = 1.0e30;

fn b(v: f32) -> u32 {
    v.to_bits()
}

fn f(v: u32) -> f32 {
    f32::from_bits(v)
}

fn listener(n: u32) -> u32 {
    GLOBALS + 0x88 + n * 0x48
}

fn guest() -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(GLOBALS, 0x1000).map(RECORD, 0x1000).map(DEF, 0x1000);
    // Test constants: floor -64, shortest 0.001, one, close 1/16, far,
    // seed scale 2^-16, limit 4, coarse step 0.5, fine step 0.125.
    g.put_f32(FLOOR, -64.0)
        .put_f32(SHORTEST, 0.001)
        .put_f32(ONE, 1.0)
        .put_f32(CLOSE, 0.0625)
        .put_f32(FAR, FAR_TEST)
        .put_f32(SEED_SCALE, 1.0 / 65536.0)
        .put_f32(OBSTRUCTION_LIMIT, 4.0)
        .put_f32(COARSE_STEP, 0.5)
        .put_f32(FINE_STEP, 0.125)
        .put_u32(CLASS_SWITCH, 0);
    g.put_u32(SOUND_GLOBALS.base, GLOBALS).put_u8(GLOBALS + 0x78, 1).put_u8(GLOBALS + 0x79, 1).put_u8(GLOBALS + 0x7a, 1);
    g.put_u32(TAG_ENTRIES.base, ENTRIES).put_u32(ENTRIES + 3 * 16 + 8, DEF).put_u32(ENTRIES + 7 * 16 + 8, CLASSES_TAG);
    g.put_u8(DEF + 2, CLASS as u8)
        .put_u8(DEF + 5, 1)
        .put_u16(DEF + 6, 1)
        .put_u16(DEF + 8, 0)
        .put_u8(DEF + 0xa, 1)
        .put_u8(DEF + 0xb, 2);
    g.put_u32(GESTALT, GEST).put_u32(GEST + 4, DEFAULTS).put_u32(GEST + 0xc, SCALES).put_u32(GEST + 0x24, ROWS);
    let d = DEFAULTS + 0x38;
    g.put_f32(d, 1.0).put_f32(d + 4, 10.0).put_f32(d + 8, 0.0).put_f32(d + 0x10, 0.0);
    let s = SCALES + 2 * 20;
    g.put_f32(s, 0.0).put_f32(s + 4, 0.0).put_f32(s + 0xc, 1.0).put_f32(s + 0x10, 1.0);
    g.put_u16(ROWS + 0xa, 1);
    g.put_u32(guest_layout::points::RANDOM_SEEDS.base, RNG).put_u32(RNG + 4, 0);
    g.put_u32(PLAYER_GLOBALS.base, USERS).put_u32(USERS + 0xc, 0).put_u32(USERS + 0x10, NONE);
    g.put_u32(GLOBALS_TAG.base, GTAG).put_u32(GTAG + 0xc0, 1).put_u32(GTAG + 0xc4, GTAG + 0x100).put_u32(GTAG + 0x104, 7);
    g.put_u32(CLASSES_TAG + 4, CLASSES).map(CLASSES, 0x1000);
    g.put_u32(super::super::levels::CLASS_STATES, STATES).put_u8(STATES + CLASS * 16 + 0xc, 1);
    g.put_u32(BSP, B);
    g
}

/// A positioned point source at the origin in cluster 2, and listeners 1
/// (active, at 3, 4, 0) and 2 (inactive, at 1, 0, 0) in cluster 2.
fn positioned(g: &mut FakeGuest) {
    g.put_u8(SOURCE + 3, 1).put_u16(SOURCE + 0x2c, 2).put_u32(SOURCE + 0x28, 8);
    g.put_u8(listener(1) + 6, 1).put_f32(listener(1) + 0x30, 3.0).put_f32(listener(1) + 0x34, 4.0).put_u16(listener(1) + 4, 2);
    g.put_f32(listener(2) + 0x30, 1.0).put_u16(listener(2) + 4, 2);
}

#[test]
fn sound_off_refuses_before_the_draw() {
    let mut g = guest();
    g.put_u8(GLOBALS + 0x79, 0);
    let r = start_gate(&mut g, RECORD, TAG).unwrap();
    assert_eq!(r, Gate { refused: Some(Refusal::Off), listener: None });
    assert_eq!(g.u32(RNG + 4).unwrap(), 0);
}

#[test]
fn a_class_with_plus_4_of_2_needs_the_switch() {
    let mut g = guest();
    g.put_u8(DEF + 4, 2);
    assert_eq!(start_gate(&mut g, RECORD, TAG).unwrap().refused, Some(Refusal::Off));
    g.put_u32(CLASS_SWITCH, 1);
    assert_eq!(start_gate(&mut g, RECORD, TAG).unwrap().refused, None);
}

#[test]
fn the_draw_steps_the_seed_and_a_full_chance_skips() {
    let mut g = guest();
    g.put_f32(DEFAULTS + 0x38 + 8, 1.0);
    let r = start_gate(&mut g, RECORD, TAG).unwrap();
    assert_eq!(r.refused, Some(Refusal::Skipped));
    assert_eq!(g.u32(RNG + 4).unwrap(), 0x3c6e_f35f);
    // 0x3c6e / 65536 = 0.236 is over a chance of 0.2.
    g.put_u32(RNG + 4, 0).put_f32(DEFAULTS + 0x38 + 8, 0.2);
    assert_eq!(start_gate(&mut g, RECORD, TAG).unwrap().refused, None);
}

#[test]
fn an_unpositioned_sound_opens_for_the_first_user_at_zero_db() {
    let mut g = guest();
    let r = start_gate(&mut g, RECORD, TAG).unwrap();
    assert_eq!(r, Gate { refused: None, listener: Some(0) });
}

#[test]
fn under_the_floor_refuses_unless_word_0_bit_8_or_11_plays_it() {
    let mut g = guest();
    g.put_f32(DEFAULTS + 0x38 + 0x10, -100.0);
    assert_eq!(start_gate(&mut g, RECORD, TAG).unwrap(), Gate { refused: Some(Refusal::Inaudible), listener: Some(0) });
    g.put_u16(SOURCE, 0x800);
    assert_eq!(start_gate(&mut g, RECORD, TAG).unwrap().refused, None);
}

#[test]
fn a_class_without_its_row_word_is_inaudible() {
    let mut g = guest();
    g.put_u16(ROWS + 0xa, 0);
    assert!(!class_plays(&g, TAG).unwrap());
    assert_eq!(start_gate(&mut g, RECORD, TAG).unwrap(), Gate { refused: Some(Refusal::Inaudible), listener: None });
}

#[test]
fn distances_take_the_source_then_the_class_then_the_default_row() {
    let mut g = guest();
    assert_eq!(distances(&g, TAG, SOURCE).unwrap(), [b(1.0), b(10.0)]);
    let class = CLASSES + CLASS * 0x5c;
    g.put_u8(DEF + 1, 0xc).put_f32(class + 0x18, 2.0).put_f32(class + 0x1c, 20.0);
    assert_eq!(distances(&g, TAG, SOURCE).unwrap(), [b(2.0), b(20.0)]);
    g.put_u8(SOURCE, 0x4).put_f32(SOURCE + 0x34, 3.0).put_f32(SOURCE + 0x38, 30.0);
    assert_eq!(distances(&g, TAG, SOURCE).unwrap(), [b(3.0), b(20.0)]);
}

#[test]
fn the_nearest_active_listener_hears_a_positioned_source_within_range() {
    let mut g = guest();
    positioned(&mut g);
    assert_eq!(f(source_distance(&g, SOURCE, 1).unwrap()), 25.0);
    assert_eq!(select_listener(&g, SOURCE, b(10.0)).unwrap(), (1, b(5.0)));
    assert_eq!(select_listener(&g, SOURCE, b(4.0)).unwrap(), (NONE, b(FAR_TEST)));
    g.put_u16(SOURCE, 0x100);
    assert_eq!(select_listener(&g, SOURCE, b(4.0)).unwrap(), (1, b(5.0)));
}

#[test]
fn an_unknown_source_cluster_is_obstructed_and_falls_back_on_bit_8() {
    let mut g = guest();
    positioned(&mut g);
    g.put_u16(SOURCE + 0x2c, u16::MAX);
    assert_eq!(select_listener(&g, SOURCE, b(10.0)).unwrap(), (NONE, b(5.0)));
    g.put_u16(SOURCE, 0x100);
    assert_eq!(select_listener(&g, SOURCE, b(10.0)).unwrap(), (0, b(5.0)));
}

#[test]
fn a_column_hears_only_a_listener_inside_its_height() {
    let mut g = guest();
    positioned(&mut g);
    g.put_u8(SOURCE + 2, 1).put_f32(SOURCE + 0x30, 5.0);
    g.put_f32(listener(1) + 0x38, 2.0);
    assert_eq!(f(source_distance(&g, SOURCE, 1).unwrap()), 25.0);
    g.put_f32(listener(1) + 0x38, 6.0);
    assert_eq!(f(source_distance(&g, SOURCE, 1).unwrap()), FAR_TEST);
    g.put_f32(listener(1) + 0x38, -1.0);
    assert_eq!(f(source_distance(&g, SOURCE, 1).unwrap()), FAR_TEST);
}

#[test]
fn a_directional_source_answers_its_vector_length() {
    let mut g = guest();
    g.put_u8(SOURCE + 3, 2).put_f32(SOURCE + 0xc, 3.0).put_f32(SOURCE + 0x10, 4.0).put_f32(SOURCE + 0x14, 12.0);
    assert_eq!(f(source_distance(&g, SOURCE, 0).unwrap()), 13.0);
    g.put_u8(SOURCE + 3, 0);
    assert_eq!(source_distance(&g, SOURCE, 0).unwrap(), 0);
}

#[test]
fn cluster_distance_reads_the_fine_table_between_its_bounds() {
    let mut g = guest();
    g.put_u32(B + 0x224, 1).put_u32(B + 0x9c, 4).put_u32(B + 0x228, FINE);
    g.put_u32(FINE + 0x28, TABLE).put_f32(FINE + 4, 0.0).put_f32(FINE + 8, 10.0);
    // Pair (1, 3) of 4: row bias 2, upper 3: byte 4.
    g.put_u8(TABLE + 4, 3);
    assert_eq!(f(cluster_distance(&g, B, 1, 3).unwrap()), 2.5);
    assert_eq!(f(cluster_distance(&g, B, 3, 1).unwrap()), 2.5);
    assert_eq!(cluster_distance(&g, B, 2, 2).unwrap(), 0);
    g.put_u8(TABLE + 4, 0xff);
    assert_eq!(f(cluster_distance(&g, B, 1, 3).unwrap()), 4.0);
    g.put_u8(TABLE + 4, 0);
    assert_eq!(cluster_distance(&g, B, 1, 3).unwrap(), 0);
}

#[test]
fn cluster_distance_without_a_fine_table_scales_the_coarse_byte() {
    let mut g = guest();
    g.put_u32(B + 0x9c, 4).put_u32(B + 0xe8, COARSE).put_u8(COARSE + 4, 0x85);
    assert_eq!(f(cluster_distance(&g, B, 3, 1).unwrap()), 2.5);
}

#[test]
fn a_far_cluster_obstructs_by_the_distance_ratio() {
    let mut g = guest();
    positioned(&mut g);
    g.put_u16(listener(1) + 4, 1).put_u16(SOURCE + 0x2c, 3).put_u32(listener(1), 7);
    g.put_u32(B + 0x224, 1).put_u32(B + 0x9c, 4).put_u32(B + 0x228, FINE);
    g.put_u32(FINE + 0x28, TABLE).put_f32(FINE + 8, 10.0).put_u8(TABLE + 4, 3);
    assert!(!obstructed(&g, 1, SOURCE, b(1.0)).unwrap());
    assert!(obstructed(&g, 1, SOURCE, b(2.0)).unwrap());
    // The same object hears it at any ratio; bit 0 too.
    g.put_u32(SOURCE + 0x28, 7);
    assert!(!obstructed(&g, 1, SOURCE, b(2.0)).unwrap());
    assert!(obstructed(&g, NONE, SOURCE, 0).unwrap());
}

#[test]
fn the_ramp_runs_from_outer_to_inner_and_clamps() {
    let g = guest();
    let r = |d: f32| f(ramp(&g, false, b(10.0), b(1.0), b(d)).unwrap());
    assert_eq!(r(5.5), 0.5);
    assert_eq!(r(0.0), 1.0);
    assert_eq!(r(20.0), 0.0);
    // Bounds closer than 1/16: one inside the outer bound, else zero.
    let close = |d: f32, inside: bool| f(ramp(&g, inside, b(3.0), b(3.01), b(d)).unwrap());
    assert_eq!(close(2.0, false), 1.0);
    assert_eq!(close(4.0, false), 0.0);
    assert_eq!(close(2.0, true), 0.0);
}

#[test]
fn distance_gain_ramps_inside_the_maximum_and_falls_off_past_it() {
    let g = guest();
    assert_eq!(f(distance_gain(&g, TAG, SOURCE, b(5.5)).unwrap()), 0.5);
    assert_eq!(f(distance_gain(&g, TAG, SOURCE, b(0.5)).unwrap()), 1.0);
    assert_eq!(distance_gain(&g, TAG, SOURCE, b(20.0)).unwrap(), 0);
}
