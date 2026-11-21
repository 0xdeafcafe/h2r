use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Duration;

use agent_proto::{
    Object,
    sim::{SessionKind, SessionView},
};
use engine::datum::layout;
use engine::testing::FakeGuest;
use engine::view::{fake, objects::Header};
use serde_json::json;

use super::*;
use crate::sim::{self, bots::FakeBots, calls};
use crate::{ConnId, GameStatus, Reader};

const SCENARIO: u32 = 0x8000_0000;
const OBJECTS: u32 = 0x8001_0000;
const STARTS: u32 = 0x8002_0000;
const GAME: u32 = 0x8003_0000;
const WORLD: u32 = 0x8004_0000;
const AI: u32 = 0x8005_0000;
const ACTORS: u32 = 0x8006_0000;
const TAG_INDEX: u32 = 0x8007_0000;
const TAG_ENTRIES: u32 = 0x8008_0000;
const TAG: u32 = 0xa123_0001;
const ACTOR_GLOBAL: u32 = 0x004f_55f0;

struct Game {
    memory: FakeGuest,
    tick: u64,
    map: &'static str,
    mode: &'static str,
}

impl Reader for Game {
    fn status(&mut self) -> GameStatus {
        GameStatus {
            tick: Some(self.tick),
            map: Some(self.map.into()),
            mode: Some(self.mode.into()),
            bsp: Some(0),
        }
    }

    fn objects(&mut self) -> Result<Vec<Object>, String> {
        Ok(Vec::new())
    }

    fn with_guest(&mut self, f: &mut dyn FnMut(&mut dyn GuestMemory)) -> bool {
        f(&mut self.memory);
        true
    }
}

fn game(sim_mode: u32) -> Game {
    let mut memory = FakeGuest::new();
    memory
        .map(SCENARIO, 0x400)
        .map(GAME, 0x1200)
        .map(WORLD, 0x100)
        .put_u32(global::SCENARIO.base, SCENARIO)
        .put_u16(SCENARIO + SCENARIO_TYPE, 1)
        .put_u32(global::GAME_GLOBALS.base, GAME)
        .put_u32(global::SIM_GLOBALS.base, WORLD)
        .put_u32(WORLD + 8, sim_mode)
        .put_u32(global::OBJECTS.base, OBJECTS)
        .put_u32(TAG_LOW, 0x8010_0000)
        .put_u32(TAG_HIGH, 0x8018_0000)
        .put_u32(AI_GLOBALS, 0)
        .put_u32(ACTOR_GLOBAL, 0)
        .put_u32(0x0051_e9d8, 0)
        .put_u32(0x0051_e9dc, 0);
    fake::datums::<Header>(&mut memory, OBJECTS, 8);
    Game {
        memory,
        tick: 90,
        map: "fixture_arena",
        mode: "multiplayer",
    }
}

struct Tags;

impl calls::Tags for Tags {
    fn find(&mut self, tag: &str, among: &[&str]) -> Result<calls::TagRef, String> {
        assert_eq!(among, ["bloc"]);
        let group = match tag {
            "0xa1230001" | "crate" => "bloc",
            "scen:prop" => "scen",
            _ => return Err("unknown fixture tag".into()),
        };
        Ok(calls::TagRef {
            datum: TAG,
            group: group.into(),
            name: Some("fixture".into()),
        })
    }
}

struct Harness {
    agent: Agent,
    conn: ConnId,
    replies: Receiver<String>,
    calls: calls::Shared,
    hashes: &'static HashLog,
}

