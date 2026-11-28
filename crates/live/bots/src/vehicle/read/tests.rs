//! The reader on synthetic guest memory: a Warthog with its gun, the bot
//! driving and an enemy on the gun, a ghost nobody is in, a wreck.

use super::*;
use crate::guest::{MODE_MULTIPLAYER, OPTIONS, PLAYER_MACHINE, VARIANT_FLAGS};
use guest_layout::datum::layout as dl;
use guest_layout::testing::FakeGuest;

const GAME: u32 = 0x0060_0000;
const PLAYERS_HEADER: u32 = 0x0061_0000;
const PLAYERS_DATA: u32 = 0x0061_1000;
const OBJECTS_HEADER: u32 = 0x0062_0000;
const OBJECTS_DATA: u32 = 0x0062_1000;
const BODIES: u32 = 0x0063_0000;
const TAG_INDEX: u32 = 0x0068_0000;
const TAG_ENTRIES: u32 = 0x0068_1000;
const TAGS: u32 = 0x0069_0000;
const BOT_MACHINE: [u8; 6] = [0xc1, 0xa4, 0, 0, 0, 1];

fn array(g: &mut FakeGuest, header: u32, data: u32, size: u32, count: u32) {
    g.map(header, 0x48)
        .put_u32(header + dl::ELEMENT_SIZE, size)
        .put_u32(header + dl::HIGH_WATER, count)
        .put_u32(header + dl::DATA, data)
        .map(data, size * count.max(1));
}

fn body(i: u32) -> u32 {
    BODIES + i * 0x400
}
fn handle(i: u32) -> DatumHandle {
    DatumHandle::new(0xb000 + i as u16, i)
}
/// Tag `i`'s datum (as an object's definition holds it).
fn tag(i: u32) -> u32 {
    0xe100_0000 | i
}

fn vec3(g: &mut FakeGuest, at: u32, v: Vec3) {
    for (k, c) in (0u32..).zip(v) {
        g.put_f32(at + 4 * k, c);
    }
}

