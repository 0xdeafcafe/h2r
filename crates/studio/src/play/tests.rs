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
use agent_proto::{Endpoint, Kind, Object, RpcError, code};
use scenario_edit::{BlockKind, Command, Value as FieldValue};
use serde_json::{Value, json};

use super::testing::Temp;
use super::*;
use crate::app::{Action, App};

const PLAYER_AT: [f64; 3] = [10.0, -4.0, 2.0];
const SCNR: &str = "0xe1780003";

struct World {
    map: String,
}

impl Reader for World {
    fn status(&mut self) -> GameStatus {
        GameStatus {
            tick: Some(100),
            map: Some(self.map.clone()),
            mode: Some("multiplayer".into()),
            bsp: Some(0),
        }
    }
    fn objects(&mut self) -> Result<Vec<Object>, String> {
        Ok(vec![
            Object {
                id: "player.0".into(),
                kind: Kind::Player,
                pos: Some(PLAYER_AT),
                forward: Some([0.0, 1.0, 0.0]),
                team: Some(0),
                name: Some("Tester".into()),
                ..Object::default()
            },
            Object {
                id: "vehicle.e0010002".into(),
                kind: Kind::Vehicle,
                pos: Some([1.0, 2.0, 3.0]),
                ..Object::default()
            },
            // A turret, relative to its vehicle: not drawn.
            Object {
                id: "vehicle.e0020003".into(),
                kind: Kind::Vehicle,
                pos: Some([0.0, 0.0, 1.0]),
                mounted_on: Some("vehicle.e0010002".into()),
                ..Object::default()
            },
        ])
    }
}

/// The invented game: what the host methods were sent.
struct Game {
    path: std::path::PathBuf,
    agent: Agent,
    _server: Server,
    stop: Arc<AtomicBool>,
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
        let thread = thread::spawn(move || {
            let mut w = World { map };
            while !s.load(Ordering::Relaxed) {
                a.on_tick(&mut w);
                a.between_slices(&mut w, 0);
                thread::sleep(Duration::from_millis(1));
            }
        });
        Game {
            path,
            agent,
            _server: server,
            stop,
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
    until(&mut play, &mut app, "the first poll", |p, _| {
        p.is_paired() && p.session.is_some() && p.scenario_tag.is_some() && !p.objects.is_empty()
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
    // The turret rides its vehicle: only the player and the vehicle.
    let ids: Vec<&str> = play.objects.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(ids, ["player.0", "vehicle.e0010002"]);
    assert_eq!(play.player().unwrap().name.as_deref(), Some("Tester"));
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