impl Harness {
    fn new() -> Self {
        let agent = Agent::new();
        let sim = sim::install(
            &agent,
            Box::new(FakeBots::default()),
            Some(Box::new(|m| {
                let sim_mode = m.u32(WORLD + 8).ok();
                SessionView {
                    kind: sim_mode.map_or(SessionKind::Unknown, SessionKind::from_sim_mode),
                    sim_mode,
                    protocol: Some(if matches!(sim_mode, Some(2 | 3)) {
                        "synchronous".into()
                    } else {
                        "distributed".into()
                    }),
                    ..Default::default()
                }
            })),
        );
        let calls = calls::shared();
        calls::lock(&calls).arm();
        sim.set_calls(calls.clone());
        sim.set_tags(Box::new(Tags));
        let hashes = Box::leak(Box::new(HashLog::new(32)));
        install(&agent, sim, hashes);
        let (tx, replies) = sync_channel(8);
        let conn = agent.connect(tx);
        Self {
            agent,
            conn,
            replies,
            calls,
            hashes,
        }
    }

    fn send(&self, method: &str, params: Value) {
        let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        self.agent.submit(self.conn, &request.to_string());
    }

    fn reply(&self) -> Value {
        serde_json::from_str(&self.replies.recv_timeout(Duration::from_secs(1)).unwrap()).unwrap()
    }

    fn call(&self, game: &mut Game, method: &str, params: Value) -> Value {
        self.send(method, params);
        self.agent.on_tick(game);
        self.reply()
    }

    fn ok(&self, game: &mut Game, method: &str, params: Value) -> Value {
        let reply = self.call(game, method, params);
        assert!(reply["error"].is_null(), "{reply}");
        reply["result"].clone()
    }
}

fn block(m: &mut FakeGuest, offset: u32, size: u32, address: u32, count: u32) {
    m.map(address, count * size)
        .put_u32(SCENARIO + offset, count)
        .put_u32(SCENARIO + offset + 4, address);
}

#[test]
fn capacity_reports_arena_pool_counts_caps_and_contiguous_kit_tails() {
    let h = Harness::new();
    let mut game = game(4);
    let m = &mut game.memory;
    let pool = engine::view::Datums::<Header>::at(OBJECTS);
    fake::push(m, &pool);
    fake::push(m, &pool);
    let scenery = 0x8100_0000;
    block(m, 0x50, 0x5c, scenery, 4);
    m.put_u16(scenery, u16::MAX)
        .put_u16(scenery + 2 * 0x5c, u16::MAX)
        .put_u16(scenery + 3 * 0x5c, u16::MAX)
        .put_u32(SCENARIO + 0xa0, 1)
        .put_u32(SCENARIO + 0xa4, 0x8101_0000);
    let result = h.ok(&mut game, method::CAPACITY, Value::Null);
    assert_eq!(result["tag_arena"]["headroom_bytes"], 0x80000);
    assert_eq!(result["object_pool"], json!({"used": 2, "limit": 8}));
    assert_eq!(
        result["blocks"]["scenery"],
        json!({"count": 4, "limit": 2000, "kit_left": 2})
    );
    assert_eq!(
        result["blocks"]["device_groups"],
        json!({"count": 1, "limit": null, "kit_left": null})
    );
    assert_eq!(
        result["blocks"]["netgame_equipment"],
        json!({"count": 0, "limit": 100, "kit_left": 0})
    );
    assert_eq!(result["at"]["boundary"], "tick");
}

#[test]
fn capacity_refuses_bad_params_missing_map_and_invalid_extents() {
    let h = Harness::new();
    let mut game = game(4);
    let r = h.call(&mut game, method::CAPACITY, json!({"extra": 1}));
    assert_eq!(r["error"]["code"], code::INVALID_PARAMS);
    game.memory.put_u32(TAG_HIGH, 1);
    let r = h.call(&mut game, method::CAPACITY, Value::Null);
    assert_eq!(r["error"]["code"], code::UNAVAILABLE);
    game.memory
        .put_u32(TAG_HIGH, 0x8018_0000)
        .put_u32(SCENARIO + 0x50, u32::MAX);
    let r = h.call(&mut game, method::CAPACITY, Value::Null);
    assert_eq!(r["error"]["code"], code::UNAVAILABLE);
    game.mode = "main_menu";
    let r = h.call(&mut game, method::CAPACITY, Value::Null);
    assert_eq!(r["error"]["code"], code::UNAVAILABLE);
}

