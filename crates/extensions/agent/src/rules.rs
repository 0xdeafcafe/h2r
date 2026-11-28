//! `rules.*` (docs/runtime/agent-api.md, "Rules scripts"): the rules
//! script of the game running now, answered at a tick boundary, and
//! reloading it.
//!
//! The runtime lives in the game (`game::rules`), which this crate does
//! not link: the game registers it as a [`Source`] ([`install`]), and the
//! method checks its params and calls it. A reload the source answers
//! with a restart is queued through the agent's guest calls
//! ([`crate::sim::calls::Call::QueueGame`], the level boot's queue), and
//! answered once the calls are made.

use std::sync::Arc;

use agent_proto::rules::{ReloadParams, ReloadResult, State, Zones, method};
use agent_proto::{RpcError, code};
use guest_layout::GuestMemory;
use serde_json::Value;

use crate::sim::calls::{self, Call, GameStep};
use crate::{Access, Agent, GameCtx};

/// Ticks `rules.reload` waits for a restart's guest calls (one tick
/// makes them).
pub const RESTART_TICKS: u64 = 30;

/// What a reload asks of the method.
#[derive(Debug, Clone, PartialEq)]
pub enum Reload {
    /// Made at this boundary.
    Done(ReloadResult),
    /// Queue the game again with these steps; `result` is the answer once
    /// they are made.
    Restart {
        result: ReloadResult,
        steps: Vec<GameStep>,
    },
}

/// The game's rules runtime, as the methods see it. Each call runs on the
/// game thread at a tick boundary.
pub trait Source: Send + Sync {
    /// `rules.state`: always answers; `running` false says why not.
    fn state(&self) -> State;

    /// `rules.zones`: the zones the running script's host built; none when
    /// no script runs.
    fn zones(&self) -> Zones {
        Zones::default()
    }

    /// `rules.reload` over the game at this boundary. `restart`: why this
    /// host cannot queue a game (no guest calls), or `None` when it can.
    /// A refusal is an error with its reason.
    fn reload(
        &self,
        _m: &mut dyn GuestMemory,
        _params: ReloadParams,
        _restart: Option<&'static str>,
    ) -> Result<Reload, RpcError> {
        Err(RpcError::new(
            code::UNAVAILABLE,
            "this game's rules runtime cannot reload a script",
        ))
    }

    /// What came of a restart's guest calls: the result, or why they
    /// failed.
    fn restart_outcome(&self, _outcome: Result<&ReloadResult, &str>) {}
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

/// `rules.reload`'s params: exactly one of `ir_b64` and `source`.
pub fn reload_params(params: Value) -> Result<ReloadParams, RpcError> {
    let p: ReloadParams = serde_json::from_value(params)
        .map_err(|e| RpcError::new(code::INVALID_PARAMS, format!("{}: {e}", method::RELOAD)))?;
    if p.ir_b64.is_some() == p.source.is_some() {
        return Err(RpcError::new(
            code::INVALID_PARAMS,
            format!("{}: give exactly one of ir_b64 or source", method::RELOAD),
        ));
    }
    Ok(p)
}

/// Adds `rules.state`, `rules.zones` and `rules.reload` over `source`. `calls`: the
/// agent's guest-call queue, which a restart needs.
pub fn install(agent: &Agent, source: Arc<dyn Source>, calls: Option<calls::Shared>) {
    let state = Arc::clone(&source);
    agent.add_game_method(method::STATE, Access::Read, move |params, _ctx| {
        no_params(method::STATE, &params)?;
        Ok(serde_json::to_value(state.state()).expect("rules.state serialises"))
    });
    let zones = Arc::clone(&source);
    agent.add_game_method(method::ZONES, Access::Read, move |params, _ctx| {
        no_params(method::ZONES, &params)?;
        Ok(serde_json::to_value(zones.zones()).expect("rules.zones serialises"))
    });
    agent.add_game_method(method::RELOAD, Access::Act, move |params, ctx| {
        let p = reload_params(params)?;
        let restart = calls.is_none().then_some(
            "the game's own functions aren't reachable here: the agent-calls feature isn't installed",
        );
        let src = Arc::clone(&source);
        match ctx.with_guest(|m| src.reload(m, p, restart))? {
            Reload::Done(r) => Ok(serde_json::to_value(r).expect("rules.reload serialises")),
            Reload::Restart { result, steps } => {
                let q = calls.clone().ok_or_else(|| {
                    RpcError::new(code::UNAVAILABLE, restart.unwrap_or("no guest calls"))
                })?;
                let ticket = calls::submit(&q, Call::QueueGame { steps })
                    .map_err(|e| RpcError::new(code::UNAVAILABLE, e))?;
                let src = Arc::clone(&source);
                ctx.defer(
                    RESTART_TICKS,
                    Box::new(move |_: &mut GameCtx<'_>| {
                        let out = ticket.outcome()?;
                        match &out {
                            Ok(_) => src.restart_outcome(Ok(&result)),
                            Err(e) => src.restart_outcome(Err(&e.message)),
                        }
                        Some(out.map(|_| {
                            serde_json::to_value(&result).expect("rules.reload serialises")
                        }))
                    }),
                );
                Ok(Value::Null)
            }
        }
    });
}

#[cfg(test)]
mod tests;
