//! The binary world stream negotiated over the JSON agent endpoint.
//! A length-prefixed frame carries independently length-prefixed layers.
//! All integers and IEEE 754 words on the wire are little endian.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read};

use serde::{Deserialize, Serialize};

mod fields;
use fields::{Field, Record};
#[cfg(test)]
mod tests;

pub const PROTOCOL: &str = "h2-live-mirror/1";
pub const SUBSCRIBE: &str = "live.subscribe";
pub const STATUS: &str = "live.status";
pub const CLOSE: &str = "live.close";
pub const METHODS: &[&str] = &[SUBSCRIBE, STATUS, CLOSE];

const OBJECTS: u8 = 1;
const PLAYERS: u8 = 2;
const MAX_FRAME: usize = 8 * 1024 * 1024;
const MAX_RECORDS: usize = u16::MAX as usize;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscribeParams {
    pub layers: Vec<String>,
    #[serde(default = "default_every")]
    pub every: u32,
    #[serde(default = "default_key_every")]
    pub key_every: u32,
}

fn default_every() -> u32 {
    1
}
fn default_key_every() -> u32 {
    150
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Layers {
    pub objects: bool,
    pub players: bool,
}

impl Layers {
    pub fn negotiate(names: &[String]) -> (Self, BTreeMap<String, String>) {
        let mut result = Self::default();
        let mut refused = BTreeMap::new();
        for name in names {
            match name.as_str() {
                "objects" => result.objects = true,
                "players" => result.players = true,
                _ => {
                    refused.insert(name.clone(), "layer unavailable on this server".into());
                }
            }
        }
        (result, refused)
    }

    pub fn names(self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.objects {
            names.push("objects");
        }
        if self.players {
            names.push("players");
        }
        names
    }
}

/// One complete capture. An object's position, forward and up are local
/// to its parent if it has one. Velocity is in world units per second.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct World {
    pub objects: Vec<Object>,
    pub players: Vec<Player>,
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Object {
    /// Full salted handle. Reuse of the low 16-bit slot creates a new id.
    pub id: u32,
    pub tag: Option<u32>,
    pub kind: u8,
    pub flags: u8,
    pub parent: Option<u32>,
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
    pub scale: f32,
    pub velocity: [f32; 3],
    pub body: f32,
    pub shield: f32,
    pub placement: Option<u16>,
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Player {
    pub index: u16,
    pub name: String,
    pub team: i16,
    pub unit: Option<u32>,
    pub held: Option<u32>,
    pub aim: [f32; 3],
    pub local_user: Option<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Kind {
    Key = 1,
    Delta = 2,
    Notice = 3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header {
    pub kind: Kind,
    pub seq: u64,
    pub tick: u64,
    pub epoch: u64,
    pub paused: bool,
    pub cut: bool,
}

/// Records with their changed-field masks, followed on the wire by the
/// ids of deleted records. New rows have every field bit set.
#[derive(Clone, Debug)]
struct Changes<R: Record> {
    rows: Vec<(u16, R)>,
    removed: Vec<R::Id>,
}

impl<R: Record> Changes<R> {
    fn between(previous: &[R], current: &[R]) -> Self {
        let mut old: BTreeMap<_, _> = previous.iter().map(|r| (r.id(), r)).collect();
        let mut rows = Vec::new();
        for row in current {
            let mask = old.remove(&row.id()).map_or(R::ALL, |old| row.diff(old));
            if mask != 0 {
                rows.push((mask, row.clone()));
            }
        }
        Self {
            rows,
            removed: old.into_keys().collect(),
        }
    }

    fn write(&self, out: &mut Vec<u8>) -> io::Result<()> {
        count(self.rows.len())?.write(out)?;
        for (mask, row) in &self.rows {
            row.id().write(out)?;
            mask.write(out)?;
            row.write_fields(*mask, out)?;
        }
        count(self.removed.len())?.write(out)?;
        for id in &self.removed {
            id.write(out)?;
        }
        Ok(())
    }

    fn read(mut input: &[u8], kind: Kind) -> io::Result<Self> {
        let changed = u16::read(&mut input)?;
        let mut rows = Vec::new();
        let mut ids = BTreeSet::new();
        for _ in 0..changed {
            let id = R::Id::read(&mut input)?;
            let mask = u16::read(&mut input)?;
            if mask == 0
                || mask & !R::ALL != 0
                || (kind == Kind::Key && mask != R::ALL)
                || (kind == Kind::Delta && mask & R::KEY_ONLY != 0)
                || !ids.insert(id)
            {
                return Err(invalid("invalid live record mask or duplicate id"));
            }
            rows.push((mask, R::read_fields(id, mask, &mut input)?));
        }
        let deletions = u16::read(&mut input)?;
        if kind == Kind::Key && deletions != 0 {
            return Err(invalid("key frame contains deletions"));
        }
        let mut removed = Vec::new();
        for _ in 0..deletions {
            let id = R::Id::read(&mut input)?;
            if !ids.insert(id) {
                return Err(invalid("duplicate live record id"));
            }
            removed.push(id);
        }
        if !input.is_empty() {
            return Err(invalid("trailing layer payload"));
        }
        Ok(Self { rows, removed })
    }

    fn applied(&self, base: &[R]) -> io::Result<Vec<R>> {
        let mut result: BTreeMap<_, R> = base.iter().map(|r| (r.id(), r.clone())).collect();
        for id in &self.removed {
            if result.remove(id).is_none() {
                return Err(invalid("missing deleted record"));
            }
        }
        for (mask, patch) in &self.rows {
            match result.entry(patch.id()) {
                std::collections::btree_map::Entry::Occupied(mut row) => {
                    row.get_mut().patch(*mask, patch);
                }
                std::collections::btree_map::Entry::Vacant(row) if *mask == R::ALL => {
                    row.insert(patch.clone());
                }
                _ => return Err(invalid("partial update without a base record")),
            }
        }
        if result.len() > MAX_RECORDS {
            return Err(invalid("too many live records"));
        }
        Ok(result.into_values().collect())
    }
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub header: Header,
    objects: Option<Changes<Object>>,
    players: Option<Changes<Player>>,
}

impl Frame {
    /// Compare with the last *sent* capture of this stream. A player
    /// arriving or changing names requires a key because deltas omit names.
    pub fn between(
        mut header: Header,
        layers: Layers,
        previous: Option<&World>,
        current: &World,
    ) -> Self {
        if header.kind == Kind::Notice {
            return Self {
                header,
                objects: None,
                players: None,
            };
        }
        if previous.is_none()
            || header.cut
            || (layers.players
                && current.players.iter().any(|p| {
                    previous
                        .and_then(|w| w.players.iter().find(|q| q.index == p.index))
                        .is_none_or(|q| q.name != p.name)
                }))
        {
            header.kind = Kind::Key;
        }
        let empty = World::default();
        let base = if header.kind == Kind::Key {
            &empty
        } else {
            previous.unwrap_or(&empty)
        };
        Self {
            header,
            objects: layers
                .objects
                .then(|| Changes::between(&base.objects, &current.objects)),
            players: layers
                .players
                .then(|| Changes::between(&base.players, &current.players)),
        }
    }

    /// Includes the u32 little-endian length, ready for a socket's write_all.
    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut out = vec![0; 4];
        out.push(self.header.kind as u8);
        self.header.seq.write(&mut out)?;
        self.header.tick.write(&mut out)?;
        self.header.epoch.write(&mut out)?;
        out.push(u8::from(self.header.paused) | (u8::from(self.header.cut) << 1));
        if let Some(objects) = &self.objects {
            write_layer(&mut out, OBJECTS, objects)?;
        }
        if let Some(players) = &self.players {
            write_layer(&mut out, PLAYERS, players)?;
        }
        let size = out.len() - 4;
        if size > MAX_FRAME {
            return Err(invalid("live frame too large"));
        }
        out[..4].copy_from_slice(&(size as u32).to_le_bytes());
        Ok(out)
    }

    /// Reads exactly one frame, including its length prefix. Unknown layer
    /// ids are skipped without interpreting their payload.
    pub fn read(source: &mut impl Read) -> io::Result<Self> {
        let mut prefix = [0; 4];
        source.read_exact(&mut prefix)?;
        let size = u32::from_le_bytes(prefix) as usize;
        if size > MAX_FRAME {
            return Err(invalid("live frame too large"));
        }
        let mut buf = vec![0; size];
        source.read_exact(&mut buf)?;
        let mut input = buf.as_slice();
        let kind = match u8::read(&mut input)? {
            1 => Kind::Key,
            2 => Kind::Delta,
            3 => Kind::Notice,
            _ => return Err(invalid("unknown live frame kind")),
        };
        let seq = u64::read(&mut input)?;
        let tick = u64::read(&mut input)?;
        let epoch = u64::read(&mut input)?;
        let flags = u8::read(&mut input)?;
        if flags & !3 != 0 || (kind == Kind::Delta && flags & 2 != 0) {
            return Err(invalid("invalid live frame flags"));
        }
        let header = Header {
            kind,
            seq,
            tick,
            epoch,
            paused: flags & 1 != 0,
            cut: flags & 2 != 0,
        };
        let mut frame = Self {
            header,
            objects: None,
            players: None,
        };
        while !input.is_empty() {
            let id = u8::read(&mut input)?;
            let len = u32::read(&mut input)? as usize;
            let (section, rest) = input
                .split_at_checked(len)
                .ok_or_else(|| invalid("truncated live layer"))?;
            input = rest;
            match id {
                OBJECTS if frame.objects.is_none() && kind != Kind::Notice => {
                    frame.objects = Some(Changes::read(section, kind)?);
                }
                PLAYERS if frame.players.is_none() && kind != Kind::Notice => {
                    frame.players = Some(Changes::read(section, kind)?);
                }
                OBJECTS | PLAYERS => return Err(invalid("unexpected live layer")),
                _ => {}
            }
        }
        Ok(frame)
    }

    /// Apply both layers atomically. Keys replace all state; delta rows
    /// are keyed by salted handle / player index. Outputs are sorted by id.
    /// The store checks the stream sequence and epoch before applying a delta.
    pub fn apply(&self, world: &mut World) -> io::Result<()> {
        let empty = World::default();
        let base = if self.header.kind == Kind::Key {
            &empty
        } else {
            &*world
        };
        let objects = self
            .objects
            .as_ref()
            .map(|s| s.applied(&base.objects))
            .transpose()?;
        let players = self
            .players
            .as_ref()
            .map(|s| s.applied(&base.players))
            .transpose()?;
        if self.header.kind == Kind::Key {
            *world = empty;
        }
        if let Some(objects) = objects {
            world.objects = objects;
        }
        if let Some(players) = players {
            world.players = players;
        }
        Ok(())
    }
}

fn count(n: usize) -> io::Result<u16> {
    u16::try_from(n).map_err(|_| invalid("too many live records"))
}

fn write_layer<R: Record>(out: &mut Vec<u8>, id: u8, changes: &Changes<R>) -> io::Result<()> {
    out.push(id);
    let offset = out.len();
    out.extend_from_slice(&[0; 4]);
    changes.write(out)?;
    let size =
        u32::try_from(out.len() - offset - 4).map_err(|_| invalid("live layer too large"))?;
    out[offset..offset + 4].copy_from_slice(&size.to_le_bytes());
    Ok(())
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
