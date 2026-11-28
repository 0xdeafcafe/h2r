//! The calls over a fake guest, the callees played by the tests as the
//! real ones behave (engine's `entry::testing::run`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use agent_proto::sim::REFUSED;
use guest_layout::GuestMemory;
use guest_layout::entry::testing::run as drive;
use guest_layout::entry::{Cx, EntryFn, EntryFuture, GuestCall, Regs, Ret};
use guest_layout::testing::FakeGuest;
use guest_layout::view::fake;
use guest_layout::view::objects::{Body, Header};
use guest_layout::view::players::Player;
use serde_json::json;

use super::*;

const PLAYERS_HDR: u32 = 0x8000_0000;
const OBJECTS_HDR: u32 = 0x8010_0000;
const UNIT_BODY: u32 = 0x8020_0000;
const SCRATCH: u32 = 0x8040_0000;

struct World {
    g: FakeGuest,
    /// The player's handle and its unit's.
    player: u32,
    unit: u32,
}

/// One player (local user 0) whose unit stands at the origin facing +x.
fn world() -> World {
    let mut g = FakeGuest::new();
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
    unit.set_parent(&mut g, None).unwrap();
    unit.set_forward(&mut g, [1.0, 0.0, 0.0]).unwrap();
    let (player_h, p) = fake::push(&mut g, &players);
    p.set_unit(&mut g, Some(unit_h)).unwrap();
    p.set_local_user(&mut g, 0).unwrap();
    g.map(SCRATCH, SCRATCH_SIZE);
    World {
        g,
        player: player_h.raw(),
        unit: unit_h.raw(),
    }
}

/// Runs `call` once; its outcome and the guest calls it made.
fn one(
    w: World,
    call: Call,
    callee: impl FnMut(&GuestCall, &mut dyn GuestMemory) -> Regs,
) -> (Outcome, Vec<GuestCall>, FakeGuest) {
    one_with_queue(w, call, shared(), callee)
}

fn one_with_queue(
    w: World,
    call: Call,
    queue: Shared,
    callee: impl FnMut(&GuestCall, &mut dyn GuestMemory) -> Regs,
) -> (Outcome, Vec<GuestCall>, FakeGuest) {
    let got: Arc<Mutex<Option<Outcome>>> = Arc::default();
    let g2 = Arc::clone(&got);
    let f: Box<EntryFn> = Box::new(move |cx: Cx| -> EntryFuture {
        let (call, got) = (call.clone(), Arc::clone(&g2));
        let queue = Arc::clone(&queue);
        Box::pin(async move {
            let out = run(&cx, &call, SCRATCH, &queue).await?;
            *got.lock().unwrap() = Some(out);
            Ok(Ret::new(0, 0))
        })
    });
    let r = drive(&*f, w.g, Regs::default(), callee);
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    let out = got.lock().unwrap().take().expect("an outcome");
    (out, r.calls, r.guest)
}

fn al(v: u32) -> Regs {
    Regs {
        eax: v,
        ..Regs::default()
    }
}

fn vec3(m: &dyn GuestMemory, at: u32) -> [f32; 3] {
    [
        m.f32(at).unwrap(),
        m.f32(at + 4).unwrap(),
        m.f32(at + 8).unwrap(),
    ]
}

#[test]
fn a_teleport_places_the_player_through_the_game_then_turns_unit_and_view() {
    let w = world();
    let (player, unit) = (w.player, w.unit);
    let f = [0.0, -1.0, 0.0];
    let (out, calls, g) = one(
        w,
        Call::Teleport {
            player,
            pos: Some([10.0, 20.0, 3.0]),
            facing: Some(f),
        },
        |c, m| {
            if c.target == addr::PLAYER_PLACE_NEAR {
                // The game places the unit a little above the point.
                let p = vec3(m, c.esi.unwrap());
                Body::at(UNIT_BODY)
                    .set_position(m, [p[0], p[1], p[2] + 0.5])
                    .unwrap();
                return al(1);
            }
            if c.target == addr::OBJECT_SET_POSITION {
                let fwd = vec3(m, c.ecx.unwrap());
                Body::at(UNIT_BODY).set_forward(m, fwd).unwrap();
            }
            Regs::default()
        },
    );
    let out = out.unwrap();
    let targets: Vec<u32> = calls.iter().map(|c| c.target).collect();
    assert_eq!(
        targets,
        [
            addr::PLAYER_PLACE_NEAR,
            addr::PLAYER_SET_FACING,
            addr::OBJECT_SET_POSITION
        ]
    );
    // PLAYER_PLACE_NEAR: EBX the player, ESI the point, the anchor -1.
    assert_eq!(calls[0].ebx, Some(player));
    assert_eq!(calls[0].args, vec![u32::MAX]);
    assert_eq!(vec3(&g, calls[0].esi.unwrap()), [10.0, 20.0, 3.0]);
    // The view: ECX the local user, EAX the forward.
    assert_eq!(calls[1].ecx, Some(0));
    assert_eq!(vec3(&g, calls[1].eax.unwrap()), f);
    // The orientation alone: no position, the forward and an up.
    assert_eq!(calls[2].eax, Some(0));
    assert_eq!(calls[2].args, vec![unit, 0, 0]);
    assert_eq!(vec3(&g, calls[2].edx.unwrap()), [0.0, 0.0, 1.0]);
    for o in addr::UNIT_FACINGS {
        assert_eq!(vec3(&g, UNIT_BODY + o), f, "unit +{o:#x}");
    }
    assert_eq!(out["pos"], json!([10.0, 20.0, 3.5]));
    assert_eq!(out["body_forward"], json!(f));
    assert_eq!(out["faced"], true);
}

#[test]
fn a_teleport_the_placement_refuses_is_an_error_and_nothing_turns() {
    let w = world();
    let player = w.player;
    let (out, calls, _) = one(
        w,
        Call::Teleport {
            player,
            pos: Some([1.0, 2.0, 3.0]),
            facing: Some([0.0, 1.0, 0.0]),
        },
        |_, _| al(0),
    );
    assert!(out.unwrap_err().message.contains("0x14e970"));
    assert_eq!(calls.len(), 1, "no turn after a refused placement");
}

#[test]
fn a_seated_unit_moves_only_by_its_position_and_says_why_it_did_not_turn() {
    let mut w = world();
    Body::at(UNIT_BODY)
        .set_parent(&mut w.g, Some(guest_layout::handle::DatumHandle(0x1234_0005)))
        .unwrap();
    let player = w.player;
    let (out, calls, _) = one(
        w,
        Call::Teleport {
            player,
            pos: None,
            facing: Some([0.0, 1.0, 0.0]),
        },
        |_, _| Regs::default(),
    );
    assert!(calls.is_empty());
    assert!(out.unwrap()["facing_left"].is_string());
}

#[test]
fn a_teleport_of_a_player_between_lives_is_refused_before_any_call() {
    let mut w = world();
    let p = guest_layout::view::players::array(&w.g)
        .unwrap()
        .unwrap()
        .get(&w.g, guest_layout::handle::DatumHandle(w.player))
        .unwrap()
        .unwrap();
    p.set_unit(&mut w.g, None).unwrap();
    let player = w.player;
    let (out, calls, _) = one(
        w,
        Call::Teleport {
            player,
            pos: Some([0.0; 3]),
            facing: None,
        },
        |_, _| Regs::default(),
    );
    assert!(out.unwrap_err().message.contains("between lives"));
    assert!(calls.is_empty());
}

#[test]
fn a_give_makes_the_weapon_as_a_starting_profile_does() {
    let w = world();
    let unit = w.unit;
    let (out, calls, _) = one(
        w,
        Call::Give {
            unit,
            weapon: 0xe123_0045,
            mode: addr::MODE_REPLACE,
        },
        |c, _| match c.target {
            addr::OBJECT_NEW => al(0x00aa_0009),
            addr::UNIT_ADD_WEAPON => al(1),
            _ => Regs::default(),
        },
    );
    let out = out.unwrap();
    let made = 0x00aa_0009;
    let targets: Vec<u32> = calls.iter().map(|c| c.target).collect();
    assert_eq!(
        targets,
        [addr::PLACEMENT_NEW, addr::OBJECT_NEW, addr::UNIT_ADD_WEAPON]
    );
    // placement_data_new: ESI the placement, (definition, owner unit, 0).
    assert_eq!(calls[0].esi, Some(SCRATCH));
    assert_eq!(calls[0].args, vec![0xe123_0045, unit, 0]);
    assert_eq!(calls[1].args, vec![SCRATCH]);
    assert_eq!(calls[2].args, vec![unit, made, addr::MODE_REPLACE]);
    assert_eq!(out["object"], format!("{made:08x}"));
    assert_eq!(out["mode"], 2);
}

#[test]
fn a_refused_weapon_is_deleted_and_a_refused_object_is_an_error() {
    let w = world();
    let unit = w.unit;
    let (out, calls, _) = one(
        w,
        Call::Give {
            unit,
            weapon: 7,
            mode: addr::MODE_ADD,
        },
        |c, _| match c.target {
            addr::OBJECT_NEW => al(0x00aa_0009),
            _ => al(0),
        },
    );
    assert!(out.unwrap_err().message.contains("deleted"));
    let last = calls.last().unwrap();
    assert_eq!(
        (last.target, last.args.clone()),
        (addr::OBJECT_DELETE, vec![0x00aa_0009])
    );
    let w = world();
    let unit = w.unit;
    let (out, calls, _) = one(
        w,
        Call::Give {
            unit,
            weapon: 7,
            mode: addr::MODE_ADD,
        },
        |c, _| match c.target {
            addr::OBJECT_NEW => al(u32::MAX),
            _ => al(1),
        },
    );
    assert!(out.unwrap_err().message.contains("object_new"));
    assert_eq!(calls.len(), 2, "nothing added, nothing to delete");
}

