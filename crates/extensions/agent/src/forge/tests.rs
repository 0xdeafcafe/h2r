use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Duration;

// The base calls are the session's `World` calls.
use ::forge::World as _;

use agent_proto::{
    Object,
    sim::{SessionKind, SessionView},
};
use guest_layout::datum::layout;
use guest_layout::testing::FakeGuest;
use guest_layout::view::{fake, objects::Header};
use serde_json::{Value, json};

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
    let pool = guest_layout::view::Datums::<Header>::at(OBJECTS);
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

    let pool = guest_layout::view::Datums::<Header>::at(OBJECTS);
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
    use guest_layout::tags::{entry_layout as entry, index_layout as index};
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

fn draft() -> forge_doc::ForgeDoc {
    forge_doc::ForgeDoc {
        format: forge_doc::FORMAT,
        author: "Editor".into(),
        title: "Fixture".into(),
        description: String::new(),
        base: forge_doc::MapRef {
            map: "fixture".into(),
            sha256: forge_doc::Sha256::of(b"base"),
        },
        builder: forge_doc::BuilderVersions {
            forge_build: 1,
            pathfind: 1,
            bots: 1,
        },
        sources: vec![],
        recipes: vec![],
        imports: vec![],
        ops: vec![],
        expect: None,
        waves: None,
    }
}

#[test]
fn local_edits_publish_the_engine_handle_only_after_a_confirmed_spawn() {
    use ::forge::{BlockKind, Frame, PaletteItem, Session, TagKey};
    use local::{ForgeWorld, Guard};
    let limits = forge_doc::Limits {
        world: Some(forge_doc::WorldBounds {
            min: [-8.0; 3],
            max: [8.0; 3],
        }),
        base_counts: [(BlockKind::Scenery, 0)].into(),
        ..Default::default()
    };
    let tag = TagKey {
        group: "scen".into(),
        path: "objects\\fixture".into(),
    };
    let session = Session::new(
        draft(),
        limits,
        vec![PaletteItem {
            block: BlockKind::Scenery,
            tag: tag.clone(),
        }],
        3,
    )
    .unwrap();
    let q = calls::shared();
    calls::lock(&q).arm();
    let guard = Guard {
        scenario: SCENARIO,
        generation: 1,
        admitted: |_, _| true,
    };
    let place = || {
        ForgeWorld::apply(
            &session,
            &q,
            guard,
            |requested| {
                assert_eq!(requested, &tag);
                Ok(TAG)
            },
            |s, w| s.place(w, 90, 0, Frame::default()),
        )
        .unwrap()
    };
    let mut edit = place();
    let (ticket, call) = calls::lock(&q).drain().pop().unwrap();
    assert!(
        matches!(call, calls::Call::Forge { steps, .. } if steps == [
            calls::ForgeStep::Make { tag: TAG, frame: Frame::default() },
        ])
    );
    assert!(edit.poll().is_none());
    calls::lock(&q).finish(
        ticket,
        Err(agent_proto::RpcError::new(
            agent_proto::code::UNAVAILABLE,
            "object_new refused",
        )),
    );
    assert_eq!(edit.poll().unwrap().unwrap_err(), "object_new refused");
    assert!(session.log().is_empty());
    let mut edit = place();
    let ticket = calls::lock(&q).drain()[0].0;
    calls::lock(&q).finish(ticket, Ok(json!({"handles":[0xa123_0003u32]})));
    let (updated, id) = edit.poll().unwrap().unwrap();
    assert_eq!(updated.log()[0].op.id, id);
    assert_eq!(updated.objects()[0].handle, 0xa123_0003);
    assert!(edit.poll().is_none());
    let mut batch = ForgeWorld::apply(
        &session,
        &q,
        guard,
        |_| Ok(TAG),
        |s, w| {
            s.place(w, 92, 0, Frame::default())?;
            s.place(
                w,
                92,
                0,
                Frame {
                    position: [2.0; 3],
                    ..Default::default()
                },
            )?;
            Ok(())
        },
    )
    .unwrap();
    let (ticket, call) = calls::lock(&q).drain().pop().unwrap();
    assert!(matches!(call, calls::Call::Forge { steps, .. } if steps.len() == 2));
    calls::lock(&q).finish(
        ticket,
        Ok(json!({"handles":[0xa000_0001u32, 0xa000_0002u32]})),
    );
    let (session, ()) = batch.poll().unwrap().unwrap();
    assert_eq!(
        session
            .objects()
            .iter()
            .map(|o| o.handle)
            .collect::<Vec<_>>(),
        [0xa000_0001, 0xa000_0002]
    );
}

/// Drain the one queued Forge call, answer it with `outcome`, and poll.
fn answer<T>(
    q: &calls::Shared,
    edit: &mut local::ForgeEdit<T>,
    outcome: calls::Outcome,
) -> (Vec<calls::ForgeStep>, Result<(::forge::Session, T), String>) {
    let mut work = calls::lock(q).drain();
    assert_eq!(work.len(), 1);
    let (ticket, call) = work.pop().unwrap();
    let calls::Call::Forge { steps, .. } = call else {
        panic!("expected a Forge call, got {call:?}")
    };
    assert!(
        edit.poll().is_none(),
        "nothing is visible before the game answers"
    );
    calls::lock(q).finish(ticket, outcome);
    (steps, edit.poll().unwrap())
}

/// Stage one local edit over the fixture tag.
fn stage(
    session: &::forge::Session,
    q: &calls::Shared,
    guard: local::Guard,
    edit: impl FnOnce(&mut ::forge::Session, &mut local::ForgeWorld) -> Result<(), String>,
) -> local::ForgeEdit<()> {
    local::ForgeWorld::apply(session, q, guard, |_| Ok(TAG), edit).unwrap()
}

#[test]
fn local_moves_deletes_and_restores_apply_to_the_live_object() {
    use ::forge::{BlockKind, Frame, PaletteItem, Session, TagKey};
    use calls::{ForgeStep, ForgeTarget};
    use local::{ForgeWorld, Guard};
    let limits = forge_doc::Limits {
        world: Some(forge_doc::WorldBounds {
            min: [-8.0; 3],
            max: [8.0; 3],
        }),
        base_counts: [(BlockKind::Scenery, 0)].into(),
        ..Default::default()
    };
    let item = PaletteItem {
        block: BlockKind::Scenery,
        tag: TagKey {
            group: "scen".into(),
            path: "objects\\fixture".into(),
        },
    };
    let base = Session::new(draft(), limits, vec![item], 3).unwrap();
    let q = calls::shared();
    calls::lock(&q).arm();
    let guard = Guard {
        scenario: SCENARIO,
        generation: 1,
        admitted: |_, _| true,
    };
    let (a, b) = (
        Frame {
            position: [1.0, 2.0, 3.0],
            rotation: [0.0; 3],
        },
        Frame {
            position: [4.0, 2.0, 3.0],
            rotation: [0.0; 3],
        },
    );
    let live = |key| {
        ForgeTarget::Live(agent_proto::ObjectId {
            kind: agent_proto::Kind::Scenery,
            key,
        })
    };

    let mut edit =
        ForgeWorld::apply(&base, &q, guard, |_| Ok(TAG), |s, w| s.place(w, 90, 0, a)).unwrap();
    let (_, placed) = answer(&q, &mut edit, Ok(json!({"handles":[0xa123_0003u32]})));
    let (placed, id) = placed.unwrap();

    // A move is one op and one world.object.set on the placed object.
    let mut edit = stage(&placed, &q, guard, |s, w| s.move_object(w, 91, id, b));
    let (steps, moved) = answer(&q, &mut edit, Ok(json!({"handles":[]})));
    assert_eq!(
        steps,
        [ForgeStep::Move {
            target: live(0xa123_0003),
            frame: b
        }]
    );
    let moved = moved.unwrap().0;
    assert_eq!(moved.log().len(), placed.log().len() + 1);
    assert_eq!(moved.document().ops.len(), 2);
    assert_eq!(moved.objects()[0].frame, b);
    assert_eq!(moved.objects()[0].handle, 0xa123_0003);

    // A stale handle is the game's named refusal; nothing is committed.
    let mut edit = stage(&moved, &q, guard, |s, w| s.delete(w, 92, id));
    let gone = agent_proto::RpcError::new(
        agent_proto::sim::REFUSED,
        format!(
            "{}: scenery.a1230003 is no longer in the world",
            local::OBJECT_GONE
        ),
    );
    let (steps, refused) = answer(&q, &mut edit, Err(gone));
    assert_eq!(
        steps,
        [ForgeStep::Delete {
            target: live(0xa123_0003)
        }]
    );
    assert!(refused.unwrap_err().starts_with(local::OBJECT_GONE));
    assert_eq!(moved.document().ops.len(), 2);
    assert_eq!(moved.objects().len(), 1);

    // Delete, then undo restores it through a new object at the same frame.
    let mut edit = stage(&moved, &q, guard, |s, w| s.delete(w, 93, id));
    let (steps, deleted) = answer(&q, &mut edit, Ok(json!({"handles":[]})));
    assert_eq!(
        steps,
        [ForgeStep::Delete {
            target: live(0xa123_0003)
        }]
    );
    let deleted = deleted.unwrap().0;
    assert!(deleted.objects().is_empty());
    let mut edit = stage(&deleted, &q, guard, |s, w| s.undo(w, 94));
    let (steps, restored) = answer(&q, &mut edit, Ok(json!({"handles":[0xa456_0003u32]})));
    assert_eq!(steps, [ForgeStep::Make { tag: TAG, frame: b }]);
    let restored = restored.unwrap().0;
    assert_eq!(restored.objects()[0].id, id);
    assert_eq!(restored.objects()[0].handle, 0xa456_0003);
    assert_eq!(restored.objects()[0].frame, b);
    assert_eq!(restored.document(), moved.document());

    // Undoing the move moves the restored object back; redo moves it again.
    let mut edit = stage(&restored, &q, guard, |s, w| s.undo(w, 95));
    let (steps, unmoved) = answer(&q, &mut edit, Ok(json!({"handles":[]})));
    assert_eq!(
        steps,
        [ForgeStep::Move {
            target: live(0xa456_0003),
            frame: a
        }]
    );
    let unmoved = unmoved.unwrap().0;
    assert_eq!(unmoved.objects()[0].frame, a);
    let mut edit = stage(&unmoved, &q, guard, |s, w| s.redo(w, 96));
    let (steps, redone) = answer(&q, &mut edit, Ok(json!({"handles":[]})));
    assert_eq!(
        steps,
        [ForgeStep::Move {
            target: live(0xa456_0003),
            frame: b
        }]
    );
    let redone = redone.unwrap().0;

    // Replaying the log from the base gives the session's document.
    assert_eq!(
        forge_doc::fold(base.document(), redone.log()).unwrap(),
        *redone.document()
    );
    assert_eq!(redone.document(), moved.document());
    assert!(!calls::lock(&q).has_pending());
}

