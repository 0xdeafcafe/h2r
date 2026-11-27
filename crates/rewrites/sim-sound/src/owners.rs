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
    /// A sound a start belongs to (`0x44a1c0`, made by `0x21dc60`): the
    /// start (payload `+0x28`) and the owner the request named (its table
    /// kept at payload `+0x2c`). Alive while the start is not stopping
    /// (`0x21d9f0`); the mouth goes to `own` (`0x21da50`); its end tells
    /// `own` and stops the start (`0x21da90`); its stop does the same with
    /// reason 1 and gives the sound back to `own` (`0x21db30`).
    Start { start: Handle, own: Own },
    /// A sound a start's component admitted (`0x44a1e0`): alive while its
    /// start is live and not done (`0x21d970`); nothing hears its end.
    Started(Handle),
}

/// The owner a start's sound had before the start took it over: the ones a
/// request record can name (finish `0x18c8c0`, `0x18c910`, or no table).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Own {
    #[default]
    None,
    Unit(u32),
    ScriptLine(Option<u32>),
}

impl Own {
    pub fn owner(self) -> Owner {
        match self {
            Own::None => Owner::None,
            Own::Unit(object) => Owner::Unit(object),
            Own::ScriptLine(object) => Owner::ScriptLine(object),
        }
    }
}

impl Owner {
    /// The unit an owner speaks through, if any: the one that gets the
    /// mouth and the dialogue end.
    pub fn unit(&self) -> Option<u32> {
        match *self {
            Owner::Unit(object) => Some(object),
            Owner::ScriptLine(object) => object,
            Owner::Start { own, .. } => own.owner().unit(),
            Owner::None | Owner::Loop { .. } | Owner::Started(_) => None,
        }
    }

    /// True for the owners [`OwnerHost`] is called for.
    pub fn has_host(&self) -> bool {
        matches!(self, Owner::Unit(_) | Owner::ScriptLine(_))
    }

    /// The owner [`OwnerHost`] hears for this one, if any: itself, or the
    /// owner a start's sound keeps.
    pub fn host(&self) -> Option<Owner> {
        let owner = match *self {
            Owner::Start { own, .. } => own.owner(),
            other => other,
        };
        owner.has_host().then_some(owner)
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
            Owner::Start { start, own } => {
                w.u8(4);
                w.u32(start.raw());
                own.owner().write(w);
            }
            Owner::Started(start) => {
                w.u8(5);
                w.u32(start.raw());
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
            4 => {
                let start = Handle::from_raw(r.u32()?);
                let own = match Owner::read(r)? {
                    Owner::None => Own::None,
                    Owner::Unit(object) => Own::Unit(object),
                    Owner::ScriptLine(object) => Own::ScriptLine(object),
                    _ => return Err(bad("start owner's own owner")),
                };
                Owner::Start { start, own }
            }
            5 => Owner::Started(Handle::from_raw(r.u32()?)),
            _ => return Err(bad("owner kind")),
        })
    }
}

/// Why an instance finished: the engine's reason codes, passed to the
/// owner's `+0x10` callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Reason {
    /// A start's sound was stopped (`0x21db30`): told to the owner the
    /// start keeps, never a sound's end, so not in [`Reason::ALL`].
    Stopped = 1,
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
            Reason::Stopped => "stopped",
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
            Owner::Start { start: Handle::new(0xef73, 1), own: Own::ScriptLine(Some(9)) },
            Owner::Start { start: Handle::new(0xef73, 1), own: Own::None },
            Owner::Started(Handle::new(0xef74, 2)),
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
        // A start's sound speaks through the owner it keeps.
        let wrapped = Owner::Start { start: Handle::new(0xef73, 1), own: Own::Unit(4) };
        assert_eq!((wrapped.unit(), wrapped.host(), wrapped.has_host()), (Some(4), Some(Owner::Unit(4)), false));
        assert_eq!(Owner::Started(Handle::NONE).host(), None);
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
