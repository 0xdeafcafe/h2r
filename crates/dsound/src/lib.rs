//! Guest-facing DirectSound for h2recomp.
//!
//! The Xbox DirectSound that Halo 2 links statically is replaced at the API
//! level: guest calls (stdcall, stack arguments) land on [`DirectSound`],
//! which reads and writes guest memory through the [`Host`] trait and drives
//! `audio::Mixer`. Only [`register`] knows about the runtime crate; the rest
//! is runtime-neutral. See `docs/dsound.md`.
//!
//! - [`host`]: guest memory and event/callback abstractions.
//! - [`abi`]: constants and guest structure layouts.
//! - [`state`]: the object model and every API implementation.
//! - [`overrides`]: address table and dispatcher for the override layer.
//! - [`register`]: installs [`overrides`] into a runtime `Dispatch`.
//! - [`padroute`]: recognising sounds by their bytes and routing chosen ones to a controller speaker.
//! - [`output`]: the output thread, the one consumer that takes the mixer to a sink.
//! - [`testing`]: a synthetic guest memory for tests.
//! - [`xwma`]: the native WMA decoder at the game's codec boundary (`0x38a0c5`).

pub mod abi;
pub mod host;
pub mod output;
pub mod overrides;
pub mod padroute;
pub mod register;
pub mod state;
pub mod testing;
pub mod xwma;

pub use host::{EventSink, GuestMemory, Host, MemFault, StreamCallback};
pub use output::{Output, OutputConfig, OutputStats, Pacing, SideOpener, SideOutput, SideStats};
pub use register::{
    EventRef, Events, Hle, Pump, RegisterError, Registration, register, unregister,
};
pub use state::{
    AudioClock, Consumer, ConsumerConflict, DirectSound, EffectDesc, Layout, Lease, R, Stop,
};
