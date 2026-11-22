//! `variants.quick` (docs/runtime/agent-api.md, "Game variants"): the
//! lobby variant's Quick Options as a rules script marks them, read at a
//! tick boundary.
//!
//! The rows live in the game (`game::variant_quick`), which this crate
//! does not link: the game registers them as a [`Source`] ([`install`]),
//! and the method checks its params and calls it. The wire types are
//! here, with the method.

use std::sync::Arc;

use agent_proto::{RpcError, code};
use engine::GuestMemory;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Access, Agent};

pub mod method {
    /// Read: the lobby variant's script, its Quick Options rows and their
    /// values.
    pub const QUICK: &str = "variants.quick";
    pub const ALL: &[&str] = &[QUICK];
}

/// `variants.quick`'s result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quick {
    /// The lobby variant's name.
    pub variant: String,
    /// Its engine index.
    pub engine: i32,
    /// The script its block names, when this machine has it; absent for a
    /// variant with no script, which keeps retail's Quick Options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<Script>,
    /// Whether the lobby's panel shows these rows: the `variant-quick`
    /// switch is on and the script was found.
    pub shown: bool,
    /// The script's marks in its order, at most five; empty for a script
    /// with no `quick` line.
    pub rows: Vec<Row>,
}

/// The script a variant names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script {
    pub name: String,
    /// Its SHA-256, in hex.
    pub hash: String,
}

/// What a row changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A record setting of `rules::settings::TABLE`.
    Setting,
    /// One of the script's options.
    Option,
}

/// One Quick Options row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// Its place in the panel and the dialog, 0 to 4.
    pub slot: u8,
    pub kind: Kind,
    /// The setting's or the option's name in the script.
    pub name: String,
    pub label: String,
    /// The current value, as the record stores it.
    pub value: i32,
    /// The current value's text.
    pub text: String,
    /// The panel's line: "label: text".
    pub line: String,
    /// The option's description; a setting has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// The values the row may take: for a setting, the ones the editor
    /// offered at the last variant list load (none before one); for an
    /// option, its range.
    pub values: Vec<Choice>,
}

/// One value a row may take.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub value: i32,
    pub text: String,
}

/// The game's Quick Options rows, as the method sees them. Each call runs
/// on the game thread at a tick boundary.
pub trait Source: Send + Sync {
    /// The lobby variant's rows; an error when there is no lobby variant.
    fn quick(&self, m: &dyn GuestMemory) -> Result<Quick, RpcError>;
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

/// Adds `variants.quick` over `source`.
pub fn install(agent: &Agent, source: Arc<dyn Source>) {
    agent.add_game_method(method::QUICK, Access::Read, move |params, ctx| {
        no_params(method::QUICK, &params)?;
        let quick = ctx.read_guest(|m| source.quick(m))??;
        Ok(serde_json::to_value(quick).expect("variants.quick serialises"))
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use agent_proto::Object;
    use engine::testing::FakeGuest;
    use serde_json::json;

    use super::*;
    use crate::{GameStatus, Reader};

    struct Game(FakeGuest);

    impl Reader for Game {
        fn status(&mut self) -> GameStatus {
            GameStatus {
                tick: Some(1),
                map: Some("mainmenu".into()),
                mode: Some("menu".into()),
                bsp: Some(0),
            }
        }
        fn objects(&mut self) -> Result<Vec<Object>, String> {
            Ok(Vec::new())
        }
        fn with_guest(&mut self, f: &mut dyn FnMut(&mut dyn GuestMemory)) -> bool {
            f(&mut self.0);
            true
        }
    }

    /// A lobby with a script variant, or none.
    struct Fake(bool);

    impl Source for Fake {
        fn quick(&self, _m: &dyn GuestMemory) -> Result<Quick, RpcError> {
            if !self.0 {
                return Err(RpcError::new(code::UNAVAILABLE, "no lobby variant to read"));
            }
            Ok(Quick {
                variant: "Superbounce".into(),
                engine: 2,
                script: Some(Script {
                    name: "Superbounce".into(),
                    hash: "ab".repeat(32),
                }),
                shown: true,
                rows: vec![Row {
                    slot: 0,
                    kind: Kind::Option,
                    name: "suicide_costs".into(),
                    label: "Suicide penalty".into(),
                    value: 1,
                    text: "On".into(),
                    line: "Suicide penalty: On".into(),
                    help: Some("A suicide costs a point".into()),
                    values: vec![
                        Choice {
                            value: 0,
                            text: "Off".into(),
                        },
                        Choice {
                            value: 1,
                            text: "On".into(),
                        },
                    ],
                }],
            })
        }
    }

    fn call(lobby: bool, params: Value) -> Value {
        let agent = Agent::new();
        install(&agent, Arc::new(Fake(lobby)));
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let conn = agent.connect(tx);
        agent.submit(
            conn,
            &json!({"jsonrpc": "2.0", "id": 1, "method": method::QUICK, "params": params})
                .to_string(),
        );
        agent.on_tick(&mut Game(FakeGuest::new()));
        serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
    }

    #[test]
    fn quick_reads_the_rows_and_takes_no_params() {
        for params in [Value::Null, json!({})] {
            let r = call(true, params);
            assert!(r["error"].is_null(), "{r}");
            let q = &r["result"];
            assert_eq!(q["variant"], "Superbounce");
            assert_eq!(q["script"]["name"], "Superbounce");
            assert_eq!(q["shown"], true);
            assert_eq!(q["rows"][0]["kind"], "option");
            assert_eq!(q["rows"][0]["line"], "Suicide penalty: On");
            assert_eq!(q["rows"][0]["values"][1]["text"], "On");
        }
        let r = call(true, json!({"slot": 0}));
        assert_eq!(r["error"]["code"], code::INVALID_PARAMS);
    }

    #[test]
    fn quick_says_when_there_is_no_lobby_variant() {
        let r = call(false, Value::Null);
        assert_eq!(r["error"]["code"], code::UNAVAILABLE);
        assert_eq!(r["error"]["message"], "no lobby variant to read");
    }

    #[test]
    fn a_variant_with_no_script_has_no_script_field() {
        let q = Quick {
            variant: "Slayer".into(),
            engine: 2,
            ..Quick::default()
        };
        let v = serde_json::to_value(&q).unwrap();
        assert!(v.get("script").is_none(), "{v}");
        assert_eq!(v["rows"], json!([]));
        assert_eq!(serde_json::from_value::<Quick>(v).unwrap(), q);
    }
}
