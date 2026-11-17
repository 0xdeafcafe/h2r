use std::sync::mpsc::{Receiver, sync_channel};
use std::thread;

use serde_json::json;

use super::*;
use crate::reader::GameStatus;

/// A game in a jar: a status and a list of objects, changed by the test.
#[derive(Default, Clone)]
struct Fake {
    status: GameStatus,
    objects: Vec<Object>,
    fail: bool,
}

impl Reader for Fake {
    fn status(&mut self) -> GameStatus {
        self.status.clone()
    }
    fn objects(&mut self) -> Result<Vec<Object>, String> {
        if self.fail {
            Err("fault at 0x1000".into())
        } else {
            Ok(self.objects.clone())
        }
    }
}

fn obj(id: &str, kind: Kind, pos: [f64; 3], team: Option<i32>) -> Object {
    Object {
        id: id.into(),
        kind,
        pos: Some(pos),
        team,
        ..Default::default()
    }
}

fn level() -> Fake {
    Fake {
        status: GameStatus {
            tick: Some(100),
            map: Some("03a_oldmombasa".into()),
            mode: Some("campaign".into()),
            bsp: Some(0),
        },
        objects: vec![
            obj("player.0", Kind::Player, [0.0, 0.0, 0.0], Some(0)),
            obj("actor.e0010001", Kind::Actor, [3.0, 4.0, 0.0], Some(3)),
            obj("actor.e0020002", Kind::Actor, [30.0, 0.0, 0.0], Some(3)),
            obj("actor.e0030003", Kind::Actor, [1.0, 0.0, 0.0], Some(1)),
            obj("vehicle.e0040004", Kind::Vehicle, [2.0, 0.0, 0.0], None),
        ],
        fail: false,
    }
}

fn client(a: &Agent) -> (ConnId, Receiver<String>) {
    let (tx, rx) = sync_channel(1024);
    (a.connect(tx), rx)
}

fn ask(a: &Agent, conn: ConnId, id: u64, method: &str, params: Value) {
    let r = Request::new(id, method, params);
    a.submit(conn, &serde_json::to_string(&r).unwrap());
}

fn next(rx: &Receiver<String>) -> Value {
    let line = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("a line from the server");
    serde_json::from_str(&line).unwrap()
}

fn none(rx: &Receiver<String>) {
    assert!(rx.try_recv().is_err(), "nothing more expected");
}

#[test]
fn status_is_answered_between_slices_when_the_game_is_not_ticking() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    ask(&a, c, 1, method::STATUS, Value::Null);
    a.between_slices(&mut Fake::default(), 7);
    let v = next(&rx);
    assert_eq!(v["id"], 1);
    let at = &v["result"]["at"];
    assert_eq!(at["phase"], "loading");
    assert_eq!(at["boundary"], "slice");
    assert_eq!(at["present"], 7);
    assert_eq!(v["result"]["protocol"], PROTOCOL);
    assert_eq!(v["result"]["controller"], false);
}

#[test]
fn world_reads_wait_for_a_tick_while_the_game_is_ticking() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    let mut g = level();
    a.on_tick(&mut g);
    ask(&a, c, 1, method::WORLD_COUNT, Value::Null);
    ask(&a, c, 2, method::STATUS, Value::Null);
    a.between_slices(&mut g, 1);
    // Status goes out at once; the world read waits for the tick.
    assert_eq!(next(&rx)["id"], 2);
    none(&rx);
    a.on_tick(&mut g);
    let v = next(&rx);
    assert_eq!(v["id"], 1);
    assert_eq!(v["result"]["at"]["boundary"], "tick");
    assert_eq!(v["result"]["at"]["phase"], "gameplay");
    assert_eq!(v["result"]["at"]["ticks"], 2);
    assert_eq!(v["result"]["total"], 5);
    assert_eq!(v["result"]["groups"]["actor"], 3);
    assert_eq!(v["result"]["groups"]["vehicle"], 1);
}

#[test]
fn world_reads_are_answered_between_slices_once_ticks_stop() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    let mut g = level();
    ask(&a, c, 1, method::WORLD_COUNT, json!({"group_by": "team"}));
    a.between_slices(&mut g, 0);
    let v = next(&rx);
    assert_eq!(v["result"]["at"]["boundary"], "slice");
    assert_eq!(v["result"]["groups"]["3"], 2);
    assert_eq!(v["result"]["groups"]["none"], 1);
}

