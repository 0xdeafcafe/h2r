//! The float rules for every Rust sim kernel (docs/engine/sim-rewrite.md
//! section 3, "Float rules"), and the arithmetic that follows them.
//!
//! - IEEE binary32 or binary64 arithmetic, each operation rounded once.
//!   No `mul_add` (the crate's clippy.toml forbids it) and no fast-math.
//! - Reciprocal and reciprocal square root estimates are the Pentium III's
//!   tables from the `simd` crate, never the host's `rcp`/`rsqrt`.
//! - The guest's float environment is checked once per call, at entry
//!   ([`fast_env`]), not reset per operation.
//! - [`checked`] tests the thread's host float status around its scalar
//!   operations. [`Chain`] reproduces masked, round-to-nearest x87 chains
//!   in single and double precision. [`x87_neg_quotient`] also handles
//!   extended precision for the bounded BSP split.
//!
//! [`Fp`] provides range-proved arithmetic. Each operation computes
//! the correctly rounded host result and records whether its operands lie
//! in the fast-path range ([`crate::lanes::ordinary1`]). When every operand
//! of every operation did, and [`fast_env`] and [`host_fast_env`] held
//! at entry, the results match guest SSE and the guest's MXCSR is unchanged.
//! When any input fails, [`Fp::exact`] is false and the caller must
//! discard the results and let the original code run.
mod chain;
mod host;
mod region;

pub use chain::Chain;
pub use region::{Checked, checked, checked_env, checked_inputs, host_fast_env};

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

/// The x87 state a kernel's x87 sequences need: all six exceptions masked,
/// no exception pending, round to nearest, a precision control the
/// hardware defines (single, double or extended), and `free` registers
/// below TOP empty so the sequence's pushes cannot overflow the stack.
/// `tag` is the full tag word (two bits per physical register, 3 empty).
pub fn x87_env(control: u16, status: u16, tag: u16, free: u32) -> bool {
    if control & 0x3f != 0x3f || control & 0xc00 != 0 || control & 0x300 == 0x100 {
        return false;
    }
    if status & 0x80 != 0 || free > 8 {
        return false;
    }
    let top = u32::from(status >> 11) & 7;
    (1..=free).all(|k| {
        let r = (top + 8 - k) & 7;
        (tag >> (2 * r)) & 3 == 3
    })
}

/// The bits `fld a; fdiv b; fchs; fstp m32` leaves, and whether it set the
/// x87 precision flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct X87Store {
    pub value: u32,
    pub inexact: bool,
}

/// `fld dword a; fdiv dword b; fchs; fstp dword`, under [`x87_env`] with
/// `a` and `b` on the fast path ([`crate::lanes::ordinary1`]) and `b`
/// nonzero.
///
/// Why one host `f32` division gives the guest's bits in every precision
/// control. The quotient of two such values has magnitude zero or in
/// `(2^-126, 2^126)`, so it is normal in every format and no flag but PE can
/// be raised. With precision single, `fdiv` rounds it to 24 bits once. With
/// double or extended it rounds to 53 or 64 bits and `fstp` rounds that to
/// 24: a double rounding, which for a quotient is innocuous whenever the
/// first precision is at least `2p + 2 = 50` bits (Figueroa, "When is double
/// rounding innocuous?", 1995). `fchs` only flips the sign. PE is set when
/// the exact quotient is not a binary32 value, which is when the rounded
/// single times `b` (exact in `f64`: 24 by 24 bits) differs from `a`.
pub fn x87_neg_quotient(a: u32, b: u32) -> X87Store {
    let (x, y) = (f32::from_bits(a), f32::from_bits(b));
    let q = x / y;
    X87Store {
        value: (-q).to_bits(),
        inexact: f64::from(q) * f64::from(y) != f64::from(x),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x87_environment_needs_masked_nearest_and_free_registers() {
        // TOP 0, every register empty.
        assert!(x87_env(0x037f, 0, 0xffff, 2));
        assert!(x87_env(0x027f, 0, 0xffff, 2));
        assert!(x87_env(0x007f, 0, 0xffff, 2));
        assert!(!x87_env(0x037f, 0, 0xffff, 9), "more than eight pushes");
        assert!(!x87_env(0x017f, 0, 0xffff, 2), "reserved precision");
        assert!(!x87_env(0x0b7f, 0, 0xffff, 2), "round up");
        assert!(!x87_env(0x037e, 0, 0xffff, 2), "IE unmasked");
        assert!(!x87_env(0x037f, 0x80, 0xffff, 2), "exception pending");
        // TOP 0: pushes go to physical 7 then 6.
        let tag = 0xffff & !(3 << 12);
        assert!(x87_env(0x037f, 0, tag, 1));
        assert!(!x87_env(0x037f, 0, tag, 2));
        // TOP 3: pushes go to physical 2 then 1.
        let status = 3 << 11;
        assert!(x87_env(0x037f, status, 0xffff & !(3 << 6), 2));
        assert!(!x87_env(0x037f, status, 0xffff & !(3 << 2), 2));
    }

    /// The guest sequence on the `x87` crate, in each precision control.
    fn guest_neg_quotient(control: u16, a: u32, b: u32) -> X87Store {
        use x87::abi as fp;
        let mut s = fp::X87Abi {
            control,
            tag: 0xffff,
            ..Default::default()
        };
        fp::ld_f32(&mut s, a);
        fp::arith(&mut s, fp::arith_op::DIV, 0, fp::src_kind::F32, u64::from(b));
        fp::unary(&mut s, fp::unary_op::CHS);
        let (_, store) = fp::st_f32(&s, 0);
        fp::store_commit(&mut s, store.status, true);
        X87Store {
            value: store.value,
            inexact: s.status & fp::H2_FP_PE != 0,
        }
    }

    #[test]
    fn negated_quotient_matches_the_x87_in_every_precision() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let edges = [
            0u32,
            0x8000_0000,
            crate::lanes::LOW,
            crate::lanes::HIGH - 1,
            0x3f80_0000,
            0xbf80_0000,
            0x4040_0000,
            0x3eaa_aaab,
        ];
        for i in 0..20_000 {
            let r = next();
            let pick = |w: u64| {
                if w % 5 == 0 {
                    edges[(w >> 8) as usize % edges.len()]
                } else {
                    // Magnitudes across the whole fast-path range.
                    let span = crate::lanes::HIGH - crate::lanes::LOW;
                    let m = crate::lanes::LOW + (w >> 3) as u32 % span;
                    m | (((w >> 40) as u32 & 1) << 31)
                }
            };
            let a = pick(r);
            let mut b = pick((r >> 17) | ((i as u64) << 47));
            if b & 0x7fff_ffff == 0 {
                b = 0x3f80_0000;
            }
            let got = x87_neg_quotient(a, b);
            for control in [0x007f, 0x027f, 0x037f] {
                assert_eq!(
                    got,
                    guest_neg_quotient(control, a, b),
                    "{a:#x} / {b:#x} control {control:#x}"
                );
            }
        }
    }

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
