//! Forge's synchronous network mode: the edit log on the network, roles,
//! holding and join in progress (docs/game/forge-design.md, "The edit log
//! and multiplayer").
//!
//! The protocol is pure: no clock, socket or guest. [`Host`] is the one
//! authority. It takes each machine's [`Up`] messages, admits or refuses
//! them, and writes one ordered stream of [`Stamped`] items, each with an
//! apply tick, that every machine (the host's own included) feeds to its
//! [`Peer`]. A peer hands its game the items due at each tick, in stream
//! order, and holds the tick until the host has said that nothing earlier
//! is still to come ([`Item::Through`]). So every machine applies every
//! item at the same tick, as the synchronous protocol needs.
//!
//! Document edits are F1 entries (`forge_doc::Entry`): their canonical
//! bytes are `host_log::Entry<forge_doc::Op>`'s with the author set, and
//! they form the hash chain. Holds, carries, releases, members and the
//! watermark ride the same stream but are not in the chain: they never
//! reach the document.
use std::collections::BTreeMap;
use std::fmt;

use forge_doc::{
    BlockKind, Edit, Endpoint, Entry, F32, ForgeDoc, LogHead, MAX_DOC_BYTES, MAX_ENTRY_BYTES,
    MAX_LOG_BYTES, Op, OpId, Placement, Proposal, Refusal, Sha256,
};
use serde::{Deserialize, Serialize};

use crate::Frame;

pub mod film;
mod host;
mod node;
mod peer;
mod state;
mod wire;

pub use host::{Host, Out};
pub use node::{HOST_SEAT, HoldRow, Lane, MemberRow, Node, NodeError, Report, Settled, Step};
pub use peer::{Peer, PeerError};
pub use state::State;
pub use wire::{BEAT_BYTES, Down, Framer, MAX_FRAME_BYTES, ToHost, frame};

/// Seats in a session: `coop.max_players` is at most 16.
pub const SEATS: u8 = 16;
/// Ticks between the host stamping an item and every machine applying it.
pub const APPLY_DELAY: u32 = 4;
/// Stream items a peer buffers ahead of the next one it is missing.
pub const MAX_AHEAD: u64 = 4096;
/// The longest member name, as a document's author.
pub const MAX_NAME: usize = 128;
/// The largest [`Up`] or [`Stamped`] message.
pub const MAX_MESSAGE_BYTES: usize = MAX_ENTRY_BYTES + 1024;
/// The largest [`Welcome`]: a document, its log and the session's tables.
pub const MAX_WELCOME_BYTES: usize = MAX_DOC_BYTES + MAX_LOG_BYTES + 1024 * 1024;

/// A player's place in the session, `0..SEATS`. The transport
/// authenticates it: a message's seat is the connection's, never a field
/// the sender writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seat(pub u8);

impl fmt::Display for Seat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "seat {}", self.0)
    }
}

/// What the host lets a player do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Every tool: edits, holds and carries.
    Editor,
    /// Plays the map as it is; no edit.
    Player,
    /// Watches; no edit.
    Spectator,
}

impl Role {
    /// The role may propose edits and hold things.
    pub fn edits(self) -> bool {
        matches!(self, Self::Editor)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Editor => "editor",
            Self::Player => "player",
            Self::Spectator => "spectator",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    /// The authenticated name, written as the author of the seat's entries.
    pub name: String,
    pub role: Role,
}

/// A held thing's frame, bit-exact: position, then yaw, pitch and roll.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Carry {
    pub position: [F32; 3],
    pub rotation: [F32; 3],
}

impl Carry {
    pub fn new(frame: Frame) -> Result<Self, Refusal> {
        let f = |v: [f32; 3]| -> Result<[F32; 3], Refusal> {
            Ok([F32::new(v[0])?, F32::new(v[1])?, F32::new(v[2])?])
        };
        Ok(Self {
            position: f(frame.position)?,
            rotation: f(frame.rotation)?,
        })
    }

    pub fn frame(self) -> Frame {
        Frame {
            position: self.position.map(F32::get),
            rotation: self.rotation.map(F32::get),
        }
    }
}

/// A seat's hold: the thing, and where it was last carried to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Held {
    pub target: Endpoint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<Carry>,
}

/// A machine to the host, on the reliable ordered channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Up {
    /// One edit. The host overwrites `author` with the seat's name.
    Propose { nonce: u32, proposal: Proposal },
    /// Pick a thing up.
    Hold { nonce: u32, target: Endpoint },
    /// Move the held thing. At most one a tick is kept; it never reaches
    /// the document.
    Carry { target: Endpoint, frame: Carry },
    /// Let go of the held thing without an edit. A drop that moves it is
    /// the edit's proposal, then this.
    Release,
}

