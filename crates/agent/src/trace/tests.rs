//! `trace.*` and `flight.*` over a fake game: every tick gives a row, in
//! order, with nothing dropped, and the flight ring keeps the last N
//! seconds with each tick's controller reads.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Duration;

use agent_proto::trace::{ShowFilter, TickRow, show};
use agent_proto::{Kind, Physics, Request};
use serde_json::json;
use xapi::tap::InputTap;

use super::*;
use crate::reader::GameStatus;
use crate::{Agent, ConnId};

/// A level with player 0 crouching a little more each tick and an actor
/// that leaves.
struct Game {
    tick: u64,
    actor: bool,
    fail: bool,
}

impl Reader for Game {
    fn status(&mut self) -> GameStatus {
        GameStatus {
            tick: Some(self.tick),
            map: Some("01b_spacestation".into()),
            mode: Some("campaign".into()),
            bsp: Some(0),
        }
    }
    fn objects(&mut self) -> Result<Vec<Object>, String> {
        if self.fail {
            return Err("fault at 0x1000".into());
        }
        let mut v = vec![Object {
            id: "player.0".into(),
            kind: Kind::Player,
            pos: Some([self.tick as f64, 0.0, 0.0]),
            team: Some(0),
            physics: Some(Physics {
                crouch: Some((self.tick % 10) as f64 / 10.0),
                ..Default::default()
            }),
            ..Default::default()
        }];
        if self.actor {
            v.push(Object {
                id: "actor.e0010001".into(),
                kind: Kind::Actor,
                pos: Some([1.0, 2.0, 3.0]),
                ..Default::default()
            });
        }
        Ok(v)
    }
}

fn game() -> Game {
    Game {
        tick: 0,
        actor: true,
        fail: false,
    }
}

/// A fresh directory under the system temp dir.
fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("agent-trace-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Rig {
    a: Agent,
    c: ConnId,
    rx: Receiver<String>,
    g: Game,
    id: u64,
}

impl Rig {
    fn new(dir: &Path) -> Rig {
        let a = Agent::new();
        a.set_dir(dir.to_owned());
        let (tx, rx) = sync_channel(1024);
        let c = a.connect(tx);
        Rig {
            a,
            c,
            rx,
            g: game(),
            id: 0,
        }
    }

    /// One game tick.
    fn tick(&mut self) {
        self.g.tick += 1;
        self.a.on_tick(&mut self.g);
    }

    /// A call answered between slices: its result, or its error.
    fn call(&mut self, m: &str, p: Value) -> Result<Value, Value> {
        self.id += 1;
        let r = Request::new(self.id, m, p);
        self.a.submit(self.c, &serde_json::to_string(&r).unwrap());
        self.a.between_slices(&mut self.g, 0);
        let line = self.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], self.id);
        match v.get("error") {
            Some(e) => Err(e.clone()),
            None => Ok(v["result"].clone()),
        }
    }
}

fn rows(path: &str) -> Vec<TickRow> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn a_trace_writes_one_row_per_tick_with_the_fields_asked_for() {
    let dir = scratch("rows");
    let mut r = Rig::new(&dir);
    r.tick();
    let s = r
        .call(
            method::TRACE_START,
            json!({"objects": ["player.0", "actor.e0010001", "player.0"],
                   "fields": ["physics", "pos"], "path": "walk.jsonl"}),
        )
        .unwrap();
    assert_eq!(s["active"], true);
    assert_eq!(s["objects"], json!(["player.0", "actor.e0010001"]));
    let path = s["path"].as_str().unwrap().to_owned();
    assert_eq!(PathBuf::from(&path), dir.join("traces/walk.jsonl"));

    for n in 0..100 {
        // The actor leaves at tick 51, and one read fails.
        r.g.actor = n < 49;
        r.g.fail = n == 70;
        r.tick();
    }
    let s = r.call(method::TRACE_STATUS, Value::Null).unwrap();
    assert_eq!((s["active"].clone(), s["rows"].clone()), (json!(true), json!(100)));
    let s = r.call(method::TRACE_STOP, Value::Null).unwrap();
    assert_eq!(s["active"], false);
    assert_eq!(s["rows"], 100);
    assert_eq!((s["first_tick"].clone(), s["last_tick"].clone()), (json!(2), json!(101)));

    // Rows after the stop aren't written.
    r.tick();
    let rows = rows(&path);
    assert_eq!(rows.len(), 100);
    assert_eq!(s["bytes"], std::fs::metadata(&path).unwrap().len());
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row.tick, Some(i as u64 + 2), "no tick skipped");
        assert_eq!(row.ticks, i as u64 + 2);
        assert_eq!(row.map.as_deref(), Some("01b_spacestation"));
        assert!(row.input.is_none(), "traces carry no input");
    }
    let p0 = &rows[0].objects["player.0"];
    assert_eq!(p0["physics"]["crouch"], 0.2);
    assert_eq!(p0["pos"], json!([2.0, 0.0, 0.0]));
    assert_eq!(p0["kind"], "player");
    assert!(p0.get("team").is_none(), "trimmed to the fields: {p0}");
    assert!(rows[48].objects.contains_key("actor.e0010001"));
    assert_eq!(rows[49].gone, ["actor.e0010001"]);
    assert_eq!(rows[70].error.as_deref(), Some("fault at 0x1000"));
    assert!(rows[70].objects.is_empty());

    // The last trace's counts stay for trace.status; a new one may start.
    let s = r.call(method::TRACE_STATUS, Value::Null).unwrap();
    assert_eq!((s["active"].clone(), s["rows"].clone()), (json!(false), json!(100)));
    let s = r
        .call(method::TRACE_START, json!({"objects": ["player.0"]}))
        .unwrap();
    let fresh = s["path"].as_str().unwrap();
    assert!(fresh.starts_with(dir.join("traces/trace-").to_str().unwrap()), "{fresh}");
    r.tick();
    r.call(method::TRACE_STOP, Value::Null).unwrap();
    let one = super::tests::rows(fresh);
    assert_eq!(one.len(), 1);
    // No fields: the whole object.
    assert_eq!(one[0].objects["player.0"]["team"], 0);
}

