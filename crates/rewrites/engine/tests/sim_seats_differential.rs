//! The seats' differential harness (docs/engine/sim-rewrite.md, phase 7):
//! every seat body runs once against the lifted original from the same
//! synthetic state — the same registers, the same float environment and
//! the same guest memory — and the two must agree at every call and
//! return: the transfer, the registers, and every byte of RAM.
//!
//! Callees are answered by seeded stubs: a return value in `eax` (and,
//! for the zoom level, `xmm0`), a stack cleanup the stub table knows,
//! and no memory writes, so both sides re-read the state the fixture
//! left. Values the stubs answer vary with the boundary and the case, so
//! the branches a seat takes differ per case.
//!
//! The seats covered here are the straight-line ones: the look curve, the
//! unit control packet, the control update (its unbound entry — the
//! phantom seats' path — and its tail from the target update's return,
//! with the frame the original built), the action and neutral control
//! builders, the target comparator, the aim-assist cone, the HUD seat
//! update and the observer resource walk. The interactive search family
//! (`1a4900`, `1a5370`, `1a4c00`) is proven on the disc by the shadow's
//! guest arm (`scripts/shadow-gate.sh`'s `sim.seats` group), which is the
//! phase's parity stage. The observer camera's oracle is
//! `sim_physics_differential::observer_camera`.
//!
//! Needs the user's XBE (`lifted::image`); skipped without it.
#![cfg(feature = "differential")]

use engine::points;
use engine::sim_seats::{self, Seat};
use ir::interp::{Cpu, FlatMemory, Flow};
use lifted::Code;
use lifted::cpu::from_interp as abi_cpu;
use runtime::{Guest, TransferAbi, abi, guest::Permissions};
use std::collections::BTreeSet;

const RAM: usize = 8 << 20;
const STACK: u32 = 0x007f_0000;
const EXIT: u32 = 0xffff_fffc;
const PHASE: u32 = 0x0060_0000;
const HEADER: u32 = 0x0060_4000;
const DATA: u32 = 0x0060_5000;
const RECORDS: u32 = 0x0061_0000;
const SETTINGS: u32 = 0x004e_034c;
const CONTROL_TABLE: u32 = 0x004e_d284;
const HUD_SEATS: u32 = 0x0051_0c4c;

/// Every callee the covered seats reach, with the stack the real one
/// leaves: the return's cleanup, or a fastcall's nothing.
fn cleanup(target: u32) -> u32 {
    match target {
        0x000c_6ef0 | 0x0010_1090 | 0x0010_11d0 | 0x0010_1160 | 0x0014_ddc0
        | 0x0014_de70 | 0x0018_7b30 | 0x0012_cc10 | 0x0012_dcb0 | 0x0016_e770
        | 0x0016_e890 | 0x0016_e8f0 | 0x000c_6de0 | 0x000b_67a0 | 0x001d_08e0
        | 0x0011_df30 | 0x000e_4050 | 0x0010_0f70 => 0,
        0x0020_03dc | 0x0013_992a | 0x0003_bcb0 | 0x0016_ea60 | 0x0016_e7b0
        | 0x0016_e820 | 0x0016_e950 | 0x000c_b810 => 4,
        0x0013_a720 | 0x0013_aa27 => 8,
        0x0015_0400 => 12,
        _ => panic!("unexpected callee {target:#x}"),
    }
}

/// One stub answer: the value in `eax`, the zoom level in `xmm0` (the
/// zoom call answers there), and the state to continue at.
struct Answer {
    eax: u32,
    zoom: Option<f32>,
}

fn answer(target: u32, boundary: u32, case: u32) -> Answer {
    let value = target.wrapping_mul(0x9127).rotate_left(boundary % 32) ^ case;
    Answer {
        eax: match target {
            // The user test answers a user, or none.
            0x0014_ddc0 => u32::from(case % 2 == 0) * (value & 0xffff),
            // The user's player: index 2 in the fixture's players array.
            0x0014_de70 => 2,
            _ => value,
        },
        zoom: (target == 0x0010_1090).then(|| match case {
            0 => 1.0,
            1 => 2.0,
            _ => 1.0 + case as f32 * 0.25,
        }),
    }
}

