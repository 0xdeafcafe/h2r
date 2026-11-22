//! `host.pause` and `host.step` (docs/runtime/agent-api.md, "Window pause
//! and frame step"): the window's own pause (Cmd+P's) over the agent API,
//! through [`agent_proto::host_pause`]'s hub.
//!
//! Both are connection methods ([`Agent::add_connection_read`]): answered
//! on the connection's own thread, because the guest thread they hold
//! answers nothing while held. A ticker counts each game tick for the
//! step gate.

use std::time::Duration;

use agent_proto::host_pause::{self, Hub, PauseParams, StepParams, method};
use agent_proto::{RpcError, code};
use serde_json::{Value, json};

use crate::Agent;

fn params<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, RpcError> {
    let v = if v.is_null() { json!({}) } else { v };
    serde_json::from_value(v).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
}

/// `host.pause {on}` over `hub`.
pub fn pause(hub: &Hub, v: Value, shell_wait: Duration, hold_wait: Duration) -> Result<Value, RpcError> {
    let p: PauseParams = params(v)?;
    let s = hub.pause(p.on, shell_wait, hold_wait)?;
    Ok(serde_json::to_value(s).expect("serialises"))
}

/// `host.step {ticks}` over `hub`.
pub fn step(hub: &Hub, v: Value) -> Result<Value, RpcError> {
    let p: StepParams = params(v)?;
    let s = hub.step(p.ticks, host_pause::step_wait(p.ticks))?;
    Ok(serde_json::to_value(s).expect("serialises"))
}

/// Registers `host.pause`, `host.step` and the tick counter on `agent`,
/// over `hub` (the process's in the game).
pub fn install(agent: &Agent, hub: &'static Hub) {
    agent.add_connection_read(method::PAUSE, move |v| {
        pause(hub, v, host_pause::SHELL_WAIT, host_pause::HOLD_WAIT)
    });
    agent.add_connection_read(method::STEP, move |v| step(hub, v));
    agent.add_ticker(method::STEP, move |ctx| hub.ticked(ctx.at.tick));
}
