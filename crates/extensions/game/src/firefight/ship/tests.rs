//! The guest half over invented guest memory and fake engine callees.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use super::*;
use crate::firefight::guest::read_kit;
use guest_layout::entry::{EntryFuture, Regs, Ret, testing};
use guest_layout::testing::FakeGuest;

const SCNR: u32 = 0x1_0000;
const SQUADS: u32 = 0x2_0000;
const STARTS: u32 = 0x2_1000;
const ZONES: u32 = 0x2_2000;
const AREAS: u32 = 0x2_3000;
const FIRING: u32 = 0x2_4000;
const ORDERS: u32 = 0x2_5000;

type Body<T> = Pin<Box<dyn Future<Output = anyhow::Result<T>> + Send>>;

/// Run one async body as an entry; `callee` answers each guest call.
fn drive<T: Send + 'static>(
    guest: FakeGuest,
    body: impl Fn(Cx) -> Body<T> + Send + Sync + 'static,
    callee: impl FnMut(&GuestCall, &mut dyn GuestMemory) -> Regs,
) -> (anyhow::Result<T>, Vec<GuestCall>, FakeGuest) {
    let out = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&out);
    let entry = move |cx: Cx| -> EntryFuture {
        let fut = body(cx);
        let slot = Arc::clone(&slot);
        Box::pin(async move {
            let r = fut.await;
            *slot.lock().unwrap() = Some(r);
            Ok(Ret::new(0, 0))
        })
    };
    let run = testing::run(&entry, guest, Regs::default(), callee);
    assert_eq!(run.result, Ok(Ret::new(0, 0)));
    let result = out.lock().unwrap().take().expect("the body finished");
    (result, run.calls, run.guest)
}

fn returns(eax: u32) -> impl FnMut(&GuestCall, &mut dyn GuestMemory) -> Regs {
    move |_, _| Regs {
        eax,
        ..Default::default()
    }
}

fn named(m: &mut FakeGuest, at: u32, name: &str) {
    m.put(at, name.as_bytes()).put_u8(at + name.len() as u32, 0);
}

fn pair(m: &mut FakeGuest, at: u32, count: u32, address: u32) {
    m.put_u32(at, count).put_u32(at + 4, address);
}

/// A scenario with one base squad and two kit slots; a zone of the base
/// map, one drop zone of three firing positions and the two base zones
/// (base 1's first); three palette rows, named by the kit's class squads
/// (the Elite, a Grunt and a Hunter, and a squad naming a row the palette
/// lacks); and an order of the base map before the two hunt orders (base
/// 1's first).
fn scenario() -> FakeGuest {
    let mut m = FakeGuest::new();
    m.map(SCNR, 0x300);
    m.put_u32(points::SCENARIO.base, SCNR);
    pair(&mut m, SCNR + 0x160, 7, SQUADS);
    m.map(SQUADS, 7 * 0x74);
    for (i, name) in [
        "marines",
        "ff_slot_00",
        "ff_slot_01",
        "ff_class_elite",
        "ff_class_grunt",
        "ff_class_hunter",
        "ff_class_ghost",
    ]
    .into_iter()
    .enumerate()
    {
        named(&mut m, SQUADS + 0x74 * i as u32, name);
    }
    for (squad, row) in [(3u32, 0u16), (4, 1), (5, 2), (6, 9)] {
        m.put_u16(SQUADS + 0x74 * squad + 0x36, row);
        // Each record squad is on its class's faction's team (Covenant, 3).
        m.put_u16(SQUADS + 0x74 * squad + 0x24, 3);
    }
    let slot0 = SQUADS + 0x74;
    m.put_u16(slot0 + 0x24, 3)
        .put_u16(slot0 + 0x36, 0xffff)
        .put_u16(slot0 + 0x38, 0xffff);
    pair(&mut m, slot0 + 0x48, 2, STARTS);
    m.map(STARTS, 2 * 0x64);
    m.put_u16(SQUADS + 2 * 0x74 + 0x24, 3);

    pair(&mut m, SCNR + 0x168, 4, ZONES);
    m.map(ZONES, 4 * 0x38);
    named(&mut m, ZONES, "base_zone");
    let zone = ZONES + 0x38;
    named(&mut m, zone, "ff_drop_00");
    pair(&mut m, zone + 0x28, 3, FIRING);
    pair(&mut m, zone + 0x30, 1, AREAS);
    m.map(AREAS, 3 * 0x88);
    for (k, v) in [1.0, 2.0, 3.0].into_iter().enumerate() {
        m.put_f32(AREAS + 0x24 + 4 * k as u32, v);
    }
    m.map(FIRING, 5 * 0x20);
    for i in 0..5u32 {
        m.put_f32(FIRING + 0x20 * i, 10.0 * (i + 1) as f32)
            .put_u16(FIRING + 0x20 * i + 0x0c, 0xffff);
    }
    // Zone 2 is base 1's, zone 3 the home base's: one area and one firing
    // position each.
    for (z, name, x) in [(2u32, "ff_base_01", 90.0), (3, "ff_base_00", -90.0)] {
        let zone = ZONES + 0x38 * z;
        let area = AREAS + 0x88 * (z - 1);
        named(&mut m, zone, name);
        pair(&mut m, zone + 0x28, 1, FIRING + 0x20 * (z + 1));
        pair(&mut m, zone + 0x30, 1, area);
        m.put_f32(area + 0x24, x);
    }

    pair(&mut m, SCNR + 0x178, 3, 0x2_6000);
    crate::firefight::guest::fake_characters(&mut m, 0x2_6000, 3);
    pair(&mut m, SCNR + 0x240, 3, ORDERS);
    m.map(ORDERS, 3 * 0x7c);
    named(&mut m, ORDERS, "ff_other");
    named(&mut m, ORDERS + 0x7c, "ff_hunt_01");
    named(&mut m, ORDERS + 2 * 0x7c, "ff_hunt_00");
    m
}

