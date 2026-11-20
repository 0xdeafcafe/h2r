//! Conversions between ext80 and binary32/binary64, integers and BCD.
//! Widening is exact; narrowing rounds once.
#![forbid(unsafe_code)]

use crate::round::{Unp, nan1, rnd, round_int, shr_sticky, unpack};
use crate::{F80, Flags, Rc};

pub(crate) const fn from_f32_bits(b: u32) -> F80 {
    let sign = ((b >> 31) as u16) << 15;
    let e = (b >> 23) & 0xff;
    let f = (b & 0x7f_ffff) as u64;
    if e == 0xff {
        // Infinity (f == 0) or NaN with payload, signalling kept.
        F80 {
            sig: 1 << 63 | f << 40,
            se: sign | 0x7fff,
        }
    } else if e == 0 {
        if f == 0 {
            F80 { sig: 0, se: sign }
        } else {
            // f * 2^-149, normalised.
            let lz = f.leading_zeros();
            F80 {
                sig: f << lz,
                se: sign | (16446 - 149 - lz) as u16,
            }
        }
    } else {
        F80 {
            sig: 1 << 63 | f << 40,
            se: sign | (e + 16383 - 127) as u16,
        }
    }
}

pub(crate) const fn from_f64_bits(b: u64) -> F80 {
    let sign = ((b >> 63) as u16) << 15;
    let e = ((b >> 52) & 0x7ff) as u32;
    let f = b & ((1 << 52) - 1);
    if e == 0x7ff {
        F80 {
            sig: 1 << 63 | f << 11,
            se: sign | 0x7fff,
        }
    } else if e == 0 {
        if f == 0 {
            F80 { sig: 0, se: sign }
        } else {
            // f * 2^-1074, normalised.
            let lz = f.leading_zeros();
            F80 {
                sig: f << lz,
                se: sign | (16446 - 1074 - lz) as u16,
            }
        }
    } else {
        F80 {
            sig: 1 << 63 | f << 11,
            se: sign | (e + 16383 - 1023) as u16,
        }
    }
}

pub(crate) const fn from_u64(neg: bool, mag: u64) -> F80 {
    let sign = if neg { 0x8000 } else { 0 };
    if mag == 0 {
        return F80 { sig: 0, se: sign };
    }
    let lz = mag.leading_zeros();
    F80 {
        sig: mag << lz,
        se: sign | (16383 + 63 - lz) as u16,
    }
}

pub(crate) const fn from_i64(v: i64) -> F80 {
    from_u64(v < 0, v.unsigned_abs())
}

/// Narrow to an IEEE format with `mant` fraction bits and `ebits` exponent
/// bits, rounding once under `rc` with IEEE gradual underflow. Tininess is
/// detected after rounding and UE is the masked rule (tiny and inexact).
/// SNaN and unsupported set IE; NaN payloads are truncated.
pub(crate) fn narrow(x: F80, mant: u32, ebits: u32, rc: Rc) -> (u64, Flags) {
    let neg = x.is_negative();
    let sign = (neg as u64) << (mant + ebits);
    let emax_field = (1u64 << ebits) - 1;
    let bias = (1i64 << (ebits - 1)) - 1;
    let frac_mask = (1u64 << mant) - 1;
    match unpack(x) {
        Unp::Bad => (
            1 << (mant + ebits) | emax_field << mant | 1 << (mant - 1),
            Flags::IE,
        ),
        Unp::Nan => {
            let (q, f) = nan1(x);
            let payload = (q.sig << 1) >> (64 - mant);
            (sign | emax_field << mant | payload, f)
        }
        Unp::Inf(_) => (sign | emax_field << mant, Flags::NONE),
        Unp::Zero(_) => (sign, Flags::NONE),
        Unp::Fin(f) => {
            let p = mant + 1;
            let drop = 128 - p;
            let mut e = f.top() as i64;
            let emin = 1 - bias;
            let m = (f.sig as u128) << 64;
            let mut fl = Flags::NONE;
            let bits = if e >= emin {
                let (mut q, inexact, up) = rnd(m, drop, neg, rc);
                if q >> p != 0 {
                    q >>= 1;
                    e += 1;
                }
                if e > bias {
                    let to_inf = match rc {
                        Rc::Near => true,
                        Rc::Chop => false,
                        Rc::Up => !neg,
                        Rc::Down => neg,
                    };
                    return if to_inf {
                        (sign | emax_field << mant, Flags::OE | Flags::PE | Flags::C1)
                    } else {
                        (
                            sign | (emax_field - 1) << mant | frac_mask,
                            Flags::OE | Flags::PE,
                        )
                    };
                }
                if inexact {
                    fl |= Flags::PE;
                }
                if up {
                    fl |= Flags::C1;
                }
                sign | ((e + bias) as u64) << mant | (q as u64 & frac_mask)
            } else {
                let tiny = e < emin - 1 || rnd(m, drop, neg, rc).0 >> p == 0;
                let m2 = shr_sticky(m, (emin - e) as u64);
                let (q, inexact, up) = rnd(m2, drop, neg, rc);
                if tiny {
                    fl |= Flags::TINY;
                }
                if inexact {
                    fl |= Flags::PE;
                    if tiny {
                        fl |= Flags::UE;
                    }
                }
                if up {
                    fl |= Flags::C1;
                }
                // A carry into bit `mant` is exactly the smallest normal.
                sign | q as u64
            };
            (bits, fl)
        }
    }
}

