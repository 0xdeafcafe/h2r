//! `actors.net` (docs/runtime/agent-api.md, "actors.net"): the actor
//! channel of docs/game/ai-networking.md as this machine runs it. On a
//! distributed host, each connected peer's budget; on a distributed
//! client, its puppets, what waits for a body and its digest check.
//!
//! The channel lives in the game (`game::actor_net`), which this crate
//! does not link: the game publishes a [`Report`] each tick and registers
//! the reader with [`install`].

use agent_proto::{RpcError, code};
use serde::Serialize;
use serde_json::Value;

use crate::{Access, Agent};

pub const METHOD: &str = "actors.net";

/// What this machine does with the actor channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Not in a distributed game, or the `actor-net` feature is off.
    #[default]
    Off,
    /// The distributed host: captures and schedules for each peer.
    Host,
    /// A distributed client: applies what the host sends.
    Client,
}

/// One connected peer's budget on the host, counted since it connected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Peer {
    /// The tunnel connection's id.
    pub peer: u32,
    /// Bytes charged in the last 30 host ticks, headers included.
    pub bytes_per_second: u64,
    pub packets: u64,
    pub bytes: u64,
    pub keys: u64,
    pub digests: u64,
    /// Records sent, repeated copies of lines and cues included.
    pub records: u64,
    /// Records offered and dropped by the budget.
    pub dropped: u64,
}

/// An actor whose body this client has bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Puppet {
    pub entity: u32,
    pub character: u16,
    pub squad: u16,
    pub team: u8,
    pub posture: u8,
    /// The local unit's salted handle.
    pub unit: u32,
}

/// The client's side of the channel.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Client {
    /// Actors in the replica, bound or not.
    pub actors: usize,
    pub puppets: Vec<Puppet>,
    /// Actors whose body has not arrived through retail replication.
    pub waiting: usize,
    /// Dialogue lines waiting for their speaker's body.
    pub lines_waiting: usize,
    /// Bytes of the host's mode state (the Firefight HUD's blob).
    pub mode_state_bytes: usize,
    /// Disagreeing digests in the current run.
    pub digest_misses: u8,
    /// A key is wanted and not yet asked for.
    pub key_requested: bool,
    /// The channel is closed for this match (`actor_channel_diverged`).
    pub diverged: bool,
    /// The host log head of the last digest, in hex.
    pub host_log_head: Option<String>,
}

/// The whole reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    pub role: Role,
    /// The simulation tick the report was taken at.
    pub tick: Option<u32>,
    /// Peers on the host, in id order; empty elsewhere.
    pub peers: Vec<Peer>,
    /// The client's side; none elsewhere.
    pub client: Option<Client>,
}

/// The log head as `firefight.state` writes hashes: 64 lowercase hex digits.
pub fn hex(head: &[u8; 32]) -> String {
    head.iter().map(|b| format!("{b:02x}")).collect()
}

/// The reply to one call.
pub fn reply(params: &Value, report: Report) -> Result<Value, RpcError> {
    if !params.is_null() && !params.as_object().is_some_and(|o| o.is_empty()) {
        return Err(RpcError::new(
            code::INVALID_PARAMS,
            "actors.net takes no parameters",
        ));
    }
    Ok(serde_json::to_value(report).expect("actors.net serialises"))
}

/// Registers `actors.net`. `report` returns the last tick's report.
pub fn install(agent: &Agent, report: impl Fn() -> Report + Send + 'static) {
    agent.add_game_method(METHOD, Access::Read, move |params, _ctx| {
        reply(&params, report())
    });
}

#[cfg(test)]
mod tests;
