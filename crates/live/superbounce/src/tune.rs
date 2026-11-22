//! Live overrides for the model's global constants (docs/runtime/agent-api.md,
//! `superbounce.tune`): [`speed::BOUNCE_SPEED`](crate::speed::BOUNCE_SPEED),
//! [`speed::EXCESS_GAIN`](crate::speed::EXCESS_GAIN) and
//! [`CATCH`](crate::CATCH). Each constant is the default; the model reads
//! it through its accessor ([`crate::speed::bounce_speed`],
//! [`crate::speed::excess_gain`], [`crate::catch`]), which returns the
//! override while one is set. Overrides live in memory only, for the
//! process; nothing is saved.

use std::sync::atomic::{AtomicU32, Ordering};

/// The "no override" pattern: a NaN, which [`Tunable::set`] refuses.
const NONE: u32 = u32::MAX;

/// A constant with an optional override, the f32's bits in an atomic.
pub struct Tunable {
    default: f32,
    bits: AtomicU32,
}

impl Tunable {
    pub const fn new(default: f32) -> Self {
        Self {
            default,
            bits: AtomicU32::new(NONE),
        }
    }

    /// The value in effect: the override, else the default.
    pub fn get(&self) -> f32 {
        match self.bits.load(Ordering::Relaxed) {
            NONE => self.default,
            bits => f32::from_bits(bits),
        }
    }

    /// Overrides with `v`; a NaN clears instead.
    pub fn set(&self, v: f32) {
        if v.is_nan() {
            self.clear();
        } else {
            self.bits.store(v.to_bits(), Ordering::Relaxed);
        }
    }

    /// Back to the default.
    pub fn clear(&self) {
        self.bits.store(NONE, Ordering::Relaxed);
    }

    /// Whether an override is set.
    pub fn overridden(&self) -> bool {
        self.bits.load(Ordering::Relaxed) != NONE
    }

    pub fn default_value(&self) -> f32 {
        self.default
    }

    /// `default` or `override`: where [`Tunable::get`] comes from.
    pub fn source(&self) -> &'static str {
        if self.overridden() { "override" } else { "default" }
    }
}

pub static BOUNCE_SPEED: Tunable = Tunable::new(crate::speed::BOUNCE_SPEED);
pub static EXCESS_GAIN: Tunable = Tunable::new(crate::speed::EXCESS_GAIN);
pub static CATCH: Tunable = Tunable::new(crate::CATCH);

/// Clears every override.
pub fn clear_all() {
    BOUNCE_SPEED.clear();
    EXCESS_GAIN.clear();
    CATCH.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_overrides_and_clears() {
        let t = Tunable::new(5.4);
        assert_eq!((t.get(), t.source(), t.overridden()), (5.4, "default", false));
        t.set(6.25);
        assert_eq!((t.get(), t.source(), t.overridden()), (6.25, "override", true));
        assert_eq!(t.default_value(), 5.4);
        t.set(0.0);
        assert_eq!(t.get(), 0.0, "zero is a value, not 'none'");
        t.set(f32::NAN);
        assert_eq!((t.get(), t.source()), (5.4, "default"));
        t.set(1.0);
        t.clear();
        assert_eq!(t.get(), 5.4);
    }

    #[test]
    fn the_statics_default_to_the_constants() {
        // No test sets the statics, so these hold in any order.
        assert_eq!(BOUNCE_SPEED.default_value(), crate::speed::BOUNCE_SPEED);
        assert_eq!(EXCESS_GAIN.default_value(), crate::speed::EXCESS_GAIN);
        assert_eq!(CATCH.default_value(), crate::CATCH);
    }
}
