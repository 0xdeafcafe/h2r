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
//! builders, the target comparator, the aim-assist cone's register-level
//! oracle, the HUD seat update, the observer resource walk, and the
//! phantom executor's action builder `186ab0` and interaction search
//! `151110` (`sim_seats::BUILDER`, `sim_seats::INTERACTION`). The cone's
//! kernel arm runs against the original with its real callees
//! (`aim_assist_matches_lifted`). The interactive search family
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

/// The first byte where `a` and `b` differ outside `dead`: slice compares
/// (memcmp) first, the byte walk only once a difference is known.
fn first_outside(a: &[u8], b: &[u8], dead: &std::ops::Range<usize>) -> Option<usize> {
    let (lo, hi) = (dead.start.min(a.len()), dead.end.min(a.len()));
    if a[..lo] == b[..lo] && a[hi..] == b[hi..] {
        return None;
    }
    a.iter().zip(b).enumerate().find(|(i, (x, y))| x != y && !dead.contains(i)).map(|(i, _)| i)
}

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
        // The pickup scan's candidate.
        0x0015_0e30 => 8,
        // The action builder's and the interaction search's callees.
        0x0012_be90 | 0x000c_fe40 | 0x000c_dff0 | 0x0015_5760 | 0x000c_85c0
        | 0x000c_8960 | 0x0037_3b6a | 0x0014_d040 => 0,
        0x0014_8fff => 4,
        0x0018_7510 | 0x0015_1110 | 0x0015_1780 | 0x0015_16c0 | 0x0015_1430
        | 0x0015_15e0 => 8,
        0x000c_deb0 => 0x10,
        0x0018_5be0 => 0x14,
        0x000b_b050 => 0x1c,
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
            // The builder's dialog and block tests: blocked one case in three.
            0x0014_8fff => u32::from(case % 3 == 0),
            0x0012_be90 => 0,
            // The unit's weapon slot: none, the block's, another.
            0x000c_fe40 => [0xffff, 1, 0, 2][case as usize % 4],
            0x000c_dff0 => case % 2,
            // The camera mode: zoom allowed but one case in four.
            0x0015_5760 => u32::from(case % 4 == 3),
            0x000c_85c0 => u32::from(case % 5 != 4),
            0x000c_8960 => [0xffff, 1][case as usize % 2],
            // The interaction query's count, at most the fixture's seven.
            0x000b_b050 => [0, 1, 3, 5, 7, 2][case as usize % 6],
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
    for seat in [&sim_seats::PARAMS_ORACLE, &sim_seats::BUILDER, &sim_seats::INTERACTION] {
        if name == seat.name {
            return seat;
        }
    }
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
            panic!("{} first different byte at {first:#x}: original {:#04x}, port {:#04x}", why(), mem.bytes[first], ram[first]);
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
differential!(phantom_action_build, "sim.seats.phantom_action_build", 12, build_fixture);
differential!(phantom_interaction_search, "sim.seats.phantom_interaction_search", 12, interaction_fixture);

// The action builder `0x186ab0` and the interaction search `0x151110`
// (docs/perf/sim-phantom-build.md, "The builder body in Rust"): a player
// record, its control block, the unit with its weapon and grenade slots
// and its facing, the zoom and pause globals, the frame's action as the
// control update leaves it, and for the search the listed objects with a
// child chain. Cases walk blocked input, a unit and none, the grenade
// walk, the zoom and flashlight counts and the facing's `fpatan` and
// `fmod`s.

const PLAYERS_GLOBAL: u32 = 0x004e_8c24;
const OBJECTS_GLOBAL: u32 = 0x004e_0300;
const OBJECT_HEADER: u32 = HEADER + 0x100;
const OBJECT_TABLE: u32 = DATA + 0x3000;
const BUILD_PLAYER: u32 = DATA + 0x21c;
const BUILD_UNIT: u32 = RECORDS + 0x400;
const BUILD_CONTROLS: u32 = RECORDS + 0x3000;
const BUILD_OUT: u32 = RECORDS + 0x5000;
const LISTED: u32 = RECORDS + 0x6000;

fn byte_at(m: &mut FlatMemory, a: u32, v: u8) {
    m.bytes[a as usize] = v;
}

fn half_at(m: &mut FlatMemory, a: u32, v: u16) {
    assert!(m.load(a, &v.to_le_bytes()));
}

/// Object `index` at `object`, in the fixture's object table.
fn object(m: &mut FlatMemory, index: u32, object: u32, kind: u8) {
    word(m, OBJECT_TABLE + index * 12 + 8, object);
    byte_at(m, OBJECT_TABLE + index * 12 + 3, kind);
}

fn players_and_unit(m: &mut FlatMemory, case: u32) {
    word(m, PLAYERS_GLOBAL, HEADER);
    word(m, HEADER + 0x44, DATA);
    word(m, OBJECTS_GLOBAL, OBJECT_HEADER);
    word(m, OBJECT_HEADER + 0x44, OBJECT_TABLE);
    word(m, STACK + 4, 0xe000_0001);
    // Unit type bit 0 of `1 << kind`: the facing runs for kind 0 only.
    object(m, 3, BUILD_UNIT, (case % 2) as u8 * 3);
    for (i, v) in [0.6f32, -0.8, 0.25, 1.0, 2.0, 3.0].into_iter().enumerate() {
        float(m, BUILD_UNIT + 0x168 + 4 * i as u32, v * (1.0 + case as f32 * 0.125));
    }
    for (i, v) in [1.0f32, 2.0, 3.0, 0.5].into_iter().enumerate() {
        float(m, BUILD_UNIT + 0x30 + 4 * i as u32, v);
    }
}

fn build_fixture(m: &mut FlatMemory, _: &mut Cpu, case: u32) {
    players_and_unit(m, case);
    let seat = case % 2;
    word(m, BUILD_PLAYER + 0x24, 0);
    half_at(m, BUILD_PLAYER + 0x28, seat as u16);
    word(m, 0x004e_d284, BUILD_CONTROLS);
    byte_at(m, BUILD_CONTROLS + 0x10, u8::from(case == 7));
    word(m, SETTINGS, PHASE);
    word(m, PHASE + 0xf4, RECORDS + 0x2800);
    half_at(m, RECORDS + 0x2800 + 0x6e, 3);
    // The control block: the unit, its weapon and grenade slots, the zoom
    // and flashlight state.
    let block = BUILD_CONTROLS + 0x14 + seat * 0x94;
    word(m, block, if case % 7 == 6 { u32::MAX } else { 0x8200_0003 });
    word(m, block + 0x28, [0x0002_0001, 0x0011_0004][case as usize % 2]);
    byte_at(m, block + 0x2a, [0xff, 1, 0][case as usize % 3]);
    byte_at(m, block + 0x2b, [0xff, 1][case as usize % 2]);
    half_at(m, block + 0x2c, [0xffff, 0, 1][case as usize % 3]);
    half_at(m, block + 0x2e, [0xffff, 2][case as usize % 2]);
    byte_at(m, block + 0x89, (case % 3 == 1) as u8);
    half_at(m, block + 0x8a, 7);
    half_at(m, block + 0x8c, [0, 0x10, 0x7f][case as usize % 3]);
    byte_at(m, block + 0x91, (case % 2) as u8);
    byte_at(m, block + 0x92, (case % 4 == 1) as u8);
    float(m, block + 0x18, [0.9f32, -0.2, 0.05][case as usize % 3]);
    byte_at(m, block + 0x31, [0, 0x7f, 0xff, 5][case as usize % 4]);
    byte_at(m, block + 0x68, (case % 5 == 3) as u8);
    // The unit: no parent but in one case, weapon and grenade fields, the
    // facing request bit.
    word(m, BUILD_UNIT + 0x14, if case % 5 == 2 { 0x8200_0007 } else { u32::MAX });
    byte_at(m, BUILD_UNIT + 0x212, [0xff, 1][case as usize % 2]);
    byte_at(m, BUILD_UNIT + 0x213, [0xff, 0][(case / 2) as usize % 2]);
    word(m, BUILD_UNIT + 0x214, 0x0005_0002);
    byte_at(m, BUILD_UNIT + 0x23d, 1);
    byte_at(m, BUILD_UNIT + 0x23e, [2, 0, 1][case as usize % 3]);
    byte_at(m, BUILD_UNIT + 0x23f, [0, 3, 1][case as usize % 3]);
    byte_at(m, BUILD_UNIT + 0x348, if case % 2 == 0 { 0x61 } else { 0x41 });
    // Slowdown, pause and the film state.
    let slow = RECORDS + 0x5a00;
    word(m, 0x0051_0c50, if case % 3 == 1 { slow } else { 0 });
    byte_at(m, slow + 5, (case % 2) as u8);
    let pause = RECORDS + 0x5900;
    word(m, 0x0051_0c54, pause);
    byte_at(m, pause, (case % 4 == 2) as u8);
    byte_at(m, pause + 1, 1);
    let sim = RECORDS + 0x5800;
    word(m, 0x004e_6948, sim);
    byte_at(m, sim + 0xd, if case % 5 == 0 { 3 } else { 1 });
    // The profile's flashlight bits.
    word(m, 0x0054_e8e0, 0x10);
    word(m, 0x0054_e9f4, if case % 3 == 2 { 0 } else { 0x08 });
    // The arguments and the frame's action (the control update's stub
    // writes nothing): the body's frame starts 0x64 below the entry stack.
    float(m, STACK + 8, 1.0 / 30.0);
    float(m, STACK + 0xc, 1.0 / 30.0);
    word(m, STACK + 0x10, BUILD_OUT);
    let sp = STACK - 0x64;
    for i in 0..0x13u32 {
        float(m, sp + 0x18 + 4 * i, (i as f32 - 6.0) * 0.125 * (1.0 + case as f32 * 0.25));
    }
    float(m, sp + 0x2c, [-0.5f32, 0.0, 0.75][case as usize % 3]);
    word(m, sp + 0x30, [0, 0x40, 0x200, 0x0100_0000][case as usize % 4]);
    byte_at(m, sp + 0x34, [0x4e, 0x00, 0x46, 0x08, 0x40, 0x0c][case as usize % 6]);
    byte_at(m, sp + 0x39, [0x01, 0x00, 0x03][case as usize % 3]);
}

/// The listed objects: kinds by the jump table (weapon, vehicle, biped,
/// control), one far, one whose kind is skipped, and a scenery object
/// with two children.
fn interaction_fixture(m: &mut FlatMemory, _: &mut Cpu, case: u32) {
    players_and_unit(m, case);
    word(m, BUILD_PLAYER + 0x2c, if case % 5 == 4 { u32::MAX } else { 0x8200_0003 });
    word(m, BUILD_UNIT + 0x14, if case % 7 == 5 { 0x8200_0009 } else { u32::MAX });
    word(m, STACK + 8, BUILD_OUT);
    let kinds: [u8; 9] = [2, 1, 0, 8, 6, 4, 2, 7, 3];
    let rotate = case as usize % kinds.len();
    for i in 0..kinds.len() as u32 {
        let index = 4 + i;
        let at = LISTED + i * 0x100;
        object(m, index, at, 0);
        byte_at(m, at + 0xaa, kinds[(i as usize + rotate) % kinds.len()]);
        // Near the unit but the seventh, far off.
        let far = if i == 6 { 50.0 } else { 0.0 };
        for (j, v) in [1.25f32 + far, 2.0, 3.5, 0.25].into_iter().enumerate() {
            float(m, at + 0x30 + 4 * j as u32, v);
        }
        word(m, at + 0x10, u32::MAX);
        word(m, at + 0xc, u32::MAX);
    }
    // The scenery object's children: 13, then 14.
    for (index, next) in [(13u32, 0x8000_000e_u32), (14, u32::MAX)] {
        let at = LISTED + 0x1000 + index * 0x100;
        object(m, index, at, 0);
        word(m, at + 0xc, next);
        word(m, at + 0x10, u32::MAX);
        byte_at(m, at + 0xaa, 6);
    }
    for i in 0..kinds.len() as u32 {
        let at = LISTED + i * 0x100;
        if m.bytes[(at + 0xaa) as usize] == 6 {
            word(m, at + 0x10, 0x8000_000d);
        }
    }
    // The query's list, as the stub leaves it: the frame starts 0x118
    // below the entry stack, the list 0x14 above its saved ESI.
    let list = STACK - 0x114 + 0x14;
    for i in 0..0x40u32 {
        word(m, list + 4 * i, 0x8000_0004 + i % kinds.len() as u32);
    }
}

