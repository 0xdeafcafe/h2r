//! Synthetic object-bound sounds, including callbacks that rebind a source.
use super::*;

const POOL: u32 = ARENA;
const RECORDS: u32 = ARENA + 0x1000;
const TAGS: u32 = ARENA + 0x2000;
const DEFINITION: u32 = ARENA + 0x3000;
const GLOBALS: u32 = ARENA + 0x4000;
const SCENARIO: u32 = ARENA + 0x5000;
const ENTRY: u32 = ARENA + 0x6000;
const HANDLE: u32 = 0x1357_0002;
const RECORD: u32 = RECORDS + 2 * 24;

pub(super) fn cases(entry: u32) -> Vec<Case> {
    let n = match entry {
        0x18a7b0 => 13,
        0x18cbc0 => return (0..12).map(|seed| Case { setup: descriptor, reply: describe, seed }).collect(),
        0x18d670 => 6,
        0x18d6e0 => 3,
        _ => return vec![],
    };
    (0..n).map(|seed| Case { setup, reply, seed }).collect()
}

fn setup(g: &mut Guest, c: &mut Cpu, seed: u32) {
    g.set_u32(0x4e3b44, TAGS).unwrap();
    g.set_u32(TAGS + 3 * 16 + 8, DEFINITION).unwrap();
    match c.eip {
        0x18a7b0 => {
            g.set_u32(STACK + 4, HANDLE).unwrap();
            g.set_u32(0x4ed28c, POOL).unwrap();
            g.set_u32(POOL + 0x44, RECORDS).unwrap();
            let kind = match seed { 3 | 4 => 0, 9 => 4, _ => 1 };
            let stage = if seed == 1 || seed == 2 { 3 } else { (seed & 1) as u8 };
            let flags: u16 = match seed {
                0 | 1 => 0,
                2 => 3,
                5 => 0x101,
                6 | 7 => 0x103,
                8 => 0x107,
                9 => 0x10b,
                11 | 12 => 0x90,
                _ => 1,
            };
            g.set_u8(RECORD + 2, stage).unwrap();
            g.set_u8(RECORD + 3, kind).unwrap();
            g.set_u16(RECORD + 4, flags).unwrap();
            g.set_u32(RECORD + 8, 1.25f32.to_bits()).unwrap();
            g.set_u32(RECORD + 12, 3).unwrap();
            g.set_u32(RECORD + 16, 2).unwrap();
            g.set_u32(RECORD + 20, if seed == 3 { u32::MAX } else { 4 }).unwrap();
            g.set_u32(DEFINITION, if seed % 2 == 0 { 0x10 } else { 0x20 }).unwrap();
            g.set_u32(DEFINITION + 16, 2.5f32.to_bits()).unwrap();
            let config = ARENA + 0x7000;
            g.set_u32(0x510c50, config).unwrap();
            g.set_u8(config + 5, u8::from(seed == 5)).unwrap();
            g.set_u32(0x4e0300, ARENA + 0x8000).unwrap();
            g.set_u32(ARENA + 0x8044, ARENA + 0x9000).unwrap();
            g.set_u8(ARENA + 0x9000 + 2 * 12 + 3, 1).unwrap();
        }
        0x18d670 => {
            g.set_u32(engine::points::SOUND_GLOBALS.base, GLOBALS).unwrap();
            c.gpr[6] = ENTRY;
            c.xmm[3] = u128::from(3.0f32.to_bits());
            let origin = if seed == 4 { f32::NAN } else { 0.0 };
            g.set_u32(ENTRY, origin.to_bits()).unwrap();
            for slot in 0..4 {
                let listener = GLOBALS + 0x88 + slot * 0x48;
                g.set_u8(listener + 6, u8::from(seed != 0 && (seed != 1 || slot > 0))).unwrap();
                let x: f32 = match seed { 1 => 3.0, 2 if slot == 2 => 2.0, 5 => -3.0, _ => 8.0 };
                g.set_u32(listener + 0x30, x.to_bits()).unwrap();
            }
        }
        0x18d6e0 => {
            g.set_u32(0x4e034c, SCENARIO).unwrap();
            g.set_u32(SCENARIO + 0xc0, u32::from(seed != 0)).unwrap();
            g.set_u32(SCENARIO + 0xc4, ENTRY).unwrap();
            let row = if seed == 0 { 0 } else { ENTRY };
            g.set_u32(row + 0x14, if seed == 1 { 3 } else { u32::MAX }).unwrap();
            g.set_u32(DEFINITION + 0x54, 0.125f32.to_bits()).unwrap();
        }
        _ => unreachable!(),
    }
}