use guest_layout::view::fake;
use guest_layout::view::objects::{Body as Obj, Header};
use guest_layout::actor::Actor;

const ACTORS: u32 = 0x7000_0100;
const OBJECTS: u32 = 0x7100_0100;
const BODIES: u32 = 0x7200_0000;
const SHIP_AT: [f32; 3] = [10.0, 20.0, 30.0];
const OTHER: u16 = 0x7fff;

/// The kit's slot-0 squad, its index in the scenario.
const SQUAD: u16 = 1;

/// A game with the kit, a ship of `capacity` seats with `riders` units
/// already attached, and `pool` actors the callee can turn into the slot
/// squad's (the engine's placement), unattached.
struct World {
    m: FakeGuest,
    ship: u32,
    pool: Vec<(Actor, Obj, u32)>,
}

fn world(riders: u8, pool: usize) -> World {
    let mut m = scenario();
    let actors = fake::datums::<Actor>(&mut m, ACTORS, 32);
    let objects = fake::datums::<Header>(&mut m, OBJECTS, 32);
    m.put_u32(guest_layout::global::ACTORS.base, actors.array.addr)
        .put_u32(points::OBJECTS.base, objects.array.addr);
    let mut next = BODIES;
    let mut make = |m: &mut FakeGuest, kind: u8| {
        let (handle, header) = fake::push(m, &objects);
        m.map(next, 0x1000);
        header.set_kind(m, kind).unwrap();
        header.set_body(m, next).unwrap();
        let body = Obj::at(next);
        body.set_parent(m, None).unwrap();
        next += 0x1000;
        (handle, body)
    };
    let (ship, ship_body) = make(&mut m, guest_layout::view::objects::VEHICLE);
    ship_body.set_position(&mut m, SHIP_AT).unwrap();
    ship_body.set_up(&mut m, [0.0, 0.0, 1.0]).unwrap();
    // An actor and its unit, linked both ways as the engine's actors are, so
    // the guest's own count of a squad's actors sees them.
    let actor_of = |m: &mut FakeGuest, squad: u32, unit: DatumHandle, body: Obj| {
        let (id, actor) = fake::push(m, &actors);
        actor.set_unit(m, Some(unit)).unwrap();
        actor.set_squad(m, squad).unwrap();
        let raw = guest_layout::Object::at(body.addr);
        let view = raw.as_unit();
        view.set(m, guest_layout::Unit::ACTOR, guest_layout::Datum(id.raw())).unwrap();
        view.set(m, guest_layout::Unit::PLAYER, guest_layout::Datum::NONE).unwrap();
        raw.set(m, guest_layout::Object::ENTITY, u32::MAX).unwrap();
        actor
    };
    for _ in 0..riders {
        let (unit, body) = make(&mut m, guest_layout::view::objects::BIPED);
        body.set_parent(&mut m, Some(ship)).unwrap();
        actor_of(&mut m, u32::from(OTHER), unit, body);
    }
    let pool = (0..pool)
        .map(|_| {
            let (unit, body) = make(&mut m, guest_layout::view::objects::BIPED);
            let actor = actor_of(&mut m, u32::from(OTHER), unit, body);
            (actor, body, unit.raw())
        })
        .collect();
    World { m, ship: ship.raw(), pool }
}

