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

/// [`fast_env`] but for PE: all exceptions masked and round to nearest,
/// with PE sticky or not. [`Fp::tracked`] runs the fast path under it and
/// raises PE as the guest would; the live game runs at `0x1f80`.
pub fn guest_env(mxcsr: u32) -> bool {
    fast_env(mxcsr | 0x20)
}

/// Exact-or-decline binary32 arithmetic. See the module docs.
///
/// [`Fp::guest`] runs under the guest's MXCSR instead: an operation on the
/// fast path (the MXCSR passes [`fast_env`], the operands are ordinary)
/// takes the host result, and every other one takes the `simd` crate's
/// SSE model, which rounds as the MXCSR says and raises its flags. Only an
/// unmasked exception, which the guest would deliver as `#XM`, leaves
/// [`Fp::exact`] false. [`Fp::mxcsr`] is the guest's MXCSR afterwards.
///
/// [`Fp::tracked`] is the fast path under the guest's MXCSR when that
/// MXCSR passes [`guest_env`] but PE is not yet sticky: it declines as
/// [`Fp::new`] does, and models operations only until one raises PE.
#[derive(Clone, Copy, Debug)]
pub struct Fp {
    ok: bool,
    guest: Option<u32>,
    /// [`Fp::tracked`]: an operand off the fast path declines.
    strict: bool,
}

impl Default for Fp {
    fn default() -> Self {
        Self::new()
    }
}

impl Fp {
    pub const fn new() -> Self {
        Self { ok: true, guest: None, strict: false }
    }

    /// Arithmetic under the guest's `mxcsr`, as the instructions would run.
    pub const fn guest(mxcsr: u32) -> Self {
        Self { ok: true, guest: Some(mxcsr), strict: false }
    }

    /// [`Fp::new`]'s fast path under the guest's `mxcsr`, which needs only
    /// [`guest_env`]: results are the host's, an operand off the fast path
    /// declines, and [`Fp::mxcsr`] gains the flags (PE) the operations
    /// raise. Operations run through the SSE model until PE is sticky,
    /// then on the host.
    pub const fn tracked(mxcsr: u32) -> Self {
        Self { ok: true, guest: Some(mxcsr), strict: true }
    }

    /// The guest's MXCSR after the operations so far, for [`Fp::guest`].
    pub fn mxcsr(&self) -> Option<u32> {
        self.guest
    }

    /// Under [`Fp::guest`], raise `flags` as an instruction outside the
    /// arithmetic would (a conversion's PE); an unmasked one refuses.
    /// Without `guest` nothing changes.
    pub fn raise(&mut self, flags: u32) {
        let Some(m) = self.guest else { return };
        let masked = (m >> simd::MASK_SHIFT) & 0x3f;
        if flags & !masked & 0x3f != 0 {
            self.ok = false;
        } else {
            self.guest = Some(m | flags);
        }
    }

    /// One modelled instruction: its result, or a refusal on `#XM`.
    fn model<T: Copy>(&mut self, mxcsr: u32, fallback: T, run: impl FnOnce(&mut u32) -> Result<T, simd::Xm>) -> T {
        let mut m = mxcsr;
        match run(&mut m) {
            Ok(r) => {
                self.guest = Some(m);
                r
            }
            Err(simd::Xm) => {
                self.ok = false;
                fallback
            }
        }
    }

    /// Whether every operation so far stayed on the fast path.
    pub fn exact(&self) -> bool {
        self.ok
    }

    /// Record operands that some other instruction (a compare, a
    /// conversion) consumes, under the same range rule.
    ///
    /// Under [`Fp::guest`] the operand is a `comiss` operand: a NaN raises
    /// IE and a subnormal DE (unless DAZ), as the compare would.
    #[inline]
    pub fn check(&mut self, x: u32) {
        let Some(m) = self.guest.filter(|_| !self.strict) else {
            // An ordinary operand raises nothing in a compare.
            self.ok &= lanes::ordinary1(x);
            return;
        };
        let bits = x & 0x7fff_ffff;
        let flag = if bits > 0x7f80_0000 {
            simd::IE
        } else if bits != 0 && bits < 0x0080_0000 && m & 0x40 == 0 {
            simd::DE
        } else {
            0
        };
        if flag != 0 {
            if m & (flag << simd::MASK_SHIFT) == 0 {
                self.ok = false;
            } else {
                self.guest = Some(m | flag);
            }
        }
    }