/// Objects: 0 the phantom's biped (player 0), 1 an enemy's biped
/// (player 1), 2 a Warthog at (10, 20, 1) facing +Y, 3 its gun mounted
/// 1 behind and 0.5 up, 4 an empty Ghost at (30, 0, 0), 5 a wrecked
/// Ghost, 6 a weapon on the Warthog's gun, 7 one on the Ghost, 8 one
/// the enemy biped holds. Tags: 0 the Warthog (driver, passenger), 1
/// its gun (gunner), 2 the Ghost (one seat, flags not set).
fn world() -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(GAME, 0x400)
        .put_u32(points::GAME_GLOBALS.base, GAME)
        .put_u32(GAME + OPTIONS, MODE_MULTIPLAYER)
        .put_u32(GAME + OPTIONS + VARIANT_FLAGS, 1);
    // Tags.
    g.put_u32(points::TAG_INDEX.base, TAG_INDEX)
        .map(TAG_INDEX, 0x40)
        .put_u32(TAG_INDEX + 0x18, 3);
    g.put_u32(points::TAG_ENTRIES.base, TAG_ENTRIES)
        .map(TAG_ENTRIES, 0x100);
    let seats: [(u16, &[u32]); 3] = [
        (1, &[SEAT_DRIVER | 1 << 4, 1 << 5]),
        (6, &[SEAT_GUNNER]),
        (4, &[0]),
    ];
    for (i, (class, flags)) in (0u32..).zip(seats) {
        let e = TAG_ENTRIES + 0x10 * i;
        let addr = TAGS + 0x1000 * i;
        g.put_u32(e, u32::from_be_bytes(*b"vehi"))
            .put_u32(e + 4, tag(i))
            .put_u32(e + 8, addr);
        g.map(addr, 0x1000);
        let block = addr + 0x400;
        g.put_u16(addr + VEHI_TYPE, class)
            .put_u32(addr + VEHI_SEATS, flags.len() as u32)
            .put_u32(addr + VEHI_SEATS + 4, block);
        for (k, f) in (0u32..).zip(flags) {
            g.put_u32(block + k * SEAT_STRIDE + SEAT_FLAGS, *f);
        }
    }
    // Objects.
    g.put_u32(points::OBJECTS.base, OBJECTS_HEADER);
    array(&mut g, OBJECTS_HEADER, OBJECTS_DATA, 0xc, 9);
    let kinds = [
        objects::BIPED,
        objects::BIPED,
        objects::VEHICLE,
        objects::VEHICLE,
        objects::VEHICLE,
        objects::VEHICLE,
        objects::WEAPON,
        objects::WEAPON,
        objects::WEAPON,
    ];
    for (i, kind) in (0u32..).zip(kinds) {
        let h = OBJECTS_DATA + i * 0xc;
        g.put_u16(h, handle(i).salt())
            .put_u8(h + 3, kind)
            .put_u32(h + 8, body(i));
        g.map(body(i), 0x400);
        g.put_u32(body(i) + 0x14, u32::MAX);
        vec3(&mut g, body(i) + 0x70, [1.0, 0.0, 0.0]);
        vec3(&mut g, body(i) + 0x7c, [0.0, 0.0, 1.0]);
        g.put_f32(body(i) + 0xec, 1.0);
    }
    // The bot drives the Warthog (seat 0), the enemy is on its gun.
    g.put_u32(body(0) + 0x14, handle(2).raw())
        .put_u16(body(0) + UNIT_SEAT, 0);
    vec3(&mut g, body(0) + 0x64, [0.2, 0.3, 0.4]);
    g.put_u32(body(1) + 0x14, handle(3).raw())
        .put_u16(body(1) + UNIT_SEAT, 0);
    // The Warthog, facing +Y.
    g.put_u32(body(2), tag(0));
    vec3(&mut g, body(2) + 0x64, [10.0, 20.0, 1.0]);
    vec3(&mut g, body(2) + 0x70, [0.0, 1.0, 0.0]);
    g.put_f32(body(2) + 0xec, 0.75);
    // Its gun: 1 behind, 0.5 up, relative to it.
    g.put_u32(body(3), tag(1))
        .put_u32(body(3) + 0x14, handle(2).raw());
    vec3(&mut g, body(3) + 0x64, [-1.0, 0.0, 0.5]);
    // The Ghost, and a wreck.
    g.put_u32(body(4), tag(2));
    vec3(&mut g, body(4) + 0x64, [30.0, 0.0, 0.0]);
    g.put_u32(body(5), tag(2))
        .put_u16(body(5) + 0x10a, BODY_DEPLETED);
    // Weapons: on the Warthog's gun, on the Ghost, and in the enemy's hands.
    g.put_u32(body(6) + 0x14, handle(3).raw());
    g.put_u32(body(7) + 0x14, handle(4).raw());
    g.put_u32(body(8) + 0x14, handle(1).raw());
    // Players: 0 the phantom (team 0), 1 the enemy (team 1).
    g.put_u32(points::PLAYERS.base, PLAYERS_HEADER);
    array(&mut g, PLAYERS_HEADER, PLAYERS_DATA, 0x21c, 2);
    for (i, machine) in (0u32..).zip([BOT_MACHINE, [0xc1, 0xa4, 0, 0, 0, 2]]) {
        let p = PLAYERS_DATA + i * 0x21c;
        g.put_u16(p, 0xe000 + i as u16)
            .put_u16(p + 0x28, u16::MAX)
            .put(p + PLAYER_MACHINE, &machine)
            .put_u32(p + 0x2c, handle(i).raw())
            .put_u8(p + 0xc0, i as u8);
    }
    g
}

fn near(a: Vec3, b: Vec3) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5)
}

#[test]
fn the_vehicles_their_seats_and_their_riders_are_read() {
    let g = world();
    let s = sight(&g, BOT_MACHINE).unwrap();
    let ids: Vec<u32> = s.vehicles.iter().map(|v| v.id).collect();
    assert_eq!(
        ids,
        vec![handle(2).raw(), handle(3).raw(), handle(4).raw()],
        "the wreck is left out"
    );
    let hog = s.get(handle(2).raw()).unwrap();
    assert_eq!(hog.class, Class::Jeep);
    assert_eq!(hog.health, 0.75);
    assert_eq!(hog.on, None);
    assert_eq!(
        hog.seats,
        vec![
            Seat {
                role: SeatRole::Driver,
                rider: Some(Rider { player: 0, team: 0 }),
                entries: Vec::new(),
                boards: None,
            },
            Seat {
                role: SeatRole::Passenger,
                rider: None,
                entries: Vec::new(),
                boards: None,
            },
        ]
    );
    let gun = s.get(handle(3).raw()).unwrap();
    assert_eq!(gun.class, Class::Turret);
    assert_eq!(gun.on, Some(handle(2).raw()));
    assert_eq!(gun.seats[0].role, SeatRole::Gunner);
    assert_eq!(gun.seats[0].rider, Some(Rider { player: 1, team: 1 }));
    // The gun in the world: 1 behind a Warthog facing +Y is -Y.
    assert!(near(gun.position, [10.0, 19.0, 1.5]), "{:?}", gun.position);
    assert!(near(gun.forward, [0.0, 1.0, 0.0]), "{:?}", gun.forward);
    // A Ghost whose one seat has no flags: it is the driver's.
    let ghost = s.get(handle(4).raw()).unwrap();
    assert_eq!(ghost.class, Class::Scout);
    assert_eq!(ghost.seats[0].role, SeatRole::Driver);
    assert_eq!(ghost.seats[0].rider, None);
    // The bot drives.
    assert_eq!(
        s.riding,
        Some(Riding {
            vehicle: handle(2).raw(),
            seat: 0,
            role: SeatRole::Driver
        })
    );
    // The enemy on the gun rides the Warthog.
    assert_eq!(
        s.ridden_by(1).map(|(v, r)| (v.id, r)),
        Some((hog.id, SeatRole::Gunner))
    );
    // A machine with no player has no rider.
    let other = sight(&g, [9; 6]).unwrap();
    assert_eq!(other.riding, None);
}

