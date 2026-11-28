//! The slice 7b bodies on a synthetic guest: the frame and registers at
//! their callouts and returns, and what they store. Every value is made
//! up for the test (the image constants and the jump table entry they
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
const COMPONENTS: u32 = 0x0060_7000;
const RECORDS: u32 = 0x0060_7100;
const BIPED: u32 = 0x8001_0001;
/// `dd990`'s `esp` at its sites, entered with `esp` at [`STACK`].
const S: u32 = STACK - FRAME - 16;

fn f(v: f32) -> u32 {
    v.to_bits()
}

/// Object slot 1, a biped (type 0, active) with its definition at tag
/// index 0, game time at 30 ticks a second (a quarter-second tick, so
/// the products are exact), and physics component 2. The whole low 8 MiB
/// is mapped, so image addresses (constants, immediates, the jump table,
/// the x87 exits' instruction bytes) read as planted or zero.
fn world() -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(0, 0x0080_0000, Permissions::RW).unwrap();
    g.set_u32(STACK, CALLER).unwrap();
    g.set_u32(STACK + 4, BIPED).unwrap();
    g.set_u32(points::OBJECTS.base, ARRAY).unwrap();
    g.set_u32(ARRAY + 0x44, HEADERS).unwrap();
    for slot in 1..4u32 {
        let h = HEADERS + 12 * slot;
        let body = BODY + 0x500 * (slot - 1);
        g.set_u16(h, 0x8001).unwrap();
        g.set_u8(h + 2, 0x01).unwrap();
        g.set_u32(h + 8, body).unwrap();
        for link in [0x0c, 0x10, 0x14, 0xb4, 0xd4, 0x12c, 0x13c, 0x394] {
            g.set_u32(body + link, NONE).unwrap();
        }
        g.set_u16(body + 0x12a, 0xffff).unwrap();
    }
    g.set_u32(points::TAG_ENTRIES.base, TAGS).unwrap();
    g.set_u32(TAGS + 8, DEF).unwrap();
    g.set_u32(points::GAME_TIME.base, TIME).unwrap();
    g.set_u16(TIME + 2, 30).unwrap();
    g.set_u32(TIME + 4, f(0.25)).unwrap();
    g.set_u32(TIME + 8, 0x1234).unwrap();
    g.set_u32(points::PHYSICS_COMPONENTS.base, COMPONENTS).unwrap();
    g.set_u32(COMPONENTS + 0x44, RECORDS).unwrap();
    g.set_u32(K_45DBC0, f(1.0)).unwrap();
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
fn the_forced_crouch_needs_state_6_the_byte_and_bit_13() {
    for (state, byte3f8, flags, answer) in
        [(6u8, 1u8, 1u16 << 13, 1u8), (5, 1, 1 << 13, 0), (6, 0, 1 << 13, 0), (6, 1, 1 << 12, 0)]
    {
        let (mut cpu, mut g) = world();
        g.set_u8(BODY + 0x3dc, state).unwrap();
        g.set_u8(BODY + 0x3f8, byte3f8).unwrap();
        g.set_u16(BODY + 0x348, flags).unwrap();
        cpu.eax = BIPED;
        assert_eq!(run(crouch_forced, &mut cpu, &mut g, points::BIPED_CROUCH_FORCED.base), Next::Ret { pop: 0 });
        assert_eq!(cpu.eax & 0xff, u32::from(answer), "{state} {byte3f8} {flags:#x}");
        // `al` replaced; above it the scaled index, 3.
        assert_eq!(cpu.eax & !0xff, 0);
        assert_eq!(cpu.esp, STACK);
    }
}

#[test]
fn the_ground_flag_reads_the_surface_only_without_objects() {
    const SURFACE_FLAGS: u32 = 0x0060_8000;
    for (object, surface, second, flag, answer) in [
        (NONE, 3u32, NONE, 1u8, 1u32),
        (NONE, 3, NONE, 2, 0),
        (5, 3, NONE, 1, 0),
        (NONE, NONE, NONE, 1, 0),
        (NONE, 3, 7, 1, 0),
    ] {
        let (mut cpu, mut g) = world();
        g.set_u32(0x004e_0340, 0x0060_7f00).unwrap();
        g.set_u32(0x0060_7f00 + 0x2c, SURFACE_FLAGS).unwrap();
        g.set_u8(SURFACE_FLAGS + 8 * 3 + 4, flag).unwrap();
        g.set_u32(BODY + 0x35c, object).unwrap();
        g.set_u32(BODY + 0x354, surface).unwrap();
        g.set_u32(BODY + 0x360, second).unwrap();
        cpu.eax = BIPED;
        run(ground_flag, &mut cpu, &mut g, points::BIPED_GROUND_FLAG.base);
        assert_eq!(cpu.eax & 0xff, answer, "{object:#x} {surface:#x} {second:#x} {flag}");
        if answer == 1 {
            // The global's value with `al` replaced.
            assert_eq!(cpu.eax, 0x0060_7f01);
        }
    }
}

