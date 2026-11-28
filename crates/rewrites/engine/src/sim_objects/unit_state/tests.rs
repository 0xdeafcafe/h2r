//! The slice 6b bodies on a synthetic guest: their callouts, frames and
//! returns, and what they store. Every value is made up for the test (the
//! image constants they read are planted with test values); no game data.

use super::*;
use guest_layout::points;
use crate::sim_objects::{TABLE, drive, frame};
use runtime::guest::Permissions;
use runtime::{CpuAbi, Guest, TransferAbi};

const STACK: u32 = 0x8000;
const CALLER: u32 = 0xdead;
const ARRAY: u32 = 0x0060_0000;
const HEADERS: u32 = 0x0060_1000;
const BODY: u32 = 0x0060_2000;
const TAGS: u32 = 0x0060_4000;
const DEF: u32 = 0x0060_5000;
const TIME: u32 = 0x0060_6000;
const VECTOR: u32 = 0x0060_7000;
const SEATS: u32 = 0x0060_8000;
const UNIT: u32 = 0x8001_0001;
/// The registers every body must hand back.
const SAVED: (u32, u32, u32, u32) = (0x1111, 0x2222, 0x3333, 0x4444);

fn f(v: f32) -> u32 {
    v.to_bits()
}

fn f32_at(g: &Guest, addr: u32) -> f32 {
    f32::from_bits(g.u32(addr).unwrap())
}

fn vec3(g: &mut Guest, at: u32, v: [f32; 3]) {
    for (i, x) in v.into_iter().enumerate() {
        g.set_u32(at + 4 * i as u32, f(x)).unwrap();
    }
}

/// Object slot 1, a unit with a definition at tag index 0, no linked
/// objects and no parent, game time at 30 ticks a second, and the image
/// constants the bodies read, with values chosen for the test.
fn world() -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(STACK - 0x1000, 0x2000, Permissions::RW).unwrap();
    g.map_zeroed(0x0044_a000, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(0x0045_d000, 0x2000, Permissions::RW).unwrap();
    g.map_zeroed(0x0046_8000, 0x1000, Permissions::RW).unwrap();
    // The globals, game time among them.
    g.map_zeroed(0x004e_0000, 0x4_0000, Permissions::RW).unwrap();
    // Zeroed code: the x87 exits read their instruction's bytes for FOP.
    g.map_zeroed(0x000c_0000, 0x1_0000, Permissions::RW).unwrap();
    g.map_zeroed(ARRAY, 0x1_0000, Permissions::RW).unwrap();
    g.set_u32(STACK, CALLER).unwrap();
    g.set_u32(points::OBJECTS.base, ARRAY).unwrap();
    g.set_u32(ARRAY + 0x44, HEADERS).unwrap();
    g.set_u16(HEADERS + 12, (UNIT >> 16) as u16).unwrap();
    g.set_u8(HEADERS + 12 + 2, 0x01).unwrap();
    g.set_u32(HEADERS + 12 + 8, BODY).unwrap();
    for link in [0x0c, 0x10, 0x14, 0x248, 0x24c] {
        g.set_u32(BODY + link, NONE).unwrap();
    }
    g.set_u32(points::TAG_ENTRIES.base, TAGS).unwrap();
    g.set_u32(TAGS + 8, DEF).unwrap();
    g.set_u32(points::GAME_TIME.base, TIME).unwrap();
    g.set_u16(TIME + 2, 30).unwrap();
    g.set_u32(TIME + 4, f(1.0 / 30.0)).unwrap();
    g.set_u32(TIME + 8, 0x1234).unwrap();
    g.set_u32(ONE, f(1.0)).unwrap();
    g.set_u32(SMALL, f(0.001)).unwrap();
    g.set_u32(VECTOR_A4, VECTOR).unwrap();
    g.set_u32(VECTOR_A8, VECTOR + 0x10).unwrap();
    vec3(&mut g, VECTOR + 0x10, [7.0, 8.0, 9.0]);
    let (ebx, ebp, esi, edi) = SAVED;
    // The default x87 state: empty, every exception masked.
    let cpu = CpuAbi { esp: STACK, ebx, ebp, esi, edi, ..Default::default() };
    (cpu, g)
}

