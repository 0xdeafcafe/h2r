//! Inspect native collision sources, optionally evicting the resident world
//! for the cache-cold parity run.
use std::sync::Arc;

use agent_proto::{Boundary, RpcError, code, collision::method};
use engine::sim_collision::{WORLD, source_audit};
use runtime::Runtime;
use serde_json::json;

use crate::{Access, Agent};

pub fn install(agent: &Agent, runtime: &Arc<Runtime>) {
    for (name, access, clear) in [
        (method::SOURCES, Access::Read, false),
        (method::CLEAR_SOURCES, Access::Claimed, true),
    ] {
        let runtime = Arc::downgrade(runtime);
        agent.add_game_method(name, access, move |params, ctx| {
            if !(params.is_null() || params.as_object().is_some_and(|o| o.is_empty())) {
                return Err(RpcError::new(code::INVALID_PARAMS, format!("{name} takes no parameters")));
            }
            if ctx.at.boundary != Boundary::Tick {
                return Err(RpcError::new(code::UNAVAILABLE, format!("{name} needs a game tick")));
            }
            let rt = runtime.upgrade().ok_or_else(|| RpcError::new(code::UNAVAILABLE, "game has shut down"))?;
            let sources = source_audit::read(&rt.memory(), &WORLD, clear)
                .map_err(|bail| RpcError::new(code::UNAVAILABLE, format!("{name}: {}", bail.name())))?;
            let mismatched = sources.iter().filter(|s| s.live.is_ok_and(|h| h != s.cached)).count();
            let unreadable = sources.iter().filter(|s| s.live.is_err()).count();
            let rows: Vec<_> = sources.into_iter().map(|s| json!({
                "kind": s.kind,
                "address": format!("{:08x}", s.addr),
                "cached": format!("{:016x}", s.cached),
                "live": s.live.ok().map(|h| format!("{h:016x}")),
                "bail": s.live.err().map(|b| b.name()),
            })).collect();
            Ok(json!({ "sources": rows, "mismatched": mismatched, "unreadable": unreadable, "cleared": clear }))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GameStatus, Reader};
    use runtime::{Dispatch, Guest};
    use serde_json::Value;

    struct Tick;
    impl Reader for Tick {
        fn status(&mut self) -> GameStatus {
            GameStatus {
                tick: Some(11),
                ..GameStatus::default()
            }
        }
        fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
            Ok(vec![])
        }
    }

    #[test]
    fn clear_requires_the_claim_and_traps_stop_the_audit() {
        let rt = Runtime::new(Guest::new(), Dispatch::default());
        rt.memory().install_trap(4);
        let a = Agent::new();
        install(&a, &rt);
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let conn = a.connect(tx);
        let call = |name: &str, params: Value| -> Value {
            a.submit(
                conn,
                &json!({"jsonrpc":"2.0","id":1,"method":name,"params":params}).to_string(),
            );
            a.on_tick(&mut Tick);
            let s = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            serde_json::from_str(&s).unwrap()
        };
        let blocked = call(method::CLEAR_SOURCES, Value::Null);
        assert_eq!(blocked["error"]["code"], code::NOT_CONTROLLER);
        assert_eq!(
            call(method::SOURCES, json!({"clear":true}))["error"]["code"],
            code::INVALID_PARAMS
        );
        assert!(call(agent_proto::method::CLAIM, Value::Null)["error"].is_null());
        for method in [method::SOURCES, method::CLEAR_SOURCES] {
            let reply = call(method, Value::Null);
            assert_eq!(reply["error"]["code"], code::UNAVAILABLE, "{reply}");
            assert!(reply["error"]["message"].as_str().unwrap().contains("trap"));
        }
    }
}
