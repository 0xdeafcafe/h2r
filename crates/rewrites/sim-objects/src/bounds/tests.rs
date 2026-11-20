//! The bounds kernel on synthetic definitions and matrices.

use super::*;
use guest_layout::Datum;

fn bits(v: [f32; 3]) -> [u32; 3] {
    v.map(f32::to_bits)
}

fn def(flags: u16, dynamic_scale: f32) -> Def {
    Def {
        tag: Datum(0xe100_0001),
        flags,
        bounding_radius: 0.75,
        bounding_offset: [0.5, -0.25, 1.0],
        dynamic_scale,
        dynamic_offset: [0.125, 0.25, -0.5],
    }
}

fn identity(scale: f32, t: [f32; 3]) -> NodeMatrix {
    NodeMatrix {
        scale: scale.to_bits(),
        forward: bits([1.0, 0.0, 0.0]),
        left: bits([0.0, 1.0, 0.0]),
        up: bits([0.0, 0.0, 1.0]),
        translation: bits(t),
    }
}

#[test]
fn a_unit_matrix_moves_the_offset_by_the_translation() {
    let b = spheres(&def(0, 0.0), &identity(1.0, [10.0, 20.0, 30.0]), 2.0f32.to_bits()).unwrap();
    assert_eq!(b.centre, bits([10.5, 19.75, 31.0]));
    assert_eq!(b.radius, 1.5f32.to_bits());
    // No second sphere: a copy of the first.
    assert_eq!((b.second_centre, b.second_radius), (b.centre, b.radius));
    assert_eq!(b.partition(), (b.centre, b.radius));
}

#[test]
fn a_scaled_matrix_scales_the_offset_and_not_the_radius() {
    let b = spheres(&def(0, 0.0), &identity(2.0, [0.0; 3]), 1.0f32.to_bits()).unwrap();
    assert_eq!(b.centre, bits([1.0, -0.5, 2.0]));
    assert_eq!(b.radius, 0.75f32.to_bits());
}

#[test]
fn a_positive_dynamic_scale_makes_the_second_sphere_and_bit_12_picks_it() {
    let b = spheres(&def(0x1000, 4.0), &identity(1.0, [0.0; 3]), 0.5f32.to_bits()).unwrap();
    assert_eq!(b.second_centre, bits([0.625, 0.0, 0.5]));
    assert_eq!(b.second_radius, 2.0f32.to_bits());
    assert_eq!(b.partition(), (b.second_centre, b.second_radius));
    // Zero or negative: the copy.
    let b = spheres(&def(0x1000, -1.0), &identity(1.0, [0.0; 3]), 0.5f32.to_bits()).unwrap();
    assert_eq!(b.second_centre, b.centre);
}

#[test]
fn a_rotation_follows_the_row_order() {
    // Forward along y, left along -x, up along z.
    let m = NodeMatrix {
        scale: 1.0f32.to_bits(),
        forward: bits([0.0, 1.0, 0.0]),
        left: bits([-1.0, 0.0, 0.0]),
        up: bits([0.0, 0.0, 1.0]),
        translation: bits([1.0, 1.0, 1.0]),
    };
    let b = spheres(&def(0, 0.0), &m, 1.0f32.to_bits()).unwrap();
    // x' = -y + 1, y' = x + 1, z' = z + 1.
    assert_eq!(b.centre, bits([1.25, 1.5, 2.0]));
}

#[test]
fn off_the_fast_path_the_kernel_declines() {
    let mut d = def(0, 0.0);
    d.bounding_offset[0] = f32::NAN;
    assert!(spheres(&d, &identity(1.0, [0.0; 3]), 1.0f32.to_bits()).is_none());
    assert!(spheres(&def(0, 0.0), &identity(1.0, [0.0; 3]), 1.0e-30f32.to_bits()).is_none());
}

#[test]
fn relink_on_a_move_past_the_threshold_or_a_new_radius() {
    let at = (bits([1.0, 2.0, 3.0]), 0.75f32.to_bits());
    let threshold = 0.25f32.to_bits();
    assert_eq!(relink(at, at, threshold), Some(false));
    // Squared distance exactly the threshold does not move it (`ja`).
    let edge = (bits([1.5, 2.0, 3.0]), at.1);
    assert_eq!(relink(at, edge, threshold), Some(false));
    let far = (bits([2.0, 2.0, 3.0]), at.1);
    assert_eq!(relink(at, far, threshold), Some(true));
    let grown = (at.0, 1.0f32.to_bits());
    assert_eq!(relink(at, grown, threshold), Some(true));
}
