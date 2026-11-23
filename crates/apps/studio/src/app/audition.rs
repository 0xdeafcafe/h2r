//! Sound audition: decode a `snd!` tag's permutations, play them through
//! the host's audio output, and show their waveform. docs/editor/
//! rich-editors.md, "Sounds and facial animation" (slice A1).
//!
//! **What plays.** A sound's permutation is the concatenation of its
//! chunks' encoded bytes ([`tags::sound::Gestalt::read_encoded`], over the
//! map's raw-resource files as [`App::shown_raw_files`] names them).
//! [`decode`] turns them into 16-bit PCM by the sound's own header:
//!
//! - Xbox ADPCM, mono or stereo (`audio::adpcm`), at the header's rate;
//! - a codec stream (WMA in ASF framing), through `wma`, which needs the
//!   game's executable for its tables ([`NEEDS_EXECUTABLE`] without it,
//!   [`wma::VerifiedImage`] verifying it with). Its output is stereo at
//!   44.1 kHz, as `h2 sound` writes it.
//!
//! Any other compression is refused by name; nothing is guessed.
//!
//! **How it plays.** [`Output`] owns an audio thread. The thread opens the
//! sink (`audio::output::DeviceSink` is not `Send`, so it is opened where
//! it lives), keeps about [`Playback::TARGET_MS`] of audio queued in it and
//! publishes the position. [`Output::paced_null`] stands a headless sink
//! (`audio::sink::NullSink`, advanced by the wall clock) in for the
//! device. The device is opened on the first play, never at start-up, and
//! a machine without one gets [`SoundError::Failed`] with the reason, not
//! silence. A stop ends the feed; what the sink already queued (under
//! `TARGET_MS`) still plays out.
//!
//! The window ([`crate::ui`]'s Sound window) and the agent API
//! (`sound.read`, `sound.play`, `sound.stop`) both go through the `App`
//! methods here, so they are one path.
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use audio::sink::{AudioSink, NullSink, SinkStats};
use map::{RawPointer, TagId};
use tags::sound::{
    Compression, Encoding, Gestalt, Permutation, ResourceReader, Sound, SoundHeader,
};

use super::App;

/// Why a codec stream cannot be decoded without the disc's executable.
pub const NEEDS_EXECUTABLE: &str = "This sound needs the game's executable to decode.";

/// Most interleaved samples one decoded codec stream may hold.
const MAX_SAMPLES: usize = 128 * 1024 * 1024;

/// Why a sound is not played.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoundError {
    /// The sound is checked and cannot be decoded here: why.
    Refused(String),
    /// Something failed while reading or opening: why.
    Failed(String),
}

impl fmt::Display for SoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(s) | Self::Failed(s) => f.write_str(s),
        }
    }
}

impl From<String> for SoundError {
    fn from(s: String) -> Self {
        Self::Failed(s)
    }
}

/// Decoded audio: interleaved 16-bit samples.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    pub rate: u32,
    pub channels: u16,
    pub pcm: Arc<[i16]>,
}

impl Clip {
    pub fn frames(&self) -> usize {
        self.pcm.len() / usize::from(self.channels.max(1))
    }

    pub fn duration_ms(&self) -> u64 {
        self.frames() as u64 * 1000 / u64::from(self.rate.max(1))
    }

    /// `bins` pairs of `[lowest, highest]`, each over its share of the
    /// frames, the channels averaged: the waveform's columns.
    pub fn peaks(&self, bins: usize) -> Vec<[i16; 2]> {
        let (frames, ch) = (self.frames(), usize::from(self.channels.max(1)));
        if frames == 0 || bins == 0 {
            return Vec::new();
        }
        (0..bins)
            .map(|b| {
                let lo = b * frames / bins;
                let hi = ((b + 1) * frames / bins).max(lo + 1).min(frames);
                let mut out = [i16::MAX, i16::MIN];
                for f in lo..hi {
                    let sum: i32 = self.pcm[f * ch..(f + 1) * ch]
                        .iter()
                        .map(|&s| i32::from(s))
                        .sum();
                    let v = (sum / ch as i32) as i16;
                    out[0] = out[0].min(v);
                    out[1] = out[1].max(v);
                }
                out
            })
            .collect()
    }

