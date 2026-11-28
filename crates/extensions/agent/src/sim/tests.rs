//! `sim.*` over synthetic guest memory: a players array, an objects array,
//! a unit and its weapon, laid out by the ported datum functions.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_proto::sim::{REFUSED, SessionKind, SessionView, method};
use agent_proto::{Kind, Object, code};
use guest_layout::GuestMemory;
use guest_layout::testing::FakeGuest;
use guest_layout::view::fake;
use guest_layout::view::objects::{Body, Header};
use guest_layout::view::players::Player;
use live_telemetry::layout as l;
use serde_json::{Value, json};

use super::bots::{Bot, FakeBots, NotLanded, PadSource};
use crate::{Agent, GameStatus, Reader};

mod authorization;

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
/// 1 between lives.
fn game() -> Game {
    let mut g = FakeGuest::new();
    use guest_layout::global::{CACHE_MAP_NAME, SCENARIO, SIM_GLOBALS, SQUAD_SESSION};
    g.put(CACHE_MAP_NAME.base, b"lockout\0");
    g.put_u32(SCENARIO.base, 0x8110_0000);
    g.put_u32(SIM_GLOBALS.base, 0x8120_0000);
    g.put_u32(SQUAD_SESSION.base, 0x8130_0000);
    for (global, hdr) in [
        (guest_layout::points::PLAYERS.base, PLAYERS_HDR),
        (guest_layout::points::OBJECTS.base, OBJECTS_HDR),
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
                peers: Some(1),
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
            "player": "player.0", "team": 1,
            "health": 0.25, "shield": 2.0,
            "grenades": {"frag": 4, "plasma": 0},
            "ammo": {"loaded": 12, "reserve": 48}
        }),
    );
    assert_eq!(r["player"], "player.0");
    assert_eq!(
        r["wrote"],
        json!(["team", "health", "shield", "grenades", "ammo"])
    );
    assert_eq!(r["session"]["kind"], "mp_host");
    let g = &game.g;
    assert_eq!(g.u16(UNIT_BODY + l::UNIT_TEAM).unwrap(), 1);
    let v = h.ok(&mut game, method::VIEW, json!({"roots": ["player"]}));
    let p0 = &v["player"]["0"];
    assert_eq!(p0["health"], 0.25);
    assert_eq!(p0["shield"], 2.0);
    assert_eq!(p0["grenades"], json!({"frag": 4, "plasma": 0}));
    assert_eq!(p0["ammo"], json!({"loaded": 12, "reserve": 48}));
    // The player record's team, as world.query reads it.
    let pl = guest_layout::view::players::array(&game.g)
        .unwrap()
        .unwrap()
        .get_absolute(&game.g, 0)
        .unwrap()
        .unwrap();
    assert_eq!(pl.team(&game.g).unwrap(), 1);
}

/// The player record's lives: a count, null when unlimited, and there
/// between lives too.
#[test]
fn the_view_reports_each_players_lives() {
    let (h, mut game) = (Harness::new(), game());
    let players = guest_layout::view::players::array(&game.g).unwrap().unwrap();
    for (i, lives) in [(0, 2u16), (1, 0xffff)] {
        let p = players.get_absolute(&game.g, i).unwrap().unwrap();
        game.g.put_u16(p.addr + super::state::PLAYER_LIVES, lives);
    }
    let v = h.ok(&mut game, method::VIEW, json!({"roots": ["player"]}));
    assert_eq!(v["player"]["0"]["lives"], 2);
    assert!(v["player"]["1"]["lives"].is_null(), "{v}");
    assert!(v["player"]["1"].get("health").is_none(), "player 1 is between lives: {v}");
    let p = players.get_absolute(&game.g, 1).unwrap().unwrap();
    game.g.put_u16(p.addr + super::state::PLAYER_LIVES, 0);
    let v = h.ok(&mut game, method::VIEW, json!({"roots": ["player"]}));
    assert_eq!(v["player"]["1"]["lives"], 0);
}

