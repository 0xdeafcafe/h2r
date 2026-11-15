//! Host audio output through `cpal` (CoreAudio on macOS).
//!
//! This is the only module that touches a device. Everything else in the
//! crate is device-agnostic, so a tvOS or Xbox port replaces this file (or
//! disables the `device` feature) and calls [`Mixer::render`] itself from
//! whatever callback the platform provides. cpal also targets iOS/tvOS
//! CoreAudio, so this module may work there unchanged; that is untested.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::{
    FromSample, Sample, SampleFormat, SizedSample, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use crate::Mixer;
use crate::sink::{AudioSink, Consumer, Producer, SinkStats, ring};

/// Errors opening or running the host stream.
#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("no default audio output device")]
    NoDevice,
    #[error("unsupported device sample format {0}")]
    UnsupportedFormat(SampleFormat),
    #[error("audio device error: {0}")]
    Device(String),
}

fn dev<E: std::fmt::Display>(e: E) -> OutputError {
    OutputError::Device(e.to_string())
}

/// A running output stream pulling from a shared [`Mixer`]. Dropping it stops playback.
pub struct Output {
    _stream: Stream,
    mixer: Arc<Mutex<Mixer>>,
    sample_rate: u32,
}

impl Output {
    /// Opens the default device and starts a stream. The returned mixer is
    /// created at the device's rate, so no further rate conversion happens.
    ///
    /// The audio callback uses `try_lock`: if the game thread holds the mixer
    /// at that instant the callback emits silence for that period rather than
    /// blocking the audio thread. Keep critical sections short.
    pub fn start_default() -> Result<Self, OutputError> {
        let device = cpal::default_host().default_output_device().ok_or(OutputError::NoDevice)?;
        let config = device.default_output_config().map_err(dev)?;
        let sample_rate = config.sample_rate();
        let mixer = Arc::new(Mutex::new(Mixer::new(sample_rate)));
        let format = config.sample_format();
        let cfg = config.into();
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, cfg, &mixer)?,
            SampleFormat::I16 => build::<i16>(&device, cfg, &mixer)?,
            SampleFormat::I32 => build::<i32>(&device, cfg, &mixer)?,
            SampleFormat::U16 => build::<u16>(&device, cfg, &mixer)?,
            other => return Err(OutputError::UnsupportedFormat(other)),
        };
        stream.play().map_err(dev)?;
        Ok(Self { _stream: stream, mixer, sample_rate })
    }

    /// The mixer the stream renders from. Lock it briefly to create or
    /// control voices.
    pub fn mixer(&self) -> &Arc<Mutex<Mixer>> {
        &self.mixer
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

/// Latency figures for a [`DeviceSink`], averaged since it was opened.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LatencyReport {
    /// Mean audio queued in our ring at callback time, in ms.
    pub ring_ms: f64,
    /// Peak queued in the ring, in ms.
    pub ring_peak_ms: f64,
    /// Mean callback-to-DAC delay the driver reports (its own buffering), in ms.
    pub device_ms: f64,
    /// `ring_ms + device_ms`: expected write-to-speaker delay.
    pub total_ms: f64,
    pub callbacks: u64,
}

#[derive(Default)]
struct Meter {
    callbacks: AtomicU64,
    ring_frames_sum: AtomicU64,
    ring_frames_peak: AtomicU64,
    device_us_sum: AtomicU64,
    lost: AtomicBool,
}

