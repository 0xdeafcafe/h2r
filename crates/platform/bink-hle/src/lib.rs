//! Runtime overrides for the statically linked Bink 1.x player in Halo 2.
//!
//! Skip mode is always available through [`register_skip`]. With the
//! `playback` feature, `register` selects between two modes:
//!
//! * **Skip** (no movie source): the engine's movie-start wrapper
//!   ([`addr::MOVIE_START`]) is replaced by a stub that starts nothing. The
//!   wrapper's own state says "no movie" (`ST_BINK == 0`), so every caller
//!   (intro, attract, campaign, credits) moves on at once. This is the path
//!   that unblocks the main menu and needs no files.
//! * **Playback** (a `MovieSource` is present): the Bink API itself
//!   (`BinkOpen`, `BinkDoFrame`, `BinkCopyToBuffer`, ...) is replaced. The
//!   engine's wrapper runs unchanged and drives a decoded stream, including
//!   its own skip-on-button and end-of-movie handling. A movie the source
//!   cannot open makes `BinkOpen` return NULL, which the wrapper already
//!   treats as a failed open, so a missing transcode is still a clean skip.
//!
//! No Bink decoding happens here. Decoded frames come from a
//! `MovieStream`, normally fed from the transcoded MOV (see
//! `docs/audio/movies.md`) by a platform decoder behind `MovieDecoder`.
#![deny(unsafe_code)]

#[cfg(all(feature = "playback", target_os = "macos"))]
#[allow(unsafe_code)]
mod avfoundation;
#[cfg(all(feature = "playback", target_os = "macos"))]
pub use avfoundation::AvFoundationDecoder;

pub mod addr;
#[cfg(feature = "playback")]
mod audio_out;
#[cfg(feature = "playback")]
pub mod display;
#[cfg(feature = "playback")]
mod playback;
#[cfg(feature = "playback")]
pub mod select;
#[cfg(feature = "playback")]
mod source;

#[cfg(feature = "playback")]
pub use audio_out::{MixerAudio, MovieAudioOut, MovieVoice};
#[cfg(feature = "playback")]
pub use movies::status::{DiscMovie, disc_movies};
#[cfg(feature = "playback")]
pub use display::{MovieDisplay, MovieDraw, MoviePicture, MovieQuad, set_display};
#[cfg(feature = "playback")]
pub use playback::{BinkHle, DEFAULT_ARENA_HINT, HLE_RESERVED};
#[cfg(feature = "playback")]
pub use select::{Selection, platform_decoder, select};
#[cfg(feature = "playback")]
pub use source::{
    AudioInfo, Clock, MovieDecoder, MovieSource, MovieStream, Mp4Source, StreamInfo,
    SyntheticDecoder, SystemClock, TranscodedSource, synthetic_audio_frames, synthetic_sample,
};

use runtime::{Dispatch, GuestFault, dispatch::Override};
use std::sync::Arc;

/// The movie shim in a build with only the wrapper's skip stub.
#[cfg(not(feature = "playback"))]
pub struct BinkHle;

#[cfg(not(feature = "playback"))]
impl BinkHle {
    /// Skip mode never opens a stream.
    pub fn open_streams(&self) -> usize {
        0
    }

    pub fn prepare(&self, _path: &str) -> Result<(), MovieOpenError> {
        Err(MovieOpenError::NoSource)
    }

    pub fn discard_prepared(&self, _path: &str) {}

    pub fn last_request(&self, _path: &str) -> Option<RequestOutcome> {
        None
    }
}

impl BinkHle {
    /// Skip every movie at the engine's movie-start wrapper.
    pub fn skip_only() -> Arc<Self> {
        #[cfg(feature = "playback")]
        {
            Self::new(None)
        }
        #[cfg(not(feature = "playback"))]
        {
            Arc::new(Self)
        }
    }

    /// Replacement for the wrapper's `MovieStart(path, flags) ret 8`.
    fn skip_start(
        &self,
        cpu: &mut runtime::CpuAbi,
        ctx: &mut runtime::ExecCtx,
    ) -> Result<runtime::TransferAbi, GuestFault> {
        let mut mem = ctx.memory();
        #[cfg(feature = "playback")]
        self.record_skip(
            &mem,
            mem.u32(cpu.esp.wrapping_add(4))?,
            mem.u32(cpu.esp.wrapping_add(8))?,
        );
        use addr::{ST_BINK, ST_FINISHED, ST_MOVIE_OPEN};
        // No movie is open and the last one is "finished". Only touch state
        // when the wrapper's page is mapped (it always is in the real image).
        if mem.u32(ST_BINK).is_ok() {
            mem.set_u32(ST_BINK, 0)?;
            mem.set_u8(ST_MOVIE_OPEN, 0)?;
            mem.set_u8(ST_FINISHED, 1)?;
        }
        mem.ret(cpu, 8)
    }
}