#[test]
fn a_rider_is_where_its_vehicle_is() {
    let g = world();
    let p = crate::guest::perceive(&g, BOT_MACHINE).unwrap();
    assert_eq!(p.me.map(|m| m.position), Some([10.0, 20.0, 1.0]));
    // The enemy on the gun: the gun's world position.
    assert!(
        near(p.others[0].position, [10.0, 19.0, 1.5]),
        "{:?}",
        p.others
    );
}

#[test]
fn a_vehicle_whose_tag_is_not_a_vehicles_is_left_out() {
    let mut g = world();
    g.put_u32(TAG_ENTRIES + 0x20, u32::from_be_bytes(*b"weap"));
    let s = sight(&g, BOT_MACHINE).unwrap();
    assert_eq!(s.vehicles.len(), 2);
    // Out of a multiplayer game nothing is read.
    g.put_u32(GAME + OPTIONS, 1);
    assert_eq!(sight_in_game(&g, BOT_MACHINE), Sight::default());
}

#[test]
fn seat_roles_come_from_the_flags_with_a_driver_guessed_when_none_is_flagged() {
    use SeatRole::*;
    assert_eq!(
        roles(&[SEAT_GUNNER, SEAT_DRIVER, 0]),
        vec![Gunner, Driver, Passenger]
    );
    assert_eq!(roles(&[SEAT_GUNNER, 0, 0]), vec![Gunner, Driver, Passenger]);
    assert_eq!(roles(&[SEAT_GUNNER]), vec![Gunner]);
    assert_eq!(roles(&[]), Vec::<SeatRole>::new());
}