/// Lock-free host output: a cpal stream whose callback only drains a ring.
/// The game thread keeps the [`Mixer`] and feeds this through
/// [`Pump`](crate::Pump). This is the API `boot` should use.
///
/// Device changes: a stream error (device unplugged, format changed) marks the
/// sink lost; [`AudioSink::maintain`] (called by every pump) then reopens the
/// default device. It also polls the default device id about twice a second,
/// so switching the system output follows even when the old device stays
/// alive. A reopen can change the sample rate; [`Pump`](crate::Pump) notices
/// and calls [`Mixer::set_sample_rate`], and the ring is simply recreated
/// (a few ms of queued audio is dropped). Counters survive reopens.
///
/// The same poll also compares the default device's nominal sample rate with
/// the one the stream was opened at, so changing the rate in Audio MIDI Setup
/// on the same device reopens it too. This is a poll (worst case ~0.5 s of
/// stale rate), not a CoreAudio property listener
/// (`kAudioDevicePropertyNominalSampleRate`); a listener would react at once
/// but needs a callback thread and extra FFI. What cannot be tested without
/// hardware: that CoreAudio really reports the new rate through cpal's
/// `default_output_config`, and that the old stream errors out or keeps
/// running at the old rate. The decision logic and the reopen path are tested
/// through injected probes and openers.
pub struct DeviceSink {
    stream: Option<StreamHandle>,
    prod: Producer,
    meter: Arc<Meter>,
    rate: u32,
    device_id: String,
    ring_frames: usize,
    last_check: Instant,
    reopen_failed_at: Option<Instant>,
    base_stats: SinkStats,
    reopens: u32,
    open: OpenFn,
    probe: ProbeFn,
    /// A device-name pattern ([`crate::devices::pick_controller`]), or
    /// `None` to follow the system default output.
    target: Option<String>,
}

/// Keeps a running stream alive; dropping it stops playback. Boxed so tests
/// can stand in for a device.
type StreamHandle = Box<dyn std::any::Any>;
type Opened = (StreamHandle, Producer, Arc<Meter>, u32, String);
type OpenFn = fn(usize, Option<&str>) -> Result<Opened, OutputError>;
/// The target device's id and nominal rate (the default output when the
/// target is `None`), or `None` if unknown or absent.
type ProbeFn = fn(Option<&str>) -> Option<(String, u32)>;

impl DeviceSink {
    /// Ring size in frames: ~0.34 s at 48 kHz, plenty over the latency target.
    const RING_FRAMES: usize = 16384;

    pub fn open_default() -> Result<Self, OutputError> {
        Self::open_with(open_stream, probe_default, None)
    }

    /// The first output device whose name matches `pattern` (case-insensitive
    /// substring; `auto` tries the PlayStation controller names in
    /// [`crate::devices::CONTROLLER_NAMES`]). The sink stays on that device:
    /// it never falls back to the default output, and if the device goes away
    /// it retries once a second until it is back.
    pub fn open_named(pattern: &str) -> Result<Self, OutputError> {
        Self::open_with(open_stream, probe_default, Some(pattern.to_string()))
    }

    /// Output device names, in the host's order (for `--list-audio-devices`
    /// style diagnostics and the controller-speaker setting).
    pub fn output_device_names() -> Vec<String> {
        named_outputs().into_iter().map(|(name, _)| name).collect()
    }

    fn open_with(open: OpenFn, probe: ProbeFn, target: Option<String>) -> Result<Self, OutputError> {
        let (stream, prod, meter, rate, device_id) = open(Self::RING_FRAMES, target.as_deref())?;
        Ok(Self {
            stream: Some(stream),
            prod,
            meter,
            rate,
            device_id,
            ring_frames: Self::RING_FRAMES,
            last_check: Instant::now(),
            reopen_failed_at: None,
            base_stats: SinkStats::default(),
            reopens: 0,
            open,
            probe,
            target,
        })
    }

