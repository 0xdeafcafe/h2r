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

/// A decoded video. Frames are tightly packed 32-bit BGRA (`width * 4` bytes
/// per row), which is what Bink's `BINKSURFACE32` writes on little-endian.
pub trait MovieStream: Send {
    fn info(&self) -> StreamInfo;
    /// Decode frame `index` (0-based) into `bgra`, `width * height * 4` bytes.
    /// Returns false if the frame cannot be produced; the buffer then keeps
    /// the previous picture.
    fn decode(&mut self, index: u32, bgra: &mut [u8]) -> bool;
}

/// Turns a transcoded MP4 into a stream. Implemented per platform
/// (AVFoundation on Apple targets); this crate has no codec.
pub trait MovieDecoder: Send + Sync {
    fn open(&self, mp4: &Path) -> Option<Box<dyn MovieStream>>;
}

/// Resolves a movie name (`intro_60`) to a stream, or `None` to skip it.
pub trait MovieSource: Send + Sync {
    fn open(&self, name: &str) -> Option<Box<dyn MovieStream>>;
}

/// Looks the movie up in the `movies.json` manifest of a transcode
/// directory, then hands the MP4 to the decoder.
pub struct Mp4Source {
    dir: PathBuf,
    decoder: Arc<dyn MovieDecoder>,
}
impl Mp4Source {
    pub fn new(dir: impl Into<PathBuf>, decoder: Arc<dyn MovieDecoder>) -> Self {
        Self {
            dir: dir.into(),
            decoder,
        }
    }
}
impl MovieSource for Mp4Source {
    fn open(&self, name: &str) -> Option<Box<dyn MovieStream>> {
        let mp4 = movies::locate(&self.dir, name)?;
        self.decoder.open(&mp4)
    }
}

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
pub struct SyntheticDecoder {
    pub info: StreamInfo,
}
impl MovieDecoder for SyntheticDecoder {
    fn open(&self, _mp4: &Path) -> Option<Box<dyn MovieStream>> {
        Some(Box::new(Synthetic(self.info)))
    }
}
struct Synthetic(StreamInfo);
impl MovieStream for Synthetic {
    fn info(&self) -> StreamInfo {
        self.0
    }
    fn decode(&mut self, index: u32, bgra: &mut [u8]) -> bool {
        let w = self.0.width as usize;
        for (i, px) in bgra.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x, y) = (i % w, i / w);
            *px = [(x as u32 + index * 4) as u8, y as u8, index as u8, 0xff];
        }
        true
    }
}