#[test]
fn a_failed_read_is_unavailable_and_stamped() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    let mut g = level();
    g.fail = true;
    ask(&a, c, 1, method::WORLD_QUERY, Value::Null);
    a.between_slices(&mut g, 0);
    let v = next(&rx);
    assert_eq!(v["error"]["code"], code::UNAVAILABLE);
    assert!(v["error"]["message"].as_str().unwrap().contains("0x1000"));
    assert_eq!(v["error"]["data"]["map"], "03a_oldmombasa");
}

#[test]
fn query_filters_by_kind_and_distance_nearest_first() {
    let p = QueryParams {
        kind: Some(vec![Kind::Actor]),
        near: Some(agent_proto::Near {
            player: Some(0),
            radius: 10.0,
            ..Default::default()
        }),
        ..Default::default()
    };
    let page = query(level().objects, &p).unwrap();
    let ids: Vec<&str> = page
        .objects
        .iter()
        .map(|o| o["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["actor.e0030003", "actor.e0010001"]);
    assert_eq!(page.objects[1]["distance"], 5.0);
    assert_eq!((page.total, page.more), (2, false));
}

#[test]
fn query_pages_and_trims_fields() {
    let p = QueryParams {
        fields: Some(vec!["team".into()]),
        limit: Some(2),
        offset: Some(1),
        ..Default::default()
    };
    let page = query(level().objects, &p).unwrap();
    assert_eq!((page.total, page.offset, page.more), (5, 1, true));
    assert_eq!(page.objects.len(), 2);
    assert_eq!(
        page.objects[0],
        json!({"id": "actor.e0010001", "kind": "actor", "team": 3})
    );
}

#[test]
fn query_near_a_player_without_a_unit_is_gone() {
    let mut objects = level().objects;
    objects[0].pos = None;
    let p = QueryParams {
        near: Some(agent_proto::Near {
            player: Some(0),
            radius: 1.0,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(query(objects, &p).unwrap_err().code, code::GONE);
    let both = QueryParams {
        near: Some(agent_proto::Near {
            player: Some(0),
            point: Some([0.0; 3]),
            radius: 1.0,
        }),
        ..Default::default()
    };
    assert_eq!(query(vec![], &both).unwrap_err().code, code::INVALID_PARAMS);
}

#[test]
fn an_unknown_field_is_refused_with_the_list() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    ask(&a, c, 1, method::WORLD_QUERY, json!({"fields": ["health"]}));
    a.between_slices(&mut level(), 0);
    let v = next(&rx);
    assert_eq!(v["error"]["code"], code::INVALID_PARAMS);
    assert!(v["error"]["message"].as_str().unwrap().contains("velocity"));
}

#[test]
fn world_object_finds_by_id_and_says_gone() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    ask(
        &a,
        c,
        1,
        method::WORLD_OBJECT,
        json!({"id": "vehicle.e0040004"}),
    );
    // Same slot, different salt: another object.
    ask(
        &a,
        c,
        2,
        method::WORLD_OBJECT,
        json!({"id": "vehicle.e0050004"}),
    );
    ask(&a, c, 3, method::WORLD_OBJECT, json!({"id": "warthog"}));
    a.between_slices(&mut level(), 0);
    assert_eq!(next(&rx)["result"]["object"]["pos"], json!([2.0, 0.0, 0.0]));
    assert_eq!(next(&rx)["error"]["code"], code::GONE);
    assert_eq!(next(&rx)["error"]["code"], code::INVALID_PARAMS);
}

#[test]
fn control_needs_a_claim_and_only_one_holds_it() {
    let a = Agent::new();
    let (c1, rx1) = client(&a);
    let (c2, rx2) = client(&a);
    let mut g = level();
    ask(&a, c1, 1, method::PAUSE, Value::Null);
    ask(&a, c1, 2, method::CLAIM, Value::Null);
    ask(&a, c2, 3, method::CLAIM, Value::Null);
    a.between_slices(&mut g, 0);
    assert_eq!(next(&rx1)["error"]["code"], code::NOT_CONTROLLER);
    assert_eq!(next(&rx1)["result"]["controller"], true);
    assert_eq!(next(&rx2)["error"]["code"], code::NOT_CONTROLLER);
    // The holder leaves: the other may claim.
    a.disconnect(c1);
    ask(&a, c2, 4, method::CLAIM, Value::Null);
    a.between_slices(&mut g, 0);
    assert_eq!(next(&rx2)["result"]["controller"], true);
}

/// Runs the "game thread" until `stop`: ticks, as a game would.
fn game_thread(a: &Agent, ticks: u64) -> thread::JoinHandle<u64> {
    let a = a.clone();
    thread::spawn(move || {
        let mut g = level();
        for t in 0..ticks {
            g.status.tick = Some(100 + t);
            a.on_tick(&mut g);
        }
        ticks
    })
}

/// Reads lines until the response with `id`, keeping events seen on the way.
fn reply(rx: &Receiver<String>, id: u64, events: &mut Vec<Value>) -> Value {
    loop {
        let v = next(rx);
        if v["method"] == "event" {
            events.push(v["params"].clone());
        } else if v["id"] == id {
            return v;
        } else {
            panic!("unexpected line while waiting for {id}: {v}");
        }
    }
}

/// Queues claim, subscribe and pause, then starts a game of `ticks` ticks;
/// it pauses at its first tick.
fn paused_game(
    a: &Agent,
    c: ConnId,
    rx: &Receiver<String>,
    pause: Value,
    ticks: u64,
) -> (thread::JoinHandle<u64>, Vec<Value>) {
    let mut events = Vec::new();
    ask(a, c, 1, method::CLAIM, Value::Null);
    ask(a, c, 2, method::SUBSCRIBE, json!({"events": ["pause"]}));
    ask(a, c, 3, method::PAUSE, pause);
    let game = game_thread(a, ticks);
    for id in 1..=3 {
        assert!(reply(rx, id, &mut events).get("result").is_some());
    }
    (game, events)
}

#[test]
fn a_pause_holds_the_tick_answers_and_steps() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    let (game, mut events) = paused_game(&a, c, &rx, Value::Null, 10);
    assert_eq!(events.remove(0)["kind"], "paused");
    // Paused at the first tick, still answering.
    ask(&a, c, 4, method::STATUS, Value::Null);
    let v = reply(&rx, 4, &mut events);
    assert_eq!(v["result"]["at"]["paused"], true);
    assert_eq!(v["result"]["at"]["ticks"], 1);
    assert_eq!(v["result"]["at"]["boundary"], "tick");
    assert!(!game.is_finished());
    // Three ticks, then paused again.
    ask(&a, c, 5, method::STEP, json!({"ticks": 3}));
    let v = reply(&rx, 5, &mut events);
    assert_eq!(v["result"]["ticks"], 3);
    assert_eq!(v["result"]["at"]["ticks"], 4);
    assert_eq!(v["result"]["at"]["paused"], true);
    assert!(!game.is_finished());
    ask(&a, c, 6, method::RESUME, Value::Null);
    reply(&rx, 6, &mut events);
    assert_eq!(game.join().unwrap(), 10);
    let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["paused", "resumed"]);
    assert_eq!(events[1]["why"], "request");
}

#[test]
fn a_forgotten_pause_resumes_by_itself() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    let (game, mut events) = paused_game(&a, c, &rx, json!({"max_ms": 30}), 3);
    assert_eq!(game.join().unwrap(), 3);
    while let Ok(line) = rx.recv_timeout(Duration::from_millis(100)) {
        events.push(serde_json::from_str::<Value>(&line).unwrap()["params"].clone());
    }
    assert!(
        events
            .iter()
            .any(|e| e["kind"] == "resumed" && e["why"] == "timeout")
    );
}