    /// Times the stream was rebuilt after a device or format change.
    pub fn reopen_count(&self) -> u32 {
        self.reopens
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn latency(&self) -> LatencyReport {
        let n = self.meter.callbacks.load(Ordering::Relaxed);
        if n == 0 {
            return LatencyReport::default();
        }
        let to_ms = |frames: f64| frames * 1000.0 / f64::from(self.rate);
        let ring_ms = to_ms(self.meter.ring_frames_sum.load(Ordering::Relaxed) as f64 / n as f64);
        let device_ms = self.meter.device_us_sum.load(Ordering::Relaxed) as f64 / n as f64 / 1000.0;
        LatencyReport {
            ring_ms,
            ring_peak_ms: to_ms(self.meter.ring_frames_peak.load(Ordering::Relaxed) as f64),
            device_ms,
            total_ms: ring_ms + device_ms,
            callbacks: n,
        }
    }

    fn reopen(&mut self) {
        // Stop the old stream first so its callback can no longer touch the
        // ring, then fold its counters into the baseline exactly once and
        // swap in a fresh (zeroed) ring. If the open below fails, `stats()`
        // is still base + zero rather than counting the old ring twice.
        if self.stream.take().is_some() {
            self.base_stats = self.stats();
            self.prod = ring(self.ring_frames).0;
        }
        match (self.open)(self.ring_frames, self.target.as_deref()) {
            Ok((stream, prod, meter, rate, id)) => {
                self.stream = Some(stream);
                self.prod = prod;
                self.meter = meter;
                self.rate = rate;
                self.device_id = id;
                self.reopens += 1;
                self.reopen_failed_at = None;
            }
            Err(_) => self.reopen_failed_at = Some(Instant::now()),
        }
    }
}

/// Whether the default output now differs from the one we opened.
fn device_changed(now: Option<&(String, u32)>, have_id: &str, have_rate: u32) -> bool {
    now.is_some_and(|(id, rate)| id != have_id || *rate != have_rate)
}

impl AudioSink for DeviceSink {
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
        let s = self.prod.stats();
        SinkStats {
            underruns: self.base_stats.underruns + s.underruns,
            underrun_frames: self.base_stats.underrun_frames + s.underrun_frames,
            played_frames: self.base_stats.played_frames + s.played_frames,
        }
    }
    fn maintain(&mut self) {
        let now = Instant::now();
        if self.stream.is_none() || self.meter.lost.load(Ordering::Relaxed) {
            // No device (or a dead one): retry at most once a second.
            if self.reopen_failed_at.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(1)) {
                self.reopen();
            }
            return;
        }
        if now.duration_since(self.last_check) >= Duration::from_millis(500) {
            self.last_check = now;
            let current = (self.probe)(self.target.as_deref());
            if device_changed(current.as_ref(), &self.device_id, self.rate) {
                self.reopen();
            }
        }
    }
}

/// Every output device with its name.
fn named_outputs() -> Vec<(String, cpal::Device)> {
    let Ok(devices) = cpal::default_host().output_devices() else {
        return Vec::new();
    };
    devices.filter_map(|d| Some((d.description().ok()?.name().to_string(), d))).collect()
}

/// The target device: the default output, or the first whose name matches.
fn target_device(target: Option<&str>) -> Option<cpal::Device> {
    match target {
        None => cpal::default_host().default_output_device(),
        Some(pattern) => {
            let mut all = named_outputs();
            let names: Vec<&str> = all.iter().map(|(n, _)| n.as_str()).collect();
            let i = crate::devices::pick_controller(&names, pattern)?;
            Some(all.swap_remove(i).1)
        }
    }
}

fn probe_default(target: Option<&str>) -> Option<(String, u32)> {
    let device = target_device(target)?;
    let id = device.id().ok()?.to_string();
    let rate = device.default_output_config().ok()?.sample_rate();
    Some((id, rate))
}

fn open_stream(ring_frames: usize, target: Option<&str>) -> Result<Opened, OutputError> {
    let device = target_device(target).ok_or(OutputError::NoDevice)?;
    let config = device.default_output_config().map_err(dev)?;
    let rate = config.sample_rate();
    let format = config.sample_format();
    let id = device.id().map(|i| i.to_string()).unwrap_or_default();
    let (prod, cons) = ring(ring_frames);
    let meter = Arc::new(Meter::default());
    let cfg = config.into();
    let stream: Stream = match format {
        SampleFormat::F32 => build_ring::<f32>(&device, cfg, cons, &meter)?,
        SampleFormat::I16 => build_ring::<i16>(&device, cfg, cons, &meter)?,
        SampleFormat::I32 => build_ring::<i32>(&device, cfg, cons, &meter)?,
        SampleFormat::U16 => build_ring::<u16>(&device, cfg, cons, &meter)?,
        other => return Err(OutputError::UnsupportedFormat(other)),
    };
    stream.play().map_err(dev)?;
    Ok((Box::new(stream), prod, meter, rate, id))
}

