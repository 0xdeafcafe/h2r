//! `firefight.*` (docs/runtime/agent-api.md, "firefight"): the Firefight
//! runtime of the loaded map, answered at a tick boundary.
//!
//! The runtime lives in the game (`game::firefight`), which this crate does
//! not link: the game registers it as a [`Source`] ([`install`]), and the
//! methods check their params, call it and turn its refusals into error
//! codes. A command is appended to the host log on the host only: a
//! client keeps the host's log and a replica (a synchronous peer, a film's
//! playback) applies what it receives, so both refuse.

use std::sync::Arc;

use agent_proto::firefight::{CommandParams, CommandReply, Kit, State, method};
use agent_proto::{RpcError, code};
use guest_layout::GuestMemory;
use serde_json::Value;

use crate::{Access, Agent};

/// Why the runtime did not do what a method asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The mode does not run here: the feature is off, there is no
    /// multiplayer game, or the map has no kit. `UNAVAILABLE`.
    NotRunning(String),
    /// Only the host writes its log: this machine is a client or a replica.
    /// [`agent_proto::sim::REFUSED`].
    NotHost(String),
    /// The command is not one the runtime takes. `INVALID_PARAMS`.
    Invalid(String),
    /// A guest read faulted. `UNAVAILABLE`.
    Unavailable(String),
}

impl From<Refused> for RpcError {
    fn from(r: Refused) -> Self {
        match r {
            Refused::NotRunning(why) => RpcError::new(code::UNAVAILABLE, why),
            Refused::NotHost(why) => RpcError::new(agent_proto::sim::REFUSED, why),
            Refused::Invalid(why) => RpcError::new(code::INVALID_PARAMS, why),
            Refused::Unavailable(why) => RpcError::new(code::UNAVAILABLE, why),
        }
    }
}

/// The game's Firefight runtime, as the methods see it. Each call runs on
/// the game thread at a tick boundary.
pub trait Source: Send + Sync {
    /// `firefight.state`: always answers; `running` false says why not.
    fn state(&self) -> State;
    /// `firefight.command`: apply the director change, then append the
    /// command at the next tick.
    fn command(&self, params: CommandParams) -> Result<CommandReply, Refused>;
    /// `firefight.kit`: the kit read from the loaded scenario.
    fn kit(&self, m: &dyn GuestMemory) -> Result<Kit, Refused>;
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, message)
}

fn no_params(name: &str, params: &Value) -> Result<(), RpcError> {
    if params.is_null() || params.as_object().is_some_and(|p| p.is_empty()) {
        return Ok(());
    }
    Err(invalid(format!("{name} takes no parameters")))
}

/// `firefight.command`'s params, checked: an object naming a command, a
/// director change, or both.
pub fn command_params(params: Value) -> Result<CommandParams, RpcError> {
    if !params.is_object() {
        return Err(invalid(
            "firefight.command expects {\"command\": {...}} and/or {\"director\": \"hold\" | \"run\"}",
        ));
    }
    let p: CommandParams = serde_json::from_value(params).map_err(|e| invalid(e.to_string()))?;
    if p.command.is_none() && p.director.is_none() {
        return Err(invalid(
            "nothing to do: give a command, a director change, or both",
        ));
    }
    Ok(p)
}

/// Adds `firefight.state`, `firefight.command` and `firefight.kit` over
/// `source`.
pub fn install(agent: &Agent, source: Arc<dyn Source>) {
    let s = Arc::clone(&source);
    agent.add_game_method(method::STATE, Access::Read, move |params, _ctx| {
        no_params(method::STATE, &params)?;
        Ok(serde_json::to_value(s.state()).expect("firefight.state serialises"))
    });
    let s = Arc::clone(&source);
    agent.add_game_method(method::COMMAND, Access::Act, move |params, _ctx| {
        let p = command_params(params)?;
        let reply = s.command(p)?;
        Ok(serde_json::to_value(reply).expect("firefight.command serialises"))
    });
    let s = source;
    agent.add_game_method(method::KIT, Access::Read, move |params, ctx| {
        no_params(method::KIT, &params)?;
        let kit = ctx.read_guest(|m| s.kit(m))??;
        Ok(serde_json::to_value(kit).expect("firefight.kit serialises"))
    });
}

#[cfg(test)]
mod tests;
