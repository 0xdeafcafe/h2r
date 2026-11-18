//! `sim.*` end to end: a client on the socket, a "game thread" ticking, the
//! client-side helpers (`sim::until`, the lobby steps) over a real Client.
#![cfg(unix)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use agent::sim::bots::FakeBots;
use agent::{Access, Agent, GameStatus, Reader, Server};
use agent_proto::expect::Expect;
use agent_proto::sim::menu::{self, JOIN_SCREEN, Lobby, MODE_SYSTEM_LINK, Mode};
use agent_proto::sim::{self, SessionKind, SessionView, method};
use agent_proto::{Client, Kind, Object};
use serde_json::{Value, json};

/// A multiplayer game where player 1 joins team 1 at tick 40.
struct Lockout {
    tick: Arc<AtomicU64>,
}

impl Reader for Lockout {
    fn status(&mut self) -> GameStatus {
        GameStatus {
            tick: Some(self.tick.load(Ordering::Relaxed)),
            map: Some("lockout".into()),
            mode: Some("multiplayer".into()),
            bsp: Some(0),
        }
    }
    fn objects(&mut self) -> Result<Vec<Object>, String> {
        let team = if self.tick.load(Ordering::Relaxed) >= 40 {
            1
        } else {
            0
        };
        Ok(vec![Object {
            id: "player.1".into(),
            kind: Kind::Player,
            team: Some(team),
            local_user: Some(1),
            ..Default::default()
        }])
    }
}

fn socket(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("h2s-{}-{name}.sock", std::process::id()))
}

fn game(a: &Agent) -> (Arc<AtomicBool>, thread::JoinHandle<()>) {
    let stop = Arc::new(AtomicBool::new(false));
    let (s, a) = (Arc::clone(&stop), a.clone());
    let h = thread::spawn(move || {
        let tick = Arc::new(AtomicU64::new(0));
        let mut r = Lockout {
            tick: Arc::clone(&tick),
        };
        while !s.load(Ordering::Relaxed) {
            tick.fetch_add(1, Ordering::Relaxed);
            a.on_tick(&mut r);
            a.between_slices(&mut r, 0);
            thread::sleep(Duration::from_millis(1));
        }
    });
    (stop, h)
}

#[test]
fn a_script_adds_a_bot_and_waits_for_the_game_to_agree() {
    let path = socket("until");
    let a = Agent::new();
    let bots = FakeBots::default();
    agent::sim::install(
        &a,
        Box::new(bots.clone()),
        Some(Box::new(|_| SessionView {
            kind: SessionKind::MpHost,
            ..Default::default()
        })),
    );
    let _server = Server::listen(&path, a.clone()).unwrap();
    let (stop, h) = game(&a);
    let mut c = Client::connect(&path).unwrap();
    c.set_timeout(Some(Duration::from_secs(10))).unwrap();
    let added = c
        .call(
            method::PLAYER_ADD,
            json!({"kind": "local", "source": "brain"}),
        )
        .unwrap();
    assert_eq!(added["player"], "player.1");
    let checks = [
        Expect::parse("/player/1/team == 1").unwrap(),
        Expect::parse("/sim/1/port == 1").unwrap(),
    ];
    let v = sim::until(&mut c, &checks, 600, 5).unwrap();
    assert_eq!(v["pass"], true, "{v}");
    assert!(v["at"]["tick"].as_u64().unwrap() >= 40);
    let e = sim::until(
        &mut c,
        &[Expect::parse("/player/1/team == 7").unwrap()],
        10,
        5,
    )
    .unwrap_err();
    assert_eq!(e.code, agent_proto::code::TIMEOUT);
    assert_eq!(e.data.unwrap()["expects"][0]["got"], 1);
    stop.store(true, Ordering::Relaxed);
    h.join().unwrap();
}

#[test]
fn a_lobby_step_presses_and_reads_the_screen() {
    let path = socket("lobby");
    let a = Agent::new();
    // The menus: input.pad moves from the main menu to the System Link
    // players' screen, as the game's would.
    let screen = Arc::new(Mutex::new(json!({"screen": null})));
    let pads = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (s, p) = (Arc::clone(&screen), Arc::clone(&pads));
    a.add_method(agent_proto::method::INPUT_PAD, Access::Act, move |v| {
        p.lock().unwrap().push(v);
        *s.lock().unwrap() = json!({"screen": JOIN_SCREEN, "mode": MODE_SYSTEM_LINK});
        Ok(json!({"queued": 1}))
    });
    let s = Arc::clone(&screen);
    a.add_method(agent_proto::method::UI_SCREEN, Access::Read, move |_| {
        Ok(s.lock().unwrap().clone())
    });
    let _server = Server::listen(&path, a.clone()).unwrap();
    let (stop, h) = game(&a);
    let mut c = Client::connect(&path).unwrap();
    c.set_timeout(Some(Duration::from_secs(10))).unwrap();
    let r = menu::run(
        &mut c,
        &Lobby::Mode {
            mode: Mode::SystemLink,
        },
    )
    .unwrap();
    assert_eq!(r["screen"]["mode"], MODE_SYSTEM_LINK);
    let sent = &pads.lock().unwrap()[0];
    assert_eq!(sent["player"], 0);
    assert_eq!(sent["steps"].as_array().unwrap().len(), 8, "3 downs and A, each released");
    stop.store(true, Ordering::Relaxed);
    h.join().unwrap();
}
