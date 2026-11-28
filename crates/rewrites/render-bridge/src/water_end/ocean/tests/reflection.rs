//! `0x0003bd00`, the ocean's reflection ([`super::super::reflection`]).

use super::*;
use crate::view_setup;
use crate::water_end::ocean::reflection::{self, mirrored};

/// Target 0x15's cache record, live: its surface header.
fn live_reflection(m: &mut FlatMemory) -> u32 {
    let r = view_setup::TARGET_CACHE + reflection::REFLECTION * view_setup::TARGET_RECORD;
    m.set_u32(r + 0x88, 0x0012_0000).unwrap();
    m.set_u32(r + 0x78, 1).unwrap();
    r + 0x18
}

const VIEW_COLOUR: u32 = 0x0050_c100;
const VIEW_DEPTH: u32 = 0x0050_c200;

/// [`mem`] with the reflection bit, the view on target 0 with a 640 x 480
/// window, its camera a unit basis at (1, 2, 3), and the definition's
/// height 5 and colour red.
fn reflecting() -> (FlatMemory, u32) {
    let mut m = mem(1 << PASS_BIT);
    let surface = live_reflection(&mut m);
    let set = |m: &mut FlatMemory, at: u32, v: u32| m.set_u32(at, v).unwrap();
    set(&mut m, view_setup::VIEW_COLOUR, VIEW_COLOUR);
    set(&mut m, view_setup::VIEW_DEPTH, VIEW_DEPTH);
    set(&mut m, 0x0045_dc0c, 2f32.to_bits());
    let ctx = view_setup::CONTEXT;
    m.write(ctx + 0x4c, &480u16.to_le_bytes()).unwrap();
    m.write(ctx + 0x4e, &640u16.to_le_bytes()).unwrap();
    let camera = ctx + reflection::CAMERA_OFFSET;
    for (k, v) in [1.0f32, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 3.0].into_iter().enumerate() {
        set(&mut m, camera + 4 * k as u32, v.to_bits());
    }
    set(&mut m, DEF + reflection::HEIGHT, 5f32.to_bits());
    set(&mut m, DEF + reflection::COLOUR, 1f32.to_bits());
    (m, surface)
}

/// The basis's third axis is negated and its point's height mirrored
/// about the water's; the inverse after it is the mirrored basis's.
#[test]
fn the_reflection_camera_mirrors_about_the_height() {
    let mut camera = [0u32; reflection::CAMERA_WORDS];
    for (k, v) in [1.0f32, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 3.0].into_iter().enumerate() {
        camera[k] = v.to_bits();
    }
    camera[40] = 0xdead_beef;
    let out = mirrored(&camera, 5.0, 2.0, 1.0);
    let f = |w: u32| f32::from_bits(w);
    assert_eq!([out[7], out[8], out[9]], [0.0f32.to_bits(), 0.0f32.to_bits(), (-1.0f32).to_bits()], "0 - x");
    assert_eq!(f(out[12]), 7.0, "5 x 2 - 3");
    assert_eq!(out[..7], camera[..7]);
    assert_eq!(out[10..12], camera[10..12]);
    // The inverse: the axes transposed, the point through them.
    let inverse = engine::light_frusta::inverse_words(&std::array::from_fn(|i| out[i]), 1.0);
    assert_eq!(out[13..26], inverse);
    assert_eq!(f(out[13]), 1.0);
    assert_eq!([f(out[23]), f(out[24]), f(out[25])], [-1.0, -2.0, 7.0]);
    assert_eq!(out[40], 0xdead_beef, "past the inverse, the camera's");
}

/// The pass binds target 0x15 with no depth on its whole surface, clears
/// it to the definition's colour, draws the sky under the mirrored view's
/// constants, then binds the view's target again and makes its constants
/// again; the ocean draws after it.
#[test]
fn the_reflection_draws_the_sky_into_0x15_then_binds_the_view_again() {
    use Event::*;
    let (mut m, surface) = reflecting();
    let (events, _) = run(&mut m, false);
    let one = 1f32.to_bits();
    let sky = events.iter().position(|e| matches!(e, Sky { .. })).expect("a sky");
    assert_eq!(events[..4], [
        Visible,
        Bind { colour: surface, depth: 0 },
        Viewport { words: vec![0, 0, 512, 256, 0, one] },
        Clear { flags: reflection::CLEAR_FLAGS, colour: 0x00ff_0000 },
    ]);
    let Constants { register: 0, words: mirrored_words } = &events[4] else { panic!("{:?}", events[4]) };
    assert_eq!(sky, 5);
    assert_eq!(events[5], Sky { constants: mirrored_words.clone() });
    assert_eq!(f32::from_bits(mirrored_words[32]), 256.0, "the 512-wide window's half");
    assert_eq!(events[6], Bind { colour: VIEW_COLOUR, depth: VIEW_DEPTH });
    assert_eq!(events[7], Viewport { words: vec![0, 0, 640, 480, 0, one] });
    let Constants { register: 0, words: view_words } = &events[8] else { panic!("{:?}", events[8]) };
    assert!(matches!(events[9], Draw(_)));
    // The view's own constants, as its setup makes them, and its target.
    let expected = view_setup::transform(&m, view_setup::CONTEXT).unwrap();
    assert_eq!(view_words[..], expected[..]);
    assert_ne!(view_words, mirrored_words);
    assert_eq!(m.u32(view_setup::TARGET).unwrap(), 0);
    let shadow = crate::water_end::ocean::fake::read(&m, view_setup::TRANSFORM_SHADOW, 48);
    assert_eq!(shadow[..], expected[..]);
}

/// A reflection with target 0x15 not live, or from a view target other
/// than 0 and 0x12, is named.
#[test]
fn the_reflection_names_what_it_cannot_make() {
    let mut m = mem(1 << PASS_BIT);
    assert_eq!(cause(&mut m, &mut Recorder::new()), "copy_reflection");
    let (mut m, _) = reflecting();
    m.set_u32(view_setup::TARGET, 7).unwrap();
    assert_eq!(cause(&mut m, &mut Recorder::new()), "copy_view_target");
}
