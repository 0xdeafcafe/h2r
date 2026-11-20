//! The slice 7a bodies on a synthetic guest: the frame and registers at
//! their callouts and returns, and what they store. Every value is made
//! up for the test (the image constants and instruction immediates they
//! read are planted with test values); no game data.

use super::*;
use crate::sim_objects::{TABLE, drive, frame};
use runtime::TransferAbi;
use runtime::guest::Permissions;

const STACK: u32 = 0x0007_0000;
const CALLER: u32 = 0xdead;
const ARRAY: u32 = 0x0060_0000;
const HEADERS: u32 = 0x0060_1000;
const BODY: u32 = 0x0060_2000;
const TAGS: u32 = 0x0060_4000;
const DEF: u32 = 0x0060_5000;
const TIME: u32 = 0x0060_6000;
const BIPED: u32 = 0x8001_0001;

fn f(v: f32) -> u32 {
    v.to_bits()
}

/// Object slot 1, a biped (type 0, active) with its definition at tag
/// index 0 and game time at 30 ticks a second; slots 2 and 3 for parent
/// chains. The whole low 8 MiB is mapped, so image addresses (constants,
/// immediates, the x87 exits' instruction bytes) read as planted or zero.
fn world() -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(0, 0x0080_0000, Permissions::RW).unwrap();
    g.set_u32(STACK, CALLER).unwrap();
    g.set_u32(STACK + 4, BIPED).unwrap();
    g.set_u32(points::OBJECTS.base, ARRAY).unwrap();
    g.set_u32(ARRAY + 0x44, HEADERS).unwrap();
    for slot in 1..4u32 {
        let h = HEADERS + 12 * slot;
        g.set_u16(h, 0x8001).unwrap();
        g.set_u8(h + 2, 0x01).unwrap();
        g.set_u32(h + 8, BODY + 0x400 * (slot - 1)).unwrap();
        for link in [0x0c, 0x10, 0x14, 0xb4, 0xd4, 0x13c] {
            g.set_u32(BODY + 0x400 * (slot - 1) + link, NONE).unwrap();
        }
        g.set_u16(BODY + 0x400 * (slot - 1) + 0x12a, 0xffff).unwrap();
    }
    g.set_u32(points::TAG_ENTRIES.base, TAGS).unwrap();
    g.set_u32(TAGS + 8, DEF).unwrap();
    g.set_u32(points::GAME_TIME.base, TIME).unwrap();
    g.set_u16(TIME + 2, 30).unwrap();
    g.set_u32(TIME + 4, f(1.0 / 30.0)).unwrap();
    g.set_u32(TIME + 8, 0x1234).unwrap();
    let cpu = CpuAbi { esp: STACK, ebx: 0x1111, ebp: 0x2222, esi: 0x3333, edi: 0x4444, ..Default::default() };
    (cpu, g)
}

