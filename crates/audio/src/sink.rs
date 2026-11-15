//! Device-independent host output plumbing: a lock-free ring buffer, the
//! [`AudioSink`] trait the game thread feeds, a headless [`NullSink`] and the
//! [`Pump`] that renders the [`Mixer`] ahead of the device to a latency target.
//!
//! Threading model: the game thread owns the `Mixer` outright and calls
//! [`Pump::pump`] once per frame (or from a dedicated audio-feed thread). The
//! device callback only drains the ring ([`Consumer::read`]); it never locks,
//! allocates or touches the mixer, so there is no priority inversion.
//!
//! All sample data is interleaved stereo `f32` at the sink's sample rate.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::Mixer;

/// Default latency target in milliseconds of queued audio.
pub const DEFAULT_TARGET_LATENCY_MS: u32 = 40;

struct Shared {
    /// `f32` bits. Length is a power of two, in samples (2 per frame).
    data: Box<[AtomicU32]>,
    mask: usize,
    /// Total samples ever written / read (monotonic, wrapping arithmetic).
    head: AtomicUsize,
    tail: AtomicUsize,
    primed: AtomicU64, // 0 until the producer first writes
    underruns: AtomicU64,
    underrun_frames: AtomicU64,
    played_frames: AtomicU64,
}

/// Counters read by the game thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SinkStats {
    /// Device callbacks that ran short of data after playback began.
    pub underruns: u64,
    /// Total frames of silence substituted for missing data.
    pub underrun_frames: u64,
    /// Frames handed to the device (data and silence).
    pub played_frames: u64,
}

/// Producer half (game thread). Not `Clone`: single producer.
pub struct Producer(Arc<Shared>);
/// Consumer half (device callback). Not `Clone`: single consumer.
pub struct Consumer(Arc<Shared>);

/// Creates a ring holding at least `min_frames` stereo frames.
pub fn ring(min_frames: usize) -> (Producer, Consumer) {
    let samples = (min_frames.max(1) * 2).next_power_of_two();
    let shared = Arc::new(Shared {
        data: (0..samples).map(|_| AtomicU32::new(0)).collect(),
        mask: samples - 1,
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        primed: AtomicU64::new(0),
        underruns: AtomicU64::new(0),
        underrun_frames: AtomicU64::new(0),
        played_frames: AtomicU64::new(0),
    });
    (Producer(Arc::clone(&shared)), Consumer(shared))
}

impl Producer {
    /// Ring capacity in frames.
    pub fn capacity_frames(&self) -> usize {
        (self.0.mask + 1) >> 1
    }

    /// Frames currently queued.
    pub fn queued_frames(&self) -> usize {
        let h = self.0.head.load(Ordering::Relaxed);
        let t = self.0.tail.load(Ordering::Acquire);
        h.wrapping_sub(t) / 2
    }

    pub fn free_frames(&self) -> usize {
        self.capacity_frames() - self.queued_frames()
    }

    /// Writes as many whole frames of `samples` as fit; returns frames written.
    pub fn write(&mut self, samples: &[f32]) -> usize {
        let s = &*self.0;
        let h = s.head.load(Ordering::Relaxed);
        let t = s.tail.load(Ordering::Acquire);
        let free = (s.mask + 1) - h.wrapping_sub(t);
        let n = samples.len().min(free) & !1;
        for (i, v) in samples[..n].iter().enumerate() {
            s.data[h.wrapping_add(i) & s.mask].store(v.to_bits(), Ordering::Relaxed);
        }
        if n > 0 {
            s.primed.store(1, Ordering::Relaxed);
        }
        s.head.store(h.wrapping_add(n), Ordering::Release);
        n / 2
    }

    pub fn stats(&self) -> SinkStats {
        self.0.stats()
    }
}