/// The score is the game engine's stat row `+0x00`; null without a game
/// engine.
#[test]
fn the_view_reports_each_players_score() {
    const ENGINE: u32 = 0x8140_0000;
    let (h, mut game) = (Harness::new(), game());
    let v = h.ok(&mut game, method::VIEW, json!({"roots": ["player"]}));
    assert!(v["player"]["0"]["score"].is_null(), "no game engine: {v}");
    game.g.map(guest_layout::points::GAME_ENGINE.base, 4);
    game.g.put_u32(guest_layout::points::GAME_ENGINE.base, ENGINE);
    game.g.map(ENGINE, 0x1000);
    let row = |i: u32| ENGINE + super::state::STAT_ROWS + super::state::STAT_ROW_SIZE * i;
    game.g.put_u16(row(0), 150).put_u16(row(1), (-5i16) as u16);
    let v = h.ok(&mut game, method::VIEW, json!({"roots": ["player"]}));
    assert_eq!(v["player"]["0"]["score"], 150);
    assert_eq!(v["player"]["1"]["score"], -5);
    // An engine pointer of zero (the menu) is no game engine.
    game.g.put_u32(guest_layout::points::GAME_ENGINE.base, 0);
    let v = h.ok(&mut game, method::VIEW, json!({"roots": ["player"]}));
    assert!(v["player"]["0"]["score"].is_null(), "{v}");
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
    let pl = guest_layout::view::players::array(&game.g)
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
fn phantoms_take_scripted_pads_and_resolve_by_sim_id() {
    let (h, mut game) = (Harness::new(), game());
    let r = h.ok(
        &mut game,
        method::PLAYER_ADD,
        json!({"kind": "phantom", "source": "script", "team": 1,
               "identity": {"xuid": "9", "gamertag": "Clanker"},
               "steps": [{"buttons": ["a"], "ticks": 2}]}),
    );
    assert_eq!(r["id"], "sim.1");
    assert_eq!(r["team"], 1);
    assert!(r["player"].is_null());
    h.bots.log().phantom_players.insert(1, 1);
    let remote = match h.bots.log().bots.get(&Bot { handle: 1 }) {
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
    assert_eq!(h.bots.log().removed, [Bot { handle: 1 }]);
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
fn a_missing_bots_backend_refuses_adds_saying_why() {
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
    let add = json!({"kind": "phantom", "source": "brain",
                     "identity": {"xuid": "9", "gamertag": "Clanker"}});
    let r = call(add.clone());
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap()
            .contains("bots::phantom"),
        "{r}"
    );
    sim.set_bots(Box::new(FakeBots::default()));
    let r = call(add);
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
        assert_eq!(m.as_array().unwrap().contains(&json!(name)), !method::EDITS.contains(name), "{name}");
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
        log.brains.insert(Bot { handle: 1 }, brain.clone());
        log.clankers.push(ClankerView {
            name: "Clanker".into(),
            kind: PlayerKind::Phantom,
            player: Some("player.1".into()),
            brain: brain.clone(),
            aim: None,
        });
        let mut remote = log.clankers[0].clone();
        remote.name = "Remote Clanker".into();
        remote.player = Some("player.2".into());
        remote.aim = Some(agent_proto::sim::PhantomAim {
            tick: 100,
            stick: [0.5, 0.],
            arrived_stick: [0., 0.],
            trigger: 1.,
            arrived_trigger: 0.,
            action_deg: Some([90., 0.]),
            fire_primary: false,
            assist_strength: [0.75, 0.],
            unit_forward: [0., 1., 0.],
            assist_player: Some("player.0".into()),
        });
        log.clankers.push(remote);
    }
    let added = h.ok(
        &mut game,
        method::PLAYER_ADD,
        json!({"kind": "phantom", "source": "brain",
               "identity": {"xuid": "9", "gamertag": "Clanker"}}),
    );
    assert_eq!(added["brain"], serde_json::to_value(&brain).unwrap());
    let mut updated = brain;
    updated.mood.tilt = 0.625;
    {
        let mut log = h.bots.log();
        log.brains.insert(Bot { handle: 1 }, updated.clone());
        log.clankers[0].brain = updated.clone();
    }
    let updated = serde_json::to_value(updated).unwrap();
    let listed = h.ok(&mut game, method::PLAYER_LIST, Value::Null);
    assert_eq!(listed["players"][0]["brain"], updated);
    assert_eq!(listed["clankers"][0]["brain"], updated);
    assert_eq!(listed["clankers"][0]["player"], "player.1");
    assert!(listed["clankers"][0].get("aim").is_none());
    let aim = &listed["clankers"][1]["aim"];
    assert_eq!(aim["tick"], 100);
    assert_eq!(aim["action_deg"], json!([90.0, 0.0]));
    assert_eq!(aim["assist_player"], "player.0");
    let view = h.ok(&mut game, method::VIEW, json!({"roots": ["sim"]}));
    assert_eq!(view["sim"]["1"]["brain"], updated);
}

/// The tags `sim.give` and `sim.spawn` find.
struct FakeTags;

const WEAP_TAG: u32 = 0xe100_0010;
const VEHI_TAG: u32 = 0xe200_0020;
const SND_TAG: u32 = 0xe400_0040;

impl super::calls::Tags for FakeTags {
    fn string_id(&mut self, name: &str) -> Option<u32> {
        (name == "idle").then_some(0x4d01)
    }

    /// As a map names them: a weapon's name is also its model's (`hlmt`),
    /// a vehicle's its render model's (`mode`).
    fn find(&mut self, tag: &str, among: &[&str]) -> Result<super::calls::TagRef, String> {
        const BR: &str = "objects\\weapons\\rifle\\battle_rifle\\battle_rifle";
        const HOG: &str = "objects\\vehicles\\warthog\\warthog";
        let rows = [
            (WEAP_TAG, "weap", BR),
            (0xe100_0011, "hlmt", BR),
            (VEHI_TAG, "vehi", HOG),
            (0xe200_0021, "mode", HOG),
            (0xe300_0030, "shad", "shaders\\metal"),
            (SND_TAG, "snd!", "sound\\pogo\\spring"),
        ];
        let (group, name) = match tag.split_once(':') {
            Some((g, n)) => (Some(g), n),
            None => (None, tag),
        };
        let found: Vec<_> = rows
            .iter()
            .filter(|(d, g, n)| {
                (*n == name || name == format!("{d:#010x}"))
                    && match group {
                        Some(want) => *g == want,
                        None => among.is_empty() || among.contains(g),
                    }
            })
            .collect();
        match found.as_slice() {
            [(datum, group, name)] => Ok(super::calls::TagRef {
                datum: *datum,
                group: (*group).into(),
                name: Some((*name).into()),
            }),
            [] => Err(format!("no tag named {tag:?}")),
            many => Err(format!("{tag:?} names {} tags: give the group", many.len())),
        }
    }
}

/// A harness whose sim has the calls queue (armed, as the feature's build
/// arms it) and the fake tags.
fn with_calls() -> (Harness, super::calls::Shared) {
    let (h, q, _) = with_calls_and_sim();
    (h, q)
}

fn with_calls_and_sim() -> (Harness, super::calls::Shared, super::SimHandle) {
    let h = Harness::new();
    let agent = &h.agent;
    // The sim installed by Harness::new isn't reachable: install again
    // under the same names (later registrations replace earlier ones).
    let sim = super::install(
        agent,
        Box::new(h.bots.clone()),
        Some(Box::new({
            let k = Arc::clone(&h.kind);
            move |_| SessionView {
                kind: *k.lock().unwrap(),
                peers: Some(1),
                ..Default::default()
            }
        })),
    );
    let q = super::calls::shared();
    super::calls::lock(&q).arm();
    sim.set_calls(Arc::clone(&q));
    sim.install_world_edits(agent);
    sim.set_tags(Box::new(FakeTags));
    (h, q, sim)
}

/// The feature's part at the next tick: takes the queued calls and
/// answers each with `answer`.
fn the_game_makes(
    q: &super::calls::Shared,
    mut answer: impl FnMut(&super::calls::Call) -> super::calls::Outcome,
) -> Vec<super::calls::Call> {
    let work = super::calls::lock(q).drain();
    let mut calls = Vec::new();
    for (t, c) in work {
        let out = answer(&c);
        super::calls::lock(q).finish(t, out);
        calls.push(c);
    }
    calls
}

impl Harness {
    /// Sends `m`, ticks once: the answer is deferred (nothing yet).
    fn deferred(&self, game: &mut Game, m: &str, p: Value) {
        self.send(m, p);
        self.agent.on_tick(game);
        assert!(self.rx.try_recv().is_err(), "{m}: answered at once");
    }
}

#[test]
fn a_teleport_goes_through_the_games_placement_and_answers_after_the_tick() {
    let (h, q) = with_calls();
    let mut game = game();
    let handle = super::state::player_handle(&game.g, 0).unwrap();
    h.deferred(
        &mut game,
        method::STATE_SET,
        json!({"player": "player.0", "pos": [10.0, -2.5, 3.0], "yaw": 90, "health": 0.5}),
    );
    // The health is written now; the move waits for the game.
    assert_eq!(Body::at(UNIT_BODY).health(&game.g).unwrap(), 0.5);
    assert_eq!(Body::at(UNIT_BODY).position(&game.g).unwrap(), [0.0; 3]);
    let calls = the_game_makes(&q, |_| Ok(json!({"pos": [10.0, -2.5, 3.1]})));
    let super::calls::Call::Teleport {
        player,
        pos,
        facing,
    } = calls[0].clone()
    else {
        panic!("{calls:?}");
    };
    assert_eq!((player, pos), (handle, Some([10.0, -2.5, 3.0])));
    let f = facing.unwrap();
    assert!(
        f[0].abs() < 1e-6 && (f[1] - 1.0).abs() < 1e-6 && f[2] == 0.0,
        "{f:?}"
    );
    h.agent.on_tick(&mut game);
    let r = h.reply();
    assert!(r["error"].is_null(), "{r}");
    let r = &r["result"];
    assert_eq!(r["wrote"], json!(["health", "pos", "facing"]));
    assert_eq!(r["teleport"]["pos"], json!([10.0, -2.5, 3.1]));
    assert_eq!(r["at"]["boundary"], "tick");
}

#[test]
fn a_teleport_the_game_refuses_or_never_makes_is_an_error() {
    let (h, q) = with_calls();
    let mut game = game();
    h.deferred(
        &mut game,
        method::STATE_SET,
        json!({"player": "player.0", "facing": [0, 1, 0]}),
    );
    the_game_makes(&q, |_| Err(super::unavailable("no room".into())));
    h.agent.on_tick(&mut game);
    let r = h.reply();
    assert_eq!(r["error"]["code"], code::UNAVAILABLE, "{r}");
    assert!(r["error"]["message"].as_str().unwrap().contains("no room"));
    // Queued, never made (the feature didn't run): a timeout, not a hang.
    h.deferred(
        &mut game,
        method::STATE_SET,
        json!({"player": "player.0", "pos": [1, 1, 1]}),
    );
    for _ in 0..super::TELEPORT_TICKS {
        h.agent.on_tick(&mut game);
    }
    let r = h.reply();
    assert_eq!(r["error"]["code"], code::TIMEOUT, "{r}");
}

#[test]
fn a_teleport_needs_the_calls_and_a_unit() {
    let (h, mut game) = (Harness::new(), game());
    let e = h.err(
        &mut game,
        method::STATE_SET,
        json!({"player": "player.0", "pos": [1, 2, 3]}),
    );
    assert_eq!(e["code"], code::UNAVAILABLE, "{e}");
    assert!(
        e["message"].as_str().unwrap().contains("agent-calls"),
        "{e}"
    );
    let (h, q) = with_calls();
    let e = h.err(
        &mut game,
        method::STATE_SET,
        json!({"player": "player.1", "pos": [1, 2, 3]}),
    );
    assert_eq!(e["code"], code::GONE, "between lives: {e}");
    for bad in [
        json!({"player": "player.0", "pitch": 10}),
        json!({"player": "player.0", "yaw": 0, "pitch": 95}),
        json!({"player": "player.0", "yaw": 0, "facing": [1, 0, 0]}),
        json!({"player": "player.0", "facing": [0, 0, 0]}),
    ] {
        let e = h.err(&mut game, method::STATE_SET, bad.clone());
        assert_eq!(e["code"], code::INVALID_PARAMS, "{bad}: {e}");
    }
    assert!(!super::calls::lock(&q).has_pending(), "nothing queued");
}

#[test]
fn give_makes_a_weapon_by_name_and_fills_its_magazine() {
    let (h, q) = with_calls();
    let mut game = game();
    h.deferred(
        &mut game,
        method::GIVE,
        json!({
            "player": "player.0",
            "weapon": "objects\\weapons\\rifle\\battle_rifle\\battle_rifle",
            "ammo": {"loaded": 36, "reserve": 108}
        }),
    );
    // The game makes it: the new weapon is the one the unit now holds.
    let weapon = game.g.u32(UNIT_BODY + l::UNIT_WEAPONS).unwrap();
    let unit_h = guest_layout::view::players::array(&game.g)
        .unwrap()
        .unwrap()
        .get_absolute(&game.g, 0)
        .unwrap()
        .unwrap()
        .unit(&game.g)
        .unwrap()
        .unwrap()
        .raw();
    let calls = the_game_makes(&q, |c| {
        assert_eq!(
            *c,
            super::calls::Call::Give {
                unit: unit_h,
                weapon: WEAP_TAG,
                mode: super::calls::addr::MODE_REPLACE
            }
        );
        Ok(json!({"object": format!("{weapon:08x}"), "mode": 2}))
    });
    assert_eq!(calls.len(), 1);
    h.agent.on_tick(&mut game);
    let r = h.reply();
    assert!(r["error"].is_null(), "{r}");
    let r = &r["result"];
    assert_eq!(r["weapon"]["tag"], "0xe1000010");
    assert_eq!(r["weapon"]["group"], "weap");
    assert_eq!(r["object"], format!("{weapon:08x}"));
    assert_eq!(r["held"], true);
    assert_eq!(
        game.g.u16(WEAPON_BODY + l::WEAPON_MAGAZINE_LOADED).unwrap(),
        36
    );
    assert_eq!(
        game.g
            .u16(WEAPON_BODY + l::WEAPON_MAGAZINE_RESERVE)
            .unwrap(),
        108
    );
    // keep: the game's mode 1.
    h.deferred(
        &mut game,
        method::GIVE,
        json!({"player": "player.0", "weapon": "0xe1000010", "keep": true}),
    );
    let calls = the_game_makes(&q, |_| Ok(json!({"object": "00aa0009", "mode": 1})));
    assert!(matches!(
        calls[0],
        super::calls::Call::Give {
            mode: super::calls::addr::MODE_ADD,
            ..
        }
    ));
    h.agent.on_tick(&mut game);
    assert_eq!(h.reply()["result"]["held"], false);
}

#[test]
fn give_refuses_what_is_not_a_weapon_and_players_between_lives() {
    let (h, q) = with_calls();
    let mut game = game();
    for (p, want) in [
        (
            json!({"player": "player.0", "weapon": "shaders\\metal"}),
            code::INVALID_PARAMS,
        ),
        (
            json!({"player": "player.0", "weapon": "nothing"}),
            code::INVALID_PARAMS,
        ),
        (
            json!({"player": "player.0", "weapon": "0xe1000010", "ammo": {"loaded": -1}}),
            code::INVALID_PARAMS,
        ),
        (
            json!({"player": "player.1", "weapon": "0xe1000010"}),
            code::GONE,
        ),
    ] {
        let e = h.err(&mut game, method::GIVE, p.clone());
        assert_eq!(e["code"], want, "{p}: {e}");
    }
    *h.kind.lock().unwrap() = SessionKind::CoopHost;
    let e = h.err(
        &mut game,
        method::GIVE,
        json!({"player": "player.0", "weapon": "0xe1000010"}),
    );
    assert_eq!(e["code"], code::INVALID_REQUEST);
    assert!(!super::calls::lock(&q).has_pending());
}

#[test]
fn spawn_makes_an_object_tag_at_a_point() {
    let (h, q) = with_calls();
    let mut game = game();
    h.deferred(
        &mut game,
        method::SPAWN,
        json!({"tag": "objects\\vehicles\\warthog\\warthog", "pos": [1, 2, 3], "yaw": 180}),
    );
    let calls = the_game_makes(&q, |_| {
        Ok(json!({"object": "00420003", "pos": [1.0, 2.0, 3.0]}))
    });
    let super::calls::Call::Spawn { tag, pos, facing, mp_scenario: None } = calls[0].clone() else {
        panic!("{calls:?}");
    };
    assert_eq!((tag, pos), (VEHI_TAG, [1.0, 2.0, 3.0]));
    assert!((facing.unwrap()[0] + 1.0).abs() < 1e-6);
    h.agent.on_tick(&mut game);
    let r = h.reply()["result"].clone();
    assert_eq!(r["object"], "00420003");
    assert_eq!(r["tag"]["group"], "vehi");
    let e = h.err(
        &mut game,
        method::SPAWN,
        json!({"tag": "shaders\\metal", "pos": [0, 0, 0]}),
    );
    assert_eq!(e["code"], code::INVALID_PARAMS, "{e}");
}

#[test]
fn give_and_spawn_resolve_a_shared_name_among_their_groups() {
    let (h, q) = with_calls();
    let mut game = game();
    // The battle rifle's name is also its model's: give looks among weap.
    h.deferred(
        &mut game,
        method::GIVE,
        json!({"player": "player.0", "weapon": "objects\\weapons\\rifle\\battle_rifle\\battle_rifle"}),
    );
    let calls = the_game_makes(&q, |_| Err(super::unavailable("not this time".into())));
    assert!(
        matches!(
            calls[0],
            super::calls::Call::Give {
                weapon: WEAP_TAG,
                ..
            }
        ),
        "{calls:?}"
    );
    h.agent.on_tick(&mut game);
    h.reply();
    // The warthog's is also its render model's: spawn looks among objects.
    h.deferred(
        &mut game,
        method::SPAWN,
        json!({"tag": "objects\\vehicles\\warthog\\warthog", "pos": [0, 0, 0]}),
    );
    let calls = the_game_makes(&q, |_| Err(super::unavailable("not this time".into())));
    assert!(
        matches!(calls[0], super::calls::Call::Spawn { tag: VEHI_TAG, .. }),
        "{calls:?}"
    );
    h.agent.on_tick(&mut game);
    h.reply();
    // group:name is taken as given: the model is no weapon.
    let e = h.err(
        &mut game,
        method::GIVE,
        json!({"player": "player.0", "weapon": "hlmt:objects\\weapons\\rifle\\battle_rifle\\battle_rifle"}),
    );
    assert_eq!(e["code"], code::INVALID_PARAMS, "{e}");
    assert!(
        e["message"].as_str().unwrap().contains("not a weapon"),
        "{e}"
    );
    h.deferred(
        &mut game,
        method::SPAWN,
        json!({"tag": "vehi:objects\\vehicles\\warthog\\warthog", "pos": [0, 0, 0]}),
    );
    let calls = the_game_makes(&q, |_| Err(super::unavailable("not this time".into())));
    assert!(matches!(
        calls[0],
        super::calls::Call::Spawn { tag: VEHI_TAG, .. }
    ));
}

#[test]
fn a_local_bot_is_refused_saying_why() {
    let (h, mut game) = (Harness::new(), game());
    let e = h.err(
        &mut game,
        method::PLAYER_ADD,
        json!({"kind": "local", "source": "script"}),
    );
    assert_eq!(e["code"], code::INVALID_PARAMS);
    assert!(e["message"].as_str().unwrap().contains("controller seats are for people"));
    assert!(h.bots.log().bots.is_empty());
}

#[test]
fn live_capture_matches_sim_objects_at_the_same_boundary() {
    use agent_proto::live::{Layers, World};
    use agent_proto::sim::objects::{ObjectsReply, handle};
    use guest_layout::object::header;
    use guest_layout::{Datum, Object as Layout, ObjectHeader, ObjectType};

    let (h, mut game) = (Harness::new(), game());
    let g = &mut game.g;
    let objects = guest_layout::view::objects::array(g).unwrap().unwrap();
    let unit_id = objects.array.handle_of(g, 0).unwrap().unwrap();
    let unit_header = objects.get(g, unit_id).unwrap().unwrap();
    ObjectHeader::at(unit_header.addr)
        .set(g, ObjectHeader::FLAGS, header::ACTIVE | header::AWAKE)
        .unwrap();
    let unit = Layout::at(UNIT_BODY);
    unit.set(g, Layout::DEFINITION, Datum(0x9000_0017)).unwrap();
    unit.set(g, Layout::PARENT, Datum(u32::MAX)).unwrap();
    unit.set(g, Layout::POSITION, [1.5, 2.0, -3.0]).unwrap();
    unit.set(g, Layout::FORWARD, [0.0, 1.0, 0.0]).unwrap();
    unit.set(g, Layout::UP, [0.0, 0.0, 1.0]).unwrap();
    unit.set(g, Layout::SCALE, 1.25).unwrap();
    unit.set(g, Layout::PLACEMENT_INDEX, 5).unwrap();
    unit.as_unit()
        .set(g, guest_layout::Unit::AIM, [0.0, -1.0, 0.0])
        .unwrap();
    let gun = Layout::at(WEAPON_BODY);
    gun.set(g, Layout::PARENT, Datum(unit_id.raw())).unwrap();
    gun.set(g, Layout::PLACEMENT_INDEX, -1).unwrap();
    let players = guest_layout::view::players::array(g).unwrap().unwrap();
    let player = players.get_absolute(g, 0).unwrap().unwrap();
    let mut name = [0; guest_layout::player::NAME_UNITS];
    for (dst, unit) in name.iter_mut().zip("Nyx ∆".encode_utf16()) {
        *dst = unit;
    }
    guest_layout::Player::at(player.addr)
        .set(g, guest_layout::Player::NAME, name)
        .unwrap();

    let mut sample = World::default();
    let layers = Layers { objects: true, players: true, camera: false };
    crate::live::capture::read(g, layers, &mut sample).unwrap();
    let reply: ObjectsReply =
        serde_json::from_value(h.ok(&mut game, method::OBJECTS, Value::Null)).unwrap();
    assert!(!reply.truncated);
    assert_eq!(sample.objects.len(), reply.objects.len());
    for (obj, listed) in sample.objects.iter().zip(&reply.objects) {
        assert_eq!(handle(obj.id), listed.id);
        assert_eq!(
            ObjectType::from_u8(obj.kind).map(|k| k.group()),
            listed.kind.as_deref()
        );
        assert_eq!(obj.tag.map(handle), listed.definition);
        assert_eq!(obj.parent.map(handle), listed.parent);
        assert_eq!(obj.position, listed.position);
        assert_eq!(obj.forward, listed.forward);
        assert_eq!(obj.up, listed.up);
        assert_eq!(obj.scale, listed.scale);
        assert_eq!(obj.velocity, listed.velocity);
        assert_eq!(
            (obj.body, obj.shield),
            (listed.vitality.body, listed.vitality.shield)
        );
    }
    assert_eq!(sample.objects[0].flags, header::ACTIVE | header::AWAKE);
    assert_eq!(reply.objects[0].flags, ["active", "awake"]);
    assert_eq!(sample.objects[1].flags, 0);
    assert!(reply.objects[1].flags.is_empty());
    assert_eq!(sample.objects[0].placement, Some(5));
    assert_eq!(sample.objects[1].placement, None);

    assert_eq!(sample.players.len(), 2);
    let player = &sample.players[0];
    assert_eq!(player.name, "Nyx ∆");
    assert_eq!((player.index, player.team, player.local_user), (0, 0, Some(0)));
    assert_eq!(player.unit, Some(sample.objects[0].id));
    assert_eq!(player.held, Some(sample.objects[1].id));
    assert_eq!(player.aim, [0.0, -1.0, 0.0]);
    assert_eq!(sample.players[1].unit, None, "still listed between lives");
}

#[test]
fn sim_objects_lists_the_pool_through_the_typed_views() {
    let (h, mut game) = (Harness::new(), game());
    let r = h.ok(&mut game, method::OBJECTS, Value::Null);
    let objects = r["objects"].as_array().unwrap();
    assert_eq!(objects.len(), 2, "{r}");
    assert_eq!(r["truncated"], false);
    let (unit, weapon) = (&objects[0], &objects[1]);
    assert_eq!(unit["kind"], "bipd");
    assert_eq!(weapon["kind"], "weap");
    assert_eq!(unit["velocity"][0], 3.0);
    assert_eq!(
        unit["vitality"],
        json!({"body": 1.0, "shield": 1.0, "depleted": false})
    );
    assert_eq!(unit["unit"]["weapons"][0], weapon["id"]);
    assert_eq!(unit["unit"]["weapons"][1], Value::Null);
    assert_eq!(unit["unit"]["held"], weapon["id"]);
    assert!(weapon.get("unit").is_none());
    assert_eq!(unit["nodes"], 0);

    let r = h.ok(&mut game, method::OBJECTS, json!({"kinds": ["weap"]}));
    assert_eq!(r["objects"].as_array().unwrap().len(), 1);
    assert_eq!(r["objects"][0]["id"], weapon["id"]);
    let r = h.ok(&mut game, method::OBJECTS, json!({"max": 1}));
    assert_eq!(r["objects"].as_array().unwrap().len(), 1);
    assert_eq!(r["truncated"], true);
    let e = h.err(&mut game, method::OBJECTS, json!({"kinds": ["moon"]}));
    assert_eq!(e["code"], code::INVALID_PARAMS);
}

#[test]
fn sim_events_answers_the_tick_events_the_game_thread_recorded() {
    use engine::sim_objects::damage;
    let (h, mut game) = (Harness::new(), game());
    damage::tick_starts();
    let r = h.ok(&mut game, method::EVENTS, Value::Null);
    assert_eq!(r["events"], json!([]));
    assert_eq!(r["at"]["boundary"], "tick");

    // A kill credited at PLAYER_KILLED's entry, recorded by the observer
    // the object update's switch installs: attacker, victim, code.
    let mut g = runtime::Guest::new();
    g.map_zeroed(0x1_0000, 0x1000, runtime::guest::Permissions::RW).unwrap();
    let esp = 0x1_0800;
    for (i, v) in [(1, 0xe101_0001), (2, 0xe100_0000), (4, 0x4d)] {
        g.set_u32(esp + 4 * i, v).unwrap();
    }
    damage::player_killed(&g, esp);
    let r = h.ok(&mut game, method::EVENTS, json!({"kinds": ["kill"]}));
    let events = r["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{r}");
    assert_eq!(events[0]["type"], "kill");
    assert_eq!(events[0]["attacker"], "0xe1010001");
    assert_eq!(events[0]["victim"], "0xe1000000");
    assert_eq!((events[0]["damage_type"].as_u64(), events[0]["modifier"].as_u64()), (Some(13), Some(1)));
    // No players array or game time in this guest: no units, no tick.
    assert_eq!(events[0]["victim_unit"], Value::Null);
    assert_eq!(events[0]["tick"], Value::Null);
    let r = h.ok(&mut game, method::EVENTS, json!({"kinds": ["damage"]}));
    assert_eq!(r["events"], json!([]));

    let e = h.err(&mut game, method::EVENTS, json!({"kinds": ["moon"]}));
    assert_eq!(e["code"], code::INVALID_PARAMS);
    damage::tick_starts();
}

fn edit_requests() -> [(&'static str, Value); 4] {
    [
        (method::OBJECT_SET, json!({"id":"vehicle.12340000","pos":[1,2,3]})),
        (method::OBJECT_DELETE, json!({"id":"vehicle.12340000"})),
        (method::OBJECT_RESPAWN, json!({"placement":{"block":"vehicles","index":0}})),
        (method::MAP_RELOAD, json!({})),
    ]
}

/// Run the production guest-call body over the harness guest, including the
/// tick reentry. The following on_tick settles the socket reply.
fn drive_edit_tick(
    g: &mut Game,
    q: &super::calls::Shared,
    callee: impl FnMut(&guest_layout::entry::GuestCall, &mut dyn GuestMemory) -> guest_layout::entry::Regs,
) -> Vec<guest_layout::entry::GuestCall> {
    use guest_layout::entry::{Cx, EntryFn, EntryFuture, Regs, Ret};
    use std::sync::atomic::AtomicBool;

    const SCRATCH: u32 = 0x8130_0000;
    g.g.map(SCRATCH, super::calls::SCRATCH_SIZE);
    let work = super::calls::lock(q).drain();
    assert_eq!(work.len(), 1);
    let q = q.clone();
    let f: Box<EntryFn> = Box::new(move |cx: Cx| -> EntryFuture {
        Box::pin(super::calls::run_queued(
            cx,
            q.clone(),
            work.clone(),
            SCRATCH,
            Arc::new(AtomicBool::new(false)),
        ))
    });
    let r = guest_layout::entry::testing::run(&*f, std::mem::take(&mut g.g), Regs::default(), callee);
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    assert_eq!(
        r.calls.last().unwrap().target,
        guest_layout::points::GAME_TICK.base
    );
    g.g = r.guest;
    r.calls
}

#[test]
fn world_edits_gate_the_queue() {
    let (h, q) = with_calls();
    let mut g = game();
    for (name, params) in edit_requests() {
        h.send(name, params);
        h.agent.between_slices(&mut g, 0);
        assert_eq!(h.reply()["error"]["code"], code::UNAVAILABLE);
    }
    for kind in [SessionKind::CoopHost, SessionKind::CoopGuest, SessionKind::MpClient] {
        *h.kind.lock().unwrap() = kind;
        for (name, params) in edit_requests() {
            assert_eq!(h.err(&mut g, name, params)["code"], code::INVALID_REQUEST);
        }
    }
    *h.kind.lock().unwrap() = SessionKind::Local;
    h.agent.set_live_session_probe(|_| true);
    for (name, params) in edit_requests() {
        assert_eq!(h.err(&mut g, name, params)["code"], REFUSED);
    }
    assert!(!super::calls::lock(&q).has_pending());
}

/// Matchmaking and Live custom games are the external Live probe's, whatever
/// the peers: a placement edit, a count change or not, never reaches the queue.
#[test]
fn placement_edits_gate_the_queue_like_the_object_edits() {
    let (h, q) = with_calls();
    let mut g = game();
    let requests = [
        json!({"block": "scenery", "was_count": 1, "count": 2}),
        json!({"block": "scenery", "was_count": 1, "count": 1}),
    ];
    for kind in [SessionKind::CoopHost, SessionKind::CoopGuest, SessionKind::MpClient] {
        *h.kind.lock().unwrap() = kind;
        for params in &requests {
            let e = h.err(&mut g, method::PLACEMENT_EDIT, params.clone());
            assert_eq!(e["code"], code::INVALID_REQUEST, "{kind:?}");
        }
    }
    for kind in [SessionKind::Local, SessionKind::MpHost] {
        *h.kind.lock().unwrap() = kind;
        h.agent.set_live_session_probe(|_| true);
        for params in &requests {
            assert_eq!(h.err(&mut g, method::PLACEMENT_EDIT, params.clone())["code"], REFUSED);
        }
    }
    assert!(!super::calls::lock(&q).has_pending());
}

/// The Live gate is the live-edit probe's when boot installs one: a Live
/// custom game this console hosts passes it (the checks after it are the
/// System Link host's), and matchmaking, a client and an unreadable session
/// give their reasons. Without the probe, every Live game is refused.
#[test]
fn the_live_edit_probe_decides_which_live_games_take_edits() {
    for name in [method::PLACEMENT_EDIT, method::OBJECT_DELETE] {
        let params = if name == method::OBJECT_DELETE {
            json!({"id": "bad"})
        } else {
            // Not a block: a request past the gate answers at once instead of
            // queueing for a tick this test never runs.
            json!({"block": "nope", "was_count": 1, "count": 1})
        };
        let (h, q) = with_calls();
        let mut g = game();
        *h.kind.lock().unwrap() = SessionKind::MpHost;
        h.agent.set_live_session_probe(|_| true);
        let e = h.err(&mut g, name, params.clone());
        assert_eq!(e["message"], "external Live session", "{name}: no edit probe");

        h.agent.set_live_edit_probe(|_| None);
        let e = h.err(&mut g, name, params.clone());
        let message = e["message"].as_str().unwrap_or_default();
        assert!(
            !message.contains("Live") && !message.contains("matchmaking"),
            "{name}: a Live custom host passes the gate: {e}"
        );

        h.agent.set_live_edit_probe(|_| Some("a matchmaking game"));
        let e = h.err(&mut g, name, params.clone());
        assert_eq!((e["code"].as_i64(), e["message"].as_str()), (Some(REFUSED), Some("a matchmaking game")));
        h.agent.set_live_edit_probe(|_| Some("a Live game another console hosts"));
        let e = h.err(&mut g, name, params.clone());
        assert_eq!(e["message"], "a Live game another console hosts");
        // A Live client is the session kind's refusal, whatever the probe says.
        h.agent.set_live_edit_probe(|_| None);
        *h.kind.lock().unwrap() = SessionKind::MpClient;
        assert_eq!(h.err(&mut g, name, params)["code"], code::INVALID_REQUEST);
        assert!(!super::calls::lock(&q).has_pending());
    }
}

#[test]
fn object_edits_check_the_callers_epoch_again_before_the_engine_call() {
    for (name, mut params) in edit_requests().into_iter().take(3) {
        let (h, q) = with_calls();
        let mut g = game();
        let status = h.ok(&mut g, agent_proto::live::STATUS, Value::Null);
        let epoch = status["map"]["epoch"].as_u64().unwrap();
        params["epoch"] = json!(epoch);
        h.deferred(&mut g, name, params.clone());
        h.agent.live_cut();
        let calls = drive_edit_tick(&mut g, &q, |_, _| guest_layout::entry::Regs::default());
        assert_eq!(calls.len(), 1, "{name} must only reenter the tick");
        h.agent.on_tick(&mut g);
        let reply = h.reply();
        assert_eq!(reply["error"]["code"], code::INVALID_REQUEST);
        assert_eq!(reply["error"]["message"], "the map epoch changed");

        let error = h.err(&mut g, name, params);
        assert_eq!(error, reply["error"]);
        assert!(!super::calls::lock(&q).has_pending());
    }
}

#[test]
fn confirmed_edits_recheck_the_salt_at_execution() {
    let h = Harness::new();
    let mut g = game();
    let sim = super::install(
        &h.agent,
        Box::new(h.bots.clone()),
        Some(Box::new(|_| SessionView {
            kind: SessionKind::MpHost,
            peers: Some(3),
            ..Default::default()
        })),
    );
    let q = super::calls::shared();
    super::calls::lock(&q).arm();
    sim.set_calls(q.clone());
    sim.install_world_edits(&h.agent);
    for (name, mut p) in edit_requests() {
        p["confirm_peers"] = json!(2);
        let e = h.err(&mut g, name, p);
        assert!(e["message"].as_str().unwrap().contains("peer count"));
    }
    assert!(!super::calls::lock(&q).has_pending());
    let pool = guest_layout::view::objects::array(&g.g).unwrap().unwrap();
    let slot = pool.get_absolute(&g.g, 1).unwrap().unwrap();
    let salt = slot.salt(&g.g).unwrap();
    let id = format!("weapon.{salt:04x}0001");
    // A loose weapon is addressable by its world id.
    Body::at(WEAPON_BODY).set_parent(&mut g.g, None).unwrap();
    let epoch = h.ok(&mut g, agent_proto::live::STATUS, Value::Null)["map"]["epoch"].clone();
    h.deferred(
        &mut g,
        method::OBJECT_SET,
        json!({"id":id,"pos":[5,4,3],"confirm_peers":3,"epoch":epoch}),
    );
    let calls = drive_edit_tick(&mut g, &q, |_, _| guest_layout::entry::Regs::default());
    assert_eq!(calls[0].target, super::calls::addr::OBJECT_SET_POSITION);
    h.agent.on_tick(&mut g);
    let reply = h.reply();
    let moved: agent_proto::sim::ObjectSetResult =
        serde_json::from_value(reply["result"].clone()).unwrap();
    assert_eq!(moved.object, id);
    assert_eq!(moved.at.boundary, agent_proto::Boundary::Tick);

    h.deferred(&mut g, method::OBJECT_DELETE, json!({"id":id,"confirm_peers":3}));
    // Reuse the weapon slot after accepting the request and before the hook.
    slot.set_salt(&mut g.g, salt.wrapping_add(1)).unwrap();
    let calls = drive_edit_tick(&mut g, &q, |_, _| guest_layout::entry::Regs::default());
    assert_eq!(calls.len(), 1, "only the game tick");
    h.agent.on_tick(&mut g);
    assert_eq!(h.reply()["error"]["code"], code::GONE);
}

#[test]
fn only_an_authorized_world_write_adopts_the_game() {
    let (h, q, sim) = with_calls_and_sim();
    let installs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = installs.clone();
    sim.on_first_edit(&h.agent, move |_| {
        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    });
    let mut g = game();
    h.ok(&mut g, agent_proto::method::METHODS, Value::Null);
    h.err(&mut g, method::OBJECT_DELETE, json!({"id": "bad"}));
    assert_eq!(installs.load(std::sync::atomic::Ordering::Relaxed), 0);
    for _ in 0..2 {
        h.deferred(&mut g, method::OBJECT_DELETE, json!({"id": "weapon.11220001"}));
        assert_eq!(installs.load(std::sync::atomic::Ordering::Relaxed), 1);
        the_game_makes(&q, |_| Ok(json!({})));
        h.agent.on_tick(&mut g);
        h.reply();
    }
}

#[test]
fn input_logs_and_editor_reloads_exclude_each_other() {
    let (_, q) = with_calls();
    let mut input = crate::Input::default();
    let name = "fixture";
    let path = std::path::PathBuf::from("fixture.input");
    input.record(name, path.clone()).unwrap();
    assert!(input.allow_reload(&q).unwrap_err().contains("recording"));
    input.stop().unwrap();
    input.allow_reload(&q).unwrap();
    let pending = super::calls::hold_reload(&q, Box::new(|_| Ok(()))).unwrap();
    assert!(input.record(name, path.clone()).unwrap_err().contains("reload"));
    assert!(input.replay(name, path.clone(), xapi::tap::InputLog { reads: Vec::new() })
        .unwrap_err().contains("reload"));
    drop(pending);
    input.record(name, path).unwrap();
}

#[test]
fn reload_queues_the_active_options() {
    use super::calls::OPTIONS_SIZE;
    use guest_layout::game::{GameGlobals, MODE_CAMPAIGN, MODE_MULTIPLAYER, OPTIONS};

    let mut g = game();
    let h = Harness::new();
    let methods = h.ok(&mut g, agent_proto::method::METHODS, Value::Null);
    assert!(!methods.as_array().unwrap().contains(&json!(method::MAP_RELOAD)));
    let (h, q) = with_calls();
    let methods = h.ok(&mut g, agent_proto::method::METHODS, Value::Null);
    for name in method::EDITS {
        assert!(methods.as_array().unwrap().contains(&json!(name)));
    }

    let base = 0x8140_0000;
    g.g.put_u32(guest_layout::points::GAME_GLOBALS.base, base);
    g.g.map(base, GameGlobals::SIZE);
    let e = h.err(&mut g, method::MAP_RELOAD, json!({}));
    assert_eq!(e["code"], REFUSED);
    g.g.put_u8(base + GameGlobals::MAP_ACTIVE.offset, 1);
    g.g.put_u8(super::calls::OPTIONS_PENDING, 0);
    let mut previous = 0;
    for mode in [MODE_CAMPAIGN, MODE_MULTIPLAYER] {
        g.g.map(super::calls::PENDING_OPTIONS, OPTIONS_SIZE);
        g.g.put_u8(super::calls::OPTIONS_PENDING, 0);
        let mut options = vec![0x5a; OPTIONS_SIZE as usize];
        options[..4].copy_from_slice(&mode.to_le_bytes());
        g.g.put(base + OPTIONS, &options);
        let e = h.err(&mut g, method::MAP_RELOAD, json!({"file":"preview.map"}));
        assert_eq!(e["code"], REFUSED);
        assert!(!super::calls::lock(&q).has_pending());
        h.deferred(&mut g, method::MAP_RELOAD, json!({}));
        let calls = drive_edit_tick(&mut g, &q, |call, m| {
            if call.target == guest_layout::points::OPTIONS_QUEUE.base {
                assert!(call.args.is_empty());
                assert_eq!(m.bytes(call.eax.unwrap(), options.len()).unwrap(), options);
                m.write(super::calls::PENDING_OPTIONS, &options).unwrap();
                m.write(super::calls::OPTIONS_PENDING, &[1]).unwrap();
            }
            guest_layout::entry::Regs::default()
        });
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].target, guest_layout::points::OPTIONS_QUEUE.base);
        h.agent.on_tick(&mut g);
        let reply = h.reply();
        let result: agent_proto::sim::MapReloadResult =
            serde_json::from_value(reply["result"].clone()).unwrap();
        assert!(result.queued);
        assert!(result.activation > previous);
        previous = result.activation;
        assert_eq!(Some(result.map), g.status().map);
        assert_eq!((result.file, result.checksum), (None, None));
    }
}

