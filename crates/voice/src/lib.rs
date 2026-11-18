//! The communicator under the title's own voice engine
//! (docs/live/live-voice.md, 3.1 and 3.3).
//!
//! On an Xbox a headset is two USB media objects (XMOs) per port that XPP
//! makes through XVoiceCreateMediaObjectEx (`0x411143`): a microphone
//! object that fills 640-byte 16 kHz PCM packets, and a headphone object
//! that plays them. The engine's encoder, decoder and voice detection are
//! title code and run recompiled; the host only needs these two objects.
//!
//! - [`io`]: the host side, [`VoiceIo`](io::VoiceIo) with 20 ms frames at
//!   48 kHz, and [`FakeVoiceIo`](io::FakeVoiceIo) for tests.
//! - [`resample`]: 48 kHz to and from the packet rate.
//! - [`xmo`]: the media objects: creation with the original's checks and
//!   errors, Process queueing, and the 20 ms tick that completes packets.
//!
//! - [`hle`]: the runtime side: overrides at XVoiceCreateMediaObjectEx,
//!   the XMO's Process and Release, and the engine's 20 ms DPC, which
//!   ticks the objects and runs each completion's callback first.
//!
//! [`xmo`] does not touch the runtime: guest memory is the
//! [`xmo::Memory`] trait, and a completion comes back for [`hle`] to
//! deliver.
#![forbid(unsafe_code)]

pub mod hle;
pub mod io;
pub mod resample;
pub mod xmo;

/// Host frames: 20 ms at 48 kHz.
pub const HOST_RATE: u32 = 48_000;
/// Samples in one host frame.
pub const HOST_FRAME: usize = 960;
/// The engine's tick and the length of one packet of PCM.
pub const FRAME_MS: u32 = 20;