/// Return from a callout that pops `cleanup` argument bytes.
fn returned(cpu: &mut CpuAbi, g: &Guest, cleanup: u32) -> u32 {
    let site = frame::pop(cpu, g).unwrap();
    cpu.esp += cleanup;
    site
}

fn saved(cpu: &CpuAbi) -> (u32, u32, u32, u32) {
    (cpu.ebx, cpu.ebp, cpu.esi, cpu.edi)
}

// ---------------------------------------------------------------- c5eb0, ce0c0

#[test]
fn seats_return_zero_at_once_for_a_definition_without_them() {
    let (mut cpu, mut g) = world();
    g.set_u32(DEF + 0xbc, 1 << 11).unwrap();
    g.set_u32(STACK - 4, CALLER).unwrap();
    cpu.esp = STACK - 4;
    cpu.eax = UNIT;
    cpu.ecx = 0xccdd_eeff;
    let out = drive(&mut cpu, &mut g, &TABLE, points::UNIT_POWERED_SEATS.base).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(cpu.eax & 0xff, 0);
    // The result byte is the saved `ecx`'s top byte.
    assert_eq!(cpu.ecx, 0x00dd_eeff);
    assert_eq!(cpu.esp, STACK);
    assert_eq!(saved(&cpu), SAVED);
}

#[test]
fn seats_refresh_the_control_then_move_each_seat_by_a_tick() {
    let (mut cpu, mut g) = world();
    g.set_u32(STACK - 4, CALLER).unwrap();
    cpu.esp = STACK - 4;
    cpu.eax = UNIT;
    cpu.ecx = 0xccdd_eeff;
    // Bits 25 and 26 cleared by the refresh; bit 28 picks the reset blend.
    g.set_u32(BODY + 0x134, 0x0600_0000 | 1 << 28).unwrap();
    vec3(&mut g, BODY + 0x288, [4.0, 8.0, 6.0]);
    vec3(&mut g, BODY + 0x294, [1.0, 1.0, 1.0]);
    g.set_u32(super::seats::BLEND.0, f(0.9)).unwrap();
    g.set_u32(super::seats::BLEND.1, f(0.1)).unwrap();
    g.set_u32(super::seats::BLEND_RESET.0, f(0.5)).unwrap();
    g.set_u32(super::seats::BLEND_RESET.1, f(0.25)).unwrap();
    // Two seats: a driver in the first, the second empty.
    g.set_u32(DEF + 0x1b8, 2).unwrap();
    g.set_u32(DEF + 0x1bc, SEATS).unwrap();
    g.set_u32(SEATS, f(0.5)).unwrap();
    g.set_u32(SEATS + 8, f(0.25)).unwrap();
    g.set_u32(BODY + 0x248, 0x8005_0005).unwrap();
    g.set_u32(BODY + 0x260, f(0.5)).unwrap();

    let out = drive(&mut cpu, &mut g, &TABLE, points::UNIT_POWERED_SEATS.base).unwrap();
    assert_eq!(out, TransferAbi::call(callee::D0C10, 0xc5eff));
    assert_eq!((cpu.ebx, cpu.edi, cpu.esi, cpu.ebp), (UNIT, UNIT, BODY, DEF));
    // ecx, ebp, esi, edi, ebx pushed; the return address.
    assert_eq!(cpu.esp, STACK - 4 - 20 - 4);
    let site = returned(&mut cpu, &g, 0);

    // The refresh is fused: its first callout is the control frame.
    let out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::call(callee::D1080, 0xce140));
    assert_eq!(cpu.eax, UNIT);
    let e = STACK - 4 - 20 - 4 - 0x6c - 8;
    assert_eq!(cpu.esp, e - 8 - 4);
    assert_eq!(g.u32(cpu.esp + 4).unwrap(), e + 0x40);
    assert_eq!(g.u32(cpu.esp + 8).unwrap(), e + 0x2c);
    assert_eq!(g.u32(BODY + 0x134).unwrap(), 1 << 28);
    let site = returned(&mut cpu, &g, 8);
    // No frame: the three changes are zero, and the reset blend runs.
    cpu.eax = 0x0060_0000;

    let out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(g.u32(BODY + 0x134).unwrap(), 0);
    for o in [0x294, 0x298, 0x29c] {
        assert_eq!(f32_at(&g, BODY + o), 0.0);
    }
    assert_eq!(f32_at(&g, BODY + 0x288), 2.0);
    assert_eq!(f32_at(&g, BODY + 0x28c), 4.0);
    assert_eq!(f32_at(&g, BODY + 0x290), 3.0);
    // The driver's seat powers up by one tick over half a second; the
    // empty one down by one tick over a quarter.
    assert_eq!(f32_at(&g, BODY + 0x25c), 1.0f32 / (30.0f32 * 0.5));
    assert_eq!(f32_at(&g, BODY + 0x260), 0.5f32 - 1.0f32 / (30.0f32 * 0.25));
    assert_eq!(cpu.eax & 0xff, 1);
    assert_eq!(cpu.ecx, 0x01dd_eeff);
    assert_eq!(cpu.esp, STACK);
    assert_eq!(saved(&cpu), SAVED);
}