impl Up {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Refusal> {
        decode(bytes, "message bytes", MAX_MESSAGE_BYTES)
    }
}

/// The host to one machine: the answer to a `Propose` or `Hold`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Reply {
    /// The stream item that carries it.
    Accepted {
        nonce: u32,
        n: u64,
    },
    Refused {
        nonce: u32,
        refusal: NetRefusal,
    },
}

/// One item of the host's stream, numbered from 1 in the order the host
/// wrote it. `tick` is the tick every machine applies it at; the ticks of
/// all but [`Item::Through`] never go backwards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stamped {
    pub n: u64,
    pub tick: u32,
    pub item: Item,
}

impl Stamped {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Refusal> {
        decode(bytes, "message bytes", MAX_MESSAGE_BYTES)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, Refusal> {
        encode(self, "message bytes", MAX_MESSAGE_BYTES)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Item {
    /// A document edit: the next link of the hash chain.
    Entry(Entry),
    /// `seat` picked `target` up.
    Hold { seat: Seat, target: Endpoint },
    /// `seat` moved its held thing.
    Carry {
        seat: Seat,
        target: Endpoint,
        frame: Carry,
    },
    /// `seat` let go.
    Release { seat: Seat },
    /// `seat` joined, or its role changed. A role that cannot edit lets go.
    Member { seat: Seat, member: Member },
    /// `seat` left; its hold ends.
    Left { seat: Seat },
    /// No item with a tick at or before the stamp's tick follows.
    Through,
}

/// What a late joiner is sent: the document as of the session's checkpoint
/// and the entries after it, up to the host's applied position, the tables
/// at that position, and the stream items still to apply.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Welcome {
    pub seat: Seat,
    /// The host's tick: every item at or before it is in the tables.
    pub tick: u32,
    /// The document the session's map was materialised from.
    pub start: ForgeDoc,
    pub start_head: LogHead,
    /// The entries after `start_head` up to `head`.
    pub entries: Vec<Entry>,
    pub head: LogHead,
    pub members: BTreeMap<Seat, Member>,
    pub holds: BTreeMap<Seat, Held>,
    /// The last stream item the tables include.
    pub n: u64,
    /// The host's watermark as of `n`.
    pub through: u32,
    /// The items after `n`, in order.
    pub pending: Vec<Stamped>,
}

impl Welcome {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Refusal> {
        decode(bytes, "welcome bytes", MAX_WELCOME_BYTES)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, Refusal> {
        encode(self, "welcome bytes", MAX_WELCOME_BYTES)
    }
}

/// A machine's position, sent in the session's keep-alive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Beat {
    pub head: LogHead,
    pub n: u64,
}

/// Why the host refused a message. The author sees it as a HUD notice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum NetRefusal {
    /// The seat has not joined.
    NotJoined,
    /// The seat's role cannot edit.
    Role { role: Role },
    /// Another seat holds the thing, or a thing of the block an edit names
    /// by index.
    Held { by: String },
    /// The seat holds nothing, or not that thing.
    NotHolding,
    /// An edit that names an entry by its index was made before a later
    /// entry: an add or a remove may have moved that index.
    Stale { proposed: u32, head: u32 },
    /// The proposal names a log position the host has not reached.
    Ahead { proposed: u32, head: u32 },
    /// A new op's ID is outside the seat's lane ([`lane`]).
    Lane { id: OpId },
    /// The document refuses the edit.
    Doc(Refusal),
    /// The loaded map refuses the edit (palette, budget, bounds).
    Map(String),
    /// The seat is out of range or taken.
    Seat,
    /// Another seat has the name, or it is not a name.
    Name,
    /// The machine's log head is not the host's at that position.
    HeadDiffers { seat: Seat, seq: u32 },
    /// The host's clock went backwards.
    Clock { now: u32, was: u32 },
}

impl NetRefusal {
    /// The name the agent API gives the refusal in its error's `data.code`
    /// (`agent_proto::forge::refusal`).
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotJoined => "forge_not_joined",
            Self::Role { .. } => "forge_role",
            Self::Held { .. } => "forge_held",
            Self::NotHolding => "forge_not_holding",
            Self::Stale { .. } => "forge_stale",
            Self::Ahead { .. } => "forge_ahead",
            Self::Lane { .. } => "forge_lane",
            Self::Doc(Refusal::Mismatch) => "forge_was",
            Self::Doc(Refusal::Print { .. }) => "forge_print",
            Self::Doc(_) => "forge_refused",
            Self::Map(_) => "forge_map",
            Self::Seat => "forge_seat",
            Self::Name => "forge_name",
            Self::HeadDiffers { .. } => "forge_head_differs",
            Self::Clock { .. } => "forge_clock",
        }
    }

    /// The sentence a caller of the agent API gets: the code, then the
    /// refusal's own words.
    pub fn named(&self) -> String {
        format!("{}: {self}", self.code())
    }
}

