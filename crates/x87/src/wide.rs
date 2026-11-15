//! A wide software float for the transcendentals: `N` 64-bit limbs of
//! significand (512 bits at the default `N = 8`) and an `i32` exponent.
//!
//! Every operation truncates (rounds toward zero) to `64*N` bits, so each
//! has a relative error below `2^(1-64N)`. Nothing here needs to be exact;
//! it only needs an error bound far below the final 64-bit rounding.
#![forbid(unsafe_code)]

use core::cmp::Ordering;

use crate::round::{round_pack, zero};
use crate::{F80, Flags, Rc};

pub(crate) const MAXN: usize = 20;

fn lz(m: &[u64]) -> u32 {
    for (i, &w) in m.iter().enumerate().rev() {
        if w != 0 {
            return (m.len() - 1 - i) as u32 * 64 + w.leading_zeros();
        }
    }
    m.len() as u32 * 64
}

fn shl(m: &mut [u64], s: u32) {
    let n = m.len();
    let ls = (s / 64) as usize;
    let bs = s % 64;
    for i in (0..n).rev() {
        let hi = if i >= ls { m[i - ls] << bs } else { 0 };
        let lo = if bs != 0 && i > ls {
            m[i - ls - 1] >> (64 - bs)
        } else {
            0
        };
        m[i] = hi | lo;
    }
}

fn shr(m: &mut [u64], s: u32) {
    let n = m.len();
    let ls = (s / 64) as usize;
    let bs = s % 64;
    for i in 0..n {
        let src = i.saturating_add(ls);
        let lo = if src < n { m[src] >> bs } else { 0 };
        let hi = if bs != 0 && src + 1 < n {
            m[src + 1] << (64 - bs)
        } else {
            0
        };
        m[i] = lo | hi;
    }
}

fn add_into(a: &mut [u64], b: &[u64]) -> bool {
    let mut carry = false;
    for (x, &y) in a.iter_mut().zip(b) {
        let (s1, c1) = x.overflowing_add(y);
        let (s2, c2) = s1.overflowing_add(carry as u64);
        *x = s2;
        carry = c1 || c2;
    }
    carry
}

fn sub_into(a: &mut [u64], b: &[u64]) -> bool {
    let mut borrow = false;
    for (x, &y) in a.iter_mut().zip(b) {
        let (s1, b1) = x.overflowing_sub(y);
        let (s2, b2) = s1.overflowing_sub(borrow as u64);
        *x = s2;
        borrow = b1 || b2;
    }
    borrow
}

fn cmp(a: &[u64], b: &[u64]) -> Ordering {
    for i in (0..a.len()).rev() {
        match a[i].cmp(&b[i]) {
            Ordering::Equal => {}
            o => return o,
        }
    }
    Ordering::Equal
}

/// `value = (-1)^neg * m * 2^exp`, `m` little-endian limbs, normalised so
/// the top bit of `m[N-1]` is set, or all zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Wf<const N: usize> {
    pub neg: bool,
    pub exp: i32,
    pub m: [u64; N],
}

impl<const N: usize> Wf<N> {
    pub const ZERO: Self = Wf {
        neg: false,
        exp: 0,
        m: [0; N],
    };

    const BITS: i32 = 64 * N as i32;

    pub fn is_zero(&self) -> bool {
        self.m.iter().all(|&w| w == 0)
    }

    /// `(-1)^neg * sig * 2^exp`.
    pub fn from_parts(neg: bool, sig: u64, exp: i32) -> Self {
        let mut m = [0u64; N];
        m[N - 1] = sig;
        let mut w = Wf {
            neg,
            exp: exp - 64 * (N as i32 - 1),
            m,
        };
        w.norm();
        w
    }

    pub fn from_u64(v: u64) -> Self {
        Self::from_parts(false, v, 0)
    }

    pub fn from_i64(v: i64) -> Self {
        Self::from_parts(v < 0, v.unsigned_abs(), 0)
    }

    pub fn one() -> Self {
        Self::from_u64(1)
    }

    /// From big-endian limbs (most significant first) whose leading bit has
    /// exponent `top`; only the first `N` limbs are used (truncation).
    pub fn from_be_limbs(limbs: &[u64], top: i32) -> Self {
        let mut m = [0u64; N];
        for (i, slot) in m.iter_mut().rev().enumerate() {
            *slot = limbs.get(i).copied().unwrap_or(0);
        }
        let mut w = Wf {
            neg: false,
            exp: top - (Self::BITS - 1),
            m,
        };
        w.norm();
        w
    }

    fn norm(&mut self) {
        let z = lz(&self.m);
        if z as i32 == Self::BITS {
            self.exp = 0;
            return;
        }
        shl(&mut self.m, z);
        self.exp -= z as i32;
    }

