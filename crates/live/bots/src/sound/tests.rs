use super::*;
use engine::datum::layout as dl;
use engine::points;
use engine::testing::FakeGuest;

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
    assert!(shot_from_guest(&g, u32::MAX).unwrap().is_none());
}
