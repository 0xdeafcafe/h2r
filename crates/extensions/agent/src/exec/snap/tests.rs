//! `state.*` through the executor, over a fake store and machine.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use agent_proto::state::{HashCheck, List, Listed, Loaded, Saved, method as state};
use agent_proto::{At, Request, RpcError, code, method};
use serde_json::{Value, json};

use crate::reader::{GameStatus, Reader};
use crate::{Agent, ConnId, SnapshotMachine, Snapshots};

/// Snapshots by name: the tick each was saved at.
type Shelf = Arc<Mutex<BTreeMap<String, u64>>>;

struct Store(Shelf);

impl Snapshots for Store {
    fn list(&mut self) -> Result<List, RpcError> {
        Ok(List {
            dir: "/snaps/0123".into(),
            snapshots: self
                .0
                .lock()
                .unwrap()
                .iter()
                .map(|(name, tick)| Listed {
                    name: name.clone(),
                    tick: *tick,
                    ..Default::default()
                })
                .collect(),
        })
    }
    fn remove(&mut self, name: &str) -> Result<(), RpcError> {
        match self.0.lock().unwrap().remove(name) {
            Some(_) => Ok(()),
            None => Err(RpcError::new(code::INVALID_PARAMS, "no such snapshot")),
        }
    }
    fn exists(&mut self, name: &str) -> bool {
        self.0.lock().unwrap().contains_key(name)
    }
}

/// A machine that is busy for its first `busy` tries, and logs what it
/// was asked.
#[derive(Default)]
struct Machine {
    shelf: Shelf,
    busy: u64,
    calls: Vec<String>,
}

impl SnapshotMachine for Machine {
    fn blockers(&mut self) -> Vec<String> {
        self.calls.push("blockers".into());
        if self.busy > 0 {
            self.busy -= 1;
            vec!["d3d: inside Begin/End".into()]
        } else {
            Vec::new()
        }
    }
    fn save(&mut self, name: &str, _replace: bool, at: &At, _cow: bool) -> Result<Saved, RpcError> {
        self.calls.push(format!("save {name}"));
        let tick = at.tick.unwrap_or(0);
        self.shelf.lock().unwrap().insert(name.into(), tick);
        Ok(Saved {
            name: name.into(),
            path: format!("/snaps/0123/{name}.h2snap"),
            bytes: 64,
            sections: vec!["d3d".into(), "ram".into(), "kcore".into()],
            ..Default::default()
        })
    }
    fn load(&mut self, name: &str, _cow: bool) -> Result<Loaded, RpcError> {
        self.calls.push(format!("load {name}"));
        let tick = self.shelf.lock().unwrap()[name];
        Ok(Loaded {
            name: name.into(),
            tick,
            applied: vec!["ram".into(), "kcore".into(), "d3d".into()],
            hash: HashCheck {
                checked: true,
                equal: true,
                differing_mib: Vec::new(),
            },
            ..Default::default()
        })
    }
}

#[derive(Clone)]
struct Game(u64);

impl Reader for Game {
    fn status(&mut self) -> GameStatus {
        GameStatus {
            tick: Some(self.0),
            map: Some("03a_oldmombasa".into()),
            mode: Some("campaign".into()),
            bsp: Some(0),
        }
    }
    fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
        Ok(Vec::new())
    }
}

fn agent() -> (Agent, Machine) {
    let a = Agent::new();
    let m = Machine::default();
    a.set_snapshots(Box::new(Store(Arc::clone(&m.shelf))));
    (a, m)
}

fn client(a: &Agent) -> (ConnId, Receiver<String>) {
    let (tx, rx) = sync_channel(256);
    (a.connect(tx), rx)
}

fn ask(a: &Agent, c: ConnId, id: u64, m: &str, p: Value) {
    a.submit(c, &serde_json::to_string(&Request::new(id, m, p)).unwrap());
}

fn next(rx: &Receiver<String>) -> Value {
    serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
}

fn none(rx: &Receiver<String>) {
    assert!(rx.try_recv().is_err(), "nothing more expected");
}