    /// Interleaved stereo `f32` at `rate` Hz (a mono clip on both sides).
    pub fn stereo(&self, rate: u32) -> Vec<f32> {
        let ch = usize::from(self.channels.max(1));
        let mut out = Vec::with_capacity(self.frames() * 2);
        for f in self.pcm.chunks_exact(ch) {
            let left = f32::from(f[0]) / 32768.0;
            let right = f32::from(f[ch.min(2) - 1]) / 32768.0;
            out.push(left);
            out.push(right);
        }
        audio::resample::resample_linear(&out, 2, self.rate, rate)
    }
}

/// Whether decoding `header`'s sound needs the game's executable.
pub fn needs_executable(header: &SoundHeader) -> bool {
    matches!(
        (header.compression, header.encoding),
        (Compression::Wma | Compression::XboxAdpcm, Encoding::Codec)
    )
}

/// Decode one permutation's encoded bytes by `header`. `tables` are the
/// codec tables from the executable ([`load_tables`]); a codec stream
/// without them is refused.
pub fn decode(
    header: &SoundHeader,
    data: &[u8],
    tables: Option<&wma::Tables>,
) -> Result<Clip, SoundError> {
    match (header.compression, header.encoding) {
        (Compression::XboxAdpcm, enc @ (Encoding::Mono | Encoding::Stereo)) => {
            let channels = if enc == Encoding::Mono { 1 } else { 2 };
            let rate = header.sample_rate.hz().ok_or_else(|| {
                SoundError::Refused(format!(
                    "the sample rate selector {:?} is not a known rate",
                    header.sample_rate
                ))
            })?;
            let pcm = audio::adpcm::decode(data, channels)
                .map_err(|e| SoundError::Failed(format!("Xbox ADPCM: {e}")))?;
            Ok(Clip {
                rate,
                channels: channels as u16,
                pcm: pcm.into(),
            })
        }
        (Compression::Wma | Compression::XboxAdpcm, Encoding::Codec) => {
            let tables = tables.ok_or_else(|| SoundError::Refused(NEEDS_EXECUTABLE.into()))?;
            let decoded = wma::decode_asf(tables, data, MAX_SAMPLES)
                .map_err(|e| SoundError::Failed(format!("WMA: {e}")))?;
            Ok(Clip {
                rate: 44100,
                channels: 2,
                pcm: decoded.pcm.into(),
            })
        }
        (c, e) => Err(SoundError::Refused(format!(
            "compression {c:?} with encoding {e:?} is not decoded by the editor"
        ))),
    }
}

/// The codec tables from the executable's bytes, after `wma` has verified
/// the image.
pub fn load_tables(xbe: &[u8]) -> Result<wma::Tables, SoundError> {
    let image = wma::VerifiedImage::load(xbe)
        .map_err(|e| SoundError::Refused(format!("the game's executable: {e}")))?;
    wma::Tables::load(&image)
        .map_err(|e| SoundError::Failed(format!("the game's executable's codec tables: {e}")))
}

/// Decode permutation `p` of a sound: read its chunks from `reader`, then
/// [`decode`].
pub fn decode_permutation(
    header: &SoundHeader,
    gestalt: &Gestalt,
    p: &Permutation,
    reader: &mut dyn ResourceReader,
    tables: Option<&wma::Tables>,
) -> Result<Clip, SoundError> {
    if needs_executable(header) && tables.is_none() {
        return Err(SoundError::Refused(NEEDS_EXECUTABLE.into()));
    }
    let data = gestalt
        .read_encoded(p, reader)
        .map_err(|e| SoundError::Failed(e.to_string()))?;
    decode(header, &data, tables)
}

/// [`ResourceReader`] over the map's raw-resource files.
struct Raw<'a>(&'a mut assets::ResourceFiles);

impl ResourceReader for Raw<'_> {
    fn check(&self, p: RawPointer, size: u32) -> tags::Result<()> {
        let Some(length) = self.0.length(p.file) else {
            return Err(tags::Error::MissingResource(p.file));
        };
        if u64::from(p.offset) + u64::from(size) > length {
            return Err(tags::Error::ResourceBounds {
                file: p.file,
                offset: p.offset,
                size,
                length,
            });
        }
        Ok(())
    }

    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.check(p, size)?;
        self.0
            .read(p.file, p.offset, size)
            .map_err(|_| tags::Error::Invalid("a raw resource file did not read"))
    }
}

