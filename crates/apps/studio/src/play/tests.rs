//! Play and pair against the agent crate's real server and executor, with
//! an invented game behind it: a status, one player and a vehicle, and the
//! host methods the editor uses (`edit.tags`, `edit.set`, `sim.session`,
//! `sim.state.set`) recording what they are sent. No game, no game data.
#![cfg(unix)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agent::{Access, Agent, GameStatus, Reader, Server};
use agent_proto::sim::SessionKind;
use agent_proto::live;
use agent_proto::{Endpoint, Object, RpcError, code};
use scenario_edit::{BlockKind, Command, Value as FieldValue};
use serde_json::{Value, json};

use super::testing::Temp;
use super::*;
use crate::app::{Action, App};

const PLAYER_AT: [f32; 3] = [10.0, -4.0, 2.0];
const VEHICLE: u32 = 0x0010_0002;
const UNIT: u32 = 0x0010_0001;
const SCNR: &str = "0xe1780003";

struct World {
    map: String,
    tick: u64,
    moved: Arc<AtomicBool>,
}

impl Reader for World {
    fn status(&mut self) -> GameStatus {
        GameStatus {
            tick: Some(self.tick),
            map: Some(self.map.clone()),
            mode: Some("multiplayer".into()),
            bsp: Some(0),
        }
    }

    fn objects(&mut self) -> Result<Vec<Object>, String> {
        panic!("the editor must read the mirror, not poll world.query")
    }

    fn live_capture(&mut self, layers: live::Layers, world: &mut live::World) -> bool {
        world.objects.clear();
        world.players.clear();
        if layers.objects {
            let unit = live::Object {
                id: UNIT,
                tag: Some(0x1234),
                kind: 0,
                position: PLAYER_AT,
                scale: 1.0,
                forward: [0.0, 1.0, 0.0],
                up: [0.0, 0.0, 1.0],
                ..Default::default()
            };
            world.objects.push(unit);
            world.objects.push(live::Object {
                id: VEHICLE,
                kind: 1,
                position: if self.moved.load(Ordering::Relaxed) {
                    [5.0, 2.0, 3.0]
                } else {
                    [1.0, 2.0, 3.0]
                },
                ..unit
            });
        }
        if layers.players {
            world.players.push(live::Player {
                index: 0,
                name: "Tester".into(),
                unit: Some(UNIT),
                local_user: Some(0),
                aim: [0.0, 1.0, 0.0],
                ..Default::default()
            });
        }
        true
    }
}

/// The invented game: what the host methods were sent.
struct Game {
    path: std::path::PathBuf,
    agent: Agent,
    _server: Server,
    stop: Arc<AtomicBool>,
    moved: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Game {
    fn start(tag: &str, map: &str, kind: SessionKind) -> Game {
        let path = std::env::temp_dir().join(format!("h2e-{}-{tag}.sock", std::process::id()));
        let agent = Agent::new();
        let calls: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        let rec = |name: &'static str| {
            let calls = Arc::clone(&calls);
            move |v: Value| {
                calls.lock().unwrap().push((name.to_owned(), v.clone()));
                v
            }
        };
        let r = rec("edit.tags");
        agent.add_method("edit.tags", Access::Read, move |v| {
            r(v);
            Ok(json!({"scenario": SCNR, "names": true, "total": 1, "tags": []}))
        });
        let r = rec("edit.set");
        agent.add_method("edit.set", Access::Act, move |v| {
            let v = r(v);
            // As hot-tags answers: the bytes that were there.
            let n = v["value"].as_str().map_or(0, str::len);
            Ok(json!({"id": 1, "path": v["path"], "old": "f".repeat(n), "new": v["value"]}))
        });
        let view = serde_json::to_value(
            agent_proto::sim::SessionView {
                kind,
                ..Default::default()
            }
            .settle(),
        )
        .unwrap();
        agent.add_method(agent_proto::sim::method::SESSION, Access::Read, move |_| {
            Ok(view.clone())
        });
        let r = rec("sim.state.set");
        agent.add_method(agent_proto::sim::method::STATE_SET, Access::Act, move |v| {
            // The game's own refusal (boot's sim glue).
            kind.writes()
                .map_err(|m| RpcError::new(code::INVALID_REQUEST, m))?;
            r(v);
            Ok(json!({"wrote": ["pos"]}))
        });
        let server = Server::listen(&path, agent.clone()).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let (s, a) = (Arc::clone(&stop), agent.clone());
        let map = map.to_owned();
        let moved = Arc::new(AtomicBool::new(false));
        let game_moved = Arc::clone(&moved);
        let thread = thread::spawn(move || {
            let mut w = World { map, tick: 100, moved: game_moved };
            while !s.load(Ordering::Relaxed) {
                w.tick += 1;
                a.on_tick(&mut w);
                a.between_slices(&mut w, 0);
                thread::sleep(Duration::from_millis(5));
            }
        });
        Game {
            path,
            agent,
            _server: server,
            stop,
            moved,
            thread: Some(thread),
            calls,
        }
    }