#[test]
fn a_spawn_places_the_object_where_and_facing_as_asked() {
    let w = world();
    let (out, calls, _) = one(
        w,
        Call::Spawn {
            tag: 0xe200_0002,
            pos: [5.0, 6.0, 7.0],
            facing: Some([0.0, 1.0, 0.0]),
            mp_scenario: None,
        },
        |c, m| {
            if c.target == addr::OBJECT_NEW {
                // The placement is filled before the object is made.
                let p = c.args[0];
                assert_eq!(vec3(m, p + addr::PLACEMENT_POSITION), [5.0, 6.0, 7.0]);
                assert_eq!(vec3(m, p + addr::PLACEMENT_FORWARD), [0.0, 1.0, 0.0]);
                assert_eq!(vec3(m, p + addr::PLACEMENT_UP), [0.0, 0.0, 1.0]);
                return al(0x0042_0003);
            }
            Regs::default()
        },
    );
    assert_eq!(calls[0].args, vec![0xe200_0002, u32::MAX, 0]);
    assert_eq!(out.unwrap()["object"], "00420003");
}

#[test]
fn a_successful_spawn_can_enroll_the_new_object_before_the_tick() {
    const ENROLL: u32 = 0x4000;
    const MADE: u32 = 0x8123_0003;
    let w = world();
    let q = shared();
    lock(&q).arm();
    lock(&q).set_spawn_enrollment(|_, object| Some(GuestCall::new(ENROLL).args([object])));
    let ticket = submit(
        &q,
        Call::Spawn {
            tag: 0xe200_0002,
            pos: [5.0, 6.0, 7.0],
            facing: None,
            mp_scenario: None,
        },
    ).unwrap();
    let f = move |cx| -> EntryFuture {
        let work = lock(&q).drain();
        Box::pin(run_queued(cx, Arc::clone(&q), work, SCRATCH, Arc::default()))
    };
    let r = drive(&f, w.g, Regs::default(), |c, _| {
        if c.target == addr::OBJECT_NEW {
            al(MADE)
        } else {
            Regs::default()
        }
    });
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    let targets: Vec<_> = r.calls.iter().map(|call| call.target).collect();
    assert_eq!(targets, [
        addr::PLACEMENT_NEW, addr::OBJECT_NEW, ENROLL, guest_layout::points::GAME_TICK.base,
    ]);
    assert_eq!(r.calls[2].args, [MADE]);
    assert_eq!(ticket.outcome().unwrap().unwrap()["object"], hex(MADE));
}

#[test]
fn queued_calls_run_then_the_tick_with_reentry_set() {
    let w = world();
    let q = shared();
    assert!(
        submit(
            &q,
            Call::Give {
                unit: 1,
                weapon: 2,
                mode: 1,
            },
        )
        .is_err(),
        "unarmed"
    );
    lock(&q).arm();
    let player = w.player;
    let t = submit(
        &q,
        Call::Teleport {
            player,
            pos: Some([1.0, 1.0, 1.0]),
            facing: None,
        },
    )
    .unwrap();
    assert!(lock(&q).live().is_some(), "queued calls block a snapshot");
    let work = lock(&q).drain();
    assert!(lock(&q).live().is_some(), "drained calls still block a snapshot");
    let reentry = Arc::new(AtomicBool::new(false));
    let (q2, r2) = (Arc::clone(&q), Arc::clone(&reentry));
    let f: Box<EntryFn> = Box::new(move |cx: Cx| -> EntryFuture {
        Box::pin(run_queued(
            cx,
            Arc::clone(&q2),
            work.clone(),
            SCRATCH,
            Arc::clone(&r2),
        ))
    });
    let seen = Arc::clone(&reentry);
    let r = drive(&*f, w.g, Regs::default(), move |c, _| {
        if c.target == guest_layout::points::GAME_TICK.base {
            assert!(
                seen.load(Ordering::SeqCst),
                "the tick runs with reentry set"
            );
        }
        al(1)
    });
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    assert_eq!(
        r.calls.last().unwrap().target,
        guest_layout::points::GAME_TICK.base
    );
    assert!(!reentry.load(Ordering::SeqCst));
    assert!(lock(&q).live().is_none(), "the call finished");
    assert!(t.outcome().unwrap().is_ok());
    assert!(t.outcome().is_none(), "taken once");
}

#[test]
fn up_is_square_to_forward() {
    let f = [0.6f32, 0.0, 0.8];
    let u = up_of(f);
    let dot: f32 = f.iter().zip(u).map(|(a, b)| a * b).sum();
    assert!(dot.abs() < 1e-6, "{u:?}");
    assert_eq!(up_of([0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]);
}

#[test]
fn a_queued_game_builds_its_options_in_the_scratch_then_queues_them() {
    const NEW: u32 = 0x0013_8110;
    const CHECK: u32 = 0x0013_8180;
    const QUEUE: u32 = 0x0018_e790;
    let options = SCRATCH + SCRATCH_OPTIONS;
    let steps = vec![
        GameStep::Call {
            target: NEW,
            args: vec![],
            eax: None,
            esi: Some(Arg::Options(0)),
            report: None,
        },
        GameStep::Write {
            offset: 0x18,
            bytes: vec![1, 2, 3, 4],
        },
        GameStep::Call {
            target: CHECK,
            args: vec![Arg::Options(0), Arg::Value(7)],
            eax: None,
            esi: None,
            report: Some("check"),
        },
        GameStep::Call {
            target: QUEUE,
            args: vec![],
            eax: Some(Arg::Options(0)),
            esi: None,
            report: None,
        },
    ];
    let (out, calls, g) = one(world(), Call::QueueGame { steps }, |c, _| {
        if c.target == CHECK { al(0x101) } else { Regs::default() }
    });
    let out = out.unwrap();
    let targets: Vec<u32> = calls.iter().map(|c| c.target).collect();
    assert_eq!(targets, [NEW, CHECK, QUEUE]);
    assert_eq!(calls[0].esi, Some(options));
    assert_eq!(calls[1].args, vec![options, 7]);
    assert_eq!(calls[2].eax, Some(options));
    assert_eq!(g.u32(options + 0x18).unwrap(), 0x0403_0201);
    // AL, not EAX: 0x101 is a pass.
    assert_eq!(out["check"], true);
    assert_eq!(out["queued"], true);
    assert_eq!(out["calls"], 3);
}

#[test]
fn a_queued_game_refuses_a_write_past_its_options_block() {
    let steps = vec![
        GameStep::Write {
            offset: OPTIONS_SIZE - 2,
            bytes: vec![0; 4],
        },
        GameStep::Call {
            target: 0x0018_e790,
            args: vec![],
            eax: Some(Arg::Options(0)),
            esi: None,
            report: None,
        },
    ];
    let (out, calls, _) = one(world(), Call::QueueGame { steps }, |_, _| Regs::default());
    assert!(out.unwrap_err().message.contains("past the options block"));
    assert!(calls.is_empty());
}

#[test]
fn a_replication_spawn_checks_the_host_and_scenario_when_the_call_runs() {
    const SCENARIO: u32 = 0x8100_0000;
    const WORLD: u32 = 0x8101_0000;
    for (scenario, mode) in [(SCENARIO, 3), (SCENARIO + 0x1000, 4)] {
        let mut w = world();
        w.g.put_u32(guest_layout::global::SCENARIO.base, scenario)
            .put_u32(guest_layout::global::SIM_GLOBALS.base, WORLD)
            .put_u32(WORLD + 8, mode);
        let (out, calls, _) = one(
            w,
            Call::Spawn {
                tag: 0xa100_0001,
                pos: [1.0, 2.0, 3.0],
                facing: None,
                mp_scenario: Some(SCENARIO),
            },
            |_, _| panic!("changed session must not enter the guest"),
        );
        assert!(out.unwrap_err().message.contains("host or scenario changed"));
        assert!(calls.is_empty());
    }
}

fn vehicle() -> (World, ObjectId) {
    let mut w = world();
    let id = ObjectId { kind: agent_proto::Kind::Vehicle, key: w.unit };
    let header = objects::array(&w.g).unwrap().unwrap()
        .get(&w.g, DatumHandle(id.key)).unwrap().unwrap();
    header.set_kind(&mut w.g, objects::VEHICLE).unwrap();
    Body::at(UNIT_BODY).set_up(&mut w.g, [0.0, 0.0, 1.0]).unwrap();
    (w, id)
}

#[test]
fn object_set_preserves_missing_axes() {
    let (w, id) = vehicle();
    let (out, calls, _) = one(
        w,
        Call::ObjectSet {
            id,
            pos: Some([2.0, 3.0, 4.0]),
            forward: Some([0.0, 1.0, 0.0]),
            up: None,
        },
        |c, m| {
            assert_eq!(c.target, addr::OBJECT_SET_POSITION);
            assert_eq!(c.args, [id.key, 0, 0]);
            assert_eq!(vec3(m, c.eax.unwrap()), [2.0, 3.0, 4.0]);
            assert_eq!(vec3(m, c.ecx.unwrap()), [0.0, 1.0, 0.0]);
            assert_eq!(vec3(m, c.edx.unwrap()), [0.0, 0.0, 1.0]);
            al(0)
        },
    );
    assert_eq!(calls.len(), 1);
    assert_eq!(out.unwrap()["object"], id.to_string());

    let (w, id) = vehicle();
    let (out, calls, _) = one(
        w,
        Call::ObjectSet { id, pos: None, forward: None, up: Some([0.0, 1.0, 0.0]) },
        |c, m| {
            assert_eq!(c.target, addr::OBJECT_SET_POSITION);
            assert_eq!(c.args, [id.key, 0, 0]);
            assert_eq!(c.eax, Some(0));
            assert_eq!(vec3(m, c.ecx.unwrap()), [1.0, 0.0, 0.0]);
            assert_eq!(vec3(m, c.edx.unwrap()), [0.0, 1.0, 0.0]);
            al(0)
        },
    );
    assert_eq!(calls.len(), 1);
    assert_eq!(out.unwrap()["object"], id.to_string());

    let (w, id) = vehicle();
    let (out, calls, _) = one(
        w,
        Call::ObjectSet { id, pos: Some([1.0; 3]), forward: None, up: None },
        |c, m| {
            assert_eq!(c.target, addr::OBJECT_SET_POSITION);
            assert_eq!(c.args, [id.key, 0, 0]);
            assert_eq!(vec3(m, c.eax.unwrap()), [1.0; 3]);
            assert_eq!((c.ecx, c.edx), (Some(0), Some(0)));
            al(0)
        },
    );
    assert_eq!(calls.len(), 1);
    assert_eq!(out.unwrap()["object"], id.to_string());
}

fn respawn(target: RespawnTarget) -> Call {
    Call::ObjectRespawn { target }
}

#[test]
fn object_delete_checks_salt_and_kind() {
    let (w, id) = vehicle();
    let (out, calls, _) = one(w, Call::ObjectDelete { id }, |_, _| al(0));
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].target, addr::OBJECT_DELETE);
    assert_eq!(calls[0].args, [id.key]);
    assert_eq!(out.unwrap()["deleted"], id.to_string());
    let stale = ObjectId { key: id.key ^ (1 << 16), ..id };
    for call in [
        Call::ObjectDelete { id: stale },
        Call::ObjectSet { id: stale, pos: Some([0.0; 3]), forward: None, up: None },
        respawn(RespawnTarget::Object(stale)),
        Call::ObjectDelete { id: ObjectId { kind: agent_proto::Kind::Weapon, ..id } },
    ] {
        let (w, _) = vehicle();
        let (out, calls, _) = one(w, call, |_, _| panic!("gone object reached the game"));
        assert!(calls.is_empty());
        assert_eq!(out.unwrap_err().code, code::GONE);
    }
}