#[cfg(feature = "playback")]
fn read_cstr(mem: &runtime::Guest, mut p: u32) -> String {
    let mut out = Vec::new();
    while out.len() < 260 {
        match mem.u8(p) {
            Ok(0) | Err(_) => break,
            Ok(b) => out.push(b),
        }
        p = p.wrapping_add(1);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One installed override and whatever it displaced.
pub struct Registered {
    pub addr: u32,
    pub name: &'static str,
    pub previous: Option<Override>,
}

/// What `register` installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Skip,
    Playback,
}

/// A movie the engine asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub path: String,
    pub flags: u32,
    /// True when Bink opened a stream. The wrapper also has to start it.
    pub played: bool,
}

/// A request's result at the Bink boundary. The sequence increases across
/// all movies, so the caller can tell whether a wrapper call reached Bink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestOutcome {
    pub sequence: usize,
    pub request: Request,
    pub failure: Option<MovieOpenError>,
}

/// Why a movie could not be prepared or opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MovieOpenError {
    NoSource,
    Unavailable,
    MissingOrChangedOutput,
    Decoder(String),
    InvalidMetadata,
    FirstFrame,
    NoHandle,
    NoArena,
}

impl std::fmt::Display for MovieOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::NoSource => "movie playback source is disabled or unavailable",
            Self::Unavailable => "movie is missing or is not current for this disc",
            Self::MissingOrChangedOutput => "transcoded movie output is missing or changed",
            Self::Decoder(why) => return write!(f, "movie decoder could not open the output ({why})"),
            Self::InvalidMetadata => "movie dimensions, frame count or rate are invalid",
            Self::FirstFrame => "movie decoder could not produce the first video frame",
            Self::NoHandle => "no free Bink handle",
            Self::NoArena => "no guest page for Bink handles",
        };
        f.write_str(reason)
    }
}

impl std::error::Error for MovieOpenError {}

#[cfg(feature = "playback")]
/// Install the overrides for the mode the instance is configured for.
pub fn register(dispatch: &mut Dispatch, hle: &Arc<BinkHle>) -> (Mode, Vec<Registered>) {
    if hle.has_source() {
        (Mode::Playback, register_playback(dispatch, hle))
    } else {
        (Mode::Skip, register_skip(dispatch, hle))
    }
}

/// Only the wrapper-level skip stub.
pub fn register_skip(dispatch: &mut Dispatch, hle: &Arc<BinkHle>) -> Vec<Registered> {
    let h = Arc::clone(hle);
    vec![install(
        dispatch,
        addr::MOVIE_START,
        "bink_hle::movie_start_skip",
        move |cpu, ctx, _| h.skip_start(cpu, ctx),
    )]
}

#[cfg(feature = "playback")]
/// The Bink API overrides. The wrapper runs as compiled, except its decode
/// and blit and its draw, which are Rust's ([`display`]).
pub fn register_playback(dispatch: &mut Dispatch, hle: &Arc<BinkHle>) -> Vec<Registered> {
    use addr::*;
    let mut out = Vec::new();
    macro_rules! api {
        ($addr:expr, $name:expr, $method:ident) => {{
            let h = Arc::clone(hle);
            out.push(install(dispatch, $addr, $name, move |cpu, ctx, _| {
                h.$method(cpu, ctx)
            }));
        }};
    }
    api!(
        BINK_SET_SOUND_SYSTEM,
        "bink_hle::BinkSetSoundSystem",
        set_sound_system
    );
    api!(BINK_OPEN, "bink_hle::BinkOpen", open);
    api!(BINK_WAIT, "bink_hle::BinkWait", wait);
    api!(BINK_SET_MIX_BINS, "bink_hle::BinkSetMixBins", set_mix_bins);
    api!(
        BINK_SET_MIX_BIN_VOLUMES,
        "bink_hle::BinkSetMixBinVolumes",
        set_mix_bin_volumes
    );
    api!(BINK_SET_MEMORY, "bink_hle::BinkSetMemory", set_memory);
    api!(
        BINK_COPY_TO_BUFFER,
        "bink_hle::BinkCopyToBuffer",
        copy_to_buffer
    );
    api!(BINK_DO_FRAME, "bink_hle::BinkDoFrame", do_frame);
    api!(BINK_NEXT_FRAME, "bink_hle::BinkNextFrame", next_frame);
    api!(BINK_CLOSE, "bink_hle::BinkClose", close);
    api!(
        MOVIE_DECODE_AND_BLIT,
        "bink_hle::movie_decode_native",
        decode_native
    );
    api!(MOVIE_DRAW, "bink_hle::movie_draw_native", draw_native);
    out
}

/// Put displaced overrides back (in reverse order).
pub fn unregister(dispatch: &mut Dispatch, installed: Vec<Registered>) {
    for r in installed.into_iter().rev() {
        match r.previous {
            Some(p) => {
                dispatch.restore_override(r.addr, p);
            }
            None => {
                dispatch.set_enabled(r.addr, false);
            }
        }
    }
}

fn install<F>(dispatch: &mut Dispatch, addr: u32, name: &'static str, f: F) -> Registered
where
    F: Fn(
            &mut runtime::CpuAbi,
            &mut runtime::ExecCtx,
            u32,
        ) -> Result<runtime::TransferAbi, GuestFault>
        + Send
        + Sync
        + 'static,
{
    Registered {
        addr,
        name,
        previous: dispatch.register_override(addr, name, f),
    }
}
