//! The guest half over invented guest memory and fake engine callees.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use super::*;
use guest_layout::entry::{EntryFuture, Ret, testing};
use guest_layout::testing::FakeGuest;

const GLOBALS: u32 = 0x4000;
const SCNR: u32 = 0x1_0000;
const SQUADS: u32 = 0x2_0000;
const STARTS: u32 = 0x2_1000;
const ZONES: u32 = 0x2_2000;
const AREAS: u32 = 0x2_3000;
const FIRING: u32 = 0x2_4000;
const ORDERS: u32 = 0x2_5000;
const PLAYERS: u32 = 0x3_0000;
const PLAYER_DATA: u32 = 0x3_1000;
const GAME_GLOBALS: u32 = 0x4_0000;
const ENGINE: u32 = 0x5_0000;
const SCRATCH: u32 = 0x6_0000;

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
    // Each record squad is on its class's faction's team: the Elite, the
    // Grunt and the Hunter are Covenant (3).
    for (squad, row) in [(3u32, 0u16), (4, 1), (5, 2), (6, 9)] {
        m.put_u16(SQUADS + 0x74 * squad + 0x36, row);
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
    fake_characters(&mut m, 0x2_6000, 3);
    pair(&mut m, SCNR + 0x240, 5, ORDERS);
    m.map(ORDERS, 5 * 0x7c);
    named(&mut m, ORDERS, "ff_other");
    named(&mut m, ORDERS + 0x7c, "ff_hunt_01");
    named(&mut m, ORDERS + 2 * 0x7c, "ff_hunt_00");
    named(&mut m, ORDERS + 3 * 0x7c, "ff_hold_01");
    named(&mut m, ORDERS + 4 * 0x7c, "ff_hold_00");
    m
}

#[test]
fn read_kit_finds_the_hold_orders() {
    let kit = layout();
    assert_eq!(kit.hold_orders, [Some(4), Some(3)]);
    assert_eq!(kit.hunt_orders, [Some(2), Some(1)]);
    assert_eq!(hold_order_name(1), "ff_hold_01");
    // Another name is no hold order.
    let mut m = scenario();
    named(&mut m, ORDERS + 4 * 0x7c, "ff_hold");
    assert_eq!(read_kit(&m).unwrap().unwrap().hold_orders, [None, Some(3)]);
}

fn layout() -> KitLayout {
    read_kit(&scenario()).unwrap().expect("the kit")
}

const ACTOR_POOL: u32 = 0x7_0000;
const OBJECT_POOL: u32 = 0x8_0000;
const BODY: u32 = 0x9_0000;

/// `m` with an actor pool holding one AI unit that belongs to no squad
/// and has no replication entity, as a local game's actors have none.
/// The returned actor is the one a placement "creates" by giving it a
/// squad ([`joins_squad`]).
fn with_an_idle_actor(mut m: FakeGuest) -> (FakeGuest, guest_layout::actor::Actor) {
    use guest_layout::{Datum, Object, Unit, actor::Actor, view::fake, view::objects::Header};
    let actors = fake::datums::<Actor>(&mut m, ACTOR_POOL, 4);
    let objects = fake::datums::<Header>(&mut m, OBJECT_POOL, 4);
    m.put_u32(guest_layout::global::ACTORS.base, actors.array.addr)
        .put_u32(guest_layout::global::OBJECTS.base, objects.array.addr);
    let (id, actor) = fake::push(&mut m, &actors);
    let (handle, header) = fake::push(&mut m, &objects);
    let body = Object::at(BODY);
    let unit = body.as_unit();
    m.map(body.addr, Unit::SIZE);
    actor.set_unit(&mut m, Some(handle)).unwrap();
    actor.set_squad(&mut m, 0xffff).unwrap();
    header.set_body(&mut m, body.addr).unwrap();
    unit.set(&mut m, Unit::ACTOR, Datum(id.raw())).unwrap();
    unit.set(&mut m, Unit::PLAYER, Datum::NONE).unwrap();
    body.set(&mut m, Object::ENTITY, u32::MAX).unwrap();
    body.set(&mut m, Object::PARENT, Datum::NONE).unwrap();
    body.set(&mut m, Object::POSITION, [0.0; 3]).unwrap();
    (m, actor)
}