fn reload_fixture() -> (Game, u32, std::path::PathBuf) {
    use guest_layout::game::{GameGlobals, MODE_MULTIPLAYER, OPTIONS};

    let mut g = game();
    let base = 0x8140_0000;
    let options = base + OPTIONS;
    g.g.put_u32(guest_layout::points::GAME_GLOBALS.base, base);
    g.g.map(base, GameGlobals::SIZE);
    g.g.put_u8(base + GameGlobals::MAP_ACTIVE.offset, 1);
    g.g.put_u8(super::calls::OPTIONS_PENDING, 0);
    g.g.put_u32(options, MODE_MULTIPLAYER);
    g.g.put(options + 0x1c, br"levels\fixture\scenario");
    g.g.map(super::calls::PENDING_OPTIONS, super::calls::OPTIONS_SIZE);
    let file = std::env::temp_dir().join("reload").join("Lockout.MAP");
    (g, options, file)
}

fn finished_preparation(
    result: Result<super::PreparedMapReload, String>,
) -> std::sync::mpsc::Receiver<Result<super::PreparedMapReload, String>> {
    let (send, recv) = std::sync::mpsc::channel();
    send.send(result).unwrap();
    recv
}

#[test]
fn file_reload_binds_between_options_queue_and_the_tick() {
    let (h, q, sim) = with_calls_and_sim();
    let (mut g, options, file) = reload_fixture();
    let events = Arc::new(Mutex::new(Vec::new()));
    let prepared = events.clone();
    let expected_file = file.clone();
    sim.set_map_rebind(Box::new(move |map, scenario, path, _| {
        assert_eq!(map, "lockout");
        assert_eq!(scenario, r"levels\fixture\scenario");
        assert_eq!(path, expected_file.as_path());
        prepared.lock().unwrap().push("prepare");
        let committed = prepared.clone();
        let binding = super::PreparedMapReload::new(0x1234, move || {
            committed.lock().unwrap().push("reserve");
            Ok(super::ReservedMapReload::new(
                move || committed.lock().unwrap().push("commit"),
                || panic!("a successful reload was rolled back"),
            ))
        });
        Ok(finished_preparation(Ok(binding)))
    }));
    h.deferred(&mut g, method::MAP_RELOAD, json!({ "file": file }));
    assert_eq!(*events.lock().unwrap(), ["prepare"]);
    let second = h.err(&mut g, method::MAP_RELOAD, json!({}));
    assert_eq!(second["code"], REFUSED);
    let calls = drive_edit_tick(&mut g, &q, |c, m| {
        if c.target == guest_layout::points::OPTIONS_QUEUE.base {
            let size = super::calls::OPTIONS_SIZE as usize;
            assert_eq!(
                m.bytes(c.eax.unwrap(), size).unwrap(),
                m.bytes(options, size).unwrap()
            );
            let bytes = m.bytes(c.eax.unwrap(), size).unwrap();
            m.write(super::calls::PENDING_OPTIONS, &bytes).unwrap();
            m.write(super::calls::OPTIONS_PENDING, &[1]).unwrap();
            events.lock().unwrap().push("queue");
        } else {
            events.lock().unwrap().push("tick");
        }
        guest_layout::entry::Regs::default()
    });
    assert_eq!(calls.len(), 2);
    assert_eq!(
        *events.lock().unwrap(),
        ["prepare", "reserve", "queue", "commit", "tick"]
    );
    h.agent.on_tick(&mut g);
    let result = &h.reply()["result"];
    assert_eq!(result["queued"], true);
    assert_eq!(result["map"], "lockout");
    assert_eq!(result["file"], json!(file));
    assert_eq!(result["checksum"], 0x1234);
    // The real main loop has not consumed the first request yet.
    h.deferred(&mut g, method::MAP_RELOAD, json!({}));
    let calls = drive_edit_tick(&mut g, &q, |_, _| guest_layout::entry::Regs::default());
    assert_eq!(calls.len(), 1, "only the tick");
    h.agent.on_tick(&mut g);
    assert_eq!(h.reply()["error"]["code"], REFUSED);
}