    fn calls(&self, name: &str) -> Vec<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .collect()
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

/// The editor with the invented map open as `example`.
fn editor(t: &Temp) -> App {
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(t.map("example")));
    assert_eq!(app.loaded_map(), Some("example"));
    app
}

/// Runs the editor's frames until `done` holds (or fails after 25 s; a call to a game that
/// stopped answering times out after 10).
fn until(play: &mut Play, app: &mut App, what: &str, mut done: impl FnMut(&Play, &App) -> bool) {
    let end = Instant::now() + Duration::from_secs(25);
    while Instant::now() < end {
        play.update(app);
        if done(play, app) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "timed out waiting for {what}; log: {:?}",
        app.log().iter().map(|l| &l.status).collect::<Vec<_>>()
    );
}

fn paired(tag: &str, map: &str, kind: SessionKind) -> (Temp, Game, App, Play) {
    let t = Temp::new(tag);
    let game = Game::start(tag, map, kind);
    let mut app = editor(&t);
    let mut play = Play::new();
    play.pair(&mut app, Endpoint::Unix(game.path.clone()));
    until(&mut play, &mut app, "the paired stream", |p, a| {
        p.is_paired() && p.session.is_some() && p.scenario_tag.is_some() && a.pairing.player().is_some()
    });
    (t, game, app, play)
}

fn move_flag(app: &mut App, to: [f32; 3]) {
    app.session
        .apply(Command::set(
            BlockKind::NetgameFlags,
            0,
            "position",
            FieldValue::Vec3(to),
        ))
        .unwrap();
}

#[test]
fn pairing_shows_the_game_and_its_objects() {
    let (_t, _game, mut app, mut play) = paired("show", "example", SessionKind::MpHost);
    assert_eq!(play.map(), Some("example"));
    assert!(
        play.chip().starts_with("Game: example playing tick"),
        "{}",
        play.chip()
    );
    let sample = app.pairing.shown.as_ref().unwrap();
    let ids: Vec<u32> = sample.objects.iter().map(|o| o.id).collect();
    assert_eq!(ids, [UNIT, VEHICLE]);
    assert_eq!(app.pairing.player().unwrap().0.name, "Tester");
    assert_eq!(play.edits_blocked(&app), None);
    // Go to player: the camera's focus is on them, looking their way (+y).
    play.go_to_player(&mut app);
    let cam = *app.session.camera();
    assert_eq!(cam.target[..2], [10.0, -4.0]);
    let f = cam.forward();
    assert!(f[1] > 0.9 && f[0].abs() < 1e-3, "{f:?}");
    // Unpairing ends it.
    play.unpair(&mut app);
    assert_eq!(play.chip(), "Game: not paired");
    assert!(app.pairing.mirror.is_none() && app.pairing.shown.is_none());
}

