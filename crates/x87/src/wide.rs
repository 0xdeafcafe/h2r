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
        if N == 2 {
            return self.add2(o);
        }
        self.add_limbs(o)
    }

    /// [`add`](Self::add) on limbs, for every `N`.
    fn add_limbs(self, o: Self) -> Self {
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
        // One guard limb below the significand. `[[u64; N]; 2]` holds the
        // N + 1 limbs without zeroing MAXN-sized scratch on every call.
        let mut wb = [[0u64; N]; 2];
        let mut ws = [[0u64; N]; 2];
        let wb = &mut wb.as_flattened_mut()[..=N];
        let ws = &mut ws.as_flattened_mut()[..=N];
        wb[1..=N].copy_from_slice(&big.m);
        ws[1..=N].copy_from_slice(&small.m);
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
        if N == 2 {
            return self.mul2(o);
        }
        self.mul_limbs(o)
    }

    /// [`mul`](Self::mul) on limbs, for every `N`.
    fn mul_limbs(self, o: Self) -> Self {
        if self.is_zero() || o.is_zero() {
            return Self::ZERO;
        }
        let mut p = [[0u64; N]; 2];
        let p = p.as_flattened_mut();
        for i in 0..N {
            let mut carry: u128 = 0;
            for j in 0..N {
                let t = self.m[i] as u128 * o.m[j] as u128 + p[i + j] as u128 + carry;
                p[i + j] = t as u64;
                carry = t >> 64;
            }
            p[i + N] = carry as u64;
        }
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

    /// Truncated division: the top `64N` bits of `floor(a * 2^(64N) / d)`
    /// for significands `a`, `d` (Knuth's algorithm D on 32-bit digits, so
    /// every step is a native 64/64 division). Bit-identical to the
    /// restoring long division it replaced (`tests::div_restoring`).
    pub fn div(self, o: Self) -> Self {
        assert!(!o.is_zero(), "wide division by zero");
        if self.is_zero() {
            return Self::ZERO;
        }
        let n = 2 * N; // divisor digits; the dividend has 2n, plus a zero
        let mut u = [[0u32; N]; 5];
        let mut v = [[0u32; N]; 2];
        let mut qd = [[0u32; N]; 3];
        let (u, v, qd) = (
            &mut u.as_flattened_mut()[..=2 * n],
            v.as_flattened_mut(),
            &mut qd.as_flattened_mut()[..=n],
        );
        for i in 0..N {
            u[n + 2 * i] = self.m[i] as u32;
            u[n + 2 * i + 1] = (self.m[i] >> 32) as u32;
            v[2 * i] = o.m[i] as u32;
            v[2 * i + 1] = (o.m[i] >> 32) as u32;
        }
        // v is normalised (top bit set), so no shift is needed.
        const B: u64 = 1 << 32;
        let vt = v[n - 1] as u64;
        let vs = v[n - 2] as u64;
        for j in (0..=n).rev() {
            let num = ((u[j + n] as u64) << 32) | u[j + n - 1] as u64;
            let mut qhat = num / vt;
            let mut rhat = num - qhat * vt;
            while qhat >= B || qhat * vs > ((rhat << 32) | u[j + n - 2] as u64) {
                qhat -= 1;
                rhat += vt;
                if rhat >= B {
                    break;
                }
            }
            let mut k: i64 = 0;
            for i in 0..n {
                let p = qhat * v[i] as u64;
                let t = u[i + j] as i64 - k - (p & 0xffff_ffff) as i64;
                u[i + j] = t as u32;
                k = (p >> 32) as i64 - (t >> 32);
            }
            let t = u[j + n] as i64 - k;
            u[j + n] = t as u32;
            if t < 0 {
                // qhat was one too large: add the divisor back.
                qhat -= 1;
                let mut c = 0u64;
                for i in 0..n {
                    let t = u[i + j] as u64 + v[i] as u64 + c;
                    u[i + j] = t as u32;
                    c = t >> 32;
                }
                u[j + n] = u[j + n].wrapping_add(c as u32);
            }
            qd[j] = qhat as u32;
        }
        let mut q = [[0u64; N]; 2];
        let q = &mut q.as_flattened_mut()[..=N];
        for i in 0..N {
            q[i] = qd[2 * i] as u64 | (qd[2 * i + 1] as u64) << 32;
        }
        q[N] = qd[n] as u64;
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
        if N == 2 {
            return self.div_u64_2(k);
        }
        self.div_u64_limbs(k)
    }

    /// [`div_u64`](Self::div_u64) on limbs, for every `N`.
    fn div_u64_limbs(self, k: u64) -> Self {
        debug_assert!(k != 0);
        if self.is_zero() {
            return self;
        }
        let mut w = [[0u64; N]; 2];
        let w = &mut w.as_flattened_mut()[..=N];
        w[1..=N].copy_from_slice(&self.m);
        if k <= u32::MAX as u64 {
            // Two native divisions per limb: rem < k < 2^32 keeps every
            // partial dividend below 2^64. Same quotient as the u128 form.
            let mut rem = 0u64;
            for limb in w.iter_mut().rev() {
                let hi = (rem << 32) | (*limb >> 32);
                let (qh, r) = (hi / k, hi % k);
                let lo = (r << 32) | (*limb & 0xffff_ffff);
                *limb = (qh << 32) | (lo / k);
                rem = lo % k;
            }
        } else {
            let mut rem: u128 = 0;
            for limb in w.iter_mut().rev() {
                let cur = (rem << 64) | *limb as u128;
                *limb = (cur / k as u128) as u64;
                rem = cur % k as u128;
            }
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

    // ---- N = 2 (the fast transcendentals' width) on native u128 ----
    //
    // The same operations as the limb loops, step for step: the same guard
    // limb, the same truncations, the same normalisation. Bit-identical to
    // them (`tests::two_limb_paths_match_the_limb_loops`).

    fn sig2(&self) -> u128 {
        ((self.m[1] as u128) << 64) | self.m[0] as u128
    }

    fn from_sig2(neg: bool, exp: i32, sig: u128) -> Self {
        let mut m = [0u64; N];
        m[0] = sig as u64;
        m[1] = (sig >> 64) as u64;
        Wf { neg, exp, m }
    }

    fn add2(self, o: Self) -> Self {
        let (a, b) = (self.sig2(), o.sig2());
        if a == 0 {
            return o;
        }
        if b == 0 {
            return self;
        }
        let self_big = match self.exp.cmp(&o.exp) {
            Ordering::Equal => a >= b,
            c => c == Ordering::Greater,
        };
        let (big, bs, small, ss) = if self_big {
            (self, a, o, b)
        } else {
            (o, b, self, a)
        };
        let d = (big.exp as i64 - small.exp as i64).min(u32::MAX as i64) as u32;
        // The smaller significand above a 64-bit guard limb, shifted right
        // by `d` with the bits below the guard limb dropped.
        let (sh, sl): (u128, u64) = match d {
            0 => (ss, 0),
            1..=63 => (ss >> d, (ss << (64 - d)) as u64),
            64..=127 => (ss >> d, (ss >> (d - 64)) as u64),
            128..=191 => (0, (ss >> (d - 64)) as u64),
            _ => (0, 0),
        };
        if big.neg == small.neg {
            let (hi, carry) = bs.overflowing_add(sh);
            if carry {
                // 193 bits: shift right one, the carry becomes the top bit.
                return Self::from_sig2(big.neg, big.exp + 1, (hi >> 1) | 1 << 127);
            }
            // The big significand was normalised: no shift.
            return Self::from_sig2(big.neg, big.exp, hi);
        }
        let (lo, borrow) = 0u64.overflowing_sub(sl);
        let hi = bs - sh - borrow as u128;
        if hi == 0 && lo == 0 {
            return Self::ZERO;
        }
        let z = if hi != 0 {
            hi.leading_zeros()
        } else {
            128 + lo.leading_zeros()
        };
        // Shift the 192-bit (hi, lo) left by z and keep the top 128 bits.
        let top = match z {
            0 => hi,
            1..=63 => (hi << z) | ((lo as u128) >> (64 - z)),
            64..=127 => (hi << z) | ((lo as u128) << (z - 64)),
            _ => (lo as u128) << (z - 64),
        };
        Self::from_sig2(big.neg, big.exp - z as i32, top)
    }

    fn mul2(self, o: Self) -> Self {
        let (a, b) = (self.sig2(), o.sig2());
        if a == 0 || b == 0 {
            return Self::ZERO;
        }
        let (a1, a0) = (a >> 64, a & u64::MAX as u128);
        let (b1, b0) = (b >> 64, b & u64::MAX as u128);
        let ll = a0 * b0;
        let lh = a0 * b1;
        let hl = a1 * b0;
        let hh = a1 * b1;
        // 256-bit product (hi, lo).
        let (mid, c1) = lh.overflowing_add(hl);
        let (lo, c2) = ll.overflowing_add(mid << 64);
        let hi = hh + (mid >> 64) + ((c1 as u128) << 64) + c2 as u128;
        // Both significands are normalised: the product's top bit is at
        // 255 or 254.
        let z = hi.leading_zeros();
        let top = if z == 0 { hi } else { (hi << 1) | (lo >> 127) };
        Self::from_sig2(self.neg ^ o.neg, self.exp + o.exp + 128 - z as i32, top)
    }

    fn div_u64_2(self, k: u64) -> Self {
        debug_assert!(k != 0);
        let a = self.sig2();
        if a == 0 {
            return self;
        }
        if k.is_power_of_two() {
            return Self::from_sig2(self.neg, self.exp - k.trailing_zeros() as i32, a);
        }
        if k <= u16::MAX as u64 {
            return self.div_u16_2(a, k);
        }
        // floor((a << 64) / k): a 192-bit quotient (qh, ql).
        let (qh, ql) = if k <= u32::MAX as u64 {
            // Native 64/64 divisions on 32-bit digits, as the limb loop.
            let mut rem = 0u64;
            let mut q = [0u64; 3];
            let w = [0u64, a as u64, (a >> 64) as u64];
            for i in (0..3).rev() {
                let hi = (rem << 32) | (w[i] >> 32);
                let (qh, r) = (hi / k, hi % k);
                let lo = (r << 32) | (w[i] & 0xffff_ffff);
                q[i] = (qh << 32) | (lo / k);
                rem = lo % k;
            }
            (((q[2] as u128) << 64) | q[1] as u128, q[0])
        } else {
            let kk = k as u128;
            let qh = a / kk;
            let r = a % kk;
            (qh, ((r << 64) / kk) as u64)
        };
        let z = if qh != 0 {
            qh.leading_zeros()
        } else {
            128 + ql.leading_zeros()
        };
        let top = match z {
            0 => qh,
            1..=63 => (qh << z) | ((ql as u128) >> (64 - z)),
            64..=127 => (qh << z) | ((ql as u128) << (z - 64)),
            _ => (ql as u128) << (z - 64),
        };
        Self::from_sig2(self.neg, self.exp - z as i32, top)
    }

    /// The series mostly divide by 16-bit constants. Divide three 48-bit
    /// digits of `a << 16`; those 16 appended bits suffice because a
    /// normalized significand shifts by at most 16 after this division.
    fn div_u16_2(self, a: u128, k: u64) -> Self {
        const DIGIT_MASK: u64 = (1 << 48) - 1;
        // With d < 2^64, floor(d * floor((2^64 - 1) / k) / 2^64)
        // underestimates floor(d / k) by at most one.
        let reciprocal = (u64::MAX / k) as u128;
        let digit = |d: u64| {
            let q = ((d as u128 * reciprocal) >> 64) as u64;
            let remainder = d - q * k;
            let correction = u64::from(remainder >= k);
            (q + correction, remainder - correction * k)
        };
        let (high, rem) = digit((a >> 80) as u64);
        let (middle, rem) = digit((rem << 48) | ((a >> 32) as u64 & DIGIT_MASK));
        let (low, _) = digit((rem << 48) | ((a as u32 as u64) << 16));

        // high is nonzero and occupies at most 48 bits. Drop the unused
        // guard bits to recover exactly the top 128 bits of the quotient.
        let shift = 32 - high.leading_zeros();
        let sig = ((high as u128) << (96 - shift))
            | ((middle as u128) << (48 - shift))
            | (low >> shift) as u128;
        Self::from_sig2(self.neg, self.exp - (16 - shift) as i32, sig)
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

    /// A small xorshift, for operand sweeps.
    fn rng(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn wf2(state: &mut u64) -> Wf<2> {
        let r = rng(state);
        if r.is_multiple_of(61) {
            return Wf::ZERO;
        }
        // Mostly full significands, sometimes sparse ones (equal limbs,
        // single bits) to reach the carry, borrow and cancellation edges.
        let (hi, lo) = match r % 7 {
            0 => (1u64 << 63, 0),
            1 => (u64::MAX, u64::MAX),
            2 => (1u64 << 63, rng(state) & 0xff),
            _ => (rng(state) | 1 << 63, rng(state)),
        };
        let exp = (rng(state) % 400) as i32 - 200;
        Wf {
            neg: rng(state) & 1 == 1,
            exp,
            m: [lo, hi],
        }
    }

    #[test]
    fn two_limb_paths_match_the_limb_loops() {
        let mut s = 0x9e37_79b9_7f4a_7c15u64;
        for i in 0..400_000 {
            let a = wf2(&mut s);
            let mut b = wf2(&mut s);
            // Close exponents and near-equal values for cancellation.
            if i % 3 == 0 {
                b.exp = a.exp + (rng(&mut s) % 5) as i32 - 2;
            }
            if i % 11 == 0 {
                b = Wf { neg: !a.neg, ..a };
                b.m[0] ^= rng(&mut s) & 0xffff;
            }
            assert_eq!(a.add2(b), a.add_limbs(b), "add {a:?} {b:?}");
            assert_eq!(a.mul2(b), a.mul_limbs(b), "mul {a:?} {b:?}");
            let k = match rng(&mut s) % 4 {
                0 => rng(&mut s) % 64 + 1,
                1 => rng(&mut s) & 0xffff_ffff | 1,
                2 => rng(&mut s) | 1,
                _ => u64::MAX - (rng(&mut s) % 3),
            };
            assert_eq!(a.div_u64_2(k), a.div_u64_limbs(k), "div_u64 {a:?} {k}");
        }
    }

    type W = Wf<4>;

    /// The original restoring long division, `64N + 1` quotient bits.
    pub(crate) fn div_restoring<const N: usize>(a: Wf<N>, o: Wf<N>) -> Wf<N> {
        if a.is_zero() {
            return Wf::ZERO;
        }
        let mut r = [0u64; MAXN + 1];
        let mut d = [0u64; MAXN + 1];
        let mut q = [0u64; MAXN + 1];
        r[..N].copy_from_slice(&a.m);
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
            neg: a.neg ^ o.neg,
            exp: a.exp - o.exp - Wf::<N>::BITS - z as i32 + 64,
            m,
        }
    }

    /// The original u128 small-integer division.
    pub(crate) fn div_u64_u128<const N: usize>(a: Wf<N>, k: u64) -> Wf<N> {
        if a.is_zero() {
            return a;
        }
        let mut w = [0u64; MAXN + 1];
        w[1..=N].copy_from_slice(&a.m);
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
            neg: a.neg,
            exp: a.exp - z as i32,
            m,
        }
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        /// A significand stressing Knuth D: random, sparse, saturated, or a
        /// near neighbour of `like` (quotients near 1 and qhat corrections).
        fn sig<const N: usize>(&mut self, like: Option<&Wf<N>>) -> [u64; N] {
            let mut m = [0u64; N];
            let kind = self.next() % 8;
            for w in m.iter_mut() {
                *w = match kind {
                    0 => !0,
                    1 => 0,
                    2 => self.next() & self.next() & self.next(),
                    3 => !(self.next() & self.next()),
                    _ => self.next(),
                };
            }
            if let (Some(l), true) = (like, kind >= 6) {
                m = l.m;
                let bit = self.next() % (64 * N as u64);
                m[(bit / 64) as usize] ^= 1 << (bit % 64);
                if kind == 7 {
                    m[0] = m[0].wrapping_add(self.next() % 3).wrapping_sub(1);
                }
            }
            m[N - 1] |= 1 << 63;
            m
        }
        fn wf<const N: usize>(&mut self, like: Option<&Wf<N>>) -> Wf<N> {
            Wf {
                neg: self.next() & 1 == 1,
                exp: (self.next() % 512) as i32 - 256,
                m: self.sig(like),
            }
        }
    }

    fn ops_match<const N: usize>(cases: usize, seed: u64) {
        let mut rng = Rng(seed);
        for _ in 0..cases {
            let a = rng.wf::<N>(None);
            let b = rng.wf::<N>(Some(&a));
            assert_eq!(a.div(b), div_restoring(a, b), "div {a:?} / {b:?}");
            let k = match rng.next() % 4 {
                0 => 1 + rng.next() % 300,
                1 => 1 + (rng.next() & 0xffff_ffff),
                2 => u32::MAX as u64 - rng.next() % 3,
                _ => (1 << 32) + rng.next() % (1 << 40),
            };
            assert_eq!(a.div_u64(k), div_u64_u128(a, k), "div_u64 {a:?} / {k}");
        }
    }

    #[test]
    fn reciprocal_digits_match_long_division_at_all_small_divisors() {
        let mut rng = Rng(0x8834_951b_716d_b18f);
        for k in 1..=(u16::MAX as u64 + 1) {
            for a in [
                Wf::<2>::from_sig2(false, 0, 1u128 << 127),
                Wf::<2>::from_sig2(true, -17, u128::MAX),
                rng.wf::<2>(None),
            ] {
                assert_eq!(a.div_u64(k), div_u64_u128(a, k), "{a:?} / {k}");
            }
        }
    }

    fn bench_ops<const N: usize>() {
        use std::{hint::black_box, time::Instant, vec::Vec};
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        let xs: Vec<_> = (0..256).map(|_| rng.wf::<N>(None)).collect();
        let time = |f: &dyn Fn(Wf<N>, Wf<N>) -> Wf<N>| {
            let mut best = f64::MAX;
            for _ in 0..400 {
                let start = Instant::now();
                for w in xs.windows(2) {
                    black_box(f(black_box(w[0]), black_box(w[1])));
                }
                best = best.min(start.elapsed().as_nanos() as f64 / 255.0);
            }
            best
        };
        let rows: [(&str, f64, f64); 2] = [
            (
                "div",
                time(&|a, b| div_restoring(a, b)),
                time(&|a, b| a.div(b)),
            ),
            (
                "div_u64",
                time(&|a, b| div_u64_u128(a, 3 + (b.m[0] & 0xff))),
                time(&|a, b| a.div_u64(3 + (b.m[0] & 0xff))),
            ),
        ];
        for (name, old, new) in rows {
            std::println!(
                "WIDE_BENCH N={N} {name} old_ns={old:.1} new_ns={new:.1} speedup={:.1}x",
                old / new
            );
        }
    }

    /// Per-op timing of the rewritten operations against the originals
    /// (minimum of 400 passes over 255 random operand pairs).
    #[test]
    #[ignore = "timing experiment; run serially in release mode"]
    fn benchmark_wide_ops() {
        bench_ops::<2>();
        bench_ops::<3>();
        bench_ops::<8>();
    }

    /// Every rewritten operation against the original algorithm: random,
    /// sparse, saturated and near-equal significands at several widths.
    /// `H2_X87_WIDE_CASES` raises the count (default 200,000 per width).
    #[test]
    fn rewritten_division_matches_the_original_bit_for_bit() {
        let cases = std::env::var("H2_X87_WIDE_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(200_000);
        ops_match::<1>(cases, 0x9e37_79b9_7f4a_7c15);
        ops_match::<2>(cases, 0xd1b5_4a32_d192_ed03);
        ops_match::<3>(cases, 0x8cb9_2ba7_2f3d_8dd7);
        ops_match::<8>(cases / 10, 0xa076_1d64_78bd_642f);
        ops_match::<12>(cases / 50, 0xe703_7ed1_a0b4_28db);
    }

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