#[test]
fn a_landing_stores_its_kind_and_ticks() {
    // Bands at 1, 2 and 4; times 0.5 and 1.0 seconds.
    let bands = [(0x1fc, 0.5f32), (0x200, 1.0), (0x204, 1.0), (0x208, 2.0), (0x20c, 4.0)];
    // (+348, speed, kind, product, ticks): bit 6 takes the whole second
    // band's time (`4 / (4 - 2)`, clamped to 1) and is cleared; a speed of
    // 1.5 is half way through the first band, half of 0.5 s (7.5 ticks,
    // rounded to even).
    for (flags, speed, kind, product, ticks) in [(0x41u8, 0.0f32, 1u16, 30.0f32, 30u8), (0x01, 1.5, 0, 7.5, 8)] {
        let (mut cpu, mut g) = world();
        for (off, v) in bands {
            g.set_u32(DEF + off, f(v)).unwrap();
        }
        g.set_u8(BODY + 0x348, flags).unwrap();
        g.set_u16(BODY + 0x34c, 0x7777).unwrap();
        sse::load(&mut cpu, 2, f(speed));
        cpu.eax = BIPED;
        assert_eq!(run(landing_update, &mut cpu, &mut g, points::BIPED_LANDING_UPDATE.base), Next::Ret { pop: 0 });
        assert_eq!(g.u8(BODY + 0x348).unwrap(), 0x01);
        assert_eq!(g.u16(BODY + 0x34c).unwrap(), kind);
        assert_eq!(g.u8(BODY + 0x34e).unwrap(), 0);
        assert_eq!(g.u8(BODY + 0x34f).unwrap(), ticks);
        // The dead store of the bit, and the `fistp` pair.
        let e = STACK - 0xc;
        assert_eq!(g.u8(e + 3).unwrap(), flags >> 6 & 1);
        assert_eq!(g.u32(e + 4).unwrap(), f(product));
        assert_eq!(g.u32(e + 8).unwrap(), u32::from(ticks));
        assert_eq!(cpu.x87.instruction_pointer, 0xe441a);
        assert_eq!(cpu.esp, STACK);
    }

    // Below the first band nothing is stored.
    let (mut cpu, mut g) = world();
    g.set_u32(DEF + 0x204, f(1.0)).unwrap();
    g.set_u16(BODY + 0x34c, 0x7777).unwrap();
    sse::load(&mut cpu, 2, f(0.5));
    cpu.eax = BIPED;
    run(landing_update, &mut cpu, &mut g, points::BIPED_LANDING_UPDATE.base);
    assert_eq!(g.u16(BODY + 0x34c).unwrap(), 0x7777);
}

#[test]
fn the_crouch_step_moves_toward_the_control_and_restores_its_registers() {
    // A rate of 2 a second over a quarter-second tick: a step of 0.5.
    for (control, fraction, step) in [(true, 0.25f32, 0.5f32), (true, 0.75, 0.25), (false, 0.25, -0.25), (false, 0.75, -0.5)] {
        let (mut cpu, mut g) = world();
        g.set_u32(DEF + 0x250, f(2.0)).unwrap();
        g.set_u32(BODY + 0x134, u32::from(control) << 23).unwrap();
        g.set_u32(BODY + 0x2c4, f(fraction)).unwrap();
        cpu.eax = BIPED;
        cpu.ecx = 0x5555;
        let out = drive(&mut cpu, &mut g, &TABLE, points::BIPED_CROUCH_STEP.base).unwrap();
        assert_eq!(out, TransferAbi::ret(CALLER));
        assert_eq!(sse::lane0(&cpu, 0), f(step), "{control} {fraction}");
        // `push ecx` made the local the rate is stored over, so `pop ecx`
        // returns the rate, not the caller's `ecx`; `esi` is restored.
        assert_eq!((cpu.ecx, cpu.esi, cpu.esp), (f(2.0), 0x3333, STACK + 4));
    }
}

