//! `shadow.*`: shadow checks of the native ports against the lifted
//! original (docs/engine/shadow-exec.md), over `feature::shadow`'s
//! registry. Boot's agent glue registers them ([`add_methods`]).

use agent_proto::shadow::{DisableParams, EnableParams, method};
use agent_proto::{RpcError, code};
use feature::shadow::{Error, SHADOW, Shadow};
use serde_json::{Value, json};

use crate::{Access, Agent};

/// `shadow.enable`, `shadow.disable` and `shadow.status` over the
/// process's registry.
pub fn add_methods(agent: &Agent) {
    add_methods_for(agent, &SHADOW);
}

/// [`add_methods`] over `shadow` (tests use their own registry).
pub fn add_methods_for(agent: &Agent, shadow: &'static Shadow) {
    agent.add_method(method::ENABLE, Access::Act, move |v| enable(shadow, v));
    agent.add_method(method::DISABLE, Access::Act, move |v| disable(shadow, v));
    agent.add_method(method::STATUS, Access::Read, move |v| status(shadow, v));
}

fn params<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, RpcError> {
    let v = if v.is_null() { json!({}) } else { v };
    serde_json::from_value(v).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
}

fn rpc(e: Error) -> RpcError {
    let c = match e {
        Error::Unavailable(_) => code::UNAVAILABLE,
        Error::Unknown(_) | Error::Invalid(_) => code::INVALID_PARAMS,
    };
    RpcError::new(c, e.to_string())
}

fn to_json(v: impl serde::Serialize) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(|e| RpcError::new(code::UNAVAILABLE, e.to_string()))
}

fn enable(shadow: &Shadow, v: Value) -> Result<Value, RpcError> {
    let p: EnableParams = params(v)?;
    to_json(
        shadow
            .enable(&p.ports, p.every, p.step_limit)
            .map_err(rpc)?,
    )
}

fn disable(shadow: &Shadow, v: Value) -> Result<Value, RpcError> {
    let p: DisableParams = params(v)?;
    Ok(json!({ "disabled": shadow.disable(&p.ports).map_err(rpc)? }))
}

fn status(shadow: &Shadow, v: Value) -> Result<Value, RpcError> {
    if !(v.is_null() || v.as_object().is_some_and(|o| o.is_empty())) {
        return Err(RpcError::new(
            code::INVALID_PARAMS,
            "shadow.status takes no parameters",
        ));
    }
    to_json(shadow.status())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> &'static Shadow {
        let s: &'static Shadow = Box::leak(Box::new(Shadow::new()));
        s.register("sim.native_a", "sim.native", "A", 0x1000, true);
        s.register("sim.native_b", "sim.native", "B", 0x2000, false);
        s
    }

    #[test]
    fn enable_reports_what_it_turned_on_and_what_is_not_installed() {
        let s = registry();
        // No image: the lifted original cannot be built.
        let e = enable(s, json!({"ports": ["sim.native"]})).unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE);
        s.set_code(Box::new(lifted::Assembled::new(0x1000, &[0xc3])));
        let r = enable(s, json!({"ports": ["sim.native"], "every": 2})).unwrap();
        assert_eq!(r["enabled"], json!(["sim.native_a"]));
        assert_eq!(r["not_installed"], json!(["sim.native_b"]));
        assert_eq!(r["every"], 2);
        let st = status(s, Value::Null).unwrap();
        assert_eq!(st["on"], true);
        assert_eq!(st["ports"][0]["function"], "0x00001000");
        assert_eq!(
            disable(s, Value::Null).unwrap()["disabled"],
            json!(["sim.native_a"])
        );
    }

    #[test]
    fn bad_params_are_invalid() {
        let s = registry();
        s.set_code(Box::new(lifted::Assembled::new(0x1000, &[0xc3])));
        for bad in [
            json!({"ports": ["nope"]}),
            json!({"ports": []}),
            json!({"port": ["all"]}),
        ] {
            assert_eq!(enable(s, bad).unwrap_err().code, code::INVALID_PARAMS);
        }
        assert_eq!(
            status(s, json!({"x": 1})).unwrap_err().code,
            code::INVALID_PARAMS
        );
    }
}
