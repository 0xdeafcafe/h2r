//! Who a sound belongs to, and what the sim tells them
//! (docs/engine/sound-instances.md, "Owners").
//!
//! Retail keeps a pointer to a table of callbacks per instance (`+0x14`)
//! with the owner at `+0x10` and a payload at `+0x5C`. The tables are a
//! closed set, so the Rust form is the [`Owner`] enum and the one trait
//! [`OwnerHost`]. [`crate::instances::Sounds`] calls the host in retail's
//! order: `ended` with the instance still in the pool, then the slot is
//! freed (`0x127320`, `0x127390`).

use sim_world::{Handle, Reader, RestoreError, Writer};

use crate::impulse::Selection;
use crate::instances::TagIndex;

/// What an instance belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Owner {
    /// No callback table: nothing hears its end.
    #[default]
    None,
    /// A unit's dialogue line (finish `0x18c8c0`, update `0x18c6a0`). The
    /// value is the object's datum.
    Unit(u32),
    /// A script's line (finish `0x18c910`): the unit work when it has a
    /// unit, then the line slot that holds the handle is cleared.
    ScriptLine(Option<u32>),
    /// One track of a looping sound (the track's list, `+0x6C`). Its end
    /// unlinks it from the track; the host hears nothing.
    Loop { looping: Handle, track: u8 },
}

impl Owner {
    /// The unit an owner speaks through, if any: the one that gets the
    /// mouth and the dialogue end.
    pub fn unit(&self) -> Option<u32> {
        match *self {
            Owner::Unit(object) => Some(object),
            Owner::ScriptLine(object) => object,
            Owner::None | Owner::Loop { .. } => None,
        }
    }

    /// True for the owners [`OwnerHost`] is called for.
    pub fn has_host(&self) -> bool {
        matches!(self, Owner::Unit(_) | Owner::ScriptLine(_))
    }

    pub(crate) fn write(&self, w: &mut Writer) {
        match *self {
            Owner::None => w.u8(0),
            Owner::Unit(object) => {
                w.u8(1);
                w.u32(object);
            }
            Owner::ScriptLine(object) => {
                w.u8(2);
                w.u32(object.unwrap_or(u32::MAX));
                w.u8(u8::from(object.is_some()));
            }
            Owner::Loop { looping, track } => {
                w.u8(3);
                w.u32(looping.raw());
                w.u8(track);
            }
        }
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self, RestoreError> {
        let bad = |what| RestoreError::Corrupt { kind: sim_world::kind::SOUND, what };
        Ok(match r.u8()? {
            0 => Owner::None,
            1 => Owner::Unit(r.u32()?),
            2 => {
                let object = r.u32()?;
                match r.u8()? {
                    0 => Owner::ScriptLine(None),
                    1 => Owner::ScriptLine(Some(object)),
                    _ => return Err(bad("script line owner flag")),
                }
            }
            3 => Owner::Loop { looping: Handle::from_raw(r.u32()?), track: r.u8()? },
            _ => return Err(bad("owner kind")),
        })
    }
}

/// Why an instance finished: the engine's reason codes, passed to the
/// owner's `+0x10` callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Reason {
    /// Its computed end tick came.
    PlayedOut = 2,
    /// [`OwnerHost::alive`] said the owner is gone.
    OwnerGone = 3,
    /// A stop's fade reached the floor.
    Faded = 4,
    /// The stop-all walk `0x126400` finished every sound.
    Cleared = 0xA,
    /// Another sound took its place.
    Superseded = 0xF,
}

impl Reason {
    pub const ALL: [Reason; 5] =
        [Reason::PlayedOut, Reason::OwnerGone, Reason::Faded, Reason::Cleared, Reason::Superseded];

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn name(self) -> &'static str {
        match self {
            Reason::PlayedOut => "played_out",
            Reason::OwnerGone => "owner_gone",
            Reason::Faded => "faded",
            Reason::Cleared => "cleared",
            Reason::Superseded => "superseded",
        }
    }
}

/// The owners' side of the table. Called only for owners with
/// [`Owner::has_host`]; the others are the table's own business.
///
/// | Retail slot | When | Method |
/// |---|---|---|
/// | `+0` | per-instance walk; false ends the sound with reason 3 | [`OwnerHost::alive`] |
/// | `+8` | once a tick per running instance | [`OwnerHost::mouth`] |
/// | `+0x10` | in the finish, before the slot is freed | [`OwnerHost::ended`] |
/// | `+0x14` | in the stop, before the fade starts | [`OwnerHost::stopping`] |
pub trait OwnerHost {
    fn alive(&mut self, owner: &Owner) -> bool;
    /// `ticks` is the ticks since the instance started.
    fn mouth(&mut self, owner: &Owner, tag: TagIndex, selection: Selection, ticks: u32);
    /// The instance is still live in `sounds` while this runs.
    fn ended(
        &mut self,
        sounds: &crate::instances::Sounds,
        owner: &Owner,
        handle: Handle,
        tag: TagIndex,
        reason: Reason,
    );
    fn stopping(&mut self, owner: &Owner, handle: Handle, tag: TagIndex);
}

/// Owners that are always alive and hear nothing: the headless host.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullOwners;

impl OwnerHost for NullOwners {
    fn alive(&mut self, _: &Owner) -> bool {
        true
    }
    fn mouth(&mut self, _: &Owner, _: TagIndex, _: Selection, _: u32) {}
    fn ended(&mut self, _: &crate::instances::Sounds, _: &Owner, _: Handle, _: TagIndex, _: Reason) {}
    fn stopping(&mut self, _: &Owner, _: Handle, _: TagIndex) {}
}

/// Where an instance is in its life.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Playing,
    /// Stopped on `since`; ends with [`Reason::Faded`] on `until`.
    Fading { since: u32, until: u32 },
}

/// The fade rule on ticks: a fading instance is done once `tick` reaches
/// the end of its ramp. A zero-length fade ends on the tick it was stopped.
pub fn ramp_done(phase: Phase, tick: u32) -> bool {
    match phase {
        Phase::Playing => false,
        Phase::Fading { until, .. } => tick >= until,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owners_round_trip_and_name_their_unit() {
        let all = [
            Owner::None,
            Owner::Unit(0xe001_0004),
            Owner::ScriptLine(None),
            Owner::ScriptLine(Some(7)),
            Owner::Loop { looping: Handle::new(0x8001, 3), track: 2 },
        ];
        for owner in all {
            let mut w = Writer::new();
            owner.write(&mut w);
            let bytes = w.finish();
            let mut r = Reader::new(&bytes);
            assert_eq!(Owner::read(&mut r).unwrap(), owner);
            assert!(r.is_empty());
        }
        assert_eq!(Owner::ScriptLine(Some(7)).unit(), Some(7));
        assert_eq!(Owner::Loop { looping: Handle::NONE, track: 0 }.unit(), None);
        assert!(!Owner::None.has_host());
    }

    #[test]
    fn a_fade_is_done_at_the_end_of_its_ramp() {
        let phase = Phase::Fading { since: 10, until: 13 };
        assert!(!ramp_done(Phase::Playing, u32::MAX));
        assert!(!ramp_done(phase, 12));
        assert!(ramp_done(phase, 13));
        assert!(ramp_done(Phase::Fading { since: 5, until: 5 }, 5));
    }
}