#[test]
fn file_reload_refuses_wrong_names_unreadable_maps_and_read_only_hosts() {
    let (h, q, sim) = with_calls_and_sim();
    let (mut g, _, file) = reload_fixture();
    let error = h.err(&mut g, method::MAP_RELOAD, json!({ "file": file }));
    assert!(error["data"]["why"].as_str().unwrap().contains("mount"));
    sim.set_map_rebind(Box::new(|_, _, path, _| {
        assert_eq!(path.file_name().unwrap(), "Lockout.MAP");
        Ok(finished_preparation(Err("invalid fixture map header".into())))
    }));
    let error = h.err(&mut g, method::MAP_RELOAD, json!({ "file": file.with_file_name("other.map") }));
    assert_eq!(error["code"], REFUSED);
    assert!(error["data"]["why"].as_str().unwrap().contains("named lockout.map"));
    h.deferred(&mut g, method::MAP_RELOAD, json!({ "file": file }));
    h.agent.on_tick(&mut g);
    let error = &h.reply()["error"];
    assert_eq!(error["code"], REFUSED);
    assert_eq!(error["data"]["why"], "invalid fixture map header");
    sim.install_reload_refusal(&h.agent, "map reload is disabled during playback");
    let error = h.err(&mut g, method::MAP_RELOAD, json!({}));
    assert_eq!(error["code"], REFUSED);
    assert!(error["data"]["why"].as_str().unwrap().contains("playback"));
    assert!(!super::calls::lock(&q).has_pending());
}

