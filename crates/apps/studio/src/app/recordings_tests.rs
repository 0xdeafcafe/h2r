//! The recordings read over the scenario crate's invented map: invented
//! streams, an invented executable, no game data.
use hs::fixture::{Builtin, Exe, Tables};
use scenario_edit::fixture;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::*;
use crate::api::Api;
use crate::app::map_cards_tests::Temp;
use crate::app::scripts::tests as scripts;
use crate::app::{Action, App};

fn open(map: std::path::PathBuf, t: &Temp) -> App {
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(map));
    assert!(app.session.is_open());
    app
}

#[test]
fn the_block_reads_and_each_stream_decodes_or_says_why() {
    let t = Temp::new("recordings-read");
    let mut app = open(scripts::scripted_source(&t.0, true), &t);
    app.sync_scripts();
    let rows = app.recordings_loaded().unwrap();
    let names: Vec<_> = rows.iter().map(|r| r.recorded.name.as_str()).collect();
    assert_eq!(names, ["arrive", "broken", "empty"]);
    let d = rows[0].decoded.as_ref().unwrap();
    assert_eq!(d.events.len(), 2);
    assert_eq!(d.ticks, 5);
    let why = rows[1].decoded.as_ref().unwrap_err();
    assert!(why.contains("type 30") && why.contains("0x48"), "{why}");
    assert!(rows[2].decoded.as_ref().unwrap_err().contains("initial state"));
    // Another stream version is named, not guessed at.
    let mut other = rows[0].recorded.clone();
    other.version = 2;
    assert!(Row::new(other).decoded.unwrap_err().contains("version 2"));
}

#[test]
fn a_map_without_a_block_reads_empty_and_without_a_map_refuses() {
    let t = Temp::new("recordings-none");
    let mut api = Api::new();
    let mut app = App::new();
    let e = api.call(&mut app, m::RECORDING_LIST, json!({})).unwrap_err();
    assert_eq!(e.code, code::NO_DOCUMENT);
    let mut app = open(
        {
            let map = t.0.join("plain.map");
            std::fs::write(&map, fixture::fixture()).unwrap();
            map
        },
        &t,
    );
    let v = api.call(&mut app, m::RECORDING_LIST, json!({})).unwrap();
    assert_eq!(v["recordings"], json!([]));
    assert_eq!(v["note"], NONE);
}

fn call(api: &mut Api, app: &mut App, method: &str, params: Value) -> Value {
    api.call(app, method, params).unwrap()
}

#[test]
fn recording_list_and_read_answer_over_the_api() {
    let t = Temp::new("recordings-api");
    let mut app = open(scripts::scripted_source(&t.0, true), &t);
    let mut api = Api::new();
    let v = call(&mut api, &mut app, m::RECORDING_LIST, json!({}));
    let rows = v["recordings"].as_array().unwrap();
    assert_eq!(rows.len(), 3, "{v}");
    assert_eq!(rows[0]["name"], "arrive");
    assert_eq!(rows[0]["events"], 2);
    assert_eq!(rows[0]["ticks"], 5);
    assert_eq!(rows[0]["played_by"], json!([]));
    assert_eq!(rows[1]["events"], Value::Null);
    assert!(rows[1]["problem"].as_str().unwrap().contains("type 30"));

    let v = call(&mut api, &mut app, m::RECORDING_READ, json!({"name": "ARRIVE"}));
    assert_eq!(v["walked_ticks"], 5);
    assert_eq!(v["initial"]["fields"]["throttle"], json!([0.0, 0.0]));
    assert_eq!(v["initial"]["trailer"].as_str().unwrap().len(), 24);
    let events = v["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["name"], "throttle");
    assert_eq!(events[0]["tick"], 4);
    assert_eq!(events[0]["fields"]["value"], json!([0.5, 0.0]));
    assert_eq!(events[0]["payload"], "0000003f00000000");
    assert_eq!((events[1]["name"].as_str(), events[1]["tick"].as_u64()), (Some("end"), Some(5)));
    assert_eq!(v["truncated"], false);
    let v = call(&mut api, &mut app, m::RECORDING_READ, json!({"name": "arrive", "limit": 1, "offset": 1}));
    assert_eq!(v["events"].as_array().unwrap().len(), 1);
    assert_eq!(v["events"][0]["name"], "end");
    assert_eq!(v["total"], 2);

    // A broken stream is refused with the offset; an unknown name and a
    // bad limit are bad params.
    let e = api.call(&mut app, m::RECORDING_READ, json!({"name": "broken"})).unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("0x48"), "{}", e.message);
    for params in [json!({"name": "absent"}), json!({"name": "arrive", "limit": 0})] {
        let e = api.call(&mut app, m::RECORDING_READ, params).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
    }
}

