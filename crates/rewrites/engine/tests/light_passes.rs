//! `engine::light_passes` on invented lights: the per-light flags and the
//! layer-12 and layer-13 implementations. The flag code and both choosers
//! are checked against the original in `light_passes_differential.rs`; the
//! threshold here is invented (written at its address), not the image's.

use engine::light_constants::{Error, FRAME_WORDS, LightGlobals};
use engine::light_passes::{
    self as lp, DEFINITION_BIT_5, PassFlags, constant, diffuse, flag, global, implementation, pass_flags, specular,
};
use guest_layout::memory::GuestMemory;
use guest_layout::testing::FakeGuest;

const DEFINITION: u32 = 0x0300_0000;
const OVERRIDE: u32 = 0x0301_0000;
const MXCSR: u32 = 0x1f80;

fn memory(threshold: f32) -> FakeGuest {
    let mut m = FakeGuest::new();
    m.map(DEFINITION, 0x100)
        .map(OVERRIDE, 0x10)
        .map(global::OVERRIDE & !0xfff, 0x1000)
        .map(global::PASS_FLAGS & !0xfff, 0x1000)
        .map(constant::SPECULAR & !0xfff, 0x1000)
        .map(engine::light_constants::global::COLOUR_B & !0xfff, 0x1000)
        .put_f32(constant::SPECULAR, threshold);
    m
}

fn light(context_word: u16, colour_b: [f32; 3], falloff_4: f32) -> LightGlobals {
    let mut falloff = [0u32; 5];
    falloff[4] = falloff_4.to_bits();
    LightGlobals {
        definition: DEFINITION,
        context_word,
        driver: 0,
        colour_a: [1.0f32.to_bits(); 3],
        colour_b: colour_b.map(f32::to_bits),
        scale: 1.0f32.to_bits(),
        falloff,
        frame: [0; FRAME_WORDS],
        frame_copy_a: [0; 4],
        frame_copy_b: [0; 4],
        mode: 0,
        rectangle: [0; 6],
        rectangle_valid: false,
        sphere: None,
        shadow: 0,
    }
}

#[test]
fn the_shadow_byte_needs_scene_flag_10() {
    assert_eq!(lp::shadow_argument(0x1234_5601, 1 << 10), 0x1234_5601);
    assert_eq!(lp::shadow_argument(0x1234_5601, 0), 0x1234_5600);
    assert_eq!(lp::shadow_argument(0x1234_56ff, !0), 0x1234_5601);
    // A zero byte is left as it came, upper bytes and all.
    assert_eq!(lp::shadow_argument(0x1234_5600, !0), 0x1234_5600);
}

#[test]
fn a_context_zero_light_with_a_bright_colour_b_draws_both_layers() {
    let mut m = memory(0.5);
    m.put_u16(DEFINITION + 4, 1);
    let g = light(0, [0.0, 0.75, 0.0], 3.0);
    // A shadow byte does nothing for a context-zero light.
    let f = pass_flags(&m, &g, 1, MXCSR).unwrap();
    assert_eq!(f.flags, flag::CONTEXT_ZERO | flag::SPECULAR);
    assert!(!f.colour_b_zeroed);
    assert_eq!(f.colour_b, g.colour_b);
    assert!(f.specular_layer() && !f.shadowed());
    assert_eq!(implementation(12, f.flags, 0, 1), Some(1));
    assert_eq!(implementation(13, f.flags, 0, 1), Some(0));
    // No integer input: layer 13 builds no entry.
    assert_eq!(implementation(13, f.flags, 0, 0), None);
}

#[test]
fn colour_b_is_zeroed_without_the_override_or_definition_word_4() {
    let mut m = memory(0.5);
    let g = light(0, [0.9, 0.9, 0.9], 0.0);
    let f = pass_flags(&m, &g, 0, MXCSR).unwrap();
    assert!(f.colour_b_zeroed);
    assert_eq!(f.colour_b, [0; 3]);
    assert_eq!(f.flags, flag::CONTEXT_ZERO);
    assert!(!f.specular_layer());

    // The override with its byte +5 keeps it.
    m.put_u32(global::OVERRIDE, OVERRIDE).put_u8(OVERRIDE + 5, 1);
    let f = pass_flags(&m, &g, 0, MXCSR).unwrap();
    assert!(!f.colour_b_zeroed);
    assert_eq!(f.flags, flag::CONTEXT_ZERO | flag::SPECULAR);
    // The override without it does not.
    m.put_u8(OVERRIDE + 5, 0);
    assert!(pass_flags(&m, &g, 0, MXCSR).unwrap().colour_b_zeroed);
}

#[test]
fn the_threshold_is_strictly_above() {
    let mut m = memory(0.5);
    m.put_u16(DEFINITION + 4, 1);
    let f = pass_flags(&m, &light(0, [0.5, 0.5, 0.5], 0.0), 0, MXCSR).unwrap();
    assert_eq!(f.flags & flag::SPECULAR, 0);
    // A NaN compares unordered: not above, and a masked exception.
    let f = pass_flags(&m, &light(0, [f32::NAN, 0.0, 0.0], 0.0), 0, MXCSR).unwrap();
    assert_eq!(f.flags & flag::SPECULAR, 0);
    // Unmasked invalid: the light stays with the original.
    let r = pass_flags(&m, &light(0, [f32::NAN, 0.0, 0.0], 0.0), 0, MXCSR & !0x80);
    assert_eq!(r, Err(Error::Simd));
}