#[test]
fn a_save_waits_for_a_tick_and_runs_between_slices() {
    let (a, mut m) = agent();
    let (c, rx) = client(&a);
    let mut g = Game(100);
    assert!(!a.on_tick(&mut g), "nothing due: the slice runs on");
    ask(&a, c, 1, state::SAVE, json!({"name": "fight"}));
    a.between_slices(&mut g, 7);
    // Queued, but the game ticks: not due until the next tick.
    assert!(a.snapshot_pending());
    a.run_snapshots(&mut m);
    assert!(m.calls.is_empty(), "{:?}", m.calls);
    none(&rx);
    g.0 = 101;
    assert!(a.on_tick(&mut g), "due: the tick observer ends the slice");
    a.run_snapshots(&mut m);
    assert_eq!(m.calls, ["blockers", "save fight"]);
    let v = next(&rx);
    assert_eq!(v["id"], 1);
    assert_eq!(v["result"]["name"], "fight");
    assert_eq!(v["result"]["retries"], 0);
    assert_eq!(v["result"]["sections"][0], "d3d");
    assert_eq!(v["result"]["at"]["tick"], 101);
    assert!(!a.snapshot_pending());
    assert!(!a.on_tick(&mut g));
    ask(&a, c, 2, state::LIST, Value::Null);
    a.between_slices(&mut g, 8);
    let v = next(&rx);
    assert_eq!(v["result"]["snapshots"][0]["name"], "fight");
    assert_eq!(v["result"]["snapshots"][0]["tick"], 101);
    assert!(v["result"]["at"].is_object());
}

#[test]
fn a_blocked_save_retries_each_tick_then_answers_busy() {
    let (a, mut m) = agent();
    let (c, rx) = client(&a);
    let mut g = Game(1);
    a.on_tick(&mut g);
    m.busy = 2;
    ask(&a, c, 1, state::SAVE, json!({"name": "a", "max_ticks": 5}));
    a.between_slices(&mut g, 0);
    for tick in 2..=4 {
        g.0 = tick;
        assert!(a.on_tick(&mut g));
        a.run_snapshots(&mut m);
        // A second pass at the same boundary waits for the next tick.
        a.run_snapshots(&mut m);
    }
    let v = next(&rx);
    assert_eq!(v["result"]["retries"], 2, "{v}");
    assert_eq!(v["result"]["at"]["tick"], 4);
    assert_eq!(
        m.calls,
        ["blockers", "blockers", "blockers", "save a"],
        "one try per tick"
    );

    m.busy = 100;
    ask(&a, c, 2, state::SAVE, json!({"name": "b", "max_ticks": 3}));
    a.between_slices(&mut g, 0);
    for tick in 5..=7 {
        g.0 = tick;
        a.on_tick(&mut g);
        a.run_snapshots(&mut m);
    }
    let v = next(&rx);
    assert_eq!(v["error"]["code"], code::BUSY, "{v}");
    assert_eq!(v["error"]["data"]["tries"], 3);
    assert_eq!(v["error"]["data"]["blockers"][0], "d3d: inside Begin/End");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("inside Begin/End")
    );
    assert!(!a.snapshot_pending());
    assert!(!m.shelf.lock().unwrap().contains_key("b"));
}

#[test]
fn load_fork_and_drop_on_a_game_that_is_not_ticking() {
    let (a, mut m) = agent();
    let (c, rx) = client(&a);
    let mut g = Game(40);
    // No ticks: every slice boundary is a try.
    ask(&a, c, 1, state::SAVE, json!({"name": "a"}));
    a.between_slices(&mut g, 0);
    a.run_snapshots(&mut m);
    assert_eq!(next(&rx)["result"]["name"], "a");
    ask(&a, c, 2, state::LOAD, json!({"name": "a"}));
    a.between_slices(&mut g, 0);
    a.run_snapshots(&mut m);
    let v = next(&rx);
    assert_eq!(v["result"]["name"], "a");
    assert_eq!(v["result"]["hash"]["equal"], true);
    assert_eq!(v["result"]["applied"][0], "ram");
    m.calls.clear();
    ask(&a, c, 3, state::FORK, json!({"from": "a", "name": "b"}));
    a.between_slices(&mut g, 0);
    a.run_snapshots(&mut m);
    let v = next(&rx);
    assert_eq!(m.calls, ["blockers", "load a", "save b"]);
    assert_eq!(v["result"]["loaded"]["name"], "a");
    assert_eq!(v["result"]["saved"]["name"], "b");
    ask(&a, c, 4, state::DROP, json!({"name": "a"}));
    ask(&a, c, 5, state::LIST, Value::Null);
    a.between_slices(&mut g, 0);
    assert_eq!(next(&rx)["result"]["name"], "a");
    let v = next(&rx);
    assert_eq!(v["result"]["snapshots"].as_array().unwrap().len(), 1);
    assert_eq!(v["result"]["snapshots"][0]["name"], "b");
}

