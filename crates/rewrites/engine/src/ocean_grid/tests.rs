use super::*;
use crate::memory::FlatMemory;

const DEF: u32 = 0x0051_0000;
const UP: u32 = 0x0051_1000;
const STATE: u32 = 0x0051_2000;
const MXCSR: u32 = 0x1f80;
const CW: u16 = 0x027f;

/// A camera 10 above still water at 0, looking along x with z up, a
/// 640x480 raster, a 1.2 field of view and a grid of angle 1, width 1,
/// far 2.
fn world() -> FlatMemory {
    let mut m = FlatMemory::new(0x0040_0000, 0x0012_0000);
    for (at, v) in [
        (constant::ONE, 1.0),
        (constant::HALF, 0.5),
        (constant::LEAST, 1.0e-4),
        (constant::CAP, 0.780_685_8),
        (constant::MARGIN, 1.13),
        (constant::ANAMORPHIC, 1.5),
        (constant::NEAR_A, 0.5),
        (constant::NEAR_B, 1.0),
        (constant::UP_WEIGHT, 0.0),
        (CAMERA, 0.0),
        (CAMERA + 4, 0.0),
        (CAMERA + 8, 10.0),
        (FOV, 1.2),
        (BASIS_FORWARD, 1.0),
        (UP + 8, 1.0),
        (DEF + ANGLE, 1.0),
        (DEF + WIDTH, 1.0),
        (DEF + LEVEL, 0.0),
        (DEF + FAR, 2.0),
        (DEF + DEPTH, 0.0),
    ] {
        m.set_f32(at, v).unwrap();
    }
    m.set_u32(BASIS_UP, UP).unwrap();
    for (o, v) in [0u16, 0, 480, 640].iter().enumerate() {
        m.set_u16(RASTER + 2 * o as u32, *v).unwrap();
    }
    m
}

fn run(m: &mut FlatMemory, lens: f32) -> Grid {
    let mut mx = MXCSR;
    grid(m, DEF, &mut mx, CW, lens).unwrap()
}

fn block(m: &FlatMemory, at: u32, n: u32) -> Vec<f32> {
    (0..n).map(|i| m.f32(at + 4 * i).unwrap()).collect()
}

fn close(got: &[f32], want: &[f32]) {
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert!((g - w).abs() <= 1e-4 * w.abs().max(1.0), "word {i}: {got:?} against {want:?}");
    }
}

/// The span over the height: the raster's tangent over the grid's half
/// width, with the margin.
fn span(aspect: f32) -> f32 {
    (0.6f32.tan() * aspect / 0.5f32.tan()) * 1.13
}

#[test]
fn a_zero_angle_writes_nothing() {
    let mut m = world();
    m.set_f32(DEF + ANGLE, 0.0).unwrap();
    let before = block(&m, QUAD, 0x24);
    assert_eq!(run(&mut m, 1.0), Grid::Skipped);
    assert_eq!(block(&m, QUAD, 0x24), before);
}

#[test]
fn the_camera_basis_rows_and_quad() {
    let mut m = world();
    assert_eq!(run(&mut m, 1.0), Grid::Built { eax: 0, edx: 0, widened: false });
    close(&block(&m, MATRIX, 13), &[10.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
    let s = span(4.0 / 3.0) * 10.0;
    close(&block(&m, ROWS, 12), &[10.0, 0.0, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    let t = 0.5f32.tan();
    let (b, e) = ((1.0 + t * t).sqrt() * t, 5.0f32.sqrt() * t);
    #[rustfmt::skip]
    close(&block(&m, QUAD, 12), &[
        10.0 * t, -s * b, 0.0,
        10.0 * t, s * b, 0.0,
        20.0, s * e, 0.0,
        20.0, -s * e, 0.0,
    ]);
}

#[test]
fn a_camera_under_the_water_takes_its_depth_and_the_least_height_at_the_surface() {
    let mut m = world();
    m.set_f32(CAMERA + 8, -4.0).unwrap();
    m.set_f32(DEF + DEPTH, 0.5).unwrap();
    run(&mut m, 1.0);
    assert_eq!(m.f32(MATRIX).unwrap(), 4.0);
    assert_eq!(m.f32(MATRIX + 0x30).unwrap(), -2.0);
    m.set_f32(CAMERA + 8, 0.0).unwrap();
    run(&mut m, 1.0);
    assert_eq!(m.f32(MATRIX).unwrap(), 1.0e-4);
}

#[test]
fn an_anamorphic_lens_widens_the_span_unless_the_record_keeps_retail() {
    let mut m = world();
    m.set_u8(WIDESCREEN, 1).unwrap();
    assert_eq!(run(&mut m, 1.0), Grid::Built { eax: 1, edx: 0, widened: false });
    close(&[m.f32(ROWS + 0x14).unwrap()], &[span(2.0) * 10.0]);
    m.set_u32(LENS_STATE, STATE).unwrap();
    m.set_u8(STATE + 1, 1).unwrap();
    m.set_u32(STATE + 8, 3).unwrap();
    assert_eq!(run(&mut m, 1.0), Grid::Built { eax: STATE, edx: 0, widened: false });
    close(&[m.f32(ROWS + 0x14).unwrap()], &[span(4.0 / 3.0) * 10.0]);
}

#[test]
fn a_narrower_displayed_lens_widens_the_aspect() {
    let mut m = world();
    assert_eq!(run(&mut m, 0.75), Grid::Built { eax: 0, edx: 0, widened: true });
    close(&[m.f32(ROWS + 0x14).unwrap()], &[span(4.0 / 3.0 / 0.75) * 10.0]);
    for lens in [1.0, 1.5, 0.0, f32::NAN] {
        assert_eq!(widen_aspect(&m, 4.0 / 3.0, lens), None, "{lens}");
    }
}

#[test]
fn a_wide_field_of_view_is_capped_and_the_widening_carries_the_cap() {
    let mut m = world();
    m.set_f32(FOV, 2.0).unwrap();
    let cap = 0.780_685_8f32;
    let want = 4.0 / 3.0 / 0.5 * 1.0f32.tan() / cap.tan();
    close(&[widen_aspect(&m, 4.0 / 3.0, 0.5).unwrap()], &[want]);
}

#[test]
fn a_forward_along_up_leaves_a_zero_basis() {
    let mut m = world();
    m.set_f32(BASIS_FORWARD, 0.0).unwrap();
    m.set_f32(BASIS_FORWARD + 8, 1.0).unwrap();
    run(&mut m, 1.0);
    assert_eq!(block(&m, MATRIX + 4, 6), [0.0; 6]);
}

#[test]
fn an_unmasked_exception_is_an_error() {
    let mut m = world();
    m.set_u16(RASTER + 4, 0).unwrap();
    let mut mx = MXCSR & !0x200;
    assert!(matches!(grid(&mut m, DEF, &mut mx, CW, 1.0), Err(Error::Simd)));
    let mut mx = MXCSR;
    grid(&mut m, DEF, &mut mx, CW, 1.0).unwrap();
    assert_ne!(mx & 0x4, 0, "the division by zero's sticky flag");
}