fn word(m: &mut FlatMemory, a: u32, v: u32) {
    assert!(m.load(a, &v.to_le_bytes()));
}

fn float(m: &mut FlatMemory, a: u32, v: f32) {
    word(m, a, v.to_bits());
}

/// The image's sections from zero, cut to the fixtures' RAM.
fn image() -> Option<(&'static lifted::image::Image, &'static FlatMemory)> {
    static RAM_IMAGE: std::sync::OnceLock<Option<FlatMemory>> = std::sync::OnceLock::new();
    let image = lifted::image::Image::get()?;
    let ram = RAM_IMAGE.get_or_init(|| {
        let mut ram = image.ram.clone();
        ram.bytes.truncate(RAM);
        Some(ram)
    });
    Some((image, ram.as_ref()?))
}

/// The lifted original for one seat's test.
fn original(image: &lifted::image::Image, point: &points::Point) -> lifted::Original {
    let mut code = image.original();
    code.prime(point.base);
    assert!(code.insn(point.base).is_some(), "{}", point.name);
    code
}

/// Step the original to its next call out or return.
fn segment(code: &mut lifted::Original, cpu: &mut Cpu, mem: &mut FlatMemory) -> TransferAbi {
    for _ in 0..500_000 {
        let flow = lifted::code::step(code, cpu, mem).unwrap_or_else(|e| panic!("{e}"));
        match flow {
            Flow::Return { target } => return TransferAbi::ret(target),
            Flow::Call { target, continuation } => return TransferAbi::call(target, continuation),
            _ => {}
        }
    }
    panic!("did not reach a call or return");
}

type Prepare = fn(&mut FlatMemory, &mut Cpu, u32);

fn find_seat(name: &str) -> &'static Seat {
    sim_seats::SEATS.iter().find(|s| s.name == name).expect("seat")
}

/// One seat over its cases: the fixture builds the state, the body and
/// the original each run on their own copy, and every boundary must
/// agree. Returns the resume sites reached, for the coverage check.
fn matches_guest(name: &str, cases: u32, prepare: Prepare) -> BTreeSet<u32> {
    let mut seen = BTreeSet::new();
    let Some((image, ram)) = image() else { return seen };
    let seat = find_seat(name);
    let mut code = original(image, seat.point);
    for case in 0..cases {
        let mut memory = ram.clone();
        word(&mut memory, STACK, EXIT);
        let mut cpu = Cpu {
            gpr: [0x1234_5678, 0x9876_5432, 0xfe3420, 0x7b1c, STACK, 0xaabbcc, 0x5151, 0xe37],
            eip: seat.point.base,
            eflags: 0x2, // no direction flag: the control bodies require it.
            ..Default::default()
        };
        // Distinguish MOVSS from memory (clears high lanes) and arithmetic
        // on a register (retains them). Input scalar lanes start at zero.
        for (i, reg) in cpu.xmm.iter_mut().enumerate() {
            *reg = (0x1234_u128 + i as u128) << 100;
        }
        cpu.x87.control = [0x007f, 0x027f, 0x037f][case as usize % 3];
        cpu.mxcsr |= (case % 4) << 13;
        prepare(&mut memory, &mut cpu, case);
        compare(seat, &mut code, memory, cpu, case, &mut seen);
    }
    seen
}