/// The builder reaches its interaction search and its copy-out, and the
/// facing's two `fmod` calls; the search reaches its query and a test.
#[test]
fn the_phantom_body_reaches_its_children() {
    let build = matches_guest("sim.seats.phantom_action_build", 12, build_fixture);
    if build.is_empty() {
        return; // no image
    }
    for site in [0x0018_6b1b, 0x0018_6c18, 0x0018_6f0f, 0x0018_6feb, 0x0018_7002, 0x0018_701d] {
        assert!(build.contains(&site), "the builder never resumes at {site:#x}");
    }
    let search = matches_guest("sim.seats.phantom_interaction_search", 12, interaction_fixture);
    assert!(search.contains(&0x0015_11bc), "the search never queries");
    assert!(
        [0x0015_1281, 0x0015_1291, 0x0015_12a1, 0x0015_12b1].iter().any(|s| search.contains(s)),
        "the search never tests an object"
    );
}

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

// The aim-assist cone through its kernel arm (`engine::sim_seats::aim::arm`,
// over `sim_objects::aim_assist`), against the lifted original with its
// real callees: the zoom magnification `101090` and the image's `pow`
// `372d48` on the interpreter's x87, the tracking test `1011d0` and the
// acquisition test `101160`. No stubs: the arm's answer, applied, must
// leave every register but the flags and every byte above the dead stack
// as the original's return does.

const AIM_UNIT: u32 = RECORDS + 0x400;
const AIM_FIRST: u32 = RECORDS + 0x1000;
const AIM_SECOND: u32 = RECORDS + 0x1100;
const AIM_ENTRIES: u32 = RECORDS + 0x1800;
const AIM_DEF_FIRST: u32 = RECORDS + 0x2000;
const AIM_DEF_SECOND: u32 = RECORDS + 0x3000;
const AIM_OUT: u32 = RECORDS + 0x5000;

/// One weapon definition: the cone, the class, the flags, the zoom range
/// and its triggers (state in the weapon object, kind in the definition).
struct AimDef {
    cone: [f32; 5],
    class: u16,
    flags: u32,
    levels: i16,
    min: f32,
    max: f32,
    triggers: &'static [(u8, u16)],
}

fn put_def(m: &mut FlatMemory, def: u32, object: u32, d: &AimDef) {
    for (i, v) in d.cone.iter().enumerate() {
        float(m, def + 0x208 + 4 * i as u32, *v);
    }
    m.bytes[(def + 0x294) as usize..][..2].copy_from_slice(&d.class.to_le_bytes());
    word(m, def + 0x12c, d.flags);
    m.bytes[(def + 0x1fe) as usize..][..2].copy_from_slice(&d.levels.to_le_bytes());
    float(m, def + 0x200, d.min);
    float(m, def + 0x204, d.max);
    word(m, def + 0x2c8, d.triggers.len() as u32);
    word(m, def + 0x2cc, def + 0x800);
    for (i, &(state, kind)) in d.triggers.iter().enumerate() {
        let i = i as u32;
        m.bytes[(object + 0x20c + 12 * i) as usize] = state;
        m.bytes[(def + 0x800 + 0x40 * i + 6) as usize..][..2].copy_from_slice(&kind.to_le_bytes());
    }
}

/// Case `case`: the zoom level, the unit's register and what the second
/// slot holds.
fn aim_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    word(m, points::OBJECTS.base, HEADER);
    word(m, HEADER + 0x44, DATA);
    word(m, points::TAG_ENTRIES.base, AIM_ENTRIES);
    word(m, DATA + 2 * 12 + 8, AIM_UNIT);
    word(m, DATA + 6 * 12 + 8, AIM_FIRST);
    word(m, DATA + 7 * 12 + 8, AIM_SECOND);
    word(m, AIM_FIRST, 6);
    word(m, AIM_SECOND, 7);
    word(m, AIM_ENTRIES + 6 * 16 + 8, AIM_DEF_FIRST);
    word(m, AIM_ENTRIES + 7 * 16 + 8, AIM_DEF_SECOND);
    m.bytes[(AIM_UNIT + 0x212) as usize] = 0;
    word(m, AIM_UNIT + 0x218, 0x8300_0006);
    for i in 0..9 {
        word(m, AIM_OUT + 4 * i, 0xdead_beef);
    }
    let scale = 1.0 + case as f32 * 0.25;
    let cone = [0.05, 12.0, 0.09, 8.0, 0.12].map(|v: f32| v * scale);
    let (zoom, first, second): (i32, AimDef, Option<Option<AimDef>>) = match case {
        0 => (-1, AimDef { cone, class: 1, flags: 0, levels: 2, min: 2.0, max: 8.0, triggers: &[(0, 5)] }, None),
        1 => (1, AimDef { cone, class: 2, flags: 0, levels: 3, min: 1.5, max: 6.0, triggers: &[(0, 1), (6, 0)] }, None),
        2 => (
            0,
            AimDef { cone, class: 1, flags: 0, levels: 2, min: 2.0, max: 4.0, triggers: &[(2, 0)] },
            Some(Some(AimDef { cone: [0.2, 5.0, 0.3, 1.0, 0.01], class: 2, flags: 0, levels: 2, min: 3.0, max: 5.0, triggers: &[] })),
        ),
        3 => (2, AimDef { cone, class: 1, flags: 0, levels: 3, min: 1.25, max: 3.0, triggers: &[] }, Some(None)),
        4 => (-1, AimDef { cone, class: 1, flags: 0x20, levels: 2, min: 2.0, max: 8.0, triggers: &[] }, None),
        5 => (0, AimDef { cone, class: 1, flags: 0, levels: 2, min: 2.0, max: 8.0, triggers: &[] }, None),
        _ => (
            1,
            AimDef { cone, class: 0, flags: 0, levels: 2, min: 0.0, max: 10.0, triggers: &[(3, 0)] },
            Some(Some(AimDef { cone: [0.01, 50.0, 0.02, 60.0, 0.5], class: 1, flags: 0, levels: 1, min: -1.0, max: 2.0, triggers: &[] })),
        ),
    };
    put_def(m, AIM_DEF_FIRST, AIM_FIRST, &first);
    match second {
        None => m.bytes[(AIM_UNIT + 0x213) as usize] = 0xff,
        Some(held) => {
            m.bytes[(AIM_UNIT + 0x213) as usize] = 1;
            match held {
                Some(d) => {
                    word(m, AIM_UNIT + 0x21c, 0x8400_0007);
                    put_def(m, AIM_DEF_SECOND, AIM_SECOND, &d);
                }
                None => word(m, AIM_UNIT + 0x21c, u32::MAX),
            }
        }
    }
    word(m, STACK + 4, 0x5a5a_0000 | (zoom as u32 & 0xffff));
    c.gpr[1] = if case == 5 { u32::MAX } else { 0x8200_0002 };
    c.gpr[6] = AIM_OUT;
}

#[test]
fn aim_assist_matches_lifted() {
    use engine::sim_seats::aim::arm;
    let Some((image, ram)) = image() else { return };
    let mut code = original(image, &points::AIM_ASSIST_PARAMS);
    for case in 0..7 {
        for control in [0x027f_u16, 0x037f, 0x007f] {
            let mut mem = ram.clone();
            word(&mut mem, STACK, EXIT);
            let mut cpu = Cpu {
                gpr: [0x1234_5678, 0, 0x00fe_3420, 0x7b1c, STACK, 0x00aa_bbcc, 0, 0x0e37],
                eip: points::AIM_ASSIST_PARAMS.base,
                eflags: 0x2,
                mxcsr: 0x1fa0,
                ..Default::default()
            };
            for (i, reg) in cpu.xmm.iter_mut().enumerate() {
                *reg = (0x1234_u128 + i as u128) << 100 | 0x4000_0000;
            }
            cpu.x87.control = control;
            aim_fixture(&mut mem, &mut cpu, case);

            let mut native = Guest::new();
            native.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
            native.write_bytes(0, &mem.bytes, 1).unwrap();
            let mut regs = abi_cpu(&cpu);
            let why = format!("case {case} control {control:#06x}");
            let prediction = arm::predict(&regs, &native).unwrap_or_else(|d| panic!("{why}: declined {d:?}"));
            let target = arm::apply(&prediction, &mut regs, &mut native).unwrap();
            assert_eq!(target, EXIT, "{why}");

            lifted::code::run_until(&mut code, &mut cpu, &mut mem, 1_000_000, |c| c.eip == EXIT)
                .unwrap_or_else(|e| panic!("{why}: {e}"));
            let want = abi_cpu(&cpu);
            let gprs = |c: &runtime::CpuAbi| [c.eax, c.ecx, c.edx, c.ebx, c.esp, c.ebp, c.esi, c.edi];
            assert_eq!(gprs(&regs), gprs(&want), "{why}: eax ecx edx ebx esp ebp esi edi");
            assert_eq!(regs.xmm, want.xmm, "{why}: xmm");
            assert_eq!(regs.mxcsr, want.mxcsr, "{why}: mxcsr");
            assert_eq!(
                (regs.x87.st, regs.x87.tag, regs.x87.status, regs.x87.control),
                (want.x87.st, want.x87.tag, want.x87.status, want.x87.control),
                "{why}: x87"
            );
            assert_eq!(prediction.differs(&want, prediction.record.map(|(out, _)| {
                std::array::from_fn(|i| mem.read_u32(out + 4 * i as u32).unwrap())
            })), None, "{why}");
            // Every byte but the dead stack the original's frames used.
            let start = native.mappings()[0].physical_start as usize;
            let live = &native.physical_ram()[start..start + RAM];
            let dead = (STACK - 0x1_0000) as usize..STACK as usize;
            let first = first_outside(&mem.bytes, &live, &dead);
            assert_eq!(first, None, "{why}: first differing byte");
        }
    }
}

// The bound controller's device read through its kernel arm
// (`engine::sim_seats::device`), against the lifted original with its
// real callees: the velocity's length `24590` and the image's `pow`
// `372d48` on the interpreter's x87. The kernel answers every read; the
// body's first transfer (the look curve's call,
// the visibility walk's, or, unseen, the one after) must leave every
// register but the flags and every byte above the dead stack as the
// original's does at the same call.

const DEV_OUT: u32 = RECORDS + 0x2000;
const DEV_SETTINGS: u32 = RECORDS + 0x2800;
const DEV_CONTROL: u32 = RECORDS + 0x3000;
const DEV_SEAT: u32 = DEV_CONTROL + 0x94 + 0x14;
const DEV_SLOT: u32 = RECORDS + 0x7000;
const DEV_TICKS: u32 = RECORDS + 0x7100;
const DEV_UNIT: u32 = RECORDS + 0x400;
const DEV_VEHICLE: u32 = RECORDS + 0x8000;
const DEV_ENTRIES: u32 = RECORDS + 0x9000;
const DEV_DEF: u32 = RECORDS + 0xa000;
const DEV_SEATS: u32 = RECORDS + 0xb000;
const DEV_CASES: u32 = 11;

