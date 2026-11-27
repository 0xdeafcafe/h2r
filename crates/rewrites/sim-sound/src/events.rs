//! What the sound simulation says happened, in the order it happened. The
//! host's mixer is fed from these and nothing else (docs/engine/sound-sim.md).

/// Sim ticks per second: the retail tick rate the end ticks are computed at.
pub const TICKS_PER_SECOND: u32 = 30;

/// One started sound. Ids count up from 1 and are never reused in a run, so
/// a script's wait on a finished sound still finds it finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SoundId(pub u32);

/// The source of a sound: the guest's object or datum handle it was started
/// on (`u32::MAX` for a sound with no source, heard in the listener's ears).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Source(pub u32);

impl Source {
    pub const NONE: Source = Source(u32::MAX);
}

/// A sound's tag and what the sim needs of it. The adapter fills this in
/// from the tag; this crate reads no tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundDef {
    /// The sound tag's datum, passed through to the host unread.
    pub tag: u32,
    /// The length of the sound in milliseconds. `None` is a sound whose tag
    /// gave none: it ends on the tick it starts.
    pub duration_ms: Option<u32>,
    /// A looping sound has no end tick; only `stop` ends it.
    pub looping: bool,
}

/// One thing that happened to one sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundEvent {
    Started { id: SoundId, source: Source, tag: u32, looping: bool, tick: u32 },
    /// The computed end tick came.
    Ended { id: SoundId, tick: u32 },
    /// Something stopped it before its end tick (or a loop's owner did).
    Stopped { id: SoundId, tick: u32 },
}

impl SoundEvent {
    pub fn id(&self) -> SoundId {
        match *self {
            SoundEvent::Started { id, .. }
            | SoundEvent::Ended { id, .. }
            | SoundEvent::Stopped { id, .. } => id,
        }
    }
}

/// One thing that happened to one sound in the table
/// ([`crate::instances::Sounds`]). Replaces [`SoundEvent`] when the table
/// is wired in (docs/engine/sound-instances.md, "Mixer feed"): the host
/// picks nothing the sim drew.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TableEvent {
    Started {
        handle: sim_world::Handle,
        tag: crate::instances::TagIndex,
        selection: crate::impulse::Selection,
        gain: f32,
        pitch: f32,
        source: crate::instances::SourceParams,
        /// A loop segment: only its loop ends it.
        looping: bool,
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