#[test]
fn a_shadowed_light_sets_the_shadow_bits() {
    let mut m = memory(0.5);
    m.put_u16(DEFINITION + 4, 1);
    let g = light(2, [0.0; 3], 3.0);
    let f = pass_flags(&m, &g, 1, MXCSR).unwrap();
    assert_eq!(f.flags, flag::SHADOW | flag::SHADOW_B | flag::SHADOW_FALLOFF);
    assert!(f.shadowed());
    // Definition word +0x14 of 2, or no falloff, clears bit 3.
    m.put_u16(DEFINITION + 0x14, 2);
    assert_eq!(pass_flags(&m, &g, 1, MXCSR).unwrap().flags, flag::SHADOW | flag::SHADOW_B);
    m.put_u16(DEFINITION + 0x14, 0);
    let g0 = light(2, [0.0; 3], 0.0);
    assert_eq!(pass_flags(&m, &g0, 1, MXCSR).unwrap().flags, flag::SHADOW | flag::SHADOW_B);
    // No shadow byte: none of them.
    assert_eq!(pass_flags(&m, &g, 0, MXCSR).unwrap().flags, 0);
}

#[test]
fn layer_12_picks_by_flags_then_definition_then_specular_then_shadow() {
    let all = flag::CONTEXT_ZERO | flag::SPECULAR | flag::SHADOW;
    assert_eq!(diffuse(all, DEFINITION_BIT_5, true), 1);
    assert_eq!(diffuse(flag::SPECULAR | flag::SHADOW, DEFINITION_BIT_5, true), 4);
    assert_eq!(diffuse(flag::SPECULAR | flag::SHADOW, 0, true), 0);
    assert_eq!(diffuse(flag::SPECULAR | flag::SHADOW, 0, false), 2);
    assert_eq!(diffuse(flag::SPECULAR, 0, false), 3);
    assert_eq!(diffuse(0, !DEFINITION_BIT_5, true), 3);
}

#[test]
fn layer_13_needs_context_zero_specular_and_the_input() {
    let both = flag::CONTEXT_ZERO | flag::SPECULAR;
    assert_eq!(specular(both, true), Some(0));
    assert_eq!(specular(both | flag::SHADOW, true), Some(0));
    assert_eq!(specular(flag::SPECULAR, true), None);
    assert_eq!(specular(flag::CONTEXT_ZERO, true), None);
    assert_eq!(specular(both, false), None);
    assert_eq!(implementation(11, both, 0, 1), None);
}

#[test]
fn the_gel_is_the_definitions_bitmap_only_for_a_plain_frustum_light() {
    let g = 0xe123_0456;
    assert_eq!(lp::gel(0, 0, u32::MAX, g), Some(g));
    assert_eq!(lp::gel(flag::SPECULAR | flag::SHADOW, 0x20, u32::MAX, g), Some(g), "other bits leave it");
    assert_eq!(lp::gel(flag::CONTEXT_ZERO, 0, u32::MAX, g), None, "context-zero case");
    assert_eq!(lp::gel(0, lp::DEFINITION_TEXTURE_18, u32::MAX, g), None, "texture 18");
    assert_eq!(lp::gel(0, 0, 0xe000_0001, g), None, "the gel shader's target");
    assert_eq!(lp::gel(0, 0, u32::MAX, u32::MAX), None, "no bitmap");
}

#[test]
fn write_stores_the_flags_and_a_zeroed_colour_b() {
    let mut m = memory(0.5);
    let cb = engine::light_constants::global::COLOUR_B;
    m.put_f32(cb, 2.0);
    let f = PassFlags {
        flags: 0x25,
        colour_b: [0; 3],
        colour_b_zeroed: true,
    };
    f.write(&mut m).unwrap();
    assert_eq!(m.u32(global::PASS_FLAGS).unwrap(), 0x25);
    assert_eq!(m.u8(global::PASSES).unwrap(), 1);
    assert_eq!(m.u32(cb).unwrap(), 0);
    m.put_f32(cb, 2.0);
    PassFlags {
        colour_b_zeroed: false,
        ..f
    }
    .write(&mut m)
    .unwrap();
    assert_eq!(m.f32(cb).unwrap(), 2.0);
}

#[test]
fn the_uploaded_frame_is_the_block_then_the_words_after_it() {
    use engine::light_constants::{FRAME_COLOUR_A, FRAME_COLOUR_B, FRAME_UPLOAD_DWORDS};
    let frame_at = engine::light_constants::global::FRAME;
    let mut m = memory(0.5);
    for i in 0..FRAME_UPLOAD_DWORDS as u32 {
        m.put_u32(frame_at + 4 * i, 0xdead_0000 + i);
    }
    let mut g = light(0, [0.0; 3], 0.0);
    for (i, w) in g.frame.iter_mut().enumerate() {
        *w = i as u32;
    }
    let c = g.frame_constants(&m).unwrap();
    assert_eq!(c[..FRAME_WORDS], g.frame[..]);
    assert_eq!(c[FRAME_WORDS..], [31, 32, 33, 34, 35].map(|i| 0xdead_0000 + i)[..]);
    assert_eq!(g.colour_a_input(), [24, 25, 26]);
    assert_eq!(g.colour_b_input(), [28, 29, 30]);
    assert_eq!((FRAME_COLOUR_A, FRAME_COLOUR_B), (24, 28));
}