/// A callee that answers 0, and on its first call (the placement) puts
/// the idle actor in `squad`.
fn joins_squad(
    actor: guest_layout::actor::Actor,
    squad: u32,
) -> impl FnMut(&GuestCall, &mut dyn GuestMemory) -> Regs {
    let mut first = true;
    move |_, m| {
        if std::mem::take(&mut first) {
            actor.set_squad(m, squad).unwrap();
        }
        Regs::default()
    }
}

fn placement(slot: u8, character: u16, zone: u8) -> Placement {
    Placement {
        slot,
        character,
        count: 3,
        zone,
        team: 5,
        weapon_tier: 0,
    }
}

#[test]
fn the_switch_sets_only_the_on_byte_and_only_when_off() {
    let mut m = FakeGuest::new();
    m.put_u32(points::AI_GLOBALS.base, 0);
    assert_eq!(ai_globals(&m).unwrap(), None);
    assert!(!switch_ai_on(&mut m).unwrap());

    m.put_u32(points::AI_GLOBALS.base, GLOBALS)
        .put_u8(GLOBALS, 0)
        .put_u8(GLOBALS + 1, 1)
        .put_u8(GLOBALS + 2, 7);
    assert_eq!(
        ai_globals(&m).unwrap(),
        Some(AiGlobals {
            on: false,
            initialised: true
        })
    );
    assert!(switch_ai_on(&mut m).unwrap());
    assert_eq!(
        ai_globals(&m).unwrap(),
        Some(AiGlobals {
            on: true,
            initialised: true
        })
    );
    assert_eq!(m.u8(GLOBALS + 2).unwrap(), 7);
    assert!(!switch_ai_on(&mut m).unwrap());
}

#[test]
fn the_kit_is_read_from_the_scenario_ai_blocks() {
    let kit = layout();
    let mut squads = [None; SQUAD_SLOTS];
    squads[0] = Some(1);
    squads[1] = Some(2);
    assert_eq!(kit.squads, squads);
    assert_eq!(
        kit.zones,
        [KitZone {
            index: 1,
            point: [1.0, 2.0, 3.0],
            starts: vec![
                ([10.0, 0.0, 0.0], 0xffff),
                ([20.0, 0.0, 0.0], 0xffff),
                ([30.0, 0.0, 0.0], 0xffff),
            ],
            roles: 0,
        }]
    );
    assert_eq!((kit.characters, kit.team), (3, 3));
    assert_eq!(
        kit.kit(),
        Kit {
            zones: vec![[1.0, 2.0, 3.0]],
            bases: vec![[-90.0, 0.0, 0.0], [90.0, 0.0, 0.0]],
            classes: [
                ("elite".to_string(), 0),
                ("grunt".to_string(), 1),
                ("hunter".to_string(), 2),
            ]
            .into(),
            team: 3,
            pod_classes: Default::default(),
            flood_classes: Default::default(),
        }
    );
}

#[test]
fn each_class_the_kit_recorded_has_its_palette_row_and_no_other_does() {
    // The record is the squads' names and characters, not a table here:
    // another build of the kit with other classes maps other rows.
    let mut m = scenario();
    named(&mut m, SQUADS + 3 * 0x74, "ff_class_brute");
    named(&mut m, SQUADS + 4 * 0x74, "ff_class_brute");
    m.put_u16(SQUADS + 4 * 0x74 + 0x36, 2);
    named(&mut m, SQUADS + 5 * 0x74, "ff_class_");
    let kit = read_kit(&m).unwrap().unwrap();
    // The first brute keeps its row; the repeat, the empty name and the
    // squad naming a row past the palette (`ghost`) record nothing.
    assert_eq!(kit.classes, [("brute".to_string(), 0)]);
    assert_eq!(kit.kit().classes, [("brute".to_string(), 0)].into());

    // A kit that recorded no classes provides none: the Elite is not
    // assumed.
    let mut m = scenario();
    for squad in 3..7u32 {
        named(&mut m, SQUADS + 0x74 * squad, "ff_extra");
    }
    let kit = read_kit(&m).unwrap().unwrap();
    assert!(kit.kit().classes.is_empty());
}