fn compare(
    seat: &Seat,
    code: &mut lifted::Original,
    mut mem: FlatMemory,
    mut cpu: Cpu,
    case: u32,
    seen: &mut BTreeSet<u32>,
) {
    let mut native = Guest::new();
    native.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
    native.write_bytes(0, &mem.bytes, 1).unwrap();
    let start = native.mappings()[0].physical_start as usize;
    let mut regs = abi_cpu(&cpu);
    let run = seat.run;
    for boundary in 0..1024 {
        let at = cpu.eip;
        let actual = run(&mut regs, &mut native, at)
            .unwrap()
            .unwrap_or_else(|| panic!("{} case {case} declined at {at:#x}", seat.name));
        if at != seat.point.base {
            seen.insert(at);
        }
        assert_ne!(actual.route, abi::RESUME, "{} case {case} escaped to generated code", seat.name);
        let expected = segment(code, &mut cpu, &mut mem);
        let why = || format!("{} case {case} boundary {boundary} from {at:#x}", seat.name);
        assert_eq!(actual, expected, "{}", why());
        regs.eip = actual.target_pc;
        assert_eq!(regs, abi_cpu(&cpu), "{}", why());
        let ram = &native.physical_ram()[start..start + RAM];
        if mem.bytes != ram {
            let first = mem.bytes.iter().zip(ram).position(|(a, b)| a != b).unwrap();
            panic!("{} first different byte at {first:#x}", why());
        }
        if actual.kind == abi::RETURN {
            assert_eq!(actual.target_pc, EXIT);
            return;
        }
        assert!(seat.sites.contains(&actual.resume_pc), "unregistered return in {}", why());
        let target = expected.target_pc;
        let stub = answer(target, boundary, case);
        cpu.gpr[0] = stub.eax;
        if let Some(zoom) = stub.zoom {
            cpu.xmm[0] = u128::from(zoom.to_bits());
        }
        // The direction and angle callees answer in memory, at the out
        // pointers the caller passed: three words for the look, two for
        // the angles.
        if matches!(target, 0x001d_08e0 | 0x0011_df30) {
            let (base, words) = if target == 0x001d_08e0 {
                (cpu.gpr[6] + 4, 3)
            } else {
                (cpu.gpr[0], 2)
            };
            for i in 0..words {
                let v = (i as f32 - 0.5 + boundary as f32 * 0.25) * (1.0 + case as f32 * 0.5);
                word(&mut mem, base + 4 * i, v.to_bits());
                native.set_u32(base + 4 * i, v.to_bits()).unwrap();
            }
        }
        let sp = cpu.gpr[4];
        let next = mem.read_u32(sp).unwrap();
        assert_eq!(next, actual.resume_pc);
        cpu.gpr[4] = sp + 4 + cleanup(target);
        cpu.eip = next;
        regs = abi_cpu(&cpu);
    }
    panic!("unbounded call loop");
}

// The look curve: a profile's evenly spaced samples, sampled at a signed
// axis position. Cases walk below zero, past the last sample and in the
// middle.
fn curve_fixture(m: &mut FlatMemory, _: &mut Cpu, case: u32) {
    let samples = RECORDS;
    for i in 0..9u32 {
        float(m, samples + 4 * i, (i as f32 - 4.0) / 4.0);
    }
    word(m, 0x0045_dbd8, 0.0f32.to_bits());
    word(m, 0x0045_dbdc, 1e-6f32.to_bits());
    word(m, STACK + 4, [0.3f32, -0.55, 1.4, 8.5][case as usize % 4].to_bits());
}

// The unit control packet: a full 0x7c-byte packet and the unit body it
// publishes to, with a weapon whose selector matches and one that does
// not.
fn apply_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    let packet = RECORDS;
    let unit = RECORDS + 0x400;
    word(m, points::OBJECTS.base, HEADER);
    word(m, HEADER + 0x44, DATA);
    word(m, DATA + 2 * 12 + 8, unit);
    c.gpr[0] = 0x8200_0002;
    c.gpr[1] = packet;
    word(m, packet, [0x2104_0000, 0x0000_0100, 0][case as usize % 3]);
    m.bytes[(packet + 4) as usize] = 3;
    m.bytes[(packet + 6) as usize] = 0x21;
    word(m, packet + 0xc, 2);
    word(m, packet + 0x1e, 0x0102_0304);
    for (i, v) in [0.25f32, -0.5, 0.75, -0.125, 0.5, -0.25, 0.375, 0.625].into_iter().enumerate() {
        float(m, packet + 0x10 + 4 * i as u32, v);
    }
    for i in 0..9u32 {
        float(m, packet + 0x58 + 4 * i, (i as f32 - 2.0) * 0.125);
    }
    word(m, unit + 0x214, 0x0021);
    m.bytes[(unit + 0x23d) as usize] = 1;
    for offset in [0x1c0, 0x1c4, 0x148, 0x14c] {
        word(m, unit + offset, 0xdead_0000 + offset);
    }
}