/// The resource reader over `files`, for decoding a permutation.
#[cfg(test)]
pub(crate) fn raw(files: &mut assets::ResourceFiles) -> impl ResourceReader + '_ {
    Raw(files)
}

/// Feeds one clip's samples (interleaved stereo at the sink's rate) to a
/// sink, keeping [`Self::TARGET_MS`] of them queued.
pub struct Playback {
    samples: Arc<[f32]>,
    /// Frames handed to the sink.
    at: usize,
}

impl Playback {
    pub const TARGET_MS: u32 = 60;

    /// Start at frame `from` (clamped to the end).
    pub fn new(samples: Arc<[f32]>, from: usize) -> Self {
        let at = from.min(samples.len() / 2);
        Self { samples, at }
    }

    pub fn frames(&self) -> usize {
        self.samples.len() / 2
    }

    /// Every frame is with the sink (some may still be queued there).
    pub fn done(&self) -> bool {
        self.at >= self.frames()
    }

    /// Top the sink's queue up; returns the frames written.
    pub fn feed(&mut self, sink: &mut dyn AudioSink) -> usize {
        let target = (u64::from(sink.sample_rate()) * u64::from(Self::TARGET_MS) / 1000) as usize;
        let want = target
            .saturating_sub(sink.queued_frames())
            .min(sink.free_frames())
            .min(self.frames() - self.at);
        if want == 0 {
            return 0;
        }
        let n = sink.write(&self.samples[self.at * 2..(self.at + want) * 2]);
        self.at += n;
        n
    }

    /// The frame being heard: those written, less those still queued.
    pub fn position(&self, sink: &dyn AudioSink) -> usize {
        self.at.saturating_sub(sink.queued_frames())
    }
}

enum Cmd {
    Play { samples: Arc<[f32]>, from: usize },
    Stop,
}

#[derive(Default)]
struct Shared {
    playing: AtomicBool,
    /// The frame being heard, at the sink's rate.
    position: AtomicU64,
}

/// A headless sink: a `NullSink` whose device time is the wall clock.
struct Paced {
    sink: NullSink,
    last: Instant,
    owed: f64,
}

impl Paced {
    fn new(rate: u32) -> Self {
        Self {
            sink: NullSink::new(rate, 1 << 15),
            last: Instant::now(),
            owed: 0.0,
        }
    }
}

impl AudioSink for Paced {
    fn sample_rate(&self) -> u32 {
        self.sink.sample_rate()
    }
    fn queued_frames(&self) -> usize {
        self.sink.queued_frames()
    }
    fn free_frames(&self) -> usize {
        self.sink.free_frames()
    }
    fn write(&mut self, samples: &[f32]) -> usize {
        self.sink.write(samples)
    }
    fn stats(&self) -> SinkStats {
        self.sink.stats()
    }
    fn maintain(&mut self) {
        let now = Instant::now();
        self.owed +=
            now.duration_since(self.last).as_secs_f64() * f64::from(self.sink.sample_rate());
        self.last = now;
        let n = (self.owed as usize).min(self.sink.queued_frames());
        self.owed -= n as f64;
        if n > 0 {
            self.sink.advance(n);
        }
    }
}

/// The audio thread and the way to it.
pub struct Output {
    tx: Option<mpsc::Sender<Cmd>>,
    shared: Arc<Shared>,
    rate: u32,
    name: &'static str,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Output {
    /// The system's default output device.
    pub fn device() -> Result<Self, SoundError> {
        Self::spawn("device", || {
            audio::output::DeviceSink::open_default()
                .map(|s| Box::new(s) as Box<dyn AudioSink>)
                .map_err(|e| format!("no audio output: {e}"))
        })
    }

    /// A headless sink at `rate` Hz that plays in real time to nowhere.
    pub fn paced_null(rate: u32) -> Result<Self, SoundError> {
        Self::spawn("headless", move || {
            Ok(Box::new(Paced::new(rate)) as Box<dyn AudioSink>)
        })
    }

