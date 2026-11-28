use super::*;
use guest_layout::entry::{self, EntryFuture, Ret};
use guest_layout::testing::FakeGuest;

const K: Constants = Constants { one: 1.0, two: 2.0, angle: 0.5, smallest: 0.001 };

fn near(a: [f32; 3], b: [f32; 3]) {
    for i in 0..3 {
        assert!((a[i] - b[i]).abs() < 1e-6, "{a:?} != {b:?}");
    }
}

#[test]
fn a_quarter_turn_about_z_takes_x_to_y_through_the_guests_arithmetic() {
    // 0x11d6a0: cos 0, so t = (0 + 1) * 2 and the sine is sqrt(2); the
    // axis is the cross product over it, the fourth word sine * 0.5.
    let q = rotation_between(K, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]).unwrap();
    let r = 2.0f32.sqrt();
    assert_eq!(q, [0.0, 0.0, 1.0 / r, r * 0.5]);
    let q = normalize(q, K.one);
    let basis = rotate(K, q, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    near(basis.forward, [0.0, 1.0, 0.0]);
    near(basis.up, [0.0, 0.0, 1.0]);
}

#[test]
fn the_identity_quaternion_leaves_both_axes() {
    let basis = rotate(K, [0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
    assert_eq!(basis, Basis { forward: [0.0, 1.0, 0.0], up: [0.0, 0.0, 1.0] });
}

#[test]
fn antiparallel_vectors_are_the_degenerate_branch() {
    assert_eq!(rotation_between(K, [1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]), None);
}

#[test]
fn the_blend_takes_the_shorter_way_round() {
    let to = [0.0, 0.0, 0.0, 1.0];
    assert_eq!(nlerp([0.0, 0.0, 0.0, 1.0], to, 0.25, 1.0), [0.0, 0.0, 0.0, 1.0]);
    // A negative dot flips the blend's sign: -1 * -0.25 + 0.75.
    assert_eq!(nlerp([0.0, 0.0, 0.0, -1.0], to, 0.25, 1.0), [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn the_normalisation_scales_to_unit_length_and_leaves_zero() {
    assert_eq!(normalize([0.0, 0.0, 0.0, 2.0], 1.0), [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(normalize([0.0; 4], 1.0), [0.0; 4]);
}

#[test]
fn the_row_holds_the_cross_axis_and_the_ears_projections() {
    let basis = Basis { forward: [1.0, 0.0, 0.0], up: [0.0, 0.0, 1.0] };
    let r = row(basis, [4.0, 5.0, 6.0], [1.0, 2.0, 3.0]);
    // up x forward: (b1 a2 - b2 a1, b2 a0 - a2 b0, a1 b0 - b1 a0).
    assert_eq!(r.cross, [0.0, 1.0, 0.0]);
    assert_eq!(r.ear, [1.0, 2.0, 3.0]);
    assert_eq!(r.position, [4.0, 5.0, 6.0]);
}

const STACK: u32 = 0x0030_0000;
const MANAGER_AT: u32 = 0x0060_0000;
const GLOBALS_AT: u32 = 0x0060_2000;
const USERS_AT: u32 = 0x0060_3000;

fn guest() -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(0x004e_0000, 0x1_0000).map(0x0045_0000, 0x1_0000).map(MANAGER_AT, 0x8000).map(STACK - 0x1000, 0x2000);
    g.put_u32(ONE, 1.0f32.to_bits()).put_u32(TWO, 2.0f32.to_bits()).put_u32(ANGLE, 0.5f32.to_bits()).put_u32(SMALLEST, 0.001f32.to_bits());
    g.put_u32(MANAGER, MANAGER_AT).put_u8(MANAGER_AT + MANAGER_ON, 1);
    g.put_u32(GLOBALS, GLOBALS_AT).put_u32(USERS, USERS_AT);
    g
}

fn run(g: FakeGuest, environment: bool) -> entry::testing::Run {
    let f = move |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            update(&cx, environment).await?;
            Ok(Ret::new(0, 4))
        })
    };
    let regs = guest_layout::entry::Regs { esp: STACK, ..Default::default() };
    entry::testing::run(&f, g, regs, |c, _| panic!("no guest call: {:#x}", c.target))
}

#[test]
fn unmapped_users_hold_minus_one_and_the_row_is_otherwise_left() {
    let mut g = guest();
    for user in 0..4 {
        g.put_u32(USERS_AT + 0xc + 4 * user, u32::MAX);
        g.put_u8(GLOBALS_AT + 0x88 + 0x48 * user + 6, 1);
    }
    let r = run(g, false);
    assert!(r.result.is_ok());
    for user in 0..4 {
        let at = GLOBALS_AT + 0x88 + 0x48 * user;
        assert_eq!(r.guest.u32(at).unwrap(), u32::MAX);
        assert_eq!(r.guest.u16(at + 4).unwrap(), 0xffff);
        assert_eq!(r.guest.u8(at + 6).unwrap(), 1, "the active byte is not cleared");
    }
    assert!(r.calls.is_empty());
}

#[test]
fn a_manager_that_is_off_runs_nothing() {
    let mut g = guest();
    g.put_u8(MANAGER_AT + MANAGER_ON, 0);
    let r = run(g, true);
    assert!(r.result.is_ok() && r.calls.is_empty());
}