#[test]
fn read_kit_finds_the_bases_and_hunt_orders() {
    let kit = layout();
    assert_eq!(
        kit.bases,
        [
            KitZone {
                index: 3,
                point: [-90.0, 0.0, 0.0],
                starts: vec![([50.0, 0.0, 0.0], 0xffff)],
                roles: 0,
            },
            KitZone {
                index: 2,
                point: [90.0, 0.0, 0.0],
                starts: vec![([40.0, 0.0, 0.0], 0xffff)],
                roles: 0,
            },
        ]
    );
    assert_eq!(kit.hunt_orders, [Some(2), Some(1)]);
    assert_eq!(hunt_order_name(1), "ff_hunt_01");

    // Only the run of base ordinals from 0 counts, and no hunt order is
    // inferred from another name.
    let mut m = scenario();
    named(&mut m, ZONES + 0x38 * 3, "ff_base_02");
    named(&mut m, ORDERS + 2 * 0x7c, "ff_hunt");
    let kit = read_kit(&m).unwrap().unwrap();
    assert!(kit.bases.is_empty());
    assert_eq!(kit.hunt_orders, [None, Some(1)]);
    assert_eq!(kit.kit().bases, Vec::<[f32; 3]>::new());
}

#[test]
fn zone_roles_read_back() {
    use ::firefight::roles;
    // The kit writes the roles above the engine's flag bits; the engine's
    // own bit (1) is not a role.
    let mut m = scenario();
    let flags = 1 | roles::pack(roles::LINE | roles::HEAVY);
    m.put_u32(ZONES + 0x38 + 0x20, flags);
    let kit = read_kit(&m).unwrap().unwrap();
    assert_eq!(kit.zones[0].roles, roles::LINE | roles::HEAVY);

    // A zone with the engine's bit alone has no role, and so do the bases.
    m.put_u32(ZONES + 0x38 + 0x20, 1);
    m.put_u32(ZONES + 0x38 * 2 + 0x20, 1);
    let kit = read_kit(&m).unwrap().unwrap();
    assert_eq!(kit.zones[0].roles, 0);
    assert!(kit.bases.iter().all(|b| b.roles == 0));

    // Every role survives the round trip.
    for r in 0..=roles::ALL {
        assert_eq!(roles::unpack(1 | roles::pack(r)), r);
    }
}

#[test]
fn a_map_without_kit_squads_has_no_kit() {
    let mut m = FakeGuest::new();
    m.put_u32(points::SCENARIO.base, 0);
    assert_eq!(read_kit(&m).unwrap(), None);
    let mut m = scenario();
    named(&mut m, SQUADS + 0x74, "ff_slot_x");
    named(&mut m, SQUADS + 2 * 0x74, "ff_slot_100");
    assert_eq!(read_kit(&m).unwrap(), None);
}

#[test]
fn a_placement_sets_the_squad_then_places_and_sets_the_team() {
    let kit = layout();
    let (guest, actor) = with_an_idle_actor(scenario());
    let (result, calls, m) = drive(
        guest,
        move |cx| {
            let kit = kit.clone();
            Box::pin(async move { place_squad(&cx, &kit, placement(0, 0, 0)).await })
        },
        joins_squad(actor, 1),
    );
    result.unwrap();
    assert_eq!(
        calls,
        [
            GuestCall::new(points::AI_SQUAD_PLACE.base)
                .eax(3)
                .args([1, u32::MAX, 0, 1]),
            GuestCall::new(points::AI_SET_TEAM.base).eax(1).ebx(5),
        ]
    );
    let squad = SQUADS + 0x74;
    assert_eq!(
        (m.u16(squad + 0x36).unwrap(), m.u16(squad + 0x38).unwrap()),
        (0, 1)
    );
    assert_eq!(m.vec3(STARTS + 4).unwrap(), [10.0, 0.0, 0.0]);
    assert_eq!(m.vec3(STARTS + 0x64 + 4).unwrap(), [20.0, 0.0, 0.0]);
    assert_eq!(m.u16(STARTS + 0x10).unwrap(), 0xffff);
}

