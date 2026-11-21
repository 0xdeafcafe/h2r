//! `rules.*` (docs/runtime/agent-api.md, "Rules scripts"): the rules
//! script of the game running now, answered at a tick boundary.
//!
//! The runtime lives in the game (`game::rules`), which this crate does
//! not link: the game registers it as a [`Source`] ([`install`]), and the
//! method checks its params and calls it.

use std::sync::Arc;

use agent_proto::rules::{State, method};
use agent_proto::{RpcError, code};
use serde_json::Value;

use crate::{Access, Agent};

/// The game's rules runtime, as the methods see it. Each call runs on the
/// game thread at a tick boundary.
pub trait Source: Send + Sync {
    /// `rules.state`: always answers; `running` false says why not.
    fn state(&self) -> State;
}

fn no_params(name: &str, params: &Value) -> Result<(), RpcError> {
    if params.is_null() || params.as_object().is_some_and(|p| p.is_empty()) {
        return Ok(());
    }
    Err(RpcError::new(
        code::INVALID_PARAMS,
        format!("{name} takes no parameters"),
    ))
}

/// Adds `rules.state` over `source`.
pub fn install(agent: &Agent, source: Arc<dyn Source>) {
    agent.add_game_method(method::STATE, Access::Read, move |params, _ctx| {
        no_params(method::STATE, &params)?;
        Ok(serde_json::to_value(source.state()).expect("rules.state serialises"))
    });
}

#[cfg(test)]
mod tests;