#[test]
fn a_block_without_live_objects_refuses_a_move_by_name() {
    use ::forge::{BlockKind, Frame, PaletteItem, Session, TagKey};
    use local::{ForgeWorld, Guard};
    let limits = forge_doc::Limits {
        world: Some(forge_doc::WorldBounds {
            min: [-8.0; 3],
            max: [8.0; 3],
        }),
        base_counts: [(BlockKind::SoundScenery, 0)].into(),
        ..Default::default()
    };
    let item = PaletteItem {
        block: BlockKind::SoundScenery,
        tag: TagKey {
            group: "ssce".into(),
            path: "sound\\fixture".into(),
        },
    };
    let session = Session::new(draft(), limits, vec![item], 3).unwrap();
    let q = calls::shared();
    calls::lock(&q).arm();
    let guard = Guard {
        scenario: SCENARIO,
        generation: 1,
        admitted: |_, _| true,
    };
    let mut edit = ForgeWorld::apply(
        &session,
        &q,
        guard,
        |_| Ok(TAG),
        |s, w| s.place(w, 90, 0, Frame::default()),
    )
    .unwrap();
    let (_, placed) = answer(&q, &mut edit, Ok(json!({"handles":[0xa123_0003u32]})));
    let (placed, id) = placed.unwrap();
    let error = ForgeWorld::apply(&placed, &q, guard, |_| Ok(TAG), |s, w| s.delete(w, 91, id))
        .err()
        .unwrap();
    assert!(error.starts_with(local::OBJECT_KIND), "{error}");
    assert!(!calls::lock(&q).has_pending());
    assert_eq!(placed.objects().len(), 1);
}

#[test]
fn local_agent_reads_the_session_and_reports_a_refused_proposal() {
    use ::forge::look::{
        census::Entry,
        monitors::{Looks, Who},
    };
    use std::sync::{Arc, Mutex};
    struct Backend(Arc<Mutex<Looks>>);
    impl local::LocalApi for Backend {
        fn who(&self, m: Option<&dyn guest_layout::GuestMemory>) -> Who {
            Who {
                profile: if m.is_some() { "Chief" } else { "default" }.into(),
                colour: 4,
            }
        }
        fn look(&self, who: &Who, seat: Option<u64>) -> Result<Value, ::forge::look::Refusal> {
            self.0.lock().unwrap().report(None, who, seat)
        }
        fn look_set(&self, who: &Who, params: Value) -> Result<Value, ::forge::look::Refusal> {
            self.0.lock().unwrap().set(None, who, &params)
        }
        fn monitors(&self, who: &Who) -> Value {
            self.0.lock().unwrap().monitors(who, true, None)
        }
        fn status(&self) -> Result<Value, String> {
            Ok(json!({"session":"local"}))
        }
        fn session(&self) -> Result<Arc<::forge::Session>, String> {
            Err("Waiting for a local multiplayer map.".into())
        }
        fn document(&self) -> Result<forge_doc::ForgeDoc, String> {
            Ok(draft())
        }
        fn log(&self, since: u32) -> Result<Vec<forge_doc::Entry>, String> {
            assert_eq!(since, 1);
            Ok(vec![])
        }
        fn input(
            &self,
            m: &dyn guest_layout::GuestMemory,
            input: ::forge::tool::Input,
        ) -> Result<(), String> {
            if m.u32(global::SCENARIO.base).ok() != Some(SCENARIO) {
                return Err("stale Forge session".into());
            }
            assert_eq!(input.action, Some(::forge::tool::Action::Toggle));
            Ok(())
        }
        fn propose(
            &self,
            tick: u32,
            op: forge_doc::Op,
            was: Option<forge_doc::Sha256>,
        ) -> Result<local::ProposalReply, String> {
            assert_eq!(tick, 90);
            assert!(was.is_none());
            assert_eq!(op.edit, forge_doc::Edit::Undo);
            let (tx, rx) = std::sync::mpsc::channel();
            tx.send(Err("there is nothing to undo".into())).unwrap();
            Ok(rx)
        }
    }
    let h = Harness::new();
    let mut game = game(1);
    let widget = Entry {
        key: "widget".into(),
        name: "widget".into(),
        source: "map_a".into(),
        sha256: "a".repeat(64),
        variants: vec!["blue".into(), "red".into()],
        change_colours: true,
        light: false,
        nodes: 4,
        triangles: 100,
    };
    let looks = Arc::new(Mutex::new(Looks::new(vec![widget], None)));
    local::install(&h.agent, Backend(looks.clone()));
    assert_eq!(
        h.ok(&mut game, method::STATUS, Value::Null)["session"],
        "local"
    );
    // The look methods: readable with no session, refused to set, and
    // each refusal carries its code in the error's data.
    let read = h.ok(&mut game, method::LOOK, json!({}));
    assert_eq!(read["session"], false);
    assert_eq!(read["look"]["model"], "marker");
    assert_eq!(read["look"]["primary"], 4);
    assert_eq!(read["catalogue"][0]["key"], "widget");
    assert_eq!(
        h.call(&mut game, method::LOOK, json!({"seat": 1}))["error"]["data"]["code"],
        "monitor_look_invalid"
    );
    assert_eq!(
        h.call(&mut game, method::LOOK, json!({"seats": 1}))["error"]["code"],
        code::INVALID_PARAMS
    );
    let refused = h.call(&mut game, method::LOOK_SET, json!({"tint": 1}));
    assert_eq!(refused["error"]["data"]["code"], "monitor_look_no_session");
    let chief = Who {
        profile: "Chief".into(),
        colour: 4,
    };
    looks.lock().unwrap().open(None, &chief);
    let set = h.ok(
        &mut game,
        method::LOOK_SET,
        json!({"model": "widget", "variant": "red", "tint": 1}),
    );
    assert_eq!(set["look"]["variant"], "red");
    let refused = h.call(&mut game, method::LOOK_SET, json!({"tint": 7}));
    assert_eq!(refused["error"]["data"]["code"], "monitor_look_invalid");
    assert_eq!(refused["error"]["data"]["field"], "tint");
    let monitors = h.ok(&mut game, method::MONITORS, json!({}));
    assert_eq!(monitors["monitors"][0]["name"], "Chief");
    assert_eq!(monitors["monitors"][0]["look"]["model"], "widget");
    assert_eq!(
        h.call(&mut game, method::MONITORS, json!({"x": 1}))["error"]["code"],
        code::INVALID_PARAMS
    );
    assert_eq!(
        h.ok(&mut game, method::DOC, json!({}))["document"],
        json!(draft())
    );
    assert_eq!(
        h.ok(&mut game, method::LOG, json!({"since":1}))["entries"],
        json!([])
    );
    assert_eq!(
        h.ok(&mut game, method::INPUT, json!({"action":"toggle"}))["queued"],
        true
    );
    for bad in [
        json!({}),
        json!({"action":"grant"}),
        json!({"look":[0.,2.]}),
        json!({"action":"toggle","roll":0.5}),
    ] {
        assert_eq!(
            h.call(&mut game, method::INPUT, bad)["error"]["code"],
            code::INVALID_PARAMS
        );
    }
    game.memory.put_u32(global::SCENARIO.base, 0);
    assert_eq!(
        h.call(&mut game, method::INPUT, json!({"action":"toggle"}))["error"]["message"],
        "stale Forge session"
    );
    game.memory.put_u32(global::SCENARIO.base, SCENARIO);
    let op = json!({"op":{"id":1,"edit":"undo"}});
    h.send(method::PROPOSE, op);
    h.agent.on_tick(&mut game);
    game.tick += 1;
    h.agent.on_tick(&mut game);
    assert_eq!(h.reply()["error"]["message"], "there is nothing to undo");
}

#[test]
fn a_teleporter_link_stages_one_channel_write_after_the_objects() {
    use ::forge::{BlockKind, Linkable, Session, Teleporter};
    use calls::ForgeStep;
    let limits = forge_doc::Limits {
        world: Some(forge_doc::WorldBounds {
            min: [-8.0; 3],
            max: [8.0; 3],
        }),
        base_counts: [(BlockKind::NetgameFlags, 2)].into(),
        ..Default::default()
    };
    let flag = |index, sender, channel| Teleporter {
        index,
        position: [index as f32; 3],
        sender,
        channel,
    };
    let session = Session::new(draft(), limits, vec![], 3)
        .unwrap()
        .with_teleporters(vec![flag(0, true, 4), flag(1, false, 6)])
        .unwrap();
    let q = calls::shared();
    calls::lock(&q).arm();
    let guard = local::Guard {
        scenario: SCENARIO,
        generation: 1,
        admitted: |_, _| true,
    };
    let mut edit = stage(&session, &q, guard, |s, w| {
        s.link(w, 5, Linkable::Teleporter(0), Linkable::Teleporter(1))
            .map(|_| ())
    });
    let (steps, result) = answer(&q, &mut edit, Ok(json!({"handles":[]})));
    assert_eq!(
        steps,
        [ForgeStep::Channel {
            flag: 1,
            channel: 4
        }]
    );
    let (session, ()) = result.unwrap();
    assert_eq!(
        session
            .teleporters()
            .iter()
            .map(|t| t.channel)
            .collect::<Vec<_>>(),
        [4, 4]
    );
    // A refused write leaves the document as it was.
    let mut edit = stage(&session, &q, guard, |s, w| s.undo(w, 6));
    let (_, result) = answer(
        &q,
        &mut edit,
        Err(agent_proto::RpcError::new(
            agent_proto::code::INVALID_REQUEST,
            "netgame flag 1 is not a teleporter",
        )),
    );
    assert!(result.unwrap_err().contains("not a teleporter"));
    assert_eq!(session.document().ops.len(), 1);
}

// ---- the base calls (docs/game/forge-map-variants.md, "Applying an edit in a session") ----