// The control update, unbound: the entry a phantom's action builder
// takes. The action and the input record are zeroed and the submitter is
// called with them.
fn control_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    let input = RECORDS + 0x2000;
    word(m, SETTINGS, PHASE);
    word(m, PHASE + 0xf4, RECORDS + 0x2800);
    word(m, CONTROL_TABLE, RECORDS + 0x3000);
    for i in 0..5u32 {
        word(m, STACK + 4 + 4 * i, if i == 4 { input } else { 1 });
    }
    word(m, RECORDS + 0x3014, 0x3600_0000 + case);
    c.gpr[2] = u32::MAX;
    c.gpr[5] = input;
}

// The action builder: a player record with a live unit, a unit body with
// the fields the packet carries, and the frame's pre-call locals.
fn action_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    let unit = RECORDS + 0x400;
    let action = RECORDS + 0x800;
    word(m, points::PLAYERS.base, HEADER);
    word(m, HEADER + 0x44, DATA);
    word(m, DATA + 3 * 12 + 8, RECORDS + 0x600);
    word(m, RECORDS + 0x600 + 0x2c, [u32::MAX, 0x8200_0003, 0x8200_0003][case as usize % 3]);
    word(m, points::OBJECTS.base, HEADER);
    word(m, DATA + 3 * 12 + 8, unit);
    c.gpr[0] = 3;
    word(m, STACK + 4, 0x8200_0003);
    word(m, STACK + 8, action);
    word(m, action, 0x0000_0102);
    for (i, v) in [0.5f32, -0.25, 0.75, -0.5, 0.25, 0.125].into_iter().enumerate() {
        float(m, action + 4 + 4 * i as u32, v * (1.0 + case as f32 * 0.1));
    }
    for i in 0..9u32 {
        float(m, action + 0x34 + 4 * i, (i as f32 - 3.0) * 0.0625);
    }
    m.bytes[(action + 0x58) as usize] = (case % 2) as u8;
    word(m, unit + 0x214, 0x0004);
    // The frame locals the pre-call code and the callees' stubs leave:
    // the body's frame starts 0xa0 below the entry stack.
    for (i, v) in [0.2f32, -0.1, 0.8, 0.15, -0.35].into_iter().enumerate() {
        float(m, STACK - 0xa0 + 4 * i as u32, v);
    }
}

// The neutral control: a unit with its three view directions. The init
// call's stub leaves the packet as the fixture's stack bytes.
fn neutral_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    let unit = RECORDS + 0x400;
    word(m, points::OBJECTS.base, HEADER);
    word(m, HEADER + 0x44, DATA);
    word(m, DATA + 5 * 12 + 8, unit);
    c.gpr[7] = 0x8200_0005;
    for (base, values) in [
        (unit + 0x150, [0.3f32, 0.5, -0.7]),
        (unit + 0x15c, [0.4, -0.6, 0.8]),
        (unit + 0x180, [0.25, 0.75, -0.35]),
    ] {
        for (i, v) in values.into_iter().enumerate() {
            float(m, base + 4 * i as u32, v * (1.0 + case as f32 * 0.5));
        }
    }
    word(m, unit + 0x214, 0x0009);
}

// The target comparator: two 0x60-byte candidate records, the query
// parameters, and every field the ordering reads.
fn compare_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    let lhs = RECORDS;
    let rhs = RECORDS + 0x60;
    word(m, STACK + 4, lhs);
    word(m, STACK + 8, rhs);
    word(m, 0x0051_e99c, [0, RECORDS + 0x800, RECORDS + 0x800][case as usize % 3]);
    if case % 3 != 0 {
        word(m, RECORDS + 0x800 + 0x14, 1 << 5 | 1 << 4);
    }
    for (record, weight) in [(lhs, 0.75f32), (rhs, 0.5)] {
        float(m, record + 0x4c, weight + case as f32 * 0.125);
        float(m, record + 0x50, 0.25 + case as f32 * 0.5);
        float(m, record + 0x54, 0.125);
        float(m, record + 0x44, 3.0 + case as f32);
        float(m, record + 0x48, 2.0);
        m.bytes[(record + 0x58) as usize] = 1;
        m.bytes[(record + 0x28) as usize] = (case % 2) as u8;
        word(m, record, 0x8200_0004);
        float(m, record + 0x2c, 0.5);
        float(m, record + 0x30, 0.25);
    }
    word(m, 0x0045_dbbc, 0.1f32.to_bits());
    c.gpr[0] = 0x5151_0000;
}