#[test]
fn a_post_update_without_a_component_returns_at_once() {
    let (mut cpu, mut g) = world();
    g.set_u8(BODY + 0x348, 0xff).unwrap();
    let out = drive(&mut cpu, &mut g, &TABLE, points::BIPED_POST_UPDATE.base).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(g.u8(BODY + 0x348).unwrap(), 0x7f);
    assert_eq!(cpu.esp, STACK + 8);
    assert_eq!((cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (0x1111, 0x2222, 0x3333, 0x4444));
}

/// A biped with component 2, attached to an early mover (`+c1` bit 0, so
/// `1ec500` is skipped), a crouch fraction of 0.25 that a quarter-second
/// step brings to 0, the crouch height off (definition `+264` bit 1), the
/// probe off (`+134` bit 9) and no jump.
fn settled() -> (CpuAbi, Guest) {
    let (cpu, mut g) = world();
    g.set_u32(BODY + 0xb4, 2).unwrap();
    g.set_u8(BODY + 0xc1, 1).unwrap();
    g.set_u32(DEF + 0x250, f(2.0)).unwrap();
    g.set_u32(DEF + 0x264, 2).unwrap();
    g.set_u32(BODY + 0x134, 1 << 9).unwrap();
    g.set_u32(BODY + 0x2c4, f(0.25)).unwrap();
    g.set_u8(BODY + 0x3dc, 1).unwrap();
    g.set_u8(BODY + 0x349, 0x18).unwrap();
    (cpu, g)
}

#[test]
fn a_settled_biped_ends_after_the_component_test() {
    let (mut cpu, mut g) = settled();
    g.set_u8(BODY + 0xc0, 0x40).unwrap();
    g.set_u8(BODY + 0x348, 0x80).unwrap();
    let out = drive(&mut cpu, &mut g, &TABLE, points::BIPED_POST_UPDATE.base).unwrap();
    assert_eq!(out, TransferAbi::call(X1D1C10, 0xddb70));
    assert_eq!(cpu.esi, RECORDS + 2 * 0xa0);
    assert_eq!((cpu.ebp, cpu.edi), (BODY, BIPED));
    assert_eq!(g.u32(S + 0x10).unwrap(), RECORDS + 2 * 0xa0);
    // The fraction stepped and clamped to 0; `+349` bit 3 cleared.
    assert_eq!(g.u32(BODY + 0x2c4).unwrap(), 0);
    assert_eq!(g.u8(BODY + 0x349).unwrap(), 0x10);
    // `[S+18]`: the step (-0.25) with its low byte the jump answer, 0.
    assert_eq!(g.u32(S + 0x18).unwrap(), f(-0.25) & !0xff);
    let site = returned(&mut cpu, &g, 0);
    cpu.eax = 0xabcd_0000;
    let out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    // `+c0` bit 6 ends it: bit 7 of `+348` cleared, `ebx` popped too.
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(g.u8(BODY + 0x348).unwrap(), 0);
    assert_eq!(cpu.esp, STACK + 8);
    assert_eq!((cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (0x1111, 0x2222, 0x3333, 0x4444));
}

#[test]
fn an_unparented_biped_runs_the_physics_input_and_takes_the_output() {
    let (mut cpu, mut g) = settled();
    // `+c0` bit 6 (unparented: the input runs); bit 8 (`+c1` bit 0)
    // clear, so `1ec500` runs (answering no: no damage packet) and the
    // contact words are cleared.
    g.set_u16(BODY + 0xc0, 0x40).unwrap();
    g.set_u32(BODY + 0x354, 9).unwrap();
    g.set_u32(BODY + 0x408, 0x0bad_f00d).unwrap();
    for (i, v) in [1.0f32, 2.0, 3.0].into_iter().enumerate() {
        g.set_u32(BODY + 0x64 + 4 * i as u32, f(v)).unwrap();
    }
    // State 1's entry in the jump table.
    g.set_u32(0xdd2e4, 0xdd246).unwrap();
    let mut out = drive(&mut cpu, &mut g, &TABLE, points::BIPED_POST_UPDATE.base).unwrap();
    let record = S + 0x40;
    let answers = [(X1EC500, 0, 0u32), (X1D1C10, 0, 1), (0x000b_a160, 4, 0), (0x001e_68a0, 0x28, 0), (X1E5A60, 0, 0)];
    for (target, cleanup, al) in answers {
        assert_eq!(out.target_pc, target);
        match target {
            X1EC500 => assert_eq!(cpu.edi, BIPED),
            0x000b_a160 => assert_eq!(cpu.eax, BIPED),
            0x001e_68a0 => {
                // `eax` the record, `ecx` the state, and the four
                // arguments pushed last: the definition's `+264`, the
                // component, the object's slot (none: no early mover),
                // the landing test's answer.
                assert_eq!((cpu.eax, cpu.ecx), (record, BODY + 0x3dc));
                assert_eq!(g.u32(cpu.esp + 4).unwrap(), DEF + 0x264);
                assert_eq!(g.u32(cpu.esp + 8).unwrap(), 2);
                assert_eq!(g.u32(cpu.esp + 12).unwrap(), NONE);
                assert_eq!(g.u32(cpu.esp + 16).unwrap(), 0);
            }
            X1E5A60 => {
                assert_eq!((cpu.eax, cpu.ecx, cpu.ebx), (S + 0x40, S + 0x1c, BODY + 0x3dc));
                // The output: bit 0, no contact index, no object.
                g.set_u32(S + 0x20, 1).unwrap();
                g.set_u32(S + 0x24, NONE).unwrap();
                g.set_u32(S + 0x28, NONE).unwrap();
            }
            _ => {}
        }
        let site = returned(&mut cpu, &g, cleanup);
        assert_eq!(site, out.resume_pc);
        cpu.eax = 0xabcd_0000 | al;
        out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    }
    assert_eq!(out, TransferAbi::ret(CALLER));
    // State 1: the record's `+14` set and `+44` from body `+408`.
    assert_eq!(g.u8(record + 0x14).unwrap(), 1);
    assert_eq!(g.u32(record + 0x44).unwrap(), 0x0bad_f00d);
    // The position into `+368`, the contact cleared, `+348` bit 0 from the
    // output, `+349` bit 4 cleared.
    assert_eq!(g.u32(BODY + 0x368).unwrap(), f(1.0));
    assert_eq!(g.u32(BODY + 0x370).unwrap(), f(3.0));
    for off in [0x354, 0x358, 0x35c, 0x360, 0x364] {
        assert_eq!(g.u32(BODY + off).unwrap(), NONE, "{off:#x}");
    }
    assert_eq!(g.u8(BODY + 0x348).unwrap(), 1);
    assert_eq!(g.u8(BODY + 0x349).unwrap(), 0);
    assert_eq!(cpu.esp, STACK + 8);
    assert_eq!((cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (0x1111, 0x2222, 0x3333, 0x4444));
}

#[test]
fn the_contact_counter_restarts_on_a_new_contact() {
    let (mut cpu, mut g) = world();
    g.set_u32(STACK + 4, 0).unwrap();
    g.set_u32(BODY + 0x13c, 0x8001_0003).unwrap();
    g.set_u32(BODY + 0x394, 0x8001_0002).unwrap();
    g.set_u8(BODY + 0x398, 5).unwrap();
    // The typed lookup `badc0` runs in the table: slot 3 is live below
    // the high-water mark, so the contact's body answers and `1c9c00`
    // follows.
    g.set_u32(ARRAY + 0x24, 12).unwrap();
    g.set_u32(ARRAY + 0x38, 4).unwrap();
    cpu.eax = BIPED;
    cpu.ebx = 0x8001_0003;
    let out = drive(&mut cpu, &mut g, &TABLE, points::BIPED_CONTACT_UPDATE.base).unwrap();
    assert_eq!(out, TransferAbi::call(0x001c_9c00, 0xe3033));
    assert_eq!(cpu.eax, 0x8001_0003);
    let site = returned(&mut cpu, &g, 0);
    let out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    // A target: no `1fb6f0`; the new contact is stored, its counter 0.
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(g.u32(BODY + 0x394).unwrap(), 0x8001_0003);
    assert_eq!(g.u8(BODY + 0x398).unwrap(), 0);
    assert_eq!(cpu.esp, STACK + 8);
    assert_eq!((cpu.ebx, cpu.esi, cpu.edi), (0x8001_0003, 0x3333, 0x4444));

    // A negative counter with no contact counts up and calls nothing.
    let (mut cpu, mut g) = world();
    g.set_u8(BODY + 0x398, 0xfe).unwrap();
    cpu.eax = BIPED;
    cpu.ebx = NONE;
    let out = drive(&mut cpu, &mut g, &TABLE, points::BIPED_CONTACT_UPDATE.base).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(g.u8(BODY + 0x398).unwrap(), 0xff);
}