/// A session with no objects of its own, to stage base calls over.
fn base_session() -> ::forge::Session {
    let limits = forge_doc::Limits {
        world: Some(forge_doc::WorldBounds {
            min: [-8.0; 3],
            max: [8.0; 3],
        }),
        base_counts: [(::forge::BlockKind::Weapons, 4)].into(),
        ..Default::default()
    };
    ::forge::Session::new(draft(), limits, vec![], 3).unwrap()
}

fn base_guard() -> local::Guard {
    local::Guard {
        scenario: SCENARIO,
        generation: 1,
        admitted: |_, _| true,
    }
}

#[test]
fn each_base_call_stages_one_step_on_the_call_queue() {
    use ::forge::{BlockKind, Frame};
    use base::{BaseStep, FieldValue, RowBytes};
    use calls::ForgeStep;
    let session = base_session();
    let q = calls::shared();
    calls::lock(&q).arm();
    let frame = Frame {
        position: [1.0, 2.0, 3.0],
        rotation: [0.5, 0.0, 0.0],
    };
    let kept = vec![7; map::scenario_layout::block(BlockKind::Weapons).stride as usize];
    let mut edit = stage(&session, &q, base_guard(), |_, w| {
        w.base_move(BlockKind::Weapons, 2, frame)?;
        w.base_field(
            BlockKind::Weapons,
            2,
            "scale",
            &forge_doc::Value::Float(forge_doc::F32::new(2.0).unwrap()),
        )?;
        // A tag key resolves to the datum the game holds when the edit is applied.
        w.base_field(
            BlockKind::Weapons,
            2,
            "palette index",
            &forge_doc::Value::Tag(Some(forge_doc::TagKey {
                group: "weap".into(),
                path: "objects\\fixture".into(),
            })),
        )?;
        w.base_inert(BlockKind::Weapons, 2)?;
        w.base_restore(BlockKind::Weapons, 2, &kept)
    });
    let (steps, result) = answer(&q, &mut edit, Ok(json!({"handles":[]})));
    assert!(result.is_ok());
    let block = BlockKind::Weapons;
    assert_eq!(
        steps,
        [
            ForgeStep::Base(BaseStep::Move {
                block,
                index: 2,
                frame
            }),
            ForgeStep::Base(BaseStep::Field {
                block,
                index: 2,
                field: "scale",
                value: FieldValue::Float(2.0),
            }),
            ForgeStep::Base(BaseStep::Field {
                block,
                index: 2,
                field: "palette index",
                value: FieldValue::Palette { datum: TAG },
            }),
            ForgeStep::Base(BaseStep::Inert { block, index: 2 }),
            ForgeStep::Base(BaseStep::Restore {
                block,
                index: 2,
                row: RowBytes::new(block, &kept).unwrap(),
            }),
        ]
    );
    assert!(!calls::lock(&q).has_pending());
}

#[test]
fn a_base_call_the_layout_does_not_allow_is_refused_while_staging() {
    use ::forge::{BlockKind, Frame};
    let session = base_session();
    let q = calls::shared();
    calls::lock(&q).arm();
    let float = forge_doc::Value::Float(forge_doc::F32::new(1.0).unwrap());
    let refuse = |call: &dyn Fn(&mut local::ForgeWorld) -> Result<(), String>, says: &str| {
        let error =
            local::ForgeWorld::apply(&session, &q, base_guard(), |_| Ok(TAG), |_, w| call(w))
                .err()
                .unwrap();
        assert!(error.contains(says), "{error} does not say {says}");
    };
    refuse(
        &|w| w.base_field(BlockKind::Weapons, 0, "no such field", &float),
        "no field",
    );
    refuse(
        &|w| {
            w.base_field(
                BlockKind::Weapons,
                0,
                "name index",
                &forge_doc::Value::Int(1),
            )
        },
        "read-only",
    );
    refuse(
        &|w| w.base_field(BlockKind::Weapons, 0, "scale", &forge_doc::Value::Int(1)),
        "another kind of value",
    );
    refuse(
        &|w| {
            w.base_field(
                BlockKind::Weapons,
                0,
                "placement flags",
                &forge_doc::Value::Int(-1),
            )
        },
        "another kind of value",
    );
    refuse(&|w| w.base_restore(BlockKind::Weapons, 0, &[0; 3]), "bytes");
    refuse(
        &|w| w.base_move(BlockKind::StartingEquipment, 0, Frame::default()),
        "no position",
    );
    // Nothing was submitted by any of them.
    assert!(!calls::lock(&q).has_pending());
}

#[test]
// Each block lists its 0xff ranges; most have one.
#[allow(clippy::single_range_in_vec_init)]
fn the_inert_row_is_the_kits_spare_form_for_every_block() {
    use ::forge::BlockKind::*;
    // The marks `forge-build`'s `kit.rs` (`Block::entry`) gives a reserved
    // row: the byte ranges that are 0xff; every other byte is zero.
    let marks: &[(::forge::BlockKind, &[std::ops::Range<usize>])] = &[
        (Scenery, &[0..4]),
        (Vehicles, &[0..4]),
        (Equipment, &[0..4]),
        (Weapons, &[0..4]),
        (Machines, &[0..4]),
        (SoundScenery, &[0..4]),
        (Crates, &[0..4]),
        (StartingLocations, &[0x2c..0x2e]),
        (NetgameFlags, &[0x10..0x12]),
        (NetgameEquipment, &[0x58..0x60]),
        (StartingEquipment, &[]),
    ];
    assert_eq!(marks.len(), map::scenario_layout::BLOCKS.len());
    for (block, ranges) in marks {
        let row = base::inert_row(*block);
        assert_eq!(
            row.len(),
            map::scenario_layout::block(*block).stride as usize
        );
        assert!(row.len() <= base::ROW_MAX);
        for (i, byte) in row.iter().enumerate() {
            let marked = ranges.iter().any(|r| r.contains(&i));
            assert_eq!(
                *byte,
                if marked { 0xff } else { 0 },
                "{block:?} byte {i:#x}"
            );
        }
    }
}

// ---- the map's things over the agent API (docs/game/forge-map-variants.md, slice M10) ----

mod things {
    use std::sync::{Arc, Mutex};

    use ::forge::look::{Refusal, monitors::Who};
    use ::forge::{BaseRow, BaseRows, BlockKind, Frame, PaletteItem, Session};
    use agent_proto::forge::{THING_KINDS, refusal};
    use forge_doc::{
        Edit, Endpoint, Entry, EntryOp, F32, FieldWrite, ForgeDoc, Op, OpId, Print, Sha256, TagKey,
        Value,
    };
    use serde_json::{Value as Json, json};

    use super::{Harness, draft, game};
    use crate::forge::local::{self, LocalApi, ProposalReply};
    use agent_proto::{code, forge::method};

    /// A world that takes every call, as the game does at its next tick.
    #[derive(Default)]
    struct Takes {
        next: u32,
        base: Vec<(BlockKind, u32)>,
    }

    impl ::forge::World for Takes {
        fn place(&mut self, _: &PaletteItem, _: Frame) -> Result<u32, String> {
            self.next += 1;
            Ok(self.next)
        }
        fn move_object(&mut self, _: u32, _: Frame) -> Result<(), String> {
            Ok(())
        }
        fn delete(&mut self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn respawn(&mut self, i: &PaletteItem, f: Frame) -> Result<u32, String> {
            self.place(i, f)
        }
        fn base_move(&mut self, block: BlockKind, index: u32, _: Frame) -> Result<(), String> {
            self.base.push((block, index));
            Ok(())
        }
        fn base_field(
            &mut self,
            block: BlockKind,
            index: u32,
            _: &str,
            _: &Value,
        ) -> Result<(), String> {
            self.base.push((block, index));
            Ok(())
        }
        fn base_inert(&mut self, block: BlockKind, index: u32) -> Result<(), String> {
            self.base.push((block, index));
            Ok(())
        }
        fn base_restore(&mut self, block: BlockKind, index: u32, _: &[u8]) -> Result<(), String> {
            self.base.push((block, index));
            Ok(())
        }
    }

    /// The session's host: a session over two weapons of the map's own,
    /// with the crates block left out, applying each proposal at once.
    struct Host(Mutex<Arc<Session>>);

    impl LocalApi for Host {
        fn who(&self, _: Option<&dyn guest_layout::GuestMemory>) -> Who {
            Who {
                profile: "default".into(),
                colour: 0,
            }
        }
        fn look(&self, _: &Who, _: Option<u64>) -> Result<Json, Refusal> {
            unreachable!("the look is not this test's")
        }
        fn look_set(&self, _: &Who, _: Json) -> Result<Json, Refusal> {
            unreachable!("the look is not this test's")
        }
        fn monitors(&self, _: &Who) -> Json {
            json!({"monitors": []})
        }
        fn status(&self) -> Result<Json, String> {
            let s = self.0.lock().unwrap();
            Ok(json!({
                "session": "local",
                "variant": local::variant_status(s.document(), Some("Arena"), !s.log().is_empty()),
            }))
        }
        fn session(&self) -> Result<Arc<Session>, String> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn document(&self) -> Result<ForgeDoc, String> {
            Ok(self.0.lock().unwrap().document().clone())
        }
        fn log(&self, since: u32) -> Result<Vec<Entry>, String> {
            let s = self.0.lock().unwrap();
            Ok(s.log().iter().filter(|e| e.seq > since).cloned().collect())
        }
        fn input(
            &self,
            _: &dyn guest_layout::GuestMemory,
            _: ::forge::tool::Input,
        ) -> Result<(), String> {
            Err("Forge input requires the active local session.".into())
        }
        fn propose(&self, tick: u32, op: Op, was: Option<Sha256>) -> Result<ProposalReply, String> {
            let mut held = self.0.lock().unwrap();
            let mut session = Session::clone(&held);
            let result = session.propose(&mut Takes::default(), tick, op, was);
            if result.is_ok() {
                *held = Arc::new(session);
            }
            let (tx, rx) = std::sync::mpsc::channel();
            tx.send(result).unwrap();
            Ok(rx)
        }
    }

    fn vec3(v: [f32; 3]) -> Value {
        Value::Vec3(v.map(|c| F32::new(c).unwrap()))
    }

    fn weapon_fields(x: f32) -> Vec<Value> {
        vec![
            Value::Tag(Some(TagKey {
                group: "weap".into(),
                path: r"fixture\rifle".into(),
            })),
            Value::Int(-1),
            Value::Int(0),
            vec3([x, 0., 0.]),
            vec3([0., 0., 0.]),
            Value::Float(F32::new(0.).unwrap()),
        ]
    }

