//! The first-person listener shares its observer's forward and up axes.
//! A seat on a moving parent rotates both by the interpolated camera
//! quaternion. Each binary32 operation rounds once in the guest SSE mode.

use std::cell::Cell;

pub type Axis = [u32; 3];

#[derive(Debug)]
pub struct SeatRotation {
    pub forward: Axis,
    pub up: Axis,
    /// Values also needed on the far side of the guest's velocity and
    /// location calls. These include the spills used by the two axes.
    pub vector_scale: u32,
    pub cross_scale: u32,
    pub up_dot: u32,
    pub up_x_times_qy: u32,
    pub up_z_scaled: u32,
    pub mxcsr: u32,
}

/// Rotate both listener axes in the vehicle seat. The order of the three
/// dot products and each operand is significant for rounding and NaNs.
pub fn rotate_seat(
    forward: Axis,
    up: Axis,
    q: [u32; 4],
    one: u32,
    two: u32,
    initial_mxcsr: u32,
) -> SeatRotation {
    let mxcsr = Cell::new(initial_mxcsr);
    let op = |f: fn(&mut u32, u128, u32) -> std::result::Result<u128, simd::Xm>, a, b| {
        let mut csr = mxcsr.get();
        let v = f(&mut csr, u128::from(a), b).expect("the native listener masks SSE exceptions");
        mxcsr.set(csr);
        v as u32
    };
    let add = |a: u32, b| op(simd::addss, a, b);
    let sub = |a: u32, b| op(simd::subss, a, b);
    let mul = |a: u32, b| op(simd::mulss, a, b);
    let [qx, qy, qz, qw] = q;
    let k = sub(mul(mul(qw, qw), two), one);
    let w = mul(qw, two);

    let [x, y, z] = forward;
    let dot = mul(add(add(mul(y, qy), mul(z, qz)), mul(qx, x)), two);
    let forward = [
        add(add(mul(sub(mul(z, qy), mul(y, qz)), w), mul(qx, dot)), mul(k, x)),
        add(add(mul(sub(mul(qz, x), mul(z, qx)), w), mul(qy, dot)), mul(y, k)),
        add(add(mul(sub(mul(y, qx), mul(qy, x)), w), mul(qz, dot)), mul(z, k)),
    ];

    let [x, y, z] = up;
    let dot = mul(add(add(mul(y, qy), mul(z, qz)), mul(x, qx)), two);
    let x_qy = mul(x, qy);
    let z_k = mul(z, k);
    let up = [
        add(add(mul(sub(mul(z, qy), mul(y, qz)), w), mul(dot, qx)), mul(x, k)),
        add(add(mul(sub(mul(x, qz), mul(z, qx)), w), mul(qy, dot)), mul(y, k)),
        add(add(mul(sub(mul(y, qx), x_qy), w), mul(qz, dot)), z_k),
    ];

    SeatRotation {
        forward, up, vector_scale: k, cross_scale: w, up_dot: dot,
        up_x_times_qy: x_qy, up_z_scaled: z_k, mxcsr: mxcsr.get(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotating_a_seat_half_a_turn_preserves_its_vertical_axis() {
        let bits = |v: [f32; 3]| v.map(f32::to_bits);
        let r = rotate_seat(
            bits([1., -0.5, 0.25]),
            bits([0., 0., 1.]),
            [0., 0., 1., 0.].map(f32::to_bits),
            1f32.to_bits(), 2f32.to_bits(), 0x1f80,
        );
        assert_eq!(r.forward, bits([-1., 0.5, 0.25]));
        assert_eq!(r.up.map(f32::from_bits), [0., 0., 1.]);
        assert_eq!(r.mxcsr, 0x1f80);
    }
}