/// A scenario whose record squad of palette row `row` is on `team`.
fn scenario_with_team(row: u16, team: u16) -> FakeGuest {
    let mut m = scenario();
    m.put_u16(SQUADS + 0x74 * (3 + u32::from(row)) + 0x24, team);
    m
}

#[test]
fn the_kit_reads_each_classs_faction_from_its_record_squad() {
    let mut m = scenario();
    // Row 1 is Flood (4), row 2 a human (2); row 0 stays Covenant.
    m.put_u16(SQUADS + 0x74 * 4 + 0x24, 4)
        .put_u16(SQUADS + 0x74 * 5 + 0x24, 2);
    let kit = read_kit(&m).unwrap().unwrap();
    assert_eq!(
        kit.factions.values().copied().collect::<Vec<_>>(),
        [Faction::Covenant, Faction::Flood, Faction::Human]
    );
    assert_eq!(kit.factions[&1], Faction::Flood);
    // A record on no faction's team (the player's, 1) is no class.
    let m = scenario_with_team(2, 1);
    let kit = read_kit(&m).unwrap().unwrap();
    assert_eq!(kit.classes.len(), 2);
    assert!(!kit.factions.contains_key(&2));
    assert!(!kit.kit().classes.contains_key("hunter"));
}

#[test]
fn a_flood_squad_is_placed_on_the_flood_team() {
    // Row 1 is Flood: the squad takes team 4 whatever team the host picked.
    let m = scenario_with_team(1, 4);
    let kit = read_kit(&m).unwrap().unwrap();
    assert_eq!(kit.placement_team(1, 5).unwrap(), 4);
    let (m, actor) = with_an_idle_actor(m);
    let (result, calls, _) = drive(
        m,
        move |cx| {
            let kit = kit.clone();
            Box::pin(async move { place_squad(&cx, &kit, placement(0, 1, 0)).await })
        },
        joins_squad(actor, 1),
    );
    result.unwrap();
    assert_eq!(
        calls.last(),
        Some(&GuestCall::new(points::AI_SET_TEAM.base).eax(1).ebx(4))
    );
    // A Covenant class takes the host's pick, a human class its own team.
    let kit = layout();
    assert_eq!(kit.placement_team(0, 5).unwrap(), 5);
    let m = scenario_with_team(2, 2);
    assert_eq!(read_kit(&m).unwrap().unwrap().placement_team(2, 5).unwrap(), 2);
    // A row the kit recorded no class for is refused, not given the host's team.
    let error = kit.placement_team(9, 5).unwrap_err().to_string();
    assert!(error.contains("row 9"), "{error}");
}

#[test]
fn a_placement_that_adds_no_actor_is_refused() {
    let kit = layout();
    let (guest, _) = with_an_idle_actor(scenario());
    let (result, calls, _) = drive(
        guest,
        move |cx| {
            let kit = kit.clone();
            Box::pin(async move { place_squad(&cx, &kit, placement(0, 0, 0)).await })
        },
        returns(0),
    );
    // The engine was called, made nothing, and the team was not set.
    assert_eq!(calls.len(), 1);
    assert_eq!(
        result.unwrap_err().to_string(),
        "the engine placed no actor for slot 0: class row 0 at zone 0"
    );
}

#[test]
fn a_placement_the_kit_cannot_hold_writes_and_calls_nothing() {
    for p in [placement(2, 0, 0), placement(0, 0, 1), placement(0, 3, 0)] {
        let kit = layout();
        let (result, calls, m) = drive(
            scenario(),
            move |cx| {
                let kit = kit.clone();
                Box::pin(async move { place_squad(&cx, &kit, p).await })
            },
            returns(0),
        );
        assert!(result.is_err(), "{p:?}");
        assert!(calls.is_empty());
        assert_eq!(m.u16(SQUADS + 0x74 + 0x36).unwrap(), 0xffff);
    }
}