    fn weapon(index: u32, x: f32) -> BaseRow {
        let fields = weapon_fields(x);
        BaseRow {
            block: BlockKind::Weapons,
            index,
            raw: vec![
                index as u8 + 1;
                map::scenario_layout::block(BlockKind::Weapons).stride as usize
            ],
            print: forge_doc::print(BlockKind::Weapons, &fields).unwrap(),
            fields,
        }
    }

    fn host() -> Host {
        let limits = forge_doc::Limits {
            world: Some(forge_doc::WorldBounds {
                min: [-8.0; 3],
                max: [8.0; 3],
            }),
            base_counts: [(BlockKind::Weapons, 2), (BlockKind::Crates, 2)].into(),
            ..Default::default()
        };
        let session = Session::new(draft(), limits, vec![], 3)
            .unwrap()
            .with_base(BaseRows {
                rows: vec![weapon(0, -3.), weapon(1, 4.)],
                left_out: vec!["crates: the loaded scenario has 1 rows, the map 2".into()],
            });
        Host(Mutex::new(Arc::new(session)))
    }

    fn set_position(id: u32, target: Endpoint, x: f32) -> Op {
        Op {
            id: OpId(id),
            edit: Edit::Entry(EntryOp::Set {
                target,
                write: FieldWrite {
                    field: "position".into(),
                    value: vec3([x, 0., 0.]),
                },
            }),
        }
    }

    /// Propose `params` and read the reply the tick after.
    fn propose(h: &Harness, game: &mut super::Game, params: Json) -> Json {
        h.send(method::PROPOSE, params);
        h.agent.on_tick(game);
        game.tick += 1;
        h.agent.on_tick(game);
        h.reply()
    }