/// The invented executable with one builtin that takes a recording.
fn playing_exe() -> Exe {
    let mut types = scripts::types();
    types[40] = "cutscene_recording".to_owned();
    Exe {
        types,
        builtins: vec![Builtin {
            returns: 4,
            parameters: vec![40],
            ..Builtin::default()
        }],
        ..Exe::default()
    }
}

#[test]
fn scripts_that_play_a_recording_are_its_users() {
    let t = Temp::new("recordings-played");
    let mut b = fixture::fixture();
    let payload = &mut b[fixture::sfile(0)..fixture::sfile(fixture::SCNR_SIZE)];
    let base = fixture::META + fixture::SCNR as u32;
    let mut tables = Tables::new();
    // `cut` and `cut_again` each play `arrive` (names match without case);
    // nothing plays the others.
    let first = tables.literal(40, "Arrive", 0);
    let second = tables.literal(40, "arrive", 0);
    let a = tables.call(0, "invented_play", 4, &[first]);
    let again = tables.call(0, "invented_play", 4, &[second]);
    tables.script("cut", 0, 4, a);
    tables.script("cut_again", 0, 4, again);
    tables.write(payload, base, 0xa00);
    hs::fixture::write_recordings(payload, base, 0xd00, &scripts::recordings());
    let map = t.0.join("played.map");
    std::fs::write(&map, b).unwrap();
    std::fs::write(t.0.join("default.xbe"), playing_exe().xbe()).unwrap();
    let mut app = open(map, &t);
    let mut api = Api::new();
    let v = call(&mut api, &mut app, m::RECORDING_LIST, json!({}));
    let by = &v["recordings"][0]["played_by"];
    assert_eq!(by.as_array().unwrap().len(), 2, "{v}");
    assert_eq!(by[0]["count"], 1);
    let v = call(&mut api, &mut app, m::SCRIPT_USES, json!({"kind": "recording", "name": "arrive"}));
    assert_eq!(v["uses"].as_array().unwrap().len(), 2, "{v}");
    let v = call(&mut api, &mut app, m::SCRIPT_USES, json!({"kind": "recording", "name": "empty"}));
    assert_eq!(v["uses"], json!([]));
    let e = api
        .call(&mut app, m::SCRIPT_USES, json!({"kind": "recording", "name": "absent"}))
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
}

#[test]
fn without_the_executable_recordings_list_and_played_by_says_why() {
    let t = Temp::new("recordings-noexe");
    let mut app = open(scripts::scripted_source(&t.0, false), &t);
    let mut api = Api::new();
    let v = call(&mut api, &mut app, m::RECORDING_LIST, json!({}));
    assert_eq!(v["recordings"].as_array().unwrap().len(), 3);
    assert_eq!(v["recordings"][0]["played_by"], Value::Null);
    assert!(v["played_by_unavailable"].as_str().unwrap().contains("executable"), "{v}");
}

/// An invented game that answers `recording.play` and records what it was
/// asked, for the play tests (no game, no game data).
mod game {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use agent::{Access, Agent, GameStatus, Reader, Server};
    use agent_proto::{Object, RpcError};
    use serde_json::{Value, json};

    struct Level;

    impl Reader for Level {
        fn status(&mut self) -> GameStatus {
            GameStatus {
                tick: Some(100),
                map: Some("example".into()),
                mode: Some("multiplayer".into()),
                bsp: Some(0),
            }
        }

        fn objects(&mut self) -> Result<Vec<Object>, String> {
            Ok(Vec::new())
        }
    }

    pub struct Game {
        pub path: PathBuf,
        pub calls: Arc<Mutex<Vec<Value>>>,
        agent: Agent,
        _server: Server,
        stop: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl Game {
        /// `plays`: the game lists `recording.play`. `refuse`: it answers
        /// that the unit is gone.
        pub fn start(tag: &str, plays: bool, refuse: bool) -> Game {
            let path = std::env::temp_dir()
                .join(format!("h2e-rec-{tag}-{}.sock", std::process::id()));
            let agent = Agent::new();
            let calls: Arc<Mutex<Vec<Value>>> = Arc::default();
            if plays {
                let seen = Arc::clone(&calls);
                agent.add_method("recording.play", Access::Act, move |p| {
                    seen.lock().unwrap().push(p.clone());
                    if refuse {
                        return Err(RpcError::new(agent_proto::code::GONE, "player.0 has no unit"));
                    }
                    Ok(json!({
                        "player": p["player"], "recording": p["recording"],
                        "index": 0, "unit": "00010001",
                    }))
                });
            }
            let server = Server::listen(&path, agent.clone()).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let (s, a) = (Arc::clone(&stop), agent.clone());
            let thread = thread::spawn(move || {
                let mut level = Level;
                while !s.load(Ordering::Relaxed) {
                    a.on_tick(&mut level);
                    a.between_slices(&mut level, 0);
                    thread::sleep(Duration::from_millis(2));
                }
            });
            Game { path, calls, agent, _server: server, stop, thread: Some(thread) }
        }
    }

