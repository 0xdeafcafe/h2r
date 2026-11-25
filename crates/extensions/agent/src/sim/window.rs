//! `host.window.resize` (docs/runtime/agent-api.md, "Resize the window"):
//! the game window's size or fullscreen, over [`agent_proto::host_window`]'s
//! hub.
//!
//! A connection method ([`Agent::add_connection_read`]): answered on the
//! connection's own thread, so a paused or busy game still takes it, and
//! the wait for the window never holds the game thread.

use agent_proto::host_window::{Hub, ResizeParams, WAIT, method};
use agent_proto::{RpcError, code};
use serde_json::{Value, json};

use crate::Agent;

/// `host.window.resize {width?, height?, fullscreen?}` over `hub`.
pub fn resize(hub: &Hub, v: Value) -> Result<Value, RpcError> {
    let v = if v.is_null() { json!({}) } else { v };
    let p: ResizeParams =
        serde_json::from_value(v).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
    let r = hub.resize(p, WAIT)?;
    Ok(serde_json::to_value(r).expect("serialises"))
}

/// Registers `host.window.resize` on `agent`, over `hub` (the process's
/// in the game).
pub fn install(agent: &Agent, hub: &'static Hub) {
    agent.add_connection_read(method::RESIZE, move |v| resize(hub, v));
}