    #[test]
    fn things_entry_ops_and_the_variant_answer_and_a_refusal_carries_its_name() {
        let h = Harness::new();
        let mut game = game(1);
        local::install(&h.agent, host());
        let methods = h.ok(&mut game, agent_proto::method::METHODS, Json::Null);
        for name in [method::THINGS, method::STATUS] {
            assert!(methods.as_array().unwrap().contains(&json!(name)), "{name}");
        }

        // forge.things: every own entry with its endpoint, print and fields,
        // and the blocks left out.
        let listed = h.ok(&mut game, method::THINGS, Json::Null);
        let things = listed["things"].as_array().unwrap();
        assert_eq!(things.len(), 2);
        let print = forge_doc::print(BlockKind::Weapons, &weapon_fields(-3.)).unwrap();
        let target = Endpoint::Base {
            block: BlockKind::Weapons,
            index: 0,
            print: Some(print),
        };
        assert_eq!(things[0]["id"], json!(target));
        assert_eq!(things[0]["kind"], THING_KINDS[0]);
        assert_eq!(things[0]["block"], "weapons");
        assert_eq!(things[0]["own"], true);
        assert_eq!(things[0]["changed"], false);
        assert_eq!(things[0]["position"], json!([-3., 0., 0.]));
        assert_eq!(things[0]["fields"]["position"], json!(vec3([-3., 0., 0.])));
        assert_eq!(
            listed["left_out"][0],
            "crates: the loaded scenario has 1 rows, the map 2"
        );
        assert_eq!(
            h.ok(&mut game, method::THINGS, json!({"kind": "goal"}))["things"],
            json!([])
        );
        for bad in [json!({"kind": "flag"}), json!({"block": "weapons"})] {
            assert_eq!(
                h.call(&mut game, method::THINGS, bad)["error"]["code"],
                code::INVALID_PARAMS
            );
        }

        // forge.status: the variant, unchanged.
        let status = h.ok(&mut game, method::STATUS, Json::Null);
        assert_eq!(status["variant"]["name"], "Arena");
        assert_eq!(status["variant"]["tier"], "variant");
        assert_eq!(status["variant"]["dirty"], false);
        assert_eq!(
            status["variant"]["id"],
            json!(forge_doc::play_form(&draft()).id().unwrap())
        );

        // forge.propose takes an entry op naming the thing by its endpoint.
        let moved = propose(&h, &mut game, json!({"op": set_position(1, target, -2.)}));
        assert!(moved["error"].is_null(), "{moved}");
        assert_eq!(moved["result"]["entry"]["seq"], 1);
        let changed = h.ok(&mut game, method::THINGS, json!({"changed": true}));
        assert_eq!(changed["things"].as_array().unwrap().len(), 1);
        assert_eq!(changed["things"][0]["id"], json!(target));
        assert_eq!(changed["things"][0]["position"], json!([-2., 0., 0.]));
        let status = h.ok(&mut game, method::STATUS, Json::Null);
        assert_eq!(status["variant"]["dirty"], true);
        assert_eq!(
            status["variant"]["tier"], "variant",
            "entry ops keep it a variant"
        );
        assert!(status["variant"]["id"].is_string(), "{status}");

        // A print that is not the entry's is refused by name, and the log
        // stays where it was.
        let stranger = Endpoint::Base {
            block: BlockKind::Weapons,
            index: 1,
            print: Some(Print([0; 8])),
        };
        let refused = propose(&h, &mut game, json!({"op": set_position(2, stranger, 1.)}));
        assert_eq!(refused["error"]["code"], code::INVALID_REQUEST);
        assert_eq!(
            refused["error"]["data"]["code"],
            refusal::PRINT,
            "{refused}"
        );
        // A `was` that is not the value's.
        let refused = propose(
            &h,
            &mut game,
            json!({"op": set_position(3, target, 0.), "was": Sha256::of(b"another value")}),
        );
        assert_eq!(refused["error"]["data"]["code"], refusal::WAS, "{refused}");
        // An entry of the block left out.
        let crate_entry = Endpoint::Base {
            block: BlockKind::Crates,
            index: 0,
            print: Some(Print([1; 8])),
        };
        let refused = propose(
            &h,
            &mut game,
            json!({"op": set_position(4, crate_entry, 0.)}),
        );
        assert_eq!(
            refused["error"]["data"]["code"],
            refusal::LEFT_OUT,
            "{refused}"
        );
        assert_eq!(
            h.ok(&mut game, method::LOG, Json::Null)["entries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn forge_proposes_and_lists_areas_on_own_and_added_flags() {
        let h = Harness::new();
        let mut game = game(1);
        let block = BlockKind::NetgameFlags;
        let limits = forge_doc::Limits {
            world: Some(forge_doc::WorldBounds {
                min: [-8.0; 3],
                max: [8.0; 3],
            }),
            base_counts: [(block, 4)].into(),
            ..Default::default()
        };
        let rows = [10, 11, 11, 11]
            .into_iter()
            .enumerate()
            .map(|(index, kind)| {
                let fields = vec![
                    vec3([index as f32, 0., 0.]),
                    Value::Float(F32::new(0.).unwrap()),
                    Value::Int(kind),
                    Value::Int(8),
                    Value::Int(0),
                ];
                BaseRow {
                    block,
                    index: index as u32,
                    raw: vec![0; map::scenario_layout::block(block).stride as usize],
                    print: forge_doc::print(block, &fields).unwrap(),
                    fields,
                }
            })
            .collect();
        let session = Session::new(draft(), limits, vec![], 0).unwrap().with_base(BaseRows {
            rows,
            left_out: vec![],
        });
        local::install(&h.agent, Host(Mutex::new(Arc::new(session))));

        let listed = h.ok(&mut game, method::THINGS, json!({"kind": "goal"}));
        let target = &listed["things"][0]["id"];
        let hill = &listed["things"][1]["id"];
        assert_eq!(listed["things"][0].get("area"), Some(&Json::Null));
        let area = json!({"radius":{"radius":"40000000","below":"3f000000","above":"3fc00000"}});
        let edit = |id, target: &Json, area: &Json| {
            json!({"op":{"id":id,"edit":{"entry":{"area":{"target":target,"area":area}}}}})
        };
        let crossing = json!({"path":{
            "points":[["00000000","00000000"],["40800000","40000000"],
                      ["40800000","00000000"],["00000000","3f800000"]],
            "below":"3f000000","above":"3fc00000"
        }});
        let refused = propose(&h, &mut game, edit(1, target, &crossing));
        assert_eq!(refused["error"]["data"]["code"], refusal::REFUSED);
        assert_eq!(
            refused["error"]["message"],
            "The path crosses itself: the edge after point 1 meets the edge after point 3."
        );
        let refused = propose(&h, &mut game, edit(2, hill, &area));
        assert_eq!(refused["error"]["data"]["code"], refusal::REFUSED);
        assert_eq!(refused["error"]["message"], "a hill with corners takes no area");
        let accepted = propose(&h, &mut game, edit(3, target, &area));
        assert_eq!(accepted["result"]["entry"]["seq"], 1, "{accepted}");
        let changed = h.ok(&mut game, method::THINGS, json!({"changed":true}));
        assert_eq!(changed["things"].as_array().unwrap().len(), 1);
        assert_eq!(changed["things"][0]["area"], area);

        let added = propose(&h, &mut game, json!({"op":{"id":4,"edit":{"placement":{"add":{
            "block":"netgame_flags","fields":[{"field":"type","value":{"int":12}}]
        }}}}}));
        assert_eq!(added["result"]["entry"]["seq"], 2, "{added}");
        let added_id = json!({"added":4});
        let accepted = propose(&h, &mut game, edit(5, &added_id, &area));
        assert_eq!(accepted["result"]["entry"]["seq"], 3, "{accepted}");
        let cleared = propose(&h, &mut game, edit(6, target, &Json::Null));
        assert_eq!(cleared["result"]["entry"]["seq"], 4, "{cleared}");
        let changed = h.ok(&mut game, method::THINGS, json!({"kind":"goal","changed":true}));
        assert_eq!(changed["things"].as_array().unwrap().len(), 1);
        assert_eq!(changed["things"][0]["id"], added_id);
        assert_eq!(changed["things"][0]["area"], area);

        // Shown in play: a flag of the map is not, a flag the document
        // added is, and the op flips each.
        let flag = |things: &Json, id: &Json| {
            things["things"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| &t["id"] == id)
                .unwrap()
                .clone()
        };
        let listed = h.ok(&mut game, method::THINGS, json!({"kind": "goal"}));
        assert_eq!(flag(&listed, target)["show"], false);
        assert_eq!(flag(&listed, &added_id)["show"], true);
        let show = |id, target: &Json, on| {
            json!({"op":{"id":id,"edit":{"entry":{"show":{"target":target,"on":on}}}}})
        };
        let accepted = propose(&h, &mut game, show(7, target, true));
        assert_eq!(accepted["result"]["entry"]["seq"], 5, "{accepted}");
        let accepted = propose(&h, &mut game, show(8, &added_id, false));
        assert_eq!(accepted["result"]["entry"]["seq"], 6, "{accepted}");
        let listed = h.ok(&mut game, method::THINGS, json!({"kind": "goal"}));
        assert_eq!(flag(&listed, target)["show"], true);
        assert_eq!(flag(&listed, target)["changed"], true);
        assert_eq!(flag(&listed, &added_id)["show"], false);
    }

    #[test]
    fn each_refusal_sentence_has_its_name() {
        for (sentence, name) in [
            (
                format!(
                    "{}: crate.0000001f is no longer in the world",
                    local::OBJECT_GONE
                ),
                refusal::OBJECT_GONE,
            ),
            (
                format!(
                    "{}: live sound scenery objects cannot be moved",
                    local::OBJECT_KIND
                ),
                refusal::OBJECT_KIND,
            ),
            (
                format!(
                    "{}: the weapons 2 object is held or attached",
                    local::OBJECT_HELD
                ),
                refusal::OBJECT_HELD,
            ),
            (
                forge_doc::Refusal::Print {
                    block: BlockKind::NetgameFlags,
                    index: 12,
                }
                .to_string(),
                refusal::PRINT,
            ),
            (forge_doc::Refusal::Mismatch.to_string(), refusal::WAS),
            (
                format!("The map's crates entries {}.", ::forge::LEFT_OUT_SENTENCE),
                refusal::LEFT_OUT,
            ),
            ("Forge edit log is full.".to_owned(), refusal::REFUSED),
        ] {
            assert_eq!(local::refusal_name(&sentence), name, "{sentence}");
        }
    }

    // ---- a shared session over the agent API (docs/game/forge-design.md, F5) ----

    mod shared {
        use std::sync::{Arc, Mutex};

        use ::forge::SessionAdmit;
        use ::forge::net::{
            Admit, Beat, DocOnly, HOST_SEAT, Lane, NetRefusal, Node, Role, Seat, Step,
        };

        use super::*;
        use crate::forge::net::{self as shared_net, Replies};

        #[derive(Default)]
        struct Pipe {
            bytes: Vec<u8>,
            beat: Option<Beat>,
        }

        /// One end of the session's reliable ordered channel.
        #[derive(Clone)]
        struct Wire {
            out: Arc<Mutex<Pipe>>,
            inn: Arc<Mutex<Pipe>>,
        }

        fn wires() -> (Wire, Wire) {
            let (a, b) = (Arc::default(), Arc::default());
            (
                Wire {
                    out: Arc::clone(&a),
                    inn: Arc::clone(&b),
                },
                Wire { out: b, inn: a },
            )
        }

        impl Lane for Wire {
            fn send(&mut self, bytes: &[u8]) -> bool {
                self.out.lock().unwrap().bytes.extend_from_slice(bytes);
                true
            }
            fn recv(&mut self) -> Vec<u8> {
                std::mem::take(&mut self.inn.lock().unwrap().bytes)
            }
            fn set_beat(&mut self, beat: Option<Beat>) {
                self.out.lock().unwrap().beat = beat;
            }
            fn beat(&self) -> Option<Beat> {
                self.inn.lock().unwrap().beat
            }
        }

        struct Mach<A: Admit> {
            node: Node<A, Wire>,
            session: Session,
            world: Takes,
            name: &'static str,
        }

        impl<A: Admit> Mach<A> {
            fn step(&mut self, tick: u32) -> Step {
                let out = self.session.take_outgoing();
                if !out.is_empty() {
                    self.node.submit(out).unwrap();
                }
                let step = self.node.tick(tick).unwrap();
                if let Some(state) = &step.joined {
                    let seat = self.node.seat().unwrap();
                    self.session = self.session.clone().share(seat, self.name);
                    self.session.sync_to(&mut self.world, state).unwrap();
                }
                for item in &step.items {
                    self.session.apply_item(&mut self.world, item).unwrap();
                }
                step
            }
        }

        fn limits() -> forge_doc::Limits {
            forge_doc::Limits {
                world: Some(forge_doc::WorldBounds {
                    min: [-8.0; 3],
                    max: [8.0; 3],
                }),
                base_counts: [(BlockKind::Weapons, 2), (BlockKind::Crates, 2)].into(),
                ..Default::default()
            }
        }

        fn session() -> Session {
            Arc::try_unwrap(host().0.into_inner().unwrap()).unwrap()
        }

        /// The host machine, one guest, and the proposals the guest has
        /// sent to the agent API.
        struct Machines {
            host: Mach<SessionAdmit>,
            guest: Mach<DocOnly>,
            replies: Replies,
            tick: u32,
        }

        impl Machines {
            fn new() -> Self {
                let host_session = session().share(HOST_SEAT, "Host");
                let mut host = Mach {
                    node: Node::host(
                        host_session.document().clone(),
                        limits(),
                        host_session.admit(),
                        "Host",
                        0,
                        ::forge::net::APPLY_DELAY,
                    )
                    .unwrap(),
                    session: host_session,
                    world: Takes::default(),
                    name: "Host",
                };
                let (to_host, to_guest) = wires();
                host.node.accept(Seat(1), to_guest, Role::Editor).unwrap();
                let guest = Mach {
                    node: Node::guest(limits(), "Guest", forge::look::session::CanDraw::default(), to_host).unwrap(),
                    session: session(),
                    world: Takes::default(),
                    name: "Guest",
                };
                Self {
                    host,
                    guest,
                    replies: Replies::default(),
                    tick: 0,
                }
            }

            fn step(&mut self) {
                self.tick += 1;
                self.host.step(self.tick);
                self.guest.step(self.tick);
                let settled = self.guest.node.take_settled();
                self.replies.settle(settled);
            }
        }

        /// The agent on the guest machine.
        struct NetApi(Arc<Mutex<Machines>>);

        impl LocalApi for NetApi {
            fn who(&self, _: Option<&dyn guest_layout::GuestMemory>) -> Who {
                Who {
                    profile: "default".into(),
                    colour: 0,
                }
            }
            fn look(&self, _: &Who, _: Option<u64>) -> Result<Json, Refusal> {
                unreachable!("the look is not this test's")
            }
            fn look_set(&self, _: &Who, _: Json) -> Result<Json, Refusal> {
                unreachable!("the look is not this test's")
            }
            fn monitors(&self, _: &Who) -> Json {
                json!({"monitors": []})
            }
            fn status(&self) -> Result<Json, String> {
                let m = self.0.lock().unwrap();
                Ok(shared_net::status(
                    json!({"session": "local"}),
                    &m.guest.node.report(),
                ))
            }
            fn session(&self) -> Result<Arc<Session>, String> {
                Ok(Arc::new(self.0.lock().unwrap().guest.session.clone()))
            }
            fn document(&self) -> Result<ForgeDoc, String> {
                Ok(self.0.lock().unwrap().guest.session.document().clone())
            }
            fn log(&self, since: u32) -> Result<Vec<Entry>, String> {
                Ok(self.0.lock().unwrap().guest.node.log_since(since))
            }
            fn input(
                &self,
                _: &dyn guest_layout::GuestMemory,
                _: ::forge::tool::Input,
            ) -> Result<(), String> {
                Err("Forge input requires the active local session.".into())
            }
            fn propose(&self, _: u32, op: Op, was: Option<Sha256>) -> Result<ProposalReply, String> {
                let mut m = self.0.lock().unwrap();
                let nonce = m.guest.node.propose(op, was).map_err(|e| e.named())?;
                Ok(m.replies.wait_for(nonce))
            }
            fn wait_ticks(&self) -> u64 {
                shared_net::WAIT_TICKS
            }
        }

        /// Send `params` and step the machines and the game until the
        /// agent answers.
        fn ask(
            h: &Harness,
            game: &mut super::super::Game,
            net: &Arc<Mutex<Machines>>,
            name: &str,
            params: Json,
        ) -> Json {
            h.send(name, params);
            for _ in 0..400 {
                net.lock().unwrap().step();
                game.tick += 1;
                h.agent.on_tick(game);
                if let Ok(raw) = h.replies.try_recv() {
                    return serde_json::from_str(&raw).unwrap();
                }
            }
            panic!("{name} was not answered");
        }

        fn weapon_target() -> Endpoint {
            Endpoint::Base {
                block: BlockKind::Weapons,
                index: 0,
                print: Some(forge_doc::print(BlockKind::Weapons, &weapon_fields(-3.)).unwrap()),
            }
        }

        #[test]
        fn status_log_and_propose_work_over_a_network_session() {
            let h = Harness::new();
            let mut game = game(2);
            let net = Arc::new(Mutex::new(Machines::new()));
            local::install(&h.agent, NetApi(net.clone()));
            for _ in 0..12 {
                net.lock().unwrap().step();
            }

            // forge.status: the session is shared, and says whose seat this is.
            let status = ask(&h, &mut game, &net, method::STATUS, Json::Null)["result"].clone();
            assert_eq!(status["session"], "shared");
            assert_eq!(status["role"], "editor");
            let report = &status["net"];
            assert_eq!(
                (report["host"].clone(), report["seat"].clone(), report["name"].clone()),
                (json!(false), json!(1), json!("Guest"))
            );
            assert_eq!(report["members"].as_array().unwrap().len(), 2);
            assert_eq!(report["members"][0]["name"], "Host");
            assert_eq!(report["head"]["seq"], 0);
            assert!(report["ended"].is_null());

            // forge.propose goes to the host and answers with the entry the
            // stream carried, once this machine has applied it.
            let target = weapon_target();
            let moved = ask(
                &h,
                &mut game,
                &net,
                method::PROPOSE,
                json!({"op": set_position(2, target, -2.)}),
            );
            assert!(moved["error"].is_null(), "{moved}");
            assert_eq!(moved["result"]["entry"]["seq"], 1);
            assert_eq!(moved["result"]["entry"]["author"], "Guest");

            // Every machine has it, and forge.log and forge.things say so.
            {
                let m = net.lock().unwrap();
                assert_eq!(m.host.session.document(), m.guest.session.document());
                assert_eq!(m.host.session.head(), m.guest.session.head());
            }
            let log = ask(&h, &mut game, &net, method::LOG, json!({"since": 0}));
            assert_eq!(log["result"]["entries"].as_array().unwrap().len(), 1);
            assert_eq!(log["result"]["entries"][0]["author"], "Guest");
            let log = ask(&h, &mut game, &net, method::LOG, json!({"since": 1}));
            assert_eq!(log["result"]["entries"], json!([]));
            let changed = ask(&h, &mut game, &net, method::THINGS, json!({"changed": true}));
            assert_eq!(changed["result"]["things"][0]["position"], json!([-2., 0., 0.]));
            let status = ask(&h, &mut game, &net, method::STATUS, Json::Null)["result"].clone();
            assert_eq!(status["net"]["head"]["seq"], 1);
            assert_eq!(status["net"]["in_flight"], 0);

            // The host's refusals keep their names.
            let refused = ask(
                &h,
                &mut game,
                &net,
                method::PROPOSE,
                json!({"op": set_position(3, target, 1.)}),
            );
            assert_eq!(refused["error"]["code"], code::INVALID_REQUEST);
            assert_eq!(refused["error"]["data"]["code"], refusal::LANE, "{refused}");
            let refused = ask(
                &h,
                &mut game,
                &net,
                method::PROPOSE,
                json!({"op": set_position(18, target, 0.), "was": Sha256::of(b"another value")}),
            );
            assert_eq!(refused["error"]["data"]["code"], refusal::WAS, "{refused}");
            net.lock()
                .unwrap()
                .host
                .node
                .set_role(Seat(1), Role::Player)
                .unwrap();
            let refused = ask(
                &h,
                &mut game,
                &net,
                method::PROPOSE,
                json!({"op": set_position(18, target, 0.)}),
            );
            assert_eq!(refused["error"]["data"]["code"], refusal::ROLE, "{refused}");
            for _ in 0..8 {
                net.lock().unwrap().step();
            }
            let status = ask(&h, &mut game, &net, method::STATUS, Json::Null)["result"].clone();
            assert_eq!(status["role"], "player");

            // Nothing was added to the log by any of them.
            assert_eq!(net.lock().unwrap().host.node.state().unwrap().log().len(), 1);
        }

        #[test]
        fn a_proposal_before_the_session_has_joined_is_refused_by_name() {
            let h = Harness::new();
            let mut game = game(2);
            let net = Arc::new(Mutex::new(Machines::new()));
            local::install(&h.agent, NetApi(net.clone()));
            // No tick has run: the welcome has not come.
            h.send(method::PROPOSE, json!({"op": set_position(2, weapon_target(), 1.)}));
            h.agent.on_tick(&mut game);
            let reply = h.reply();
            assert_eq!(reply["error"]["data"]["code"], refusal::NOT_JOINED, "{reply}");
        }

        #[test]
        fn every_net_refusal_keeps_its_name_through_the_api() {
            for r in [
                NetRefusal::NotJoined,
                NetRefusal::Role { role: Role::Player },
                NetRefusal::Held { by: "A".into() },
                NetRefusal::NotHolding,
                NetRefusal::Stale { proposed: 1, head: 2 },
                NetRefusal::Ahead { proposed: 3, head: 2 },
                NetRefusal::Lane { id: OpId(3) },
                NetRefusal::Doc(forge_doc::Refusal::Mismatch),
                NetRefusal::Doc(forge_doc::Refusal::DuplicateOp(OpId(1))),
                NetRefusal::Map("Object budget exceeded (2 / 1).".into()),
                NetRefusal::Seat,
                NetRefusal::Name,
                NetRefusal::HeadDiffers { seat: Seat(1), seq: 1 },
                NetRefusal::Clock { now: 1, was: 2 },
            ] {
                assert!(refusal::SHARED.contains(&r.code()), "{r:?}");
                assert_eq!(local::refusal_name(&r.named()), r.code(), "{r:?}");
            }
        }

        #[test]
        fn proposals_waiting_when_the_session_ends_are_refused() {
            let mut replies = Replies::default();
            let one = replies.wait_for(1);
            let two = replies.wait_for(2);
            replies.settle(vec![::forge::net::Settled {
                nonce: 1,
                result: Err("forge_held: A is holding that.".into()),
            }]);
            assert_eq!(one.try_recv().unwrap().unwrap_err(), "forge_held: A is holding that.");
            assert_eq!(replies.len(), 1);
            replies.end("The host left.");
            assert!(two.try_recv().unwrap().unwrap_err().starts_with("forge_not_joined: "));
            assert!(replies.is_empty());
        }
    }
}

// ---- the AI ground over the agent API (docs/game/forge-map-variants.md, "AI ground") ----

mod ground {
    use std::sync::{Arc, Mutex};

    use ::forge::look::{Refusal, monitors::Who};
    use ::forge::{
        BlockKind, Frame, GroundBuilder, GroundClass, GroundClassCost, GroundLabel, GroundProblem,
        GroundReceipt, GroundRecipe, GroundReport, PaletteItem, Session,
    };
    use agent_proto::forge::method;
    use forge_doc::{Entry, ForgeDoc, Op, Sha256};
    use serde_json::{Value as Json, json};

    use super::{Harness, draft, game};
    use crate::forge::ground::NO_BUILDER;
    use crate::forge::local::{self, GroundReply, LocalApi, ProposalReply};

    /// A world that is never asked: a marker places nothing.
    struct Quiet;

    impl ::forge::World for Quiet {
        fn place(&mut self, _: &PaletteItem, _: Frame) -> Result<u32, String> {
            unreachable!("a marker places nothing")
        }
        fn respawn(&mut self, _: &PaletteItem, _: Frame) -> Result<u32, String> {
            unreachable!("a marker places nothing")
        }
        fn move_object(&mut self, _: u32, _: Frame) -> Result<(), String> {
            unreachable!("a marker places nothing")
        }
        fn delete(&mut self, _: u32) -> Result<(), String> {
            unreachable!("a marker places nothing")
        }
    }

    /// A builder that makes an invented report, or refuses.
    struct Kit {
        refuse: bool,
    }

    impl GroundBuilder for Kit {
        fn recipe(&self) -> GroundRecipe {
            GroundRecipe {
                kit_version: 3,
                home: 1,
                probe: false,
                classes: vec![],
            }
        }

        /// Every class from one invented donor.
        fn classes(&mut self, names: &[String]) -> Result<Vec<GroundClass>, String> {
            Ok(names
                .iter()
                .map(|name| GroundClass {
                    name: name.clone(),
                    donor: forge_doc::MapRef {
                        map: "outskirts".into(),
                        sha256: Sha256([0x22; 32]),
                    },
                })
                .collect())
        }

        /// The map keeps the first class asked for and cuts the rest.
        fn build(&mut self, play: &ForgeDoc) -> Result<GroundReport, String> {
            if self.refuse {
                return Err("No room: the sectors take 9 KB too many.".into());
            }
            let names = play
                .ground_recipe()
                .ok()
                .flatten()
                .map(|r| r.class_names())
                .unwrap_or_default();
            let keep = names.len().min(1);
            Ok(GroundReport {
                classes: std::iter::once("elite".to_owned())
                    .chain(names[..keep].iter().cloned())
                    .collect(),
                cut: names[keep..].iter().rev().cloned().collect(),
                costs: names[..keep]
                    .iter()
                    .map(|name| GroundClassCost {
                        name: name.clone(),
                        tags: 3,
                        bytes: 0x400,
                    })
                    .collect(),
                zones: 2,
                squads: 16,
                sectors: Vec::new(),
                problems: vec![GroundProblem {
                    label: "ff_drop".into(),
                    position: [1., 2., 0.],
                    reason: "off the walkable area".into(),
                }],
                receipt: Some(GroundReceipt {
                    base: "aa11".into(),
                    labels: "bb22".into(),
                    kit_version: 3,
                }),
            })
        }
    }

    /// A session host that builds with [`Kit`] when `builds`, as the game's
    /// does with its own builder, and answers at once.
    struct Host {
        session: Mutex<Arc<Session>>,
        builds: Option<bool>,
    }

    impl LocalApi for Host {
        fn who(&self, _: Option<&dyn guest_layout::GuestMemory>) -> Who {
            Who {
                profile: "default".into(),
                colour: 0,
            }
        }
        fn look(&self, _: &Who, _: Option<u64>) -> Result<Json, Refusal> {
            unreachable!("the look is not this test's")
        }
        fn look_set(&self, _: &Who, _: Json) -> Result<Json, Refusal> {
            unreachable!("the look is not this test's")
        }
        fn monitors(&self, _: &Who) -> Json {
            json!({"monitors": []})
        }
        fn status(&self) -> Result<Json, String> {
            Ok(json!({"session": "local", "variant": {"title": "Fixture"}}))
        }
        fn session(&self) -> Result<Arc<Session>, String> {
            Ok(self.session.lock().unwrap().clone())
        }
        fn set_waves(&self, set: Option<forge_doc::waves::Def>) -> Result<Json, String> {
            let mut held = self.session.lock().unwrap();
            let mut session = Session::clone(&held);
            session.set_waves(set).map_err(|e| e.to_string())?;
            let reply = crate::forge::waves::reply(&session);
            *held = Arc::new(session);
            Ok(reply)
        }
        fn document(&self) -> Result<ForgeDoc, String> {
            Ok(self.session.lock().unwrap().document().clone())
        }
        fn log(&self, _: u32) -> Result<Vec<Entry>, String> {
            Ok(vec![])
        }
        fn input(
            &self,
            _: &dyn guest_layout::GuestMemory,
            _: ::forge::tool::Input,
        ) -> Result<(), String> {
            Err("Forge input requires the active local session.".into())
        }
        fn propose(&self, _: u32, _: Op, _: Option<Sha256>) -> Result<ProposalReply, String> {
            Err("not this test's".into())
        }
        fn ground(&self) -> Result<Json, String> {
            Ok(crate::forge::ground::reply(
                &*self.session()?,
                crate::forge::ground::Facts {
                    available: self.builds.is_some(),
                    ..Default::default()
                },
            ))
        }
        fn ground_build(
            &self,
            if_stale: bool,
            classes: Option<Vec<String>>,
        ) -> Result<GroundReply, String> {
            let refuse = self.builds.ok_or(NO_BUILDER)?;
            let mut held = self.session.lock().unwrap();
            let mut session = Session::clone(&held);
            if let Some(classes) = classes {
                session.set_ground_classes(&classes)?;
            }
            let mut kit = Kit { refuse };
            let result = if if_stale {
                session.ensure_ground(&mut kit)
            } else {
                session.build_ground(&mut kit).map(|_| true)
            }
            .map(|built| {
                let mut reply = crate::forge::ground::reply(
                    &session,
                    crate::forge::ground::Facts {
                        available: true,
                        ..Default::default()
                    },
                );
                reply["built"] = json!(built);
                *held = Arc::new(session);
                reply
            });
            let (tx, rx) = std::sync::mpsc::channel();
            tx.send(result).unwrap();
            Ok(rx)
        }
    }

    fn host(builds: Option<bool>, markers: usize) -> Host {
        let limits = forge_doc::Limits {
            world: Some(forge_doc::WorldBounds {
                min: [-8.0; 3],
                max: [8.0; 3],
            }),
            base_counts: [(BlockKind::NetgameFlags, 2)].into(),
            ..Default::default()
        };
        let mut session = Session::new(draft(), limits, vec![], 3).unwrap();
        for i in 0..markers {
            session
                .place_marker(
                    &mut Quiet,
                    1 + i as u32,
                    GroundLabel::Drop,
                    Frame {
                        position: [1. + i as f32, 2., 0.],
                        rotation: [0.5, 0., 0.],
                    },
                    0,
                    None,
                )
                .unwrap();
        }
        Host {
            session: Mutex::new(Arc::new(session)),
            builds,
        }
    }

    /// Send `params` and read the reply the tick after.
    fn ask(h: &Harness, game: &mut super::Game, method: &str, params: Json) -> Json {
        h.send(method, params);
        h.agent.on_tick(game);
        game.tick += 1;
        h.agent.on_tick(game);
        h.reply()
    }

    #[test]
    fn the_ground_before_any_build_has_its_markers_and_no_report() {
        let h = Harness::new();
        let mut game = game(1);
        local::install(&h.agent, host(None, 2));
        let methods = h.ok(&mut game, agent_proto::method::METHODS, Json::Null);
        for name in [method::GROUND, method::GROUND_BUILD] {
            assert!(methods.as_array().unwrap().contains(&json!(name)), "{name}");
        }

        let ground = h.ok(&mut game, method::GROUND, Json::Null);
        assert_eq!(ground["state"], "not_built");
        assert_eq!(ground["available"], false);
        assert_eq!(ground["building"], false);
        assert_eq!(ground["recipe"], Json::Null);
        assert_eq!(ground["report"], Json::Null);
        assert_eq!(ground["problems"], json!([]));
        assert_eq!(ground["counts"]["ff_drop"], 2);
        assert_eq!(ground["counts"]["siege_site"], 0);
        let markers = ground["markers"].as_array().unwrap();
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0]["label"], "ff_drop");
        assert_eq!(markers[0]["position"], json!([1., 2., 0.]));
        assert_eq!(markers[0]["facing"], 0.5);
        assert_eq!(markers[0]["team"], 0);

        // The call takes no parameters.
        let bad = h.call(&mut game, method::GROUND, json!({"extra": 1}));
        assert!(!bad["error"].is_null(), "{bad}");
    }

