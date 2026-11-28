//! An x87 stack value in a checked region, for sequences loaded from m32.
use super::{Checked, checked_inputs, host};
use std::cmp::Ordering;
use std::ops::{Add, Div, Mul, Neg, Sub};

#[derive(Clone, Copy)]
enum Value {
    Single(f32),
    Double(f64),
    /// [`super::checked_guest`]: the region's [`super::X87`] holds it.
    Guest(super::St),
}

/// One x87 stack value. Arithmetic rounds at the guest precision after
/// each operation. Wider exponents than the host format are declined by
/// the surrounding region. A double value keeps its low bits until an
/// explicit [`store`](Self::store) to guest binary32.
#[derive(Clone, Copy)]
#[must_use]
pub struct Chain<'r> {
    region: &'r Checked,
    value: Value,
}

impl<'r> Chain<'r> {
    /// `fld m32` after the adapter has checked [`super::x87_env`] for
    /// pending exceptions and the instruction sequence's stack usage.
    /// General chains support PC=24 and PC=53 under masked, nearest
    /// controls. The specialised [`super::x87_neg_quotient`] can handle PC=64.
    ///
    /// Under [`super::checked_guest`] every load succeeds: the value and
    /// all later operations run on the region's [`super::X87`] (`control`
    /// is the region's).
    pub fn load(region: &'r Checked, control: u16, value: f32) -> Option<Self> {
        if let Some(x) = region.x87() {
            debug_assert_eq!(x.control(), control);
            return Some(Self {
                region,
                value: Value::Guest(x.load32(value.to_bits())),
            });
        }
        if !checked_inputs(&[value.to_bits()]) || control & 0x0c3f != 0x003f {
            return None;
        }
        let value = match (control >> 8) & 3 {
            0 => Value::Single(value),
            2 => Value::Double(f64::from(value)),
            _ => return None,
        };
        Some(Self { region, value })
    }

    fn wide(self) -> f64 {
        match self.value {
            Value::Single(value) => f64::from(value),
            Value::Double(value) => value,
            Value::Guest(_) => unreachable!("a guest chain has no host value"),
        }
    }

    fn x87(&self) -> &super::X87 {
        self.region.x87().expect("a guest chain's region is a guest region")
    }

    /// `fsqrt` in the guest's working precision.
    pub fn sqrt(self) -> Self {
        let value = match self.value {
            Value::Single(x) => Value::Single(self.region.x87_32(&[x], || host::sqrt32(x))),
            Value::Double(x) => Value::Double(self.region.x87_64(|| host::sqrt64(x))),
            Value::Guest(x) => Value::Guest(self.x87().sqrt(x)),
        };
        Self { value, ..self }
    }

    /// Compare the working values before either operand is stored to m32
    /// (`fcom`): `None` when unordered, which under
    /// [`super::checked_guest`] a NaN gives (raising IE).
    pub fn compare(self, other: Self) -> Option<Ordering> {
        if let (Value::Guest(a), Value::Guest(b)) = (self.value, other.value) {
            return self.x87().compare(a, b, false);
        }
        if std::mem::discriminant(&self.value) != std::mem::discriminant(&other.value) {
            return None;
        }
        self.wide().partial_cmp(&other.wide())
    }

    /// `fst(p) m32`, preserving the sign of zero.
    pub fn store(self) -> f32 {
        match self.value {
            Value::Single(value) => value,
            Value::Double(value) => self.region.narrow(value),
            Value::Guest(value) => f32::from_bits(self.x87().store32(value)),
        }
    }

    /// `fist(p) m32int`. A rounded out-of-range value needs the guest's
    /// invalid conversion and declines the query.
    ///
    /// Under [`super::checked_guest`] the guest's conversion: the
    /// indefinite `i32::MIN` and IE when the value does not fit.
    pub fn integer(self) -> Option<i32> {
        if let Value::Guest(value) = self.value {
            return Some(self.x87().fist32(value));
        }
        let wide = self.wide();
        let rounded = wide.round_ties_even();
        let fits = (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&rounded);
        if fits && rounded != wide {
            self.region.x87_rounded();
        }
        fits.then_some(rounded as i32)
    }
}

impl Chain<'_> {
    /// `fabs`.
    pub fn abs(self) -> Self {
        let value = match self.value {
            Value::Single(value) => Value::Single(value.abs()),
            Value::Double(value) => Value::Double(value.abs()),
            Value::Guest(value) => Value::Guest(self.x87().abs(value)),
        };
        Self { value, ..self }
    }
}

impl Neg for Chain<'_> {
    type Output = Self;
    fn neg(self) -> Self {
        let value = match self.value {
            Value::Single(value) => Value::Single(-value),
            Value::Double(value) => Value::Double(-value),
            Value::Guest(value) => Value::Guest(self.x87().neg(value)),
        };
        Self { value, ..self }
    }
}

macro_rules! binary {
    ($trait:ident, $op:ident, $narrow:ident, $wide:ident) => {
        impl $trait for Chain<'_> {
            type Output = Self;
            fn $op(self, other: Self) -> Self {
                let value = match (self.value, other.value) {
                    (Value::Single(a), Value::Single(b)) => {
                        Value::Single(self.region.x87_32(&[a, b], || host::$narrow(a, b)))
                    }
                    (Value::Double(a), Value::Double(b)) => {
                        Value::Double(self.region.x87_64(|| host::$wide(a, b)))
                    }
                    (Value::Guest(a), Value::Guest(b)) => Value::Guest(self.x87().$op(a, b)),
                    // A retail chain runs under one precision control.
                    _ => {
                        self.region.decline();
                        self.value
                    }
                };
                Self { value, ..self }
            }
        }
    };
}

binary!(Add, add, add32, add64);
binary!(Sub, sub, sub32, sub64);
binary!(Mul, mul, mul32, mul64);
binary!(Div, div, div32, div64);