#[test]
fn kit_reports_candidate_rows_and_runtime_census_without_certifying_them() {
    let h = Harness::new();
    let mut game = game(1);
    let base = h.ok(&mut game, method::PROBE, json!({"what": "kit"}));
    let m = &mut game.memory;
    let names = 0x8100_0000;
    let flags = 0x8101_0000;
    let equipment = 0x8102_0000;
    block(m, 0x48, 0x24, names, 1);
    block(m, 0x118, 0x20, flags, 1);
    block(m, 0x120, 0x90, equipment, 1);
    m.put_u16(names + 0x22, u16::MAX)
        .put_u16(flags + 0x10, u16::MAX)
        .put_u32(equipment + 0x5c, u32::MAX);
    let kit = h.ok(&mut game, method::PROBE, json!({"what": "kit"}));
    for name in ["object_names", "netgame_flags", "netgame_equipment"] {
        assert_eq!(base["blocks"][name]["count"], 0);
        assert_eq!(kit["blocks"][name]["kit_left"], 1);
    }
    assert_eq!(base["objects_by_group"], kit["objects_by_group"]);
    assert_eq!(base["actors"], kit["actors"]);

    let pool = engine::view::Datums::<Header>::at(OBJECTS);
    let (_, object) = fake::push(&mut game.memory, &pool);
    object
        .set_kind(&mut game.memory, sim_state::ObjectKind::Crate as u8)
        .unwrap();
    let changed = h.ok(&mut game, method::PROBE, json!({"what": "kit"}));
    assert_eq!(changed["objects_by_group"]["bloc"], 1);
}

#[test]
fn ai_reports_both_flag_bytes_and_live_actor_count() {
    let h = Harness::new();
    let mut game = game(4);
    let absent = h.ok(&mut game, method::PROBE, json!({"what": "ai"}));
    assert_eq!(absent["actors"], 0);
    assert!(absent["enabled"].is_null());
    assert!(absent["active"].is_null());

    game.memory
        .put_u32(AI_GLOBALS, AI)
        .put_u8(AI, 1)
        .put_u8(AI + 1, 0)
        .put_u32(ACTOR_GLOBAL, ACTORS)
        .put_u32(ACTORS + layout::HIGH_WATER, 5)
        .put_u32(ACTORS + layout::ACTIVE_COUNT, 2);
    let present = h.ok(&mut game, method::PROBE, json!({"what": "ai"}));
    assert_eq!(present["enabled"], true);
    assert_eq!(present["active"], false);
    assert_eq!(present["actors"], 2);
}

#[test]
fn sync_reports_protocol_variant_desync_and_only_hashes_in_the_current_window() {
    let h = Harness::new();
    let mut game = game(2);
    for tick in 88..95 {
        h.hashes.record(tick, u64::from(tick), None);
    }
    h.agent.on_tick(&mut game);
    game.tick = 91;
    let sync = h.ok(&mut game, method::PROBE, json!({"what": "sync", "max": 1}));
    assert_eq!(sync["session"]["kind"], "coop_host");
    assert_eq!(sync["session"]["protocol"], "synchronous");
    assert_eq!(sync["scenario_type"], 1);
    assert_eq!(sync["variant_engine"], 0);
    assert_eq!(sync["desync"], false);
    assert_eq!(
        sync["hashes"]["rows"],
        json!([{"tick": 90, "gameplay": "000000000000005a"}])
    );
    assert_eq!(sync["hashes"]["truncated"], true);

    game.tick = 92;
    game.memory.put_u8(WORLD + DESYNC, 1).put_u32(WORLD + 8, 3);
    let sync = h.ok(
        &mut game,
        method::PROBE,
        json!({"what": "sync", "from_tick": 91}),
    );
    assert_eq!(sync["session"]["kind"], "coop_guest");
    assert_eq!(sync["desync"], true);
    assert_eq!(sync["hashes"]["rows"].as_array().unwrap().len(), 2);
    game.map = "fixture_other";
    game.tick = 93;
    let sync = h.ok(&mut game, method::PROBE, json!({"what": "sync"}));
    assert_eq!(sync["hashes"]["rows"][0]["tick"], 93);
    assert_eq!(sync["hashes"]["rows"].as_array().unwrap().len(), 1);
}

