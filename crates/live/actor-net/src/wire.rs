//! Version 1 actor-channel datagrams. Integers are little endian. Each packet
//! starts with the version, a 32-bit simulation tick and an 8-bit record count.
//! Records have a one-byte tag and a payload whose size follows from the tag.

use std::f32::consts::PI;
use std::fmt;

pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 6;
pub const MAX_PACKET_BYTES: usize = 1_000;
pub const MAX_RECORDS: usize = 64;
pub const MAX_ACTORS: usize = 32;
pub const MAX_STATE_BYTES: usize = 63;

const KEY: u8 = 0;
const ACTOR: u8 = 1;
const POSTURE: u8 = 2;
const CONTROL: u8 = 3;
const LINE: u8 = 4;
const STATE: u8 = 5;
const DIGEST: u8 = 6;
const ACTOR_LEN: usize = 10;
const KEY_PREFIX_AND_LENGTHS: usize = 3;
pub(crate) const MAX_KEY_RECORD_BYTES: usize =
    KEY_PREFIX_AND_LENGTHS + ACTOR_LEN * MAX_ACTORS + MAX_STATE_BYTES;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Actor {
    pub entity: u32,
    pub character: u16,
    pub squad: u16,
    pub team: u8,
    pub posture: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorChange {
    Upsert(Actor),
    Remove(u32),
}

/// Two normalized throttle axes, a crouch flag and a signed facing delta.
/// Each axis and the angle use 31 evenly spaced values, including exact zero.
/// Code 31 is reserved in each five-bit field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Control(u16);

impl Control {
    /// Clamp the axes to [-1, 1] and the facing delta to [-π, π]. NaN is neutral.
    pub fn quantize(forward: f32, side: f32, crouch: bool, facing_delta_radians: f32) -> Self {
        fn axis(value: f32) -> u16 {
            let value = if value.is_nan() {
                0.0
            } else {
                value.clamp(-1.0, 1.0)
            };
            ((value * 15.0).round() as i16 + 15) as u16
        }

        Self(
            axis(forward)
                | axis(side) << 5
                | u16::from(crouch) << 10
                | axis(facing_delta_radians / PI) << 11,
        )
    }

    fn from_bits(bits: u16) -> Result<Self, WireError> {
        if [0u32, 5, 11]
            .into_iter()
            .any(|shift| (bits >> shift) & 31 == 31)
        {
            return Err(WireError::InvalidControl);
        }
        Ok(Self(bits))
    }

    fn axis(self, shift: u32) -> f32 {
        (f32::from((self.0 >> shift) & 31) - 15.0) / 15.0
    }

    pub fn forward(self) -> f32 {
        self.axis(0)
    }

    pub fn side(self) -> f32 {
        self.axis(5)
    }

    pub fn crouch(self) -> bool {
        self.0 & (1 << 10) != 0
    }

    pub fn facing_delta_radians(self) -> f32 {
        self.axis(11) * PI
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Record {
    Key {
        actors: Vec<Actor>,
        state: Vec<u8>,
    },
    Actor(ActorChange),
    Posture {
        entity: u32,
        posture: u8,
    },
    Control {
        entity: u32,
        control: Control,
    },
    Line {
        entity: u32,
        vocalization: u16,
        tick: u32,
    },
    State(Vec<u8>),
    Digest {
        log_head: [u8; 32],
        state_hash: u64,
        actor_count: u16,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Packet {
    pub tick: u32,
    pub records: Vec<Record>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    PacketTooLarge,
    TooManyRecords,
    TooManyActors,
    StateTooLarge,
    InvalidEntity,
    DuplicateEntity,
    InvalidControl,
    InvalidActorChange(u8),
    UnknownRecord(u8),
    UnsupportedVersion(u8),
    Truncated,
    TrailingBytes,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "actor-channel wire error: {self:?}")
    }
}

impl std::error::Error for WireError {}

fn check_entity(entity: u32) -> Result<(), WireError> {
    if entity == u32::MAX {
        return Err(WireError::InvalidEntity);
    }
    Ok(())
}

fn check_state(len: usize) -> Result<(), WireError> {
    if len > MAX_STATE_BYTES {
        return Err(WireError::StateTooLarge);
    }
    Ok(())
}

/// Exact size including the record tag. Collection lengths are bounded before
/// validation scans the contents or computes the size.
pub(crate) fn record_len(record: &Record) -> Result<usize, WireError> {
    Ok(match record {
        Record::Key { actors, state } => {
            if actors.len() > MAX_ACTORS {
                return Err(WireError::TooManyActors);
            }
            check_state(state.len())?;
            for (i, actor) in actors.iter().enumerate() {
                check_entity(actor.entity)?;
                if actors[..i].iter().any(|other| other.entity == actor.entity) {
                    return Err(WireError::DuplicateEntity);
                }
            }
            KEY_PREFIX_AND_LENGTHS + ACTOR_LEN * actors.len() + state.len()
        }
        Record::Actor(ActorChange::Upsert(actor)) => {
            check_entity(actor.entity)?;
            2 + ACTOR_LEN
        }
        Record::Actor(ActorChange::Remove(entity)) => {
            check_entity(*entity)?;
            6
        }
        Record::Posture { entity, .. } => {
            check_entity(*entity)?;
            6
        }
        Record::Control { entity, .. } => {
            check_entity(*entity)?;
            7
        }
        Record::Line { entity, .. } => {
            check_entity(*entity)?;
            11
        }
        Record::State(state) => {
            check_state(state.len())?;
            2 + state.len()
        }
        Record::Digest { actor_count, .. } => {
            if usize::from(*actor_count) > MAX_ACTORS {
                return Err(WireError::TooManyActors);
            }
            43
        }
    })
}

fn put_actor(out: &mut Vec<u8>, actor: &Actor) {
    out.extend(actor.entity.to_le_bytes());
    out.extend(actor.character.to_le_bytes());
    out.extend(actor.squad.to_le_bytes());
    out.extend([actor.team, actor.posture]);
}

fn put_state(out: &mut Vec<u8>, state: &[u8]) {
    out.push(state.len() as u8);
    out.extend_from_slice(state);
}

fn put_record(out: &mut Vec<u8>, record: &Record) {
    match record {
        Record::Key { actors, state } => {
            out.extend([KEY, actors.len() as u8]);
            for actor in actors {
                put_actor(out, actor);
            }
            put_state(out, state);
        }
        Record::Actor(ActorChange::Upsert(actor)) => {
            out.extend([ACTOR, 1]);
            put_actor(out, actor);
        }
        Record::Actor(ActorChange::Remove(entity)) => {
            out.extend([ACTOR, 0]);
            out.extend(entity.to_le_bytes());
        }
        Record::Posture { entity, posture } => {
            out.push(POSTURE);
            out.extend(entity.to_le_bytes());
            out.push(*posture);
        }
        Record::Control { entity, control } => {
            out.push(CONTROL);
            out.extend(entity.to_le_bytes());
            out.extend(control.0.to_le_bytes());
        }
        Record::Line {
            entity,
            vocalization,
            tick,
        } => {
            out.push(LINE);
            out.extend(entity.to_le_bytes());
            out.extend(vocalization.to_le_bytes());
            out.extend(tick.to_le_bytes());
        }
        Record::State(state) => {
            out.push(STATE);
            put_state(out, state);
        }
        Record::Digest {
            log_head,
            state_hash,
            actor_count,
        } => {
            out.push(DIGEST);
            out.extend_from_slice(log_head);
            out.extend(state_hash.to_le_bytes());
            out.extend(actor_count.to_le_bytes());
        }
    }
}

impl Packet {
    /// Validate the complete packet and return its wire size, including the header.
    pub fn encoded_len(&self) -> Result<usize, WireError> {
        if self.records.len() > MAX_RECORDS {
            return Err(WireError::TooManyRecords);
        }
        let mut len = HEADER_LEN;
        for record in &self.records {
            len += record_len(record)?;
            if len > MAX_PACKET_BYTES {
                return Err(WireError::PacketTooLarge);
            }
        }
        Ok(len)
    }
}

pub fn encode(packet: &Packet) -> Result<Vec<u8>, WireError> {
    let len = packet.encoded_len()?;
    let mut out = Vec::with_capacity(len);
    out.push(VERSION);
    out.extend(packet.tick.to_le_bytes());
    out.push(packet.records.len() as u8);
    for record in &packet.records {
        put_record(&mut out, record);
    }
    debug_assert_eq!(out.len(), len);
    Ok(out)
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], WireError> {
        let (bytes, rest) = self.0.split_first_chunk().ok_or(WireError::Truncated)?;
        self.0 = rest;
        Ok(*bytes)
    }

    fn byte(&mut self) -> Result<u8, WireError> {
        Ok(self.bytes::<1>()?[0])
    }

    fn actor(&mut self) -> Result<Actor, WireError> {
        Ok(Actor {
            entity: u32::from_le_bytes(self.bytes()?),
            character: u16::from_le_bytes(self.bytes()?),
            squad: u16::from_le_bytes(self.bytes()?),
            team: self.byte()?,
            posture: self.byte()?,
        })
    }

    fn state(&mut self) -> Result<Vec<u8>, WireError> {
        let len = usize::from(self.byte()?);
        check_state(len)?;
        let (bytes, rest) = self.0.split_at_checked(len).ok_or(WireError::Truncated)?;
        self.0 = rest;
        Ok(bytes.to_vec())
    }

    fn record(&mut self) -> Result<Record, WireError> {
        Ok(match self.byte()? {
            KEY => {
                let count = usize::from(self.byte()?);
                if count > MAX_ACTORS {
                    return Err(WireError::TooManyActors);
                }
                let actors = (0..count).map(|_| self.actor()).collect::<Result<_, _>>()?;
                Record::Key {
                    actors,
                    state: self.state()?,
                }
            }
            ACTOR => Record::Actor(match self.byte()? {
                0 => ActorChange::Remove(u32::from_le_bytes(self.bytes()?)),
                1 => ActorChange::Upsert(self.actor()?),
                action => return Err(WireError::InvalidActorChange(action)),
            }),
            POSTURE => Record::Posture {
                entity: u32::from_le_bytes(self.bytes()?),
                posture: self.byte()?,
            },
            CONTROL => Record::Control {
                entity: u32::from_le_bytes(self.bytes()?),
                control: Control::from_bits(u16::from_le_bytes(self.bytes()?))?,
            },
            LINE => Record::Line {
                entity: u32::from_le_bytes(self.bytes()?),
                vocalization: u16::from_le_bytes(self.bytes()?),
                tick: u32::from_le_bytes(self.bytes()?),
            },
            STATE => Record::State(self.state()?),
            DIGEST => Record::Digest {
                log_head: self.bytes()?,
                state_hash: u64::from_le_bytes(self.bytes()?),
                actor_count: u16::from_le_bytes(self.bytes()?),
            },
            tag => return Err(WireError::UnknownRecord(tag)),
        })
    }
}

pub fn decode(bytes: &[u8]) -> Result<Packet, WireError> {
    if bytes.len() > MAX_PACKET_BYTES {
        return Err(WireError::PacketTooLarge);
    }
    let mut reader = Reader(bytes);
    let version = reader.byte()?;
    if version != VERSION {
        return Err(WireError::UnsupportedVersion(version));
    }
    let tick = u32::from_le_bytes(reader.bytes()?);
    let count = usize::from(reader.byte()?);
    if count > MAX_RECORDS {
        return Err(WireError::TooManyRecords);
    }
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let record = reader.record()?;
        record_len(&record)?;
        records.push(record);
    }
    if !reader.0.is_empty() {
        return Err(WireError::TrailingBytes);
    }
    Ok(Packet { tick, records })
}

#[cfg(test)]
mod tests;