/// The vehicle seat: speed, range, exponent, yaw and pitch ends.
struct DevSeat {
    velocity: [f32; 3],
    range: [f32; 2],
    exponent: f32,
    yaw: [f32; 2],
    pitch: [f32; 2],
}

fn device_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    let port = 1u32;
    // seat 1, ..., the input slot, the record.
    for (i, v) in [1, 7, 7, DEV_SLOT, DEV_OUT].into_iter().enumerate() {
        word(m, STACK + 4 + 4 * i as u32, v);
    }
    word(m, SETTINGS, PHASE);
    word(m, PHASE + 0xf4, DEV_SETTINGS);
    word(m, DEV_SETTINGS + 0x74, 9);
    word(m, DEV_SETTINGS + 0x78, RECORDS + 0x2900);
    word(m, CONTROL_TABLE, DEV_CONTROL);
    word(m, 0x004e_61cc + 4 * port, 0x1234);
    m.bytes[0x004e_61b9] = u8::from(case == 1);
    word(m, 0x0051_0c54, DEV_TICKS);
    float(m, 0x0051_ea18 + port * 0x1c, 1.25 + case as f32 * 0.5);
    float(m, 0x0051_ea1c + port * 0x1c, 0.75);
    let block = 0x0051_ea88 + port * 0x50;
    for i in 0..16 {
        m.bytes[(block + i) as usize] = if i % 3 == case % 3 { 0 } else { (i as u8) | 0x40 };
        let hold = (0x100 + i * 7 + case) as u16;
        m.bytes[(block + 0x10 + 2 * i) as usize..][..2].copy_from_slice(&hold.to_le_bytes());
    }
    for i in 0..8 {
        float(m, block + 0x30 + 4 * i, i as f32 * 0.125 - 0.3);
    }
    let sticks: [(f32, f32); 3] = [(0.6, -0.3), (0.2, 0.9), (0.05, -0.02)];
    let (horizontal, vertical) = sticks[case as usize % 3];
    float(m, block + 0x44, horizontal);
    float(m, block + 0x40, vertical);
    // The seat's wanted buttons.
    word(m, DEV_SEAT + 0xc, 0x0f35_8b37 ^ case);
    // The unit, its parent and the parent's seat block.
    word(m, points::OBJECTS.base, HEADER);
    word(m, HEADER + 0x44, DATA);
    word(m, points::TAG_ENTRIES.base, DEV_ENTRIES);
    word(m, DATA + 2 * 12 + 8, DEV_UNIT);
    word(m, DATA + 3 * 12 + 8, DEV_VEHICLE);
    word(m, DEV_VEHICLE, 4);
    word(m, DEV_ENTRIES + 4 * 16 + 8, DEV_DEF);
    word(m, DEV_DEF + 0x1cc, DEV_SEATS);
    m.bytes[(DEV_UNIT + 0x1fc) as usize..][..2].copy_from_slice(&1u16.to_le_bytes());
    let seat = |velocity, range, exponent, yaw, pitch| Some(DevSeat { velocity, range, exponent, yaw, pitch });
    let (unit, vehicle, blocked) = match case {
        0..=2 => (u32::MAX, None, 0),
        3 => (2, seat([3.0, 4.0, 1.0], [1.0, 9.0], 2.0, [1.0, 5.0], [0.0, 0.0]), 0),
        4 => (2, seat([30.0, 0.0, 0.0], [1.0, 9.0], 0.0, [0.0, 2.0], [0.5, 0.25]), 0),
        5 => (2, seat([0.1, 0.0, 0.0], [1.0, 9.0], 0.5, [1.0, 5.0], [1.0, 3.0]), 0),
        6 => (2, seat([1.0, 1.0, 1.0], [4.0, 2.0], 1.5, [2.0, 1.0], [0.0, 0.0]), 0),
        7 => (2, None, 1),
        8 => (2, seat([2.0, 2.0, 1.0], [0.5, 6.0], 3.0, [1.0, 2.0], [2.0, 1.0]), 2),
        9 => (u32::MAX, None, 1),
        _ => (2, None, 2),
    };
    word(m, DEV_SEAT, if unit == u32::MAX { u32::MAX } else { 0x8200_0000 | unit });
    word(m, DEV_UNIT + 0x14, if vehicle.is_some() { 0x8300_0003 } else { u32::MAX });
    if let Some(s) = vehicle {
        let at = DEV_SEATS + 0xb0;
        for (i, v) in [s.yaw[0], s.yaw[1], s.pitch[0], s.pitch[1], s.range[0], s.range[1], s.exponent]
            .into_iter()
            .enumerate()
        {
            float(m, at + 0x44 + 4 * i as u32, v);
        }
        for (i, v) in s.velocity.into_iter().enumerate() {
            float(m, DEV_VEHICLE + 0x88 + 4 * i as u32, v);
        }
    }
    // Blocked by the console's flag (1) or the tick's pause pair (2).
    m.bytes[(DEV_CONTROL + 0x10) as usize] = u8::from(blocked == 1);
    m.bytes[DEV_TICKS as usize] = u8::from(blocked == 2);
    m.bytes[DEV_TICKS as usize + 1] = u8::from(blocked == 2) * 3;
    // Case 10's unit is not a biped: blocked and unseen.
    m.bytes[(DATA + 2 * 12 + 3) as usize] = if case == 10 { 1 } else { 0 };
    c.gpr[2] = port;
}

/// The state every bound-read test starts from.
fn device_start(ram: &FlatMemory, case: u32, control: u16) -> (FlatMemory, Cpu) {
    let mut mem = ram.clone();
    word(&mut mem, STACK, EXIT);
    let mut cpu = Cpu {
        gpr: [0x1234_5678, 0x9876_5432, 0, 0x7b1c, STACK, 0x00aa_bbcc, 0x5151, 0x0e37],
        eip: points::PLAYER_CONTROL_UPDATE.base,
        eflags: 0x2,
        mxcsr: 0x1fa0,
        ..Default::default()
    };
    for (i, reg) in cpu.xmm.iter_mut().enumerate() {
        *reg = (0x1234_u128 + i as u128) << 100 | 0x4000_0000;
    }
    cpu.x87.control = control;
    device_fixture(&mut mem, &mut cpu, case);
    (mem, cpu)
}

fn native_of(mem: &FlatMemory) -> Guest {
    let mut native = Guest::new();
    native.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
    native.write_bytes(0, &mem.bytes, 1).unwrap();
    native
}

#[test]
fn bound_control_matches_lifted() {
    let Some((image, ram)) = image() else { return };
    let seat = find_seat("sim.seats.player_control_update");
    let mut code = original(image, seat.point);
    let mut reached = BTreeSet::new();
    for case in 0..DEV_CASES {
        for control in [0x027f_u16, 0x037f, 0x007f] {
            let (mut mem, mut cpu) = device_start(ram, case, control);
            let mut native = native_of(&mem);
            let mut regs = abi_cpu(&cpu);
            let why = format!("case {case} control {control:#06x}");
            let transfer = (seat.run)(&mut regs, &mut native, seat.point.base)
                .unwrap()
                .unwrap_or_else(|| panic!("{why}: declined"));
            assert_eq!(transfer.kind, abi::CALL, "{why}");
            reached.insert(transfer.target_pc);
            regs.eip = transfer.target_pc;
            let esp = regs.esp;
            lifted::code::run_until(&mut code, &mut cpu, &mut mem, 2_000_000, |c| {
                c.eip == transfer.target_pc && c.gpr[4] == esp
            })
            .unwrap_or_else(|e| panic!("{why}: {e}"));
            let want = abi_cpu(&cpu);
            let gprs = |c: &runtime::CpuAbi| [c.eax, c.ecx, c.edx, c.ebx, c.esp, c.ebp, c.esi, c.edi];
            assert_eq!(gprs(&regs), gprs(&want), "{why}: eax ecx edx ebx esp ebp esi edi");
            assert_eq!(regs.xmm, want.xmm, "{why}: xmm");
            assert_eq!(regs.mxcsr, want.mxcsr, "{why}: mxcsr");
            assert_eq!(
                (regs.x87.st, regs.x87.tag, regs.x87.status, regs.x87.control),
                (want.x87.st, want.x87.tag, want.x87.status, want.x87.control),
                "{why}: x87"
            );
            let start = native.mappings()[0].physical_start as usize;
            let live = &native.physical_ram()[start..start + RAM];
            let dead = (STACK - 0x1_0000) as usize..esp as usize;
            let first = mem.bytes.iter().zip(live).enumerate()
                .find(|(i, (a, b))| a != b && !dead.contains(i))
                .map(|(i, (a, b))| (i, *a, *b));
            assert_eq!(first, None, "{why}: first differing byte (original, port)");
        }
    }
    // The curve, the visibility walk, and an unseen read's next call.
    assert!(reached.contains(&sim_seats::look::CURVE), "{reached:x?}");
    assert!(reached.contains(&0x000e_4050), "{reached:x?}");
    assert!(reached.len() >= 3, "{reached:x?}");
}

// The seat bodies' leaf callees (`engine::sim_seats::leaf`): each kernel's
// effect, applied, against the lifted leaf run to its return. Every
// register the leaf leaves, `xmm0`, the x87 stack, and every byte but the
// dead stack must agree.

const LEAF_USERS: u32 = RECORDS + 0x100;
const LEAF_PLAYER: u32 = RECORDS + 0x200;
const LEAF_VECTOR: u32 = RECORDS + 0x300;
const LEAF_OUT: u32 = RECORDS + 0x320;
const LEAF_BODY: u32 = RECORDS + 0x1000;
const LEAF_TICKS: u32 = RECORDS + 0x380;
const LEAF_PACKET: u32 = RECORDS + 0xc00;
/// `c6ef0`'s pointer to the vector its three directions start as.
const PACKET_VECTOR: u32 = 0x0046_87a8;