#[test]
fn changed_options_or_failed_reservation_refuse_before_the_guest_queue() {
    let (h, q, sim) = with_calls_and_sim();
    let (mut g, options, file) = reload_fixture();
    sim.set_map_rebind(Box::new(|_, _, _, _| {
        let binding = super::PreparedMapReload::new(7, || {
            Err("the scenario cache slot changed".into())
        });
        Ok(finished_preparation(Ok(binding)))
    }));
    h.deferred(&mut g, method::MAP_RELOAD, json!({ "file": file }));
    h.agent.on_tick(&mut g);
    g.g.put(options + 0x1c, b"other scenario\0");
    let calls = drive_edit_tick(&mut g, &q, |_, _| guest_layout::entry::Regs::default());
    assert_eq!(calls.len(), 1, "only the tick");
    h.agent.on_tick(&mut g);
    let error = &h.reply()["error"];
    assert_eq!(error["code"], REFUSED);
    assert!(
        error["data"]["why"].as_str().unwrap().contains("options changed")
    );

    h.deferred(&mut g, method::MAP_RELOAD, json!({ "file": file }));
    h.agent.on_tick(&mut g);
    let calls = drive_edit_tick(&mut g, &q, |call, m| {
        assert_eq!(call.target, guest_layout::points::GAME_TICK.base);
        assert_eq!(m.u8(super::calls::OPTIONS_PENDING).unwrap(), 0);
        guest_layout::entry::Regs::default()
    });
    assert_eq!(calls.len(), 1);
    h.agent.on_tick(&mut g);
    let error = &h.reply()["error"];
    assert_eq!(error["code"], REFUSED);
    assert_eq!(error["data"]["why"], "the scenario cache slot changed");
}

