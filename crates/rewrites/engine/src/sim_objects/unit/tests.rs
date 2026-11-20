//! The slice 6a bodies on a synthetic guest: the frame and registers at
//! their callouts and returns, and what they store. Every value is made
//! up for the test (the image constants they read are planted with test
//! values); no game data.

use super::*;
use crate::sim_objects::{TABLE, drive, frame};
use runtime::TransferAbi;
use runtime::guest::Permissions;

const STACK: u32 = 0x8000;
const CALLER: u32 = 0xdead;
const ARRAY: u32 = 0x0060_0000;
const HEADERS: u32 = 0x0060_1000;
const BODY: u32 = 0x0060_2000;
const TAGS: u32 = 0x0060_4000;
const DEF: u32 = 0x0060_5000;
const TIME: u32 = 0x0060_6000;
const ZERO_VECTOR: u32 = 0x0060_7000;
const DIRECTION: u32 = 0x0060_8000;
const UNIT: u32 = 0x8001_0001;
/// The unit's limits record, `[body+33e]` bytes into the body.
const LIMITS_AT: u16 = 0x400;
const LIMITS: u32 = BODY + LIMITS_AT as u32;

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

/// Object slot 1, a unit (type 0, active, unparented, childless) with a
/// definition at tag index 0, game time at 2 ticks a second, and the image
/// constants the bodies read, with values chosen for the test.
fn world() -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(STACK - 0x1000, 0x2000, Permissions::RW).unwrap();
    g.map_zeroed(0x0045_d000, 0x2000, Permissions::RW).unwrap();
    g.map_zeroed(0x0046_8000, 0x1000, Permissions::RW).unwrap();
    // The globals, game time among them.
    g.map_zeroed(0x004e_0000, 0x4_0000, Permissions::RW).unwrap();
    // Zeroed code: the x87 exits read their instruction's bytes for FOP.
    g.map_zeroed(0x000c_0000, 0x1_0000, Permissions::RW).unwrap();
    g.map_zeroed(ARRAY, 0x1_0000, Permissions::RW).unwrap();
    g.set_u32(STACK, CALLER).unwrap();
    g.set_u32(STACK + 4, UNIT).unwrap();
    g.set_u32(points::OBJECTS.base, ARRAY).unwrap();
    g.set_u32(ARRAY + 0x44, HEADERS).unwrap();
    g.set_u16(HEADERS + 12, (UNIT >> 16) as u16).unwrap();
    g.set_u8(HEADERS + 12 + 2, 0x01).unwrap();
    g.set_u32(HEADERS + 12 + 8, BODY).unwrap();
    for link in [0x0c, 0x10, 0x14] {
        g.set_u32(BODY + link, NONE).unwrap();
    }
    g.set_u16(BODY + 0x33e, LIMITS_AT).unwrap();
    g.set_u32(points::TAG_ENTRIES.base, TAGS).unwrap();
    g.set_u32(TAGS + 8, DEF).unwrap();
    g.set_u32(points::GAME_TIME.base, TIME).unwrap();
    g.set_u32(TIME + 4, f(0.5)).unwrap();
    g.set_u32(ONE, f(1.0)).unwrap();
    g.set_u32(BYTE_SCALE, f(100.0)).unwrap();
    g.set_u32(SMALL, f(0.001)).unwrap();
    g.set_u32(THRESHOLD, 0).unwrap();
    g.set_u32(VECTOR_A4, ZERO_VECTOR).unwrap();
    g.set_u32(VECTOR_88, ZERO_VECTOR).unwrap();
    g.set_u32(VECTOR_B0, ZERO_VECTOR).unwrap();
    let cpu = CpuAbi { esp: STACK, ebx: 0x1111, ebp: 0x2222, esi: 0x3333, edi: 0x4444, ..Default::default() };
    (cpu, g)
}

/// Return from a callout that pops `cleanup` argument bytes.
fn returned(cpu: &mut CpuAbi, g: &Guest, cleanup: u32) -> u32 {
    let site = frame::pop(cpu, g).unwrap();
    cpu.esp += cleanup;
    site
}

/// What `_ftol2` does to the x87 stack: `st(0)` popped.
fn ftol_pop(cpu: &mut CpuAbi) {
    let top = (cpu.x87.status >> 11) & 7;
    cpu.x87.tag |= 3 << (2 * top);
    cpu.x87.status = (cpu.x87.status & !0x3800) | (((top + 1) & 7) << 11);
}

fn x87_empty(cpu: &CpuAbi) -> bool {
    cpu.x87.tag == 0xffff
}

