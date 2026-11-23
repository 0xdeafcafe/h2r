//! `audio.meter`: the level of the host mixer's output bus
//! (docs/audio/audio.md, "Metering the mixer"; docs/runtime/agent-api.md,
//! "audio.meter").
//!
//! Boot hands [`feed`] every block DirectSound's output renders
//! (`dsound::Output::tap_samples`): interleaved stereo at the mixer's rate,
//! in logical audio time, before the device queue, so a run with no device
//! (a `NullSink`) meters the same samples a speaker would get. The host
//! clips, the Pogo Stick's ([`crate::pogo_sound`]) and the announcer's
//! ([`crate::medal_announcer`]), play as plain buffers in that mixer and no
//! game sound tag carries them, so only this bus shows their level.
//!
//! Nothing is kept until the first request arms the capture: then the
//! last [`KEEP_SECONDS`] of output. A clock-only run (a light host or sim)
//! renders no samples and never connects, and the method answers
//! `UNAVAILABLE` there. The level maths is [`audio::meter`], which
//! Studio's `sound.measure` shares.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use agent_proto::audio::{MeterParams, MeterPlay, MeterReply, MeterRequest};
use agent_proto::{RpcError, code, method};
use audio::meter::{self, round1};

use crate::medal_announcer::parse_wav;

/// The output bus: interleaved stereo.
pub const CHANNELS: u16 = 2;
/// Seconds of output an armed capture keeps.
pub const KEEP_SECONDS: u32 = 10;
/// The longest clip `play` meters, ms.
pub const MAX_CLIP_MS: u64 = 5000;
/// Output metered past a played clip's end (ms): the mixer renders in
/// 256-frame quanta, and a resampled clip's last frame lands late.
pub const TAIL_MS: u64 = 20;

/// The latest output, counted in frames from when it was armed.
pub struct Capture {
    rate: u32,
    keep: usize,
    samples: VecDeque<f32>,
    total_frames: u64,
}

impl Capture {
    pub fn new(rate: u32) -> Capture {
        Capture {
            rate,
            keep: rate as usize * KEEP_SECONDS as usize * usize::from(CHANNELS),
            samples: VecDeque::new(),
            total_frames: 0,
        }
    }

    /// Appends interleaved stereo `block`; a trailing half frame is dropped.
    pub fn feed(&mut self, block: &[f32]) {
        let ch = usize::from(CHANNELS);
        let block = &block[..block.len() / ch * ch];
        self.samples.extend(block);
        self.total_frames += (block.len() / ch) as u64;
        let over = self.samples.len().saturating_sub(self.keep);
        self.samples.drain(..over);
    }

    /// Frames fed since arming.
    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    fn held_frames(&self) -> u64 {
        (self.samples.len() / usize::from(CHANNELS)) as u64
    }

    /// Frames `from..to` (counted as [`Capture::total_frames`]), when all
    /// are held.
    pub fn span(&self, from: u64, to: u64) -> Option<Vec<f32>> {
        let first = self.total_frames - self.held_frames();
        if from < first || to > self.total_frames || from > to {
            return None;
        }
        let ch = usize::from(CHANNELS);
        let lo = (from - first) as usize * ch;
        let hi = (to - first) as usize * ch;
        Some(self.samples.range(lo..hi).copied().collect())
    }

    /// The last `frames` frames, when that many are held.
    pub fn last(&self, frames: u64) -> Option<Vec<f32>> {
        self.span(self.total_frames.checked_sub(frames)?, self.total_frames)
    }

    fn frames_in(&self, ms: u64) -> u64 {
        ms * u64::from(self.rate) / 1000
    }
}

/// The embedded bytes of host clip `name`: `pogo/<cue>` or
/// `announcer/<medal id>`.
pub fn clip_bytes(name: &str) -> Option<&'static [u8]> {
    let (set, id) = name.split_once('/')?;
    match set {
        "pogo" => crate::pogo_sound::CLIPS
            .iter()
            .find(|(_, i, _)| *i == id)
            .map(|(.., b)| *b),
        "announcer" => crate::medal_announcer::clip_bytes(id),
        _ => None,
    }
}

