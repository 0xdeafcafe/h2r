//! An internal output size. Native tracks the drawable throughout the run.

use crate::{SettingType, Value};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    Native,
    Fixed(u32, u32),
}

impl Resolution {
    /// The largest dimension the host frame handoff accepts.
    pub const MAX_DIMENSION: u32 = 8192;

    pub fn fixed(width: u32, height: u32) -> Option<Self> {
        let valid = 1..=Self::MAX_DIMENSION;
        (valid.contains(&width) && valid.contains(&height)).then_some(Self::Fixed(width, height))
    }

    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("native") {
            return Some(Self::Native);
        }
        let (w, h) = text.split_once(['x', 'X'])?;
        if ![w, h].iter().all(|s| s.bytes().all(|b| b.is_ascii_digit())) {
            return None;
        }
        Self::fixed(w.parse().ok()?, h.parse().ok()?)
    }

    /// Native follows the drawable's aspect within the UHD 4K pixel budget.
    /// Explicit resolutions use the requested raster, including full 5K.
    pub fn size(self, native: (u32, u32)) -> (u32, u32) {
        const UHD_PIXELS: u64 = 3840 * 2160;
        match self {
            Self::Fixed(w, h) => (w, h),
            Self::Native => {
                let (w, h) = (u64::from(native.0), u64::from(native.1));
                if w * h <= UHD_PIXELS {
                    return native;
                }
                // sqrt(budget * width / height) and its transpose give
                // one uniform reduction. Round down to stay within budget.
                let fit = |axis: u64, other: u64| {
                    ((UHD_PIXELS * axis) / other).isqrt().clamp(1, UHD_PIXELS) as u32
                };
                (fit(w, h), fit(h, w))
            }
        }
    }

    /// One atomic setting handle carries both dimensions across a resize.
    pub fn value(self) -> Value {
        Value::Int(match self {
            Self::Native => 0,
            Self::Fixed(w, h) => (i64::from(w) << 32) | i64::from(h),
        })
    }

    pub fn from_value(value: Value) -> Option<Self> {
        let bits = value.as_int()?;
        if bits == 0 {
            Some(Self::Native)
        } else {
            Self::fixed((bits >> 32) as u32, bits as u32)
        }
    }
}

impl SettingType for Resolution {
    fn from_bits(bits: i64) -> Self {
        Self::from_value(Value::Int(bits)).expect("validated resolution setting")
    }
}

impl fmt::Display for Resolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Native => f.write_str("native"),
            Self::Fixed(w, h) => write!(f, "{w}x{h}"),
        }
    }
}

/// The earlier scalar setting, consulted only when the output size is absent
/// at the same precedence level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyScale {
    pub key: &'static str,
    pub env: &'static str,
    pub frame: (u32, u32),
    pub max: u32,
}

impl LegacyScale {
    pub fn parse(self, text: &str) -> Result<Resolution, String> {
        let size = text
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=self.max).contains(n))
            .and_then(|n| {
                Resolution::fixed(self.frame.0.checked_mul(n)?, self.frame.1.checked_mul(n)?)
            });
        size.ok_or_else(|| format!("expected a whole number from 1 to {}", self.max))
    }
}

#[cfg(test)]
mod tests {
    use super::Resolution::{Fixed, Native};

    #[test]
    fn native_uses_a_pixel_budget_and_explicit_sizes_use_the_full_raster() {
        assert_eq!(Native.size((5120, 2880)), (3840, 2160));
        assert_eq!(Native.size((2880, 5120)), (2160, 3840));
        for physical in [(5120, 1440), (3840, 2160), (1233, 857)] {
            assert_eq!(Native.size(physical), physical);
        }
        for (w, h) in [(7680, 2160), (4096, 2560), (4001, 4001)] {
            let (rw, rh) = Native.size((w, h));
            assert!(u64::from(rw) * u64::from(rh) <= 3840 * 2160);
            assert!(rw > 0 && rh > 0);
            let error = (f64::from(rw) - f64::from(rh) * f64::from(w) / f64::from(h)).abs();
            assert!(error <= f64::from(w) / f64::from(h));
        }
        assert_eq!(Fixed(5120, 2880).size((5120, 2880)), (5120, 2880));
    }
}