/// The fault at `0x1f911a`: an actor whose character and parents have no
/// movement row reads a null row's `+4` on its first update. A class
/// whose character lacks the row, or a palette row naming none, is refused
/// before the squad is written or the engine is called; one that takes
/// the row from a parent is placed.
#[test]
fn a_class_whose_character_has_no_movement_row_is_not_placed() {
    let definition = |row: u32| FAKE_CHARACTERS + 0x100 * row;
    let unready = |m: &mut FakeGuest| {
        // Row 1 (the Grunt) has no movement row; its parent, row 2's tag,
        // has none either.
        m.put_u32(definition(1) + 0x5c, 0)
            .put_u32(definition(1) + 8, 0xe000_0002)
            .put_u32(definition(2) + 0x5c, 0);
    };
    let mut m = scenario();
    unready(&mut m);
    let error = character_ready(&m, 1).unwrap_err().to_string();
    assert_eq!(
        error,
        "palette row 1's character 0xe0000001 and its parents have no movement row, which the engine reads unchecked on an actor's first update (0x1f911a), so the class is not placed"
    );
    let kit = read_kit(&m).unwrap().unwrap();
    let (result, calls, m) = drive(
        m,
        move |cx| {
            let kit = kit.clone();
            Box::pin(async move { place_squad(&cx, &kit, placement(0, 1, 0)).await })
        },
        returns(0),
    );
    assert_eq!(result.unwrap_err().to_string(), error);
    assert!(calls.is_empty(), "{calls:?}");
    assert_eq!(m.u16(SQUADS + 0x74 + 0x36).unwrap(), 0xffff);

    // The row from a parent is the engine's row: the class is ready.
    let mut m = scenario();
    unready(&mut m);
    m.put_u32(definition(2) + 0x5c, 1);
    character_ready(&m, 1).unwrap();
    character_ready(&m, 0).unwrap();

    // A palette row that names no character is refused by its row.
    let mut m = scenario();
    m.put_u32(0x2_6000 + 8 * 2 + 4, u32::MAX);
    let error = character_ready(&m, 2).unwrap_err().to_string();
    assert!(error.contains("palette row 2 names no character"), "{error}");
}

#[test]
fn hunt_gives_the_bases_order_and_erase_takes_the_squad() {
    let kit = layout();
    let (result, calls, _) = drive(
        scenario(),
        move |cx| {
            let kit = kit.clone();
            Box::pin(async move {
                hunt(&cx, &kit, 1, 0).await?;
                hunt(&cx, &kit, 0, 1).await?;
                erase_squad(&cx, &kit, 1).await
            })
        },
        returns(0),
    );
    result.unwrap();
    assert_eq!(
        calls,
        [
            GuestCall::new(points::AI_SET_ORDERS.base).eax(2).ecx(2),
            GuestCall::new(points::AI_SET_ORDERS.base).eax(1).ecx(1),
            GuestCall::new(points::AI_ERASE.base).args([2]),
        ]
    );

    // A base without its order: an error naming it, and no call.
    for (base, missing) in [(1u8, true), (2, false)] {
        let mut kit = layout();
        if missing {
            kit.hunt_orders[1] = None;
        }
        let (result, calls, _) = drive(
            scenario(),
            move |cx| {
                let kit = kit.clone();
                Box::pin(async move { hunt(&cx, &kit, 0, base).await })
            },
            returns(0),
        );
        let error = result.unwrap_err().to_string();
        assert!(error.contains(&format!("ff_hunt_{base:02}")), "{error}");
        assert!(calls.is_empty());
    }
}

