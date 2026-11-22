//! The guest half over invented guest memory and fake engine callees.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use super::*;
use engine::entry::{EntryFuture, Ret, testing};
use engine::testing::FakeGuest;

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

/// A scenario with one base squad and two kit slots, one base zone and one
/// kit zone of three firing positions, one palette row and two orders.
fn scenario() -> FakeGuest {
    let mut m = FakeGuest::new();
    m.map(SCNR, 0x300);
    m.put_u32(points::SCENARIO.base, SCNR);
    pair(&mut m, SCNR + 0x160, 3, SQUADS);
    m.map(SQUADS, 3 * 0x74);
    for (i, name) in ["marines", "ff_slot_00", "ff_slot_01"]
        .into_iter()
        .enumerate()
    {
        named(&mut m, SQUADS + 0x74 * i as u32, name);
    }
    let slot0 = SQUADS + 0x74;
    m.put_u16(slot0 + 0x24, 3)
        .put_u16(slot0 + 0x36, 0xffff)
        .put_u16(slot0 + 0x38, 0xffff);
    pair(&mut m, slot0 + 0x48, 2, STARTS);
    m.map(STARTS, 2 * 0x64);
    m.put_u16(SQUADS + 2 * 0x74 + 0x24, 3);

    pair(&mut m, SCNR + 0x168, 2, ZONES);
    m.map(ZONES, 2 * 0x38);
    named(&mut m, ZONES, "base_zone");
    let zone = ZONES + 0x38;
    named(&mut m, zone, "ff_drop_00");
    pair(&mut m, zone + 0x28, 3, FIRING);
    pair(&mut m, zone + 0x30, 1, AREAS);
    m.map(AREAS, 0x88);
    for (k, v) in [1.0, 2.0, 3.0].into_iter().enumerate() {
        m.put_f32(AREAS + 0x24 + 4 * k as u32, v);
    }
    m.map(FIRING, 3 * 0x20);
    for i in 0..3u32 {
        m.put_f32(FIRING + 0x20 * i, 10.0 * (i + 1) as f32)
            .put_u16(FIRING + 0x20 * i + 0x0c, 0xffff);
    }

    pair(&mut m, SCNR + 0x178, 1, 0x2_6000);
    pair(&mut m, SCNR + 0x240, 2, ORDERS);
    m.map(ORDERS, 2 * 0x7c);
    named(&mut m, ORDERS, "ff_other");
    named(&mut m, ORDERS + 0x7c, "ff_hunt");
    m
}

fn layout() -> KitLayout {
    read_kit(&scenario()).unwrap().expect("the kit")
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
        }]
    );
    assert_eq!((kit.characters, kit.team, kit.hunt_order), (1, 3, Some(1)));
    assert_eq!(
        kit.kit(),
        Kit {
            zones: vec![[1.0, 2.0, 3.0]],
            classes: [("elite".to_string(), 0)].into(),
            team: 3,
        }
    );
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
    let (result, calls, m) = drive(
        scenario(),
        move |cx| {
            let kit = kit.clone();
            Box::pin(async move { place_squad(&cx, &kit, placement(0, 0, 0)).await })
        },
        returns(0),
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

#[test]
fn a_placement_the_kit_cannot_hold_writes_and_calls_nothing() {
    for p in [placement(2, 0, 0), placement(0, 0, 1), placement(0, 1, 0)] {
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

#[test]
fn hunt_gives_the_kit_order_and_erase_takes_the_squad() {
    let kit = layout();
    let (result, calls, _) = drive(
        scenario(),
        move |cx| {
            let kit = kit.clone();
            Box::pin(async move {
                hunt(&cx, &kit, 1).await?;
                erase_squad(&cx, &kit, 1).await
            })
        },
        returns(0),
    );
    result.unwrap();
    assert_eq!(
        calls,
        [
            GuestCall::new(points::AI_SET_ORDERS.base).eax(2).ecx(1),
            GuestCall::new(points::AI_ERASE.base).args([2]),
        ]
    );

    let mut kit = layout();
    kit.hunt_order = None;
    let (result, calls, _) = drive(
        scenario(),
        move |cx| {
            let kit = kit.clone();
            Box::pin(async move { hunt(&cx, &kit, 0).await })
        },
        returns(0),
    );
    assert!(result.is_err());
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
                    line_of_sight(&cx, SCRATCH, [1.0, 2.0, 3.0], [4.0, 5.0, 6.0], 0xabcd_0001).await
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
                .ecx(u32::MAX)]
        );
        assert_eq!(m.vec3(SCRATCH).unwrap(), [1.0, 2.0, 3.0]);
        assert_eq!(m.vec3(SCRATCH + 12).unwrap(), [4.0, 5.0, 6.0]);
    }
}
