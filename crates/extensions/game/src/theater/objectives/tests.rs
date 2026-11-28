use super::*;
use guest_layout::testing::FakeGuest;

const E: u32 = 0x8000;
const A: u32 = 0xc000;

fn game(mode: u32) -> FakeGuest {
    let mut m = FakeGuest::default();
    m.put_u32(objectives::GAME_ENGINE, E)
        .put_u32(E + objectives::ENGINE_INDEX, mode);
    m
}

fn registration(m: &mut FakeGuest, slot: u32, datum: u32, designator: u16, unit: u32, player: u32) {
    let r = E + objectives::OBJECTIVES + slot * objectives::OBJECTIVE_STRIDE;
    m.put_u32(r + objectives::RECORD_OBJECT, datum)
        .put_u16(r + objectives::RECORD_TEAM, designator)
        .put_u32(r + objectives::RECORD_UNIT, unit)
        .put_u32(r + objectives::RECORD_PLAYER, player);
}

#[test]
fn item_changes_follow_salted_handles_through_registry_compaction() {
    let mut m = game(3);
    let mut tracker = Tracker::default();
    let none = u32::MAX;
    m.put_u32(E + objectives::OBJECTIVE_COUNT, 2);
    registration(&mut m, 0, 0xa001_0004, 0, none, none);
    registration(&mut m, 1, 0xa030_0005, 1, none, none);
    let events = tracker.observe(&m).unwrap();
    assert!(matches!(
        events[..],
        [
            Event::ObjectiveState(Change::Item {
                item: Item::Ball,
                change: ItemChange::Registered,
                designator: 0,
                ..
            }),
            Event::ObjectiveState(Change::Item {
                item: Item::Ball,
                change: ItemChange::Registered,
                designator: 1,
                ..
            })
        ]
    ));
    registration(&mut m, 0, 0xa030_0005, 1, 0xb011_0002, 0xc000_0003);
    m.put_u32(E + objectives::OBJECTIVE_COUNT, 1);
    let events = tracker.observe(&m).unwrap();
    assert!(matches!(
        events[..],
        [
            Event::ObjectiveState(Change::Item {
                change: ItemChange::Unregistered,
                object: 0xa001_0004,
                ..
            }),
            Event::ObjectiveState(Change::Item {
                change: ItemChange::CarrierChanged,
                object: 0xa030_0005,
                unit: Some(0xb011_0002),
                player: Some(3),
                ..
            })
        ]
    ));
    registration(&mut m, 0, 0xa030_0005, 1, none, none);
    assert!(matches!(
        tracker.observe(&m).unwrap()[..],
        [Event::ObjectiveState(Change::Item {
            change: ItemChange::CarrierChanged,
            unit: None,
            player: None,
            ..
        })]
    ));
    registration(&mut m, 0, 0xa031_0005, 1, none, none);
    assert!(matches!(
        tracker.observe(&m).unwrap()[..],
        [
            Event::ObjectiveState(Change::Item {
                change: ItemChange::Unregistered,
                object: 0xa030_0005,
                ..
            }),
            Event::ObjectiveState(Change::Item {
                change: ItemChange::Registered,
                object: 0xa031_0005,
                ..
            })
        ]
    ));
    assert!(tracker.observe(&m).unwrap().is_empty());
}

#[test]
fn reads_arming_plant_and_actual_hill_scoring_mask() {
    let mut m = game(9);
    let mut tracker = Tracker::default();
    m.put_u32(points::CTF_ASSAULT_GLOBALS.base, A);
    for team in 0..9 {
        m.put_u16(A + 0x1a0 + 2 * team, u16::MAX);
    }
    assert!(tracker.observe(&m).unwrap().is_empty());
    m.put_u8(A + 0x170 + 8, 10).put_u16(A + 0x1a0 + 16, 4);
    assert_eq!(
        tracker.observe(&m).unwrap(),
        vec![Event::ObjectiveState(Change::BombState {
            team: 8,
            arming: true,
            armed: true,
            planted: true,
        })]
    );
    m.put_u32(E + objectives::ENGINE_INDEX, 4)
        .put_u32(objectives::KOTH_GLOBALS, A)
        .put_u32(A + objectives::KOTH_HILL, 6)
        .put_u16(A + objectives::HILL_OCCUPANTS, 0x8002);
    assert_eq!(
        tracker.observe(&m).unwrap(),
        vec![Event::ObjectiveState(Change::Hill {
            index: 6,
            occupants: 0x8002
        })]
    );
    assert!(tracker.observe(&m).unwrap().is_empty());
    tracker.start_match();
    assert_eq!(tracker.observe(&m).unwrap().len(), 1);
}

