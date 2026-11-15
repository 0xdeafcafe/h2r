//! The small unsafe boundary to the system AVFoundation reader.
use crate::{MovieDecoder, MovieStream, StreamInfo};
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
}
struct Stream {
    handle: NonNull<c_void>,
    info: StreamInfo,
}
// SAFETY: The bridge owns its AVAssetReader, retains no thread-affine UI
// objects, uses an autorelease pool per call, and is only called with &mut self.
unsafe impl Send for Stream {}
impl MovieDecoder for AvFoundationDecoder {
    fn open(&self, path: &Path) -> Option<Box<dyn MovieStream>> {
        self.try_open(path).ok()
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
        Ok(Box::new(Stream {
            handle,
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
    /// Sequential decoding is cheap. Going backwards (a loop rewind) is O(n):
    /// the reader restarts at frame 0 and decodes forward to `index`.
    fn decode(&mut self, index: u32, bgra: &mut [u8]) -> bool {
        let size = self.info.width as usize * self.info.height as usize * 4;
        if index >= self.info.frames || bgra.len() != size {
            return false;
        }
        // SAFETY: handle is live, uniquely accessed; bgra is writable for len
        // bytes. The bridge checks dimensions before copying and stores no pointer.
        unsafe { h2_movie_decode(self.handle.as_ptr(), index, bgra.as_mut_ptr(), bgra.len()) != 0 }
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: this is the single matching release for h2_movie_open.
        unsafe { h2_movie_close(self.handle.as_ptr()) }
    }
}
