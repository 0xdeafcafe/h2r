//! Monitor looks and poses in a film (docs/game/forge-monitor-appearance.md,
//! "Theater", ML8): the two record subtypes of a Forge entry that carry them,
//! the recorder that writes them and its seek checkpoint, the playback that
//! reads them back into the seats the live session draws from, the
//! `monitor_look_source` reason, and the camera subjects a monitor seat
//! gives Theater.
//!
//! The records are presentation. They are outside the gameplay digest, so a
//! film verifies the same with or without them, and nothing here reads a
//! clock, a guest or a socket: the film's tick is the render clock
//! (30 a second), as the session's channel samples it.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use super::MonitorLook;
use super::body::{Bodies, Draw};
use super::session::{CanDraw, Pose, Seats};

/// The first byte of a Forge entry's payload: a seat's accepted look. The
/// welcome (0) and the stream item (1) are `forge::net::film`'s.
pub const LOOK: u8 = 2;
/// The first byte of a Forge entry's payload: a seat's monitor pose.
pub const POSE: u8 = 3;

/// The reason a playback machine gives for a look it cannot draw.
pub const REASON: &str = "monitor_look_source";

/// Ticks a second: the film's clock is the render clock the channel
/// samples.
pub const TICK_HZ: f32 = 30.;
/// The ticks between two pose records of a seat: 15 a second.
pub const POSE_EVERY: u32 = 2;

/// The longest look record a reader takes.
pub const MAX_LOOK_BYTES: usize = 4096;
/// A pose record: kind, seat, flags, tick, five floats.
const POSE_BYTES: usize = 1 + 1 + 1 + 4 + 5 * 4;

/// The source map a look's model was read from, as the recording machine's
/// catalogue names it: what a playback machine must hold to draw the look.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// The map's name, for the sentence.
    pub map: String,
    /// Lower-case hex SHA-256 of the map file.
    pub sha256: String,
}

/// A seat's look as the host sent it, at the moment the film took it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LookRecord {
    pub seat: u8,
    pub look: MonitorLook,
    #[serde(default)]
    pub source: Option<Source>,
}

/// A seat's monitor pose sampled at a film tick. `monitor: false` is the
/// player back in their biped: the body goes at once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PoseRecord {
    pub seat: u8,
    pub tick: u32,
    pub pose: Pose,
    pub monitor: bool,
}

/// A record of either subtype.
#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    Look(LookRecord),
    Pose(PoseRecord),
}

/// Why a record is not readable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    Empty,
    /// A subtype this reader does not take.
    Kind(u8),
    Size(usize),
    Look(String),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("empty monitor record"),
            Self::Kind(kind) => write!(f, "unknown monitor record kind {kind}"),
            Self::Size(n) => write!(f, "monitor record of {n} bytes is the wrong size"),
            Self::Look(why) => write!(f, "monitor look record is not readable: {why}"),
        }
    }
}

impl std::error::Error for ReadError {}

impl LookRecord {
    pub fn to_payload(&self) -> Vec<u8> {
        let mut out = vec![LOOK];
        // A look is plain data: serialising it cannot fail.
        out.extend(serde_json::to_vec(self).unwrap_or_default());
        out
    }
}

impl PoseRecord {
    pub fn to_payload(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(POSE_BYTES);
        out.push(POSE);
        out.push(self.seat);
        out.push(u8::from(self.monitor));
        out.extend(self.tick.to_le_bytes());
        for v in [
            self.pose.position[0],
            self.pose.position[1],
            self.pose.position[2],
            self.pose.yaw,
            self.pose.pitch,
        ] {
            out.extend(v.to_le_bytes());
        }
        out
    }
}