// ---------------------------------------------------------------- c58f0

#[test]
fn state_counts_down_its_timers_and_sets_the_states_when_one_runs_out() {
    let (mut cpu, mut g) = world();
    // Slice 7a made the action request `e68c0` a body; without it here,
    // it stays the callout this test answers.
    let table = crate::sim_objects::Table::new(
        crate::sim_objects::BODIES.iter().filter(|b| b.point.base != points::BIPED_ACTION.base),
    )
    .unwrap();
    g.set_u32(STACK + 4, UNIT).unwrap();
    // Bit 10: no weapon or seat states; bit 11 clear: the timers and the
    // weapon loop run.
    g.set_u32(DEF + 0xbc, 1 << 10).unwrap();
    g.set_u8(BODY + 0x1f5, 1).unwrap();
    g.set_u8(BODY + 0x1fa, 2).unwrap();
    g.set_u8(BODY + 0x212, 0xff).unwrap();
    g.set_u8(BODY + 0x213, 0xff).unwrap();

    let e = STACK - 0x28 - 16;
    for state in [0x13, 9] {
        let out = drive(&mut cpu, &mut g, &table, if state == 0x13 { points::UNIT_UPDATE_STATE.base } else { 0xc5968 })
            .unwrap();
        assert_eq!(out.target_pc, callee::STATE_SET);
        assert_eq!(cpu.ecx, state);
        assert_eq!(g.u32(cpu.esp + 4).unwrap(), UNIT);
        assert_eq!(cpu.esp, e - 8);
        let site = returned(&mut cpu, &g, 4);
        assert_eq!(site, out.resume_pc);
        if state == 9 {
            // Both held weapon slots are empty: straight to `d1540`.
            let out = drive(&mut cpu, &mut g, &table, site).unwrap();
            assert_eq!(out, TransferAbi::call(callee::D1540, 0xc5ea0));
            assert_eq!(g.u32(cpu.esp + 4).unwrap(), UNIT);
            let site = returned(&mut cpu, &g, 4);
            let out = drive(&mut cpu, &mut g, &table, site).unwrap();
            assert_eq!(out, TransferAbi::ret(CALLER));
        }
    }
    assert_eq!(g.u8(BODY + 0x1f5).unwrap(), 0);
    assert_eq!(g.u8(BODY + 0x1fa).unwrap(), 1);
    assert_eq!(g.u32(e + 0x18).unwrap(), 2, "the loop index slot");
    assert_eq!(cpu.eax & 0xff, 1);
    assert_eq!(cpu.esp, STACK + 8);
    assert_eq!(saved(&cpu), SAVED);
}

// ---------------------------------------------------------------- c49b0