fn leaf_fixture(m: &mut FlatMemory, c: &mut Cpu, leaf: sim_seats::leaf::Leaf, case: u32) {
    use sim_seats::leaf::Leaf;
    match leaf {
        Leaf::UserTest | Leaf::UserPlayer => {
            word(m, 0x004e_8c20, LEAF_USERS);
            for (i, v) in [5u32, u32::MAX, 0x8100_0007, u32::MAX].into_iter().enumerate() {
                word(m, LEAF_USERS + 0xc + 4 * i as u32, v);
            }
            c.gpr[1] = [0, 1, u32::MAX, 2][case as usize % 4];
        }
        Leaf::PlayerIndex => {
            m.bytes[(LEAF_PLAYER + 0x18) as usize] = [2u8, 0xff, 5, 3][case as usize % 4];
            word(m, LEAF_PLAYER + 0x74, 4);
            c.gpr[1] = LEAF_PLAYER;
        }
        Leaf::Visible => {
            word(m, 0x004e_0300, HEADER);
            word(m, HEADER + 0x44, DATA);
            word(m, DATA + 2 * 12 + 8, LEAF_BODY);
            word(m, 0x0051_0c54, LEAF_TICKS);
            let tick = [30u16, 300, 4, 31, 0][case as usize % 5];
            m.bytes[(LEAF_TICKS + 2) as usize..][..2].copy_from_slice(&tick.to_le_bytes());
            m.bytes[(LEAF_BODY + 0xc1) as usize] = u8::from(case == 5);
            m.bytes[(LEAF_BODY + 0x348) as usize] = u8::from(case != 6);
            m.bytes[(LEAF_BODY + 0x399) as usize] = [3u8, 120, 0x80, 2, 1, 1, 1][case as usize % 7];
            m.bytes[(LEAF_BODY + 0x3dc) as usize] = [1u8, 3, 1, 2, 3, 1, 1][case as usize % 7];
            word(m, LEAF_BODY + 0x14, if case == 3 { 0x8300_0003 } else { u32::MAX });
            c.gpr[0] = 0x8200_0002;
        }
        Leaf::Submit => {
            let (rec, block, ctrl) = (RECORDS + 0x400, RECORDS + 0x480, RECORDS + 0x500);
            word(m, 0x004e_d284, ctrl);
            let seed = case.wrapping_mul(0x9e37_79b9).rotate_left(case);
            word(m, ctrl + 4, seed & 0x0003_0000);
            word(m, ctrl + 8, [0u32, 1, 4, 8, 0xd][case as usize % 5]);
            word(m, ctrl + 0xc, [0x3c0du32, 0x2405, 0x1801, 0x0c0c, 0x380f, 0, 0xffff][case as usize % 7]);
            word(m, rec + 0x18, seed & 0x0400_0026);
            word(m, rec + 0x1c, (seed >> 7) & 0x37);
            word(m, rec + 0x20, 0xa5c0_0000 | (seed >> 3) & 0xffff);
            let f: [[f32; 6]; 4] = [
                [0.5, -0.25, 1.0, 0.0, -0.5, 0.3],
                [-1.0, 0.75, -0.0, 2.0, 0.25, -0.1],
                [0.0, 0.0, -2.0, -0.5, 0.0, 0.0],
                [0.2, -0.2, 0.4, -0.4, 0.6, -0.6],
            ];
            for (o, v) in [0u32, 4, 8, 0x10, 0x14, 0x24].into_iter().zip(f[case as usize % 4]) {
                float(m, rec + o, v);
            }
            m.bytes[(block + 8) as usize] = (case % 3) as u8;
            m.bytes[(block + 9) as usize] = (case % 2) as u8 * 0x80;
            c.gpr[1] = rec;
            c.gpr[0] = block;
        }
        Leaf::WeaponReady => {
            // count, the two magazines' rounds, the zoom levels.
            let (count, rounds, levels) =
                [(2, [0i16, 3], 1i16), (2, [9, 0], 1), (2, [0, 0], 0), (0, [3, 3], 2), (1, [6, 1], 0), (2, [7, -2], -1), (u32::MAX, [1, 1], 0)]
                    [case as usize % 7];
            weapon_fixture(m, c, count, rounds, levels);
        }
        Leaf::Direction => {
            let v: [[f32; 3]; 4] = [[1.0, 0.5, -0.25], [-3.0, 0.0, 2.0], [0.0, 0.0, 0.0], [0.1, -0.7, 0.3]];
            for (i, x) in v[case as usize % 4].into_iter().enumerate() {
                float(m, LEAF_VECTOR + 4 * i as u32, x);
            }
            c.gpr[1] = LEAF_VECTOR;
            c.gpr[0] = LEAF_OUT;
        }
        Leaf::InitPacket => {
            // A packet full of stale bytes, every one of which `c6ef0`
            // writes, and a vector of its own per case.
            let v: [[f32; 3]; 4] = [[0.0, 0.0, 0.0], [1.0, -0.5, 0.25], [-0.0, 3.0e8, -1.5], [f32::NAN, 0.1, -7.0]];
            for (i, x) in v[case as usize % 4].into_iter().enumerate() {
                float(m, LEAF_VECTOR + 4 * i as u32, x);
            }
            word(m, PACKET_VECTOR, LEAF_VECTOR);
            for (i, b) in m.bytes[LEAF_PACKET as usize..][..0x7c].iter_mut().enumerate() {
                *b = ((i as u8).wrapping_mul(37) ^ (case as u8).wrapping_mul(0x5b)) | 1;
            }
            c.gpr[2] = LEAF_PACKET;
        }
    }
}

#[test]
fn seat_leaves_match_lifted() {
    use sim_seats::leaf::{self, Leaf};
    let Some((image, ram)) = image() else { return };
    for l in Leaf::ALL {
        let mut code = image.original();
        code.prime(l.target());
        for case in 0..7 {
            for control in [0x027f_u16, 0x037f, 0x007f] {
                let mut mem = ram.clone();
                word(&mut mem, STACK, EXIT);
                let mut cpu = Cpu {
                    gpr: [0x1234_5678, 0x9876_5432, 0x00fe_3420, 0x7b1c, STACK, 0x00aa_bbcc, 0x5151, 0x0e37],
                    eip: l.target(),
                    eflags: 0x2,
                    mxcsr: 0x1fa0,
                    ..Default::default()
                };
                for (i, reg) in cpu.xmm.iter_mut().enumerate() {
                    *reg = (0x1234_u128 + i as u128) << 100 | 0x4000_0000;
                }
                cpu.x87.control = control;
                leaf_fixture(&mut mem, &mut cpu, l, case);
                let mut native = Guest::new();
                native.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
                native.write_bytes(0, &mem.bytes, 1).unwrap();
                let mut regs = abi_cpu(&cpu);
                // The body's view: the call has pushed its return.
                regs.esp = STACK + 4;
                let why = format!("{} case {case} control {control:#06x}", l.name());
                let effect = leaf::kernel(l, &regs, &native).unwrap_or_else(|d| panic!("{why}: declined {d:?}"));
                leaf::apply(&mut regs, &mut native, &effect).unwrap();
                lifted::code::run_until(&mut code, &mut cpu, &mut mem, 1_000_000, |c| c.eip == EXIT)
                    .unwrap_or_else(|e| panic!("{why}: {e}"));
                let want = abi_cpu(&cpu);
                let gprs = |c: &runtime::CpuAbi| [c.eax, c.ecx, c.edx, c.ebx, c.ebp, c.esi, c.edi];
                assert_eq!(gprs(&regs), gprs(&want), "{why}: eax ecx edx ebx ebp esi edi");
                assert_eq!(regs.esp, want.esp, "{why}: esp");
                assert_eq!(regs.xmm, want.xmm, "{why}: xmm");
                assert_eq!(
                    (regs.x87.st, regs.x87.tag, regs.x87.status, regs.x87.control),
                    (want.x87.st, want.x87.tag, want.x87.status, want.x87.control),
                    "{why}: x87"
                );
                let start = native.mappings()[0].physical_start as usize;
                let live = &native.physical_ram()[start..start + RAM];
                let dead = (STACK - 0x1_0000) as usize..STACK as usize;
                let first = mem.bytes.iter().zip(live).enumerate()
                    .find(|(i, (a, b))| a != b && !dead.contains(i))
                    .map(|(i, (a, b))| (i, *a, *b));
                assert_eq!(first, None, "{why}: first differing byte (original, port)");
            }
        }
    }
}

const WEAPON_OBJECT: u32 = RECORDS + 0x600;
const WEAPON_TAGS: u32 = RECORDS + 0x700;
const WEAPON_DEF: u32 = RECORDS + 0x800;

/// A weapon (handle `0x8300_0003`, tag 5) with `count` magazines, the
/// first two holding `rounds`, and `levels` zoom levels.
fn weapon_fixture(m: &mut FlatMemory, c: &mut Cpu, count: u32, rounds: [i16; 2], levels: i16) {
    word(m, 0x004e_0300, HEADER);
    word(m, HEADER + 0x44, DATA);
    word(m, points::TAG_ENTRIES.base, WEAPON_TAGS);
    word(m, DATA + 3 * 12 + 8, WEAPON_OBJECT);
    word(m, WEAPON_OBJECT, 0x00ab_0005);
    word(m, WEAPON_TAGS + 5 * 16 + 8, WEAPON_DEF);
    word(m, WEAPON_DEF + 0x2c0, count);
    for (i, r) in rounds.into_iter().enumerate() {
        m.bytes[(WEAPON_OBJECT + 0x224 + 16 * i as u32) as usize..][..2].copy_from_slice(&r.to_le_bytes());
    }
    m.bytes[(WEAPON_DEF + 0x1fe) as usize..][..2].copy_from_slice(&levels.to_le_bytes());
    c.gpr[0] = 0x8300_0003;
}

/// `100f70` with its callee `100880` in the kernel, over magazine counts
/// (none, negative, past the two set), rounds on both sides of 1..=6 and
/// zoom levels, against the lifted `100f70` whole: every register, `esp`,
/// and every byte but the dead stack.
#[test]
fn weapon_ready_matches_lifted() {
    use sim_seats::leaf::{self, Leaf};
    let Some((image, ram)) = image() else { return };
    const SITE: u32 = 0x0018_66b3;
    let mut code = image.original();
    code.prime(Leaf::WeaponReady.target());
    let rounds = [0i16, 1, 3, 6, 7, -2, i16::MIN];
    for case in 0..5 * rounds.len() * rounds.len() * 4 {
        let count = [0u32, 1, 2, 3, u32::MAX][case % 5];
        let pair = [rounds[case / 5 % 7], rounds[case / 35 % 7]];
        let levels = [1i16, 0, 2, -1][case / 245 % 4];
        let mut mem = ram.clone();
        let esp = STACK - 4;
        word(&mut mem, esp, SITE);
        let mut cpu = Cpu {
            gpr: [0, 0x9876_5432, 0x00fe_3420, 0x7b1c, esp, 0x00aa_bbcc, 0x5151, 0x0e37],
            eip: Leaf::WeaponReady.target(),
            eflags: 0x2,
            mxcsr: 0x1fa0,
            ..Default::default()
        };
        weapon_fixture(&mut mem, &mut cpu, count, pair, levels);
        let why = format!("case {case}: count {count:#x} rounds {pair:?} levels {levels}");
        let mut native = Guest::new();
        native.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
        native.write_bytes(0, &mem.bytes, 1).unwrap();
        let mut regs = abi_cpu(&cpu);
        regs.esp = STACK;
        let effect = leaf::kernel(Leaf::WeaponReady, &regs, &native).unwrap_or_else(|d| panic!("{why}: declined {d:?}"));
        leaf::apply(&mut regs, &mut native, &effect).unwrap();
        lifted::code::run_until(&mut code, &mut cpu, &mut mem, 100_000, |c| c.eip == SITE)
            .unwrap_or_else(|e| panic!("{why}: {e}"));
        let want = abi_cpu(&cpu);
        let gprs = |c: &runtime::CpuAbi| [c.eax, c.ecx, c.edx, c.ebx, c.esp, c.ebp, c.esi, c.edi];
        assert_eq!(gprs(&regs), gprs(&want), "{why}: eax ecx edx ebx esp ebp esi edi");
        assert_eq!(regs.xmm, want.xmm, "{why}: xmm");
        let start = native.mappings()[0].physical_start as usize;
        let live = &native.physical_ram()[start..start + RAM];
        let dead = (STACK - 0x1_0000) as usize..STACK as usize;
        let first = mem.bytes.iter().zip(live).enumerate()
            .find(|(i, (a, b))| a != b && !dead.contains(i))
            .map(|(i, (a, b))| (i, *a, *b));
        assert_eq!(first, None, "{why}: first differing byte (original, port)");
    }
}