/// The engine as the callee: each `AI_SQUAD_PLACE` turns the next `eax` pool
/// actors into the squad's (the on-foot rest is a second call); `VEHICLE_LOAD` attaches the
/// first `seats` of them to the ship and answers how many it did.
struct Engine {
    pool: Vec<(Actor, Obj, u32)>,
    ship: u32,
    seats: u16,
    placed: usize,
}

impl Engine {
    fn answer(&mut self, c: &GuestCall, m: &mut dyn GuestMemory) -> Regs {
        // Every call the body makes fits its callee's signature.
        if let Some(p) = points::by_base(c.target) {
            assert_eq!(c.shape_error(&p.sig), None, "the call to {}", p.name);
        }
        let eax = if c.target == points::AI_SQUAD_PLACE.base {
            let take = c.eax.unwrap() as usize;
            for (actor, _, _) in self.pool.iter().skip(self.placed).take(take) {
                actor.set_squad(m, u32::from(SQUAD)).unwrap();
            }
            self.placed += take;
            0
        } else if c.target == points::OBJECT_LIST_FROM_AI.base {
            0x1234
        } else if c.target == points::VEHICLE_LOAD.base {
            let mut n = 0;
            for (actor, body, _) in &self.pool {
                if n < self.seats && actor.squad(m).unwrap() == u32::from(SQUAD) {
                    body.set_parent(m, DatumHandle::from_raw(self.ship)).unwrap();
                    n += 1;
                }
            }
            u32::from(n)
        } else {
            0
        };
        Regs { eax, ..Default::default() }
    }
}

fn boarding(count: u16, capacity: u8, ship: u32) -> Boarding {
    Boarding {
        placement: Placement { slot: 0, character: 0, count, zone: 0, team: 5, weapon_tier: 0 },
        ship,
        capacity,
    }
}

fn run(w: World, b: Boarding, seats: u16, whole: bool) -> (anyhow::Result<u8>, Vec<GuestCall>, FakeGuest) {
    let kit = read_kit(&w.m).unwrap().expect("the kit");
    let mut engine = Engine { pool: w.pool, ship: w.ship, seats, placed: 0 };
    drive(
        w.m,
        move |cx| {
            let kit = kit.clone();
            Box::pin(async move {
                if whole { drop_squad(&cx, &kit, b).await } else { place_squad_in(&cx, &kit, b).await }
            })
        },
        move |c, m| engine.answer(c, m),
    )
}

fn deletes(calls: &[GuestCall]) -> Vec<u32> {
    calls
        .iter()
        .filter(|c| c.target == points::OBJECT_DELETE.base)
        .map(|c| c.args[0])
        .collect()
}

#[test]
fn free_seats_are_the_capacity_less_the_units_attached() {
    let w = world(1, 0);
    assert_eq!(free_seats(&w.m, w.ship, 4), Some(3));
    assert_eq!(free_seats(&w.m, w.ship, 1), Some(0));
    assert_eq!(free_seats(&w.m, w.ship, 0), Some(0));
    assert_eq!(ship_position(&w.m, w.ship), Some(SHIP_AT));
    // A handle that is no vehicle has no seats.
    let unit = squad_units(&w.m, OTHER)[0];
    assert_eq!(free_seats(&w.m, unit, 4), None);
}

