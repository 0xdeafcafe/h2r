//! Snapshot and delta semantics, independent of the engine's object lifetimes.

use std::fmt;

use crate::newer_tick;
use crate::wire::{Actor, ActorChange, MAX_ACTORS, Packet, Record, WireError, record_len};

#[derive(Debug, PartialEq, Eq)]
pub enum SetError {
    InvalidWire(WireError),
    /// A creation with the set already full can follow a lost removal.
    /// The receiver can recover from the next full key.
    Full,
}

impl fmt::Display for SetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidWire(err) => fmt::Display::fmt(err, f),
            Self::Full => write!(f, "actor set is full ({MAX_ACTORS} live actors)"),
        }
    }
}

impl std::error::Error for SetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidWire(err) => Some(err),
            Self::Full => None,
        }
    }
}

impl From<WireError> for SetError {
    fn from(err: WireError) -> Self {
        Self::InvalidWire(err)
    }
}

/// The durable part of the channel: membership, posture and whole mode state.
/// Actors are kept in entity order so a key is independent of insertion order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActorSet {
    actors: Vec<Actor>,
    state: Vec<u8>,
}

impl ActorSet {
    pub fn actors(&self) -> &[Actor] {
        &self.actors
    }

    pub fn state(&self) -> &[u8] {
        &self.state
    }

    pub fn key(&self) -> Record {
        Record::Key {
            actors: self.actors.clone(),
            state: self.state.clone(),
        }
    }

    /// Apply one host change or restore a full key. A posture for an absent
    /// actor cannot create it. Lines, controls and digests are transient.
    pub fn apply(&mut self, record: &Record) -> Result<(), SetError> {
        record_len(record)?;
        self.apply_validated(record)
    }

    fn apply_validated(&mut self, record: &Record) -> Result<(), SetError> {
        match record {
            Record::Key { actors, state } => {
                self.actors.clone_from(actors);
                self.actors.sort_unstable_by_key(|actor| actor.entity);
                self.state.clone_from(state);
            }
            Record::Actor(ActorChange::Upsert(actor)) => match self.index(actor.entity) {
                Ok(index) => self.actors[index] = *actor,
                Err(_) if self.actors.len() >= MAX_ACTORS => return Err(SetError::Full),
                Err(index) => self.actors.insert(index, *actor),
            },
            Record::Actor(ActorChange::Remove(entity)) => {
                if let Ok(index) = self.index(*entity) {
                    self.actors.remove(index);
                }
            }
            Record::Posture { entity, posture } => {
                if let Ok(index) = self.index(*entity) {
                    self.actors[index].posture = *posture;
                }
            }
            Record::State(state) => self.state.clone_from(state),
            Record::Control { .. } | Record::Line { .. } | Record::Digest { .. } => {}
        }
        Ok(())
    }

    fn index(&self, entity: u32) -> Result<usize, usize> {
        self.actors
            .binary_search_by_key(&entity, |actor| actor.entity)
    }
}

/// One client's ordered receive state for one session. Start a fresh replica
/// when the session changes. A delayed key must never rewind the puppet table.
#[derive(Debug, Default)]
pub struct Replica {
    set: ActorSet,
    last_tick: Option<u32>,
}

impl Replica {
    pub fn set(&self) -> &ActorSet {
        &self.set
    }

    /// Returns true once for each accepted packet. Dispatch transient records
    /// only on true. Copies of a line in different packets share its original
    /// (entity, vocalization, tick), which identifies one playback.
    pub fn apply(&mut self, packet: &Packet) -> Result<bool, SetError> {
        if self
            .last_tick
            .is_some_and(|prev| !newer_tick(packet.tick, prev))
        {
            return Ok(false);
        }
        packet.encoded_len()?;
        let mut next = self.set.clone();
        for record in &packet.records {
            next.apply_validated(record)?;
        }
        self.set = next;
        self.last_tick = Some(packet.tick);
        Ok(true)
    }
}