// The aim-assist cone: a unit holding a weapon whose definition carries
// the cone fields, with the zoom level answered by the stub.
fn params_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    let unit = RECORDS + 0x400;
    let weapon = RECORDS + 0x1000;
    word(m, points::OBJECTS.base, HEADER);
    word(m, HEADER + 0x44, DATA);
    word(m, DATA + 2 * 12 + 8, unit);
    c.gpr[1] = 0x8200_0002;
    c.gpr[6] = RECORDS + 0x400;
    word(m, STACK + 4, 1);
    word(m, unit + 0x218, 0x8300_0006);
    word(m, DATA + 6 * 12 + 8, weapon);
    m.bytes[(unit + 0x212) as usize] = 6;
    m.bytes[(unit + 0x213) as usize] = [u8::MAX, u8::MAX, 7, u8::MAX][case as usize % 4];
    word(m, points::TAG_ENTRIES.base, RECORDS + 0x1800);
    word(m, RECORDS + 0x1800 + 6 * 16 + 8, RECORDS + 0x2000);
    word(m, weapon, 6);
    for (offset, v) in [
        (0x208, 0.05f32),
        (0x20c, 12.0),
        (0x210, 0.09),
        (0x214, 8.0),
        (0x218, 0.12),
    ] {
        float(m, RECORDS + 0x2000 + offset, v * (1.0 + case as f32 * 0.5));
    }
    word(m, RECORDS + 0x2000 + 0x294, u32::from(1 + case % 2));
    word(m, weapon + 0x12c, [0, 0x20, 0x20][case as usize % 3]);
}

// The HUD seat update: the seat record, the unit and player the seat
// reports, and the two weapon views. The stubs answer the user test and
// the player find differently per case.
fn hud_fixture(m: &mut FlatMemory, _: &mut Cpu, case: u32) {
    let seat = RECORDS + 0x400;
    word(m, HUD_SEATS, RECORDS + 0x800);
    word(m, STACK + 4, 1);
    word(m, points::OBJECTS.base, HEADER);
    word(m, HEADER + 0x44, DATA);
    // The players array: player 2's unit handle, object index 3.
    word(m, points::PLAYERS.base, RECORDS + 0x900);
    word(m, RECORDS + 0x900 + 0x44, RECORDS + 0x1000);
    word(m, RECORDS + 0x1000 + 2 * 0x21c + 0x2c, 0x8200_0003);
    word(m, points::GAME_TIME.base, RECORDS + 0xa00);
    word(m, RECORDS + 0xa00 + 4, 0.5f32.to_bits());
    word(m, RECORDS + 0xa00 + 8, 100);
    // The unit: object table entry 3 (12 bytes each, the record at +8),
    // its shield falling, rising or level against the displayed 0.9.
    word(m, DATA + 3 * 12 + 8, RECORDS + 0xc00);
    float(m, RECORDS + 0xc00 + 0xf0, [0.75, 0.95, 0.9][case as usize % 3]);
    // The frame the entry builds: the widget state the stubs leave and
    // the weapon views. The frame base sits 0x274 below the saved base
    // pointer, which itself is one word below the entry stack.
    m.bytes[(STACK - 0x148) as usize] = 1;
    m.bytes[(STACK - 0x145) as usize] = 1;
    float(m, seat + 0x28, 0.5);
    float(m, seat, 0.9);
    float(m, seat + 4, 0.2);
    word(m, seat + 8, 90);
    word(m, STACK - 0x26c, [u32::MAX, 0x8400_0001, u32::MAX][case as usize % 3]);
    word(m, STACK - 0x268, [u32::MAX, u32::MAX, 0x8400_0002][case as usize % 3]);
}