#[test]
fn file_preparation_waits_off_the_tick_and_rechecks_the_session() {
    let (h, q, sim) = with_calls_and_sim();
    let (mut g, _, file) = reload_fixture();
    let (send, recv) = std::sync::mpsc::channel();
    let recv = Mutex::new(Some(recv));
    sim.set_map_rebind(Box::new(move |_, _, _, _| Ok(recv.lock().unwrap().take().unwrap())));
    h.deferred(&mut g, method::MAP_RELOAD, json!({ "file": file }));
    for _ in 0..=super::CALL_TICKS {
        h.agent.on_tick(&mut g);
        assert!(!super::calls::lock(&q).has_pending());
    }
    assert!(h.rx.try_recv().is_err());
    *h.kind.lock().unwrap() = SessionKind::MpClient;
    send.send(Ok(super::PreparedMapReload::new(31, || panic!("a client reserved the source")))).unwrap();
    h.agent.on_tick(&mut g);
    let calls = drive_edit_tick(&mut g, &q, |call, _| {
        assert_eq!(call.target, guest_layout::points::GAME_TICK.base);
        guest_layout::entry::Regs::default()
    });
    assert_eq!(calls.len(), 1);
    h.agent.on_tick(&mut g);
    assert_eq!(h.reply()["error"]["code"], REFUSED);
}

#[test]
fn timed_out_file_preparation_revokes_the_worker_and_releases_the_game_ticket() {
    let (h, q, sim) = with_calls_and_sim();
    let (mut g, _, file) = reload_fixture();
    let (send, recv) = std::sync::mpsc::channel();
    let recv = Mutex::new(Some(recv));
    let (token, pending) = std::sync::mpsc::channel();
    sim.set_map_rebind(Box::new(move |_, _, _, cancelled| {
        token.send(cancelled).unwrap();
        Ok(recv.lock().unwrap().take().unwrap())
    }));
    h.deferred(&mut g, method::MAP_RELOAD, json!({ "file": file }));
    let cancelled = pending.try_recv().unwrap();
    assert!(!cancelled.load(std::sync::atomic::Ordering::Acquire));
    h.agent.expire_deferred_for_test(h.conn);
    h.agent.on_tick(&mut g);
    assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(h.reply()["error"]["code"], code::TIMEOUT);
    assert!(send.send(Err("worker finished after timeout".into())).is_err());
    assert!(super::calls::lock(&q).live().is_none());
    assert!(!super::calls::lock(&q).has_pending());
}

#[test]
fn playback_without_guest_calls_does_not_advertise_reload() {
    let h = Harness::new();
    let sim = super::install(&h.agent, Box::new(h.bots.clone()), None);
    sim.install_reload_refusal(&h.agent, "map reload is disabled during playback");
    let methods = h.ok(&mut game(), agent_proto::method::METHODS, Value::Null);
    assert!(!methods.as_array().unwrap().contains(&json!(method::MAP_RELOAD)));
}