fn reply(g: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0xbab40 => {
            let written = g.u32(c.gpr[4] + 12).unwrap();
            g.set_u32(written, 1.75f32.to_bits()).unwrap();
            c.gpr[0] = u32::from(seed >= 11);
            12
        }
        0x18cbc0 => {
            g.set_u8(c.gpr[1], if seed == 7 { 10 } else { 0 }).unwrap();
            0
        }
        0x18d670 => {
            c.gpr[0] = u32::from(seed != 12);
            0
        }
        0x18d6e0 => { c.xmm[0] = u128::from(0.375f32.to_bits()); 0 }
        0x21a250 => {
            c.gpr[0] = u32::from(seed == 5 || seed == 6);
            32
        }
        0x18d1c0 => { c.gpr[0] = HANDLE; 4 }
        0x16ba40 => 0,
        0xb9dd0 | 0xb9fc0 => { g.set_u32(c.gpr[6], 5.0f32.to_bits()).unwrap(); 0 }
        0x188dd0 => {
            if seed == 11 { g.set_u8(RECORD + 3, 2).unwrap(); }
            c.gpr[0] = 0xabcd;
            0
        }
        0xba1d0 | 0x219c30 => 4,
        _ => panic!("unknown object sound callback {target:#x}"),
    }
}

fn descriptor(g: &mut Guest, c: &mut Cpu, seed: u32) {
    g.set_u32(0x4ed28c, POOL).unwrap();
    g.set_u32(POOL + 0x44, RECORDS).unwrap();
    g.set_u32(0x4e3b44, TAGS).unwrap();
    g.set_u32(TAGS + 3 * 16 + 8, DEFINITION).unwrap();
    g.set_u32(DEFINITION, 8).unwrap();
    g.set_u32(RECORD + 12, 3).unwrap();
    let kind = match seed { 0 | 1 => 0, 2..=4 => 1, 5 | 6 => 2, 7..=9 => 3, 10 => 4, _ => 1 };
    g.set_u8(RECORD + 3, kind).unwrap();
    g.set_u8(RECORD + 4, match seed { 0 | 10 => 33, 1 | 2 | 4 => 1, 6 | 9 => 2, _ => 0 }).unwrap();
    g.set_u16(RECORD + 6, if seed == 2 { u16::MAX } else { 5 }).unwrap();
    g.set_u32(RECORD + 8, 0.75f32.to_bits()).unwrap();
    g.set_u32(RECORD + 16, 2).unwrap();
    g.set_u16(RECORD + 0x16, if seed == 0 { 2 } else { u16::MAX }).unwrap();
    g.set_u8(RECORD + 0x15, 4).unwrap();
    c.gpr[0] = HANDLE;
    c.gpr[1] = ENTRY;
    g.set_u8(ENTRY + 3, 0xab).unwrap();

    // The two kinds backed by effect and particle records read distinct pools.
    let other = ARENA + 0x10000;
    let items = ARENA + 0x12000;
    for root in [0x4ea93c, 0x51ebfc, 0x4e0300] { g.set_u32(root, other).unwrap(); }
    g.set_u32(other + 0x44, items).unwrap();
    let object = ARENA + 0x16000;
    g.set_u8(items + 2 * 12 + 3, if seed == 11 { 10 } else { 3 }).unwrap();
    g.set_u32(items + 2 * 12 + 8, object).unwrap();
    for (off, value) in [(0x134, 3.0f32), (0x138, 8.0), (0x13c, 2.0),
                          (0x140, 0.25), (0x144, -1.0)] {
        g.set_u32(object + off, value.to_bits()).unwrap();
    }
}

fn describe(g: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x188790 => { c.gpr[0] = 7; 0 }
        0x17b2d0 => { c.gpr[0] = u32::from(seed == 7); 8 }
        0x22a110 => 12,
        0x11bed0 => { g.set_u32(c.gpr[6], 99).unwrap(); 0 }
        0x188dd0 => { c.gpr[0] = 0x7000; 0 }
        0xbab40 => {
            let at = g.u32(c.gpr[4] + 12).unwrap();
            g.set_u32(at, 2.0f32.to_bits()).unwrap();
            12
        }
        0xb8c40 => { c.gpr[0] = 9; 0 }
        0xb8d30 => {
            for (axis, value) in [(0, 3.0f32), (4, 1.0), (8, -2.0)] {
                g.set_u32(c.gpr[3] + 0x60 + axis, value.to_bits()).unwrap();
            }
            12
        }
        0xba1d0 => {
            if seed == 4 { g.set_u8(RECORD + 3, 0).unwrap(); }
            4
        }
        0xba300 => 0,
        0x18d4f0 => {
            let mode = g.u32(c.gpr[4] + 8).unwrap();
            g.set_u8(mode, u8::from(seed == 2)).unwrap();
            12
        }
        _ => panic!("unknown source descriptor callback {target:#x}"),
    }
}