impl fmt::Display for NetRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotJoined => write!(f, "You are not in this Forge session."),
            Self::Role { role } => write!(f, "A {} cannot edit this map.", role.name()),
            Self::Held { by } => write!(f, "{by} is holding that."),
            Self::NotHolding => write!(f, "You are not holding that."),
            Self::Stale { proposed, head } => write!(
                f,
                "Someone else edited first (your edit was made at {proposed}, the map is at {head}). Try again."
            ),
            Self::Ahead { proposed, head } => {
                write!(f, "Edit made at {proposed}, but the host is at {head}.")
            }
            Self::Lane { id } => write!(f, "Operation {id} is not this seat's to make."),
            Self::Doc(refusal) => write!(f, "{refusal}"),
            Self::Map(reason) => write!(f, "{reason}"),
            Self::Seat => write!(f, "That seat is not free."),
            Self::Name => write!(f, "That name is taken or not a name."),
            Self::HeadDiffers { seat, seq } => {
                write!(
                    f,
                    "The Forge log on {seat} differs from the host's at {seq}."
                )
            }
            Self::Clock { now, was } => {
                write!(f, "The Forge clock went back from {was} to {now}.")
            }
        }
    }
}

impl std::error::Error for NetRefusal {}

impl From<Refusal> for NetRefusal {
    fn from(refusal: Refusal) -> Self {
        Self::Doc(refusal)
    }
}

/// The host's checks against the loaded map, beyond the document's own.
/// The game implements it over its session; a refusal names the reason.
pub trait Admit {
    /// The digest of the value an op's target holds in `doc`, for `was`.
    /// `None` when the op has no single target value.
    fn holds(&self, doc: &ForgeDoc, op: &Op) -> Result<Option<Sha256>, String> {
        let _ = (doc, op);
        Ok(None)
    }

    /// Check the document an edit leaves: palette, budget, bounds.
    fn admit(&self, doc: &ForgeDoc) -> Result<(), String> {
        let _ = doc;
        Ok(())
    }
}

/// The document's checks alone.
#[derive(Clone, Copy, Debug, Default)]
pub struct DocOnly;

impl Admit for DocOnly {}

/// Whether `id` is in `seat`'s lane: new op IDs of seat `s` are `s + 1`,
/// `s + 17`, `s + 33` and so on, so two machines never pick the same one.
/// IDs at or below the starting document's largest are the starting
/// document's (a redo of one passes the fold's own check).
pub fn lane(seat: Seat, id: OpId, start_max: u32) -> bool {
    id.0 <= start_max || (id.0 - 1) % u32::from(SEATS) == u32::from(seat.0)
}

/// The next ID in `seat`'s lane above `above`.
pub fn next_in_lane(seat: Seat, above: u32) -> Option<OpId> {
    let seats = u32::from(SEATS);
    let first = u32::from(seat.0) + 1;
    if above < first {
        return Some(OpId(first));
    }
    let k = (above - first) / seats + 1;
    k.checked_mul(seats)?.checked_add(first).map(OpId)
}

/// What an op reaches, for holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Touch {
    Nothing,
    Thing(Endpoint),
    /// An entry of the block by its running index.
    Block(BlockKind),
}

/// An op that names an entry by its running index: an add or a remove
/// made since moves it.
pub(crate) fn by_index(op: &Op) -> bool {
    matches!(
        op.edit,
        Edit::Placement(Placement::Set { .. })
            | Edit::Placement(Placement::Remove { .. })
            | Edit::Placement(Placement::Add { like: Some(_), .. })
    )
}

fn decode<T: for<'de> Deserialize<'de>>(
    bytes: &[u8],
    what: &str,
    max: usize,
) -> Result<T, Refusal> {
    if bytes.len() > max {
        return Err(Refusal::Limit {
            what: what.into(),
            max,
        });
    }
    serde_json::from_slice(bytes).map_err(|e| Refusal::Json(e.to_string()))
}

fn encode(value: &impl Serialize, what: &str, max: usize) -> Result<Vec<u8>, Refusal> {
    let bytes = serde_json::to_vec(value).map_err(|e| Refusal::Json(e.to_string()))?;
    if bytes.len() > max {
        return Err(Refusal::Limit {
            what: what.into(),
            max,
        });
    }
    Ok(bytes)
}

#[cfg(test)]
mod node_tests;
#[cfg(test)]
mod tests;