impl Shared {
    fn stats(&self) -> SinkStats {
        SinkStats {
            underruns: self.underruns.load(Ordering::Relaxed),
            underrun_frames: self.underrun_frames.load(Ordering::Relaxed),
            played_frames: self.played_frames.load(Ordering::Relaxed),
        }
    }
}

impl Consumer {
    /// Frames currently queued.
    pub fn queued_frames(&self) -> usize {
        let h = self.0.head.load(Ordering::Acquire);
        let t = self.0.tail.load(Ordering::Relaxed);
        h.wrapping_sub(t) / 2
    }

    /// Fills `out` (interleaved stereo) from the ring; any shortfall is
    /// silence. After the first write, a shortfall counts as one underrun.
    /// Real-time safe: no locks, no allocation.
    pub fn read(&mut self, out: &mut [f32]) {
        let s = &*self.0;
        let t = s.tail.load(Ordering::Relaxed);
        let h = s.head.load(Ordering::Acquire);
        let want = out.len() & !1;
        let n = want.min(h.wrapping_sub(t)) & !1;
        for (i, slot) in out[..n].iter_mut().enumerate() {
            *slot = f32::from_bits(s.data[t.wrapping_add(i) & s.mask].load(Ordering::Relaxed));
        }
        out[n..].fill(0.0);
        s.tail.store(t.wrapping_add(n), Ordering::Release);
        s.played_frames
            .fetch_add((out.len() / 2) as u64, Ordering::Relaxed);
        if n < want && s.primed.load(Ordering::Relaxed) != 0 {
            s.underruns.fetch_add(1, Ordering::Relaxed);
            s.underrun_frames
                .fetch_add(((want - n) / 2) as u64, Ordering::Relaxed);
        }
    }

    pub fn stats(&self) -> SinkStats {
        self.0.stats()
    }
}

/// Where the game thread sends mixed audio.
pub trait AudioSink {
    /// Current device rate in Hz. May change after a device change; the
    /// [`Pump`] follows it.
    fn sample_rate(&self) -> u32;
    /// Frames queued but not yet consumed by the device.
    fn queued_frames(&self) -> usize;
    /// Frames of room left.
    fn free_frames(&self) -> usize;
    /// Queues interleaved stereo samples; returns frames accepted.
    fn write(&mut self, samples: &[f32]) -> usize;
    fn stats(&self) -> SinkStats;
    /// Called once per pump so a sink can service device changes. Default: nothing.
    fn maintain(&mut self) {}
}

/// Headless sink for tests and CI. Time only advances when the test calls
/// [`NullSink::advance`], which plays the ring exactly like a device
/// callback would, keeping everything deterministic.
pub struct NullSink {
    rate: u32,
    prod: Producer,
    cons: Consumer,
    /// Everything "played", if capture is on.
    captured: Option<Vec<f32>>,
}

impl NullSink {
    pub fn new(rate: u32, capacity_frames: usize) -> Self {
        let (prod, cons) = ring(capacity_frames);
        Self {
            rate,
            prod,
            cons,
            captured: None,
        }
    }

    /// Keep every played sample (including substituted silence) for inspection.
    pub fn capture(mut self) -> Self {
        self.captured = Some(Vec::new());
        self
    }

    /// Plays `frames` frames as the device would.
    pub fn advance(&mut self, frames: usize) {
        let mut buf = vec![0.0; frames * 2];
        self.cons.read(&mut buf);
        if let Some(c) = &mut self.captured {
            c.extend_from_slice(&buf);
        }
    }

    pub fn captured(&self) -> &[f32] {
        self.captured.as_deref().unwrap_or(&[])
    }

    /// Changes the pretend device rate (simulates a device switch).
    pub fn set_rate(&mut self, rate: u32) {
        self.rate = rate;
    }
}

impl AudioSink for NullSink {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn queued_frames(&self) -> usize {
        self.prod.queued_frames()
    }
    fn free_frames(&self) -> usize {
        self.prod.free_frames()
    }
    fn write(&mut self, samples: &[f32]) -> usize {
        self.prod.write(samples)
    }
    fn stats(&self) -> SinkStats {
        self.prod.stats()
    }
}

