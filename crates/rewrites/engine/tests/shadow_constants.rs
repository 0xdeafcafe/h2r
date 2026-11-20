//! `engine::shadow_constants`'s two small ports on invented values: the
//! float-to-integer rounding of `0x00048e70` and the packed size word of
//! `0x0001ddd0`. The whole block is checked against the original in
//! `shadow_constants_differential.rs`.

use engine::shadow_constants::{ceiling, target_size_word};

#[test]
fn positive_fractions_round_up_and_the_rest_truncate() {
    let c = |x: f32| ceiling(x.to_bits());
    assert_eq!(c(0.0), 0);
    assert_eq!(c(-0.0), 0);
    assert_eq!(c(0.25), 1);
    assert_eq!(c(1.0), 1);
    assert_eq!(c(1.5), 2);
    assert_eq!(c(2.0), 2);
    assert_eq!(c(100.01), 101);
    assert_eq!(c(-0.5), 0);
    assert_eq!(c(-1.5), -1);
    assert_eq!(c(-37.9), -37);
}

#[test]
fn the_size_word_packs_width_and_height() {
    // 512 x 512: the word the pass's tail stores for its own target.
    assert_eq!(target_size_word(512, 512, true), 0x1f1f_f1ff);
    assert_eq!(target_size_word(512, 256, true), 0x1f0f_f1ff);
    assert_eq!(target_size_word(128, 64, true), 0x0703_f07f);
    assert_eq!(target_size_word(512, 512, false), 0);
}
