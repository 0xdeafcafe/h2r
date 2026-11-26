use std::collections::VecDeque;

pub(crate) const SAMPLES: usize = 300;
const MINUTE_MS: u64 = 60_000;
const GROWTH: f64 = (200 * 1024 * 1024) as f64;
const GIB: u64 = 1 << 30;

/// An integer byte count, or a binary size such as 12G, 512MiB or 64K.
pub fn parse_size(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    let (count, unit) = text.split_at(digits);
    let bad = || format!("invalid memory size {text:?} (expected bytes or a size like 12G)");
    let scale: u64 = match unit.to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => GIB,
        "t" | "tb" | "tib" => 1 << 40,
        _ => return Err(bad()),
    };
    count
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(scale))
        .ok_or_else(bad)
}

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) soft: u64,
    pub(crate) hard: u64,
}

impl Limits {
    pub(crate) fn defaults(ram: Option<u64>) -> Self {
        // Three quarters of RAM: Ascension at 4x peaks near 8 GB while it
        // loads (lab job 0026), which two fifths of an 18 GB Mac cut off.
        let hard = ram
            .filter(|n| *n > 0)
            .map_or(16 * GIB, |n| (16 * GIB).min(n / 4 * 3));
        Self {
            soft: 8 * GIB,
            hard,
        }
    }

    pub(crate) fn from_env() -> Self {
        fn size(key: &str, default: u64) -> u64 {
            let Some(raw) = std::env::var_os(key) else {
                return default;
            };
            parse_size(&raw.to_string_lossy()).unwrap_or_else(|err| {
                eprintln!("memory watchdog: {key}: {err}; using {default} bytes");
                default
            })
        }
        let defaults = Self::defaults(crate::sample::physical_ram());
        Self {
            soft: size("H2_MEM_SOFT", defaults.soft),
            hard: size("H2_MEM_HARD", defaults.hard),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trigger {
    Soft,
    Growth,
    Hard,
}

impl Trigger {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Soft => "soft limit",
            Self::Growth => "sustained growth",
            Self::Hard => "hard limit",
        }
    }
}

pub(crate) struct History {
    pub(crate) samples: VecDeque<(u64, u64)>,
    pub(crate) peak: u64,
    pub(crate) growth: f64,
    soft_active: bool,
    growth_active: bool,
}

impl Default for History {
    fn default() -> Self {
        Self {
            samples: VecDeque::with_capacity(SAMPLES),
            peak: 0,
            growth: 0.0,
            soft_active: false,
            growth_active: false,
        }
    }
}

fn rate((t0, b0): (u64, u64), (t1, b1): (u64, u64)) -> f64 {
    (b1 as f64 - b0 as f64) * MINUTE_MS as f64 / t1.saturating_sub(t0).max(1) as f64
}

impl History {
    /// Both sampler and monotonic clock are supplied by the caller. Empty
    /// readings never clear an alarm or overwrite the last known footprint.
    pub(crate) fn poll(
        &mut self,
        limits: Limits,
        clock_ms: impl FnOnce() -> u64,
        sample: impl FnOnce() -> Option<u64>,
    ) -> Option<Trigger> {
        let bytes = sample()?;
        let now = clock_ms();
        if self.samples.len() == SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back((now, bytes));
        self.peak = self.peak.max(bytes);

        // Require growth in EACH of three consecutive minutes. A one-off
        // allocation or a laptop waking from sleep isn't a sustained climb.
        let mut end = (now, bytes);
        let mut sustained = true;
        let mut oldest = end;
        for minutes in 1..=3 {
            let Some(target) = now.checked_sub(minutes * MINUTE_MS) else {
                oldest = self.samples[0];
                sustained = false;
                break;
            };
            let i = self.samples.partition_point(|&(t, _)| t <= target);
            let Some(index) = i.checked_sub(1) else {
                oldest = self.samples[0];
                sustained = false;
                break;
            };
            oldest = self.samples[index];
            sustained &= target - oldest.0 <= 4_000 && rate(oldest, end) > GROWTH;
            end = oldest;
        }
        self.growth = rate(oldest, (now, bytes));
        let soft = limits.soft != 0 && bytes > limits.soft;
        let growth = limits.soft != 0 && sustained;
        let trigger = if limits.hard != 0 && bytes > limits.hard {
            Some(Trigger::Hard)
        } else if soft && !self.soft_active {
            Some(Trigger::Soft)
        } else if growth && !self.growth_active {
            Some(Trigger::Growth)
        } else {
            None
        };
        self.soft_active = soft;
        self.growth_active = growth;
        trigger
    }
}
