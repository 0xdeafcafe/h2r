//! Where decoded frames come from, and the clock that paces them.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    /// Frame rate is `fps_num / fps_den` (30000/1001 for the disc movies).
    pub fps_num: u32,
    pub fps_den: u32,
}

/// A movie's sound track as the stream hands it out: interleaved signed
/// 16-bit PCM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioInfo {
    pub sample_rate: u32,
    pub channels: u16,
}

/// A decoded video. Frames are tightly packed 32-bit BGRA (`width * 4` bytes
/// per row), which is what Bink's `BINKSURFACE32` writes on little-endian.
pub trait MovieStream: Send {
    fn info(&self) -> StreamInfo;
    /// Decode frame `index` (0-based) into `bgra`, `width * height * 4` bytes.
    /// Returns false if the frame cannot be produced; the buffer then keeps
    /// the previous picture.
    fn decode(&mut self, index: u32, bgra: &mut [u8]) -> bool;
    /// The sound track's format, if the movie has one the stream can read.
    fn audio_info(&self) -> Option<AudioInfo> {
        None
    }
    /// The next frames of sound, in order from the start, into `pcm`
    /// (interleaved, a whole number of frames). Returns the frames written;
    /// 0 at the end of the track or on error.
    fn read_audio(&mut self, _pcm: &mut [i16]) -> usize {
        0
    }
    /// Start the sound track over (a looping movie). False if it cannot.
    fn rewind_audio(&mut self) -> bool {
        false
    }
}

/// Turns a transcoded MOV or legacy MP4 into a stream. Implemented per platform
/// (AVFoundation on Apple targets); the macOS implementation uses system codecs.
pub trait MovieDecoder: Send + Sync {
    fn open(&self, movie: &Path) -> Option<Box<dyn MovieStream>>;
}

/// Resolves a movie name (`intro_60`) to a stream, or `None` to skip it.
pub trait MovieSource: Send + Sync {
    fn open(&self, name: &str) -> Option<Box<dyn MovieStream>>;
}

/// Looks the movie up in the `movies.json` manifest of a transcode
/// directory, then hands the MOV to the decoder.
pub struct TranscodedSource {
    dir: PathBuf,
    decoder: Arc<dyn MovieDecoder>,
    /// Only these names (lowercase), when set: the movies found current for
    /// this disc ([`movies::status::availability`]).
    only: Option<Vec<String>>,
}
impl TranscodedSource {
    pub fn new(dir: impl Into<PathBuf>, decoder: Arc<dyn MovieDecoder>) -> Self {
        Self {
            dir: dir.into(),
            decoder,
            only: None,
        }
    }
    /// Decline every movie not named here (`intro_60`, any case).
    pub fn only(mut self, names: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        self.only = Some(
            names
                .into_iter()
                .map(|n| n.as_ref().to_ascii_lowercase())
                .collect(),
        );
        self
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}
impl MovieSource for TranscodedSource {
    fn open(&self, name: &str) -> Option<Box<dyn MovieStream>> {
        if let Some(only) = &self.only
            && !only.iter().any(|n| n.eq_ignore_ascii_case(name))
        {
            return None;
        }
        let movie = movies::locate(&self.dir, name)?;
        self.decoder.open(&movie)
    }
}

/// Backward-compatible name for the original manifest source. Accepts MOV too.
pub type Mp4Source = TranscodedSource;

/// Monotonic milliseconds, injectable so pacing is testable.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}
pub struct SystemClock(Instant);
impl Default for SystemClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}
impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
}

/// A decoder that ignores the file and renders a moving gradient. For
/// bring-up: it exercises the whole playback path with no MP4 or codec.
/// With `audio` set the stream also has a sound track as long as the
/// video ([`synthetic_sample`] gives every sample).
pub struct SyntheticDecoder {
    pub info: StreamInfo,
    pub audio: Option<AudioInfo>,
}
impl MovieDecoder for SyntheticDecoder {
    fn open(&self, _mp4: &Path) -> Option<Box<dyn MovieStream>> {
        Some(Box::new(Synthetic {
            info: self.info,
            audio: self.audio,
            audio_pos: 0,
        }))
    }
}

/// Sample `channel` of audio frame `frame` of a [`SyntheticDecoder`] track:
/// a ramp that differs per channel, never all zero.
pub fn synthetic_sample(frame: u64, channel: u16) -> i16 {
    ((frame % 256) as i16 - 128) * 64 + channel as i16 * 1000 + 1
}

/// Audio frames in a track exactly as long as the video:
/// `frames * fps_den / fps_num` seconds.
pub fn synthetic_audio_frames(info: &StreamInfo, audio: &AudioInfo) -> u64 {
    (info.frames as u64 * info.fps_den as u64 * audio.sample_rate as u64)
        .div_ceil(info.fps_num.max(1) as u64)
}

struct Synthetic {
    info: StreamInfo,
    audio: Option<AudioInfo>,
    audio_pos: u64,
}
impl MovieStream for Synthetic {
    fn info(&self) -> StreamInfo {
        self.info
    }
    fn decode(&mut self, index: u32, bgra: &mut [u8]) -> bool {
        let w = self.info.width as usize;
        let Some(size) = w
            .checked_mul(self.info.height as usize)
            .and_then(|s| s.checked_mul(4))
        else {
            return false;
        };
        if w == 0 || self.info.height == 0 || index >= self.info.frames || bgra.len() != size {
            return false;
        }
        for (i, px) in bgra.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x, y) = (i % w, i / w);
            *px = [
                (x as u32).wrapping_add(index.wrapping_mul(4)) as u8,
                y as u8,
                index as u8,
                0xff,
            ];
        }
        true
    }
    fn audio_info(&self) -> Option<AudioInfo> {
        self.audio
    }
    fn read_audio(&mut self, pcm: &mut [i16]) -> usize {
        let Some(a) = self.audio.filter(|a| a.channels > 0) else {
            return 0;
        };
        let ch = a.channels as usize;
        let left = synthetic_audio_frames(&self.info, &a).saturating_sub(self.audio_pos);
        let n = ((pcm.len() / ch) as u64).min(left) as usize;
        for (i, frame) in pcm[..n * ch].chunks_exact_mut(ch).enumerate() {
            for (c, s) in frame.iter_mut().enumerate() {
                *s = synthetic_sample(self.audio_pos + i as u64, c as u16);
            }
        }
        self.audio_pos += n as u64;
        n
    }
    fn rewind_audio(&mut self) -> bool {
        self.audio_pos = 0;
        self.audio.is_some()
    }
}