/// Every host clip's name.
pub fn clip_names() -> Vec<String> {
    let pogo = crate::pogo_sound::CLIPS
        .iter()
        .map(|(_, id, _)| format!("pogo/{id}"));
    let announcer = crate::medal_announcer::CLIPS
        .iter()
        .map(|(id, _)| format!("announcer/{id}"));
    pogo.chain(announcer).collect()
}

/// What a request waits for.
enum Wait {
    Window {
        ms: u64,
    },
    Play {
        clip: String,
        from: u64,
        to: u64,
        duration_ms: u64,
    },
}

/// A request waiting for its output to be rendered. A `play` request's
/// buffer leaves the mixer when it answers or is dropped.
pub struct Pending {
    wait: Wait,
    started: Option<(Arc<Mutex<audio::Mixer>>, audio::BufferId)>,
}

impl Pending {
    /// The reply once the output it meters is captured.
    pub fn poll(&mut self, meter: &Meter) -> Option<Result<MeterReply, RpcError>> {
        let capture = meter.capture.as_ref()?;
        let (samples, ms, play) = match &self.wait {
            Wait::Window { ms } => (capture.last(capture.frames_in(*ms))?, *ms, None),
            Wait::Play {
                clip,
                from,
                to,
                duration_ms,
            } => {
                if capture.total_frames() < *to {
                    return None;
                }
                let Some(span) = capture.span(*from, *to) else {
                    return Some(Err(RpcError::new(
                        code::UNAVAILABLE,
                        "the clip's output left the capture before it was metered",
                    )));
                };
                let ms = (to - from) * 1000 / u64::from(capture.rate.max(1));
                let play = MeterPlay {
                    clip: clip.clone(),
                    duration_ms: *duration_ms,
                };
                (span, ms, Some(play))
            }
        };
        self.stop();
        Some(Ok(reply(&samples, capture.rate, ms, play)))
    }

    fn stop(&mut self) {
        if let Some((mixer, id)) = self.started.take() {
            lock(&mixer).remove_buffer(id);
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.stop();
    }
}

fn reply(samples: &[f32], rate: u32, window_ms: u64, play: Option<MeterPlay>) -> MeterReply {
    let l = meter::measure_f32(samples, CHANNELS, rate);
    MeterReply {
        window_ms,
        frames: l.frames as u64,
        rate,
        channels: CHANNELS,
        peak_dbfs: round1(l.peak_dbfs),
        rms_dbfs: round1(l.rms_dbfs),
        lufs: round1(l.lufs),
        max_momentary_lufs: round1(l.max_momentary_lufs),
        clipped: l.clipped as u64,
        play,
    }
}

/// The mixer it meters and its capture.
#[derive(Default)]
pub struct Meter {
    mixer: Option<Arc<Mutex<audio::Mixer>>>,
    rate: u32,
    capture: Option<Capture>,
}

impl Meter {
    pub const fn new() -> Meter {
        Meter {
            mixer: None,
            rate: 0,
            capture: None,
        }
    }

    pub fn connect(&mut self, mixer: Arc<Mutex<audio::Mixer>>) {
        self.rate = lock(&mixer).sample_rate();
        self.mixer = Some(mixer);
        self.capture = None;
    }

    pub fn armed(&self) -> bool {
        self.capture.is_some()
    }

    /// One rendered block of the bus; nothing before the capture is armed.
    pub fn feed(&mut self, block: &[f32]) {
        if let Some(c) = &mut self.capture {
            c.feed(block);
        }
    }

