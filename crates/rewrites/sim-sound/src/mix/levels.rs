//! The four-speaker mix combines linear gains in extended precision, rounds
//! their magnitude to binary32, then converts the bounded magnitude to dB.
use crate::environment::GainCurve;
use x87::abi::{self, ARITH_POP, X87Abi, arith_op, src_kind, unary_op};

#[derive(Clone, Copy)]
pub struct SpeakerCurve {
    pub gain: GainCurve,
    pub db_floor: u32,
    pub input_floor_level: u32,
    pub output_floor_level: u32,
    pub quiet_magnitude: u32,
    pub maximum_magnitude: u32,
    pub output_scale: u32,
}

pub struct SpeakerLevel {
    /// Working value in dB, before the final output clamp.
    pub decibels: u32,
    pub send: u32,
    /// Some paths leave the maximum-magnitude SSE register untouched.
    pub loaded_maximum: bool,
    /// Determines the last x87 instruction and its last memory operand.
    pub evaluated_logarithm: bool,
}

fn above(mxcsr: &mut u32, a: u32, b: u32) -> bool {
    simd::comiss(mxcsr, a, b).expect("masked sound SSE") & 0x41 == 0
}

fn single(stack: &mut X87Abi) -> u32 {
    let (_, rounded) = abi::st_f32(stack, 0);
    abi::store_commit(stack, rounded.status, false);
    rounded.value
}

fn above_and_pop_both(stack: &mut X87Abi) -> bool {
    let flags = abi::cmp(stack, 0, src_kind::ST, 1, 1, true);
    abi::st_st(stack, 0, true);
    flags & 0x41 == 0
}

impl SpeakerCurve {
    pub fn combine(self, channels: [u32; 4], stack: &mut X87Abi, mxcsr: &mut u32) -> SpeakerLevel {
        abi::ld_f32(stack, 0);
        for channel in channels {
            let gain_db = if above(mxcsr, self.db_floor, channel) {
                self.input_floor_level
            } else if above(mxcsr, channel, 0) {
                0
            } else {
                channel
            };
            self.gain.linear(gain_db, stack);
            abi::ld_st(stack, 0);
            abi::arith(stack, arith_op::MUL, 0, src_kind::ST, 1);
            abi::arith(stack, arith_op::ADD | ARITH_POP, 2, src_kind::ST, 0);
            abi::st_st(stack, 0, true);
        }

        abi::unary(stack, unary_op::SQRT);
        let magnitude = single(stack);
        abi::ld_f32(stack, self.quiet_magnitude);
        let quiet = above_and_pop_both(stack);
        let bounded = if quiet {
            0
        } else if above(mxcsr, magnitude, self.maximum_magnitude) {
            self.maximum_magnitude
        } else {
            magnitude
        };

        // The driver tests the original rounded magnitude before taking the
        // logarithm of the bounded one, including log(0) in the quiet range.
        let evaluated_logarithm = above(mxcsr, magnitude, 0);
        let decibels = if evaluated_logarithm {
            abi::ld_f32(stack, bounded);
            abi::ld_const(stack, 5); // log10(2)
            abi::xch(stack, 1);
            abi::unary(stack, unary_op::YL2X);
            abi::arith(stack, arith_op::MUL, 0, src_kind::F32, u64::from(self.output_scale));
            let rounded = single(stack);
            abi::ld_f32(stack, self.db_floor);
            if above_and_pop_both(stack) {
                self.db_floor
            } else if above(mxcsr, rounded, 0) {
                0
            } else {
                rounded
            }
        } else {
            self.db_floor
        };

        let send = if above(mxcsr, self.db_floor, decibels) {
            self.output_floor_level
        } else if above(mxcsr, decibels, 0) {
            0
        } else {
            decibels
        };
        SpeakerLevel { decibels, send, loaded_maximum: !quiet, evaluated_logarithm }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve() -> SpeakerCurve {
        SpeakerCurve {
            gain: GainCurve { decibel_scale: 0.5f32.to_bits(), exponent_scale: 1.0f32.to_bits() },
            db_floor: (-30.0f32).to_bits(),
            input_floor_level: (-35.0f32).to_bits(),
            output_floor_level: (-32.0f32).to_bits(),
            quiet_magnitude: 0,
            maximum_magnitude: 0.1f32.to_bits(),
            output_scale: 10.0f32.to_bits(),
        }
    }

    #[test]
    fn four_unity_speakers_take_the_upper_amplitude_bound() {
        let mut stack = X87Abi::default();
        abi::ld_f32(&mut stack, 2.25f32.to_bits());
        let level = curve().combine([0; 4], &mut stack, &mut 0x1f80);
        assert_eq!(level.send, (-10.0f32).to_bits());
        assert!(level.loaded_maximum && level.evaluated_logarithm);
        assert_eq!(abi::st_f32(&stack, 0).1.value, 2.25f32.to_bits());
    }

    #[test]
    fn subthreshold_nonzero_power_takes_the_logarithm_of_zero() {
        let mut curve = curve();
        curve.quiet_magnitude = 3.0f32.to_bits();
        let mut stack = X87Abi::default();
        let level = curve.combine([0; 4], &mut stack, &mut 0x1f80);
        assert_eq!(level.send, curve.db_floor);
        assert!(!level.loaded_maximum && level.evaluated_logarithm);
        assert_ne!(stack.status & abi::H2_FP_ZE, 0);
    }
}