/// Renders a [`Mixer`] into an [`AudioSink`], keeping the queue topped up to
/// a latency target.
pub struct Pump {
    target_frames_ms: u32,
    scratch: Vec<f32>,
}

impl Pump {
    pub fn new(target_latency_ms: u32) -> Self {
        Self {
            target_frames_ms: target_latency_ms.max(1),
            scratch: Vec::new(),
        }
    }

    pub fn target_latency_ms(&self) -> u32 {
        self.target_frames_ms
    }

    /// Renders enough audio to bring the queue up to the target, switching the
    /// mixer to the sink's rate first if the device changed. Returns frames
    /// rendered. Call at least every `target_latency_ms` (a 60 Hz game frame
    /// is fine with the default 40 ms).
    pub fn pump(&mut self, mixer: &mut Mixer, sink: &mut dyn AudioSink) -> usize {
        sink.maintain();
        let rate = sink.sample_rate();
        if mixer.sample_rate() != rate {
            mixer.set_sample_rate(rate);
        }
        let target = (u64::from(rate) * u64::from(self.target_frames_ms) / 1000) as usize;
        let want = target
            .saturating_sub(sink.queued_frames())
            .min(sink.free_frames());
        if want == 0 {
            return 0;
        }
        self.scratch.resize(want * 2, 0.0);
        mixer.render(&mut self.scratch);
        // Clamp here so every sink gets device-ready samples.
        for v in &mut self.scratch {
            *v = v.clamp(-1.0, 1.0);
        }
        sink.write(&self.scratch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SoundBuffer, WaveFormat};

    #[test]
    fn ring_roundtrip_and_wrap() {
        let (mut p, mut c) = ring(4); // 8 samples
        assert_eq!(p.capacity_frames(), 4);
        for round in 0..10 {
            let v = round as f32;
            assert_eq!(p.write(&[v, -v, v + 0.5, -v - 0.5, v, v, 0.0]), 3); // odd tail dropped
            let mut out = [9.0; 6];
            c.read(&mut out);
            assert_eq!(out, [v, -v, v + 0.5, -v - 0.5, v, v]);
        }
        assert_eq!(c.stats().underruns, 0);
    }

    #[test]
    fn full_ring_rejects() {
        let (mut p, mut c) = ring(2);
        assert_eq!(p.write(&[1.0; 8]), 2);
        assert_eq!(p.free_frames(), 0);
        assert_eq!(p.write(&[1.0; 2]), 0);
        let mut out = [0.0; 2];
        c.read(&mut out);
        assert_eq!(p.free_frames(), 1);
    }

    #[test]
    fn underrun_fills_silence_and_counts() {
        let (mut p, mut c) = ring(16);
        let mut out = [5.0; 8];
        c.read(&mut out); // before first write: silence, not counted
        assert_eq!(out, [0.0; 8]);
        assert_eq!(c.stats().underruns, 0);
        p.write(&[0.25; 4]);
        let mut out = [5.0; 8];
        c.read(&mut out);
        assert_eq!(out, [0.25, 0.25, 0.25, 0.25, 0.0, 0.0, 0.0, 0.0]);
        let s = c.stats();
        assert_eq!((s.underruns, s.underrun_frames), (1, 2));
        let mut out = [5.0; 4];
        c.read(&mut out);
        assert_eq!(c.stats().underruns, 2);
        assert_eq!(c.stats().played_frames, 4 + 4 + 2);
    }

    #[test]
    fn threaded_stream_is_lossless_and_ordered() {
        let (mut p, mut c) = ring(64);
        let total = 200_000usize;
        let prod = std::thread::spawn(move || {
            let mut i = 0usize;
            while i < total {
                let n = 16.min(total - i);
                let chunk: Vec<f32> = (i..i + n).flat_map(|k| [k as f32, -(k as f32)]).collect();
                let w = p.write(&chunk);
                i += w;
                if w == 0 {
                    std::thread::yield_now();
                }
            }
        });
        let mut next = 0usize;
        let mut buf = [0.0f32; 24];
        while next < total {
            let q = c.queued_frames().min(12);
            if q == 0 {
                std::thread::yield_now();
                continue;
            }
            c.read(&mut buf[..q * 2]);
            for f in 0..q {
                assert_eq!(buf[f * 2], next as f32);
                assert_eq!(buf[f * 2 + 1], -(next as f32));
                next += 1;
            }
        }
        prod.join().unwrap();
    }

    fn tone_mixer(rate: u32) -> Mixer {
        let mut m = Mixer::new(rate);
        let bytes: Vec<u8> = (0..rate).flat_map(|_| 8192i16.to_le_bytes()).collect();
        let id = m.add_buffer(
            SoundBuffer::from_data(WaveFormat::pcm16(1, rate).unwrap(), bytes).unwrap(),
        );
        m.buffer_mut(id).unwrap().play(true);
        m
    }

    #[test]
    fn pump_holds_latency_target_without_underruns() {
        let mut m = tone_mixer(48_000);
        let mut sink = NullSink::new(48_000, 8192);
        let mut pump = Pump::new(40);
        // 60 Hz game frames; the device consumes 800 frames per tick.
        let mut max_q = 0;
        for _ in 0..600 {
            pump.pump(&mut m, &mut sink);
            max_q = max_q.max(sink.queued_frames());
            sink.advance(800);
        }
        assert_eq!(max_q, 1920); // 40 ms at 48 kHz
        let s = sink.stats();
        assert_eq!(s.underruns, 0);
        assert_eq!(s.played_frames, 600 * 800);
    }

    #[test]
    fn stalled_game_thread_underruns_then_recovers() {
        let mut m = tone_mixer(48_000);
        let mut sink = NullSink::new(48_000, 8192).capture();
        let mut pump = Pump::new(40);
        pump.pump(&mut m, &mut sink);
        sink.advance(1920); // drains the queue exactly
        assert_eq!(sink.stats().underruns, 0);
        sink.advance(480); // game stalled
        assert_eq!(sink.stats().underruns, 1);
        assert_eq!(sink.stats().underrun_frames, 480);
        pump.pump(&mut m, &mut sink);
        sink.advance(100);
        let c = sink.captured();
        assert!(c[1920 * 2..(1920 + 480) * 2].iter().all(|&v| v == 0.0));
        assert!(c[(1920 + 480) * 2] > 0.0);
    }

    #[test]
    fn pump_follows_device_rate_change() {
        let mut m = tone_mixer(48_000);
        let mut sink = NullSink::new(48_000, 8192);
        let mut pump = Pump::new(40);
        pump.pump(&mut m, &mut sink);
        sink.advance(1920);
        sink.set_rate(44_100);
        pump.pump(&mut m, &mut sink);
        assert_eq!(m.sample_rate(), 44_100);
        assert_eq!(sink.queued_frames(), 1764);
    }

    #[test]
    fn pump_clamps_output() {
        let mut m = Mixer::new(8000);
        for _ in 0..4 {
            let bytes: Vec<u8> = (0..8000).flat_map(|_| 30000i16.to_le_bytes()).collect();
            let id = m.add_buffer(
                SoundBuffer::from_data(WaveFormat::pcm16(1, 8000).unwrap(), bytes).unwrap(),
            );
            m.buffer_mut(id).unwrap().play(true);
        }
        let mut sink = NullSink::new(8000, 1024).capture();
        Pump::new(40).pump(&mut m, &mut sink);
        sink.advance(320);
        assert!(sink.captured().iter().all(|&v| v == 1.0));
    }
}