const ENTRY: u32 = 0x8101_0000;
const PALETTE: u32 = 0x8102_0000;
const TAG: u32 = 0x4321_0001;

fn placed_vehicle() -> (World, ObjectId) {
    placed_object(agent_proto::Kind::Vehicle, objects::VEHICLE, *b"vehi", 0x70)
}

fn placed_object(kind: agent_proto::Kind, ty: u8, group: [u8; 4], block: u32) -> (World, ObjectId) {
    let (mut w, mut id) = vehicle();
    id.kind = kind;
    let g = &mut w.g;
    let header = objects::array(g).unwrap().unwrap().get(g, DatumHandle(id.key)).unwrap().unwrap();
    header.set_kind(g, ty).unwrap();
    let root = 0x8100_0000;
    let index = 0x8103_0000;
    let tags = 0x8104_0000;
    let group = u32::from_be_bytes(group);
    for (global, ptr) in [
        (guest_layout::points::SCENARIO.base, root),
        (guest_layout::points::TAG_INDEX.base, index),
        (guest_layout::points::TAG_ENTRIES.base, tags),
    ] {
        g.map(global, 4).put_u32(global, ptr);
    }
    g.map(root, block + 16)
        .put_u32(root + block, 1)
        .put_u32(root + block + 4, ENTRY)
        .put_u32(root + block + 8, 2)
        .put_u32(root + block + 12, PALETTE);
    g.map(ENTRY, 0x54).put_u16(ENTRY, 1).put_u16(ENTRY + 2, u16::MAX);
    put_vec(g, ENTRY + 8, [3.0, -4.0, 8.0]).unwrap();
    put_vec(g, ENTRY + 0x14, [0.0, 0.0, std::f32::consts::FRAC_PI_2]).unwrap();
    g.map(PALETTE, 0x50).put_u32(PALETTE + 0x28, group).put_u32(PALETTE + 0x2c, TAG);
    g.map(index, 0x20).put_u32(index + guest_layout::tags::index_layout::TAG_COUNT, 2);
    g.map(tags, 0x20).put_u32(tags + 0x10, group)
        .put_u32(tags + 0x14, TAG).put_u32(tags + 0x18, 0x8110_0000);
    g.put_u16(0x8110_0000, ty.into());
    g.put_u16(UNIT_BODY + guest_layout::Object::PLACEMENT_INDEX.offset, 0);
    (w, id)
}

fn vehicle_placement() -> RespawnTarget {
    RespawnTarget::Placement(agent_proto::sim::Placement { block: "vehicles".into(), index: 0 })
}

#[test]
fn respawn_uses_the_current_palette_and_frame() {
    for by_id in [true, false] {
        let (w, id) = placed_vehicle();
        let target = if by_id { RespawnTarget::Object(id) } else { vehicle_placement() };
        let (out, calls, _) = one(w, respawn(target), |c, m| {
            match c.target {
                addr::OBJECT_DELETE => assert_eq!(c.args, [id.key]),
                addr::PLACEMENT_NEW => {
                    assert_eq!(c.esi, Some(SCRATCH));
                    assert_eq!(c.args, [TAG, u32::MAX, 0]);
                }
                addr::OBJECT_NEW => {
                    assert_eq!(c.args, [SCRATCH]);
                    assert_eq!(vec3(m, SCRATCH + addr::PLACEMENT_POSITION), [3.0, -4.0, 8.0]);
                    assert_eq!(vec3(m, SCRATCH + addr::PLACEMENT_FORWARD), [1.0, 0.0, 0.0]);
                    let up = vec3(m, SCRATCH + addr::PLACEMENT_UP);
                    assert!(up[0].abs() < 1e-5 && (up[1] + 1.0).abs() < 1e-5 && up[2].abs() < 1e-5);
                    return al(0x0042_0003);
                }
                _ => panic!("unexpected guest call"),
            }
            al(0)
        });
        assert_eq!(calls.iter().map(|c| c.target).collect::<Vec<_>>(),
            [addr::PLACEMENT_NEW, addr::OBJECT_NEW, addr::OBJECT_DELETE]);
        assert_eq!(out.unwrap(),
            json!({"old":id.to_string(),"object":"vehicle.00420003","linked":false}));
    }
}

#[test]
fn repeated_respawn_replaces_the_current_preview_without_faking_a_placement_link() {
    let queue = shared();
    let (mut w, mut current) = placed_vehicle();
    let pool = objects::array(&w.g).unwrap().unwrap();
    for (round, by_id) in [false, false, true].into_iter().enumerate() {
        let target = if by_id { RespawnTarget::Object(current) } else { vehicle_placement() };
        let mut next = None;
        let body = UNIT_BODY + 0x10000 + round as u32 * 0x1000;
        w.g.map(body, 0x400);
        let (player, unit) = (w.player, w.unit);
        let (out, calls, g) = one_with_queue(
            w,
            respawn(target),
            queue.clone(),
            |c, m| {
                match c.target {
                    addr::OBJECT_DELETE => {
                        assert_eq!(c.args, [current.key]);
                        pool.array.delete(m, DatumHandle(current.key)).unwrap();
                    }
                    addr::PLACEMENT_NEW => {}
                    addr::OBJECT_NEW => {
                        // The old occupant is still alive during allocation.
                        assert!(pool.get(m, DatumHandle(current.key)).unwrap().is_some());
                        let h = pool.array.new_datum(m).unwrap().unwrap();
                        let header = pool.get(m, h).unwrap().unwrap();
                        header.set_kind(m, objects::VEHICLE).unwrap();
                        header.set_body(m, body).unwrap();
                        Body::at(body).set_parent(m, None).unwrap();
                        Body::at(body).set_definition(m, Some(DatumHandle(TAG))).unwrap();
                        m.set_u16(body + guest_layout::Object::PLACEMENT_INDEX.offset, u16::MAX)
                            .unwrap();
                        next = Some(ObjectId { key: h.raw(), ..current });
                        return al(h.raw());
                    }
                    _ => panic!("unexpected guest call"),
                }
                al(0)
            },
        );
        let next = next.unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(out.unwrap(), json!({
            "old": current.to_string(), "object": next.to_string(), "linked": false
        }));
        current = next;
        w = World { g, player, unit };
    }
    // Retail also created a linked occupant while the preview was alive.
    // Neither of the two objects may be silently deleted by the retry.
    let mut overlap = World { g: w.g.clone(), player: w.player, unit: w.unit };
    let pool = objects::array(&overlap.g).unwrap().unwrap();
    let (_, other) = fake::push(&mut overlap.g, &pool);
    let body = UNIT_BODY + 0x1000;
    overlap.g.map(body, 0x400);
    other.set_body(&mut overlap.g, body).unwrap();
    other.set_kind(&mut overlap.g, objects::VEHICLE).unwrap();
    Body::at(body).set_parent(&mut overlap.g, None).unwrap();
    let (out, calls, _) = one_with_queue(
        overlap,
        respawn(vehicle_placement()),
        queue.clone(),
        |_, _| panic!("ambiguous occupants reached the game"),
    );
    assert!(calls.is_empty());
    assert!(out.unwrap_err().message.contains("more than one"));

    // After a checkpoint cut the same salted value can belong to a new
    // timeline. Refuse an ambiguous unlinked object before making a copy.
    lock(&queue).checkpoint_reverted();
    let (out, calls, _) = one_with_queue(
        w,
        respawn(vehicle_placement()),
        queue,
        |_, _| panic!("ambiguous preview reached the game"),
    );
    assert!(calls.is_empty());
    assert_eq!(out.unwrap_err().code, REFUSED);
}