#[test]
fn the_controller_leaving_releases_a_pause() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    let (game, _) = paused_game(&a, c, &rx, Value::Null, 3);
    assert!(a.is_paused());
    a.disconnect(c);
    assert_eq!(game.join().unwrap(), 3);
    assert!(!a.is_paused());
}

#[test]
fn shutdown_lets_a_paused_game_go_and_fails_waits() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    let (game, mut events) = paused_game(&a, c, &rx, Value::Null, 3);
    ask(&a, c, 4, method::WAIT, json!({"until": {"map": "nowhere"}}));
    // Let the paused loop pick the wait up before shutting down.
    thread::sleep(Duration::from_millis(100));
    a.shutdown();
    assert_eq!(game.join().unwrap(), 3);
    assert_eq!(reply(&rx, 4, &mut events)["error"]["code"], code::SHUTDOWN);
}

#[test]
fn step_needs_a_pause_and_a_sane_count() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    ask(&a, c, 1, method::CLAIM, Value::Null);
    ask(&a, c, 2, method::STEP, json!({"ticks": 2}));
    ask(&a, c, 3, method::STEP, json!({"ticks": 0}));
    ask(&a, c, 4, method::STEP, Value::Null);
    a.between_slices(&mut level(), 0);
    next(&rx);
    assert_eq!(next(&rx)["error"]["code"], code::INVALID_REQUEST);
    assert_eq!(next(&rx)["error"]["code"], code::INVALID_PARAMS);
    assert_eq!(next(&rx)["error"]["code"], code::INVALID_PARAMS);
}

