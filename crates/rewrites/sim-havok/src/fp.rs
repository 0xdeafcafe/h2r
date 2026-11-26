//! The float arithmetic a Havok kernel is written against
//! (sim-rewrite-havok.md section 7.1, sim-rewrite.md section 3).
//!
//! [`Arith`] is the binary32 SSE scalar set the retail code uses: separate
//! multiplies and adds, never fused, and `rsqrtss`/`rcpss` as the
//! Pentium III estimates. Two implementations:
//!
//! - [`Exact`] runs every operation through the `simd` crate under a
//!   guest MXCSR, so NaN payloads, denormals, signed zeros, flush-to-zero
//!   and all four rounding modes match the machine. An unmasked exception
//!   sets [`Exact::trapped`]; the caller must then decline.
//! - [`Fast`] is host `f32`, one rounding per operation. It matches
//!   [`Exact`] only for operands the caller proved ordinary (see
//!   `sim_core::det::Fp`) under a nearest-rounding, masked MXCSR.
//!
//! The estimates go through `simd::rsqrtss` and `simd::rcpss` in both.
//! [`X87`] is the x87 double-precision counterpart for the chains retail
//! keeps on the FPU.

use x87::abi::{self, X87Abi, arith_op, src_kind, status};

/// Binary32 scalar arithmetic on raw bits.
pub trait Arith {
    fn add(&mut self, a: u32, b: u32) -> u32;
    fn sub(&mut self, a: u32, b: u32) -> u32;
    fn mul(&mut self, a: u32, b: u32) -> u32;
    fn div(&mut self, a: u32, b: u32) -> u32;
    fn sqrt(&mut self, a: u32) -> u32;
    /// `rsqrtss`: the Pentium III estimate, never the host's.
    fn rsqrt(&mut self, a: u32) -> u32 {
        simd::rsqrtss(0, a) as u32
    }
    /// `rcpss`: the Pentium III estimate.
    fn rcp(&mut self, a: u32) -> u32 {
        simd::rcpss(0, a) as u32
    }
}

/// SSE scalar arithmetic under a guest MXCSR.
#[derive(Clone, Copy, Debug)]
pub struct Exact {
    pub mxcsr: u32,
    /// An operation raised an exception the MXCSR does not mask. Its
    /// result is zero and meaningless.
    pub trapped: bool,
}

impl Exact {
    pub const fn new(mxcsr: u32) -> Self {
        Self { mxcsr, trapped: false }
    }

    fn lane0(&mut self, r: Result<u128, simd::Xm>) -> u32 {
        match r {
            Ok(v) => v as u32,
            Err(simd::Xm) => {
                self.trapped = true;
                0
            }
        }
    }
}

impl Arith for Exact {
    fn add(&mut self, a: u32, b: u32) -> u32 {
        let r = simd::addss(&mut self.mxcsr, a.into(), b);
        self.lane0(r)
    }
    fn sub(&mut self, a: u32, b: u32) -> u32 {
        let r = simd::subss(&mut self.mxcsr, a.into(), b);
        self.lane0(r)
    }
    fn mul(&mut self, a: u32, b: u32) -> u32 {
        let r = simd::mulss(&mut self.mxcsr, a.into(), b);
        self.lane0(r)
    }
    fn div(&mut self, a: u32, b: u32) -> u32 {
        let r = simd::divss(&mut self.mxcsr, a.into(), b);
        self.lane0(r)
    }
    fn sqrt(&mut self, a: u32) -> u32 {
        let r = simd::sqrtss(&mut self.mxcsr, 0, a);
        self.lane0(r)
    }
}

/// Host binary32, one rounding per operation, no fusing.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fast;

fn f(bits: u32) -> f32 {
    f32::from_bits(bits)
}

impl Arith for Fast {
    fn add(&mut self, a: u32, b: u32) -> u32 {
        (f(a) + f(b)).to_bits()
    }
    fn sub(&mut self, a: u32, b: u32) -> u32 {
        (f(a) - f(b)).to_bits()
    }
    fn mul(&mut self, a: u32, b: u32) -> u32 {
        (f(a) * f(b)).to_bits()
    }
    fn div(&mut self, a: u32, b: u32) -> u32 {
        (f(a) / f(b)).to_bits()
    }
    fn sqrt(&mut self, a: u32) -> u32 {
        f(a).sqrt().to_bits()
    }
}

/// Double-precision x87 arithmetic: each operation loads both operands as
/// binary64, operates under the control word, and stores binary64.
#[derive(Clone, Debug)]
pub struct X87 {
    st: X87Abi,
}

impl X87 {
    pub fn new(control: u16) -> Self {
        let mut st = X87Abi::default();
        abi::init(&mut st);
        st.control = control;
        Self { st }
    }

    /// The accumulated status word (exception flags).
    pub fn status(&self) -> u16 {
        self.st.status
    }

