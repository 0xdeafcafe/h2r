//! `forge.waves.test` (docs/runtime/agent-api.md, "Local Forge session";
//! the test itself is docs/game/forge-firefight.md, "Test waves").
//!
//! The session's **Test waves** and **Stop waves** rows for an agent. The
//! game's Forge session answers ([`WavesTest`]): a start whose ground is on
//! the running map answers once the runtime has taken it at the next tick
//! boundary; one that needs the ground built first answers at once with
//! `building`, and `firefight.state`'s `test` says when it runs.
use std::sync::{Arc, mpsc};

use agent_proto::firefight::{WAVES_TEST_WAIT_TICKS, WavesTestParams, method};
use agent_proto::{Boundary, RpcError, code};
use serde_json::{Value, json};

use crate::{Access, Agent};

/// A test's answer: [`agent_proto::firefight::WavesTestReply`] as JSON, or
/// the sentence the row would say.
pub type TestReply = mpsc::Receiver<Result<Value, String>>;

/// The game's Forge session, as `forge.waves.test` sees it.
pub trait WavesTest: Send + Sync + 'static {
    /// **Test waves** (`stop` false) or **Stop waves**.
    fn waves_test(&self, stop: bool) -> Result<TestReply, String>;
}

/// Adds `forge.waves.test` over `backend`.
pub fn install(agent: &Agent, backend: impl WavesTest) {
    let backend = Arc::new(backend);
    agent.add_game_method(method::WAVES_TEST, Access::Act, move |v, ctx| {
        if ctx.at.boundary != Boundary::Tick || ctx.at.mode.as_deref() != Some("multiplayer") {
            return Err(super::unavailable(
                "a wave test requires a local multiplayer tick",
            ));
        }
        let p = params(v)?;
        let reply = backend.waves_test(p.stop).map_err(refused)?;
        ctx.defer(
            WAVES_TEST_WAIT_TICKS,
            Box::new(move |_| match reply.try_recv() {
                Ok(result) => Some(result.map_err(refused)),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err(super::unavailable("the Forge session ended")))
                }
            }),
        );
        Ok(Value::Null)
    });
}

/// The params: none, `{}`, or `{stop: bool}`.
pub fn params(v: Value) -> Result<WavesTestParams, RpcError> {
    if v.is_null() {
        return Ok(WavesTestParams::default());
    }
    serde_json::from_value(v).map_err(|e| super::invalid(e.to_string()))
}

/// A refused start or stop: the row's sentence as the message, and
/// `{code, message}` as the data, as the other session methods refuse.
fn refused(message: String) -> RpcError {
    let name = super::local::refusal_name(&message);
    let mut e = RpcError::new(code::INVALID_REQUEST, message.clone());
    e.data = Some(json!({"code": name, "message": message}));
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_params_are_none_or_a_stop() {
        assert_eq!(params(Value::Null).unwrap(), WavesTestParams::default());
        assert_eq!(params(json!({})).unwrap(), WavesTestParams::default());
        assert!(params(json!({"stop": true})).unwrap().stop);
        assert!(params(json!({"halt": true})).is_err(), "unknown fields are refused");
        assert!(params(json!({"stop": "yes"})).is_err());
    }
}
