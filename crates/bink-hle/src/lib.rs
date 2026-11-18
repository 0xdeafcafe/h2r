//! Runtime overrides for the statically linked Bink 1.x player in Halo 2.
//!
//! Two modes, chosen when [`register`] runs:
//!
//! * **Skip** (no movie source): the engine's movie-start wrapper
//!   ([`addr::MOVIE_START`]) is replaced by a stub that starts nothing. The
//!   wrapper's own state says "no movie" (`ST_BINK == 0`), so every caller
//!   (intro, attract, campaign, credits) moves on at once. This is the path
//!   that unblocks the main menu and needs no files.
//! * **Playback** (a [`MovieSource`] is present): the Bink API itself
//!   (`BinkOpen`, `BinkDoFrame`, `BinkCopyToBuffer`, ...) is replaced. The
//!   engine's wrapper runs unchanged and drives a decoded stream, including
//!   its own skip-on-button and end-of-movie handling. A movie the source
//!   cannot open makes `BinkOpen` return NULL, which the wrapper already
//!   treats as a failed open, so a missing transcode is still a clean skip.
//!
//! No Bink decoding happens here. Decoded frames come from a
//! [`MovieStream`], normally fed from the transcoded MOV (see
//! `docs/audio/movies.md`) by a platform decoder behind [`MovieDecoder`].
#![deny(unsafe_code)]

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod avfoundation;
#[cfg(target_os = "macos")]
pub use avfoundation::AvFoundationDecoder;

pub mod addr;
mod audio_out;
mod playback;
pub mod select;
mod source;

pub use audio_out::{MixerAudio, MovieAudioOut, MovieVoice};
pub use movies::status::{DiscMovie, disc_movies};
pub use playback::{AUDIO_STALL_MS, BinkHle, DEFAULT_ARENA_HINT, HLE_RESERVED};
pub use select::{Selection, platform_decoder, select};
pub use source::{
    AudioInfo, Clock, MovieDecoder, MovieSource, MovieStream, Mp4Source, StreamInfo,
    SyntheticDecoder, SystemClock, TranscodedSource, synthetic_audio_frames, synthetic_sample,
};

use runtime::{Dispatch, GuestFault, dispatch::Override};
use std::sync::Arc;

/// One installed override and whatever it displaced.
pub struct Registered {
    pub addr: u32,
    pub name: &'static str,
    pub previous: Option<Override>,
}

/// What [`register`] installed.
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
    /// True when a stream was opened for it, false when it was skipped.
    pub played: bool,
}

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

/// The Bink API overrides. The wrapper runs as compiled.
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