#[test]
fn place_squad_in_seats_at_most_the_free_seats() {
    let w = world(1, 5);
    let ship = w.ship;
    let (result, calls, m) = run(w, boarding(6, 4, ship), 3, false);
    assert_eq!(result.unwrap(), 3);
    assert_eq!(
        calls,
        [
            GuestCall::new(points::AI_SQUAD_PLACE.base).eax(3).args([1, u32::MAX, 0, 1]),
            GuestCall::new(points::AI_SET_TEAM.base).eax(1).ebx(5),
            GuestCall::new(points::OBJECT_LIST_FROM_AI.base).args([1]),
            GuestCall::new(points::VEHICLE_LOAD.base).edx(ship).args([u32::MAX, 0x1234]),
        ]
    );
    // The squad's starts are at the ship.
    assert_eq!(m.vec3(STARTS + 4).unwrap(), SHIP_AT);
    assert_eq!(m.vec3(STARTS + 0x64 + 4).unwrap(), SHIP_AT);
    assert_eq!(new_units(&m, SQUAD, ship, &[]).iter().filter(|u| u.1).count(), 3);
}

#[test]
fn a_ship_with_no_free_seat_refuses_and_calls_nothing() {
    let w = world(2, 3);
    let ship = w.ship;
    let (result, calls, _) = run(w, boarding(3, 2, ship), 3, false);
    assert!(result.unwrap_err().to_string().contains("no free seat"));
    assert!(calls.is_empty());
}

#[test]
fn a_load_the_engine_refuses_leaves_no_surplus_actor() {
    // The engine seats one of three: the other two stand under the ship
    // and are deleted.
    let w = world(0, 3);
    let (ship, units) = (w.ship, w.pool.iter().map(|p| p.2).collect::<Vec<_>>());
    let (result, calls, _) = run(w, boarding(3, 4, ship), 1, false);
    assert_eq!(result.unwrap(), 1);
    assert_eq!(deletes(&calls), units[1..]);

    // The engine seats none: every actor is deleted and the placement is
    // refused.
    let w = world(0, 3);
    let (ship, units) = (w.ship, w.pool.iter().map(|p| p.2).collect::<Vec<_>>());
    let (result, calls, _) = run(w, boarding(3, 4, ship), 0, false);
    assert!(result.unwrap_err().to_string().contains("seated none"));
    assert_eq!(deletes(&calls), units);
}

#[test]
fn riders_over_the_seats_spawn_on_foot() {
    // A squad of eight for a ship of four seats with a rider aboard: three
    // are seated, the other five stand at the zone, and none is lost.
    let w = world(1, 8);
    let ship = w.ship;
    let (result, calls, _) = run(w, boarding(8, 4, ship), 3, true);
    assert_eq!(result.unwrap(), 3);
    let place = |n| GuestCall::new(points::AI_SQUAD_PLACE.base).eax(n).args([1, u32::MAX, 0, 1]);
    let placed: Vec<u32> = calls
        .iter()
        .filter(|c| c.target == points::AI_SQUAD_PLACE.base)
        .map(|c| c.eax.unwrap())
        .collect();
    assert_eq!(placed, [3, 5]);
    assert_eq!(calls.first(), Some(&place(3)));
    assert_eq!(calls[calls.len() - 2], place(5));
    assert!(deletes(&calls).is_empty());
}

#[test]
fn the_rest_of_the_squad_spawns_on_foot() {
    let w = world(0, 6);
    let ship = w.ship;
    let (result, calls, _) = run(w, boarding(5, 2, ship), 2, true);
    assert_eq!(result.unwrap(), 2);
    let place = |n| GuestCall::new(points::AI_SQUAD_PLACE.base).eax(n).args([1, u32::MAX, 0, 1]);
    assert_eq!(calls.first(), Some(&place(2)));
    assert_eq!(calls[calls.len() - 2], place(3));
    assert_eq!(calls[calls.len() - 1], GuestCall::new(points::AI_SET_TEAM.base).eax(1).ebx(5));
}

#[test]
fn a_full_ship_puts_the_whole_squad_on_foot() {
    let w = world(2, 3);
    let ship = w.ship;
    let (result, calls, _) = run(w, boarding(3, 2, ship), 3, true);
    assert_eq!(result.unwrap(), 0);
    assert_eq!(
        calls,
        [
            GuestCall::new(points::AI_SQUAD_PLACE.base).eax(3).args([1, u32::MAX, 0, 1]),
            GuestCall::new(points::AI_SET_TEAM.base).eax(1).ebx(5),
        ]
    );
}

