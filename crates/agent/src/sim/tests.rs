//! `sim.*` over synthetic guest memory: a players array, an objects array,
//! a unit and its weapon, laid out by the ported datum functions.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_proto::sim::{SessionKind, SessionView, method};
use agent_proto::{Kind, Object, code};
use engine::GuestMemory;
use engine::testing::FakeGuest;
use engine::view::fake;
use engine::view::objects::{Body, Header};
use engine::view::players::Player;
use live_telemetry::layout as l;
use serde_json::{Value, json};

use super::bots::{Bot, FakeBots, NotLanded, PadSource};
use crate::{Agent, GameStatus, Reader};

const PLAYERS_HDR: u32 = 0x8000_0000;
const OBJECTS_HDR: u32 = 0x8010_0000;
const UNIT_BODY: u32 = 0x8020_0000;
const WEAPON_BODY: u32 = 0x8021_0000;

struct Game {
    g: FakeGuest,
    objects: Vec<Object>,
}

impl Reader for Game {
    fn status(&mut self) -> GameStatus {
        GameStatus {
            tick: Some(100),
            map: Some("lockout".into()),
            mode: Some("multiplayer".into()),
            bsp: Some(0),
        }
    }
    fn objects(&mut self) -> Result<Vec<Object>, String> {
        Ok(self.objects.clone())
    }
    fn with_guest(&mut self, f: &mut dyn FnMut(&mut dyn GuestMemory)) -> bool {
        f(&mut self.g);
        true
    }
}

/// Player 0 (the human, port 0) with a unit holding a weapon, and player
/// 1 (port 1, a local bot's) between lives.
fn game() -> Game {
    let mut g = FakeGuest::new();
    for (global, hdr) in [
        (engine::points::PLAYERS.base, PLAYERS_HDR),
        (engine::points::OBJECTS.base, OBJECTS_HDR),
    ] {
        g.map(global, 4);
        g.put_u32(global, hdr);
    }
    let players = fake::datums::<Player>(&mut g, PLAYERS_HDR, 16);
    let objects = fake::datums::<Header>(&mut g, OBJECTS_HDR, 64);
    let (unit_h, unit_hdr) = fake::push(&mut g, &objects);
    g.map(UNIT_BODY, 0x400);
    unit_hdr.set_body(&mut g, UNIT_BODY).unwrap();
    let unit = Body::at(UNIT_BODY);
    unit.set_health(&mut g, 1.0).unwrap();
    unit.set_shield(&mut g, 1.0).unwrap();
    g.put_f32(UNIT_BODY + l::BODY_VELOCITY, 3.0);
    let (weapon_h, weapon_hdr) = fake::push(&mut g, &objects);
    g.map(WEAPON_BODY, 0x400);
    weapon_hdr.set_body(&mut g, WEAPON_BODY).unwrap();
    weapon_hdr.set_kind(&mut g, 2).unwrap();
    g.put_u8(UNIT_BODY + l::UNIT_WEAPON_INDEX, 0);
    g.put_u32(UNIT_BODY + l::UNIT_WEAPONS, weapon_h.raw());
    for i in 1..4 {
        g.put_u32(UNIT_BODY + l::UNIT_WEAPONS + 4 * i, u32::MAX);
    }
    let (_, p0) = fake::push(&mut g, &players);
    p0.set_unit(&mut g, Some(unit_h)).unwrap();
    p0.set_local_user(&mut g, 0).unwrap();
    let (_, p1) = fake::push(&mut g, &players);
    p1.set_unit(&mut g, None).unwrap();
    p1.set_local_user(&mut g, 1).unwrap();
    p1.set_team(&mut g, 1).unwrap();
    let player = |i: u32, local: u8, team: i32| Object {
        id: format!("player.{i}"),
        kind: Kind::Player,
        team: Some(team),
        local_user: Some(local),
        ..Default::default()
    };
    Game {
        g,
        objects: vec![player(0, 0, 0), player(1, 1, 1)],
    }
}

struct Harness {
    agent: Agent,
    conn: crate::ConnId,
    rx: std::sync::mpsc::Receiver<String>,
    kind: Arc<Mutex<SessionKind>>,
    bots: FakeBots,
}

