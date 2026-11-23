//! The levels of a clip and its verdict against a reference set.
//! The level maths (peak, RMS, BS.1770-4 loudness) is the audio crate's
//! [`audio::meter`], which the game's `audio.meter` shares; this keeps
//! the reference spread and the verdict, which only `sound.measure` and
//! `check.run` use.

pub use audio::meter::{Levels, measure, round1};

/// A set of values' spread: nearest-rank percentiles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spread {
    pub p10: f64,
    pub p50: f64,
    pub p90: f64,
    pub max: f64,
}

/// The nearest-rank `p`th percentile (0 to 100) of `sorted` (ascending,
/// not empty).
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    let rank = ((p / 100.0) * n as f64).ceil() as usize;
    sorted[rank.clamp(1, n) - 1]
}

/// The spread of `values`; `None` for none.
pub fn spread(values: &[f64]) -> Option<Spread> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    Some(Spread {
        p10: percentile(&v, 10.0),
        p50: percentile(&v, 50.0),
        p90: percentile(&v, 90.0),
        max: v[v.len() - 1],
    })
}

/// How far over the reference's p90 counts as loud (dB): the meters'
/// rounding and a permutation's natural spread stay under it.
pub const LOUD_MARGIN_DB: f64 = 1.0;
/// How far over the reference's p90 is an error rather than a warning
/// (dB): twice the perceived loudness and more.
pub const TOO_LOUD_DB: f64 = 6.0;
/// How far under the reference's p10 counts as quiet (dB).
pub const QUIET_MARGIN_DB: f64 = 6.0;

/// A clip's loudness against a reference set's.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    /// `loud`, `quiet`, `ok`, or `no_reference`.
    pub level: &'static str,
    /// The clip's loudness less the reference's p90 (LU = dB).
    pub over_p90_db: Option<f64>,
    /// The clip's sample peak less the reference's peak p90 (dB).
    pub peak_over_p90_db: Option<f64>,
    pub text: String,
}

/// `lv` against the reference's loudness and peak spreads.
pub fn verdict(lv: &Levels, lufs: Option<Spread>, peak: Option<Spread>, of: &str) -> Verdict {
    let Some(r) = lufs else {
        return Verdict {
            level: "no_reference",
            over_p90_db: None,
            peak_over_p90_db: None,
            text: format!("no reference sounds to compare with ({of})"),
        };
    };
    let over = round1(lv.lufs - r.p90);
    let peak_over = peak.map(|p| round1(lv.peak_dbfs - p.p90));
    let (level, text) = if over > LOUD_MARGIN_DB {
        (
            "loud",
            format!("louder than the p90 of {of} by {over:.1} dB"),
        )
    } else if lv.lufs < r.p10 - QUIET_MARGIN_DB {
        (
            "quiet",
            format!("quieter than the p10 of {of} by {:.1} dB", r.p10 - lv.lufs),
        )
    } else {
        (
            "ok",
            format!("within the range of {of} ({over:+.1} dB against its p90)"),
        )
    };
    Verdict {
        level,
        over_p90_db: Some(over),
        peak_over_p90_db: peak_over,
        text,
    }
}
