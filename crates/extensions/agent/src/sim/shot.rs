//! `host.screenshot` (docs/runtime/agent-api.md, "Window screenshots"):
//! the window's newest frame as a PNG, over [`agent_proto::host_shot`]'s
//! hub.
//!
//! A connection method ([`Agent::add_connection_read`]): the capture runs
//! on the asking thread, so the in-process menu's call works while the
//! game is paused as `h2 agent`'s does.

use agent_proto::host_shot::{Hub, ShotParams, method};
use agent_proto::{RpcError, code};
use serde_json::{Value, json};

use crate::Agent;

fn params<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, RpcError> {
    let v = if v.is_null() { json!({}) } else { v };
    serde_json::from_value(v).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
}

/// `host.screenshot {path?}` over `hub`.
pub fn shot(hub: &Hub, v: Value) -> Result<Value, RpcError> {
    let p: ShotParams = params(v)?;
    let s = hub.shot(p.path)?;
    Ok(serde_json::to_value(s).expect("serialises"))
}

/// Registers `host.screenshot` on `agent`, over `hub` (the process's in
/// the game).
pub fn install(agent: &Agent, hub: &'static Hub) {
    agent.add_connection_read(method::SHOT, move |v| shot(hub, v));
}