#[test]
fn a_trace_is_flushed_every_second_and_at_exit() {
    let dir = scratch("flush");
    let mut r = Rig::new(&dir);
    let s = r
        .call(method::TRACE_START, json!({"objects": ["player.0"]}))
        .unwrap();
    let path = s["path"].as_str().unwrap().to_owned();
    for _ in 0..TICKS_PER_SECOND {
        r.tick();
    }
    assert_eq!(rows(&path).len(), 30, "a second of rows is on disk");
    for _ in 0..5 {
        r.tick();
    }
    r.a.shutdown();
    assert_eq!(rows(&path).len(), 35, "the rest at exit");
}

#[test]
fn bad_trace_and_flight_requests_are_refused() {
    let dir = scratch("refuse");
    let mut r = Rig::new(&dir);
    let err_code = |e: Result<Value, Value>| e.unwrap_err()["code"].as_i64().unwrap();
    for p in [
        json!({"objects": []}),
        json!({"objects": ["the chief"]}),
        json!({"objects": ["player.0"], "fields": ["shoes"]}),
        json!({"objects": ["player.0"], "every": 2}),
    ] {
        assert_eq!(err_code(r.call(method::TRACE_START, p.clone())), code::INVALID_PARAMS, "{p}");
    }
    assert_eq!(err_code(r.call(method::TRACE_STOP, Value::Null)), code::INVALID_REQUEST);
    assert_eq!(r.call(method::TRACE_STATUS, Value::Null).unwrap()["active"], false);
    r.call(method::TRACE_START, json!({"objects": ["player.0"]}))
        .unwrap();
    assert_eq!(
        err_code(r.call(method::TRACE_START, json!({"objects": ["player.0"]}))),
        code::UNAVAILABLE
    );

    for s in [0, 601] {
        assert_eq!(
            err_code(r.call(method::FLIGHT_START, json!({"objects": ["player.0"], "seconds": s}))),
            code::INVALID_PARAMS
        );
    }
    assert_eq!(err_code(r.call(method::FLIGHT_DUMP, Value::Null)), code::INVALID_REQUEST);
    assert_eq!(err_code(r.call(method::FLIGHT_STOP, Value::Null)), code::INVALID_REQUEST);
    r.call(method::FLIGHT_START, json!({"objects": ["player.0"]}))
        .unwrap();
    assert_eq!(
        err_code(r.call(method::FLIGHT_START, json!({"objects": ["player.0"]}))),
        code::UNAVAILABLE
    );

    let listed = r.call(agent_proto::method::METHODS, Value::Null).unwrap();
    for m in method::ALL {
        assert!(listed.as_array().unwrap().contains(&json!(m)), "{m}");
    }
}

/// A packet: packet number, then the pad.
fn packet(number: u32, pad: Gamepad) -> [u8; xapi::tap::PACKET] {
    let mut p = [0u8; xapi::tap::PACKET];
    p[..4].copy_from_slice(&number.to_le_bytes());
    p[4..].copy_from_slice(&pad.to_bytes());
    p
}