/// Frames the output callbacks process per step. A device buffer larger than
/// this is handled in several steps over the same fixed scratch.
const CHUNK_FRAMES: usize = 4096;

/// Real-time body of the ring callback: no allocation whatever the buffer
/// size. Returns the frames that were queued when it started.
fn fill_from_ring<T: Sample + FromSample<f32>>(
    data: &mut [T],
    channels: usize,
    cons: &mut Consumer,
    scratch: &mut [f32],
) -> u64 {
    let queued = cons.queued_frames() as u64;
    let frames = data.len() / channels;
    let mut out = data.chunks_mut(CHUNK_FRAMES.min(scratch.len() / 2) * channels);
    cons.drain(frames, scratch, |mix| {
        if let Some(dst) = out.next() {
            write_frames(mix, dst, channels);
        }
    });
    queued
}

/// Body of the mixer callback (`try_lock`, silence on contention).
fn fill_from_mixer<T: Sample + FromSample<f32>>(
    data: &mut [T],
    channels: usize,
    mixer: &Mutex<Mixer>,
    scratch: &mut [f32],
) {
    let step = CHUNK_FRAMES.min(scratch.len() / 2);
    for dst in data.chunks_mut(step * channels) {
        let mix = &mut scratch[..dst.len() / channels * 2];
        match mixer.try_lock() {
            Ok(mut m) => m.render(mix),
            Err(_) => mix.fill(0.0),
        }
        write_frames(mix, dst, channels);
    }
}

fn build_ring<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut cons: Consumer,
    meter: &Arc<Meter>,
) -> Result<Stream, OutputError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let m = Arc::clone(meter);
    let m_err = Arc::clone(meter);
    // Fixed size: the callback works through `CHUNK_FRAMES` at a time and
    // never allocates, however large the device buffer is.
    let mut scratch = vec![0.0f32; CHUNK_FRAMES * 2].into_boxed_slice();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let queued = fill_from_ring(data, channels, &mut cons, &mut scratch);
                let ts = info.timestamp();
                let dev_us = ts.playback.duration_since(ts.callback).as_micros() as u64;
                m.callbacks.fetch_add(1, Ordering::Relaxed);
                m.ring_frames_sum.fetch_add(queued, Ordering::Relaxed);
                m.ring_frames_peak.fetch_max(queued, Ordering::Relaxed);
                m.device_us_sum.fetch_add(dev_us, Ordering::Relaxed);
            },
            move |_e| m_err.lost.store(true, Ordering::Relaxed),
            None,
        )
        .map_err(dev)
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mixer: &Arc<Mutex<Mixer>>,
) -> Result<Stream, OutputError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mixer = Arc::clone(mixer);
    let mut scratch = vec![0.0f32; CHUNK_FRAMES * 2].into_boxed_slice();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                fill_from_mixer(data, channels, &mixer, &mut scratch);
            },
            |e| eprintln!("audio stream error: {e}"),
            None,
        )
        .map_err(dev)
}

