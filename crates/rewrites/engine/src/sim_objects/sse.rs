//! Scalar SSE and the flag idioms of the type updates, on the guest's own
//! registers (docs/engine/sim-rewrite-objects.md section 7, "Floats and
//! SIMD"). Each helper is one retail instruction: it reads and writes the
//! `xmm` registers, MXCSR and EFLAGS as the instruction does, so a body
//! hands the lifted code, or a callee, the same float state retail has.
//!
//! Arithmetic takes the host fast path (`sim_core::lanes`) when MXCSR
//! rounds to nearest with PE already sticky and both operands are
//! ordinary; otherwise the `simd` model, which also keeps MXCSR's sticky
//! flags. Bodies only run where [`super::supported`] holds, so every
//! exception is masked and the model cannot fail.

use runtime::CpuAbi;
use sim_core::lanes::{self, Op};

/// The whole register.
pub(crate) fn get(c: &CpuAbi, r: usize) -> u128 {
    u128::from_le_bytes(c.xmm[r])
}

pub(crate) fn set(c: &mut CpuAbi, r: usize, v: u128) {
    c.xmm[r] = v.to_le_bytes();
}

/// Lane 0 as binary32 bits.
pub(crate) fn lane0(c: &CpuAbi, r: usize) -> u32 {
    get(c, r) as u32
}

/// `movss xmm, m32`: lane 0 loaded, lanes 1 to 3 cleared.
pub(crate) fn load(c: &mut CpuAbi, r: usize, bits: u32) {
    set(c, r, u128::from(bits));
}

/// `movss xmm, xmm`: lane 0 replaced, lanes 1 to 3 kept.
#[cfg(test)]
pub(crate) fn mov(c: &mut CpuAbi, dst: usize, bits: u32) {
    let v = get(c, dst);
    set(c, dst, (v & !u128::from(u32::MAX)) | u128::from(bits));
}

/// `movaps dst, src`.
pub(crate) fn movaps(c: &mut CpuAbi, dst: usize, src: usize) {
    c.xmm[dst] = c.xmm[src];
}

/// `xorps r, r`.
pub(crate) fn zero(c: &mut CpuAbi, r: usize) {
    c.xmm[r] = [0; 16];
}

/// `xorps dst, [m128]` with a sign mask (`-x` on every lane).
#[allow(dead_code, reason = "mirrors the packed XOR instruction alongside register zeroing")]
pub(crate) fn xorps(c: &mut CpuAbi, dst: usize, mask: u128) {
    let v = get(c, dst);
    set(c, dst, v ^ mask);
}

/// The scalar operations the type updates use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ss {
    Add,
    Sub,
    Mul,
    Div,
    #[allow(dead_code, reason = "keeps the x86 minss operation in the scalar SSE model")]
    Min,
    #[allow(dead_code, reason = "keeps the x86 maxss operation in the scalar SSE model")]
    Max,
}

/// `op dst, b` (`addss`, `subss`, `mulss`, `divss`, `minss`, `maxss`):
/// lane 0 = lane 0 of `dst` op `b`, lanes 1 to 3 kept. Returns lane 0.
pub(crate) fn ss(c: &mut CpuAbi, op: Ss, dst: usize, b: u32) -> u32 {
    let a = get(c, dst);
    let host = match op {
        Ss::Add => Some(Op::Add),
        Ss::Sub => Some(Op::Sub),
        Ss::Mul => Some(Op::Mul),
        _ => None,
    };
    let r = match host {
        Some(h) if c.mxcsr & 0x6020 == 0x20 && lanes::ordinary1(a as u32) && lanes::ordinary1(b) => {
            (a & !u128::from(u32::MAX)) | u128::from(lanes::binary1(h, a as u32, b))
        }
        _ => {
            let m = &mut c.mxcsr;
            match op {
                Ss::Add => simd::addss(m, a, b),
                Ss::Sub => simd::subss(m, a, b),
                Ss::Mul => simd::mulss(m, a, b),
                Ss::Div => simd::divss(m, a, b),
                Ss::Min => simd::minss(m, a, b),
                Ss::Max => simd::maxss(m, a, b),
            }
            .expect("bodies run with every SSE exception masked")
        }
    };
    set(c, dst, r);
    r as u32
}

/// `op dst, src` between two registers.
pub(crate) fn ss_reg(c: &mut CpuAbi, op: Ss, dst: usize, src: usize) -> u32 {
    let b = lane0(c, src);
    ss(c, op, dst, b)
}

/// `sqrtss dst, b`.
#[allow(dead_code, reason = "mirrors the scalar square-root instruction for guest bodies")]
pub(crate) fn sqrtss(c: &mut CpuAbi, dst: usize, b: u32) -> u32 {
    let a = get(c, dst);
    let r = simd::sqrtss(&mut c.mxcsr, a, b).expect("bodies run with every SSE exception masked");
    set(c, dst, r);
    r as u32
}

