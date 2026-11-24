//! Conversion from source gains and filter settings to DSP coefficients.
//! The guest keeps intermediates in extended precision. Use its floating-point
//! stack so callers also see the same precision, status bits and rounding.

use x87::Const;
use x87::abi::{self as fp, X87Abi, arith_op, src_kind, unary_op};

/// Leave `scale * log10(gain)` on top of the caller's masked x87 stack.
pub fn gain_level(fpu: &mut X87Abi, gain: u32, scale: u32) {
    fp::ld_f32(fpu, gain);
    fp::ld_const(fpu, Const::Lg2 as u32);
    fp::xch(fpu, 1);
    fp::unary(fpu, unary_op::YL2X);
    multiply(fpu, scale);
}

/// The owner's coefficient scales, read from the image by the adapter.
pub struct CutoffScale {
    pub radians_per_hz: [u32; 2],
    pub log_scale: u32,
}

/// Convert an in-range frequency. The final store rounds to binary32 before
/// the caller truncates to the low sixteen bits.
pub fn cutoff(fpu: &mut X87Abi, hz: u32, scale: CutoffScale) -> fp::Store<u32> {
    fp::ld_f32(fpu, hz);
    for factor in scale.radians_per_hz {
        multiply(fpu, factor);
    }
    fp::unary(fpu, unary_op::SIN);
    fp::arith(fpu, arith_op::ADD, 0, src_kind::ST, 0);
    fp::ld_const(fpu, Const::Ln2 as u32);
    fp::xch(fpu, 1);
    fp::unary(fpu, unary_op::YL2X);
    multiply(fpu, scale.log_scale);
    fp::st_f32(fpu, 0).1
}

/// Leave the exponential resonance coefficient on ST(0) for the caller's
/// truncating integer conversion. Keep the decomposition into integral and
/// fractional exponents in extended precision.
pub fn resonance(fpu: &mut X87Abi, value: u32, exponent_scale: u32, output_scale: u32) {
    fp::ld_f32(fpu, value);
    multiply(fpu, exponent_scale);
    fp::ld_const(fpu, Const::L2E as u32);
    fp::arith(fpu, arith_op::MUL | fp::ARITH_POP, 1, src_kind::ST, 0);
    fp::ld_st(fpu, 0);
    fp::unary(fpu, unary_op::RNDINT);
    fp::xch(fpu, 1);
    fp::arith(fpu, arith_op::SUB, 0, src_kind::ST, 1);
    fp::unary(fpu, unary_op::X2M1);
    fp::ld_const(fpu, Const::One as u32);
    fp::arith(fpu, arith_op::ADD | fp::ARITH_POP, 1, src_kind::ST, 0);
    fp::unary(fpu, unary_op::SCALE);
    fp::st_st(fpu, 1, true);
    multiply(fpu, output_scale);
}

fn multiply(fpu: &mut X87Abi, factor: u32) {
    fp::arith(fpu, arith_op::MUL, 0, src_kind::F32, u64::from(factor));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_and_resonance_leave_one_result_above_the_callers_value() {
        let mut fpu = X87Abi::default();
        let original = 17.25f32.to_bits();
        fp::ld_f32(&mut fpu, original);
        for (input, expected) in [(1.0f32, 0.0f32), (10.0, 3.0), (100.0, 6.0)] {
            gain_level(&mut fpu, input.to_bits(), 3.0f32.to_bits());
            let result = fp::st_f32(&fpu, 0).1;
            assert_eq!(result.value, expected.to_bits());
            fp::store_commit(&mut fpu, result.status, true);
            assert_eq!(fp::st_f32(&fpu, 0).1.value, original);
        }
        resonance(&mut fpu, 0, 8.0f32.to_bits(), 3.5f32.to_bits());
        assert_eq!(fp::st_f32(&fpu, 0).1.value, 3.5f32.to_bits());
        assert_eq!(fp::st_f32(&fpu, 1).1.value, original);
    }
}
