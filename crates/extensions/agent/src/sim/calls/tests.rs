//! The calls over a fake guest, the callees played by the tests as the
//! real ones behave (engine's `entry::testing::run`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use engine::GuestMemory;
use engine::entry::testing::run as drive;
use engine::entry::{Cx, EntryFn, EntryFuture, GuestCall, Regs, Ret};
use engine::testing::FakeGuest;
use engine::view::fake;
use engine::view::objects::{Body, Header};
use engine::view::players::Player;
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
    let got: Arc<Mutex<Option<Outcome>>> = Arc::default();
    let g2 = Arc::clone(&got);
    let f: Box<EntryFn> = Box::new(move |cx: Cx| -> EntryFuture {
        let (call, got) = (call.clone(), Arc::clone(&g2));
        Box::pin(async move {
            let out = run(&cx, &call, SCRATCH).await?;
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
    assert!(out.unwrap_err().contains("0x14e970"));
    assert_eq!(calls.len(), 1, "no turn after a refused placement");
}

#[test]
fn a_seated_unit_moves_only_by_its_position_and_says_why_it_did_not_turn() {
    let mut w = world();
    Body::at(UNIT_BODY)
        .set_parent(&mut w.g, Some(engine::handle::DatumHandle(0x1234_0005)))
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
    let p = engine::view::players::array(&w.g)
        .unwrap()
        .unwrap()
        .get(&w.g, engine::handle::DatumHandle(w.player))
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
    assert!(out.unwrap_err().contains("between lives"));
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
    assert!(out.unwrap_err().contains("deleted"));
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
    assert!(out.unwrap_err().contains("object_new"));
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
fn queued_calls_run_then_the_tick_with_reentry_set() {
    let w = world();
    let q = shared();
    assert!(
        lock(&q)
            .submit(Call::Give {
                unit: 1,
                weapon: 2,
                mode: 1
            })
            .is_err(),
        "unarmed"
    );
    lock(&q).arm();
    let player = w.player;
    let t = lock(&q)
        .submit(Call::Teleport {
            player,
            pos: Some([1.0, 1.0, 1.0]),
            facing: None,
        })
        .unwrap();
    assert!(lock(&q).live().is_some(), "queued calls block a snapshot");
    let work = lock(&q).drain();
    assert!(lock(&q).live().is_none());
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
        if c.target == engine::points::GAME_TICK.base {
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
        engine::points::GAME_TICK.base
    );
    assert!(!reentry.load(Ordering::SeqCst));
    assert!(lock(&q).take(t).unwrap().is_ok());
    assert!(lock(&q).take(t).is_none(), "taken once");
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
    assert!(out.unwrap_err().contains("past the options block"));
    assert!(calls.is_empty());
}