#[test]
fn applied_powerup_preserves_the_exact_object_even_without_a_player_or_placement() {
    use guest_layout::datum::layout as d;
    let mut m = FakeGuest::default();
    let sp = 0x2000;
    let body = 0x7000;
    let object = 0xa153_0000;
    let unit = 0xb080_0001;
    let definition = 0xcafe_0090;
    m.put_u32(sp, 0x151fd2)
        .put_u32(sp + 0x20, object)
        .put_u32(points::OBJECTS.base, A)
        .put_u32(A + d::ELEMENT_SIZE, 12)
        .put_u32(A + d::HIGH_WATER, 2)
        .put_u32(A + d::DATA, E)
        .put_u16(E, 0xa153)
        .put_u8(E + 3, live_telemetry::layout::OBJECT_TYPE_EQUIPMENT)
        .put_u32(E + 8, body)
        .put_u32(body, definition);
    let event = powerup(&m, sp, unit, definition).unwrap();
    assert_eq!(
        event,
        Some(Event::PowerupTaken {
            object,
            unit,
            definition,
            player: None
        })
    );
    assert_eq!(powerup(&m, sp, unit, definition ^ 1).unwrap(), None);
    m.put_u32(sp, 0x151f00);
    assert_eq!(powerup(&m, sp, unit, definition).unwrap(), None);
}

#[test]
fn territory_owner_uses_the_salted_player_and_juggernaut_uses_engine_role_mask() {
    use guest_layout::datum::layout as d;
    use guest_layout::view::players::Player;

    let mut m = game(8);
    let mut tracker = Tracker::default();
    let header = 0x2000;
    let player = 0x4000;
    m.put_u32(points::TERRITORIES_GLOBALS.base, A)
        .put_u16(A + 0x114, 1)
        .put_u32(A + 0x70, 0xabcd_0000)
        .put_u8(A + 0x95, 0)
        .put_u32(points::PLAYERS.base, header)
        .put_u32(header + d::HIGH_WATER, 1)
        .put_u32(header + d::ELEMENT_SIZE, 0x21c)
        .put_u32(header + d::DATA, player)
        .put_u16(player, 0xabcd)
        .put_u8(player + Player::TEAM, 4)
        .put_u32(player + Player::UNIT, 0xb00b_0003);
    assert_eq!(
        tracker.observe(&m).unwrap(),
        vec![Event::ObjectiveState(Change::Territory {
            index: 0,
            owner: Some(0),
            team: Some(4),
            occupants: 1
        })]
    );
    m.put_u16(player, 0xabce).put_u8(A + 0x95, u8::MAX);
    assert_eq!(
        tracker.observe(&m).unwrap(),
        vec![Event::ObjectiveState(Change::Territory {
            index: 0,
            owner: None,
            team: None,
            occupants: 0
        })]
    );
    m.put_u32(E + objectives::ENGINE_INDEX, 7)
        .put_u32(points::JUGGERNAUT_GLOBALS.base, A)
        .put_u16(A, 1 << 12);
    assert_eq!(
        tracker.observe(&m).unwrap(),
        vec![Event::ObjectiveState(Change::Juggernauts {
            players: 1 << 12
        })]
    );
}

#[test]
fn corrupt_registration_keeps_the_previous_salted_carrier() {
    let mut m = game(3);
    let mut tracker = Tracker::default();
    m.put_u32(E + objectives::OBJECTIVE_COUNT, 1);
    registration(&mut m, 0, 0xb001_0001, 0, u32::MAX, u32::MAX);
    assert_eq!(tracker.observe(&m).unwrap().len(), 1);
    m.put_u32(E + objectives::OBJECTIVE_COUNT, u32::MAX);
    assert!(tracker.observe(&m).is_err());
    m.put_u32(E + objectives::OBJECTIVE_COUNT, 1);
    assert!(tracker.observe(&m).unwrap().is_empty());
    registration(&mut m, 0, 0xb001_0001, 0, 0xb002_0002, 0xc001_0002);
    assert!(matches!(
        tracker.observe(&m).unwrap()[..],
        [Event::ObjectiveState(Change::Item {
            change: ItemChange::CarrierChanged,
            player: Some(2),
            ..
        })]
    ));
}