#[test]
fn control_takes_the_facing_d0080_writes_as_the_desired_directions() {
    let (mut cpu, mut g) = world();
    g.set_u32(STACK + 4, UNIT).unwrap();
    g.set_u32(BODY + 0x134, 1 << 21).unwrap();
    g.set_u32(BODY + 0x148, 0x55).unwrap();
    g.set_u32(DEF + 0xbc, 1 << 11).unwrap();

    let out = drive(&mut cpu, &mut g, &TABLE, points::UNIT_UPDATE_CONTROL.base).unwrap();
    assert_eq!(out, TransferAbi::call(callee::D0D20, 0xc49f8));
    assert_eq!((cpu.eax, cpu.ebx, cpu.edi, cpu.ebp), (UNIT, UNIT, BODY, DEF));
    assert_eq!(cpu.esp, STACK - 0x1c - 16 - 4);
    let site = returned(&mut cpu, &g, 0);

    let out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::call(callee::D0080, 0xc4a15));
    assert_eq!((cpu.eax, cpu.esi), (UNIT, BODY + 0x150));
    let site = returned(&mut cpu, &g, 0);
    vec3(&mut g, BODY + 0x150, [1.0, 2.0, 3.0]);

    let out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    for o in [0x15c, 0x180] {
        assert_eq!([0, 4, 8].map(|i| f32_at(&g, BODY + o + i)), [1.0, 2.0, 3.0]);
    }
    assert_eq!([0, 4, 8].map(|i| f32_at(&g, BODY + 0x1b0 + i)), [7.0, 8.0, 9.0]);
    assert_eq!(g.u32(BODY + 0x148).unwrap(), 0);
    assert_eq!(cpu.eax & 0xff, 1);
    assert_eq!(cpu.esp, STACK + 8);
    assert_eq!(saved(&cpu), SAVED);
}

/// `c49b0` up to its first callout and back, with body `+134` bit 1 set
/// (no facing copy) and `+1ec` ticks left of a timed bit.
fn control_to_the_timers(g: &mut Guest, cpu: &mut CpuAbi, left: u32) -> crate::sim_objects::Result<TransferAbi> {
    g.set_u32(STACK + 4, UNIT).unwrap();
    g.set_u32(BODY + 0x134, 2).unwrap();
    g.set_u32(BODY + 0x1ec, left).unwrap();
    g.set_u32(BODY + 0x1f0, 0x1_0000).unwrap();
    g.set_u32(0x0045_dc20, f(0.5)).unwrap();
    let out = drive(cpu, g, &TABLE, points::UNIT_UPDATE_CONTROL.base).unwrap();
    assert_eq!(out.target_pc, callee::D0D20);
    let site = returned(cpu, g, 0);
    drive(cpu, g, &TABLE, site)
}

#[test]
fn control_toggles_the_timed_bit_every_rate_ticks() {
    // 3 ticks left, a rate of 15 ticks: the bit is cleared.
    let (mut cpu, mut g) = world();
    let out = control_to_the_timers(&mut g, &mut cpu, 3).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(g.u32(BODY + 0x148).unwrap(), 0);
    assert_eq!(g.u32(BODY + 0x1ec).unwrap(), 2);
    assert_eq!(f32_at(&g, BODY + 0x1c0), 1.0);
    // `fld; fistp` leave the stack empty, the pointers at the `fistp`.
    assert_eq!(cpu.x87.tag, 0xffff);
    assert_eq!(cpu.x87.instruction_pointer, 0xc4c0c);
    assert_eq!(cpu.x87.data_pointer, STACK - 0x1c - 16 + 0x1c);
    assert_eq!(cpu.eax & 0xff, 1);
    assert_eq!(saved(&cpu), SAVED);

    // 15 ticks left: the bit is set and the time stamped.
    let (mut cpu, mut g) = world();
    let out = control_to_the_timers(&mut g, &mut cpu, 15).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(g.u32(BODY + 0x148).unwrap(), 0x1_0000);
    assert_eq!(g.u32(BODY + 0x244).unwrap(), 0x1234);
}

#[test]
fn control_with_no_ticks_a_second_is_the_guests_divide_error() {
    let (mut cpu, mut g) = world();
    g.set_u16(TIME + 2, 0).unwrap();
    let fault = control_to_the_timers(&mut g, &mut cpu, 3).unwrap_err();
    assert_eq!(fault.addr, 0xc4c19);
}