    fn spawn(
        name: &'static str,
        open: impl FnOnce() -> Result<Box<dyn AudioSink>, String> + Send + 'static,
    ) -> Result<Self, SoundError> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let thread_shared = shared.clone();
        let join = std::thread::Builder::new()
            .name("audition".into())
            .spawn(move || match open() {
                Ok(sink) => {
                    let _ = ready_tx.send(Ok(sink.sample_rate()));
                    run(rx, thread_shared, sink);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| SoundError::Failed(format!("the audio thread did not start: {e}")))?;
        let rate = ready_rx
            .recv()
            .map_err(|_| SoundError::Failed("the audio thread stopped".into()))?
            .map_err(SoundError::Failed)?;
        Ok(Self {
            tx: Some(tx),
            shared,
            rate,
            name,
            join: Some(join),
        })
    }

    /// The sink's rate, Hz.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    fn send(&self, cmd: Cmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
    }

    /// Play `samples` (stereo at [`Self::rate`]) from frame `from`,
    /// replacing what plays.
    pub fn play(&self, samples: Arc<[f32]>, from: usize) {
        self.shared.position.store(from as u64, Ordering::Relaxed);
        self.shared.playing.store(true, Ordering::Relaxed);
        self.send(Cmd::Play { samples, from });
    }

    pub fn stop(&self) {
        self.shared.playing.store(false, Ordering::Relaxed);
        self.send(Cmd::Stop);
    }

    pub fn playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    /// The frame being heard, at [`Self::rate`].
    pub fn position(&self) -> u64 {
        self.shared.position.load(Ordering::Relaxed)
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        // Closing the channel ends the thread's loop.
        self.tx = None;
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run(rx: mpsc::Receiver<Cmd>, shared: Arc<Shared>, mut sink: Box<dyn AudioSink>) {
    let mut playback: Option<Playback> = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(5)) {
            Ok(Cmd::Play { samples, from }) => {
                playback = Some(Playback::new(samples, from));
                shared.playing.store(true, Ordering::Relaxed);
            }
            Ok(Cmd::Stop) => {
                playback = None;
                shared.playing.store(false, Ordering::Relaxed);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        sink.maintain();
        if let Some(p) = &mut playback {
            p.feed(&mut *sink);
            shared
                .position
                .store(p.position(&*sink) as u64, Ordering::Relaxed);
            if p.done() && sink.queued_frames() == 0 {
                playback = None;
                shared.playing.store(false, Ordering::Relaxed);
            }
        }
    }
}

/// A sound's header and permutations, as `sound.read` lists them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundInfo {
    pub sample_rate: Option<u32>,
    pub encoding: String,
    pub compression: String,
    pub max_play_time_ms: u32,
    /// Pitch ranges, each its permutations.
    pub ranges: Vec<Vec<PermInfo>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermInfo {
    pub encoded_bytes: u32,
    pub chunks: usize,
}

/// What is playing and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayState {
    pub playing: bool,
    pub tag: Option<TagId>,
    pub range: usize,
    pub permutation: usize,
    pub position_ms: u64,
    /// `device` or `headless`, once an output is open.
    pub output: Option<&'static str>,
}

/// One `snd!` of the shown map and its header's class byte (`+2`, not
/// named: docs/formats/sound.md), for `sound.measure`'s reference set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundTag {
    pub id: TagId,
    pub name: String,
    pub class: u8,
}

/// A started play.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Started {
    pub frames: usize,
    pub rate: u32,
    pub channels: u16,
    pub duration_ms: u64,
    pub from_ms: u64,
    pub output: &'static str,
}

/// A sound read from the map: what is needed to decode its permutations.
struct Loaded {
    /// The source and map it was read from, and the tag.
    key: (String, TagId),
    header: SoundHeader,
    gestalt: Gestalt,
    sound: Sound,
    clips: HashMap<(usize, usize), Arc<Clip>>,
}