/// `cvtsi2ss dst, r32`.
pub(crate) fn cvtsi2ss(c: &mut CpuAbi, dst: usize, i: u32) -> u32 {
    let a = get(c, dst);
    let r = simd::cvtsi2ss(&mut c.mxcsr, a, i).expect("bodies run with every SSE exception masked");
    set(c, dst, r);
    r as u32
}

/// `cvttss2si r32, b`.
#[allow(dead_code, reason = "pairs the guest conversion to float with the truncating conversion back")]
pub(crate) fn cvttss2si(c: &mut CpuAbi, b: u32) -> u32 {
    simd::cvttss2si(&mut c.mxcsr, b).expect("bodies run with every SSE exception masked")
}

/// ZF, PF and CF; `comiss` and `ucomiss` clear OF, SF and AF.
const COMPARE: u32 = 0x8d5;

/// `comiss a, b`: EFLAGS as the instruction leaves them (unordered ZF PF
/// CF, less CF, equal ZF, greater none). Returns those bits.
pub(crate) fn comiss(c: &mut CpuAbi, a: u32, b: u32) -> u32 {
    let f = simd::comiss(&mut c.mxcsr, a, b).expect("bodies run with every SSE exception masked");
    c.eflags = (c.eflags & !COMPARE) | f;
    c.eflags_defined |= COMPARE;
    f
}

/// `ucomiss a, b`: as [`comiss`], quiet on QNaN.
pub(crate) fn ucomiss(c: &mut CpuAbi, a: u32, b: u32) -> u32 {
    let f = simd::ucomiss(&mut c.mxcsr, a, b).expect("bodies run with every SSE exception masked");
    c.eflags = (c.eflags & !COMPARE) | f;
    c.eflags_defined |= COMPARE;
    f
}

/// `lahf`: `ah` = SF ZF 0 AF 0 PF 1 CF.
pub(crate) fn lahf(c: &mut CpuAbi) {
    let ah = (c.eflags & 0xd5) | 2;
    c.eax = (c.eax & !0xff00) | (ah << 8);
}

/// The jumps after a compare, on the EFLAGS it left.
pub(crate) fn below_or_equal(c: &CpuAbi) -> bool {
    c.eflags & 0x41 != 0
}

pub(crate) fn below(c: &CpuAbi) -> bool {
    c.eflags & 0x01 != 0
}

pub(crate) fn parity(c: &CpuAbi) -> bool {
    c.eflags & 0x04 != 0
}

#[allow(dead_code, reason = "keeps the equality predicate with the SSE comparison flag helpers")]
pub(crate) fn zero_flag(c: &CpuAbi) -> bool {
    c.eflags & 0x40 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu() -> CpuAbi {
        CpuAbi { mxcsr: 0x1fa0, ..Default::default() }
    }

    #[test]
    fn movss_from_memory_clears_the_upper_lanes_and_arithmetic_keeps_them() {
        let mut c = cpu();
        set(&mut c, 1, u128::MAX);
        load(&mut c, 1, 2.0f32.to_bits());
        assert_eq!(get(&c, 1), u128::from(2.0f32.to_bits()));
        set(&mut c, 2, (7u128 << 64) | u128::from(3.0f32.to_bits()));
        assert_eq!(ss(&mut c, Ss::Mul, 2, 2.0f32.to_bits()), 6.0f32.to_bits());
        assert_eq!(get(&c, 2) >> 64, 7);
        mov(&mut c, 2, 1.0f32.to_bits());
        assert_eq!(get(&c, 2), (7u128 << 64) | u128::from(1.0f32.to_bits()));
    }

    #[test]
    fn the_model_and_the_fast_path_agree_and_keep_mxcsr() {
        let mut fast = cpu();
        let mut model = CpuAbi { mxcsr: 0x1f80, ..Default::default() };
        for (a, b) in [(1.5f32, 0.1f32), (-3.0, 7.25), (1e10, 3e-5)] {
            load(&mut fast, 0, a.to_bits());
            load(&mut model, 0, a.to_bits());
            assert_eq!(ss(&mut fast, Ss::Add, 0, b.to_bits()), ss(&mut model, Ss::Add, 0, b.to_bits()));
        }
        // The model raised PE on an inexact sum; the fast path needs it sticky.
        assert_eq!(model.mxcsr & 0x20, 0x20);
        assert_eq!(fast.mxcsr, 0x1fa0);
    }

    #[test]
    fn compares_set_zf_pf_cf_and_clear_the_rest() {
        let mut c = cpu();
        c.eflags = 0x8d5;
        assert_eq!(comiss(&mut c, 1.0f32.to_bits(), 2.0f32.to_bits()), 0x01);
        assert_eq!(c.eflags & 0x8d5, 0x01);
        assert!(below(&c) && below_or_equal(&c));
        assert_eq!(comiss(&mut c, 2.0f32.to_bits(), 2.0f32.to_bits()), 0x40);
        assert!(!below(&c) && below_or_equal(&c));
        assert_eq!(ucomiss(&mut c, f32::NAN.to_bits(), 0), 0x45);
        assert!(parity(&c));
        c.eax = 0x1234_5678;
        lahf(&mut c);
        assert_eq!(c.eax, 0x1234_4778);
    }
}