    /// Exponent of the leading bit. Only meaningful when nonzero.
    pub fn top(&self) -> i32 {
        self.exp + Self::BITS - 1
    }

    pub fn neg(self) -> Self {
        Wf {
            neg: !self.neg,
            ..self
        }
    }

    /// Multiply by `2^k` exactly.
    pub fn scale(self, k: i32) -> Self {
        if self.is_zero() {
            self
        } else {
            Wf {
                exp: self.exp + k,
                ..self
            }
        }
    }

    pub fn cmp_abs(&self, o: &Self) -> Ordering {
        match (self.is_zero(), o.is_zero()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => self.exp.cmp(&o.exp).then_with(|| cmp(&self.m, &o.m)),
        }
    }

    pub fn add(self, o: Self) -> Self {
        if self.is_zero() {
            return o;
        }
        if o.is_zero() {
            return self;
        }
        let (big, small) = if self.cmp_abs(&o) != Ordering::Less {
            (self, o)
        } else {
            (o, self)
        };
        let d = (big.exp as i64 - small.exp as i64).min(u32::MAX as i64) as u32;
        // One guard limb below the significand.
        let mut wb = [0u64; MAXN + 1];
        let mut ws = [0u64; MAXN + 1];
        wb[1..=N].copy_from_slice(&big.m);
        ws[1..=N].copy_from_slice(&small.m);
        let wb = &mut wb[..=N];
        let ws = &mut ws[..=N];
        shr(ws, d);
        let mut exp = big.exp - 64;
        if big.neg == small.neg {
            if add_into(wb, ws) {
                shr(wb, 1);
                wb[N] |= 1 << 63;
                exp += 1;
            }
        } else {
            sub_into(wb, ws);
        }
        let z = lz(wb);
        if z as usize == 64 * (N + 1) {
            return Self::ZERO;
        }
        shl(wb, z);
        let mut m = [0u64; N];
        m.copy_from_slice(&wb[1..=N]);
        Wf {
            neg: big.neg,
            exp: exp - z as i32 + 64,
            m,
        }
    }

    pub fn sub(self, o: Self) -> Self {
        self.add(o.neg())
    }

    pub fn mul(self, o: Self) -> Self {
        if self.is_zero() || o.is_zero() {
            return Self::ZERO;
        }
        let mut p = [0u64; 2 * MAXN];
        for i in 0..N {
            let mut carry: u128 = 0;
            for j in 0..N {
                let t = self.m[i] as u128 * o.m[j] as u128 + p[i + j] as u128 + carry;
                p[i + j] = t as u64;
                carry = t >> 64;
            }
            p[i + N] = carry as u64;
        }
        let p = &mut p[..2 * N];
        let z = lz(p);
        shl(p, z);
        let mut m = [0u64; N];
        m.copy_from_slice(&p[N..]);
        Wf {
            neg: self.neg ^ o.neg,
            exp: self.exp + o.exp + Self::BITS - z as i32,
            m,
        }
    }

    pub fn mul_u64(self, k: u64) -> Self {
        self.mul(Self::from_u64(k))
    }

    /// Restoring binary long division, `64N + 1` quotient bits.
    pub fn div(self, o: Self) -> Self {
        assert!(!o.is_zero(), "wide division by zero");
        if self.is_zero() {
            return Self::ZERO;
        }
        let mut r = [0u64; MAXN + 1];
        let mut d = [0u64; MAXN + 1];
        let mut q = [0u64; MAXN + 1];
        r[..N].copy_from_slice(&self.m);
        d[..N].copy_from_slice(&o.m);
        let (r, d, q) = (&mut r[..=N], &d[..=N], &mut q[..=N]);
        for _ in 0..(64 * N + 1) {
            shl(q, 1);
            if cmp(r, d) != Ordering::Less {
                sub_into(r, d);
                q[0] |= 1;
            }
            shl(r, 1);
        }
        let z = lz(q);
        shl(q, z);
        let mut m = [0u64; N];
        m.copy_from_slice(&q[1..]);
        Wf {
            neg: self.neg ^ o.neg,
            exp: self.exp - o.exp - Self::BITS - z as i32 + 64,
            m,
        }
    }

    /// Divide by a small integer, keeping a full-width quotient.
    pub fn div_u64(self, k: u64) -> Self {
        debug_assert!(k != 0);
        if self.is_zero() {
            return self;
        }
        let mut w = [0u64; MAXN + 1];
        w[1..=N].copy_from_slice(&self.m);
        let w = &mut w[..=N];
        let mut rem: u128 = 0;
        for limb in w.iter_mut().rev() {
            let cur = (rem << 64) | *limb as u128;
            *limb = (cur / k as u128) as u64;
            rem = cur % k as u128;
        }
        let z = lz(w);
        shl(w, z);
        let mut m = [0u64; N];
        m.copy_from_slice(&w[1..]);
        Wf {
            neg: self.neg,
            exp: self.exp - z as i32,
            m,
        }
    }