#[test]
fn replication_uses_a_loaded_group_tag_and_waits_for_the_tick_outcome() {
    let h = Harness::new();
    let mut game = game(4);
    let m = &mut game.memory;
    use engine::tags::{entry_layout as entry, index_layout as index};
    m.put_u32(global::TAG_INDEX.base, TAG_INDEX)
        .put_u32(global::TAG_ENTRIES.base, TAG_ENTRIES)
        .put_u32(TAG_INDEX + index::TAG_COUNT, 2)
        .map(TAG_ENTRIES, 2 * entry::STRIDE)
        .put_u32(
            TAG_ENTRIES + entry::STRIDE + entry::GROUP,
            u32::from_be_bytes(*b"bloc"),
        )
        .put_u32(TAG_ENTRIES + entry::STRIDE + entry::DATUM, TAG)
        .put_u32(TAG_ENTRIES + entry::STRIDE + entry::ADDRESS, 0x8100_0000);
    block(m, 0x100, 0x34, STARTS, 2);
    m.put_u16(STARTS + 0x2c, u16::MAX)
        .put_f32(STARTS + 0x34, 8.0)
        .put_f32(STARTS + 0x38, 9.0)
        .put_f32(STARTS + 0x3c, 10.0);
    h.send(
        method::PROBE,
        json!({"what": "replication", "group": "bloc"}),
    );
    h.agent.on_tick(&mut game);
    assert!(h.replies.try_recv().is_err());
    let mut q = calls::lock(&h.calls);
    let work = q.drain();
    assert_eq!(work.len(), 1);
    assert_eq!(
        work[0].1,
        calls::Call::Spawn {
            tag: TAG,
            pos: [8.0, 9.0, 11.0],
            facing: None,
            mp_scenario: Some(SCENARIO),
        }
    );
    q.finish(
        work[0].0,
        Ok(json!({"object": "a5670003", "pos": [8.0, 9.0, 11.0]})),
    );
    drop(q);
    game.tick += 1;
    h.agent.on_tick(&mut game);
    let reply = h.reply();
    let result = &reply["result"];
    assert_eq!(result["what"], "replication", "{reply}");
    assert_eq!(result["tag"]["group"], "bloc");
    assert_eq!(result["object"], "a5670003");
    assert_eq!(result["pos"], json!([8.0, 9.0, 11.0]));
    assert_eq!(result["at"]["tick"], 91);
}

#[test]
fn replication_refuses_wrong_role_group_and_tag_without_queueing() {
    let h = Harness::new();
    let mut game = game(4);
    let ask = json!({"what": "replication", "group": "bloc", "tag": "crate", "pos": [1, 2, 3]});
    for mode in [0, 1, 2, 3, 5, 6] {
        game.memory.put_u32(WORLD + 8, mode);
        let r = h.call(&mut game, method::PROBE, ask.clone());
        assert_eq!(r["error"]["code"], code::INVALID_REQUEST, "{r}");
    }
    game.memory.put_u32(WORLD + 8, 4);
    let r = h.call(
        &mut game,
        method::PROBE,
        json!({"what": "replication", "group": "scnr"}),
    );
    assert_eq!(r["error"]["code"], code::INVALID_PARAMS);
    let mut mismatched = ask;
    mismatched["tag"] = json!("scen:prop");
    let r = h.call(&mut game, method::PROBE, mismatched);
    assert_eq!(r["error"]["code"], code::INVALID_PARAMS);
    assert!(!calls::lock(&h.calls).has_pending());
}