impl Harness {
    fn new() -> Harness {
        let agent = Agent::new();
        let kind = Arc::new(Mutex::new(SessionKind::MpHost));
        let k = Arc::clone(&kind);
        let bots = FakeBots::default();
        super::install(
            &agent,
            Box::new(bots.clone()),
            Some(Box::new(move |_| SessionView {
                kind: *k.lock().unwrap(),
                ..Default::default()
            })),
        );
        let (tx, rx) = std::sync::mpsc::sync_channel(64);
        let conn = agent.connect(tx);
        Harness {
            agent,
            conn,
            rx,
            kind,
            bots,
        }
    }

    fn send(&self, m: &str, p: Value) {
        self.agent.submit(
            self.conn,
            &json!({"jsonrpc": "2.0", "id": 1, "method": m, "params": p}).to_string(),
        );
    }

    fn reply(&self) -> Value {
        serde_json::from_str(&self.rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
    }

    /// Answers at a tick boundary.
    fn call(&self, game: &mut Game, m: &str, p: Value) -> Value {
        self.send(m, p);
        self.agent.on_tick(game);
        self.reply()
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

#[test]
fn a_host_write_lands_where_the_readers_read() {
    let (h, mut game) = (Harness::new(), game());
    let r = h.ok(
        &mut game,
        method::STATE_SET,
        json!({
            "player": "player.0", "pos": [10.0, -2.5, 3.0], "team": 1,
            "health": 0.25, "shield": 2.0,
            "grenades": {"frag": 4, "plasma": 0},
            "ammo": {"loaded": 12, "reserve": 48}
        }),
    );
    assert_eq!(r["player"], "player.0");
    assert_eq!(
        r["wrote"],
        json!(["team", "pos", "health", "shield", "grenades", "ammo"])
    );
    assert_eq!(r["session"]["kind"], "mp_host");
    let g = &game.g;
    let unit = Body::at(UNIT_BODY);
    assert_eq!(unit.position(g).unwrap(), [10.0, -2.5, 3.0]);
    assert_eq!(
        g.f32(UNIT_BODY + l::BODY_VELOCITY).unwrap(),
        0.0,
        "velocity zeroed"
    );
    assert_eq!(g.u16(UNIT_BODY + l::UNIT_TEAM).unwrap(), 1);
    let v = h.ok(&mut game, method::VIEW, json!({"roots": ["player"]}));
    let p0 = &v["player"]["0"];
    assert_eq!(p0["health"], 0.25);
    assert_eq!(p0["shield"], 2.0);
    assert_eq!(p0["grenades"], json!({"frag": 4, "plasma": 0}));
    assert_eq!(p0["ammo"], json!({"loaded": 12, "reserve": 48}));
    // The player record's team, as world.query reads it.
    let pl = engine::view::players::array(&game.g)
        .unwrap()
        .unwrap()
        .get_absolute(&game.g, 0)
        .unwrap()
        .unwrap();
    assert_eq!(pl.team(&game.g).unwrap(), 1);
}

#[test]
fn co_op_and_client_sessions_refuse_writes_and_write_nothing() {
    let (h, mut game) = (Harness::new(), game());
    for kind in [
        SessionKind::CoopHost,
        SessionKind::CoopGuest,
        SessionKind::MpClient,
        SessionKind::Unknown,
    ] {
        *h.kind.lock().unwrap() = kind;
        let e = h.err(
            &mut game,
            method::STATE_SET,
            json!({"player": "player.0", "health": 0.1}),
        );
        assert_eq!(e["code"], code::INVALID_REQUEST, "{kind:?}");
        assert_eq!(
            e["data"]["session"]["kind"],
            serde_json::to_value(kind).unwrap()
        );
        assert_eq!(e["data"]["session"]["writes"], false);
    }
    let e = h.err(
        &mut game,
        method::STATE_SET,
        json!({"player": "player.0", "health": 0.1}),
    );
    assert!(e["message"].as_str().unwrap().contains("unknown"), "{e}");
    *h.kind.lock().unwrap() = SessionKind::CoopHost;
    let e = h.err(
        &mut game,
        method::STATE_SET,
        json!({"player": "player.0", "health": 0.1}),
    );
    assert!(e["message"].as_str().unwrap().contains("sim hash"), "{e}");
    assert_eq!(Body::at(UNIT_BODY).health(&game.g).unwrap(), 1.0);
    let s = h.ok(&mut game, method::SESSION, Value::Null);
    assert_eq!(
        (s["kind"].clone(), s["writes"].clone()),
        (json!("coop_host"), json!(false))
    );
}

#[test]
fn no_probe_means_no_writes() {
    let agent = Agent::new();
    super::install(&agent, Box::new(NotLanded), None);
    let (tx, rx) = std::sync::mpsc::sync_channel(4);
    let c = agent.connect(tx);
    agent.submit(
        c,
        &json!({"jsonrpc": "2.0", "id": 1, "method": method::STATE_SET,
                "params": {"player": "player.0", "health": 0.5}})
        .to_string(),
    );
    let mut g = game();
    agent.on_tick(&mut g);
    let r: Value = serde_json::from_str(&rx.recv().unwrap()).unwrap();
    assert_eq!(r["error"]["data"]["session"]["kind"], "unknown");
}

#[test]
fn bad_values_and_missing_units_write_nothing() {
    let (h, mut game) = (Harness::new(), game());
    for bad in [
        json!({"player": "player.0", "health": 1.5}),
        json!({"player": "player.0", "shield": -1}),
        json!({"player": "player.0", "team": 99}),
        json!({"player": "player.0", "pos": [1, 2]}),
        json!({"player": "player.0"}),
        json!({"player": "player.0", "god": true}),
        json!({"player": "actor.e1230045", "health": 0.5}),
    ] {
        let e = h.err(&mut game, method::STATE_SET, bad.clone());
        assert_eq!(e["code"], code::INVALID_PARAMS, "{bad}: {e}");
    }
    // Player 1 is between lives: a unit write is refused, a team write isn't.
    let e = h.err(
        &mut game,
        method::STATE_SET,
        json!({"player": "player.1", "team": 0, "health": 0.5}),
    );
    assert_eq!(e["code"], code::GONE);
    let pl = engine::view::players::array(&game.g)
        .unwrap()
        .unwrap()
        .get_absolute(&game.g, 1)
        .unwrap()
        .unwrap();
    assert_eq!(pl.team(&game.g).unwrap(), 1, "nothing written");
    let r = h.ok(
        &mut game,
        method::STATE_SET,
        json!({"player": "player.1", "team": 0}),
    );
    assert_eq!(r["wrote"], json!(["team"]));
    let e = h.err(
        &mut game,
        method::STATE_SET,
        json!({"player": "player.9", "team": 0}),
    );
    assert_eq!(e["code"], code::GONE);
    assert_eq!(Body::at(UNIT_BODY).health(&game.g).unwrap(), 1.0);
}

#[test]
fn writes_wait_for_a_tick_boundary() {
    let (h, mut game) = (Harness::new(), game());
    h.send(
        method::STATE_SET,
        json!({"player": "player.0", "health": 0.5}),
    );
    // Never ticked: answered between slices, where nothing is written.
    h.agent.between_slices(&mut game, 0);
    let r = h.reply();
    assert_eq!(r["error"]["code"], code::UNAVAILABLE, "{r}");
    assert_eq!(Body::at(UNIT_BODY).health(&game.g).unwrap(), 1.0);
    // While ticking, a sim call between slices waits for the tick.
    h.agent.on_tick(&mut game);
    h.send(method::VIEW, json!({"roots": ["at"]}));
    h.agent.between_slices(&mut game, 0);
    assert!(h.rx.try_recv().is_err(), "held for the tick");
    h.agent.on_tick(&mut game);
    assert_eq!(h.reply()["result"]["at"]["boundary"], "tick");
}

#[test]
fn local_players_come_from_the_bots_and_take_scripted_pads() {
    let (h, mut game) = (Harness::new(), game());
    let r = h.ok(
        &mut game,
        method::PLAYER_ADD,
        json!({"kind": "local", "source": "script", "team": 1,
               "steps": [{"buttons": ["a"], "ticks": 2}]}),
    );
    assert_eq!(r["id"], "sim.1");
    assert_eq!(r["port"], 1);
    // Port 1's player is player.1 in the fake world.
    assert_eq!(r["player"], "player.1");
    assert!(r["team_pending"].is_string());
    let remote = match h.bots.log().bots.get(&Bot::Local { port: 1 }) {
        Some(PadSource::Script(r)) => r.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(remote.status().queued, 1);
    let r = h.ok(
        &mut game,
        method::PLAYER_PAD,
        json!({"id": "sim.1", "steps": [{"left": [0, 1], "ticks": 30}, {"buttons": ["x"], "ticks": 1}]}),
    );
    assert_eq!(
        (r["queued"].clone(), r["via"].clone()),
        (json!(2), json!("script"))
    );
    assert_eq!(
        remote.status().queued,
        2,
        "the bot's own queue was replaced"
    );
    // A write may name it by its sim id.
    let r = h.ok(
        &mut game,
        method::STATE_SET,
        json!({"player": "sim.1", "team": 0}),
    );
    assert_eq!(r["player"], "player.1");
    let l = h.ok(&mut game, method::PLAYER_LIST, Value::Null);
    assert_eq!(l["players"][0]["source"], "script");
    assert!(l["players"][0].get("brain").is_none());
    let r = h.ok(&mut game, method::PLAYER_REMOVE, json!({"id": "sim.1"}));
    assert_eq!(r["removed"], json!(["sim.1"]));
    assert_eq!(h.bots.log().removed, [Bot::Local { port: 1 }]);
    let e = h.err(
        &mut game,
        method::PLAYER_PAD,
        json!({"id": "sim.1", "steps": []}),
    );
    assert_eq!(e["code"], code::GONE);
}

#[test]
fn phantoms_need_a_server_issued_identity() {
    let (h, mut game) = (Harness::new(), game());
    let e = h.err(
        &mut game,
        method::PLAYER_ADD,
        json!({"kind": "phantom", "source": "brain"}),
    );
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("embedded Live server")
    );
    let e = h.err(
        &mut game,
        method::PLAYER_ADD,
        json!({"kind": "phantom", "source": "brain", "port": 2,
               "identity": {"xuid": "9", "gamertag": "Clanker 1"}}),
    );
    assert_eq!(e["code"], code::INVALID_PARAMS);
    let r = h.ok(
        &mut game,
        method::PLAYER_ADD,
        json!({"kind": "phantom", "source": "brain", "team": 0,
               "identity": {"xuid": "0009000000000001", "gamertag": "Clanker 1"}}),
    );
    assert_eq!(r["kind"], "phantom");
    assert!(r["player"].is_null(), "no player until the backend says");
    assert_eq!(h.bots.log().phantoms[&1].1, Some(0));
    h.bots.log().phantom_players.insert(1, 0);
    let l = h.ok(&mut game, method::PLAYER_LIST, Value::Null);
    assert_eq!(l["players"][0]["player"], "player.0");
    let e = h.err(
        &mut game,
        method::PLAYER_PAD,
        json!({"id": "sim.1", "steps": [{"buttons": ["a"]}]}),
    );
    assert_eq!(e["code"], code::INVALID_REQUEST);
    let r = h.ok(&mut game, method::PLAYER_REMOVE, json!({"all": true}));
    assert_eq!(r["removed"], json!(["sim.1"]));
}

#[test]
fn until_bots_land_adds_are_refused_saying_why() {
    let agent = Agent::new();
    let sim = super::install(&agent, Box::new(NotLanded), None);
    let (tx, rx) = std::sync::mpsc::sync_channel(4);
    let c = agent.connect(tx);
    let mut g = game();
    let mut call = |p: Value| -> Value {
        agent.submit(
            c,
            &json!({"jsonrpc": "2.0", "id": 1, "method": method::PLAYER_ADD, "params": p})
                .to_string(),
        );
        agent.on_tick(&mut g);
        serde_json::from_str(&rx.recv().unwrap()).unwrap()
    };
    let r = call(json!({"kind": "local", "source": "brain"}));
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap()
            .contains("bots::local"),
        "{r}"
    );
    sim.set_bots(Box::new(FakeBots::default()));
    let r = call(json!({"kind": "local", "source": "brain"}));
    assert_eq!(r["result"]["id"], "sim.1");
}

#[test]
fn expect_judges_the_view_at_one_tick() {
    let (h, mut game) = (Harness::new(), game());
    let v = h.ok(
        &mut game,
        method::EXPECT,
        json!({"checks": [
            "/at/map == \"lockout\"",
            "/count/player >= 2",
            "/player/0/health near {\"to\": 1, \"within\": 0.01}",
            {"pointer": "/player/1/team", "op": "==", "want": 0},
            "/session/kind == \"mp_host\"",
            "/sim/count == 0"
        ]}),
    );
    assert_eq!(v["pass"], false);
    assert_eq!(v["failed"], 1);
    assert_eq!(v["expects"][3]["got"], 1);
    assert_eq!(v["expects"][3]["expect"], "/player/1/team ==");
    assert_eq!(v["at"]["tick"], 100);
    let e = h.err(
        &mut game,
        method::EXPECT,
        json!({"checks": ["/nope/x exists"]}),
    );
    assert_eq!(e["code"], code::INVALID_PARAMS);
    let e = h.err(
        &mut game,
        method::EXPECT,
        json!({"checks": ["/at/tick ~ 1"]}),
    );
    assert_eq!(e["code"], code::INVALID_PARAMS);
}

#[test]
fn the_view_reads_only_the_roots_asked_for() {
    let (h, mut game) = (Harness::new(), game());
    let v = h.ok(&mut game, method::VIEW, Value::Null);
    for r in agent_proto::sim::DEFAULT_ROOTS {
        assert!(v.get(*r).is_some(), "{r}: {v}");
    }
    assert!(v.get("object").is_none());
    // No host methods registered: ui and input are null, not errors.
    assert!(v["ui"].is_null() && v["input"].is_null());
    assert_eq!(v["count"]["team"], json!({"0": 1, "1": 1}));
    let v = h.ok(&mut game, method::VIEW, json!({"roots": ["object"]}));
    assert!(v["object"]["player.1"].is_object());
    assert!(v.get("player").is_none());
    let e = h.err(&mut game, method::VIEW, json!({"roots": ["moon"]}));
    assert_eq!(e["code"], code::INVALID_PARAMS);
}

#[test]
fn session_methods_lists_them() {
    let (h, mut game) = (Harness::new(), game());
    let m = h.ok(&mut game, agent_proto::method::METHODS, Value::Null);
    for name in method::ALL {
        assert!(m.as_array().unwrap().contains(&json!(name)), "{name}");
    }
}

#[test]
fn temperament_sim_player_list_and_view_expose_current_brain_state() {
    use agent_proto::sim::{BotBrain, ClankerView, PlayerKind};

    let (h, mut game) = (Harness::new(), game());
    let brain: BotBrain = serde_json::from_value(json!({
        "skill": "legendary",
        "personality": {
            "aggression": 0.7, "caution": 0.6, "teamwork": 0.75, "objective_focus": 0.8,
            "composure": 0.9, "patience": 0.8, "showboating": 0.25, "weapon_loyalty": 0.55,
            "chattiness": 0.5, "favorite_weapon": "blade"
        },
        "mood": {"confidence": 0.125, "tilt": 0.375, "fear": 0.25, "streak": 2,
                 "revenge_target": "player.0"}
    }))
    .unwrap();
    {
        let mut log = h.bots.log();
        log.brains.insert(Bot::Local { port: 1 }, brain.clone());
        log.clankers.push(ClankerView {
            name: "Local Clanker".into(),
            kind: PlayerKind::Local,
            port: Some(1),
            player: None,
            brain: brain.clone(),
        });
    }
    let added = h.ok(
        &mut game,
        method::PLAYER_ADD,
        json!({"kind": "local", "source": "brain"}),
    );
    assert_eq!(added["brain"], serde_json::to_value(&brain).unwrap());
    let mut updated = brain;
    updated.mood.tilt = 0.625;
    {
        let mut log = h.bots.log();
        log.brains.insert(Bot::Local { port: 1 }, updated.clone());
        log.clankers[0].brain = updated.clone();
    }
    let updated = serde_json::to_value(updated).unwrap();
    let listed = h.ok(&mut game, method::PLAYER_LIST, Value::Null);
    assert_eq!(listed["players"][0]["brain"], updated);
    assert_eq!(listed["clankers"][0]["brain"], updated);
    assert_eq!(listed["clankers"][0]["player"], "player.1");
    let view = h.ok(&mut game, method::VIEW, json!({"roots": ["sim"]}));
    assert_eq!(view["sim"]["1"]["brain"], updated);
}
