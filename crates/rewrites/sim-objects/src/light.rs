//! The endpoint blend shared by the animated radius and intensity of a light.

use sim_core::det::{self, Fp};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightBlend {
    /// The weighted second endpoint also survives in the guest's XMM registers.
    pub weighted_end: u32,
    pub value: u32,
}

/// Blend binary32 endpoints in the order used by the light record update.
/// `one` is the word supplied by the guest. A fade continuation can supply
/// it in a register. A successful blend leaves all guest MXCSR flags intact.
pub fn blend(start: u32, end: u32, weight: u32, one: u32, mxcsr: u32) -> Option<LightBlend> {
    if !det::fast_env(mxcsr) || !det::host_fast_env() {
        return None;
    }
    let mut fp = Fp::new();
    let complement = fp.sub(one, weight);
    let weighted_start = fp.mul(complement, start);
    let weighted_end = fp.mul(end, weight);
    let value = fp.add(weighted_start, weighted_end);
    fp.exact().then_some(LightBlend {
        weighted_end,
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_preserves_the_weighted_end_even_when_the_sum_cancels() {
        if det::host_fast_env() {
            let result = blend(
                (-2.0f32).to_bits(),
                6.0f32.to_bits(),
                0.25f32.to_bits(),
                1.0f32.to_bits(),
                0x1fa0,
            );
            assert_eq!(
                result,
                Some(LightBlend {
                    weighted_end: 1.5f32.to_bits(),
                    value: 0
                })
            );
        }
    }

    #[test]
    fn unproved_flags_and_operands_go_to_the_sse_model() {
        let one = 1.0f32.to_bits();
        assert_eq!(blend(one, one, one, one, 0x1f80), None);
        assert_eq!(blend(1, one, one, one, 0x1fa0), None);
        assert_eq!(blend(one, f32::NAN.to_bits(), one, one, 0x1fa0), None);
    }
}
