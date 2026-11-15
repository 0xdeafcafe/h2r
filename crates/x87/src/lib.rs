//! Exact software x87: the 80-bit extended ("ext80") arithmetic authority for
//! the recompiler, specified in docs/FLOAT-ABI.md §6.
//!
//! Everything here is integer arithmetic. No host floating-point operation is
//! used to compute a result and nothing calls a host math library, so the
//! arm64 and x86-64 builds produce identical bits.
//!
//! * `add`/`sub`/`mul`/`div`/`sqrt` round the infinitely precise result once,
//!   to the precision-control width, under the rounding mode, with the 15-bit
//!   exponent range kept at every precision.
//! * `rndint`, `scale`, `xtract`, `prem`, `prem1`, `compare` and every
//!   conversion are exact (correctly rounded where they round).
//! * The transcendentals compute in a 512-bit software float and round once
//!   to 64 bits under round-to-nearest. The optional `fast-trans` feature
//!   first tries guarded 256-bit evaluation for FYL2X, F2XM1 and FPATAN;
//!   ambiguous cases retain 512 bits. See docs/x87-transcendentals.md and
//!   [`TRANSCENDENTAL_IS_CORRECTLY_ROUNDED`].
//!
//! Status flags are returned as [`Flags`] using status-word bit positions.
//! They describe the *masked* response; the `abi` module decides what an
//! unmasked exception does.

#![no_std]
#![cfg_attr(not(feature = "abi"), forbid(unsafe_code))]
// The `abi` module's `extern "C"` shims must dereference raw pointers and use
// `#[unsafe(no_mangle)]`; every other module carries its own
// `#![forbid(unsafe_code)]`.
#![cfg_attr(feature = "abi", deny(unsafe_code))]

#[cfg(test)]
extern crate std;

mod arith;
mod consts;
mod convert;
mod round;
mod trans;
mod wide;

#[cfg(feature = "abi")]
pub mod abi;

pub use arith::{Cmp, add, compare, div, mul, prem, prem1, rndint, scale, sqrt, sub, xtract};
pub use trans::{cos, patan, sin, sincos, tan, x2m1, yl2x, yl2xp1};

/// Whether the transcendentals are *proven* correctly rounded.
///
/// The reference computes with roughly 500 bits of working precision and
/// rounds once. The optional `fast-trans` path certifies unambiguous
/// rounding cells before accepting a lower working precision. Ambiguous
/// cases fall back to the reference, whose 64-bit worst cases have not
/// been exhaustively searched. Correct rounding of the total interface
/// remains unproven, and this is `false`. See `trans.rs`.
pub const TRANSCENDENTAL_IS_CORRECTLY_ROUNDED: bool = false;

/// An 80-bit extended value: explicit-integer-bit significand, then sign and
/// 15-bit biased exponent (bias 16383).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct F80 {
    pub sig: u64,
    pub se: u16,
}

/// Rounding control, x87 RC field order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rc {
    Near,
    Down,
    Up,
    Chop,
}

/// Precision control: 24, 53 or 64 significand bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pc {
    Single,
    Double,
    Extended,
}

/// Exception flags at their status-word bit positions.
///
/// Besides the spec's IE DE ZE OE UE PE SF this also carries [`Flags::C1`]
/// (bit 9): set when a rounding incremented the significand ("rounded up"),
/// which the x87 reports in C1 alongside PE.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Flags(pub u16);

/// Rounding and precision for one operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Mode {
    pub rc: Rc,
    pub pc: Pc,
}

/// Operand classification. Pseudo-denormals (exponent 0, integer bit set)
/// are `Denormal`; pseudo-NaNs, pseudo-infinities and unnormals are
/// `Unsupported`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    Zero,
    Normal,
    Denormal,
    Inf,
    QNaN,
    SNaN,
    Unsupported,
}

/// The `fld1`/`fldz`/`fldpi`/... constants, in the IR's `X87LoadConst` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Const {
    One,
    Z,
    Pi,
    L2E,
    L2T,
    Lg2,
    Ln2,
}

/// FPREM/FPREM1 are iterative: `incomplete` means C2 stays set and the guest
/// loop runs the instruction again. `q` carries the low three quotient bits
/// for C0/C3/C1 when complete (it is 0 when `incomplete`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Partial {
    pub value: F80,
    pub incomplete: bool,
    pub q: u8,
    pub flags: Flags,
}

/// `out_of_range` is the trig-only |operand| >= 2^63 case: C2 is set and the
/// operand is left unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transcendental {
    pub value: F80,
    pub out_of_range: bool,
    pub flags: Flags,
}

