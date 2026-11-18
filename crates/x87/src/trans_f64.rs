//! FSIN, FCOS, FSINCOS, FYL2X and F2XM1 in native binary64 (`libm`), for
//! a measurement only: off unless [`set_f64_transcendentals`] turns it on
//! (boot does when `H2_X87_FAST_TRANSCENDENTALS=1`). The operand is rounded
//! to 53 bits, the function evaluated in f64 and the result widened, so it
//! is not the x87's 64-bit result: about 11 bits short, and PE is always
//! set with C1 clear (the rounding direction is not known). Operands
//! outside the plain cases (not finite or normal, trig operands of 2^63 or
//! more, exact results, results that are not normal f64) take the series.
//! `tests/f64_trans.rs` measures the differences.
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicBool, Ordering};

use crate::{Class, F80, Flags, Mode, Transcendental};

static ON: AtomicBool = AtomicBool::new(false);

/// Compute the transcendentals in f64 from now on (`true`), or with the
/// correctly rounded series (`false`, the default). Process-wide.
pub fn set_f64_transcendentals(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

/// Whether [`set_f64_transcendentals`] turned the f64 path on.
pub fn f64_transcendentals() -> bool {
    ON.load(Ordering::Relaxed)
}

#[inline]
pub(crate) fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// A normal finite operand as f64, or none.
fn operand(a: F80) -> Option<f64> {
    if a.classify() != Class::Normal {
        return None;
    }
    let (bits, _) = a.to_f64_bits(Mode::EXTENDED);
    let v = f64::from_bits(bits);
    v.is_normal().then_some(v)
}

fn result(v: f64) -> Option<(F80, Flags)> {
    v.is_normal().then(|| (F80::from_f64_bits(v.to_bits()), Flags::PE))
}

/// Below 2^63: the x87's own range limit for FSIN/FCOS/FSINCOS.
fn trig_operand(a: F80) -> Option<f64> {
    operand(a).filter(|v| v.abs() < 9.223_372_036_854_775_808e18)
}

fn trig(v: Option<(F80, Flags)>) -> Option<Transcendental> {
    v.map(|(value, flags)| Transcendental { value, out_of_range: false, flags })
}

pub(crate) fn sin(a: F80) -> Option<Transcendental> {
    trig(result(libm::sin(trig_operand(a)?)))
}

pub(crate) fn cos(a: F80) -> Option<Transcendental> {
    trig(result(libm::cos(trig_operand(a)?)))
}

pub(crate) fn sincos(a: F80) -> Option<(Transcendental, F80)> {
    let x = trig_operand(a)?;
    let (s, c) = libm::sincos(x);
    let (s, c) = (result(s)?, result(c)?);
    Some((Transcendental { value: s.0, out_of_range: false, flags: Flags::PE }, c.0))
}

pub(crate) fn yl2x(y: F80, x: F80) -> Option<(F80, Flags)> {
    let (fy, fx) = (operand(y)?, operand(x)?);
    // Powers of two (log2 exact) and non-positive x: the series.
    if fx <= 0.0 || x.sig == 1 << 63 {
        return None;
    }
    result(fy * libm::log2(fx))
}

pub(crate) fn x2m1(a: F80) -> Option<(F80, Flags)> {
    let v = operand(a)?;
    // F2XM1's domain is [-1, 1]; at the ends the result is exact.
    if v.abs() >= 1.0 {
        return None;
    }
    result(libm::expm1(v * core::f64::consts::LN_2))
}
