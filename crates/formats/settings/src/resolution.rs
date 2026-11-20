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

    pub fn size(self, native: (u32, u32)) -> (u32, u32) {
        match self {
            Self::Native => native,
            Self::Fixed(w, h) => (w, h),
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