#[test]
fn unlinked_preview_refuses_snapshots_until_the_scenario_is_clean() {
    let (world, preview, queue) = world_with_unlinked_preview();
    assert!(lock(&queue).preview_refusal().unwrap().contains("unlinked"));
    let campaign = Some(("mission".into(), guest_layout::game::MODE_CAMPAIGN));
    lock(&queue).map_activated(campaign.clone());
    lock(&queue).map_activated(campaign);
    assert!(lock(&queue).preview_refusal().is_some(), "a checkpoint may restore the preview");
    lock(&queue).map_activated(Some(("different".into(), guest_layout::game::MODE_CAMPAIGN)));
    assert!(lock(&queue).preview_refusal().is_none());

    let (_, _, queue) = world_with_unlinked_preview();
    lock(&queue).active_scenario = Some(("arena".into(), guest_layout::game::MODE_MULTIPLAYER));
    let (out, _, _) = one_with_queue(world, Call::ObjectDelete { id: preview }, queue.clone(), |_, _| al(0));
    assert!(out.is_ok());
    assert!(lock(&queue).preview_refusal().is_none());
}

#[test]
fn a_clean_activation_forgets_previews_even_when_the_map_and_salt_repeat() {
    let (world, _, queue) = world_with_unlinked_preview();
    // The new instance has an unrelated loose vehicle in the exact same slot.
    lock(&queue).map_activated(Some(("arena".into(), guest_layout::game::MODE_MULTIPLAYER)));
    let (out, calls, _) = one_with_queue(
        world,
        respawn(vehicle_placement()),
        queue,
        |_, _| al(0x42_0003),
    );
    assert!(out.unwrap()["old"].is_null());
    assert_eq!(
        calls.iter().map(|c| c.target).collect::<Vec<_>>(),
        [addr::PLACEMENT_NEW, addr::OBJECT_NEW],
    );
}

fn world_with_unlinked_preview() -> (World, ObjectId, Shared) {
    let (mut world, id) = placed_vehicle();
    let queue = shared();
    {
        let mut q = lock(&queue);
        let entry = placement::read(&world.g, &vehicle_placement(), &q.previews).unwrap();
        q.previews.creating(&entry);
        q.previews.remember(&entry, id);
    }
    Body::at(UNIT_BODY).set_definition(&mut world.g, Some(DatumHandle(TAG))).unwrap();
    world.g.put_u16(UNIT_BODY + guest_layout::Object::PLACEMENT_INDEX.offset, u16::MAX);
    (world, id, queue)
}

#[test]
fn respawn_refuses_non_default_creation_fields_before_delete() {
    let call = respawn(vehicle_placement());
    for (offset, value, why) in [
        (2, 7, "named"),
        (2, 0xfffe, "named"),
        (4, 0x40, "flags"),
        (0x4c, 27, "creation fields"),
    ] {
        let (mut w, _) = placed_vehicle();
        w.g.put_u16(ENTRY + offset, value);
        let (out, calls, _) = one(w, call.clone(), |_, _| panic!("invalid record reached the game"));
        let err = out.unwrap_err();
        assert_eq!(err.code, REFUSED);
        assert!(err.message.contains(why), "{}", err.message);
        assert!(calls.is_empty());
    }
}

#[test]
fn held_weapon_is_gone_by_id_and_refused_by_placement() {
    let held = RespawnTarget::Placement(agent_proto::sim::Placement {
        block: "weapons".into(),
        index: 0,
    });
    for by_placement in [false, true] {
        let (mut w, id) = placed_object(agent_proto::Kind::Weapon, objects::WEAPON, *b"weap", 0x90);
        Body::at(UNIT_BODY)
            .set_parent(&mut w.g, Some(DatumHandle(0x0017_0001)))
            .unwrap();
        let target = if by_placement { held.clone() } else { RespawnTarget::Object(id) };
        let (out, calls, _) = one(w, respawn(target), |_, _| panic!("held weapon reached the game"));
        assert!(calls.is_empty());
        let err = out.unwrap_err();
        if by_placement {
            assert_eq!(err.code, REFUSED);
            assert!(err.message.contains("attached"));
        } else {
            assert_eq!(err.code, code::GONE);
        }
    }
}

#[test]
fn respawn_validates_before_delete() {
    let (mut w, _) = placed_vehicle();
    w.g.put_u16(UNIT_BODY + guest_layout::Object::PLACEMENT_INDEX.offset, u16::MAX);
    let call = respawn(vehicle_placement());
    let (out, calls, _) = one(w, call.clone(), |_, _| al(0x0042_0003));
    assert_eq!(out.unwrap()["old"], Value::Null);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].target, addr::PLACEMENT_NEW);

    let (mut w, _) = placed_vehicle();
    w.g.put_u32(PALETTE + 0x2c, TAG ^ (1 << 16));
    let (out, calls, _) = one(w, call.clone(), |_, _| panic!("invalid palette reached the game"));
    assert!(calls.is_empty());
    assert_eq!(out.unwrap_err().code, REFUSED);

    let (mut w, _) = placed_vehicle();
    w.g.put_f32(ENTRY + 0x20, 2.0);
    let (out, calls, _) = one(w, call, |_, _| panic!("unsupported scale reached the game"));
    assert!(calls.is_empty());
    assert_eq!(out.unwrap_err().code, REFUSED);
}

#[test]
fn abandoning_a_reload_drops_the_prepared_binding_and_frees_the_queue() {
    let q = shared();
    lock(&q).arm();
    let options = vec![0; OPTIONS_SIZE as usize];
    let binding = PreparedMapReload::new(5, || panic!("cancelled binding published"));
    let request = queue_reload(&q, options.clone(), Some(binding)).unwrap();
    assert!(queue_reload(&q, options.clone(), None).is_err());
    drop(request);
    assert!(!lock(&q).has_pending());
    assert!(queue_reload(&q, options, None).is_ok());
}

fn reload_world() -> (World, Vec<u8>) {
    let (mut w, options) = reload_world_without_destination();
    w.g.map(PENDING_OPTIONS, OPTIONS_SIZE);
    (w, options)
}

fn reload_world_without_destination() -> (World, Vec<u8>) {
    use guest_layout::game::{GameGlobals, MODE_CAMPAIGN, OPTIONS};

    let mut w = world();
    let base = 0x8140_0000;
    let mut options = vec![0; OPTIONS_SIZE as usize];
    options[..4].copy_from_slice(&MODE_CAMPAIGN.to_le_bytes());
    w.g.put_u32(guest_layout::points::GAME_GLOBALS.base, base);
    w.g.put(base + OPTIONS, &options);
    w.g.put_u8(base + GameGlobals::MAP_ACTIVE.offset, 1);
    w.g.put_u8(OPTIONS_PENDING, 0);
    (w, options)
}

#[test]
fn a_reload_preflights_the_whole_pending_destination_before_reserving() {
    let (world, options) = reload_world_without_destination();
    assert!(world.g.u8(OPTIONS_PENDING).is_ok());
    assert!(world.g.u8(PENDING_OPTIONS + OPTIONS_SIZE - 1).is_err());
    let q = shared();
    lock(&q).arm();
    let binding = PreparedMapReload::new(42, || panic!("reserved an unreadable transition"));
    let ticket = queue_reload(&q, options, Some(binding)).unwrap();
    let (cx, mut future) = queued_future(&q, world.g, SCRATCH, &Arc::new(AtomicBool::new(false)));
    expect_guest_call(&mut future, &cx, guest_layout::points::GAME_TICK.base);
    assert_eq!(ticket.outcome().unwrap().unwrap_err().code, REFUSED);
    assert_eq!(cx.read(|m| m.u8(OPTIONS_PENDING)).unwrap(), 0);
}

#[test]
fn ordinary_game_queues_keep_retails_pending_transition_rules() {
    let (mut world, options) = reload_world();
    world.g.put_u8(OPTIONS_PENDING, 1);
    let q = shared();
    lock(&q).arm();
    let first = submit(&q, replay_options(options.clone())).unwrap();
    let second = submit(&q, replay_options(options.clone())).unwrap();
    assert!(queue_reload(&q, options.clone(), None).is_err());
    let (cx, mut future) = queued_future(&q, world.g, SCRATCH, &Arc::new(AtomicBool::new(false)));
    for _ in 0..2 {
        expect_guest_call(&mut future, &cx, guest_layout::points::OPTIONS_QUEUE.base);
        guest_layout::entry::resume(&cx, Regs::default());
    }
    expect_guest_call(&mut future, &cx, guest_layout::points::GAME_TICK.base);
    assert!(first.outcome().unwrap().is_ok());
    assert!(second.outcome().unwrap().is_ok());

    let editor = queue_reload(&q, options.clone(), None).unwrap();
    assert!(submit(&q, replay_options(options)).is_err());
    assert!(submit(&q, Call::QueueGame { steps: Vec::new() }).is_ok());
    drop(editor);
}

#[test]
fn guest_fault_drops_the_prepared_reload_before_options_queue() {
    let (w, options) = reload_world();
    let q = shared();
    lock(&q).arm();
    let binding = PreparedMapReload::new(42, || panic!("a failed guest call published the file"));
    let request = queue_reload(&q, options, Some(binding)).unwrap();
    let work = lock(&q).drain();
    let queue = q.clone();
    let f: Box<EntryFn> = Box::new(move |cx| {
        Box::pin(run_queued(
            cx,
            queue.clone(),
            work.clone(),
            0x9020_0000, // unmapped scratch
            Arc::new(AtomicBool::new(false)),
        ))
    });
    let result = drive(&*f, w.g, Regs::default(), |_, _| panic!("guest call after fault"));
    assert!(matches!(result.result, Err(EntryError::Fault(_))));
    assert!(request.outcome().unwrap().is_err());
    assert!(submit(&q, Call::QueueGame { steps: vec![] }).is_ok());
}

fn queued_future(
    q: &Shared,
    guest: FakeGuest,
    scratch: u32,
    reentry: &Arc<AtomicBool>,
) -> (Cx, EntryFuture) {
    let cx = Cx::new(Mutex::new(guest), Regs::default());
    let work = lock(q).drain();
    let future = Box::pin(run_queued(
        cx.clone(),
        q.clone(),
        work,
        scratch,
        reentry.clone(),
    ));
    (cx, future)
}