    /// Record an operand that is only compared (`comiss`, `fcom`) and
    /// never enters arithmetic. Zero, normal and infinite magnitudes
    /// compare without raising a flag, so even `FLT_MAX` stays on the fast
    /// path; a NaN (invalid) or a subnormal (denormal) does not.
    #[inline]
    pub fn check_compared(&mut self, x: u32) {
        self.ok &= lanes::compared1(x);
    }

    #[inline]
    fn scalar(&mut self, op: Op, a: u32, b: u32) -> u32 {
        let fast = lanes::ordinary1(a) & lanes::ordinary1(b);
        if let Some(m) = self.guest
            && !(fast && fast_env(m))
            && (fast || !self.strict)
        {
            let host = lanes::binary1(op, a, b);
            return self.model(m, host, |m| {
                let a = u128::from(a);
                match op {
                    Op::Add => simd::addss(m, a, b),
                    Op::Sub => simd::subss(m, a, b),
                    Op::Mul => simd::mulss(m, a, b),
                }
                .map(|r| r as u32)
            });
        }
        self.ok &= fast;
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

    /// `divss` of two ordinary values: one correctly rounded host division.
    /// The quotient of ordinary values is zero or of magnitude in
    /// `(2^-126, 2^126)`, so it is normal in binary32, only PE (already
    /// sticky) can be raised, and the result cannot depend on FTZ or DAZ
    /// (the argument [`x87_neg_quotient`] makes in every precision). A zero
    /// divisor is excluded: its infinity is exact, but `0/0` is a NaN whose
    /// payload the host picks.
    #[inline]
    pub fn div(&mut self, a: u32, b: u32) -> u32 {
        let fast = lanes::ordinary1(a) & lanes::ordinary1(b) & (b & 0x7fff_ffff != 0);
        let host = (f32::from_bits(a) / f32::from_bits(b)).to_bits();
        if let Some(m) = self.guest
            && !(fast && fast_env(m))
            && (fast || !self.strict)
        {
            return self.model(m, host, |m| simd::divss(m, u128::from(a), b).map(|r| r as u32));
        }
        self.ok &= fast;
        host
    }

    #[inline]
    fn packed(&mut self, op: Op, a: Vec4, b: Vec4) -> Vec4 {
        let fast = lanes::scalar::ordinary(a, b);
        if let Some(m) = self.guest
            && !(fast && fast_env(m))
            && (fast || !self.strict)
        {
            let host = lanes::scalar::binary32(op, a, b);
            let pack = |v: Vec4| v.iter().rev().fold(0u128, |bits, &x| (bits << 32) | u128::from(x));
            let (a, b) = (pack(a), pack(b));
            return self.model(m, host, |m| {
                match op {
                    Op::Add => simd::addps(m, a, b),
                    Op::Sub => simd::subps(m, a, b),
                    Op::Mul => simd::mulps(m, a, b),
                }
                .map(|r| std::array::from_fn(|i| (r >> (32 * i)) as u32))
            });
        }
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
    fn guest_arithmetic_runs_under_any_masked_mxcsr() {
        let (a, b) = (1.1f32.to_bits(), 3.3f32.to_bits());
        // PE clear: the result is the host's and PE comes out set.
        let mut fp = Fp::guest(0x1f80);
        assert_eq!(fp.mul(a, b), (1.1f32 * 3.3).to_bits());
        assert_eq!(fp.mxcsr(), Some(0x1fa0));
        let q = fp.mul4([a; 4], [b; 4]);
        assert_eq!(q, [(1.1f32 * 3.3).to_bits(); 4]);
        assert!(fp.exact());
        // An exact operation leaves PE clear.
        let mut fp = Fp::guest(0x1f80);
        assert_eq!(fp.add(1.0f32.to_bits(), 2.0f32.to_bits()), 3.0f32.to_bits());
        assert_eq!(fp.mxcsr(), Some(0x1f80));
        // Operands off the fast range take the SSE model: a product that
        // underflows raises UE and PE, and the result is the model's.
        let tiny = 1.0e-30f32.to_bits();
        let mut fp = Fp::guest(0x1fa0);
        let mut m = 0x1fa0;
        let want = simd::mulss(&mut m, u128::from(tiny), tiny).unwrap() as u32;
        assert_eq!(fp.mul(tiny, tiny), want);
        assert_eq!(fp.mxcsr(), Some(m));
        assert_ne!(m & simd::UE, 0);
        assert!(fp.exact());
        // A comiss operand: a NaN raises IE.
        fp.check(0x7fc0_0000);
        assert_ne!(fp.mxcsr().unwrap() & simd::IE, 0);
        // Round toward zero is modelled, not refused.
        let mut fp = Fp::guest(0x7f80);
        assert_eq!(fp.div(1.0f32.to_bits(), 3.0f32.to_bits()), 0x3eaa_aaaa);
        assert!(fp.exact());
        // An unmasked exception is the guest's `#XM`: refused.
        let mut fp = Fp::guest(0x0f80);
        fp.mul(a, b);
        assert!(!fp.exact());
        // Without `guest` nothing changes: off-range operands refuse.
        let mut fp = Fp::new();
        fp.mul(tiny, tiny);
        assert!(!fp.exact());
        assert_eq!(fp.mxcsr(), None);
    }

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
    fn tracked_arithmetic_is_the_fast_path_and_raises_pe() {
        let (a, b) = (1.1f32.to_bits(), 3.3f32.to_bits());
        // The live MXCSR, PE clear: host results, PE out set.
        let mut fp = Fp::tracked(0x1f80);
        assert_eq!(fp.add(1.0f32.to_bits(), 2.0f32.to_bits()), 3.0f32.to_bits());
        assert_eq!(fp.mxcsr(), Some(0x1f80), "an exact add raises nothing");
        assert_eq!(fp.mul(a, b), (1.1f32 * 3.3).to_bits());
        assert_eq!(fp.div(a, b), (1.1f32 / 3.3).to_bits());
        assert_eq!(fp.mul4([a; 4], [b; 4]), [(1.1f32 * 3.3).to_bits(); 4]);
        fp.check(a);
        assert_eq!(fp.mxcsr(), Some(0x1fa0));
        assert!(fp.exact());
        // An operand off the fast path declines, as under `new`, in a
        // scalar, a packed operation and a compare alike.
        let runs: [fn(&mut Fp); 4] = [
            |fp| {
                fp.mul(1.0e-30f32.to_bits(), 1.0e-30f32.to_bits());
            },
            |fp| {
                fp.add4([0; 4], [0x7f80_0000, 0, 0, 0]);
            },
            |fp| {
                fp.div(1.0f32.to_bits(), 0);
            },
            |fp| fp.check(0x7fc0_0000),
        ];
        for run in runs {
            let mut fp = Fp::tracked(0x1f80);
            run(&mut fp);
            assert!(!fp.exact());
        }
        assert!(guest_env(0x1f80) && guest_env(0x1fa0));
        assert!(!guest_env(0x3f80) && !guest_env(0x1f00));
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

    #[test]
    fn division_rounds_once_and_declines_zero_divisors() {
        let mut fp = Fp::new();
        assert_eq!(fp.div(1.0f32.to_bits(), 3.0f32.to_bits()), (1.0f32 / 3.0).to_bits());
        assert_eq!(fp.div(0x8000_0000, 2.0f32.to_bits()), 0x8000_0000, "-0 / 2 = -0");
        assert!(fp.exact());
        fp.div(1.0f32.to_bits(), 0);
        assert!(!fp.exact(), "0/0's NaN payload is the host's");
        let mut fp = Fp::new();
        fp.div(0x7f80_0000, 2.0f32.to_bits());
        assert!(!fp.exact(), "an infinity operand is off the fast path");
    }
}