/// Round to a `bits`-wide two's complement integer under `rc`. The integer
/// indefinite (`-2^(bits-1)`) with IE for NaN, infinity, unsupported and
/// out of range; no PE in that case.
pub(crate) fn to_int(x: F80, bits: u32, rc: Rc) -> (i64, Flags) {
    let indef = -(1i128 << (bits - 1)) as i64;
    match unpack(x) {
        Unp::Bad | Unp::Nan | Unp::Inf(_) => (indef, Flags::IE),
        Unp::Zero(_) => (0, Flags::NONE),
        Unp::Fin(f) => {
            let (mag, inexact, up) = if f.exp > 0 {
                return (indef, Flags::IE);
            } else if f.exp == 0 {
                (f.sig as u128, false, false)
            } else {
                round_int(f.neg, f.sig, f.exp, rc)
            };
            let limit = 1u128 << (bits - 1);
            if (!f.neg && mag >= limit) || (f.neg && mag > limit) {
                return (indef, Flags::IE);
            }
            let v = if f.neg { -(mag as i128) } else { mag as i128 };
            let mut fl = Flags::NONE;
            if inexact {
                fl |= Flags::PE;
            }
            if up {
                fl |= Flags::C1;
            }
            (v as i64, fl)
        }
    }
}

pub(crate) const fn from_bcd(m: [u8; 10]) -> F80 {
    let mut v: u64 = 0;
    let mut i = 9;
    while i > 0 {
        i -= 1;
        let b = m[i] as u64;
        v = v * 100 + (b >> 4) * 10 + (b & 0xf);
    }
    from_u64(m[9] & 0x80 != 0, v)
}

pub(crate) const BCD_INDEFINITE: [u8; 10] = [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0xff];

pub(crate) fn to_bcd(x: F80, rc: Rc) -> ([u8; 10], Flags) {
    const MAX: u128 = 999_999_999_999_999_999;
    let (neg, mut mag, fl) = match unpack(x) {
        Unp::Bad | Unp::Nan | Unp::Inf(_) => return (BCD_INDEFINITE, Flags::IE),
        Unp::Zero(n) => (n, 0u128, Flags::NONE),
        Unp::Fin(f) => {
            if f.exp >= 0 {
                return (BCD_INDEFINITE, Flags::IE);
            }
            let (mag, inexact, up) = round_int(f.neg, f.sig, f.exp, rc);
            if mag > MAX {
                return (BCD_INDEFINITE, Flags::IE);
            }
            let mut fl = Flags::NONE;
            if inexact {
                fl |= Flags::PE;
            }
            if up {
                fl |= Flags::C1;
            }
            (f.neg, mag, fl)
        }
    };
    let mut out = [0u8; 10];
    for b in out.iter_mut().take(9) {
        let lo = (mag % 10) as u8;
        mag /= 10;
        let hi = (mag % 10) as u8;
        mag /= 10;
        *b = hi << 4 | lo;
    }
    out[9] = if neg { 0x80 } else { 0 };
    (out, fl)
}
