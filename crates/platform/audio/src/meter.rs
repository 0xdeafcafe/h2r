//! Levels of PCM: sample peak, RMS, loudness after ITU-R BS.1770-4
//! (K-weighted, gated), the loudest 400 ms block, length and full-scale
//! samples. Pure. Studio's `sound.measure` meters decoded 16-bit clips
//! with [`measure`]; the game's `audio.meter` meters the mixer's output
//! bus with [`measure_f32`] (docs/audio/audio.md, "Metering the mixer").

/// What a level of silence reads (dB): a finite floor JSON can carry.
pub const SILENCE_DB: f64 = -120.0;
/// BS.1770's absolute gate (LUFS).
const ABSOLUTE_GATE: f64 = -70.0;
/// BS.1770's relative gate, below the absolute-gated loudness (LU).
const RELATIVE_GATE: f64 = 10.0;
/// The gating block and its step (ms): 400 ms blocks overlapping by 75%.
const BLOCK_MS: u64 = 400;
const STEP_MS: u64 = 100;

/// One signal's levels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    /// The largest sample magnitude, dBFS (full scale 1.0, or 32768 for
    /// 16-bit samples).
    pub peak_dbfs: f64,
    /// The root mean square of every sample of every channel, dBFS.
    pub rms_dbfs: f64,
    /// Integrated loudness, LUFS (BS.1770-4: K-weighting, 400 ms blocks
    /// stepped 100 ms, gated at -70 LUFS then 10 LU under the gated
    /// mean; a signal shorter than a block is one block). Each channel
    /// weighs 1, so a mono 997 Hz sine reads its RMS in dBFS.
    pub lufs: f64,
    /// The loudest single block, LUFS (momentary loudness's maximum).
    pub max_momentary_lufs: f64,
    pub frames: usize,
    pub rate: u32,
    pub channels: u16,
    pub duration_ms: u64,
    /// Samples at full scale (32767 or -32768; at or past ±1.0 for floats).
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

/// The levels of interleaved 16-bit `pcm` of `channels` channels at
/// `rate` Hz.
pub fn measure(pcm: &[i16], channels: u16, rate: u32) -> Levels {
    let ch = usize::from(channels.max(1));
    let pcm = &pcm[..pcm.len() / ch * ch];
    let clipped = pcm
        .iter()
        .filter(|&&s| s == i16::MAX || s == i16::MIN)
        .count();
    levels(
        pcm.iter().map(|&s| f64::from(s) / 32768.0),
        pcm.len(),
        channels,
        rate,
        clipped,
    )
}

/// The levels of interleaved float `samples` (full scale ±1.0) of
/// `channels` channels at `rate` Hz: a mixer's output.
pub fn measure_f32(samples: &[f32], channels: u16, rate: u32) -> Levels {
    let ch = usize::from(channels.max(1));
    let samples = &samples[..samples.len() / ch * ch];
    let clipped = samples.iter().filter(|s| s.abs() >= 1.0).count();
    levels(
        samples.iter().map(|&s| f64::from(s)),
        samples.len(),
        channels,
        rate,
        clipped,
    )
}

/// The levels of `len` interleaved samples, whole frames.
fn levels(
    samples: impl Iterator<Item = f64>,
    len: usize,
    channels: u16,
    rate: u32,
    clipped: usize,
) -> Levels {
    let ch = usize::from(channels.max(1));
    let frames = len / ch;
    let mut peak = 0f64;
    let mut sum_sq = 0f64;
    // Each frame's K-weighted power, summed over the channels (weight 1).
    let mut filters: Vec<[Biquad; 2]> = (0..ch).map(|_| k_weighting(rate)).collect();
    let mut power = Vec::with_capacity(frames);
    let mut frame_power = 0.0;
    for (i, v) in samples.enumerate() {
        peak = peak.max(v.abs());
        sum_sq += v * v;
        let c = i % ch;
        let [shelf, pass] = &mut filters[c];
        let y = pass.run(shelf.run(v));
        frame_power += y * y;
        if c + 1 == ch {
            power.push(frame_power);
            frame_power = 0.0;
        }
    }
    let (lufs, max_momentary_lufs) = gated(&power, rate);
    Levels {
        peak_dbfs: db(peak * peak),
        rms_dbfs: if len == 0 {
            SILENCE_DB
        } else {
            db(sum_sq / len as f64)
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

/// `v` to one decimal place, as the agent APIs give levels.
pub fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(amplitude: f64, hz: f64, rate: u32, seconds: f64, channels: usize) -> Vec<f32> {
        let frames = (f64::from(rate) * seconds) as usize;
        (0..frames)
            .flat_map(|i| {
                let v = amplitude * (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin();
                std::iter::repeat_n(v as f32, channels)
            })
            .collect()
    }

    fn near(a: f64, b: f64, within: f64) -> bool {
        (a - b).abs() <= within
    }

    #[test]
    fn a_float_sine_reads_its_peak_rms_and_loudness() {
        let mono = measure_f32(&sine(0.5, 997.0, 48_000, 2.0, 1), 1, 48_000);
        assert!(near(mono.peak_dbfs, -6.02, 0.03), "{mono:?}");
        assert!(near(mono.rms_dbfs, -9.03, 0.01), "{mono:?}");
        assert!(near(mono.lufs, -9.03, 0.1), "{mono:?}");
        assert_eq!(
            (mono.frames, mono.duration_ms, mono.clipped),
            (96_000, 2000, 0)
        );
        // Two equal channels: the same RMS, 3 dB more loudness.
        let stereo = measure_f32(&sine(0.5, 997.0, 48_000, 2.0, 2), 2, 48_000);
        assert!(near(stereo.rms_dbfs, mono.rms_dbfs, 0.01), "{stereo:?}");
        assert!(near(stereo.lufs, mono.lufs + 3.01, 0.05), "{stereo:?}");
    }

    #[test]
    fn floats_and_16_bit_samples_agree() {
        let f = sine(0.25, 440.0, 44_100, 1.0, 2);
        let i: Vec<i16> = f.iter().map(|&v| (v * 32768.0).round() as i16).collect();
        let (a, b) = (measure_f32(&f, 2, 44_100), measure(&i, 2, 44_100));
        assert!(near(a.peak_dbfs, b.peak_dbfs, 0.01), "{a:?} {b:?}");
        assert!(near(a.rms_dbfs, b.rms_dbfs, 0.01), "{a:?} {b:?}");
        assert!(near(a.lufs, b.lufs, 0.01), "{a:?} {b:?}");
        assert_eq!(a.frames, b.frames);
    }

    #[test]
    fn silence_a_partial_frame_and_full_scale() {
        let s = measure_f32(&[0.0; 960], 2, 48_000);
        assert_eq!(
            (s.peak_dbfs, s.rms_dbfs, s.lufs),
            (SILENCE_DB, SILENCE_DB, SILENCE_DB)
        );
        assert_eq!(s.frames, 480);
        // A trailing half frame is left out.
        assert_eq!(measure_f32(&[0.5, 0.5, 0.5], 2, 48_000).frames, 1);
        let hot = measure_f32(&[1.0, -1.0, 0.5, 1.2], 2, 48_000);
        assert_eq!(hot.clipped, 3);
        assert_eq!(round1(-6.0206), -6.0);
    }
}