#[test]
fn composing_turns_and_moves_a_local_pose() {
    let parent = [[1.0, 2.0, 3.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let local = [[2.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
    let [p, f, u] = compose(parent, local);
    // Forward +Y, so left is -X: 2 ahead and 1 left.
    assert!(near(p, [0.0, 4.0, 3.0]), "{p:?}");
    assert!(near(f, [0.0, 1.0, 0.0]));
    assert!(near(u, [0.0, 0.0, 1.0]));
}

#[test]
fn reads_a_seat_entry_marker_in_its_live_node_and_checks_the_tag_radius() {
    let mut g = world();
    let hlmt = TAGS + 0x4000;
    let mode = TAGS + 0x5000;
    let groups = mode + 0x200;
    let marker = mode + 0x240;
    let name = 0x0c00_0da6;
    g.put_u32(TAG_INDEX + 0x18, 5);
    for (i, kind, at) in [(3, *b"hlmt", hlmt), (4, *b"mode", mode)] {
        let e = TAG_ENTRIES + i * 0x10;
        g.put_u32(e, u32::from_be_bytes(kind))
            .put_u32(e + 4, tag(i))
            .put_u32(e + 8, at)
            .map(at, 0x400);
    }
    let seat = TAGS + 0x400;
    g.put_u32(TAGS + 0x38, tag(3))
        .put_u32(hlmt + 4, tag(4))
        .put_u32(mode + 0x14, 1)
        .put_u32(mode + 0x18, mode + 0x120)
        .put_f32(mode + 0x128, -0.7)
        .put_f32(mode + 0x12c, 0.8)
        .put_f32(mode + 0x130, -0.1)
        .put_f32(mode + 0x134, 1.2)
        .put_u32(mode + 0x58, 1)
        .put_u32(mode + 0x5c, groups)
        .put_u32(groups, name)
        .put_u32(groups + 4, 1)
        .put_u32(groups + 8, marker)
        .put_u32(seat + 0xc, name)
        .put_f32(seat + 0x98, 0.85)
        .put_f32(seat + 0x9c, 1.4)
        .put_f32(seat + 0xa0, 1.0)
        .put_f32(seat + 0xa4, 3.0)
        .put_u16(body(2) + 0x114, 0x34)
        .put_u16(body(2) + 0x116, 0x200);
    g.put_f32(marker + 0x1c, 1.0);
    vec3(&mut g, marker + 4, [0.5, 0.25, 0.2]);
    let node = body(2) + 0x200;
    g.put_f32(node, 1.0);
    vec3(&mut g, node + 4, [0.0, 1.0, 0.0]);
    vec3(&mut g, node + 0x10, [-1.0, 0.0, 0.0]);
    vec3(&mut g, node + 0x1c, [0.0, 0.0, 1.0]);
    vec3(&mut g, node + 0x28, [10.0, 20.0, 1.4]);
    let sight = sight(&g, BOT_MACHINE).unwrap();
    let hog = sight.get(handle(2).raw()).unwrap();
    assert_eq!(hog.spec().half_width, 0.8 + 0.12);
    let entry = hog.seats[0].entries[0];
    assert!(near(entry.at, [9.75, 20.5, 1.6]));
    assert!(near(entry.forward, [0.0, 1.0, 0.0]));
    assert_eq!(entry.radius, 0.85);
    assert!(crate::vehicle::plan::in_entry(
        hog,
        entry,
        [9.75, 21.3, 1.0]
    ));
    assert!(!crate::vehicle::plan::in_entry(
        hog,
        entry,
        [9.75, 21.4, 1.0]
    ));

    // The biped camera shares this model. With multiple variants only its
    // common marker is active. Resolve it in the biped's own live matrix.
    g.put_u32(body(0), tag(0))
        .put_u32(TAG_ENTRIES, u32::from_be_bytes(*b"bipd"))
        .put_u32(TAGS + 0xd4, name)
        .put_u32(groups + 4, 2)
        .put_u16(marker, 1)
        .put_u16(marker + 0x24, 0xffff)
        .put_f32(marker + 0x24 + 0x1c, 1.0)
        .put_u16(body(0) + 0x114, 0x34)
        .put_u16(body(0) + 0x116, 0x200);
    let head = body(0) + 0x200;
    g.put_f32(head, 1.0);
    vec3(&mut g, head + 4, [1.0, 0.0, 0.0]);
    vec3(&mut g, head + 0x10, [0.0, 1.0, 0.0]);
    vec3(&mut g, head + 0x1c, [0.0, 0.0, 1.0]);
    vec3(&mut g, head + 0x28, [20.0, 1.0, 3.0]);
    vec3(&mut g, marker + 0x24 + 4, [0.0, 0.0, 0.2]);
    assert!(near(
        unit_eye(&g, handle(0), 0.5).unwrap().unwrap(),
        [20.0, 1.0, 3.2]
    ));
}

#[test]
fn an_unmarked_biped_eye_follows_its_live_crouch_and_parent_pose() {
    let mut g = world();
    // Reuse the synthetic tag's backing memory as the passenger's biped.
    g.put_u32(body(0), tag(0))
        .put_u32(TAG_ENTRIES, u32::from_be_bytes(*b"bipd"))
        .put_f32(TAGS + 0x268, 0.725)
        .put_f32(TAGS + 0x26c, 0.5)
        .put_f32(TAGS + 0x270, 0.175);
    // The riding unit's origin [0.2, 0.3, 0.4] is relative to its vehicle
    // at [10, 20, 1] facing +Y. The upper collision cap uses height-radius.
    assert!(near(
        unit_eye(&g, handle(0), 0.0).unwrap().unwrap(),
        [9.7, 20.2, 1.95]
    ));
    assert!(near(
        unit_eye(&g, handle(0), 1.0).unwrap().unwrap(),
        [9.7, 20.2, 1.725]
    ));
    assert!(unit_eye(&g, DatumHandle::new(1, 0), 0.0).unwrap().is_none());
}

#[test]
fn a_mounted_weapons_shot_is_the_seated_players_and_a_held_one_is_its_holder() {
    let g = world();
    // The Warthog's chaingun: its gunner (player 1) fires it.
    assert_eq!(
        firing_player(&g, handle(6)).unwrap(),
        Some(FiredBy::Seated(1))
    );
    // The Ghost's plasma with nobody aboard: nobody.
    assert_eq!(firing_player(&g, handle(7)).unwrap(), None);
    // A handheld weapon: its holder's biped.
    assert_eq!(
        firing_player(&g, handle(8)).unwrap(),
        Some(FiredBy::Holder(handle(1)))
    );
    // The bot boards the empty Ghost: the driver fires the plasma.
    let mut g = world();
    g.put_u32(body(0) + 0x14, handle(4).raw())
        .put_u16(body(0) + UNIT_SEAT, 0);
    assert_eq!(
        firing_player(&g, handle(7)).unwrap(),
        Some(FiredBy::Seated(0))
    );
}