/// A magazine count `100880`'s `movsx` index would wrap is declined, not
/// answered.
#[test]
fn weapon_ready_declines_a_wrapping_count() {
    use sim_seats::leaf::{self, Decline, Leaf};
    let mut mem = FlatMemory::new(0, RAM);
    let mut cpu = Cpu::default();
    weapon_fixture(&mut mem, &mut cpu, 0x8000, [0, 0], 1);
    let mut native = Guest::new();
    native.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
    native.write_bytes(0, &mem.bytes, 1).unwrap();
    let regs = abi_cpu(&cpu);
    assert_eq!(leaf::kernel(Leaf::WeaponReady, &regs, &native), Err(Decline::Fault));
}

/// `c6ef0` declines a packet over the vector it copies or over the
/// pointer to it (the guest reads both after its first stores), and a set
/// direction flag (its `rep stosd` would run backwards).
#[test]
fn init_packet_declines_an_overlap() {
    use sim_seats::leaf::{self, Decline, Leaf};
    let mut mem = FlatMemory::new(0, RAM);
    word(&mut mem, PACKET_VECTOR, LEAF_VECTOR);
    let mut native = Guest::new();
    native.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
    native.write_bytes(0, &mem.bytes, 1).unwrap();
    let mut regs = abi_cpu(&Cpu::default());
    for (packet, eflags, want) in [
        (LEAF_PACKET, 0x2, None),
        (LEAF_VECTOR - 0x78, 0x2, Some(Decline::Fault)),
        (LEAF_VECTOR + 8, 0x2, Some(Decline::Fault)),
        (PACKET_VECTOR - 0x10, 0x2, Some(Decline::Fault)),
        (LEAF_PACKET, 0x402, Some(Decline::Float)),
    ] {
        regs.edx = packet;
        regs.eflags = eflags;
        let got = leaf::kernel(Leaf::InitPacket, &regs, &native).err();
        assert_eq!(got, want, "packet {packet:#x} eflags {eflags:#x}");
    }
}

// The respawn gate `15dd40` and its callees `15db30`, `158a50` and `15b2f0`,
// and the countdown's `a7840`, `b58c0`, `a7c50`, `19eb90`, `19eb30`,
// `19de80`, `19df10` and `19e890`, through their kernel
// (`engine::sim_seats::respawn`), each against the lifted original with its
// real callees (no stubs). The world is drawn from a seeded xorshift so
// every branch of the bodies is taken: the local-user cell empty, a
// player's flags, its `+0xc0` and unit slot, the lives setting and lives,
// the user count, the machine mode, the timer (below zero, zero, the
// second ticks and the ticks between) and the ticks per second, the
// staggered check's tick delta against the slot, the player's entity and
// the network state and replication bit, the announcement rows and the
// event's subjects, a player's local user and the quiet flag. A decline
// must be one the original reaches: its stop address is where the guest
// goes on alone.

const RS_GAME: u32 = RECORDS + 0x100;
const RS_HEADER: u32 = RECORDS + 0x400;
const RS_DATA: u32 = RECORDS + 0x1000;
const RS_MODE: u32 = RECORDS + 0x4000;
const RS_TICKS: u32 = RECORDS + 0x4100;
const RS_USERS: u32 = 0x0055_e4d0;
const RS_GAME_CELL: u32 = 0x004e_9ae8;
const RS_PLAYERS_CELL: u32 = 0x004e_8c24;
const RS_MODE_CELL: u32 = 0x004e_6948;
const RS_TICKS_CELL: u32 = 0x0051_0c54;
const RS_STRIDE: u32 = 0x21c;
const RS_SETTINGS: u32 = RECORDS + 0x6000;
const RS_TAGS: u32 = RECORDS + 0x6400;
const RS_DEF: u32 = RECORDS + 0x6800;
const RS_ANN: u32 = RECORDS + 0x6c00;
const RS_EVENT: u32 = RECORDS + 0x7000;
const RS_NET: u32 = RECORDS + 0x8000;
const RS_NETDATA: u32 = RECORDS + 0x1_0000;
const RS_NETFLAGS: u32 = RECORDS + 0x2_0000;
const RS_ROWS: u32 = RECORDS + 0x2_4000;
const RS_TAGS_CELL: u32 = 0x004e_3b44;
const RS_NET_CELL: u32 = 0x004c_f77c;
/// The players' entity indices the world hands out (low ten bits 7, 0x3ff
/// and 5).
const RS_ENTITIES: [u32; 3] = [0x1_0007, 0x3ff, 5];
/// `19df10`'s announcement block offsets.
const RS_BLOCKS: [u32; 9] = [0x98, 0xa0, 0xa8, 0xb0, 0xb8, 0xc8, 0xd8, 0xe0, 0xe8];
/// Where the original goes on alone after each named decline.
const RS_ANNOUNCE_AT: u32 = 0x0019_e90d;
const RS_POST_AT: u32 = 0x0008_b4d0;
const RS_EVERY_AT: u32 = 0x0019_f240;
const RS_STOPS: [u32; 3] = [RS_ANNOUNCE_AT, RS_POST_AT, RS_EVERY_AT];

struct Xorshift(u32);

impl Xorshift {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
    fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[self.next() as usize % from.len()]
    }
}

/// A random world and the registers of a call: the slot in `eax` (with
/// garbage above its low word), garbage in the rest.
fn respawn_world(m: &mut FlatMemory, r: &mut Xorshift) -> (u32, [u32; 8]) {
    let user = r.pick(&[0u32, 1, 1, 1]);
    word(m, RS_GAME_CELL, RS_GAME);
    word(m, RS_GAME + 0xc14, 2);
    word(m, RS_USERS + 8, user * 0x1234);
    word(m, RS_GAME + 0x6c, r.pick(&[1, 1, 1, 2]));
    word(m, RS_GAME + 0x70, 1000);
    word(m, RS_GAME + 0xc04, r.pick(&[1, 0, 1]));
    word(m, RS_PLAYERS_CELL, RS_HEADER);
    word(m, RS_HEADER + 0x44, RS_DATA);
    word(m, RS_MODE_CELL, RS_MODE);
    m.bytes[(RS_MODE + 0xc) as usize] = r.pick(&[4, 0, 5, 0]);
    word(m, RS_MODE + 0x1b8, r.pick(&[0, 3, 3, 0xffff_fffe]));
    word(m, RS_TICKS_CELL, RS_TICKS);
    let slot = r.pick(&[0u32, 3, 5, 15, 33, 0x10005]);
    for i in 0..16u32 {
        let p = RS_DATA + i * RS_STRIDE;
        m.bytes[(p + 2) as usize..][..2].copy_from_slice(&r.pick(&[1u16, 1, 1, 0, 3, 0x4001, 0x4000]).to_le_bytes());
        m.bytes[(p + 0xc0) as usize] = r.pick(&[0xff, 1, 1]);
        word(m, p + 0x2c, r.pick(&[u32::MAX, u32::MAX, 0x1234]));
        m.bytes[(p + 0x1ac) as usize..][..2].copy_from_slice(&r.pick(&[0u16, 0, 2]).to_le_bytes());
        word(m, p + 0x164, r.pick(&[0u32, 0, 0, 0xffff_fff0, 7, 1, 2, 3, 30, 60, 90, 31]));
        m.bytes[(p + 0x28) as usize..][..2].copy_from_slice(&r.pick(&[0xffffu16, 0xffff, 0]).to_le_bytes());
    }
    let delta = r.pick(&[-4i32, 0, 3, 4, 5, 32 + (slot & 0x1f) as i32, 37, 64 + (slot & 0x1f) as i32]);
    word(m, RS_TICKS + 8, (1000 + delta) as u32);
    m.bytes[(RS_TICKS + 2) as usize..][..2].copy_from_slice(&r.pick(&[30u16, 30, 1, 2, 3]).to_le_bytes());
    // The entity mark: each slot's entity, the network state, the
    // replication bits and the queued words.
    for s in [0u32, 3, 5, 15, 33] {
        word(m, RS_GAME + 0x2c + 4 * s, r.pick(&[u32::MAX, RS_ENTITIES[0], RS_ENTITIES[1], RS_ENTITIES[2]]));
    }
    word(m, RS_NET_CELL, RS_NET);
    word(m, RS_NET + 8, r.pick(&[0, 3, 4, 4, 5]));
    word(m, RS_NET + 4, RS_NETDATA);
    word(m, RS_NETDATA + 0x20a4, RS_NETFLAGS);
    for e in RS_ENTITIES {
        let e = e & 0x3ff;
        m.bytes[(RS_NETFLAGS + e * 8 + 0x44) as usize] = r.pick(&[4, 0, 0xff]);
        word(m, RS_NETDATA + 0x2098 + e * 32 + 0x20, r.next());
    }
    // The announcements: the globals tag, its blocks, three rows.
    m.bytes[(RS_MODE + 0x1128) as usize] = r.pick(&[0, 1, 1]);
    word(m, SETTINGS, RS_SETTINGS);
    word(m, RS_SETTINGS + 0x16c, r.pick(&[3, 0x2_0003]));
    word(m, RS_TAGS_CELL, RS_TAGS);
    word(m, RS_TAGS + 3 * 16 + 8, RS_DEF);
    word(m, RS_DEF + 0xc, RS_ANN);
    for off in RS_BLOCKS {
        word(m, RS_ANN + off, r.pick(&[0, 1, 3, 3, u32::MAX]));
        word(m, RS_ANN + off + 4, RS_ROWS);
    }
    for i in 0..3 {
        let row = RS_ROWS + i * 0xa8;
        rs_half(m, row + 4, r.pick(&[0x14, 0x15, 0x13, 0x14]));
        rs_half(m, row + 6, r.pick(&[0, 1, 2, 3, 4, 0, 4]));
        rs_half(m, row + 0x10, r.pick(&[0, 1, 2, 3, 4, 5]));
        rs_half(m, row + 0x12, r.pick(&[0, 1, 2, 3, 4, 5]));
    }
    // An event for the callees on their own.
    let words = [
        r.pick(&[0, 0, 1, 2, 6, 10]),
        r.pick(&[0x14, 0x15, 0x13]),
        r.pick(&[slot, slot, u32::MAX]),
        r.pick(&[slot, u32::MAX, 7]),
        r.pick(&[u32::MAX, 1, 0]),
        r.pick(&[slot, u32::MAX]),
        r.pick(&[u32::MAX, 1]),
        0,
        r.next(),
    ];
    for (i, v) in words.into_iter().enumerate() {
        word(m, RS_EVENT + 4 * i as u32, v);
    }
    let regs = [r.next(), r.next(), r.next(), 0, STACK, r.next(), r.next(), r.next()];
    (slot, regs)
}

fn rs_half(m: &mut FlatMemory, a: u32, v: u16) {
    assert!(m.load(a, &v.to_le_bytes()));
}

fn respawn_guest(mem: &FlatMemory) -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
    g.write_bytes(0, &mem.bytes, 1).unwrap();
    g
}

fn at(image: &lifted::image::Image, addr: u32) -> lifted::Original {
    let mut code = image.original();
    code.prime(addr);
    assert!(code.insn(addr).is_some(), "{addr:#x}");
    code
}

/// One lifted call from `entry` with `args` above its return address, run
/// to its return or to one of [`RS_STOPS`].
fn rs_call(code: &mut lifted::Original, entry: u32, gpr: [u32; 8], args: &[u32], mem: &mut FlatMemory) -> Cpu {
    for (i, a) in args.iter().enumerate() {
        word(mem, STACK + 4 + 4 * i as u32, *a);
    }
    let mut cpu = Cpu { gpr, eip: entry, eflags: 0x2, mxcsr: 0x1fa0, ..Default::default() };
    cpu.gpr[4] = STACK;
    lifted::code::run_until(code, &mut cpu, mem, 1_000_000, |c| c.eip == EXIT || RS_STOPS.contains(&c.eip))
        .unwrap_or_else(|e| panic!("{e}"));
    cpu
}