    #[test]
    fn a_build_answers_with_the_report_its_problems_and_its_receipt() {
        let h = Harness::new();
        let mut game = game(1);
        local::install(&h.agent, host(Some(false), 1));

        let built = ask(&h, &mut game, method::GROUND_BUILD, Json::Null);
        assert!(built["error"].is_null(), "{built}");
        let built = &built["result"];
        assert_eq!(built["built"], true);
        assert_eq!(built["state"], "current");
        assert_eq!(
            built["recipe"],
            json!({"kit_version": 3, "home": 1, "probe": false, "classes": []})
        );
        assert_eq!(built["classes"], json!([]));
        assert_eq!(built["report"]["classes"], json!(["elite"]));
        assert_eq!(built["report"]["cut"], json!([]));
        assert_eq!(built["report"]["costs"], json!([]));
        assert_eq!(built["report"]["zones"], 2);
        assert_eq!(built["report"]["squads"], 16);
        assert_eq!(built["report"]["problems"], 1);
        assert_eq!(
            built["report"]["receipt"],
            json!({"base": "aa11", "labels": "bb22", "kit_version": 3})
        );
        assert_eq!(built["problems"][0]["label"], "ff_drop");
        assert_eq!(built["problems"][0]["reason"], "off the walkable area");

        // The read sees the same, and the session's build is what it reads.
        let ground = h.ok(&mut game, method::GROUND, Json::Null);
        assert_eq!(ground["state"], "current");
        assert_eq!(ground["available"], true);
        assert_eq!(ground["problems"], built["problems"]);

        // Test waves: current ground is not built again, and a build
        // that is asked for is.
        let again = ask(
            &h,
            &mut game,
            method::GROUND_BUILD,
            json!({"if_stale": true}),
        );
        assert_eq!(again["result"]["built"], false);
        let forced = ask(
            &h,
            &mut game,
            method::GROUND_BUILD,
            json!({"if_stale": false}),
        );
        assert_eq!(forced["result"]["built"], true);
    }