impl Record {
    /// Read a payload that leads with its subtype.
    pub fn from_payload(payload: &[u8]) -> Result<Record, ReadError> {
        let (&kind, bytes) = payload.split_first().ok_or(ReadError::Empty)?;
        match kind {
            LOOK => {
                if bytes.len() > MAX_LOOK_BYTES {
                    return Err(ReadError::Size(payload.len()));
                }
                let record: LookRecord =
                    serde_json::from_slice(bytes).map_err(|e| ReadError::Look(e.to_string()))?;
                record
                    .look
                    .validate()
                    .map_err(|e| ReadError::Look(e.sentence()))?;
                Ok(Record::Look(record))
            }
            POSE => {
                if payload.len() != POSE_BYTES {
                    return Err(ReadError::Size(payload.len()));
                }
                let f = |at: usize| {
                    let mut b = [0u8; 4];
                    b.copy_from_slice(&bytes[at..at + 4]);
                    f32::from_le_bytes(b)
                };
                let mut t = [0u8; 4];
                t.copy_from_slice(&bytes[2..6]);
                let read = |i: usize| f(6 + 4 * i);
                let values = [read(0), read(1), read(2), read(3), read(4)];
                if values.iter().any(|v| !v.is_finite()) {
                    return Err(ReadError::Look("a pose is not finite".into()));
                }
                Ok(Record::Pose(PoseRecord {
                    seat: bytes[0],
                    tick: u32::from_le_bytes(t),
                    pose: Pose {
                        position: [values[0], values[1], values[2]],
                        yaw: values[3],
                        pitch: values[4],
                    },
                    monitor: bytes[1] != 0,
                }))
            }
            other => Err(ReadError::Kind(other)),
        }
    }
}

/// The film's clock as the render clock.
pub fn clock_of(tick: u32) -> f32 {
    tick as f32 / TICK_HZ
}

/// Writes a session's looks and poses as film records, and repeats them at
/// each seek checkpoint. A machine records what it applies: the host's
/// table as it sent it, and the poses it sampled or relayed.
#[derive(Clone, Debug, Default)]
pub struct Recorder {
    looks: BTreeMap<u8, LookRecord>,
    last: BTreeMap<u8, PoseRecord>,
}

impl Recorder {
    /// An empty recorder, for a static.
    pub const fn new() -> Recorder {
        Recorder {
            looks: BTreeMap::new(),
            last: BTreeMap::new(),
        }
    }

    /// A seat's accepted look. A record only when the table changed.
    pub fn look(&mut self, seat: u8, look: MonitorLook, source: Option<Source>) -> Option<Vec<u8>> {
        let record = LookRecord { seat, look, source };
        if self.looks.get(&seat) == Some(&record) {
            return None;
        }
        let payload = record.to_payload();
        self.looks.insert(seat, record);
        Some(payload)
    }

    /// A seat's pose at `tick`: at most [`POSE_EVERY`] ticks apart, a seat
    /// leaving its monitor at once.
    pub fn pose(&mut self, tick: u32, seat: u8, pose: Pose, monitor: bool) -> Option<Vec<u8>> {
        if let Some(last) = self.last.get(&seat)
            && last.monitor == monitor
            && tick >= last.tick
            && tick - last.tick < POSE_EVERY
        {
            return None;
        }
        let record = PoseRecord {
            seat,
            tick,
            pose,
            monitor,
        };
        self.last.insert(seat, record);
        Some(record.to_payload())
    }

    /// A seat left the session: its look and pose go.
    pub fn left(&mut self, seat: u8) {
        self.looks.remove(&seat);
        self.last.remove(&seat);
    }

    /// The records a seek checkpoint repeats: the table, then each seat's
    /// last pose, in seat order. A playback fed these and then the records
    /// after the checkpoint holds what one fed the whole film holds.
    pub fn checkpoint(&self) -> Vec<Vec<u8>> {
        self.looks
            .values()
            .map(LookRecord::to_payload)
            .chain(self.last.values().map(PoseRecord::to_payload))
            .collect()
    }
}

/// A look the playback machine cannot draw: the seat and the map it needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unseen {
    pub seat: u8,
    pub map: String,
    /// Always [`REASON`].
    pub reason: String,
}

impl Unseen {
    /// The sentence the Theater bar shows once.
    pub fn sentence(&self) -> String {
        format!(
            "Seat {} has a monitor look this machine cannot draw: it needs {}. The marker is drawn instead.",
            self.seat, self.map
        )
    }
}

/// How a camera takes a monitor seat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Follow,
    Orbit,
    /// The monitor's own camera pose, with no body drawn.
    FirstPerson,
}