fn rs_stop(d: engine::sim_seats::respawn::Decline) -> u32 {
    use engine::sim_seats::respawn::Decline;
    match d {
        Decline::Announce => RS_ANNOUNCE_AT,
        Decline::Post => RS_POST_AT,
        Decline::EveryPlayer => RS_EVERY_AT,
        Decline::Fault => panic!("a fault in the respawn world"),
    }
}

/// One countdown callee's port against the lifted original from the same
/// registers, stack arguments and memory: `eax`, `ecx`, `edx` as the port
/// says, the rest as they came, the arguments popped, the port's writes and
/// nothing else above the dead stack; or, for a decline, the original
/// reaching the decline's stop. Returns the outcome's name.
fn rs_check(
    why: &str,
    code: &mut lifted::Original,
    entry: u32,
    gpr: [u32; 8],
    args: &[u32],
    mem: &FlatMemory,
    got: Result<(engine::sim_seats::respawn::Regs, engine::sim_seats::respawn::Writes), engine::sim_seats::respawn::Decline>,
) -> &'static str {
    let mut want_mem = mem.clone();
    let cpu = rs_call(code, entry, gpr, args, &mut want_mem);
    let (r, writes) = match got {
        Err(d) => {
            assert_eq!(cpu.eip, rs_stop(d), "{why}: declined {d:?}; the original went elsewhere");
            return d.name();
        }
        Ok(got) => got,
    };
    assert_eq!(cpu.eip, EXIT, "{why}: the port answered; the original stopped at {:#x}", cpu.eip);
    let mut expect = gpr;
    expect[0] = r.eax;
    expect[1] = r.ecx;
    expect[2] = r.edx;
    expect[4] = STACK + 4 + 4 * args.len() as u32;
    assert_eq!(cpu.gpr, expect, "{why}: eax ecx edx ebx esp ebp esi edi");
    let mut port = mem.clone();
    for (a, v) in writes.iter() {
        word(&mut port, a, v);
    }
    let dead = (STACK - 0x1_0000) as usize..(STACK + 0x40) as usize;
    let first = first_outside(&want_mem.bytes, &port.bytes, &dead);
    assert_eq!(first, None, "{why}: first differing byte");
    if writes.is_empty() { "answered" } else { "wrote" }
}

#[test]
fn respawn_ready_and_its_callees_match_lifted() {
    use engine::sim_seats::respawn::{self, Decline, Regs};
    let Some((image, ram)) = image() else { return };
    let mut gate = at(image, points::PLAYER_RESPAWN_READY.base);
    let mut part = at(image, 0x0015_8a50);
    let mut lives = at(image, 0x0015_db30);
    let mut user = at(image, 0x0015_b2f0);
    let mut queue = at(image, 0x000b_58c0);
    let mut mark = at(image, 0x000a_7840);
    let mut post = at(image, 0x000a_7c50);
    let mut find = at(image, 0x0019_df10);
    let mut announce = at(image, 0x0019_e890);
    let mut subjects = at(image, 0x0019_de80);
    let mut walk = at(image, 0x0019_eb30);
    let mut death = at(image, 0x0019_eb90);
    let mut rng = Xorshift(0x9e37_79b9);
    let (mut answered, mut countdown) = (0, 0);
    let (mut yes, mut no) = (0, 0);
    let mut outcomes = std::collections::BTreeMap::<String, u32>::new();
    let mut tally = |what: &str, outcome: &str| *outcomes.entry(format!("{what} {outcome}")).or_default() += 1;
    for case in 0..6000 {
        let mut mem = ram.clone();
        word(&mut mem, STACK, EXIT);
        let (slot, regs) = respawn_world(&mut mem, &mut rng);
        let why = format!("case {case} slot {slot:#x}");
        let run = |code: &mut lifted::Original, entry: u32, gpr: [u32; 8], mem: &mut FlatMemory| {
            let mut cpu = Cpu { gpr, eip: entry, eflags: 0x2, mxcsr: 0x1fa0, ..Default::default() };
            lifted::code::run_until(code, &mut cpu, mem, 1_000_000, |c| c.eip == EXIT)
                .unwrap_or_else(|e| panic!("{e}"));
            cpu
        };
        let g = respawn_guest(&mem);
        let gprs = |c: &runtime::CpuAbi| [c.eax, c.ecx, c.edx, c.ebx, c.esp, c.ebp, c.esi, c.edi];
        let player_regs = |gpr: [u32; 8]| Regs { eax: gpr[0], ecx: gpr[1], edx: gpr[2] };

        // 15db30 (ecx: the player).
        let mut gpr = regs;
        gpr[1] = slot;
        let want = run(&mut lives, 0x0015_db30, gpr, &mut mem.clone());
        let (got, r) = respawn::out_of_lives(&g, player_regs(gpr)).unwrap();
        assert_eq!((r.eax, r.ecx, r.edx), (want.gpr[0], want.gpr[1], want.gpr[2]), "{why}: 15db30");
        assert_eq!(got, want.gpr[0] & 0xff != 0, "{why}: 15db30 al");

        // 158a50 (esi: the player, ebx low byte clear as the gate leaves it).
        let mut gpr = regs;
        gpr[6] = slot;
        gpr[3] = 0;
        let want = run(&mut part, 0x0015_8a50, gpr, &mut mem.clone());
        let a = respawn::takes_part(&g, slot).unwrap();
        assert_eq!((a.regs.eax, a.regs.ecx, a.regs.edx), (want.gpr[0], want.gpr[1], want.gpr[2]), "{why}: 158a50");
        assert_eq!(a.ready, want.gpr[0] & 0xff != 0, "{why}: 158a50 al");

        // 15b2f0 (no inputs).
        let want = run(&mut user, 0x0015_b2f0, regs, &mut mem.clone());
        let a = respawn::user_ready(&g, regs[0]).unwrap();
        assert_eq!((a.regs.eax, a.regs.ecx, a.regs.edx), (want.gpr[0], want.gpr[1], want.gpr[2]), "{why}: 15b2f0");
        assert_eq!(a.ready, want.gpr[0] & 0xff != 0, "{why}: 15b2f0 al");

        // The countdown's callees on their own.
        let regs3 = Regs { eax: regs[0], ecx: regs[1], edx: regs[2] };
        let (any, other) = (rng.next(), rng.next());
        let entity = rng.pick(&[RS_ENTITIES[0], RS_ENTITIES[1], RS_ENTITIES[2], any]);
        let flags = rng.pick(&[0x80, other]);
        let mut m = respawn::Mem::new(&g);
        let got = respawn::entity_queue(&mut m, regs3, entity, flags);
        let o = rs_check(&format!("{why}: b58c0"), &mut queue, 0x000b_58c0, regs, &[entity, flags], &mem, got.map(|r| (r, m.writes())));
        tally("b58c0", o);

        let mut m = respawn::Mem::new(&g);
        let got = respawn::entity_mark(&mut m, regs3, slot, 0x80);
        let o = rs_check(&format!("{why}: a7840"), &mut mark, 0x000a_7840, regs, &[slot, 0x80], &mem, got.map(|r| (r, m.writes())));
        tally("a7840", o);

        let mut gpr = regs;
        gpr[2] = RS_EVENT;
        let m = respawn::Mem::new(&g);
        let got = respawn::event_post(&m, Regs { edx: RS_EVENT, ..regs3 }, STACK);
        let o = rs_check(&format!("{why}: a7c50"), &mut post, 0x000a_7c50, gpr, &[], &mem, got.map(|r| (r, m.writes())));
        tally("a7c50", o);

        let kind = rng.pick(&[0u32, 1, 2, 3, 4]);
        let mut gpr = regs;
        gpr[0] = slot;
        gpr[7] = RS_EVENT;
        let m = respawn::Mem::new(&g);
        let got = respawn::announcement(&m, slot, RS_EVENT, kind);
        let found = got.map(|r| r.eax != 0).unwrap_or(false);
        let o = rs_check(&format!("{why}: 19df10 kind {kind}"), &mut find, 0x0019_df10, gpr, &[kind], &mem, got.map(|r| (r, m.writes())));
        tally("19df10", if found { "found" } else { o });

        let row = rng.pick(&[RS_ROWS, RS_ROWS + 0xa8]);
        let mut gpr = regs;
        gpr[0] = slot;
        gpr[1] = row;
        let m = respawn::Mem::new(&g);
        let got = respawn::announce(&m, slot, row, RS_EVENT);
        let o = rs_check(&format!("{why}: 19e890"), &mut announce, 0x0019_e890, gpr, &[RS_EVENT], &mem, got.map(|r| (r, m.writes())));
        tally("19e890", o);

        let mut gpr = regs;
        gpr[0] = RS_EVENT;
        gpr[6] = slot;
        let m = respawn::Mem::new(&g);
        let got = respawn::announce_player(&m, RS_EVENT, slot);
        let o = rs_check(&format!("{why}: 19de80"), &mut subjects, 0x0019_de80, gpr, &[], &mem, got.map(|r| (r, m.writes())));
        tally("19de80", o);

        let mut gpr = regs;
        gpr[7] = RS_EVENT;
        let m = respawn::Mem::new(&g);
        let got = respawn::announce_event(&m, RS_EVENT);
        let o = rs_check(&format!("{why}: 19eb30"), &mut walk, 0x0019_eb30, gpr, &[], &mem, got.map(|r| (r, m.writes())));
        tally("19eb30", o);

        let mut gpr = regs;
        gpr[0] = RS_EVENT;
        let m = respawn::Mem::new(&g);
        let got = respawn::death_event(&m, Regs { eax: RS_EVENT, ..regs3 }, STACK, RS_EVENT);
        let o = rs_check(&format!("{why}: 19eb90"), &mut death, 0x0019_eb90, gpr, &[], &mem, got.map(|r| (r, m.writes())));
        tally("19eb90", o);

        // 15dd40 (eax: the player), the whole body, through the arm's
        // prediction and its apply.
        let mut gpr = regs;
        gpr[0] = (regs[0] & 0xffff_0000) | (slot & 0xffff);
        let mut cpu_regs = abi_cpu(&Cpu { gpr, eip: points::PLAYER_RESPAWN_READY.base, eflags: 0x2, mxcsr: 0x1fa0, ..Default::default() });
        let timer = mem.read_u32(RS_DATA + (slot & 0xffff) * RS_STRIDE + 0x164).unwrap_or(0) as i32;
        match respawn::predict(&cpu_regs, &g) {
            Err(Decline::Fault) => panic!("{why}: declined fault"),
            Err(d) => {
                let want = rs_call(&mut gate, points::PLAYER_RESPAWN_READY.base, gpr, &[], &mut mem.clone());
                assert_eq!(want.eip, rs_stop(d), "{why}: 15dd40 declined {d:?}; the original went elsewhere");
                tally("15dd40", d.name());
            }
            Ok(prediction) => {
                answered += 1;
                let mut native = respawn_guest(&mem);
                let target = respawn::apply(&prediction, &mut cpu_regs, &mut native).unwrap();
                assert_eq!(target, EXIT, "{why}");
                let want = rs_call(&mut gate, points::PLAYER_RESPAWN_READY.base, gpr, &[], &mut mem);
                assert_eq!(want.eip, EXIT, "{why}: 15dd40 answered; the original stopped at {:#x}", want.eip);
                let want = abi_cpu(&want);
                assert_eq!(gprs(&cpu_regs), gprs(&want), "{why}: 15dd40 eax ecx edx ebx esp ebp esi edi");
                assert_eq!(prediction.differs(&want), None, "{why}");
                if want.eax & 0xff == 0 { no += 1 } else { yes += 1 }
                if timer > 0 && !prediction.writes.is_empty() {
                    countdown += 1;
                    tally("15dd40", "countdown");
                }
                // Every byte above the dead stack: the countdown's writes
                // landed by `apply`, nothing else.
                let start = native.mappings()[0].physical_start as usize;
                let live = &native.physical_ram()[start..start + RAM];
                let dead = (STACK - 0x1_0000) as usize..STACK as usize;
                // Slice compares first (memcmp); the byte walk only names
                // the first difference once one is known to exist.
                let same = mem.bytes[..dead.start] == live[..dead.start] && mem.bytes[dead.end..] == live[dead.end..];
                if !same {
                    let first = first_outside(&mem.bytes, &live, &dead);
                    assert_eq!(first, None, "{why}: first differing byte");
                }
            }
        }
    }
    drop(tally);
    let n = |k: &str| outcomes.get(k).copied().unwrap_or(0);
    assert!(answered > 500 && countdown > 30, "answered {answered}, countdown {countdown}");
    for k in [
        "b58c0 wrote", "b58c0 answered", "a7840 wrote", "a7840 answered", "a7c50 post", "a7c50 answered",
        "19df10 found", "19df10 answered", "19e890 announce", "19e890 answered", "19de80 answered",
        "19de80 announce", "19eb30 every-player", "19eb30 answered", "19eb90 answered", "19eb90 post",
    ] {
        assert!(n(k) > 5, "{k}: {}; outcomes {outcomes:?}", n(k));
    }
    assert!(n("15dd40 announce") + n("15dd40 post") > 0, "outcomes {outcomes:?}");
    assert!(yes > 5 && no > 50, "yes {yes}, no {no}");
}