    #[test]
    fn a_build_with_classes_reports_them_kept_and_cut_with_costs() {
        let h = Harness::new();
        let mut game = game(1);
        local::install(&h.agent, host(Some(false), 1));
        let built = ask(
            &h,
            &mut game,
            method::GROUND_BUILD,
            json!({"classes": ["jackal", "grunt"]}),
        );
        assert!(built["error"].is_null(), "{built}");
        let built = &built["result"];
        assert_eq!(built["built"], true);
        // Asked for in keep order, the donors in the recipe.
        assert_eq!(built["classes"], json!(["grunt", "jackal"]));
        let hash = Sha256([0x22; 32]).to_string();
        assert_eq!(
            built["recipe"]["classes"],
            json!([
                {"name": "grunt", "donor": "outskirts", "sha256": hash},
                {"name": "jackal", "donor": "outskirts", "sha256": hash},
            ])
        );
        assert_eq!(built["report"]["classes"], json!(["elite", "grunt"]));
        assert_eq!(built["report"]["cut"], json!(["jackal"]));
        assert_eq!(
            built["report"]["costs"],
            json!([{"name": "grunt", "tags": 3, "bytes": 0x400}])
        );
        // The same list again is current: Test waves builds nothing.
        let again = ask(
            &h,
            &mut game,
            method::GROUND_BUILD,
            json!({"if_stale": true, "classes": ["grunt", "jackal"]}),
        );
        assert_eq!(again["result"]["built"], false);
        // A name the table lacks is refused and changes nothing.
        let bad = ask(
            &h,
            &mut game,
            method::GROUND_BUILD,
            json!({"classes": ["marine"]}),
        );
        assert_eq!(bad["error"]["data"]["code"], "forge_refused", "{bad}");
        let ground = h.ok(&mut game, method::GROUND, Json::Null);
        assert_eq!(ground["state"], "current");
        assert_eq!(ground["classes"], json!(["grunt", "jackal"]));
    }

    #[test]
    fn a_build_that_fails_or_has_no_builder_is_refused_by_name() {
        let h = Harness::new();
        let mut g = game(1);
        local::install(&h.agent, host(Some(true), 1));
        let failed = ask(&h, &mut g, method::GROUND_BUILD, Json::Null);
        assert_eq!(failed["error"]["data"]["code"], "forge_refused");
        assert!(
            failed["error"]["message"]
                .as_str()
                .unwrap()
                .contains("No room"),
            "{failed}"
        );
        let ground = h.ok(&mut g, method::GROUND, Json::Null);
        assert_eq!(
            ground["state"], "not_built",
            "a failed build changes nothing"
        );

        let h = Harness::new();
        let mut game = game(1);
        local::install(&h.agent, host(None, 1));
        let none = h.call(&mut game, method::GROUND_BUILD, Json::Null);
        assert_eq!(none["error"]["message"], NO_BUILDER);
        let bad = h.call(&mut game, method::GROUND_BUILD, json!({"unknown": true}));
        assert!(!bad["error"].is_null(), "{bad}");
    }

    #[test]
    fn nav_before_any_generation_is_not_built_and_generating_needs_a_generator() {
        let h = Harness::new();
        let mut game = game(1);
        local::install(&h.agent, host(None, 0));
        let methods = h.ok(&mut game, agent_proto::method::METHODS, Json::Null);
        assert!(methods.as_array().unwrap().contains(&json!(method::NAV)));

        let nav = h.ok(&mut game, method::NAV, Json::Null);
        assert_eq!(nav["state"], "not_built");
        assert_eq!(nav["available"], false);
        assert_eq!(nav["counts"], Json::Null);
        assert_eq!(nav["problems"], json!([]));
        assert_eq!(nav["overlay"], json!([]));
        let read = h.ok(&mut game, method::NAV, json!({"generate": false}));
        // `at` carries the tick counters, which move with every call.
        let (mut read, mut nav) = (read, nav);
        read.as_object_mut().unwrap().remove("at");
        nav.as_object_mut().unwrap().remove("at");
        assert_eq!(read, nav);

        let none = h.call(&mut game, method::NAV, json!({"generate": true}));
        assert_eq!(none["error"]["message"], crate::forge::nav::NO_GENERATOR);
        let bad = h.call(&mut game, method::NAV, json!({"unknown": true}));
        assert!(!bad["error"].is_null(), "{bad}");
    }

    #[test]
    fn nav_reply_carries_the_counts_problems_and_overlay_of_the_last_generation() {
        struct Gen;
        impl ::forge::NavBuilder for Gen {
            fn generate(
                &mut self,
                _: &ForgeDoc,
                _: &std::collections::BTreeMap<BlockKind, u32>,
            ) -> Result<::forge::NavReport, String> {
                Ok(::forge::NavReport {
                    sectors: 40,
                    links: [("jump".to_string(), 3)].into(),
                    out_of_map: 2,
                    solids: 1,
                    problems: vec![::forge::NavProblem {
                        kind: "start_off_mesh".into(),
                        what: "start 1".into(),
                        position: [0., -5., 4.],
                        reason: "no floor under it".into(),
                    }],
                    lines: vec![::forge::NavLine {
                        kind: "jump".into(),
                        a: [0., 0., 0.],
                        b: [1., 0., 1.],
                    }],
                })
            }
        }
        let host = host(None, 0);
        let mut session = Session::clone(&host.session.lock().unwrap());
        session.generate_nav(&mut Gen).unwrap();
        let reply = crate::forge::nav::reply(
            &session,
            crate::forge::nav::Facts {
                available: true,
                generating: false,
            },
        );
        assert_eq!(reply["state"], "current");
        assert_eq!(
            reply["counts"],
            json!({"sectors": 40, "links": {"jump": 3}, "out_of_map": 2, "solids": 1})
        );
        assert_eq!(reply["problems"][0]["kind"], "start_off_mesh");
        assert_eq!(reply["problems"][0]["what"], "start 1");
        assert_eq!(
            reply["overlay"][0],
            json!({"kind": "jump", "a": [0., 0., 0.], "b": [1., 0., 1.]})
        );
        let status = crate::forge::nav::status(&session, Default::default());
        assert_eq!(status["sectors"], 40);
        assert_eq!(status["problems"], 1);
    }