/// The audition's state in the [`App`].
#[derive(Default)]
pub struct Audition {
    output: Option<Output>,
    /// Play to a headless sink instead of the device (tests, and a host
    /// with no audio).
    pub headless: bool,
    /// The codec tables, and the source they were read for.
    tables: Option<(String, Arc<wma::Tables>)>,
    loaded: Option<Loaded>,
    now: Option<(TagId, usize, usize)>,
}

impl Audition {
    fn output(&mut self) -> Result<&Output, SoundError> {
        if self.output.is_none() {
            self.output = Some(if self.headless {
                Output::paced_null(48000)?
            } else {
                Output::device()?
            });
        }
        Ok(self.output.as_ref().expect("opened"))
    }

    /// What plays now.
    pub fn state(&self) -> PlayState {
        let (tag, range, permutation) = match self.now {
            Some((t, r, p)) => (Some(t), r, p),
            None => (None, 0, 0),
        };
        let (playing, position_ms, output) = match &self.output {
            Some(o) => (
                o.playing(),
                o.position() * 1000 / u64::from(o.rate().max(1)),
                Some(o.name()),
            ),
            None => (false, 0, None),
        };
        PlayState {
            playing,
            tag,
            range,
            permutation,
            position_ms,
            output,
        }
    }
}

impl App {
    fn sound_key(&self, id: TagId) -> (String, TagId) {
        let source = self
            .source
            .as_ref()
            .map(|s| s.path().display().to_string())
            .unwrap_or_default();
        (
            format!("{source}|{}", self.shown_map_name().unwrap_or_default()),
            id,
        )
    }

    fn ensure_sound(&mut self, id: TagId) -> Result<(), SoundError> {
        let key = self.sound_key(id);
        if self.audition.loaded.as_ref().is_some_and(|l| l.key == key) {
            return Ok(());
        }
        let t = self
            .shown_tags()
            .tag(id)
            .ok_or_else(|| SoundError::Failed("no such tag".into()))?;
        if t.group.0 != *b"snd!" {
            return Err(SoundError::Refused(format!(
                "{} is not a sound (snd!)",
                tags::defs::fourcc_text(t.group)
            )));
        }
        let mut map = self.open_shown_map()?;
        let failed = |e: tags::Error| SoundError::Failed(e.to_string());
        let header = tags::sound::read_header(&mut map, id).map_err(failed)?;
        // Another sound of the same map shares its gestalt: keep it, so a
        // run over many sounds (sound.measure's reference set) reads it
        // once.
        let gestalt = match self.audition.loaded.take() {
            Some(l) if l.key.0 == key.0 => l.gestalt,
            _ => Gestalt::read(&mut map).map_err(failed)?,
        };
        let sound = gestalt.resolve(&header).map_err(failed)?;
        self.audition.loaded = Some(Loaded {
            key,
            header,
            gestalt,
            sound,
            clips: HashMap::new(),
        });
        Ok(())
    }

    /// Tag `id`'s header and permutations.
    pub fn sound_info(&mut self, id: TagId) -> Result<SoundInfo, SoundError> {
        self.ensure_sound(id)?;
        let l = self.audition.loaded.as_ref().expect("loaded");
        Ok(SoundInfo {
            sample_rate: l.header.sample_rate.hz(),
            encoding: format!("{:?}", l.header.encoding),
            compression: format!("{:?}", l.header.compression),
            max_play_time_ms: l.header.max_play_time,
            ranges: l
                .sound
                .ranges
                .iter()
                .map(|r| {
                    r.permutations
                        .iter()
                        .map(|p| PermInfo {
                            encoded_bytes: p.encoded_size,
                            chunks: l.gestalt.chunks_of(p).len(),
                        })
                        .collect()
                })
                .collect(),
        })
    }

