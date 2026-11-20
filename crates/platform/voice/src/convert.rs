//! Device-side conversion for [`crate::host`]: any device rate and channel
//! count to and from the 48 kHz mono the media objects use. Kept apart
//! from the device code so it is tested without hardware.

use std::collections::VecDeque;

/// Streaming linear rate conversion of mono samples. The position carries
/// across calls, so a stream cut into any block sizes gives the same
/// output as one long block.
#[derive(Clone, Debug)]
pub struct RateConverter {
    from: u64,
    to: u64,
    /// Position of the next output sample in units of 1/`to` input
    /// samples after `last`, so it never drifts.
    pos: u64,
    last: f32,
}

impl RateConverter {
    pub fn new(from: u32, to: u32) -> Self {
        assert!(from > 0 && to > 0);
        Self {
            from: u64::from(from),
            to: u64::from(to),
            pos: 0,
            last: 0.0,
        }
    }

    /// Converts `input`, calling `out` for each output sample.
    pub fn run(&mut self, input: &[f32], mut out: impl FnMut(f32)) {
        // Position 0 is `last`; input[i] is at i + 1.
        let end = input.len() as u64 * self.to;
        while self.pos < end {
            let i = (self.pos / self.to) as usize;
            let t = (self.pos % self.to) as f32 / self.to as f32;
            let a = if i == 0 { self.last } else { input[i - 1] };
            let b = input[i];
            out(a + (b - a) * t);
            self.pos += self.from;
        }
        if let Some(&l) = input.last() {
            self.last = l;
        }
        self.pos -= end;
    }
}

/// The mean of each interleaved frame's channels.
pub fn downmix(data: &[f32], channels: usize, mut out: impl FnMut(f32)) {
    for frame in data.chunks_exact(channels.max(1)) {
        out(frame.iter().sum::<f32>() / frame.len() as f32);
    }
}

/// A bounded queue of 48 kHz samples between a device thread and the
/// guest tick. Past `cap` the oldest are dropped, so a stalled reader
/// never builds latency past `cap`.
#[derive(Debug)]
pub struct Fifo {
    samples: VecDeque<i16>,
    cap: usize,
}

impl Fifo {
    pub fn new(cap: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(cap),
            cap,
        }
    }

    pub fn push(&mut self, s: f32) {
        if self.samples.len() == self.cap {
            self.samples.pop_front();
        }
        self.samples
            .push_back((s * 32768.0).clamp(-32768.0, 32767.0) as i16);
    }

    pub fn push_i16(&mut self, s: i16) {
        if self.samples.len() == self.cap {
            self.samples.pop_front();
        }
        self.samples.push_back(s);
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Fill `out` if there are enough samples; false (and `out` untouched)
    /// if not.
    pub fn take_exact(&mut self, out: &mut [i16]) -> bool {
        if self.samples.len() < out.len() {
            return false;
        }
        let n = out.len();
        for (o, s) in out.iter_mut().zip(self.samples.drain(..n)) {
            *o = s;
        }
        true
    }

    pub fn pop(&mut self) -> Option<i16> {
        self.samples.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_is_one_sample_late_and_exact() {
        let mut c = RateConverter::new(48_000, 48_000);
        let mut out = Vec::new();
        c.run(&[1.0, 2.0, 3.0], |s| out.push(s));
        assert_eq!(out, [0.0, 1.0, 2.0]);
        c.run(&[4.0], |s| out.push(s));
        assert_eq!(out, [0.0, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn block_sizes_do_not_change_the_output() {
        let input: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin()).collect();
        for (from, to) in [
            (44_100, 48_000),
            (48_000, 44_100),
            (16_000, 48_000),
            (96_000, 48_000),
        ] {
            let mut whole = Vec::new();
            RateConverter::new(from, to).run(&input, |s| whole.push(s));
            let mut parts = Vec::new();
            let mut c = RateConverter::new(from, to);
            for chunk in input.chunks(37) {
                c.run(chunk, |s| parts.push(s));
            }
            assert_eq!(whole.len(), parts.len(), "{from} -> {to}");
            for (a, b) in whole.iter().zip(&parts) {
                assert!((a - b).abs() < 1e-5, "{from} -> {to}");
            }
            let expected = input.len() as f64 * f64::from(to) / f64::from(from);
            assert!(
                (whole.len() as f64 - expected).abs() <= 1.0,
                "{from} -> {to}"
            );
        }
    }

    #[test]
    fn downmix_averages_channels() {
        let mut out = Vec::new();
        downmix(&[1.0, 0.0, 0.5, 0.5], 2, |s| out.push(s));
        assert_eq!(out, [0.5, 0.5]);
    }

    #[test]
    fn the_fifo_drops_the_oldest_past_its_cap() {
        let mut f = Fifo::new(3);
        for s in [1, 2, 3, 4] {
            f.push_i16(s);
        }
        let mut out = [0; 3];
        assert!(f.take_exact(&mut out));
        assert_eq!(out, [2, 3, 4]);
        assert!(!f.take_exact(&mut out));
        f.push(1.0);
        assert_eq!(f.pop(), Some(32767));
    }
}