#[test]
fn map_identity_tracks_the_frame_epoch_and_the_loaded_file() {
    let t = Temp::new("map-identity");
    let mut app = editor(&t);
    let sample = live_mirror::Sample {
        tick: 100, epoch: 2, stale: false, objects: Vec::new(), players: Vec::new()
    };
    let checksum = app.session.source_checksum().unwrap();
    app.pairing.stream_map = json!({"name": "example", "epoch": 1, "checksum": checksum});
    assert!(!app.pairing.matches_sample(&app, &sample));
    app.pairing.stream_map["epoch"] = json!(2);
    assert!(app.pairing.matches_sample(&app, &sample));
    move_flag(&mut app, [2.0, 4.0, 8.0]);
    assert!(app.pairing.matches_sample(&app, &sample));
    let other = !checksum;
    app.pairing.stream_map["checksum"] = json!(other);
    assert!(!app.pairing.matches_sample(&app, &sample));
    app.pairing.play_copy = Some((app.session.world_rev(), other));
    assert!(app.pairing.matches_sample(&app, &sample));
    app.pairing.stream_map["name"] = json!("another");
    assert!(!app.pairing.matches_sample(&app, &sample));
}

#[test]
fn a_moved_game_object_reaches_mirror_objects_within_two_frames() {
    let (_t, game, mut app, mut play) = paired("motion", "example", SessionKind::Local);
    let mut api = crate::api::Api::new();
    let mut read = |app: &mut App| {
        let answer = api.call(app, studio_proto::method::MIRROR_OBJECTS, json!({"kind":"vehi"})).unwrap();
        assert_eq!(answer["objects"].as_array().unwrap().len(), 1);
        assert_eq!(answer["objects"][0]["id"], agent_proto::sim::objects::handle(VEHICLE));
        (answer["tick"].as_u64().unwrap(), answer["objects"][0]["pos"].clone())
    };
    // Hold the fake game, so the test controls how many frames it can send.
    play.pause();
    until(&mut play, &mut app, "the paused tick", |p, a| {
        p.status.as_ref().is_some_and(|s| s.at.paused &&
            s.at.tick == a.pairing.shown.as_ref().map(|shown| shown.tick))
    });
    let (before, old) = read(&mut app);
    assert_eq!(old, json!([1.0, 2.0, 3.0]));
    game.moved.store(true, Ordering::Relaxed);
    play.step(2);
    until(&mut play, &mut app, "two game ticks", |_, a| {
        a.pairing.shown.as_ref().is_some_and(|s| s.tick == before + 2)
    });
    let (after, moved) = read(&mut app);
    assert_eq!(after, before + 2);
    assert_eq!(moved, json!([5.0, 2.0, 3.0]));
    play.resume();
    until(&mut play, &mut app, "the resume", |p, _| !p.paused);
}

#[test]
fn a_moved_placement_is_written_to_the_games_scenario() {
    let (_t, game, mut app, mut play) = paired("edit", "example", SessionKind::Local);
    move_flag(&mut app, [5.0, 6.0, 7.0]);
    until(&mut play, &mut app, "the edit", |_, _| {
        !game.calls("edit.set").is_empty()
    });
    let sets = game.calls("edit.set");
    assert_eq!(sets.len(), 1, "{sets:?}");
    let s = &sets[0];
    assert_eq!(s["tag"], SCNR);
    assert_eq!(s["group"], "scnr");
    let b = scenario_edit::layout::block(BlockKind::NetgameFlags);
    let f = b.field("position").unwrap();
    assert_eq!(
        s["path"],
        format!(
            "+{:#x}/{:#x}[0].+{:#x}:bytes[12]",
            b.offset, b.stride, f.offset
        )
    );
    let want: Vec<u8> = [5.0f32, 6.0, 7.0]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    assert_eq!(s["value"], sync::hex(&want));
    // The invented game's copy differed (all ff): the editor says so once.
    until(&mut play, &mut app, "the answer", |p, _| p.sent.is_empty());
    assert!(app.log().iter().any(
        |l| matches!(&l.status, editor_core::Status::Error(m) if m.contains("another version"))
    ),);
    // Nothing more goes until something changes; undo sends the old bytes.
    play.update(&mut app);
    app.dispatch(Action::Undo);
    until(&mut play, &mut app, "the undo", |_, _| {
        game.calls("edit.set").len() == 2
    });
    assert_ne!(game.calls("edit.set")[1]["value"], s["value"]);
    // An added entry is a reload, not a write.
    app.session.set_allow_growth(true);
    let kind = scenario_edit::layout::BLOCKS
        .iter()
        .map(|b| b.kind)
        .find(|k| app.session.add_refusal(*k).is_none())
        .expect("the invented map has a block that grows");
    app.session.add_entry(kind, None).unwrap();
    until(&mut play, &mut app, "the reload note", |p, _| {
        p.reload == [kind]
    });
    assert_eq!(game.calls("edit.set").len(), 2);
}

