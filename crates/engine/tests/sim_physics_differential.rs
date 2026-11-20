//! Execute the user's XBE to the next call/return, then compare the Rust
//! segment with it. A child sees all registers, flags and the *original*
//! return address. Seeded child answers exercise callback clobbers and
//! changes to the live set before the next bitmap read.
#![cfg(feature = "differential")]

mod common;

use engine::points;
use engine::sim_physics::{PORTS, Port};
use ir::interp::{self, Cpu, FlatMemory, Flow};
use runtime::{CpuAbi, Guest, TransferAbi, abi, guest::Permissions};
use std::collections::BTreeMap;

const RAM: usize = 8 << 20;
const STACK: u32 = 0x007f_0000;
const EXIT: u32 = 0xffff_fffc;
const PHASE: u32 = 0x0060_0000;
const LIST: u32 = 0x0060_1000;
const HEADER: u32 = 0x0060_4000;
const DATA: u32 = 0x0060_5000;
const BITS: u32 = 0x0060_6000;
const RECORDS: u32 = 0x0061_0000;

fn word(m: &mut FlatMemory, a: u32, v: u32) {
    assert!(m.load(a, &v.to_le_bytes()));
}

fn fixture(m: &mut FlatMemory, case: u32) {
    word(m, 0x004d_e2f4, PHASE);
    word(m, 0x0051_07f0, LIST);
    word(m, points::OBJECTS.base, HEADER);
    word(m, HEADER + 0x44, DATA);
    word(m, HEADER + 0x48, BITS);
    word(m, HEADER + 0x24, 12);
    word(m, LIST + 0x1d80, if case == 0 { 0 } else { 3 });
    for (i, slot) in [4, 2, 5].into_iter().enumerate() {
        word(m, LIST + 0x1d00 + i as u32 * 4, 0x8200_0000 | slot);
    }
    let high = [0, 31, 32, 33, 97, u32::MAX][case as usize % 6];
    word(m, HEADER + 0x38, high);
    for slot in 0..100 {
        word(m, DATA + slot * 12, ((slot << 4 | 7) << 16) | (0x8200 + slot));
        word(m, DATA + slot * 12 + 8, RECORDS + slot * 256);
        m.bytes[(RECORDS + slot * 256 + 0xc0) as usize] = (u8::from(slot % 3 == 1)) << 3;
    }
    word(m, BITS, if case == 2 { 0 } else { 1 << 31 | 4 });
    word(m, BITS + 4, 1);
    word(m, BITS + 8, 0);
    word(m, BITS + 12, u32::MAX);
    word(m, STACK, EXIT);
    word(m, STACK + 4, 0xdeaf_0002);
}

fn object_fixture(m: &mut FlatMemory, case: u32) {
    let component_header = DATA + 0x9a0;
    let components = RECORDS + 0x10000;
    let object = RECORDS + 0x300;
    word(m, points::GAME_GLOBALS.base, PHASE);
    m.bytes[(PHASE + 12) as usize] = 4;
    word(m, STACK + 4, 0x9100_0002);
    word(m, DATA + 2*12 + 8, object);
    word(m, DATA + 3*12 + 8, object + 0x400);
    m.bytes[(DATA + 2*12 + 2) as usize] = [0, 34, 2, 6, 66, 2][case as usize];
    m.bytes[(DATA + 2*12 + 3) as usize] = if case >= 4 { 7 } else { 1 };
    word(m, object + 0x10, if case == 3 { 0x8400_0003 } else { u32::MAX });
    word(m, object + 0x400 + 12, u32::MAX);
    word(m, object + 180, if case >= 4 { 2 } else { u32::MAX });
    m.bytes[(object + 216) as usize] = (case & 1) as u8;
    m.bytes[(object + 192) as usize] = [0, 0, 4, 2, 64, 68][case as usize];
    m.bytes[(object + 179) as usize] = if case == 5 { 0x80 } else { 0 };
    assert!(m.load(object + 276, &104u16.to_le_bytes()));
    let link: u16 = if case == 3 { 0x170 } else { u16::MAX };
    assert!(m.load(object + 298, &link.to_le_bytes()));
    word(m, object + 0x170 + 104, 0x9000);
    word(m, 0x0051_e9b8, component_header);
    word(m, component_header + 0x44, components);
    word(m, components + 2*0xa0 + 4, if case == 5 { 1 << 15 } else { 0 });
}