#[test]
fn the_pose_is_level_to_the_flight() {
    assert_eq!(level_up([0.0, 0.0, 1.0], [1.0, 0.0, 0.0]), [0.0, 0.0, 1.0]);
    let up = level_up([0.0, 0.0, 1.0], [0.6, 0.0, 0.8]);
    assert!((dot(up, [0.6, 0.0, 0.8])).abs() < 1e-5, "{up:?}");
    assert!((dot(up, up) - 1.0).abs() < 1e-5);
    // Straight up has no usable up: another axis stands in.
    let up = level_up([0.0, 0.0, 1.0], [0.0, 0.0, 1.0]);
    assert!(dot(up, [0.0, 0.0, 1.0]).abs() < 1e-5 && (dot(up, up) - 1.0).abs() < 1e-5);
}

#[test]
fn a_ship_put_passes_the_pose_and_facing_to_the_engine() {
    let w = world(0, 0);
    let ship = w.ship;
    let (result, calls, _) = drive(
        w.m,
        move |cx| {
            Box::pin(async move {
                put_ship(&cx, ShipPut { object: ship, at: [1.0, 2.0, 3.0], forward: [1.0, 0.0, 0.0] }).await
            })
        },
        returns(1),
    );
    result.unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].target, points::OBJECT_SET_TRANSFORM.base);
    assert_eq!(calls[0].args[0], ship);
    let pose: Vec<f32> = calls[0].args[9..].iter().map(|b| f32::from_bits(*b)).collect();
    assert_eq!(pose, [1.0, 2.0, 3.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn the_game_keeps_a_launch_and_defers_a_drop() {
    use crate::firefight::host::Game;
    use crate::firefight::runtime::{Call, GuestGame, SkullSink};

    let mut m = FakeGuest::new();
    let mut skulls = std::collections::BTreeSet::new();
    let sink: Box<SkullSink> = Box::new(|_, _| Ok(()));
    let mut g = GuestGame::new(&mut m, None, &[], &mut skulls, &*sink);
    let launch = Launch {
        ship: 0x8001_0002,
        path: vec![[0.0; 3], [10.0, 0.0, 0.0]],
        ticks: 8,
    };
    g.begin_entry(4);
    g.fly(launch.clone()).unwrap();
    g.drop_squad(boarding(3, 4, launch.ship)).unwrap();
    assert_eq!(g.launches, std::slice::from_ref(&launch));
    assert_eq!(g.calls, [(4, Call::Drop(boarding(3, 4, launch.ship)))]);
}

#[test]
fn the_hold_and_the_unload_are_the_engines_vehicle_entries() {
    let w = world(0, 0);
    let ship = w.ship;
    let (result, calls, _) = drive(
        w.m,
        move |cx| {
            Box::pin(async move {
                hover(&cx, ship, true).await?;
                unload(&cx, ship).await
            })
        },
        returns(0),
    );
    result.unwrap();
    assert_eq!(
        calls,
        [
            GuestCall::new(points::VEHICLE_HOVER.base).eax(ship).ecx(1),
            GuestCall::new(points::VEHICLE_UNLOAD.base).edx(ship).ecx(u32::MAX),
        ]
    );
    // A ship that is gone is refused, and nothing is called.
    let (result, calls, _) = drive(
        FakeGuest::new(),
        move |cx| Box::pin(async move { unload(&cx, ship).await }),
        returns(0),
    );
    assert!(result.unwrap_err().to_string().contains("not a live vehicle"));
    assert!(calls.is_empty());
}

#[test]
fn a_list_cast_with_the_squad_in_eax_is_refused_before_the_guest_sees_it() {
    // The shape that crashed a Firefight game: the cast found no stack
    // argument, read the entry's return address as the squad and wrote its
    // list datum over it, and the entry returned to that datum.
    let (result, calls, _) = drive(
        FakeGuest::new(),
        |cx| {
            Box::pin(async move {
                guest::call(&cx, GuestCall::new(points::OBJECT_LIST_FROM_AI.base).eax(1)).await
            })
        },
        |_, _| panic!("a refused call reaches no callee"),
    );
    let why = result.unwrap_err().to_string();
    assert!(why.contains("OBJECT_LIST_FROM_AI"), "{why}");
    assert!(why.contains("1 stack argument(s) and the call passes 0"), "{why}");
    assert!(calls.is_empty());
}