#[test]
fn timed_out_edits_make_no_guest_calls_when_the_hook_resumes() {
    use guest_layout::entry::{Cx, EntryFuture, Regs, Step, poll};
    use std::sync::atomic::AtomicBool;

    let (h, q) = with_calls();
    let (mut g, _, _) = reload_fixture();
    for (name, params) in edit_requests() {
        h.send(name, params);
    }
    h.agent.on_tick(&mut g);
    assert!(h.rx.try_recv().is_err());
    assert!(super::calls::lock(&q).has_pending());
    for _ in 0..super::CALL_TICKS {
        h.agent.on_tick(&mut g);
    }
    for _ in edit_requests() {
        assert_eq!(h.reply()["error"]["code"], code::TIMEOUT);
    }
    let work = super::calls::lock(&q).drain();
    assert!(work.is_empty());
    let cx = Cx::new(Mutex::new(g.g), Regs::default());
    let mut resumed: EntryFuture = Box::pin(super::calls::run_queued(
        cx.clone(),
        q,
        work,
        0x8130_0000,
        Arc::new(AtomicBool::new(false)),
    ));
    let Step::Call(call) = poll(&mut resumed, &cx) else {
        panic!("expected the game tick")
    };
    assert_eq!(call.target, guest_layout::points::GAME_TICK.base);
    guest_layout::entry::resume(&cx, Regs::default());
    assert!(matches!(poll(&mut resumed, &cx), Step::Done(Ok(_))));
}

#[test]
fn shutdown_answers_deferred_edits_and_cancels_their_tickets() {
    let (h, q) = with_calls();
    let mut g = game();
    h.deferred(&mut g, method::OBJECT_DELETE, json!({"id": "vehicle.12340000"}));
    h.agent.shutdown();
    assert_eq!(h.reply()["error"]["code"], code::SHUTDOWN);
    assert!(super::calls::lock(&q).live().is_none());
}

/// Player 0 standing at a point, aiming along +y.
fn standing_game() -> Game {
    let mut g = game();
    g.objects[0].pos = Some([5.0, 6.0, 7.0]);
    g.objects[0].forward = Some([0.0, 1.0, 0.0]);
    g.objects[0].yaw = Some(90.0);
    g.objects[0].pitch = Some(0.0);
    g
}

#[test]
fn a_mark_keeps_the_local_players_place_and_a_recall_teleports_back() {
    let (h, q) = with_calls();
    let mut game = standing_game();
    let m = h.ok(&mut game, method::PLAYER_MARK, json!({}));
    assert_eq!(m["slot"], agent_proto::sim::DEFAULT_MARK_SLOT);
    assert_eq!(m["player"], "player.0");
    assert_eq!(m["pos"], json!([5.0, 6.0, 7.0]));
    assert_eq!(m["forward"], json!([0.0, 1.0, 0.0]));
    assert_eq!(m["map"], "lockout");
    assert_eq!(m["slots"], json!(["quick"]));
    let m = h.ok(&mut game, method::PLAYER_MARK, json!({"slot": 2}));
    assert_eq!(m["slots"], json!(["2", "quick"]));
    // Moved away; the recall goes through the game's teleport.
    game.objects[0].pos = Some([0.0; 3]);
    let handle = super::state::player_handle(&game.g, 0).unwrap();
    h.deferred(&mut game, method::PLAYER_RECALL, json!({}));
    let calls = the_game_makes(&q, |_| Ok(json!({"pos": [5.0, 6.0, 7.1]})));
    let super::calls::Call::Teleport {
        player,
        pos,
        facing,
    } = calls[0].clone()
    else {
        panic!("{calls:?}");
    };
    assert_eq!((player, pos), (handle, Some([5.0, 6.0, 7.0])));
    assert_eq!(facing, Some([0.0, 1.0, 0.0]));
    h.agent.on_tick(&mut game);
    let r = h.reply();
    assert!(r["error"].is_null(), "{r}");
    let r = &r["result"];
    assert_eq!(r["slot"], "quick");
    assert_eq!(r["mark"]["pos"], json!([5.0, 6.0, 7.0]));
    assert_eq!(r["wrote"], json!(["pos", "facing"]));
    assert_eq!(r["teleport"]["pos"], json!([5.0, 6.0, 7.1]));
}

#[test]
fn marks_refuse_what_they_cannot_keep_or_put_back() {
    let (h, q) = with_calls();
    let mut game = game();
    // Player 0 has no position here.
    let e = h.err(&mut game, method::PLAYER_MARK, json!({}));
    assert_eq!(e["code"], code::GONE, "{e}");
    let e = h.err(&mut game, method::PLAYER_MARK, json!({"slot": ""}));
    assert_eq!(e["code"], code::INVALID_PARAMS, "{e}");
    let e = h.err(&mut game, method::PLAYER_MARK, json!({"player": "actor.e1230045"}));
    assert_eq!(e["code"], code::INVALID_PARAMS, "{e}");
    let e = h.err(&mut game, method::PLAYER_RECALL, json!({"slot": "nowhere"}));
    assert_eq!(e["code"], code::GONE, "{e}");
    assert!(e["message"].as_str().unwrap().contains("player.mark"), "{e}");
    // Too many slots.
    let mut game = standing_game();
    for i in 0..agent_proto::sim::MAX_MARKS {
        h.ok(&mut game, method::PLAYER_MARK, json!({"slot": i}));
    }
    let e = h.err(&mut game, method::PLAYER_MARK, json!({"slot": "one more"}));
    assert_eq!(e["code"], code::INVALID_PARAMS, "{e}");
    h.ok(&mut game, method::PLAYER_MARK, json!({"slot": 0}));
    // A co-op session refuses the recall's teleport, as sim.state.set does.
    *h.kind.lock().unwrap() = SessionKind::CoopHost;
    let e = h.err(&mut game, method::PLAYER_RECALL, json!({"slot": 0}));
    assert_eq!(e["code"], code::INVALID_REQUEST, "{e}");
    assert!(!super::calls::lock(&q).has_pending(), "nothing queued");
}

#[test]
fn a_mark_on_another_map_is_not_recalled() {
    struct Elsewhere(Game);
    impl Reader for Elsewhere {
        fn status(&mut self) -> GameStatus {
            GameStatus {
                map: Some("zanzibar".into()),
                ..self.0.status()
            }
        }
        fn objects(&mut self) -> Result<Vec<Object>, String> {
            self.0.objects()
        }
        fn with_guest(&mut self, f: &mut dyn FnMut(&mut dyn GuestMemory)) -> bool {
            self.0.with_guest(f)
        }
    }
    let (h, q) = with_calls();
    let mut game = standing_game();
    h.ok(&mut game, method::PLAYER_MARK, json!({"slot": "ledge"}));
    let mut other = Elsewhere(standing_game());
    h.send(method::PLAYER_RECALL, json!({"slot": "ledge"}));
    h.agent.on_tick(&mut other);
    let r = h.reply();
    assert_eq!(r["error"]["code"], code::INVALID_REQUEST, "{r}");
    assert!(r["error"]["message"].as_str().unwrap().contains("lockout"), "{r}");
    assert!(!super::calls::lock(&q).has_pending());
}

#[test]
fn host_pause_needs_a_window_and_host_step_counts_the_agents_ticks() {
    use agent_proto::host_pause::{self, Hub};
    let hub: &'static Hub = Box::leak(Box::new(Hub::new()));
    let (h, mut game) = (Harness::new(), game());
    super::pause::install(&h.agent, hub);
    // Answered on the connection's thread: no tick needed.
    h.send(host_pause::method::PAUSE, json!({"on": true}));
    let r = h.reply();
    assert_eq!(r["error"]["code"], code::UNAVAILABLE, "{r}");
    h.send(host_pause::method::STEP, json!({"ticks": 0}));
    assert_eq!(h.reply()["error"]["code"], code::INVALID_PARAMS);
    // `{}` reads the state: refused too while there is no window.
    h.send(host_pause::method::PAUSE, json!({}));
    assert_eq!(h.reply()["error"]["code"], code::UNAVAILABLE);
    // A window, paused, the guest held: two ticks run under the step.
    std::thread::spawn(move || hub.attach(Box::new(|| {}))).join().unwrap();
    hub.report(true);
    hub.set_held(true);
    h.send(host_pause::method::PAUSE, json!({}));
    let r = h.reply();
    assert_eq!(r["result"]["paused"], json!(true), "{r}");
    let t = std::thread::spawn(move || hub.step(2, Duration::from_secs(10)));
    while !hub.gate_open() {
        std::thread::yield_now();
    }
    hub.set_held(false);
    h.agent.on_tick(&mut game);
    assert!(hub.gate_open());
    h.agent.on_tick(&mut game);
    assert!(!hub.gate_open());
    hub.set_held(true);
    let r = t.join().unwrap().unwrap();
    assert_eq!(r.stepped, Some(2));
    assert_eq!(r.tick, Some(100));
    let m = h.ok(&mut game, agent_proto::method::METHODS, Value::Null);
    for name in host_pause::method::ALL {
        assert!(m.as_array().unwrap().contains(&json!(name)), "{name}");
    }
}

#[test]
fn host_quit_needs_a_window_and_asks_it_to_close() {
    use agent_proto::host_quit::{self, Hub};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let hub: &'static Hub = Box::leak(Box::new(Hub::new()));
    let (h, mut game) = (Harness::new(), game());
    super::quit::install(&h.agent, hub);
    // Answered on the connection's thread: no tick needed.
    h.send(host_quit::method::QUIT, json!({}));
    let r = h.reply();
    assert_eq!(r["error"]["code"], code::UNAVAILABLE, "{r}");
    h.send(host_quit::method::QUIT, json!({"save": true}));
    assert_eq!(h.reply()["error"]["code"], code::INVALID_PARAMS);
    let closes: &'static AtomicUsize = Box::leak(Box::new(AtomicUsize::new(0)));
    hub.attach(Box::new(move || {
        closes.fetch_add(1, Ordering::SeqCst);
    }));
    h.send(host_quit::method::QUIT, Value::Null);
    let r = h.reply();
    assert_eq!(r["result"]["closing"], json!(true), "{r}");
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    assert!(hub.requested());
    let m = h.ok(&mut game, agent_proto::method::METHODS, Value::Null);
    for name in host_quit::method::ALL {
        assert!(m.as_array().unwrap().contains(&json!(name)), "{name}");
    }
}

