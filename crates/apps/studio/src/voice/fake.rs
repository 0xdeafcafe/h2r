//! A voice that says every line as an invented signal, and counts how
//! often it is asked. What Studio uses in its tests: the real service is
//! never reached from one.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::key::KeyError;
use super::{Pcm, Request, VoiceError, VoiceSource, audio};

/// The fake: half a second of silence, a second of a 440 Hz tone at 5% of
/// full scale (quiet, so normalising has work to do), half a second of
/// silence.
#[derive(Clone, Debug, Default)]
pub struct Fake {
    /// Lines asked for.
    pub calls: Arc<AtomicUsize>,
    /// The key is missing: every ask answers [`VoiceError::NoKey`].
    pub no_key: bool,
    /// What it says instead of the tone: all silence.
    pub silent: bool,
}

impl Fake {
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

/// The signal, `seconds` of a tone at `amplitude` between the silences.
pub fn signal(amplitude: f64, seconds: f64) -> Vec<i16> {
    let rate = f64::from(audio::RATE);
    let pad = (rate * 0.5) as usize;
    let tone = (rate * seconds) as usize;
    let mut out = vec![0i16; pad];
    out.extend((0..tone).map(|i| {
        let v = amplitude * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate).sin();
        (v * 32767.0).round() as i16
    }));
    out.extend(vec![0i16; pad]);
    out
}

impl VoiceSource for Fake {
    fn say(&mut self, _req: &Request) -> Result<Pcm, VoiceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.no_key {
            return Err(VoiceError::NoKey);
        }
        let samples = if self.silent {
            vec![0; audio::RATE as usize]
        } else {
            signal(0.05, 1.0)
        };
        Ok(Pcm {
            rate: audio::RATE,
            samples,
        })
    }

    fn ready(&mut self) -> Result<(), VoiceError> {
        if self.no_key {
            Err(VoiceError::NoKey)
        } else {
            Ok(())
        }
    }

    fn name(&self) -> &'static str {
        "fake"
    }
}

/// A key store with a fixed answer.
pub struct FixedKey(pub Result<String, KeyError>);

impl super::key::KeySource for FixedKey {
    fn key(&self) -> Result<super::key::Secret, KeyError> {
        self.0.clone().map(super::key::Secret::new)
    }

    fn present(&self) -> Result<bool, KeyError> {
        match &self.0 {
            Ok(_) => Ok(true),
            Err(KeyError::Missing) => Ok(false),
            Err(e) => Err(e.clone()),
        }
    }
}
