//! The effect ports with no guest callee, run from one synthetic state
//! against the user's XBE. Each case runs the Rust port and the lifted
//! original to their return and compares the transfer, every register and
//! all memory. Skipped without the image (docs/engine/shadow-exec.md).
#![cfg(feature = "differential")]

use engine::points;
use engine::sim_effects::{Port, ports};
use ir::interp::{Cpu, FlatMemory, Flow};
use lifted::Code;
use lifted::cpu::from_interp as abi_cpu;
use runtime::{Guest, TransferAbi, guest::Permissions};

const RAM: usize = 8 << 20;
const STACK: u32 = 0x007f_0000;
const EXIT: u32 = 0xffff_fffc;
const NONE: u32 = u32::MAX;
const HEADER: u32 = 0x0060_0000;
const DATA: u32 = 0x0061_0000;
const BUCKETS: u32 = 0x0062_0000;
const TRAILS: u32 = 0x0063_0000;
const TRAIL_DATA: u32 = 0x0063_1000;
const TAGS: u32 = 0x0063_2000;
const DEFINITION: u32 = 0x0063_3000;
const CASES: u32 = 8;

fn word(m: &mut FlatMemory, a: u32, v: u32) {
    assert!(m.load(a, &v.to_le_bytes()));
}

fn decal_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    word(m, 0x004e_a950, HEADER);
    word(m, 0x004e_a94c, BUCKETS);
    word(m, HEADER + 0x44, DATA);
    word(m, HEADER + 0x20, 16);
    for bucket in 0..2048 {
        word(m, BUCKETS + 4 * bucket, NONE);
    }
    // An occupied bucket on odd cases, so the old head gets its back link.
    let (mode, cluster) = [(0u32, 0u32), (1, 5), (2, 9), (3, 0x1ff)][case as usize % 4];
    if case % 2 == 1 {
        word(m, BUCKETS + (mode * 512 + cluster) * 4, 0x8100_0001);
    }
    c.gpr[2] = mode;
    c.gpr[3] = cluster;
    c.gpr[7] = 0x8200_0002;
}

fn clock_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    word(m, 0x004e_a944, TRAILS);
    word(m, TRAILS + 0x44, TRAIL_DATA);
    word(m, points::TAG_ENTRIES.base, TAGS);
    word(m, TAGS + 3 * 16 + 8, DEFINITION);
    word(m, TRAIL_DATA + 4, 3);
    let rate = [10.0f32, 5.0, 3.0, 40.0][case as usize % 4];
    word(m, DEFINITION + 4, rate.to_bits());
    m.bytes[(DEFINITION + 2) as usize] = (case % 2) as u8;
    word(m, TRAIL_DATA + 0x14, 2.0f32.to_bits());
    word(m, TRAIL_DATA + 0x24, (0.05 * (case + 1) as f32).to_bits());
    let dt = [0.27f32, 0.0, 0.016, 0.5, -0.0, 0.1, 0.0333, 0.2][case as usize];
    word(m, STACK + 4, dt.to_bits());
    c.gpr[0] = 0;
}

type Fixture = fn(&mut FlatMemory, &mut Cpu, u32);

fn segment(code: &mut lifted::Original, cpu: &mut Cpu, mem: &mut FlatMemory) -> TransferAbi {
    for _ in 0..100_000 {
        match lifted::code::step(code, cpu, mem).unwrap_or_else(|e| panic!("{e}")) {
            Flow::Return { target } => return TransferAbi::ret(target),
            Flow::Call { target, continuation } => return TransferAbi::call(target, continuation),
            _ => {}
        }
    }
    panic!("did not reach a call or return");
}

fn matches_guest(port: &Port, prepare: Fixture) {
    let Some(image) = lifted::image::Image::get() else { return };
    let mut code = image.original();
    code.prime(port.point.base);
    for case in 0..CASES {
        let mut mem = image.ram.clone();
        mem.bytes.truncate(RAM);
        word(&mut mem, STACK, EXIT);
        let mut cpu = Cpu {
            gpr: [0x1234_5678, 0x9876_5432, 0xfe3420, 0x7b1c, STACK, 0xaabbcc, 0x5151, 0xe37],
            eip: port.point.base,
            eflags: 0xa93,
            ..Default::default()
        };
        cpu.x87.control = 0x027f;
        cpu.mxcsr = 0x1f80;
        prepare(&mut mem, &mut cpu, case);

        let mut native = Guest::new();
        native.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
        native.write_bytes(0, &mem.bytes, 1).unwrap();
        let start = native.mappings()[0].physical_start as usize;
        let mut regs = abi_cpu(&cpu);
        let name = port.point.name;
        let supported = (port.supported)(&regs, &native, port.point.base);
        let actual = supported.then(|| port.invoke(&mut regs, &mut native, port.point.base).unwrap());
        let expected = segment(&mut code, &mut cpu, &mut mem);
        // A declined call is the original's, so there is nothing to compare.
        let Some(actual) = actual else { continue };
        assert_eq!(actual, expected, "{name} case {case}");
        regs.eip = actual.target_pc;
        assert_eq!(regs, abi_cpu(&cpu), "{name} case {case}");
        let ram = &native.physical_ram()[start..start + RAM];
        if mem.bytes != ram {
            let first = mem.bytes.iter().zip(ram).position(|(a, b)| a != b).unwrap();
            panic!("{name} case {case}: first different byte at {first:#x}");
        }
    }
}

const DECAL_LINK: u32 = 0x0017_ce00;
const CONTRAIL_CLOCK: u32 = 0x0017_c040;

fn at(base: u32) -> &'static Port {
    ports().find(|p| p.point.base == base).expect("port registered")
}

#[test]
fn decal_link_bucket() {
    matches_guest(at(DECAL_LINK), decal_fixture);
}

#[test]
fn contrail_point_spawn_clock() {
    matches_guest(at(CONTRAIL_CLOCK), clock_fixture);
}