fn expect_guest_call(future: &mut EntryFuture, cx: &Cx, target: u32) {
    match guest_layout::entry::poll(future, cx) {
        guest_layout::entry::Step::Call(call) => assert_eq!(call.target, target),
        other => panic!("expected call to {target:#x}, got {other:?}"),
    }
}

#[test]
fn interrupted_transfer_finishes_current_and_remaining_tickets() {
    let (w, id) = vehicle();
    let q = shared();
    lock(&q).arm();
    let first = submit(&q, Call::ObjectDelete { id }).unwrap();
    let second = queue_reload(&q, vec![0; OPTIONS_SIZE as usize], None).unwrap();
    let reentry = Arc::new(AtomicBool::new(false));
    let (cx, mut future) = queued_future(&q, w.g, SCRATCH, &reentry);
    expect_guest_call(&mut future, &cx, addr::OBJECT_DELETE);
    drop(future); // nested transfer failed before resume
    assert_eq!(first.outcome().unwrap().unwrap_err().code, code::UNAVAILABLE);
    assert_eq!(second.outcome().unwrap().unwrap_err().code, code::UNAVAILABLE);
    assert!(submit(&q, Call::QueueGame { steps: vec![] }).is_ok());
}

#[test]
fn dropping_the_reentered_tick_resets_the_hook() {
    let q = shared();
    let reentry = Arc::new(AtomicBool::new(false));
    let (cx, mut future) = queued_future(&q, world().g, SCRATCH, &reentry);
    expect_guest_call(&mut future, &cx, guest_layout::points::GAME_TICK.base);
    assert!(reentry.load(Ordering::SeqCst));
    drop(future);
    assert!(!reentry.load(Ordering::SeqCst));
}

#[test]
fn unreadable_omitted_axis_only_fails_its_edit() {
    let (mut w, id) = vehicle();
    // Only the page containing the parent and the start of the body exists.
    let body = 0x8220_0f90;
    let header = objects::array(&w.g)
        .unwrap()
        .unwrap()
        .get(&w.g, DatumHandle(id.key))
        .unwrap()
        .unwrap();
    header.set_body(&mut w.g, body).unwrap();
    w.g.map(body, 0x60);
    Body::at(body).set_parent(&mut w.g, None).unwrap();

    let q = shared();
    lock(&q).arm();
    let first = submit(
        &q,
        Call::ObjectSet {
            id,
            pos: None,
            forward: Some([1.0, 0.0, 0.0]),
            up: None,
        },
    )
    .unwrap();
    let second = submit(&q, Call::ObjectDelete { id }).unwrap();
    let reentry = Arc::new(AtomicBool::new(false));
    let (cx, mut future) = queued_future(&q, w.g, SCRATCH, &reentry);
    expect_guest_call(&mut future, &cx, addr::OBJECT_DELETE);
    assert_eq!(first.outcome().unwrap().unwrap_err().code, code::UNAVAILABLE);
    guest_layout::entry::resume(&cx, Regs::default());
    expect_guest_call(&mut future, &cx, guest_layout::points::GAME_TICK.base);
    assert!(second.outcome().unwrap().is_ok());
}

#[test]
fn all_65_callers_retain_their_outcomes_from_one_tick() {
    let (w, id) = vehicle();
    let q = shared();
    lock(&q).arm();
    let tickets: Vec<_> = (0..65)
        .map(|_| submit(&q, Call::ObjectDelete { id }).unwrap())
        .collect();
    let reentry = Arc::new(AtomicBool::new(false));
    let (cx, mut future) = queued_future(&q, w.g, SCRATCH, &reentry);
    for _ in &tickets {
        expect_guest_call(&mut future, &cx, addr::OBJECT_DELETE);
        guest_layout::entry::resume(&cx, Regs::default());
    }
    expect_guest_call(&mut future, &cx, guest_layout::points::GAME_TICK.base);
    for ticket in tickets {
        assert!(ticket.outcome().unwrap().is_ok());
        assert!(ticket.outcome().is_none());
    }
}

#[test]
fn scratch_fault_answers_later_drained_calls_without_executing_them() {
    let (w, id) = vehicle();
    let q = shared();
    lock(&q).arm();
    let first = submit(
        &q,
        Call::ObjectSet {
            id,
            pos: Some([1.0; 3]),
            forward: None,
            up: None,
        },
    )
    .unwrap();
    let next = submit(&q, Call::ObjectDelete { id }).unwrap();
    let reentry = Arc::new(AtomicBool::new(false));
    let (cx, mut future) = queued_future(&q, w.g, 0x9020_0000, &reentry);
    assert!(matches!(
        guest_layout::entry::poll(&mut future, &cx),
        guest_layout::entry::Step::Done(Err(EntryError::Fault(_)))
    ));
    assert_eq!(first.outcome().unwrap().unwrap_err().code, code::UNAVAILABLE);
    assert_eq!(next.outcome().unwrap().unwrap_err().code, code::UNAVAILABLE);
    assert!(lock(&q).live().is_none());
}

#[test]
fn dropping_a_drained_waiter_skips_its_call() {
    let (w, id) = vehicle();
    let q = shared();
    lock(&q).arm();
    let first = submit(&q, Call::ObjectDelete { id }).unwrap();
    let next = submit(&q, Call::ObjectDelete { id }).unwrap();
    let reentry = Arc::new(AtomicBool::new(false));
    let (cx, mut future) = queued_future(&q, w.g, SCRATCH, &reentry);
    expect_guest_call(&mut future, &cx, addr::OBJECT_DELETE);
    drop(next);
    guest_layout::entry::resume(&cx, Regs::default());
    expect_guest_call(&mut future, &cx, guest_layout::points::GAME_TICK.base);
    assert!(first.outcome().unwrap().is_ok());
    assert!(lock(&q).live().is_none());
}

#[test]
fn interrupted_options_queue_reconciles_the_binding_before_the_transition() {
    for pending in [PendingOptions::Empty, PendingOptions::Reload, PendingOptions::Other] {
        let (w, options) = reload_world();
        let q = shared();
        lock(&q).arm();
        let effects = Arc::new(Mutex::new(Vec::new()));
        let events = effects.clone();
        let binding = PreparedMapReload::new(42, move || {
            events.lock().unwrap().push("reserve");
            let commit = events.clone();
            Ok(ReservedMapReload::new(
                move || commit.lock().unwrap().push("publish"),
                move || {
                    events.lock().unwrap().push("restore");
                    Ok(())
                },
            ))
        });
        let request = queue_reload(&q, options, Some(binding)).unwrap();
        let (cx, mut future) = queued_future(
            &q, w.g, SCRATCH, &Arc::new(AtomicBool::new(false)),
        );
        expect_guest_call(&mut future, &cx, guest_layout::points::OPTIONS_QUEUE.base);
        let skulls = effects.clone();
        let options = SCRATCH + SCRATCH_OPTIONS;
        assert!(!lock(&q).stage_reload_skulls(options + 4, || || panic!("unrelated options")));
        assert!(lock(&q).stage_reload_skulls(options, || move || {
            skulls.lock().unwrap().push("skulls");
        }));
        cx.write(|m| {
            if pending != PendingOptions::Empty {
                m.write(OPTIONS_PENDING, &[1]).unwrap();
                let src = SCRATCH + SCRATCH_OPTIONS;
                // Simulate a before-hook changing a byte before retail copies it.
                m.write(src + 0x12c, &[5]).unwrap();
                let bytes = m.bytes(src, OPTIONS_SIZE as usize).unwrap();
                m.write(PENDING_OPTIONS, &bytes).unwrap();
                if pending == PendingOptions::Other {
                    m.write(PENDING_OPTIONS + 0x1c, b"other map").unwrap();
                }
            }
        });
        drop(future);
        assert_eq!(*effects.lock().unwrap(), ["reserve"]);
        assert!(request.outcome().is_none());
        assert!(lock(&q).live().is_some());
        reconcile_reload(&q, |options| cx.read(|m| pending_options(m, options)));
        assert_eq!(lock(&q).live().is_some(), pending == PendingOptions::Reload);
        if pending != PendingOptions::Reload {
            assert!(request.outcome().unwrap().is_err());
            assert_eq!(*effects.lock().unwrap(), ["reserve", "restore"]);
        } else {
            assert_eq!(request.outcome().unwrap().unwrap()["queued"], true);
            assert_eq!(*effects.lock().unwrap(), ["reserve", "publish", "skulls"]);
        }
        assert_eq!(cx.read(|m| m.u8(OPTIONS_PENDING)).unwrap(), u8::from(pending != PendingOptions::Empty));
        reconcile_reload(&q, |_| panic!("reconciled twice"));
    }
}

#[test]
fn returning_without_a_pending_reload_leaves_the_skull_selection_alone() {
    let q = shared();
    lock(&q).arm();
    let (world, options) = reload_world();
    let request = queue_reload(&q, options, None).unwrap();
    let (cx, mut future) = queued_future(&q, world.g, SCRATCH, &Arc::new(AtomicBool::new(false)));
    expect_guest_call(&mut future, &cx, guest_layout::points::OPTIONS_QUEUE.base);
    assert!(lock(&q).stage_reload_skulls(SCRATCH + SCRATCH_OPTIONS, || {
        || panic!("an unaccepted reload changed the skulls")
    }));
    guest_layout::entry::resume(&cx, Regs::default());
    expect_guest_call(&mut future, &cx, guest_layout::points::GAME_TICK.base);
    assert!(request.outcome().unwrap().unwrap_err().message.contains("did not queue"));
    assert!(lock(&q).live().is_none());
}