// The observer resource walk: a list of predictions over the entry types
// the dispatch table names, with the tag records they point at.
fn resources_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    let list = RECORDS;
    let entries = RECORDS + 0x400;
    let kinds: [u16; 5] = match case {
        0 => [8, 9, 10, 16, 8],
        1 => [11, 12, 13, 14, 15],
        _ => [0, 7, 8, 3, 9],
    };
    word(m, list, kinds.len() as u32);
    word(m, list + 4, entries);
    for (i, &kind) in kinds.iter().enumerate() {
        let entry = (entries + i as u32 * 8) as usize;
        m.bytes[entry..entry + 2].copy_from_slice(&kind.to_le_bytes());
        m.bytes[entry + 2] = 5;
        m.bytes[entry + 3] = 0;
        word(m, entries + i as u32 * 8 + 4, 0x1234);
    }
    word(m, points::TAG_ENTRIES.base, RECORDS + 0x800);
    word(m, RECORDS + 0x800 + 0x1234 * 16 + 8, RECORDS + 0x1000);
    word(m, RECORDS + 0x1000 + 0x48, 0x0060_0000);
    c.gpr[7] = [2, 0, 1][case as usize % 3];
    word(m, STACK + 4, list);
}

// The control update's tail, from the target update's return: the frame
// the original built (the base pointer at the seat's control record, the
// input record in `edi`, the player record in `ebx`), the rates, the
// assist strengths, the button bytes and the two histories.
const CONTROL: u32 = RECORDS + 0x3000;
const INPUT: u32 = RECORDS + 0x2000;
const PLAYER: u32 = RECORDS + 0x4000;
const UNIT: u32 = RECORDS + 0x400;

fn tail_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    // The frame: 0xe0 locals below the return, four saved registers.
    let sp = STACK - 0xf0;
    word(m, STACK, EXIT);
    for i in 0..5u32 {
        word(m, STACK + 4 + 4 * i, if i == 4 { INPUT } else { 1 });
    }
    // The third argument points at the seat's device record.
    word(m, STACK + 4 + 4 * 3, RECORDS + 0x7000);
    word(m, RECORDS + 0x7000, RECORDS + 0x7400);
    m.bytes[(RECORDS + 0x7406) as usize] = 0x7f;
    m.bytes[(RECORDS + 0x7407) as usize] = 0x10;
    word(m, SETTINGS, PHASE);
    word(m, PHASE + 0xf4, RECORDS + 0x2800);
    word(m, PHASE + 0x134, RECORDS + 0x2880);
    float(m, RECORDS + 0x2880 + 0x7c, 1.5);
    word(m, 0x004e_0300, HEADER);
    word(m, HEADER + 0x44, DATA);
    // An odd object type, so the tracked-object walk runs.
    m.bytes[(DATA + 2 * 12 + 3) as usize] = 5;
    word(m, DATA + 2 * 12 + 8, UNIT);
    word(m, UNIT + 0x3e4, [u32::MAX, 0x8200_0002, 0x8200_0002][case as usize % 3]);
    word(m, UNIT + 0xb4, [u32::MAX, 2, 2][case as usize % 3]);
    word(m, 0x0051_e9b8, RECORDS + 0x5000);
    word(m, RECORDS + 0x5000 + 0x44, RECORDS + 0x6000);
    // The rates, strengths and histories the tail reads.
    float(m, sp + 0x18, 0.3);
    float(m, sp + 0x1c, -0.2);
    float(m, sp + 0x40, [0.0f32, 0.8, 0.1][case as usize % 3]);
    word(m, sp + 0x44, RECORDS + 0x2840);
    float(m, RECORDS + 0x2840, 0.5);
    float(m, RECORDS + 0x2840 + 4, 0.25);
    for i in 0..3u32 {
        float(m, sp + 0x38 + 4 * i, (i as f32 - 1.0) * 0.4);
    }
    float(m, sp + 0xb4, 0.7);
    float(m, sp + 0xf8, 1.0 / 30.0);
    float(m, sp + 0xfc, 2.0);
    // The input record under construction.
    float(m, INPUT, 0.05);
    float(m, INPUT + 4, -0.03);
    word(m, INPUT + 0x18, 0x1004_0000);
    word(m, INPUT + 0x1c, 0x0100_0000);
    m.bytes[(INPUT + 0x20) as usize] = 3;
    m.bytes[(INPUT + 0x21) as usize] = 1;
    // The seat's control record.
    word(m, CONTROL, [u32::MAX, 0x8200_0002, 0x8200_0002][case as usize % 3]);
    m.bytes[(CONTROL + 0x2e) as usize] = 1;
    m.bytes[(CONTROL + 0x2e + 1) as usize] = 0;
    word(m, CONTROL + 0x34, 0);
    float(m, CONTROL + 0x38, 0.2);
    float(m, CONTROL + 0x3c, -0.4);
    float(m, CONTROL + 0x40, 0.6);
    m.bytes[(CONTROL + 0x68) as usize] = 0;
    float(m, CONTROL + 0x78, 0.1);
    float(m, CONTROL + 0x7c, -0.05);
    m.bytes[(CONTROL + 0x88) as usize] = (case % 2) as u8;
    m.bytes[(CONTROL + 0x8e) as usize] = 0;
    m.bytes[(CONTROL + 0x8f) as usize] = 1;
    m.bytes[(CONTROL + 0x90) as usize] = 0;
    m.bytes[(CONTROL + 0x93) as usize] = 0;
    word(m, CONTROL + 0xc, 0x0101);
    word(m, CONTROL + 0xe, 0x0101);
    // The frame's button bytes, counts and the script input.
    for i in 0..16u32 {
        m.bytes[(sp + 0x6c + i) as usize] = ((i + case) % 3) as u8;
        word(m, sp + 0x7c + 2 * i, 0x0200 + i);
    }
    for i in 0..13u32 {
        m.bytes[(sp + 0x20 + i) as usize] = ((i + case) % 2) as u8;
    }
    m.bytes[(sp + 0x2a) as usize] = 1;
    m.bytes[(sp + 0x2b) as usize] = [0u8, 1, 1][case as usize % 3];
    word(m, sp + 0x48, 1);
    word(m, 0x004e_61cc, 0);
    word(m, sp + 0x50, 0x0100);
    word(m, sp + 0x52, 0x0200);
    float(m, sp + 0x9c, 0.3);
    float(m, sp + 0xa0, 0.45);
    float(m, sp + 0xac, 0.125);
    float(m, sp + 0xb0, 0.5);
    word(m, sp + 0xb8, 0x0102);
    m.bytes[(sp + 0xba) as usize] = (case % 2) as u8;
    word(m, 0x004e_6948, PHASE);
    m.bytes[(PHASE + 8) as usize] = 4;
    word(m, 0x0051_0c50, RECORDS + 0x6800);
    m.bytes[(RECORDS + 0x6805) as usize] = 1;
    c.gpr[4] = sp;
    c.gpr[5] = CONTROL;
    c.gpr[7] = INPUT;
    c.gpr[3] = PLAYER;
    c.eip = 0x0018_61a6;
}

