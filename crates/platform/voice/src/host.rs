//! Host capture and playback through cpal (CoreAudio on macOS), feature
//! `device` (docs/live/live-voice.md, 3.2).
//!
//! The device callbacks only move samples: capture downmixes to mono,
//! converts to 48 kHz and pushes into a [`Fifo`]; playback pops 48 kHz
//! samples and converts to the device's rate and channels. Both use
//! `try_lock`, so a device thread never waits on the guest: on contention
//! the capture block is dropped and the output plays silence. Each FIFO
//! holds at most 100 ms, so latency cannot build up.
//!
//! [`VoiceIo::capture`] returns one 20 ms frame when a whole one has
//! arrived and silence otherwise; [`VoiceIo::playback`] queues one.
//!
//! A device that cannot be opened is left out: capture is silent, or
//! playback is dropped. Nothing here is exercised by the tests, which use
//! [`FakeVoiceIo`](crate::io::FakeVoiceIo); the conversion it relies on
//! is tested in [`crate::convert`].

use crate::{
    HOST_FRAME, HOST_RATE,
    convert::{Fifo, RateConverter, downmix},
    io::{Capture, Frame, Playback, VoiceIo},
};
use cpal::{
    FromSample, Sample, SampleFormat, SizedSample, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use std::sync::{Arc, Mutex, mpsc};

/// 100 ms at 48 kHz.
const FIFO_CAP: usize = HOST_FRAME * 5;

pub struct HostVoiceIo {
    input: Arc<Mutex<Fifo>>,
    output: Arc<Mutex<Fifo>>,
    /// Dropping it ends the thread that owns the streams (cpal's `Stream`
    /// is not `Send`, so it stays on the thread that made it).
    _streams: mpsc::Sender<()>,
}

impl HostVoiceIo {
    /// Opens the microphone with host id `input` (`None`: the system
    /// default) and, when `output` names one, a separate output device.
    pub fn open(input: Option<&str>, output: Option<&str>) -> Self {
        let in_fifo = Arc::new(Mutex::new(Fifo::new(FIFO_CAP)));
        let out_fifo = Arc::new(Mutex::new(Fifo::new(FIFO_CAP)));
        let (stop, stopped) = mpsc::channel::<()>();
        let (input, output) = (input.map(str::to_owned), output.map(str::to_owned));
        let (fi, fo) = (Arc::clone(&in_fifo), Arc::clone(&out_fifo));
        let spawned = std::thread::Builder::new()
            .name("voice devices".into())
            .spawn(move || {
                let _capture = open_capture(input.as_deref(), &fi)
                    .map_err(|e| eprintln!("voice: microphone not opened: {e}"))
                    .ok();
                let _playback = output.as_deref().and_then(|id| {
                    open_playback(id, &fo)
                        .map_err(|e| eprintln!("voice: output {id} not opened: {e}"))
                        .ok()
                });
                // Until the HostVoiceIo goes.
                let _ = stopped.recv();
            });
        if let Err(e) = spawned {
            eprintln!("voice: device thread not started: {e}");
        }
        Self {
            input: in_fifo,
            output: out_fifo,
            _streams: stop,
        }
    }

    /// An opener for boot's `Host::voice_io`.
    pub fn opener() -> crate::io::OpenIo {
        Arc::new(|input, output| Box::new(Self::open(input, output)))
    }
}

impl Capture for HostVoiceIo {
    fn read(&mut self, frame: &mut Frame) -> bool {
        let got = self
            .input
            .lock()
            .map(|mut f| f.take_exact(frame))
            .unwrap_or(false);
        if !got {
            frame.fill(0);
        }
        got
    }
}

impl Playback for HostVoiceIo {
    fn write(&mut self, frame: &Frame) {
        if let Ok(mut f) = self.output.lock() {
            for &s in frame {
                f.push_i16(s);
            }
        }
    }
}

impl VoiceIo for HostVoiceIo {
    fn capture(&mut self) -> &mut dyn Capture {
        self
    }
    fn playback(&mut self) -> &mut dyn Playback {
        self
    }
}

fn find(
    devices: Result<impl Iterator<Item = cpal::Device>, cpal::Error>,
    id: &str,
) -> Option<cpal::Device> {
    devices
        .ok()?
        .find(|d| d.id().is_ok_and(|i| i.to_string() == id))
}

fn open_capture(id: Option<&str>, fifo: &Arc<Mutex<Fifo>>) -> Result<Stream, String> {
    let host = cpal::default_host();
    let device = match id {
        None => host.default_input_device(),
        Some(id) => find(host.input_devices(), id),
    }
    .ok_or("no such input device")?;
    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let format = config.sample_format();
    let rate = config.sample_rate();
    let cfg: cpal::StreamConfig = config.into();
    let stream = match format {
        SampleFormat::F32 => build_capture::<f32>(&device, cfg, rate, fifo),
        SampleFormat::I16 => build_capture::<i16>(&device, cfg, rate, fifo),
        SampleFormat::I32 => build_capture::<i32>(&device, cfg, rate, fifo),
        SampleFormat::U16 => build_capture::<u16>(&device, cfg, rate, fifo),
        other => return Err(format!("unsupported sample format {other:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

fn build_capture<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    rate: u32,
    fifo: &Arc<Mutex<Fifo>>,
) -> Result<Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let fifo = Arc::clone(fifo);
    let mut convert = RateConverter::new(rate, HOST_RATE);
    let mut mono = Vec::with_capacity(4096);
    let mut wide = Vec::with_capacity(4096 * channels.max(1));
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                wide.clear();
                wide.extend(data.iter().map(|s| f32::from_sample(*s)));
                mono.clear();
                downmix(&wide, channels, |s| mono.push(s));
                if let Ok(mut f) = fifo.try_lock() {
                    convert.run(&mono, |s| f.push(s));
                }
            },
            |e| eprintln!("voice: microphone: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

fn open_playback(id: &str, fifo: &Arc<Mutex<Fifo>>) -> Result<Stream, String> {
    let device = find(cpal::default_host().output_devices(), id).ok_or("no such output device")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let format = config.sample_format();
    let rate = config.sample_rate();
    let cfg: cpal::StreamConfig = config.into();
    let stream = match format {
        SampleFormat::F32 => build_playback::<f32>(&device, cfg, rate, fifo),
        SampleFormat::I16 => build_playback::<i16>(&device, cfg, rate, fifo),
        SampleFormat::I32 => build_playback::<i32>(&device, cfg, rate, fifo),
        SampleFormat::U16 => build_playback::<u16>(&device, cfg, rate, fifo),
        other => return Err(format!("unsupported sample format {other:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

fn build_playback<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    rate: u32,
    fifo: &Arc<Mutex<Fifo>>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = (config.channels as usize).max(1);
    let fifo = Arc::clone(fifo);
    let mut convert = RateConverter::new(HOST_RATE, rate);
    let mut pending: std::collections::VecDeque<f32> = Default::default();
    let mut block = Vec::with_capacity(4096);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let frames = data.len() / channels;
                // Convert enough 48 kHz input for this callback.
                while pending.len() < frames {
                    block.clear();
                    if let Ok(mut f) = fifo.try_lock() {
                        for _ in 0..256 {
                            match f.pop() {
                                Some(s) => block.push(f32::from(s) / 32768.0),
                                None => break,
                            }
                        }
                    }
                    if block.is_empty() {
                        break;
                    }
                    convert.run(&block, |s| pending.push_back(s));
                }
                for frame in data.chunks_exact_mut(channels) {
                    let s = pending.pop_front().unwrap_or(0.0);
                    frame.fill(T::from_sample(s));
                }
            },
            |e| eprintln!("voice: output: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}
