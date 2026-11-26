//! The active camouflage `c6810` on a synthetic guest: the Assassins
//! skull's campaign gate, retail's in the campaign and answered outside it
//! only through `skull_gate`. Every value is invented for the test.

use super::*;
use crate::sim_objects::{TABLE, drive};
use runtime::TransferAbi;
use runtime::guest::Permissions;

const STACK: u32 = 0x8000;
const CALLER: u32 = 0xdead;
const ARRAY: u32 = 0x0060_0000;
const HEADERS: u32 = 0x0060_1000;
const BODY: u32 = 0x0060_2000;
const TIMES: u32 = 0x0060_6000;
const OPTIONS: u32 = 0x0060_7000;
const UNIT: u32 = 0x8001_0001;

/// An actor's unit (body `+12c` an actor, team none) in options mode
/// `mode` with the Assassins byte on, the camouflage level at 0 rising at
/// 0.5 a second, half a second a tick.
fn world(mode: u32) -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(STACK - 0x1000, 0x2000, Permissions::RW).unwrap();
    g.map_zeroed(0x0045_d000, 0x2000, Permissions::RW).unwrap();
    g.map_zeroed(0x004e_0000, 0x4_0000, Permissions::RW).unwrap();
    g.map_zeroed(ARRAY, 0x1_0000, Permissions::RW).unwrap();
    g.set_u32(STACK, CALLER).unwrap();
    g.set_u32(OBJECTS, ARRAY).unwrap();
    g.set_u32(ARRAY + 0x44, HEADERS).unwrap();
    g.set_u32(HEADERS + 12 + 8, BODY).unwrap();
    g.set_u32(BODY + 0x12c, 7).unwrap();
    g.set_u16(BODY + 0x138, 0xffff).unwrap();
    g.set_u32(BODY + 0xd4, NONE).unwrap();
    g.set_u32(BODY + 0x2b8, 0.5f32.to_bits()).unwrap();
    g.set_u32(GAME, OPTIONS).unwrap();
    g.set_u32(OPTIONS + 8, mode).unwrap();
    g.set_u8(SKULLS + u32::from(ASSASSINS), 1).unwrap();
    g.set_u32(TIME, TIMES).unwrap();
    g.set_u32(TIMES + 4, 0.5f32.to_bits()).unwrap();
    g.set_u32(K_ONE, 1.0f32.to_bits()).unwrap();
    g.set_u32(K_CAMO_RATE, 1.0f32.to_bits()).unwrap();
    let cpu = CpuAbi { esp: STACK, eax: UNIT, ..Default::default() };
    (cpu, g)
}

/// Runs `c6810` to its return: body `+134` and the level `+2b0`.
fn camo_of(mode: u32) -> (u32, f32) {
    let (mut cpu, mut g) = world(mode);
    let out = drive(&mut cpu, &mut g, &TABLE, points::UNIT_CAMO_UPDATE.base).unwrap();
    assert_eq!(out.kind, TransferAbi::ret(0).kind);
    assert_eq!(out.target_pc, CALLER);
    (g.u32(BODY + 0x134).unwrap(), f32::from_bits(g.u32(BODY + 0x2b0).unwrap()))
}

#[test]
fn assassins_cloaks_an_actor_in_the_campaign_and_where_the_gate_is_answered() {
    let _held = crate::skull_gate::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::skull_gate::answer(0);
    // The campaign: retail's path, the gate not consulted.
    assert_eq!(camo_of(1), (8, 0.25));
    // A multiplayer game: retail never reads the byte.
    assert_eq!(camo_of(2), (0, 0.0));
    // A Firefight host answers the gate for Assassins: the byte decides.
    crate::skull_gate::answer(1 << ASSASSINS);
    assert_eq!(camo_of(2), (8, 0.25));
    // Another skull's answer is not Assassins'.
    crate::skull_gate::answer(1 << 12);
    assert_eq!(camo_of(2), (0, 0.0));
    crate::skull_gate::answer(0);
}