#[test]
fn waits_settle_on_ticks_conditions_and_limits() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    let mut g = level();
    ask(&a, c, 1, method::WAIT, json!({"until": {"ticks": 3}}));
    ask(
        &a,
        c,
        2,
        method::WAIT,
        json!({"until": {"map": "05a_deltaapproach"}, "timeout_ticks": 2}),
    );
    ask(&a, c, 3, method::WAIT, json!({"until": {}}));
    ask(&a, c, 4, method::WAIT, json!({"until": {"tick": 50}}));
    a.on_tick(&mut g);
    // Bad until, and one already true, are answered on the first tick.
    assert_eq!(next(&rx)["error"]["code"], code::INVALID_PARAMS);
    let v = next(&rx);
    assert_eq!(
        (v["id"].clone(), v["result"]["ticks"].clone()),
        (json!(4), json!(0))
    );
    none(&rx);
    a.on_tick(&mut g);
    a.on_tick(&mut g);
    let v = next(&rx);
    assert_eq!(v["id"], 2);
    assert_eq!(v["error"]["code"], code::TIMEOUT);
    assert_eq!(v["error"]["data"]["map"], "03a_oldmombasa");
    none(&rx);
    a.on_tick(&mut g);
    let v = next(&rx);
    assert_eq!(
        (v["id"].clone(), v["result"]["ticks"].clone()),
        (json!(1), json!(3))
    );
}

#[test]
fn a_wall_clock_timeout_fires_between_slices() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    ask(
        &a,
        c,
        1,
        method::WAIT,
        json!({"until": {"phase": "gameplay"}, "timeout_ms": 10}),
    );
    a.between_slices(&mut Fake::default(), 0);
    none(&rx);
    thread::sleep(Duration::from_millis(20));
    a.between_slices(&mut Fake::default(), 0);
    assert_eq!(next(&rx)["error"]["code"], code::TIMEOUT);
}

#[test]
fn map_phase_and_bsp_changes_are_events_for_subscribers_only() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    let (_quiet, quiet_rx) = client(&a);
    ask(
        &a,
        c,
        1,
        method::SUBSCRIBE,
        json!({"events": ["map", "phase", "bsp"]}),
    );
    let mut g = Fake {
        status: GameStatus {
            tick: Some(1),
            map: Some("mainmenu".into()),
            mode: Some("main_menu".into()),
            bsp: Some(0),
        },
        ..Default::default()
    };
    a.on_tick(&mut g);
    let mut events = Vec::new();
    reply(&rx, 1, &mut events);
    g.status = level().status;
    g.status.bsp = Some(1);
    a.on_tick(&mut g);
    while let Ok(line) = rx.try_recv() {
        events.push(serde_json::from_str::<Value>(&line).unwrap()["params"].clone());
    }
    let seen: Vec<(String, String)> = events
        .iter()
        .map(|e| {
            let to = e.get("to").map_or(String::new(), |t| t.to_string());
            (e["kind"].as_str().unwrap().to_owned(), to)
        })
        .collect();
    // Subscribed after the first tick's read: the menu loading isn't seen.
    assert_eq!(
        seen,
        [
            ("map".into(), "\"03a_oldmombasa\"".into()),
            ("bsp".into(), "1".into()),
            ("phase".into(), "\"gameplay\"".into()),
        ]
    );
    none(&quiet_rx);
}