#[test]
fn ally_sets_the_players_team_then_the_bases_hold_order() {
    let kit = layout();
    let (result, calls, _) = drive(
        scenario(),
        move |cx| {
            let kit = kit.clone();
            Box::pin(async move { ally(&cx, &kit, 1, 1, 0).await })
        },
        returns(0),
    );
    result.unwrap();
    assert_eq!(
        calls,
        [
            GuestCall::new(points::AI_SET_TEAM.base).eax(2).ebx(1),
            GuestCall::new(points::AI_SET_ORDERS.base).eax(2).ecx(4),
        ]
    );
    // A base without its hold order: an error naming it, and no call.
    let mut kit = layout();
    kit.hold_orders[1] = None;
    let (result, calls, _) = drive(
        scenario(),
        move |cx| {
            let kit = kit.clone();
            Box::pin(async move { ally(&cx, &kit, 1, 1, 1).await })
        },
        returns(0),
    );
    assert!(result.unwrap_err().to_string().contains("ff_hold_01"));
    assert!(calls.is_empty());
}

fn players() -> FakeGuest {
    let mut m = FakeGuest::new();
    m.put_u32(points::PLAYERS.base, PLAYERS)
        .put_u32(PLAYERS + 0x44, PLAYER_DATA);
    m.map(PLAYER_DATA, 16 * 0x21c);
    m.put_u16(PLAYER_DATA + 2 * 0x21c, 0xe1a5);
    m.put_u32(points::GAME_GLOBALS.base, GAME_GLOBALS);
    m.map(GAME_GLOBALS, 0x1130);
    m.put_u32(points::GAME_ENGINE.base, ENGINE);
    m.map(ENGINE, 0xc9c);
    m
}

#[test]
fn a_score_goes_through_the_engine_with_the_salted_player() {
    let body = |cx: Cx| -> Body<()> { Box::pin(async move { credit_score(&cx, 2, 150).await }) };
    let (result, calls, _) = drive(players(), body, returns(1));
    result.unwrap();
    assert_eq!(
        calls,
        [GuestCall::new(points::SCORE_ADD.base)
            .eax(150)
            .args([0xe1a5_0002])]
    );

    let (result, _, _) = drive(players(), body, returns(0x100));
    assert!(result.is_err(), "al clear: the engine wrote nothing");
    for (slot, points) in [(3, 1), (2, 30_001)] {
        let (result, calls, _) = drive(
            players(),
            move |cx| Box::pin(async move { credit_score(&cx, slot, points).await }),
            returns(1),
        );
        assert!(result.is_err());
        assert!(calls.is_empty());
    }
}

#[test]
fn a_respawn_gives_one_life_only_while_the_engine_holds_respawns() {
    let body = |cx: Cx| -> Body<()> { Box::pin(async move { respawn(&cx, 2).await }) };
    let (result, calls, m) = drive(players(), body, returns(0));
    assert!(result.is_err(), "unlimited lives");
    assert!(calls.is_empty());
    assert_eq!(m.u16(PLAYER_DATA + 2 * 0x21c + 0x1ac).unwrap(), 0);

    let mut m = players();
    m.put_u32(GAME_GLOBALS + 0x1b8, 1);
    assert!(engine_holds_respawns(&m).unwrap());
    let (result, calls, m) = drive(m, body, returns(0));
    result.unwrap();
    assert_eq!(m.u16(PLAYER_DATA + 2 * 0x21c + 0x1ac).unwrap(), 1);
    assert_eq!(
        calls,
        [GuestCall::new(points::PLAYER_ENTITY_DIRTY.base).args([0xe1a5_0002, 0x20])]
    );
}

#[test]
fn the_game_ends_as_the_round_over_path_ends_it() {
    let body = |cx: Cx| -> Body<()> { Box::pin(async move { end_game(&cx).await }) };
    let mut m = players();
    m.put_u32(ENGINE, 1);
    let (result, calls, m) = drive(m, body, returns(0));
    result.unwrap();
    assert_eq!(calls, [GuestCall::new(points::GAME_END.base)]);
    assert_eq!(
        (m.u32(ENGINE + 0xc04).unwrap(), m.u32(ENGINE).unwrap()),
        (2, 0x21)
    );

    let mut m = players();
    m.put_u8(GAME_GLOBALS + 0x1128, 1);
    let (result, calls, m) = drive(m, body, returns(0));
    result.unwrap();
    assert!(calls.is_empty(), "already latched");
    assert_eq!(m.u32(ENGINE + 0xc04).unwrap(), 2);

    let mut m = players();
    m.put_u8(GAME_GLOBALS + 0x0c, 4);
    let (result, calls, m) = drive(m, body, returns(0));
    assert!(result.is_err());
    assert!(calls.is_empty());
    assert_eq!(m.u32(ENGINE + 0xc04).unwrap(), 0);
}

