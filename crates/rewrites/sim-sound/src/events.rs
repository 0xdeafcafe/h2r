//! What the sound table says happened, in the order it happened. The
//! host's mixer is fed from these and nothing else
//! (docs/engine/sound-instances.md, "Mixer feed").

/// Sim ticks per second: the retail tick rate the end ticks are computed at.
pub const TICKS_PER_SECOND: u32 = 30;

/// One thing that happened to one sound in the table
/// ([`crate::instances::Sounds`]): the host picks nothing the sim drew.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TableEvent {
    Started {
        handle: sim_world::Handle,
        tag: crate::instances::TagIndex,
        selection: crate::impulse::Selection,
        gain: f32,
        pitch: f32,
        source: crate::instances::SourceParams,
        /// A loop segment: linked to a loop's track.
        looping: bool,
        /// A loop's detail sound: the host places it (presentation, its own
        /// seed copy, the listener); the sim took no placement draw.
        detail: bool,
        /// `0x1251e0`'s level for the request, dB: the volume is `level + gain`.
        level: f32,
        /// Minimum and maximum distance ([`crate::instances::distances`]).
        distance: [f32; 2],
        /// The loop and track that hold it.
        track: Option<(sim_world::Handle, u8)>,
        /// The track sound it continues with no gap (a loop's next segment).
        follows: Option<sim_world::Handle>,
        tick: u32,
    },
    /// A stop: the sound fades for `fade_ticks`, then ends with `Faded`.
    Stopped { handle: sim_world::Handle, fade_ticks: u32, tick: u32 },
    Ended { handle: sim_world::Handle, reason: crate::owners::Reason, tick: u32 },
}

impl TableEvent {
    pub fn handle(&self) -> sim_world::Handle {
        match *self {
            TableEvent::Started { handle, .. }
            | TableEvent::Stopped { handle, .. }
            | TableEvent::Ended { handle, .. } => handle,
        }
    }
}