impl Flags {
    pub const NONE: Flags = Flags(0);
    pub const IE: Flags = Flags(0x0001);
    pub const DE: Flags = Flags(0x0002);
    pub const ZE: Flags = Flags(0x0004);
    pub const OE: Flags = Flags(0x0008);
    pub const UE: Flags = Flags(0x0010);
    pub const PE: Flags = Flags(0x0020);
    pub const SF: Flags = Flags(0x0040);
    /// Not an exception: the result's significand was rounded up.
    pub const C1: Flags = Flags(0x0200);
    /// Not an exception and never written to the status word: the rounded
    /// result is nonzero and below the destination format's smallest
    /// normal (tininess is detected after rounding, as on x86), whether or
    /// not it is exact. With UE masked only a tiny *inexact* result raises
    /// UE, which the flags already carry; with UE unmasked the x87 raises
    /// UE for every tiny result (SDM Vol. 1 §4.9.1.5, §8.5.5), so the
    /// `abi` module treats this bit as UE when UM is clear.
    pub const TINY: Flags = Flags(0x1000);

    pub const fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }
    pub const fn union(self, other: Flags) -> Flags {
        Flags(self.0 | other.0)
    }
    pub const fn without(self, other: Flags) -> Flags {
        Flags(self.0 & !other.0)
    }
    /// The six exception bits IE..PE.
    pub const fn exceptions(self) -> u16 {
        self.0 & 0x3f
    }
}

impl core::ops::BitOr for Flags {
    type Output = Flags;
    fn bitor(self, rhs: Flags) -> Flags {
        Flags(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for Flags {
    fn bitor_assign(&mut self, rhs: Flags) {
        self.0 |= rhs.0;
    }
}

impl core::fmt::Debug for Flags {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        const NAMES: [(u16, &str); 9] = [
            (0x0001, "IE"),
            (0x0002, "DE"),
            (0x0004, "ZE"),
            (0x0008, "OE"),
            (0x0010, "UE"),
            (0x0020, "PE"),
            (0x0040, "SF"),
            (0x0200, "C1"),
            (0x1000, "TINY"),
        ];
        write!(f, "Flags(")?;
        let mut first = true;
        for (bit, name) in NAMES {
            if self.0 & bit != 0 {
                if !first {
                    write!(f, "|")?;
                }
                write!(f, "{name}")?;
                first = false;
            }
        }
        let rest = self.0 & !0x127f;
        if rest != 0 {
            write!(f, "{}{rest:#x}", if first { "" } else { "|" })?;
        } else if first {
            write!(f, "-")?;
        }
        write!(f, ")")
    }
}

impl Pc {
    /// Significand bits kept by this precision.
    pub const fn bits(self) -> u32 {
        match self {
            Pc::Single => 24,
            Pc::Double => 53,
            Pc::Extended => 64,
        }
    }
    /// PC field (control word bits 8-9). The reserved value 01 is treated as
    /// extended.
    pub const fn from_cw(cw: u16) -> Pc {
        match (cw >> 8) & 3 {
            0 => Pc::Single,
            2 => Pc::Double,
            _ => Pc::Extended,
        }
    }
}

impl Rc {
    /// RC field (control word bits 10-11).
    pub const fn from_cw(cw: u16) -> Rc {
        match (cw >> 10) & 3 {
            0 => Rc::Near,
            1 => Rc::Down,
            2 => Rc::Up,
            _ => Rc::Chop,
        }
    }
}

impl Mode {
    /// Round to nearest, 64-bit precision.
    pub const EXTENDED: Mode = Mode {
        rc: Rc::Near,
        pc: Pc::Extended,
    };
    pub const fn new(rc: Rc, pc: Pc) -> Mode {
        Mode { rc, pc }
    }
    pub const fn from_cw(cw: u16) -> Mode {
        Mode {
            rc: Rc::from_cw(cw),
            pc: Pc::from_cw(cw),
        }
    }
}

impl F80 {
    pub const ZERO: F80 = F80 { sig: 0, se: 0 };
    pub const ONE: F80 = F80 {
        sig: 1 << 63,
        se: 0x3fff,
    };
    /// The real indefinite: negative QNaN with only the quiet bit set.
    pub const INDEFINITE: F80 = F80 {
        sig: 0xc000_0000_0000_0000,
        se: 0xffff,
    };
    pub const INFINITY: F80 = F80 {
        sig: 1 << 63,
        se: 0x7fff,
    };
    pub const NEG_INFINITY: F80 = F80 {
        sig: 1 << 63,
        se: 0xffff,
    };
    /// The constants below are the round-to-nearest values of the exact
    /// constants (what `fldpi` etc. load under RC=nearest). `consts.rs`
    /// tests them against 1024-bit evaluations.
    pub const PI: F80 = F80 {
        sig: 0xc90f_daa2_2168_c235,
        se: 0x4000,
    };
    pub const L2E: F80 = F80 {
        sig: 0xb8aa_3b29_5c17_f0bc,
        se: 0x3fff,
    };
    pub const L2T: F80 = F80 {
        sig: 0xd49a_784b_cd1b_8afe,
        se: 0x4000,
    };
    pub const LG2: F80 = F80 {
        sig: 0x9a20_9a84_fbcf_f799,
        se: 0x3ffd,
    };
    pub const LN2: F80 = F80 {
        sig: 0xb172_17f7_d1cf_79ac,
        se: 0x3ffe,
    };

    pub const fn new(se: u16, sig: u64) -> F80 {
        F80 { sig, se }
    }

    /// Decode the guest's 10-byte little-endian memory format.
    pub const fn from_bytes(m: [u8; 10]) -> F80 {
        F80 {
            sig: u64::from_le_bytes([m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7]]),
            se: u16::from_le_bytes([m[8], m[9]]),
        }
    }

    /// Encode to the guest's 10-byte little-endian memory format.
    pub const fn to_bytes(self) -> [u8; 10] {
        let s = self.sig.to_le_bytes();
        let e = self.se.to_le_bytes();
        [s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7], e[0], e[1]]
    }