fn run(f: fn(&mut Cx<'_>, u32) -> Result<Next>, cpu: &mut CpuAbi, g: &mut Guest, pc: u32) -> Next {
    f(&mut Cx { cpu, g }, pc).unwrap()
}

/// Return from a callout that pops `cleanup` argument bytes.
fn returned(cpu: &mut CpuAbi, g: &Guest, cleanup: u32) -> u32 {
    let site = frame::pop(cpu, g).unwrap();
    cpu.esp += cleanup;
    site
}

#[test]
fn the_carrier_skips_ancestors_flagged_bit_26() {
    let (mut cpu, mut g) = world();
    // 1 -> 2 -> 3: 1 and 2 flagged, 3 not.
    g.set_u32(BODY + 4, 1 << 26).unwrap();
    g.set_u32(BODY + 0x14, 0x8001_0002).unwrap();
    g.set_u32(BODY + 0x404, 1 << 26).unwrap();
    g.set_u32(BODY + 0x414, 0x8001_0003).unwrap();
    cpu.eax = BIPED;
    assert_eq!(run(carrier, &mut cpu, &mut g, points::OBJECT_CARRIER.base), Next::Ret { pop: 0 });
    assert_eq!(cpu.eax, 0x8001_0003);
    assert_eq!(cpu.ecx, BODY + 0x800);
    assert_eq!((cpu.esi, cpu.esp), (0x3333, STACK));
    // `test dl, 1` with the bit clear.
    assert_eq!(cpu.eflags & 0x40, 0x40);

    // The chain ends flagged: none.
    g.set_u32(BODY + 0x804, 1 << 26).unwrap();
    cpu.eax = BIPED;
    run(carrier, &mut cpu, &mut g, points::OBJECT_CARRIER.base);
    assert_eq!(cpu.eax, NONE);
    assert_eq!(cpu.esi, 0x3333);

    // None in, none out, nothing pushed.
    cpu.eax = NONE;
    cpu.ecx = 0x77;
    run(carrier, &mut cpu, &mut g, points::OBJECT_CARRIER.base);
    assert_eq!((cpu.eax, cpu.ecx, cpu.esp), (NONE, 0x77, STACK));
}

#[test]
fn the_animation_kind_follows_the_signed_compares() {
    let (mut cpu, mut g) = world();
    // Test values at the nine immediates.
    for (at, v) in [
        (0xdd302, 1000),
        (0xdd30e, 500),
        (0xdd318, 100),
        (0xdd320, 200),
        (0xdd329, 600),
        (0xdd331, 700),
        (0xdd33d, 1010),
        (0xdd34a, 20),
    ] {
        g.set_u32(at, v).unwrap();
    }
    g.set_u8(0xdd345, 3).unwrap();
    for (name, kind, ecx) in [
        (1000u32, 0u8, 1000u32),
        (500, 0, 500),
        (100, 0, 100),
        (200, 1, 200),
        (50, 2, 50),
        (450, 2, 450),
        (550, 2, 550),
        (650, 1, 650),
        (700, 1, 700),
        (800, 2, 800),
        (1010, 1, 0),
        (1013, 1, 0),
        (1033, 1, 0),
        (1034, 2, 1),
        ((-5i32) as u32, 2, (-5i32) as u32),
    ] {
        cpu.ecx = name;
        cpu.eax = 0xabcd_ef00;
        assert_eq!(run(animation_kind, &mut cpu, &mut g, points::BIPED_ANIMATION_KIND.base), Next::Ret { pop: 0 });
        assert_eq!(cpu.eax, 0xabcd_ef00 | u32::from(kind), "name {name}");
        assert_eq!(cpu.ecx, ecx, "name {name}");
    }
}

#[test]
fn seconds_become_ticks_in_eax_and_ecx() {
    let (mut cpu, mut g) = world();
    g.set_u32(STACK + 4, f(0.5)).unwrap();
    cpu.ecx = 0x5555;
    let next = run(seconds_to_ticks, &mut cpu, &mut g, points::GAME_SECONDS_TO_TICKS.base);
    assert_eq!(next, Next::Ret { pop: 4 });
    assert_eq!((cpu.eax, cpu.ecx, cpu.esp), (15, 15, STACK));
    // The product is stored over the argument.
    assert_eq!(g.u32(STACK + 4).unwrap(), f(15.0));
    assert_eq!(cpu.x87.instruction_pointer, 0x146a0e);
    assert_eq!(cpu.x87.data_pointer, STACK - 4);
}

#[test]
fn the_airborne_test_needs_every_condition() {
    // 0.25 s at 30 ticks a second: 7.5, rounded to 8 ticks.
    let cases = [
        // (+c1, +348, +399, +3dc, parent, answer)
        (0u8, 1u8, 8u8, 1u8, NONE, 1u32),
        (0, 1, 9, 3, NONE, 1),
        (0, 1, 7, 1, NONE, 0),
        (0, 1, 8, 2, NONE, 0),
        (0, 1, 8, 1, 0x8001_0002, 0),
        (1, 1, 8, 1, NONE, 0),
        (0, 0, 8, 1, NONE, 0),
        (0, 1, 0x80, 1, NONE, 0),
    ];
    for (c1, flags, count, state, parent, answer) in cases {
        let (mut cpu, mut g) = world();
        g.set_u32(0x0045_df20, f(0.25)).unwrap();
        g.set_u8(BODY + 0xc1, c1).unwrap();
        g.set_u8(BODY + 0x348, flags).unwrap();
        g.set_u8(BODY + 0x399, count).unwrap();
        g.set_u8(BODY + 0x3dc, state).unwrap();
        g.set_u32(BODY + 0x14, parent).unwrap();
        cpu.eax = BIPED;
        let next = run(airborne_test, &mut cpu, &mut g, points::BIPED_AIRBORNE_TEST.base);
        assert_eq!(next, Next::Ret { pop: 0 });
        assert_eq!(cpu.eax, answer, "{c1} {flags} {count} {state} {parent:#x}");
        assert_eq!((cpu.edx, cpu.esp), (1, STACK));
    }
}

#[test]
fn an_update_without_a_component_returns_zero_at_once() {
    let (mut cpu, mut g) = world();
    cpu.eax = 0x1234_56ff;
    let out = drive(&mut cpu, &mut g, &TABLE, points::BIPED_UPDATE.base).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(cpu.eax & 0xff, 0);
    assert_eq!(cpu.esp, STACK + 8);
    assert_eq!((cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (0x1111, 0x2222, 0x3333, 0x4444));
    // The definition and header offset were stored before the return.
    let s0 = STACK - FRAME - 12;
    assert_eq!(g.u32(s0 + 0x10).unwrap(), DEF);
    assert_eq!(g.u32(s0 + 0x24).unwrap(), 12);
}

#[test]
fn an_unparented_update_calls_the_movement_steps_in_order() {
    let (mut cpu, mut g) = world();
    // A component; definition `+264` bit 3, so `+150` is not normalised.
    g.set_u32(BODY + 0xb4, 7).unwrap();
    g.set_u32(DEF + 0x264, 1 << 3).unwrap();
    // The record words, the animation kind's and the state update's
    // immediates: test values; the biped's animation name matches none.
    g.set_u32(RECORD_A, 0x0000_1234).unwrap();
    g.set_u32(RECORD_B, 0x0000_5678).unwrap();
    for (at, v) in [(0xdd302, 1000), (0xdd30e, 500), (0xdd318, 100), (0xdd320, 200)] {
        g.set_u32(at, v).unwrap();
    }
    g.set_u32(0xe4c9a, 0x111).unwrap();
    g.set_u32(0xe4ca2, 0x222).unwrap();
    g.set_u32(BODY + 0x1b0, f(1.0)).unwrap();
    g.set_u32(K_45E02C, f(0.01)).unwrap();
    g.set_u8(BODY + 0x399, 5).unwrap();
    g.set_u8(BODY + 0x39a, 6).unwrap();

    // Slice 6c made the unit animation state `111650` a body; without it
    // here, it stays the callout this test answers.
    let table = crate::sim_objects::Table::new(
        crate::sim_objects::BODIES.iter().filter(|b| b.point.base != points::UNIT_ANIMATION_STATE.base),
    )
    .unwrap();
    let s = STACK - FRAME - 16;
    let mut out = drive(&mut cpu, &mut g, &table, points::BIPED_UPDATE.base).unwrap();
    let mut called = Vec::new();
    let answers = [(E01D0, 8, 0x02u32), (DC5C0, 8, 0x40), (X1E5AF0, 4, 0x80), (E24F0, 12, 0x04), (E6830, 4, 0x08), (X111650, 8, 0x10)];
    for (target, cleanup, al) in answers {
        assert_eq!(out.target_pc, target, "after {called:x?}");
        called.push(target);
        match target {
            E01D0 => {
                assert_eq!(g.u32(cpu.esp + 4).unwrap(), BIPED);
                assert_eq!(g.u32(cpu.esp + 8).unwrap(), s + 0x18);
                assert_eq!(g.u32(s + 0x18).unwrap(), 0x1234);
                assert_eq!(g.u32(s + 0x1c).unwrap(), 0x5678);
                assert_eq!(g.u32(s + 0x20).unwrap(), 0);
                assert_eq!(g.u32(s + 0x7c).unwrap(), NONE);
                // The kind of name 0 (the block at `body - 1`, unset) is 2.
                assert_eq!(g.u8(BODY + 0x34a).unwrap(), 2);
                // Neither `+348` bit: both counters cleared.
                assert_eq!((g.u8(BODY + 0x399).unwrap(), g.u8(BODY + 0x39a).unwrap()), (0, 0));
                // Unparented: `esi` is still the carrier's answer.
                assert_eq!(cpu.esi, NONE);
            }
            X1E5AF0 => {
                assert_eq!((cpu.ecx, cpu.edx, cpu.eax), (BODY + 0x7c, BODY + 0x3dc, s + 0x2c));
                assert_eq!(g.u32(cpu.esp + 4).unwrap(), BODY + 0x70);
            }
            E24F0 => {
                assert_eq!(g.u32(cpu.esp + 8).unwrap(), s + 0x18);
                assert_eq!(g.u32(cpu.esp + 12).unwrap(), s + 0x2c);
            }
            E6830 => {
                // The look update found no target; the biped was woken for
                // the post walk with the tick at `+bc`.
                assert_eq!(g.u32(BODY + 0xbc).unwrap(), 0x1234);
                assert_eq!(g.u32(cpu.esp + 4).unwrap(), BIPED);
            }
            X111650 => assert_eq!(g.u8(s + 0x13).unwrap(), 0x08),
            _ => {}
        }
        assert_eq!((cpu.edi, cpu.ebp), (BIPED, BODY));
        let site = returned(&mut cpu, &g, cleanup);
        assert_eq!(site, out.resume_pc);
        cpu.eax = 0xabcd_0000 | al;
        out = drive(&mut cpu, &mut g, &table, site).unwrap();
    }
    assert_eq!(out, TransferAbi::ret(CALLER));
    // `bl`: 0 from the timer, `e01d0` 2, `e24f0` 4, 1 before `b58c0`'s
    // test, then `e6830` 8 and `111650` 16.
    assert_eq!(cpu.eax & 0xff, 0x1f);
    assert_eq!(cpu.esp, STACK + 8);
    assert_eq!((cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (0x1111, 0x2222, 0x3333, 0x4444));
}