    /// Arms the capture and starts `request`.
    pub fn begin(&mut self, request: MeterRequest) -> Result<Pending, RpcError> {
        match request {
            MeterRequest::Window { ms } => {
                self.arm()?;
                Ok(Pending {
                    wait: Wait::Window { ms: u64::from(ms) },
                    started: None,
                })
            }
            MeterRequest::Play { clip } => {
                let bytes = clip_bytes(&clip).ok_or_else(|| {
                    RpcError::new(
                        code::INVALID_PARAMS,
                        format!("no host clip {clip:?}; one of {}", clip_names().join(", ")),
                    )
                })?;
                self.play(clip, bytes)
            }
        }
    }

    fn arm(&mut self) -> Result<&mut Capture, RpcError> {
        if self.mixer.is_none() || self.rate == 0 {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "no mixer output to meter: audio is clock-only (a light host or sim) or DirectSound is off",
            ));
        }
        let rate = self.rate;
        Ok(self.capture.get_or_insert_with(|| Capture::new(rate)))
    }

    /// Starts the WAV `bytes` as a plain 2D buffer and meters from now to
    /// its end.
    fn play(&mut self, clip: String, bytes: &[u8]) -> Result<Pending, RpcError> {
        let unavailable = |why: String| RpcError::new(code::UNAVAILABLE, format!("{clip}: {why}"));
        let wav = parse_wav(bytes).map_err(|e| unavailable(e.to_string()))?;
        let frames = wav.data.len() as u64 / (2 * u64::from(wav.channels));
        let duration_ms = (frames * 1000).div_ceil(u64::from(wav.rate));
        if duration_ms > MAX_CLIP_MS {
            return Err(unavailable(format!(
                "{duration_ms} ms is longer than the {MAX_CLIP_MS} ms play meters"
            )));
        }
        let format = audio::WaveFormat::pcm16(wav.channels, wav.rate)
            .map_err(|e| unavailable(e.to_string()))?;
        let mut buffer = audio::SoundBuffer::from_data(format, wav.data)
            .map_err(|e| unavailable(e.to_string()))?;
        buffer.play(false);
        let capture = self.arm()?;
        let from = capture.total_frames();
        let to = from + capture.frames_in(duration_ms + TAIL_MS);
        let mixer = Arc::clone(self.mixer.as_ref().expect("arm checked the mixer"));
        let id = lock(&mixer).add_buffer(buffer);
        Ok(Pending {
            wait: Wait::Play {
                clip,
                from,
                to,
                duration_ms,
            },
            started: Some((mixer, id)),
        })
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Set once a request armed the capture: [`feed`] does nothing before.
static ARMED: AtomicBool = AtomicBool::new(false);
static METER: Mutex<Meter> = Mutex::new(Meter::new());

/// Gives the meter DirectSound's mixer (boot, once audio renders samples).
pub fn connect(mixer: Arc<Mutex<audio::Mixer>>) {
    lock(&METER).connect(mixer);
    ARMED.store(false, Ordering::Relaxed);
}

/// The output's sample tap: every rendered block, interleaved stereo.
pub fn feed(block: &[f32]) {
    if ARMED.load(Ordering::Relaxed) {
        lock(&METER).feed(block);
    }
}

/// Registers `audio.meter`.
pub fn add_agent_method(agent: &agent::Agent) {
    agent.add_game_method(method::AUDIO_METER, agent::Access::Read, |params, ctx| {
        let params = if params.is_null() {
            MeterParams::default()
        } else {
            serde_json::from_value::<MeterParams>(params)
                .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?
        };
        let request = params
            .check()
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e))?;
        let mut pending = {
            let mut meter = lock(&METER);
            let pending = meter.begin(request)?;
            ARMED.store(meter.armed(), Ordering::Relaxed);
            pending
        };
        ctx.defer(
            agent::DEFER_TICKS_MAX,
            Box::new(move |_| {
                // MeterReply is numbers and strings: it always serialises.
                let answer = pending.poll(&lock(&METER))?;
                Some(answer.map(|reply| serde_json::to_value(reply).unwrap_or_default()))
            }),
        );
        Ok(serde_json::Value::Null)
    });
}

#[cfg(test)]
mod tests;
