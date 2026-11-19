//! Tickets and host assignments for the OptiMatch QoS queue.

use crate::Xuid;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::SocketAddrV4};

pub const MATCH_HEADER: &str = "x-h2-match";
pub const MAX_CANDIDATES: usize = 32;
pub const ASSIGNMENT_SECONDS: i64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Nat {
    Open,
    Moderate,
    Strict,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connectivity {
    pub nat: Option<Nat>,
    pub upload_kbps: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ticket {
    /// OptiMatch hopper index.
    pub hopper: u32,
    pub port: u16,
    #[serde(default)]
    pub dedicated: bool,
    #[serde(default)]
    pub connectivity: Connectivity,
    #[serde(default)]
    pub samples: Vec<Sample>,
}

/// An observed game endpoint, with an opaque generation for this ticket.
/// The client probes base+2 using the existing QoS echo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub xuid: Xuid,
    pub endpoint: SocketAddrV4,
    pub generation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub target: Xuid,
    pub generation: String,
    /// Null means a probe was sent and timed out.
    pub rtt_ms: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignment {
    pub id: String,
    pub hopper: u32,
    pub host: Xuid,
    pub dedicated: bool,
    pub members: Vec<Xuid>,
    /// Omitted for free-for-all. Team IDs are zero-based.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub teams: BTreeMap<Xuid, u8>,
    /// Member -> selected host, in milliseconds. A peer host's own entry is zero.
    pub rtt_ms: BTreeMap<Xuid, u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Queue {
    Probing { candidates: Vec<Candidate> },
    Host { assignment: Assignment },
    Waiting { assignment: Assignment },
    Join { session_id: String },
}