// The pickup scan `1508e0` (`engine::sim_seats::player_objects`), a seat
// over the stub harness: the world is drawn from a seeded generator (a pool
// of objects of every type the query asks for, with parents, owners, link
// chains and positions, and four players with and without a unit), and the
// frame's list holds the handles the query stub "returns". Every call
// (`bb050`, `150e30`), the registers and every byte of RAM must agree.

const PO_PLAYERS: u32 = 0x0068_0000;
const PO_POOL: u32 = 0x0068_2000;
const PO_OBJECTS: u32 = 0x0068_4000;
const PO_TOPOLOGY: u32 = 0x006a_0000;
const PO_REFS: [(u32, u32, u32); 2] = [(0x006b_0000, 0x006b_0100, 0x006b_0400), (0x006b_2000, 0x006b_2100, 0x006b_2400)];
const PO_UNIT: u32 = 0x7001_0001;

fn po_handle(i: u32) -> u32 {
    0x5a00_0000 | i
}

fn pick(x: u32, from: &[u32]) -> u32 {
    from[x as usize % from.len()]
}

/// A random world; returns the player slot the scan is called with.
fn objects_world(m: &mut FlatMemory, r: &mut sim_collision::synth::Rng) -> u32 {
    use engine::sim_collision::layout;
    use sim_collision::synth::{self, Reference, References};
    m.bytes[layout::OBJECT_MARKS as usize..][..4 * layout::OBJECT_MARK_SLOTS as usize].fill(0);
    m.bytes[layout::CLUSTER_MARKS as usize..][..4 * 512].fill(0);
    word(m, 0x004e_8c24, PO_PLAYERS);
    word(m, PO_PLAYERS + 0x44, PO_PLAYERS + 0x100);
    word(m, 0x004e_0300, PO_POOL);
    word(m, PO_POOL + 0x44, PO_POOL + 0x100);
    for p in 0..4 {
        let unit = if r.chance(5) { u32::MAX } else { PO_UNIT };
        word(m, PO_PLAYERS + 0x100 + p * 0x21c + 0x2c, unit);
    }
    let n = 2 + r.below(3);
    for i in 0..16u32 {
        let entry = PO_POOL + 0x100 + i * 12;
        let object = PO_OBJECTS + i * 0x200;
        word(m, entry, (0x5a00 + i) << 16 | i);
        m.bytes[(entry + 3) as usize] = pick(r.next(), &[0, 1, 2, 3, 4, 7, 11, 7, 11]) as u8;
        word(m, entry + 8, object);
        m.bytes[(object + 0xaa) as usize] = pick(r.next(), &[2, 3, 4, 7, 11, 0, 5, 2, 3, 4]) as u8;
        let parent = if i == 1 {
            pick(r.next(), &[u32::MAX, u32::MAX, u32::MAX, u32::MAX, po_handle(5)])
        } else {
            pick(r.next(), &[u32::MAX, u32::MAX, u32::MAX, po_handle(1 + r.below(12)), po_handle(2 + r.below(10))])
        };
        word(m, object + 0x14, parent);
        word(m, object + 0x10, pick(r.next(), &[u32::MAX, po_handle(2 + r.below(10)), po_handle(2 + r.below(10))]));
        word(m, object + 0xc, pick(r.next(), &[u32::MAX, u32::MAX, po_handle(2 + r.below(10))]));
        word(m, object + 0x14c, pick(r.next(), &[PO_UNIT, 0x1234, u32::MAX, PO_UNIT]));
        let cluster = if r.chance(10) { 0xffff } else { r.below(n) as u16 };
        m.bytes[(object + 0x2c) as usize..][..2].copy_from_slice(&cluster.to_le_bytes());
        for (o, scale) in [(0x30u32, 2), (0x34, 2), (0x38, 1)] {
            float(m, object + o, r.eighths(scale));
        }
        float(m, object + 0x3c, [0.5, 1.0, 2.0, 0.0, -1.0, 3.0][r.below(6) as usize]);
    }
    let portals = synth::random_portals(r, n);
    let (_, bsp) = portals.write(PO_TOPOLOGY, |a, d| assert!(m.load(a, d)));
    word(m, layout::ACTIVE_STRUCTURE, PO_TOPOLOGY);
    word(m, layout::STRUCTURE_BSP, bsp);
    let lists = [
        (layout::COLLIDEABLE_HEADS, layout::COLLIDEABLE_ARRAY),
        (layout::OTHER_HEADS, layout::OTHER_ARRAY),
    ];
    for (k, ((heads_cell, array_cell), (heads, array, data))) in lists.into_iter().zip(PO_REFS).enumerate() {
        let mut clusters = Vec::new();
        for _ in 0..n {
            let mut list = Vec::new();
            for _ in 0..r.below(if k == 0 { 5 } else { 3 }) {
                list.push(Reference {
                    handle: po_handle(1 + r.below(12)),
                    kind: pick(r.next(), &[2, 3, 4, 7, 11, 0, 5, 2, 7, 11]) as u8,
                    bounds: [r.eighths(2), r.eighths(2), r.eighths(1), [0.5, 1.0, 2.0][r.below(3) as usize]],
                });
            }
            clusters.push(list);
        }
        References { clusters }.write(heads, array, data, |a, d| assert!(m.load(a, d)));
        word(m, heads_cell, heads);
        word(m, array_cell, array);
    }
    word(m, layout::FLOOD_EPOCH, pick(r.next(), &[9, u32::MAX, 0x1234]));
    word(m, layout::VISIT_GENERATION, pick(r.next(), &[5, u32::MAX, 0x1234]));
    m.bytes[layout::FLOOD_ACTIVE as usize] = r.below(2) as u8;
    m.bytes[layout::VISIT_ACTIVE as usize] = r.below(2) as u8;
    pick(r.next(), &[0, 1, 2, 0x3_0001])
}

/// The scan's world and its frame's list: handles 2 to 13 of the pool,
/// so the walk meets candidates, machines and crates with link chains.
fn pickup_scan_fixture(m: &mut FlatMemory, _: &mut Cpu, case: u32) {
    let mut r = sim_collision::synth::Rng(0x9e37_79b9_7f4a_7c15 ^ u64::from(case).wrapping_mul(0x1000_0001));
    let slot = objects_world(m, &mut r);
    word(m, STACK + 4, slot);
    for i in 0..16u32 {
        word(m, STACK - 0x40 + 4 * i, po_handle(2 + r.below(12)));
    }
}

/// The scan reaches its query, a candidate, and the link walk's
/// appends (a list longer than the query's count).
#[test]
fn player_pickup_scan_matches_lifted() {
    let seen = matches_guest("sim.seats.player_pickup_scan", 60, pickup_scan_fixture);
    if seen.is_empty() {
        return; // no image
    }
    use engine::sim_seats::player_objects::{CANDIDATE_DONE, QUERIED};
    assert!(seen.contains(&QUERIED), "the scan never queries");
    assert!(seen.contains(&CANDIDATE_DONE), "the scan never calls a candidate");
}

// The phantom build's input stage (`sim_seats::input`,
// docs/perf/sim-phantom-build.md): each piece against its lifted entry
// run to the return, from the same RAM and float environment. The pad
// read's `XInputGetState` is answered as the phantom executor answers it:
// the case's report, and success. Every byte of RAM outside the stack
// must agree, or the piece must refuse.
mod input_stage {
    use super::*;
    use engine::sim_seats::input::{self, REPORT, Refusal};

    const XINPUT_GET_STATE: u32 = 0x0040_9b6c;
    const CONTROLS: u32 = RECORDS;
    const OBJECT_TABLE: u32 = DATA;
    const OBJECT: u32 = RECORDS + 0x1000;

    /// The original from `entry` with `gpr` and `args`, to its return.
    fn original(mem: &mut FlatMemory, entry: u32, cpu: &mut Cpu, args: &[u32], report: &[u8; REPORT]) {
        let (image, _) = image().unwrap();
        let mut code = at(image, entry);
        word(mem, STACK, EXIT);
        for (i, a) in args.iter().enumerate() {
            word(mem, STACK + 4 + 4 * i as u32, *a);
        }
        cpu.gpr[4] = STACK;
        cpu.eip = entry;
        loop {
            lifted::code::run_until(&mut code, cpu, mem, 1_000_000, |c| c.eip == EXIT || c.eip == XINPUT_GET_STATE)
                .unwrap_or_else(|e| panic!("{entry:#x}: {e}"));
            if cpu.eip == EXIT {
                return;
            }
            let sp = cpu.gpr[4];
            let out = mem.read_u32(sp + 8).unwrap();
            assert!(mem.load(out, report));
            cpu.gpr[0] = 0;
            cpu.eip = mem.read_u32(sp).unwrap();
            cpu.gpr[4] = sp + 12;
        }
    }

    /// The first byte outside the stack where the two differ.
    fn first_difference(mem: &FlatMemory, guest: &Guest) -> Option<usize> {
        let base = guest.mappings()[0].physical_start as usize;
        let live = &guest.physical_ram()[base..base + RAM];
        let stack = (STACK - 0x1_0000) as usize..(STACK + 0x40) as usize;
        mem.bytes.iter().zip(live).enumerate()
            .find(|(i, (a, b))| a != b && !stack.contains(i))
            .map(|(i, _)| i)
    }