    /// Square root of a positive value, digit by digit, `64N` root bits.
    pub fn sqrt(self) -> Self {
        assert!(!self.neg || self.is_zero(), "wide sqrt of a negative");
        if self.is_zero() {
            return self;
        }
        let mut rad = [0u64; 2 * MAXN];
        rad[N..2 * N].copy_from_slice(&self.m);
        let rad = &mut rad[..2 * N];
        let mut e = self.exp - Self::BITS;
        if e & 1 != 0 {
            shr(rad, 1);
            e += 1;
        }
        let mut rem = [0u64; MAXN + 1];
        let mut root = [0u64; MAXN + 1];
        let mut trial = [0u64; MAXN + 1];
        let (rem, root, trial) = (&mut rem[..=N], &mut root[..=N], &mut trial[..=N]);
        for i in (0..(64 * N)).rev() {
            let bit = 2 * i;
            let two = (rad[bit / 64] >> (bit % 64)) & 3;
            shl(rem, 2);
            rem[0] |= two;
            trial.copy_from_slice(root);
            shl(trial, 2);
            trial[0] |= 1;
            shl(root, 1);
            if cmp(rem, trial) != Ordering::Less {
                sub_into(rem, trial);
                root[0] |= 1;
            }
        }
        let mut m = [0u64; N];
        m.copy_from_slice(&root[..N]);
        let mut w = Wf {
            neg: false,
            exp: e / 2,
            m,
        };
        w.norm();
        w
    }

    /// Round to nearest at 64 bits. `tail` describes the exact value
    /// relative to this one when this one's bits below the top 128 are all
    /// zero: +1 slightly larger in magnitude, -1 slightly smaller, 0 exact.
    /// Otherwise the low bits act as the sticky bit.
    pub fn to_f80(self, tail: i8) -> (F80, Flags) {
        if self.is_zero() {
            return (zero(self.neg), Flags::NONE);
        }
        let hi = ((self.m[N - 1] as u128) << 64) | self.m[N - 2] as u128;
        let low_nonzero = self.m[..N - 2].iter().any(|&w| w != 0);
        let t = if low_nonzero { 1 } else { tail };
        round_pack(
            self.neg,
            hi,
            self.exp as i64 + 64 * (N as i64 - 2),
            t,
            64,
            Rc::Near,
        )
    }

    /// Split a nonnegative value below 2^63 into integer and fractional parts.
    pub fn int_frac(self) -> (u64, Self) {
        debug_assert!(!self.neg);
        if self.is_zero() || self.top() < 0 {
            return (0, self);
        }
        debug_assert!(self.top() < 64);
        let sh = (-self.exp) as u32;
        let mut ip = self.m;
        shr(&mut ip, sh);
        let mut fm = self.m;
        let li = (sh / 64) as usize;
        let lb = sh % 64;
        if li < N {
            fm[li] &= (1u64 << lb).wrapping_sub(1);
            for w in fm.iter_mut().skip(li + 1) {
                *w = 0;
            }
        }
        let mut f = Wf {
            neg: false,
            exp: self.exp,
            m: fm,
        };
        f.norm();
        (ip[0], f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type W = Wf<4>;

    #[test]
    fn basic_ops() {
        let three = W::from_u64(3);
        let seven = W::from_u64(7);
        assert_eq!(three.add(seven), W::from_u64(10));
        assert_eq!(three.sub(seven), W::from_i64(-4));
        assert_eq!(three.mul(seven), W::from_u64(21));
        assert_eq!(W::from_u64(21).div(seven), three);
        assert_eq!(W::from_u64(21).div_u64(3), seven);
        assert_eq!(W::from_u64(49).sqrt(), seven);
        assert_eq!(W::from_u64(4).scale(-4).sqrt(), W::from_u64(1).scale(-1));
        assert!(three.sub(three).is_zero());
        // 1/3 * 3 is one ulp short of 1 after truncation.
        let third = W::one().div(three);
        let back = third.mul(three);
        assert_eq!(back.top(), -1);
        assert!(back.m.iter().all(|&w| w == !0));
        let (i, f) = W::from_u64(10).add(third).int_frac();
        assert_eq!(i, 10);
        // The low bits of 1/3 that did not fit beside the 10 are gone.
        assert!(f.sub(third).top() < third.top() - 250);
    }
}
