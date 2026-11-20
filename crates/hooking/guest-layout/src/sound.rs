//! Guest looping-sound and source records (docs/runtime/agent-api.md,
//! "audio.loops"). The sound frame at 0x125b40 visits the loops at
//! 0x21b0e0 before updating the sound instances and channels they use.

use crate::Datum;

crate::layout! {
    /// A datum in the looping-sound array allocated at 0x21a0c0.
    pub struct LoopingSound: 0xd4 {
        TAG tag @ 0x4: Datum,
    }
}

impl LoopingSound {
    /// The three runtime tracks occupy the rows before the detail sound
    /// timers at +0xc4 (0x21a587, 0x21a3a3).
    pub const TRACK_COUNT: u8 = 3;
    const TRACKS: u32 = 0x88;

    pub fn track(&self, index: u8) -> Option<LoopingTrack> {
        (index < Self::TRACK_COUNT).then(|| {
            LoopingTrack::at(
                self.addr
                    .wrapping_add(Self::TRACKS + u32::from(index) * LoopingTrack::SIZE),
            )
        })
    }
}

crate::layout! {
    /// The state of one track in a looping-sound datum (0x21a587, 0x21b66a).
    pub struct LoopingTrack: 0x14 {
        /// Head of the linked sound instances, including overlapping fades.
        HEAD head @ 0x0: Datum,
        /// The selected segment used to resolve a track's sound tag (0x21a75e).
        SEGMENT segment @ 0xc: u8,
    }
}

crate::layout! {
    /// The relevant fields in a sound instance (0x21b940, 0x21c400).
    pub struct SoundInstance: 0xbc {
        TAG tag @ 0xc: Datum,
        LOOP_OWNER loop_owner @ 0x10: Datum,
        TRACK track @ 0x60: u8,
        /// The next overlapping instance on this track (0x21b8bf).
        NEXT_ON_TRACK next_on_track @ 0x6c: Datum,
        /// Pitch range and permutation are offsets within the tag and the
        /// selected pitch range, respectively. The chunk is within the permutation.
        PITCH_RANGE pitch_range @ 0x9c: i8,
        PERMUTATION permutation @ 0x9d: i8,
        CHUNK chunk @ 0x9e: i16,
        /// Sound-channel slot. -1 when no channel is assigned (0x127350).
        CHANNEL channel @ 0xac: i16,
    }
}

crate::layout! {
    /// The loaded `lsnd` tag header, read at 0x21a551 and 0x21a578.
    /// See crates/formats/tags/src/defs/lsnd.txt.
    pub struct LoopingSoundTag: 0x2c {
        TRACK_COUNT track_count @ 0x1c: i32,
        TRACKS tracks @ 0x20: u32,
    }
}

crate::layout! {
    /// The track's segment sound datums, each the second word of a tag reference.
    pub struct LoopingTrackTag: 0x58 {
        IN in_sound @ 0x18: Datum,
        LOOP loop_sound @ 0x20: Datum,
        OUT out_sound @ 0x28: Datum,
        ALT_LOOP alt_loop_sound @ 0x30: Datum,
        ALT_OUT alt_out_sound @ 0x38: Datum,
        ALT_IN alt_in_sound @ 0x44: Datum,
        ALT_TRANSITION_OUT alt_transition_out_sound @ 0x4c: Datum,
    }
}

crate::layout! {
    /// A mixer channel record indexed by a sound instance (0x129f2f).
    pub struct SoundChannel: 0x24 {
        OWNER owner @ 0x0: Datum,
        STREAM stream @ 0xc: i16,
    }
}

crate::layout! {
    /// A guest stream record in the sound driver (0x21f8b1, 0x2ae68d).
    pub struct SoundStream: 0x34 {
        /// IDirectSoundStream object.
        OBJECT object @ 0x24: u32,
    }
}

impl SoundStream {
    /// The driver stream array starts after the header at the global pointer.
    pub const ARRAY_OFFSET: u32 = 0xc;
}