#[test]
fn names_and_params_are_checked_before_any_wait() {
    let (a, mut m) = agent();
    let (c, rx) = client(&a);
    m.shelf.lock().unwrap().insert("taken".into(), 1);
    for (id, meth, p) in [
        (1, state::LOAD, json!({"name": "missing"})),
        (2, state::SAVE, json!({"name": "taken"})),
        (3, state::SAVE, json!({"name": "Bad Name"})),
        (4, state::SAVE, json!({"name": "x", "max_ticks": 0})),
        (5, state::FORK, json!({"from": "missing", "name": "y"})),
        (6, state::SAVE, json!({"name": "x", "loud": true})),
        (7, state::DROP, json!({"name": "missing"})),
    ] {
        ask(&a, c, id, meth, p);
    }
    a.between_slices(&mut Game(1), 0);
    for id in 1..=7 {
        let v = next(&rx);
        assert_eq!(v["id"], id);
        assert_eq!(v["error"]["code"], code::INVALID_PARAMS, "{v}");
    }
    assert!(!a.snapshot_pending());
    // Replacing is asked for.
    ask(
        &a,
        c,
        8,
        state::SAVE,
        json!({"name": "taken", "replace": true}),
    );
    a.between_slices(&mut Game(1), 0);
    a.run_snapshots(&mut m);
    assert_eq!(next(&rx)["result"]["name"], "taken");
}

#[test]
fn without_snapshots_state_is_unavailable_and_listed() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    ask(&a, c, 1, state::SAVE, json!({"name": "a"}));
    ask(&a, c, 2, method::METHODS, Value::Null);
    a.between_slices(&mut Game(1), 0);
    assert_eq!(next(&rx)["error"]["code"], code::UNAVAILABLE);
    let v = next(&rx);
    let all = v["result"].as_array().unwrap();
    assert!(all.contains(&json!("state.save")) && all.contains(&json!("state.fork")));
}

#[test]
fn another_controller_blocks_saves_but_not_lists() {
    let (a, _m) = agent();
    let (c1, rx1) = client(&a);
    let (c2, rx2) = client(&a);
    ask(&a, c1, 1, method::CLAIM, Value::Null);
    ask(&a, c2, 2, state::SAVE, json!({"name": "a"}));
    ask(&a, c2, 3, state::LIST, Value::Null);
    a.between_slices(&mut Game(1), 0);
    assert_eq!(next(&rx1)["result"]["controller"], true);
    assert_eq!(next(&rx2)["error"]["code"], code::NOT_CONTROLLER);
    assert!(next(&rx2)["result"]["snapshots"].is_array());
}

#[test]
fn a_paused_game_lets_a_save_reach_the_slice_end_and_stays_paused() {
    let (a, mut m) = agent();
    let (c, rx) = client(&a);
    ask(&a, c, 1, method::CLAIM, Value::Null);
    ask(&a, c, 2, method::PAUSE, Value::Null);
    let game = {
        let a = a.clone();
        thread::spawn(move || a.on_tick(&mut Game(500)))
    };
    assert_eq!(next(&rx)["id"], 1);
    assert_eq!(next(&rx)["result"]["at"]["paused"], true);
    assert!(!game.is_finished(), "held at the tick");
    ask(&a, c, 3, state::SAVE, json!({"name": "held"}));
    assert!(game.join().unwrap(), "the hold lets go for the save");
    assert!(a.is_paused(), "and holds again at the next tick");
    a.run_snapshots(&mut m);
    let v = next(&rx);
    assert_eq!(v["result"]["name"], "held");
    assert_eq!(v["result"]["at"]["tick"], 500);
    assert_eq!(m.calls, ["blockers", "save held"]);
}

#[test]
fn queued_jobs_fail_when_the_game_exits() {
    let (a, _m) = agent();
    let (c, rx) = client(&a);
    a.on_tick(&mut Game(1));
    ask(&a, c, 1, state::LOAD, json!({"name": "a"}));
    a.between_slices(&mut Game(1), 0);
    // `a` does not exist: answered at once.
    assert_eq!(next(&rx)["error"]["code"], code::INVALID_PARAMS);
    ask(&a, c, 2, state::SAVE, json!({"name": "a"}));
    a.between_slices(&mut Game(1), 0);
    assert!(a.snapshot_pending());
    a.shutdown();
    assert_eq!(next(&rx)["error"]["code"], code::SHUTDOWN);
}