#[test]
fn the_shell_ors_the_first_five_answers_and_restores_its_registers() {
    let (mut cpu, mut g) = world();
    // The movement returns 0 at once: definition `+bc` bit 10.
    g.set_u32(DEF + 0xbc, 1 << 10).unwrap();
    let mut out = drive(&mut cpu, &mut g, &TABLE, points::UNIT_UPDATE.base).unwrap();
    let mut answers = [0x01u32, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80].into_iter();
    let mut called = Vec::new();
    loop {
        if out.kind == TransferAbi::ret(0).kind {
            break;
        }
        called.push(out.target_pc);
        // Stack-argument callees see the unit at `[esp+4]`; the others in
        // `eax`. `esi` holds it throughout.
        let stack_arg = [callee::C49B0, callee::C58F0].contains(&out.target_pc);
        if stack_arg {
            assert_eq!(g.u32(cpu.esp + 4).unwrap(), UNIT);
        } else {
            assert_eq!(cpu.eax, UNIT, "{:#x}", out.target_pc);
        }
        assert_eq!(cpu.esi, UNIT);
        let site = returned(&mut cpu, &g, if stack_arg { 4 } else { 0 });
        assert_eq!(site, out.resume_pc);
        cpu.eax = 0xabcd_0000 | answers.next().unwrap();
        out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    }
    assert_eq!(
        called,
        [
            callee::C49B0,
            callee::C58F0,
            callee::C5EB0,
            callee::C6740,
            callee::DIALOGUE,
            callee::C6990,
            callee::C6810,
            callee::C50A0,
        ]
    );
    assert_eq!(out, TransferAbi::ret(CALLER));
    // c49b0 1, c58f0 2, the movement 0, c5eb0 4, c6740 8; the rest ignored.
    assert_eq!(cpu.eax & 0xff, 0x0f);
    assert_eq!(cpu.esp, STACK + 8);
    assert_eq!((cpu.ebx, cpu.esi), (0x1111, 0x3333));
}

#[test]
fn movement_without_rates_takes_the_desired_directions_and_wakes_the_unit() {
    let (mut cpu, mut g) = world();
    vec3(&mut g, BODY + 0x15c, [1.0, 0.0, 0.0]);
    vec3(&mut g, BODY + 0x168, [0.0, 1.0, 0.0]);
    vec3(&mut g, BODY + 0x174, [9.0, 9.0, 9.0]);
    vec3(&mut g, BODY + 0x180, [0.0, 0.0, 1.0]);
    vec3(&mut g, BODY + 0x18c, [0.0, 0.0, 1.0]);
    vec3(&mut g, BODY + 0x198, [9.0, 9.0, 9.0]);
    // Neither limits bit: the clamp returns at once.
    g.set_u8(LIMITS, 0).unwrap();

    let out = drive(&mut cpu, &mut g, &TABLE, points::UNIT_MOVEMENT.base).unwrap();
    assert_eq!(out, TransferAbi::call(callee::C4970, 0xc61eb));
    assert_eq!(cpu.ecx, UNIT, "the unit in ecx");
    assert_eq!(cpu.ebp, BODY);
    assert_eq!(cpu.ebx, BODY + 0x168);
    // sub esp,7ch; four pushes; the return address.
    assert_eq!(cpu.esp, STACK - 0x7c - 16 - 4);
    // The desired aim is copied before the call.
    assert_eq!(f32_at(&g, BODY + 0x168), 1.0);
    let site = returned(&mut cpu, &g, 0);
    cpu.eax = 0;

    // No clamp: the aim velocity from the zero vector; rates zero, so both
    // fractions are 0 and `_ftol2` gets `0 * 100` on the x87.
    let out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::call(callee::FTOL, 0xc645f));
    assert_eq!(f32_at(&g, BODY + 0x174), 0.0);
    assert!(!x87_empty(&cpu), "st(0) holds the scaled fraction");
    let site = returned(&mut cpu, &g, 0);
    ftol_pop(&mut cpu);
    cpu.eax = 0x0000_0011;
    let out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::call(callee::FTOL, 0xc6474));
    assert_eq!(g.u8(BODY + 0x243).unwrap(), 0x11);
    let site = returned(&mut cpu, &g, 0);
    ftol_pop(&mut cpu);
    cpu.eax = 0x0000_0022;

    // The look: copied, the clamp (a body) runs and answers 0, the look
    // velocity from the zero vector; the aim moved, so the unit is woken
    // for the post walk (a body too) and the result is 1.
    let out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(g.u8(BODY + 0x1f8).unwrap(), 0x22);
    assert_eq!(f32_at(&g, BODY + 0x18c + 8), 1.0);
    assert_eq!(f32_at(&g, BODY + 0x198), 0.0);
    assert_eq!(cpu.eax & 0xff, 1);
    assert_eq!(g.u8(HEADERS + 12 + 2).unwrap() & 0x06, 0x06, "AWAKE and POST");
    assert_eq!(cpu.esp, STACK + 8);
    assert_eq!((cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (0x1111, 0x2222, 0x3333, 0x4444));
    assert!(x87_empty(&cpu), "the moved tests pop what they load");
}

