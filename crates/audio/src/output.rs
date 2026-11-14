//! Host audio output through `cpal` (CoreAudio on macOS).
//!
//! This is the only module that touches a device. Everything else in the
//! crate is device-agnostic, so a tvOS or Xbox port replaces this file (or
//! disables the `device` feature) and calls [`Mixer::render`] itself from
//! whatever callback the platform provides. cpal also targets iOS/tvOS
//! CoreAudio, so this module may work there unchanged; that is untested.

use std::sync::{Arc, Mutex};

use cpal::{
    FromSample, Sample, SampleFormat, SizedSample, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use crate::Mixer;

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
    for (src, dst) in mix.chunks_exact(2).zip(out.chunks_exact_mut(channels)) {
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