#[test]
fn a_cutscene_is_its_own_phase_and_ends_with_its_map() {
    let a = Agent::new();
    let mut g = level();
    a.set_cinematic(true);
    a.on_tick(&mut g);
    let (c, rx) = client(&a);
    ask(&a, c, 1, method::STATUS, Value::Null);
    a.on_tick(&mut g);
    assert_eq!(next(&rx)["result"]["at"]["phase"], "cinematic");
    g.status.map = Some("03b_newmombasa".into());
    ask(&a, c, 2, method::STATUS, Value::Null);
    a.on_tick(&mut g);
    assert_eq!(next(&rx)["result"]["at"]["phase"], "gameplay");
}

#[test]
fn bad_lines_get_json_rpc_errors_and_notifications_get_nothing() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    a.submit(c, "{not json");
    a.submit(c, r#"{"jsonrpc":"1.0","id":7,"method":"session.status"}"#);
    a.submit(c, r#"{"method":"session.status"}"#);
    ask(&a, c, 8, "session.dance", Value::Null);
    a.between_slices(&mut level(), 0);
    assert_eq!(next(&rx)["error"]["code"], code::PARSE);
    let v = next(&rx);
    assert_eq!(
        (v["id"].clone(), v["error"]["code"].clone()),
        (json!(7), json!(code::INVALID_REQUEST))
    );
    let v = next(&rx);
    assert_eq!(
        (v["id"].clone(), v["error"]["code"].clone()),
        (json!(8), json!(code::METHOD_NOT_FOUND))
    );
    none(&rx);
}

#[test]
fn methods_and_features_list() {
    let a = Agent::new();
    a.set_plan(vec!["lockstep: on".into(), "agent: on".into()]);
    let (c, rx) = client(&a);
    ask(&a, c, 1, method::METHODS, Value::Null);
    ask(&a, c, 2, method::FEATURES, Value::Null);
    a.between_slices(&mut level(), 0);
    assert_eq!(
        next(&rx)["result"].as_array().unwrap().len(),
        method::ALL.len()
    );
    assert_eq!(next(&rx)["result"]["plan"][1], "agent: on");
}

// ---- review findings ----

#[test]
fn a_long_pause_then_step_never_reports_loading() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    let mut g = level();
    a.on_tick(&mut g);
    ask(&a, c, 1, method::SUBSCRIBE, json!({"events": ["phase"]}));
    ask(&a, c, 2, method::CLAIM, Value::Null);
    ask(&a, c, 3, method::PAUSE, Value::Null);
    // The game: a tick, then a slice, as boot runs them.
    let game = {
        let a = a.clone();
        thread::spawn(move || {
            let mut g = level();
            for _ in 0..3 {
                a.on_tick(&mut g);
                a.between_slices(&mut g, 0);
            }
        })
    };
    let mut events = Vec::new();
    for id in 1..=3 {
        reply(&rx, id, &mut events);
    }
    // Held longer than a tick may go missing before it counts as loading.
    thread::sleep(TICK_STALE + Duration::from_millis(100));
    ask(&a, c, 4, method::STEP, json!({"ticks": 1}));
    assert_eq!(reply(&rx, 4, &mut events)["result"]["ticks"], 1);
    ask(&a, c, 5, method::RESUME, Value::Null);
    reply(&rx, 5, &mut events);
    game.join().unwrap();
    assert!(events.is_empty(), "no phase change: {events:?}");
}

#[test]
fn a_pause_asked_while_loading_still_runs_out() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    ask(&a, c, 1, method::CLAIM, Value::Null);
    ask(&a, c, 2, method::PAUSE, json!({"max_ms": 20}));
    a.between_slices(&mut level(), 0);
    next(&rx);
    next(&rx);
    assert!(a.is_paused());
    thread::sleep(Duration::from_millis(30));
    a.between_slices(&mut level(), 0);
    assert!(!a.is_paused());
}

