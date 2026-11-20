//! The small unsafe boundary to the system AVFoundation reader.
use crate::{AudioInfo, MovieDecoder, MovieOpenError, MovieStream, StreamInfo};
use std::ffi::{CStr, CString, c_char, c_void};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr::NonNull;

/// macOS system decoder. No ffmpeg process or bundled decoder at playback.
#[derive(Default)]
pub struct AvFoundationDecoder;

#[repr(C)]
#[derive(Default)]
struct NativeInfo {
    width: u32,
    height: u32,
    frames: u32,
    fps_num: u32,
    fps_den: u32,
}
unsafe extern "C" {
    fn h2_movie_open(
        path: *const c_char,
        info: *mut NativeInfo,
        error: *mut c_char,
        error_len: usize,
    ) -> *mut c_void;
    fn h2_movie_decode(handle: *mut c_void, index: u32, dest: *mut u8, len: usize) -> i32;
    fn h2_movie_close(handle: *mut c_void);
    fn h2_movie_audio_rate(handle: *mut c_void) -> u32;
    fn h2_movie_audio_read(handle: *mut c_void, dest: *mut i16, frames: usize) -> usize;
    fn h2_movie_audio_rewind(handle: *mut c_void) -> i32;
}
/// The bridge always hands out two channels.
const AUDIO_CHANNELS: u16 = 2;
struct Stream {
    handle: NonNull<c_void>,
    info: StreamInfo,
    audio: Option<AudioInfo>,
    /// The last picture handed out and its index: asking for it again is a
    /// copy, not a reader restart from frame 0.
    last: Option<(u32, Vec<u8>)>,
}
// SAFETY: The bridge owns its AVAssetReader, retains no thread-affine UI
// objects, uses an autorelease pool per call, and is only called with &mut self.
unsafe impl Send for Stream {}
impl MovieDecoder for AvFoundationDecoder {
    fn open(&self, path: &Path) -> Option<Box<dyn MovieStream>> {
        self.try_open(path).ok()
    }

    fn try_open(&self, path: &Path) -> Result<Box<dyn MovieStream>, MovieOpenError> {
        AvFoundationDecoder::try_open(self, path).map_err(|e| MovieOpenError::Decoder(e.to_string()))
    }
}
impl AvFoundationDecoder {
    /// Like MovieDecoder::open, with an error suitable for diagnostics.
    pub fn try_open(&self, path: &Path) -> io::Result<Box<dyn MovieStream>> {
        let path = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
        let mut info = NativeInfo::default();
        // SAFETY: path is NUL terminated, info is writable and the C layout
        // matches H2MovieInfo. Ownership of a non-null return passes to Stream.
        let mut error = [0 as c_char; 1024];
        let handle = NonNull::new(unsafe {
            h2_movie_open(path.as_ptr(), &mut info, error.as_mut_ptr(), error.len())
        })
        .ok_or_else(|| {
            // SAFETY: error starts zeroed; the bridge writes at most len
            // bytes with snprintf, which always NUL terminates this buffer.
            let error = unsafe { CStr::from_ptr(error.as_ptr()) };
            io::Error::other(error.to_string_lossy().into_owned())
        })?;
        // SAFETY: handle is the live reader just opened.
        let rate = unsafe { h2_movie_audio_rate(handle.as_ptr()) };
        Ok(Box::new(Stream {
            handle,
            last: None,
            audio: (rate > 0).then_some(AudioInfo {
                sample_rate: rate,
                channels: AUDIO_CHANNELS,
            }),
            info: StreamInfo {
                width: info.width,
                height: info.height,
                frames: info.frames,
                fps_num: info.fps_num,
                fps_den: info.fps_den,
            },
        }))
    }
}
impl MovieStream for Stream {
    fn info(&self) -> StreamInfo {
        self.info
    }
    /// Sequential decoding is cheap, and the last picture is kept, so
    /// repeating it is a copy. Going further back (a loop rewind) is O(n):
    /// the reader restarts at frame 0 and decodes forward to `index`.
    fn decode(&mut self, index: u32, bgra: &mut [u8]) -> bool {
        let size = self.info.width as usize * self.info.height as usize * 4;
        if index >= self.info.frames || bgra.len() != size {
            return false;
        }
        if let Some((i, pic)) = &self.last
            && *i == index
        {
            bgra.copy_from_slice(pic);
            return true;
        }
        // SAFETY: handle is live, uniquely accessed; bgra is writable for len
        // bytes. The bridge checks dimensions before copying and stores no pointer.
        let ok = unsafe {
            h2_movie_decode(self.handle.as_ptr(), index, bgra.as_mut_ptr(), bgra.len()) != 0
        };
        if ok {
            let (_, pic) = self.last.get_or_insert_with(|| (index, vec![0; size]));
            pic.copy_from_slice(bgra);
            self.last.as_mut().expect("set").0 = index;
        }
        ok
    }
    fn audio_info(&self) -> Option<AudioInfo> {
        self.audio
    }
    fn read_audio(&mut self, pcm: &mut [i16]) -> usize {
        let frames = Self::audio_frames(pcm);
        if self.audio.is_none() || frames == 0 {
            return 0;
        }
        // SAFETY: handle is live and uniquely accessed; pcm holds `frames`
        // stereo frames, which is all the bridge writes.
        unsafe { h2_movie_audio_read(self.handle.as_ptr(), pcm.as_mut_ptr(), frames) }
    }
    fn rewind_audio(&mut self) -> bool {
        // SAFETY: handle is live and uniquely accessed.
        self.audio.is_some() && unsafe { h2_movie_audio_rewind(self.handle.as_ptr()) != 0 }
    }
}
impl Stream {
    fn audio_frames(pcm: &[i16]) -> usize {
        pcm.len() / AUDIO_CHANNELS as usize
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: this is the single matching release for h2_movie_open.
        unsafe { h2_movie_close(self.handle.as_ptr()) }
    }
}