#[test]
fn movement_that_moves_nothing_returns_zero_without_waking() {
    let (mut cpu, mut g) = world();
    for at in [0x15c, 0x168] {
        vec3(&mut g, BODY + at, [1.0, 0.0, 0.0]);
    }
    for at in [0x180, 0x18c] {
        vec3(&mut g, BODY + at, [0.0, 1.0, 0.0]);
    }
    let mut out = drive(&mut cpu, &mut g, &TABLE, points::UNIT_MOVEMENT.base).unwrap();
    while out.kind != TransferAbi::ret(0).kind {
        let site = returned(&mut cpu, &g, 0);
        if out.target_pc == callee::FTOL {
            ftol_pop(&mut cpu);
        }
        cpu.eax = 0;
        out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    }
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(cpu.eax & 0xff, 0);
    assert_eq!(g.u8(HEADERS + 12 + 2).unwrap(), 0x01, "not woken");
    assert_eq!((cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (0x1111, 0x2222, 0x3333, 0x4444));
}

#[test]
fn the_clamp_leaves_an_unflagged_direction_alone() {
    let (mut cpu, mut g) = world();
    g.set_u32(STACK + 4, DIRECTION).unwrap();
    g.set_u32(STACK + 8, 0).unwrap();
    cpu.eax = UNIT;
    // The look's bit (6) is set, the aim's (5) is not.
    g.set_u8(LIMITS, 1 << 6).unwrap();
    let out = drive(&mut cpu, &mut g, &TABLE, points::UNIT_CLAMP_DIRECTION.base).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(cpu.esp, STACK + 12);
    assert_eq!(cpu.eax & 0xff, 0);
    assert_eq!((cpu.ebx, cpu.ebp), (0x1111, 0x2222));
}

#[test]
fn the_clamp_turns_a_direction_past_its_yaw_limit_back_to_the_limit() {
    let (mut cpu, mut g) = world();
    g.set_u32(STACK + 4, DIRECTION).unwrap();
    g.set_u32(STACK + 8, 0).unwrap();
    cpu.eax = UNIT;
    g.set_u8(LIMITS, 1 << 5).unwrap();
    // Yaw from -0.5 to 0.5, pitch from -1 to 1.
    vec3(&mut g, LIMITS + 4, [-0.5, 0.5, -1.0]);
    g.set_u32(LIMITS + 0x10, f(1.0)).unwrap();
    // Straight along +y: a yaw of pi/2.
    vec3(&mut g, DIRECTION, [0.0, 1.0, 0.0]);
    // The unit's forward and up, which the frame `b9fc0` copies out.
    vec3(&mut g, BODY + 0x70, [1.0, 0.0, 0.0]);
    vec3(&mut g, BODY + 0x7c, [0.0, 0.0, 1.0]);

    // The frame `b9fc0` is slice 4's `OBJECT_WORLD_BASIS` body, so the
    // dispatcher fuses it, and the aim's clamp goes straight on to the
    // matrix.
    let out = drive(&mut cpu, &mut g, &TABLE, points::UNIT_CLAMP_DIRECTION.base).unwrap();
    assert_eq!(out, TransferAbi::call(callee::MATRIX, 0xcbb99));
    let e = cpu.esp + 4 + 0x10;
    assert_eq!((cpu.eax, cpu.ecx, cpu.edx, cpu.esi), (e + 4, e + 0x10, e + 0x1c, ZERO_VECTOR));
    let forward = [f32_at(&g, e + 0x10), f32_at(&g, e + 0x14), f32_at(&g, e + 0x18)];
    let up = [f32_at(&g, e + 4), f32_at(&g, e + 8), f32_at(&g, e + 0xc)];
    assert_eq!((forward, up), ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]), "the frame, from the body");
    // The identity: forward x, left y, up z.
    let m = cpu.edx;
    g.set_u32(m, f(1.0)).unwrap();
    vec3(&mut g, m + 4, [1.0, 0.0, 0.0]);
    vec3(&mut g, m + 0x10, [0.0, 1.0, 0.0]);
    vec3(&mut g, m + 0x1c, [0.0, 0.0, 1.0]);
    let site = returned(&mut cpu, &g, 0);

    let out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(cpu.esp, STACK + 12);
    assert_eq!(cpu.eax & 0xff, 1, "clamped");
    let d = [f32_at(&g, DIRECTION), f32_at(&g, DIRECTION + 4), f32_at(&g, DIRECTION + 8)];
    assert!((d[0] - 0.5f32.cos()).abs() < 1e-6, "{d:?}");
    assert!((d[1] - 0.5f32.sin()).abs() < 1e-6, "{d:?}");
    assert!(d[2].abs() < 1e-6, "{d:?}");
    assert_eq!((cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (0x1111, 0x2222, 0x3333, 0x4444));
    assert!(x87_empty(&cpu));
    assert_eq!(cpu.x87.data_pointer, DIRECTION + 8, "the last x87 store");
}

