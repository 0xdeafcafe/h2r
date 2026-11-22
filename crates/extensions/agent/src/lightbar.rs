//! The controller lightbar state and session-only overrides.

use agent_proto::{RpcError, code, method};
use input::PortIndex;
use input::lightbar::{Lightbars, Rgb, State};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    port: usize,
    // A required member whose value can be null.
    rgb: Value,
}

pub fn install(agent: &crate::Agent, lightbars: Lightbars) {
    let read = lightbars.clone();
    agent.add_method(method::INPUT_LIGHTBAR, crate::Access::Read, move |params| {
        if !params.is_null() {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "input.lightbar takes no params",
            ));
        }
        Ok(reply(read.state()))
    });
    agent.add_method(
        method::INPUT_LIGHTBAR_SET,
        crate::Access::Act,
        move |params| {
            let params: Params = serde_json::from_value(params)
                .map_err(|err| RpcError::new(code::INVALID_PARAMS, err.to_string()))?;
            let port = PortIndex::new(params.port)
                .ok_or_else(|| RpcError::new(code::INVALID_PARAMS, "port must be 0 to 3"))?;
            let rgb: Option<Rgb> = serde_json::from_value(params.rgb).map_err(|_| {
                RpcError::new(code::INVALID_PARAMS, "rgb must be three bytes or null")
            })?;
            Ok(reply(lightbars.set_override(port, rgb)))
        },
    );
}

fn reply(state: State) -> Value {
    let ports: Vec<_> = state
        .desired()
        .into_iter()
        .enumerate()
        .map(|(port, rgb)| {
            json!({
                "port": port,
                "game_rgb": state.game[port],
                "override_rgb": state.overrides[port],
                "rgb": rgb,
            })
        })
        .collect();
    json!({"enabled": state.enabled, "ports": ports})
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;
    use std::time::Duration;

    use crate::{Agent, GameStatus, Reader};

    use super::*;

    struct Empty;

    impl Reader for Empty {
        fn status(&mut self) -> GameStatus {
            GameStatus::default()
        }

        fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn read_override_and_clear_a_lightbar_without_a_pad() {
        let agent = Agent::new();
        let input = input::InputManager::new(Default::default()).unwrap();
        let lightbars = input.lightbars();
        let game = [Some([32, 64, 96]), Some([40, 70, 99]), None, None];
        lightbars.update_game(true, game);
        install(&agent, lightbars.clone());
        let (tx, rx) = sync_channel(8);
        let conn = agent.connect(tx);
        let call = |method: &str, params: Value| -> Value {
            agent.submit(
                conn,
                &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string(),
            );
            agent.between_slices(&mut Empty, 0);
            serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
        };

        let get = || call(method::INPUT_LIGHTBAR, Value::Null)["result"].clone();
        let set = |params| call(method::INPUT_LIGHTBAR_SET, params);
        assert_eq!(get()["ports"][1]["rgb"], json!([40, 70, 99]));
        let changed = set(json!({"port": 1, "rgb": [0, 0, 0]}));
        assert_eq!(
            changed["result"]["ports"][1]["override_rgb"],
            json!([0, 0, 0])
        );
        assert_eq!(get()["ports"][1]["rgb"], json!([0, 0, 0]));
        assert_eq!(get()["ports"][0]["rgb"], json!([32, 64, 96]));

        lightbars.update_game(false, game);
        assert_eq!(get()["enabled"], false);
        assert!(get()["ports"][1]["rgb"].is_null());
        lightbars.update_game(true, game);
        let cleared = set(json!({"port": 1, "rgb": null}));
        assert!(cleared["result"]["ports"][1]["override_rgb"].is_null());
        assert_eq!(get()["ports"][1]["rgb"], json!([40, 70, 99]));

        for invalid in [
            json!({"port": 4, "rgb": null}),
            json!({"port": 1, "rgb": [256, 0, 0]}),
            json!({"port": 1, "rgb": [1, 2]}),
            json!({"port": 1}),
        ] {
            assert_eq!(set(invalid)["error"]["code"], code::INVALID_PARAMS);
        }
        assert_eq!(get()["ports"][1]["rgb"], json!([40, 70, 99]));
    }
}
