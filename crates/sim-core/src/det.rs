//! The float rules for every Rust sim kernel (docs/engine/sim-rewrite.md
//! section 3, "Float rules"), and the arithmetic that follows them.
//!
//! - IEEE binary32 add, subtract and multiply only, each rounded once.
//!   No `mul_add` (the crate's clippy.toml forbids it) and no fast-math.
//! - Reciprocal and reciprocal square root estimates are the Pentium III's
//!   tables from the `simd` crate, never the host's `rcp`/`rsqrt`.
//! - The guest's float environment is checked once per call, at entry
//!   ([`fast_env`]), not reset per operation.
//!
//! [`Fp`] is the arithmetic a bar-E kernel uses. Each operation computes
//! the correctly rounded host result and records whether its operands lie
//! in the fast-path range ([`crate::lanes::ordinary1`]). When every operand
//! of every operation did, and [`fast_env`] held at entry, the results are
//! the bits the guest's SSE produces and the guest's MXCSR would not have
//! changed. When any did not, [`Fp::exact`] is false and the caller must
//! discard the results and let the original code run.
use crate::lanes::{self, Op, Vec4};

/// MXCSR in the state the fast path needs: all exceptions masked, round to
/// nearest, precision already sticky. FZ and DAZ have nothing to act on
/// in the fast path, so either value is fine.
pub fn fast_env(mxcsr: u32) -> bool {
    mxcsr & 0x1f80 == 0x1f80 && mxcsr & 0x6020 == 0x20
}

/// Exact-or-decline binary32 arithmetic. See the module docs.
#[derive(Clone, Copy, Debug)]
pub struct Fp {
    ok: bool,
}

impl Default for Fp {
    fn default() -> Self {
        Self::new()
    }
}

impl Fp {
    pub const fn new() -> Self {
        Self { ok: true }
    }

    /// Whether every operation so far stayed on the fast path.
    pub fn exact(&self) -> bool {
        self.ok
    }

    /// Record operands that some other instruction (a compare, a
    /// conversion) consumes, under the same range rule.
    #[inline]
    pub fn check(&mut self, x: u32) {
        self.ok &= lanes::ordinary1(x);
    }

    #[inline]
    fn scalar(&mut self, op: Op, a: u32, b: u32) -> u32 {
        self.ok &= lanes::ordinary1(a) & lanes::ordinary1(b);
        lanes::binary1(op, a, b)
    }

    #[inline]
    pub fn add(&mut self, a: u32, b: u32) -> u32 {
        self.scalar(Op::Add, a, b)
    }

    #[inline]
    pub fn sub(&mut self, a: u32, b: u32) -> u32 {
        self.scalar(Op::Sub, a, b)
    }

    #[inline]
    pub fn mul(&mut self, a: u32, b: u32) -> u32 {
        self.scalar(Op::Mul, a, b)
    }

    #[inline]
    fn packed(&mut self, op: Op, a: Vec4, b: Vec4) -> Vec4 {
        if lanes::enabled() {
            self.ok &= lanes::ordinary(a, b);
            lanes::binary32(op, a, b)
        } else {
            self.ok &= lanes::scalar::ordinary(a, b);
            lanes::scalar::binary32(op, a, b)
        }
    }

    /// `addps`: four lanes, each rounded once.
    #[inline]
    pub fn add4(&mut self, a: Vec4, b: Vec4) -> Vec4 {
        self.packed(Op::Add, a, b)
    }

    #[inline]
    pub fn sub4(&mut self, a: Vec4, b: Vec4) -> Vec4 {
        self.packed(Op::Sub, a, b)
    }

    #[inline]
    pub fn mul4(&mut self, a: Vec4, b: Vec4) -> Vec4 {
        self.packed(Op::Mul, a, b)
    }
}

/// `rsqrtss`/`rsqrtps` per lane: the Pentium III estimate. Raises nothing.
#[inline]
pub fn rsqrt(x: u32) -> u32 {
    simd::rsqrt_approx(x)
}

/// `rcpss`: the Pentium III estimate. Raises nothing.
#[inline]
pub fn rcp(x: u32) -> u32 {
    simd::rcp_approx(x)
}

/// `cvtsi2ss` of a value the kernel knows is exactly representable
/// (`|x| < 2^24`); larger values round to nearest, which with PE sticky
/// changes nothing either.
#[inline]
pub fn from_int(x: i32) -> u32 {
    (x as f32).to_bits()
}

/// `cvttss2si` in the fast path: `x` is ordinary and below `2^31` in
/// magnitude ([`truncates`]), so the result is the truncation and only PE,
/// already sticky, could be raised.
#[inline]
pub fn truncate(x: u32) -> i32 {
    f32::from_bits(x) as i32
}

/// Whether [`truncate`] matches `cvttss2si` without raising IE.
#[inline]
pub fn truncates(x: u32) -> bool {
    lanes::ordinary1(x) && x & 0x7fff_ffff < 0x4f00_0000
}

/// `comiss a, b` for ordinary operands: `Some(Ordering)` (never unordered
/// in the fast path). Signed zeros compare equal.
#[inline]
pub fn compare(a: u32, b: u32) -> std::cmp::Ordering {
    f32::from_bits(a)
        .partial_cmp(&f32::from_bits(b))
        .unwrap_or(std::cmp::Ordering::Equal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_needs_masked_nearest_and_sticky_precision() {
        assert!(fast_env(0x1fa0));
        assert!(fast_env(0x9fe0));
        assert!(!fast_env(0x1f80));
        assert!(!fast_env(0x3fa0));
        assert!(!fast_env(0x1f20));
    }

    #[test]
    fn out_of_range_operands_decline() {
        let mut fp = Fp::new();
        assert_eq!(fp.add(1.5f32.to_bits(), 2.0f32.to_bits()), 3.5f32.to_bits());
        assert!(fp.exact());
        fp.mul(1.0f32.to_bits(), 1);
        assert!(!fp.exact());
        let mut fp = Fp::new();
        fp.add4([0; 4], [0x7f80_0000, 0, 0, 0]);
        assert!(!fp.exact());
    }

    #[test]
    fn truncation_range_excludes_overflow() {
        assert!(truncates(2.5f32.to_bits()));
        assert!(truncates((-2147483520.0f32).to_bits()));
        assert!(!truncates(2147483648.0f32.to_bits()));
        assert_eq!(truncate((-2.75f32).to_bits()), -2);
    }
}