fn portal_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    let (bsp, portals, planes, center, geometry) = (DATA, DATA + 128, DATA + 384, DATA + 512, DATA + 640);
    word(m, bsp + 0x60, portals);
    word(m, bsp + 0x18, geometry);
    word(m, geometry + 12, planes);
    word(m, 0x004e_033c, geometry);
    let portal = portals + 36;
    word(m, portal + 4, 0);
    word(m, portal + 28, if case == 5 { 0 } else { 4 });
    word(m, portal + 32, DATA + 768);
    for i in 0..12 {
        word(m, DATA + 768 + 4*i, ((i as f32 - 4.25) * 1.7).to_bits());
    }
    let normal = if case == 4 { f32::NAN } else { -1.0 };
    word(m, planes, normal.to_bits());
    word(m, planes + 4, 0.0f32.to_bits());
    word(m, planes + 8, 0.0f32.to_bits());
    word(m, planes + 12, 0.0f32.to_bits());
    word(m, center, 1.0f32.to_bits());
    word(m, center + 4, (-0.75f32).to_bits());
    word(m, center + 8, 0.25f32.to_bits());
    word(m, portal + 16, if case == 2 { 20.0f32 } else { 0.0 }.to_bits());
    word(m, portal + 20, 1.0f32.to_bits());
    let radius: f32 = match case { 0 => -1.0, 1 => 1.0, _ => 3.75 };
    word(m, STACK + 4, radius.to_bits());
    c.gpr[0] = 0xabc0_0001;
    c.gpr[1] = center;
    c.gpr[2] = bsp;
    c.x87.control = [0x037f, 0x007f, 0x027f][case as usize % 3];
    c.mxcsr |= (case % 4) << 13;
}

fn abi_cpu(c: &Cpu) -> CpuAbi {
    let [eax, ecx, edx, ebx, esp, ebp, esi, edi] = c.gpr;
    let x = &c.x87;
    CpuAbi {
        eax, ecx, edx, ebx, esp, ebp, esi, edi,
        eip: c.eip, fs_base: c.fs_base, mxcsr: c.mxcsr,
        eflags: c.eflags, eflags_defined: c.flags_valid,
        xmm: c.xmm.map(u128::to_le_bytes),
        x87: runtime::cpu::X87Abi {
            st: x.st, control: x.control, status: x.status, tag: x.tag, opcode: x.opcode,
            instruction_pointer: x.instruction_pointer, data_pointer: x.data_pointer,
            instruction_selector: x.instruction_selector, data_selector: x.data_selector,
        },
        ..Default::default()
    }
}

fn segment(insns: &BTreeMap<u32, ir::IrInsn>, cpu: &mut Cpu, mem: &mut FlatMemory) -> TransferAbi {
    for _ in 0..500_000 {
        let pc = cpu.eip;
        let i = insns.get(&pc).unwrap_or_else(|| panic!("missing instruction at {pc:#x}"));
        let flow = interp::step(cpu, mem, i).unwrap_or_else(|e| panic!("{pc:#x}: {e:?}"));
        cpu.eip = flow.target();
        match flow {
            Flow::Return { target } => return TransferAbi::ret(target),
            Flow::Call { target, continuation } => return TransferAbi::call(target, continuation),
            _ => {}
        }
    }
    panic!("did not reach a call or return");
}

