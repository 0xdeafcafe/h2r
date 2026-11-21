//! `rules.state` over a fake runtime: the params it takes and what it
//! answers.

use std::sync::Arc;
use std::time::Duration;

use agent_proto::rules::{Budget, Refusal, State};
use agent_proto::{Object, code};
use engine::GuestMemory;
use engine::testing::FakeGuest;
use serde_json::{Value, json};

use super::*;
use crate::{GameStatus, Reader};

struct Game(FakeGuest);

impl Reader for Game {
    fn status(&mut self) -> GameStatus {
        GameStatus {
            tick: Some(40),
            map: Some("lockout".into()),
            mode: Some("multiplayer".into()),
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

/// A runtime running Slayer, or refusing a script.
struct Fake {
    running: bool,
}

impl Source for Fake {
    fn state(&self) -> State {
        if !self.running {
            return State {
                refused: Some("no blob with hash prefix 0102030405060708".into()),
                hash: Some("0102030405060708".into()),
                ..Default::default()
            };
        }
        State {
            running: true,
            script: Some("Slayer".into()),
            version: Some(1),
            hash: Some("ab".repeat(32)),
            source: Some("builtin".into()),
            carrier: Some(2),
            options: Some(1),
            authority: Some(true),
            budget: Some(Budget {
                tick: 40,
                ops: 12,
                limit: 20_000,
                worst_tick: 400,
            }),
            pending: 1,
            errors: vec![Refusal {
                tick: 39,
                call: "score".into(),
                error: "the engine took no score for slot 3".into(),
            }],
            ..Default::default()
        }
    }
}

struct Harness {
    agent: Agent,
    conn: crate::ConnId,
    rx: std::sync::mpsc::Receiver<String>,
}

impl Harness {
    fn new(running: bool) -> Harness {
        let agent = Agent::new();
        install(&agent, Arc::new(Fake { running }));
        let (tx, rx) = std::sync::mpsc::sync_channel(16);
        let conn = agent.connect(tx);
        Harness { agent, conn, rx }
    }

    fn call(&self, game: &mut Game, m: &str, p: Value) -> Value {
        self.agent.submit(
            self.conn,
            &json!({"jsonrpc": "2.0", "id": 1, "method": m, "params": p}).to_string(),
        );
        self.agent.on_tick(game);
        serde_json::from_str(&self.rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
    }
}

fn game() -> Game {
    Game(FakeGuest::new())
}

#[test]
fn state_reports_the_script_and_its_budget() {
    let (h, mut g) = (Harness::new(true), game());
    for params in [Value::Null, json!({})] {
        let r = h.call(&mut g, method::STATE, params);
        assert!(r["error"].is_null(), "{r}");
        let s = &r["result"];
        assert_eq!(s["running"], true);
        assert_eq!(s["script"], "Slayer");
        assert_eq!(s["hash"], "ab".repeat(32));
        assert_eq!(s["source"], "builtin");
        assert_eq!(s["budget"]["ops"], 12);
        assert_eq!(s["budget"]["limit"], 20_000);
        assert_eq!(s["pending"], 1);
        assert_eq!(s["errors"][0]["call"], "score");
        assert!(s.get("refused").is_none(), "{s}");
    }
    let r = h.call(&mut g, method::STATE, json!({"verbose": true}));
    assert_eq!(r["error"]["code"], code::INVALID_PARAMS);
}

#[test]
fn state_names_a_refusal() {
    let (h, mut g) = (Harness::new(false), game());
    let r = h.call(&mut g, method::STATE, Value::Null);
    let s = &r["result"];
    assert_eq!(s["running"], false);
    assert_eq!(s["refused"], "no blob with hash prefix 0102030405060708");
    assert!(s.get("script").is_none() && s.get("budget").is_none(), "{s}");
}
