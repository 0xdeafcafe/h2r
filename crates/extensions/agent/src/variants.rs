//! `variants.quick` and `variants.quick.set` (docs/runtime/agent-api.md,
//! "Game variants"): the lobby variant's Quick Options as a rules script
//! marks them, read at a tick boundary, and one row's value changed as
//! the lobby's Quick Options dialog changes it.
//!
//! The rows live in the game (`game::variant_quick`), which this crate
//! does not link: the game registers them as a [`Source`] ([`install`]),
//! and the method checks its params and calls it. The wire types are
//! here, with the method.

use std::sync::Arc;

use agent_proto::{RpcError, code};
use guest_layout::GuestMemory;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Access, Agent, DEFER_TICKS_MAX};

pub mod method {
    /// Read: the lobby variant's script, its Quick Options rows and their
    /// values.
    pub const QUICK: &str = "variants.quick";
    /// Act: one row's value, written through the game's lobby variant
    /// setter as the Quick Options dialog writes it.
    pub const QUICK_SET: &str = "variants.quick.set";
    pub const ALL: &[&str] = &[QUICK, QUICK_SET];
}

/// `variants.quick.set`'s params: the row by its slot, and the value it
/// takes (one of the row's `values`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetParams {
    pub slot: u8,
    pub value: i32,
}

/// A change waiting for the game's setter ([`Source::set`]).
pub type Ticket = u64;

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

/// The game's Quick Options rows, as the methods see them. Each call runs
/// on the game thread at a tick boundary.
pub trait Source: Send + Sync {
    /// The lobby variant's rows; an error when there is no lobby variant.
    fn quick(&self, m: &dyn GuestMemory) -> Result<Quick, RpcError>;
    /// Checks a change of row `slot` to `value` on the lobby's record
    /// (the row and value exist, the record the change makes is one the
    /// game keeps) and queues it for the lobby variant setter: the
    /// change's ticket, or why there is none.
    fn set(&self, m: &dyn GuestMemory, slot: u8, value: i32) -> Result<Ticket, RpcError>;
    /// `None` while change `ticket` waits for the setter; then whether
    /// the setter took it.
    fn outcome(&self, ticket: Ticket) -> Option<Result<(), RpcError>>;
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

fn set_params(params: Value) -> Result<SetParams, RpcError> {
    serde_json::from_value(params).map_err(|e| {
        RpcError::new(
            code::INVALID_PARAMS,
            format!("{}: {{slot, value}}: {e}", method::QUICK_SET),
        )
    })
}

/// Adds `variants.quick` and `variants.quick.set` over `source`. A set
/// answers once the game's setter has run (at its next UI update): the
/// rows as they are then.
pub fn install(agent: &Agent, source: Arc<dyn Source>) {
    let s = Arc::clone(&source);
    agent.add_game_method(method::QUICK, Access::Read, move |params, ctx| {
        no_params(method::QUICK, &params)?;
        let quick = ctx.read_guest(|m| s.quick(m))??;
        Ok(serde_json::to_value(quick).expect("variants.quick serialises"))
    });
    agent.add_game_method(method::QUICK_SET, Access::Act, move |params, ctx| {
        let p = set_params(params)?;
        let ticket = ctx.read_guest(|m| source.set(m, p.slot, p.value))??;
        let s = Arc::clone(&source);
        ctx.defer(
            DEFER_TICKS_MAX,
            Box::new(move |ctx| match s.outcome(ticket)? {
                Ok(()) => Some(
                    ctx.read_guest(|m| s.quick(m))
                        .and_then(|q| q)
                        .map(|q| serde_json::to_value(q).expect("variants.quick serialises")),
                ),
                Err(e) => Some(Err(e)),
            }),
        );
        Ok(Value::Null)
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use agent_proto::Object;
    use guest_layout::testing::FakeGuest;
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

    /// A lobby with a script variant, or none; a set takes the value at
    /// once unless the setter refuses it (`refuse`).
    struct Fake {
        lobby: bool,
        refuse: bool,
        value: std::sync::Mutex<i32>,
    }

    fn fake(lobby: bool) -> Fake {
        Fake {
            lobby,
            refuse: false,
            value: std::sync::Mutex::new(1),
        }
    }

    impl Source for Fake {
        fn set(&self, _m: &dyn GuestMemory, slot: u8, value: i32) -> Result<Ticket, RpcError> {
            if !self.lobby {
                return Err(RpcError::new(code::UNAVAILABLE, "no lobby variant to read"));
            }
            if slot != 0 || !(0..=1).contains(&value) {
                return Err(RpcError::new(code::INVALID_PARAMS, "no such row or value"));
            }
            if !self.refuse {
                *self.value.lock().unwrap() = value;
            }
            Ok(7)
        }
        fn outcome(&self, ticket: Ticket) -> Option<Result<(), RpcError>> {
            assert_eq!(ticket, 7);
            Some(if self.refuse {
                Err(RpcError::new(code::UNAVAILABLE, "the game refused the variant"))
            } else {
                Ok(())
            })
        }
        fn quick(&self, _m: &dyn GuestMemory) -> Result<Quick, RpcError> {
            if !self.lobby {
                return Err(RpcError::new(code::UNAVAILABLE, "no lobby variant to read"));
            }
            let value = *self.value.lock().unwrap();
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
                    value,
                    text: if value == 1 { "On" } else { "Off" }.into(),
                    line: format!("Suicide penalty: {}", if value == 1 { "On" } else { "Off" }),
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
        call_on(fake(lobby), method::QUICK, params)
    }

    fn call_on(source: Fake, name: &str, params: Value) -> Value {
        let agent = Agent::new();
        install(&agent, Arc::new(source));
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let conn = agent.connect(tx);
        agent.submit(
            conn,
            &json!({"jsonrpc": "2.0", "id": 1, "method": name, "params": params}).to_string(),
        );
        // The set answers at the boundary after the one that queued it.
        for _ in 0..3 {
            agent.on_tick(&mut Game(FakeGuest::new()));
            if let Ok(line) = rx.try_recv() {
                return serde_json::from_str(&line).unwrap();
            }
        }
        serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
    }

    /// A set queues the change, answers once the setter took it with the
    /// rows as they are then, and checks its params; a refusal by the
    /// game is the answer's error.
    #[test]
    fn quick_set_changes_one_row_and_answers_with_the_rows() {
        let r = call_on(fake(true), method::QUICK_SET, json!({"slot": 0, "value": 0}));
        assert!(r["error"].is_null(), "{r}");
        assert_eq!(r["result"]["rows"][0]["value"], 0);
        assert_eq!(r["result"]["rows"][0]["line"], "Suicide penalty: Off");
        for bad in [json!({}), json!({"slot": 0}), json!({"slot": 0, "value": 1, "x": 1})] {
            let r = call_on(fake(true), method::QUICK_SET, bad);
            assert_eq!(r["error"]["code"], code::INVALID_PARAMS, "{r}");
        }
        let r = call_on(fake(true), method::QUICK_SET, json!({"slot": 3, "value": 0}));
        assert_eq!(r["error"]["code"], code::INVALID_PARAMS, "{r}");
        let r = call_on(fake(false), method::QUICK_SET, json!({"slot": 0, "value": 0}));
        assert_eq!(r["error"]["code"], code::UNAVAILABLE, "{r}");
        let refusing = Fake {
            refuse: true,
            ..fake(true)
        };
        let r = call_on(refusing, method::QUICK_SET, json!({"slot": 0, "value": 0}));
        assert_eq!(r["error"]["message"], "the game refused the variant", "{r}");
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