fn compare(port: &Port, insns: &BTreeMap<u32, ir::IrInsn>, mut mem: FlatMemory, mut cpu: Cpu, case: u32) {
    let mut native = Guest::new();
    native.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
    native.write_bytes(0, &mem.bytes, 1).unwrap();
    let start = native.mappings()[0].physical_start as usize;
    let mut regs = abi_cpu(&cpu);
    for n in 0..1024 {
        let at = cpu.eip;
        let expected = segment(insns, &mut cpu, &mut mem);
        let actual = (port.run)(&mut regs, &mut native, at).unwrap();
        let why = || format!("{} case {case} boundary {n} from {at:#x}", port.switch);
        assert_eq!(actual, expected, "{}", why());
        regs.eip = actual.target_pc;
        assert_eq!(regs, abi_cpu(&cpu), "{}", why());
        let ram = &native.physical_ram()[start..start + RAM];
        if mem.bytes != ram {
            let first = mem.bytes.iter().zip(ram).position(|(a,b)| a != b).unwrap();
            panic!("{} first different byte at {first:#x}", why());
        }
        if actual.kind == abi::RETURN {
            assert_eq!(actual.target_pc, EXIT);
            return;
        }
        let target = expected.target_pc;
        let cleanup = match target {
            0xbc470 | 0xbc820 | 0x109580 | 0xc49b0 | 0xc58f0 | 0xc60c0 => 4,
            0x23a160 | 0x108c60 | 0xd5de0 | 0xbe8e0 | 0xbc5e0 | 0xbd090 | 0xbdef0 => 4,
            0x1d3920 => 12,
            0xbef30 => 20,
            0xbba20 | 0xa9570 | 0x1cd4e0 | 0x1ced00 | 0x108cd0 | 0xbe1d0 => 0,
            0x120850 | 0x109660 | 0xfd910 | 0xc5eb0 | 0xc6740 | 0x114c60 | 0xc6990 | 0xc6810 | 0xc50a0 => 0,
            _ => panic!("unexpected callee {target:#x}"),
        };
        let sp = cpu.gpr[4];
        let next = mem.read_u32(sp).unwrap();
        assert_eq!(next, actual.resume_pc);
        // A callback unlinks a future slot, grows or shrinks the bitmap, or
        // detaches the listed object. Both sides must re-read those fields.
        if matches!(target, 0xbc470 | 0xbc820) {
            let slot = mem.read_u32(sp + 4).unwrap() & 0xffff;
            if n == 0 && case & 1 != 0 {
                for (a, v) in [(BITS + 4, 0b110), (HEADER + 0x38, 35)] {
                    word(&mut mem, a, v);
                    native.set_u32(a, v).unwrap();
                }
                mem.bytes[(DATA + slot * 12 + 2) as usize] = 8;
                native.set_u8(DATA + slot * 12 + 2, 8).unwrap();
            }
        }
        let answer = target.wrapping_mul(0x9127).rotate_left(n % 32) ^ case;
        cpu.gpr[0] = if target == 0x120850 { case % 3 } else { answer };
        cpu.gpr[1] = answer.rotate_left(9);
        cpu.gpr[2] = !answer;
        cpu.gpr[4] = sp + 4 + cleanup;
        cpu.eip = next;
        regs = abi_cpu(&cpu);
    }
    panic!("unbounded callback loop");
}

#[test]
fn native_simulation_matches_xbe() {
    let Some(path) = common::xbe_path() else {
        assert!(std::env::var_os("H2_XBE").is_none(), "H2_XBE does not name a readable file");
        eprintln!("SKIPPED native_simulation_matches_xbe: set H2_XBE");
        return;
    };
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(common::sha256(&bytes), common::IMAGE_SHA256);
    let xbe = xbe::Xbe::parse(&bytes).unwrap();
    let analysis = analyze::analyze(&xbe).unwrap();
    let lifter = lift::Lifter::new(&analysis, &xbe).unwrap();
    let mut image = lift::harness::load_image(&xbe).unwrap();
    image.bytes.truncate(RAM);
    for port in PORTS {
        let function = lifter.lift(port.point.base).expect(port.point.name);
        let insns = function.blocks.into_values().flat_map(|b| b.insns)
            .map(|i| (i.pc, i)).collect();
        for case in 0..6 {
            let mut m = image.clone();
            fixture(&mut m, case);
            let mut cpu = Cpu {
                gpr: [0x12345678, 0x98765432, 0xfe3420, 0x7b1c, STACK, 0xaabbcc, 0x5151, 0xe37],
                eip: port.point.base,
                eflags: 0xa93, ..Default::default()
            };
            cpu.xmm[0] = 0x1234 << 100;
            if [points::OBJECT_PRE_UPDATE.base, points::OBJECT_POST_UPDATE.base].contains(&port.point.base) {
                object_fixture(&mut m, case);
            }
            if port.point.base == points::OBJECT_SPATIAL_PORTAL.base { portal_fixture(&mut m, &mut cpu, case); }
            compare(port, &insns, m, cpu, case);
        }
    }
}