#[test]
fn line_of_sight_passes_both_points_and_reads_al() {
    for (eax, clear) in [(0x101, true), (0x100, false)] {
        let mut m = FakeGuest::new();
        m.map(SCRATCH, 24);
        let (result, calls, m) = drive(
            m,
            |cx| {
                Box::pin(async move {
                    line_of_sight(&cx, SCRATCH, [1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [0xabcd_0001, 0xabcd_0002]).await
                })
            },
            returns(eax),
        );
        assert_eq!(result.unwrap(), clear);
        assert_eq!(
            calls,
            [GuestCall::new(points::AI_LINE_OF_SIGHT.base)
                .esi(SCRATCH)
                .edi(SCRATCH + 12)
                .edx(0xabcd_0001)
                .ecx(0xabcd_0002)]
        );
        assert_eq!(m.vec3(SCRATCH).unwrap(), [1.0, 2.0, 3.0]);
        assert_eq!(m.vec3(SCRATCH + 12).unwrap(), [4.0, 5.0, 6.0]);
    }
}

const NAMES: u32 = 0x2_7000;
const SCRIPTING: u32 = 0x2_8000;
const SETS: u32 = 0x2_8100;
const POINTS: u32 = 0x2_8300;

/// [`scenario`] with the ship's object name (a vehicle at placement 5, and a
/// scenery of the same name's neighbour), and scripting data whose point
/// sets are `ff_path_00` and the stray `ff_path_07`, each with `points`
/// named in order at the positions `x, 1, 2`.
fn with_ship(points: &[&str]) -> FakeGuest {
    let mut m = scenario();
    pair(&mut m, SCNR + 0x48, 2, NAMES);
    m.map(NAMES, 2 * 0x24);
    named(&mut m, NAMES, "ff_crate");
    m.put_u16(NAMES + 0x20, 6).put_u16(NAMES + 0x22, 1);
    named(&mut m, NAMES + 0x24, "ff_ship_pelican");
    m.put_u16(NAMES + 0x24 + 0x20, 1).put_u16(NAMES + 0x24 + 0x22, 5);

    pair(&mut m, SCNR + 0x1d8, 1, SCRIPTING);
    m.map(SCRIPTING, 0x80);
    pair(&mut m, SCRIPTING, 2, SETS);
    m.map(SETS, 2 * 0x30);
    m.map(POINTS, 6 * 0x3c);
    for (set, name) in [(0u32, "ff_path_00"), (1, "ff_path_07")] {
        let at = SETS + 0x30 * set;
        named(&mut m, at, name);
        pair(&mut m, at + 0x20, points.len() as u32, POINTS + 3 * 0x3c * set);
        for (i, point) in points.iter().enumerate() {
            let p = POINTS + 3 * 0x3c * set + 0x3c * i as u32;
            named(&mut m, p, point);
            for (k, v) in [100.0 * (set + 1) as f32 + i as f32, 1.0, 2.0].into_iter().enumerate() {
                m.put_f32(p + 0x20 + 4 * k as u32, v);
            }
        }
    }
    m
}

#[test]
fn the_game_reads_the_ship_and_its_paths() {
    // The base kit carries neither.
    let kit = layout();
    assert_eq!((kit.ships, kit.paths.clone()), ([None, None], vec![None]));

    let kit = read_kit(&with_ship(&["run_in", "hover_pelican", "run_out"])).unwrap().unwrap();
    // The vehicle placement `ff_ship_pelican` names; the crate of another
    // type is not it, and the Phantom is not parked.
    assert_eq!(kit.ships, [None, Some(5)]);
    // One path a drop zone, by its number: `ff_path_00` is `ff_drop_00`'s;
    // `ff_path_07` has no zone.
    assert_eq!(
        kit.paths,
        [Some(ShipPath {
            run_in: [100.0, 1.0, 2.0],
            hovers: [None, Some([101.0, 1.0, 2.0])],
            run_out: [102.0, 1.0, 2.0],
        })]
    );
    // The reads change nothing else of the kit.
    assert_eq!(kit.kit(), layout().kit());

    // A set missing a point is no path; a ship name on another type is no
    // ship.
    let kit = read_kit(&with_ship(&["run_in", "run_out", "hold"])).unwrap().unwrap();
    assert_eq!(kit.paths, [None]);
    let mut m = with_ship(&["run_in", "hover_pelican", "run_out"]);
    m.put_u16(NAMES + 0x24 + 0x20, 2);
    assert_eq!(read_kit(&m).unwrap().unwrap().ships, [None, None]);
}

/// [`scenario`] with the pools' object names and two zone falls and a
/// start's, in the scripting data.
fn with_pods(points: &[&str]) -> FakeGuest {
    let mut m = scenario();
    pair(&mut m, SCNR + 0x48, 4, NAMES);
    m.map(NAMES, 4 * 0x24);
    for (i, (name, kind, at)) in [
        ("ff_pod_01", 1u16, 3u16),
        ("ff_pod_00", 1, 2),
        ("ff_ppod_00", 1, 4),
        ("ff_pod_02", 2, 9),
    ]
    .into_iter()
    .enumerate()
    {
        let n = NAMES + 0x24 * i as u32;
        named(&mut m, n, name);
        m.put_u16(n + 0x20, kind).put_u16(n + 0x22, at);
    }
    pair(&mut m, SCNR + 0x1d8, 1, SCRIPTING);
    m.map(SCRIPTING, 0x80);
    pair(&mut m, SCRIPTING, 3, SETS);
    m.map(SETS, 3 * 0x30);
    m.map(POINTS, 6 * 0x3c);
    for (set, name) in [(0u32, "ff_fall_00"), (1, "ff_fall_05"), (2, "ff_pfall_03")] {
        let at = SETS + 0x30 * set;
        named(&mut m, at, name);
        pair(&mut m, at + 0x20, points.len() as u32, POINTS + 2 * 0x3c * set);
        for (i, point) in points.iter().enumerate() {
            let p = POINTS + 2 * 0x3c * set + 0x3c * i as u32;
            named(&mut m, p, point);
            for (k, v) in [10.0 * (set + 1) as f32 + i as f32, 1.0, 2.0].into_iter().enumerate() {
                m.put_f32(p + 0x20 + 4 * k as u32, v);
            }
        }
    }
    m
}

#[test]
fn the_game_reads_pods_and_falls() {
    // The base kit carries none.
    let kit = layout();
    assert!(kit.pods.is_empty() && kit.player_pods.is_empty() && kit.player_falls.is_empty());
    assert_eq!(kit.falls, vec![None]);

    let kit = read_kit(&with_pods(&["top", "ground"])).unwrap().unwrap();
    // Squad pods in NN order; the player pool apart; a pod name on another
    // object type is no pod.
    assert_eq!(kit.pods, [2, 3]);
    assert_eq!(kit.player_pods, [4]);
    // `ff_fall_00` is `ff_drop_00`'s; `ff_fall_05` has no zone; the start's
    // fall is by its number.
    assert_eq!(
        kit.falls,
        [Some(PodFall { top: [10.0, 1.0, 2.0], ground: [11.0, 1.0, 2.0] })]
    );
    assert_eq!(
        kit.player_falls,
        [(3, PodFall { top: [30.0, 1.0, 2.0], ground: [31.0, 1.0, 2.0] })]
    );
    assert_eq!(kit.kit(), layout().kit());

    // A set missing a point is no fall.
    let kit = read_kit(&with_pods(&["top", "hover"])).unwrap().unwrap();
    assert_eq!((kit.falls, kit.player_falls), (vec![None], vec![]));
}
