//! `rules.state` over a fake runtime: the params it takes and what it
//! answers.

use std::sync::Arc;
use std::time::Duration;

use agent_proto::rules::{Budget, Refusal, Reloaded, State};
use agent_proto::{Object, code};
use guest_layout::GuestMemory;
use guest_layout::testing::FakeGuest;
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
    fn zones(&self) -> agent_proto::rules::Zones {
        agent_proto::rules::Zones {
            zones: if self.running { vec![Fake::zone()] } else { Vec::new() },
        }
    }

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

impl Fake {
    fn zone() -> agent_proto::rules::Zone {
        agent_proto::rules::Zone {
            id: 0x8_0001,
            label: "hill_corner".into(),
            number: Some(0),
            shape: "area".into(),
            area: json!({"radius": 4.0, "below": 0.5, "above": 1.5}),
            at: [10.0, 0.0, 0.0],
            reports: true,
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
        install(&agent, Arc::new(Fake { running }), None);
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

#[test]
fn zones_report_each_zones_shape_and_area() {
    let (h, mut g) = (Harness::new(true), game());
    let r = h.call(&mut g, method::ZONES, Value::Null);
    assert!(r["error"].is_null(), "{r}");
    let z = &r["result"]["zones"][0];
    assert_eq!(z["label"], "hill_corner");
    assert_eq!(z["shape"], "area");
    assert_eq!(z["area"]["radius"], 4.0);
    assert_eq!(z["at"], json!([10.0, 0.0, 0.0]));
    assert_eq!(z["reports"], true);
    let r = h.call(&mut g, method::ZONES, json!({"label": "x"}));
    assert_eq!(r["error"]["code"], code::INVALID_PARAMS);
    // No script, no zones.
    let h = Harness::new(false);
    let r = h.call(&mut g, method::ZONES, json!({}));
    assert_eq!(r["result"]["zones"], json!([]));
}

/// A runtime that answers each reload by its `source` text, and records
/// what it is told.
#[derive(Default)]
struct Reloader {
    outcomes: std::sync::Mutex<Vec<Result<String, String>>>,
    restart_refusal: std::sync::Mutex<Option<Option<&'static str>>>,
}

fn result(mode: Reloaded) -> ReloadResult {
    ReloadResult {
        mode,
        hash: "cd".repeat(32),
        reasons: match mode {
            Reloaded::InPlace => vec![],
            Reloaded::Restarted => vec!["player variable best changed".into()],
        },
        tick: 40,
    }
}

impl Source for Reloader {
    fn state(&self) -> State {
        State::default()
    }

    fn reload(
        &self,
        _m: &mut dyn GuestMemory,
        params: ReloadParams,
        restart: Option<&'static str>,
    ) -> Result<Reload, RpcError> {
        *self.restart_refusal.lock().unwrap() = Some(restart);
        match params.source.as_deref() {
            Some("same") => Ok(Reload::Done(result(Reloaded::InPlace))),
            Some("shape") => Ok(Reload::Restart {
                result: result(Reloaded::Restarted),
                steps: Vec::new(),
            }),
            _ => Err(RpcError::new(
                agent_proto::sim::REFUSED,
                "a matchmade game takes no script edits",
            )),
        }
    }

    fn restart_outcome(&self, outcome: Result<&ReloadResult, &str>) {
        self.outcomes
            .lock()
            .unwrap()
            .push(outcome.map(|r| r.hash.clone()).map_err(str::to_owned));
    }
}

fn reloader(calls: Option<crate::sim::calls::Shared>) -> (Harness, Arc<Reloader>) {
    let agent = Agent::new();
    let source = Arc::new(Reloader::default());
    install(&agent, source.clone(), calls);
    let (tx, rx) = std::sync::mpsc::sync_channel(16);
    let conn = agent.connect(tx);
    (Harness { agent, conn, rx }, source)
}

#[test]
fn reload_takes_exactly_one_of_a_blob_or_source() {
    let ((h, _), mut g) = (reloader(None), game());
    for params in [
        json!({}),
        json!({"ir_b64": "AAAA", "source": "same"}),
        json!({"source": "same", "mode": "sideways"}),
        json!({"source": "same", "verbose": true}),
    ] {
        let r = h.call(&mut g, method::RELOAD, params.clone());
        assert_eq!(r["error"]["code"], code::INVALID_PARAMS, "{params}: {r}");
    }
}

#[test]
fn an_in_place_reload_answers_at_once() {
    let ((h, source), mut g) = (reloader(None), game());
    let r = h.call(&mut g, method::RELOAD, json!({"source": "same", "mode": "auto"}));
    assert!(r["error"].is_null(), "{r}");
    assert_eq!(r["result"]["mode"], "in_place");
    assert_eq!(r["result"]["hash"], "cd".repeat(32));
    assert_eq!(r["result"]["reasons"], json!([]));
    // Without guest calls the source is told it cannot restart.
    assert!(source.restart_refusal.lock().unwrap().unwrap().is_some());
}

#[test]
fn a_refusal_comes_back_with_its_reason() {
    let ((h, _), mut g) = (reloader(None), game());
    let r = h.call(&mut g, method::RELOAD, json!({"source": "ranked"}));
    assert_eq!(r["error"]["code"], agent_proto::sim::REFUSED);
    assert_eq!(r["error"]["message"], "a matchmade game takes no script edits");
}

#[test]
fn a_restart_needs_the_guest_calls() {
    // The queue exists but no tick hook arms it: the restart is refused by
    // the queue's own reason, and the source was told it may restart.
    let ((h, source), mut g) = (reloader(Some(crate::sim::calls::shared())), game());
    let r = h.call(&mut g, method::RELOAD, json!({"source": "shape"}));
    assert_eq!(r["error"]["code"], code::UNAVAILABLE, "{r}");
    assert!(r["error"]["message"].as_str().unwrap().contains("agent-calls"), "{r}");
    assert_eq!(*source.restart_refusal.lock().unwrap(), Some(None));
    assert!(source.outcomes.lock().unwrap().is_empty());
}