    /// Runs `piece` on a copy of `mem` and the original on `mem`: they
    /// agree, or the piece refused and wrote nothing. Returns the refusal.
    fn agree(
        mem: &mut FlatMemory,
        mut cpu: Cpu,
        entry: u32,
        args: &[u32],
        report: &[u8; REPORT],
        why: &str,
        piece: impl FnOnce(&mut Guest, &runtime::CpuAbi) -> Result<(), Refusal>,
    ) -> Option<Refusal> {
        let mut guest = respawn_guest(mem);
        let before = mem.clone();
        let regs = abi_cpu(&cpu);
        let outcome = piece(&mut guest, &regs);
        if let Err(refusal) = outcome {
            assert_eq!(first_difference(&before, &guest), None, "{why}: {refusal:?} wrote");
            return Some(refusal);
        }
        original(mem, entry, &mut cpu, args, report);
        if let Some(at) = first_difference(mem, &guest) {
            let live = guest.physical_ram()[guest.mappings()[0].physical_start as usize + at];
            panic!("{why}: first different byte at {at:#x}: original {:#04x}, Rust {live:#04x}", mem.bytes[at]);
        }
        None
    }

    fn report(rng: &mut Xorshift) -> [u8; REPORT] {
        let mut r = [0u8; REPORT];
        for b in &mut r {
            *b = rng.next() as u8;
        }
        // Thumbs at the dead zone's edges, the limits, an axis and noise.
        for j in 0..4 {
            let noise = rng.next() as i16;
            let v = rng.pick(&[0, 9000, 9001, -9000, -9001, i16::MAX, i16::MIN, 20_000, -20_000, noise]);
            r[0xe + 2 * j..0x10 + 2 * j].copy_from_slice(&v.to_le_bytes());
        }
        r
    }

    fn scribble(mem: &mut FlatMemory, rng: &mut Xorshift, at: u32, len: u32) {
        let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        assert!(mem.load(at, &bytes));
    }

    fn start(case: u32) -> Cpu {
        let mut cpu = Cpu { eflags: 0x2, ..Default::default() };
        cpu.x87.control = [0x007f, 0x027f, 0x037f][case as usize % 3];
        cpu
    }

    #[test]
    fn the_pad_read_matches_lifted() {
        let Some((_, ram)) = image() else { return };
        let mut rng = Xorshift(0x1240_c001);
        for case in 0..400 {
            let mut mem = ram.clone();
            for port in 0..4 {
                word(&mut mem, input::HANDLES + 4 * port, if rng.next() % 3 == 0 { 0 } else { 1 + port });
            }
            scribble(&mut mem, &mut rng, input::RAW, input::RAW_SIZE * 4);
            let noise = rng.next();
            let elapsed = rng.pick(&[33, 34, 0, 0xffff, 0x8000_0000, noise]);
            let pad = report(&mut rng);
            let why = format!("124c00 case {case}");
            let refused = agree(&mut mem, start(case), input::READ_PADS, &[elapsed], &pad, &why, |g, _| {
                input::read_pads(g, elapsed, &pad)
            });
            assert_eq!(refused, None, "{why}");
        }
    }

    #[test]
    fn the_look_rates_match_lifted() {
        let Some((_, ram)) = image() else { return };
        let mut rng = Xorshift(0x190c_34aa);
        for case in 0..200 {
            let mut mem = ram.clone();
            let user = case % 4;
            let profile = input::PROFILES + user * input::PROFILE_STRIDE;
            scribble(&mut mem, &mut rng, profile, 7);
            // Each layout and look mode, and the sensitivity's ends.
            assert!(mem.load(profile + 4, &[(case % 5) as u8, (case % 6) as u8, rng.pick(&[0, 1, 5, 10, 11, 0xff])]));
            let mut cpu = start(case);
            cpu.gpr[0] = user;
            let why = format!("190c34 case {case}");
            let refused = agree(&mut mem, cpu, input::LOOK_RATES, &[], &[0; REPORT], &why, |g, _| input::look_rates(g, user));
            assert_eq!(refused, None, "{why}");
        }
    }

    #[test]
    fn the_remap_matches_lifted() {
        let Some((_, ram)) = image() else { return };
        let mut rng = Xorshift(0x217c_4000);
        let mut modes = [0u32; 5];
        for case in 0..600 {
            let mut mem = ram.clone();
            for port in 0..4 {
                word(&mut mem, input::HANDLES + 4 * port, if port > 0 && rng.next() % 2 == 0 { 0 } else { 1 });
                let raw = input::RAW + input::RAW_SIZE * port;
                scribble(&mut mem, &mut rng, raw, input::RAW_SIZE);
                let pad = report(&mut rng);
                assert!(mem.load(raw + 0x40, &pad[0xe..REPORT]));
                let rates = input::RATES + input::RATES_SIZE * port;
                scribble(&mut mem, &mut rng, rates, input::RATES_SIZE);
                let slots: Vec<u8> = (0..16).map(|_| (rng.next() % 16) as u8).collect();
                assert!(mem.load(rates + 8, &slots));
                let mode = rng.pick(&[0u16, 1, 2, 3, 2, 3, 7]);
                modes[usize::from(mode).min(4)] += 1;
                let invert = (rng.next() % 2) as u8;
                assert!(mem.load(rates + 0x18, &[mode as u8, (mode >> 8) as u8, invert, invert & (rng.next() % 2) as u8]));
                scribble(&mut mem, &mut rng, input::LAYOUT + input::LAYOUT_SIZE * port, input::LAYOUT_SIZE);
            }
            scribble(&mut mem, &mut rng, input::LATCH, 4);
            assert!(mem.load(input::REPLAY_SELECT, &[0]));
            let why = format!("217c40 case {case}");
            let refused = agree(&mut mem, start(case), input::REMAP, &[], &[0; REPORT], &why, |g, c| input::remap(g, c));
            assert_eq!(refused, None, "{why}");
        }
        assert!(modes.iter().all(|&n| n > 50), "modes {modes:?}");
    }

    #[test]
    fn the_bind_matches_lifted() {
        let Some((_, ram)) = image() else { return };
        let mut rng = Xorshift(0x1872_d0d0);
        let (mut kept, mut bound) = (0, 0);
        for case in 0..300 {
            let mut mem = ram.clone();
            let user = case % 4;
            let unit = match case % 5 {
                0 => u32::MAX,
                _ => (rng.next() & 0xffff_0000) | (case % 7),
            };
            word(&mut mem, 0x004e_d284, CONTROLS);
            scribble(&mut mem, &mut rng, CONTROLS, 0x14 + 4 * input::CONTROL_SIZE);
            assert!(mem.load(CONTROLS, &[(case % 3 != 0) as u8]));
            let block = CONTROLS + 0x14 + user * input::CONTROL_SIZE;
            if case % 2 == 0 {
                word(&mut mem, block, unit);
            }
            word(&mut mem, HEADER + 0x44, OBJECT_TABLE);
            word(&mut mem, 0x004e_0300, HEADER);
            word(&mut mem, OBJECT_TABLE + (unit & 0xffff) * 12 + 8, OBJECT);
            scribble(&mut mem, &mut rng, OBJECT + 0x200, 0x48);
            // A unit forward vector, an axis, or none at all.
            let yaw = (rng.next() % 6283) as f32 / 1000.0 - 3.14;
            let pitch = (rng.next() % 3000) as f32 / 1000.0 - 1.5;
            let forward = match case % 7 {
                0 => [0.0, 0.0, 0.0],
                1 => [0.0, -1.0, 0.0],
                _ => [yaw.cos() * pitch.cos(), yaw.sin() * pitch.cos(), pitch.sin()],
            };
            for (i, v) in forward.iter().enumerate() {
                float(&mut mem, OBJECT + 0x150 + 4 * i as u32, *v);
            }
            let held = mem.read_u32(CONTROLS).unwrap() & 0xff != 0 && mem.read_u32(block).unwrap() == unit;
            if held { kept += 1 } else { bound += 1 }
            let mut cpu = start(case);
            (cpu.gpr[0], cpu.gpr[1]) = (user, unit);
            let why = format!("1872d0 case {case}");
            let refused = agree(&mut mem, cpu, input::BIND, &[], &[0; REPORT], &why, |g, c| input::bind(g, c, user, unit));
            assert_eq!(refused, None, "{why}");
        }
        assert!(kept > 50 && bound > 100, "kept {kept}, bound {bound}");
    }

    /// The refusals the stage names leave RAM alone; the caller then runs
    /// the guest entry.
    #[test]
    fn the_stage_refuses_by_name() {
        let Some((_, ram)) = image() else { return };
        let mut mem = ram.clone();
        word(&mut mem, input::HANDLES, 1);
        let rates = input::RATES;
        assert!(mem.load(rates + 8, &[0, 4, 2, 3, 1, 5, 6, 7, 0xc, 0xd, 0xe, 0xf, 0xa, 0xb, 0, 1]));
        let mut cpu = start(2);
        let remap = |mem: &mut FlatMemory, cpu: Cpu| {
            agree(mem, cpu, input::REMAP, &[], &[0; REPORT], "refusal", |g, c| input::remap(g, c))
        };
        cpu.mxcsr = 0x1f80 | 1 << 13;
        assert_eq!(remap(&mut mem.clone(), cpu), Some(Refusal::Mxcsr));
        cpu = start(2);
        cpu.x87.control = 0x0b7f; // round up: the x87 crate's transcendentals stop.
        assert_eq!(remap(&mut mem.clone(), cpu), Some(Refusal::X87));
        let mut helper = mem.clone();
        assert!(helper.load(rates + 0x1a, &[0, 1]));
        assert_eq!(remap(&mut helper, start(2)), Some(Refusal::LookHelper));
        let mut replay = mem.clone();
        assert!(replay.load(input::REPLAY_SELECT, &[1]));
        assert_eq!(remap(&mut replay, start(2)), Some(Refusal::ReplayRecord));
        let mut slot = mem.clone();
        assert!(slot.load(rates + 8, &[16]));
        assert_eq!(remap(&mut slot, start(2)), Some(Refusal::Slot));
        assert_eq!(Refusal::LookHelper.name(), "input.look-helper");
    }
}

/// Tick phase 29 (`engine::countdown_requests`) against the lifted
/// `0x16f200`: each request byte, set or clear, under each camera
/// override kind, and every byte of RAM after.
#[test]
fn countdown_requests_match_lifted() {
    use engine::countdown_requests::{self as requests, ENTRY, FIRST};
    let Some((image, ram)) = image() else { return };
    let mut code = at(image, ENTRY);
    const CAMERA: u32 = PHASE;
    let mut cases = 0;
    for first in [0u8, 1, 0x80] {
        for second in [0u8, 1] {
            for kind in [0u32, 1, 2, 3] {
                let why = format!("requests {first} {second}, camera kind {kind}");
                let mut mem = ram.clone();
                word(&mut mem, STACK, EXIT);
                word(&mut mem, engine::view_records::CAMERA_OVERRIDE, CAMERA);
                word(&mut mem, CAMERA + 8, kind);
                assert!(mem.load(FIRST, &[first, second]));
                for o in (0..0x40).step_by(4) {
                    word(&mut mem, engine::view_records::COUNTDOWN + o, 0x5555_0000 + o);
                }
                let mut port = engine::FlatMemory { base: 0, bytes: mem.bytes.clone() };
                requests::run(&mut port).unwrap_or_else(|e| panic!("{why}: {e:?}"));
                let mut cpu = Cpu { eip: ENTRY, eflags: 0x2, mxcsr: 0x1fa0, ..Default::default() };
                cpu.gpr[4] = STACK;
                lifted::code::run_until(&mut code, &mut cpu, &mut mem, 10_000, |c| c.eip == EXIT)
                    .unwrap_or_else(|e| panic!("{why}: {e}"));
                // The saved `esi` below the return address is dead stack.
                let dead = (STACK - 0x10) as usize..(STACK + 4) as usize;
                assert_eq!(first_outside(&mem.bytes, &port.bytes, &dead), None, "{why}: first differing byte");
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 24);
}