    /// The codec tables, from the source's executable (read once per
    /// source).
    fn sound_tables(&mut self) -> Result<Arc<wma::Tables>, SoundError> {
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| SoundError::Refused(NEEDS_EXECUTABLE.into()))?;
        let key = source.path().display().to_string();
        if let Some((k, t)) = &self.audition.tables
            && *k == key
        {
            return Ok(t.clone());
        }
        let bytes = source
            .executable()
            .map(|(_, bytes)| bytes)
            .map_err(|_| SoundError::Refused(NEEDS_EXECUTABLE.into()))?;
        let tables = Arc::new(load_tables(&bytes)?);
        self.audition.tables = Some((key, tables.clone()));
        Ok(tables)
    }

    /// The decoded audio of permutation `permutation` of pitch range
    /// `range` of sound `id` (decoded once, then kept).
    pub fn sound_clip(
        &mut self,
        id: TagId,
        range: usize,
        permutation: usize,
    ) -> Result<Arc<Clip>, SoundError> {
        self.ensure_sound(id)?;
        let (header, p, wants_tables) = {
            let l = self.audition.loaded.as_ref().expect("loaded");
            if let Some(c) = l.clips.get(&(range, permutation)) {
                return Ok(c.clone());
            }
            let p = *l
                .sound
                .ranges
                .get(range)
                .ok_or_else(|| SoundError::Failed(format!("no pitch range {range}")))?
                .permutations
                .get(permutation)
                .ok_or_else(|| {
                    SoundError::Failed(format!("no permutation {permutation} in range {range}"))
                })?;
            (l.header.clone(), p, needs_executable(&l.header))
        };
        let tables = if wants_tables {
            Some(self.sound_tables()?)
        } else {
            None
        };
        let mut files = self.shown_raw_files()?;
        let clip = {
            let l = self.audition.loaded.as_ref().expect("loaded");
            decode_permutation(
                &header,
                &l.gestalt,
                &p,
                &mut Raw(&mut files),
                tables.as_deref(),
            )?
        };
        let clip = Arc::new(clip);
        if let Some(l) = self.audition.loaded.as_mut() {
            l.clips.insert((range, permutation), clip.clone());
        }
        Ok(clip)
    }

    /// Play a permutation from `from_ms` into it, replacing what plays.
    pub fn sound_play(
        &mut self,
        id: TagId,
        range: usize,
        permutation: usize,
        from_ms: u64,
    ) -> Result<Started, SoundError> {
        let clip = self.sound_clip(id, range, permutation)?;
        let out = self.audition.output()?;
        let rate = out.rate();
        let samples: Arc<[f32]> = clip.stereo(rate).into();
        let from = (from_ms.saturating_mul(u64::from(rate)) / 1000) as usize;
        out.play(samples, from);
        let output = out.name();
        self.audition.now = Some((id, range, permutation));
        Ok(Started {
            frames: clip.frames(),
            rate: clip.rate,
            channels: clip.channels,
            duration_ms: clip.duration_ms(),
            from_ms,
            output,
        })
    }

    /// Plays a clip that is not a tag of the map (the Announcer panel's
    /// line) through the same output, replacing what plays.
    pub fn sound_play_clip(&mut self, clip: &Clip) -> Result<Started, SoundError> {
        let out = self.audition.output()?;
        let rate = out.rate();
        out.play(clip.stereo(rate).into(), 0);
        let output = out.name();
        self.audition.now = None;
        Ok(Started {
            frames: clip.frames(),
            rate: clip.rate,
            channels: clip.channels,
            duration_ms: clip.duration_ms(),
            from_ms: 0,
            output,
        })
    }

    /// Every `snd!` of the shown map, in tag order, with its class: one
    /// pass over the map's headers (20 bytes each), nothing decoded. A
    /// header that does not read is left out.
    pub fn sound_tags(&self) -> Result<Vec<SoundTag>, SoundError> {
        let mut map = self.open_shown_map()?;
        let ids: Vec<(TagId, String)> = map
            .tags()
            .iter()
            .filter(|t| t.group.0 == *b"snd!")
            .map(|t| (t.id, t.name.clone().unwrap_or_default()))
            .collect();
        Ok(ids
            .into_iter()
            .filter_map(|(id, name)| {
                let h = tags::sound::read_header(&mut map, id).ok()?;
                Some(SoundTag {
                    id,
                    name,
                    class: h.class,
                })
            })
            .collect())
    }

    /// Stop what plays; whether anything did.
    pub fn sound_stop(&mut self) -> bool {
        match &self.audition.output {
            Some(o) if o.playing() => {
                o.stop();
                true
            }
            _ => false,
        }
    }

    /// What plays now.
    pub fn sound_state(&self) -> PlayState {
        self.audition.state()
    }
}
