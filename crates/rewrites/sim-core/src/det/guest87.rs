//! x87 sequences under the guest's control word, for any operand.
//!
//! A value the host holds exactly stays a host `f64` and takes the host's
//! operation when the result is clear of the format's edges; inexactness
//! comes from an error-free transform, not the host's status word. Every
//! other value or operation (NaN, infinity, a subnormal operand, a result
//! near overflow or underflow, a division by zero, extended precision, a
//! rounding mode other than nearest) runs on the `x87` crate's model, which
//! rounds as the control word says and raises the flags the instruction
//! does. The caller checks `super::x87_env` first, so every exception is
//! masked and the model's masked responses are the guest's results.
use std::cell::Cell;
use std::cmp::Ordering;
use x87::{Cmp, F80, Mode, Rc};

/// One x87 stack value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct St(Repr);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Repr {
    /// Exactly the register's value: zero, or a magnitude in the fast range
    /// of the working precision, representable in it.
    Host(f64),
    Wide(F80),
}

impl St {
    /// Whether the value left the host's fast path.
    pub fn is_wide(self) -> bool {
        matches!(self.0, Repr::Wide(_))
    }

    fn wide(self) -> F80 {
        match self.0 {
            Repr::Host(v) => F80::from_f64_bits(v.to_bits()),
            Repr::Wide(w) => w,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pc {
    Single,
    Double,
    Model,
}

#[derive(Clone, Copy)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

/// The x87 under one control word, gathering the exception flags its
/// operations raise.
#[derive(Debug)]
pub struct X87 {
    control: u16,
    flags: Cell<u16>,
}

// The fast range: products, sums and their error terms stay normal.
const LOW32: f64 = f64::from_bits((1023 - 100) << 52);
const HIGH32: f64 = f64::from_bits((1023 + 100) << 52);
const LOW64: f64 = f64::from_bits((1023 - 900) << 52);
const HIGH64: f64 = f64::from_bits((1023 + 900) << 52);

impl X87 {
    pub fn new(control: u16) -> Self {
        Self {
            control,
            flags: Cell::new(0),
        }
    }

    pub fn control(&self) -> u16 {
        self.control
    }

    /// IE, DE, ZE, OE, UE and PE as the status word gains them.
    pub fn flags(&self) -> u16 {
        self.flags.get()
    }

    fn raise(&self, f: x87::Flags) {
        self.flags.set(self.flags.get() | (f.0 & 0x3f));
    }

    fn mode(&self) -> Mode {
        Mode::from_cw(self.control)
    }

    fn pc(&self) -> Pc {
        if Rc::from_cw(self.control) != Rc::Near {
            return Pc::Model;
        }
        match (self.control >> 8) & 3 {
            0 => Pc::Single,
            2 => Pc::Double,
            _ => Pc::Model,
        }
    }

    fn fits(&self, v: f64) -> bool {
        let m = v.abs();
        match self.pc() {
            Pc::Single => v == 0.0 || ((LOW32..HIGH32).contains(&m) && f64::from(v as f32) == v),
            Pc::Double => v == 0.0 || (LOW64..HIGH64).contains(&m),
            Pc::Model => false,
        }
    }

    /// A model result, back on the host when it fits exactly.
    fn settle(&self, w: F80) -> St {
        let (bits, f) = w.to_f64_bits(Mode::EXTENDED);
        let v = f64::from_bits(bits);
        if f.0 & 0x3f == 0 && v.is_finite() && self.fits(v) {
            St(Repr::Host(v))
        } else {
            St(Repr::Wide(w))
        }
    }

    /// `fld m32`: an SNaN is quieted with IE, a subnormal raises DE.
    pub fn load32(&self, bits: u32) -> St {
        let (exp, mant) = ((bits >> 23) & 0xff, bits & 0x7f_ffff);
        if exp == 0 && mant != 0 {
            self.raise(x87::Flags::DE);
        }
        let v = f64::from(f32::from_bits(bits));
        if v.is_finite() && self.fits(v) {
            return St(Repr::Host(v));
        }
        let mut w = F80::from_f32_bits(bits);
        if exp == 0xff && mant != 0 && mant & 0x40_0000 == 0 {
            self.raise(x87::Flags::IE);
            w.sig |= 1 << 62;
        }
        St(Repr::Wide(w))
    }

    /// `fild`: exact in every precision control.
    pub fn int(&self, v: i64) -> St {
        let f = v as f64;
        if f as i64 == v && self.fits(f) {
            St(Repr::Host(f))
        } else {
            St(Repr::Wide(F80::from_i64(v)))
        }
    }

    pub fn add(&self, a: St, b: St) -> St {
        self.op(Op::Add, a, b)
    }
    pub fn sub(&self, a: St, b: St) -> St {
        self.op(Op::Sub, a, b)
    }
    pub fn mul(&self, a: St, b: St) -> St {
        self.op(Op::Mul, a, b)
    }
    pub fn div(&self, a: St, b: St) -> St {
        self.op(Op::Div, a, b)
    }

    fn op(&self, op: Op, a: St, b: St) -> St {
        if let (Repr::Host(x), Repr::Host(y)) = (a.0, b.0)
            && let Some((r, inexact)) = self.fast(op, x, y)
        {
            if inexact {
                self.raise(x87::Flags::PE);
            }
            return St(Repr::Host(r));
        }
        let (a, b, m) = (a.wide(), b.wide(), self.mode());
        let (r, f) = match op {
            Op::Add => x87::add(a, b, m),
            Op::Sub => x87::sub(a, b, m),
            Op::Mul => x87::mul(a, b, m),
            Op::Div => x87::div(a, b, m),
        };
        self.raise(f);
        self.settle(r)
    }

    /// The host's result and whether it was inexact, when it is the
    /// guest's.
    fn fast(&self, op: Op, x: f64, y: f64) -> Option<(f64, bool)> {
        match self.pc() {
            Pc::Single => {
                let (a, b) = (x as f32, y as f32);
                let r = match op {
                    Op::Add => a + b,
                    Op::Sub => a - b,
                    Op::Mul => a * b,
                    Op::Div => a / b,
                };
                let r64 = f64::from(r);
                let zero_ok = match op {
                    Op::Add | Op::Sub => true,
                    Op::Mul => a == 0.0 || b == 0.0,
                    Op::Div => a == 0.0 && b != 0.0,
                };
                if !(if r == 0.0 { zero_ok } else { self.fits(r64) }) {
                    return None;
                }
                let inexact = match op {
                    Op::Add => two_sum32(a, b, r),
                    Op::Sub => two_sum32(a, -b, r),
                    Op::Mul => f64::from(a) * f64::from(b) != r64,
                    Op::Div => r64 * f64::from(b) != f64::from(a),
                };
                Some((r64, inexact))
            }
            Pc::Double => {
                let r = match op {
                    Op::Add => x + y,
                    Op::Sub => x - y,
                    Op::Mul => x * y,
                    Op::Div => x / y,
                };
                let zero_ok = match op {
                    Op::Add | Op::Sub => true,
                    Op::Mul => x == 0.0 || y == 0.0,
                    Op::Div => x == 0.0 && y != 0.0,
                };
                if !(if r == 0.0 { zero_ok } else { self.fits(r) }) {
                    return None;
                }
                let inexact = match op {
                    Op::Add => two_sum64(x, y, r),
                    Op::Sub => two_sum64(x, -y, r),
                    Op::Mul => x.mul_add(y, -r) != 0.0,
                    Op::Div => r.mul_add(y, -x) != 0.0,
                };
                Some((r, inexact))
            }
            Pc::Model => None,
        }
    }

    /// `fsqrt`.
    pub fn sqrt(&self, a: St) -> St {
        if let Repr::Host(x) = a.0
            && x >= 0.0
        {
            match self.pc() {
                Pc::Single => {
                    let r = (x as f32).sqrt();
                    let r64 = f64::from(r);
                    if self.fits(r64) {
                        if r64 * r64 != x {
                            self.raise(x87::Flags::PE);
                        }
                        return St(Repr::Host(r64));
                    }
                }
                Pc::Double => {
                    let r = x.sqrt();
                    if self.fits(r) {
                        if r.mul_add(r, -x) != 0.0 {
                            self.raise(x87::Flags::PE);
                        }
                        return St(Repr::Host(r));
                    }
                }
                Pc::Model => {}
            }
        }
        let (r, f) = x87::sqrt(a.wide(), self.mode());
        self.raise(f);
        self.settle(r)
    }

    /// `fchs`.
    pub fn neg(&self, a: St) -> St {
        match a.0 {
            Repr::Host(v) => St(Repr::Host(-v)),
            Repr::Wide(w) => St(Repr::Wide(F80 {
                se: w.se ^ 0x8000,
                ..w
            })),
        }
    }

    /// `fabs`.
    pub fn abs(&self, a: St) -> St {
        match a.0 {
            Repr::Host(v) => St(Repr::Host(v.abs())),
            Repr::Wide(w) => St(Repr::Wide(F80 {
                se: w.se & 0x7fff,
                ..w
            })),
        }
    }

    /// `fcom` (`quiet`: `fucom`): `None` when unordered (C0, C2 and C3
    /// set). A NaN raises IE (an SNaN only, for `fucom`); a denormal DE.
    pub fn compare(&self, a: St, b: St, quiet: bool) -> Option<Ordering> {
        if let (Repr::Host(x), Repr::Host(y)) = (a.0, b.0) {
            return x.partial_cmp(&y);
        }
        let (c, f) = x87::compare(a.wide(), b.wide(), quiet);
        self.raise(f);
        match c {
            Cmp::Less => Some(Ordering::Less),
            Cmp::Equal => Some(Ordering::Equal),
            Cmp::Greater => Some(Ordering::Greater),
            Cmp::Unordered => None,
        }
    }

    /// `fst(p) m32`: the stored bits.
    pub fn store32(&self, a: St) -> u32 {
        if let Repr::Host(v) = a.0
            && self.pc() != Pc::Model
        {
            let r = v as f32;
            let m = r.to_bits() & 0x7fff_ffff;
            if (r == 0.0 && v == 0.0) || (0x0080_0001..0x7f80_0000).contains(&m) {
                if f64::from(r) != v {
                    self.raise(x87::Flags::PE);
                }
                return r.to_bits();
            }
        }
        let (bits, f) = a.wide().to_f32_bits(self.mode());
        self.raise(f);
        bits
    }

    /// `fist(p) m32int` under the control's rounding: the indefinite
    /// `0x8000_0000` and IE when the value does not fit.
    pub fn fist32(&self, a: St) -> i32 {
        if let Repr::Host(v) = a.0
            && Rc::from_cw(self.control) == Rc::Near
        {
            let r = v.round_ties_even();
            if (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&r) {
                if r != v {
                    self.raise(x87::Flags::PE);
                }
                return r as i32;
            }
        }
        let (i, f) = a.wide().to_i32(self.mode(), false);
        self.raise(f);
        i
    }
}

/// Whether `a + b` rounded to `s` was inexact (Knuth's TwoSum; exact
/// barring overflow, which the fast range excludes).
fn two_sum32(a: f32, b: f32, s: f32) -> bool {
    let bb = s - a;
    let err = (a - (s - bb)) + (b - bb);
    err != 0.0
}

fn two_sum64(a: f64, b: f64, s: f64) -> bool {
    let bb = s - a;
    let err = (a - (s - bb)) + (b - bb);
    err != 0.0
}

#[cfg(test)]
mod tests;
