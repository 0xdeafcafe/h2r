//! Linear-interpolation sample-rate conversion.
//!
//! Linear is what the mixer uses per voice: cheap, bounded, and good enough
//! for game audio where voices are pitch-shifted constantly anyway. It does
//! not band-limit, so large downsamples alias. A windowed-sinc path can slot
//! in behind the same signatures later.

/// Reads the frame at fractional position `pos` (in frames) from interleaved
/// `src`, writing `channels` values to `out`. Positions past the end clamp to
/// the last frame.
#[inline]
pub fn lerp_frame(src: &[f32], channels: usize, pos: f64, out: &mut [f32]) {
    let frames = src.len() / channels;
    debug_assert!(frames > 0);
    let i = pos.floor() as usize;
    let t = (pos - i as f64) as f32;
    let a = i.min(frames - 1);
    let b = (i + 1).min(frames - 1);
    for c in 0..channels {
        let x = src[a * channels + c];
        let y = src[b * channels + c];
        out[c] = x + (y - x) * t;
    }
}

/// Resamples a whole interleaved clip from `from` Hz to `to` Hz.
pub fn resample_linear(src: &[f32], channels: usize, from: u32, to: u32) -> Vec<f32> {
    assert!(channels > 0 && from > 0 && to > 0);
    let frames = src.len() / channels;
    if frames == 0 || from == to {
        return src.to_vec();
    }
    let out_frames = (frames as u64 * to as u64 / from as u64) as usize;
    let step = from as f64 / to as f64;
    let mut out = vec![0.0; out_frames * channels];
    for (n, frame) in out.chunks_exact_mut(channels).enumerate() {
        lerp_frame(src, channels, n as f64 * step, frame);
    }
    out
}

/// Streaming resampler: feed chunks, get converted chunks, with continuity
/// across chunk boundaries.
#[derive(Clone, Debug)]
pub struct LinearResampler {
    channels: usize,
    step: f64,
    /// Position of the next output frame relative to `carry`'s first frame.
    pos: f64,
    /// Unconsumed input frames kept between calls.
    carry: Vec<f32>,
}

impl LinearResampler {
    pub fn new(channels: usize, from: u32, to: u32) -> Self {
        assert!(channels > 0 && from > 0 && to > 0);
        Self { channels, step: from as f64 / to as f64, pos: 0.0, carry: Vec::new() }
    }

    /// Input frames per output frame (`from / to` at construction).
    pub fn step(&self) -> f64 {
        self.step
    }

    /// Changes the conversion ratio (input frames per output frame) from the
    /// next output frame on, keeping the position: for a consumer that trims
    /// the rate a fraction of a percent to hold a buffer level.
    pub fn set_step(&mut self, step: f64) {
        assert!(step.is_finite() && step > 0.0);
        self.step = step;
    }

    /// Appends converted output for `input` to `out`. Output frames are only
    /// produced once the interpolation has both neighbours, so the last input
    /// frame is held back until the next call.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.carry.extend_from_slice(input);
        let ch = self.channels;
        let frames = self.carry.len() / ch;
        let mut frame = vec![0.0; ch];
        while frames > 0 && self.pos + 1.0 < frames as f64 {
            lerp_frame(&self.carry, ch, self.pos, &mut frame);
            out.extend_from_slice(&frame);
            self.pos += self.step;
        }
        let drop = (self.pos.floor() as usize).min(frames.saturating_sub(1));
        self.carry.drain(..drop * ch);
        self.pos -= drop as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| i as f32).collect()
    }

    #[test]
    fn identity_rate_is_identity() {
        let s = ramp(10);
        assert_eq!(resample_linear(&s, 1, 48000, 48000), s);
    }

    #[test]
    fn upsample_2x_interpolates_midpoints() {
        let out = resample_linear(&ramp(4), 1, 1000, 2000);
        assert_eq!(out.len(), 8);
        assert_eq!(&out[..6], &[0.0, 0.5, 1.0, 1.5, 2.0, 2.5]);
        // The tail clamps to the last frame instead of reading out of range.
        assert_eq!(out[7], 3.0);
    }

    #[test]
    fn downsample_picks_every_other() {
        let out = resample_linear(&ramp(8), 1, 2000, 1000);
        assert_eq!(out, vec![0.0, 2.0, 4.0, 6.0]);
    }

    #[test]
    fn stereo_channels_stay_separate() {
        // L ramps up, R constant.
        let src = [0.0, 1.0, 2.0, 1.0, 4.0, 1.0];
        let out = resample_linear(&src, 2, 1, 2);
        assert_eq!(&out[..4], &[0.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn streaming_matches_whole_clip() {
        let src = ramp(100);
        let whole = resample_linear(&src, 1, 3, 5);
        let mut r = LinearResampler::new(1, 3, 5);
        let mut out = Vec::new();
        for chunk in src.chunks(7) {
            r.process(chunk, &mut out);
        }
        // Streaming holds the last frame back, so it is a prefix.
        assert!(out.len() + 2 >= whole.len());
        for (a, b) in out.iter().zip(&whole) {
            assert!((a - b).abs() < 1e-4, "{a} vs {b}");
        }
    }

    #[test]
    fn set_step_changes_the_ratio_without_a_jump() {
        let mut r = LinearResampler::new(1, 1, 1);
        let mut out = Vec::new();
        r.process(&ramp(100), &mut out);
        let before = out.len();
        r.set_step(1.01);
        r.process(&(100..1100).map(|i| i as f32).collect::<Vec<_>>(), &mut out);
        // A ramp stays a ramp: steps of 1, then 1.01, never a jump.
        for w in out.windows(2) {
            let d = w[1] - w[0];
            assert!((0.99..=1.02).contains(&d), "{d}");
        }
        // 1% faster: 1% fewer output frames for the next thousand.
        assert_eq!(before, 99, "the last frame is held back");
        assert_eq!(out.len() - before, 991);
    }
}
