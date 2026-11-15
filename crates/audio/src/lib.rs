//! Audio foundation for h2recomp.
//!
//! The Xbox SDK's DirectSound (and the MCPX APU behind it) is replaced at the
//! API level. Nothing here emulates the hardware. See `docs/audio.md`.
//!
//! Layout:
//! - [`adpcm`]: Xbox ADPCM decoder (pure, no I/O).
//! - [`format`]: wave formats and decoding of every format DirectSound takes.
//! - [`fx`]: per-voice filters, LFOs, envelopes, headroom and I3DL2 source shaping.
//! - [`reverb`]: the I3DL2-style environmental reverb behind the effects send.
//! - [`resample`]: linear sample-rate conversion.
//! - [`dsound`]: buffers, streams, 3D stub: the DirectSound-shaped model.
//! - [`mixer`]: software mixer rendering voices to a host buffer.
//! - [`route`]: Xbox mix bins folded down to stereo.
//! - [`sink`]: lock-free ring, `AudioSink`, headless `NullSink`, latency-targeting `Pump`.
//! - [`output`]: the only device-touching code (feature `device`).

pub mod adpcm;
pub mod dsound;
pub mod format;
pub mod fx;
pub mod mixer;
pub mod resample;
pub mod reverb;
pub mod route;
pub mod sink;

#[cfg(feature = "device")]
pub mod output;

pub use dsound::{BufferId, MixIn, SoundBuffer, Stream, StreamId};
pub use format::{Codec, WaveFormat};
pub use reverb::{I3dl2Listener, Reverb};
pub use mixer::{MixInId, Mixer};
pub use sink::{AudioSink, NullSink, Pump, SinkStats};

/// Errors from the audio layer.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum AudioError {
    #[error("unsupported format: {0}")]
    UnsupportedFormat(&'static str),
    #[error("data length {len} is not a multiple of the {unit}-byte {what}")]
    BadLength {
        len: usize,
        unit: usize,
        what: &'static str,
    },
    #[error("range {offset}+{len} is outside the {size}-byte buffer")]
    OutOfRange {
        offset: usize,
        len: usize,
        size: usize,
    },
    #[error("no such buffer or stream")]
    InvalidHandle,
}
