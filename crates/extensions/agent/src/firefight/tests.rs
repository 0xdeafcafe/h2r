//! `firefight.*` over a fake runtime: the params each method takes, what
//! it answers, and how the runtime's refusals reach the caller.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_proto::firefight::{
    Appended, Command, CommandParams, CommandReply, Director, DirectorRun, Kit, KitZone, Log, Role,
    State,
};
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
            map: Some("firefight-probe".into()),
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

/// A runtime in one role, recording the commands it is given. `teams`
/// is the match's player teams.
struct Fake {
    role: Option<Role>,
    teams: u8,
    held: Mutex<bool>,
    seq: Mutex<u32>,
    got: Mutex<Vec<CommandParams>>,
}

impl Fake {
    fn new(role: Option<Role>) -> Arc<Fake> {
        Fake::with_teams(role, 1)
    }
    fn with_teams(role: Option<Role>, teams: u8) -> Arc<Fake> {
        Arc::new(Fake {
            role,
            teams,
            held: Mutex::new(false),
            seq: Mutex::new(0),
            got: Mutex::new(Vec::new()),
        })
    }
    fn log(&self) -> Log {
        Log {
            seq: *self.seq.lock().unwrap(),
            tick: 41,
            hash: "00".repeat(32),
            applied: 0,
        }
    }
}

impl Source for Fake {
    fn state(&self) -> State {
        let Some(role) = self.role else {
            return State {
                refused: Some("the map does not carry the Firefight kit".into()),
                map: Some("lockout".into()),
                ..Default::default()
            };
        };
        State {
            running: true,
            test: false,
            map: Some("firefight-probe".into()),
            role: Some(role),
            team: Some(3),
            director: (role == Role::Host).then(|| Director {
                phase: "warmup".into(),
                phase_tick: Some(300),
                reserve: 4,
                teams: self.teams,
                pools: [7, 5][..usize::from(self.teams)].to_vec(),
                scores: [300, 150][..usize::from(self.teams)].to_vec(),
                cap: 8,
                players: 1,
                held: *self.held.lock().unwrap(),
                ..Default::default()
            }),
            log: Some(self.log()),
            errors: Vec::new(),
            refused: None,
            placements_unseen: 0,
            pods: None,
        }
    }
    fn command(&self, params: CommandParams) -> Result<CommandReply, Refused> {
        match self.role {
            None => {
                return Err(Refused::NotRunning(
                    "the map does not carry the Firefight kit".into(),
                ));
            }
            Some(Role::Client) => {
                return Err(Refused::NotHost(
                    "a distributed client keeps the host's log".into(),
                ));
            }
            Some(Role::Replica) => {
                return Err(Refused::NotHost(
                    "a replica applies the host's entries".into(),
                ));
            }
            Some(Role::Host) => {}
        }
        if let Some(Command::PlaceSquad { class, .. }) = &params.command
            && class != "elite"
        {
            return Err(Refused::Invalid(format!("the kit has no class {class}")));
        }
        self.got.lock().unwrap().push(params.clone());
        if let Some(d) = params.director {
            *self.held.lock().unwrap() = d == DirectorRun::Hold;
        }
        let appended = params.command.map(|_| {
            let mut seq = self.seq.lock().unwrap();
            *seq += 1;
            Appended {
                seq: *seq,
                tick: 41,
            }
        });
        Ok(CommandReply {
            appended,
            held: *self.held.lock().unwrap(),
            log: self.log(),
        })
    }
    fn kit(&self, m: &dyn GuestMemory) -> Result<Kit, Refused> {
        // Reads the guest, so a fault is the caller's UNAVAILABLE.
        let team = m
            .u8(0x1000)
            .map_err(|e| Refused::Unavailable(format!("{:#x} faulted", e.addr)))?;
        Ok(Kit {
            kit: true,
            slots: vec![Some(1), None],
            team: Some(team),
            zones: vec![KitZone {
                index: 1,
                point: [1.0, 2.0, 3.0],
                starts: 3,
            }],
            classes: [("elite".to_string(), 0)].into(),
            bases: vec![[4.0, 5.0, 6.0], [-4.0, -5.0, 6.0]],
            characters: 1,
            hunt_orders: vec![Some(0), None],
            hold_orders: vec![Some(1), None],
        })
    }
}