/// What a camera on a monitor seat follows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Subject {
    pub seat: u8,
    /// The pose to follow (follow, orbit) or to look from (first person).
    pub pose: Pose,
    /// Whether the seat's own body is drawn: not in first person.
    pub body: bool,
}

/// A film's looks and poses read back into the seats the live session
/// draws from. A seek makes a new one and feeds it the checkpoint.
#[derive(Clone, Debug, Default)]
pub struct Playback {
    seats: Seats,
    looks: BTreeMap<u8, LookRecord>,
    announced: BTreeSet<u8>,
}

impl Playback {
    pub fn feed(&mut self, payload: &[u8]) -> Result<(), ReadError> {
        match Record::from_payload(payload)? {
            Record::Look(record) => {
                self.seats.look(record.seat, record.look.clone());
                self.looks.insert(record.seat, record);
            }
            Record::Pose(p) => self.seats.pose(p.seat, clock_of(p.tick), p.pose, p.monitor),
        }
        Ok(())
    }

    /// The table as the film has it so far, in seat order.
    pub fn table(&self) -> Vec<(u8, &MonitorLook)> {
        self.seats.looks().collect()
    }

    /// The seat's drawn pose at `tick`, as the live session's seats give
    /// it, and how faded it is.
    pub fn body(&self, seat: u8, tick: u32) -> Option<(Pose, f32)> {
        self.seats.body(seat, clock_of(tick))
    }

    /// The bodies drawn at `tick`, by the code the live session uses
    /// ([`Bodies::draw`]). `hide` is the seat a first-person camera looks
    /// from: its body is not drawn. A look this machine has no model for
    /// draws the marker in the look's colours.
    pub fn draws(
        &self,
        bodies: &Bodies,
        tick: u32,
        hide: Option<u8>,
        rgb: impl Fn(u8) -> [u8; 3],
    ) -> Vec<Draw> {
        let now = clock_of(tick);
        self.seats
            .looks()
            .filter(|(seat, _)| Some(*seat) != hide)
            .filter_map(|(seat, look)| {
                let (pose, _) = self.seats.body(seat, now)?;
                Some(bodies.draw(seat, look, pose, now, &rgb))
            })
            .collect()
    }

    /// Whether the seat is a monitor the film has spoken of.
    pub fn is_monitor(&self, seat: u8) -> bool {
        self.seats.is_monitor(seat)
    }

    /// What a camera on `seat` follows at `tick`: its body's pose for
    /// follow and orbit, its own camera pose for first person with no
    /// body. `None` while the seat is not a monitor at that moment.
    pub fn subject(&self, seat: u8, tick: u32, view: View) -> Option<Subject> {
        let (pose, _) = self.seats.pose_at(seat, clock_of(tick))?;
        Some(Subject {
            seat,
            pose,
            body: view != View::FirstPerson,
        })
    }

    /// The looks this machine cannot draw, by seat: each names the map it
    /// needs. A marker is always drawn.
    pub fn unseen(&self, can: &CanDraw) -> Vec<Unseen> {
        self.looks
            .values()
            .filter_map(|record| unseen_of(record, can))
            .collect()
    }

    /// The looks this machine cannot draw whose seat has not been
    /// announced: the Theater bar shows each once, when its seat first
    /// appears.
    pub fn announce(&mut self, can: &CanDraw) -> Vec<Unseen> {
        let mut out = Vec::new();
        for record in self.looks.values() {
            if self.announced.contains(&record.seat) {
                continue;
            }
            self.announced.insert(record.seat);
            out.extend(unseen_of(record, can));
        }
        out
    }
}

fn unseen_of(record: &LookRecord, can: &CanDraw) -> Option<Unseen> {
    let base = record.source.as_ref().map(|s| s.sha256.as_str());
    // `draws` refuses only for the renderer or a missing source map.
    match can.draws("this machine", &record.look, base) {
        Ok(()) => None,
        Err(_) => Some(Unseen {
            seat: record.seat,
            map: record
                .source
                .as_ref()
                .map(|s| s.map.clone())
                .unwrap_or_else(|| record.look.model.clone()),
            reason: REASON.into(),
        }),
    }
}

#[cfg(test)]
mod tests;
