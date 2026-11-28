use super::*;
use guest_layout::datum::layout as dl;
use guest_layout::points;
use guest_layout::testing::FakeGuest;

#[test]
fn shots_are_heard_at_the_holders_world_position() {
    let mut g = FakeGuest::new();
    let clock = 0x1000;
    let objects = 0x2000;
    let headers = 0x3000;
    let holder = 0x4000;
    let weapon = 0x5000;
    let gun = DatumHandle::new(0xa101, 1);
    g.put_u32(points::GAME_TIME.base, clock)
        .put_u32(clock + crate::guest::GAME_TIME_TICK, 42)
        .put_u32(points::OBJECTS.base, objects)
        .map(objects, 0x48)
        .put_u32(objects + dl::ELEMENT_SIZE, 0xc)
        .put_u32(objects + dl::HIGH_WATER, 2)
        .put_u32(objects + dl::DATA, headers)
        .map(headers, 0x18)
        .put_u16(headers, 0xa100)
        .put_u8(headers + 3, objects::BIPED)
        .put_u32(headers + 8, holder)
        .put_u16(headers + 0xc, 0xa101)
        .put_u8(headers + 0xf, objects::WEAPON)
        .put_u32(headers + 0x14, weapon)
        .map(holder, 0x10c)
        .map(weapon, 0x10c)
        .put_u32(weapon + 0x14, DatumHandle::new(0xa100, 0).raw())
        .put_f32(holder + 0x64, 12.0)
        .put_f32(holder + 0x68, -3.0);
    let sound = shot_from_guest(&g, gun.raw()).unwrap().unwrap();
    assert_eq!(sound.at, [12.0, -3.0, crate::mind::EYE]);
    assert_eq!(sound.tick, 42);
    assert_eq!(sound.shooter, None, "no players array: no slot");
    assert!(shot_from_guest(&g, u32::MAX).unwrap().is_none());
}

#[test]
fn a_shot_names_the_slot_whose_unit_fired_and_shows_it_as_firing() {
    let mut g = FakeGuest::new();
    let clock = 0x1000;
    let objects = 0x2000;
    let headers = 0x3000;
    let holder = 0x4000;
    let weapon = 0x5000;
    let players = 0x6000;
    let roster = 0x6100;
    let gun = DatumHandle::new(0xa101, 1);
    let unit = DatumHandle::new(0xa100, 0);
    g.put_u32(points::GAME_TIME.base, clock)
        .put_u32(clock + crate::guest::GAME_TIME_TICK, 42)
        .put_u32(points::OBJECTS.base, objects)
        .map(objects, 0x48)
        .put_u32(objects + dl::ELEMENT_SIZE, 0xc)
        .put_u32(objects + dl::HIGH_WATER, 2)
        .put_u32(objects + dl::DATA, headers)
        .map(headers, 0x18)
        .put_u16(headers, 0xa100)
        .put_u8(headers + 3, objects::BIPED)
        .put_u32(headers + 8, holder)
        .put_u16(headers + 0xc, 0xa101)
        .put_u8(headers + 0xf, objects::WEAPON)
        .put_u32(headers + 0x14, weapon)
        .map(holder, 0x10c)
        .map(weapon, 0x10c)
        .put_u32(weapon + 0x14, unit.raw())
        // Two players: slot 0 controls another unit, slot 1 the holder.
        .put_u32(points::PLAYERS.base, players)
        .map(players, 0x48)
        .put_u32(players + dl::ELEMENT_SIZE, 0x21c)
        .put_u32(players + dl::HIGH_WATER, 2)
        .put_u32(players + dl::DATA, roster)
        .map(roster, 2 * 0x21c)
        .put_u16(roster, 0xe000)
        .put_u32(roster + 0x2c, DatumHandle::new(0xb000, 5).raw())
        .put_u16(roster + 0x21c, 0xe001)
        .put_u32(roster + 0x21c + 0x2c, unit.raw());
    let sound = shot_from_guest(&g, gun.raw()).unwrap().unwrap();
    assert_eq!(sound.shooter, Some(1));
    // The tracker's firing mask: within the heard window, by slot only.
    let other = ShotSound {
        at: [0.0; 3],
        tick: 30,
        shooter: Some(3),
    };
    let shots = [sound, other];
    assert_eq!(firing_in(shots.iter(), 42), 1 << 1);
    assert_eq!(firing_in(shots.iter(), 36), 1 << 3);
    assert_eq!(firing_in(shots.iter(), 42 + HEARD_TICKS + 1), 0);
}