struct Harness {
    agent: Agent,
    conn: crate::ConnId,
    rx: std::sync::mpsc::Receiver<String>,
}

impl Harness {
    fn new(source: Arc<Fake>) -> Harness {
        let agent = Agent::new();
        install(&agent, source);
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

    fn ok(&self, game: &mut Game, m: &str, p: Value) -> Value {
        let r = self.call(game, m, p);
        assert!(r["error"].is_null(), "{m}: {r}");
        r["result"].clone()
    }

    fn err(&self, game: &mut Game, m: &str, p: Value) -> Value {
        let r = self.call(game, m, p);
        assert!(r["result"].is_null(), "{m}: {r}");
        r["error"].clone()
    }
}

fn game() -> Game {
    let mut g = FakeGuest::new();
    g.map(0x1000, 0x10).put_u8(0x1000, 3);
    Game(g)
}

#[test]
fn state_reports_the_director() {
    let (h, mut g) = (Harness::new(Fake::new(Some(Role::Host))), game());
    for params in [Value::Null, json!({})] {
        let s = h.ok(&mut g, method::STATE, params);
        assert_eq!(s["running"], true);
        assert_eq!(s["role"], "host");
        assert_eq!(s["team"], 3);
        assert_eq!(s["director"]["phase"], "warmup");
        assert_eq!(s["director"]["phase_tick"], 300);
        assert_eq!(s["director"]["reserve"], 4);
        assert_eq!(s["director"]["teams"], 1);
        assert_eq!(s["director"]["pools"], json!([7]));
        assert_eq!(s["director"]["scores"], json!([300]));
        assert_eq!(s["director"]["cap"], 8);
        assert_eq!(s["director"]["held"], false);
        assert_eq!(s["log"]["seq"], 0);
        assert_eq!(s["errors"], json!([]));
        assert!(s.get("refused").is_none(), "{s}");
    }
    let e = h.err(&mut g, method::STATE, json!({"verbose": true}));
    assert_eq!(e["code"], code::INVALID_PARAMS);
}

/// With two teams the state carries each team's pool and points.
#[test]
fn state_reports_each_teams_pool_and_score() {
    let (h, mut g) = (
        Harness::new(Fake::with_teams(Some(Role::Host), 2)),
        game(),
    );
    let s = h.ok(&mut g, method::STATE, Value::Null);
    assert_eq!(s["director"]["teams"], 2);
    assert_eq!(s["director"]["pools"], json!([7, 5]));
    assert_eq!(s["director"]["scores"], json!([300, 150]));
}

/// The two-team commands pass the method's checks to the runtime in the
/// host log's shape.
#[test]
fn command_takes_the_two_team_commands() {
    let fake = Fake::new(Some(Role::Host));
    let (h, mut g) = (Harness::new(Arc::clone(&fake)), game());
    for value in [
        json!({"target": {"slot": 2, "team": 1}}),
        json!({"team_lives": {"team": 0, "pool": 4}}),
        json!({"finish": {"outcome": {"team_won": {"team": 1}}}}),
        json!({"finish": {"outcome": "draw"}}),
    ] {
        h.ok(&mut g, method::COMMAND, json!({ "command": value }));
    }
    let got = fake.got.lock().unwrap();
    assert_eq!(got[0].command, Some(Command::Target { slot: 2, team: 1 }));
    assert_eq!(got[1].command, Some(Command::TeamLives { team: 0, pool: 4 }));
    assert_eq!(
        got[2].command,
        Some(Command::Finish {
            outcome: agent_proto::firefight::Outcome::TeamWon { team: 1 }
        })
    );
    assert_eq!(
        got[3].command,
        Some(Command::Finish {
            outcome: agent_proto::firefight::Outcome::Draw
        })
    );
}

#[test]
fn state_says_why_the_mode_is_not_running() {
    let (h, mut g) = (Harness::new(Fake::new(None)), game());
    let s = h.ok(&mut g, method::STATE, Value::Null);
    assert_eq!(s["running"], false);
    assert_eq!(s["refused"], "the map does not carry the Firefight kit");
    assert!(
        s.get("director").is_none() && s.get("role").is_none(),
        "{s}"
    );
    let e = h.err(
        &mut g,
        method::COMMAND,
        json!({"command": {"respawn": {"player": 0}}}),
    );
    assert_eq!(e["code"], code::UNAVAILABLE);
    assert_eq!(e["message"], "the map does not carry the Firefight kit");
}

#[test]
fn command_appends_on_the_host_and_holds_the_director() {
    let fake = Fake::new(Some(Role::Host));
    let (h, mut g) = (Harness::new(Arc::clone(&fake)), game());
    let r = h.ok(
        &mut g,
        method::COMMAND,
        json!({"command": {"erase_squad": {"slot": 0}}, "director": "hold"}),
    );
    assert_eq!(r["appended"], json!({"seq": 1, "tick": 41}));
    assert_eq!(r["held"], true);
    assert_eq!(r["log"]["seq"], 1);
    let r = h.ok(&mut g, method::COMMAND, json!({"director": "run"}));
    assert!(r.get("appended").is_none(), "{r}");
    assert_eq!(r["held"], false);
    let got = fake.got.lock().unwrap();
    assert_eq!(got[0].command, Some(Command::EraseSquad { slot: 0 }));
    assert_eq!(got[0].director, Some(DirectorRun::Hold));
    assert_eq!(got[1].command, None);
}

#[test]
fn command_params_are_checked_before_the_runtime_sees_them() {
    let fake = Fake::new(Some(Role::Host));
    let (h, mut g) = (Harness::new(Arc::clone(&fake)), game());
    for params in [
        Value::Null,
        json!({}),
        json!("respawn"),
        json!({"command": {"skip_wave": {}}}),
        json!({"command": {"respawn": {"player": 0}}, "force": true}),
        json!({"director": "pause"}),
    ] {
        let e = h.err(&mut g, method::COMMAND, params.clone());
        assert_eq!(e["code"], code::INVALID_PARAMS, "{params}");
    }
    // The runtime's own check: a class the kit lacks.
    let e = h.err(
        &mut g,
        method::COMMAND,
        json!({"command": {"place_squad": {"slot": 0, "class": "grunt", "count": 2, "zone": 0, "weapon_tier": 0}}}),
    );
    assert_eq!(e["code"], code::INVALID_PARAMS);
    assert!(fake.got.lock().unwrap().is_empty());
}

#[test]
fn command_is_refused_on_a_client_and_in_playback() {
    for (role, why) in [
        (Role::Client, "a distributed client keeps the host's log"),
        (Role::Replica, "a replica applies the host's entries"),
    ] {
        let fake = Fake::new(Some(role));
        let (h, mut g) = (Harness::new(Arc::clone(&fake)), game());
        let e = h.err(
            &mut g,
            method::COMMAND,
            json!({"command": {"finish": {"outcome": "won"}}}),
        );
        assert_eq!(e["code"], agent_proto::sim::REFUSED);
        assert_eq!(e["message"], why);
        assert!(fake.got.lock().unwrap().is_empty());
        // Reading still works.
        let s = h.ok(&mut g, method::STATE, Value::Null);
        assert!(s.get("director").is_none(), "{s}");
    }
}

#[test]
fn kit_reads_the_loaded_scenario() {
    let (h, mut g) = (Harness::new(Fake::new(Some(Role::Host))), game());
    let k = h.ok(&mut g, method::KIT, Value::Null);
    assert_eq!(k["kit"], true);
    assert_eq!(k["slots"], json!([1, null]));
    assert_eq!(k["team"], 3);
    assert_eq!(
        k["zones"][0],
        json!({"index": 1, "point": [1.0, 2.0, 3.0], "starts": 3})
    );
    assert_eq!(k["classes"], json!({"elite": 0}));
    assert_eq!(k["bases"], json!([[4.0, 5.0, 6.0], [-4.0, -5.0, 6.0]]));
    assert_eq!(k["hunt_orders"], json!([0, null]));
    let e = h.err(&mut g, method::KIT, json!({"zone": 1}));
    assert_eq!(e["code"], code::INVALID_PARAMS);
    // A fault reading the scenario.
    let mut empty = Game(FakeGuest::new());
    let e = h.err(&mut empty, method::KIT, Value::Null);
    assert_eq!(e["code"], code::UNAVAILABLE);
}

#[test]
fn session_methods_lists_them() {
    let (h, mut g) = (Harness::new(Fake::new(None)), game());
    let m = h.ok(&mut g, agent_proto::method::METHODS, Value::Null);
    for name in method::ALL {
        assert!(m.as_array().unwrap().contains(&json!(name)), "{name}: {m}");
    }
}