/// Run the idle test to its return, answering each state test with
/// `states[k]` and each weapon test with `weapon`. The calls made.
fn idle_run(g: &mut Guest, cpu: &mut CpuAbi, states: &[u8], weapon: u8) -> Vec<(u32, u32, u32)> {
    let mut calls = Vec::new();
    let mut out = drive(cpu, g, &TABLE, points::UNIT_IDLE_TEST.base).unwrap();
    while out.kind != TransferAbi::ret(0).kind {
        calls.push((out.target_pc, cpu.eax, if out.target_pc == callee::STATE { cpu.ecx } else { cpu.ebx }));
        let site = returned(cpu, g, 0);
        let al = if out.target_pc == callee::STATE { states[calls.len() - 1] } else { weapon };
        cpu.eax = 0x5500 | u32::from(al);
        out = drive(cpu, g, &TABLE, site).unwrap();
    }
    assert_eq!(out, TransferAbi::ret(CALLER));
    calls
}

#[test]
fn the_idle_test_asks_seven_states_then_the_held_weapons() {
    let (mut cpu, mut g) = world();
    cpu.eax = UNIT;
    cpu.ecx = 0x7766_5544;
    // One held weapon, in the second slot.
    g.set_u8(BODY + 0x212, 0xff).unwrap();
    g.set_u8(BODY + 0x213, 1).unwrap();
    g.set_u32(BODY + 0x218 + 4, 0xe200_0002).unwrap();
    let calls = idle_run(&mut g, &mut cpu, &[0; 7], 0);
    let states: Vec<_> = calls[..7].iter().map(|c| (c.0, c.1, c.2)).collect();
    assert_eq!(
        states,
        [0x16, 8, 0x12, 0, 1, 0xa, 0xb].map(|n| (callee::STATE, UNIT, n)).to_vec()
    );
    assert_eq!(calls[7], (callee::WEAPON, 0xe200_0002, NONE));
    assert_eq!(calls.len(), 8);
    assert_eq!(cpu.eax & 0xff, 1);
    // The saved `ecx` comes back with the result in its top byte.
    assert_eq!(cpu.ecx, 0x0166_5544);
    assert_eq!(cpu.esp, STACK + 4);
    assert_eq!((cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (0x1111, 0x2222, 0x3333, 0x4444));

    // A state that answers stops it with 0.
    let (mut cpu, mut g) = world();
    cpu.eax = UNIT;
    let calls = idle_run(&mut g, &mut cpu, &[0, 0, 1, 0, 0, 0, 0], 0);
    assert_eq!(calls.len(), 3);
    assert_eq!(cpu.eax & 0xff, 0);

    // A held weapon that answers too.
    let (mut cpu, mut g) = world();
    cpu.eax = UNIT;
    g.set_u8(BODY + 0x212, 0).unwrap();
    g.set_u8(BODY + 0x213, 0xff).unwrap();
    g.set_u32(BODY + 0x218, 0xe200_0003).unwrap();
    let calls = idle_run(&mut g, &mut cpu, &[0; 7], 1);
    assert_eq!(calls.len(), 8);
    assert_eq!(cpu.eax & 0xff, 0);
    assert_eq!((cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (0x1111, 0x2222, 0x3333, 0x4444));

    // A parent: no calls at all.
    let (mut cpu, mut g) = world();
    cpu.eax = UNIT;
    g.set_u32(BODY + 0x14, 0x8002_0002).unwrap();
    assert!(idle_run(&mut g, &mut cpu, &[], 0).is_empty());
    assert_eq!(cpu.eax & 0xff, 0);
}

#[test]
fn a_pc_a_body_does_not_own_is_a_fault() {
    let (mut cpu, mut g) = world();
    let mut cx = Cx { cpu: &mut cpu, g: &mut g };
    assert!(movement(&mut cx, 0xc6734).is_err(), "an exit, not a site");
    assert!(clamp(&mut cx, 0xcbd39).is_err());
    assert!(idle(&mut cx, 0xc86c5).is_err());
    assert!(aim(&mut cx, 0xc7841).is_err());
    assert!(shell(&mut cx, 0xc6b31).is_err());
}
