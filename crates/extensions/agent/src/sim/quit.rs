//! `host.quit` (docs/runtime/agent-api.md, "Close the window"): close the
//! game window as its close button does, over [`agent_proto::host_quit`]'s
//! hub.
//!
//! A connection method ([`Agent::add_connection_read`]): answered on the
//! connection's own thread, so a paused or busy game still takes it.

use agent_proto::host_quit::{Hub, QuitParams, method};
use agent_proto::{RpcError, code};
use serde_json::{Value, json};

use crate::Agent;

/// `host.quit {}` over `hub`.
pub fn quit(hub: &Hub, v: Value) -> Result<Value, RpcError> {
    let v = if v.is_null() { json!({}) } else { v };
    let QuitParams {} =
        serde_json::from_value(v).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
    let r = hub.quit()?;
    Ok(serde_json::to_value(r).expect("serialises"))
}

/// Registers `host.quit` on `agent`, over `hub` (the process's in the
/// game).
pub fn install(agent: &Agent, hub: &'static Hub) {
    agent.add_connection_read(method::QUIT, move |v| quit(hub, v));
}