#[test]
fn without_the_tick_hook_nothing_pauses_and_the_mode_gives_the_phase() {
    let a = Agent::new();
    a.set_no_tick_hook();
    let (c, rx) = client(&a);
    ask(&a, c, 1, method::CLAIM, Value::Null);
    ask(&a, c, 2, method::PAUSE, Value::Null);
    ask(&a, c, 3, method::STEP, json!({"ticks": 1}));
    ask(&a, c, 4, method::WORLD_COUNT, Value::Null);
    a.between_slices(&mut level(), 0);
    next(&rx);
    assert_eq!(next(&rx)["error"]["code"], code::UNAVAILABLE);
    assert_eq!(next(&rx)["error"]["code"], code::UNAVAILABLE);
    let v = next(&rx);
    assert_eq!(v["result"]["total"], 5);
    assert_eq!(v["result"]["at"]["phase"], "gameplay");
    assert_eq!(v["result"]["at"]["boundary"], "slice");
}

#[test]
fn a_resume_during_a_step_answers_the_step() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    let (game, mut events) = paused_game(&a, c, &rx, Value::Null, 3);
    ask(&a, c, 4, method::STEP, json!({"ticks": 10}));
    // The game runs its last two ticks and ends, short of ten.
    game.join().unwrap();
    ask(&a, c, 5, method::RESUME, Value::Null);
    a.between_slices(&mut level(), 0);
    let v = reply(&rx, 4, &mut events);
    assert_eq!(v["error"]["code"], code::INTERRUPTED);
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("after 2 ticks")
    );
    assert!(reply(&rx, 5, &mut events).get("result").is_some());
}

#[test]
fn a_second_pause_during_a_step_answers_the_step() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    let (game, mut events) = paused_game(&a, c, &rx, Value::Null, 2);
    ask(&a, c, 4, method::STEP, json!({"ticks": 10}));
    game.join().unwrap();
    ask(&a, c, 5, method::PAUSE, Value::Null);
    a.between_slices(&mut level(), 0);
    assert_eq!(
        reply(&rx, 4, &mut events)["error"]["code"],
        code::INTERRUPTED
    );
    reply(&rx, 5, &mut events);
    assert!(a.is_paused());
}

#[test]
fn a_half_closed_connection_still_gets_its_answers_then_goes() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    ask(&a, c, 1, method::STATUS, Value::Null);
    a.disconnect(c);
    a.between_slices(&mut level(), 0);
    assert_eq!(next(&rx)["id"], 1);
    // Nothing left to answer: dropped, so its writer ends.
    assert!(matches!(
        rx.recv_timeout(Duration::from_millis(100)),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
}

#[test]
fn requests_after_shutdown_are_refused() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    a.shutdown();
    ask(&a, c, 1, method::STATUS, Value::Null);
    assert_eq!(next(&rx)["error"]["code"], code::SHUTDOWN);
}

#[test]
fn non_requests_are_invalid_and_unknown_params_refused() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    a.submit(c, r#"{"id": 3}"#);
    a.submit(c, "[1, 2]");
    ask(
        &a,
        c,
        4,
        method::WAIT,
        json!({"until": {"phase": "gameplay", "mapp": "x"}}),
    );
    a.between_slices(&mut level(), 0);
    let v = next(&rx);
    assert_eq!(
        (v["id"].clone(), v["error"]["code"].clone()),
        (json!(3), json!(code::INVALID_REQUEST))
    );
    assert_eq!(next(&rx)["error"]["code"], code::INVALID_REQUEST);
    let v = next(&rx);
    assert_eq!(v["error"]["code"], code::INVALID_PARAMS);
    assert!(v["error"]["message"].as_str().unwrap().contains("mapp"));
}

