//! `engine::light_constants` on invented lights: the colour scale modes,
//! the falloff words with and without a record, the frame of a plain
//! light, and the screen rectangle (projected, and the whole viewport when
//! the bounds call says no). The image's constants are invented too, at
//! their addresses, with the values the code's names say. The word-for-word
//! check against the original is `light_constants_differential.rs`.

use engine::FlatMemory;
use engine::light_constants::{BoundsQuery, Call, Fpu, constant, global, light_globals};
use engine::memory::GuestMemory;

const ARRAY: u32 = 0x0058_0000;
const RECORDS: u32 = 0x0058_1000;
const VIEW: u32 = 0x0059_0000;
const CONTEXT: u32 = 0x0059_1000;
const DEFINITION: u32 = 0x0059_2000;
const TAIL: u32 = 0x0059_3000;
const LOOSE_FRAME: u32 = 0x0059_4000;

const FPU: Fpu = Fpu {
    mxcsr: 0x1f80,
    x87_control: 0x027f,
};

fn f(m: &mut FlatMemory, at: u32, v: &[f32]) {
    for (i, x) in v.iter().enumerate() {
        m.set_u32(at + 4 * i as u32, x.to_bits()).unwrap();
    }
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

/// A plain light (no special flags) in a type-0 context, depth range 2.
fn world() -> FlatMemory {
    let mut m = FlatMemory::new(0, 0x0060_0000);
    f(&mut m, constant::ONE, &[1.0]);
    f(&mut m, constant::MINUS_ONE, &[-1.0]);
    f(&mut m, constant::HALF, &[0.5]);
    f(&mut m, constant::TWO, &[2.0]);
    f(&mut m, constant::QUARTER, &[0.25]);
    m.set_u32(global::FRAME_TAIL, TAIL).unwrap();
    f(&mut m, TAIL, &[7.0, 8.0, 9.0]);
    m.set_u32(global::LIGHT_ARRAY, ARRAY).unwrap();
    m.set_u32(ARRAY + 0x44, RECORDS).unwrap();
    m.set_u32(global::VIEW, VIEW).unwrap();
    // The view's projection bounds and the two depth words.
    f(&mut m, VIEW + 0xa4, &[-1.0, 1.0, -1.0, 1.0]);
    f(&mut m, VIEW + 0x74, &[3.0]);
    f(&mut m, VIEW + 0x8c, &[5.0]);
    // Viewport: y0 0, x0 0, y1 480, x1 640.
    m.write(global::VIEWPORT, &[0, 0, 0, 0, 0xe0, 0x01, 0x80, 0x02])
        .unwrap();
    // Context type 0: origin, a vector, the projection's depth range 2.
    m.set_u32(CONTEXT, 0).unwrap();
    f(&mut m, CONTEXT + 0x1c, &[2.0, 4.0, 6.0]);
    f(&mut m, CONTEXT + 0x2c, &[10.0, 20.0, 30.0]);
    f(&mut m, CONTEXT + 0x38 + 0xc, &[2.0]);
    m.set_u32(DEFINITION, 0).unwrap();
    m.write(DEFINITION + 0x8e, &[0, 0]).unwrap();
    m
}

/// Lays a light frame block at `p`: forward, up, colours, scale, offsets
/// and falloff words.
fn frame_block(m: &mut FlatMemory, p: u32, a: [f32; 3], b: [f32; 3], k: f32) {
    f(m, p + 0x28, &[1.0, 0.0, 0.0]);
    f(m, p + 0x34, &[0.0, 1.0, 0.0]);
    f(m, p + 0x50, &a);
    f(m, p + 0x5c, &b);
    f(m, p + 0x68, &[k]);
    f(m, p + 0x6c, &[2.0, 4.0]);
    f(m, p + 0x74, &[11.0, 12.0, 0.5, 3.0, 13.0]);
}

fn call(frame: u32, light: u32) -> Call {
    Call {
        frame,
        context: CONTEXT,
        light,
        definition: DEFINITION,
        shadow: 0x1234_5601,
        weight: 2.0f32.to_bits(),
    }
}

#[test]
fn a_light_without_a_record_scales_bright_colours_by_a_quarter() {
    let mut m = world();
    frame_block(&mut m, LOOSE_FRAME, [0.5, 0.25, 1.0], [0.2, 0.2, 0.2], 3.0);
    let g = light_globals(&m, &call(LOOSE_FRAME, u32::MAX), FPU, &mut |_| {
        panic!("no record, no bounds call")
    })
    .unwrap();

    assert_eq!(g.driver, u32::MAX);
    assert_eq!(g.falloff.to_vec(), bits(&[1.0, 1.0, 1.0, 1.0, 0.0]));
    assert_eq!(g.colour_a.to_vec(), bits(&[0.5, 0.25, 1.0]));
    assert_eq!(g.scale, 3.0f32.to_bits());
    // 3.0 is the brightest word: over 2, so a quarter, clamped to [0, 1].
    assert_eq!(g.mode, 2);
    let q = |c: f32| (c * 3.0 * 0.25).clamp(0.0, 1.0);
    assert_eq!(g.frame[24..27].to_vec(), bits(&[q(0.5), q(0.25), q(1.0)]));
    assert_eq!(g.frame[28..31].to_vec(), bits(&[q(0.2), q(0.2), q(0.2)]));

    // The frame: origin twice with a 1 between, the context's vector and
    // the light's up times -1/2, forward times -1/2 and the tail vector;
    // the record offsets (2 and 4) added along forward.
    let s = -0.5f32;
    assert_eq!(g.frame[..7].to_vec(), bits(&[10.0, 20.0, 30.0, 1.0, 10.0, 20.0, 30.0]));
    assert_eq!(g.frame_copy_a.to_vec(), bits(&[2.0 * s, 4.0 * s, 6.0 * s, 0.0]));
    assert_eq!(g.frame_copy_b.to_vec(), bits(&[0.0 * s, 1.0 * s, 0.0 * s, 0.0]));
    assert_eq!(g.frame[16..24].to_vec(), bits(&[s, 0.0 * s, 0.0 * s, s * 0.5, 7.0, 8.0, 9.0, 0.0]));
    assert_eq!(g.frame[8..11].to_vec(), bits(&[s * 2.0 + 2.0 * s, 0.0 * s * 2.0 + 4.0 * s, 0.0 * s * 2.0 + 6.0 * s]));
    assert_eq!(g.frame[12..15].to_vec(), bits(&[s * 4.0 + 0.0 * s, 0.0 * s * 4.0 + s, 0.0 * s * 4.0 + 0.0 * s]));

    // No record: the whole viewport, and the depth words negated.
    assert_eq!(g.rectangle.to_vec(), bits(&[0.0, 640.0, 0.0, 480.0, -5.0, -3.0]));
    assert!(!g.rectangle_valid);
    assert_eq!(g.sphere, None);
    assert_eq!(g.shadow, 0x01);
}

#[test]
fn a_record_weights_its_colours_and_projects_its_bounds() {
    let mut m = world();
    let rec = RECORDS + 2 * 0x110;
    frame_block(&mut m, rec + 0x84, [0.5, 0.25, 0.75], [0.1, 0.2, 0.3], 1.0);
    m.set_u32(rec + 0x4c, 0xe123_0004).unwrap();
    m.write(rec + 0x54, &[7, 0]).unwrap();
    f(&mut m, rec + 0x18, &[1.0, 2.0, 3.0, 4.0]);
    let mut asked = None;
    let g = light_globals(&m, &call(rec + 0x84, 0xabcd_0002), FPU, &mut |q: BoundsQuery| {
        asked = Some(q);
        Some(bits(&[-0.5, 0.5, -0.5, 0.5, 6.0, 9.0]).try_into().unwrap())
    })
    .unwrap();
    assert_eq!(
        asked,
        Some(BoundsQuery {
            centre: rec + 0x28,
            view: VIEW + 4,
            word: 0,
        })
    );
    assert_eq!(g.driver, 0xe123_0004);
    // Colour A times weight x 0.5 (= 1), colour B times min(3, 0.5).
    assert_eq!(g.falloff.to_vec(), bits(&[11.0, 12.0, 1.0, 0.5, 13.0]));
    assert_eq!(g.colour_a.to_vec(), bits(&[0.5, 0.25, 0.75]));
    assert_eq!(g.colour_b.to_vec(), bits(&[0.05, 0.1, 0.15]));
    assert_eq!(g.mode, 0);
    // [-0.5, 0.5] of [-1, 1] across 640 and 480 (y flipped).
    assert_eq!(g.rectangle.to_vec(), bits(&[160.0, 480.0, 120.0, 360.0, 6.0, 9.0]));
    assert!(g.rectangle_valid);
    assert_eq!(g.sphere, Some([1.0f32.to_bits(), 2.0f32.to_bits(), 3.0f32.to_bits(), 4.0f32.to_bits()]));

    // Off screen: the whole viewport, still marked valid, sphere copied.
    let g = light_globals(&m, &call(rec + 0x84, 0xabcd_0002), FPU, &mut |_| None).unwrap();
    assert_eq!(g.rectangle.to_vec(), bits(&[0.0, 640.0, 0.0, 480.0, -5.0, -3.0]));
    assert!(g.rectangle_valid);
    assert!(g.sphere.is_some());
}

#[test]
fn colours_between_one_and_two_are_halved() {
    let mut m = world();
    frame_block(&mut m, LOOSE_FRAME, [0.5, 0.25, 1.0], [0.9, 0.2, 0.2], 1.5);
    let g = light_globals(&m, &call(LOOSE_FRAME, u32::MAX), FPU, &mut |_| None).unwrap();
    assert_eq!(g.mode, 1);
    let h = |c: f32| c * 1.5 * 0.5;
    assert_eq!(g.frame[24..27].to_vec(), bits(&[h(0.5), h(0.25), h(1.0)]));
    assert_eq!(g.frame[28..31].to_vec(), bits(&[h(0.9), h(0.2), h(0.2)]));

    // Writing the block stores what the original stores.
    g.write(&mut m).unwrap();
    assert_eq!(m.u8(global::ACTIVE).unwrap(), 1);
    assert_eq!(m.u32(global::MODE).unwrap(), 1);
    assert_eq!(m.u32(global::FRAME + 4 * 19).unwrap(), (-0.25f32).to_bits());
    assert_eq!(m.u32(global::ZERO).unwrap(), 0);
    assert_eq!(m.u8(global::RECTANGLE_VALID).unwrap(), 0);
}
