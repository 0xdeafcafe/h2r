//! Levels of decoded 16-bit PCM: sample peak, RMS, loudness after
//! ITU-R BS.1770-4 (K-weighted, gated), the loudest 400 ms block, length
//! and full-scale samples. Pure: `sound.measure` and `check.run` feed it
//! what `app::audition` decodes and the WAV clips of ours.

/// What a level of silence reads (dB): a finite floor JSON can carry.
pub const SILENCE_DB: f64 = -120.0;
/// BS.1770's absolute gate (LUFS).
const ABSOLUTE_GATE: f64 = -70.0;
/// BS.1770's relative gate, below the absolute-gated loudness (LU).
const RELATIVE_GATE: f64 = 10.0;
/// The gating block and its step (ms): 400 ms blocks overlapping by 75%.
const BLOCK_MS: u64 = 400;
const STEP_MS: u64 = 100;

/// One clip's levels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    /// The largest sample magnitude, dBFS (full scale 32768).
    pub peak_dbfs: f64,
    /// The root mean square of every sample of every channel, dBFS.
    pub rms_dbfs: f64,
    /// Integrated loudness, LUFS (BS.1770-4: K-weighting, 400 ms blocks
    /// stepped 100 ms, gated at -70 LUFS then 10 LU under the gated
    /// mean; a clip shorter than a block is one block). Each channel
    /// weighs 1, so a mono 997 Hz sine reads its RMS in dBFS.
    pub lufs: f64,
    /// The loudest single block, LUFS (momentary loudness's maximum).
    pub max_momentary_lufs: f64,
    pub frames: usize,
    pub rate: u32,
    pub channels: u16,
    pub duration_ms: u64,
    /// Samples at full scale (32767 or -32768).
    pub clipped: usize,
}

fn db(power: f64) -> f64 {
    if power > 0.0 {
        (10.0 * power.log10()).max(SILENCE_DB)
    } else {
        SILENCE_DB
    }
}

fn lufs_of(mean_square_sum: f64) -> f64 {
    if mean_square_sum > 0.0 {
        (-0.691 + 10.0 * mean_square_sum.log10()).max(SILENCE_DB)
    } else {
        SILENCE_DB
    }
}

/// A direct form I biquad.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    fn run(&mut self, x0: f64) -> f64 {
        let y0 = self.b[0] * x0 + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x0, self.x[0]];
        self.y = [y0, self.y[0]];
        y0
    }
}

/// BS.1770's K-weighting at `rate`: the high shelf then the high pass,
/// their coefficients derived for the rate from the 48 kHz design's
/// analogue parameters (the derivation libebur128 uses).
fn k_weighting(rate: u32) -> [Biquad; 2] {
    let rate = f64::from(rate.max(1));
    let pi = std::f64::consts::PI;
    // The shelf.
    let (f0, gain, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = (pi * f0 / rate).tan();
    let vh = 10f64.powf(gain / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        x: [0.0; 2],
        y: [0.0; 2],
    };
    // The high pass.
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = (pi * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let pass = Biquad {
        b: [1.0, -2.0, 1.0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        x: [0.0; 2],
        y: [0.0; 2],
    };
    [shelf, pass]
}

/// The levels of interleaved `pcm` of `channels` channels at `rate` Hz.
pub fn measure(pcm: &[i16], channels: u16, rate: u32) -> Levels {
    let ch = usize::from(channels.max(1));
    let frames = pcm.len() / ch;
    let pcm = &pcm[..frames * ch];
    let mut peak = 0i32;
    let mut sum_sq = 0f64;
    let mut clipped = 0usize;
    for &s in pcm {
        let m = i32::from(s).abs();
        peak = peak.max(m);
        if s == i16::MAX || s == i16::MIN {
            clipped += 1;
        }
        let v = f64::from(s) / 32768.0;
        sum_sq += v * v;
    }
    // Each frame's K-weighted power, summed over the channels (weight 1).
    let mut filters: Vec<[Biquad; 2]> = (0..ch).map(|_| k_weighting(rate)).collect();
    let mut power = Vec::with_capacity(frames);
    for frame in pcm.chunks_exact(ch) {
        let mut p = 0.0;
        for (c, &s) in frame.iter().enumerate() {
            let [shelf, pass] = &mut filters[c];
            let y = pass.run(shelf.run(f64::from(s) / 32768.0));
            p += y * y;
        }
        power.push(p);
    }
    let (lufs, max_momentary_lufs) = gated(&power, rate);
    Levels {
        peak_dbfs: db((f64::from(peak) / 32768.0).powi(2)),
        rms_dbfs: if pcm.is_empty() {
            SILENCE_DB
        } else {
            db(sum_sq / pcm.len() as f64)
        },
        lufs,
        max_momentary_lufs,
        frames,
        rate,
        channels,
        duration_ms: frames as u64 * 1000 / u64::from(rate.max(1)),
        clipped,
    }
}

/// Integrated loudness and the loudest block of per-frame K-weighted
/// channel-summed `power`.
fn gated(power: &[f64], rate: u32) -> (f64, f64) {
    if power.is_empty() {
        return (SILENCE_DB, SILENCE_DB);
    }
    let block = ((u64::from(rate) * BLOCK_MS / 1000) as usize).max(1);
    let step = ((u64::from(rate) * STEP_MS / 1000) as usize).max(1);
    // Prefix sums make each block's mean one subtraction.
    let mut prefix = Vec::with_capacity(power.len() + 1);
    prefix.push(0.0);
    for p in power {
        prefix.push(prefix.last().copied().unwrap_or(0.0) + p);
    }
    let mean = |lo: usize, hi: usize| (prefix[hi] - prefix[lo]) / (hi - lo) as f64;
    let blocks: Vec<f64> = if power.len() <= block {
        vec![mean(0, power.len())]
    } else {
        (0..=(power.len() - block) / step)
            .map(|i| mean(i * step, i * step + block))
            .collect()
    };
    let max_momentary = blocks
        .iter()
        .copied()
        .fold(SILENCE_DB, |m, z| m.max(lufs_of(z)));
    let above: Vec<f64> = blocks
        .iter()
        .copied()
        .filter(|z| lufs_of(*z) > ABSOLUTE_GATE)
        .collect();
    if above.is_empty() {
        return (SILENCE_DB, max_momentary);
    }
    let relative = lufs_of(above.iter().sum::<f64>() / above.len() as f64) - RELATIVE_GATE;
    let kept: Vec<f64> = above
        .into_iter()
        .filter(|z| lufs_of(*z) > relative)
        .collect();
    let integrated = if kept.is_empty() {
        SILENCE_DB
    } else {
        lufs_of(kept.iter().sum::<f64>() / kept.len() as f64)
    };
    (integrated, max_momentary)
}

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

/// `v` to one decimal place, as the API gives levels.
pub fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}
