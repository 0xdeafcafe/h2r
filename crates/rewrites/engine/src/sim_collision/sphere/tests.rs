use super::*;
use crate::sim_native::geometry_test;
use runtime::{Guest, guest::Permissions};
use sim_core::Epoch;

const BASE: u32 = 0x10_000;
const START: u32 = BASE + 0x20;
const CENTER: u32 = BASE + 0x40;
const DELTA: u32 = BASE + 0x60;
const STACK: u32 = BASE + 0x800;
const UNMAPPED: u32 = BASE + 0x2000;
static EPOCH: Epoch = Epoch::new();

fn fixture(start: [f32; 3], center: [f32; 3], delta_at: u32, delta: [f32; 3]) -> (Guest, CpuAbi) {
    let mut g = Guest::new();
    g.map(BASE, 0x1000, 0, Permissions::RW).unwrap();
    let mut cpu = CpuAbi {
        eax: CENTER,
        ecx: START,
        edx: delta_at,
        esp: STACK,
        mxcsr: 0x1fa0,
        xmm: [[0xa5; 16]; 8],
        ..CpuAbi::default()
    };
    cpu.eflags |= 0x800;
    g.set_u32(STACK, AFTER_OBJECT_SPHERE).unwrap();
    g.set_u32(STACK + 4, 1.0f32.to_bits()).unwrap();
    g.write_bytes(STACK - FRAME_BYTES, &[0x5a; FRAME_BYTES as usize], 1)
        .unwrap();
    for (addr, xyz) in [(START, start), (CENTER, center), (delta_at, delta)] {
        if addr != UNMAPPED {
            for (i, value) in xyz.into_iter().enumerate() {
                g.set_u32(addr + i as u32 * 4, value.to_bits()).unwrap();
            }
        }
    }
    (g, cpu)
}

fn bytes(g: &Guest) -> [u8; 0x1000] {
    let mut ram = [0; 0x1000];
    g.read_bytes(BASE, &mut ram, 1).unwrap();
    ram
}

#[test]
fn object_sphere_replays_the_scalar_ports_cpu_and_memory() {
    let world = CollisionWorld::new(&EPOCH);
    for (start, center, delta_at, delta, hit) in [
        ([0.0; 3], [0.0; 3], UNMAPPED, [0.0; 3], true),
        ([2.0, 0.0, 0.0], [0.0; 3], DELTA, [1.0, -0.0, 0.0], false),
        ([2.0, 1.0, 0.0], [0.0; 3], DELTA, [-3.0, 0.0, 0.0], false),
        ([2.0, 0.0, 0.0], [0.0; 3], DELTA, [-1.0, 0.0, 0.0], false),
        ([2.0, 0.0, 0.0], [0.0; 3], DELTA, [-1.5, 0.0, 0.0], true),
        ([2.0, 0.0, 0.0], [0.0; 3], STACK - 16, [-3.0, 0.0, 0.0], true),
        ([3.0, 3.0, 3.0], [1.0, 2.0, 1.0], DELTA, [-2.2, -1.2, -2.2], true),
    ] {
        // PE sticky, and the live game's MXCSR with PE clear, which the
        // port's inexact operations set.
        for mxcsr in [0x1fa0, 0x1f80] {
            let (mut guest, mut cpu) = fixture(start, center, delta_at, delta);
            let (mut oracle_guest, mut oracle_cpu) = fixture(start, center, delta_at, delta);
            cpu.mxcsr = mxcsr;
            oracle_cpu.mxcsr = mxcsr;
            let transfer = geometry_test(&mut oracle_cpu, &mut oracle_guest).unwrap();

            assert_eq!(
                object_sphere_pretest(&mut cpu, &mut guest, &world).unwrap(),
                Some(transfer)
            );
            assert_eq!(cpu.eax as u8, u8::from(hit));
            assert_eq!(cpu, oracle_cpu, "mxcsr {mxcsr:#x}");
            assert_eq!(bytes(&guest), bytes(&oracle_guest));
        }
    }
    assert_eq!(world.stats().entries[Entry::ObjectSphere as usize].native, 14);
}

#[test]
fn object_sphere_declines_before_changing_cpu_or_memory() {
    let world = CollisionWorld::new(&EPOCH);
    for case in 0..5 {
        let (mut g, mut cpu) = fixture([2.0, 0.0, 0.0], [0.0; 3], DELTA, [-2.0, 0.0, 0.0]);
        match case {
            0 => g.set_u32(STACK, BASE).unwrap(),
            // Rounding toward zero; PE clear alone runs (the live MXCSR).
            1 => cpu.mxcsr |= 0x6000,
            2 => cpu.edx = UNMAPPED,
            3 => g.set_u32(DELTA, 1).unwrap(),
            4 => {
                // The return word is mapped through an alias of the first
                // word of the spill frame.
                g.map(UNMAPPED, 12, 0, Permissions::RW).unwrap();
                g.map(UNMAPPED + 12, 8, 0, Permissions::RW).unwrap();
                cpu.esp = UNMAPPED + 12;
                g.set_u32(cpu.esp, AFTER_OBJECT_SPHERE).unwrap();
                g.set_u32(cpu.esp + 4, 1.0f32.to_bits()).unwrap();
            }
            _ => unreachable!(),
        }
        let before = cpu;
        let ram = bytes(&g);
        assert_eq!(object_sphere_pretest(&mut cpu, &mut g, &world).unwrap(), None);
        assert_eq!(cpu, before);
        assert_eq!(bytes(&g), ram);
    }
    let stats = &world.stats().entries[Entry::ObjectSphere as usize];
    for bail in [Bail::Env, Bail::Fault, Bail::Float, Bail::Alias] {
        assert_eq!(stats.bails[bail as usize], 1);
    }
}