    /// `None` when the helper reported anything but `OK`.
    fn op(&mut self, op: u32, a: u64, b: u64) -> Option<u64> {
        if abi::ld_f64(&mut self.st, a) & status::MASK != status::OK {
            return None;
        }
        if abi::arith(&mut self.st, op, 0, src_kind::F64, b) & status::MASK != status::OK {
            return None;
        }
        let (rc, store) = abi::st_f64(&self.st, 0);
        if rc & status::MASK != status::OK {
            return None;
        }
        abi::store_commit(&mut self.st, store.status, true);
        Some(store.value)
    }

    pub fn add(&mut self, a: u64, b: u64) -> Option<u64> {
        self.op(arith_op::ADD, a, b)
    }
    pub fn sub(&mut self, a: u64, b: u64) -> Option<u64> {
        self.op(arith_op::SUB, a, b)
    }
    pub fn mul(&mut self, a: u64, b: u64) -> Option<u64> {
        self.op(arith_op::MUL, a, b)
    }
    pub fn div(&mut self, a: u64, b: u64) -> Option<u64> {
        self.op(arith_op::DIV, a, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODES: [u32; 4] = [0x1f80, 0x3f80, 0x5f80, 0x7f80];

    fn rng(seed: &mut u64) -> u32 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*seed >> 32) as u32
    }

    /// Specials first, then seeded words and seeded ordinary values.
    fn inputs() -> Vec<u32> {
        let mut v = vec![
            0,
            0x8000_0000,
            1,
            0x8000_0001,
            0x007f_ffff,
            0x0080_0000,
            0x3f80_0000,
            0xbf80_0000,
            0x7f7f_ffff,
            0x7f80_0000,
            0xff80_0000,
            0x7fc0_0000,
            0x7f80_0001,
            0xffc0_1234,
        ];
        let mut s = 0x1234_5678_9abc_def0u64;
        for _ in 0..200 {
            v.push(rng(&mut s));
        }
        for _ in 0..200 {
            let x = (rng(&mut s) as i32 as f32) / 65536.0;
            v.push(x.to_bits());
        }
        v
    }

    fn lane(r: Result<u128, simd::Xm>) -> u32 {
        r.unwrap() as u32
    }

    #[test]
    fn exact_matches_simd_in_every_rounding_mode() {
        let xs = inputs();
        for mxcsr in MODES {
            for (i, &a) in xs.iter().enumerate() {
                let b = xs[(i * 7 + 3) % xs.len()];
                let mut e = Exact::new(mxcsr);
                let mut m = mxcsr;
                assert_eq!(e.add(a, b), lane(simd::addss(&mut m, a.into(), b)));
                assert_eq!(e.sub(a, b), lane(simd::subss(&mut m, a.into(), b)));
                assert_eq!(e.mul(a, b), lane(simd::mulss(&mut m, a.into(), b)));
                assert_eq!(e.div(a, b), lane(simd::divss(&mut m, a.into(), b)));
                assert_eq!(e.sqrt(a), lane(simd::sqrtss(&mut m, 0, a)));
                assert_eq!(e.mxcsr, m);
                assert!(!e.trapped);
            }
        }
    }

    #[test]
    fn estimates_are_the_simd_tables_in_both() {
        for a in inputs() {
            assert_eq!(Fast.rsqrt(a), simd::rsqrt_approx(a));
            assert_eq!(Exact::new(0x1f80).rsqrt(a), simd::rsqrt_approx(a));
            assert_eq!(Fast.rcp(a), simd::rcp_approx(a));
        }
    }

    #[test]
    fn unmasked_exception_sets_trapped() {
        // Invalid operation unmasked: inf - inf.
        let mut e = Exact::new(0x1f80 & !(simd::IE << simd::MASK_SHIFT));
        e.sub(0x7f80_0000, 0x7f80_0000);
        assert!(e.trapped);
    }

    #[test]
    fn fast_matches_exact_on_ordinary_values_at_nearest() {
        let mut s = 99u64;
        let mut e = Exact::new(0x1f80);
        for _ in 0..2000 {
            let a = ((rng(&mut s) as i32) as f32 / 4096.0).to_bits();
            let b = ((rng(&mut s) as i32) as f32 / 8192.0 + 0.5).to_bits();
            assert_eq!(Fast.add(a, b), e.add(a, b));
            assert_eq!(Fast.sub(a, b), e.sub(a, b));
            assert_eq!(Fast.mul(a, b), e.mul(a, b));
            assert_eq!(Fast.div(a, b), e.div(a, b));
        }
    }

    #[test]
    fn x87_double_matches_host_at_nearest() {
        let mut s = 5u64;
        for _ in 0..500 {
            let a = ((rng(&mut s) as i32) as f64 / 1024.0).to_bits();
            let b = ((rng(&mut s) as i32) as f64 / 777.0 + 0.25).to_bits();
            let (x, y) = (f64::from_bits(a), f64::from_bits(b));
            let mut u = X87::new(0x027f);
            assert_eq!(u.add(a, b), Some((x + y).to_bits()));
            assert_eq!(u.mul(a, b), Some((x * y).to_bits()));
            assert_eq!(u.sub(a, b), Some((x - y).to_bits()));
            assert_eq!(u.div(a, b), Some((x / y).to_bits()));
        }
    }
}