#[test]
fn a_closed_connection_cannot_take_control() {
    let a = Agent::new();
    let (c1, rx1) = client(&a);
    ask(&a, c1, 1, method::CLAIM, Value::Null);
    ask(&a, c1, 2, method::PAUSE, json!({"max_ms": 60000}));
    a.disconnect(c1);
    a.between_slices(&mut level(), 0);
    assert_eq!(next(&rx1)["error"]["code"], code::NOT_CONTROLLER);
    assert_eq!(next(&rx1)["error"]["code"], code::NOT_CONTROLLER);
    assert!(!a.is_paused());
    let (c2, rx2) = client(&a);
    ask(&a, c2, 3, method::CLAIM, Value::Null);
    a.between_slices(&mut level(), 0);
    assert_eq!(next(&rx2)["result"]["controller"], true);
}

/// Seen on the real main menu: its background runs `cinematic_start`.
#[test]
fn the_main_menu_is_menu_even_under_its_cutscene() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    a.set_cinematic(true);
    ask(&a, c, 1, method::STATUS, Value::Null);
    a.on_tick(&mut Fake {
        status: GameStatus {
            tick: Some(1),
            map: Some("mainmenu".into()),
            mode: Some("main_menu".into()),
            bsp: Some(0),
        },
        ..Default::default()
    });
    assert_eq!(next(&rx)["result"]["at"]["phase"], "menu");
}

/// Seen on a real level: its first tick comes before the opening cutscene,
/// so the stamp says how long the map has run.
#[test]
fn map_ticks_count_from_each_map_load() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    let mut g = level();
    ask(
        &a,
        c,
        1,
        method::WAIT,
        json!({"until": {"phase": "gameplay", "map_ticks": 3}}),
    );
    a.on_tick(&mut g);
    a.on_tick(&mut g);
    none(&rx);
    a.on_tick(&mut g);
    assert_eq!(next(&rx)["result"]["at"]["map_ticks"], 3);
    g.status.map = Some("03b_newmombasa".into());
    ask(&a, c, 2, method::STATUS, Value::Null);
    a.on_tick(&mut g);
    let v = next(&rx);
    assert_eq!(
        (
            v["result"]["at"]["map_ticks"].clone(),
            v["result"]["at"]["ticks"].clone()
        ),
        (json!(1), json!(4))
    );
}

// ---- from the real-game runs ----

/// A read between slices while the game ticks could catch a tick half done
/// (seen: a BSP switch read as 0, none, 3): answers keep the last tick's.
#[test]
fn slices_while_ticking_keep_the_last_ticks_state() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    ask(&a, c, 1, method::SUBSCRIBE, json!({"events": ["bsp"]}));
    let mut g = level();
    a.on_tick(&mut g);
    next(&rx);
    g.status.bsp = None;
    ask(&a, c, 2, method::STATUS, Value::Null);
    a.between_slices(&mut g, 5);
    let v = next(&rx);
    assert_eq!(v["result"]["at"]["bsp"], 0);
    assert_eq!(v["result"]["at"]["boundary"], "tick");
    assert_eq!(v["result"]["at"]["present"], 5);
    none(&rx);
}

#[test]
fn a_maps_first_tick_says_one_in_its_events() {
    let a = Agent::new();
    let (c, rx) = client(&a);
    ask(&a, c, 1, method::SUBSCRIBE, json!({"events": ["map"]}));
    a.between_slices(&mut Fake::default(), 0);
    next(&rx);
    a.on_tick(&mut level());
    let v = next(&rx);
    assert_eq!(v["params"]["kind"], "map");
    assert_eq!(v["params"]["at"]["map_ticks"], 1);
}

#[test]
fn a_client_that_stops_reading_is_dropped() {
    let a = Agent::new();
    let (tx, rx) = sync_channel(1);
    let c = a.connect(tx);
    ask(&a, c, 1, method::CLAIM, Value::Null);
    for id in 2..6 {
        ask(&a, c, id, method::STATUS, Value::Null);
    }
    // The game thread never blocks on the full queue.
    a.between_slices(&mut level(), 0);
    assert_eq!(next(&rx)["id"], 1);
    assert!(matches!(
        rx.recv_timeout(Duration::from_millis(100)),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
    // Its control went with it.
    let (c2, rx2) = client(&a);
    ask(&a, c2, 7, method::CLAIM, Value::Null);
    a.between_slices(&mut level(), 0);
    assert_eq!(next(&rx2)["result"]["controller"], true);
}