#[test]
fn unreadable_interrupted_reload_finishes_and_releases_the_reservation() {
    let q = shared();
    lock(&q).arm();
    let (world, options) = reload_world();
    let reserved = Arc::new(());
    let released = Arc::downgrade(&reserved);
    let binding = PreparedMapReload::new(42, move || {
        Ok(ReservedMapReload::new(move || drop(reserved), || Ok(())))
    });
    let request = queue_reload(&q, options, Some(binding)).unwrap();
    let (cx, mut future) = queued_future(&q, world.g, SCRATCH, &Arc::new(AtomicBool::new(false)));
    expect_guest_call(&mut future, &cx, guest_layout::points::OPTIONS_QUEUE.base);
    drop(future);
    reconcile_reload(&q, |_| Err("unreadable pending options".into()));
    assert_eq!(request.outcome().unwrap().unwrap_err().message, "unreadable pending options");
    assert!(released.upgrade().is_none());
    assert!(lock(&q).live().is_none());
    reconcile_reload(&q, |_| panic!("a finished ticket must leave later ticks alone"));
}

#[test]
fn partial_cache_retirement_keeps_only_the_repair_after_answering() {
    let q = shared();
    lock(&q).arm();
    let (world, options) = reload_world();
    let reserved = Arc::new(());
    let released = Arc::downgrade(&reserved);
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = attempts.clone();
    let binding = PreparedMapReload::new(42, move || {
        Ok(ReservedMapReload::new(move || drop(reserved), move || {
            if count.fetch_add(1, Ordering::SeqCst) < 2 {
                Err("cache key is unwritable".into())
            } else {
                Ok(())
            }
        }).with_retirement(Err("second key failed".into())))
    });
    let request = queue_reload(&q, options, Some(binding)).unwrap();
    let (cx, mut future) = queued_future(&q, world.g, SCRATCH, &Arc::new(AtomicBool::new(false)));
    expect_guest_call(&mut future, &cx, guest_layout::points::GAME_TICK.base);
    let error = request.outcome().unwrap().unwrap_err();
    assert!(error.message.contains("second key failed"));
    assert!(error.message.contains("cache key is unwritable"));
    assert!(released.upgrade().is_none());
    assert!(lock(&q).live().unwrap().contains("keys to restore"));
    for _ in 0..3 {
        reconcile_reload(&q, |_| panic!("OPTIONS_QUEUE did not run"));
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert!(lock(&q).live().is_none());
}

#[test]
fn a_dropped_object_new_blocks_snapshots_but_a_known_refusal_does_not() {
    for interrupted in [true, false] {
        let (world, occupant) = placed_vehicle();
        let q = shared();
        lock(&q).arm();
        let request = submit(&q, respawn(vehicle_placement())).unwrap();
        let (cx, mut future) = queued_future(&q, world.g, SCRATCH, &Arc::new(AtomicBool::new(false)));
        expect_guest_call(&mut future, &cx, addr::PLACEMENT_NEW);
        guest_layout::entry::resume(&cx, Regs::default());
        expect_guest_call(&mut future, &cx, addr::OBJECT_NEW);
        if interrupted {
            drop(future);
        } else {
            guest_layout::entry::resume(&cx, al(u32::MAX));
            expect_guest_call(&mut future, &cx, guest_layout::points::GAME_TICK.base);
        }
        assert!(request.outcome().unwrap().is_err());
        assert_eq!(lock(&q).preview_refusal().is_some(), interrupted);
        assert!(cx.read(|m| placement::resolve(m, occupant)).is_ok());
    }
}

#[test]
fn interrupted_deletion_refuses_a_third_copy_of_the_placement() {
    let (mut world, old) = placed_vehicle();
    let body = UNIT_BODY + 0x1000;
    world.g.map(body, 0x400);
    let q = shared();
    lock(&q).arm();
    let ticket = submit(&q, respawn(vehicle_placement())).unwrap();
    let (cx, mut future) = queued_future(&q, world.g, SCRATCH, &Arc::new(AtomicBool::new(false)));
    expect_guest_call(&mut future, &cx, addr::PLACEMENT_NEW);
    guest_layout::entry::resume(&cx, Regs::default());
    expect_guest_call(&mut future, &cx, addr::OBJECT_NEW);
    let made = cx.write(|m| {
        let pool = objects::array(m).unwrap().unwrap();
        let made = pool.array.new_datum(m).unwrap().unwrap();
        let header = pool.get(m, made).unwrap().unwrap();
        header.set_kind(m, objects::VEHICLE).unwrap();
        header.set_body(m, body).unwrap();
        Body::at(body).set_parent(m, None).unwrap();
        Body::at(body).set_definition(m, Some(DatumHandle(TAG))).unwrap();
        m.set_u16(body + guest_layout::Object::PLACEMENT_INDEX.offset, u16::MAX).unwrap();
        made
    });
    guest_layout::entry::resume(&cx, al(made.raw()));
    expect_guest_call(&mut future, &cx, addr::OBJECT_DELETE);
    drop(future);
    assert!(ticket.outcome().unwrap().is_err());
    assert!(cx.read(|m| placement::resolve(m, old)).is_ok());
    let error = cx.read(|m| placement::read(m, &vehicle_placement(), &lock(&q).previews)).err().unwrap();
    assert_eq!(error.code, REFUSED);
}

#[test]
fn interrupted_or_faulted_placement_preparation_keeps_the_occupant() {
    for interrupt in [true, false] {
        let (w, id) = placed_vehicle();
        let q = shared();
        lock(&q).arm();
        let request = submit(&q, respawn(vehicle_placement())).unwrap();
        // The first write after placement_data_new returns will fault.
        let (cx, mut future) = queued_future(
            &q, w.g, 0x9020_0000, &Arc::new(AtomicBool::new(false)),
        );
        expect_guest_call(&mut future, &cx, addr::PLACEMENT_NEW);
        if interrupt {
            drop(future);
        } else {
            guest_layout::entry::resume(&cx, Regs::default());
            assert!(matches!(
                guest_layout::entry::poll(&mut future, &cx),
                guest_layout::entry::Step::Done(Err(EntryError::Fault(_)))
            ));
        }
        let error = request.outcome().unwrap().unwrap_err();
        assert_eq!(error.code, code::UNAVAILABLE);
        assert!(error.data.is_none());
        assert!(cx.read(|m| placement::resolve(m, id)).is_ok());
    }
}

#[test]
fn a_forge_spawn_preserves_roll_and_rechecks_the_session_before_entering_the_guest() {
    use crate::forge::local::Guard;
    let scenario = 0x8100_0000;
    let frame = ::forge::Frame {
        position: [2.0, 3.0, 4.0],
        rotation: [0.0, 0.0, std::f32::consts::FRAC_PI_2],
    };
    for (generation, loaded) in [(3, scenario), (2, scenario), (3, scenario + 4)] {
        let mut w = world();
        w.g.put_u32(guest_layout::global::SCENARIO.base, loaded);
        let (out, calls, _) = one(
            w,
            Call::Forge {
                guard: Guard {
                    scenario,
                    generation,
                    admitted: |_, generation| generation == 3,
                },
                steps: vec![ForgeStep::Make { tag: 0xa100_0001, frame }],
            },
            |c, m| {
                assert_eq!((generation, loaded), (3, scenario));
                if c.target == addr::OBJECT_NEW {
                    let placement = c.args[0];
                    assert_eq!(
                        vec3(m, placement + addr::PLACEMENT_POSITION),
                        frame.position
                    );
                    assert_eq!(
                        vec3(m, placement + addr::PLACEMENT_FORWARD),
                        [1.0, 0.0, 0.0]
                    );
                    let up = vec3(m, placement + addr::PLACEMENT_UP);
                    assert!(up[0].abs() < 1e-5 && (up[1] + 1.0).abs() < 1e-5 && up[2].abs() < 1e-5);
                    return al(0x100_0001);
                }
                Regs::default()
            },
        );
        if generation == 3 && loaded == scenario {
            assert_eq!(out.unwrap()["handles"], json!([0x0100_0001]));
            assert_eq!(calls.len(), 2);
        } else {
            assert!(out.unwrap_err().message.contains("session changed"));
            assert!(calls.is_empty());
        }
    }
}

#[test]
fn a_forge_batch_releases_its_own_objects_when_a_later_placement_fails() {
    let scenario = 0x8100_0000;
    let mut w = world();
    w.g.put_u32(guest_layout::global::SCENARIO.base, scenario);
    let guard = crate::forge::local::Guard {
        scenario,
        generation: 0,
        admitted: |_, _| true,
    };
    let mut created = 0;
    let (out, calls, _) = one(
        w,
        Call::Forge {
            guard,
            steps: vec![ForgeStep::Make { tag: 0xa100_0001, frame: ::forge::Frame::default() }; 3],
        },
        |c, _| {
            if c.target == addr::OBJECT_NEW {
                created += 1;
                return al(if created == 3 {
                    u32::MAX
                } else {
                    0x1000_0000 + created
                });
            }
            Regs::default()
        },
    );
    assert!(out.unwrap_err().message.contains("object_new"));
    let deleted: Vec<_> = calls
        .iter()
        .filter(|c| c.target == addr::OBJECT_DELETE)
        .map(|c| c.args[0])
        .collect();
    assert_eq!(deleted, [0x1000_0002, 0x1000_0001]);
}

#[test]
fn a_forge_channel_writes_only_a_teleporter_flag_identifier() {
    let scenario = 0x8100_0000;
    let flags = 0x8200_0000;
    // Flag 1's identifier is the low half of the word at +0x14; the high
    // half belongs to the next field and must survive.
    for (kind, flag, written) in [(7, 1, true), (8, 1, true), (2, 1, false), (7, 2, false)] {
        let mut w = world();
        w.g.put_u32(guest_layout::global::SCENARIO.base, scenario)
            .put_u32(scenario + 0x118, 2)
            .put_u32(scenario + 0x11c, flags)
            .put_u32(flags + 0x30, kind)
            .put_u32(flags + 0x34, 0xabcd_0003);
        let guard = crate::forge::local::Guard { scenario, generation: 0, admitted: |_, _| true };
        let (out, calls, g) = one(
            w,
            Call::Forge { guard, steps: vec![ForgeStep::Channel { flag, channel: 9 }] },
            |_, _| Regs::default(),
        );
        assert!(calls.is_empty());
        if written {
            assert_eq!(out.unwrap()["handles"], json!([]));
            assert_eq!(g.u32(flags + 0x34).unwrap(), 0xabcd_0009);
        } else {
            let message = out.unwrap_err().message;
            assert!(message.contains("not a teleporter") || message.contains("no netgame flag 2"), "{message}");
            assert_eq!(g.u32(flags + 0x34).unwrap(), 0xabcd_0003);
        }
    }
}

/// The vehicle fixture inside an admitted local Forge session.
fn forge_vehicle() -> (World, ObjectId, crate::forge::local::Guard) {
    let (mut w, id) = vehicle();
    let scenario = 0x8100_0000;
    w.g.put_u32(guest_layout::global::SCENARIO.base, scenario);
    let guard = crate::forge::local::Guard { scenario, generation: 0, admitted: |_, _| true };
    (w, id, guard)
}

#[test]
fn a_forge_move_and_delete_are_the_world_object_calls_on_the_live_object() {
    let frame = ::forge::Frame { position: [2.0, 3.0, 4.0], rotation: [std::f32::consts::FRAC_PI_2, 0.0, 0.0] };
    let (w, id, guard) = forge_vehicle();
    let target = ForgeTarget::Live(id);
    let (out, calls, _) = one(
        w,
        Call::Forge { guard, steps: vec![ForgeStep::Move { target, frame }] },
        |c, m| {
            assert_eq!(c.target, addr::OBJECT_SET_POSITION);
            assert_eq!(c.args, [id.key, 0, 0]);
            assert_eq!(vec3(m, c.eax.unwrap()), frame.position);
            let forward = vec3(m, c.ecx.unwrap());
            assert!(forward[0].abs() < 1e-5 && (forward[1] - 1.0).abs() < 1e-5);
            assert_eq!(vec3(m, c.edx.unwrap()), [0.0, 0.0, 1.0]);
            al(0)
        },
    );
    assert_eq!(calls.len(), 1);
    assert_eq!(out.unwrap()["handles"], json!([]));

    let (w, id, guard) = forge_vehicle();
    let (out, calls, _) = one(
        w,
        Call::Forge { guard, steps: vec![ForgeStep::Delete { target: ForgeTarget::Live(id) }] },
        |_, _| al(0),
    );
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].target, addr::OBJECT_DELETE);
    assert_eq!(calls[0].args, [id.key]);
    assert_eq!(out.unwrap()["handles"], json!([]));
}