    /// An invented set of one round whose waves each send one Elite squad
    /// to zone 0 and one to zone 1.
    fn two_zone_set() -> Json {
        let squad = |zone: u8| {
            json!({
                "class": "elite",
                "count": {"per_player": 1, "min": 1, "max": 2},
                "drop": {"prefer_zone": zone},
                "weapon_tier": 0,
            })
        };
        let waves: Vec<Json> = (0..5)
            .map(|i| {
                json!({"squads": [squad(0), squad(1)], "boss": i == 4, "gap_ticks": 300})
            })
            .collect();
        json!({
            "format": 3,
            "name": "Two Zones",
            "teams": 1,
            "set": {"endless": false, "scale": 0, "skulls": []},
            "rounds": [{"waves": waves, "skulls_on": []}],
            "lives": {"pool_start": 5, "per_round_bonus": 1, "shared": true},
            "caps": {"live_actors_by_players": [[4, 32]]},
            "scoring": {
                "by_class": {"elite": 100},
                "player_kill": 0,
                "difficulty_multiplier": {"easy": 1000, "normal": 1000, "heroic": 1000, "legendary": 1000},
                "skull_multipliers": {},
                "assist": 500,
            },
        })
    }

    #[test]
    fn waves_answers_the_set_its_id_and_the_drop_order() {
        let h = Harness::new();
        let mut game = game(1);
        local::install(&h.agent, host(None, 2));
        let methods = h.ok(&mut game, agent_proto::method::METHODS, Json::Null);
        assert!(methods.as_array().unwrap().contains(&json!(method::WAVES)));

        // No set of its own: Holdout, the default.
        let read = h.ok(&mut game, method::WAVES, Json::Null);
        let holdout = forge_doc::waves::default_set();
        assert_eq!(read["name"], "Holdout");
        assert_eq!(read["default"], true);
        assert_eq!(read["id"], json!(holdout.id().unwrap()));
        assert_eq!(read["set"], json!(holdout));
        assert_eq!(read["problems"], json!([]));
        let drops = read["drops"].as_array().unwrap();
        assert_eq!(drops.len(), 2);
        assert_eq!(drops[0]["zone"], 0);
        assert_eq!(drops[0]["position"], json!([1., 2., 0.]));
        assert_eq!(drops[1]["position"], json!([2., 2., 0.]));
        assert_eq!(drops[0]["facing"], 0.5);
        let status = h.ok(&mut game, method::STATUS, Json::Null);
        assert_eq!(status["variant"]["waves"], Json::Null);

        // Set: the reply is the new set's, and the session holds it.
        let set = two_zone_set();
        let def: forge_doc::waves::Def = serde_json::from_value(set.clone()).unwrap();
        let reply = h.ok(&mut game, method::WAVES, json!({"set": set}));
        assert_eq!(reply["name"], "Two Zones");
        assert_eq!(reply["default"], false);
        assert_eq!(reply["id"], json!(def.id().unwrap()));
        assert_eq!(
            reply["set"]["rounds"][0]["waves"][0]["squads"][1]["drop"],
            json!({"prefer_zone": 1})
        );
        let read = h.ok(&mut game, method::WAVES, json!({}));
        assert_eq!(read["id"], reply["id"]);
        let status = h.ok(&mut game, method::STATUS, Json::Null);
        assert_eq!(status["variant"]["waves"], "Two Zones");
        assert_eq!(status["variant"]["title"], "Fixture");

        // A refused set is named and changes nothing.
        let mut ogres = two_zone_set();
        ogres["scoring"]["by_class"] = json!({"ogre": 100});
        for squad in 0..2 {
            for wave in 0..5 {
                ogres["rounds"][0]["waves"][wave]["squads"][squad]["class"] = json!("ogre");
            }
        }
        let refused = h.call(&mut game, method::WAVES, json!({"set": ogres}));
        assert_eq!(refused["error"]["data"]["code"], "forge_waves");
        assert!(
            refused["error"]["message"].as_str().unwrap().contains("ogre"),
            "{refused}"
        );
        let shapeless = h.call(&mut game, method::WAVES, json!({"set": {"name": "x"}}));
        assert_eq!(shapeless["error"]["data"]["code"], "forge_waves");
        let read = h.ok(&mut game, method::WAVES, Json::Null);
        assert_eq!(read["id"], reply["id"]);

        // `null` clears it; an unknown parameter is refused.
        let cleared = h.ok(&mut game, method::WAVES, json!({"set": null}));
        assert_eq!(cleared["default"], true);
        assert_eq!(cleared["name"], "Holdout");
        let bad = h.call(&mut game, method::WAVES, json!({"stop": true}));
        assert!(!bad["error"].is_null(), "{bad}");
    }

    /// A world that holds the definitions it is told of.
    #[derive(Default)]
    struct Holds {
        loaded: Vec<String>,
        next: u32,
    }

    impl ::forge::World for Holds {
        fn place(&mut self, _: &PaletteItem, _: Frame) -> Result<u32, String> {
            self.next += 1;
            Ok(self.next)
        }
        fn respawn(&mut self, item: &PaletteItem, frame: Frame) -> Result<u32, String> {
            self.place(item, frame)
        }
        fn move_object(&mut self, _: u32, _: Frame) -> Result<(), String> {
            Ok(())
        }
        fn delete(&mut self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn piece_loaded(&self, name: &str) -> bool {
            self.loaded.iter().any(|n| n == name)
        }
    }

    #[test]
    fn pieces_lists_each_piece_with_its_definition_state_and_bounds() {
        use ::forge::{PieceDef, Shape};
        let h = Harness::new();
        let mut game = game(1);
        let limits = forge_doc::Limits {
            world: Some(forge_doc::WorldBounds {
                min: [-8.0; 3],
                max: [8.0; 3],
            }),
            base_counts: [(BlockKind::Crates, 2)].into(),
            ..Default::default()
        };
        let mut session = Session::new(draft(), limits, vec![], 8).unwrap();
        let held = PieceDef::dev(Shape::Block, [100, 200, 300], 0);
        let missing = PieceDef::dev(Shape::Ramp, [128, 128, 64], 0);
        let mut world = Holds {
            loaded: vec![held.name()],
            ..Default::default()
        };
        let at = |x: f32| Frame {
            position: [x, 1., 0.],
            rotation: [0.; 3],
        };
        let live = session.place_piece(&mut world, 1, held.clone(), at(0.)).unwrap();
        let pending = session
            .place_piece(&mut world, 2, missing.clone(), at(3.))
            .unwrap();
        local::install(
            &h.agent,
            Host {
                session: Mutex::new(Arc::new(session)),
                builds: None,
            },
        );
        let methods = h.ok(&mut game, agent_proto::method::METHODS, Json::Null);
        assert!(methods.as_array().unwrap().contains(&json!(method::PIECES)));

        let reply = h.ok(&mut game, method::PIECES, Json::Null);
        assert_eq!(reply["pending"], 1);
        assert_eq!(reply["slots"], json!([]));
        let pieces = reply["pieces"].as_array().unwrap();
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0]["target"], json!({"added": live.0}));
        assert_eq!(pieces[0]["name"], held.name());
        assert_eq!(pieces[0]["state"], "live");
        assert_eq!(pieces[0]["def"], json!(held));
        assert_eq!(
            pieces[0]["bounds"],
            json!({"min": [0.0, 1.0, 0.0], "max": [1.0, 3.0, 3.0]})
        );
        assert_eq!(pieces[1]["target"], json!({"added": pending.0}));
        assert_eq!(pieces[1]["state"], "pending");

        // `forge.things` names the kind.
        let things = h.ok(&mut game, method::THINGS, json!({"kind": "piece"}));
        assert_eq!(things["things"].as_array().unwrap().len(), 2);
        assert_eq!(things["things"][0]["kind"], "piece");
        let refused = h.call(&mut game, method::PIECES, json!({"all": true}));
        assert!(!refused["error"].is_null(), "{refused}");
    }
}

#[test]
fn a_piece_is_loaded_when_the_palette_holds_its_bloc() {
    use ::forge::{BlockKind, PaletteItem, PieceDef, Session, Shape, World};
    use local::{ForgeWorld, Guard};
    let held = PieceDef::dev(Shape::Block, [64, 64, 64], 0);
    let other = PieceDef::dev(Shape::Ramp, [128, 128, 64], 0);
    let limits = forge_doc::Limits {
        world: Some(forge_doc::WorldBounds {
            min: [-8.0; 3],
            max: [8.0; 3],
        }),
        base_counts: [(BlockKind::Crates, 0)].into(),
        ..Default::default()
    };
    let palette = vec![
        PaletteItem {
            block: BlockKind::Crates,
            tag: ::forge::piece::bloc_key(&held),
        },
        // A crate that is not a definition's is not a loaded piece.
        PaletteItem {
            block: BlockKind::Crates,
            tag: ::forge::TagKey {
                group: "bloc".into(),
                path: other.name(),
            },
        },
    ];
    let session = Session::new(draft(), limits, palette, 3).unwrap();
    let q = calls::shared();
    calls::lock(&q).arm();
    let guard = Guard {
        scenario: SCENARIO,
        generation: 1,
        admitted: |_, _| true,
    };
    ForgeWorld::apply(
        &session,
        &q,
        guard,
        |_| Ok(TAG),
        |_, w| {
            assert!(w.piece_loaded(&held.name()));
            assert!(!w.piece_loaded(&other.name()));
            assert!(!w.piece_loaded("block_1x1x1_0_000000000000"));
            Ok(())
        },
    )
    .unwrap();
}