    impl Drop for Game {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            self.agent.shutdown();
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
}

fn pair(app: &mut App, game: &game::Game, paused: bool) {
    app.pairing.published = Some(json!({
        "state": "connected", "socket": game.path.display().to_string(),
        "live_edits": false, "paused": paused, "edit_refusal": null,
    }));
}

fn sample(tick: u64, at: [f32; 3]) -> live_mirror::Sample {
    live_mirror::Sample {
        tick,
        epoch: 1,
        stale: false,
        objects: vec![agent_proto::live::Object { id: 7, position: at, ..Default::default() }],
        players: vec![agent_proto::live::Player { index: 0, unit: Some(7), ..Default::default() }],
        cameras: Vec::new(),
    }
}

#[test]
fn play_on_unit_sends_the_call_and_the_streamed_positions_make_a_path() {
    let t = Temp::new("recordings-play");
    let mut app = open(scripts::scripted_source(&t.0, true), &t);
    let game = game::Game::start("play", true, false);
    pair(&mut app, &game, false);
    let mut api = Api::new();
    let v = call(&mut api, &mut app, m::RECORDING_PLAY, json!({"name": "ARRIVE", "mode": "hover"}));
    // The game is asked by the name as the map spells it.
    assert_eq!(
        *game.calls.lock().unwrap(),
        [json!({"recording": "arrive", "player": "player.0", "mode": "hover"})]
    );
    assert_eq!(v["recording"], "arrive");
    assert_eq!(v["path"]["points"], 0);
    // The mirror's samples, as the window feeds them, make the path.
    for (tick, x) in [(10, 1.0), (10, 5.0), (12, 2.0), (14, 3.0)] {
        let s = sample(tick, [x, 0.0, 0.0]);
        app.pairing.path.record(&s);
    }
    let v = call(&mut api, &mut app, m::RECORDING_PATH, json!({"limit": 2}));
    assert_eq!(v["recording"], "arrive");
    assert_eq!((v["count"].as_u64(), v["truncated"].as_bool()), (Some(3), Some(true)));
    assert_eq!(v["points"], json!([[2.0, 0.0, 0.0], [3.0, 0.0, 0.0]]));
    let v = call(&mut api, &mut app, m::RECORDING_PATH, json!({"clear": true}));
    assert_eq!(v["count"], 3);
    let v = call(&mut api, &mut app, m::RECORDING_PATH, json!({}));
    assert_eq!((v["count"].clone(), v["recording"].clone()), (json!(0), Value::Null));
}

#[test]
fn play_on_unit_is_refused_by_name_and_starts_no_path() {
    let t = Temp::new("recordings-play-refused");
    let mut app = open(scripts::scripted_source(&t.0, true), &t);
    let mut api = Api::new();
    // Not paired.
    let e = api.call(&mut app, m::RECORDING_PLAY, json!({"name": "arrive"})).unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    // Bad params: an unknown recording, a unit that is no player, a mode.
    for params in [
        json!({"name": "absent"}),
        json!({"name": "arrive", "unit": "sim.1"}),
        json!({"name": "arrive", "mode": "kill"}),
    ] {
        let e = api.call(&mut app, m::RECORDING_PLAY, params).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
    }
    // A paused game would not answer; a game without the method; a game
    // that refuses.
    let game = game::Game::start("refused", true, true);
    pair(&mut app, &game, true);
    let e = api.call(&mut app, m::RECORDING_PLAY, json!({"name": "arrive"})).unwrap_err();
    assert!(e.message.contains("paused"), "{}", e.message);
    pair(&mut app, &game, false);
    let e = api.call(&mut app, m::RECORDING_PLAY, json!({"name": "arrive"})).unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("has no unit"), "{}", e.message);
    let old = game::Game::start("old", false, false);
    pair(&mut app, &old, false);
    let e = api.call(&mut app, m::RECORDING_PLAY, json!({"name": "arrive"})).unwrap_err();
    assert!(e.message.contains("does not list recording.play"), "{}", e.message);
    assert_eq!(app.pairing.path.recording, None);
}