#[test]
fn the_flight_recorder_keeps_the_last_seconds_with_their_input() {
    let dir = scratch("flight");
    let mut r = Rig::new(&dir);
    let mut tap = r.a.input_tap(None);
    tap.opened(0x5800_0000, 0);
    // A read before the start isn't kept.
    let (mut status, mut p) = (0, packet(1, Gamepad::default()));
    tap.read(0x5800_0000, &mut status, &mut p);

    let s = r
        .call(
            method::FLIGHT_START,
            json!({"objects": ["player.0"], "fields": ["physics"], "seconds": 1}),
        )
        .unwrap();
    assert_eq!((s["capacity"].clone(), s["rows"].clone()), (json!(30), json!(0)));

    for n in 0..75u32 {
        // Two reads a tick; A held on the second from tick 70 on.
        let mut pad = Gamepad::default();
        tap.read(0x5800_0000, &mut status, &mut packet(n * 2, pad));
        if n >= 69 {
            pad.analog[0] = 255;
            pad.thumb_ly = 32767;
        }
        tap.read(0x5800_0000, &mut status, &mut packet(n * 2 + 1, pad));
        r.tick();
    }
    let s = r.call(method::FLIGHT_STATUS, Value::Null).unwrap();
    assert_eq!((s["rows"].clone(), s["ticks"].clone()), (json!(30), json!(75)));
    assert_eq!((s["first_tick"].clone(), s["last_tick"].clone()), (json!(46), json!(75)));

    let s = r
        .call(method::FLIGHT_DUMP, json!({"path": "crouch.jsonl"}))
        .unwrap();
    let path = s["path"].as_str().unwrap().to_owned();
    assert_eq!(PathBuf::from(&path), dir.join("flights/crouch.jsonl"));
    let rows = rows(&path);
    assert_eq!(rows.len(), 30);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row.tick, Some(46 + i as u64));
        let input = row.input.as_ref().expect("flight rows carry input");
        assert_eq!(input.len(), 2, "both reads of the tick");
        assert_eq!(input[0].port, Some(0));
        assert_eq!(input[1].packet, 2 * (45 + i as u32) + 1);
    }
    let last = &rows[29].input.as_ref().unwrap()[1];
    assert_eq!(last.buttons, ["a"]);
    assert_eq!(last.left, [0.0, 1.0]);
    assert_eq!(last.raw.len(), 36);
    assert!(rows[0].input.as_ref().unwrap()[1].buttons.is_empty());

    // The dump scrubs as data.
    let text = std::fs::read_to_string(&path).unwrap();
    let f = ShowFilter {
        from: Some(74),
        to: Some(75),
        fields: vec!["physics.crouch".into(), "input".into()],
    };
    let shown = show(&text, &f).unwrap();
    assert_eq!(shown.len(), 2);
    assert!(
        shown[1].starts_with("tick 75  player.0 physics.crouch=0.5  in0[ L[0.0, 0.0]"),
        "{}",
        shown[1]
    );
    assert!(shown[1].ends_with("in0[a L[0.0, 1.0] R[0.0, 0.0]]"), "{}", shown[1]);

    // Still running after a dump; a stop drops the ring and the tap's reads.
    r.tick();
    let s = r.call(method::FLIGHT_STOP, Value::Null).unwrap();
    assert_eq!((s["active"].clone(), s["ticks"].clone()), (json!(false), json!(76)));
    tap.read(0x5800_0000, &mut status, &mut p);
    assert!(tap::lock(&r.a.input_for_tests()).flight.is_none());
    assert_eq!(r.call(method::FLIGHT_STATUS, Value::Null).unwrap()["active"], false);
}

#[test]
fn a_flood_of_reads_between_ticks_is_counted_not_kept() {
    let dir = scratch("flood");
    let mut r = Rig::new(&dir);
    let mut tap = r.a.input_tap(None);
    tap.opened(0x5800_0000, 0);
    r.call(method::FLIGHT_START, json!({"objects": ["player.0"]}))
        .unwrap();
    // A load: the game reads the pad but doesn't tick.
    for n in 0..100 {
        tap.read(0x5800_0000, &mut 0, &mut packet(n, Gamepad::default()));
    }
    r.tick();
    r.tick();
    let s = r.call(method::FLIGHT_DUMP, Value::Null).unwrap();
    let rows = rows(s["path"].as_str().unwrap());
    assert_eq!(rows[0].input.as_ref().unwrap().len(), InputRead::MAX_PER_TICK);
    assert_eq!(rows[0].input_dropped, 36);
    assert_eq!(rows[1].input, Some(vec![]));
    assert_eq!(rows[1].input_dropped, 0);
}

#[test]
fn without_an_input_tap_flight_rows_have_no_input() {
    let dir = scratch("notap");
    let mut r = Rig::new(&dir);
    r.call(method::FLIGHT_START, json!({"objects": ["player.0"]}))
        .unwrap();
    r.tick();
    let s = r.call(method::FLIGHT_DUMP, Value::Null).unwrap();
    assert!(s["path"].as_str().unwrap().contains("/flights/flight-"));
    assert_eq!(rows(s["path"].as_str().unwrap())[0].input, None);
}

#[test]
fn the_flight_ring_is_bounded_by_bytes_too() {
    let sel = Selection::new(vec!["player.0".into()], None).unwrap();
    let mut f = Flight {
        sel: sel.clone(),
        seconds: 10,
        capacity: 300,
        max_bytes: 1000,
        ring: VecDeque::new(),
        bytes: 0,
        ticks: 0,
    };
    let mut g = game();
    for t in 0..50 {
        g.tick = t;
        let tick = Tick {
            ticks: t,
            tick: Some(t),
            map: None,
        };
        f.push(&sel.row(&tick, &g.objects()));
    }
    assert!(f.bytes <= 1000, "{}", f.bytes);
    assert_eq!(f.bytes, f.ring.iter().map(|r| r.1.len()).sum::<usize>());
    assert!(f.ring.len() < 50 && !f.ring.is_empty());
    assert_eq!(f.ring.back().unwrap().0, Some(49), "the newest is kept");
    assert_eq!(f.ticks, 50);
}