macro_rules! differential {
    ($name:ident, $seat:literal, $cases:literal, $prepare:expr) => {
        #[test]
        fn $name() {
            matches_guest($seat, $cases, $prepare);
        }
    };
}

differential!(player_look_curve, "sim.seats.player_look_curve", 4, curve_fixture);
differential!(unit_apply_control, "sim.seats.unit_apply_control", 3, apply_fixture);
differential!(player_control_update_unbound, "sim.seats.player_control_update", 3, control_fixture);
differential!(player_control_update_tail, "sim.seats.player_control_update", 3, tail_fixture);
differential!(player_action_control, "sim.seats.player_action_control", 3, action_fixture);
differential!(player_neutral_control, "sim.seats.player_neutral_control", 2, neutral_fixture);
differential!(aim_target_compare, "sim.seats.aim_target_compare", 3, compare_fixture);
differential!(aim_assist_params, "sim.seats.aim_assist_params", 4, params_fixture);
differential!(hud_seat_update, "sim.seats.hud_seat_update", 3, hud_fixture);
differential!(predict_resources, "sim.seats.predict_resources", 3, resources_fixture);

/// Every covered seat reaches at least one child call, so the resume
/// machinery is exercised, the control update reaches its submitter, and
/// the tail drives the whole selection and flag walk.
#[test]
fn the_covered_seats_reach_their_children() {
    assert!(matches_guest("sim.seats.player_control_update", 1, control_fixture)
        .contains(&0x0018_6a94));
    let tail = matches_guest("sim.seats.player_control_update", 3, tail_fixture);
    assert!(tail.contains(&0x0018_6a94), "the tail never submits");
    assert!(tail.contains(&0x0018_61a6), "the tail never resumes at the target update");
    assert!(!matches_guest("sim.seats.hud_seat_update", 1, hud_fixture).is_empty());
    assert!(!matches_guest("sim.seats.predict_resources", 1, resources_fixture).is_empty());
}