/// Maps interleaved stereo `mix` onto a device with `channels` channels,
/// clamping to -1..1. Mono devices get the average; channels past the first
/// two are silent.
pub fn write_frames<T: Sample + FromSample<f32>>(mix: &[f32], out: &mut [T], channels: usize) {
    for (src, dst) in mix.as_chunks::<2>().0.iter().zip(out.chunks_exact_mut(channels)) {
        let (l, r) = (src[0].clamp(-1.0, 1.0), src[1].clamp(-1.0, 1.0));
        for (c, slot) in dst.iter_mut().enumerate() {
            let v = match (channels, c) {
                (1, _) => (l + r) * 0.5,
                (_, 0) => l,
                (_, 1) => r,
                _ => 0.0,
            };
            *slot = T::from_sample(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_mapping() {
        let mix = [0.5, -0.5, 2.0, 0.0];
        let mut mono = [0.0f32; 2];
        write_frames(&mix, &mut mono, 1);
        assert_eq!(mono, [0.0, 0.5]);
        let mut six = [9.0f32; 12];
        write_frames(&mix, &mut six, 6);
        assert_eq!(&six[..6], &[0.5, -0.5, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(six[6], 1.0); // clamped
        let mut i16s = [0i16; 2];
        write_frames(&mix[..2], &mut i16s, 2);
        assert!(i16s[0] > 16000 && i16s[1] < -16000);
    }

    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    /// Counts allocations made by the current thread so a test can assert
    /// that a code path makes none.
    struct Counting;
    thread_local! {
        static ALLOCS: Cell<usize> = const { Cell::new(0) };
    }
    // SAFETY: forwards to the system allocator unchanged.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
            unsafe { System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            unsafe { System.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
            unsafe { System.realloc(p, l, n) }
        }
    }
    #[global_allocator]
    static A: Counting = Counting;

    fn allocs() -> usize {
        ALLOCS.with(Cell::get)
    }

    #[test]
    fn ring_callback_never_allocates_even_for_huge_buffers() {
        let (mut prod, mut cons) = ring(16384);
        let mut scratch = vec![0.0f32; CHUNK_FRAMES * 2].into_boxed_slice();
        let (ptr, len) = (scratch.as_ptr(), scratch.len());
        let mut i16s = vec![0i16; 3 * CHUNK_FRAMES * 2 + 6]; // 3+ chunks, stereo, ragged tail
        let mut f32s = vec![0.0f32; 20_000 * 6];
        prod.write(&vec![0.25f32; 2 * 9000]);
        let before = allocs();
        let q = fill_from_ring(&mut i16s, 2, &mut cons, &mut scratch);
        fill_from_ring(&mut f32s, 6, &mut cons, &mut scratch); // 20k frames > the old 8192 limit
        assert_eq!(allocs(), before, "the callback allocated");
        assert_eq!(q, 9000);
        assert_eq!((scratch.as_ptr(), scratch.len()), (ptr, len));
        // Data came out in order, then silence, across chunk boundaries.
        assert!(i16s[..9000 * 2].iter().take(3 * CHUNK_FRAMES * 2).all(|&v| v > 0));
    }

    #[test]
    fn mixer_callback_never_allocates_in_the_copy_path() {
        let mixer = Mutex::new(Mixer::new(48_000));
        let mut scratch = vec![0.0f32; CHUNK_FRAMES * 2].into_boxed_slice();
        let mut out = vec![1.0f32; 10_000 * 2];
        let guard = mixer.lock().unwrap();
        // Contended: the callback falls back to silence without allocating.
        let before = allocs();
        fill_from_mixer(&mut out, 2, &mixer, &mut scratch);
        assert_eq!(allocs(), before);
        drop(guard);
        assert!(out.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn chunked_ring_callback_matches_one_shot_read() {
        let (mut p1, mut c1) = ring(16384);
        let (mut p2, mut c2) = ring(16384);
        let data: Vec<f32> = (0..2 * 5000).map(|v| (v as f32 * 0.001).sin() * 0.5).collect();
        p1.write(&data);
        p2.write(&data);
        let mut scratch = vec![0.0f32; CHUNK_FRAMES * 2].into_boxed_slice();
        let mut got = vec![0.0f32; 7000 * 2];
        fill_from_ring(&mut got, 2, &mut c1, &mut scratch);
        let mut want = vec![0.0f32; 7000 * 2];
        c2.read(&mut want);
        assert_eq!(got, want);
        assert_eq!(c1.stats(), c2.stats(), "chunking must not change the statistics");
    }

    // --- DeviceSink maintenance, with injected open/probe ---------------

    thread_local! {
        static OPENS: Cell<u32> = const { Cell::new(0) };
        static OPEN_OK: Cell<bool> = const { Cell::new(true) };
        static PROBE: std::cell::RefCell<Option<(String, u32)>> = const { std::cell::RefCell::new(None) };
    }
    fn fake_open(frames: usize, _: Option<&str>) -> Result<Opened, OutputError> {
        OPENS.with(|c| c.set(c.get() + 1));
        if !OPEN_OK.with(Cell::get) {
            return Err(OutputError::NoDevice);
        }
        let (prod, _cons) = ring(frames);
        let (id, rate) = PROBE.with(|p| p.borrow().clone()).unwrap_or(("dev-a".into(), 48_000));
        Ok((Box::new(()), prod, Arc::new(Meter::default()), rate, id))
    }
    fn fake_probe(_: Option<&str>) -> Option<(String, u32)> {
        PROBE.with(|p| p.borrow().clone())
    }
    fn setup(ok: bool, probe: Option<(&str, u32)>) -> DeviceSink {
        OPENS.with(|c| c.set(0));
        OPEN_OK.with(|c| c.set(true));
        PROBE.with(|p| *p.borrow_mut() = Some(("dev-a".into(), 48_000)));
        let sink = DeviceSink::open_with(fake_open, fake_probe, None).unwrap();
        OPEN_OK.with(|c| c.set(ok));
        PROBE.with(|p| *p.borrow_mut() = probe.map(|(i, r)| (i.to_string(), r)));
        sink
    }
    fn due(sink: &mut DeviceSink) {
        sink.last_check = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn maintain_follows_a_device_change() {
        let mut sink = setup(true, Some(("dev-b", 44_100)));
        sink.maintain(); // poll not due yet
        assert_eq!(sink.reopen_count(), 0);
        due(&mut sink);
        sink.maintain();
        assert_eq!((sink.reopen_count(), sink.device_id(), sink.sample_rate()), (1, "dev-b", 44_100));
    }

    #[test]
    fn maintain_follows_a_rate_change_on_the_same_device() {
        let mut sink = setup(true, Some(("dev-a", 96_000)));
        due(&mut sink);
        sink.maintain();
        assert_eq!((sink.reopen_count(), sink.sample_rate()), (1, 96_000));
    }

    #[test]
    fn maintain_leaves_an_unchanged_or_unknown_device_alone() {
        let mut sink = setup(true, Some(("dev-a", 48_000)));
        due(&mut sink);
        sink.maintain();
        PROBE.with(|p| *p.borrow_mut() = None);
        due(&mut sink);
        sink.maintain();
        assert_eq!((sink.reopen_count(), OPENS.with(Cell::get)), (0, 1));
    }

    #[test]
    fn maintain_reopens_a_lost_stream() {
        let mut sink = setup(true, Some(("dev-a", 48_000)));
        sink.meter.lost.store(true, Ordering::Relaxed);
        sink.maintain();
        assert_eq!(sink.reopen_count(), 1);
        assert!(!sink.meter.lost.load(Ordering::Relaxed), "fresh meter after reopen");
    }

    #[test]
    fn maintain_retries_a_failed_reopen_at_most_once_a_second() {
        let mut sink = setup(false, Some(("dev-a", 48_000)));
        sink.meter.lost.store(true, Ordering::Relaxed);
        sink.maintain();
        sink.maintain();
        assert_eq!(OPENS.with(Cell::get), 2, "initial open + one failed retry");
        assert!(sink.stream.is_none());
        sink.reopen_failed_at = Instant::now().checked_sub(Duration::from_secs(2));
        OPEN_OK.with(|c| c.set(true));
        sink.maintain();
        assert_eq!((OPENS.with(Cell::get), sink.reopen_count()), (3, 1));
        assert!(sink.stream.is_some());
    }

    #[test]
    fn failed_reopen_counts_the_old_ring_once() {
        let mut sink = setup(false, None);
        // Play 5 frames from a ring holding 2: 2 data, 3 short.
        let (mut prod, mut cons) = ring(8);
        prod.write(&[0.5; 4]);
        cons.read(&mut [0.0; 10]);
        sink.prod = prod;
        let before = sink.stats();
        assert_eq!((before.played_frames, before.underruns, before.underrun_frames), (5, 1, 3));
        sink.meter.lost.store(true, Ordering::Relaxed);
        sink.maintain(); // open fails
        assert_eq!(sink.stats(), before, "failed reopen double-counted");
        sink.reopen_failed_at = None;
        sink.maintain(); // fails again
        assert_eq!(sink.stats(), before, "second failure double-counted");
        // Success keeps the folded totals and adds nothing.
        OPEN_OK.with(|c| c.set(true));
        sink.reopen_failed_at = None;
        sink.maintain();
        assert_eq!(sink.stats(), before);
    }
}