#[test]
fn teleport_and_spawn_move_the_player() {
    let (_t, game, mut app, mut play) = paired("tp", "example", SessionKind::MpHost);
    app.session.camera_mut().target = [3.0, 4.0, 5.0];
    play.teleport_to_camera(&mut app);
    until(&mut play, &mut app, "the teleport", |_, _| {
        !game.calls("sim.state.set").is_empty()
    });
    let s = &game.calls("sim.state.set")[0];
    assert_eq!(s["player"], "player.0");
    // No structure in the invented map: the point itself, lifted.
    assert_eq!(s["pos"], json!([3.0, 4.0, 5.0 + f64::from(LIFT)]));
    // A launched game's spawn waits for gameplay, then moves the player.
    play.pending_spawn = Some([1.0, 2.0, 3.0]);
    until(&mut play, &mut app, "the spawn", |_, _| {
        game.calls("sim.state.set").len() == 2
    });
    assert_eq!(
        game.calls("sim.state.set")[1]["pos"],
        json!([1.0, 2.0, 3.0])
    );
}

#[test]
fn a_coop_session_gets_no_writes() {
    let (_t, game, mut app, mut play) = paired("coop", "example", SessionKind::CoopHost);
    let why = play.edits_blocked(&app).unwrap();
    assert!(why.contains("co-op"), "{why}");
    move_flag(&mut app, [5.0, 6.0, 7.0]);
    play.teleport_to_camera(&mut app);
    play.pending_spawn = Some([1.0, 2.0, 3.0]);
    let end = Instant::now() + Duration::from_millis(600);
    while Instant::now() < end {
        play.update(&mut app);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(game.calls("edit.set").is_empty());
    assert!(game.calls("sim.state.set").is_empty());
    assert!(
        play.pending_spawn.is_none(),
        "the spawn is dropped, not kept"
    );
}

#[test]
fn another_map_in_the_game_blocks_edits() {
    let (_t, game, mut app, mut play) = paired("other", "zanzibar", SessionKind::Local);
    let why = play.edits_blocked(&app).unwrap();
    assert!(why.contains("zanzibar") && why.contains("example"), "{why}");
    move_flag(&mut app, [5.0, 6.0, 7.0]);
    for _ in 0..50 {
        play.update(&mut app);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(game.calls("edit.set").is_empty());
}

#[test]
fn pause_step_and_resume_hold_control_only_while_needed() {
    let (_t, game, mut app, mut play) = paired("pause", "example", SessionKind::Local);
    play.pause();
    until(&mut play, &mut app, "the pause", |p, _| {
        p.status.as_ref().is_some_and(|s| s.at.paused)
    });
    assert!(game.agent.is_paused());
    assert!(play.chip().contains("paused"), "{}", play.chip());
    assert!(play.controller);
    play.step(2);
    until(&mut play, &mut app, "the step", |_, a| {
        a.log()
            .iter()
            .any(|l| matches!(&l.status, editor_core::Status::Info(m) if m == "game: step"))
    });
    play.resume();
    until(&mut play, &mut app, "the resume", |p, _| {
        p.status
            .as_ref()
            .is_some_and(|s| !s.at.paused && !s.controller)
    });
    assert!(!game.agent.is_paused());
}

#[test]
fn a_game_that_goes_away_is_lost() {
    let (_t, game, mut app, mut play) = paired("lost", "example", SessionKind::Local);
    drop(game);
    until(&mut play, &mut app, "the loss", |p, _| {
        matches!(p.conn, Conn::Lost(_))
    });
    assert_eq!(play.chip(), "Game: lost");
}

#[test]
fn nothing_listening_is_reported() {
    let t = Temp::new("none");
    let mut app = editor(&t);
    let mut play = Play::new();
    play.pair(&mut app, Endpoint::Unix(t.0.join("nobody.sock")));
    until(&mut play, &mut app, "the failure", |p, _| {
        matches!(p.conn, Conn::Lost(_))
    });
}

#[test]
fn play_writes_the_edited_map_for_the_override() {
    let t = Temp::new("prepare");
    let mut app = editor(&t);
    move_flag(&mut app, [5.0, 6.0, 7.0]);
    let mut play = Play::new();
    play.h2_path = "/opt/h2".into();
    // The source is a loose map: the disc comes from H2_DISC or the
    // repository's disc/; without either, Play says so.
    match play.prepare(&mut app) {
        Ok(l) => {
            assert_eq!(l.level, "example");
            assert_eq!(l.h2, std::path::PathBuf::from("/opt/h2"));
            let dir = l.map_dir.unwrap();
            let written = std::fs::read(dir.join("example.map")).unwrap();
            let mut s = scenario_edit::Session::new();
            s.open(scenario_edit::MapSource::file(dir.join("example.map")))
                .unwrap();
            let p = s
                .scenario()
                .unwrap()
                .get(BlockKind::NetgameFlags, 0, "position")
                .unwrap()
                .as_vec3()
                .unwrap();
            assert_eq!(p, [5.0, 6.0, 7.0]);
            assert!(!written.is_empty());
            assert!(app.is_dirty(), "writing the play copy is not a save");
            let _ = std::fs::remove_dir_all(launch::scratch());
        }
        Err(e) => assert!(e.contains("disc"), "{e}"),
    }
}

// ---- finding games ----

/// A finder that looks only at these sockets (the real one reads the
/// user's agents directory).
fn finder_for(paths: &[&Game]) -> pair::Finder {
    let paths: Vec<PathBuf> = paths.iter().map(|g| g.path.clone()).collect();
    pair::Finder::with_scan(std::sync::Arc::new(move || {
        agent_proto::discover::scan_paths(&paths, Duration::from_secs(5), false)
    }))
}

#[test]
fn a_running_game_is_offered_with_its_map_and_pairs_on_request() {
    let t = Temp::new("offer");
    let game = Game::start("offer", "example", SessionKind::Local);
    let mut app = editor(&t);
    app.pairing.auto_pair = false;
    let mut play = Play::new();
    play.finder = finder_for(&[&game]);
    until(&mut play, &mut app, "the game to be found", |p, a| p.notice(a).is_some());
    let n = play.notice(&app).unwrap();
    assert_eq!(n.matches, MapMatch::Same);
    assert!(n.text.contains("example"), "{}", n.text);
    assert_eq!(n.socket, Some(game.path.clone()));
    assert!(
        play.chip_in(&app).starts_with("Game: Halo 2 is running example"),
        "{}",
        play.chip_in(&app)
    );
    // Auto-pair is off: it waits for the button.
    assert_eq!(play.conn, Conn::Off);
    assert_eq!(play.auto_target(false), None);
    play.request(Request::Pair(Endpoint::Unix(game.path.clone())));
    until(&mut play, &mut app, "the pairing", |p, _| p.is_paired() && p.map().is_some());
    assert!(play.notice(&app).is_none(), "paired: nothing to offer");
    assert!(play.chip_in(&app).contains("(the open map)"), "{}", play.chip_in(&app));
}

#[test]
fn a_game_on_another_map_is_offered_and_says_so() {
    let t = Temp::new("other");
    let game = Game::start("other", "zanzibar", SessionKind::Local);
    let mut app = editor(&t);
    app.pairing.auto_pair = false;
    let mut play = Play::new();
    play.finder = finder_for(&[&game]);
    until(&mut play, &mut app, "the game to be found", |p, a| p.notice(a).is_some());
    let n = play.notice(&app).unwrap();
    assert_eq!(n.matches, MapMatch::Different);
    assert!(n.text.contains("zanzibar") && n.text.contains("example"), "{}", n.text);
    play.request(Request::Pair(Endpoint::Unix(game.path.clone())));
    until(&mut play, &mut app, "the pairing", |p, _| p.is_paired() && p.map().is_some());
    assert!(play.chip_in(&app).contains("the editor has example"), "{}", play.chip_in(&app));
    assert!(play.edits_blocked(&app).unwrap().contains("zanzibar"));
}

#[test]
fn the_one_running_game_is_paired_on_its_own_once() {
    let t = Temp::new("auto");
    let game = Game::start("auto", "example", SessionKind::Local);
    let mut app = editor(&t);
    assert!(app.pairing.auto_pair, "on by default");
    let mut play = Play::new();
    play.finder = finder_for(&[&game]);
    until(&mut play, &mut app, "the automatic pairing", |p, _| {
        p.is_paired() && p.map().is_some()
    });
    // After Unpair it stays unpaired, however often it looks.
    play.unpair(&mut app);
    for _ in 0..3 {
        play.finder.look_again();
        let end = Instant::now() + Duration::from_millis(300);
        while Instant::now() < end {
            play.update(&mut app);
            thread::sleep(Duration::from_millis(5));
        }
    }
    assert_eq!(play.conn, Conn::Off);
    assert_eq!(play.auto_target(true), None);
    assert!(play.notice(&app).is_some(), "still offered by hand");
}

#[test]
fn several_games_are_not_paired_for_the_user() {
    let t = Temp::new("many");
    let a = Game::start("many-a", "example", SessionKind::Local);
    let b = Game::start("many-b", "zanzibar", SessionKind::Local);
    let mut app = editor(&t);
    let mut play = Play::new();
    play.finder = finder_for(&[&a, &b]);
    until(&mut play, &mut app, "both to be found", |p, _| p.finder.running().len() == 2);
    assert_eq!(play.auto_target(true), None);
    let n = play.notice(&app).unwrap();
    assert_eq!(n.socket, None);
    assert!(n.text.starts_with("2 "), "{}", n.text);
    assert_eq!(play.conn, Conn::Off);
}

#[test]
fn a_game_that_is_gone_is_no_longer_offered() {
    let t = Temp::new("gone");
    let game = Game::start("gone", "example", SessionKind::Local);
    let mut app = editor(&t);
    app.pairing.auto_pair = false;
    let mut play = Play::new();
    play.finder = finder_for(&[&game]);
    until(&mut play, &mut app, "the game to be found", |p, a| p.notice(a).is_some());
    drop(game);
    let end = Instant::now() + Duration::from_secs(25);
    while play.notice(&app).is_some() {
        assert!(Instant::now() < end, "the game was still offered");
        play.finder.look_again();
        play.update(&mut app);
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_agent_api_lists_pairs_and_unpairs() {
    let t = Temp::new("api");
    let game = Game::start("api", "example", SessionKind::Local);
    let mut app = editor(&t);
    app.pairing.auto_pair = false;
    let mut api = crate::api::Api::new();
    // No window yet: nothing is published.
    assert!(api.call(&mut app, "pair.status", Value::Null).is_err());
    assert!(api.call(&mut app, "pair.connect", json!({"socket": "/x"})).is_err());
    assert_eq!(
        api.call(&mut app, "pair.auto", json!({"enabled": false})).unwrap(),
        json!({"auto_pair": false})
    );
    let mut play = Play::new();
    play.update(&mut app);
    let s = api.call(&mut app, "pair.status", Value::Null).unwrap();
    assert_eq!((s["state"].as_str(), s["open_map"].as_str()), (Some("off"), Some("example")));
    let socket = game.path.display().to_string();
    let r = api.call(&mut app, "pair.connect", json!({"socket": socket})).unwrap();
    assert_eq!(r["requested"], socket);
    until(&mut play, &mut app, "the pairing", |p, _| p.is_paired() && p.map().is_some());
    let s = api.call(&mut app, "pair.status", Value::Null).unwrap();
    assert_eq!(s["state"], "connected");
    assert_eq!(s["map"], "example");
    assert_eq!(s["matches"], "same");
    assert_eq!(s["socket"], socket);
    api.call(&mut app, "pair.disconnect", Value::Null).unwrap();
    until(&mut play, &mut app, "the unpairing", |p, _| p.conn == Conn::Off);
    let s = api.call(&mut app, "pair.status", Value::Null).unwrap();
    assert_eq!(s["state"], "off");
}