#[test]
fn a_forge_restore_makes_its_object_before_the_batch_deletes() {
    let (w, id, guard) = forge_vehicle();
    let frame = ::forge::Frame { position: [1.0; 3], ..Default::default() };
    let (out, calls, _) = one(
        w,
        Call::Forge {
            guard,
            steps: vec![
                ForgeStep::Delete { target: ForgeTarget::Live(id) },
                ForgeStep::Make { tag: 0xa100_0001, frame },
            ],
        },
        |c, _| if c.target == addr::OBJECT_NEW { al(0x1000_0007) } else { al(0) },
    );
    let targets: Vec<_> = calls.iter().map(|c| c.target).collect();
    assert_eq!(targets, [addr::PLACEMENT_NEW, addr::OBJECT_NEW, addr::OBJECT_DELETE]);
    assert_eq!(calls[2].args, [id.key]);
    assert_eq!(out.unwrap()["handles"], json!([0x1000_0007]));
}

#[test]
fn a_stale_forge_object_is_a_named_refusal_before_any_guest_call() {
    let frame = ::forge::Frame::default();
    let cases: [fn(ObjectId) -> Vec<ForgeStep>; 2] = [
        |stale| vec![ForgeStep::Move { target: ForgeTarget::Live(stale), frame: ::forge::Frame::default() }],
        |stale| vec![
            ForgeStep::Make { tag: 0xa100_0001, frame: ::forge::Frame::default() },
            ForgeStep::Delete { target: ForgeTarget::Live(stale) },
        ],
    ];
    for steps in cases {
        let (w, id, guard) = forge_vehicle();
        let stale = ObjectId { key: id.key ^ (1 << 16), ..id };
        let (out, calls, _) = one(w, Call::Forge { guard, steps: steps(stale) }, |_, _| {
            panic!("a stale Forge object reached the game")
        });
        assert!(calls.is_empty());
        let error = out.unwrap_err();
        assert_eq!(error.code, REFUSED);
        assert!(error.message.starts_with(crate::forge::local::OBJECT_GONE), "{}", error.message);
        assert!(error.message.contains(&stale.to_string()));
    }
    let (w, id, guard) = forge_vehicle();
    let wrong = ObjectId { kind: agent_proto::Kind::Crate, ..id };
    let (out, calls, _) = one(
        w,
        Call::Forge { guard, steps: vec![ForgeStep::Move { target: ForgeTarget::Live(wrong), frame }] },
        |_, _| panic!("a Forge object of another kind reached the game"),
    );
    assert!(calls.is_empty());
    assert!(out.unwrap_err().message.starts_with(crate::forge::local::OBJECT_GONE));
}

#[test]
fn probe_animation_uses_the_units_graph_and_resolves_the_engine_selection() {
    use guest_layout::tags::{GroupTag, TAG_ENTRIES_GLOBAL, TAG_INDEX_GLOBAL};
    let mut w = world();
    let tag = 0x45670000;
    let group = GroupTag::from_chars(*b"jmad").0;
    w.g.put_u16(
        UNIT_BODY + live_telemetry::layout::OBJECT_ANIMATION_MANAGER,
        0x200,
    )
    .put_u32(
        UNIT_BODY + 0x200 + live_telemetry::layout::MANAGER_GRAPH,
        tag,
    )
    .put_u32(TAG_INDEX_GLOBAL, 0x6000)
    .put_u32(TAG_ENTRIES_GLOBAL, 0x7000)
    .put_u32(0x6000, 0x8000)
    .put_u32(0x6004, 1)
    .put_u32(0x6018, 1)
    .put_u32(0x7000, group)
    .put_u32(0x7004, tag)
    .put_u32(0x7008, 0x9000)
    .put_u32(0x8000, group)
    .put_u32(0xa000, 77)
    .put_u16(0xa014, 31);
    let unit = w.unit;
    let (out, calls, _) = one(
        w,
        Call::Animation {
            unit,
            path: [1, 2, 3, 4],
            overlay: true,
        },
        |call, m| match call.target {
            0x1dbc80 => {
                assert_eq!(call.eax, Some(4));
                assert_eq!(&call.args[..5], &[0x9000, SCRATCH, 1, 2, 3]);
                for (i, value) in [0x0003ffff, 1, 2, 3].into_iter().enumerate() {
                    m.set_u32(SCRATCH + 4 * i as u32, value).unwrap();
                }
                al(SCRATCH)
            }
            0x1daea0 => {
                assert_eq!(call.edx, Some(0x9000));
                al(0xa000)
            }
            _ => panic!("unexpected guest call"),
        },
    );
    assert_eq!(calls.len(), 2);
    let out = out.unwrap();
    assert_eq!(out["index"], 3);
    assert_eq!(out["frames"], 31);
    assert_eq!(out["animation"], 77);
    assert_eq!(out["selected"], json!([1, 2, 3]));
}


/// A game with structure BSP `loaded` loaded and `active` active: the
/// game globals at [`GLOBALS`].
const GLOBALS: u32 = 0x8050_0000;

fn structure_world(loaded: i16, active: i16) -> World {
    let mut w = world();
    w.g.map(guest_layout::points::GAME_GLOBALS.base, 4);
    w.g.put_u32(guest_layout::points::GAME_GLOBALS.base, GLOBALS);
    w.g.map(GLOBALS, 0x10);
    w.g.put_u16(GLOBALS + addr::GAME_GLOBALS_ACTIVE_BSP, active as u16);
    w.g.map(addr::LOADED_STRUCTURE_BSP, 2);
    w.g.put_u16(addr::LOADED_STRUCTURE_BSP, loaded as u16);
    w
}

/// The callees as the game's: deactivate sets the active BSP to -1,
/// activate to `to` (the loaded one, in the game).
fn structure_callees(to: i16) -> impl FnMut(&GuestCall, &mut dyn GuestMemory) -> Regs {
    move |c, m| {
        let at = GLOBALS + addr::GAME_GLOBALS_ACTIVE_BSP;
        if c.target == guest_layout::points::STRUCTURE_BSP_DEACTIVATE.base {
            m.set_u16(at, u16::MAX).unwrap();
        } else if c.target == guest_layout::points::STRUCTURE_BSP_ACTIVATE.base {
            m.set_u16(at, to as u16).unwrap();
        }
        Regs::default()
    }
}

#[test]
fn reactivating_the_structure_deactivates_then_activates_the_loaded_bsp() {
    let (out, calls, g) = one(structure_world(1, 1), Call::ReactivateStructure, structure_callees(1));
    assert_eq!(out.unwrap(), json!({"bsp": 1}));
    let targets: Vec<u32> = calls.iter().map(|c| c.target).collect();
    assert_eq!(
        targets,
        [
            guest_layout::points::STRUCTURE_BSP_DEACTIVATE.base,
            guest_layout::points::STRUCTURE_BSP_ACTIVATE.base
        ]
    );
    // Neither takes an argument.
    assert!(calls.iter().all(|c| c.args.is_empty()));
    assert_eq!(g.u16(GLOBALS + addr::GAME_GLOBALS_ACTIVE_BSP).unwrap(), 1);
    // The loaded BSP is the game's to change: the call leaves it.
    assert_eq!(g.u16(addr::LOADED_STRUCTURE_BSP).unwrap(), 1);
}