#[test]
fn host_window_resize_needs_a_window_and_answers_its_new_shape() {
    use agent_proto::host_window::{self, Hub, WindowState};
    let hub: &'static Hub = Box::leak(Box::new(Hub::new()));
    let (h, mut game) = (Harness::new(), game());
    super::window::install(&h.agent, hub);
    // Answered on the connection's thread: no tick needed.
    h.send(host_window::method::RESIZE, json!({"fullscreen": true}));
    let r = h.reply();
    assert_eq!(r["error"]["code"], code::UNAVAILABLE, "{r}");
    for bad in [json!({}), Value::Null, json!({"width": 800}), json!({"size": 1})] {
        h.send(host_window::method::RESIZE, bad.clone());
        assert_eq!(h.reply()["error"]["code"], code::INVALID_PARAMS, "{bad}");
    }
    // A window shell on its own thread takes the request and resizes.
    let (ready, attached) = std::sync::mpsc::channel();
    let shell = std::thread::spawn(move || {
        hub.attach(Box::new(|| {}));
        ready.send(()).unwrap();
        loop {
            if let Some(r) = hub.take_request() {
                let (width, height) = r.size.unwrap();
                hub.observe(WindowState { width, height, fullscreen: r.fullscreen, scale: 2.0 });
                return;
            }
            std::thread::yield_now();
        }
    });
    attached.recv().unwrap();
    h.send(host_window::method::RESIZE, json!({"width": 1920, "height": 816}));
    let mut r = h.reply();
    shell.join().unwrap();
    // Every result carries the session stamp; the shape is the rest.
    r["result"].as_object_mut().unwrap().remove("at");
    assert_eq!(
        r["result"],
        json!({"width": 1920, "height": 816, "fullscreen": false, "scale": 2.0}),
        "{r}"
    );
    let m = h.ok(&mut game, agent_proto::method::METHODS, Value::Null);
    for name in host_window::method::ALL {
        assert!(m.as_array().unwrap().contains(&json!(name)), "{name}");
    }
}

#[test]
fn host_screenshot_needs_a_window_and_answers_what_it_wrote() {
    use agent_proto::host_shot::{self, Hub, Shot};
    let hub: &'static Hub = Box::leak(Box::new(Hub::new()));
    let (h, mut game) = (Harness::new(), game());
    super::shot::install(&h.agent, hub);
    // Answered on the connection's thread: no tick needed.
    h.send(host_shot::method::SHOT, json!({}));
    let r = h.reply();
    assert_eq!(r["error"]["code"], code::UNAVAILABLE, "{r}");
    hub.attach(Box::new(|path| {
        Ok(Shot {
            path: path.unwrap_or_else(|| "/shots/h2-1.png".into()),
            width: 1280,
            height: 960,
        })
    }));
    h.send(host_shot::method::SHOT, json!({}));
    let r = h.reply();
    assert_eq!(
        r["result"],
        json!({"path": "/shots/h2-1.png", "width": 1280, "height": 960, "at": r["result"]["at"]}),
        "{r}"
    );
    h.send(host_shot::method::SHOT, json!({"path": "/tmp/menu.png"}));
    assert_eq!(h.reply()["result"]["path"], json!("/tmp/menu.png"));
    hub.detach();
    h.send(host_shot::method::SHOT, json!({}));
    assert_eq!(h.reply()["error"]["code"], code::UNAVAILABLE);
    let m = h.ok(&mut game, agent_proto::method::METHODS, Value::Null);
    for name in host_shot::method::ALL {
        assert!(m.as_array().unwrap().contains(&json!(name)), "{name}");
    }
}

#[test]
fn audio_play_queues_the_sound_start_at_the_players_unit_and_refuses_what_is_no_sound() {
    let (h, q) = with_calls();
    let mut game = game();
    h.deferred(
        &mut game,
        method::AUDIO_PLAY,
        json!({"sound": "sound\\pogo\\spring"}),
    );
    let unit_h = guest_layout::view::players::array(&game.g)
        .unwrap()
        .unwrap()
        .get_absolute(&game.g, 0)
        .unwrap()
        .unwrap()
        .unit(&game.g)
        .unwrap()
        .unwrap()
        .raw();
    let calls = the_game_makes(&q, |c| {
        assert_eq!(
            *c,
            super::calls::Call::Sound {
                unit: unit_h,
                sound: SND_TAG
            }
        );
        Ok(json!({"instance": "00cc0001"}))
    });
    assert_eq!(calls.len(), 1);
    h.agent.on_tick(&mut game);
    let r = h.reply();
    assert!(r["error"].is_null(), "{r}");
    assert_eq!(r["result"]["sound"]["group"], "snd!");
    assert_eq!(r["result"]["instance"], "00cc0001");
    assert_eq!(r["result"]["player"], "player.0");
    // A weapon is not a sound; an unknown name is not a tag.
    for (p, want) in [
        (json!({"sound": "0xe1000010"}), code::INVALID_PARAMS),
        (json!({"sound": "nothing"}), code::INVALID_PARAMS),
        (json!({"sound": "0xe4000040", "player": "player.1"}), code::GONE),
    ] {
        let e = h.err(&mut game, method::AUDIO_PLAY, p.clone());
        assert_eq!(e["code"], want, "{p}: {e}");
    }
}

#[test]
fn object_animate_queues_the_named_animation_on_the_object_and_refuses_what_it_cannot_name() {
    let (h, q) = with_calls();
    let mut game = game();
    h.deferred(
        &mut game,
        method::OBJECT_ANIMATE,
        json!({"id": "scenery.12340000", "animation": "idle"}),
    );
    let calls = the_game_makes(&q, |c| {
        assert!(matches!(c, super::calls::Call::Animate { name: 0x4d01, .. }), "{c:?}");
        Ok(json!({"index": 3, "frames": 31, "frame": 0.0}))
    });
    assert_eq!(calls.len(), 1);
    h.agent.on_tick(&mut game);
    let r = h.reply();
    assert!(r["error"].is_null(), "{r}");
    assert_eq!(r["result"]["object"], "scenery.12340000");
    assert_eq!(r["result"]["index"], 3);
    // A name the map has no string ID for, a bad id, a player's id, and a
    // co-op session (the object's pose is simulation state) are refused.
    for (p, want) in [
        (json!({"id": "scenery.12340000", "animation": "nothing"}), code::INVALID_PARAMS),
        (json!({"id": "bad", "animation": "idle"}), code::INVALID_PARAMS),
        (json!({"id": "player.0", "animation": "idle"}), code::INVALID_PARAMS),
    ] {
        let e = h.err(&mut game, method::OBJECT_ANIMATE, p.clone());
        assert_eq!(e["code"], want, "{p}: {e}");
    }
    *h.kind.lock().unwrap() = SessionKind::CoopHost;
    let e = h.err(
        &mut game,
        method::OBJECT_ANIMATE,
        json!({"id": "scenery.12340000", "animation": "idle"}),
    );
    assert_eq!(e["code"], code::INVALID_REQUEST, "{e}");
}

#[test]
fn recording_play_finds_the_recording_by_name_and_queues_the_players_call() {
    let (h, q) = with_calls();
    let mut game = game();
    game.g.put_u32(0x8110_0000 + 0x110, 2);
    game.g.put_u32(0x8110_0000 + 0x114, 0x8111_0000);
    game.g.put(0x8111_0000, b"arrive\0");
    game.g.put(0x8111_0000 + 0x34, b"leave\0");
    h.deferred(
        &mut game,
        method::RECORDING_PLAY,
        json!({"recording": "LEAVE", "mode": "hover"}),
    );
    let unit_h = guest_layout::view::players::array(&game.g)
        .unwrap()
        .unwrap()
        .get_absolute(&game.g, 0)
        .unwrap()
        .unwrap()
        .unit(&game.g)
        .unwrap()
        .unwrap()
        .raw();
    let calls = the_game_makes(&q, |c| {
        assert_eq!(
            *c,
            super::calls::Call::Recording {
                unit: unit_h,
                index: 1,
                flags: 0x10
            }
        );
        Ok(json!({"unit": "00010001", "index": 1}))
    });
    assert_eq!(calls.len(), 1);
    h.agent.on_tick(&mut game);
    let r = h.reply();
    assert!(r["error"].is_null(), "{r}");
    assert_eq!(r["result"]["recording"], "leave");
    assert_eq!(r["result"]["index"], 1);
    assert_eq!(r["result"]["player"], "player.0");
    // A name the scenario lacks and a mode that is none are refused.
    for (p, want) in [
        (json!({"recording": "depart"}), code::INVALID_PARAMS),
        (json!({"recording": "arrive", "mode": "kill"}), code::INVALID_PARAMS),
        (json!({"recording": "arrive", "player": "player.1"}), code::GONE),
    ] {
        let e = h.err(&mut game, method::RECORDING_PLAY, p.clone());
        assert_eq!(e["code"], want, "{p}: {e}");
    }
}
