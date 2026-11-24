//! The unit look clamp's frame. The guest calls the normalizer on the up
//! direction after rebasing it around the current aim.

use sim_core::det::Fp;

/// The three differences of the second cross product and their right
/// operands. The normalizer sees all six values in its caller's registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LookUp {
    pub up: [u32; 3],
    pub subtrahends: [u32; 3],
}

/// Complete `left = frame_up × aim`, then calculate `aim × left`.
/// The unit body computes left.x before its final frame load. All values
/// are binary32 bits. Each product and subtraction rounds separately.
/// A caller with the guest and host fast float environments can use the
/// result without changing any guest exception flags.
pub fn look_up(frame_up: [u32; 3], aim: [u32; 3], left_x: u32) -> Option<LookUp> {
    let [ux, uy, uz] = frame_up;
    let [ax, ay, az] = aim;
    let mut fp = Fp::new();
    let a = fp.mul4([uz, ux, 0, 0], [ax, ay, 0, 0]);
    let b = fp.mul4([az, uy, 0, 0], [ux, ax, 0, 0]);
    let [left_y, left_z, ..] = fp.sub4(a, b);
    let a = fp.mul4([left_z, az, left_y, 0], [ay, left_x, ax, 0]);
    let b = fp.mul4([left_y, left_z, left_x, 0], [az, ax, ay, 0]);
    let [x, y, z, _] = fp.sub4(a, b);
    fp.exact().then_some(LookUp {
        up: [x, y, z],
        subtrahends: [b[0], b[1], b[2]],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebasing_preserves_the_length_factor_of_a_nonunit_aim() {
        // aim × (up × aim), with an oblique up and aim of length three.
        let result = look_up(
            [2.0f32, 7.0, -1.0].map(f32::to_bits),
            [0.0f32, 3.0, 0.0].map(f32::to_bits),
            3.0f32.to_bits(),
        )
        .unwrap();
        assert_eq!(result.up.map(f32::from_bits), [18.0, 0.0, -9.0]);
    }

    #[test]
    fn ordinary_inputs_can_still_produce_an_ineligible_intermediate() {
        let large = 2.0f32.powi(40).to_bits();
        let zero = 0.0f32.to_bits();
        assert!(look_up([large, zero, zero], [zero, large, zero], zero).is_none());
    }
}