#[test]
fn reactivating_needs_a_loaded_active_structure_and_checks_what_it_left() {
    // Nothing loaded, or a switch half done: refused before any call.
    for (loaded, active) in [(-1, -1), (0, -1), (1, 0)] {
        let (out, calls, _) = one(structure_world(loaded, active), Call::ReactivateStructure, structure_callees(loaded));
        assert_eq!(out.unwrap_err().code, REFUSED, "{loaded} {active}");
        assert!(calls.is_empty());
    }
    // An activation that leaves another BSP active is an error.
    let (out, calls, _) = one(structure_world(0, 0), Call::ReactivateStructure, structure_callees(-1));
    assert!(out.unwrap_err().message.contains("left BSP -1 active"));
    assert_eq!(calls.len(), 2);
}

mod base;
mod definition;
mod same_calls;

#[test]
fn a_sound_is_started_at_the_units_head_marker_as_a_dialogue_line_is() {
    let w = world();
    let unit = w.unit;
    let (out, calls, _) = one(
        w,
        Call::Sound {
            unit,
            sound: 0xe400_0040,
        },
        |c, _| {
            assert_eq!(c.target, addr::SOUND_START_AT_MARKER);
            al(0x00cc_0001)
        },
    );
    assert_eq!(out.unwrap()["instance"], "00cc0001");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].esi, Some(unit));
    assert_eq!(calls[0].eax, Some(addr::HEAD_MARKER));
    // The pointer, then the four words it points to.
    assert_eq!(
        calls[0].args[1..],
        [u32::MAX, 0xe400_0040, 1.0f32.to_bits(), 0]
    );
}

#[test]
fn a_sound_the_game_does_not_start_is_an_error() {
    let w = world();
    let unit = w.unit;
    let (out, _, _) = one(
        w,
        Call::Sound { unit, sound: 7 },
        |_, _| al(u32::MAX),
    );
    assert!(out.unwrap_err().message.contains("started no sound"));
}

#[test]
fn a_recording_is_played_by_the_recording_player_with_the_units_index_and_flags() {
    let w = world();
    let unit = w.unit;
    let (out, calls, _) = one(
        w,
        Call::Recording {
            unit,
            index: 3,
            flags: 8,
        },
        |c, _| {
            assert_eq!(c.target, addr::RECORDING_PLAYER);
            al(1)
        },
    );
    assert_eq!(out.unwrap()["index"], 3);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].eax, Some(unit));
    assert_eq!(calls[0].ecx, Some(3));
    // The play flags are the one stack argument, after the return slot.
    assert_eq!(calls[0].args, [8]);
}

#[test]
fn a_recording_the_player_refuses_is_an_error() {
    let w = world();
    let unit = w.unit;
    let (out, _, _) = one(
        w,
        Call::Recording {
            unit,
            index: 9,
            flags: 0,
        },
        |_, _| al(0x7700),
    );
    assert!(out.unwrap_err().message.contains("started no recording 9"));
}

/// A running map whose globals name a tag at `matg +0x16c`; `inner` is the
/// pointer the variant name lookup (`0x19d2d7`) reads from that tag's data.
fn variant_world(inner: u32) -> World {
    const MATG: u32 = 0x0300_0000;
    const TABLE: u32 = 0x0301_0000;
    const DATA: u32 = 0x0302_0000;
    let mut w = world();
    for (at, len) in [(0x004e_034c, 4), (0x004e_3b44, 4), (MATG, 0x200), (TABLE, 0x40), (DATA, 8)] {
        w.g.map(at, len);
    }
    w.g.put_u32(0x004e_034c, MATG);
    w.g.put_u32(0x004e_3b44, TABLE);
    w.g.put_u32(MATG + 0x16c, 1);
    w.g.put_u32(TABLE + 0x10 + 8, DATA);
    w.g.put_u32(DATA + 4, inner);
    w
}

#[test]
fn a_variant_built_on_a_map_without_the_name_tag_skips_the_lookup() {
    const VARIANT: u32 = 0x0019_d220;
    let step = GameStep::Call {
        target: VARIANT,
        args: vec![Arg::Options(0x134), Arg::Value(0)],
        eax: None,
        esi: None,
        report: None,
    };
    let seen = Arc::new(Mutex::new(None));
    let s2 = Arc::clone(&seen);
    // The campaign map's tag holds a null where the lookup reads.
    let (out, _, g) = one(
        variant_world(0),
        Call::QueueGame { steps: vec![step.clone()] },
        move |_, m| {
            *s2.lock().unwrap() = Some(m.u32(0x004e_034c).unwrap());
            Regs::default()
        },
    );
    out.unwrap();
    assert_eq!(*seen.lock().unwrap(), Some(0), "matg hidden during the call");
    assert_eq!(g.u32(0x004e_034c).unwrap(), 0x0300_0000, "and put back after");
    // A map that has the tag keeps its name lookup.
    let seen = Arc::new(Mutex::new(None));
    let s2 = Arc::clone(&seen);
    let (out, _, _) = one(
        variant_world(0x0302_0000),
        Call::QueueGame { steps: vec![step] },
        move |_, m| {
            *s2.lock().unwrap() = Some(m.u32(0x004e_034c).unwrap());
            Regs::default()
        },
    );
    out.unwrap();
    assert_eq!(*seen.lock().unwrap(), Some(0x0300_0000));
}

/// A world with a second object, the body `OBJECT_NEW` gives a Forge body
/// swap, in a session the guard admits.
fn body_world() -> (World, u32, crate::forge::local::Guard) {
    const BODY: u32 = 0x8030_0000;
    let scenario = 0x8100_0000;
    let mut w = world();
    w.g.put_u32(guest_layout::global::SCENARIO.base, scenario);
    let objects = guest_layout::view::objects::array(&w.g).unwrap().unwrap();
    let (body_h, body_hdr) = fake::push(&mut w.g, &objects);
    w.g.map(BODY, 0x400);
    body_hdr.set_body(&mut w.g, BODY).unwrap();
    Body::at(BODY).set_parent(&mut w.g, None).unwrap();
    let guard = crate::forge::local::Guard {
        scenario,
        generation: 0,
        admitted: |_, _| true,
    };
    (w, body_h.raw(), guard)
}

/// Enter makes the body with the ownerless spawn, gives it to the player
/// with `player_set_unit_index` and turns it and the view; the biped it
/// replaced is answered, not deleted.
#[test]
fn a_forge_body_is_made_then_given_to_the_player() {
    let (w, made, guard) = body_world();
    let (player, biped) = (w.player, w.unit);
    let (out, calls, _) = one(
        w,
        Call::ForgeBody {
            guard,
            player,
            step: BodyStep::Enter {
                tag: 0xe300_0001,
                pos: [1.0, 2.0, 3.0],
                forward: [0.0, 1.0, 0.0],
            },
        },
        |c, _| {
            if c.target == addr::OBJECT_NEW {
                return al(made);
            }
            Regs::default()
        },
    );
    let out = out.unwrap();
    assert_eq!(out["body"], hex(made));
    assert_eq!(out["biped"], hex(biped));
    let targets: Vec<u32> = calls.iter().map(|c| c.target).collect();
    assert_eq!(
        targets,
        [
            addr::PLACEMENT_NEW,
            addr::OBJECT_NEW,
            addr::PLAYER_SET_UNIT,
            addr::PLAYER_SET_FACING,
            addr::OBJECT_SET_POSITION,
        ]
    );
    assert_eq!(calls[2].ebx, Some(made));
    assert_eq!(calls[2].args, vec![player]);
    assert!(!targets.contains(&addr::OBJECT_DELETE), "the biped is kept");
}

/// Leave gives the kept biped back, places it where the body is, level on
/// the body's heading, then deletes the body. A body the player is not in
/// is refused before any call.
#[test]
fn leaving_a_forge_body_puts_the_biped_where_the_body_is() {
    const BODY: u32 = 0x8030_0000;
    let (mut w, made, guard) = body_world();
    let (player, biped) = (w.player, w.unit);
    let players = guest_layout::view::players::array(&w.g).unwrap().unwrap();
    let p = players.get(&w.g, DatumHandle(player)).unwrap().unwrap();
    p.set_unit(&mut w.g, DatumHandle::from_raw(made)).unwrap();
    Body::at(BODY).set_position(&mut w.g, [4.0, 5.0, 6.0]).unwrap();
    Body::at(BODY).set_forward(&mut w.g, [0.6, 0.0, 0.8]).unwrap();
    let (out, calls, g) = one(
        w,
        Call::ForgeBody {
            guard,
            player,
            step: BodyStep::Leave { body: made, biped },
        },
        |c, m| {
            if c.target == addr::PLAYER_PLACE_NEAR {
                assert_eq!(vec3(m, c.esi.unwrap()), [4.0, 5.0, 6.0]);
                return al(1);
            }
            Regs::default()
        },
    );
    let out = out.unwrap();
    assert_eq!(out["placed"], true);
    let targets: Vec<u32> = calls.iter().map(|c| c.target).collect();
    assert_eq!(
        targets,
        [
            addr::PLAYER_SET_UNIT,
            addr::PLAYER_PLACE_NEAR,
            addr::PLAYER_SET_FACING,
            addr::OBJECT_SET_POSITION,
            addr::OBJECT_DELETE,
        ]
    );
    assert_eq!(calls[0].ebx, Some(biped));
    assert_eq!(calls[4].args, vec![made]);
    assert_eq!(vec3(&g, UNIT_BODY + addr::UNIT_FACINGS[0]), [1.0, 0.0, 0.0]);

    // The player's unit is the biped: a leave of the body is refused.
    let (w, made, guard) = body_world();
    let (player, biped) = (w.player, w.unit);
    let (out, calls, _) = one(
        w,
        Call::ForgeBody {
            guard,
            player,
            step: BodyStep::Leave { body: made, biped },
        },
        |_, _| Regs::default(),
    );
    assert!(out.unwrap_err().message.contains("not in that Forge body"));
    assert!(calls.is_empty());
}
