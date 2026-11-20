//! `load.timeline` through the executor: ticks with a map change between
//! them, frame totals from a fake.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent::{Agent, GameStatus, Reader, TotalsSource};
use agent_proto::{Object, code, method};
use serde_json::{Value, json};
use timeline::phase::{Counter, Totals};

struct Game {
    map: &'static str,
    tick: u64,
}

impl Reader for Game {
    fn status(&mut self) -> GameStatus {
        GameStatus {
            tick: Some(self.tick),
            map: Some(self.map.into()),
            mode: Some("campaign".into()),
            bsp: Some(0),
        }
    }
    fn objects(&mut self) -> Result<Vec<Object>, String> {
        Ok(vec![])
    }
}

struct Fake(Arc<Mutex<Totals>>);

impl TotalsSource for Fake {
    fn totals(&mut self) -> Option<Totals> {
        Some(*self.0.lock().unwrap())
    }
}

#[test]
fn a_map_change_is_a_load_with_what_it_read() {
    let a = Agent::new();
    let totals = Arc::new(Mutex::new(Totals::default()));
    a.set_totals_source(Box::new(Fake(Arc::clone(&totals))));
    let (tx, rx) = std::sync::mpsc::sync_channel(64);
    let c = a.connect(tx);
    let mut g = Game {
        map: "mainmenu",
        tick: 0,
    };
    let call = |a: &Agent, g: &mut Game, p: Value| -> Value {
        a.submit(
            c,
            &json!({"jsonrpc": "2.0", "id": 1, "method": method::LOAD_TIMELINE, "params": p})
                .to_string(),
        );
        a.between_slices(g, 0);
        serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
    };
    for _ in 0..31 {
        g.tick += 1;
        a.on_tick(&mut g);
    }
    // The level: 3 MB read between the menu's last tick and its first.
    totals.lock().unwrap().counts[Counter::ReadBytes as usize] = 3 << 20;
    g.map = "01b_spacestation";
    for _ in 0..31 {
        g.tick += 1;
        a.on_tick(&mut g);
    }
    let r = call(&a, &mut g, Value::Null);
    let loads = r["result"]["loads"].as_array().unwrap();
    assert_eq!(loads.len(), 2, "{r}");
    assert_eq!(loads[0]["kind"], "boot");
    let lvl = &loads[1];
    assert_eq!(lvl["kind"], "map");
    assert_eq!(lvl["from_map"], "mainmenu");
    assert_eq!(lvl["map"], "01b_spacestation");
    assert_eq!(lvl["settled"], true);
    assert_eq!(lvl["segments"][0]["counts"]["read_bytes"], 3 << 20);
    let r = call(&a, &mut g, json!({"last": 1}));
    assert_eq!(r["result"]["loads"].as_array().unwrap().len(), 1);
    let r = call(&a, &mut g, json!({"last": 0}));
    assert_eq!(r["error"]["code"], code::INVALID_PARAMS);
}
