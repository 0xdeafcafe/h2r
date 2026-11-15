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
pub struct DeviceSink {
    stream: Option<Stream>,
    prod: Producer,
    meter: Arc<Meter>,
    rate: u32,
    device_id: String,
    ring_frames: usize,
    last_check: Instant,
    reopen_failed_at: Option<Instant>,
    base_stats: SinkStats,
    reopens: u32,
}

impl DeviceSink {
    /// Ring size in frames: ~0.34 s at 48 kHz, plenty over the latency target.
    const RING_FRAMES: usize = 16384;

    pub fn open_default() -> Result<Self, OutputError> {
        let (stream, prod, meter, rate, device_id) = open_stream(Self::RING_FRAMES)?;
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
        self.base_stats = self.stats();
        self.stream = None; // stop the old stream first
        match open_stream(self.ring_frames) {
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
            let current = cpal::default_host()
                .default_output_device()
                .and_then(|d| d.id().ok())
                .map(|i| i.to_string());
            if current.is_some_and(|c| c != self.device_id) {
                self.reopen();
            }
        }
    }
}

type Opened = (Stream, Producer, Arc<Meter>, u32, String);

fn open_stream(ring_frames: usize) -> Result<Opened, OutputError> {
    let device = cpal::default_host().default_output_device().ok_or(OutputError::NoDevice)?;
    let config = device.default_output_config().map_err(dev)?;
    let rate = config.sample_rate();
    let format = config.sample_format();
    let id = device.id().map(|i| i.to_string()).unwrap_or_default();
    let (prod, cons) = ring(ring_frames);
    let meter = Arc::new(Meter::default());
    let cfg = config.into();
    let stream = match format {
        SampleFormat::F32 => build_ring::<f32>(&device, cfg, cons, &meter)?,
        SampleFormat::I16 => build_ring::<i16>(&device, cfg, cons, &meter)?,
        SampleFormat::I32 => build_ring::<i32>(&device, cfg, cons, &meter)?,
        SampleFormat::U16 => build_ring::<u16>(&device, cfg, cons, &meter)?,
        other => return Err(OutputError::UnsupportedFormat(other)),
    };
    stream.play().map_err(dev)?;
    Ok((stream, prod, meter, rate, id))
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
    // Pre-sized so the callback does not allocate in the common case.
    let mut scratch: Vec<f32> = Vec::with_capacity(16384);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let frames = data.len() / channels;
                scratch.resize(frames * 2, 0.0);
                let queued = cons.queued_frames() as u64;
                cons.read(&mut scratch);
                write_frames(&scratch, data, channels);
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
    let mut scratch: Vec<f32> = Vec::new();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let frames = data.len() / channels;
                scratch.resize(frames * 2, 0.0);
                match mixer.try_lock() {
                    Ok(mut m) => m.render(&mut scratch),
                    Err(_) => scratch.fill(0.0),
                }
                write_frames(&scratch, data, channels);
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
}