    pub const fn is_negative(self) -> bool {
        self.se & 0x8000 != 0
    }

    pub const fn exponent_field(self) -> u16 {
        self.se & 0x7fff
    }

    pub const fn classify(self) -> Class {
        let e = self.se & 0x7fff;
        let j = self.sig >> 63 != 0;
        if e == 0 {
            if self.sig == 0 {
                Class::Zero
            } else {
                Class::Denormal
            }
        } else if e == 0x7fff {
            if !j {
                Class::Unsupported
            } else if self.sig << 1 == 0 {
                Class::Inf
            } else if self.sig & (1 << 62) != 0 {
                Class::QNaN
            } else {
                Class::SNaN
            }
        } else if j {
            Class::Normal
        } else {
            Class::Unsupported
        }
    }

    pub const fn is_nan(self) -> bool {
        matches!(self.classify(), Class::QNaN | Class::SNaN)
    }

    pub const fn is_snan(self) -> bool {
        matches!(self.classify(), Class::SNaN)
    }

    /// Flip the sign bit (FCHS). No flags, NaNs included.
    pub const fn neg(self) -> F80 {
        F80 {
            sig: self.sig,
            se: self.se ^ 0x8000,
        }
    }

    /// Clear the sign bit (FABS). No flags, NaNs included.
    pub const fn abs(self) -> F80 {
        F80 {
            sig: self.sig,
            se: self.se & 0x7fff,
        }
    }

    /// Exact widening of a binary32 bit pattern. NaN payloads are kept and
    /// an SNaN stays signalling (the load helper quiets it and raises IE).
    pub const fn from_f32_bits(b: u32) -> F80 {
        convert::from_f32_bits(b)
    }

    /// Exact widening of a binary64 bit pattern; NaNs as for `from_f32_bits`.
    pub const fn from_f64_bits(b: u64) -> F80 {
        convert::from_f64_bits(b)
    }

    /// Exact.
    pub const fn from_i64(v: i64) -> F80 {
        convert::from_i64(v)
    }

    /// Correctly rounded narrowing under `m.rc` (`m.pc` is ignored).
    pub fn to_f32_bits(self, m: Mode) -> (u32, Flags) {
        let (b, f) = convert::narrow(self, 23, 8, m.rc);
        (b as u32, f)
    }

    /// Correctly rounded narrowing under `m.rc` (`m.pc` is ignored).
    pub fn to_f64_bits(self, m: Mode) -> (u64, Flags) {
        convert::narrow(self, 52, 11, m.rc)
    }

    /// Round by `m.rc`, or truncate when `trunc`. NaN, infinity, unsupported
    /// or out of range gives `i16::MIN` and IE.
    pub fn to_i16(self, m: Mode, trunc: bool) -> (i16, Flags) {
        let (v, f) = convert::to_int(self, 16, if trunc { Rc::Chop } else { m.rc });
        (v as i16, f)
    }

    /// As `to_i16`, giving `i32::MIN` (0x8000_0000) for the indefinite.
    pub fn to_i32(self, m: Mode, trunc: bool) -> (i32, Flags) {
        let (v, f) = convert::to_int(self, 32, if trunc { Rc::Chop } else { m.rc });
        (v as i32, f)
    }

    /// As `to_i16`, giving `i64::MIN` for the indefinite.
    pub fn to_i64(self, m: Mode, trunc: bool) -> (i64, Flags) {
        convert::to_int(self, 64, if trunc { Rc::Chop } else { m.rc })
    }

    /// FBLD: 18 packed BCD digits, sign in bit 7 of byte 9. Exact. Nibbles
    /// above 9 are undefined on hardware; here they are weighted as digits.
    pub const fn from_bcd(m: [u8; 10]) -> F80 {
        convert::from_bcd(m)
    }

    /// FBSTP: round by `rc`; NaN, infinity, unsupported or |value| >= 10^18
    /// gives the BCD indefinite (`ff ff c0 00 ..` from the top byte) and IE.
    pub fn to_bcd(self, rc: Rc) -> ([u8; 10], Flags) {
        convert::to_bcd(self, rc)
    }

    /// A load-constant instruction under rounding mode `rc`. The constant is
    /// correctly rounded from its exact value, so RC=nearest gives the
    /// associated `F80` constant. No flags.
    pub const fn constant(c: Const, rc: Rc) -> F80 {
        consts::load_const(c, rc)
    }
}
